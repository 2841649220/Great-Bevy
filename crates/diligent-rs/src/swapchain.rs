//! Swap chain: presentation, back-buffer access, resizing.

use diligent_sys::bindings as sys;

use crate::error::{Error, Result};
use crate::handle::{Handle, NonOwning};

/// Owning handle to the swap chain (`ISwapChain`).
pub struct SwapChain {
    handle: Handle<sys::ISwapChain>,
}

impl SwapChain {
    pub(crate) fn from_raw(ptr: *mut sys::ISwapChain) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw swap chain pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::ISwapChain {
        self.handle.as_ptr()
    }

    fn vtbl(&self) -> &sys::ISwapChainMethods {
        unsafe { &(*(*self.as_raw()).pVtbl).SwapChain }
    }

    /// The swap chain description (actual values after creation, e.g. the
    /// effective pre-transform and buffer count). Copied out.
    pub fn desc(&self) -> sys::SwapChainDesc {
        let get = self
            .vtbl()
            .GetDesc
            .expect("diligent-rs: ISwapChain::GetDesc missing from vtable");
        // Safety: the engine returns a pointer to internal storage that is
        // valid while the swap chain is alive; we copy the value out.
        unsafe { *get(self.as_raw()) }
    }

    /// The current back-buffer render target view (borrowed - do not
    /// release). The pointer flips every `Present` call for D3D12, so fetch
    /// it fresh each frame.
    pub fn current_back_buffer_rtv(&self) -> Option<NonOwning<sys::ITextureView>> {
        let get = self
            .vtbl()
            .GetCurrentBackBufferRTV
            .expect("diligent-rs: ISwapChain::GetCurrentBackBufferRTV missing from vtable");
        // Safety: the returned view is owned by the swap chain (no refcount
        // increment), which is held by this handle.
        NonOwning::from_raw_opt(unsafe { get(self.as_raw()) })
    }

    /// The swap chain's depth-stencil view (borrowed - do not release).
    pub fn depth_dsv(&self) -> Option<NonOwning<sys::ITextureView>> {
        let get = self
            .vtbl()
            .GetDepthBufferDSV
            .expect("diligent-rs: ISwapChain::GetDepthBufferDSV missing from vtable");
        NonOwning::from_raw_opt(unsafe { get(self.as_raw()) })
    }

    /// Presents the frame. `sync_interval` 0 = no vsync, 1 = vsync.
    pub fn present(&self, sync_interval: u32) {
        let present = self
            .vtbl()
            .Present
            .expect("diligent-rs: ISwapChain::Present missing from vtable");
        // Safety: no arguments beyond the swap chain itself.
        unsafe { present(self.as_raw(), sync_interval) };
    }

    /// Resizes the swap chain to the new window size (D3D12 requires a full
    /// resize path; the engine picks the optimal surface transform).
    pub fn resize(&self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "swap chain dimensions must be > 0",
            ));
        }
        let resize = self
            .vtbl()
            .Resize
            .expect("diligent-rs: ISwapChain::Resize missing from vtable");
        // Safety: no pointers involved.
        unsafe {
            resize(
                self.as_raw(),
                width,
                height,
                sys::_SURFACE_TRANSFORM::SURFACE_TRANSFORM_OPTIMAL as sys::SURFACE_TRANSFORM,
            )
        };
        Ok(())
    }
}
