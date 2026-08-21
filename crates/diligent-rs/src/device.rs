//! Render device: creation of buffers, shaders, signatures and pipelines.

use std::ffi::CStr;

use diligent_sys::bindings as sys;

use crate::desc;
use crate::error::{Error, Result};
use crate::handle::{cstring, Handle};
use crate::resource::{
    Buffer, Fence, PipelineResourceSignature, PipelineState, PipelineStateCache, Shader,
};
use crate::sampler::Sampler;
use crate::texture::{Texture, TextureView};

/// Owning handle to the render device (`IRenderDevice`).
pub struct RenderDevice {
    handle: Handle<sys::IRenderDevice>,
}

impl RenderDevice {
    /// Wraps a render-device pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IRenderDevice` instance returned by
    /// `IEngineFactory::CreateDeviceAndContexts` (the engine AddRefs it);
    /// ownership is transferred to the wrapper, which releases it on drop.
    /// Only engine-returned pointers may be passed here - arbitrary pointers
    /// would be dereferenced on drop.
    pub unsafe fn from_raw(ptr: *mut sys::IRenderDevice) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw device pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IRenderDevice {
        self.handle.as_ptr()
    }

    /// Device info (backend type, API version, features). Borrowed from the
    /// device; the returned struct is copied out.
    pub fn device_info(&self) -> sys::RenderDeviceInfo {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .GetDeviceInfo
                .as_ref()
                .expect("diligent-rs: IDeviceContext::GetDeviceInfo missing from vtable")
        };
        // Safety: the engine returns a pointer to internal storage that is
        // valid while the device is alive; we copy the value out.
        unsafe { *get(self.as_raw()) }
    }

    /// Adapter info (GPU name, vendor, adapter type). Copied out; the
    /// embedded name pointer stays valid while the device is alive.
    pub fn adapter_info(&self) -> sys::GraphicsAdapterInfo {
        let get = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .GetAdapterInfo
                .as_ref()
                .expect("diligent-rs: IRenderDevice::GetAdapterInfo missing from vtable")
        };
        unsafe { *get(self.as_raw()) }
    }

    /// The native `ID3D12Device` handle of the underlying D3D12 device
    /// (M5a, task 16.1 escape hatch).
    ///
    /// Resolution path: `QueryInterface(IID_RenderDeviceD3D12)` on the
    /// device object yields the backend-specific [`IRenderDeviceD3D12`]
    /// interface; `GetD3D12Device()` then returns the raw `ID3D12Device*`
    /// that vendor SDKs (NGX DLSS, FSR, XeSS, DirectSR) take as their
    /// native device. The returned pointer is **borrowed** from the engine
    /// (`GetD3D12Device` does not AddRef; do not Release it) and stays valid
    /// for as long as the [`RenderDevice`] lives.
    ///
    /// Returns `Ok(Some(ptr))` on D3D12, `Ok(None)` when the device does
    /// not expose the D3D12 interface (e.g. a Vulkan device), and `Err` when
    /// the resolution itself fails.
    pub fn native_d3d12_device(&self) -> Result<Option<*mut sys::ID3D12Device>> {
        // The IID_RenderDeviceD3D12 interface id is a header-local static
        // constexpr in Diligent (RenderDeviceD3D12.h) with no exported
        // symbol, so it cannot be referenced through `bindings` (the
        // bindgen-generated extern static would fail to link). The value is
        // inlined here - a stable ABI constant from the locked header:
        // {0xc7987c98, 0x87fe, 0x4309, {0xae, 0x88, 0xe9, 0x8f, 0x04, 0x4b, 0x00, 0xf6}}.
        const IID_RENDER_DEVICE_D3D12: sys::INTERFACE_ID = sys::INTERFACE_ID {
            Data1: 0xc7987c98,
            Data2: 0x87fe,
            Data3: 0x4309,
            Data4: [0xae, 0x88, 0xe9, 0x8f, 0x04, 0x4b, 0x00, 0xf6],
        };
        // QueryInterface is the universal `IObject` slot (the same vtable
        // head every Diligent interface shares).
        let query = unsafe {
            (*(*self.as_raw()).pVtbl)
                .Object
                .QueryInterface
                .as_ref()
                .ok_or(Error::MissingMethod("IObject::QueryInterface"))?
        };
        let mut native: *mut sys::IObject = std::ptr::null_mut();
        // Safety: the IID points at a live constant and `native` is an out
        // param; the engine AddRefs the returned interface when it supports
        // it.
        unsafe {
            query(
                self.as_raw().cast::<sys::IObject>(),
                &IID_RENDER_DEVICE_D3D12,
                &mut native,
            )
        };
        if native.is_null() {
            // Not a D3D12 device (e.g. Vulkan): no D3D12 interface.
            return Ok(None);
        }
        let native = native.cast::<sys::IRenderDeviceD3D12>();
        let get_d3d12 = unsafe {
            (*(*native).pVtbl)
                .RenderDeviceD3D12
                .GetD3D12Device
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDeviceD3D12::GetD3D12Device"))?
        };
        // Safety: `native` is alive for the call; the returned pointer is
        // borrowed (no AddRef), so ownership stays with the engine.
        let device = unsafe { get_d3d12(native) };
        // Balance the AddRef from QueryInterface: the native D3D12 interface
        // must be released exactly once.
        let obj = native.cast::<sys::IObject>();
        let release = unsafe {
            (*(*obj).pVtbl)
                .Object
                .Release
                .expect("diligent-rs: IObject::Release missing from vtable")
        };
        // Safety: the interface is released exactly once here.
        unsafe { release(obj) };
        Ok(Some(device))
    }

    /// Creates a vertex/instance/index/uniform buffer.
    ///
    /// `initial_data` must be provided for `USAGE_IMMUTABLE` buffers
    /// (`USAGE_IMMUTABLE` is the zero sentinel and the default for
    /// [`desc::buffer`] callers that do not override it). `cpu_access` must
    /// be 0 for `USAGE_DEFAULT`/`USAGE_IMMUTABLE` and `CPU_ACCESS_READ` for
    /// `USAGE_STAGING` buffers (the engine validates the combination; use
    /// [`create_staging_buffer`](Self::create_staging_buffer) for the
    /// readback pattern).
    pub fn create_buffer(
        &self,
        name: &str,
        size: u64,
        bind_flags: sys::BIND_FLAGS,
        usage: sys::USAGE,
        cpu_access: sys::CPU_ACCESS_FLAGS,
        initial_data: Option<&[u8]>,
    ) -> Result<Buffer> {
        if size == 0 {
            return Err(Error::InvalidArgument("buffer size must be > 0"));
        }
        if usage == sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE && initial_data.is_none() {
            return Err(Error::InvalidArgument(
                "USAGE_IMMUTABLE buffers must be initialized at creation",
            ));
        }
        if let Some(data) = initial_data {
            if data.len() as u64 > size {
                return Err(Error::InvalidArgument(
                    "initial data larger than the buffer size",
                ));
            }
        }

        let name_c = cstring(name)?;
        let mut buffer_desc = desc::buffer(size, bind_flags, usage, cpu_access);
        // BUFFER_MODE: the engine rejects buffers created with
        // `BIND_SHADER_RESOURCE` / `BIND_UNORDERED_ACCESS` whose mode is
        // `BUFFER_MODE_UNDEFINED` (BufferBase.cpp:68), and only
        // STRUCTURED/RAW-mode bindable buffers get default SRV/UAV views
        // (BufferBase::CreateDefaultViews). `BUFFER_MODE_RAW` is the generic
        // byte-addressed buffer the wgpu storage bindings map to (naga hlsl
        // emits `ByteAddressBuffer` for them); vertex/index/uniform-only and
        // staging buffers keep the UNDEFINED default.
        if (bind_flags
            & (sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as sys::BIND_FLAGS
                | sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as sys::BIND_FLAGS))
            != 0
        {
            buffer_desc.Mode =
                sys::_BUFFER_MODE::BUFFER_MODE_RAW as sys::BUFFER_MODE;
        }
        buffer_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        let buffer_data = initial_data.map(desc::buffer_data);

        let mut buffer: *mut sys::IBuffer = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateBuffer
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateBuffer"))?
        };
        // Safety: `buffer_desc` and `buffer_data` are valid FFI structs;
        // `buffer` is an out param. The engine copies the initial data
        // synchronously.
        unsafe { create(self.as_raw(), &buffer_desc, buffer_data.as_ref().map_or(std::ptr::null(), |d| d), &mut buffer) };

        if buffer.is_null() {
            return Err(Error::CreateFailed("buffer"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Buffer::from_raw(buffer) })
    }

    /// Creates a staging (readback) buffer: `USAGE_STAGING` with
    /// `CPU_ACCESS_READ` and no bind flags. Pair with
    /// [`DeviceContext::copy_buffer`](crate::DeviceContext::copy_buffer) and
    /// [`DeviceContext::map_buffer`](crate::DeviceContext::map_buffer).
    pub fn create_staging_buffer(&self, name: &str, size: u64) -> Result<Buffer> {
        if size == 0 {
            return Err(Error::InvalidArgument("staging buffer size must be > 0"));
        }
        let name_c = cstring(name)?;
        let mut buffer_desc = desc::staging_buffer(size);
        buffer_desc._DeviceObjectAttribs.Name = name_c.as_ptr();

        let mut buffer: *mut sys::IBuffer = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateBuffer
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateBuffer"))?
        };
        // Safety: `buffer_desc` is a valid FFI struct; `buffer` is an out
        // param.
        unsafe { create(self.as_raw(), &buffer_desc, std::ptr::null(), &mut buffer) };

        if buffer.is_null() {
            return Err(Error::CreateFailed("staging buffer"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Buffer::from_raw(buffer) })
    }

    /// Convenience: a vertex buffer holding `vertices` (plain bytes).
    pub fn create_vertex_buffer(&self, name: &str, vertices: &[u8]) -> Result<Buffer> {
        self.create_buffer(
            name,
            vertices.len() as u64,
            sys::_BIND_FLAGS::BIND_VERTEX_BUFFER as sys::BIND_FLAGS,
            sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE,
            0,
            Some(vertices),
        )
    }

    /// Compiles and creates a shader from embedded HLSL source.
    ///
    /// `source` must be valid HLSL; `entry_point` defaults to `"main"`.
    /// Compilation is synchronous (the engine returns the compile result in
    /// the return status of the shader object; creation failure surfaces as
    /// a `CreateFailed` error here).
    pub fn create_shader(
        &self,
        name: &str,
        source: &str,
        shader_type: sys::SHADER_TYPE,
    ) -> Result<Shader> {
        if source.trim().is_empty() {
            return Err(Error::InvalidArgument("shader source is empty"));
        }
        let name_c = cstring(name)?;
        let source_c = cstring(source)?;
        let entry_c = cstring("main")?;

        let mut ci: sys::ShaderCreateInfo = unsafe { std::mem::zeroed() };
        ci.Source = source_c.as_ptr();
        ci.EntryPoint = entry_c.as_ptr();
        ci.Desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci.Desc.ShaderType = shader_type;
        ci.SourceLanguage =
            sys::_SHADER_SOURCE_LANGUAGE::SHADER_SOURCE_LANGUAGE_HLSL as sys::SHADER_SOURCE_LANGUAGE;
        ci.ShaderCompiler = sys::_SHADER_COMPILER::SHADER_COMPILER_DEFAULT as sys::SHADER_COMPILER;

        let mut shader: *mut sys::IShader = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateShader
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateShader"))?
        };
        // Safety: all pointers in `ci` are live CStrings; the shader and
        // compiler-output pointers are out params. Compiler output is
        // intentionally not requested (null).
        unsafe { create(self.as_raw(), &ci, &mut shader, std::ptr::null_mut()) };

        if shader.is_null() {
            return Err(Error::CreateFailed("shader"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Shader::from_raw(shader) })
    }

    /// Creates a shader from precompiled device bytecode
    /// (`SHADER_SOURCE_LANGUAGE_BYTECODE`).
    ///
    /// This is the SPIR-V entry point: pass the naga-compiled SPIR-V
    /// bytecode for the **Vulkan** backend (the bytecode is used verbatim,
    /// no compilation happens). The locked Diligent version has no
    /// `SHADER_SOURCE_LANGUAGE_SPIRV`; `SHADER_SOURCE_LANGUAGE_BYTECODE`
    /// (Shader.h:89, "device-specific bytecode (e.g. DXBC or DXIL for
    /// Direct3D11/Direct3D12, SPIRV for Vulkan)") is its equivalent. On
    /// D3D12 this entry point expects DXBC/DXIL, so SPIR-V fails there -
    /// keep using [`create_shader`] (HLSL) for the D3D12 path.
    ///
    /// `spirv` must be non-empty and 4-byte aligned (SPIR-V words).
    pub fn create_shader_spirv(
        &self,
        name: &str,
        spirv: &[u8],
        shader_type: sys::SHADER_TYPE,
    ) -> Result<Shader> {
        if spirv.is_empty() {
            return Err(Error::InvalidArgument("SPIR-V bytecode is empty"));
        }
        if spirv.len() % 4 != 0 {
            return Err(Error::InvalidArgument(
                "SPIR-V bytecode length must be a multiple of 4",
            ));
        }
        let name_c = cstring(name)?;

        let mut ci: sys::ShaderCreateInfo = unsafe { std::mem::zeroed() };
        ci.ByteCode = spirv.as_ptr().cast();
        // `__bindgen_anon_1` is the SourceLength/ByteCodeSize union; writing
        // the ByteCodeSize member sets it as the active member.
        ci.__bindgen_anon_1.ByteCodeSize = spirv.len();
        ci.EntryPoint = std::ptr::null();
        ci.Desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci.Desc.ShaderType = shader_type;
        ci.SourceLanguage = sys::_SHADER_SOURCE_LANGUAGE::SHADER_SOURCE_LANGUAGE_BYTECODE
            as sys::SHADER_SOURCE_LANGUAGE;
        ci.ShaderCompiler = sys::_SHADER_COMPILER::SHADER_COMPILER_DEFAULT as sys::SHADER_COMPILER;

        let mut shader: *mut sys::IShader = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateShader
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateShader"))?
        };
        // Safety: `ByteCode` points at the caller's slice for the duration
        // of the call (the engine copies it); `shader` is an out param.
        unsafe { create(self.as_raw(), &ci, &mut shader, std::ptr::null_mut()) };

        if shader.is_null() {
            return Err(Error::CreateFailed("shader (bytecode)"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Shader::from_raw(shader) })
    }

    /// Creates a 2D texture (or 2D array), optionally with initial data.
    ///
    /// `array_size` 0/1 = plain 2D texture, `> 1` = array. `mip_levels` 0
    /// generates the full chain (only allowed without `initial_data`).
    ///
    /// `initial_data` uploads a **single** subresource (mip 0 of slice 0),
    /// so it requires `mip_levels`/`array_size` to describe exactly one
    /// subresource. `USAGE_IMMUTABLE` textures must be initialized at
    /// creation (same rule as `create_buffer`); the data length must match
    /// `width * height * array_size` times the format's bytes-per-pixel
    /// exactly (the engine copies `Stride x Height` bytes unconditionally,
    /// so a shorter slice would be read out of bounds). Note that
    /// `TextureData.pContext` is always null, so `USAGE_DEFAULT` /
    /// `USAGE_DYNAMIC` combined with `initial_data` fails at the engine
    /// level with a generic `CreateFailed`; initial data therefore implies
    /// `USAGE_IMMUTABLE` (wiring a context through is planned for M1b).
    /// Block-compressed formats cannot be initialized this way (their size
    /// is block-based); pass `USAGE_DEFAULT`/`USAGE_DYNAMIC` and upload
    /// via the context instead.
    ///
    /// `sample_count` 0/1 = single-sample; `> 1` creates a multisampled
    /// (MSAA) texture (`TextureDesc.SampleCount`). Multisampled textures
    /// cannot be created with `initial_data` (the engine rejects it).
    pub fn create_texture(
        &self,
        name: &str,
        width: u32,
        height: u32,
        array_size: u32,
        mip_levels: u32,
        format: sys::TEXTURE_FORMAT,
        bind_flags: sys::BIND_FLAGS,
        usage: sys::USAGE,
        sample_count: u32,
        initial_data: Option<&[u8]>,
    ) -> Result<Texture> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "texture width and height must be > 0",
            ));
        }
        if sample_count > 1 && initial_data.is_some() {
            return Err(Error::InvalidArgument(
                "MSAA textures cannot be initialized at creation",
            ));
        }
        if usage == sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE && initial_data.is_none() {
            return Err(Error::InvalidArgument(
                "USAGE_IMMUTABLE textures must be initialized at creation",
            ));
        }
        if let Some(data) = initial_data {
            if data.is_empty() {
                return Err(Error::InvalidArgument("texture initial data is empty"));
            }
            if mip_levels != 1 || array_size > 1 {
                return Err(Error::InvalidArgument(
                    "initial data uploads one subresource; mip_levels must be 1 and array_size <= 1",
                ));
            }
            let Some(bpp) = crate::format::bytes_per_pixel(format) else {
                return Err(Error::InvalidArgument(
                    "initial data for block-compressed formats is not supported by this wrapper",
                ));
            };
            // `array_size` 0/1 both mean plain 2D (see `desc::texture`). The
            // engine copies `Stride x Height` bytes unconditionally (no size
            // field in `TextureSubResData`), so anything other than exactly
            // one full subresource must be rejected here.
            let expected = width as u64 * height as u64 * array_size.max(1) as u64 * bpp as u64;
            if data.len() as u64 != expected {
                return Err(Error::InvalidArgument(
                    "initial data length must match the texture subresource exactly",
                ));
            }
        }

        let name_c = cstring(name)?;
        let mut tex_desc = desc::texture(
            width,
            height,
            array_size,
            mip_levels,
            format,
            bind_flags,
            usage,
            sample_count,
        );
        tex_desc._DeviceObjectAttribs.Name = name_c.as_ptr();

        let mut sub_res: Option<sys::TextureSubResData> = initial_data.map(|data| {
            let bpp = crate::format::bytes_per_pixel(format)
                .expect("validated above: plain format");
            let row_stride = width as u64 * bpp as u64;
            desc::texture_subres_data(data, row_stride, row_stride * height as u64)
        });
        let tex_data: Option<sys::TextureData> = sub_res.as_mut().map(|s| sys::TextureData {
            pSubResources: s,
            NumSubresources: 1,
            pContext: std::ptr::null_mut(),
        });

        let mut texture: *mut sys::ITexture = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateTexture
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateTexture"))?
        };
        // Safety: `tex_desc` and `tex_data` are valid FFI structs pointing at
        // live data; `texture` is an out param. The engine copies the
        // initial data synchronously.
        unsafe {
            create(
                self.as_raw(),
                &tex_desc,
                tex_data.as_ref().map_or(std::ptr::null(), |d| d),
                &mut texture,
            )
        };

        if texture.is_null() {
            return Err(Error::CreateFailed("texture"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Texture::from_raw(texture) })
    }

    /// Creates a staging (readback) texture: `USAGE_STAGING` with
    /// `CPU_ACCESS_READ`, no bind flags and a single mip. The format and
    /// dimensions must match the source texture's subresource. Pair with
    /// [`DeviceContext::copy_texture`](crate::DeviceContext::copy_texture)
    /// and
    /// [`DeviceContext::map_texture_subresource`](crate::DeviceContext::map_texture_subresource).
    pub fn create_staging_texture(
        &self,
        name: &str,
        width: u32,
        height: u32,
        format: sys::TEXTURE_FORMAT,
    ) -> Result<Texture> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "staging texture width and height must be > 0",
            ));
        }
        let name_c = cstring(name)?;
        let mut tex_desc = desc::staging_texture(width, height, format);
        tex_desc._DeviceObjectAttribs.Name = name_c.as_ptr();

        let mut texture: *mut sys::ITexture = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateTexture
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateTexture"))?
        };
        // Safety: `tex_desc` is a valid FFI struct; `texture` is an out
        // param.
        unsafe { create(self.as_raw(), &tex_desc, std::ptr::null(), &mut texture) };

        if texture.is_null() {
            return Err(Error::CreateFailed("staging texture"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Texture::from_raw(texture) })
    }

    /// Creates an owned texture view (`ITexture::CreateView`).
    ///
    /// `view_desc.Format = TEX_FORMAT_UNKNOWN` matches the texture format;
    /// any other value overrides it - the sRGB dual-view entry point
    /// (pair `TextureViewDesc.Format` with
    /// [`crate::format::srgb_view_format`]).
    pub fn create_texture_view(
        &self,
        texture: &Texture,
        view_desc: &sys::TextureViewDesc,
    ) -> Result<TextureView> {
        texture.create_view(view_desc)
    }

    /// Creates a sampler state object (`IRenderDevice::CreateSampler`).
    ///
    /// See [`desc::sampler`] for the field semantics. The engine returns the
    /// same sampler for identical descriptors (samplers are effectively
    /// deduplicated by the device).
    pub fn create_sampler(&self, name: &str, sampler_desc: &sys::SamplerDesc) -> Result<Sampler> {
        if sampler_desc.MaxLOD < sampler_desc.MinLOD {
            return Err(Error::InvalidArgument(
                "sampler MaxLOD must be >= MinLOD",
            ));
        }
        let is_anisotropic = |f: sys::FILTER_TYPE| {
            f == sys::_FILTER_TYPE::FILTER_TYPE_ANISOTROPIC as sys::FILTER_TYPE
                || f == sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_ANISOTROPIC as sys::FILTER_TYPE
        };
        if sampler_desc.MaxAnisotropy > 0
            && !(is_anisotropic(sampler_desc.MinFilter)
                && is_anisotropic(sampler_desc.MagFilter)
                && is_anisotropic(sampler_desc.MipFilter))
        {
            return Err(Error::InvalidArgument(
                "sampler MaxAnisotropy requires all three filters to be anisotropic",
            ));
        }

        let mut sampler_desc = *sampler_desc;
        let name_c = cstring(name)?;
        sampler_desc._DeviceObjectAttribs.Name = name_c.as_ptr();

        let mut sampler: *mut sys::ISampler = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateSampler
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateSampler"))?
        };
        // Safety: `sampler_desc` points at a live CString; `sampler` is an
        // out param and the engine takes its own reference.
        unsafe { create(self.as_raw(), &sampler_desc, &mut sampler) };
        if sampler.is_null() {
            return Err(Error::CreateFailed("sampler"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Sampler::from_raw(sampler) })
    }

    /// Creates an (explicit) pipeline resource signature (PRS).
    ///
    /// `resources` are raw `PipelineResourceDesc` entries built via
    /// [`desc::shader_resource`-style helpers](crate::desc) or directly from
    /// the bindings. Pass an empty slice for a signature with no resources.
    pub fn create_pipeline_resource_signature(
        &self,
        name: &str,
        resources: &[sys::PipelineResourceDesc],
    ) -> Result<PipelineResourceSignature> {
        let name_c = cstring(name)?;
        let mut prs_desc: sys::PipelineResourceSignatureDesc =
            unsafe { std::mem::zeroed() };
        prs_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        prs_desc.Resources = resources.as_ptr();
        prs_desc.NumResources = resources.len() as u32;
        prs_desc.BindingIndex = 0;

        let mut signature: *mut sys::IPipelineResourceSignature = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreatePipelineResourceSignature
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IRenderDevice::CreatePipelineResourceSignature",
                ))?
        };
        // Safety: `prs_desc` points at live CStrings and the caller-owned
        // resource array; `signature` is an out param.
        unsafe { create(self.as_raw(), &prs_desc, &mut signature) };

        if signature.is_null() {
            return Err(Error::CreateFailed("pipeline resource signature"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { PipelineResourceSignature::from_raw(signature) })
    }

    /// Creates a graphics pipeline state that binds the given explicit
    /// resource signatures (at least one is required - the PRS -> PSO chain
    /// this wrapper is built around).
    ///
    /// `rtv_format` must match the swap chain color buffer format (e.g.
    /// `TEX_FORMAT_RGBA8_UNORM_SRGB`). `layout_elements` describe the vertex
    /// input; the elements' HLSL semantic strings are owned by the caller
    /// and must stay alive until this call returns.
    ///
    /// `dsv_format` = `TEX_FORMAT_UNKNOWN` disables the depth test/write
    /// state - the right choice when no depth-stencil view is bound during
    /// rendering (as in `examples/triangle.rs`). Any other format keeps the
    /// C++-default depth state (test + write enabled, `COMPARISON_FUNC_LESS`)
    /// and is used as the pipeline's DSV format; the caller must then bind a
    /// depth-stencil view with a matching format every frame.
    ///
    /// `sample_count` 0/1 = single-sample; `> 1` creates a multisampled
    /// pipeline (`SmplDesc.Count`), which must match the sample count of the
    /// bound render-target views (MSAA).
    #[allow(clippy::too_many_arguments)]
    pub fn create_graphics_pipeline(
        &self,
        name: &str,
        vs: &Shader,
        ps: &Shader,
        rtv_format: sys::TEXTURE_FORMAT,
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        sample_count: u32,
    ) -> Result<PipelineState> {
        let mut blend: sys::BlendStateDesc = unsafe { std::mem::zeroed() };
        blend.RenderTargets[0].RenderTargetWriteMask =
            sys::_COLOR_MASK::COLOR_MASK_ALL as sys::COLOR_MASK;
        let mut ds: sys::DepthStencilStateDesc = unsafe { std::mem::zeroed() };
        ds.DepthEnable =
            dsv_format != sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;
        ds.DepthWriteEnable = ds.DepthEnable;
        let mut ra: sys::RasterizerStateDesc = unsafe { std::mem::zeroed() };
        ra.DepthClipEnable = true;
        self.create_graphics_pipeline_multi_rt(
            name,
            vs,
            Some(ps),
            std::slice::from_ref(&rtv_format),
            &blend.RenderTargets,
            layout_elements,
            resource_signatures,
            dsv_format,
            &ra,
            &ds,
            sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST
                as sys::PRIMITIVE_TOPOLOGY,
            sample_count,
            desc::DEFAULT_SAMPLE_MASK,
        )
    }

    /// Creates a graphics pipeline state that binds the given explicit
    /// resource signatures (at least one is required - the PRS -> PSO chain
    /// this wrapper is built around).
    ///
    /// `rtv_format` must match the swap chain color buffer format (e.g.
    /// `TEX_FORMAT_RGBA8_UNORM_SRGB`). `layout_elements` describe the vertex
    /// input; the elements' HLSL semantic strings are owned by the caller
    /// and must stay alive until this call returns.
    ///
    /// `dsv_format` = `TEX_FORMAT_UNKNOWN` disables the depth test/write
    /// state - the right choice when no depth-stencil view is bound during
    /// rendering (as in `examples/triangle.rs`). Any other format keeps the
    /// C++-default depth state (test + write enabled, `COMPARISON_FUNC_LESS`)
    /// and is used as the pipeline's DSV format; the caller must then bind a
    /// depth-stencil view with a matching format every frame.
    ///
    /// `sample_count` 0/1 = single-sample; `> 1` creates a multisampled
    /// pipeline (`SmplDesc.Count`), which must match the sample count of the
    /// bound render-target views (MSAA).
    #[allow(clippy::too_many_arguments)]
    pub fn create_graphics_pipeline_multi_rt(
        &self,
        name: &str,
        vs: &Shader,
        ps: Option<&Shader>,
        rtv_formats: &[sys::TEXTURE_FORMAT],
        blend_targets: &[sys::RenderTargetBlendDesc],
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        rasterizer: &sys::RasterizerStateDesc,
        depth_stencil: &sys::DepthStencilStateDesc,
        topology: sys::PRIMITIVE_TOPOLOGY,
        sample_count: u32,
        sample_mask: u32,
    ) -> Result<PipelineState> {
        self.create_graphics_pipeline_multi_rt_inner(
            name,
            vs,
            ps,
            rtv_formats,
            blend_targets,
            layout_elements,
            resource_signatures,
            dsv_format,
            rasterizer,
            depth_stencil,
            topology,
            sample_count,
            sample_mask,
            false,
            None,
        )
    }

    /// Same as [`create_graphics_pipeline_multi_rt`], but stores/loads the
    /// PSO through a pipeline-state cache (`pPSOCache` in
    /// `PipelineStateCreateInfo`). The cache is the M3b §8.10 in-memory half
    /// of the "memory cache + startup pre-warm" decision: a same-named PSO
    /// creation reuses the driver blob and skips recompilation. Passing
    /// `None` behaves exactly like [`create_graphics_pipeline_multi_rt`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_graphics_pipeline_multi_rt_cached(
        &self,
        name: &str,
        vs: &Shader,
        ps: Option<&Shader>,
        rtv_formats: &[sys::TEXTURE_FORMAT],
        blend_targets: &[sys::RenderTargetBlendDesc],
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        rasterizer: &sys::RasterizerStateDesc,
        depth_stencil: &sys::DepthStencilStateDesc,
        topology: sys::PRIMITIVE_TOPOLOGY,
        sample_count: u32,
        sample_mask: u32,
        pso_cache: Option<&PipelineStateCache>,
    ) -> Result<PipelineState> {
        self.create_graphics_pipeline_multi_rt_inner(
            name,
            vs,
            ps,
            rtv_formats,
            blend_targets,
            layout_elements,
            resource_signatures,
            dsv_format,
            rasterizer,
            depth_stencil,
            topology,
            sample_count,
            sample_mask,
            false,
            pso_cache,
        )
    }

    /// Same as [`create_graphics_pipeline_multi_rt`], but sets
    /// `PSO_CREATE_FLAG_ASYNCHRONOUS | SHADER_COMPILE_FLAG_ASYNCHRONOUS`
    /// on the creation info: the PSO compiles in the background and the
    /// caller polls [`PipelineState::status`](crate::PipelineState::status)
    /// until it reaches `READY`/`FAILED`. V20 verified dGPU async is
    /// 1.7-3.1x faster wall-clock than sync cold-start (30-229ms vs
    /// 17-75ms). Engines that do not support async compilation ignore the
    /// flag and create synchronously.
    #[allow(clippy::too_many_arguments)]
    pub fn create_graphics_pipeline_multi_rt_async(
        &self,
        name: &str,
        vs: &Shader,
        ps: Option<&Shader>,
        rtv_formats: &[sys::TEXTURE_FORMAT],
        blend_targets: &[sys::RenderTargetBlendDesc],
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        rasterizer: &sys::RasterizerStateDesc,
        depth_stencil: &sys::DepthStencilStateDesc,
        topology: sys::PRIMITIVE_TOPOLOGY,
        sample_count: u32,
        sample_mask: u32,
    ) -> Result<PipelineState> {
        self.create_graphics_pipeline_multi_rt_inner(
            name,
            vs,
            ps,
            rtv_formats,
            blend_targets,
            layout_elements,
            resource_signatures,
            dsv_format,
            rasterizer,
            depth_stencil,
            topology,
            sample_count,
            sample_mask,
            true,
            None,
        )
    }

    /// Same as [`create_graphics_pipeline_multi_rt_async`], but stores/loads
    /// the PSO through a pipeline-state cache (see
    /// [`create_graphics_pipeline_multi_rt_cached`]).
    #[allow(clippy::too_many_arguments)]
    pub fn create_graphics_pipeline_multi_rt_async_cached(
        &self,
        name: &str,
        vs: &Shader,
        ps: Option<&Shader>,
        rtv_formats: &[sys::TEXTURE_FORMAT],
        blend_targets: &[sys::RenderTargetBlendDesc],
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        rasterizer: &sys::RasterizerStateDesc,
        depth_stencil: &sys::DepthStencilStateDesc,
        topology: sys::PRIMITIVE_TOPOLOGY,
        sample_count: u32,
        sample_mask: u32,
        pso_cache: Option<&PipelineStateCache>,
    ) -> Result<PipelineState> {
        self.create_graphics_pipeline_multi_rt_inner(
            name,
            vs,
            ps,
            rtv_formats,
            blend_targets,
            layout_elements,
            resource_signatures,
            dsv_format,
            rasterizer,
            depth_stencil,
            topology,
            sample_count,
            sample_mask,
            true,
            pso_cache,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_graphics_pipeline_multi_rt_inner(
        &self,
        name: &str,
        vs: &Shader,
        ps: Option<&Shader>,
        rtv_formats: &[sys::TEXTURE_FORMAT],
        blend_targets: &[sys::RenderTargetBlendDesc],
        layout_elements: &[sys::LayoutElement],
        resource_signatures: &[&PipelineResourceSignature],
        dsv_format: sys::TEXTURE_FORMAT,
        rasterizer: &sys::RasterizerStateDesc,
        depth_stencil: &sys::DepthStencilStateDesc,
        topology: sys::PRIMITIVE_TOPOLOGY,
        sample_count: u32,
        sample_mask: u32,
        async_compile: bool,
        pso_cache: Option<&PipelineStateCache>,
    ) -> Result<PipelineState> {
        if layout_elements.is_empty() {
            return Err(Error::InvalidArgument(
                "graphics pipeline needs at least one layout element",
            ));
        }
        if resource_signatures.is_empty() {
            return Err(Error::InvalidArgument(
                "graphics pipeline needs at least one explicit resource signature",
            ));
        }
        if rtv_formats.is_empty()
            && dsv_format == sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT
        {
            return Err(Error::InvalidArgument(
                "graphics pipeline needs a render target or a depth-stencil format",
            ));
        }
        if rtv_formats.len() > sys::DILIGENT_MAX_RENDER_TARGETS as usize {
            return Err(Error::InvalidArgument(
                "too many render target formats",
            ));
        }
        if blend_targets.len() < rtv_formats.len() {
            return Err(Error::InvalidArgument(
                "blend_targets must cover every render target",
            ));
        }

        let name_c = cstring(name)?;
        let signature_ptrs: Vec<*mut sys::IPipelineResourceSignature> =
            resource_signatures.iter().map(|s| s.as_raw()).collect();

        let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
        // Pipeline type GRAPHICS (0). The C API cannot carry C++ default
        // member initializers (`DILIGENT_CPP_INTERFACE`-gated), so the
        // documented state defaults are filled in explicitly below; a fully
        // zeroed struct is rejected by engine validation.
        ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci._PipelineStateCreateInfo.ResourceSignaturesCount = signature_ptrs.len() as u32;
        ci._PipelineStateCreateInfo.ppResourceSignatures = signature_ptrs.as_ptr().cast_mut();
        // M2a-2 async PSO: `PSO_CREATE_FLAG_ASYNCHRONOUS` + the matching
        // `SHADER_COMPILE_FLAG_ASYNCHRONOUS` (the shader's create flags carry
        // the shader-side half; `_PipelineStateCreateInfo.Flags` carries the
        // PSO half - api-baseline §1.6). The engine ignores both when async
        // compilation is unsupported, so this is safe on all devices.
        if async_compile {
            ci._PipelineStateCreateInfo.Flags |=
                sys::_PSO_CREATE_FLAGS::PSO_CREATE_FLAG_ASYNCHRONOUS as sys::PSO_CREATE_FLAGS;
            // The wrapper creates the shaders synchronously via
            // `create_shader`; the PSO-side async flag alone is sufficient
            // for the engine to spawn the background compile of the pipeline
            // (the PSO compile includes the driver shader compilation).
        }
        // M3b §8.10: when a pipeline-state cache is present, non-null
        // `pPSOCache` makes the engine look the PSO up by `PSODesc.Name` and
        // store the compiled result (D3D12 `ID3D12PipelineLibrary`).
        if let Some(cache) = pso_cache {
            ci._PipelineStateCreateInfo.pPSOCache = cache.as_raw();
        }

        ci.GraphicsPipeline.InputLayout.LayoutElements = layout_elements.as_ptr();
        ci.GraphicsPipeline.InputLayout.NumElements = layout_elements.len() as u32;
        ci.GraphicsPipeline.PrimitiveTopology = topology;
        // M2a-2: one render target per RTV format; the write mask for EVERY
        // slot comes from the matching blend_targets entry (a zeroed mask on
        // any slot makes D3D12 discard all PS output for that target - the
        // M1 "only RT0 has a write mask" gap).
        ci.GraphicsPipeline.NumRenderTargets = rtv_formats.len() as u8;
        ci.GraphicsPipeline.NumViewports = 1;
        for (slot, format) in rtv_formats.iter().copied().enumerate() {
            ci.GraphicsPipeline.RTVFormats[slot] = format;
            ci.GraphicsPipeline.BlendDesc.RenderTargets[slot] = blend_targets[slot];
        }
        ci.GraphicsPipeline.DSVFormat = dsv_format;

        // SampleMask/SmplDesc have no C++ defaults either
        // (GraphicsPipelineDesc in PipelineState.h declares
        // `Uint32 SampleMask; SampleDesc SmplDesc;` with no member
        // initializers). The D3D12 backend passes both straight into
        // D3D12_GRAPHICS_PIPELINE_STATE_DESC (PipelineStateD3D12Impl.cpp:679
        // and :715): a zeroed SampleMask discards every pixel and
        // SmplDesc.Count = 0 makes D3D12's CreateGraphicsPipelineState
        // fail with E_INVALIDARG. Always set them explicitly (Count from
        // the `sample_count` argument - MSAA pipelines must match the
        // bound render-target sample count).
        ci.GraphicsPipeline.SampleMask = sample_mask;
        ci.GraphicsPipeline.SmplDesc.Count = sample_count.max(1) as u8;
        ci.GraphicsPipeline.SmplDesc.Quality = 0;

        ci.GraphicsPipeline.RasterizerDesc = *rasterizer;
        ci.GraphicsPipeline.DepthStencilDesc = *depth_stencil;

        ci.pVS = vs.as_raw();
        ci.pPS = ps.map_or(std::ptr::null_mut(), |p| p.as_raw());

        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateGraphicsPipelineState
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IRenderDevice::CreateGraphicsPipelineState",
                ))?
        };
        // Safety: all pointers in `ci` are live for the duration of the
        // call; `pso` is an out param.
        unsafe { create(self.as_raw(), &ci, &mut pso) };

        if pso.is_null() {
            return Err(Error::CreateFailed("graphics pipeline state"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { PipelineState::from_raw(pso) })
    }

    /// Creates a compute pipeline state (`IRenderDevice::CreateComputePipelineState`,
    /// PipelineState.h - the `pCS` shader plus the PRS array).
    ///
    /// `resource_signatures` may be empty: the engine's implicit-signature
    /// path rejects a fully-zeroed `PipelineResourceLayoutDesc` (the C API
    /// has no C++ default member initializers), so the wrapper then creates
    /// an explicit **empty** signature internally - the result is
    /// equivalent (a signature without resources, named after the
    /// pipeline). The returned [`PipelineState`] is a compute pipeline
    /// (`PipelineType` = `PIPELINE_TYPE_COMPUTE` - see
    /// [`PipelineState::desc`](crate::PipelineState::desc)).
    pub fn create_compute_pipeline(
        &self,
        name: &str,
        cs: &Shader,
        resource_signatures: &[&PipelineResourceSignature],
    ) -> Result<PipelineState> {
        // The implicit-signature path (0 explicit signatures) is rejected by
        // the engine for a zeroed `ResourceLayout`; emulate it with an
        // explicit empty signature (a PSO with no shader resources).
        let owned_empty_prs;
        let resource_signatures: Vec<&PipelineResourceSignature> = if resource_signatures.is_empty() {
            owned_empty_prs =
                self.create_pipeline_resource_signature(&format!("{name}_implicit"), &[])?;
            vec![&owned_empty_prs]
        } else {
            resource_signatures.to_vec()
        };
        let signature_ptrs: Vec<*mut sys::IPipelineResourceSignature> =
            resource_signatures.iter().map(|s| s.as_raw()).collect();

        let name_c = cstring(name)?;
        let mut ci: sys::ComputePipelineStateCreateInfo = unsafe { std::mem::zeroed() };
        // The C API cannot carry C++ default member initializers
        // (`DILIGENT_CPP_INTERFACE`-gated): `PipelineType` is zeroed to
        // `PIPELINE_TYPE_GRAPHICS` (the enum value for graphics is 0), so it
        // must be set explicitly to COMPUTE.
        ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci._PipelineStateCreateInfo.PSODesc.PipelineType =
            sys::_PIPELINE_TYPE::PIPELINE_TYPE_COMPUTE as sys::PIPELINE_TYPE;
        ci._PipelineStateCreateInfo.ResourceSignaturesCount = signature_ptrs.len() as u32;
        ci._PipelineStateCreateInfo.ppResourceSignatures = signature_ptrs.as_ptr().cast_mut();
        ci.pCS = cs.as_raw();

        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateComputePipelineState
                .as_ref()
                .ok_or(Error::MissingMethod(
                    "IRenderDevice::CreateComputePipelineState",
                ))?
        };
        // Safety: all pointers in `ci` are live for the duration of the
        // call (`name_c` is alive in the temporary `ci`'s scope); `pso` is
        // an out param.
        unsafe { create(self.as_raw(), &ci, &mut pso) };

        if pso.is_null() {
            return Err(Error::CreateFailed("compute pipeline state"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { PipelineState::from_raw(pso) })
    }

    /// TEMP DIAGNOSTIC: same as [`create_graphics_pipeline`] but with zero
    /// explicit resource signatures (implicit root signature), to bisect
    /// whether the explicit-PRS draw path is what kills rasterization.
    pub fn create_graphics_pipeline_raw_no_prs(
        &self,
        name: &str,
        vs: &Shader,
        ps: &Shader,
        rtv_format: sys::TEXTURE_FORMAT,
        layout_elements: &[sys::LayoutElement],
        dsv_format: sys::TEXTURE_FORMAT,
    ) -> Result<PipelineState> {
        let name_c = cstring(name)?;
        let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
        ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci._PipelineStateCreateInfo.ResourceSignaturesCount = 0;
        ci._PipelineStateCreateInfo.ppResourceSignatures = std::ptr::null_mut();

        ci.GraphicsPipeline.InputLayout.LayoutElements = layout_elements.as_ptr();
        ci.GraphicsPipeline.InputLayout.NumElements = layout_elements.len() as u32;
        ci.GraphicsPipeline.PrimitiveTopology =
            sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
        ci.GraphicsPipeline.NumRenderTargets = 1;
        ci.GraphicsPipeline.NumViewports = 1;
        ci.GraphicsPipeline.RTVFormats[0] = rtv_format;
        ci.GraphicsPipeline.DSVFormat = dsv_format;
        ci.GraphicsPipeline.SampleMask = desc::DEFAULT_SAMPLE_MASK;
        ci.GraphicsPipeline.SmplDesc.Count = 1;
        ci.GraphicsPipeline.SmplDesc.Quality = 0;
        let ra = &mut ci.GraphicsPipeline.RasterizerDesc;
        ra.FillMode = sys::_FILL_MODE::FILL_MODE_SOLID as sys::FILL_MODE;
        ra.CullMode = sys::_CULL_MODE::CULL_MODE_NONE as sys::CULL_MODE;
        ra.DepthClipEnable = true;
        let ds = &mut ci.GraphicsPipeline.DepthStencilDesc;
        ds.DepthEnable = dsv_format != sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;
        ds.DepthWriteEnable = ds.DepthEnable;
        ds.DepthFunc = sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS as sys::COMPARISON_FUNCTION;
        ds.StencilReadMask = 0xFF;
        ds.StencilWriteMask = 0xFF;
        for face in [&mut ds.FrontFace, &mut ds.BackFace] {
            face.StencilFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
            face.StencilDepthFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
            face.StencilPassOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
            face.StencilFunc = sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS as sys::COMPARISON_FUNCTION;
        }
        ci.pVS = vs.as_raw();
        ci.pPS = ps.as_raw();

        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateGraphicsPipelineState
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateGraphicsPipelineState"))?
        };
        unsafe { create(self.as_raw(), &ci, &mut pso) };
        if pso.is_null() {
            return Err(Error::CreateFailed("graphics pipeline state"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { PipelineState::from_raw(pso) })
    }

    /// Helper to build a shader resource desc for a PRS (raw bindings type,
    /// kept as an escape hatch for resource layouts).
    pub fn shader_resource(
        name: &CStr,
        stages: sys::SHADER_TYPE,
        resource_type: sys::SHADER_RESOURCE_TYPE,
        var_type: sys::SHADER_RESOURCE_VARIABLE_TYPE,
    ) -> sys::PipelineResourceDesc {
        let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
        r.Name = name.as_ptr();
        r.ShaderStages = stages;
        r.ArraySize = 1;
        r.ResourceType = resource_type;
        r.VarType = var_type;
        r
    }

    /// Creates a `FENCE_TYPE_GENERAL` GPU fence for CPU-GPU synchronization
    /// (see `IRenderDevice::CreateFence`, bindings.rs:12937).
    pub fn create_fence(&self, name: &str) -> Result<Fence> {
        let name_c = cstring(name)?;
        let mut desc: sys::FenceDesc = unsafe { std::mem::zeroed() };
        desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        desc.Type = sys::_FENCE_TYPE::FENCE_TYPE_GENERAL as sys::FENCE_TYPE;

        let mut fence: *mut sys::IFence = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreateFence
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreateFence"))?
        };
        // Safety: `desc` points at a live CString; `fence` is an out param
        // and the engine takes its own reference.
        unsafe { create(self.as_raw(), &desc, &mut fence) };
        if fence.is_null() {
            return Err(Error::CreateFailed("fence"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { Fence::from_raw(fence) })
    }

    /// Creates a pipeline-state cache in `LOAD_STORE` mode
    /// (`IRenderDevice::CreatePipelineStateCache`, bindings.rs:13009).
    ///
    /// M3b §8.10 (tasks.md 13.3): the in-memory cache half of the "memory
    /// cache + startup pre-warm" decision (the disk-read side of the generic
    /// archive crashes on this engine snapshot - V20, L4). With no
    /// `pCacheData` fed in, the cache starts empty and every PSO created with
    /// its [`PipelineState::as_raw`](crate::PipelineState::as_raw) pointer in
    /// the `pPSOCache` slot is stored (D3D12 `ID3D12PipelineLibrary`
    /// semantics); a same-named creation reuses the driver blob and skips
    /// recompilation. [`PipelineStateCache::get_data`] serializes the library
    /// for the post-L4 disk path.
    ///
    /// On devices without pipeline-state-cache support (D3D11/OpenGL) the
    /// engine silently produces no cache and the wrapper reports `Err`
    /// (the caller then creates PSOs without a cache).
    pub fn create_pipeline_state_cache(&self, name: &str) -> Result<PipelineStateCache> {
        let name_c = cstring(name)?;
        let mut ci: sys::PipelineStateCacheCreateInfo = unsafe { std::mem::zeroed() };
        ci.Desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci.Desc.Mode =
            sys::_PSO_CACHE_MODE::PSO_CACHE_MODE_LOAD_STORE as sys::PSO_CACHE_MODE;
        ci.Desc.Flags = sys::_PSO_CACHE_FLAGS::PSO_CACHE_FLAG_NONE as sys::PSO_CACHE_FLAGS;

        let mut cache: *mut sys::IPipelineStateCache = std::ptr::null_mut();
        let create = unsafe {
            (*(*self.as_raw()).pVtbl)
                .RenderDevice
                .CreatePipelineStateCache
                .as_ref()
                .ok_or(Error::MissingMethod("IRenderDevice::CreatePipelineStateCache"))?
        };
        // Safety: `ci` points at a live CString-backed name; `cache` is an
        // out param and the engine takes its own reference.
        unsafe { create(self.as_raw(), &ci, &mut cache) };
        if cache.is_null() {
            return Err(Error::CreateFailed("pipeline state cache"));
        }
        // Safety: the engine AddRefs created objects, so ownership transfer
        // into the wrapper is sound.
        Ok(unsafe { PipelineStateCache::from_raw(cache) })
    }
}

