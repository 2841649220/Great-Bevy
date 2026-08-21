//! Engine factory: entry point, device/context creation, swap chain creation.

use std::ffi::c_void;

use diligent_sys::bindings as sys;

use crate::context::DeviceContext;
use crate::desc;
use crate::device::RenderDevice;
use crate::error::{Error, Result};
use crate::handle::Handle;
use crate::swapchain::SwapChain;

/// Owning handle to the Direct3D12 engine factory
/// (`IEngineFactoryD3D12`, obtained from `Diligent_GetEngineFactoryD3D12`).
///
/// Released (and with it the underlying D3D12 library) on drop.
pub struct EngineFactoryD3D12 {
    handle: Handle<sys::IEngineFactoryD3D12>,
}

impl EngineFactoryD3D12 {
    /// Resolves the engine factory entry point for the D3D12 backend.
    pub fn d3d12() -> Result<Self> {
        let ptr = unsafe { sys::Diligent_GetEngineFactoryD3D12() };
        if ptr.is_null() {
            return Err(Error::CreateFailed("D3D12 engine factory"));
        }
        Ok(Self {
            handle: Handle::from_raw(ptr),
        })
    }

    /// The raw factory pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IEngineFactoryD3D12 {
        self.handle.as_ptr()
    }

    /// Creates the render device and the single immediate context
    /// (D3D12 backend). `EngineCI` defaults to adapter 0, validation off.
    pub fn create_device_and_contexts(&self) -> Result<(RenderDevice, DeviceContext)> {
        self.create_device_and_contexts_with_validation(crate::desc::ValidationLevel::default())
    }

    /// Same as [`create_device_and_contexts`], but with an explicit Diligent
    /// validation level (task 19.3 debug toolchain). Debug/development
    /// builds force validation on via [`crate::desc::ValidationLevel`]; the
    /// caller may pick [`crate::desc::ValidationLevel::Off`] to disable the
    /// D3D12 debug layer / Vulkan validation layers on a release profile.
    pub fn create_device_and_contexts_with_validation(
        &self,
        level: crate::desc::ValidationLevel,
    ) -> Result<(RenderDevice, DeviceContext)> {
        let engine_ci = crate::desc::engine_d3d12_with_validation(level);
        let mut device: *mut sys::IRenderDevice = std::ptr::null_mut();
        let mut context: *mut sys::IDeviceContext = std::ptr::null_mut();

        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .EngineFactoryD3D12
                .CreateDeviceAndContextsD3D12
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IEngineFactoryD3D12::CreateDeviceAndContextsD3D12",
                ))?
        };
        // Safety: `device`/`context` are out params written by the engine;
        // both pointers are valid storage. The engine takes its own
        // references, which this crate then owns via the handles.
        unsafe { create(self.as_raw(), &engine_ci, &mut device, &mut context) };

        if device.is_null() {
            return Err(Error::CreateFailed("render device (D3D12)"));
        }
        if context.is_null() {
            // The engine produced a device but no context: release the
            // device before failing.
            drop(Handle::from_raw(device));
            return Err(Error::CreateFailed("immediate device context (D3D12)"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound (see the comment above the create call).
        Ok((
            unsafe { RenderDevice::from_raw(device) },
            unsafe { DeviceContext::from_raw(context) },
        ))
    }

    /// Creates a swap chain for a native window handle (`HWND`).
    ///
    /// `hwnd` must remain valid for the lifetime of the returned swap chain.
    pub fn create_swap_chain(
        &self,
        device: &RenderDevice,
        context: &DeviceContext,
        hwnd: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<SwapChain> {
        if hwnd.is_null() {
            return Err(Error::InvalidArgument("hwnd is null"));
        }
        let sc_desc = desc::swap_chain(width, height);
        let fs_desc: sys::FullScreenModeDesc = unsafe { std::mem::zeroed() };
        let window = sys::NativeWindow { hWnd: hwnd };
        let mut swap_chain: *mut sys::ISwapChain = std::ptr::null_mut();

        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .EngineFactoryD3D12
                .CreateSwapChainD3D12
                .as_ref()
                .ok_or(Error::MissingMethod("IEngineFactoryD3D12::CreateSwapChainD3D12"))?
        };
        // Safety: all structs are valid FFI types, the swap chain pointer is
        // an out param, and `window` (HWND) is the caller's responsibility.
        unsafe {
            create(
                self.as_raw(),
                device.as_raw(),
                context.as_raw(),
                &sc_desc,
                &fs_desc,
                &window,
                &mut swap_chain,
            )
        };

        if swap_chain.is_null() {
            return Err(Error::CreateFailed("swap chain"));
        }
        Ok(SwapChain::from_raw(swap_chain))
    }
}
