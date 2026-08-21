use crate::renderer::{RenderAdapterInfo, RenderDevice, RenderQueue};
use tracy_client::GpuContext;

/// Creates a tracy GPU context for the render queue.
///
/// M1-4b-2: the diligent path has no timestamp queries, so no tracy GPU
/// context can be created (the CPU-side tracy spans still work).
pub fn new_tracy_gpu_context(
    _adapter_info: &RenderAdapterInfo,
    _device: &RenderDevice,
    _queue: &RenderQueue,
) -> Option<GpuContext> {
    None
}
