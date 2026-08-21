//! wgpu descriptor -> Diligent descriptor mappings (M1b replacement point 1).
//!
//! Every Diligent enum value used here is verified against the locked headers
//! (`.diligent_research/api-baseline.md` §1 and the M1-1 format mapping in
//! `crates/diligent-rs/src/format.rs`); no values are invented. The Diligent
//! enums mostly mirror D3D12, so the wgpu <-> Diligent mapping is a direct
//! translation (same semantics, verified names).
//!
//! All functions are pure (no device access) so they can be unit-tested
//! without a GPU. Failures are reported through `Option`/`Result` and must be
//! surfaced as graceful `None`/warn paths by callers (never panics) - see the
//! task brief's "新建路径失败必须走 RenderErrorPolicy/错误返回" rule.

use diligent_rs::diligent_sys::bindings as sys;
use wgpu_types::{AddressMode, BufferBindingType, BufferUsages, CompareFunction, FilterMode};

/// wgpu `BufferUsages` -> Diligent `BIND_FLAGS` (creation-time bind flags).
///
/// Note: wgpu `STORAGE` covers both read-write and read-only storage
/// bindings; Diligent expresses the access via the binding variable type
/// (`BUFFER_UAV` vs `BUFFER_SRV`), so the buffer itself gets both
/// `BIND_UNORDERED_ACCESS | BIND_SHADER_RESOURCE`.
pub fn buffer_usage_to_bind_flags(usage: BufferUsages) -> sys::BIND_FLAGS {
    let mut flags = 0u32;
    let mut or = |d: sys::BIND_FLAGS| flags |= d;
    if usage.contains(BufferUsages::INDEX) {
        or(sys::_BIND_FLAGS::BIND_INDEX_BUFFER as sys::BIND_FLAGS);
    }
    if usage.contains(BufferUsages::VERTEX) {
        or(sys::_BIND_FLAGS::BIND_VERTEX_BUFFER as sys::BIND_FLAGS);
    }
    if usage.contains(BufferUsages::UNIFORM) {
        or(sys::_BIND_FLAGS::BIND_UNIFORM_BUFFER as sys::BIND_FLAGS);
    }
    if usage.contains(BufferUsages::STORAGE) {
        or(sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as sys::BIND_FLAGS);
        // A read-only storage binding needs an SRV view of the same buffer.
        or(sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as sys::BIND_FLAGS);
    }
    if usage.contains(BufferUsages::INDIRECT) {
        or(sys::_BIND_FLAGS::BIND_INDIRECT_DRAW_ARGS as sys::BIND_FLAGS);
    }
    if usage.intersects(BufferUsages::BLAS_INPUT | BufferUsages::TLAS_INPUT) {
        or(sys::_BIND_FLAGS::BIND_RAY_TRACING as sys::BIND_FLAGS);
    }
    flags
}

/// wgpu `BufferUsages` -> Diligent `USAGE` (staging vs default).
///
/// `MAP_READ`/`MAP_WRITE` buffers get `USAGE_STAGING` (map semantics - the
/// readback pool and the diagnostic timestamp buffers); every other buffer
/// is `USAGE_DEFAULT` (the engine's copy path does not need a special
/// usage). Callers that provide initial data override to `USAGE_IMMUTABLE`
/// (M1-1 wrapper contract: immutable buffers must be initialized at
/// creation).
pub fn buffer_usage_to_usage(usage: BufferUsages) -> sys::USAGE {
    if usage.contains(BufferUsages::MAP_READ) || usage.contains(BufferUsages::MAP_WRITE) {
        sys::_USAGE::USAGE_STAGING as sys::USAGE
    } else {
        sys::_USAGE::USAGE_DEFAULT as sys::USAGE
    }
}

/// wgpu `BufferUsages` -> Diligent `CPU_ACCESS_FLAGS` (only meaningful on
/// `USAGE_STAGING` buffers - `RenderDevice::create_diligent_buffer` passes
/// 0 for the other usages, which the engine validates).
pub fn buffer_usage_to_cpu_access(usage: BufferUsages) -> sys::CPU_ACCESS_FLAGS {
    let mut flags = 0u8;
    if usage.contains(BufferUsages::MAP_READ) {
        flags |= sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as u8;
    }
    if usage.contains(BufferUsages::MAP_WRITE) {
        flags |= sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_WRITE as u8;
    }
    flags
}

/// wgpu `TextureUsages` -> Diligent `BIND_FLAGS`.
///
/// wgpu `RENDER_ATTACHMENT` covers both color and depth-stencil attachment
/// uses; the texture is created with both bind flags and the view type picks
/// the one in use.
pub fn texture_usage_to_bind_flags(usage: wgpu_types::TextureUsages) -> sys::BIND_FLAGS {
    let mut flags = 0u32;
    let mut or = |d: sys::BIND_FLAGS| flags |= d;
    if usage.contains(wgpu_types::TextureUsages::TEXTURE_BINDING) {
        or(sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as sys::BIND_FLAGS);
    }
    if usage.contains(wgpu_types::TextureUsages::STORAGE_BINDING) {
        or(sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as sys::BIND_FLAGS);
    }
    if usage.contains(wgpu_types::TextureUsages::RENDER_ATTACHMENT) {
        or(sys::_BIND_FLAGS::BIND_RENDER_TARGET as sys::BIND_FLAGS);
        or(sys::_BIND_FLAGS::BIND_DEPTH_STENCIL as sys::BIND_FLAGS);
    }
    flags
}

/// wgpu `VertexFormat` -> Diligent `VALUE_TYPE` + component count +
/// normalized flag. Returns `None` for formats without a Diligent
/// counterpart (64-bit and packed formats; none are used by bevy meshes).
pub fn vertex_format_to_value_type(
    format: wgpu_types::VertexFormat,
) -> Option<(sys::VALUE_TYPE, u32, bool)> {
    use wgpu_types::VertexFormat as F;
    let vt = |v: sys::_VALUE_TYPE| v as sys::VALUE_TYPE;
    Some(match format {
        F::Uint8 => (vt(sys::_VALUE_TYPE::VT_UINT8), 1, false),
        F::Uint8x2 => (vt(sys::_VALUE_TYPE::VT_UINT8), 2, false),
        F::Uint8x4 => (vt(sys::_VALUE_TYPE::VT_UINT8), 4, false),
        F::Sint8 => (vt(sys::_VALUE_TYPE::VT_INT8), 1, false),
        F::Sint8x2 => (vt(sys::_VALUE_TYPE::VT_INT8), 2, false),
        F::Sint8x4 => (vt(sys::_VALUE_TYPE::VT_INT8), 4, false),
        F::Unorm8 => (vt(sys::_VALUE_TYPE::VT_UINT8), 1, true),
        F::Unorm8x2 => (vt(sys::_VALUE_TYPE::VT_UINT8), 2, true),
        F::Unorm8x4 => (vt(sys::_VALUE_TYPE::VT_UINT8), 4, true),
        F::Snorm8 => (vt(sys::_VALUE_TYPE::VT_INT8), 1, true),
        F::Snorm8x2 => (vt(sys::_VALUE_TYPE::VT_INT8), 2, true),
        F::Snorm8x4 => (vt(sys::_VALUE_TYPE::VT_INT8), 4, true),
        F::Uint16 => (vt(sys::_VALUE_TYPE::VT_UINT16), 1, false),
        F::Uint16x2 => (vt(sys::_VALUE_TYPE::VT_UINT16), 2, false),
        F::Uint16x4 => (vt(sys::_VALUE_TYPE::VT_UINT16), 4, false),
        F::Sint16 => (vt(sys::_VALUE_TYPE::VT_INT16), 1, false),
        F::Sint16x2 => (vt(sys::_VALUE_TYPE::VT_INT16), 2, false),
        F::Sint16x4 => (vt(sys::_VALUE_TYPE::VT_INT16), 4, false),
        F::Unorm16 => (vt(sys::_VALUE_TYPE::VT_UINT16), 1, true),
        F::Unorm16x2 => (vt(sys::_VALUE_TYPE::VT_UINT16), 2, true),
        F::Unorm16x4 => (vt(sys::_VALUE_TYPE::VT_UINT16), 4, true),
        F::Snorm16 => (vt(sys::_VALUE_TYPE::VT_INT16), 1, true),
        F::Snorm16x2 => (vt(sys::_VALUE_TYPE::VT_INT16), 2, true),
        F::Snorm16x4 => (vt(sys::_VALUE_TYPE::VT_INT16), 4, true),
        F::Float16 => (vt(sys::_VALUE_TYPE::VT_FLOAT16), 1, false),
        F::Float16x2 => (vt(sys::_VALUE_TYPE::VT_FLOAT16), 2, false),
        F::Float16x4 => (vt(sys::_VALUE_TYPE::VT_FLOAT16), 4, false),
        F::Float32 => (vt(sys::_VALUE_TYPE::VT_FLOAT32), 1, false),
        F::Float32x2 => (vt(sys::_VALUE_TYPE::VT_FLOAT32), 2, false),
        F::Float32x3 => (vt(sys::_VALUE_TYPE::VT_FLOAT32), 3, false),
        F::Float32x4 => (vt(sys::_VALUE_TYPE::VT_FLOAT32), 4, false),
        F::Uint32 => (vt(sys::_VALUE_TYPE::VT_UINT32), 1, false),
        F::Uint32x2 => (vt(sys::_VALUE_TYPE::VT_UINT32), 2, false),
        F::Uint32x3 => (vt(sys::_VALUE_TYPE::VT_UINT32), 3, false),
        F::Uint32x4 => (vt(sys::_VALUE_TYPE::VT_UINT32), 4, false),
        F::Sint32 => (vt(sys::_VALUE_TYPE::VT_INT32), 1, false),
        F::Sint32x2 => (vt(sys::_VALUE_TYPE::VT_INT32), 2, false),
        F::Sint32x3 => (vt(sys::_VALUE_TYPE::VT_INT32), 3, false),
        F::Sint32x4 => (vt(sys::_VALUE_TYPE::VT_INT32), 4, false),
        F::Float64 | F::Float64x2 | F::Float64x3 | F::Float64x4
        | F::Unorm10_10_10_2
        | F::Unorm8x4Bgra => {
            return None
        }
    })
}

/// wgpu `CompareFunction` -> Diligent `COMPARISON_FUNCTION`.
pub fn comparison_function(f: CompareFunction) -> sys::COMPARISON_FUNCTION {
    let cf = |v: sys::_COMPARISON_FUNCTION| v as sys::COMPARISON_FUNCTION;
    match f {
        CompareFunction::Never => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_NEVER),
        CompareFunction::Less => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS),
        CompareFunction::Equal => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_EQUAL),
        CompareFunction::LessEqual => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS_EQUAL),
        CompareFunction::Greater => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_GREATER),
        CompareFunction::NotEqual => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_NOT_EQUAL),
        CompareFunction::GreaterEqual => {
            cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_GREATER_EQUAL)
        }
        CompareFunction::Always => cf(sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS),
    }
}

/// wgpu `AddressMode` -> Diligent `TEXTURE_ADDRESS_MODE`.
pub fn address_mode(mode: AddressMode) -> sys::TEXTURE_ADDRESS_MODE {
    let am = |v: sys::_TEXTURE_ADDRESS_MODE| v as sys::TEXTURE_ADDRESS_MODE;
    match mode {
        AddressMode::ClampToEdge => am(sys::_TEXTURE_ADDRESS_MODE::TEXTURE_ADDRESS_CLAMP),
        AddressMode::Repeat => am(sys::_TEXTURE_ADDRESS_MODE::TEXTURE_ADDRESS_WRAP),
        AddressMode::MirrorRepeat => am(sys::_TEXTURE_ADDRESS_MODE::TEXTURE_ADDRESS_MIRROR),
        AddressMode::ClampToBorder => am(sys::_TEXTURE_ADDRESS_MODE::TEXTURE_ADDRESS_BORDER),
    }
}

/// wgpu filter (mag/min/mip) -> Diligent `FILTER_TYPE`.
///
/// `comparison` selects the `COMPARISON_*` variants (shadow samplers);
/// `anisotropy` selects `ANISOTROPIC` when the filter is linear.
pub fn filter_type(mode: FilterMode, comparison: bool, anisotropy: bool) -> sys::FILTER_TYPE {
    let ft = |v: sys::_FILTER_TYPE| v as sys::FILTER_TYPE;
    match (mode, comparison, anisotropy) {
        (FilterMode::Nearest, false, _) => ft(sys::_FILTER_TYPE::FILTER_TYPE_POINT),
        (FilterMode::Nearest, true, _) => {
            ft(sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_POINT)
        }
        (FilterMode::Linear, false, true) => ft(sys::_FILTER_TYPE::FILTER_TYPE_ANISOTROPIC),
        (FilterMode::Linear, false, false) => ft(sys::_FILTER_TYPE::FILTER_TYPE_LINEAR),
        (FilterMode::Linear, true, true) => {
            ft(sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_ANISOTROPIC)
        }
        (FilterMode::Linear, true, false) => {
            ft(sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_LINEAR)
        }
    }
}

/// wgpu `BlendFactor` -> Diligent `BLEND_FACTOR` (D3D12 mirror enums).
pub fn blend_factor(factor: wgpu_types::BlendFactor) -> sys::BLEND_FACTOR {
    let bf = |v: sys::_BLEND_FACTOR| v as sys::BLEND_FACTOR;
    match factor {
        wgpu_types::BlendFactor::Zero => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_ZERO),
        wgpu_types::BlendFactor::One => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_ONE),
        wgpu_types::BlendFactor::Src => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_SRC_COLOR),
        wgpu_types::BlendFactor::OneMinusSrc => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_SRC_COLOR),
        wgpu_types::BlendFactor::SrcAlpha => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_SRC_ALPHA),
        wgpu_types::BlendFactor::OneMinusSrcAlpha => {
            bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_SRC_ALPHA)
        }
        wgpu_types::BlendFactor::Dst => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_DEST_COLOR),
        wgpu_types::BlendFactor::OneMinusDst => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_DEST_COLOR),
        wgpu_types::BlendFactor::DstAlpha => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_DEST_ALPHA),
        wgpu_types::BlendFactor::OneMinusDstAlpha => {
            bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_DEST_ALPHA)
        }
        wgpu_types::BlendFactor::SrcAlphaSaturated => {
            bf(sys::_BLEND_FACTOR::BLEND_FACTOR_SRC_ALPHA_SAT)
        }
        wgpu_types::BlendFactor::Constant => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_BLEND_FACTOR),
        wgpu_types::BlendFactor::OneMinusConstant => {
            bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_BLEND_FACTOR)
        }
        wgpu_types::BlendFactor::Src1 => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_SRC1_COLOR),
        wgpu_types::BlendFactor::OneMinusSrc1 => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_SRC1_COLOR),
        wgpu_types::BlendFactor::Src1Alpha => bf(sys::_BLEND_FACTOR::BLEND_FACTOR_SRC1_ALPHA),
        wgpu_types::BlendFactor::OneMinusSrc1Alpha => {
            bf(sys::_BLEND_FACTOR::BLEND_FACTOR_INV_SRC1_ALPHA)
        }
    }
}

/// wgpu `BlendOperation` -> Diligent `BLEND_OPERATION` (D3D12 mirror enums).
pub fn blend_operation(op: wgpu_types::BlendOperation) -> sys::BLEND_OPERATION {
    let bo = |v: sys::_BLEND_OPERATION| v as sys::BLEND_OPERATION;
    match op {
        wgpu_types::BlendOperation::Add => bo(sys::_BLEND_OPERATION::BLEND_OPERATION_ADD),
        wgpu_types::BlendOperation::Subtract => bo(sys::_BLEND_OPERATION::BLEND_OPERATION_SUBTRACT),
        wgpu_types::BlendOperation::ReverseSubtract => {
            bo(sys::_BLEND_OPERATION::BLEND_OPERATION_REV_SUBTRACT)
        }
        wgpu_types::BlendOperation::Min => bo(sys::_BLEND_OPERATION::BLEND_OPERATION_MIN),
        wgpu_types::BlendOperation::Max => bo(sys::_BLEND_OPERATION::BLEND_OPERATION_MAX),
    }
}

/// wgpu `ColorWrites` -> Diligent `COLOR_MASK`.
///
/// Both flag sets use the same bit layout (RED=1, GREEN=2, BLUE=4, ALPHA=8,
/// ALL=15 - see api-baseline.md §1 and wgpu-types), so this is a straight
/// bit copy.
pub fn color_writes(writes: wgpu_types::ColorWrites) -> sys::COLOR_MASK {
    writes.bits() as sys::COLOR_MASK
}

/// wgpu `StencilOperation` -> Diligent `STENCIL_OP`.
pub fn stencil_operation(op: wgpu_types::StencilOperation) -> sys::STENCIL_OP {
    let so = |v: sys::_STENCIL_OP| v as sys::STENCIL_OP;
    match op {
        wgpu_types::StencilOperation::Keep => so(sys::_STENCIL_OP::STENCIL_OP_KEEP),
        wgpu_types::StencilOperation::Zero => so(sys::_STENCIL_OP::STENCIL_OP_ZERO),
        wgpu_types::StencilOperation::Replace => so(sys::_STENCIL_OP::STENCIL_OP_REPLACE),
        wgpu_types::StencilOperation::IncrementClamp => so(sys::_STENCIL_OP::STENCIL_OP_INCR_SAT),
        wgpu_types::StencilOperation::DecrementClamp => so(sys::_STENCIL_OP::STENCIL_OP_DECR_SAT),
        wgpu_types::StencilOperation::Invert => so(sys::_STENCIL_OP::STENCIL_OP_INVERT),
        wgpu_types::StencilOperation::IncrementWrap => so(sys::_STENCIL_OP::STENCIL_OP_INCR_WRAP),
        wgpu_types::StencilOperation::DecrementWrap => so(sys::_STENCIL_OP::STENCIL_OP_DECR_WRAP),
    }
}

/// wgpu `PrimitiveTopology` -> Diligent `PRIMITIVE_TOPOLOGY`.
pub fn primitive_topology(topology: wgpu_types::PrimitiveTopology) -> sys::PRIMITIVE_TOPOLOGY {
    let pt = |v: sys::_PRIMITIVE_TOPOLOGY| v as sys::PRIMITIVE_TOPOLOGY;
    match topology {
        wgpu_types::PrimitiveTopology::PointList => {
            pt(sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_POINT_LIST)
        }
        wgpu_types::PrimitiveTopology::LineList => {
            pt(sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_LINE_LIST)
        }
        wgpu_types::PrimitiveTopology::LineStrip => {
            pt(sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_LINE_STRIP)
        }
        wgpu_types::PrimitiveTopology::TriangleList => {
            pt(sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST)
        }
        wgpu_types::PrimitiveTopology::TriangleStrip => {
            pt(sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP)
        }
    }
}

/// wgpu `PolygonMode` -> Diligent `FILL_MODE`.
///
/// Returns `None` for `PolygonMode::Point` (no Diligent equivalent - the
/// locked enum only has SOLID/WIREFRAME, GraphicsTypes.h).
pub fn fill_mode(mode: wgpu_types::PolygonMode) -> Option<sys::FILL_MODE> {
    let fm = |v: sys::_FILL_MODE| v as sys::FILL_MODE;
    match mode {
        wgpu_types::PolygonMode::Fill => Some(fm(sys::_FILL_MODE::FILL_MODE_SOLID)),
        wgpu_types::PolygonMode::Line => Some(fm(sys::_FILL_MODE::FILL_MODE_WIREFRAME)),
        wgpu_types::PolygonMode::Point => None,
    }
}

/// wgpu `Face` (cull mode) -> Diligent `CULL_MODE`.
pub fn cull_mode(mode: Option<wgpu_types::Face>) -> sys::CULL_MODE {
    let cm = |v: sys::_CULL_MODE| v as sys::CULL_MODE;
    match mode {
        None => cm(sys::_CULL_MODE::CULL_MODE_NONE),
        Some(wgpu_types::Face::Front) => cm(sys::_CULL_MODE::CULL_MODE_FRONT),
        Some(wgpu_types::Face::Back) => cm(sys::_CULL_MODE::CULL_MODE_BACK),
    }
}

/// wgpu `ShaderStages` -> Diligent `SHADER_TYPE` bit flags (visibility).
///
/// Diligent's bits: VERTEX=1<<0, PIXEL=1<<1, COMPUTE=1<<5 (api-baseline
/// §1.2 verified via bindings; SHADER_TYPE_VERTEX=1, SHADER_TYPE_PIXEL=2,
/// SHADER_TYPE_COMPUTE=32).
pub fn shader_stages(stages: wgpu_types::ShaderStages) -> sys::SHADER_TYPE {
    let mut bits = 0u32;
    if stages.contains(wgpu_types::ShaderStages::VERTEX) {
        bits |= sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as u32;
    }
    if stages.contains(wgpu_types::ShaderStages::FRAGMENT) {
        bits |= sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as u32;
    }
    if stages.contains(wgpu_types::ShaderStages::COMPUTE) {
        bits |= sys::_SHADER_TYPE::SHADER_TYPE_COMPUTE as u32;
    }
    bits
}

/// naga stage -> Diligent `SHADER_TYPE` (single shader creation).
pub fn shader_type_from_naga(stage: naga::ShaderStage) -> sys::SHADER_TYPE {
    let st = |v: sys::_SHADER_TYPE| v as sys::SHADER_TYPE;
    match stage {
        naga::ShaderStage::Vertex => st(sys::_SHADER_TYPE::SHADER_TYPE_VERTEX),
        naga::ShaderStage::Fragment => st(sys::_SHADER_TYPE::SHADER_TYPE_PIXEL),
        naga::ShaderStage::Compute => st(sys::_SHADER_TYPE::SHADER_TYPE_COMPUTE),
        // naga::ShaderStage is non_exhaustive (future stages); vertex is a
        // defensive stand-in that never matches today.
        _ => st(sys::_SHADER_TYPE::SHADER_TYPE_VERTEX),
    }
}

/// wgpu `BindingType` -> Diligent `SHADER_RESOURCE_TYPE`.
///
/// Returns `None` for types without a Diligent counterpart (external
/// textures).
pub fn binding_type_to_resource_type(ty: &wgpu_types::BindingType) -> Option<sys::SHADER_RESOURCE_TYPE> {
    let rt = |v: sys::_SHADER_RESOURCE_TYPE| v as sys::SHADER_RESOURCE_TYPE;
    Some(match ty {
        wgpu_types::BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            ..
        } => rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER),
        wgpu_types::BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            ..
        } => rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV),
        wgpu_types::BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: false },
            ..
        } => rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_UAV),
        wgpu_types::BindingType::Texture { .. } => {
            rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV)
        }
        wgpu_types::BindingType::StorageTexture { .. } => {
            rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_UAV)
        }
        wgpu_types::BindingType::Sampler(_) => {
            rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_SAMPLER)
        }
        wgpu_types::BindingType::AccelerationStructure { .. } => {
            rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_ACCEL_STRUCT)
        }
        wgpu_types::BindingType::ExternalTexture => return None,
    })
}

/// The `ArraySize` for a wgpu binding (the `count` field of a
/// `BindGroupLayoutEntry`).
///
/// V15 report: the PRS `ArraySize` must be passed through verbatim (D3D12
/// rejects both directions on mismatch); `count: None` means a single
/// binding.
pub fn binding_count(entry: &wgpu_types::BindGroupLayoutEntry) -> u32 {
    entry.count.map(|n| n.get()).unwrap_or(1)
}

/// The Diligent variable type tier for a wgpu binding (plan §8.2 layering).
///
/// * `DYNAMIC` - buffer bindings with `has_dynamic_offset` (the wgpu
///   dynamic-offset bindings: the bevy view group's high-frequency uniform
///   slots, driven through `SetBufferOffset` per draw).
/// * `MUTABLE` - every other BGL binding (the wgpu semantics are "bound per
///   SRB"; the SRB-side variables are set once at `create_bind_group`).
/// * `STATIC` - reserved: the plan assigns it to immutable samplers and
///   view-independent constants, which the wgpu BGL descriptor cannot
///   express (no immutable-sampler signal), so no BGL entry ever derives it.
///
/// The tier lands on both the SRB-side (canonical) and the PSO-side
/// (shader-named) PRS through `pipeline_resource_desc`.
pub fn binding_var_type(entry: &wgpu_types::BindGroupLayoutEntry) -> sys::SHADER_RESOURCE_VARIABLE_TYPE {
    let vt = |v: sys::_SHADER_RESOURCE_VARIABLE_TYPE| v as sys::SHADER_RESOURCE_VARIABLE_TYPE;
    match entry.ty {
        wgpu_types::BindingType::Buffer {
            has_dynamic_offset: true,
            ..
        } => vt(sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC),
        _ => vt(sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE),
    }
}

/// The PRS resource flags for a wgpu binding (V17 rung 2 discipline + V19
/// RUNTIME_ARRAY).
///
/// Two flag families, combined with bitwise OR:
///
/// 1. **`NO_DYNAMIC_BUFFERS`** (every **non-dynamic buffer-typed**
///    variable): releases the dynamic-buffer budget (V17: a partial-range
///    `SetBufferRange` counts as a dynamic binding unless the variable
///    carries the flag) and switches the D3D12 slot from a CBV/SRV root
///    view to a descriptor table. Only valid on `CONSTANT_BUFFER` /
///    `BUFFER_SRV` / `BUFFER_UAV` resources (the engine's
///    `GetValidPipelineResourceFlags` rejects it elsewhere); dynamic
///    variables must NOT carry it (`SetBufferOffset` is rejected for
///    NO_DYNAMIC_BUFFERS variables - ShaderResourceVariableBase.hpp:750-754).
///
/// 2. **`RUNTIME_ARRAY`** (texture/sampler bindings declared with a wgpu
///    `count` - the bindless material arrays, `bindless.wgsl:19-35` nine
///    unbounded `binding_array` declarations whose layout side is bounded at
///    2048/64 via `.count(slab_limit)`): marks the PRS resource as a
///    runtime-sized shader array (`PipelineResourceSignature.h:141-142`).
///    V19 verified `{TEXTURE_SRV, ArraySize=5000, RUNTIME_ARRAY}` on D3D12.
///    The wgpu-side unbounded declaration + bounded layout count + PRS
///    RUNTIME_ARRAY is the v2.0 binding-model mechanism for all thirteen
///    unbounded arrays (bindless 9 + Solari 4). The flag is only valid on
///    non-buffer resources (buffers use ArraySize without the runtime flag).
pub fn binding_resource_flags(
    entry: &wgpu_types::BindGroupLayoutEntry,
    resource_type: sys::SHADER_RESOURCE_TYPE,
) -> sys::PIPELINE_RESOURCE_FLAGS {
    let is_buffer = resource_type
        == sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER
            as sys::SHADER_RESOURCE_TYPE
        || resource_type
            == sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV
                as sys::SHADER_RESOURCE_TYPE
        || resource_type
            == sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_UAV
                as sys::SHADER_RESOURCE_TYPE;
    let is_dynamic = matches!(
        entry.ty,
        wgpu_types::BindingType::Buffer {
            has_dynamic_offset: true,
            ..
        }
    );
    let mut flags: sys::PIPELINE_RESOURCE_FLAGS = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE
        as sys::PIPELINE_RESOURCE_FLAGS;
    if is_buffer && !is_dynamic {
        flags |= sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS
            as sys::PIPELINE_RESOURCE_FLAGS;
    }
    // V19: unbounded texture/sampler arrays (bindless + Solari scene arrays)
    // become runtime-sized PRS resources. Distinguishing from the bounded
    // `binding_array` declarations (mesh_view_bindings ×8u, lightmap ×4u -
    // v2.0: 6 bounded, ArraySize only): the unbounded arrays' layout-side
    // `count` is the slab/resource limit (>= 64: bindless 2048/64, Solari
    // 500/5000), while bounded arrays cap at small counts (<= 8). The
    // runtime flag is exclusive to non-buffer resources.
    if !is_buffer && entry.count.is_some() && binding_count(entry) >= 64 {
        flags |= sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_RUNTIME_ARRAY
            as sys::PIPELINE_RESOURCE_FLAGS;
    }
    flags
}

/// The ascending binding indices of the buffer entries with
/// `has_dynamic_offset` in a bind group layout.
///
/// wgpu orders dynamic offsets by ascending binding index; the list
/// maps the `set_bind_group(..., &[u32])` offset array to the SRB variables
/// one-to-one (§6.1.1).
///
/// M2a-1 review, fix 1: the entries' declaration order is not guaranteed to
/// be ascending (wgpu's BGL validation that enforced it is gone in this
/// fork), so the indices are sorted here - the ascending-order contract is
/// enforced at this single point instead of at every PRS/offset consumer.
pub fn dynamic_buffer_bindings(entries: &[wgpu_types::BindGroupLayoutEntry]) -> Vec<u32> {
    let mut bindings: Vec<u32> = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.ty,
                wgpu_types::BindingType::Buffer {
                    has_dynamic_offset: true,
                    ..
                }
            )
        })
        .map(|entry| entry.binding)
        .collect();
    bindings.sort_unstable();
    bindings
}

/// The `GetVariableByName` probe stages for an SRB built from a bind group
/// layout.
///
/// The engine derives the PRS's pipeline type from the union of its
/// resources' shader stages (`m_PipelineType =
/// PipelineTypeFromShaderStages(m_ShaderStages)`,
/// PipelineResourceSignatureBase.hpp:829; the type rule in
/// GraphicsAccessories.cpp:2366: any graphics stage -> GRAPHICS, COMPUTE
/// only -> COMPUTE), and probing a stage that is invalid for the pipeline
/// type makes the engine log a warning per probe
/// (ShaderResourceBindingBase.hpp:185) - e.g. VERTEX/PIXEL probes on a
/// compute signature. Every binding's variable lives in one of its visible
/// stages' managers, so probing only the consistent stages is both silent
/// and complete.
pub fn srb_variable_probe_stages(
    entries: &[wgpu_types::BindGroupLayoutEntry],
) -> wgpu_types::ShaderStages {
    use wgpu_types::ShaderStages;
    let mut all = ShaderStages::empty();
    for entry in entries {
        all |= entry.visibility;
    }
    if all.contains(ShaderStages::COMPUTE) && !all.intersects(ShaderStages::VERTEX_FRAGMENT) {
        ShaderStages::COMPUTE
    } else {
        all.intersection(ShaderStages::VERTEX_FRAGMENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_usages_map_to_bind_flags() {
        let vb = sys::_BIND_FLAGS::BIND_VERTEX_BUFFER as sys::BIND_FLAGS;
        let ib = sys::_BIND_FLAGS::BIND_INDEX_BUFFER as sys::BIND_FLAGS;
        let ub = sys::_BIND_FLAGS::BIND_UNIFORM_BUFFER as sys::BIND_FLAGS;
        assert_eq!(buffer_usage_to_bind_flags(BufferUsages::VERTEX), vb);
        assert_eq!(buffer_usage_to_bind_flags(BufferUsages::INDEX), ib);
        assert_eq!(buffer_usage_to_bind_flags(BufferUsages::UNIFORM), ub);
        assert_eq!(buffer_usage_to_bind_flags(BufferUsages::empty()), 0);

        let storage = buffer_usage_to_bind_flags(BufferUsages::STORAGE);
        assert_ne!(storage & (sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as u32), 0);
        assert_ne!(storage & (sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as u32), 0);

        let combined = buffer_usage_to_bind_flags(BufferUsages::VERTEX | BufferUsages::INDEX);
        assert_eq!(combined, vb | ib);
    }

    #[test]
    fn map_usage_picks_staging() {
        let staging = sys::_USAGE::USAGE_STAGING as sys::USAGE;
        assert_eq!(buffer_usage_to_usage(BufferUsages::MAP_READ), staging);
        assert_eq!(buffer_usage_to_usage(BufferUsages::MAP_WRITE), staging);
        let default = sys::_USAGE::USAGE_DEFAULT as sys::USAGE;
        assert_eq!(buffer_usage_to_usage(BufferUsages::VERTEX), default);
        let read = sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as u8;
        assert_eq!(buffer_usage_to_cpu_access(BufferUsages::MAP_READ), read);

        // M1-4b-1: the readback pool's COPY_DST|MAP_READ usage must create
        // a staging buffer with zero bind flags (BufferBase.cpp:105-106:
        // staging buffers can't be bound to the pipeline).
        let readback = BufferUsages::COPY_DST | BufferUsages::MAP_READ;
        assert_eq!(buffer_usage_to_usage(readback), staging);
        assert_eq!(buffer_usage_to_bind_flags(readback), 0);
    }

    #[test]
    fn texture_usages_map_to_bind_flags() {
        let srv = sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as u32;
        assert_eq!(
            texture_usage_to_bind_flags(wgpu_types::TextureUsages::TEXTURE_BINDING),
            srv
        );
        let attachment = texture_usage_to_bind_flags(wgpu_types::TextureUsages::RENDER_ATTACHMENT);
        assert_ne!(attachment & (sys::_BIND_FLAGS::BIND_RENDER_TARGET as u32), 0);
        assert_ne!(attachment & (sys::_BIND_FLAGS::BIND_DEPTH_STENCIL as u32), 0);
    }

    #[test]
    fn vertex_formats_map_to_value_types() {
        let f32 = sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE;
        assert_eq!(
            vertex_format_to_value_type(wgpu_types::VertexFormat::Float32x3),
            Some((f32, 3, false))
        );
        let u8 = sys::_VALUE_TYPE::VT_UINT8 as sys::VALUE_TYPE;
        assert_eq!(
            vertex_format_to_value_type(wgpu_types::VertexFormat::Unorm8x4),
            Some((u8, 4, true))
        );
        let u16 = sys::_VALUE_TYPE::VT_UINT16 as sys::VALUE_TYPE;
        assert_eq!(
            vertex_format_to_value_type(wgpu_types::VertexFormat::Unorm16x2),
            Some((u16, 2, true))
        );
        assert!(vertex_format_to_value_type(wgpu_types::VertexFormat::Float64).is_none());
        assert!(vertex_format_to_value_type(wgpu_types::VertexFormat::Unorm10_10_10_2).is_none());
    }

    #[test]
    fn compare_functions_map_directly() {
        use wgpu_types::CompareFunction as C;
        let expect = |v: sys::_COMPARISON_FUNCTION| v as sys::COMPARISON_FUNCTION;
        for (w, d) in [
            (C::Never, sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_NEVER),
            (C::Less, sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS),
            (C::LessEqual, sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS_EQUAL),
            (C::Greater, sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_GREATER),
            (C::Always, sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS),
        ] {
            assert_eq!(comparison_function(w), expect(d));
        }
    }

    #[test]
    fn filters_cover_comparison_and_anisotropy() {
        let point = sys::_FILTER_TYPE::FILTER_TYPE_POINT as sys::FILTER_TYPE;
        let linear = sys::_FILTER_TYPE::FILTER_TYPE_LINEAR as sys::FILTER_TYPE;
        let aniso = sys::_FILTER_TYPE::FILTER_TYPE_ANISOTROPIC as sys::FILTER_TYPE;
        let cpoint = sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_POINT as sys::FILTER_TYPE;
        let clinear = sys::_FILTER_TYPE::FILTER_TYPE_COMPARISON_LINEAR as sys::FILTER_TYPE;
        assert_eq!(filter_type(FilterMode::Nearest, false, false), point);
        assert_eq!(filter_type(FilterMode::Linear, false, false), linear);
        assert_eq!(filter_type(FilterMode::Linear, false, true), aniso);
        assert_eq!(filter_type(FilterMode::Nearest, true, false), cpoint);
        assert_eq!(filter_type(FilterMode::Linear, true, false), clinear);
    }

    #[test]
    fn blend_state_maps_directly() {
        use wgpu_types::{BlendFactor as BF, BlendOperation as BO};
        let one = sys::_BLEND_FACTOR::BLEND_FACTOR_ONE as sys::BLEND_FACTOR;
        let rev = sys::_BLEND_OPERATION::BLEND_OPERATION_REV_SUBTRACT as sys::BLEND_OPERATION;
        assert_eq!(blend_factor(BF::One), one);
        assert_eq!(blend_operation(BO::ReverseSubtract), rev);
        assert_eq!(blend_factor(BF::SrcAlphaSaturated), sys::_BLEND_FACTOR::BLEND_FACTOR_SRC_ALPHA_SAT as sys::BLEND_FACTOR);
        assert_eq!(blend_factor(BF::Constant), sys::_BLEND_FACTOR::BLEND_FACTOR_BLEND_FACTOR as sys::BLEND_FACTOR);
    }

    #[test]
    fn color_writes_is_a_bit_copy() {
        assert_eq!(
            color_writes(wgpu_types::ColorWrites::ALL),
            sys::_COLOR_MASK::COLOR_MASK_ALL as sys::COLOR_MASK
        );
        // COLOR (RGB) is the ALL mask minus the alpha bit.
        let rgb = (sys::_COLOR_MASK::COLOR_MASK_RED as u8
            | sys::_COLOR_MASK::COLOR_MASK_GREEN as u8
            | sys::_COLOR_MASK::COLOR_MASK_BLUE as u8) as sys::COLOR_MASK;
        assert_eq!(color_writes(wgpu_types::ColorWrites::COLOR), rgb);
        assert_eq!(
            color_writes(wgpu_types::ColorWrites::ALPHA),
            sys::_COLOR_MASK::COLOR_MASK_ALPHA as sys::COLOR_MASK
        );
        assert_eq!(
            color_writes(wgpu_types::ColorWrites::empty()),
            sys::_COLOR_MASK::COLOR_MASK_NONE as sys::COLOR_MASK
        );
    }

    #[test]
    fn stencil_ops_map_directly() {
        use wgpu_types::StencilOperation as S;
        for (w, d) in [
            (S::Keep, sys::_STENCIL_OP::STENCIL_OP_KEEP),
            (S::Zero, sys::_STENCIL_OP::STENCIL_OP_ZERO),
            (S::IncrementClamp, sys::_STENCIL_OP::STENCIL_OP_INCR_SAT),
            (S::DecrementWrap, sys::_STENCIL_OP::STENCIL_OP_DECR_WRAP),
        ] {
            assert_eq!(stencil_operation(w), d as sys::STENCIL_OP);
        }
    }

    #[test]
    fn topology_and_raster_state_map() {
        let tri = sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
        assert_eq!(primitive_topology(wgpu_types::PrimitiveTopology::TriangleList), tri);
        assert_eq!(
            fill_mode(wgpu_types::PolygonMode::Fill),
            Some(sys::_FILL_MODE::FILL_MODE_SOLID as sys::FILL_MODE)
        );
        assert_eq!(fill_mode(wgpu_types::PolygonMode::Point), None);
        assert_eq!(cull_mode(None), sys::_CULL_MODE::CULL_MODE_NONE as sys::CULL_MODE);
        assert_eq!(cull_mode(Some(wgpu_types::Face::Front)), sys::_CULL_MODE::CULL_MODE_FRONT as sys::CULL_MODE);
        assert_eq!(cull_mode(Some(wgpu_types::Face::Back)), sys::_CULL_MODE::CULL_MODE_BACK as sys::CULL_MODE);
    }

    #[test]
    fn shader_stages_map_to_type_bits() {
        let vs = sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as u32;
        let ps = sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as u32;
        let cs = sys::_SHADER_TYPE::SHADER_TYPE_COMPUTE as u32;
        assert_eq!(shader_stages(wgpu_types::ShaderStages::VERTEX), vs);
        assert_eq!(shader_stages(wgpu_types::ShaderStages::VERTEX_FRAGMENT), vs | ps);
        assert_eq!(shader_stages(wgpu_types::ShaderStages::COMPUTE), cs);
        assert_eq!(shader_stages(wgpu_types::ShaderStages::empty()), 0);
    }

    #[test]
    fn binding_types_map_to_resource_types() {
        use wgpu_types::{BindingType, BufferBindingType, TextureSampleType, TextureViewDimension};
        let uniform = BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        assert_eq!(
            binding_type_to_resource_type(&uniform),
            Some(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE)
        );
        let readonly = BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        assert_eq!(
            binding_type_to_resource_type(&readonly),
            Some(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV as sys::SHADER_RESOURCE_TYPE)
        );
        let tex = BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        };
        let entry = wgpu_types::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty: tex,
            count: Some(std::num::NonZeroU32::new(4).unwrap()),
        };
        assert_eq!(binding_count(&entry), 4);
        let single = wgpu_types::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: uniform,
            count: None,
        };
        assert_eq!(binding_count(&single), 1);
    }

    /// The §6.1.1 mapping table, complete: every wgpu `BindingType` maps to
    /// exactly one Diligent `SHADER_RESOURCE_TYPE` (external textures have
    /// none - the only `None` case).
    #[test]
    fn binding_model_mapping_table_is_complete() {
        use wgpu_types::{
            BindingType, BufferBindingType, SamplerBindingType, StorageTextureAccess, TextureFormat,
            TextureSampleType, TextureViewDimension,
        };
        let rt = |v: sys::_SHADER_RESOURCE_TYPE| v as sys::SHADER_RESOURCE_TYPE;
        let cb = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER);
        let bsrv = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV);
        let buav = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_UAV);
        let tsrv = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV);
        let tuav = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_UAV);
        let sam = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_SAMPLER);
        let accel = rt(sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_ACCEL_STRUCT);

        assert_eq!(
            binding_type_to_resource_type(&BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            }),
            Some(cb)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: None,
            }),
            Some(cb)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            }),
            Some(bsrv)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            }),
            Some(buav)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            }),
            Some(tsrv)
        );
        // The layout-level format field is dropped (§2.9 implementation
        // note): StorageTexture maps to TEXTURE_UAV regardless of its
        // declared format - the view carries the real format.
        for format in [TextureFormat::Rgba8Unorm, TextureFormat::Rgba16Float] {
            assert_eq!(
                binding_type_to_resource_type(&BindingType::StorageTexture {
                    access: StorageTextureAccess::ReadWrite,
                    format,
                    view_dimension: TextureViewDimension::D2,
                }),
                Some(tuav)
            );
        }
        assert_eq!(
            binding_type_to_resource_type(&BindingType::Sampler(
                SamplerBindingType::Filtering
            )),
            Some(sam)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::AccelerationStructure {
                vertex_return: false,
            }),
            Some(accel)
        );
        assert_eq!(
            binding_type_to_resource_type(&BindingType::ExternalTexture),
            None
        );
    }

    /// VarType layering (§8.2): DYNAMIC iff a buffer binding carries
    /// `has_dynamic_offset`; everything else is MUTABLE. STATIC is not
    /// derivable from a wgpu BGL (no immutable-sampler signal) - reserved.
    #[test]
    fn var_type_tiers_map_dynamic_and_mutable() {
        use wgpu_types::{BindingType, BufferBindingType, TextureSampleType, TextureViewDimension};
        let entry = |binding, ty| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let dynamic = sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
        let mutable = sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;

        // The bevy view group's high-frequency uniform slots (mesh_view_bindings:
        // ViewUniform / GpuLights / LightProbes / GpuFog / SSR / ContactShadows / OIT).
        for (binding, has_dynamic_offset) in [(0u32, true), (1, true), (12, true), (13, true)] {
            assert_eq!(
                binding_var_type(&entry(
                    binding,
                    BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset,
                        min_binding_size: None,
                    }
                )),
                dynamic
            );
        }
        // Non-dynamic buffers, textures, storage textures and samplers are
        // MUTABLE (bound once per SRB).
        assert_eq!(
            binding_var_type(&entry(
                8,
                BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }
            )),
            mutable
        );
        assert_eq!(
            binding_var_type(&entry(
                9,
                BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }
            )),
            mutable
        );
        assert_eq!(
            binding_var_type(&entry(
                10,
                BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                }
            )),
            mutable
        );
        // Storage buffers CAN be dynamic in wgpu; the tier follows the flag,
        // not the binding type.
        assert_eq!(
            binding_var_type(&entry(
                11,
                BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: true,
                    min_binding_size: None,
                }
            )),
            dynamic
        );
    }

    /// V17 rung 2 discipline: `NO_DYNAMIC_BUFFERS` on every non-dynamic
    /// buffer-typed variable, never on dynamic ones, never on textures or
    /// samplers (the engine's `GetValidPipelineResourceFlags` only allows
    /// the flag on CONSTANT_BUFFER / BUFFER_SRV / BUFFER_UAV).
    #[test]
    fn no_dynamic_flags_land_on_non_dynamic_buffer_variables() {
        use wgpu_types::{BindingType, BufferBindingType, TextureSampleType, TextureViewDimension};
        let entry = |ty| wgpu_types::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let cb = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER
            as sys::SHADER_RESOURCE_TYPE;
        let bsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV
            as sys::SHADER_RESOURCE_TYPE;
        let buav = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_UAV
            as sys::SHADER_RESOURCE_TYPE;
        let tsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV
            as sys::SHADER_RESOURCE_TYPE;
        let no_dynamic = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS
            as sys::PIPELINE_RESOURCE_FLAGS;
        let none = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE
            as sys::PIPELINE_RESOURCE_FLAGS;

        // Non-dynamic uniform + storage buffers carry the flag.
        assert_eq!(
            binding_resource_flags(
                &entry(BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }),
                cb
            ),
            no_dynamic
        );
        assert_eq!(
            binding_resource_flags(
                &entry(BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }),
                bsrv
            ),
            no_dynamic
        );
        assert_eq!(
            binding_resource_flags(
                &entry(BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }),
                buav
            ),
            no_dynamic
        );
        // Dynamic variables must NOT carry the flag (SetBufferOffset is
        // rejected on NO_DYNAMIC_BUFFERS variables).
        assert_eq!(
            binding_resource_flags(
                &entry(BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                }),
                cb
            ),
            none
        );
        // Textures/samplers: no flag (invalid per GetValidPipelineResourceFlags).
        assert_eq!(
            binding_resource_flags(
                &entry(BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                }),
                tsrv
            ),
            none
        );
    }

    /// V15 discipline: the PRS `ArraySize` is the BGL `count` verbatim - the
    /// six bounded-array cases of the bevy renderers in equivalent form:
    /// mesh_view_bindings.wgsl:135/136/147/157 (`binding_array<_, 8u>`,
    /// texture entries at bindings 0/1/3/6) and lightmap.wgsl:6-7
    /// (`binding_array<_, 4>`, texture + sampler at bindings 4/5). No
    /// rounding, no expansion.
    #[test]
    fn bounded_array_counts_pass_through_verbatim() {
        use wgpu_types::{
            BindingType, SamplerBindingType, TextureSampleType, TextureViewDimension,
        };
        let texture = |binding, dimension, count| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: dimension,
                multisampled: false,
            },
            count: Some(std::num::NonZeroU32::new(count).unwrap()),
        };
        let sampler = |binding, count| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: Some(std::num::NonZeroU32::new(count).unwrap()),
        };

        // mesh_view_bindings.wgsl:135/136 - diffuse/specular environment maps.
        assert_eq!(
            binding_count(&texture(0, TextureViewDimension::Cube, 8)),
            8
        );
        assert_eq!(
            binding_count(&texture(1, TextureViewDimension::Cube, 8)),
            8
        );
        // mesh_view_bindings.wgsl:147 - irradiance volumes.
        assert_eq!(
            binding_count(&texture(3, TextureViewDimension::D3, 8)),
            8
        );
        // mesh_view_bindings.wgsl:157 - clustered decal textures.
        assert_eq!(
            binding_count(&texture(6, TextureViewDimension::D2, 8)),
            8
        );
        // lightmap.wgsl:6-7 - lightmap texture + sampler arrays.
        assert_eq!(
            binding_count(&texture(4, TextureViewDimension::D2, 4)),
            4
        );
        assert_eq!(binding_count(&sampler(5, 4)), 4);

        // And the flag tier stays MUTABLE for arrayed textures (no flag).
        let tsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV
            as sys::SHADER_RESOURCE_TYPE;
        assert_eq!(
            binding_resource_flags(&texture(0, TextureViewDimension::Cube, 8), tsrv),
            0
        );
        assert_eq!(
            binding_var_type(&texture(0, TextureViewDimension::Cube, 8)),
            sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
                as sys::SHADER_RESOURCE_VARIABLE_TYPE
        );
    }

    /// V19 RUNTIME_ARRAY discipline: unbounded texture/sampler arrays (the
    /// bindless material arrays - bindless.wgsl:19-35, layout-side count
    /// 2048/64 - and the Solari scene arrays with 500/5000) carry
    /// `PIPELINE_RESOURCE_FLAG_RUNTIME_ARRAY`; bounded arrays (count <= 8)
    /// and single bindings do not. Buffers never carry the runtime flag.
    #[test]
    fn unbounded_arrays_carry_runtime_array_flag() {
        use wgpu_types::{
            BindingType, SamplerBindingType, TextureSampleType, TextureViewDimension,
        };
        let texture = |binding, count| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: Some(std::num::NonZeroU32::new(count).unwrap()),
        };
        let sampler = |binding, count| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: Some(std::num::NonZeroU32::new(count).unwrap()),
        };
        let tsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV
            as sys::SHADER_RESOURCE_TYPE;
        let sampler_rt = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_SAMPLER
            as sys::SHADER_RESOURCE_TYPE;
        let runtime = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_RUNTIME_ARRAY
            as sys::PIPELINE_RESOURCE_FLAGS;

        // Bindless slab: 2048 (non-Apple) / 64 (Apple). Both >= 64.
        assert_eq!(binding_resource_flags(&texture(1, 2048), tsrv), runtime);
        assert_eq!(binding_resource_flags(&texture(2, 64), tsrv), runtime);
        assert_eq!(binding_resource_flags(&sampler(1, 2048), sampler_rt), runtime);

        // Solari scene arrays (binder.rs: 500 / 5000) also carry the flag.
        assert_eq!(binding_resource_flags(&texture(2, 500), tsrv), runtime);
        assert_eq!(binding_resource_flags(&texture(2, 5000), tsrv), runtime);

        // Bounded arrays (mesh_view ×8, lightmap ×4) and single bindings
        // (count None) carry no runtime flag.
        assert_eq!(binding_resource_flags(&texture(0, 8), tsrv), 0);
        assert_eq!(binding_resource_flags(&sampler(5, 4), sampler_rt), 0);
        let single = wgpu_types::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu_types::ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        assert_eq!(binding_resource_flags(&single, tsrv), 0);
    }

    /// The dynamic-offset array maps to the layout's dynamic buffer bindings
    /// in ascending binding order (the wgpu `set_bind_group` offset order).
    #[test]
    fn dynamic_buffer_bindings_are_ascending() {
        use wgpu_types::{BindingType, BufferBindingType};
        let entry = |binding, ty| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let dynamic_uniform =
            |has| BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: has,
                min_binding_size: None,
            };
        // A view-group-shaped layout: dynamic at 0/1/12, static buffers and
        // textures in between.
        let entries = vec![
            entry(0, dynamic_uniform(true)),
            entry(1, dynamic_uniform(true)),
            entry(2, BindingType::Texture {
                sample_type: wgpu_types::TextureSampleType::Depth,
                view_dimension: wgpu_types::TextureViewDimension::Cube,
                multisampled: false,
            }),
            entry(12, dynamic_uniform(true)),
            entry(13, dynamic_uniform(false)),
        ];
        assert_eq!(dynamic_buffer_bindings(&entries), vec![0, 1, 12]);
        assert_eq!(dynamic_buffer_bindings(&[]), Vec::<u32>::new());
    }

    /// M2a-1 review, fix 1: the binding indices must be sorted even when the
    /// layout declares them out of order (the wgpu BGL validation that used
    /// to require ascending declaration order is gone in this fork, so the
    /// sort in `dynamic_buffer_bindings` is the contract's only enforcer).
    #[test]
    fn dynamic_buffer_bindings_sort_unsorted_input() {
        use wgpu_types::{BindingType, BufferBindingType};
        let entry = |binding, ty| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let dynamic_uniform =
            |has| BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: has,
                min_binding_size: None,
            };
        // Binding indices in descending order, static bindings interleaved -
        // the ascending output is what the offset array maps against.
        let entries = vec![
            entry(12, dynamic_uniform(true)),
            entry(7, dynamic_uniform(false)),
            entry(5, dynamic_uniform(true)),
            entry(0, dynamic_uniform(true)),
            entry(9, dynamic_uniform(true)),
        ];
        assert_eq!(dynamic_buffer_bindings(&entries), vec![0, 5, 9, 12]);
    }

    /// The `GetVariableByName` probe stages mirror the engine's
    /// `PipelineTypeFromShaderStages` rule (GraphicsAccessories.cpp:2366):
    /// an SRB built from a compute-only layout is probed on COMPUTE only
    /// (VERTEX/PIXEL probes would log an engine warning per probe,
    /// ShaderResourceBindingBase.hpp:185); any graphics visibility makes
    /// the SRB a graphics signature, probed on the graphics stages.
    #[test]
    fn srb_variable_probe_stages_follow_the_pipeline_type_rule() {
        use wgpu_types::{
            BindingType, ShaderStages as S, TextureSampleType, TextureViewDimension,
        };
        let entry = |visibility| wgpu_types::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // Compute-only layout (e.g. the GPU-preprocessing bind groups):
        // COMPUTE probe only.
        assert_eq!(srb_variable_probe_stages(&[entry(S::COMPUTE)]), S::COMPUTE);
        assert_eq!(
            srb_variable_probe_stages(&[entry(S::COMPUTE), entry(S::COMPUTE)]),
            S::COMPUTE
        );
        // Any graphics stage makes the signature graphics-typed: probe the
        // graphics stages only (COMPUTE would warn on a graphics signature).
        assert_eq!(
            srb_variable_probe_stages(&[entry(S::VERTEX_FRAGMENT)]),
            S::VERTEX_FRAGMENT
        );
        assert_eq!(
            srb_variable_probe_stages(&[entry(S::VERTEX), entry(S::FRAGMENT)]),
            S::VERTEX_FRAGMENT
        );
        // Mixed compute + graphics visibility (shared bind groups): the
        // signature is graphics-typed (the engine rule: any graphics stage
        // wins) - probe the graphics stages.
        assert_eq!(
            srb_variable_probe_stages(&[entry(S::VERTEX_FRAGMENT), entry(S::COMPUTE)]),
            S::VERTEX_FRAGMENT
        );
        assert_eq!(srb_variable_probe_stages(&[]), S::empty());
    }
}
