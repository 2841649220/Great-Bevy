//! Resource objects: buffer, shader, pipeline resource signature, pipeline
//! state and shader resource binding.

use diligent_sys::bindings as sys;

use crate::error::{Error, Result};
use crate::handle::{impl_shared_ownership, Handle};

/// Owning handle to a buffer (`IBuffer`).
pub struct Buffer {
    handle: Handle<sys::IBuffer>,
}

impl_shared_ownership!(Buffer, sys::IBuffer);

impl Buffer {
    /// Wraps a buffer pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IBuffer` instance returned by the engine (which
    /// AddRefs it); ownership is transferred to the wrapper, which releases
    /// it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IBuffer) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw buffer pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IBuffer {
        self.handle.as_ptr()
    }

    /// The buffer description (`IBuffer::GetDesc` via the universal
    /// `IDeviceObject` slot; the `DeviceObjectAttribs` member is the first
    /// field of `BufferDesc`). Copied out.
    pub fn desc(&self) -> Result<sys::BufferDesc> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .DeviceObject
                .GetDesc
                .as_ref()
                .ok_or(Error::MissingMethod("IBuffer::GetDesc"))?
        };
        // Safety: the engine returns a pointer to internal storage that is
        // valid while the buffer is alive; the desc's first member is the
        // `DeviceObjectAttribs` the call returns.
        let ptr = unsafe { get(self.as_raw().cast::<sys::IDeviceObject>()) };
        if ptr.is_null() {
            return Err(Error::MissingMethod("IBuffer::GetDesc returned null"));
        }
        Ok(unsafe { *ptr.cast::<sys::BufferDesc>() })
    }

    /// The buffer size in bytes.
    pub fn size(&self) -> Result<u64> {
        Ok(self.desc()?.Size)
    }
}

/// Owning handle to a compiled shader (`IShader`).
pub struct Shader {
    handle: Handle<sys::IShader>,
}

impl_shared_ownership!(Shader, sys::IShader);

impl Shader {
    /// Wraps a shader pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IShader` instance returned by the engine (which
    /// AddRefs it); ownership is transferred to the wrapper, which releases
    /// it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IShader) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw shader pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IShader {
        self.handle.as_ptr()
    }
}

/// Owning handle to a pipeline resource signature (`IPipelineResourceSignature`).
pub struct PipelineResourceSignature {
    handle: Handle<sys::IPipelineResourceSignature>,
}

impl_shared_ownership!(PipelineResourceSignature, sys::IPipelineResourceSignature);

impl PipelineResourceSignature {
    /// Wraps a signature pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IPipelineResourceSignature` instance returned by the
    /// engine (which AddRefs it); ownership is transferred to the wrapper,
    /// which releases it on drop. Only engine-returned pointers may be
    /// passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IPipelineResourceSignature) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw signature pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IPipelineResourceSignature {
        self.handle.as_ptr()
    }

    /// Creates a shader resource binding for this signature. The SRB is
    /// bound to the *pipeline* that uses the signature; for pipelines with
    /// explicit signatures (as created by
    /// [`RenderDevice::create_graphics_pipeline`](crate::RenderDevice::create_graphics_pipeline))
    /// this is the method to use.
    ///
    /// `init_static_resources` = true initializes any static variables
    /// immediately (harmless when the signature has no resources).
    pub fn create_shader_resource_binding(&self, init_static_resources: bool) -> Result<ShaderResourceBinding> {
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .PipelineResourceSignature
                .CreateShaderResourceBinding
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IPipelineResourceSignature::CreateShaderResourceBinding",
                ))?
        };
        let mut srb: *mut sys::IShaderResourceBinding = std::ptr::null_mut();
        // Safety: `srb` is an out param; the engine takes its own reference.
        unsafe { create(self.as_raw(), &mut srb, init_static_resources) };
        if srb.is_null() {
            return Err(Error::CreateFailed("shader resource binding"));
        }
        // Safety: the engine AddRefs the SRB before returning it, so
        // ownership transfer into the wrapper is sound.
        Ok(unsafe { ShaderResourceBinding::from_raw(srb) })
    }
}

/// Owning handle to a pipeline state (`IPipelineState`).
pub struct PipelineState {
    handle: Handle<sys::IPipelineState>,
}

impl_shared_ownership!(PipelineState, sys::IPipelineState);

impl PipelineState {
    /// Wraps a pipeline-state pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IPipelineState` instance returned by the engine
    /// (which AddRefs it); ownership is transferred to the wrapper, which
    /// releases it on drop. Only engine-returned pointers may be passed
    /// here.
    pub unsafe fn from_raw(ptr: *mut sys::IPipelineState) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw PSO pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IPipelineState {
        self.handle.as_ptr()
    }

    /// The pipeline-state description (`IPipelineState::GetDesc` via the
    /// universal `IDeviceObject` slot). `PSODesc.PipelineType` distinguishes
    /// compute (`PIPELINE_TYPE_COMPUTE`) from graphics
    /// (`PIPELINE_TYPE_GRAPHICS`) pipelines. Copied out.
    pub fn desc(&self) -> Result<sys::PipelineStateDesc> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .DeviceObject
                .GetDesc
                .as_ref()
                .ok_or(Error::MissingMethod("IPipelineState::GetDesc"))?
        };
        // Safety: the engine returns a pointer to internal storage that is
        // valid while the PSO is alive; the desc's first member is the
        // `DeviceObjectAttribs` the call returns.
        let ptr = unsafe { get(self.as_raw().cast::<sys::IDeviceObject>()) };
        if ptr.is_null() {
            return Err(Error::MissingMethod("IPipelineState::GetDesc returned null"));
        }
        Ok(unsafe { *ptr.cast::<sys::PipelineStateDesc>() })
    }

    /// The pipeline-state compilation status
    /// (`IPipelineState::GetStatus`, PipelineState.h).
    ///
    /// `WaitForCompletion = false` returns the current status without
    /// blocking (the async-compilation poll used by
    /// `pipeline_cache`); `true` blocks until the PSO is ready or failed.
    /// Synchronously created PSOs are already `READY`, so the poll is a
    /// no-op for them (the engine documents that the flag is ignored for
    /// synchronously compiled PSOs). Returned as the raw
    /// `PIPELINE_STATE_STATUS` so the caller can match on the
    /// `READY`/`FAILED` constants from `diligent-sys`.
    pub fn status(&self, wait_for_completion: bool) -> Result<sys::PIPELINE_STATE_STATUS> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .PipelineState
                .GetStatus
                .as_ref()
                .ok_or(Error::MissingMethod("IPipelineState::GetStatus"))?
        };
        // Safety: the PSO outlives the call; the status is an enum return.
        Ok(unsafe { get(self.as_raw(), wait_for_completion) })
    }

    /// Creates a shader resource binding for this pipeline (implicit
    /// signature path). For pipelines created with explicit signatures,
    /// prefer
    /// [`PipelineResourceSignature::create_shader_resource_binding`].
    pub fn create_shader_resource_binding(&self, init_static_resources: bool) -> Result<ShaderResourceBinding> {
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .PipelineState
                .CreateShaderResourceBinding
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IPipelineState::CreateShaderResourceBinding",
                ))?
        };
        let mut srb: *mut sys::IShaderResourceBinding = std::ptr::null_mut();
        // Safety: `srb` is an out param; the engine takes its own reference.
        unsafe { create(self.as_raw(), &mut srb, init_static_resources) };
        if srb.is_null() {
            return Err(Error::CreateFailed("shader resource binding"));
        }
        // Safety: the engine AddRefs the SRB before returning it, so
        // ownership transfer into the wrapper is sound.
        Ok(unsafe { ShaderResourceBinding::from_raw(srb) })
    }
}

/// Owning handle to a shader resource binding (`IShaderResourceBinding`).
pub struct ShaderResourceBinding {
    handle: Handle<sys::IShaderResourceBinding>,
}

impl_shared_ownership!(ShaderResourceBinding, sys::IShaderResourceBinding);

impl ShaderResourceBinding {
    /// Wraps a shader-resource-binding pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IShaderResourceBinding` instance returned by the
    /// engine (which AddRefs it); ownership is transferred to the wrapper,
    /// which releases it on drop. Only engine-returned pointers may be
    /// passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IShaderResourceBinding) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw SRB pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IShaderResourceBinding {
        self.handle.as_ptr()
    }
}

/// Owning handle to a GPU fence (`IFence`), used to synchronize CPU-side
/// reads of GPU-written data (e.g. staging texture readback).
pub struct Fence {
    handle: Handle<sys::IFence>,
}

impl_shared_ownership!(Fence, sys::IFence);

impl Fence {
    /// Wraps a fence pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IFence` instance returned by the engine (which
    /// AddRefs it); ownership is transferred to the wrapper, which releases
    /// it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IFence) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw fence pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IFence {
        self.handle.as_ptr()
    }

    /// Blocks the calling thread until the fence reaches `value`
    /// (`IFence::Wait`; see `IFenceMethods::Wait` in bindings.rs:9397).
    pub fn wait(&self, value: u64) -> Result<()> {
        let wait = unsafe {
            (*(*self.as_raw()).pVtbl)
                .Fence
                .Wait
                .as_ref()
                .ok_or(Error::MissingMethod("IFence::Wait"))?
        };
        // Safety: the fence is alive for the duration of the call.
        unsafe { wait(self.as_raw(), value) };
        Ok(())
    }

    /// The last value the fence has completed, without blocking
    /// (`IFence::GetCompletedValue`). Values are monotonically increasing:
    /// `get_completed_value() >= v` means every signal <= `v` has been
    /// reached by the GPU - the non-blocking poll of the cross-frame
    /// readback pattern (pair the signal
    /// [`DeviceContext::enqueue_signal`](crate::DeviceContext::enqueue_signal)
    /// with a poll here, and fall back to [`Fence::wait`] as the blocking
    /// completion).
    pub fn get_completed_value(&self) -> Result<u64> {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .Fence
                .GetCompletedValue
                .as_ref()
                .ok_or(Error::MissingMethod("IFence::GetCompletedValue"))?
        };
        // Safety: the fence is alive for the duration of the call.
        Ok(unsafe { get(self.as_raw()) })
    }
}

/// Owning handle to a pipeline-state cache (`IPipelineStateCache`).
///
/// M3b §8.10 (tasks.md 13.3): the disk-read side of the generic archive
/// (`IDearchiver::LoadArchive`) crashes deterministically on this engine
/// snapshot (V20, L4 engine-level defect), so the decision is an in-memory
/// cache + startup pre-warm. The wrapper exposes:
///
/// * creation in `LOAD_STORE` mode (`RenderDevice::create_pipeline_state_cache`),
/// * [`PipelineStateCache::get_data`] - serialize the driver PSO library blob
///   (the write side; kept for the post-L4 disk path, versioned by PSO-desc
///   hash + driver + platform per construction §7.2.4),
/// * a raw pointer for the `pPSOCache` slot of every `PipelineStateCreateInfo`
///   the PSO creation wrappers feed in (non-null => the engine stores each
///   compiled PSO in the cache and reuses it on a same-named creation).
///
/// On devices that do not support pipeline-state caches (e.g. Direct3D11,
/// OpenGL) the engine silently returns no cache; the wrapper reports `Err`.
pub struct PipelineStateCache {
    handle: Handle<sys::IPipelineStateCache>,
}

impl_shared_ownership!(PipelineStateCache, sys::IPipelineStateCache);

impl PipelineStateCache {
    /// Wraps a cache pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IPipelineStateCache` instance returned by the engine
    /// (which AddRefs it); ownership is transferred to the wrapper, which
    /// releases it on drop. Only engine-returned pointers may be passed here.
    pub unsafe fn from_raw(ptr: *mut sys::IPipelineStateCache) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw cache pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IPipelineStateCache {
        self.handle.as_ptr()
    }

    /// Serializes the driver PSO library blob (`IPipelineStateCache::GetData`,
    /// bindings.rs:7749). The returned bytes can be fed back into
    /// `PipelineStateCacheCreateInfo::pCacheData` on the next startup to
    /// rebuild the library and skip recompilation of cached PSOs (the post-L4
    /// disk path, construction §7.2.4). The engine's blob is copied out before
    /// the internal `IDataBlob` is released.
    pub fn get_data(&self) -> Result<Vec<u8>> {
        let get_data = unsafe {
            (*(*self.as_raw()).pVtbl)
                .PipelineStateCache
                .GetData
                .as_ref()
                .ok_or(Error::MissingMethod("IPipelineStateCache::GetData"))?
        };
        let mut blob: *mut sys::IDataBlob = std::ptr::null_mut();
        // Safety: `blob` is an out param; the engine AddRefs it before
        // returning, so the caller owns one reference to release.
        unsafe { get_data(self.as_raw(), &mut blob) };
        if blob.is_null() {
            return Ok(Vec::new());
        }
        let result = (|| {
            let blob_ref = unsafe { &*blob };
            let blob_vtbl = unsafe { &*blob_ref.pVtbl };
            let get_size = blob_vtbl
                .DataBlob
                .GetSize
                .as_ref()
                .ok_or(Error::MissingMethod("IDataBlob::GetSize"))?;
            let get_ptr = blob_vtbl
                .DataBlob
                .GetConstDataPtr
                .as_ref()
                .ok_or(Error::MissingMethod("IDataBlob::GetConstDataPtr"))?;
            let size = unsafe { get_size(blob) };
            if size == 0 {
                return Ok(Vec::new());
            }
            let ptr = unsafe { get_ptr(blob, 0) };
            if ptr.is_null() {
                return Err(Error::MissingMethod("IDataBlob::GetConstDataPtr returned null"));
            }
            // Safety: `size` bytes are readable at `ptr` while the blob is
            // alive (the blob outlives this closure scope).
            let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) }.to_vec();
            Ok(bytes)
        })();
        // Release the blob's reference (balanced against the engine's AddRef).
        let obj = blob.cast::<sys::IObject>();
        let release = unsafe {
            (*(*obj).pVtbl)
                .Object
                .Release
                .expect("diligent-rs: IObject::Release missing from vtable")
        };
        // Safety: the blob is released exactly once here.
        unsafe { release(obj) };
        result
    }
}

