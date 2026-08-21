//! Capture driver plugin.
//!
//! Deterministic frame stepping: a manual frame counter advances a
//! `DeterministicClock` by a fixed delta per rendered frame, and one
//! `Screenshot` request is in flight at a time (spawned in `Update`, resolved
//! via an observer a frame or two later). Each captured frame is written as:
//! - `frame_<n>.png`  : 8-bit RGB color
//! - `frame_<n>.exr`  : float RGB copy of the same color data
//! - `depth_<n>.exr`  : the REAL depth buffer (Depth32Float), read back from
//!   the render graph via `depth::` (see depth.rs and the README §5)
//! - `frame_<n>.json` : metadata sidecar (adapter, backend, driver, resolution,
//!   frame number, capture time, depth metadata)
//!
//! The PNG/color-EXR half and the depth half of a frame resolve asynchronously
//! and independently; the JSON sidecar is written only once both halves are
//! done (or the job fails).

use std::{path::PathBuf, time::Duration};

use bevy::{
    app::{App, AppExit, Plugin, Update},
    ecs::prelude::*,
    image::Image,
    log::{error, info, warn},
    render::{
        renderer::RenderAdapterInfo,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
    window::{PrimaryWindow, Window},
};

use crate::{
    depth::{self, DepthCaptureState, DepthJob, DepthReadbackChannel},
    metadata::{self, AdapterMeta, DeterminismMeta, FrameFiles, FrameMetadata},
};

/// Fixed per-frame delta of the deterministic clock (1/60 s).
pub const FIXED_DELTA: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Deterministic clock for scene animations. Advanced by exactly one
/// `FIXED_DELTA` per rendered frame; scenes must derive animation from this
/// clock instead of `Time` to stay reproducible across runs.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct DeterministicClock(Duration);

impl DeterministicClock {
    pub fn advance(&mut self) {
        self.0 += FIXED_DELTA;
    }

    pub fn elapsed(&self) -> Duration {
        self.0
    }
}

pub struct CapturePlugin {
    pub out_dir: PathBuf,
    pub scene: String,
    pub frames: u32,
    pub warmup: u32,
    pub width: u32,
    pub height: u32,
}

impl CapturePlugin {
    pub fn new(
        out_dir: PathBuf,
        scene: &str,
        frames: u32,
        warmup: u32,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            out_dir,
            scene: scene.to_string(),
            frames,
            warmup,
            width,
            height,
        }
    }
}

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CaptureState::new(self));
        app.add_systems(Update, (capture_tick, capture_exit, depth_tick));
        depth::setup(app);
    }
}

#[derive(Resource)]
pub struct CaptureState {
    pub next_frame: u32,
    pub in_flight: bool,
    pub done: bool,
    pub captured: u32,
    pub scene: String,
    pub frames: u32,
    pub warmup: u32,
    pub out_dir: PathBuf,
    pub width: u32,
    pub height: u32,
    pub adapter: AdapterMeta,
    pub clock: DeterministicClock,
    pub error: Option<String>,
    /// Per-frame depth capture state (drives the render-world readback).
    pub depth: DepthCaptureState,
    /// True when the PNG half of the current frame was saved.
    pub png_saved: bool,
    /// True when the captured PNG was uniform (see `image_uniform`).
    pub image_uniform: bool,
    /// Physical surface metadata recorded at PNG save time.
    pub surface_logical_size: (f32, f32),
    pub scale_factor: f32,
    pub pixel_format: String,
}

impl CaptureState {
    fn new(plugin: &CapturePlugin) -> Self {
        Self {
            next_frame: 0,
            in_flight: false,
            done: false,
            captured: 0,
            scene: plugin.scene.clone(),
            frames: plugin.frames,
            warmup: plugin.warmup,
            out_dir: plugin.out_dir.clone(),
            width: plugin.width,
            height: plugin.height,
            adapter: AdapterMeta::unavailable(),
            clock: DeterministicClock::default(),
            error: None,
            depth: DepthCaptureState::default(),
            png_saved: false,
            image_uniform: false,
            surface_logical_size: (plugin.width as f32, plugin.height as f32),
            scale_factor: 1.0,
            pixel_format: "unknown".into(),
        }
    }

    /// Frame number of the job currently being captured, if any.
    fn current_frame(&self) -> Option<u32> {
        self.depth.job.as_ref().map(|job| job.frame)
    }
}

/// True when every pixel of an RGB8 buffer is identical (e.g. the frame was
/// captured before render pipelines finished compiling). Cheap scan.
fn is_uniform_rgb(data: &[u8]) -> bool {
    let Some(first) = data.first() else {
        return true;
    };
    data.iter().all(|&b| b == *first)
}

/// Per-request data captured by the observer closure.
struct CaptureJob {
    frame: u32,
    scene_dir: PathBuf,
}

/// Advances the deterministic clock once per rendered frame and requests one
/// screenshot (and one depth readback) at a time during the capture window
/// `[warmup, warmup + frames)`.
fn capture_tick(mut state: ResMut<CaptureState>, mut commands: Commands) {
    if state.done || state.error.is_some() || state.in_flight {
        return;
    }
    let frame = state.next_frame;
    state.clock.advance();
    if frame >= state.warmup && frame < state.warmup + state.frames {
        let job = CaptureJob {
            frame,
            scene_dir: state.out_dir.clone(),
        };
        // The depth readback for this frame is requested alongside the
        // screenshot: both reflect the frame that is about to be rendered.
        state.depth.job = Some(DepthJob {
            frame,
            scene_dir: state.out_dir.clone(),
        });
        state.depth.png_saved = false;
        state.depth.depth_saved = false;
        state.depth.depth_file = None;
        state.depth.depth_meta = None;
        state.png_saved = false;
        commands.spawn(Screenshot::primary_window()).observe(
            move |captured: On<ScreenshotCaptured>,
                  mut state: ResMut<CaptureState>,
                  adapter: Option<Res<RenderAdapterInfo>>,
                  window: Option<Single<&Window, With<PrimaryWindow>>>| {
                if let Some(adapter) = adapter {
                    state.adapter = AdapterMeta::from_adapter_info(&adapter);
                }
                match save_color_frame(&job, &captured.image, &mut state, window) {
                    Ok(()) => {
                        state.png_saved = true;
                        state.depth.png_saved = true;
                        info!(
                            "frame {} color saved (t={:.2}s)",
                            job.frame,
                            state.clock.elapsed().as_secs_f32()
                        );
                    }
                    Err(e) => {
                        error!("frame {} save failed: {e}", job.frame);
                        state.error = Some(e);
                    }
                }
                maybe_complete_job(&mut state);
            },
        );
        state.in_flight = true;
    }
    state.next_frame += 1;
}

/// Resolves the async depth readback from the render world: writes the depth
/// EXR and completes the frame job once both halves are saved.
fn depth_tick(mut state: ResMut<CaptureState>, channel: Res<DepthReadbackChannel>) {
    if state.done || state.error.is_some() {
        return;
    }
    let rx = channel.0.lock().unwrap();
    while let Ok(result) = rx.try_recv() {
        if let Some(frame) = state.current_frame()
            && frame != result.frame
        {
            warn!(
                "depth readback for frame {} arrived, current job is frame {}",
                result.frame, frame
            );
        }
        let Some(job) = state.depth.job.clone() else {
            // Late result for an already-completed frame (e.g. a redundant
            // copy): nothing to save.
            warn!("depth readback result for frame {} arrived after its job completed", result.frame);
            continue;
        };
        if let Some(err) = result.error {
            error!("depth readback failed for frame {}: {err}", result.frame);
            state.error =
                Some(format!("depth readback failed for frame {}: {err}", result.frame));
        } else {
            match depth::save_depth_exr(
                &job.scene_dir,
                result.frame,
                result.width,
                result.height,
                &result.data,
            ) {
                Ok((file, stats)) => {
                    state.depth.depth_file = Some(file.clone());
                    state.depth.depth_meta = Some(depth::make_meta(
                        &file,
                        &stats,
                        depth::is_uniform(&result.data),
                    ));
                    info!(
                        "frame {} depth saved ({}x{}, min={:.6}, max={:.6}) -> {}",
                        result.frame,
                        result.width,
                        result.height,
                        stats.min,
                        stats.max,
                        job.scene_dir.join(&file).display()
                    );
                }
                Err(e) => {
                    error!("depth save failed for frame {}: {e}", result.frame);
                    state.error = Some(e);
                }
            }
        }
        state.depth.depth_saved = true;
        maybe_complete_job(&mut state);
    }
}

/// Writes the JSON sidecar once both halves (PNG/color EXR and depth EXR) of
/// the current frame job are saved, then releases the job.
fn maybe_complete_job(state: &mut CaptureState) {
    if state.depth.job.is_none() || !state.depth.png_saved || !state.depth.depth_saved {
        return;
    }
    let frame = state.depth.job.as_ref().unwrap().frame;
    match write_sidecar(state) {
        Ok(()) => {
            state.captured += 1;
            info!("captured frame {} (t={:.2}s) -> {}", frame, state.clock.elapsed().as_secs_f32(), state.out_dir.display());
        }
        Err(e) => {
            error!("frame {frame} sidecar write failed: {e}");
            state.error = Some(e);
        }
    }
    state.depth.job = None;
    state.in_flight = false;
}

/// Exits the app once all frames have been captured and written (waits for
/// the last in-flight job to resolve, including its depth readback), or
/// immediately when a save error has been recorded.
fn capture_exit(mut state: ResMut<CaptureState>, mut exit: MessageWriter<AppExit>) {
    if state.done
        || (state.error.is_none()
            && (state.next_frame < state.warmup + state.frames
                || state.in_flight
                || state.depth.job.is_some()))
    {
        return;
    }
    state.done = true;
    if let Some(e) = &state.error {
        error!("capture failed: {e}");
        exit.write(AppExit::Error(std::num::NonZero::new(1).unwrap()));
    } else {
        info!(
            "capture complete: {} frames written to {}",
            state.captured,
            state.out_dir.display()
        );
        exit.write(AppExit::Success);
    }
}

fn save_color_frame(
    job: &CaptureJob,
    img: &Image,
    state: &mut CaptureState,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
) -> Result<(), String> {
    let png_path = job.scene_dir.join(format!("frame_{:04}.png", job.frame));
    let exr_path = job.scene_dir.join(format!("frame_{:04}.exr", job.frame));

    let dynamic = img
        .clone()
        .try_into_dynamic()
        .map_err(|e| format!("image conversion failed: {e}"))?;
    // Discard alpha like Bevy's own `save_to_disk` (alpha stores brightness
    // values when HDR is enabled).
    let rgb8 = dynamic.to_rgb8();
    state.image_uniform = is_uniform_rgb(rgb8.as_raw());
    state.pixel_format = format!("{:?}", img.texture_descriptor.format);
    match window {
        Some(w) => {
            state.surface_logical_size = (w.width(), w.height());
            state.scale_factor = w.scale_factor();
        }
        None => {
            state.surface_logical_size = (state.width as f32, state.height as f32);
            state.scale_factor = 1.0;
        }
    }
    rgb8.save_with_format(&png_path, image::ImageFormat::Png)
        .map_err(|e| format!("png save failed: {e}"))?;

    let rgb32f = image::DynamicImage::ImageRgb8(rgb8).to_rgb32f();
    rgb32f
        .save_with_format(&exr_path, image::ImageFormat::OpenExr)
        .map_err(|e| format!("exr save failed: {e}"))?;
    Ok(())
}

/// Writes the JSON sidecar for the current frame job.
fn write_sidecar(state: &CaptureState) -> Result<(), String> {
    let frame = state.depth.job.as_ref().map(|j| j.frame);
    let Some(frame) = frame else {
        return Err("write_sidecar called with no active frame job".into());
    };
    let json_path = state.out_dir.join(format!("frame_{frame:04}.json"));
    let captured_at = metadata::now_unix_secs();
    let meta = FrameMetadata {
        tool: "reference_frames".into(),
        tool_version: crate::TOOL_VERSION.into(),
        bevy_version: crate::BEVY_VERSION.into(),
        wgpu_version: crate::WGPU_VERSION.into(),
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        scene: state.scene.clone(),
        frame,
        width: state.width,
        height: state.height,
        surface_logical_size: state.surface_logical_size,
        scale_factor: state.scale_factor,
        pixel_format: state.pixel_format.clone(),
        captured_at_unix_secs: captured_at,
        captured_at_utc: metadata::unix_to_rfc3339_utc(captured_at),
        capture_source: "Screenshot::primary_window (bevy_render::view::screenshot); depth: Core3d graph node + MAP_READ buffer".into(),
        image_uniform: state.image_uniform,
        files: FrameFiles {
            png: format!("frame_{frame:04}.png"),
            exr: format!("frame_{frame:04}.exr"),
            json: format!("frame_{frame:04}.json"),
            depth: state.depth.depth_file.clone(),
        },
        depth: state.depth.depth_meta.clone(),
        adapter: state.adapter.clone(),
        determinism: DeterminismMeta {
            camera: "fixed per scene (registry camera rig)".into(),
            time_source: "manual frame counter + DeterministicClock (fixed 1/60 delta per frame)".into(),
            animation_seed: "none for static scenes; DefaultHasher over fixed coords where used".into(),
        },
    };
    let json = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("json serialize failed: {e}"))?;
    std::fs::write(&json_path, json).map_err(|e| format!("json write failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(error: Option<String>, next_frame: u32) -> CaptureState {
        CaptureState {
            next_frame,
            in_flight: false,
            done: false,
            captured: 0,
            scene: "test".into(),
            frames: 30,
            warmup: 200,
            out_dir: PathBuf::from("unused"),
            width: 1280,
            height: 720,
            adapter: AdapterMeta::unavailable(),
            clock: DeterministicClock::default(),
            error,
            depth: DepthCaptureState::default(),
            png_saved: false,
            image_uniform: false,
            surface_logical_size: (1280.0, 720.0),
            scale_factor: 1.0,
            pixel_format: "unknown".into(),
        }
    }

    #[test]
    fn recorded_error_exits_promptly_with_error_code() {
        // Simulate a mid-run save failure: `error` is recorded while
        // `next_frame` is still below the completion threshold
        // (warmup + frames), which used to hang the app forever.
        let mut app = App::new();
        app.insert_resource(state(Some("png save failed: disk full".into()), 7));
        app.add_systems(Update, capture_exit);
        let exit = app.run();
        assert!(
            exit.is_error(),
            "expected AppExit::Error, got {exit:?} while a save error was recorded"
        );
        assert_eq!(exit, AppExit::Error(std::num::NonZero::new(1).unwrap()));
    }

    #[test]
    fn job_completes_only_after_both_halves_saved() {
        let mut state = state(None, 0);
        state.depth.job = Some(DepthJob {
            frame: 7,
            scene_dir: PathBuf::from("unused"),
        });
        state.in_flight = true;

        // Only the color half saved: job must stay open.
        state.depth.png_saved = true;
        state.png_saved = true;
        maybe_complete_job(&mut state);
        assert!(state.depth.job.is_some(), "job completed with depth pending");
        assert!(state.in_flight, "in_flight cleared with depth pending");

        // Depth half saved: job completes and releases in_flight.
        state.depth.depth_saved = true;
        maybe_complete_job(&mut state);
        assert!(state.depth.job.is_none(), "job did not complete");
        assert!(!state.in_flight, "in_flight not released");
    }

    #[test]
    fn exit_waits_for_pending_depth_job() {
        let mut app = App::new();
        let mut state = state(None, 230); // capture window fully advanced
        state.in_flight = false;
        state.depth.job = Some(DepthJob {
            frame: 229,
            scene_dir: PathBuf::from("unused"),
        });
        app.insert_resource(state);
        app.add_systems(Update, capture_exit);
        let exit = app.run();
        assert!(exit.is_success(), "exited while a depth job was pending: {exit:?}");
    }
}
