pub(crate) mod diligent_draw;
pub(crate) mod diligent_features;
pub(crate) mod diligent_mapping;
pub(crate) mod diligent_pso;
pub(crate) mod diligent_registry;
mod render_context;
/// The M2a SRB/bind-group helpers (apply_dynamic_offsets,
/// set_inline_constants, ImmediateSrb) are consumed by the pass recording
/// paths (`draw_state`, `wgpu_compat`) - hence the pub(crate) module.
pub(crate) mod render_device;
mod wgpu_wrapper;

pub use diligent_features::DiligentFeatures;
pub use render_context::{
    CurrentView, FlushCommands, PendingCommandBuffers, RenderContext, RenderContextState, ViewQuery,
};
pub use render_device::*;
pub use wgpu_wrapper::WgpuWrapper;
use crate::renderer::diligent_registry::DiligentHandle;
use crate::render_resource::wgpu_compat::{Adapter, CommandBuffer, Device, Instance, QueueWriteBufferView};
use crate::{
    render_resource::Buffer,
    settings::{RenderResources, WgpuSettings},
    view::{ExtractedWindows, ViewTarget, WindowSurfaces},
};
use alloc::sync::Arc;
use bevy_camera::NormalizedRenderTarget;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::schedule::ScheduleLabel;
use bevy_ecs::{prelude::*, system::SystemState};
#[cfg(feature = "trace")]
use bevy_log::info_span;
use bevy_log::{debug, info, warn};
use bevy_render::camera::ExtractedCamera;
use bevy_utils::default;
use bevy_window::RawHandleWrapperHolder;
use diligent_rs::diligent_sys::bindings as sys;
use std::sync::Mutex;
use wgpu_types::Backends;

/// Schedule label for the root render graph schedule. This schedule runs once per frame
/// in the [`render_system`] system and is responsible for driving the entire rendering process.
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RenderGraph;

impl RenderGraph {
    pub fn base_schedule() -> Schedule {
        let mut schedule = Schedule::new(Self);
        schedule.configure_sets(
            (
                RenderGraphSystems::Begin,
                RenderGraphSystems::Render,
                RenderGraphSystems::Submit,
                RenderGraphSystems::Finish,
            )
                .chain(),
        );
        schedule
    }
}

/// System sets for the root [`RenderGraph`] schedule.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderGraphSystems {
    /// Runs before rendering. Used for per-frame setup.
    Begin,
    /// The main rendering phase.
    Render,
    /// Submits pending command buffers generated during [`RenderGraphSystems::Render`]
    Submit,
    /// Runs after rendering and submit. Used for per-frame finalization.
    Finish,
}

/// The main render system that drives the rendering process. This system runs the [`RenderGraph`]
/// schedule, runs any finalization commands like screenshot captures and GPU readbacks, and
/// calls present on swap chains that need to be presented.
pub fn render_system(
    world: &mut World,
    state: &mut SystemState<Query<(&ViewTarget, &ExtractedCamera)>>,
) {
    #[cfg(feature = "trace")]
    let _span = info_span!("main_render_schedule").entered();

    {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();

        // M1-4b-1: wire the Diligent immediate context into the queue BEFORE
        // the render schedule runs, so the `write_buffer`/`write_texture`
        // upload paths inside the schedule find the context wired (frame 1
        // included).
        render_queue.attach(render_device);
    }

    world.run_schedule(RenderGraph);

    // M1-4b-1: the per-frame copies (screenshots, GPU readbacks) run on the
    // Diligent immediate context; the owned handle ends the world borrows
    // before the mutable-world calls below.
    let diligent_context = world
        .resource::<RenderDevice>()
        .diligent_context_handle();

    if let Some(context) = diligent_context {
        crate::view::screenshot::submit_screenshot_commands(world, &context);
        crate::gpu_readback::submit_readback_commands(world, &context);
        // M1-3: the render graph recorded on the Diligent immediate
        // context during the schedule; Flush submits the recorded
        // command list to the GPU.
        let _guard = diligent_registry::context_guard();
        context.flush();
    }

    {
        #[cfg(feature = "trace")]
        let _span = info_span!("present_frames").entered();

        world.resource_scope(|world, mut windows: Mut<ExtractedWindows>| {
            let window_surfaces = world.resource::<WindowSurfaces>();
            let views = state.get(world).unwrap();
            for window in windows.values_mut() {
                let view_needs_present = views.iter().any(|(view_target, camera)| {
                    matches!(
                        camera.target,
                        Some(NormalizedRenderTarget::Window(w)) if w.entity() == window.entity
                    ) && view_target.needs_present()
                });

                if view_needs_present || window.needs_initial_present {
                    // M1-3: present on the window's Diligent swap chain (the
                    // sync interval follows the window's present mode).
                    window_surfaces.present(&window.entity);
                    window.needs_initial_present = false;
                }
            }
        });

        #[cfg(feature = "tracing-tracy")]
        bevy_log::event!(
            bevy_log::Level::INFO,
            message = "finished frame",
            tracy.frame_mark = true
        );
    }

    crate::view::screenshot::collect_screenshots(world);
}

/// This queue is used to enqueue tasks for the GPU to execute asynchronously.
///
/// M1-4b-1: `write_buffer` / `write_texture` are implemented on the Diligent
/// immediate context (`UpdateBuffer` / `UpdateTexture`); M1-4b-2: the
/// transition wgpu queue is gone - the diligent context is the only
/// execution path (submitted `CommandBuffer`s are ordering tokens, their
/// commands were recorded at encode time).
#[derive(Resource, Clone)]
pub struct RenderQueue {
    /// The Diligent immediate context, wired once from the `RenderDevice`
    /// (see [`RenderQueue::attach`]; `None` when the engine failed to
    /// initialize).
    diligent_context: Arc<Mutex<Option<DiligentHandle<diligent_rs::DeviceContext>>>>,
}

impl Default for RenderQueue {
    fn default() -> Self {
        Self {
            diligent_context: default(),
        }
    }
}

impl RenderQueue {
    /// Creates an empty queue (the context is wired by [`RenderQueue::attach`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Wires the Diligent immediate context from the render device (once;
    /// subsequent calls are no-ops). Called by `render_system` immediately
    /// before `world.run_schedule(RenderGraph)` runs, so the
    /// `write_buffer`/`write_texture` upload paths inside the schedule
    /// (frame 1 included) find the context wired.
    pub(crate) fn attach(&self, render_device: &RenderDevice) {
        let mut slot = self.diligent_context.lock().unwrap();
        if slot.is_none() {
            *slot = render_device.diligent_context_handle();
        }
    }

    /// Copies the bytes of `data` into `buffer` at `offset`.
    ///
    /// M1-4b-1: issues `IDeviceContext::UpdateBuffer` (recorded in command
    /// order on the immediate context). Same semantics as
    /// `wgpu Queue::write_buffer`.
    pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) {
        let Some(context) = self.diligent_context.lock().unwrap().clone() else {
            bevy_log::debug!("diligent: write_buffer skipped (no diligent context)");
            return;
        };
        let Some(diligent) = buffer.diligent() else {
            bevy_log::debug!("diligent: write_buffer skipped (buffer has no diligent side)");
            return;
        };
        let _guard = diligent_registry::context_guard();
        if let Err(err) = context.update_buffer(diligent, offset, data) {
            bevy_log::warn!("diligent: buffer update via the immediate context failed: {err}");
        }
    }

    /// Copies the bytes of `data` into a texture.
    ///
    /// M1-4b-1: issues `IDeviceContext::UpdateTexture` (aspect `All` only).
    /// Same semantics as `wgpu Queue::write_texture`.
    pub fn write_texture(
        &self,
        destination: crate::render_resource::TexelCopyTextureInfo<'_>,
        data: &[u8],
        data_layout: wgpu_types::TexelCopyBufferLayout,
        size: wgpu_types::Extent3d,
    ) {
        if destination.aspect != wgpu_types::TextureAspect::All {
            bevy_log::debug!(
                "diligent: write_texture skipped (aspect {:?} is not All)",
                destination.aspect
            );
            return;
        }
        let Some(context) = self.diligent_context.lock().unwrap().clone() else {
            bevy_log::debug!("diligent: write_texture skipped (no diligent context)");
            return;
        };
        let Some(texture) = diligent_registry::registry().resolve_texture(destination.texture.id())
        else {
            bevy_log::debug!("diligent: write_texture skipped (texture has no diligent side)");
            return;
        };
        let _guard = diligent_registry::context_guard();
        if let Err(err) = diligent_upload_texture(&context, texture, &destination, data, data_layout, size) {
            bevy_log::warn!("diligent: write_texture failed: {err}");
        }
    }

    /// Allocates a temporary write view for `size` bytes at `offset` in
    /// `buffer`; the bytes are uploaded to the diligent context when the
    /// returned view is dropped.
    ///
    /// Same semantics as `wgpu Queue::write_buffer_with`.
    pub fn write_buffer_with(
        &self,
        buffer: &Buffer,
        offset: u64,
        size: wgpu_types::BufferSize,
    ) -> Option<QueueWriteBufferView> {
        let context = self.diligent_context.lock().unwrap().clone();
        let has_diligent = buffer.diligent().is_some();
        if !has_diligent {
            return None;
        }
        Some(QueueWriteBufferView::new(
            buffer.clone(),
            offset,
            vec![0u8; size.get() as usize],
            context,
        ))
    }

    /// Submits finished command buffers. The commands were already recorded
    /// on the diligent immediate context at encode time (M1-4b-2), so the
    /// buffers are ordering tokens.
    pub fn submit<T: IntoIterator<Item = CommandBuffer>>(
        &self,
        _command_buffers: T,
    ) -> crate::render_resource::wgpu_compat::SubmissionIndex {
        crate::render_resource::wgpu_compat::SubmissionIndex(0)
    }

    /// Compacts a BLAS (no-op on the diligent path - returns a clone of the
    /// input handle).
    pub fn compact_blas(&self, blas: &crate::render_resource::Blas) -> crate::render_resource::Blas {
        let _ = self;
        blas.clone()
    }
}

/// `IDeviceContext::UpdateTexture` for a wgpu `write_texture` call: the
/// `DstBox`/`Stride`/`DepthStride` translation (see
/// `DeviceContextD3D12Impl::UpdateTexture` - the source row stride and the
/// destination box drive the upload; compressed formats get block-aligned
/// boxes, which the engine realigns internally).
fn diligent_upload_texture(
    context: &diligent_rs::DeviceContext,
    texture: *mut sys::ITexture,
    destination: &crate::render_resource::TexelCopyTextureInfo<'_>,
    data: &[u8],
    data_layout: wgpu_types::TexelCopyBufferLayout,
    size: wgpu_types::Extent3d,
) -> Result<(), String> {
    let format = destination.texture.format();
    let (block_width, block_height) = format.block_dimensions();
    let bytes_per_block = format.block_copy_size(None).unwrap_or(0) as u32;
    let blocks_per_row = size.width.div_ceil(block_width);
    let row_bytes = data_layout.bytes_per_row.unwrap_or(blocks_per_row * bytes_per_block) as u64;
    let rows = size.height.div_ceil(block_height) as u64;
    let depth_stride = data_layout
        .rows_per_image
        .map(|rows| rows as u64 * row_bytes)
        .unwrap_or(rows * row_bytes);
    let (min_z, max_z, slice) = match destination.texture.dimension() {
        wgpu_types::TextureDimension::D3 => (
            destination.origin.z,
            destination.origin.z + size.depth_or_array_layers,
            0,
        ),
        _ => (0, 1, destination.origin.z),
    };
    let mut subres: sys::TextureSubResData = unsafe { std::mem::zeroed() };
    subres.pData = data.as_ptr().cast();
    subres.Stride = row_bytes;
    subres.DepthStride = depth_stride;
    subres.pSrcBuffer = std::ptr::null_mut();
    let box_ = sys::Box {
        MinX: destination.origin.x,
        MinY: destination.origin.y,
        MinZ: min_z,
        MaxX: destination.origin.x + size.width,
        MaxY: destination.origin.y + size.height,
        MaxZ: max_z,
    };
    let update = unsafe {
        (*(*context.as_raw()).pVtbl)
            .DeviceContext
            .UpdateTexture
            .as_ref()
            .ok_or("IDeviceContext::UpdateTexture missing from vtable")?
    };
    // Safety: `data` is valid for the duration of the call (the engine
    // copies it into the upload heap) and the box/strides were derived from
    // the caller's size/origin/layout.
    unsafe {
        update(
            context.as_raw(),
            texture,
            destination.mip_level,
            slice,
            &box_,
            &subres,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        )
    };
    Ok(())
}

/// The handle to the physical device being used for rendering.
/// See [`Adapter`] for more info.
#[derive(Resource, Clone, Debug, Deref, DerefMut)]
pub struct RenderAdapter(pub Arc<WgpuWrapper<Adapter>>);

/// The GPU instance is used to initialize the [`RenderQueue`] and [`RenderDevice`],
/// as well as to create [`WindowSurfaces`](crate::view::window::WindowSurfaces).
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RenderInstance(pub Arc<WgpuWrapper<Instance>>);

/// The [`AdapterInfo`] of the adapter in use by the renderer.
#[derive(Resource, Clone, Deref, DerefMut)]
pub struct RenderAdapterInfo(pub WgpuWrapper<wgpu_types::AdapterInfo>);

/// The wgpu-types adapter info derived from the diligent device/adapter
/// queries.
fn diligent_adapter_info(
    device_info: &sys::RenderDeviceInfo,
    adapter: &sys::GraphicsAdapterInfo,
) -> wgpu_types::AdapterInfo {
    let mut name_bytes = [0u8; 128];
    for (i, ch) in adapter.Description.iter().enumerate() {
        let c = *ch as u8;
        if c == 0 {
            break;
        }
        name_bytes[i] = c;
    }
    let name = String::from_utf8_lossy(&name_bytes[..])
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let device_type = match adapter.Type {
        t if t == sys::_ADAPTER_TYPE::ADAPTER_TYPE_DISCRETE as sys::ADAPTER_TYPE => {
            wgpu_types::DeviceType::DiscreteGpu
        }
        t if t == sys::_ADAPTER_TYPE::ADAPTER_TYPE_INTEGRATED as sys::ADAPTER_TYPE => {
            wgpu_types::DeviceType::IntegratedGpu
        }
        t if t == sys::_ADAPTER_TYPE::ADAPTER_TYPE_SOFTWARE as sys::ADAPTER_TYPE => {
            wgpu_types::DeviceType::Cpu
        }
        _ => wgpu_types::DeviceType::Other,
    };
    let backend = match device_info.Type {
        t if t == sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_D3D12 as sys::RENDER_DEVICE_TYPE => {
            wgpu_types::Backend::Dx12
        }
        t if t == sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_VULKAN as sys::RENDER_DEVICE_TYPE => {
            wgpu_types::Backend::Vulkan
        }
        _ => wgpu_types::Backend::Noop,
    };
    wgpu_types::AdapterInfo {
        name,
        vendor: adapter.VendorId,
        device: adapter.DeviceId,
        device_type,
        device_pci_bus_id: String::new(),
        driver: String::new(),
        driver_info: String::new(),
        backend,
        subgroup_min_size: wgpu_types::MINIMUM_SUBGROUP_MIN_SIZE,
        subgroup_max_size: wgpu_types::MAXIMUM_SUBGROUP_MAX_SIZE,
        transient_saves_memory: false,
    }
}

/// Initializes the renderer: bootstraps the Diligent engine (factory +
/// device + immediate context - the only rendering path), derives the
/// capability data and assembles the render resources.
pub fn initialize_renderer(
    _backends: Backends,
    _primary_window: Option<RawHandleWrapperHolder>,
    options: &WgpuSettings,
) -> RenderResources {
    let (diligent_factory, diligent_device, diligent_context, backend, caps, adapter_info) =
        match diligent_rs::EngineFactoryD3D12::d3d12() {
            Ok(factory) => {
                // Task 19.3: debug builds force the Diligent validation layer
                // on (D3D12 debug layer / Vulkan validation layers); release
                // builds default to off. `DILIGENT_RS_VALIDATION=off` forces
                // it off in a debug build (performance probes), and
                // `DILIGENT_RS_VALIDATION=level2` enables GPU-based
                // validation (catches the AMD iGPU device-removal trigger).
                let level = match std::env::var("DILIGENT_RS_VALIDATION").as_deref() {
                    Ok("off") => diligent_rs::desc::ValidationLevel::Off,
                    Ok("level2") => diligent_rs::desc::ValidationLevel::Level2,
                    Ok("level1") | Ok("") => diligent_rs::desc::ValidationLevel::Level1,
                    _ => diligent_rs::desc::ValidationLevel::default(),
                };
                match factory.create_device_and_contexts_with_validation(level) {
                Ok((device, context)) => {
                    let device_info = device.device_info();
                    let adapter = device.adapter_info();
                    // M2a-1 review, fix 3: latch the device's constant buffer
                    // offset alignment - the `SetBufferOffset` offset
                    // validation rule on the draw path.
                    latch_constant_buffer_offset_alignment(
                        adapter.Buffer.ConstantBufferOffsetAlignment,
                    );
                    let backend = match device_info.Type {
                        t if t == (sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_D3D12
                            as sys::RENDER_DEVICE_TYPE) =>
                        {
                            diligent_pso::DiligentBackend::D3D12
                        }
                        t if t == (sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_VULKAN
                            as sys::RENDER_DEVICE_TYPE) =>
                        {
                            diligent_pso::DiligentBackend::Vulkan
                        }
                        _ => diligent_pso::DiligentBackend::Other,
                    };
                    let caps = diligent_features::DiligentCaps::derive(&device);
                    let info = diligent_adapter_info(&device_info, &adapter);
                    (
                        Some(DiligentHandle::new(Arc::new(factory))),
                        Some(DiligentHandle::new(Arc::new(device))),
                        Some(DiligentHandle::new(Arc::new(context))),
                        backend,
                        caps,
                        Some(info),
                    )
                }
                Err(err) => {
                    bevy_log::warn!("diligent: engine initialization failed ({err})");
                    (None, None, None, diligent_pso::DiligentBackend::Other, None, None)
                }
            }
            },
            Err(err) => {
                bevy_log::warn!("diligent: engine factory resolution failed ({err})");
                (None, None, None, diligent_pso::DiligentBackend::Other, None, None)
            }
        };

    // The capability-derived feature/limit set (M1-4a): the diligent
    // feature mask is intersected with the `WgpuSettings` feature bits
    // (requested/disabled features fold in below - drops bits, never adds).
    let mut features = caps
        .as_ref()
        .map_or(wgpu_types::Features::empty(), |caps| caps.features().as_features())
        | options.features;
    if let Some(disabled_features) = options.disabled_features {
        features.remove(disabled_features);
    }
    let caps = caps.map(|caps| caps.intersect_settings_features(features));

    let mut limits = wgpu_types::Limits::default();
    if let Some(caps) = &caps {
        limits.max_storage_buffers_per_shader_stage = caps.max_storage_buffers_per_shader_stage();
        limits.max_storage_textures_per_shader_stage = caps.max_storage_textures_per_shader_stage();
    }
    if let Some(constrained_limits) = options.constrained_limits.as_ref() {
        limits = limits.or_worse_values_from(constrained_limits);
    }

    let downlevel_capabilities = wgpu_types::DownlevelCapabilities {
        flags: wgpu_types::DownlevelFlags::compliant(),
        limits: wgpu_types::DownlevelLimits::default(),
        shader_model: wgpu_types::ShaderModel::Sm5,
    };

    let adapter_info = adapter_info.unwrap_or_else(|| wgpu_types::AdapterInfo {
        name: "Diligent (D3D12)".to_string(),
        vendor: 0,
        device: 0,
        device_type: wgpu_types::DeviceType::Other,
        device_pci_bus_id: String::new(),
        driver: String::new(),
        driver_info: String::new(),
        backend: wgpu_types::Backend::Dx12,
        subgroup_min_size: wgpu_types::MINIMUM_SUBGROUP_MIN_SIZE,
        subgroup_max_size: wgpu_types::MAXIMUM_SUBGROUP_MAX_SIZE,
        transient_saves_memory: false,
    });
    info!("{:?}", adapter_info);

    if adapter_info.device_type == wgpu_types::DeviceType::Cpu {
        warn!(
            "The selected adapter is using a driver that only supports software rendering. \
             This is likely to be very slow. See https://bevy.org/learn/errors/b0006/"
        );
    }

    debug!("Configured diligent adapter Limits: {:#?}", limits);
    debug!("Configured diligent adapter Features: {:#?}", features);

    let device_facade = Device::new(features, limits.clone());
    let adapter_facade = Adapter::new(adapter_info.clone(), features, limits, downlevel_capabilities);

    // M3b §8.10: create the in-memory pipeline-state cache (LOAD_STORE).
    // `Err` (no PSO-cache support on D3D11/OpenGL-like devices, or engine
    // init failure) degrades to no cache - PSO creation then simply does not
    // feed `pPSOCache`.
    let pso_cache = diligent_device
        .as_deref()
        .and_then(|device| match device.create_pipeline_state_cache("bevy_pso_cache") {
            Ok(cache) => {
                bevy_log::debug!("diligent: pipeline-state cache created (M3b §8.10)");
                Some(DiligentHandle::new(Arc::new(cache)))
            }
            Err(err) => {
                bevy_log::warn!("diligent: pipeline-state cache unavailable ({err})");
                None
            }
        });

    let render_device = RenderDevice::from_parts(
        diligent_factory,
        diligent_device,
        diligent_context,
        backend,
        caps,
        device_facade,
        pso_cache,
    );

    RenderResources(
        render_device,
        RenderQueue::new(),
        RenderAdapterInfo(WgpuWrapper::new(adapter_info)),
        RenderAdapter(Arc::new(WgpuWrapper::new(adapter_facade))),
        RenderInstance(Arc::new(WgpuWrapper::new(Instance::default()))),
    )
}
