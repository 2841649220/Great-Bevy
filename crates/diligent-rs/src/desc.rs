//! Safe default constructors for the raw Diligent description structs.
//!
//! The bindings crate has no `Default` impls, and Diligent's C API applies
//! its C++ defaults on the engine side whenever a field is left at its
//! `UNDEFINED`/zero sentinel. Every function here starts from a zeroed
//! struct and fills in the fields that must be explicit (e.g. a swap chain
//! color buffer format), mirroring the C++ default constructors.

use std::ffi::CStr;

use diligent_sys::bindings as sys;

/// `EngineD3D12CreateInfo` with the engine API version filled in.
///
/// Everything else defaults: single immediate graphics context, validation
/// off, adapter 0. `D3D12DllName` must be set explicitly: the C API has no
/// default-argument mechanism, and the engine passes the value straight to
/// `LoadLibraryA` (a NULL name fails the load and the follow-up error log
/// dereferences the NULL name, crashing the process).
pub fn engine_d3d12() -> sys::EngineD3D12CreateInfo {
    engine_d3d12_with_validation(ValidationLevel::default())
}

/// The validation strength of the Diligent engine D3D12 backend (task 19.3
/// debug toolchain).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationLevel {
    /// `EnableValidation = false`: the D3D12 debug layer and Vulkan
    /// validation layers are off. Matches the Diligent release default.
    Off,
    /// `EnableValidation = true`, `VALIDATION_LEVEL_1`: the standard
    /// validation layer set, with `D3D12_VALIDATION_FLAG_BREAK_ON_CORRUPTION`
    /// (the engine's default when validation is enabled).
    Level1,
    /// `EnableValidation = true`, `VALIDATION_LEVEL_2`: level 1 plus
    /// commit-time resource relevance checks and GPU-based validation
    /// (`D3D12_VALIDATION_FLAG_ENABLE_GPU_BASED_VALIDATION`). Expensive;
    /// intended for debug builds only.
    Level2,
}

impl Default for ValidationLevel {
    /// The engine's documented default: validation is enabled in
    /// Debug/Development builds and disabled in Release builds.
    fn default() -> Self {
        if cfg!(debug_assertions) {
            Self::Level1
        } else {
            Self::Off
        }
    }
}

impl ValidationLevel {
    /// Whether the backend-specific validation layer is enabled at this level.
    pub fn enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// `EngineD3D12CreateInfo` for an explicit validation level (task 19.3).
///
/// Same defaults as [`engine_d3d12`], but `EnableValidation` /
/// `ValidationLevel` / the D3D12 validation flags follow `level` instead of
/// the hard-coded on-always policy. Callers (bevy_render settings) pick the
/// level from the build profile: debug/development builds force validation
/// on, release builds may override via the `DILIGENT_RS_VALIDATION` env var.
pub fn engine_d3d12_with_validation(level: ValidationLevel) -> sys::EngineD3D12CreateInfo {
    let mut ci: sys::EngineD3D12CreateInfo = unsafe { std::mem::zeroed() };
    ci._EngineCreateInfo.EngineAPIVersion = sys::DILIGENT_API_VERSION as i32;
    ci._EngineCreateInfo.EnableValidation = level.enabled();
    ci.D3D12DllName = c"d3d12.dll".as_ptr();
    // The C API exposes the struct fields as plain members (the C++ defaults
    // are `DILIGENT_CPP_INTERFACE`-gated), so the documented C++ defaults must
    // be set explicitly. A zeroed `D3D12DllName` fails the DLL load, and zeroed
    // descriptor heap sizes crash inside the engine (GPUDescriptorHeap with no
    // descriptors), so these fields are mandatory.
    // Level 1: break on corruption (the engine's default when validation is
    // enabled). Level 2 additionally enables GPU-based validation, which
    // catches device-removal triggers (the render-pass/resolve path that
    // faults the AMD iGPU, tracked as TEMP-DIAG-M2A2).
    if level != ValidationLevel::Off {
        ci.D3D12ValidationFlags |=
            sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_BREAK_ON_CORRUPTION
                as sys::D3D12_VALIDATION_FLAGS;
    }
    if level == ValidationLevel::Level2 {
        ci.D3D12ValidationFlags |=
            sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_ENABLE_GPU_BASED_VALIDATION
                as sys::D3D12_VALIDATION_FLAGS;
    }
    // TEMP-DIAG-M2A2: GPU-based validation to catch the device-removal
    // trigger (the render-pass/resolve path that faults the AMD iGPU).
    if std::env::var_os("DILIGENT_RS_GBV").is_some() {
        ci.D3D12ValidationFlags |= sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_ENABLE_GPU_BASED_VALIDATION as sys::D3D12_VALIDATION_FLAGS;
    }
    ci.CPUDescriptorHeapAllocationSize = [8192, 2048, 1024, 1024];
    ci.GPUDescriptorHeapSize = [16384, 1024];
    ci.GPUDescriptorHeapDynamicSize = [8192, 1024];
    ci.DynamicDescriptorAllocationChunkSize = [256, 32];
    ci.DynamicHeapPageSize = 1 << 20;
    ci.NumDynamicHeapPagesToReserve = 1;
    ci.QueryPoolSizes = [0, 128, 128, 512, 128, 256];
    ci
}

/// `SwapChainDesc` with Diligent's documented defaults.
///
/// Color buffer `TEX_FORMAT_RGBA8_UNORM_SRGB`, depth buffer
/// `TEX_FORMAT_D32_FLOAT`, usage `SWAP_CHAIN_USAGE_RENDER_TARGET`, optimal
/// pre-transform, two buffers, depth clear value 1.0.
pub fn swap_chain(width: u32, height: u32) -> sys::SwapChainDesc {
    let mut d: sys::SwapChainDesc = unsafe { std::mem::zeroed() };
    d.Width = width;
    d.Height = height;
    d.ColorBufferFormat =
        sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB as sys::TEXTURE_FORMAT;
    d.DepthBufferFormat = sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT as sys::TEXTURE_FORMAT;
    d.Usage = sys::_SWAP_CHAIN_USAGE_FLAGS::SWAP_CHAIN_USAGE_RENDER_TARGET as sys::SWAP_CHAIN_USAGE_FLAGS;
    d.PreTransform = sys::_SURFACE_TRANSFORM::SURFACE_TRANSFORM_OPTIMAL as sys::SURFACE_TRANSFORM;
    d.BufferCount = 2;
    d.DefaultDepthValue = 1.0;
    d
}

/// `BufferDesc` for a plain buffer (usage, bind flags, size and CPU access).
///
/// `cpu_access` must be 0 (no CPU access) for `USAGE_DEFAULT` /
/// `USAGE_IMMUTABLE` buffers and `CPU_ACCESS_READ` for `USAGE_STAGING`
/// buffers (the engine validates the combination).
/// `ImmediateContextMask` is set to context 0 so the buffer is usable in
/// the immediate context created by `EngineFactoryD3D12::create_device_and_contexts`.
pub fn buffer(
    size: u64,
    bind_flags: sys::BIND_FLAGS,
    usage: sys::USAGE,
    cpu_access: sys::CPU_ACCESS_FLAGS,
) -> sys::BufferDesc {
    let mut d: sys::BufferDesc = unsafe { std::mem::zeroed() };
    d.Size = size;
    d.BindFlags = bind_flags;
    d.Usage = usage;
    d.CPUAccessFlags = cpu_access;
    d.ImmediateContextMask = 1;
    d
}

/// `BufferDesc` for a staging (readback) buffer: `USAGE_STAGING` with
/// `CPU_ACCESS_READ` and no bind flags - the `MAP_READ` side of the
/// `CopyBuffer` + `MapBuffer` readback pattern (see
/// [`DeviceContext::map_buffer`](crate::DeviceContext::map_buffer)).
pub fn staging_buffer(size: u64) -> sys::BufferDesc {
    buffer(
        size,
        0,
        sys::_USAGE::USAGE_STAGING as sys::USAGE,
        sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as sys::CPU_ACCESS_FLAGS,
    )
}

/// `BufferData` pointing at CPU-side initial contents.
pub fn buffer_data(data: &[u8]) -> sys::BufferData {
    let mut d: sys::BufferData = unsafe { std::mem::zeroed() };
    d.pData = data.as_ptr().cast();
    d.DataSize = data.len() as u64;
    d
}

/// One vertex layout element (e.g. a `float3 position` attribute).
///
/// `hlsl_semantic` must stay alive only for the duration of the
/// `create_graphics_pipeline` call that consumes the layout.
pub fn layout_element(
    hlsl_semantic: &CStr,
    input_index: u32,
    buffer_slot: u32,
    num_components: u32,
    value_type: sys::VALUE_TYPE,
    is_normalized: bool,
) -> sys::LayoutElement {
    let mut e: sys::LayoutElement = unsafe { std::mem::zeroed() };
    e.HLSLSemantic = hlsl_semantic.as_ptr();
    e.InputIndex = input_index;
    e.BufferSlot = buffer_slot;
    e.NumComponents = num_components;
    e.ValueType = value_type;
    e.IsNormalized = is_normalized;
    // Auto offset / stride - the engine packs the elements itself.
    e.RelativeOffset = crate::handle::LAYOUT_ELEMENT_AUTO;
    e.Stride = crate::handle::LAYOUT_ELEMENT_AUTO;
    e.Frequency =
        sys::_INPUT_ELEMENT_FREQUENCY::INPUT_ELEMENT_FREQUENCY_PER_VERTEX as sys::INPUT_ELEMENT_FREQUENCY;
    e
}

/// `Viewport` covering the full render target, DirectX convention
/// (origin top-left, Y down).
pub fn viewport(width: f32, height: f32) -> sys::Viewport {
    let mut v: sys::Viewport = unsafe { std::mem::zeroed() };
    v.Width = width;
    v.Height = height;
    v.MinDepth = 0.0;
    v.MaxDepth = 1.0;
    v
}

/// Default blend sample mask: all bits set (`0xFFFFFFFF`).
///
/// `GraphicsPipelineDesc::SampleMask` has **no C++ default initializer**
/// (see `PipelineState.h`, where it is declared as a plain `Uint32` member),
/// so a zeroed C-API struct would leave it 0 and D3D12 would discard every
/// pixel. The engine passes the value straight into the D3D12 pipeline
/// state desc (PipelineStateD3D12Impl.cpp:679), so this must always be set
/// explicitly. `create_graphics_pipeline` does that for you.
pub const DEFAULT_SAMPLE_MASK: u32 = 0xFFFF_FFFF;

/// `TextureDesc` for a single 2D texture (or 2D array).
///
/// `array_size` of 0/1 creates a plain 2D texture, `> 1` a 2D array.
/// `mip_levels` 0 asks the engine to generate the full chain. Texture
/// dimension is fixed to `RESOURCE_DIM_TEX_2D`; 3D textures are outside
/// this wrapper's scope. `ImmediateContextMask` is set to context 0.
/// `sample_count` 0/1 = single-sample, `> 1` = multisampled (MSAA); the
/// engine validates the count against the format's MSAA support. No CPU
/// access.
pub fn texture(
    width: u32,
    height: u32,
    array_size: u32,
    mip_levels: u32,
    format: sys::TEXTURE_FORMAT,
    bind_flags: sys::BIND_FLAGS,
    usage: sys::USAGE,
    sample_count: u32,
) -> sys::TextureDesc {
    let mut d: sys::TextureDesc = unsafe { std::mem::zeroed() };
    d.Type = sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_2D as sys::RESOURCE_DIMENSION;
    d.Width = width;
    d.Height = height;
    d.__bindgen_anon_1 = sys::TextureDesc__bindgen_ty_1 {
        ArraySize: array_size,
    };
    d.Format = format;
    d.MipLevels = mip_levels;
    d.SampleCount = sample_count.max(1);
    d.BindFlags = bind_flags;
    d.Usage = usage;
    d.CPUAccessFlags = 0;
    d.MiscFlags = 0;
    d.ImmediateContextMask = 1;
    d
}

/// `TextureDesc` for a staging (readback) texture: `USAGE_STAGING` with
/// `CPU_ACCESS_READ`, no bind flags and a single mip - the `MAP_READ` side
/// of the `CopyTexture` + `MapTextureSubresource` readback pattern (see
/// [`DeviceContext::map_texture_subresource`](crate::DeviceContext::map_texture_subresource)).
/// The format and dimensions must match the source texture's subresource.
pub fn staging_texture(width: u32, height: u32, format: sys::TEXTURE_FORMAT) -> sys::TextureDesc {
    let mut d = texture(width, height, 1, 1, format, 0, sys::_USAGE::USAGE_STAGING as sys::USAGE, 1);
    d.CPUAccessFlags = sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as sys::CPU_ACCESS_FLAGS;
    d
}

/// `TextureSubResData` pointing at CPU-side contents for one subresource.
pub fn texture_subres_data(data: &[u8], row_stride: u64, depth_stride: u64) -> sys::TextureSubResData {
    let mut d: sys::TextureSubResData = unsafe { std::mem::zeroed() };
    d.pData = data.as_ptr().cast();
    d.Stride = row_stride;
    d.DepthStride = depth_stride;
    d
}

/// `TextureViewDesc` for a shader-resource/render-target/depth view.
///
/// `format_override = None` matches the texture format (`TEX_FORMAT_UNKNOWN`),
/// `Some` overrides it - the sRGB dual-view entry point
/// (see [`crate::format::srgb_view_format`]). `first_mip`/`num_mips` of
/// 0/0 address all mip levels (SRV) or the largest mip (RTV/DSV);
/// `first_slice`/`num_slices` of 0/0 address all array slices. The view
/// dimension is `RESOURCE_DIM_UNDEFINED` (matches the texture).
pub fn texture_view(
    view_type: sys::TEXTURE_VIEW_TYPE,
    format_override: Option<sys::TEXTURE_FORMAT>,
    first_mip: u32,
    num_mips: u32,
    first_slice: u32,
    num_slices: u32,
) -> sys::TextureViewDesc {
    let mut d: sys::TextureViewDesc = unsafe { std::mem::zeroed() };
    d.ViewType = view_type;
    d.TextureDim = sys::_RESOURCE_DIMENSION::RESOURCE_DIM_UNDEFINED as sys::RESOURCE_DIMENSION;
    d.Format = format_override
        .unwrap_or(sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT);
    d.MostDetailedMip = first_mip;
    d.NumMipLevels = num_mips;
    d.__bindgen_anon_1 = sys::TextureViewDesc__bindgen_ty_1 {
        FirstArraySlice: first_slice,
    };
    d.__bindgen_anon_2 = sys::TextureViewDesc__bindgen_ty_2 {
        NumArraySlices: num_slices,
    };
    d
}

/// `SamplerDesc` mirroring Diligent's C++ defaults where possible.
///
/// `comparison_func = None` leaves the engine default
/// (`COMPARISON_FUNC_NEVER`); pass `Some` together with
/// `FILTER_TYPE_COMPARISON_*` filters for shadow samplers. `max_anisotropy`
/// 0 disables anisotropy; the filters must then not be
/// `FILTER_TYPE_ANISOTROPIC` (the engine validates this). `min_lod`/
/// `max_lod` default to 0 / `f32::MAX`.
pub fn sampler(
    min_filter: sys::FILTER_TYPE,
    mag_filter: sys::FILTER_TYPE,
    mip_filter: sys::FILTER_TYPE,
    address_u: sys::TEXTURE_ADDRESS_MODE,
    address_v: sys::TEXTURE_ADDRESS_MODE,
    address_w: sys::TEXTURE_ADDRESS_MODE,
    comparison_func: Option<sys::COMPARISON_FUNCTION>,
    max_anisotropy: u32,
    min_lod: f32,
    max_lod: f32,
) -> sys::SamplerDesc {
    let mut d: sys::SamplerDesc = unsafe { std::mem::zeroed() };
    d.MinFilter = min_filter;
    d.MagFilter = mag_filter;
    d.MipFilter = mip_filter;
    d.AddressU = address_u;
    d.AddressV = address_v;
    d.AddressW = address_w;
    d.Flags = sys::_SAMPLER_FLAGS::SAMPLER_FLAG_NONE as sys::SAMPLER_FLAGS;
    d.MaxAnisotropy = max_anisotropy;
    d.ComparisonFunc = comparison_func.unwrap_or(
        sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_NEVER as sys::COMPARISON_FUNCTION,
    );
    d.MinLOD = min_lod;
    d.MaxLOD = max_lod;
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The validation-level constructor drives the `EnableValidation` and
    /// D3D12 validation-flag fields exactly (task 19.3 debug toolchain):
    /// `Off` disables the debug layer, `Level1` breaks on corruption, and
    /// `Level2` adds GPU-based validation.
    #[test]
    fn engine_d3d12_with_validation_sets_expected_fields() {
        let off = engine_d3d12_with_validation(ValidationLevel::Off);
        assert!(!off._EngineCreateInfo.EnableValidation, "Off disables validation");
        let off_flags = off.D3D12ValidationFlags;
        assert_eq!(
            off_flags
                & sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_BREAK_ON_CORRUPTION
                    as sys::D3D12_VALIDATION_FLAGS,
            0,
            "Off must not set break-on-corruption"
        );

        let l1 = engine_d3d12_with_validation(ValidationLevel::Level1);
        assert!(l1._EngineCreateInfo.EnableValidation, "Level1 enables validation");
        assert_ne!(
            l1.D3D12ValidationFlags
                & sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_BREAK_ON_CORRUPTION
                    as sys::D3D12_VALIDATION_FLAGS,
            0,
            "Level1 must break on corruption"
        );
        assert_eq!(
            l1.D3D12ValidationFlags
                & sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_ENABLE_GPU_BASED_VALIDATION
                    as sys::D3D12_VALIDATION_FLAGS,
            0,
            "Level1 must not enable GPU-based validation"
        );

        let l2 = engine_d3d12_with_validation(ValidationLevel::Level2);
        assert!(l2._EngineCreateInfo.EnableValidation, "Level2 enables validation");
        assert_ne!(
            l2.D3D12ValidationFlags
                & sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_ENABLE_GPU_BASED_VALIDATION
                    as sys::D3D12_VALIDATION_FLAGS,
            0,
            "Level2 must enable GPU-based validation"
        );
    }

    /// `ValidationLevel::default()` follows the build profile: on in debug
    /// builds, off in release (the Diligent documented behavior).
    #[test]
    fn validation_level_default_follows_build_profile() {
        let default = ValidationLevel::default();
        assert_eq!(default.enabled(), cfg!(debug_assertions));
    }
}
