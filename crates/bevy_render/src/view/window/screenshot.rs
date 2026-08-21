use super::ExtractedWindows;
use crate::{
    render_asset::RenderAssets,
    render_phase::TrackedRenderPass,
    render_resource::{
        BindGroup, BindGroupEntries, CachedRenderPipelineId, PipelineCache,
        RenderPipeline, SpecializedRenderPipeline, SpecializedRenderPipelines, Texture,
        TextureUsages, TextureView,
    },
    renderer::{
        diligent_draw,
        diligent_registry::DiligentHandle,
        RenderDevice,
    },
    texture::{GpuImage, ManualTextureViews, OutputColorAttachment},
    view::{prepare_view_attachments, prepare_view_targets, ViewTargetAttachments, WindowSurfaces},
    ExtractSchedule, GpuResourceAppExt, MainWorld, Render, RenderApp, RenderStartup, RenderSystems,
};
use alloc::{borrow::Cow, sync::Arc};
use bevy_app::{First, Plugin, Update};
use bevy_asset::{embedded_asset, load_embedded_asset, AssetServer, Handle, RenderAssetUsages};
use bevy_camera::{ManualTextureViewHandle, NormalizedRenderTarget, RenderTarget};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    entity::EntityHashMap, message::message_update_system, prelude::*, system::SystemState,
};
use bevy_image::{Image, TextureFormatPixelInfo, ToExtents};
use bevy_log::{debug, error, info, warn};
use bevy_material::{
    bind_group_layout_entries::{binding_types::texture_2d, BindGroupLayoutEntries},
    descriptor::{
        BindGroupLayoutDescriptor, FragmentState, RenderPipelineDescriptor, VertexState,
    },
};
use bevy_platform::collections::HashSet;
use bevy_reflect::Reflect;
use bevy_shader::Shader;
use bevy_tasks::AsyncComputeTaskPool;
use bevy_utils::default;
use bevy_window::{PrimaryWindow, WindowRef};
use core::ops::Deref;
use std::{
    path::Path,
    sync::{
        mpsc::{Receiver, Sender},
        Mutex,
    },
};
use wgpu_types::{Extent3d, TextureFormat};

#[derive(EntityEvent, Reflect, Deref, DerefMut, Debug)]
#[reflect(Debug, Event)]
pub struct ScreenshotCaptured {
    pub entity: Entity,
    #[deref]
    pub image: Image,
}

/// A component that signals to the renderer to capture a screenshot this frame.
///
/// This component should be spawned on a new entity with an observer that will trigger
/// with [`ScreenshotCaptured`] when the screenshot is ready.
///
/// Screenshots are captured asynchronously and may not be available immediately after the frame
/// that the component is spawned on. The observer should be used to handle the screenshot when it
/// is ready.
///
/// Note that the screenshot entity will be despawned after the screenshot is captured and the
/// observer is triggered.
///
/// # Usage
///
/// ```
/// # use bevy_ecs::prelude::*;
/// # use bevy_render::view::screenshot::{save_to_disk, Screenshot};
///
/// fn take_screenshot(mut commands: Commands) {
///    commands.spawn(Screenshot::primary_window())
///       .observe(save_to_disk("screenshot.png"));
/// }
/// ```
#[derive(Component, Deref, DerefMut, Reflect, Debug)]
#[reflect(Component, Debug)]
pub struct Screenshot(pub RenderTarget);

/// A marker component that indicates that a screenshot is currently being captured.
#[derive(Component, Default)]
pub struct Capturing;

/// A marker component that indicates that a screenshot has been captured, the image is ready, and
/// the screenshot entity can be despawned.
#[derive(Component, Default)]
pub struct Captured;

impl Screenshot {
    /// Capture a screenshot of the provided window entity.
    pub fn window(window: Entity) -> Self {
        Self(RenderTarget::Window(WindowRef::Entity(window)))
    }

    /// Capture a screenshot of the primary window, if one exists.
    pub fn primary_window() -> Self {
        Self(RenderTarget::Window(WindowRef::Primary))
    }

    /// Capture a screenshot of the provided render target image.
    pub fn image(image: Handle<Image>) -> Self {
        Self(RenderTarget::Image(image.into()))
    }

    /// Capture a screenshot of the provided manual texture view.
    pub fn texture_view(texture_view: ManualTextureViewHandle) -> Self {
        Self(RenderTarget::TextureView(texture_view))
    }
}

struct ScreenshotPreparedState {
    pub texture: Texture,
    /// The diligent staging texture the capture target is copied into for
    /// readback (M1-4b-1: `None` when the diligent device is unavailable -
    /// the screenshot then never completes, matching the wgpu-only
    /// fallback behavior of the pre-swap transition path).
    pub staging: Option<DiligentHandle<diligent_rs::Texture>>,
    pub bind_group: BindGroup,
    pub pipeline_id: CachedRenderPipelineId,
    pub size: Extent3d,
    /// The fence value of the pending copy (0 = no copy in flight). The
    /// cross-frame readback pattern (方案 A): the copy is re-issued and
    /// re-signaled every frame until the fence reports the value completed;
    /// after `SYNC_FALLBACK_FRAMES` pending frames the wait becomes
    /// blocking (方案 C).
    pending_fence_value: u64,
    pending_frames: u32,
}

/// The cross-frame readback fence shared by all prepared screenshots
/// (M1-4b-1 方案 A): every per-frame copy signals the fence with a
/// monotonically increasing value, and `collect_screenshots` polls
/// `get_completed_value`.
#[derive(Resource, Default)]
struct ScreenshotReadbackFence {
    fence: Option<DiligentHandle<diligent_rs::Fence>>,
    next_value: u64,
}

/// Pending frames before the readback falls back to a blocking fence wait
/// (方案 C).
const SYNC_FALLBACK_FRAMES: u32 = 8;

#[derive(Resource, Deref, DerefMut)]
pub struct CapturedScreenshots(pub Arc<Mutex<Receiver<(Entity, Image)>>>);

#[derive(Resource, Deref, DerefMut, Default)]
struct RenderScreenshotTargets(EntityHashMap<NormalizedRenderTarget>);

#[derive(Resource, Deref, DerefMut, Default)]
struct RenderScreenshotsPrepared(EntityHashMap<ScreenshotPreparedState>);

#[derive(Resource, Deref, DerefMut)]
struct RenderScreenshotsSender(Sender<(Entity, Image)>);

/// Saves the captured screenshot to disk at the provided path.
pub fn save_to_disk(path: impl AsRef<Path>) -> impl FnMut(On<ScreenshotCaptured>) {
    let path = path.as_ref().to_owned();
    move |screenshot_captured| {
        let img = screenshot_captured.image.clone();
        match img.try_into_dynamic() {
            Ok(dyn_img) => match image::ImageFormat::from_path(&path) {
                Ok(format) => {
                    // discard the alpha channel which stores brightness values when HDR is enabled to make sure
                    // the screenshot looks right
                    let img = dyn_img.to_rgb8();
                    #[cfg(not(target_arch = "wasm32"))]
                    match img.save_with_format(&path, format) {
                        Ok(_) => info!("Screenshot saved to {}", path.display()),
                        Err(e) => error!("Cannot save screenshot, IO error: {e}"),
                    }

                    #[cfg(target_arch = "wasm32")]
                    {
                        let save_screenshot = || {
                            use image::EncodableLayout;
                            use wasm_bindgen::{JsCast, JsValue};

                            let mut image_buffer = std::io::Cursor::new(Vec::new());
                            img.write_to(&mut image_buffer, format)
                                .map_err(|e| JsValue::from_str(&format!("{e}")))?;

                            let parts = js_sys::Array::of1(
                                &js_sys::Uint8Array::new_from_slice(
                                    image_buffer.into_inner().as_bytes(),
                                )
                                .into(),
                            );
                            let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)?;
                            let url = web_sys::Url::create_object_url_with_blob(&blob)?;
                            let window = web_sys::window().unwrap();
                            let document = window.document().unwrap();
                            let link = document.create_element("a")?;
                            link.set_attribute("href", &url)?;
                            link.set_attribute(
                                "download",
                                path.file_name()
                                    .and_then(|filename| filename.to_str())
                                    .ok_or_else(|| JsValue::from_str("Invalid filename"))?,
                            )?;
                            let html_element = link.dyn_into::<web_sys::HtmlElement>()?;
                            html_element.click();
                            web_sys::Url::revoke_object_url(&url)?;
                            Ok::<(), JsValue>(())
                        };

                        match (save_screenshot)() {
                            Ok(_) => info!("Screenshot saved to {}", path.display()),
                            Err(e) => error!("Cannot save screenshot, error: {e:?}"),
                        };
                    }
                }
                Err(e) => error!("Cannot save screenshot, requested format not recognized: {e}"),
            },
            Err(e) => error!("Cannot save screenshot, screen format cannot be understood: {e}"),
        }
    }
}

fn clear_screenshots(mut commands: Commands, screenshots: Query<Entity, With<Captured>>) {
    for entity in screenshots.iter() {
        commands.entity(entity).despawn();
    }
}

pub fn trigger_screenshots(
    mut commands: Commands,
    captured_screenshots: ResMut<CapturedScreenshots>,
) {
    let captured_screenshots = captured_screenshots.lock().unwrap();
    while let Ok((entity, image)) = captured_screenshots.try_recv() {
        commands.entity(entity).insert(Captured);
        commands.trigger(ScreenshotCaptured { image, entity });
    }
}

fn extract_screenshots(
    mut targets: ResMut<RenderScreenshotTargets>,
    mut main_world: ResMut<MainWorld>,
    mut system_state: Local<
        Option<
            SystemState<(
                Commands,
                Query<Entity, With<PrimaryWindow>>,
                Query<(Entity, &Screenshot), Without<Capturing>>,
            )>,
        >,
    >,
    mut seen_targets: Local<HashSet<NormalizedRenderTarget>>,
) {
    if system_state.is_none() {
        *system_state = Some(SystemState::new(&mut main_world));
    }
    let system_state = system_state.as_mut().unwrap();
    let (mut commands, primary_window, screenshots) =
        system_state.get_mut(&mut main_world).unwrap();

    targets.clear();
    seen_targets.clear();

    let primary_window = primary_window.iter().next();

    for (entity, screenshot) in screenshots.iter() {
        let render_target = screenshot.0.clone();
        let Some(render_target) = render_target.normalize(primary_window) else {
            warn!(
                "Unknown render target for screenshot, skipping: {:?}",
                render_target
            );
            continue;
        };
        if seen_targets.contains(&render_target) {
            warn!(
                "Duplicate render target for screenshot, skipping entity {}: {:?}",
                entity, render_target
            );
            // If we don't despawn the entity here, it will be captured again in the next frame
            commands.entity(entity).despawn();
            continue;
        }
        seen_targets.insert(render_target.clone());
        targets.insert(entity, render_target);
        commands.entity(entity).insert(Capturing);
    }

    system_state.apply(&mut main_world);
}

fn prepare_screenshots(
    targets: Res<RenderScreenshotTargets>,
    mut prepared: ResMut<RenderScreenshotsPrepared>,
    window_surfaces: Res<WindowSurfaces>,
    render_device: Res<RenderDevice>,
    screenshot_pipeline: Res<ScreenshotToScreenPipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<ScreenshotToScreenPipeline>>,
    images: Res<RenderAssets<GpuImage>>,
    manual_texture_views: Res<ManualTextureViews>,
    mut view_target_attachments: ResMut<ViewTargetAttachments>,
) {
    prepared.clear();
    for (entity, target) in targets.iter() {
        match target {
            NormalizedRenderTarget::Window(window) => {
                let window = window.entity();
                let Some(surface_data) = window_surfaces.surfaces.get(&window) else {
                    warn!("Unknown window for screenshot, skipping: {}", window);
                    continue;
                };
                let view_format = surface_data
                    .texture_view_format
                    .unwrap_or(surface_data.configuration.format);
                let size = Extent3d {
                    width: surface_data.configuration.width,
                    height: surface_data.configuration.height,
                    ..default()
                };
                let (texture_view, state) = prepare_screenshot_state(
                    size,
                    view_format,
                    &render_device,
                    &screenshot_pipeline,
                    &pipeline_cache,
                    &mut pipelines,
                );
                prepared.insert(*entity, state);
                view_target_attachments.insert(
                    target.clone(),
                    OutputColorAttachment::new(texture_view.clone(), view_format),
                );
            }
            NormalizedRenderTarget::Image(image) => {
                let Some(gpu_image) = images.get(&image.handle) else {
                    warn!("Unknown image for screenshot, skipping: {:?}", image);
                    continue;
                };
                let view_format = gpu_image.view_format();
                let (texture_view, state) = prepare_screenshot_state(
                    gpu_image.texture_descriptor.size,
                    view_format,
                    &render_device,
                    &screenshot_pipeline,
                    &pipeline_cache,
                    &mut pipelines,
                );
                prepared.insert(*entity, state);
                view_target_attachments.insert(
                    target.clone(),
                    OutputColorAttachment::new(texture_view.clone(), view_format),
                );
            }
            NormalizedRenderTarget::TextureView(texture_view) => {
                let Some(manual_texture_view) = manual_texture_views.get(texture_view) else {
                    warn!(
                        "Unknown manual texture view for screenshot, skipping: {:?}",
                        texture_view
                    );
                    continue;
                };
                let view_format = manual_texture_view.view_format;
                let size = manual_texture_view.size.to_extents();
                let (texture_view, state) = prepare_screenshot_state(
                    size,
                    view_format,
                    &render_device,
                    &screenshot_pipeline,
                    &pipeline_cache,
                    &mut pipelines,
                );
                prepared.insert(*entity, state);
                view_target_attachments.insert(
                    target.clone(),
                    OutputColorAttachment::new(texture_view.clone(), view_format),
                );
            }
            NormalizedRenderTarget::None { .. } => {
                // Nothing to screenshot!
            }
        }
    }
}

fn prepare_screenshot_state(
    size: Extent3d,
    format: TextureFormat,
    render_device: &RenderDevice,
    pipeline: &ScreenshotToScreenPipeline,
    pipeline_cache: &PipelineCache,
    pipelines: &mut SpecializedRenderPipelines<ScreenshotToScreenPipeline>,
) -> (TextureView, ScreenshotPreparedState) {
    let texture = render_device.create_texture(&crate::render_resource::TextureDescriptor {
        label: Some("screenshot-capture-rendertarget"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu_types::TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::COPY_SRC
            | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let texture_view = texture.create_view(&Default::default());
    // M1-4b-1: the staging side of the readback (CopyTexture + Map reads it
    // back; the Diligent copy API has no texture-to-buffer direction, so the
    // staging target is a texture of the same format and size).
    let staging = render_device.diligent_device().and_then(|device| {
        let format = match diligent_rs::format::to_diligent(format) {
            Ok(format) => format,
            Err(err) => {
                warn!("diligent: screenshot staging texture format: {err}");
                return None;
            }
        };
        match device.create_staging_texture("bevy_screenshot_staging", size.width, size.height, format) {
            Ok(staging) => Some(DiligentHandle::new(Arc::new(staging))),
            Err(err) => {
                warn!("diligent: screenshot staging texture creation failed: {err}");
                None
            }
        }
    });
    let bind_group = render_device.create_bind_group(
        "screenshot-to-screen-bind-group",
        &pipeline_cache.get_bind_group_layout(&pipeline.bind_group_layout),
        &BindGroupEntries::single(&texture_view),
    );
    let pipeline_id = pipelines.specialize(pipeline_cache, pipeline, format);

    (
        texture_view,
        ScreenshotPreparedState {
            texture,
            staging,
            bind_group,
            pipeline_id,
            size,
            pending_fence_value: 0,
            pending_frames: 0,
        },
    )
}

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut bevy_app::App) {
        embedded_asset!(app, "screenshot.wgsl");

        let (tx, rx) = std::sync::mpsc::channel();
        app.register_type::<Screenshot>()
            .register_type::<ScreenshotCaptured>()
            .insert_resource(CapturedScreenshots(Arc::new(Mutex::new(rx))))
            .add_systems(
                First,
                clear_screenshots
                    .after(message_update_system)
                    .before(ApplyDeferred),
            )
            .add_systems(Update, trigger_screenshots);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .insert_resource(RenderScreenshotsSender(tx))
            .init_resource::<RenderScreenshotTargets>()
            .init_resource::<RenderScreenshotsPrepared>()
            .init_resource::<ScreenshotReadbackFence>()
            .init_gpu_resource::<SpecializedRenderPipelines<ScreenshotToScreenPipeline>>()
            .add_systems(RenderStartup, init_screenshot_to_screen_pipeline)
            .add_systems(ExtractSchedule, extract_screenshots.ambiguous_with_all())
            .add_systems(
                Render,
                prepare_screenshots
                    .after(prepare_view_attachments)
                    .before(prepare_view_targets)
                    .in_set(RenderSystems::PrepareViews),
            );
    }
}

#[derive(Resource)]
pub struct ScreenshotToScreenPipeline {
    pub bind_group_layout: BindGroupLayoutDescriptor,
    pub shader: Handle<Shader>,
}

pub fn init_screenshot_to_screen_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    let bind_group_layout = BindGroupLayoutDescriptor::new(
        "screenshot-to-screen-bgl",
        &BindGroupLayoutEntries::single(
            wgpu_types::ShaderStages::FRAGMENT,
            texture_2d(wgpu_types::TextureSampleType::Float { filterable: false }),
        ),
    );

    let shader = load_embedded_asset!(asset_server.as_ref(), "screenshot.wgsl");

    commands.insert_resource(ScreenshotToScreenPipeline {
        bind_group_layout,
        shader,
    });
}

impl SpecializedRenderPipeline for ScreenshotToScreenPipeline {
    type Key = TextureFormat;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some(Cow::Borrowed("screenshot-to-screen")),
            layout: vec![self.bind_group_layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                ..default()
            },
            primitive: wgpu_types::PrimitiveState {
                cull_mode: Some(wgpu_types::Face::Back),
                ..Default::default()
            },
            multisample: Default::default(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                targets: vec![Some(wgpu_types::ColorTargetState {
                    format: key,
                    blend: None,
                    write_mask: wgpu_types::ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }
    }
}

/// Submits the per-frame screenshot commands on the Diligent immediate
/// context (M1-4b-1): the capture-target -> staging copy plus the
/// "screenshot to screen" blit into the target view, and a fence signal for
/// the cross-frame readback (方案 A).
///
/// Called by `renderer::render_system` after the render schedule and before
/// the context flush, so the copies and the blit are recorded after the
/// scene's upscaling pass (same command list, correct ordering).
pub(crate) fn submit_screenshot_commands(
    world: &mut World,
    context: &diligent_rs::DeviceContext,
) {
    // Phase 1: resolve the per-entity capture views and their blit
    // pipelines (immutable world reads only - the borrows must end before
    // `prepared` is borrowed mutably below).
    let render_device = world.resource::<RenderDevice>().clone();
    let targets = world.resource::<RenderScreenshotTargets>();
    let prepared_read = world.resource::<RenderScreenshotsPrepared>();
    let pipelines = world.resource::<PipelineCache>();
    let gpu_images = world.resource::<RenderAssets<GpuImage>>();
    let windows = world.resource::<ExtractedWindows>();
    let manual_texture_views = world.resource::<ManualTextureViews>();

    let mut pending: Vec<(Entity, TextureView, Option<RenderPipeline>)> = Vec::new();
    for (entity, render_target) in targets.iter() {
        let view = match render_target {
            NormalizedRenderTarget::Window(window) => {
                let window = window.entity();
                let Some(window) = windows.get(&window) else {
                    continue;
                };
                let Some(swap_chain_texture_view) = window.swap_chain_texture_view.as_ref() else {
                    continue;
                };
                swap_chain_texture_view.clone()
            }
            NormalizedRenderTarget::Image(image) => {
                let Some(gpu_image) = gpu_images.get(&image.handle) else {
                    warn!("Unknown image for screenshot, skipping: {:?}", image);
                    continue;
                };
                gpu_image.texture_view.clone()
            }
            NormalizedRenderTarget::TextureView(texture_view) => {
                let Some(texture_view) = manual_texture_views.get(texture_view) else {
                    warn!(
                        "Unknown manual texture view for screenshot, skipping: {:?}",
                        texture_view
                    );
                    continue;
                };
                texture_view.texture_view.clone()
            }
            NormalizedRenderTarget::None { .. } => {
                // Nothing to screenshot!
                continue;
            }
        };
        let pipeline = prepared_read
            .get(entity)
            .and_then(|state| pipelines.get_render_pipeline(state.pipeline_id))
            .cloned();
        pending.push((*entity, view, pipeline));
    }

    // Phase 2: the cross-frame readback fence (created lazily on first use;
    // owned clones end the world borrows).
    let fence_handle = world.resource::<ScreenshotReadbackFence>().fence.clone();
    let mut next_value = world.resource::<ScreenshotReadbackFence>().next_value;
    let fence_handle = match fence_handle {
        Some(fence) => fence,
        None => {
            let Some(device) = render_device.diligent_device() else {
                return;
            };
            let fence = match device.create_fence("bevy_screenshot_readback") {
                Ok(fence) => fence,
                Err(err) => {
                    warn!("diligent: screenshot readback fence creation failed: {err}");
                    return;
                }
            };
            let fence = DiligentHandle::new(Arc::new(fence));
            world.resource_mut::<ScreenshotReadbackFence>().fence = Some(fence.clone());
            fence
        }
    };

    // Phase 3: the per-frame copies (mutating the prepared state).
    let mut prepared = world.resource_mut::<RenderScreenshotsPrepared>();
    let mut copied: HashSet<Entity> = HashSet::default();
    for (entity, view, pipeline) in &pending {
        if render_screenshot(
            context,
            &render_device,
            &mut prepared,
            pipeline.as_ref(),
            entity,
            &view,
        ) {
            copied.insert(*entity);
        }
    }

    // Signal the readback fence after the successful copies of this frame
    // (one value per pending screenshot; the signal order matches the copy
    // order). M1-4b-1: a copy that failed is NOT signaled - the fence
    // completing for a copy that never ran would map stale staging content.
    for (entity, state) in prepared.iter_mut() {
        if state.staging.is_some() && copied.contains(entity) {
            next_value += 1;
            state.pending_fence_value = next_value;
            state.pending_frames += 1;
            // M1-4b-2 review, fix 1: `EnqueueSignal` is an
            // immediate-context call - serialize it.
            let _guard = crate::renderer::diligent_registry::context_guard();
            if let Err(err) = context.enqueue_signal(&fence_handle, next_value) {
                warn!("diligent: screenshot readback signal failed: {err}");
                state.pending_fence_value = 0;
            }
        }
    }
    world.resource_mut::<ScreenshotReadbackFence>().next_value = next_value;
}

/// Issues the diligent commands for one prepared screenshot: the
/// capture-target -> staging copy and the "screenshot to screen" blit into
/// `texture_view` (recorded through the diligent render-pass path - the
/// attachment resolves to the real swap-chain RTV).
///
/// `pipeline` was pre-resolved from the pipeline cache (the caller holds the
/// mutable prepared state, so the cache must not be borrowed while it is
/// borrowed).
///
/// Returns `true` iff the capture-target -> staging copy was issued
/// successfully (M1-4b-1: the readback fence must only ever be signaled for
/// a copy that actually ran - a completed fence maps the staging contents,
/// and a failed copy would map stale bytes).
fn render_screenshot(
    context: &diligent_rs::DeviceContext,
    render_device: &RenderDevice,
    prepared: &mut RenderScreenshotsPrepared,
    pipeline: Option<&RenderPipeline>,
    entity: &Entity,
    texture_view: &crate::render_resource::WgpuTextureView,
) -> bool {
    let Some(prepared_state) = &mut prepared.get_mut(entity) else {
        return false;
    };
    // M1-4b-1: capture-target -> staging texture copy (the readback side;
    // the fence signal is guarded on this result by the caller).
    let mut copy_issued = false;
    if let (Some(staging), Some(capture)) =
        (prepared_state.staging.as_deref(), prepared_state.texture.diligent())
    {
        // M1-4b-2 review, fix 1: `CopyTexture` is an immediate-context
        // call - serialize it (scoped: the blit path below takes its own
        // guards via the render-pass helpers).
        let _guard = crate::renderer::diligent_registry::context_guard();
        match context.copy_texture(capture, 0, 0, staging, 0, 0) {
            Ok(()) => copy_issued = true,
            Err(err) => {
                warn!("diligent: screenshot staging copy failed: {err}");
            }
        }
    } else {
        debug!(
            "diligent: screenshot for {:?} has no diligent staging side \
             (fallback device); the capture never completes",
            entity
        );
    }

    // The "screenshot to screen" blit: draw the capture target into the
    // target view through the diligent render-pass path (falls back to
    // nothing when the pass cannot begin - the capture itself is
    // unaffected).
    let Some(pipeline) = pipeline else {
        return copy_issued;
    };
    let pass = crate::render_resource::RenderPassDescriptor {
        label: Some("screenshot_to_screen_pass"),
        color_attachments: &[Some(crate::render_resource::RenderPassColorAttachment {
            view: texture_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu_types::Operations {
                load: wgpu_types::LoadOp::Load,
                store: wgpu_types::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    };
    if diligent_draw::begin_tracked_render_pass(render_device, context, &pass).is_err() {
        return copy_issued;
    }
    let mut tracked = TrackedRenderPass::diligent(render_device, context);
    tracked.set_render_pipeline(pipeline);
    tracked.set_bind_group(0, &prepared_state.bind_group, &[]);
    tracked.draw(0..3, 0..1);
    copy_issued
}

/// Polls the readback fence and maps the completed staging textures
/// (方案 A; blocking fence wait after `SYNC_FALLBACK_FRAMES` - 方案 C).
/// Called by `renderer::render_system` after present.
pub(crate) fn collect_screenshots(world: &mut World) {
    #[cfg(feature = "trace")]
    let _span = bevy_log::info_span!("collect_screenshots").entered();

    let sender = world.resource::<RenderScreenshotsSender>().deref().clone();
    let context = world.resource::<RenderDevice>().diligent_context_handle();
    // Owned clones: the fence borrow must end before `prepared` is borrowed
    // mutably below.
    let (fence_handle, _next_value) = {
        let fence = world.resource::<ScreenshotReadbackFence>();
        (fence.fence.clone(), fence.next_value)
    };

    let Some(context) = context else {
        return;
    };
    let Some(fence_handle) = fence_handle else {
        return;
    };
    let completed = match fence_handle.get_completed_value() {
        Ok(value) => value,
        Err(err) => {
            warn!("diligent: screenshot readback fence query failed: {err}");
            return;
        }
    };
    let mut prepared = world.resource_mut::<RenderScreenshotsPrepared>();

    for (entity, state) in prepared.iter_mut() {
        if state.pending_fence_value == 0 {
            continue;
        }
        let ready = if completed >= state.pending_fence_value {
            true
        } else if state.pending_frames >= SYNC_FALLBACK_FRAMES {
            // 方案 C: blocking wait for the copy.
            fence_handle.wait(state.pending_fence_value).is_ok()
        } else {
            false
        };
        if !ready {
            continue;
        }
        let Some(staging) = state.staging.as_deref() else {
            continue;
        };
        // M1-4b-2 review, fix 1: `MapTextureSubresource` is an
        // immediate-context call - serialize it.
        let _guard = crate::renderer::diligent_registry::context_guard();
        let mapped = match context.map_texture_subresource(
            staging,
            0,
            0,
            diligent_rs::diligent_sys::bindings::_MAP_TYPE::MAP_READ
                as diligent_rs::diligent_sys::bindings::MAP_TYPE,
            true,
        ) {
            Ok(Some(mapped)) => mapped,
            Ok(None) => {
                // The fence reported completion but the map is not visible
                // yet (other backends); retry next frame.
                continue;
            }
            Err(err) => {
                warn!("diligent: screenshot staging map failed: {err}");
                state.pending_fence_value = 0;
                continue;
            }
        };

        let entity = *entity;
        let width = state.size.width;
        let height = state.size.height;
        let texture_format = state.texture.format();
        let Ok(pixel_size) = texture_format.pixel_size() else {
            continue;
        };
        let sender = sender.clone();
        let stride = mapped.stride();
        // Immediately move the data to CPU memory (the map must not be held
        // across frames); the padded-row strip runs in the async task.
        let mut result = Vec::with_capacity(stride * height as usize);
        for row in 0..height {
            result.extend_from_slice(unsafe {
                core::slice::from_raw_parts(mapped.row(row as usize), stride)
            });
        }
        drop(mapped);
        state.pending_fence_value = 0;
        state.pending_frames = 0;

        let finish = async move {
            let initial_row_bytes = width as usize * pixel_size;
            let buffered_row_bytes = stride;
            if buffered_row_bytes != initial_row_bytes {
                let mut take_offset = buffered_row_bytes;
                let mut place_offset = initial_row_bytes;
                for _ in 1..height {
                    result.copy_within(
                        take_offset..take_offset + buffered_row_bytes,
                        place_offset,
                    );
                    take_offset += buffered_row_bytes;
                    place_offset += initial_row_bytes;
                }
                result.truncate(initial_row_bytes * height as usize);
            }

            if let Err(e) = sender.send((
                entity,
                Image::new(
                    Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    wgpu_types::TextureDimension::D2,
                    result,
                    texture_format,
                    RenderAssetUsages::MAIN_WORLD,
                ),
            )) {
                error!("Failed to send screenshot: {}", e);
            }
        };

        AsyncComputeTaskPool::get().spawn(finish).detach();
    }
}
