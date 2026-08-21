use bevy_app::AppExit;
use bevy_ecs::{
    resource::Resource,
    world::{Mut, World},
};
pub use wgpu_types::error::ErrorType;

use crate::{
    insert_future_resources,
    render_resource::PipelineCache,
    renderer::{RenderDevice, WgpuWrapper},
    settings::RenderCreation,
    FutureRenderResources, RenderStartup,
};

/// The source of a rendering error (the wgpu `ErrorSource` shape: a boxed
/// error).
pub type ErrorSource = Box<dyn core::error::Error + Send + Sync + 'static>;

/// Resource to indicate renderer behavior upon error.
pub enum RenderErrorPolicy {
    /// Pretends nothing happened and continues rendering.
    /// This discards the error after logging it to console.
    /// WARNING: Using this policy could cause hazardous rapid flashing
    /// if the conditions causing the error remain unaddressed, since
    /// rendering will attempt to continue executing.
    /// When choosing to use this policy, be sure to test that the application
    /// remains safe to use.
    Ignore,
    /// Keeps the app alive, but stops rendering further.
    /// This keeps the error state, and will continue polling the [`RenderErrorHandler`]
    /// every frame until some other policy is returned.
    StopRendering,
    /// Attempt renderer recovery with the given [`RenderCreation`].
    Recover(RenderCreation),
}

/// Determines what [`RenderErrorPolicy`] should be used to respond to a given [`RenderError`].
///
/// The handler has access to both the main world and the render world in that order.
/// By the time this is invoked, the error has already been logged. The error is provided
/// for the decision-making reason of how to appropriately respond to it.
///
/// Note that failing to address the source of an error and continuing to render may cause rapid flashing.
/// Be sure to thoroughly test your error handler to ensure you application remains safe
/// to use.
#[derive(Resource)]
pub struct RenderErrorHandler(
    pub for<'a> fn(&'a RenderError, &'a mut World, &'a mut World) -> RenderErrorPolicy,
);

impl RenderErrorHandler {
    fn handle(&self, error: &RenderError, main_world: &mut World, render_world: &mut World) {
        match self.0(error, main_world, render_world) {
            RenderErrorPolicy::Ignore => {
                // Pretend that didn't happen.
                render_world.insert_resource(RenderState::Ready);
            }
            RenderErrorPolicy::StopRendering => {
                // do nothing
            }
            RenderErrorPolicy::Recover(render_creation) => {
                assert!(insert_future_resources(&render_creation, main_world));
                render_world.insert_resource(RenderState::Reinitializing);
            }
        }
    }
}

impl Default for RenderErrorHandler {
    fn default() -> Self {
        // Quit the application for any RenderError. This is overzealous at the moment,
        // but requires more extensive use of the non-fatal error handling pattern in
        // upstream wgpu. RenderErrors are issued when wgpu is used incorrectly.
        // Ignoring a wgpu OutOfMemory or Validation error without addressing the
        // root cause (via hiding or deleting entities or changing rendering settings) will
        // likely hit the same error repeatedly, resulting in hazardous strobing effects.
        // The parameters to this function are (error, main_world, render_world).
        Self(|error, main_world, _| {
            bevy_log::error!("Quitting the application due to {:?} RenderError", error.ty);
            main_world.write_message(AppExit::error());
            RenderErrorPolicy::StopRendering
        })
    }
}

/// An error encountered during rendering. These are errors reported by wgpu validation layers,
/// and typically indicate problems in the way it is being used.
#[derive(Debug)]
pub struct RenderError {
    pub ty: ErrorType,
    pub description: String,
    pub source: Option<WgpuWrapper<ErrorSource>>,
}

/// The current state of the renderer.
#[derive(Resource, Debug)]
pub(crate) enum RenderState {
    /// Just started, [`crate::RenderStartup`] will run in this state.
    Initializing,
    /// Everything is okay and we are rendering stuff every frame.
    Ready,
    /// An error was encountered, and we may decide how to handle it.
    Errored(RenderError),
    /// We are recreating the render context after an error to recover.
    Reinitializing,
}

/// Resource to allow polling render error handlers.
///
/// V13 (M1-5): the wgpu device-lost callbacks are gone; the diligent device
/// removal channel on D3D12 is the fence: `IFence::GetCompletedValue()`
/// returns `UINT64_MAX` when the device has been removed (FenceD3D12Impl.cpp:
/// "If the device has been removed, the return value will be UINT64_MAX").
/// `poll()` checks a dedicated fence once per frame; a removed device
/// surfaces as a `RenderError` of type [`ErrorType::DeviceLost`], which the
/// existing `RenderErrorHandler` machinery maps to the configured policy
/// (`StopRendering` is the default - error -> stop rendering -> exit).
/// `Recover` remains a P3 item (the diligent device has no in-engine
/// re-initialization hook yet).
#[derive(Resource)]
pub(crate) struct DeviceErrorHandler {
    /// The fence polled for device-removal detection (D3D12). `None` when no
    /// diligent device is available (headless / fallback paths).
    fence: Option<crate::renderer::diligent_registry::DiligentHandle<diligent_rs::Fence>>,
}

impl DeviceErrorHandler {
    /// Creates the handler and its device-removal probe fence (V13).
    pub(crate) fn new(device: &RenderDevice) -> Self {
        let fence = device.diligent_device().and_then(|d| {
            match d.create_fence("bevy_device_error_poll_fence") {
                Ok(fence) => {
                    Some(crate::renderer::diligent_registry::DiligentHandle::new(
                        alloc::sync::Arc::new(fence),
                    ))
                }
                Err(err) => {
                    bevy_log::warn!("diligent: device-error probe fence creation failed: {err}");
                    None
                }
            }
        });
        Self { fence }
    }

    /// Checks for device-removal and returns a [`RenderError`] when the GPU
    /// went away.
    ///
    /// V13 (D3D12): `GetCompletedValue() == u64::MAX` marks a removed device
    /// (FenceD3D12Impl.cpp:79; the value is also what `Fence::Wait` sees).
    /// `Err` (interface failure) is treated as fatal too. Vulkan device-lost
    /// has no `UINT64_MAX` contract in this locked version (FenceVkImpl
    /// returns the semaphore counter); it surfaces through the engine
    /// message callback - recorded as a P2 gap in the M1-5 report.
    pub(crate) fn poll(&self) -> Option<RenderError> {
        let Some(fence) = &self.fence else {
            return None;
        };
        match fence.get_completed_value() {
            Ok(value) if value == u64::MAX => Some(RenderError {
                ty: ErrorType::DeviceLost,
                description: "Diligent D3D12 device removed: \
                               IFence::GetCompletedValue returned UINT64_MAX"
                    .to_string(),
                source: None,
            }),
            Ok(value) => {
                // TEMP-DIAG-M2A2: log the probe value (device-removal false
                // positive bisection).
                if value != 0 {
                    bevy_log::debug!(
                        "diligent: device-error probe fence completed value = {value}"
                    );
                }
                None
            }
            // Conservative mapping (V13, Fix round 1): EVERY fence-call
            // failure maps to DeviceLost - including MissingMethod (an
            // interface/ABI integrity symptom rather than a lost device).
            // Deliberately not discriminated: a broken probe should trip
            // the error state machine instead of being silently ignored.
            // (The `u64::MAX` branch above is untestable without a real
            // removed device; the Err branch is its reachable proxy.)
            Err(err) => Some(RenderError {
                ty: ErrorType::DeviceLost,
                description: format!(
                    "Diligent device-removal probe failed (fence poll): {err}"
                ),
                source: None,
            }),
        }
    }
}

/// Updates the state machine that handles the renderer and device lifecycle.
/// Polls the [`DeviceErrorHandler`] and fires the [`RenderErrorHandler`] if needed.
///
/// Runs [`crate::RenderStartup`] after every time a [`RenderDevice`] is acquired.
///
/// We need both the main and render world to properly handle errors, so we wedge ourselves into [extract](bevy_app::SubApp::set_extract).
pub(crate) fn update_state(main_world: &mut World, render_world: &mut World) {
    if let Some(error) = render_world.resource::<DeviceErrorHandler>().poll() {
        render_world.insert_resource(RenderState::Errored(error));
    };

    // Remove the render state so we can provide both worlds to the `RenderErrorHandler`.
    let state = render_world.remove_resource::<RenderState>().unwrap();

    match &state {
        RenderState::Initializing => {
            render_world.run_schedule(RenderStartup);
            render_world.insert_resource(RenderState::Ready);
        }
        RenderState::Ready => {
            // all is well
        }
        RenderState::Errored(error) => {
            main_world.resource_scope(|main_world, error_handler: Mut<RenderErrorHandler>| {
                error_handler.handle(error, main_world, render_world);
            });
        }
        RenderState::Reinitializing => {
            if let Some(render_resources) = main_world
                .get_resource::<FutureRenderResources>()
                .unwrap()
                .clone()
                .lock()
                .unwrap()
                .take()
            {
                let synchronous_pipeline_compilation = render_world
                    .resource::<PipelineCache>()
                    .synchronous_pipeline_compilation;
                render_resources.unpack_into(
                    main_world,
                    render_world,
                    synchronous_pipeline_compilation,
                );
                render_world.insert_resource(RenderState::Initializing);
            }
        }
    }

    // Put the state back if we didn't set a new one
    if render_world.get_resource::<RenderState>().is_none() {
        render_world.insert_resource(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a diligent device the handler polls nothing (headless path).
    #[test]
    fn poll_without_probe_fence_reports_nothing() {
        let handler = DeviceErrorHandler { fence: None };
        assert!(handler.poll().is_none());
    }
}
