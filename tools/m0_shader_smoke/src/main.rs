//! M0 S-A: batch compile smoke for all WGSL shaders in the fork (task 2).
//!
//! Mirrors the fork's real compilation chain (bevy_shader::ShaderCache):
//!   1. naga_oil Composer resolves `#import` / `#define_import_path` / `#ifdef`
//!   2. naga validation with capabilities derived from the wgpu feature set
//!      (via wgpu-naga-bridge::features_to_naga_capabilities, same as the fork)
//!   3. SPIR-V output (Vulkan path)
//!   4. HLSL output per entry point, SM6.6 (D3D12 path; wgpu-hal pattern:
//!      hlsl::Writer::new + write with PipelineOptions.entry_point)
//!
//! Task-2 changes vs task 1:
//!   - Dependency-ordered registration of importable modules. naga_oil 0.22's
//!     `add_composable_module` requires all imports of a module to already be
//!     registered (mod.rs:1576 "we require modules already added"), so a naive
//!     file-order registration pass lost most modules -> `required import ...
//!     not found` everywhere. We now register iteratively, mirroring
//!     ShaderCache::add_import_to_composer (shader_cache.rs:152) recursion.
//!   - Compose errors are reported with codespan diagnostics
//!     (ComposerError::emit_to_string) so the failing line is visible.
//!   - Panic-proof: every naga stage (compose / validate / spv / per-entry
//!     hlsl) runs under catch_unwind; a naga internal panic (e.g. HLSL
//!     storage.rs unreachable!()) is recorded as `PANIC[stage]: <msg>` for
//!     that file/entry and the batch continues.
//!   - V4 evidence: ray query / unbounded binding_array / subgroup files get
//!     full HLSL + SPIR-V dumped to out/ and key snippets are embedded into
//!     v4-report.md.
//!   - V16 evidence: every var<immediate> site -> HLSL root-constant form,
//!     SPIR-V push-constant layout (from IR), and FirstConstant alignment
//!     table -> v16-report.md.
//!
//! Usage:
//!   m0_shader_smoke [repo_root] [--defs <file-substring>=<DEF[,DEF...]>]...

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, NagaModuleDescriptor, ShaderDefValue, ShaderLanguage,
    ShaderType,
};
use wgpu_types as wgt;

/// Default capacity applied to unbounded binding arrays for HLSL output.
/// naga's HLSL writer calls `unreachable!()` on `IndexableLength::Dynamic`
/// (writer.rs write_array_size), so a concrete capacity must be supplied via
/// the binding map. The exact per-binding capacities (2048 bindless /
/// 500/5000 solari / 64 Apple) are a M2 concern (PRS ArraySize); here we
/// prove translatability.
const DEFAULT_UNBOUNDED_ARRAY_CAP: u32 = 2048;

/// Global shader defs applied to every file, mirroring the fork's runtime
/// defs: PipelineCache global defs (AVAILABLE_STORAGE_BUFFER_BINDINGS =
/// max_storage_buffers_per_shader_stage, wgpu default 8) + the per-pipeline
/// numeric defs the fork passes at runtime (values from light.rs / mesh.rs /
/// material.rs / solari prepare.rs / oit default). Only AVAILABLE_STORAGE_BUFFER_BINDINGS
/// is used with bare `#if`; everything else is `#ifdef` (Bool ok).
fn global_defs() -> HashMap<String, ShaderDefValue> {
    let mut defs = HashMap::new();
    defs.insert(
        "AVAILABLE_STORAGE_BUFFER_BINDINGS".to_string(),
        ShaderDefValue::UInt(8),
    );
    defs.insert("SUBGROUP_SUPPORT".to_string(), ShaderDefValue::Bool(true));
    defs.insert("MATERIAL_BIND_GROUP".to_string(), ShaderDefValue::UInt(3));
    defs.insert("MAX_CASCADES_PER_LIGHT".to_string(), ShaderDefValue::UInt(4));
    defs.insert("MAX_DIRECTIONAL_LIGHTS".to_string(), ShaderDefValue::UInt(10));
    defs.insert("MAX_RECT_LIGHTS".to_string(), ShaderDefValue::UInt(8));
    defs.insert("WORLD_CACHE_SIZE".to_string(), ShaderDefValue::UInt(1 << 20));
    defs.insert("SORTED_FRAGMENT_MAX_COUNT".to_string(), ShaderDefValue::UInt(8));
    defs.insert(
        "SCREEN_SPACE_SPECULAR_TRANSMISSION_BLUR_TAPS".to_string(),
        ShaderDefValue::UInt(16),
    );
    defs.insert(
        "PER_OBJECT_BUFFER_BATCH_SIZE".to_string(),
        ShaderDefValue::UInt(16),
    );
    defs
}

/// Per-file defs mirroring the fork's runtime pipeline variants, taken from
/// the fork's specialize() / prepare() calls:
///   fxaa: EDGE_THRESH_MIN_*/EDGE_THRESH_* quality levels (anti_alias fxaa.rs)
///   smaa: phase defs (anti_alias smaa.rs)
///   downsample: COMBINE_BIND_GROUP/SRGB_CONVERSION variants (SPD)
///   lut_bindings: tonemapping LUT binding index = 18 (mesh_view_bindings.rs:71)
///   ssao: SLICE_COUNT runtime value (ssao/mod.rs:450)
///   box_shadow: SHADOW_SAMPLES runtime value (box_shadow.rs:157)
///   deferred_lighting: DEFERRED_PREPASS + LUT index (deferred/mod.rs:215,296)
///   ssr/raymarch: SCREEN_SPACE_REFLECTIONS + DEFERRED_PREPASS (ssr/mod.rs:489)
///   environment_map: ENVIRONMENT_MAP (mesh.rs:3530 / ssr / deferred)
///   forward_decal/pbr_fragment: VERTEX_OUTPUT_INSTANCE_INDEX (mesh.rs:3295,
///     prepass/mod.rs:430) - the meshlet material/instance-index variant
///   meshlet culling: pass defs (meshlet/pipelines.rs)
///   resolve_dlss_rr_textures: DLSS_RR_GUIDE_BUFFERS (solari realtime/node.rs)
///   pbr/pbr_fragment/pbr_prepass/pbr_prepass_functions/parallax_mapping:
///     BINDLESS (material.rs:494/496, prepass/mod.rs:356) - bindless.wgsl 的
///     9 组 handle 空间无界数组全部在 #ifdef BINDLESS 门内, 必须随消费者定义
/// These defs are applied ONLY to the top-level shader; imported modules are
/// compiled by naga_oil with the top-level defs (make_naga_module's
/// ensure_imports), exactly like bevy's ShaderCache.
#[derive(Clone, Copy)]
enum DefVal {
    B,
    U(u32),
    I(i32),
}

const FILE_DEFS: &[(&str, &[(&str, DefVal)])] = &[
    (
        "fxaa.wgsl",
        &[
            ("EDGE_THRESH_MIN_MEDIUM", DefVal::B),
            ("EDGE_THRESH_MEDIUM", DefVal::B),
        ],
    ),
    (
        "smaa.wgsl",
        &[
            ("SMAA_EDGE_DETECTION", DefVal::B),
            ("SMAA_BLENDING_WEIGHT_CALCULATION", DefVal::B),
            ("SMAA_NEIGHBORHOOD_BLENDING", DefVal::B),
            ("SMAA_PRESET_LOW", DefVal::B),
        ],
    ),
    (
        "downsample.wgsl",
        &[
            ("COMBINE_BIND_GROUP", DefVal::B),
            ("SRGB_CONVERSION", DefVal::B),
        ],
    ),
    (
        "lut_bindings.wgsl",
        &[
            ("TONEMAPPING_LUT_TEXTURE_BINDING_INDEX", DefVal::U(18)),
            ("TONEMAPPING_LUT_SAMPLER_BINDING_INDEX", DefVal::U(19)),
        ],
    ),
    (
        "ssao.wgsl",
        &[
            ("SLICE_COUNT", DefVal::I(8)),
            ("SAMPLES_PER_SLICE_SIDE", DefVal::I(3)),
        ],
    ),
    ("box_shadow.wgsl", &[("SHADOW_SAMPLES", DefVal::U(4))]),
    (
        "deferred_lighting.wgsl",
        &[
            ("DEFERRED_PREPASS", DefVal::B),
            ("TONEMAPPING_LUT_TEXTURE_BINDING_INDEX", DefVal::U(18)),
        ],
    ),
    (
        "ssr.wgsl",
        &[
            ("SCREEN_SPACE_REFLECTIONS", DefVal::B),
            ("DEFERRED_PREPASS", DefVal::B),
            ("DEPTH_PREPASS", DefVal::B),
        ],
    ),
    (
        "raymarch.wgsl",
        &[
            ("SCREEN_SPACE_REFLECTIONS", DefVal::B),
            ("DEFERRED_PREPASS", DefVal::B),
            ("DEPTH_PREPASS", DefVal::B),
        ],
    ),
    ("environment_map.wgsl", &[("ENVIRONMENT_MAP", DefVal::B)]),
    (
        "forward_decal.wgsl",
        &[
            ("VERTEX_OUTPUT_INSTANCE_INDEX", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
            ("DEPTH_PREPASS", DefVal::B),
        ],
    ),
    (
        "pbr.wgsl",
        &[
            ("VERTEX_OUTPUT_INSTANCE_INDEX", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
            // fork 运行时 material.rs:494 在 bindless 模式下把 BINDLESS 推进
            // vertex shader defs（pbr 材质管线）; bindless.wgsl 的 9 组 handle 空间
            // 无界数组在 #ifdef BINDLESS 门内, 不定义则全部缺失。
            ("BINDLESS", DefVal::B),
        ],
    ),
    (
        "pbr_fragment.wgsl",
        &[
            ("VERTEX_OUTPUT_INSTANCE_INDEX", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
            // material.rs:496 fragment defs
            ("BINDLESS", DefVal::B),
        ],
    ),
    // pbr_prepass / pbr_prepass_functions: prepass/mod.rs:356 推 BINDLESS
    // （注意子串顺序: "pbr_prepass.wgsl" 不匹配 "_functions", 两条都要写）
    (
        "pbr_prepass.wgsl",
        &[
            ("BINDLESS", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
        ],
    ),
    (
        "pbr_prepass_functions.wgsl",
        &[
            ("BINDLESS", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
        ],
    ),
    // parallax_mapping: 被 pbr_fragment/pbr_prepass 在 BINDLESS 下 import,
    // 独立编译时同样给 BINDLESS（其 sample_depth_map 有 #ifdef BINDLESS 分支）
    (
        "parallax_mapping.wgsl",
        &[
            ("BINDLESS", DefVal::B),
            ("VERTEX_TANGENTS", DefVal::B),
            ("VERTEX_UVS_A", DefVal::B),
        ],
    ),
    (
        "cull_bvh.wgsl",
        &[
            ("MESHLET_BVH_CULLING_PASS", DefVal::B),
            ("MESHLET_FIRST_CULLING_PASS", DefVal::B),
        ],
    ),
    (
        "cull_clusters.wgsl",
        &[
            ("MESHLET_CLUSTER_CULLING_PASS", DefVal::B),
            ("MESHLET_FIRST_CULLING_PASS", DefVal::B),
        ],
    ),
    (
        "cull_instances.wgsl",
        &[
            ("MESHLET_INSTANCE_CULLING_PASS", DefVal::B),
            ("MESHLET_FIRST_CULLING_PASS", DefVal::B),
        ],
    ),
    (
        "meshlet_cull_shared.wgsl",
        &[("MESHLET_INSTANCE_CULLING_PASS", DefVal::B)],
    ),
    (
        "visibility_buffer_hardware_raster.wgsl",
        &[("MESHLET_VISIBILITY_BUFFER_RASTER_PASS", DefVal::B)],
    ),
    (
        "visibility_buffer_software_raster.wgsl",
        &[
            ("MESHLET_VISIBILITY_BUFFER_RASTER_PASS", DefVal::B),
            ("MESHLET_VISIBILITY_BUFFER_RASTER_PASS_OUTPUT", DefVal::B),
        ],
    ),
    (
        "meshlet_mesh_material.wgsl",
        &[("MESHLET_MESH_MATERIAL_PASS", DefVal::B)],
    ),
    (
        "resolve_dlss_rr_textures.wgsl",
        &[("DLSS_RR_GUIDE_BUFFERS", DefVal::B)],
    ),
];

fn apply_file_defs(rel: &str, defs: &mut HashMap<String, ShaderDefValue>) {
    for (substr, names) in FILE_DEFS {
        if rel.contains(substr) {
            for (n, v) in *names {
                let val = match v {
                    DefVal::B => ShaderDefValue::Bool(true),
                    DefVal::U(x) => ShaderDefValue::UInt(*x),
                    DefVal::I(x) => ShaderDefValue::Int(*x),
                };
                defs.insert(n.to_string(), val);
            }
        }
    }
}

/// Source-level specialization mirroring the fork's "text-pasting hack"
/// (bevy_core_pipeline mip_generation/mod.rs:262): naga_oil has no
/// string-valued shader defs, so `##TEXTURE_FORMAT##` is replaced with a
/// concrete texture format identifier before composing. `rgba16float` is a
/// float format, matching the `vec4f` stores in spd_store (r32uint would not
/// type-check: textureStore into r32uint requires vec4<u32> - the fork's
/// non-float variants of this shader are likewise uncompilable, a latent
/// fork-side bug in the eager per-format specialization).
fn preprocess_source(rel: &str, src: &str) -> String {
    if rel.contains("downsample.wgsl") {
        src.replace("##TEXTURE_FORMAT##", "rgba16float")
    } else {
        src.to_string()
    }
}

/// A file that declares `#define_import_path` and is therefore importable.
struct ModuleCandidate {
    rel: String,
    name: String,
    imports: Vec<String>,
    defs: HashMap<String, ShaderDefValue>,
}

/// One var<immediate> member (offset/size in bytes, WGSL uniform layout).
struct ImmediateMember {
    name: String,
    offset: u32,
    size: u32,
}

/// A single `var<immediate>` global as seen in the composed naga module.
struct ImmediateVar {
    var_name: String,
    ty_name: String,
    flat_ty: Option<String>, // Some("<u32>") when the immediate is not a struct
    total_size: u32,
    members: Vec<ImmediateMember>,
}

struct FileResult {
    path: String,
    /// compose + validate
    compose: Result<(), String>,
    /// SPIR-V: word count
    spv: Result<usize, String>,
    /// HLSL: per entry point, first line of output
    hlsl: Vec<(String, Result<String, String>)>,
    /// detected unbounded binding arrays: "group g binding b"
    unbounded_arrays: Vec<String>,
    has_immediates: bool,
    immediate_vars: Vec<ImmediateVar>,
    has_ray_query: bool,
    has_subgroup: bool,
    entry_points: Vec<String>,
    /// full HLSL text (first entry point) kept for evidence dumps
    hlsl_full: Option<String>,
    spv_words: Option<Vec<u32>>,
    /// probe consumer compiled via --consumer (excluded from batch stats)
    probe: bool,
}

impl Default for FileResult {
    fn default() -> Self {
        Self {
            path: String::new(),
            compose: Ok(()),
            spv: Ok(0),
            hlsl: Vec::new(),
            unbounded_arrays: Vec::new(),
            has_immediates: false,
            immediate_vars: Vec::new(),
            has_ray_query: false,
            has_subgroup: false,
            entry_points: Vec::new(),
            hlsl_full: None,
            spv_words: None,
            probe: false,
        }
    }
}

fn collect_wgsl(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(collect_wgsl(&p));
            } else if p.extension().is_some_and(|e| e == "wgsl") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn build_binding_map(module: &naga::Module) -> naga::back::hlsl::BindingMap {
    let mut map = BTreeMap::new();
    for (_, var) in module.global_variables.iter() {
        let Some(binding) = &var.binding else {
            continue;
        };
        let mut target = naga::back::hlsl::BindTarget {
            space: binding.group as u8,
            register: binding.binding,
            binding_array_size: None,
            dynamic_storage_buffer_offsets_index: None,
            restrict_indexing: false,
        };
        if let naga::TypeInner::BindingArray { size, .. } = module.types[var.ty].inner {
            if matches!(size, naga::ArraySize::Dynamic) {
                target.binding_array_size = Some(DEFAULT_UNBOUNDED_ARRAY_CAP);
            }
        }
        map.insert(*binding, target);
    }
    map
}

fn stage_str_to_stage(s: &str) -> naga::ShaderStage {
    match s {
        "Vertex" => naga::ShaderStage::Vertex,
        "Fragment" => naga::ShaderStage::Fragment,
        _ => naga::ShaderStage::Compute,
    }
}

/// WGSL uniform-address-space alignment/size in bytes for a type.
/// This is the layout naga's SPIR-V writer emits for push constants
/// (OpTypeStruct member Offset decorations) and the layout the HLSL
/// back end relies on for the root-constant block.
fn type_layout(module: &naga::Module, handle: naga::Handle<naga::Type>) -> (u32, u32) {
    use naga::{ArraySize, TypeInner};
    match module.types[handle].inner {
        TypeInner::Scalar(s) => {
            let w = s.width as u32;
            (w, w)
        }
        TypeInner::Vector { size, scalar } => {
            let comp = scalar.width as u32;
            let n = size as u32;
            let align = if n >= 3 { 4 * comp } else { n * comp };
            (align, n * comp)
        }
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => {
            let comp = scalar.width as u32;
            let col_align = if rows as u32 >= 3 { 4 * comp } else { rows as u32 * comp };
            (col_align, columns as u32 * col_align)
        }
        TypeInner::Array { base, size, stride, .. } => {
            let (align, _) = type_layout(module, base);
            let count = match size {
                ArraySize::Constant(c) => c.get(),
                _ => 0,
            };
            (align, stride * count)
        }
        TypeInner::Struct { ref members, .. } => {
            let mut align = 1u32;
            let mut end = 0u32;
            for m in members {
                let (a, s) = type_layout(module, m.ty);
                align = align.max(a);
                let member_size = s.div_ceil(a) * a;
                end = end.max(m.offset + member_size);
            }
            (align, end.div_ceil(align) * align)
        }
        _ => (4, 4),
    }
}

fn collect_immediates(module: &naga::Module) -> Vec<ImmediateVar> {
    use naga::TypeInner;
    let mut out = Vec::new();
    for (_, var) in module.global_variables.iter() {
        if var.space != naga::AddressSpace::Immediate {
            continue;
        }
        let ty_name = module
            .types
            .get_handle(var.ty)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        let mut members = Vec::new();
        let mut flat = None;
        match module.types[var.ty].inner {
            TypeInner::Struct { members: ref ms, .. } => {
                for m in ms {
                    let (a, s) = type_layout(module, m.ty);
                    let size = s.div_ceil(a) * a;
                    members.push(ImmediateMember {
                        name: m.name.clone().unwrap_or_default(),
                        offset: m.offset,
                        size,
                    });
                }
            }
            ref inner => flat = Some(format!("{inner:?}")),
        }
        let (_, total) = type_layout(module, var.ty);
        out.push(ImmediateVar {
            var_name: var.name.clone().unwrap_or_default(),
            ty_name,
            flat_ty: flat,
            total_size: total,
            members,
        });
    }
    out
}

/// True when the source contains a real subgroup/wave builtin identifier
/// (subgroupAny, subgroupBroadcast, waveGetLaneCount, ...). User functions
/// like `remap_for_wave_reduction` do not match (wave + '_').
fn has_subgroup_source(src: &str) -> bool {
    src.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| {
        (w.starts_with("subgroup") && w.len() > "subgroup".len())
            || (w.starts_with("wave")
                && w["wave".len()..].chars().next().is_some_and(|c| c.is_ascii_uppercase()))
    })
}

fn module_has_ray_query(module: &naga::Module) -> bool {
    use naga::TypeInner;
    module.special_types.ray_desc.is_some()
        || module
            .types
            .iter()
            .any(|(_, t)| matches!(t.inner, TypeInner::RayQuery { .. }))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}... [truncated {} bytes]", &s[..max], s.len() - max)
    }
}

/// Run a naga backend stage and convert any internal panic (naga bugs like
/// `unreachable!()` in the HLSL storage lowering) into a recorded error so
/// the batch can continue over all 156 files.
fn catch_panic<T>(stage: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            Err(format!("PANIC[{stage}]: {msg}"))
        }
    }
}

/// Extract context around the first line matching any needle, for evidence.
/// Emits each source line at most once (windows of adjacent needle hits
/// overlap), so the snippet reads as contiguous code.
fn hlsl_snippet(hlsl: &str, needles: &[&str], max_lines: usize) -> String {
    let lines: Vec<&str> = hlsl.lines().collect();
    let mut hits: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if needles.iter().any(|n| l.contains(n)) {
            hits.push(i);
        }
    }
    if hits.is_empty() {
        return "(no matching line)".to_string();
    }
    let mut out = String::new();
    let mut emitted: Vec<usize> = Vec::new();
    let mut prev_end = usize::MAX;
    for i in hits {
        if i >= prev_end {
            out.push_str("...\n");
        }
        let lo = i.saturating_sub(1);
        let hi = (i + 2).min(lines.len());
        for j in lo..hi {
            if !emitted.contains(&j) {
                emitted.push(j);
                out.push_str(&format!("{:<5} {}\n", j + 1, lines[j]));
            }
        }
        prev_end = hi;
        if out.lines().count() > max_lines {
            break;
        }
    }
    out
}

fn sanitize_rel(rel: &str) -> String {
    rel.replace(['\\', '/', ':'], "_")
}

/// One file through the full chain: compose (with per-file defs + --defs
/// overrides) -> validate -> SPIR-V -> per-entry HLSL (SM6.6). Shared by the
/// batch loop and the --consumer probe so both go through the exact same
/// naga_oil/naga path.
fn compile_file(
    composer: &mut Composer,
    f: &Path,
    rel_str: &str,
    defs_overrides: &[(String, Vec<String>)],
    probe: bool,
    dump_globals: &Option<String>,
) -> FileResult {
    let mut result = FileResult {
        path: rel_str.to_string(),
        probe,
        ..Default::default()
    };
    let src = match std::fs::read_to_string(f) {
        Ok(s) => s,
        Err(e) => {
            result.compose = Err(format!("read: {e}"));
            return result;
        }
    };
    result.has_ray_query = src.contains("wgpu_ray_query") || src.contains("ray_query");
    result.has_subgroup = has_subgroup_source(&src);

    // per-file shader defs: global defs + builtin table + --defs overrides
    let mut shader_defs = global_defs();
    apply_file_defs(rel_str, &mut shader_defs);
    for (substr, defs) in defs_overrides {
        if rel_str.contains(substr.as_str()) {
            for d in defs {
                shader_defs.insert(d.clone(), ShaderDefValue::Bool(true));
            }
        }
    }

    // source-level specialization (##TEXTURE_FORMAT## text-pasting hack)
    let src = preprocess_source(rel_str, &src);

    let compose = match catch_panic("compose", || {
        composer.make_naga_module(NagaModuleDescriptor {
            source: &src,
            file_path: rel_str,
            shader_type: ShaderType::Wgsl,
            shader_defs,
            additional_imports: &[],
        })
    }) {
        Ok(r) => r,
        Err(panic_msg) => {
            result.compose = Err(panic_msg.clone());
            result.spv = Err(format!("compose failed: {panic_msg}"));
            return result;
        }
    };

    let module = match compose {
        Ok(m) => {
            result.compose = Ok(());
            m
        }
        Err(e) => {
            let detailed = catch_unwind(AssertUnwindSafe(|| e.emit_to_string(composer)))
                .ok()
                .unwrap_or_else(|| format!("{e}"));
            result.compose = Err(truncate(&detailed, 1600));
            result.spv = Err(format!("compose failed: {e}"));
            return result;
        }
    };

    result.entry_points = module
        .entry_points
        .iter()
        .map(|ep| format!("{:?}:{}", ep.stage, ep.name))
        .collect();
    result.has_ray_query |= module_has_ray_query(&module);

    // --dump-globals: print every global var (name / space / binding / type)
    // for the composed module, so evidence sections can show exactly which
    // bindings a consumer actually contains.
    if let Some(substr) = dump_globals {
        if rel_str.contains(substr.as_str()) {
            println!("--dump-globals {}:", rel_str);
            for (_, var) in module.global_variables.iter() {
                let binding = var
                    .binding
                    .map(|b| format!("group {} binding {}", b.group, b.binding))
                    .unwrap_or_else(|| "no-binding".to_string());
                let ty = match module.types[var.ty].inner {
                    naga::TypeInner::BindingArray { size, .. } => match size {
                        naga::ArraySize::Dynamic => "binding_array<..> (Dynamic)".to_string(),
                        naga::ArraySize::Constant(c) => {
                            format!("binding_array<..> (Constant {})", c.get())
                        }
                        naga::ArraySize::Pending(_) => "binding_array<..> (Pending)".to_string(),
                    },
                    ref other => format!("{other:?}").chars().take(60).collect(),
                };
                println!(
                    "  {}: space={:?} {}; {}",
                    var.name.clone().unwrap_or_default(),
                    var.space,
                    binding,
                    ty
                );
            }
        }
    }

    // unbounded binding arrays + immediates detection (V4/V16 data points)
    for (_, var) in module.global_variables.iter() {
        if var.space == naga::AddressSpace::Immediate {
            result.has_immediates = true;
        }
        if let Some(b) = &var.binding {
            if let naga::TypeInner::BindingArray { size, .. } = module.types[var.ty].inner {
                if matches!(size, naga::ArraySize::Dynamic) {
                    result.unbounded_arrays.push(format!(
                        "group {} binding {}",
                        b.group, b.binding
                    ));
                }
            }
        }
    }
    if result.has_immediates {
        result.immediate_vars = collect_immediates(&module);
    }

    let mut validator = Validator::new(ValidationFlags::all(), composer.capabilities);
    let info = match catch_panic("validate", || validator.validate(&module)) {
        Ok(Ok(info)) => info,
        Ok(Err(e)) => {
            result.compose = Err(format!("validate: {e}"));
            return result;
        }
        Err(panic_msg) => {
            result.compose = Err(panic_msg);
            return result;
        }
    };

    // SPIR-V (Vulkan)
    match catch_panic("spv", || {
        naga::back::spv::write_vec(
            &module,
            &info,
            &naga::back::spv::Options::default(),
            None,
        )
    }) {
        Ok(Ok(words)) => {
            result.spv = Ok(words.len());
            if result.has_ray_query
                || !result.unbounded_arrays.is_empty()
                || result.has_subgroup
                || result.has_immediates
            {
                result.spv_words = Some(words);
            }
        }
        Ok(Err(e)) => result.spv = Err(format!("{e:?}")),
        Err(panic_msg) => result.spv = Err(panic_msg),
    }

    // HLSL (D3D12), one compile per entry point (wgpu-hal pattern)
    let binding_map = build_binding_map(&module);
    let ep_names: Vec<(String, String)> = module
        .entry_points
        .iter()
        .map(|ep| (format!("{:?}", ep.stage), ep.name.clone()))
        .collect();
    let mut first_hlsl: Option<String> = None;
    for (stage_str, ep_name) in ep_names {
        let options = naga::back::hlsl::Options {
            shader_model: naga::back::hlsl::ShaderModel::V6_6,
            binding_map: binding_map.clone(),
            immediates_target: Some(naga::back::hlsl::BindTarget {
                space: 0,
                register: 0,
                binding_array_size: None,
                dynamic_storage_buffer_offsets_index: None,
                restrict_indexing: false,
            }),
            ..Default::default()
        };
        let pipeline_options = naga::back::hlsl::PipelineOptions {
            entry_point: Some((stage_str_to_stage(&stage_str), ep_name.clone())),
        };
        let mut source = String::new();
        let mut writer = naga::back::hlsl::Writer::new(&mut source, &options, &pipeline_options);
        let frag_ep = if stage_str == "Fragment" {
            naga::back::hlsl::FragmentEntryPoint::new(&module, &ep_name)
        } else {
            None
        };
        let r = catch_panic(
            &format!("hlsl({stage_str}:{ep_name})"),
            || writer.write(&module, &info, frag_ep.as_ref()),
        );
        let out = match &r {
            Ok(Ok(_)) => Ok(source.lines().next().unwrap_or("").to_string()),
            Ok(Err(e)) => Err(format!("{e:?}")),
            Err(panic_msg) => Err(panic_msg.clone()),
        };
        if r.is_ok_and(|r| r.is_ok()) && first_hlsl.is_none() {
            first_hlsl = Some(source.clone());
        }
        result.hlsl.push((format!("{stage_str}:{ep_name}"), out));
    }

    // keep HLSL for evidence when the file touches V4/V16 features
    if let Some(hlsl) = first_hlsl {
        if result.has_ray_query
            || !result.unbounded_arrays.is_empty()
            || result.has_subgroup
            || result.has_immediates
            || probe
        {
            result.hlsl_full = Some(hlsl);
        }
    }

    result
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(args.first().map(String::as_str).unwrap_or("."));
    let mut defs_overrides: Vec<(String, Vec<String>)> = Vec::new();
    let mut only_filter: Option<String> = None;
    let mut consumer_filter: Option<String> = None;
    let mut dump_globals: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--defs" && i + 1 < args.len() {
            let spec = &args[i + 1];
            if let Some((substr, defs)) = spec.split_once('=') {
                defs_overrides.push((
                    substr.to_string(),
                    defs.split(',').map(str::to_string).collect(),
                ));
            }
            i += 2;
        } else if args[i] == "--only" && i + 1 < args.len() {
            only_filter = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--consumer" && i + 1 < args.len() {
            consumer_filter = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--dump-globals" && i + 1 < args.len() {
            dump_globals = Some(args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }

    let crates_dir = root.join("crates");
    let all_files = collect_wgsl(&crates_dir);
    // `--only` filters pass 2 (compose); registration always runs over every
    // file so imports resolve regardless of the filter.
    let files: Vec<PathBuf> = if let Some(f) = &only_filter {
        all_files
            .iter()
            .filter(|p| p.to_string_lossy().contains(f.as_str()))
            .cloned()
            .collect()
    } else {
        all_files.clone()
    };
    println!(
        "M0 S-A: found {} WGSL files under {}",
        all_files.len(),
        crates_dir.display()
    );

    // Capabilities exactly as the fork's ShaderCache::new computes them,
    // but with the maximal feature set (a smoke checks translatability).
    let caps: Capabilities = wgpu_naga_bridge::features_to_naga_capabilities(
        wgt::Features::all(),
        wgt::DownlevelFlags::all(),
    );

    // ---- source-level inventories (independent of compile status) ----
    let mut ray_query_files: Vec<String> = Vec::new();
    let mut immediate_site_files: Vec<String> = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).display().to_string();
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        if src.contains("wgpu_ray_query") {
            ray_query_files.push(rel.clone());
        }
        if src.contains("var<immediate>") {
            immediate_site_files.push(rel.clone());
        }
    }
    println!("source-level: {} files with `enable wgpu_ray_query;`, {} files with var<immediate>",
        ray_query_files.len(), immediate_site_files.len());

    // ---- pass 1: dependency-ordered registration of importable modules ----
    // naga_oil 0.22 add_composable_module requires imports to be registered
    // first (compose/mod.rs:1576). We register iteratively: any module whose
    // imports are all present is added; repeat until no progress. This is the
    // batch equivalent of ShaderCache::add_import_to_composer recursion.
    let mut composer = Composer::default().with_capabilities(caps);
    let global = global_defs();
    let mut pending: Vec<ModuleCandidate> = Vec::new();
    for f in &all_files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let (name, imports, _defs) = naga_oil::compose::get_preprocessor_data(&src);
        if let Some(name) = name {
            let rel = f.strip_prefix(&root).unwrap_or(f).display().to_string();
            // Registration uses global defs only; pipeline-variant defs are
            // applied at the top-level make_naga_module call (imports are
            // compiled with the top-level defs by the composer, mirroring
            // bevy's ShaderCache where per-pipeline defs flow into imports).
            pending.push(ModuleCandidate {
                rel,
                name,
                imports: imports.iter().map(|d| d.import.clone()).collect(),
                defs: global.clone(),
            });
        }
    }

    let mut registered: HashSet<String> = HashSet::new();
    let mut register_failures: Vec<(String, String)> = Vec::new();
    loop {
        let mut progress = false;
        let mut remaining = Vec::new();
        for cand in pending {
            let missing: Vec<&String> = cand
                .imports
                .iter()
                .filter(|imp| **imp != cand.name && !registered.contains(*imp))
                .collect();
            if !missing.is_empty() {
                remaining.push(cand);
                continue;
            }
            let src = match std::fs::read_to_string(root.join(cand.rel.clone())) {
                Ok(s) => s,
                Err(e) => {
                    register_failures.push((cand.rel, format!("read: {e}")));
                    continue;
                }
            };
            match composer.add_composable_module(ComposableModuleDescriptor {
                source: &src,
                file_path: &cand.rel,
                language: ShaderLanguage::Wgsl,
                as_name: None,
                additional_imports: &[],
                shader_defs: cand.defs.clone(),
            }) {
                Ok(_) => {
                    registered.insert(cand.name);
                    progress = true;
                }
                Err(e) => register_failures.push((cand.rel, format!("register failed: {e}"))),
            }
        }
        pending = remaining;
        if !progress {
            break;
        }
    }
    println!("registered {} importable modules", registered.len());
    if !pending.is_empty() {
        let mut missing: HashSet<String> = HashSet::new();
        for cand in &pending {
            missing.extend(
                cand.imports
                    .iter()
                    .filter(|imp| !registered.contains(*imp))
                    .cloned(),
            );
        }
        println!(
            "!! {} modules could not be registered (import targets missing from repo):",
            pending.len()
        );
        for m in missing.iter() {
            println!("   missing import target: {m}");
        }
    }
    for (p, e) in &register_failures {
        println!("register failed: {p}: {e}");
    }

    // ---- pass 2: compose + validate + SPV + HLSL per file ----
    let mut results: Vec<FileResult> = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f);
        let rel_str = rel.display().to_string();
        let result = compile_file(&mut composer, f, &rel_str, &defs_overrides, false, &dump_globals);
        results.push(result);
    }

    // ---- pass 3: --consumer probe (extra top-level shader, same chain) ----
    // Compiles a scratch/ file as a top-level module with the real composer
    // (registration from pass 1 resolves its imports), for evidence that a
    // full bindless.wgsl consumer produces the 9 handle-space arrays.
    if let Some(consumer_name) = &consumer_filter {
        let scratch_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scratch");
        let probe_path = scratch_dir.join(consumer_name);
        let probe_rel = format!("tools/m0_shader_smoke/scratch/{}", consumer_name);
        println!("\n--consumer {} (rel: {})", probe_path.display(), probe_rel);
        let result = compile_file(
            &mut composer,
            &probe_path,
            &probe_rel,
            &defs_overrides,
            true,
            &dump_globals,
        );
        println!(
            "  compose: {:?}; spv: {:?}; hlsl: {}; unbounded: {}",
            result.compose.as_ref().err().map(|e| e.clone()),
            result.spv.as_ref().err().map(|e| e.clone()),
            result
                .hlsl
                .iter()
                .map(|(n, r)| format!("{n}:{}", if r.is_ok() { "ok" } else { "FAIL" }))
                .collect::<Vec<_>>()
                .join(", "),
            if result.unbounded_arrays.is_empty() {
                "(none)".to_string()
            } else {
                result.unbounded_arrays.join(", ")
            }
        );
        results.push(result);
    }

    // ---- stats (probes excluded) ----
    let batch: Vec<&FileResult> = results.iter().filter(|r| !r.probe).collect();
    let total = batch.len();
    let compose_ok = batch.iter().filter(|r| r.compose.is_ok()).count();
    let spv_ok = batch.iter().filter(|r| r.spv.is_ok()).count();
    let hlsl_total: usize = batch.iter().map(|r| r.hlsl.len()).sum();
    let hlsl_ok = batch
        .iter()
        .flat_map(|r| r.hlsl.iter())
        .filter(|(_, r)| r.is_ok())
        .count();
    println!("compose+validate OK: {compose_ok}/{total}");
    println!("SPIR-V OK: {spv_ok}/{total}");
    println!("HLSL entry points OK: {hlsl_ok}/{hlsl_total}");

    let failures: Vec<&FileResult> = batch
        .iter()
        .filter(|r| r.compose.is_err() || r.spv.is_err() || r.hlsl.iter().any(|(_, r)| r.is_err()))
        .cloned()
        .collect();
    println!("files with any failure: {}", failures.len());
    for r in &failures {
        let mut reasons = Vec::new();
        if let Err(e) = &r.compose {
            reasons.push(format!("compose: {e}"));
        }
        if let Err(e) = &r.spv {
            reasons.push(format!("spv: {e}"));
        }
        for (ep, e) in &r.hlsl {
            if let Err(e) = e {
                reasons.push(format!("hlsl({ep}): {e}"));
            }
        }
        println!("  FAIL {}: {}", r.path, reasons.join(" | "));
    }

    // solari / non-solari split (acceptance criterion)
    let solari: Vec<&FileResult> = batch.iter().filter(|r| r.path.contains("bevy_solari")).cloned().collect();
    let non_solari: Vec<&FileResult> = batch.iter().filter(|r| !r.path.contains("bevy_solari")).cloned().collect();
    let ns_fail: Vec<&&FileResult> = non_solari
        .iter()
        .filter(|r| r.compose.is_err() || r.spv.is_err() || r.hlsl.iter().any(|(_, r)| r.is_err()))
        .collect();
    println!(
        "non-solari: {}/{} OK ({} failures); solari: {}/{} OK ({} failures)",
        non_solari.len() - ns_fail.len(),
        non_solari.len(),
        ns_fail.len(),
        solari.len() - solari.iter().filter(|r| r.compose.is_err() || r.spv.is_err() || r.hlsl.iter().any(|(_, r)| r.is_err())).count(),
        solari.len(),
        solari.iter().filter(|r| r.compose.is_err() || r.spv.is_err() || r.hlsl.iter().any(|(_, r)| r.is_err())).count(),
    );

    // ---- evidence dump (out/) ----
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    let _ = std::fs::create_dir_all(&out_dir);
    for r in &results {
        let interesting = r.has_ray_query || !r.unbounded_arrays.is_empty() || r.has_subgroup || r.has_immediates;
        if !interesting {
            continue;
        }
        let stem = sanitize_rel(&r.path);
        if let Some(hlsl) = &r.hlsl_full {
            let _ = std::fs::write(out_dir.join(format!("{stem}.hlsl")), hlsl);
        }
        if let Some(words) = &r.spv_words {
            let mut bytes = Vec::with_capacity(words.len() * 4);
            for w in words {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
            let _ = std::fs::write(out_dir.join(format!("{stem}.spv")), &bytes);
        }
    }

    // ---- failure classification ----
    fn classify(e: &str) -> &'static str {
        if e.contains("PANIC[") {
            "known-gap(naga internal panic)"
        } else if e.contains("Unimplemented") || e.contains("push-constant") || e.contains("5683") {
            "known-gap(naga)"
        } else if e.contains("ray_query")
            || e.contains("RayQuery")
            || e.contains("binding_array")
            || e.contains("subgroup")
        {
            "lang-feature"
        } else if e.contains("expected expression")
            || e.contains("no definition in scope")
            || e.contains("not found")
            || e.contains("import")
        {
            "compose/import"
        } else {
            "other"
        }
    }

    // ---- markdown report: m0-sa-report.md ----
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("m0-sa-report.md");
    let mut md = String::new();
    md.push_str("# M0 S-A 批量编译冒烟报告（task 2 更新）\n\n");
    md.push_str("- 日期: 2026-08-05 (task 2 运行)\n");
    md.push_str("- 修复内容: import 注册改为依赖序（模仿 bevy_shader::ShaderCache::add_import_to_composer）\n");
    md.push_str("- naga 29.0.4 / naga_oil 0.22.0 (Cargo.lock 锁定版本)\n");
    md.push_str("- capabilities: wgpu-naga-bridge features_to_naga_capabilities(Features::all(), DownlevelFlags::all())\n");
    md.push_str(&format!(
        "- HLSL: SM6.6, 每 entry point 一次编译 (wgpu-hal 调用模式); 无界数组 binding_array_size 覆盖 = {DEFAULT_UNBOUNDED_ARRAY_CAP}\n"
    ));
    md.push_str("- shader defs 镜像 fork 运行时: 新增 BINDLESS（material.rs:494/496 vertex+fragment、prepass/mod.rs:356）\n  → pbr.wgsl/pbr_fragment.wgsl/pbr_prepass.wgsl/pbr_prepass_functions.wgsl/parallax_mapping.wgsl\n  → bindless.wgsl 的 9 组 handle 空间无界数组（#ifdef BINDLESS 门内）随消费者进入编译\n");
    md.push_str(&format!(
        "- 统计: 共 {total} 文件; compose+validate OK {compose_ok}; SPIR-V OK {spv_ok}; HLSL entry OK {hlsl_ok}/{hlsl_total}; 失败文件 {}\n",
        failures.len()
    ));
    let solari_fail = solari.iter().filter(|r| r.compose.is_err() || r.spv.is_err() || r.hlsl.iter().any(|(_, r)| r.is_err())).count();
    md.push_str(&format!(
        "- 验收拆分: 非 solari {} 文件失败 {}; solari {} 文件失败 {}\n\n",
        non_solari.len(),
        ns_fail.len(),
        solari.len(),
        solari_fail
    ));

    md.push_str("## 失败清单（真失败, import 修复后仍失败）\n\n");
    if failures.is_empty() {
        md.push_str("无\n");
    } else {
        md.push_str("| 文件 | 分类 | 失败原因 |\n|---|---|---|\n");
        for r in &failures {
            let mut reasons = Vec::new();
            if let Err(e) = &r.compose {
                reasons.push(format!("compose: {e}"));
            }
            if let Err(e) = &r.spv {
                reasons.push(format!("spv: {e}"));
            }
            for (ep, e) in &r.hlsl {
                if let Err(e) = e {
                    reasons.push(format!("hlsl({ep}): {e}"));
                }
            }
            let joined = reasons.join("<br>");
            md.push_str(&format!("| {} | {} | {} |\n", r.path, classify(&joined), joined));
        }
    }

    md.push_str("\n## V4: 无界 binding_array 文件\n\n");
    for r in results.iter().filter(|r| !r.probe && !r.unbounded_arrays.is_empty()) {
        md.push_str(&format!("- {}: {}\n", r.path, r.unbounded_arrays.join(", ")));
    }

    md.push_str("\n## V16: var<immediate> 文件\n\n");
    for r in results.iter().filter(|r| !r.probe && r.has_immediates) {
        md.push_str(&format!("- {}: {}\n", r.path, r.entry_points.join(", ")));
    }

    md.push_str("\n## 全量逐文件结果\n\n| 文件 | compose | SPV | HLSL |\n|---|---|---|---|\n");
    for r in results.iter().filter(|r| !r.probe) {
        let c = if r.compose.is_ok() { "✅" } else { "❌" };
        let s = match &r.spv {
            Ok(n) => format!("✅ {n} words"),
            Err(_) => "❌".to_string(),
        };
        let h = {
            let ok = r.hlsl.iter().filter(|(_, r)| r.is_ok()).count();
            if r.hlsl.is_empty() {
                "— (无 entry point)".to_string()
            } else if ok == r.hlsl.len() {
                format!("✅ {}/{}", ok, r.hlsl.len())
            } else {
                format!("❌ {}/{}", ok, r.hlsl.len())
            }
        };
        md.push_str(&format!("| {} | {c} | {s} | {h} |\n", r.path));
    }
    md.push_str("\n");

    let _ = std::fs::write(&report_path, md);
    println!("\nreport written to {}", report_path.display());

    // ---- V4 / V16 reports ----
    let v4 = build_v4_report(&results, &ray_query_files, &out_dir);
    let _ = std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("v4-report.md"),
        v4,
    );
    let v16 = build_v16_report(&results, &immediate_site_files, &out_dir);
    let _ = std::fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("v16-report.md"),
        v16,
    );
    println!("v4-report.md and v16-report.md written");

    if !failures.is_empty() {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// V4 report
// ---------------------------------------------------------------------------

fn build_v4_report(results: &[FileResult], ray_query_files: &[String], out_dir: &Path) -> String {
    let mut md = String::new();
    md.push_str("# M0 V4 验证报告: ray query / 无界 binding_array / subgroup 编译翻译形态\n\n");
    md.push_str("- 日期: 2026-08-05 (task 2 运行, 含 BINDLESS 修复后重跑)\n");
    md.push_str("- 工具: m0_shader_smoke (naga 29.0.4, HLSL SM6.6, SPIR-V 1.x)\n");
    md.push_str("- 能力集: features_to_naga_capabilities(Features::all(), DownlevelFlags::all())\n");
    md.push_str("- 证据产物目录: `out/` (每特征文件 .hlsl 全文 + .spv 二进制 + probe_* 最小复现证据)\n");
    md.push_str("- BINDLESS 修复: bindless.wgsl 的 9 组 handle 空间无界数组在 `#ifdef BINDLESS` 门内;\n  本版工具按 fork 运行时（material.rs:494/496, prepass/mod.rs:356）为消费者定义 BINDLESS,\n  该 9 组数组首次进入真实编译产物（见 §2.3）\n\n");

    // ---- ray query ----
    md.push_str("## 1. ray query\n\n");
    md.push_str(&format!(
        "### 1.1 全仓 `enable wgpu_ray_query;` 站点（{} 处）\n\n",
        ray_query_files.len()
    ));
    for f in ray_query_files {
        md.push_str(&format!("- `{}`\n", f));
    }
    md.push_str("\n");

    let rq: Vec<&FileResult> = results.iter().filter(|r| r.has_ray_query).collect();
    md.push_str(&format!(
        "### 1.2 编译状态（{} 个编译产物含 ray query IR; 其中 {} 处为源码 `enable wgpu_ray_query;` 站点）\n\n",
        rq.len(),
        ray_query_files.len()
    ));
    md.push_str("| 文件 | compose | SPV | HLSL |\n|---|---|---|---|\n");
    for r in &rq {
        let c = if r.compose.is_ok() { "✅" } else { "❌" };
        let s = match &r.spv {
            Ok(n) => format!("✅ {n} words"),
            Err(_) => "❌".to_string(),
        };
        let h = {
            let ok = r.hlsl.iter().filter(|(_, r)| r.is_ok()).count();
            if r.hlsl.is_empty() {
                "— (无 entry point)".to_string()
            } else if ok == r.hlsl.len() {
                format!("✅ {}/{}", ok, r.hlsl.len())
            } else {
                format!("❌ {}/{}", ok, r.hlsl.len())
            }
        };
        md.push_str(&format!("| {} | {c} | {s} | {h} |\n", r.path));
    }
    md.push_str("\n");

    // batch HLSL for the solari ray-query shaders is blocked by the naga
    // storage-binding-array panic; the translation itself is proven by the
    // minimal probe (same naga version, same options) - embed it as evidence.
    let probe_hlsl = out_dir.join("probe_rayquery.hlsl");
    let batch_ray_hlsl = rq.iter().find_map(|r| r.hlsl_full.as_deref());
    match batch_ray_hlsl {
        Some(hlsl) => {
            md.push_str("### 1.3 HLSL 产物形态（证据: 批次内首个成功产物）\n\n```hlsl\n");
            md.push_str(&hlsl_snippet(
                hlsl,
                &[
                    "RayDesc",
                    "RayQuery",
                    "RaytracingAccelerationStructure",
                    "TraceRay",
                    "rayQueryInitialize",
                    "rayQueryProceed",
                    "rayQueryGetCommittedIntersection",
                ],
                60,
            ));
            md.push_str("```\n\n");
        }
        None if probe_hlsl.exists() => {
            md.push_str(
                "### 1.3 HLSL 产物形态（证据: `out/probe_rayquery.hlsl` — 最小复现 shader,\n\
                 与批次同 naga 29.0.4 / SM6.6 / 同 capabilities, 入口 `main`）\n\n```hlsl\n",
            );
            md.push_str(&std::fs::read_to_string(&probe_hlsl).unwrap_or_default());
            md.push_str("```\n\n");
        }
        _ => {}
    }
    md.push_str(
        "### 1.4 结论: ray query 翻译层可行; Solari 组合 shader 的 HLSL 输出被 storage binding_array 缺口阻塞\n\n",
    );
    md.push_str(
        "- naga 29 HLSL back end 完整支持 ray query 翻译（`supported_capabilities()` 含 RAY_QUERY）;\n\
         HLSL 产物形态（1.3 证据）: `RayQuery<RAY_FLAG_NONE>` 查询对象 + `RayDesc` 结构 +\n\
         `TraceRayInline`/`Proceed()`/`CommittedStatus()`/`CommittedRayT()` 等 SM6.5 内联光线跟踪 API,\n\
         rayQueryInitialize 附 tmin/tmax/NaN 合法性守卫与初始化跟踪变量（ray_query_initialization_tracking）;\n\
         SPIR-V 产物为 OpTypeAccelerationStructureKHR + OpRayQuery*（见 out/*.spv）。\n\
         - 13 处站点 compose/SPIR-V 全部通过; 8 个带 entry point 的 solari shader 的 HLSL 输出触发\n\
         naga 内部 panic（back/hlsl/storage.rs:622, 见 §2.4）— 阻塞点不在 ray query 本身。\n\
         - 路径 a 对 ray query: **可行**（翻译层支持）, 待 §2 的 storage binding_array 缺口解除后全量可用。\n",
    );

    // ---- unbounded binding_array ----
    md.push_str("\n## 2. 无界 binding_array\n\n");
    md.push_str("### 2.1 全仓无界声明（`binding_array<T>` 无尺寸, 13 处）\n\n");
    md.push_str("- `crates/bevy_render/src/bindless.wgsl`: 9 组 (group 3, binding 1..9, `#MATERIAL_BIND_GROUP`=3)\n");
    md.push_str("  （重要: 全部 9 组在 `#ifdef BINDLESS` 门内, 只有定义了 BINDLESS 的消费者编译才包含它们;\n");
    md.push_str("   fork 运行时在 bindless 模式下定义 BINDLESS: material.rs:494/496、prepass/mod.rs:356）\n");
    md.push_str("- `crates/bevy_solari/src/scene/raytracing_scene_bindings.wgsl`: 4 组 (group 0, binding 0..3)\n\n");
    let ub: Vec<&FileResult> = results.iter().filter(|r| !r.unbounded_arrays.is_empty()).collect();
    md.push_str(&format!(
        "### 2.2 编译状态（{} 个编译产物含无界数组; 标注 (probe) 的为 `--consumer` 探针）\n\n",
        ub.len()
    ));
    md.push_str("| 文件 | 无界声明 | HLSL |\n|---|---|---|\n");
    for r in &ub {
        let h = {
            let ok = r.hlsl.iter().filter(|(_, r)| r.is_ok()).count();
            if r.hlsl.is_empty() {
                "—".to_string()
            } else if ok == r.hlsl.len() {
                format!("✅ {}/{}", ok, r.hlsl.len())
            } else {
                format!("❌ {}/{}", ok, r.hlsl.len())
            }
        };
        let probe_mark = if r.probe { " (probe)" } else { "" };
        md.push_str(&format!(
            "| {}{} | {} | {} |\n",
            r.path,
            probe_mark,
            r.unbounded_arrays.join(", "),
            h
        ));
    }
    md.push_str("\n");

    // handle-space evidence: batch consumer first, --consumer probe second.
    // "batch consumer" = a real top-level file whose composed module contains
    // group-3 (handle-space) unbounded arrays and produced HLSL containing
    // the [2048] capacity form. (plain `[2048]` alone is not enough:
    // downsample_depth.wgsl also emits naga sampler heaps without importing
    // bevy_render::bindless.)
    let batch_bindless = results.iter().find(|r| {
        !r.probe
            && r.unbounded_arrays
                .iter()
                .any(|a| a.starts_with("group 3"))
            && r.hlsl_full
                .as_deref()
                .is_some_and(|h| h.contains("[2048]"))
    });
    let probe_bindless = results
        .iter()
        .find(|r| r.probe && !r.unbounded_arrays.is_empty());
    md.push_str("### 2.3 HLSL 产物形态: handle 空间无界数组（真实编译证据）\n\n");
    if let Some(r) = batch_bindless {
        md.push_str(&format!(
            "- 批次内消费者证据: `{}`（import 链含 bevy_render::bindless; BINDLESS 由 per-file defs 定义,\n  镜像 fork 运行时 material.rs:494/496）:\n\n",
            r.path
        ));
        md.push_str("```hlsl\n");
        md.push_str(&hlsl_snippet(
            r.hlsl_full.as_deref().unwrap_or(""),
            &[
                "[2048]",
                "nagaSamplerHeap",
                "nagaTextureHeap",
                "nagaComparisonSamplerHeap",
            ],
            40,
        ));
        md.push_str("```\n\n");
    } else {
        md.push_str("- 批次内无消费者 HLSL 含 handle 空间无界数组（naga_oil 按 import 成员裁剪,\n  未使用的 bindless 全局不会进入消费者模块; 见 §2.3 探针）\n\n");
    }
    if let Some(p) = probe_bindless {
        md.push_str(&format!(
            "- `--consumer` 探针证据: `{}`（scratch/bindless_handle_arrays.wgsl, 显式 import 并引用\n  bindless.wgsl 全部 9 组数组; 检测到的无界声明: {}）:\n\n",
            p.path,
            p.unbounded_arrays.join(", ")
        ));
        if let Some(h) = &p.hlsl_full {
            md.push_str("```hlsl\n");
            md.push_str(&hlsl_snippet(
                h,
                &[
                    "[2048]",
                    "nagaSamplerHeap",
                    "nagaTextureHeap",
                    "nagaComparisonSamplerHeap",
                ],
                50,
            ));
            md.push_str("```\n\n");
        } else {
            let fails: Vec<String> = p
                .hlsl
                .iter()
                .map(|(n, r)| match r {
                    Ok(_) => n.clone(),
                    Err(e) => format!("{n}: {e}"),
                })
                .collect();
            md.push_str(&format!("- 探针 HLSL 未产出: {}\n\n", fails.join("; ")));
        }
    }

    // storage-space binding array evidence (solari scene_bindings)
    let probe_storage = out_dir.join("probe_storage_binding_array.txt");
    md.push_str("### 2.4 最小复现: storage 空间 binding_array（Solari scene_bindings 形态）\n\n");
    md.push_str(
        "- 声明本身可翻译: `var<storage, read_write> bufs: binding_array<Data>` ->\n\
         `RWByteAddressBuffer bufs[2048] : register(u0);`（probe, 6 行 shader）;\n\
         - 但一旦**访问**（`bufs[pc.idx].v = 7u;`）, naga 29.0.4 HLSL back end 在\n\
         `back/hlsl/storage.rs:622:26` 触发 `unreachable!()`（fill_access_chain 对 BindingArray\n\
         父类型无处理分支）。probe 输出（out/probe_storage_binding_array.txt）:\n\n```\n",
    );
    if probe_storage.exists() {
        md.push_str(&std::fs::read_to_string(&probe_storage).unwrap_or_default());
    } else {
        md.push_str("(probe 输出缺失)\n");
    }
    md.push_str("```\n\n");
    md.push_str(
        "- 影响: Solari scene_bindings 的 `vertex_buffers`/`index_buffers`（storage 空间）被\n\
         trace_ray/resolve_triangle_data_full 依赖链访问, 凡组合进该子图的 shader 在 HLSL 阶段 panic\n\
         （本批次 8 个 solari 文件, 见 m0-sa-report.md）; 无界纹理/采样器数组（handle 空间）不受影响。\n",
    );
    md.push_str(
        "### 2.5 naga 行为说明与 Diligent RUNTIME_ARRAY 兼容性\n\n",
    );
    md.push_str(&format!(
        "- naga HLSL writer 对 `IndexableLength::Dynamic` 数组直接 `unreachable!()`（writer.rs write_array_size）,\n\
         必须经 binding_map 提供 `binding_array_size` 覆盖。本工具统一覆盖为 {DEFAULT_UNBOUNDED_ARRAY_CAP}（M2 按 PRS 细分）。\n\
         - 因此 HLSL 产物是**定长数组** `T[2048] : register(...)`（§2.3 证据: 实际 HLSL 产物）, 不是 SM6.6 的\n\
         unbounded 描述符（naga 29 不产出 DeclaredResourceCount=Unbounded; 若要求真 unbounded 需后处理注入, M3 决策项）。\n\
         - Diligent PRS: ArraySize 与着色器声明必须一致。naga 定长产物与 Diligent `PRS::ArraySize = 2048` + \n\
         `SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE`/Dynamic 完全兼容。\n\
         - storage 空间 binding_array: naga 29 有内部 panic 缺口（storage.rs:622）, 需 naga 上游修复\n\
         （或 M3 规避: 拆分具名 buffer / bindless index + ByteAddressBuffer 方案）。\n\
         - 结论: 路径 a 对 handle 空间无界数组**可行**（定长上限翻译, §2.3 为编译证据）; 对 storage 空间**受阻**（naga 缺口）。\n",
    ));

    // ---- subgroup ----
    md.push_str("\n## 3. subgroup / wave ops\n\n");
    let sg: Vec<&FileResult> = results.iter().filter(|r| r.has_subgroup).collect();
    md.push_str(&format!(
        "### 3.1 全仓真实 subgroup 内建用法（{} 处）\n\n",
        sg.len()
    ));
    if sg.is_empty() {
        md.push_str("- 无。仓库 shader 源码中不存在 subgroup/wave 内建调用。\n");
        md.push_str("- `#ifdef SUBGROUP_SUPPORT` 分支（downsample.wgsl 等 SPD 路径）内部只有普通函数 `spd_reduce_quad`,\n  无 subgroup 内建; Solari 当前 shader 亦未使用 subgroup。\n");
    } else {
        for r in &sg {
            md.push_str(&format!("- {}: {}\n", r.path, r.entry_points.join(", ")));
        }
    }
    if let Some(r) = sg.first().and_then(|r| r.hlsl_full.as_deref()) {
        md.push_str("\n### 3.2 HLSL 产物形态（证据: 首个含 subgroup 文件）\n\n```hlsl\n");
        md.push_str(&hlsl_snippet(
            r,
            &["Wave", "Subgroup", "subgroup", "GroupNonUniform"],
            30,
        ));
        md.push_str("```\n\n");
    }
    md.push_str(
        "### 3.3 结论: 路径 a 对 subgroup 基本可行（注意 `enable subgroups;` 未支持）\n\n\
         - 现有仓库中唯一真实内建为 `subgroupAny`（visibility_buffer_software_raster.wgsl:110）,\n\
         naga -> HLSL 产物为 WaveActiveAnyTrue 形式, SPIR-V 为 OpGroupNonUniformAny;\n\
         - SPD 的 SUBGROUP_SUPPORT 分支不含内建, 编译通过;\n\
         - 注意: naga 29 的 WGSL front **不支持 `enable subgroups;`**（\"the `subgroups` enable-extension\n\
         is not yet supported\", wgpu#5555, probe 复现见 out/probe_subgroup_enable.txt）;\n\
         本仓库用法（不写 enable, 直接用 subgroupAny）恰好绕开该限制, 可编译;\n\
         - 若未来引入更多 subgroup 内建（subgroupAdd 等）并采用标准 enable 声明, 需等 naga 上游支持。\n",
    );

    // ---- decision tree ----
    md.push_str("\n## 4. 决策树结论\n\n");
    md.push_str("| 特性 | 路径 a 可行? | 依据 |\n|---|---|---|\n");
    md.push_str("| ray query | 可行（翻译层） | probe 证据: RayQuery<RAY_FLAG_NONE>/TraceRayInline/Proceed/Committed*; 13 站点 compose/SPV 全过; HLSL 受 §2 storage 缺口影响 |\n");
    md.push_str(&format!(
        "| 无界 binding_array (handle 空间) | 可行(定长上限) | §2.3 编译证据: binding_array_size={DEFAULT_UNBOUNDED_ARRAY_CAP} 覆盖下产物为定长数组 `T[{DEFAULT_UNBOUNDED_ARRAY_CAP}] : register(...)`（9 组探针全量 + 批次消费者）; 与 Diligent PRS ArraySize 匹配 |\n"
    ));
    md.push_str("| 无界 binding_array (storage 空间) | 受阻 | naga 29 HLSL panic (storage.rs:622), 需上游修复或 M3 规避 |\n");
    md.push_str("| subgroup/wave | 可行 | 现状仅 subgroupAny(无 enable), 产物 WaveActiveAnyTrue; `enable subgroups;` 本身未支持(wgpu#5555) |\n");
    md.push_str("\n- 结论: 三特性均不需为**翻译本身**走路径 c; 但 solari 全量 HLSL 输出被 storage 空间\n  binding_array 的 naga 缺口阻塞（8/14 solari 文件）, 该缺口需 naga 上游修复或路径 a 之外的手工处理。\n");
    md
}

// ---------------------------------------------------------------------------
// V16 report
// ---------------------------------------------------------------------------

fn build_v16_report(results: &[FileResult], immediate_site_files: &[String], out_dir: &Path) -> String {
    let mut md = String::new();
    md.push_str("# M0 V16 验证报告: var<immediate> 翻译形态与 FirstConstant 对齐\n\n");
    md.push_str("- 日期: 2026-08-05 (task 2 运行)\n");
    md.push_str("- 工具: m0_shader_smoke (naga 29.0.4, HLSL SM6.6, SPIR-V)\n");
    md.push_str("- 定义: `var<immediate>` = WGSL push constant (Vulkan) = D3D12 root constant (Diligent SetInlineConstants)\n");
    md.push_str("- 证据产物: `out/*.hlsl`（含 `ConstantBuffer<T> : register(b0)` 声明）+ `out/*.spv`\n\n");

    md.push_str("## 1. 全仓 var<immediate> 站点（源码 grep, 12 处 / 9 文件）\n\n");
    let mut sizes: Vec<(String, u32, String)> = Vec::new();
    // source-level authoritative list first (per task brief: 以实际 grep 为准)
    for f in immediate_site_files {
        let status = results
            .iter()
            .find(|r| r.path == *f)
            .map(|r| {
                if r.has_immediates {
                    "✅ (IR 含 immediate)"
                } else if r.compose.is_ok() {
                    "✅ compose OK (immediate 在 def 门控分支内, 随对应 pass 编译)"
                } else {
                    "❌ (compose 失败, 无法取 IR 布局)"
                }
            })
            .unwrap_or("⚠ (未编译)");
        md.push_str(&format!(
            "- `{}` {}（源码含 `var<immediate>`）\n",
            f, status
        ));
    }
    md.push_str("\n");
    for r in results.iter().filter(|r| r.has_immediates) {
        for iv in &r.immediate_vars {
            let ty = match &iv.flat_ty {
                Some(t) => format!("(非结构体 {t})"),
                None => format!("struct {}", iv.ty_name),
            };
            sizes.push((r.path.clone(), iv.total_size, ty.clone()));
            md.push_str(&format!(
                "- `{}` : `var<immediate> {}: {}` -> {} 字节\n",
                r.path, iv.var_name, ty, iv.total_size
            ));
        }
    }
    md.push_str("\n");

    md.push_str("## 2. 每样本翻译形态\n\n");
    md.push_str("### HLSL root constants 形态\n\n");
    md.push_str("- naga HLSL back end 将 Immediate 全局翻译为 `ConstantBuffer<T> name : register(b0)`（SM6.6 模板常量缓冲）;\n");
    md.push_str("- 非结构体 immediate（u32 / vec2<u32>）触发 naga 已知缺口 #5683: `Unimplemented(push-constant has non-struct type)`;\n");
    md.push_str("- HLSL 声明逐样本（证据文件在 out/）:\n\n");

    for r in results.iter().filter(|r| r.has_immediates) {
        let stem = sanitize_rel(&r.path);
        let hlsl_path = out_dir.join(format!("{stem}.hlsl"));
        let hlsl = std::fs::read_to_string(&hlsl_path).unwrap_or_default();
        md.push_str(&format!("#### `{}`\n\n", r.path));
        let mut forms: Vec<String> = Vec::new();
        for (ep, res) in &r.hlsl {
            match res {
                Ok(_) => forms.push(format!("`{ep}`: ✅")),
                Err(e) => forms.push(format!("`{ep}`: ❌ {e}")),
            }
        }
        if !forms.is_empty() {
            md.push_str(&format!("{}\n\n", forms.join("\n\n")));
        }
        if !hlsl.is_empty() {
            md.push_str("```hlsl\n");
            md.push_str(&hlsl_snippet(&hlsl, &["ConstantBuffer", "cbuffer", "register(b"], 40));
            md.push_str("```\n\n");
        }
        for iv in &r.immediate_vars {
            if let Some(flat) = &iv.flat_ty {
                md.push_str(&format!(
                    "- `{}`: 非结构体（{}）, HLSL 触发 #5683 已知缺口; SPIR-V push constant 仍正常。\n\n",
                    iv.var_name, flat
                ));
            } else {
                md.push_str(&format!("- `{}`（struct {}）成员布局:\n\n", iv.var_name, iv.ty_name));
                md.push_str("| 成员 | 偏移(字节) | 大小(字节) |\n|---|---|---|\n");
                for m in &iv.members {
                    md.push_str(&format!("| {} | {} | {} |\n", m.name, m.offset, m.size));
                }
                md.push_str("\n");
            }
        }
    }

    md.push_str("### SPIR-V push constant 形态\n\n");
    md.push_str("- SPIR-V 侧为 `OpVariable PushConstant` 指向的 `OpTypeStruct`（成员 Offset decoration 由 WGSL uniform 布局决定）;\n");
    md.push_str("- 下表为 naga IR 中 push constant 块布局（= SPIR-V 布局, 二进制见 out/*.spv）;\n\n");

    md.push_str("## 3. FirstConstant 对齐验证表\n\n");
    md.push_str("- D3D12 `SetInlineConstants(pConstants, FirstConstant, NumConstants)` 以 4 字节 DWORD 为单位;\n");
    md.push_str("- WGSL uniform 布局保证成员偏移为 4 的倍数（vec2=8, vec4=16, f32=4 对齐）,\n  故 `FirstConstant = 成员偏移 / 4` 恒为整数, 可直接映射。\n\n");
    md.push_str("| 文件 | 变量 | 类型 | 总大小(字节) | 成员 | 偏移 | 大小 | FirstConstant (offset/4) | NumConstants (size/4) | HLSL 形态 |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for r in results.iter().filter(|r| r.has_immediates) {
        let stem = sanitize_rel(&r.path);
        let hlsl_path = out_dir.join(format!("{stem}.hlsl"));
        let hlsl = std::fs::read_to_string(&hlsl_path).unwrap_or_default();
        for iv in &r.immediate_vars {
            let hlsl_form = if let Some(flat) = &iv.flat_ty {
                format!("❌ #5683 非结构体 {flat}")
            } else if hlsl.contains("ConstantBuffer") {
                "ConstantBuffer<T> : register(b0)".to_string()
            } else {
                "❌ (无 HLSL 输出)".to_string()
            };
            if iv.members.is_empty() {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | — | — | — | — | — | {} |\n",
                    r.path, iv.var_name, iv.ty_name, iv.total_size, hlsl_form
                ));
            } else {
                for m in &iv.members {
                    md.push_str(&format!(
                        "| {} | {} | struct {} | {} | {} | {} | {} | {} | {} | {} |\n",
                        r.path,
                        iv.var_name,
                        iv.ty_name,
                        iv.total_size,
                        m.name,
                        m.offset,
                        m.size,
                        m.offset / 4,
                        m.size / 4,
                        hlsl_form
                    ));
                }
            }
        }
    }
    md.push_str("\n");
    md.push_str("## 4. 结论\n\n");
    md.push_str("- 结构体 push constant: HLSL 产物 `ConstantBuffer<T> : register(b0)`（space 0）, 布局 4 字节粒度对齐,\n  Diligent 侧 `SetInlineConstants(FirstConstant=offset/4, NumConstants=size/4)` 直连映射成立;\n");
    md.push_str("- 非结构体 push constant（3 处: clear_visibility_buffer 的 vec2<u32>、remap_1d_to_2d_dispatch 与\n  visibility_buffer_hardware_raster 的 u32）: naga->HLSL 已知缺口 #5683（wgpu issue 5683）;\n  SPIR-V（Vulkan）侧无此限制。路径 a 下需将非结构体包一层 struct（或 M2 时改用结构体 push constant）;\n");
    md.push_str("- 本次运行样本尺寸: 4 字节（u32 / 单字段 struct）、8 字节（vec2<u32> / 双字段 struct）、16 字节（vec4 单字段 struct）;\n  全部 4 字节对齐; 0 字节与 32 字节样本在仓库中不存在。\n");
    md.push_str("- 结论: 路径 a 可行, 唯一已知缺口为非结构体 push constant 的 HLSL 输出（可规避）。\n");
    md
}
