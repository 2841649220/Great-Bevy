use crate::camera::extract_cameras;
use crate::{
    render_resource::{TextureFormat, TextureView, WgpuTextureView},
    renderer::{diligent_draw, diligent_registry::DiligentHandle, RenderDevice},
    Extract, ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_app::{App, Plugin};
use bevy_ecs::entity::EntityHashSet;
use bevy_ecs::{entity::EntityHashMap, prelude::*};
use bevy_log::{debug, info, warn};
use bevy_window::{
    CompositeAlphaMode, PresentMode, PrimaryWindow, RawHandleWrapper, Window, WindowClosing,
};
use core::{
    ffi::c_void,
    num::NonZero,
    ops::{Deref, DerefMut},
};

pub mod screenshot;

use screenshot::ScreenshotPlugin;

pub struct WindowRenderPlugin;

impl Plugin for WindowRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ScreenshotPlugin);

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<ExtractedWindows>()
                .init_gpu_resource::<WindowSurfaces>()
                .add_systems(ExtractSchedule, extract_windows.before(extract_cameras))
                .add_systems(
                    Render,
                    create_surfaces
                        .run_if(need_surface_configuration)
                        .before(prepare_windows),
                )
                .add_systems(Render, prepare_windows.in_set(RenderSystems::PrepareViews));
        }
    }
}

pub struct ExtractedWindow {
    /// An entity that contains the components in [`Window`].
    pub entity: Entity,
    pub handle: RawHandleWrapper,
    pub physical_width: u32,
    pub physical_height: u32,
    pub present_mode: PresentMode,
    pub desired_maximum_frame_latency: Option<NonZero<u32>>,
    /// Note: this will not always be the swap chain texture view. When taking a screenshot,
    /// this will point to an alternative texture instead to allow for copying the render result
    /// to CPU memory.
    pub swap_chain_texture_view: Option<TextureView>,
    pub swap_chain_texture_format: Option<TextureFormat>,
    /// This is an srgb view of [`ExtractedWindow::swap_chain_texture_format`]
    /// so that in shaders we are always in linear space.
    pub swap_chain_texture_view_format: Option<TextureFormat>,
    pub size_changed: bool,
    pub present_mode_changed: bool,
    pub alpha_mode: CompositeAlphaMode,
    /// Whether this window needs an initial buffer commit.
    ///
    /// On Wayland, windows must present at least once before they are shown.
    /// See <https://wayland.app/protocols/xdg-shell#xdg_surface>
    pub needs_initial_present: bool,
}

impl ExtractedWindow {
    /// Stores the swap-chain texture view for this frame.
    ///
    /// M1-3: the per-frame back-buffer RTV is registered under this view's
    /// id by `prepare_windows` (the render-pass path resolves it like any
    /// other attachment).
    fn set_swapchain_texture(&mut self, view: TextureView) {
        self.swap_chain_texture_view = Some(view);
    }

    /// Presents the frame (no-op: the M1-3 present path runs on the diligent
    /// swap chain - see `renderer::render_system`, which presents through
    /// `WindowSurfaces`).
    pub fn present(&mut self) {}
}

#[derive(Default, Resource)]
pub struct ExtractedWindows {
    pub primary: Option<Entity>,
    pub windows: EntityHashMap<ExtractedWindow>,
}

impl Deref for ExtractedWindows {
    type Target = EntityHashMap<ExtractedWindow>;

    fn deref(&self) -> &Self::Target {
        &self.windows
    }
}

impl DerefMut for ExtractedWindows {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.windows
    }
}

fn extract_windows(
    mut extracted_windows: ResMut<ExtractedWindows>,
    mut closing: Extract<MessageReader<WindowClosing>>,
    windows: Extract<Query<(Entity, &Window, &RawHandleWrapper, Option<&PrimaryWindow>)>>,
    mut removed: Extract<RemovedComponents<RawHandleWrapper>>,
    mut window_surfaces: ResMut<WindowSurfaces>,
) {
    for (entity, window, handle, primary) in windows.iter() {
        if primary.is_some() {
            extracted_windows.primary = Some(entity);
        }

        let (new_width, new_height) = (
            window.resolution.physical_width().max(1),
            window.resolution.physical_height().max(1),
        );

        let extracted_window = extracted_windows.entry(entity).or_insert(ExtractedWindow {
            entity,
            handle: handle.clone(),
            physical_width: new_width,
            physical_height: new_height,
            present_mode: window.present_mode,
            desired_maximum_frame_latency: window.desired_maximum_frame_latency,
            swap_chain_texture_view: None,
            size_changed: false,
            swap_chain_texture_format: None,
            swap_chain_texture_view_format: None,
            present_mode_changed: false,
            alpha_mode: window.composite_alpha_mode,
            needs_initial_present: true,
        });

        // The swap-chain texture view is dropped here if it was already
        // presented (the diligent swap chain has no wgpu frame to keep).
        extracted_window.swap_chain_texture_view = None;
        extracted_window.size_changed = new_width != extracted_window.physical_width
            || new_height != extracted_window.physical_height;
        extracted_window.present_mode_changed =
            window.present_mode != extracted_window.present_mode;

        if extracted_window.size_changed {
            debug!(
                "Window size changed from {}x{} to {}x{}",
                extracted_window.physical_width,
                extracted_window.physical_height,
                new_width,
                new_height
            );
            extracted_window.physical_width = new_width;
            extracted_window.physical_height = new_height;
        }

        if extracted_window.present_mode_changed {
            debug!(
                "Window Present Mode changed from {:?} to {:?}",
                extracted_window.present_mode, window.present_mode
            );
            extracted_window.present_mode = window.present_mode;
        }
    }

    for closing_window in closing.read() {
        extracted_windows.remove(&closing_window.window);
        window_surfaces.remove(&closing_window.window);
    }
    for removed_window in removed.read() {
        extracted_windows.remove(&removed_window);
        window_surfaces.remove(&removed_window);
    }
}

/// The window surface configuration, mirroring the wgpu type's field names
/// (M1-3: the wgpu `SurfaceConfiguration` is replaced by a self-made
/// struct; the present mode is kept as the bevy_window type because this
/// Diligent version has no `PRESENT_MODE` enumeration - V12, api-baseline
/// §1.9).
struct SurfaceConfiguration {
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    pub present_mode: PresentMode,
    pub desired_maximum_frame_latency: u32,
    /// TODO-REMOVE-M1-4: the locked `ISwapChain` has no alpha-mode control
    /// (the M1-1 wrapper's swap-chain descriptor fixes the surface
    /// properties); kept for the wgpu field-name compatibility.
    #[expect(dead_code, reason = "TODO-REMOVE-M1-4: no swap-chain alpha-mode control")]
    pub alpha_mode: CompositeAlphaMode,
    /// TODO-REMOVE-M1-4: the locked `ISwapChain` cannot expose alternative
    /// view formats; kept for the wgpu field-name compatibility.
    #[expect(dead_code, reason = "TODO-REMOVE-M1-4: no swap-chain view-format alternatives")]
    pub view_formats: Vec<TextureFormat>,
}

struct SurfaceData {
    /// The Diligent swap chain (M1-3; replaces the wgpu surface).
    swap_chain: DiligentHandle<diligent_rs::SwapChain>,
    /// The swap-chain texture view template: the wgpu side is a transition
    /// dummy texture (TODO-REMOVE-M1-4 - the wgpu-side consumers of the
    /// swap-chain view keep working against it); the diligent side is the
    /// per-frame back-buffer RTV, registered fresh in `prepare_windows`
    /// under the dummy view's address.
    swap_chain_texture_view: TextureView,
    configuration: SurfaceConfiguration,
    /// `Some` when the swap-chain format has a separate srgb view (a
    /// non-srgb swap chain); `None` when the swap chain is already srgb.
    texture_view_format: Option<TextureFormat>,
}

impl SurfaceData {
    /// Presents the frame on the swap chain (the sync interval is derived
    /// from the configured present mode - see
    /// [`diligent_draw::present_mode_to_sync_interval`]).
    fn present(&self) {
        let sync_interval =
            diligent_draw::present_mode_to_sync_interval(self.configuration.present_mode);
        // M1-4b-2: the present touches the swap chain's command queue - it
        // must not race with a concurrent immediate-context call.
        let _guard = crate::renderer::diligent_registry::context_guard();
        self.swap_chain.present(sync_interval);
    }
}

#[derive(Resource, Default)]
pub struct WindowSurfaces {
    surfaces: EntityHashMap<SurfaceData>,
    /// List of windows that we have already called the initial `configure_surface` for
    configured_windows: EntityHashSet,
}

impl WindowSurfaces {
    fn remove(&mut self, window: &Entity) {
        self.surfaces.remove(window);
        self.configured_windows.remove(window);
    }

    /// M1-3: presents the frame on the window's swap chain (no-op when the
    /// window has no surface). Called by `renderer::render_system`.
    pub(crate) fn present(&self, window: &Entity) {
        if let Some(surface_data) = self.surfaces.get(window) {
            surface_data.present();
        }
    }
}

/// (re)configures window surfaces, and obtains a swapchain texture for rendering.
///
/// NOTE: `get_current_texture` in `prepare_windows` can take a long time if the GPU workload is
/// the performance bottleneck. This can be seen in profiles as multiple prepare-set systems all
/// taking an unusually long time to complete, and all finishing at about the same time as the
/// `prepare_windows` system. Improvements in bevy are planned to avoid this happening when it
/// should not but it will still happen as it is easy for a user to create a large GPU workload
/// relative to the GPU performance and/or CPU workload.
/// This can be caused by many reasons, but several of them are:
/// - GPU workload is more than your current GPU can manage
/// - Error / performance bug in your custom shaders
/// - wgpu was unable to detect a proper GPU hardware-accelerated device given the chosen
///   [`Backends`](crate::settings::Backends), [`WgpuLimits`](crate::settings::WgpuLimits),
///   and/or [`WgpuFeatures`](crate::settings::WgpuFeatures). For example, on Windows currently
///   `DirectX 11` is not supported by wgpu 0.12 and so if your GPU/drivers do not support Vulkan,
///   it may be that a software renderer called "Microsoft Basic Render Driver" using `DirectX 12`
///   will be chosen and performance will be very poor. This is visible in a log message that is
///   output during renderer initialization.
///   Another alternative is to try to use [`ANGLE`](https://github.com/gfx-rs/wgpu#angle) and
///   [`Backends::GL`](crate::settings::Backends::GL) with the `gles` feature enabled if your
///   GPU/drivers support `OpenGL 4.3` / `OpenGL ES 3.0` or later.
pub fn prepare_windows(
    mut windows: ResMut<ExtractedWindows>,
    mut window_surfaces: ResMut<WindowSurfaces>,
    _render_device: Res<RenderDevice>,
    sorted_cameras: Res<crate::camera::SortedCameras>,
) {
    for window in windows.windows.values_mut() {
        // Skip acquiring a swap-chain texture for windows that no camera
        // targets. This avoids a wasted clear pass in
        // `handle_uncovered_swap_chains` that triggers a DMA-fence fd leak on
        // Adreno 740 (Quest 3). The exception is windows that still need their
        // initial present (required on Wayland).
        //
        // M1-3 re-evaluation: the workaround is kept - the skip is cheap and
        // still prevents the transition clear pass (and with it the fence
        // churn the workaround was introduced for); with the Diligent swap
        // chain there is no acquire to leak, so the workaround becomes a
        // pure optimization (TODO-REMOVE-M1-4: re-verify on Quest 3 with
        // the diligent surface).
        let is_camera_target = sorted_cameras.0.iter().any(|c| {
            matches!(
                &c.target,
                Some(bevy_camera::NormalizedRenderTarget::Window(w)) if w.entity() == window.entity
            ) && matches!(c.output_mode, bevy_camera::CameraOutputMode::Write { .. })
        });
        if !is_camera_target && !window.needs_initial_present {
            continue;
        }

        let window_surfaces = window_surfaces.deref_mut();
        let Some(surface_data) = window_surfaces.surfaces.get(&window.entity) else {
            continue;
        };

        // M1-3: the swap chain's current back-buffer RTV is fetched fresh
        // every frame (the pointer flips per `Present` on D3D12) and
        // registered under the transition dummy view's address, so the
        // render-pass path (`RenderContext::begin_tracked_render_pass`)
        // resolves it like any other attachment. The wgpu-side consumers of
        // the swap-chain view (raw-encoder passes, screenshots) keep using
        // the dummy texture (TODO-REMOVE-M1-4).
        let Some(rtv) = surface_data.swap_chain.current_back_buffer_rtv() else {
            // D3D12/Vulkan always have a back buffer; only the OpenGL
            // backend returns null here.
            bevy_log::debug!(
                "diligent: swap chain has no current back-buffer RTV; skipping window {:?}",
                window.entity
            );
            continue;
        };
        let view = surface_data.swap_chain_texture_view.clone();
        crate::renderer::diligent_registry::registry().register_texture_view(view.id(), rtv.as_ptr());
        window.set_swapchain_texture(view);
        window.swap_chain_texture_view_format = Some(
            surface_data
                .texture_view_format
                .unwrap_or(surface_data.configuration.format),
        );
        window.swap_chain_texture_format = Some(surface_data.configuration.format);
    }
}

pub fn need_surface_configuration(
    windows: Res<ExtractedWindows>,
    window_surfaces: Res<WindowSurfaces>,
) -> bool {
    for window in windows.windows.values() {
        if !window_surfaces.configured_windows.contains(&window.entity)
            || window.size_changed
            || window.present_mode_changed
        {
            return true;
        }
    }
    false
}

// 2 is wgpu's default/what we've been using so far.
// 1 is the minimum, but may cause lower framerates due to the cpu waiting for the gpu to finish
// all work for the previous frame before starting work on the next frame, which then means the gpu
// has to wait for the cpu to finish to start on the next frame.
const DEFAULT_DESIRED_MAXIMUM_FRAME_LATENCY: u32 = 2;

/// Creates window surfaces.
pub fn create_surfaces(
    // By accessing a NonSend resource, we tell the scheduler to put this system on the main thread,
    // which is necessary for some OS's
    #[cfg(any(target_os = "macos", target_os = "ios"))] _marker: bevy_ecs::system::NonSendMarker,
    mut windows: ResMut<ExtractedWindows>,
    mut window_surfaces: ResMut<WindowSurfaces>,
    render_device: Res<RenderDevice>,
) {
    for window in windows.windows.values_mut() {
        let window_surfaces = window_surfaces.deref_mut();
        let data = match window_surfaces.surfaces.entry(window.entity) {
            bevy_platform::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            bevy_platform::collections::hash_map::Entry::Vacant(entry) => {
                match create_surface_data(&render_device, window) {
                    Some(data) => entry.insert(data),
                    None => {
                        // Mark the window configured so a failed creation
                        // (e.g. no diligent device) is not retried and
                        // warned about every frame.
                        window_surfaces.configured_windows.insert(window.entity);
                        continue;
                    }
                }
            }
        };

        if window.size_changed || window.present_mode_changed {
            // normally this is dropped on present but we double check here to be safe as failure to
            // drop it will cause validation errors in wgpu
                #[cfg_attr(
                target_arch = "wasm32",
                expect(clippy::drop_non_drop, reason = "texture views are not drop on wasm")
            )]
            drop(window.swap_chain_texture_view.take());

            data.configuration.width = window.physical_width;
            data.configuration.height = window.physical_height;
            data.configuration.present_mode = window.present_mode;
            if let Err(err) = data
                .swap_chain
                .resize(window.physical_width, window.physical_height)
            {
                warn!("diligent: swap chain resize failed: {err}");
            }
            // The cached framebuffers reference the old back-buffer views.
            render_device.invalidate_render_passes();
            // Recreate the transition dummy texture at the new size
            // (TODO-REMOVE-M1-4).
            data.swap_chain_texture_view =
                create_transition_texture_view(&render_device, &data.configuration);
        }

        window_surfaces.configured_windows.insert(window.entity);
    }
}

/// Creates the diligent swap chain for one window (M1-3).
///
/// Returns `None` (with a warning) when the diligent device/factory is
/// unavailable or the window handle is not a Win32 window (the M1 target is
/// D3D12/Windows - TODO-REMOVE-M1-4).
fn create_surface_data(render_device: &RenderDevice, window: &ExtractedWindow) -> Option<SurfaceData> {
    let factory = render_device.engine_factory()?;
    let device = render_device.diligent_device()?;
    let context = render_device.diligent_context()?;
    let hwnd = window_hwnd(&window.handle)?;

    let swap_chain = match factory.create_swap_chain(
        device,
        context,
        hwnd,
        window.physical_width,
        window.physical_height,
    ) {
        Ok(swap_chain) => swap_chain,
        Err(err) => {
            warn!(
                "diligent: swap chain creation failed for window {:?}: {err}",
                window.entity
            );
            return None;
        }
    };
    let latency = window
        .desired_maximum_frame_latency
        .map(NonZero::<u32>::get)
        .unwrap_or(DEFAULT_DESIRED_MAXIMUM_FRAME_LATENCY);
    info!("Created Diligent swap chain for window {:?}", window.entity);

    // The M1-1 wrapper's swap-chain descriptor fixes the color buffer format
    // to TEX_FORMAT_RGBA8_UNORM_SRGB (desc::swap_chain).
    let format = TextureFormat::Rgba8UnormSrgb;
    let configuration = SurfaceConfiguration {
        format,
        width: window.physical_width,
        height: window.physical_height,
        present_mode: window.present_mode,
        desired_maximum_frame_latency: latency,
        alpha_mode: window.alpha_mode,
        view_formats: vec![],
    };
    let texture_view_format = if !format.is_srgb() {
        Some(format.add_srgb_suffix())
    } else {
        None
    };
    let swap_chain_texture_view = create_transition_texture_view(render_device, &configuration);

    // Apply the frame-latency setting from the configuration (SwapChain.h:99
    // - D3D11/D3D12 only; set once at creation).
    diligent_draw::set_maximum_frame_latency(
        &swap_chain,
        configuration.desired_maximum_frame_latency,
    );

    Some(SurfaceData {
        swap_chain: DiligentHandle::new(alloc::sync::Arc::new(swap_chain)),
        swap_chain_texture_view,
        configuration,
        texture_view_format,
    })
}

/// The Win32 HWND of a window, when the window is a Win32 window.
fn window_hwnd(handle: &RawHandleWrapper) -> Option<*mut c_void> {
    match handle.get_window_handle() {
        raw_window_handle::RawWindowHandle::Win32(win32) => {
            Some(win32.hwnd.get() as *mut c_void)
        }
        _ => {
            warn!(
                "diligent: swap chains are only supported for Win32 windows so far; \
                 this window will not render (TODO-REMOVE-M1-4)"
            );
            None
        }
    }
}

/// The swap-chain texture view handle (M1-4b-2: the transition wgpu dummy
/// texture is gone - the handle carries no diligent view of its own; the
/// per-frame back-buffer RTV is registered under this view's id by
/// `prepare_windows`).
fn create_transition_texture_view(
    _render_device: &RenderDevice,
    configuration: &SurfaceConfiguration,
) -> TextureView {
    let id = crate::render_resource::TextureViewId::new();
    TextureView {
        id,
        inner: alloc::sync::Arc::new(WgpuTextureView {
            id,
            value: None,
            format: configuration.format,
            size: wgpu_types::Extent3d {
                width: configuration.width,
                height: configuration.height,
                depth_or_array_layers: 1,
            },
            dimension: wgpu_types::TextureViewDimension::D2,
        }),
    }
}
