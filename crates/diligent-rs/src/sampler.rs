//! Sampler wrapper (`ISampler`).

use diligent_sys::bindings as sys;

use crate::handle::{impl_shared_ownership, Handle};

/// Owning handle to a sampler state object (`ISampler`).
///
/// Samplers are created by the device
/// ([`crate::device::RenderDevice::create_sampler`]) and are assigned to
/// texture views (`ITextureView::SetSampler`) - in Diligent the sampler
/// lives on the *view*, unlike wgpu where it lives in the bind group layout.
pub struct Sampler {
    handle: Handle<sys::ISampler>,
}

impl_shared_ownership!(Sampler, sys::ISampler);

impl Sampler {
    /// Wraps a sampler pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `ISampler` instance returned by the engine (which
    /// AddRefs it); ownership is transferred to the wrapper, which releases
    /// it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::ISampler) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw sampler pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::ISampler {
        self.handle.as_ptr()
    }
}
