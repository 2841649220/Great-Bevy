//! Real depth-buffer readback and EXR export.
//!
//! Architecture (all tool-side, no fork source changes):
//!
//! 1. The capture camera is forced to `Msaa::Off` and `Camera3d.depth_texture_usages`
//!    gains `COPY_SRC` (public per-camera API). This makes the main 3D pass depth
//!    texture (`ViewDepthTexture`, `Depth32Float`, single-sample) copyable.
//! 2. A render-graph node system is appended to the `Core3d` schedule after the
//!    `MainPass` set. When a depth capture is requested for the current frame it
//!    copies the view's depth texture straight into a `MAP_READ | COPY_DST` buffer.
//!    wgpu 29 allows `Depth32Float` as a texel-copy *source* (only `Depth24Plus`
//!    is forbidden), so no format conversion pass is needed.
//! 3. A `Render`-schedule system (Cleanup set, after `render_system`) maps the
//!    buffer asynchronously (same pattern as `bevy_render::gpu_readback::map_buffers`)
//!    and ships the de-padded `f32` pixels to the main world over an mpsc channel.
//! 4. The main world writes `depth_<n>.exr` — an Rgb32F EXR with R=G=B=depth
//!    (the `image` crate's EXR encoder only supports Rgb32F/Rgba32F output), and
//!    records depth metadata in the JSON sidecar.
//!
//! Depth value convention (documented in tests/reference-frames/README.md):
//! Bevy 0.19's default `PerspectiveProjection` uses an **infinite reverse-Z**
//! mapping (`Mat4::perspective_infinite_reverse_rh`): depth 1.0 = near plane
//! (0.1), depth 0.0 = far plane / background (the depth texture clear value is
//! `Camera3dDepthLoadOp::Clear(0.0)`). Raw values are the linear
//! `near / view_distance` ratio for reverse-Z.

use std::{
    path::Path,
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
};

use bevy::{
    app::{App, PostStartup},
    camera::Camera3d,
    core_pipeline::{Core3d, Core3dSystems},
    ecs::prelude::*,
    log::{info, warn},
    prelude::{Deref, DerefMut},
    render::{
        render_resource::{
            Buffer, BufferDescriptor, BufferUsages, MapMode, TexelCopyBufferInfo,
            TexelCopyBufferLayout, TextureUsages,
        },
        renderer::{RenderContext, ViewQuery},
        view::{Msaa, ViewDepthTexture},
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
    },
};

/// Row alignment required by wgpu for texel-buffer copies.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Rounds `bytes` up to a multiple of the copy row alignment.
fn align_byte_size(bytes: u32) -> u32 {
    (bytes + COPY_BYTES_PER_ROW_ALIGNMENT - 1) & !(COPY_BYTES_PER_ROW_ALIGNMENT - 1)
}

/// A depth readback requested by the main world for one frame.
#[derive(Clone, Debug)]
pub struct DepthJob {
    pub frame: u32,
    pub scene_dir: std::path::PathBuf,
}

/// Main-world capture state for the depth half of a frame job. Embedded in
/// [`CaptureState`](crate::capture::CaptureState) so the render world's
/// extraction reads the same instance the main world mutates.
#[derive(Default)]
pub struct DepthCaptureState {
    /// The frame currently being captured, if any.
    pub job: Option<DepthJob>,
    /// Whether the PNG/color-EXR half of the job has been saved.
    pub png_saved: bool,
    /// Whether the depth EXR half of the job has been saved.
    pub depth_saved: bool,
    /// Depth EXR file name once saved.
    pub depth_file: Option<String>,
    /// Depth metadata for the JSON sidecar once saved.
    pub depth_meta: Option<DepthMeta>,
}

/// Render-world copy request, extracted from `DepthCaptureState` every frame.
#[derive(Resource, Default)]
pub struct DepthCopyRequest {
    pub active: bool,
    pub frame: u32,
}

/// A buffer holding one frame's depth data, pending map+read.
pub struct DepthCopyBuffer {
    pub buffer: Buffer,
    pub width: u32,
    pub height: u32,
    pub frame: u32,
}

/// Render-world state for the in-flight depth buffer.
#[derive(Resource, Default)]
pub struct DepthCopyState {
    pub pending: Option<DepthCopyBuffer>,
    /// Frame whose depth has already been copied this job; the readback spans
    /// several render frames (map callback is async), so the node must not
    /// issue a second copy while the request is still active.
    pub last_copied_frame: Option<u32>,
}

/// Result of a depth readback, sent from the render world to the main world.
pub struct DepthReadbackResult {
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    /// Row-aligned raw f32 pixels (height rows, width columns).
    pub data: Vec<f32>,
    pub error: Option<String>,
}

/// Render-world sender for readback results.
#[derive(Resource)]
pub struct DepthReadbackSender(pub Sender<DepthReadbackResult>);

/// Main-world receiver for readback results.
#[derive(Resource, Deref, DerefMut)]
pub struct DepthReadbackChannel(pub Arc<Mutex<Receiver<DepthReadbackResult>>>);

/// Depth metadata recorded in the JSON sidecar (see README §5).
#[derive(serde::Serialize, Clone, Debug)]
pub struct DepthMeta {
    pub file: String,
    pub format: String,
    pub value_range: String,
    pub projection: String,
    pub source_pass: String,
    pub msaa: String,
    pub clear_value: f32,
    /// True when every pixel has the identical depth (e.g. a frame captured
    /// before pipelines finished compiling).
    pub uniform: bool,
    pub stats: DepthStats,
}

/// Summary statistics over the raw depth values.
#[derive(serde::Serialize, Clone, Copy, Debug)]
pub struct DepthStats {
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    pub non_finite: usize,
}

/// Sets up the depth-capture pipeline in both worlds.
///
/// Call from the capture plugin's `build`. Uses only public Bevy APIs:
/// - forces `Msaa::Off` + `COPY_SRC` depth usages on every 3D camera
/// - appends the copy node to the per-camera `Core3d` render graph schedule
/// - maps readback buffers in the render world and ships results to the main world
pub fn setup(app: &mut App) {
    let (tx, rx) = std::sync::mpsc::channel();
    app.insert_resource(DepthReadbackChannel(Arc::new(Mutex::new(rx))))
        // Runs after the scene's Startup systems have spawned the camera (Startup
        // system order is not guaranteed), so the first extraction sees COPY_SRC.
        .add_systems(PostStartup, force_depth_capture_camera_settings);

    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(DepthReadbackSender(tx))
        .init_resource::<DepthCopyRequest>()
        .init_resource::<DepthCopyState>()
        .add_systems(ExtractSchedule, extract_depth_request)
        .add_systems(Core3d, copy_depth_node.after(Core3dSystems::MainPass))
        .add_systems(
            Render,
            map_depth_buffer
                .in_set(RenderSystems::Cleanup)
                .ambiguous_with_all(),
        );
}

/// Makes the capture cameras produce a single-sample, copyable depth texture:
/// `Msaa::Off` (multisampled textures cannot be texel-copied) and `COPY_SRC` on
/// the camera's depth texture usages (the main-pass depth texture is otherwise
/// render-attachment-only).
fn force_depth_capture_camera_settings(mut cameras: Query<(&mut Camera3d, &mut Msaa)>) {
    let mut count = 0;
    for (mut camera_3d, mut msaa) in &mut cameras {
        let mut usage = TextureUsages::from(camera_3d.depth_texture_usages);
        usage |= TextureUsages::COPY_SRC;
        camera_3d.depth_texture_usages = usage.into();
        if *msaa != Msaa::Off {
            info!("depth capture: forcing Msaa::Off on 3D camera (single-sample depth copy requirement)");
            *msaa = Msaa::Off;
        }
        count += 1;
    }
    if count == 0 {
        warn!("depth capture: no Camera3d found at PostStartup; depth EXR will be unavailable");
    }
}

/// Copies the main-world depth request into the render world each frame.
///
/// Reads [`CaptureState`] (which embeds the depth job) directly so the render
/// world observes the same instance the main world mutates.
fn extract_depth_request(
    mut dst: ResMut<DepthCopyRequest>,
    src: Extract<Option<Res<crate::capture::CaptureState>>>,
) {
    dst.active = false;
    if let Some(state) = src.as_ref()
        && let Some(job) = state.depth.job.as_ref()
    {
        dst.active = true;
        dst.frame = job.frame;
    }
}

/// Render-graph node: copies the view's real depth texture into a `MAP_READ`
/// buffer when a capture is requested for this frame. Runs after the main 3D
/// pass(es) so the depth buffer is fully written.
fn copy_depth_node(
    depth: ViewQuery<&ViewDepthTexture>,
    mut ctx: RenderContext,
    request: Res<DepthCopyRequest>,
    mut state: ResMut<DepthCopyState>,
) {
    if !request.active
        || state.pending.is_some()
        || state.last_copied_frame == Some(request.frame)
    {
        return;
    }
    let depth_texture = depth.into_inner();
    let size = depth_texture.texture.size();
    if size.width == 0 || size.height == 0 {
        warn!("depth capture: view depth texture has zero size, skipping copy");
        return;
    }

    let buffer = ctx.render_device().create_buffer(&BufferDescriptor {
        label: Some("reference_frames_depth_readback"),
        size: (align_byte_size(size.width * 4) as u64) * size.height as u64,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let layout = TexelCopyBufferLayout {
        bytes_per_row: Some(align_byte_size(size.width * 4)),
        rows_per_image: None,
        offset: 0,
    };
    ctx.command_encoder().copy_texture_to_buffer(
        depth_texture.texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout,
        },
        size,
    );
    state.pending = Some(DepthCopyBuffer {
        buffer,
        width: size.width,
        height: size.height,
        frame: request.frame,
    });
}

/// Maps the depth buffer after the render graph has submitted, then sends the
/// de-padded pixel data to the main world. Same asynchronous pattern as
/// `bevy_render::gpu_readback::map_buffers`.
fn map_depth_buffer(
    mut state: ResMut<DepthCopyState>,
    sender: Res<DepthReadbackSender>,
) {
    let Some(copy) = state.pending.take() else {
        return;
    };
    state.last_copied_frame = Some(copy.frame);
    let frame = copy.frame;
    let width = copy.width;
    let height = copy.height;
    let buffer = copy.buffer;
    // Keep a clone alive for the map callback; `slice` borrows the original.
    let slice = buffer.slice(..);
    let map_buffer = buffer.clone();
    let tx = sender.0.clone();

    slice.map_async(MapMode::Read, move |result| {
        let buffer_slice = map_buffer.slice(..);
        if let Err(err) = result {
            let _ = tx.send(DepthReadbackResult {
                frame,
                width,
                height,
                data: Vec::new(),
                error: Some(err.to_string()),
            });
            return;
        }
        let data = buffer_slice.get_mapped_range();
        let raw = Vec::from(&*data);
        drop(data);
        map_buffer.unmap();
        let pixels = dealign_rows(&raw, width, height);
        let _ = tx.send(DepthReadbackResult {
            frame,
            width,
            height,
            data: pixels,
            error: None,
        });
    });
}

/// Strips per-row padding from a texel-buffer copy and reinterprets the bytes
/// as `f32` depth values (Depth32Float texels are host-endian IEEE floats).
fn dealign_rows(raw: &[u8], width: u32, height: u32) -> Vec<f32> {
    let initial_row_bytes = width as usize * 4;
    let buffered_row_bytes = align_byte_size(width * 4) as usize;
    let mut out = Vec::with_capacity(initial_row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * buffered_row_bytes;
        for chunk in raw[start..start + initial_row_bytes].chunks_exact(4) {
            out.push(f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
    }
    out
}

/// Computes summary statistics over raw depth values.
pub fn compute_stats(data: &[f32]) -> DepthStats {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sum = 0.0f64;
    let mut non_finite = 0;
    for &v in data {
        if v.is_finite() {
            min = min.min(v);
            max = max.max(v);
            sum += v as f64;
        } else {
            non_finite += 1;
        }
    }
    let mean = if data.is_empty() { 0.0 } else { (sum / data.len() as f64) as f32 };
    DepthStats { min, max, mean, non_finite }
}

/// True when every value is identical (capture not ready / uniform content).
pub fn is_uniform(data: &[f32]) -> bool {
    data.first().is_none_or(|first| data.iter().all(|v| v == first))
}

/// Writes the depth EXR for a frame: Rgb32F with R=G=B=depth (the `image`
/// crate's EXR encoder only emits Rgb32F/Rgba32F).
pub fn save_depth_exr(
    scene_dir: &Path,
    frame: u32,
    width: u32,
    height: u32,
    data: &[f32],
) -> Result<(String, DepthStats), String> {
    let file = format!("depth_{frame:04}.exr");
    let path = scene_dir.join(&file);
    let expected = (width as usize) * (height as usize);
    if data.len() != expected {
        return Err(format!(
            "depth data length mismatch: got {} pixels, expected {expected} ({width}x{height})",
            data.len()
        ));
    }
    let image = image::ImageBuffer::from_fn(width, height, |x, y| {
        let d = data[(y as usize) * width as usize + x as usize];
        image::Rgb([d, d, d])
    });
    image::DynamicImage::ImageRgb32F(image)
        .save_with_format(&path, image::ImageFormat::OpenExr)
        .map_err(|e| format!("depth exr save failed: {e}"))?;
    let stats = compute_stats(data);
    Ok((file, stats))
}

/// Depth metadata for the JSON sidecar.
pub fn make_meta(file: &str, stats: &DepthStats, uniform: bool) -> DepthMeta {
    DepthMeta {
        file: file.to_string(),
        format: "Depth32Float, exported as Rgb32F EXR (R=G=B=depth)".into(),
        value_range: "[0, 1]: 1.0 = near plane, 0.0 = far (background/sky)".into(),
        projection: "perspective_infinite_reverse_rh (near=0.1, far=inf)".into(),
        source_pass: "ViewDepthTexture (main 3d pass depth attachment) copied after main passes".into(),
        msaa: "Off (single-sample depth texture required for texel copy)".into(),
        clear_value: 0.0,
        uniform,
        stats: *stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dealign_rows_strips_padding_and_reinterprets() {
        let width = 3u32;
        let height = 2u32;
        let aligned = align_byte_size(width * 4) as usize;
        let mut raw = vec![0u8; aligned * height as usize];
        let values = [1.0f32, -0.5, 2.25, 0.0, 0.125, 1.0];
        for (i, v) in values.iter().enumerate() {
            let row = i / width as usize;
            let col = i % width as usize;
            let bytes = v.to_ne_bytes();
            let start = row * aligned + col * 4;
            raw[start..start + 4].copy_from_slice(&bytes);
        }
        let out = dealign_rows(&raw, width, height);
        assert_eq!(out, values);
    }

    #[test]
    fn dealign_rows_identity_when_aligned() {
        let raw = [0.25f32, 0.5, 0.75].iter().flat_map(|v| v.to_ne_bytes()).collect::<Vec<_>>();
        let out = dealign_rows(&raw, 3, 1);
        assert_eq!(out, vec![0.25, 0.5, 0.75]);
    }

    #[test]
    fn stats_cover_range_and_mean() {
        let data = [0.0f32, 0.5, 1.0, 0.25];
        let stats = compute_stats(&data);
        assert_eq!(stats.min, 0.0);
        assert_eq!(stats.max, 1.0);
        assert_eq!(stats.mean, 0.4375);
        assert_eq!(stats.non_finite, 0);
    }

    #[test]
    fn stats_skip_non_finite() {
        let data = [f32::NAN, 1.0, f32::INFINITY, 0.5];
        let stats = compute_stats(&data);
        assert_eq!(stats.min, 0.5);
        assert_eq!(stats.max, 1.0);
        assert_eq!(stats.non_finite, 2);
    }

    #[test]
    fn uniform_detection() {
        assert!(is_uniform(&[0.5, 0.5, 0.5]));
        assert!(!is_uniform(&[0.5, 0.5, 0.6]));
        assert!(is_uniform(&[]));
    }

    #[test]
    fn meta_records_convention() {
        let stats = compute_stats(&[0.0, 1.0]);
        let meta = make_meta("depth_0000.exr", &stats, false);
        assert!(meta.value_range.contains("1.0 = near"));
        assert!(meta.value_range.contains("0.0 = far"));
        assert!(meta.projection.contains("reverse"));
        assert!(!meta.uniform);
    }
}
