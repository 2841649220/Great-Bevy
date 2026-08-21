//! Diligent PSO / PRS / shader creation for the M1b replacement points 1+2.
//!
//! This module translates wgpu pipeline descriptors into real Diligent
//! objects:
//!
//! * [`ShaderModuleRecord`] - the bevy_shader cache record: the transition
//!   wgpu module plus the naga module used to compile a Diligent `IShader`
//!   per shader stage (HLSL on D3D12, SPIR-V on Vulkan);
//! * [`PipelineLayoutRecord`] - the `LayoutCache` value: the "PRS array +
//!   immediate_size" combination the task brief §5.3.3-3/4 prescribes, plus
//!   the transition wgpu layout;
//! * PRS construction - canonical names for the SRB-side signature (built
//!   from a `BindGroupLayoutDescriptor` alone) and shader-derived names for
//!   the PSO-side signatures (V15 report: Diligent matches shader resources
//!   to PRS resources **by name** on D3D12; names come from the naga module
//!   globals, whose HLSL names are emitted verbatim);
//! * graphics/compute PSO creation through the raw `IRenderDevice` vtables
//!   (the diligent-rs wrapper only covers a single-RTV fixed-state subset;
//!   the raw path carries the full wgpu state translation, mirroring the
//!   wrapper's documented defaults - COLOR_MASK_ALL, SampleMask, ...).
//!
//! # Failure policy
//!
//! Every creation path returns `Option`/`Result` and never panics: a
//! failing diligent creation logs a `warn!` and degrades to `None` (the
//! transition wgpu object still exists and drives rendering until M2/M3).

use alloc::ffi::CString;
use alloc::sync::Arc;
use bevy_material::descriptor::BindGroupLayoutDescriptor;
use core::ffi::CStr;
use diligent_rs::diligent_sys::bindings as sys;
use std::sync::Mutex;

use super::{diligent_mapping, diligent_registry::DiligentHandle};
use crate::renderer::RenderDevice;
use crate::render_resource::{PipelineLayout, ShaderModule};

/// The backend the diligent device was created for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiligentBackend {
    D3D12,
    Vulkan,
    Other,
}

/// The bevy_shader cache record for a compiled shader module.
///
/// Replaces `WgpuWrapper<ShaderModule>` as the `ShaderCache` value type: the
/// module handle carries the naga module the Diligent per-stage shaders
/// compile from (M1-4b-2: the wgpu module is gone).
pub(crate) struct ShaderModuleRecord {
    /// The shader module handle (carries the naga module; the id is the
    /// registry key the descriptors resolve through).
    pub(crate) module: ShaderModule,
    /// Per-stage Diligent shaders (compiled lazily at PSO creation), keyed by
    /// (stage, entry point) - two entry points on the same stage compile
    /// distinct shaders.
    diligent: Mutex<Vec<((naga::ShaderStage, String), DiligentHandle<diligent_rs::Shader>)>>,
}

impl ShaderModuleRecord {
    pub(crate) fn new(module: ShaderModule) -> Self {
        Self {
            module,
            diligent: Mutex::new(Vec::new()),
        }
    }

    /// The naga module compiled by bevy_shader (or parsed from WGSL /
    /// SPIR-V). `None` when the source could not be parsed (only the
    /// Diligent shader is skipped).
    pub(crate) fn naga_module(&self) -> Option<&naga::Module> {
        self.module.naga_module()
    }

    /// The cached Diligent shader for `stage` + `entry_point`, compiling it
    /// from the naga module when missing (HLSL on D3D12, SPIR-V on Vulkan -
    /// brief point 5).
    fn diligent_shader(
        &self,
        device: &diligent_rs::RenderDevice,
        backend: DiligentBackend,
        stage: naga::ShaderStage,
        entry_point: &str,
    ) -> Result<DiligentHandle<diligent_rs::Shader>, String> {
        {
            let cache = self.diligent.lock().unwrap();
            if let Some(((_, _), shader)) =
                cache.iter().find(|((s, e), _)| *s == stage && e.as_str() == entry_point)
            {
                return Ok(shader.clone());
            }
        }
        let module = self
            .naga_module()
            .ok_or_else(|| "no naga module available for the diligent shader".to_string())?;
        let shader = compile_naga_shader(device, backend, module, stage, entry_point)?;
        let mut cache = self.diligent.lock().unwrap();
        if let Some((_, existing)) =
            cache.iter().find(|((s, e), _)| *s == stage && e.as_str() == entry_point)
        {
            return Ok(existing.clone());
        }
        cache.push(((stage, entry_point.to_string()), shader.clone()));
        Ok(shader)
    }
}

/// The `LayoutCache` value: "PRS 数组+immediate_size" combination record
/// (brief §5.3.3-3/4).
pub(crate) struct PipelineLayoutRecord {
    /// The PSO-side pipeline resource signatures (shader-named), one per
    /// bind group, in group order.
    pub(crate) prs: Vec<DiligentHandle<diligent_rs::PipelineResourceSignature>>,
    /// The dedicated immediate-constants PRS (only when `immediate_size` >
    /// 0 and the shaders declare an `Immediate` global).
    pub(crate) immediate_prs: Option<DiligentHandle<diligent_rs::PipelineResourceSignature>>,
    /// The immediate PRS resource name (the shader's `Immediate` global name
    /// - the `GetVariableByName` key for the `SetInlineConstants` path).
    pub(crate) immediate_name: Option<CString>,
    /// The `GetVariableByName` probe stages for the immediate SRB (M2a): the
    /// union of the shaders' entry-point stages - the same union the
    /// immediate PRS resource's `ShaderStages` is built from (probing a
    /// stage invalid for the signature's pipeline type logs an engine
    /// warning per probe, ShaderResourceBindingBase.hpp:185).
    pub(crate) immediate_probe_stages: wgpu_types::ShaderStages,
    /// wgpu immediate constants size in bytes (M2a: the
    /// `SetInlineConstants` bounds check of `set_immediates`).
    pub(crate) immediate_size: u32,
    /// The pipeline layout handle (M1-4b-2: the wgpu layout is gone; the
    /// handle's id is the registry key descriptors resolve through).
    pub(crate) layout: PipelineLayout,
}

impl PipelineLayoutRecord {
    /// The full PRS array for PSO creation: per-group signatures plus the
    /// dedicated immediate signature, in that order.
    pub(crate) fn pso_prs(&self) -> Vec<&diligent_rs::PipelineResourceSignature> {
        self.prs
            .iter()
            .map(|p| &**p)
            .chain(self.immediate_prs.iter().map(|p| &**p))
            .collect()
    }
}

/// Compiles the naga module into a Diligent shader for one stage.
///
/// D3D12: WGSL -> HLSL (via naga `back::hlsl`, SM 6.0) -> `create_shader`
/// (SHADER_SOURCE_LANGUAGE_HLSL). Vulkan: -> SPIR-V (`back::spv`) ->
/// `create_shader_spirv` (SHADER_SOURCE_LANGUAGE_BYTECODE, the locked
/// version's SPIR-V entry point - see the M1-1 report §5).
fn compile_naga_shader(
    device: &diligent_rs::RenderDevice,
    backend: DiligentBackend,
    module: &naga::Module,
    stage: naga::ShaderStage,
    entry_point: &str,
) -> Result<DiligentHandle<diligent_rs::Shader>, String> {
    // Validate once; both backends need the ModuleInfo.
    let module_info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(module)
    .map_err(|e| format!("naga validation failed: {e}"))?;

    let shader_type = diligent_mapping::shader_type_from_naga(stage);
    let name = format!("bevy_{entry_point}");
    match backend {
        DiligentBackend::D3D12 => {
            let mut hlsl_options = naga::back::hlsl::Options::default();
            hlsl_options.shader_model = naga::back::hlsl::ShaderModel::V6_0;
            hlsl_options.immediates_target = immediate_target(module);
            let pipeline_options = naga::back::hlsl::PipelineOptions {
                entry_point: Some((stage, entry_point.to_string())),
            };
            let mut output = String::new();
            let mut writer = naga::back::hlsl::Writer::new(
                &mut output,
                &hlsl_options,
                &pipeline_options,
            );
            let fragment_entry_point = (stage == naga::ShaderStage::Fragment)
                .then(|| naga::back::hlsl::FragmentEntryPoint::new(module, entry_point))
                .flatten();
            writer
                .write(
                    module,
                    &module_info,
                    fragment_entry_point.as_ref(),
                )
                .map_err(|e| format!("naga HLSL generation failed: {e}"))?;
            device
                .create_shader(&name, &output, shader_type)
                .map(|s| DiligentHandle::new(Arc::new(s)))
                .map_err(|e| format!("Diligent HLSL shader creation failed: {e}"))
        }
        DiligentBackend::Vulkan => {
            let spv_options = naga::back::spv::Options::default();
            let pipeline_options = naga::back::spv::PipelineOptions {
                shader_stage: stage,
                entry_point: entry_point.to_string(),
            };
            let words = naga::back::spv::write_vec(
                module,
                &module_info,
                &spv_options,
                Some(&pipeline_options),
            )
            .map_err(|e| format!("naga SPIR-V generation failed: {e}"))?;
            let bytes: Vec<u8> = words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect();
            device
                .create_shader_spirv(&name, &bytes, shader_type)
                .map(|s| DiligentHandle::new(Arc::new(s)))
                .map_err(|e| format!("Diligent SPIR-V shader creation failed: {e}"))
        }
        DiligentBackend::Other => Err("no diligent backend configured".to_string()),
    }
}

/// The register target for the shader's `Immediate` global, when present
/// (naga hlsl requires `immediates_target` for `AddressSpace::Immediate`
/// globals; the D3D12 remapper reassigns registers, so the value only needs
/// to be consistent).
fn immediate_target(module: &naga::Module) -> Option<naga::back::hlsl::BindTarget> {
    module
        .global_variables
        .iter()
        .any(|(_, g)| g.space == naga::AddressSpace::Immediate)
        .then(|| naga::back::hlsl::BindTarget {
            register: 0,
            space: 0,
            binding_array_size: None,
            dynamic_storage_buffer_offsets_index: None,
            restrict_indexing: false,
        })
}

/// Builds one `PipelineResourceDesc` for a wgpu bind group layout entry.
///
/// `name` must be the shader's HLSL variable name (V15 report: PRS resources
/// are matched to shader resources by name on D3D12). `group` is the bind
/// group index; the entries are processed in `binding` order so the PRS
/// resource order equals the binding order (the SRB "BindingIndex 消歧"
/// rule from the task brief point 6).
///
/// M2a (binding model, §6.1.1):
/// * VarType follows the §8.2 layering: DYNAMIC for `has_dynamic_offset`
///   buffer bindings (the `SetBufferOffset` path), MUTABLE otherwise;
/// * ArraySize is the BGL `count` verbatim (V15 discipline - no rounding,
///   no expansion);
/// * non-dynamic buffer variables carry `PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS`
///   (V17 rung 2 discipline - the flag releases the dynamic-buffer budget
///   and is only valid on CONSTANT_BUFFER/BUFFER_SRV/BUFFER_UAV resources).
fn pipeline_resource_desc(
    group: u32,
    entry: &wgpu_types::BindGroupLayoutEntry,
    name: &CStr,
) -> Result<sys::PipelineResourceDesc, String> {
    let resource_type = diligent_mapping::binding_type_to_resource_type(&entry.ty).ok_or_else(
        || format!("binding {group}:{} has no Diligent resource type", entry.binding),
    )?;
    let mut desc: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    desc.Name = name.as_ptr();
    desc.ShaderStages = diligent_mapping::shader_stages(entry.visibility);
    // V15: verbatim pass-through of the BGL array size (D3D12 rejects both
    // directions on mismatch); `count: None` = a single binding.
    desc.ArraySize = diligent_mapping::binding_count(entry);
    desc.ResourceType = resource_type;
    desc.VarType = diligent_mapping::binding_var_type(entry);
    desc.Flags = diligent_mapping::binding_resource_flags(entry, resource_type);
    Ok(desc)
}

/// The canonical SRB-side PRS resource name for a binding (used when the
/// shader names are unknown - the PRS stays compatible with the PSO-side
/// signature, whose resources are name-independent per
/// `IPipelineResourceSignature::IsCompatibleWith`).
pub(crate) fn canonical_prs_name(binding: u32) -> String {
    format!("binding_{binding}")
}

/// Creates the SRB-side PRS for a bind group layout descriptor (canonical
/// `binding_{n}` names, `BindGroupLayoutDescriptor` content hash -> PRS,
/// brief §5.3.3-5).
///
/// The resources are sorted by binding index so the PRS resource order
/// equals the binding order - the invariant `create_bind_group` relies on
/// for the canonical-name lookup (`GetVariableByName("binding_{binding}")`,
/// which is order-independent, but keeps the PRS order-predictable for the
/// SRB-side resource iteration).
pub(crate) fn create_canonical_prs(
    device: &diligent_rs::RenderDevice,
    descriptor: &BindGroupLayoutDescriptor,
) -> Result<DiligentHandle<diligent_rs::PipelineResourceSignature>, String> {
    let mut entries: Vec<&wgpu_types::BindGroupLayoutEntry> = descriptor.entries.iter().collect();
    entries.sort_by_key(|entry| entry.binding);
    let mut names = Vec::with_capacity(entries.len());
    let mut resources = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = CString::new(canonical_prs_name(entry.binding))
            .map_err(|e| format!("PRS name: {e}"))?;
        resources.push(pipeline_resource_desc(0, entry, &name)?);
        names.push(name);
    }
    device
        .create_pipeline_resource_signature(
            &format!("prs_{}", descriptor.label),
            &resources,
        )
        .map(|p| DiligentHandle::new(Arc::new(p)))
        .map_err(|e| format!("PRS creation failed: {e}"))
}

/// Extracts (group, binding) -> shader variable name from a naga module.
///
/// The naga global names are emitted verbatim as HLSL resource names (the
/// hlsl namer only mangles colliding names, and naga requires unique global
/// names), so these are exactly the names the Diligent remapper matches.
pub(crate) fn shader_resource_names(
    module: &naga::Module,
) -> std::collections::HashMap<(u32, u32), String> {
    module
        .global_variables
        .iter()
        .filter_map(|(_, global)| {
            let binding = global.binding?;
            let name = global.name.clone()?;
            Some(((binding.group, binding.binding), name))
        })
        .collect()
}

/// The shader variable name for (group, binding), or the canonical fallback.
fn shader_name_for(
    names: &[&std::collections::HashMap<(u32, u32), String>],
    group: u32,
    binding: u32,
) -> String {
    for map in names {
        if let Some(name) = map.get(&(group, binding)) {
            return name.clone();
        }
    }
    canonical_prs_name(binding)
}

/// Creates the PSO-side PRS for one bind group (shader-derived names).
///
/// The resources are built from the bind group layout descriptor entries in
/// binding order (matching the canonical SRB-side signature for
/// compatibility); names come from the shader modules.
pub(crate) fn create_shader_named_prs(
    device: &diligent_rs::RenderDevice,
    group: u32,
    descriptor: &BindGroupLayoutDescriptor,
    shader_names: &[&std::collections::HashMap<(u32, u32), String>],
) -> Result<DiligentHandle<diligent_rs::PipelineResourceSignature>, String> {
    let mut entries: Vec<&wgpu_types::BindGroupLayoutEntry> = descriptor.entries.iter().collect();
    entries.sort_by_key(|entry| entry.binding);
    let mut names = Vec::with_capacity(entries.len());
    let mut resources = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = CString::new(shader_name_for(shader_names, group, entry.binding))
            .map_err(|e| format!("PRS name: {e}"))?;
        resources.push(pipeline_resource_desc(group, entry, &name)?);
        names.push(name);
    }
    device
        .create_pipeline_resource_signature(
            &format!("prs_{}_g{}", descriptor.label, group),
            &resources,
        )
        .map(|p| DiligentHandle::new(Arc::new(p)))
        .map_err(|e| format!("PRS creation failed: {e}"))
}

/// The shader stages of the immediate PRS resource: the union of every
/// entry-point stage in the modules (the same union `create_immediate_prs`
/// writes into the PRS resource's `ShaderStages` - the probe stages for the
/// immediate SRB's `GetVariableByName`, M2a).
pub(crate) fn immediate_prs_stages(modules: &[Option<&naga::Module>]) -> wgpu_types::ShaderStages {
    let mut stages = wgpu_types::ShaderStages::empty();
    for module in modules.iter().flatten() {
        for ep in &module.entry_points {
            stages |= match ep.stage {
                naga::ShaderStage::Vertex => wgpu_types::ShaderStages::VERTEX,
                naga::ShaderStage::Fragment => wgpu_types::ShaderStages::FRAGMENT,
                naga::ShaderStage::Compute => wgpu_types::ShaderStages::COMPUTE,
                _ => wgpu_types::ShaderStages::empty(),
            };
        }
    }
    stages
}

/// The shader's `Immediate` global variable name, when any of the modules
/// declares one (the immediate PRS resource name - the `GetVariableByName`
/// key for the `SetInlineConstants` path).
pub(crate) fn immediate_global_name(modules: &[Option<&naga::Module>]) -> Option<String> {
    modules.iter().flatten().find_map(|module| {
        module.global_variables.iter().find_map(|(_, global)| {
            (global.space == naga::AddressSpace::Immediate)
                .then(|| global.name.clone())
                .flatten()
        })
    })
}

/// Creates the dedicated immediate-constants PRS (brief §5.3.3-4: "专用
/// immediate PRS"; `immediate_size == 0` or no `Immediate` global in the
/// shaders -> `Ok(None)`).
pub(crate) fn create_immediate_prs(
    device: &diligent_rs::RenderDevice,
    modules: &[Option<&naga::Module>],
    immediate_size: u32,
) -> Result<Option<DiligentHandle<diligent_rs::PipelineResourceSignature>>, String> {
    if immediate_size == 0 {
        return Ok(None);
    }
    let Some(name) = immediate_global_name(modules) else {
        return Ok(None);
    };
    let name_c = CString::new(name.as_str()).map_err(|e| format!("PRS name: {e}"))?;
    let mut resource: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    resource.Name = name_c.as_ptr();
    // The stages of every entry point in the shaders (a PRS resource that a
    // stage does not use is tolerated - V15 sample A - so over-covering is
    // safe).
    resource.ShaderStages = modules.iter().flatten().fold(0u32, |bits, module| {
        module
            .entry_points
            .iter()
            .fold(bits, |bits, ep| bits | diligent_mapping::shader_type_from_naga(ep.stage))
    });
    // ArraySize is in 32-bit constants (DILIGENT_MAX_INLINE_CONSTANTS = 64
    // DWORDs, Constants.h:66); wgpu immediate_size is in bytes. Oversized
    // immediates are an error (no silent clamp - the caller warns and drops
    // the immediate PRS).
    let size_dwords = immediate_size.div_ceil(4);
    if size_dwords > 64 {
        return Err(format!(
            "immediate constants size {immediate_size} bytes exceeds the \
             Diligent maximum of 64 DWORDs ({} dwords requested)",
            size_dwords
        ));
    }
    resource.ArraySize = size_dwords;
    resource.ResourceType =
        sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER
            as sys::SHADER_RESOURCE_TYPE;
    resource.VarType =
        sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
    resource.Flags = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS
        as sys::PIPELINE_RESOURCE_FLAGS;
    device
        .create_pipeline_resource_signature(
            &format!("prs_immediate_{name}"),
            &[resource],
        )
        .map(|p| Some(DiligentHandle::new(Arc::new(p))))
        .map_err(|e| format!("immediate PRS creation failed: {e}"))
}

/// Creates a graphics PSO from a wgpu render pipeline descriptor.
///
/// `layout` carries the PRS array + immediate size; `vertex`/`fragment` are
/// the shader records. Failures are logged and return `None` (the transition
/// wgpu pipeline is unaffected).
///
/// M2a-2 (the bevy_pbr capability completion): the M1-2 scope-down (exactly
/// one color target, wrapper-fixed state) is replaced by the full state
/// translation through the wrapper's
/// `create_graphics_pipeline_multi_rt`:
///
/// * up to `DILIGENT_MAX_RENDER_TARGETS` (8) render targets - every slot
///   gets its write mask from the wgpu `ColorWrites` (the M1-review "only
///   RT0 has a write mask" gap: a zeroed mask on any slot makes D3D12
///   discard all PS output for that target) - plus per-target blend state;
/// * depth-only pipelines (`fragment.targets == []` with a depth-stencil
///   target - the CSM shadow passes) with `NumRenderTargets = 0`;
/// * rasterizer (topology / fill / cull / front-face / depth bias),
///   depth-stencil (compare / write / stencil faces) and MSAA
///   (sample count + sample mask) state from the descriptor.
///
/// Remaining gaps (honest): fullscreen pipelines without vertex buffers
/// still get no Diligent PSO (the wrapper requires >= 1 layout element),
/// `PolygonMode::Point` has no Diligent counterpart, conservative
/// rasterization and `alpha_to_coverage` are not represented (bevy never
/// enables either), and the wgpu sample mask is truncated to 32 bits.
pub(crate) fn create_graphics_pipeline(
    device: &RenderDevice,
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
    vertex: &Arc<ShaderModuleRecord>,
    fragment: Option<&Arc<ShaderModuleRecord>>,
    layout: Option<&PipelineLayoutRecord>,
) -> Option<DiligentHandle<diligent_rs::PipelineState>> {
    create_graphics_pipeline_inner(device, desc, vertex, fragment, layout, false)
}

/// Async variant of [`create_graphics_pipeline`]: creates the Diligent PSO
/// with `PSO_CREATE_FLAG_ASYNCHRONOUS` (V20: dGPU async cold-start is
/// 1.7-3.1x faster wall-clock than sync). The returned PSO must be polled
/// via [`PipelineState::status`](diligent_rs::PipelineState::status) until
/// `READY`/`FAILED` before use (a pipeline whose PSO is still compiling
/// degrades the pass exactly like a missing PSO - the poison mechanism).
/// Engines without async support ignore the flag and return a ready PSO.
pub(crate) fn create_graphics_pipeline_async(
    device: &RenderDevice,
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
    vertex: &Arc<ShaderModuleRecord>,
    fragment: Option<&Arc<ShaderModuleRecord>>,
    layout: Option<&PipelineLayoutRecord>,
) -> Option<DiligentHandle<diligent_rs::PipelineState>> {
    create_graphics_pipeline_inner(device, desc, vertex, fragment, layout, true)
}

#[allow(clippy::too_many_arguments)]
fn create_graphics_pipeline_inner(
    device: &RenderDevice,
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
    vertex: &Arc<ShaderModuleRecord>,
    fragment: Option<&Arc<ShaderModuleRecord>>,
    layout: Option<&PipelineLayoutRecord>,
    async_compile: bool,
) -> Option<DiligentHandle<diligent_rs::PipelineState>> {
    let Some(diligent) = device.diligent_device() else {
        return None;
    };
    let backend = device.diligent_backend();

    let (rtv_formats, blend_targets, ps) = match desc.fragment.as_ref() {
        Some(fragment_state) => {
            if fragment_state.targets.len() > sys::DILIGENT_MAX_RENDER_TARGETS as usize {
                bevy_log::warn!(
                    "diligent: render pipeline with {} color targets exceeds the Diligent maximum of {}",
                    fragment_state.targets.len(),
                    sys::DILIGENT_MAX_RENDER_TARGETS
                );
                return None;
            }
            // M2a-2: one RTV format + blend desc per slot; `None` entries (unused
            // slots) get TEX_FORMAT_UNKNOWN + a disabled blend desc.
            let mut rtv_formats: Vec<sys::TEXTURE_FORMAT> =
                Vec::with_capacity(fragment_state.targets.len());
            let mut blend_targets: Vec<sys::RenderTargetBlendDesc> =
                Vec::with_capacity(fragment_state.targets.len());
            for target in fragment_state.targets.iter() {
                let Some(target) = target else {
                    rtv_formats.push(sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT);
                    blend_targets.push(no_write_blend_target());
                    continue;
                };
                let format = match diligent_rs::format::to_diligent(target.format) {
                    Ok(format) => format,
                    Err(err) => {
                        bevy_log::warn!("diligent: RTV format: {err}");
                        return None;
                    }
                };
                rtv_formats.push(format);
                blend_targets.push(translate_blend_target(target));
            }
            let ps_handle = match fragment {
                Some(record) => record.diligent_shader(
                    diligent,
                    backend,
                    naga::ShaderStage::Fragment,
                    entry_point(fragment_state.entry_point),
                ),
                None => Err("no fragment record".to_string()),
            }
            .map_err(|e| bevy_log::warn!("diligent: fragment shader: {e}"))
            .ok()?;
            (rtv_formats, blend_targets, Some(ps_handle))
        }
        None => (Vec::new(), Vec::new(), None),
    };
    let dsv_format = match &desc.depth_stencil {
        Some(ds) => match diligent_rs::format::to_diligent(ds.format) {
            Ok(format) => format,
            Err(err) => {
                bevy_log::warn!("diligent: DSV format: {err}");
                return None;
            }
        },
        None => sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT,
    };
    if rtv_formats.is_empty() && dsv_format == sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT {
        bevy_log::debug!("diligent: render pipeline without any color or depth target");
        return None;
    }

    let Some(vs) = vertex
        .diligent_shader(
            diligent,
            backend,
            naga::ShaderStage::Vertex,
            entry_point(desc.vertex.entry_point),
        )
        .map_err(|e| bevy_log::warn!("diligent: vertex shader: {e}"))
        .ok()
    else {
        return None;
    };

    let Some((layout_elements, semantic_names)) = build_vertex_layout(desc) else {
        return None;
    };
    let _ = semantic_names;

    let Some(record) = layout else {
        bevy_log::debug!("diligent: render pipeline without resource signatures is not supported");
        return None;
    };
    let prs_refs = record.pso_prs();
    if prs_refs.is_empty() {
        bevy_log::debug!("diligent: render pipeline without resource signatures is not supported");
        return None;
    }

    let rasterizer = translate_rasterizer(desc)?;
    let depth_stencil = translate_depth_stencil(desc, dsv_format);
    let name = desc
        .label
        .map(|l| format!("bevy_{l}"))
        .unwrap_or_else(|| "bevy_render_pipeline".to_string());
    // M2a-2: the wgpu sample mask is a u64; D3D12 consumes the low 32 bits
    // (wgpu-hal dx12 does the same truncation).
    let sample_mask = desc.multisample.mask as u32;
    // M3b §8.10: feed the in-memory pipeline-state cache into `pPSOCache`.
    // A same-named PSO reuses the driver blob; `None` (no PSO-cache support)
    // degrades to plain creation.
    let pso_cache = device.pso_cache();
    let ps_ref = ps.as_deref();
    let create_result = if async_compile {
        diligent.create_graphics_pipeline_multi_rt_async_cached(
            &name,
            &vs,
            ps_ref,
            &rtv_formats,
            &blend_targets,
            &layout_elements,
            &prs_refs,
            dsv_format,
            &rasterizer,
            &depth_stencil,
            diligent_mapping::primitive_topology(desc.primitive.topology),
            desc.multisample.count,
            sample_mask,
            pso_cache,
        )
    } else {
        diligent.create_graphics_pipeline_multi_rt_cached(
            &name,
            &vs,
            ps_ref,
            &rtv_formats,
            &blend_targets,
            &layout_elements,
            &prs_refs,
            dsv_format,
            &rasterizer,
            &depth_stencil,
            diligent_mapping::primitive_topology(desc.primitive.topology),
            desc.multisample.count,
            sample_mask,
            pso_cache,
        )
    };
    match create_result {
        Ok(pso) => Some(DiligentHandle::new(Arc::new(pso))),
        Err(err) => {
            bevy_log::warn!("diligent: graphics pipeline creation failed: {err}");
            None
        }
    }
}

/// A render-target blend desc that writes nothing (used for unused slots).
fn no_write_blend_target() -> sys::RenderTargetBlendDesc {
    let mut rt: sys::RenderTargetBlendDesc = unsafe { std::mem::zeroed() };
    rt.RenderTargetWriteMask = sys::_COLOR_MASK::COLOR_MASK_NONE as sys::COLOR_MASK;
    rt
}

/// The per-target blend + write mask for a wgpu color target.
///
/// The write mask is translated from the wgpu `ColorWrites` (bit-compatible
/// flag sets - the M1-review gap: the M1-1 wrapper only ever set
/// `COLOR_MASK_ALL` on slot 0, which on D3D12 silently discards the PS
/// output of every other render target).
fn translate_blend_target(target: &wgpu_types::ColorTargetState) -> sys::RenderTargetBlendDesc {
    let mut rt: sys::RenderTargetBlendDesc = unsafe { std::mem::zeroed() };
    rt.RenderTargetWriteMask = diligent_mapping::color_writes(target.write_mask);
    if let Some(blend) = target.blend {
        rt.BlendEnable = true;
        rt.SrcBlend = diligent_mapping::blend_factor(blend.color.src_factor);
        rt.DestBlend = diligent_mapping::blend_factor(blend.color.dst_factor);
        rt.BlendOp = diligent_mapping::blend_operation(blend.color.operation);
        rt.SrcBlendAlpha = diligent_mapping::blend_factor(blend.alpha.src_factor);
        rt.DestBlendAlpha = diligent_mapping::blend_factor(blend.alpha.dst_factor);
        rt.BlendOpAlpha = diligent_mapping::blend_operation(blend.alpha.operation);
    }
    rt
}

/// The rasterizer state for a wgpu render pipeline (`RasterizerState.h`).
///
/// Returns `None` for `PolygonMode::Point` (no Diligent counterpart in the
/// locked enums - `GraphicsTypes.h` has only SOLID/WIREFRAME).
fn translate_rasterizer(
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
) -> Option<sys::RasterizerStateDesc> {
    let mut ra: sys::RasterizerStateDesc = unsafe { std::mem::zeroed() };
    ra.FillMode = diligent_mapping::fill_mode(desc.primitive.polygon_mode)?;
    ra.CullMode = diligent_mapping::cull_mode(desc.primitive.cull_mode);
    // wgpu: CCW = front face (wgpu-types FrontFace::Ccw default); Diligent
    // mirrors the D3D12 convention (FrontCounterClockwise = True => CCW).
    ra.FrontCounterClockwise = match desc.primitive.front_face {
        wgpu_types::FrontFace::Ccw => true,
        wgpu_types::FrontFace::Cw => false,
    };
    ra.DepthClipEnable = !desc.primitive.unclipped_depth;
    if desc.primitive.conservative {
        bevy_log::debug!(
            "diligent: conservative rasterization is not representable in this engine version"
        );
    }
    if let Some(ds) = &desc.depth_stencil {
        ra.DepthBias = ds.bias.constant;
        ra.DepthBiasClamp = ds.bias.clamp;
        ra.SlopeScaledDepthBias = ds.bias.slope_scale;
    }
    Some(ra)
}

/// The depth-stencil state for a wgpu render pipeline
/// (`DepthStencilState.h`; `dsv_format == TEX_FORMAT_UNKNOWN` disables the
/// depth test).
fn translate_depth_stencil(
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
    dsv_format: sys::TEXTURE_FORMAT,
) -> sys::DepthStencilStateDesc {
    let mut ds: sys::DepthStencilStateDesc = unsafe { std::mem::zeroed() };
    let Some(state) = &desc.depth_stencil else {
        return ds;
    };
    if dsv_format == sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT {
        return ds;
    }
    ds.DepthEnable = true;
    ds.DepthWriteEnable = state.depth_write_enabled.unwrap_or(false);
    ds.DepthFunc = diligent_mapping::comparison_function(
        state.depth_compare.unwrap_or(wgpu_types::CompareFunction::Always),
    );
    if state.stencil.is_enabled() {
        ds.StencilEnable = true;
        ds.StencilReadMask = state.stencil.read_mask as u8;
        ds.StencilWriteMask = state.stencil.write_mask as u8;
        ds.FrontFace = translate_stencil_face(&state.stencil.front);
        ds.BackFace = translate_stencil_face(&state.stencil.back);
    }
    ds
}

/// One stencil face state (ops + compare; `DepthStencilState.h` StencilOpDesc).
///
/// wgpu-types 29's `StencilFaceState` carries plain (non-Option) values with
/// the `IGNORE` sentinel for the operations (`compare: Always` disables the
/// test side), so no unwrapping is needed.
fn translate_stencil_face(face: &wgpu_types::StencilFaceState) -> sys::StencilOpDesc {
    let mut op: sys::StencilOpDesc = unsafe { std::mem::zeroed() };
    op.StencilFailOp = diligent_mapping::stencil_operation(face.fail_op);
    op.StencilDepthFailOp = diligent_mapping::stencil_operation(face.depth_fail_op);
    op.StencilPassOp = diligent_mapping::stencil_operation(face.pass_op);
    op.StencilFunc = diligent_mapping::comparison_function(face.compare);
    op
}

/// Creates a compute PSO from a wgpu compute pipeline descriptor.
///
/// M1-4b-1: the wrapper's `create_compute_pipeline` entry point
/// (`IRenderDevice::CreateComputePipelineState` + PRS array + compute
/// shader) replaces the M1-1 None fallback; the PSO carries the layout
/// record's PRS array (empty when no layout record is registered - the
/// wrapper then emulates the implicit signature with an explicit empty
/// one).
pub(crate) fn create_compute_pipeline(
    device: &RenderDevice,
    desc: &crate::render_resource::RawComputePipelineDescriptor,
    module: &Arc<ShaderModuleRecord>,
    layout: Option<&PipelineLayoutRecord>,
) -> Option<DiligentHandle<diligent_rs::PipelineState>> {
    let Some(diligent) = device.diligent_device() else {
        return None;
    };
    let backend = device.diligent_backend();
    let entry = entry_point(desc.entry_point);
    let shader = match module.diligent_shader(
        diligent,
        backend,
        naga::ShaderStage::Compute,
        entry,
    ) {
        Ok(shader) => shader,
        Err(err) => {
            bevy_log::warn!("diligent: compute shader: {err}");
            return None;
        }
    };
    let prs_refs = layout.map(|record| record.pso_prs()).unwrap_or_default();
    let name = desc
        .label
        .map(|l| format!("bevy_{l}"))
        .unwrap_or_else(|| "bevy_compute_pipeline".to_string());
    match diligent.create_compute_pipeline(&name, &shader, &prs_refs) {
        Ok(pso) => Some(DiligentHandle::new(Arc::new(pso))),
        Err(err) => {
            bevy_log::warn!("diligent: compute pipeline creation failed: {err}");
            None
        }
    }
}

/// Builds the Diligent vertex input layout for a wgpu render pipeline.
///
/// One element per attribute, semantic `LOC{location}` (naga hlsl emits
/// `LOC{n}` for location-n varyings - writer.rs LOCATION_SEMANTIC),
/// `InputIndex` = the shader input slot (attributes sorted by shader
/// location, mirroring naga's input ordering). The returned `CString`s keep
/// the semantic names alive for the wrapper call. Returns `None` for vertex
/// formats without a Diligent counterpart or when the descriptor has no
/// vertex buffers (the wrapper requires >= 1 element).
fn build_vertex_layout(
    desc: &crate::render_resource::RawRenderPipelineDescriptor,
) -> Option<(Vec<sys::LayoutElement>, Vec<CString>)> {
    let mut layout_elements: Vec<sys::LayoutElement> = Vec::new();
    let mut semantic_names: Vec<CString> = Vec::new();
    let mut attributes: Vec<(
        u32,
        u32,
        u32,
        u32,
        sys::VALUE_TYPE,
        bool,
        sys::INPUT_ELEMENT_FREQUENCY,
    )> = Vec::new();
    for (buffer_index, buffer) in desc.vertex.buffers.iter().enumerate() {
        let frequency = match buffer.step_mode {
            wgpu_types::VertexStepMode::Vertex => {
                sys::_INPUT_ELEMENT_FREQUENCY::INPUT_ELEMENT_FREQUENCY_PER_VERTEX
            }
            wgpu_types::VertexStepMode::Instance => {
                sys::_INPUT_ELEMENT_FREQUENCY::INPUT_ELEMENT_FREQUENCY_PER_INSTANCE
            }
        } as sys::INPUT_ELEMENT_FREQUENCY;
        for attribute in buffer.attributes.iter() {
            let Some((value_type, components, normalized)) =
                diligent_mapping::vertex_format_to_value_type(attribute.format)
            else {
                bevy_log::warn!(
                    "diligent: vertex format {:?} has no Diligent counterpart",
                    attribute.format
                );
                return None;
            };
            attributes.push((
                attribute.shader_location,
                buffer_index as u32,
                attribute.offset as u32,
                components,
                value_type,
                normalized,
                frequency,
            ));
        }
    }
    attributes.sort_by_key(|(location, ..)| *location);
    for (shader_index, (location, buffer_index, offset, components, value_type, normalized, frequency)) in
        attributes.into_iter().enumerate()
    {
        let semantic = CString::new(format!("LOC{location}")).ok()?;
        let element = sys::LayoutElement {
            HLSLSemantic: semantic.as_ptr(),
            InputIndex: shader_index as u32,
            BufferSlot: buffer_index,
            NumComponents: components,
            ValueType: value_type,
            IsNormalized: normalized,
            RelativeOffset: offset,
            Stride: sys::LAYOUT_ELEMENT_AUTO_STRIDE,
            Frequency: frequency,
            InstanceDataStepRate: 0,
        };
        semantic_names.push(semantic);
        layout_elements.push(element);
    }
    if layout_elements.is_empty() {
        bevy_log::debug!(
            "diligent: render pipeline without vertex buffers is not supported by the M1-1 wrapper"
        );
        return None;
    }
    Some((layout_elements, semantic_names))
}

/// The wgpu entry-point default ("main", matching wgpu's own default when
/// `None`).
pub(crate) fn entry_point(entry_point: Option<&str>) -> &str {
    entry_point.unwrap_or("main")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical PRS names are deterministic per binding - the SRB-side
    /// signature (`create_canonical_prs`) and the `GetVariableByName` lookup
    /// in `bind_srb_variable` must agree on every binding.
    #[test]
    fn canonical_prs_name_is_binding_derived_and_unique() {
        assert_eq!(canonical_prs_name(0), "binding_0");
        assert_eq!(canonical_prs_name(7), "binding_7");
        assert_ne!(canonical_prs_name(0), canonical_prs_name(1));
    }

    /// The documented `IsCompatibleWith` semantics ("same order, names
    /// disregarded"): the PSO-side signature may carry shader-derived names
    /// while the SRB-side signature uses the canonical names - the fallback
    /// must produce the canonical name whenever the shader does not declare
    /// a variable for a (group, binding).
    #[test]
    fn shader_name_for_prefers_shader_names_with_canonical_fallback() {
        let shader_names = std::collections::HashMap::from([
            ((0u32, 2u32), "g_Textures".to_string()),
            ((0u32, 3u32), "g_Camera".to_string()),
        ]);
        let maps = [&shader_names];
        // The shader-derived name wins when the shader declares it...
        assert_eq!(shader_name_for(&maps, 0, 2), "g_Textures");
        assert_eq!(shader_name_for(&maps, 0, 3), "g_Camera");
        // ...and the canonical binding_N name is the deterministic fallback
        // for bindings the shader does not use (V15 sample A: the PRS stays
        // compatible by order, names disregarded).
        assert_eq!(shader_name_for(&maps, 0, 5), canonical_prs_name(5));
        assert_eq!(shader_name_for(&maps, 1, 0), canonical_prs_name(0));
    }

    /// The BGL -> PRS generator lands the §6.1.1 binding model: VarType
    /// layering (DYNAMIC for the has_dynamic_offset buffer bindings, MUTABLE
    /// otherwise), the NO_DYNAMIC_BUFFERS flag on non-dynamic buffer
    /// variables (V17 rung 2) and the verbatim ArraySize pass-through (V15)
    /// - including the six bounded-array equivalents (mesh_view_bindings
    /// 4x8u texture arrays + lightmap 2x4u texture/sampler arrays).
    #[test]
    fn pipeline_resource_desc_applies_the_binding_model() {
        use wgpu_types::{
            BindingType, BufferBindingType, SamplerBindingType, TextureSampleType,
            TextureViewDimension,
        };
        let name = |binding: u32| CString::new(canonical_prs_name(binding)).unwrap();
        let entry = |binding, ty, count: Option<u32>| wgpu_types::BindGroupLayoutEntry {
            binding,
            visibility: wgpu_types::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: count.map(std::num::NonZeroU32::new).flatten(),
        };
        let dynamic_uniform =
            |has| BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: has,
                min_binding_size: None,
            };
        let texture = |dimension| BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        };
        let dynamic = sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
        let mutable = sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
        let no_dynamic = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS
            as sys::PIPELINE_RESOURCE_FLAGS;
        let none = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE
            as sys::PIPELINE_RESOURCE_FLAGS;
        let cb = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER
            as sys::SHADER_RESOURCE_TYPE;
        let tsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV
            as sys::SHADER_RESOURCE_TYPE;
        let sam = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_SAMPLER
            as sys::SHADER_RESOURCE_TYPE;

        // View-uniform slots (binding 0/1/12/13 with has_dynamic_offset):
        // DYNAMIC var type, NO_DYNAMIC flag absent, ArraySize 1.
        for binding in [0u32, 1, 12, 13] {
            let desc = pipeline_resource_desc(
                0,
                &entry(binding, dynamic_uniform(true), None),
                &name(binding),
            )
            .unwrap();
            assert_eq!(desc.VarType, dynamic, "binding {binding} must be DYNAMIC");
            assert_eq!(desc.Flags, none, "binding {binding} must not carry the flag");
            assert_eq!(desc.ArraySize, 1, "binding {binding}");
        }

        // Non-dynamic uniform (e.g. Globals at 11): MUTABLE + NO_DYNAMIC_BUFFERS.
        let desc = pipeline_resource_desc(0, &entry(11, dynamic_uniform(false), None), &name(11)).unwrap();
        assert_eq!(desc.VarType, mutable);
        assert_eq!(desc.Flags, no_dynamic);
        assert_eq!(desc.ResourceType, cb);

        // Bounded texture arrays (mesh_view_bindings 4x8u): MUTABLE, no
        // flag, ArraySize passed through verbatim (V15).
        for (binding, dimension) in [
            (0u32, TextureViewDimension::Cube),
            (1, TextureViewDimension::Cube),
            (3, TextureViewDimension::D3),
            (6, TextureViewDimension::D2),
        ] {
            let desc = pipeline_resource_desc(
                1,
                &entry(binding, texture(dimension), Some(8)),
                &name(binding),
            )
            .unwrap();
            assert_eq!(desc.ArraySize, 8, "binding {binding} (mesh_view_bindings)");
            assert_eq!(desc.VarType, mutable);
            assert_eq!(desc.Flags, none);
            assert_eq!(desc.ResourceType, tsrv);
        }

        // Lightmap arrays (lightmap.wgsl:6-7, 2x4u): texture + sampler.
        let tex = pipeline_resource_desc(
            2,
            &entry(4, texture(TextureViewDimension::D2), Some(4)),
            &name(4),
        )
        .unwrap();
        assert_eq!(tex.ArraySize, 4);
        assert_eq!(tex.ResourceType, tsrv);
        assert_eq!(tex.VarType, mutable);
        assert_eq!(tex.Flags, none);
        let sampler = pipeline_resource_desc(
            2,
            &entry(
                5,
                BindingType::Sampler(SamplerBindingType::Filtering),
                Some(4),
            ),
            &name(5),
        )
        .unwrap();
        assert_eq!(sampler.ArraySize, 4);
        assert_eq!(sampler.ResourceType, sam);
        assert_eq!(sampler.VarType, mutable);
        assert_eq!(sampler.Flags, none);

        // Storage buffers: MUTABLE + NO_DYNAMIC_BUFFERS (SRV/UAV).
        let bsrv = sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_SRV
            as sys::SHADER_RESOURCE_TYPE;
        let desc = pipeline_resource_desc(
            0,
            &entry(
                14,
                BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                None,
            ),
            &name(14),
        )
        .unwrap();
        assert_eq!(desc.ResourceType, bsrv);
        assert_eq!(desc.VarType, mutable);
        assert_eq!(desc.Flags, no_dynamic);
    }

    /// M3b/M5a-adjacent unit-test baseline (task 19.2): the WGSL -> naga ->
    /// HLSL translation that feeds Diligent `create_shader` on D3D12 must
    /// produce valid HLSL with the expected entry point and resource
    /// bindings - without touching a GPU. This is the golden-shader-set
    /// entry for the fragment stage of a minimal full-screen pass.
    fn translate_wgsl_to_hlsl(wgsl: &str, entry_point: &str) -> String {
        let module = naga::front::wgsl::parse_str(wgsl).expect("wgsl parse");
        let module_info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("naga validation");
        let mut hlsl_options = naga::back::hlsl::Options::default();
        hlsl_options.shader_model = naga::back::hlsl::ShaderModel::V6_0;
        let pipeline_options = naga::back::hlsl::PipelineOptions {
            entry_point: Some((naga::ShaderStage::Fragment, entry_point.to_string())),
        };
        let mut output = String::new();
        let mut writer = naga::back::hlsl::Writer::new(&mut output, &hlsl_options, &pipeline_options);
        writer
            .write(&module, &module_info, None)
            .expect("hlsl generation");
        output
    }

    #[test]
    fn wgsl_fragment_translates_to_hlsl_with_expected_entry() {
        let hlsl = translate_wgsl_to_hlsl(
            r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(uv, 0.0, 1.0);
}
"#,
            "main",
        );
        assert!(
            hlsl.contains("main"),
            "HLSL must expose the entry point (got {})",
            hlsl
        );
        // naga 29's HLSL fragment signature names the stage I/O
        // `PS_OUTPUT`/`SV_TARGET`; tolerate both spellings (the generator
        // version pins the exact casing).
        assert!(
            hlsl.contains("PS_OUTPUT")
                || hlsl.to_ascii_uppercase().contains("SV_TARGET"),
            "HLSL fragment must emit a pixel-shader output signature (got {})",
            hlsl
        );
        assert!(
            hlsl.contains("float4") && hlsl.contains("0.0"),
            "HLSL must keep the float4 composition"
        );
    }

    /// The same WGSL also translates to SPIR-V (the Vulkan backend path) -
    /// both outputs are produced from one naga module.
    #[test]
    fn wgsl_fragment_translates_to_spirv() {
        let module = naga::front::wgsl::parse_str(
            r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(uv, 0.0, 1.0);
}
"#,
        )
        .expect("wgsl parse");
        let module_info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("naga validation");
        let words = naga::back::spv::write_vec(
            &module,
            &module_info,
            &naga::back::spv::Options::default(),
            Some(&naga::back::spv::PipelineOptions {
                shader_stage: naga::ShaderStage::Fragment,
                entry_point: "main".to_string(),
            }),
        )
        .expect("spir-v generation");
        assert!(
            words.len() > 8,
            "SPIR-V must be a non-trivial module ({} words)",
            words.len()
        );
        // The magic header identifies SPIR-V (0x07230203).
        assert_eq!(words[0], 0x0723_0203, "SPIR-V magic number");
    }
}
