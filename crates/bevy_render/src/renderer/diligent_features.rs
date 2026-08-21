//! M1-4a: Diligent device capability expression (construction plan §5.3.7).
//!
//! This module re-derives the wgpu [`Features`] bitmask and the storage
//! resource limits that the rest of the renderer gates on from the Diligent
//! capability queries (`IRenderDevice::GetDeviceInfo` /
//! `IRenderDevice::GetAdapterInfo`, exposed by diligent-rs as
//! [`RenderDevice::device_info`](diligent_rs::RenderDevice::device_info) /
//! [`adapter_info`](diligent_rs::RenderDevice::adapter_info); the locked
//! Diligent headers have no `IRenderDevice::GetDeviceCaps` - M0 fact).
//!
//! Design rules (per brief):
//!
//! * The `WgpuFeatures` alias and every wgpu variant name stay untouched
//!   (consumers reference `WgpuFeatures::IMMEDIATES` etc. - red line).
//!   `DiligentFeatures` is the expression layer *behind* the alias: a pure
//!   bit mask derived from device/adapter capability data.
//! * Every derivation function is pure (mock `RenderDeviceInfo` /
//!   `GraphicsAdapterInfo` -> bits), so the mapping is unit-testable without
//!   a live GPU (tests at the bottom).
//! * `IMMEDIATES` is always set on D3D12: the gate
//!   (`wireframe.rs:128`, `meshlet/mod.rs:126`,
//!   `batching/gpu_preprocessing.rs:1342`) depends on it, and V3 empirically
//!   verified inline-constant (immediate) support on this machine's D3D12.
//! * Feature derivation mirrors what wgpu-hal reports for the same D3D12
//!   hardware (ground truth the consumers were designed against), sourced
//!   from Diligent's own feature queries where a Diligent feature exists,
//!   plus D3D12 platform constants otherwise. Sources per bit are recorded
//!   inline.

use diligent_rs::diligent_sys::bindings as sys;
use wgpu_types::Features;

/// The maximum number of storage-buffer bindings per shader stage on D3D12.
///
/// D3D12 shader model 5.1+ exposes 64 UAV slots per shader stage (the
/// classic binding-tier 1/2 constant; wgpu-hal mirrors it in
/// `wgpu-hal/src/dx12/adapter.rs:806-812` - "Maximum number of Unordered
/// Access Views in all descriptor tables across all stages": 64 for
/// tier 1 (FL >= 11.1) and tier 2). Tier-3 hardware like this machine's
/// RTX 3050 Ti lifts the *descriptor heap* bound to ~1M entries, and wgpu
/// derives ~heap-sized limits from it; Diligent PRS bindings however map to
/// per-stage register space, so the tier-1/2 constant is the honest
/// per-stage bound here. Every bevy consumer only gates on thresholds
/// `<= 10` (`gpu_preprocessing.rs:1345` requires >= 10,
/// `oit/resolve/mod.rs:35` requires 3, `sparse_buffer_vec.rs:689` requires
/// 3, `view/mod.rs:682` and `gpu_array_buffer.rs:45/81/101` only test
/// `!= 0`), so this value keeps every gate outcome identical while being
/// the number Diligent-D3D12 can actually honor.
pub const D3D12_MAX_STORAGE_BUFFERS_PER_SHADER_STAGE: u32 = 64;

/// The maximum number of storage-texture bindings per shader stage on D3D12.
///
/// Storage textures occupy the same UAV register space as storage buffers
/// (64 slots per stage, see above). Consumers gate on
/// `max_storage_textures_per_shader_stage >= 12`
/// (`batching/gpu_preprocessing.rs:1345`) and `>= 9`
/// (`bevy_pbr/light_probe` binding arrays), both satisfied.
pub const D3D12_MAX_STORAGE_TEXTURES_PER_SHADER_STAGE: u32 = 64;

/// True when the feature state is `DEVICE_FEATURE_STATE_ENABLED`.
///
/// `OPTIONAL`/`DISABLED` mean "not enabled on this device"; only `ENABLED`
/// advertises the capability (`DeviceFeatures.h`:
/// `DEVICE_FEATURE_STATE_ENABLED = 1`).
#[inline]
pub(crate) fn feature_state_enabled(state: sys::DEVICE_FEATURE_STATE) -> bool {
    state == sys::_DEVICE_FEATURE_STATE::DEVICE_FEATURE_STATE_ENABLED as sys::DEVICE_FEATURE_STATE
}

/// The Diligent device type of the given device info (D3D12 on this build).
#[inline]
fn is_d3d12(device_info: &sys::RenderDeviceInfo) -> bool {
    device_info.Type == sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_D3D12
}

/// The maximum supported shader model as a `(major, minor)` pair (from
/// `RenderDeviceInfo.MaxShaderVersion.HLSL`; D3D12 encodes the shader model,
/// e.g. 6.6 - `RenderDeviceD3D12Impl.cpp:238-240`).
#[inline]
fn shader_model(device_info: &sys::RenderDeviceInfo) -> (u32, u32) {
    (
        device_info.MaxShaderVersion.HLSL.Major,
        device_info.MaxShaderVersion.HLSL.Minor,
    )
}

/// The D3D12 wgpu feature set.
///
/// Derivation sources, per bit group:
///
/// * **Platform constants** - always available on D3D12; wgpu-hal enables
///   them unconditionally for the DX12 backend
///   (`wgpu-hal/src/dx12/adapter.rs:459-485`). `MAPPABLE_PRIMARY_BUFFERS`
///   is deliberately excluded (bevy disables it for discrete GPUs,
///   `renderer/mod.rs:316`), as is `PASSTHROUGH_SHADERS` (a wgpu-internal
///   SPIR-V passthrough flag; the diligent shader path compiles via naga)
///   and `PIPELINE_STATISTICS_QUERY` (wgpu-D3D12 does not support it, and
///   the query sets are created on the transition wgpu device -
///   `diagnostic/internal.rs:244-267`).
/// * **`DeviceFeatures`-driven bits** - sourced from the Diligent feature
///   query results. The engine fills these from the D3D12 capability probes
///   (`EngineFactoryD3DBase.hpp:214-248`, `EngineFactoryD3D12.cpp:774-983`).
/// * **Draw-command bits** - `GraphicsAdapterInfo.DrawCommand.CapFlags`
///   (`EngineFactoryD3DBase.hpp:261-268`, `EngineFactoryD3D12.cpp:1104-1117`).
/// * **Shader-model-gated bits** - `MaxShaderVersion.HLSL`; wgpu gates
///   `SHADER_INT64` on SM 6.0, `EXPERIMENTAL_RAY_QUERY` on SM 6.5 +
///   ray-tracing tier 1.1, and the int64-atomics on SM 6.6
///   (`wgpu-hal/src/dx12/adapter.rs:564-680`). Diligent exposes no direct
///   int64-atomic query, so the shader model is the recorded proxy.
fn d3d12_feature_bits(
    device_info: &sys::RenderDeviceInfo,
    adapter_info: &sys::GraphicsAdapterInfo,
) -> Features {
    let features = &device_info.Features;
    let (sm_major, sm_minor) = shader_model(device_info);

    let mut bits = Features::DEPTH32FLOAT_STENCIL8
        | Features::ADDRESS_MODE_CLAMP_TO_BORDER
        | Features::ADDRESS_MODE_CLAMP_TO_ZERO
        | Features::CLEAR_TEXTURE
        | Features::TEXTURE_FORMAT_16BIT_NORM
        | Features::PRIMITIVE_INDEX
        | Features::RG11B10UFLOAT_RENDERABLE
        | Features::TEXTURE_FORMAT_NV12
        | Features::FLOAT32_FILTERABLE
        | Features::EXTERNAL_TEXTURE
        | Features::MEMORY_DECORATION_COHERENT
        | Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        | Features::TEXTURE_ATOMIC
        // Always available on D3D12 (wgpu-hal dx12 adapter.rs:459-485).
        // V3 empirically verified inline-constant partial writes on this
        // machine's D3D12 (immediates supported).
        | Features::IMMEDIATES
        // Feature level >= 11.1, universal in practice for D3D12
        // (wgpu-hal dx12 adapter.rs:493-495).
        | Features::VERTEX_WRITABLE_STORAGE;

    if feature_state_enabled(features.WireframeFill) {
        bits |= Features::POLYGON_MODE_LINE;
    }
    if feature_state_enabled(features.DepthClamp) {
        bits |= Features::DEPTH_CLIP_CONTROL;
    }
    if feature_state_enabled(features.DualSourceBlend) {
        bits |= Features::DUAL_SOURCE_BLENDING;
    }
    if feature_state_enabled(features.TimestampQueries) {
        // D3D12 allows timestamps inside passes and encoders
        // (wgpu-hal dx12 adapter.rs:469-471).
        bits |= Features::TIMESTAMP_QUERY
            | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
            | Features::TIMESTAMP_QUERY_INSIDE_PASSES;
    }
    if feature_state_enabled(features.TextureCompressionBC) {
        bits |= Features::TEXTURE_COMPRESSION_BC | Features::TEXTURE_COMPRESSION_BC_SLICED_3D;
    }

    // Binding arrays (a.k.a. bindless): D3D12 SM5.1+ always enables both
    // bindless resources and runtime-sized shader arrays
    // (EngineFactoryD3D12.cpp:778-781, :815). This is the capability
    // `bevy_pbr`'s `binding_arrays_are_usable` (light_probe/mod.rs:783-798)
    // checks via `TEXTURE_BINDING_ARRAY | ..._NON_UNIFORM_INDEXING`, and the
    // one V19 verified on this machine (ArraySize=5000 PRS works).
    if feature_state_enabled(features.BindlessResources)
        && feature_state_enabled(features.ShaderResourceRuntimeArrays)
    {
        bits |= Features::TEXTURE_BINDING_ARRAY
            | Features::STORAGE_RESOURCE_BINDING_ARRAY
            | Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING
            | Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
            | Features::PARTIALLY_BOUND_BINDING_ARRAY;
    }

    if feature_state_enabled(features.WaveOp) {
        // D3D12 wave intrinsics imply SM 6.0 (wgpu-hal dx12
        // adapter.rs:588-593).
        bits |= Features::SUBGROUP;
    }

    // Ray queries: ray tracing tier 1.1 (inline ray tracing) + SM 6.5,
    // mirroring wgpu (wgpu-hal dx12 adapter.rs:604-623).
    if feature_state_enabled(features.RayTracing)
        && adapter_info.RayTracing.CapFlags
            & (sys::_RAY_TRACING_CAP_FLAGS::RAY_TRACING_CAP_FLAG_INLINE_RAY_TRACING
                as sys::RAY_TRACING_CAP_FLAGS)
            != 0
        && (sm_major, sm_minor) >= (6, 5)
    {
        bits |= Features::EXPERIMENTAL_RAY_QUERY
            | Features::EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS
            | Features::ACCELERATION_STRUCTURE_BINDING_ARRAY;
    }

    if sm_major >= 6 {
        // SM 6.0 + Int64ShaderOps (wgpu-hal dx12 adapter.rs:564-569);
        // Int64ShaderOps is universally true on SM 6.0 hardware.
        bits |= Features::SHADER_INT64;
    }

    if (sm_major, sm_minor) >= (6, 6) {
        // SM 6.6 64-bit typed-UAV atomics (wgpu-hal dx12
        // adapter.rs:625-680; Diligent exposes no direct query, so the
        // shader model is the recorded proxy).
        bits |= Features::SHADER_INT64_ATOMIC_ALL_OPS
            | Features::SHADER_INT64_ATOMIC_MIN_MAX
            | Features::TEXTURE_INT64_ATOMIC;
    }

    if feature_state_enabled(features.MeshShaders) {
        bits |= Features::EXPERIMENTAL_MESH_SHADER;
    }
    if feature_state_enabled(features.ShaderBarycentrics) {
        bits |= Features::SHADER_BARYCENTRICS;
    }

    // Draw-command capabilities (EngineFactoryD3DBase.hpp:261-268 sets
    // `DRAW_INDIRECT | DRAW_INDIRECT_FIRST_INSTANCE`; EngineFactoryD3D12.cpp
    // :1112-1115 adds `NATIVE_MULTI_DRAW_INDIRECT |
    // DRAW_INDIRECT_COUNTER_BUFFER` - D3D12 ExecuteIndirect).
    if adapter_info.DrawCommand.CapFlags
        & (sys::_DRAW_COMMAND_CAP_FLAGS::DRAW_COMMAND_CAP_FLAG_DRAW_INDIRECT_FIRST_INSTANCE
            as sys::DRAW_COMMAND_CAP_FLAGS)
        != 0
    {
        bits |= Features::INDIRECT_FIRST_INSTANCE;
    }
    if adapter_info.DrawCommand.CapFlags
        & (sys::_DRAW_COMMAND_CAP_FLAGS::DRAW_COMMAND_CAP_FLAG_NATIVE_MULTI_DRAW_INDIRECT
            as sys::DRAW_COMMAND_CAP_FLAGS)
        != 0
    {
        bits |= Features::MULTI_DRAW_INDIRECT_COUNT;
    }

    bits
}

/// The wgpu [`Features`] bitmask expressed from Diligent device/adapter
/// capability queries (plan §5.3.7 "自研 `DiligentFeatures`").
///
/// The `WgpuFeatures` alias and every variant name are preserved for
/// consumers; this type is the diligent-backed expression layer that
/// `RenderDevice::features()` returns (falling back to the wgpu transition
/// device's own features when no diligent caps could be derived).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiligentFeatures {
    bits: Features,
}

impl DiligentFeatures {
    /// Derives the feature mask from the Diligent capability queries.
    ///
    /// Returns `None` for backends this checkout cannot express (only D3D12
    /// is built; anything else degrades to the wgpu transition device's
    /// features, matching the Option-based graceful degradation of M1-2/3).
    pub(crate) fn derive_from_info(
        device_info: &sys::RenderDeviceInfo,
        adapter_info: &sys::GraphicsAdapterInfo,
    ) -> Option<Self> {
        if !is_d3d12(device_info) {
            return None;
        }
        Some(Self {
            bits: d3d12_feature_bits(device_info, adapter_info),
        })
    }

    /// Whether all of `features` are present in this device's mask.
    #[inline]
    pub fn contains(&self, features: Features) -> bool {
        self.bits.contains(features)
    }

    /// The raw wgpu feature mask.
    #[inline]
    pub fn as_features(self) -> Features {
        self.bits
    }

    /// Intersects this mask with `features` (drops bits, never adds).
    #[inline]
    pub fn intersect(mut self, features: Features) -> Self {
        self.bits &= features;
        self
    }
}

/// Derives `max_storage_buffers_per_shader_stage` from the Diligent device
/// info (D3D12: 64 UAV slots per stage - see
/// [`D3D12_MAX_STORAGE_BUFFERS_PER_SHADER_STAGE`]). `None` on backends this
/// checkout cannot express.
pub(crate) fn derive_max_storage_buffers_per_shader_stage(
    device_info: &sys::RenderDeviceInfo,
) -> Option<u32> {
    is_d3d12(device_info).then_some(D3D12_MAX_STORAGE_BUFFERS_PER_SHADER_STAGE)
}

/// Derives `max_storage_textures_per_shader_stage` from the Diligent device
/// info (D3D12: 64 UAV slots per stage - see
/// [`D3D12_MAX_STORAGE_TEXTURES_PER_SHADER_STAGE`]). `None` on backends this
/// checkout cannot express.
pub(crate) fn derive_max_storage_textures_per_shader_stage(
    device_info: &sys::RenderDeviceInfo,
) -> Option<u32> {
    is_d3d12(device_info).then_some(D3D12_MAX_STORAGE_TEXTURES_PER_SHADER_STAGE)
}

/// The device-level capability set `RenderDevice` stores: the wgpu feature
/// mask plus the storage-resource limits that `RenderDevice::features()` /
/// `RenderDevice::limits()` serve from the diligent device.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiligentCaps {
    features: DiligentFeatures,
    max_storage_buffers_per_shader_stage: u32,
    max_storage_textures_per_shader_stage: u32,
}

impl DiligentCaps {
    /// Queries the diligent device's info and derives the caps. `None` when
    /// the backend cannot be expressed (non-D3D12) - the caller then keeps
    /// the wgpu transition device's values.
    pub(crate) fn derive(device: &diligent_rs::RenderDevice) -> Option<Self> {
        let device_info = device.device_info();
        let adapter_info = device.adapter_info();
        Self::derive_from_info(&device_info, &adapter_info)
    }

    /// Pure derivation over the copied capability structs (unit-testable).
    fn derive_from_info(
        device_info: &sys::RenderDeviceInfo,
        adapter_info: &sys::GraphicsAdapterInfo,
    ) -> Option<Self> {
        Some(Self {
            features: DiligentFeatures::derive_from_info(device_info, adapter_info)?,
            max_storage_buffers_per_shader_stage: derive_max_storage_buffers_per_shader_stage(
                device_info,
            )?,
            max_storage_textures_per_shader_stage: derive_max_storage_textures_per_shader_stage(
                device_info,
            )?,
        })
    }

    /// The diligent-derived wgpu feature mask.
    #[inline]
    pub(crate) fn features(&self) -> DiligentFeatures {
        self.features
    }

    /// Intersects the derived feature mask with the settings-derived bits
    /// (the `WgpuSettings` `requested_features`/`disabled_features` fold -
    /// see `renderer::initialize_renderer`). Drops bits, never adds - a
    /// user-disabled feature cannot pass `RenderDevice::features()` gates.
    pub(crate) fn intersect_settings_features(mut self, settings_features: Features) -> Self {
        self.features = self.features.intersect(settings_features);
        self
    }

    /// The diligent-derived `max_storage_buffers_per_shader_stage`.
    #[inline]
    pub(crate) fn max_storage_buffers_per_shader_stage(&self) -> u32 {
        self.max_storage_buffers_per_shader_stage
    }

    /// The diligent-derived `max_storage_textures_per_shader_stage`.
    #[inline]
    pub(crate) fn max_storage_textures_per_shader_stage(&self) -> u32 {
        self.max_storage_textures_per_shader_stage
    }
}

/// The Adreno model parsed from an adapter name ("Adreno (TM) 610" -> 610),
/// or `None` for non-Adreno names.
///
/// This is the cfg-free core of [`crate::get_adreno_model`] (the
/// `target_os = "android"` gate lives there). It feeds the Adreno <= 610
/// `binding_arrays_are_usable` blacklist (`bevy_pbr` `light_probe/mod.rs:790`,
/// a red-line consumer, unchanged): models 610 and below claim bindless
/// support but are too buggy to use.
pub(crate) fn adreno_model_from_name(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("Adreno (TM) ")?;
    // M1-4a review fix round 1: upstream bevy parses the digit run after
    // the prefix and yields `None` when it is empty or does not start with
    // a digit (an empty digit run fails to parse). The previous fold
    // produced `Some(0)` for those names, which vetoed the `> 610`
    // blacklist gate in bevy_pbr `light_probe/mod.rs:790` (`Some(0)` fails
    // `is_none_or`) instead of passing it like upstream `None` does.
    let mut digits = rest.chars();
    let first = digits.next()?.to_digit(10)?;
    Some(digits.map_while(|c| c.to_digit(10)).fold(first, |acc, digit| acc * 10 + digit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu_types::Features;

    const ON: sys::DEVICE_FEATURE_STATE =
        sys::_DEVICE_FEATURE_STATE::DEVICE_FEATURE_STATE_ENABLED as sys::DEVICE_FEATURE_STATE;

    fn d3d12_device_info(shader_major: u32, shader_minor: u32) -> sys::RenderDeviceInfo {
        let mut info: sys::RenderDeviceInfo = unsafe { std::mem::zeroed() };
        info.Type = sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_D3D12;
        info.MaxShaderVersion.HLSL = sys::Version {
            Major: shader_major,
            Minor: shader_minor,
        };
        info
    }

    fn adapter_info() -> sys::GraphicsAdapterInfo {
        unsafe { std::mem::zeroed() }
    }

    /// An RTX-3050-Ti-like D3D12 mock: every feature enabled, ray tracing
    /// tier 1.1 (inline), native multi-draw-indirect + first-instance draw
    /// commands, SM 6.6.
    fn rtx3050ti_mock() -> (sys::RenderDeviceInfo, sys::GraphicsAdapterInfo) {
        let mut device = d3d12_device_info(6, 6);
        device.Features.WireframeFill = ON;
        device.Features.DepthClamp = ON;
        device.Features.DualSourceBlend = ON;
        device.Features.TimestampQueries = ON;
        device.Features.TextureCompressionBC = ON;
        device.Features.BindlessResources = ON;
        device.Features.ShaderResourceRuntimeArrays = ON;
        device.Features.WaveOp = ON;
        device.Features.RayTracing = ON;
        device.Features.MeshShaders = ON;
        device.Features.ShaderBarycentrics = ON;

        let mut adapter = adapter_info();
        adapter.RayTracing.CapFlags =
            sys::_RAY_TRACING_CAP_FLAGS::RAY_TRACING_CAP_FLAG_INLINE_RAY_TRACING
                as sys::RAY_TRACING_CAP_FLAGS;
        adapter.DrawCommand.CapFlags =
            (sys::_DRAW_COMMAND_CAP_FLAGS::DRAW_COMMAND_CAP_FLAG_DRAW_INDIRECT_FIRST_INSTANCE
                as sys::DRAW_COMMAND_CAP_FLAGS)
                | (sys::_DRAW_COMMAND_CAP_FLAGS::DRAW_COMMAND_CAP_FLAG_NATIVE_MULTI_DRAW_INDIRECT
                    as sys::DRAW_COMMAND_CAP_FLAGS);

        (device, adapter)
    }

    /// Every consumer gate (bevy_pbr / bevy_core_pipeline / bevy_render)
    /// must pass on the target-machine profile (D3D12, RTX 3050 Ti).
    #[test]
    fn d3d12_target_profile_sets_every_consumer_gate_feature() {
        let (device, adapter) = rtx3050ti_mock();
        let features = DiligentFeatures::derive_from_info(&device, &adapter)
            .expect("D3D12 must derive");
        let mask = features.as_features();

        // IMMEDIATES: wireframe.rs:128 / meshlet/mod.rs:126 /
        // gpu_preprocessing.rs:1342 gate.
        assert!(mask.contains(Features::IMMEDIATES));
        // INDIRECT_FIRST_INSTANCE | IMMEDIATES: gpu_preprocessing.rs:1342.
        assert!(mask.contains(Features::INDIRECT_FIRST_INSTANCE));
        // POLYGON_MODE_LINE | IMMEDIATES: wireframe.rs:128.
        assert!(mask.contains(Features::POLYGON_MODE_LINE));
        // Meshlet required set (meshlet/mod.rs:121-126).
        assert!(mask.contains(
            Features::TEXTURE_INT64_ATOMIC
                | Features::TEXTURE_ATOMIC
                | Features::SHADER_INT64
                | Features::SUBGROUP
                | Features::DEPTH_CLIP_CONTROL
        ));
        // Binding arrays: light_probe/mod.rs:795-796,
        // material_bind_groups.rs:1277, decal/clustered.rs:492/573.
        assert!(mask.contains(
            Features::TEXTURE_BINDING_ARRAY
                | Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
                | Features::PARTIALLY_BOUND_BINDING_ARRAY
        ));
        // EXPERIMENTAL_RAY_QUERY: bevy_solari/lib.rs:52.
        assert!(mask.contains(Features::EXPERIMENTAL_RAY_QUERY));
        // MULTI_DRAW_INDIRECT_COUNT: render_phase/mod.rs:1094,
        // gpu_preprocess.rs:1578.
        assert!(mask.contains(Features::MULTI_DRAW_INDIRECT_COUNT));
        // DUAL_SOURCE_BLENDING: atmosphere/resources.rs:382.
        assert!(mask.contains(Features::DUAL_SOURCE_BLENDING));
        // Timestamp diagnostics (internal.rs:244-294).
        assert!(mask.contains(
            Features::TIMESTAMP_QUERY
                | Features::TIMESTAMP_QUERY_INSIDE_PASSES
                | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        ));
        // CompressedImageFormats (settings.rs:193) - BC yes...
        assert!(mask.contains(Features::TEXTURE_COMPRESSION_BC));
        // ...ETC2/ASTC no (D3D12, matching wgpu).
        assert!(!mask.contains(Features::TEXTURE_COMPRESSION_ETC2));

        // Bits that must stay clear to preserve current behavior.
        assert!(!mask.contains(Features::PIPELINE_STATISTICS_QUERY));
        assert!(!mask.contains(Features::BUFFER_BINDING_ARRAY));
        assert!(!mask.contains(Features::MAPPABLE_PRIMARY_BUFFERS));
    }

    /// IMMEDIATES is a platform constant: set even on a bare D3D12 device
    /// with no feature query results (V3: inline constants work on D3D12).
    #[test]
    fn immediates_is_always_set_on_d3d12() {
        let device = d3d12_device_info(6, 6);
        let adapter = adapter_info();
        let features = DiligentFeatures::derive_from_info(&device, &adapter)
            .expect("D3D12 must derive");
        assert!(features.contains(Features::IMMEDIATES));
        // Nothing feature-driven is claimed without the queries though:
        // binding arrays require BindlessResources.
        assert!(!features.contains(Features::TEXTURE_BINDING_ARRAY));
        assert!(!features.contains(Features::POLYGON_MODE_LINE));
    }

    /// The shader-model gates mirror wgpu: ray queries need SM 6.5,
    /// int64 atomics SM 6.6, SHADER_INT64 SM 6.0.
    #[test]
    fn shader_model_gates() {
        let (device, adapter) = rtx3050ti_mock();
        let mut device_65 = device;
        device_65.MaxShaderVersion.HLSL = sys::Version { Major: 6, Minor: 5 };
        let features_65 = DiligentFeatures::derive_from_info(&device_65, &adapter).unwrap();
        assert!(features_65.contains(Features::EXPERIMENTAL_RAY_QUERY));
        assert!(features_65.contains(Features::SHADER_INT64));
        assert!(!features_65.contains(Features::TEXTURE_INT64_ATOMIC));

        let mut device_60 = device;
        device_60.MaxShaderVersion.HLSL = sys::Version { Major: 6, Minor: 0 };
        let features_60 = DiligentFeatures::derive_from_info(&device_60, &adapter).unwrap();
        assert!(!features_60.contains(Features::EXPERIMENTAL_RAY_QUERY));
        assert!(features_60.contains(Features::SHADER_INT64));
        assert!(!features_60.contains(Features::TEXTURE_INT64_ATOMIC));
    }

    /// D3D12 desktop tier: 64 storage buffers and 64 storage textures per
    /// stage (see D3D12_MAX_STORAGE_* docs; the recorded source is the D3D12
    /// UAV slot bound mirrored by wgpu-hal dx12/adapter.rs:806-812).
    #[test]
    fn storage_limits_are_d3d12_desktop_tier() {
        let device = d3d12_device_info(6, 6);
        assert_eq!(
            derive_max_storage_buffers_per_shader_stage(&device),
            Some(64)
        );
        assert_eq!(
            derive_max_storage_textures_per_shader_stage(&device),
            Some(64)
        );
    }

    /// Non-D3D12 backends yield no caps: RenderDevice then falls back to the
    /// wgpu transition device's features/limits (the graceful-degradation
    /// path; this checkout builds D3D12 only).
    #[test]
    fn non_d3d12_backend_derives_nothing() {
        let mut device = d3d12_device_info(6, 6);
        device.Type = sys::RENDER_DEVICE_TYPE::RENDER_DEVICE_TYPE_VULKAN;
        let adapter = adapter_info();
        assert!(DiligentFeatures::derive_from_info(&device, &adapter).is_none());
        assert!(derive_max_storage_buffers_per_shader_stage(&device).is_none());
        assert!(DiligentCaps::derive_from_info(&device, &adapter).is_none());
    }

    /// Adreno <= 610 blacklist parsing (bevy_pbr light_probe/mod.rs:790
    /// consumes it via crate::get_adreno_model; the <= 610 veto itself stays
    /// in the red-line consumer).
    #[test]
    fn adreno_blacklist_model_parsing() {
        assert_eq!(adreno_model_from_name("Adreno (TM) 610"), Some(610));
        assert_eq!(adreno_model_from_name("Adreno (TM) 650"), Some(650));
        // Suffixes (Adreno 642L) parse as 642, matching upstream bevy.
        assert_eq!(adreno_model_from_name("Adreno (TM) 642L"), Some(642));
        assert_eq!(adreno_model_from_name("Adreno (TM) 610?!"), Some(610));
        assert_eq!(adreno_model_from_name("NVIDIA GeForce RTX 3050 Ti"), None);
        assert_eq!(adreno_model_from_name("AMD Radeon(TM) Graphics"), None);
        // M1-4a review fix round 1: empty / non-digit-leading tails yield
        // `None` like the upstream parse (`Some(0)` would wrongly veto the
        // > 610 blacklist gate).
        assert_eq!(adreno_model_from_name("Adreno (TM) "), None);
        assert_eq!(adreno_model_from_name("Adreno (TM) X610"), None);
    }

    /// M1-4a review fix round 1: the diligent-derived mask must reflect
    /// the `WgpuSettings`-derived bits - `disabled_features` are folded
    /// into the transition wgpu device's feature set (renderer/mod.rs:324
    /// -329), and `intersect_settings_features` drops them from the mask;
    /// intersection never adds bits.
    #[test]
    fn derived_mask_intersects_disabled_settings_features() {
        let (device, adapter) = rtx3050ti_mock();
        let caps = DiligentCaps::derive_from_info(&device, &adapter).expect("D3D12 must derive");
        let derived = caps.features().as_features();

        // A disabled feature (e.g. bindless via
        // `WgpuSettings::disabled_features`) clears the bit: the
        // light_probe/mod.rs:795 gate must see it absent.
        let disabled = Features::TEXTURE_BINDING_ARRAY;
        assert!(derived.contains(disabled), "precondition: derived mask has the bit");
        let mask = caps
            .intersect_settings_features(derived & !disabled)
            .features()
            .as_features();
        assert!(!mask.contains(disabled), "disabled bit must be cleared");
        assert_eq!(mask, derived & !disabled, "nothing else is dropped");

        // A settings bit outside the derived mask (a `requested_features`
        // wgpu-D3D12 would never grant, e.g. ETC2) is never added.
        let mask = caps
            .intersect_settings_features(derived | Features::TEXTURE_COMPRESSION_ETC2)
            .features()
            .as_features();
        assert!(!mask.contains(Features::TEXTURE_COMPRESSION_ETC2));
        assert_eq!(mask, derived);
    }
}
