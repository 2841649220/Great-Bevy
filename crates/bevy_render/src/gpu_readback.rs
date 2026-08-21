use crate::{
    extract_component::ExtractComponentPlugin,
    render_asset::RenderAssets,
    render_resource::{Buffer, BufferUsages, Texture, TextureFormat},
    renderer::{
        diligent_registry::DiligentHandle,
        RenderDevice,
    },
    storage::{GpuShaderBuffer, ShaderBuffer},
    sync_world::MainEntity,
    texture::GpuImage,
    ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems,
};
use async_channel::{Receiver, Sender};
use bevy_app::{App, Plugin};
use bevy_asset::Handle;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    change_detection::ResMut,
    entity::Entity,
    event::EntityEvent,
    prelude::{Component, Resource, World},
    system::{Query, Res},
};
use bevy_ecs::{schedule::IntoScheduleConfigs, template::FromTemplate};
use bevy_image::{Image, TextureFormatPixelInfo};
use bevy_log::{debug, warn};
use bevy_platform::collections::HashMap;
use bevy_reflect::Reflect;
use bevy_render_macros::ExtractComponent;
use encase::internal::ReadFrom;
use encase::private::Reader;
use encase::ShaderType;

/// A plugin that enables reading back gpu buffers and textures to the cpu.
pub struct GpuReadbackPlugin {
    /// Describes the number of frames a buffer can be unused before it is removed from the pool in
    /// order to avoid unnecessary reallocations.
    max_unused_frames: usize,
}

impl Default for GpuReadbackPlugin {
    fn default() -> Self {
        Self {
            max_unused_frames: 10,
        }
    }
}

impl Plugin for GpuReadbackPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<Readback>::default());

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<GpuReadbackBufferPool>()
                .init_resource::<GpuReadbacks>()
                .insert_resource(GpuReadbackMaxUnusedFrames(self.max_unused_frames))
                .add_systems(ExtractSchedule, sync_readbacks.ambiguous_with_all())
                .add_systems(
                    Render,
                    (
                        prepare_buffers.in_set(RenderSystems::PrepareResources),
                        // TODO: this should be in the graph somehow
                        map_buffers.in_set(RenderSystems::Cleanup),
                    ),
                );
        }
    }
}

/// A component that registers the wrapped handle for gpu readback, either a texture or a buffer.
///
/// Data is read asynchronously and will be triggered on the entity via the [`ReadbackComplete`] event
/// when complete. If this component is not removed, the readback will be attempted every frame
#[derive(Component, ExtractComponent, Clone, Debug, FromTemplate)]
pub enum Readback {
    #[default]
    Texture(Handle<Image>),
    Buffer {
        buffer: Handle<ShaderBuffer>,
        start_offset_and_size: Option<(u64, u64)>,
    },
}

impl Readback {
    /// Create a readback component for a texture using the given handle.
    pub fn texture(image: Handle<Image>) -> Self {
        Self::Texture(image)
    }

    /// Create a readback component for a full buffer using the given handle.
    pub fn buffer(buffer: Handle<ShaderBuffer>) -> Self {
        Self::Buffer {
            buffer,
            start_offset_and_size: None,
        }
    }

    /// Create a readback component for a buffer range using the given handle, a start offset in bytes
    /// and a number of bytes to read.
    pub fn buffer_range(buffer: Handle<ShaderBuffer>, start_offset: u64, size: u64) -> Self {
        Self::Buffer {
            buffer,
            start_offset_and_size: Some((start_offset, size)),
        }
    }
}

/// An event that is triggered when a gpu readback is complete.
///
/// The event contains the data as a `Vec<u8>`, which can be interpreted as the raw bytes of the
/// requested buffer or texture.
#[derive(EntityEvent, Deref, DerefMut, Reflect, Debug)]
#[reflect(Debug)]
pub struct ReadbackComplete {
    pub entity: Entity,
    #[deref]
    pub data: Vec<u8>,
}

impl ReadbackComplete {
    /// Convert the raw bytes of the event to a shader type.
    pub fn to_shader_type<T: ShaderType + ReadFrom + Default>(&self) -> T {
        let mut val = T::default();
        let mut reader = Reader::new::<T>(&self.data, 0).expect("Failed to create Reader");
        T::read_from(&mut val, &mut reader);
        val
    }
}

#[derive(Resource)]
struct GpuReadbackMaxUnusedFrames(usize);

/// The staging slot of a readback: a bevy `Buffer` (a dual-object buffer -
/// its Diligent side is created `USAGE_STAGING` for `MAP_READ` usage, see
/// `RenderDevice::create_diligent_buffer`; the wgpu side stays as the
/// transition carrier) or a diligent-only staging texture (M1-4b-1: the
/// Diligent copy API has no texture-to-buffer direction, so texture
/// readbacks stage into a texture of the same format and size).
#[derive(Clone)]
enum ReadbackStaging {
    Buffer(Buffer),
    Texture(DiligentHandle<diligent_rs::Texture>),
}

struct GpuReadbackBuffer {
    buffer: Buffer,
    taken: bool,
    frames_unused: usize,
}

struct GpuReadbackTexture {
    texture: DiligentHandle<diligent_rs::Texture>,
    taken: bool,
    frames_unused: usize,
}

#[derive(Resource, Default)]
struct GpuReadbackBufferPool {
    // Map of buffer size to list of buffers, with a flag for whether the buffer is taken and how
    // many frames it has been unused for.
    // TODO: We could ideally write all readback data to one big buffer per frame, the assumption
    // here is that very few entities well actually be read back at once, and their size is
    // unlikely to change.
    buffers: HashMap<u64, Vec<GpuReadbackBuffer>>,
    // Map of (width, height, format) to staging textures (M1-4b-1; the staging
    // texture must match the source texture's subresource exactly).
    textures: HashMap<(u32, u32, TextureFormat), Vec<GpuReadbackTexture>>,
}

impl GpuReadbackBufferPool {
    fn get(&mut self, render_device: &RenderDevice, size: u64) -> Option<ReadbackStaging> {
        let buffers = self.buffers.entry(size).or_default();

        // find an untaken buffer for this size
        if let Some(buf) = buffers.iter_mut().find(|x| !x.taken) {
            buf.taken = true;
            buf.frames_unused = 0;
            return Some(ReadbackStaging::Buffer(buf.buffer.clone()));
        }

        let buffer = render_device.create_buffer(&crate::render_resource::BufferDescriptor {
            label: Some("Readback Buffer"),
            size,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // No diligent side (fallback device): the readback cannot progress -
        // keep the requested entry empty so the caller skips it.
        if buffer.diligent().is_none() {
            return None;
        }
        buffers.push(GpuReadbackBuffer {
            buffer: buffer.clone(),
            taken: true,
            frames_unused: 0,
        });
        Some(ReadbackStaging::Buffer(buffer))
    }

    fn get_texture(
        &mut self,
        render_device: &RenderDevice,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Option<ReadbackStaging> {
        let key = (width, height, format);
        let textures = self.textures.entry(key).or_default();

        if let Some(tex) = textures.iter_mut().find(|x| !x.taken) {
            tex.taken = true;
            tex.frames_unused = 0;
            return Some(ReadbackStaging::Texture(tex.texture.clone()));
        }

        let diligent_format = diligent_rs::format::to_diligent(format).ok()?;
        let texture = render_device.diligent_device()?.create_staging_texture(
            "bevy_readback_staging",
            width,
            height,
            diligent_format,
        );
        let Ok(texture) = texture else {
            warn!("diligent: readback staging texture creation failed");
            return None;
        };
        let texture = DiligentHandle::new(alloc::sync::Arc::new(texture));
        textures.push(GpuReadbackTexture {
            texture: texture.clone(),
            taken: true,
            frames_unused: 0,
        });
        Some(ReadbackStaging::Texture(texture))
    }

    // Returns the staging slot to the pool so it can be used in a future frame
    fn return_staging(&mut self, slot: &ReadbackStaging) {
        match slot {
            ReadbackStaging::Buffer(buffer) => {
                let size = buffer.size();
                let buffers = self
                    .buffers
                    .get_mut(&size)
                    .expect("Returned buffer of untracked size");
                if let Some(buf) = buffers.iter_mut().find(|x| x.buffer.id() == buffer.id()) {
                    buf.taken = false;
                } else {
                    warn!("Returned buffer that was not allocated");
                }
            }
            ReadbackStaging::Texture(texture) => {
                // The pool key (width, height, format) is not recoverable
                // from the slot alone - search every bucket for the pointer
                // (bounded: one entry per in-flight readback).
                for (_, textures) in self.textures.iter_mut() {
                    if let Some(tex) =
                        textures.iter_mut().find(|x| x.texture.as_raw() == texture.as_raw())
                    {
                        tex.taken = false;
                        return;
                    }
                }
                warn!("Returned staging texture that was not allocated");
            }
        }
    }

    fn update(&mut self, max_unused_frames: usize) {
        for (_, buffers) in &mut self.buffers {
            // Tick all the buffers
            for buf in &mut *buffers {
                if !buf.taken {
                    buf.frames_unused += 1;
                }
            }

            // Remove buffers that haven't been used for MAX_UNUSED_FRAMES
            buffers.retain(|x| x.frames_unused < max_unused_frames);
        }

        for (_, textures) in &mut self.textures {
            for tex in &mut *textures {
                if !tex.taken {
                    tex.frames_unused += 1;
                }
            }
            textures.retain(|x| x.frames_unused < max_unused_frames);
        }

        // Remove empty buffer sizes
        self.buffers.retain(|_, buffers| !buffers.is_empty());
        self.textures.retain(|_, textures| !textures.is_empty());
    }
}

enum ReadbackSource {
    Texture {
        texture: Texture,
    },
    Buffer {
        buffer: Buffer,
        start_offset_and_size: Option<(u64, u64)>,
    },
}

#[derive(Resource, Default)]
struct GpuReadbacks {
    requested: Vec<GpuReadback>,
    mapped: Vec<GpuReadback>,
    /// The cross-frame readback fence (方案 A): every per-frame copy signals
    /// the fence with a monotonically increasing value, `map_buffers` polls
    /// `get_completed_value` and falls back to a blocking `wait` after
    /// `SYNC_FALLBACK_FRAMES` (方案 C).
    fence: Option<DiligentHandle<diligent_rs::Fence>>,
    next_value: u64,
}

/// Pending frames before the readback falls back to a blocking fence wait
/// (方案 C).
const SYNC_FALLBACK_FRAMES: u32 = 8;

struct GpuReadback {
    pub entity: Entity,
    pub src: ReadbackSource,
    pub staging: ReadbackStaging,
    /// The fence value of this readback's latest copy (0 = never submitted).
    pub fence_value: u64,
    /// Frames the readback has been waiting for its copy to complete.
    pub pending_frames: u32,
    pub rx: Receiver<(Entity, ReadbackStaging, Vec<u8>)>,
    pub tx: Sender<(Entity, ReadbackStaging, Vec<u8>)>,
}

fn sync_readbacks(
    mut main_world: ResMut<MainWorld>,
    mut buffer_pool: ResMut<GpuReadbackBufferPool>,
    mut readbacks: ResMut<GpuReadbacks>,
    max_unused_frames: Res<GpuReadbackMaxUnusedFrames>,
) {
    readbacks.mapped.retain(|readback| {
        if let Ok((entity, staging, data)) = readback.rx.try_recv() {
            main_world.trigger(ReadbackComplete { data, entity });
            buffer_pool.return_staging(&staging);
            false
        } else {
            true
        }
    });

    buffer_pool.update(max_unused_frames.0);
}

fn prepare_buffers(
    render_device: Res<RenderDevice>,
    mut readbacks: ResMut<GpuReadbacks>,
    mut buffer_pool: ResMut<GpuReadbackBufferPool>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    ssbos: Res<RenderAssets<GpuShaderBuffer>>,
    handles: Query<(&MainEntity, &Readback)>,
) {
    for (entity, readback) in handles.iter() {
        match readback {
            Readback::Texture(image) => {
                if let Some(gpu_image) = gpu_images.get(image)
                    && gpu_image.texture_descriptor.format.pixel_size().is_ok()
                {
                    let Some(staging) = buffer_pool.get_texture(
                        &render_device,
                        gpu_image.texture_descriptor.size.width,
                        gpu_image.texture_descriptor.size.height,
                        gpu_image.texture_descriptor.format,
                    ) else {
                        debug!(
                            "diligent: no staging texture for readback of {:?} \
                             (fallback device); skipping",
                            image
                        );
                        continue;
                    };
                    let (tx, rx) = async_channel::bounded(1);
                    readbacks.requested.push(GpuReadback {
                        entity: entity.id(),
                        src: ReadbackSource::Texture {
                            texture: gpu_image.texture.clone(),
                        },
                        staging,
                        fence_value: 0,
                        pending_frames: 0,
                        rx,
                        tx,
                    });
                }
            }
            Readback::Buffer {
                buffer,
                start_offset_and_size,
            } => {
                if let Some(ssbo) = ssbos.get(buffer) {
                    let full_size = ssbo.buffer.size();
                    let size = start_offset_and_size
                        .map(|(start, size)| {
                            let end = start + size;
                            if end > full_size {
                                panic!(
                                    "Tried to read past the end of the buffer (start: {start}, \
                                    size: {size}, buffer size: {full_size})."
                                );
                            }
                            size
                        })
                        .unwrap_or(full_size);
                    let Some(staging) = buffer_pool.get(&render_device, size) else {
                        debug!(
                            "diligent: no staging buffer for readback of {:?} \
                             (fallback device); skipping",
                            buffer
                        );
                        continue;
                    };
                    let (tx, rx) = async_channel::bounded(1);
                    readbacks.requested.push(GpuReadback {
                        entity: entity.id(),
                        src: ReadbackSource::Buffer {
                            start_offset_and_size: *start_offset_and_size,
                            buffer: ssbo.buffer.clone(),
                        },
                        staging,
                        fence_value: 0,
                        pending_frames: 0,
                        rx,
                        tx,
                    });
                }
            }
        }
    }
}

/// Submits the per-frame readback copies on the Diligent immediate context
/// (M1-4b-1; called by `renderer::render_system` after the render schedule
/// and before the context flush) and signals the readback fence.
pub(crate) fn submit_readback_commands(world: &mut World, context: &diligent_rs::DeviceContext) {
    // Owned clone: the device is only queried for the lazy fence creation,
    // and the world borrow must not overlap the `readbacks` mutable borrow.
    let render_device = world.resource::<RenderDevice>().clone();
    let mut readbacks = world.resource_mut::<GpuReadbacks>();

    if readbacks.fence.is_none() {
        if let Some(device) = render_device.diligent_device()
            && let Ok(fence) = device.create_fence("bevy_readback_fence")
        {
            readbacks.fence = Some(DiligentHandle::new(alloc::sync::Arc::new(fence)));
        }
    }
    let Some(fence) = readbacks.fence.clone() else {
        return;
    };

    let GpuReadbacks {
        requested,
        next_value,
        ..
    } = &mut *readbacks;
    // M1-4b-2 review, fix 1: the copies and the fence signal are
    // immediate-context calls - serialize them like every other call site
    // (the loop only makes direct `IDeviceContext` calls, no locked
    // wrapper paths, so a single guard is safe here).
    let _guard = crate::renderer::diligent_registry::context_guard();
    for readback in requested.iter_mut() {
        let result = match &readback.src {
            ReadbackSource::Texture { texture } => {
                let (Some(src), ReadbackStaging::Texture(dst)) =
                    (texture.diligent(), &readback.staging)
                else {
                    continue;
                };
                context.copy_texture(src, 0, 0, dst, 0, 0)
            }
            ReadbackSource::Buffer {
                buffer,
                start_offset_and_size,
            } => {
                let (src_start, size) = start_offset_and_size.unwrap_or((0, buffer.size()));
                let ReadbackStaging::Buffer(dst) = &readback.staging else {
                    continue;
                };
                let (Some(src), Some(dst)) = (buffer.diligent(), dst.diligent()) else {
                    continue;
                };
                context.copy_buffer(src, src_start, dst, 0, size)
            }
        };
        if let Err(err) = result {
            warn!("diligent: readback copy failed: {err}");
            continue;
        }
        *next_value += 1;
        readback.fence_value = *next_value;
        if let Err(err) = context.enqueue_signal(&fence, *next_value) {
            warn!("diligent: readback fence signal failed: {err}");
            readback.fence_value = 0;
        }
    }
}

/// Maps the completed readback staging resources (M1-4b-1 方案 A: fence poll
/// per frame, `SYNC_FALLBACK_FRAMES` pending frames -> blocking wait,
/// 方案 C) and sends the data through the per-readback channel.
fn map_buffers(
    render_device: Res<RenderDevice>,
    mut readbacks: ResMut<GpuReadbacks>,
) {
    let requested = readbacks.requested.drain(..).collect::<Vec<GpuReadback>>();
    if requested.is_empty() {
        return;
    }
    let Some(context) = render_device.diligent_context() else {
        readbacks.mapped.extend(requested);
        return;
    };
    let Some(fence) = readbacks.fence.clone() else {
        readbacks.mapped.extend(requested);
        return;
    };
    let completed = match fence.get_completed_value() {
        Ok(value) => value,
        Err(err) => {
            warn!("diligent: readback fence query failed: {err}");
            readbacks.mapped.extend(requested);
            return;
        }
    };

    for mut readback in requested {
        let ready = if readback.fence_value > 0 && completed >= readback.fence_value {
            true
        } else {
            readback.pending_frames += 1;
            if readback.pending_frames >= SYNC_FALLBACK_FRAMES {
                // 方案 C: blocking wait for the copy.
                readback.fence_value > 0 && fence.wait(readback.fence_value).is_ok()
            } else {
                false
            }
        };
        if !ready {
            readbacks.mapped.push(readback);
            continue;
        }

        let data = match map_staging(context, &readback.staging) {
            Ok(Some(data)) => data,
            Ok(None) => {
                // The fence reported completion but the map is not visible
                // yet (other backends); retry next frame.
                readbacks.mapped.push(readback);
                continue;
            }
            Err(err) => {
                warn!("diligent: readback map failed: {err}");
                readbacks.mapped.push(readback);
                continue;
            }
        };
        let entity = readback.entity;
        let staging = readback.staging.clone();
        if let Err(e) = readback.tx.try_send((entity, staging, data)) {
            warn!("Failed to send readback result: {}", e);
        }
    }
}

/// Maps one staging resource and copies its contents to the CPU. `Ok(None)`
/// = the map is not visible yet (retry next frame).
fn map_staging(
    context: &diligent_rs::DeviceContext,
    staging: &ReadbackStaging,
) -> Result<Option<Vec<u8>>, String> {
    use diligent_rs::diligent_sys::bindings as sys;
    let map_type = sys::_MAP_TYPE::MAP_READ as sys::MAP_TYPE;
    // M1-4b-2 review, fix 1: `MapBuffer` / `MapTextureSubresource` are
    // immediate-context calls - serialize them.
    let _guard = crate::renderer::diligent_registry::context_guard();
    match staging {
        ReadbackStaging::Buffer(buffer) => {
            let Some(diligent) = buffer.diligent() else {
                return Err("readback buffer has no diligent side".into());
            };
            let mapped = context
                .map_buffer(diligent, map_type, true)
                .map_err(|e| e.to_string())?;
            Ok(mapped.map(|mapped| Vec::from(mapped.as_slice())))
        }
        ReadbackStaging::Texture(texture) => {
            let mapped = context
                .map_texture_subresource(texture, 0, 0, map_type, true)
                .map_err(|e| e.to_string())?;
            let Some(mapped) = mapped else {
                return Ok(None);
            };
            let stride = mapped.stride();
            let desc = match texture.desc() {
                Ok(desc) => desc,
                Err(err) => {
                    // The descriptor query failed; treat as not-ready and
                    // retry next frame - never return padded garbage rows
                    // for an unknown height.
                    warn!("diligent: readback staging texture descriptor query failed: {err}");
                    return Ok(None);
                }
            };
            let height = desc.Height as usize;
            let mut data = Vec::with_capacity(stride * height);
            for row in 0..height {
                // Safety: the mapping is alive; each row is `stride` bytes.
                data.extend_from_slice(unsafe {
                    core::slice::from_raw_parts(mapped.row(row), stride)
                });
            }
            drop(mapped);
            Ok(Some(data))
        }
    }
}
