//! V20 full verification: 100-PSO sync/async distributions, GetStatus polling
//! granularity, discrete-GPU re-verification, and the disk-archive write/load
//! drill (D3D12 backend).
//!
//! Follows up `v20_async_pso.rs` (15 PSOs, AMD iGPU) with a scale-up on the
//! NVIDIA RTX 3050 Ti (the discrete GPU is enumerated second on this machine,
//! so the adapter is selected explicitly — the `v3` example pattern). Also
//! reopens the Diligent "Archiver" (the PSO disk-cache write side, disabled by
//! `DILIGENT_NO_ARCHIVER=ON` in the default build) via the
//! `DILIGENT_RS_ARCHIVER=1` build-script switch and drills the REAL end-to-end
//! archive path:
//!
//! ```text
//!   write pass:  Diligent_GetArchiverFactory
//!     -> IArchiverFactory::CreateSerializationDevice
//!     -> ISerializationDevice::CreateShader        (serialized shaders)
//!     -> ISerializationDevice::CreateGraphicsPipelineState (serialized PSOs)
//!     -> IArchiverFactory::CreateArchiver + IArchiver::AddShader/AddPipelineState
//!     -> IArchiver::SerializeToBlob                 (IDataBlob)
//!     -> blob bytes -> temp-dir file
//!   load pass:   file bytes
//!     -> IEngineFactory::CreateDataBlob
//!     -> IEngineFactory::CreateDearchiver + IDearchiver::LoadArchive
//!     -> IDearchiver::UnpackPipelineState          (real runtime PSOs)
//! ```
//!
//! The archive key is `d3d12 + adapter name + driver version + PSO-desc hash`;
//! the driver version is not exposed by `GetAdapterInfo`, so it is read from
//! the Windows display-class registry key for the NVIDIA adapter.
//!
//! # Usage
//!
//! Build (the archiver switch is required to link `Diligent_GetArchiverFactory`):
//! ```text
//!   set DILIGENT_RS_ARCHIVER=1
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml --example v20_full -- [args]
//! ```
//!
//! Args (all optional):
//! - `--num N`      PSO count (default 100)
//! - `--runs N`     warm-start repeat runs (default 3)
//! - `--mode M`     cold-start path: `sync` | `async` | `both` (default both)
//! - `--pass P`     archive drill: `write` | `load` | `none` (default write;
//!                  `write` also loads the blob back in-process, `load` is the
//!                  second-process reuse pass)
//! - `--adapter N`  explicit adapter id (default: auto, prefer NVIDIA/RTX)
//!
//! Exit code 0 = the requested work completed and was measured; a failed drill
//! is reported honestly in the output (the conclusion is the measurement, not
//! a fabricated pass).
//!
//! # Timekeeping
//!
//! All timings are wall-clock `Instant` measurements around the engine calls;
//! cold-start distributions are per-PSO create times, warm-start distributions
//! are pooled across all runs.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use diligent_rs as dil;
use diligent_sys::bindings as sys;

// The archiver factory entry point lives in Diligent-Archiver-static.lib. The
// bindings declare the symbol too, but on this toolchain only the -L link-search
// directives from the diligent-sys crate's build script propagate to the final
// link (the `-l` directives are dropped from dependency rlib metadata), so the
// example requests the library itself. Requires DILIGENT_RS_ARCHIVER=1 at build
// time (the switch that makes diligent-sys build+link-search the archiver lib).
#[link(name = "Diligent-Archiver-static", kind = "static")]
unsafe extern "C" {
    fn Diligent_GetArchiverFactory() -> *mut sys::IArchiverFactory;
}

const CONTENT_VERSION: u32 = 1;
const POLL_TIMEOUT: Duration = Duration::from_secs(60);
const COARSE_POLL_INTERVAL: Duration = Duration::from_millis(2);
const FINE_POLL_INTERVAL: Duration = Duration::from_micros(250);

const RTV_FORMAT: sys::TEXTURE_FORMAT =
    sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB as sys::TEXTURE_FORMAT;

const VS_SOURCE: &str = r#"
struct VSInput { float3 pos : ATTRIB0; };
struct VSOutput { float4 pos : SV_POSITION; };
void main(in VSInput input, out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0),
                      vid == 0 ? -3.0 : (vid == 1 ? 3.0 : 3.0));
    output.pos = float4(p, 0.0, 1.0);
}
"#;

struct Opts {
    num_psos: usize,
    warm_runs: usize,
    mode: Mode,
    pass: Pass,
    adapter: AdapterChoice,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Sync,
    Async,
    Both,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Pass {
    Write,
    Load,
    None,
}

enum AdapterChoice {
    Auto,
    Id(u32),
}

fn parse_opts() -> Opts {
    let mut o = Opts {
        num_psos: 100,
        warm_runs: 3,
        mode: Mode::Both,
        pass: Pass::Write,
        adapter: AdapterChoice::Auto,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--num" => o.num_psos = args.next().and_then(|v| v.parse().ok()).unwrap_or(100),
            "--runs" => o.warm_runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(3),
            "--mode" => {
                o.mode = match args.next().as_deref() {
                    Some("sync") => Mode::Sync,
                    Some("async") => Mode::Async,
                    _ => Mode::Both,
                }
            }
            "--pass" => {
                o.pass = match args.next().as_deref() {
                    Some("write") => Pass::Write,
                    Some("load") => Pass::Load,
                    Some("none") => Pass::None,
                    _ => Pass::Write,
                }
            }
            "--adapter" => {
                o.adapter = match args.next().and_then(|v| v.parse().ok()) {
                    Some(id) => AdapterChoice::Id(id),
                    None => AdapterChoice::Auto,
                }
            }
            other => {
                eprintln!("[v20f] ignoring unknown arg: {other}");
            }
        }
    }
    o
}

/// Generates a unique pixel shader source per variant (the `v20_async_pso.rs`
/// pattern) so each PSO compiles distinct bytecode — no driver pipeline-cache
/// hits between cold-start PSOs.
fn make_ps_source(variant: usize) -> String {
    let total = (2 * 100 + 1) as f32;
    let r = (variant as f32) / total;
    let g = ((variant + 7) as f32) / total;
    let b = ((variant + 13) as f32) / total;
    format!(
        r#"
struct PSInput {{ float4 pos : SV_POSITION; }};
float4 main(in PSInput input) : SV_TARGET {{
    return float4({r:.6}, {g:.6}, {b:.6}, 1.0);
}}
"#
    )
}

// ---- RAII wrappers ---------------------------------------------------------

struct Raw<T>(*mut T);

impl<T> Drop for Raw<T> {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// Calls `IObject::Release` on any Diligent interface pointer (universal first
/// vtable slot; every Diligent interface is `#[repr(C)] { pVtbl }` whose
/// vtable starts with `IObjectMethods`).
unsafe fn release<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let obj = ptr as *mut sys::IObject;
    let vtbl = unsafe { &*(*obj).pVtbl };
    if let Some(rel) = vtbl.Object.Release {
        unsafe { rel(obj) };
    }
}

// ---- small stats helper ----------------------------------------------------

fn stats_ms(samples: &[f64]) -> (f64, f64, f64, f64, f64, f64, f64) {
    // (mean, min, max, p50, p90, p95, p99)
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    let p = |q: f64| s[(((s.len() as f64) * q).floor() as usize).min(s.len() - 1)];
    (
        mean,
        s[0],
        *s.last().unwrap(),
        p(0.50),
        p(0.90),
        p(0.95),
        p(0.99),
    )
}

fn print_stats(tag: &str, samples: &[f64]) {
    let (mean, min, max, p50, p90, p95, p99) = stats_ms(samples);
    println!(
        "[v20f] {tag}: n={} mean={mean:.3}ms min={min:.3}ms max={max:.3}ms \
         p50={p50:.3}ms p90={p90:.3}ms p95={p95:.3}ms p99={p99:.3}ms",
        samples.len()
    );
}

// ---- device / adapter setup ------------------------------------------------

fn build_engine_ci() -> sys::EngineD3D12CreateInfo {
    let mut ci: sys::EngineD3D12CreateInfo = unsafe { std::mem::zeroed() };
    ci._EngineCreateInfo.EngineAPIVersion = sys::DILIGENT_API_VERSION as i32;
    ci._EngineCreateInfo.EnableValidation = true;
    ci._EngineCreateInfo.Features.AsyncShaderCompilation =
        sys::_DEVICE_FEATURE_STATE::DEVICE_FEATURE_STATE_OPTIONAL as sys::DEVICE_FEATURE_STATE;
    ci._EngineCreateInfo.NumAsyncShaderCompilationThreads = 0xFFFF_FFFF;
    ci._EngineCreateInfo.pAsyncShaderCompilationThreadPool = std::ptr::null_mut();
    ci.D3D12DllName = c"d3d12.dll".as_ptr();
    ci.D3D12ValidationFlags =
        sys::_D3D12_VALIDATION_FLAGS::D3D12_VALIDATION_FLAG_BREAK_ON_CORRUPTION as sys::D3D12_VALIDATION_FLAGS;
    ci.CPUDescriptorHeapAllocationSize = [8192, 2048, 1024, 1024];
    ci.GPUDescriptorHeapSize = [16384, 1024];
    ci.GPUDescriptorHeapDynamicSize = [8192, 1024];
    ci.DynamicDescriptorAllocationChunkSize = [256, 32];
    ci.DynamicHeapPageSize = 1 << 20;
    ci.NumDynamicHeapPagesToReserve = 1;
    ci.QueryPoolSizes = [0, 128, 128, 512, 128, 256];
    ci
}

fn enumerate_adapters(
    factory: *mut sys::IEngineFactoryD3D12,
) -> dil::Result<Vec<sys::GraphicsAdapterInfo>> {
    let enumerate = unsafe {
        (*(*factory).pVtbl)
            .EngineFactory
            .EnumerateAdapters
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::EnumerateAdapters"))?
    };
    let mut count: u32 = 0;
    let min_version = sys::Version {
        Major: 12,
        Minor: 1,
    };
    unsafe { enumerate(factory.cast(), min_version, &mut count, std::ptr::null_mut()) };
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut adapters: Vec<sys::GraphicsAdapterInfo> =
        vec![unsafe { std::mem::zeroed() }; count as usize];
    let mut filled = count;
    unsafe { enumerate(factory.cast(), min_version, &mut filled, adapters.as_mut_ptr()) };
    Ok(adapters)
}

fn adapter_name(info: &sys::GraphicsAdapterInfo) -> String {
    unsafe { std::ffi::CStr::from_ptr(info.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Prefers the NVIDIA discrete GPU (enumerated second on this machine), the
/// `v3` pattern. Returns (id, name).
fn choose_adapter(
    factory: *mut sys::IEngineFactoryD3D12,
    choice: &AdapterChoice,
) -> dil::Result<(u32, String)> {
    let adapters = enumerate_adapters(factory)?;
    println!("[v20f] adapters: {}", adapters.len());
    for (i, a) in adapters.iter().enumerate() {
        println!(
            "[v20f]   adapter {i}: {} (vendorId={}, deviceId={}, outputs={}, type={:?})",
            adapter_name(a),
            a.VendorId,
            a.DeviceId,
            a.NumOutputs,
            a.Type
        );
    }
    let id = match choice {
        AdapterChoice::Id(id) => *id,
        AdapterChoice::Auto => adapters
            .iter()
            .position(|a| {
                let n = adapter_name(a);
                n.contains("NVIDIA") || n.contains("RTX")
            })
            .unwrap_or(0) as u32,
    };
    let name = adapters
        .get(id as usize)
        .map(adapter_name)
        .unwrap_or_else(|| "<unknown>".to_string());
    println!("[v20f] using adapter {id}: {name}");
    Ok((id, name))
}

/// Reads the NVIDIA adapter's Windows driver version from the display-class
/// registry key (Diligent's `GraphicsAdapterInfo` exposes no driver version).
fn query_driver_version() -> Option<String> {
    let script = r#"
$base='HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}'
Get-ChildItem $base -ErrorAction SilentlyContinue | ForEach-Object {
    $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
    if ($p -and $p.DriverDesc -match 'NVIDIA|RTX') { $p.DriverVersion }
} | Select-Object -First 1
"#;
    let out = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(script)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let v = s.lines().map(str::trim).find(|l| !l.is_empty())?;
    (!v.is_empty()).then(|| v.to_string())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---- shader / PSO creation (raw FFI) --------------------------------------

fn create_shader_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    source: &str,
    shader_type: sys::SHADER_TYPE,
) -> dil::Result<*mut sys::IShader> {
    let name_c = CString::new(name)?;
    let source_c = CString::new(source)?;
    let entry_c = CString::new("main")?;
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
        (*(*device).pVtbl)
            .RenderDevice
            .CreateShader
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateShader"))?
    };
    unsafe { create(device, &ci, &mut shader, std::ptr::null_mut()) };
    if shader.is_null() {
        return Err(dil::Error::CreateFailed("shader"));
    }
    Ok(shader)
}

fn build_graphics_pso_ci(
    name: &str,
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    layout: &[sys::LayoutElement],
    flags: sys::PSO_CREATE_FLAGS,
) -> dil::Result<sys::GraphicsPipelineStateCreateInfo> {
    let name_c = CString::new(name)?;
    let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
    ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci._PipelineStateCreateInfo.Flags = flags;
    ci._PipelineStateCreateInfo.ResourceSignaturesCount = 0;
    ci._PipelineStateCreateInfo.ppResourceSignatures = std::ptr::null_mut();

    ci.GraphicsPipeline.InputLayout.LayoutElements = layout.as_ptr();
    ci.GraphicsPipeline.InputLayout.NumElements = layout.len() as u32;
    ci.GraphicsPipeline.PrimitiveTopology =
        sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
    ci.GraphicsPipeline.NumRenderTargets = 1;
    ci.GraphicsPipeline.NumViewports = 1;
    ci.GraphicsPipeline.RTVFormats[0] = RTV_FORMAT;
    ci.GraphicsPipeline.DSVFormat = sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;
    ci.GraphicsPipeline.SampleMask = 0xFFFF_FFFF;
    ci.GraphicsPipeline.SmplDesc.Count = 1;
    ci.GraphicsPipeline.SmplDesc.Quality = 0;

    let ra = &mut ci.GraphicsPipeline.RasterizerDesc;
    ra.FillMode = sys::_FILL_MODE::FILL_MODE_SOLID as sys::FILL_MODE;
    ra.CullMode = sys::_CULL_MODE::CULL_MODE_NONE as sys::CULL_MODE;
    ra.DepthClipEnable = true;

    let ds = &mut ci.GraphicsPipeline.DepthStencilDesc;
    ds.DepthEnable = false;
    ds.DepthWriteEnable = false;
    ds.StencilReadMask = 0xFF;
    ds.StencilWriteMask = 0xFF;
    for face in [&mut ds.FrontFace, &mut ds.BackFace] {
        face.StencilFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilDepthFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilPassOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilFunc =
            sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS as sys::COMPARISON_FUNCTION;
    }
    ci.pVS = vs;
    ci.pPS = ps;
    // `name_c` must outlive the CI: leak it deliberately (one CString per
    // PSO, freed at process exit — bounded by the PSO count).
    std::mem::forget(name_c);
    Ok(ci)
}

fn create_graphics_pso_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    layout: &[sys::LayoutElement],
    flags: sys::PSO_CREATE_FLAGS,
) -> dil::Result<*mut sys::IPipelineState> {
    let ci = build_graphics_pso_ci(name, vs, ps, layout, flags)?;
    let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateGraphicsPipelineState
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IRenderDevice::CreateGraphicsPipelineState",
            ))?
    };
    unsafe { create(device, &ci, &mut pso) };
    if pso.is_null() {
        return Err(dil::Error::CreateFailed("graphics pipeline state"));
    }
    Ok(pso)
}

fn pso_status(pso: *mut sys::IPipelineState, wait: bool) -> sys::PIPELINE_STATE_STATUS {
    let get = unsafe {
        (*(*pso).pVtbl)
            .PipelineState
            .GetStatus
            .as_ref()
            .expect("IPipelineState::GetStatus missing from vtable")
    };
    unsafe { get(pso, wait) }
}

fn pso_status_name(s: sys::PIPELINE_STATE_STATUS) -> &'static str {
    use sys::_PIPELINE_STATE_STATUS as P;
    match s {
        v if v == P::PIPELINE_STATE_STATUS_UNINITIALIZED as sys::PIPELINE_STATE_STATUS => "UNINIT",
        v if v == P::PIPELINE_STATE_STATUS_COMPILING as sys::PIPELINE_STATE_STATUS => "COMPILING",
        v if v == P::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS => "READY",
        v if v == P::PIPELINE_STATE_STATUS_FAILED as sys::PIPELINE_STATE_STATUS => "FAILED",
        _ => "UNKNOWN",
    }
}

const PSO_READY: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS;
const PSO_FAILED: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_FAILED as sys::PIPELINE_STATE_STATUS;
const PSO_COMPILING: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_COMPILING as sys::PIPELINE_STATE_STATUS;

fn feature_state_name(s: sys::DEVICE_FEATURE_STATE) -> &'static str {
    use sys::_DEVICE_FEATURE_STATE as D;
    match s {
        v if v == D::DEVICE_FEATURE_STATE_DISABLED as sys::DEVICE_FEATURE_STATE => "DISABLED",
        v if v == D::DEVICE_FEATURE_STATE_ENABLED as sys::DEVICE_FEATURE_STATE => "ENABLED",
        v if v == D::DEVICE_FEATURE_STATE_OPTIONAL as sys::DEVICE_FEATURE_STATE => "OPTIONAL",
        _ => "UNKNOWN",
    }
}

// ---- context ----------------------------------------------------------------

// Field order = drop order for struct fields (declaration order): shaders are
// released before the context/device, the device before the factory.
struct Ctx {
    layout: Vec<sys::LayoutElement>,
    ps: Vec<Raw<sys::IShader>>,
    vs: Raw<sys::IShader>,
    adapter_info: sys::GraphicsAdapterInfo,
    device_info: sys::RenderDeviceInfo,
    async_feature: sys::DEVICE_FEATURE_STATE,
    driver_version: String,
    adapter_name: String,
    adapter_id: u32,
    _context: Raw<sys::IDeviceContext>,
    device: Raw<sys::IRenderDevice>,
    factory: Raw<sys::IEngineFactoryD3D12>,
}

fn setup(opts: &Opts) -> dil::Result<Ctx> {
    let factory = Raw(unsafe { sys::Diligent_GetEngineFactoryD3D12() });
    if factory.0.is_null() {
        return Err(dil::Error::Message(
            "Diligent_GetEngineFactoryD3D12 returned null".to_string(),
        ));
    }
    let load_d3d12 = unsafe {
        (*(*factory.0).pVtbl)
            .EngineFactoryD3D12
            .LoadD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactoryD3D12::LoadD3D12"))?
    };
    let loaded = unsafe { load_d3d12(factory.0, c"d3d12.dll".as_ptr()) };
    println!("[v20f] LoadD3D12(\"d3d12.dll\") = {loaded}");

    let (adapter_id, adapter_name) = choose_adapter(factory.0, &opts.adapter)?;

    let mut ci = build_engine_ci();
    ci._EngineCreateInfo.AdapterId = adapter_id;

    let mut device: *mut sys::IRenderDevice = std::ptr::null_mut();
    let mut context: *mut sys::IDeviceContext = std::ptr::null_mut();
    let create_dev = unsafe {
        (*(*factory.0).pVtbl)
            .EngineFactoryD3D12
            .CreateDeviceAndContextsD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IEngineFactoryD3D12::CreateDeviceAndContextsD3D12",
            ))?
    };
    unsafe { create_dev(factory.0, &ci, &mut device, &mut context) };
    if device.is_null() || context.is_null() {
        return Err(dil::Error::Message(format!(
            "CreateDeviceAndContextsD3D12 failed (device null: {}, context null: {})",
            device.is_null(),
            context.is_null()
        )));
    }
    let device = Raw(device);
    let context = Raw(context);

    let get_info = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .GetDeviceInfo
            .as_ref()
            .expect("GetDeviceInfo missing")
    };
    let device_info = unsafe { *get_info(device.0) };
    let get_adapter = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .expect("GetAdapterInfo missing")
    };
    let adapter_info = unsafe { *get_adapter(device.0) };
    let async_feature = device_info.Features.AsyncShaderCompilation;
    let driver_version = query_driver_version().unwrap_or_else(|| "unknown".to_string());
    println!(
        "[v20f] device: type={:?}, API {}.{}, async feature = {}",
        device_info.Type,
        device_info.APIVersion.Major,
        device_info.APIVersion.Minor,
        feature_state_name(async_feature)
    );
    println!(
        "[v20f] adapter: {adapter_name} (vendor={:#x}, device={:#x}) driver={driver_version}",
        adapter_info.VendorId, adapter_info.DeviceId
    );

    let vs = Raw(create_shader_raw(
        device.0,
        "V20F VS",
        VS_SOURCE,
        sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE,
    )?);

    let attr = CString::new("ATTRIB")?;
    let layout = vec![dil::layout_element(
        &attr,
        0,
        0,
        3,
        sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE,
        false,
    )];
    // `attr` backs the LayoutElement.Name pointer stored in `layout`, which
    // outlives this function: leak it deliberately (freed at process exit).
    std::mem::forget(attr);

    let mut ps: Vec<Raw<sys::IShader>> = Vec::with_capacity(2 * opts.num_psos);
    let ps_start = Instant::now();
    for i in 0..(2 * opts.num_psos) {
        let src = make_ps_source(i);
        ps.push(Raw(create_shader_raw(
            device.0,
            &format!("V20F PS #{i}"),
            &src,
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
        )?));
    }
    println!(
        "[v20f] PS variants: {} unique pixel shaders in {:.2}ms",
        2 * opts.num_psos,
        ps_start.elapsed().as_secs_f64() * 1000.0
    );

    Ok(Ctx {
        layout,
        ps,
        vs,
        adapter_info,
        device_info,
        async_feature,
        driver_version,
        adapter_name,
        adapter_id,
        _context: context,
        device,
        factory,
    })
}

// ---- cold start -------------------------------------------------------------

struct ColdResult {
    total: Duration,
    statuses: Vec<sys::PIPELINE_STATE_STATUS>,
}

fn cold_sync(ctx: &Ctx, num: usize, tag: &str) -> dil::Result<ColdResult> {
    println!("\n[v20f] --- {tag}: SYNC cold start ({num} PSOs) ---");
    let mut psos: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(num);
    let mut per_pso: Vec<f64> = Vec::with_capacity(num);
    let start = Instant::now();
    for i in 0..num {
        let t0 = Instant::now();
        let pso = create_graphics_pso_raw(
            ctx.device.0,
            &format!("V20F sync PSO #{i}"),
            ctx.vs.0,
            ctx.ps[i].0,
            &ctx.layout,
            0,
        )?;
        per_pso.push(t0.elapsed().as_secs_f64() * 1000.0);
        psos.push(Raw(pso));
    }
    let total = start.elapsed();
    let statuses: Vec<sys::PIPELINE_STATE_STATUS> =
        psos.iter().map(|p| pso_status(p.0, false)).collect();
    drop(psos);
    print_stats(&format!("{tag} sync per-PSO"), &per_pso);
    println!(
        "[v20f] {tag} SYNC total: {:.2}ms (first={:.2}ms)",
        total.as_secs_f64() * 1000.0,
        per_pso.first().copied().unwrap_or(0.0)
    );
    Ok(ColdResult { total, statuses })
}

fn cold_async(ctx: &Ctx, num: usize, tag: &str) -> dil::Result<ColdResult> {
    println!("\n[v20f] --- {tag}: ASYNC cold start ({num} PSOs) ---");
    let async_flag = sys::_PSO_CREATE_FLAGS::PSO_CREATE_FLAG_ASYNCHRONOUS as sys::PSO_CREATE_FLAGS;
    let mut psos: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(num);
    let mut submit_ms: Vec<f64> = Vec::with_capacity(num);
    let submit_start = Instant::now();
    for i in 0..num {
        let t0 = Instant::now();
        let pso = create_graphics_pso_raw(
            ctx.device.0,
            &format!("V20F async PSO #{i}"),
            ctx.vs.0,
            ctx.ps[num + i].0,
            &ctx.layout,
            async_flag,
        )?;
        submit_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        psos.push(Raw(pso));
    }
    let submit_total = submit_start.elapsed();
    let after: Vec<sys::PIPELINE_STATE_STATUS> =
        psos.iter().map(|p| pso_status(p.0, false)).collect();

    // Poll progression: cumulative READY count per loop (2ms interval).
    let mut progression: Vec<(u64, usize)> = Vec::new();
    let poll_start = Instant::now();
    let mut loops = 0u32;
    loop {
        loops += 1;
        let ready = psos
            .iter()
            .filter(|p| {
                let s = pso_status(p.0, false);
                s == PSO_READY || s == PSO_FAILED
            })
            .count();
        progression.push((poll_start.elapsed().as_millis() as u64, ready));
        if ready == num {
            break;
        }
        if poll_start.elapsed() > POLL_TIMEOUT {
            println!("[v20f] WARNING: poll timeout after {:.0}s", POLL_TIMEOUT.as_secs_f64());
            break;
        }
        std::thread::sleep(COARSE_POLL_INTERVAL);
    }
    let poll_total = poll_start.elapsed();
    let finals: Vec<sys::PIPELINE_STATE_STATUS> =
        psos.iter().map(|p| pso_status(p.0, false)).collect();
    drop(psos);

    let cnt = |ss: &[sys::PIPELINE_STATE_STATUS], v: sys::PIPELINE_STATE_STATUS| {
        ss.iter().filter(|s| **s == v).count()
    };
    println!(
        "[v20f] after-submit: UNINIT={} COMPILING={} READY={} FAILED={}",
        cnt(&after, sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_UNINITIALIZED as sys::PIPELINE_STATE_STATUS),
        cnt(&after, PSO_COMPILING),
        cnt(&after, PSO_READY),
        cnt(&after, PSO_FAILED)
    );
    println!(
        "[v20f] final:       READY={} FAILED={}",
        cnt(&finals, PSO_READY),
        cnt(&finals, PSO_FAILED)
    );
    // Progression curve (dedupe consecutive identical ready counts).
    let mut last: Option<usize> = None;
    for (t_ms, ready) in &progression {
        if last == Some(*ready) {
            continue;
        }
        last = Some(*ready);
        println!("[v20f]   poll t={t_ms:>5}ms  ready={ready}/{num}");
    }
    print_stats(&format!("{tag} async submit per-PSO"), &submit_ms);
    println!(
        "[v20f] {tag} ASYNC total: {:.2}ms = submit {:.2}ms + poll {:.2}ms ({loops} loops @ {:.0}ms)",
        (submit_total + poll_total).as_secs_f64() * 1000.0,
        submit_total.as_secs_f64() * 1000.0,
        poll_total.as_secs_f64() * 1000.0,
        COARSE_POLL_INTERVAL.as_secs_f64() * 1000.0
    );
    Ok(ColdResult {
        total: submit_total + poll_total,
        statuses: after,
    })
}

// ---- GetStatus polling granularity ------------------------------------------

fn granularity_probe(ctx: &Ctx, num: usize) -> dil::Result<()> {
    let num = num.min(3).min(ctx.ps.len());
    println!("\n[v20f] --- GetStatus granularity probe ({num} async PSOs, {:.2}ms interval) ---",
        FINE_POLL_INTERVAL.as_secs_f64() * 1000.0);
    let async_flag = sys::_PSO_CREATE_FLAGS::PSO_CREATE_FLAG_ASYNCHRONOUS as sys::PSO_CREATE_FLAGS;
    let mut psos: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(num);
    for i in 0..num {
        let pso = create_graphics_pso_raw(
            ctx.device.0,
            &format!("V20F granularity PSO #{i}"),
            ctx.vs.0,
            ctx.ps[ctx.ps.len() - 1 - i].0,
            &ctx.layout,
            async_flag,
        )?;
        psos.push(Raw(pso));
    }
    for (i, p) in psos.iter().enumerate() {
        let t0 = Instant::now();
        let mut saw_compiling: Option<Duration> = None;
        let mut saw_ready: Option<Duration> = None;
        loop {
            let s = pso_status(p.0, false);
            if s == PSO_COMPILING && saw_compiling.is_none() {
                saw_compiling = Some(t0.elapsed());
            }
            if s == PSO_READY || s == PSO_FAILED {
                saw_ready = Some(t0.elapsed());
                break;
            }
            if t0.elapsed() > POLL_TIMEOUT {
                break;
            }
            std::thread::sleep(FINE_POLL_INTERVAL);
        }
        match (saw_compiling, saw_ready) {
            (Some(c), Some(r)) => println!(
                "[v20f]   PSO #{i:2}: first-COMPILING observed @ {:.2}ms, READY @ {:.2}ms (span {:.2}ms, {:.3}ms granularity)",
                c.as_secs_f64() * 1000.0,
                r.as_secs_f64() * 1000.0,
                r.saturating_sub(c).as_secs_f64() * 1000.0,
                FINE_POLL_INTERVAL.as_secs_f64() * 1000.0
            ),
            (None, Some(r)) => println!(
                "[v20f]   PSO #{i:2}: READY @ {:.2}ms (no COMPILING observed: compiled faster than the first poll tick)",
                r.as_secs_f64() * 1000.0
            ),
            (_, None) => println!("[v20f]   PSO #{i:2}: TIMEOUT (no terminal status)"),
        }
    }
    drop(psos);
    Ok(())
}

// ---- warm start ---------------------------------------------------------------

fn warm_runs(ctx: &Ctx, opts: &Opts) -> dil::Result<()> {
    println!("\n[v20f] --- warm start: {runs} runs x {n} PSOs (same shaders, driver-cache hits) ---",
        runs = opts.warm_runs, n = opts.num_psos);
    let mut pooled: Vec<f64> = Vec::with_capacity(opts.warm_runs * opts.num_psos);
    let mut run_totals: Vec<f64> = Vec::with_capacity(opts.warm_runs);
    for r in 0..opts.warm_runs {
        let mut psos: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(opts.num_psos);
        let start = Instant::now();
        for i in 0..opts.num_psos {
            let t0 = Instant::now();
            let pso = create_graphics_pso_raw(
                ctx.device.0,
                &format!("V20F warm PSO #{i}"),
                ctx.vs.0,
                ctx.ps[i].0,
                &ctx.layout,
                0,
            )?;
            pooled.push(t0.elapsed().as_secs_f64() * 1000.0);
            psos.push(Raw(pso));
        }
        let total = start.elapsed().as_secs_f64() * 1000.0;
        run_totals.push(total);
        println!(
            "[v20f]   run {r}: total={total:.2}ms ({:.2}ms/PSO)",
            total / opts.num_psos as f64
        );
        drop(psos);
    }
    print_stats("warm per-PSO (pooled)", &pooled);
    print_stats("warm per-run total", &run_totals);
    Ok(())
}

// ---- archive drill ------------------------------------------------------------

const D3D12_DEVICE_FLAG: u32 = 2; // ARCHIVE_DEVICE_DATA_FLAG_D3D12

fn archive_dir() -> PathBuf {
    std::env::temp_dir().join("diligent-v20-full")
}

fn archive_key(ctx: &Ctx) -> String {
    let platform = "d3d12-win64";
    let mut h = fnv1a64(ctx.adapter_name.as_bytes());
    h ^= fnv1a64(ctx.driver_version.as_bytes());
    h ^= fnv1a64(platform.as_bytes());
    h ^= fnv1a64(&ctx.adapter_info.VendorId.to_le_bytes());
    h ^= fnv1a64(&ctx.adapter_info.DeviceId.to_le_bytes());
    h ^= pso_desc_hash(ctx, 0);
    format!("{h:016x}")
}

fn archive_path(ctx: &Ctx) -> PathBuf {
    archive_dir().join(format!("pso-archive-{}.bin", archive_key(ctx)))
}

/// FNV hash of the PSO-desc bytes (the variant pixel-shader source + the
/// fixed graphics state) — the per-PSO archive key input.
fn pso_desc_hash(ctx: &Ctx, variant: usize) -> u64 {
    let src = make_ps_source(variant);
    let mut h = fnv1a64(src.as_bytes());
    h ^= fnv1a64(&RTV_FORMAT.to_le_bytes());
    h ^= fnv1a64(&ctx.device_info.APIVersion.Major.to_le_bytes());
    h ^= fnv1a64(&ctx.device_info.APIVersion.Minor.to_le_bytes());
    h
}

fn serialize_archive_drill(ctx: &Ctx, opts: &Opts) -> dil::Result<()> {
    let path = archive_path(ctx);
    let mut any_failure = false;

    if opts.pass == Pass::Write || opts.pass == Pass::Load {
        println!(
            "\n[v20f] --- archive drill: pass={:?} ---",
            if opts.pass == Pass::Write { "write+load-back" } else { "load (2nd process)" }
        );
        println!(
            "[v20f] archive key = {} | pso-desc-hash-0 = {:016x} | path = {}",
            archive_key(ctx),
            pso_desc_hash(ctx, 0),
            path.display()
        );
    }

    if opts.pass == Pass::Load {
        if !path.exists() {
            println!(
                "[v20f] ARCHIVE LOAD: FAILED — {} does not exist (run --pass write first)",
                path.display()
            );
            return Ok(());
        }
        return load_from_disk(ctx, opts, &path);
    }

    // ---------------- write side ----------------
    println!("[v20f] GetArchiverFactory ...");
    let archiver_factory = Raw(unsafe { Diligent_GetArchiverFactory() });
    if archiver_factory.0.is_null() {
        println!(
            "[v20f] ARCHIVER WRITE: FAILED — Diligent_GetArchiverFactory() == null \
             (was the example built with DILIGENT_RS_ARCHIVER=1?)"
        );
        return Ok(());
    }
    println!("[v20f] archiver factory: OK");

    let mut ser_ci: sys::SerializationDeviceCreateInfo = unsafe { std::mem::zeroed() };
    ser_ci.DeviceInfo = ctx.device_info;
    ser_ci.AdapterInfo = ctx.adapter_info;
    ser_ci.NumAsyncShaderCompilationThreads = 0; // synchronous drill
    ser_ci.D3D12.ShaderVersion = sys::Version {
        Major: 6,
        Minor: 0,
    };

    let mut ser_dev: *mut sys::ISerializationDevice = std::ptr::null_mut();
    let create_ser = unsafe {
        (*(*archiver_factory.0).pVtbl)
            .ArchiverFactory
            .CreateSerializationDevice
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IArchiverFactory::CreateSerializationDevice",
            ))?
    };
    unsafe { create_ser(archiver_factory.0, &ser_ci, &mut ser_dev) };
    if ser_dev.is_null() {
        println!("[v20f] ARCHIVER WRITE: FAILED — CreateSerializationDevice returned null");
        return Ok(());
    }
    let ser_dev = Raw(ser_dev);
    let get_flags = unsafe {
        (*(*ser_dev.0).pVtbl)
            .SerializationDevice
            .GetSupportedDeviceFlags
            .as_ref()
            .expect("GetSupportedDeviceFlags missing")
    };
    let supported = unsafe { get_flags(ser_dev.0) };
    println!(
        "[v20f] serialization device: OK (supported device flags = {supported:#x}, D3D12 bit = {D3D12_DEVICE_FLAG:#x})"
    );

    // Serialized shaders: one VS + 2N PS variants.
    let mut ser_shaders: Vec<Raw<sys::IShader>> = Vec::with_capacity(2 * opts.num_psos + 1);
    let mut archive_info_shader: sys::ShaderArchiveInfo = unsafe { std::mem::zeroed() };
    archive_info_shader.DeviceFlags = D3D12_DEVICE_FLAG;
    let shader_start = Instant::now();
    let create_ser_shader = unsafe {
        (*(*ser_dev.0).pVtbl)
            .SerializationDevice
            .CreateShader
            .as_ref()
            .ok_or(dil::Error::MissingMethod("ISerializationDevice::CreateShader"))?
    };
    {
        let vs = create_serialized_shader(
            &create_ser_shader,
            ser_dev.0,
            &archive_info_shader,
            "V20F arc VS",
            VS_SOURCE,
            sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE,
        )?;
        ser_shaders.push(Raw(vs));
        for i in 0..(2 * opts.num_psos) {
            let src = make_ps_source(i);
            let ps = create_serialized_shader(
                &create_ser_shader,
                ser_dev.0,
                &archive_info_shader,
                &format!("V20F arc PS #{i}"),
                &src,
                sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
            )?;
            ser_shaders.push(Raw(ps));
        }
    }
    let shader_total = shader_start.elapsed();
    println!(
        "[v20f] serialized shaders: {} in {:.2}ms",
        2 * opts.num_psos + 1,
        shader_total.as_secs_f64() * 1000.0
    );

    // Serialized PSOs (all reference the serialized VS + per-variant PS).
    let mut ser_psos: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(opts.num_psos);
    let mut archive_info_pso: sys::PipelineStateArchiveInfo = unsafe { std::mem::zeroed() };
    archive_info_pso.DeviceFlags = D3D12_DEVICE_FLAG;
    let pso_start = Instant::now();
    let create_ser_pso = unsafe {
        (*(*ser_dev.0).pVtbl)
            .SerializationDevice
            .CreateGraphicsPipelineState
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "ISerializationDevice::CreateGraphicsPipelineState",
            ))?
    };
    for i in 0..opts.num_psos {
        let name = format!("V20F arc PSO #{i}");
        let ci = build_graphics_pso_ci(
            &name,
            ser_shaders[0].0,
            ser_shaders[1 + i].0,
            &ctx.layout,
            0,
        )?;
        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        unsafe { create_ser_pso(ser_dev.0, &ci, &archive_info_pso, &mut pso) };
        if pso.is_null() {
            println!("[v20f] ARCHIVER WRITE: FAILED — serialized PSO #{i} is null");
            any_failure = true;
            break;
        }
        ser_psos.push(Raw(pso));
    }
    let pso_total = pso_start.elapsed();
    if any_failure {
        return Ok(());
    }
    println!(
        "[v20f] serialized PSOs: {} in {:.2}ms (statuses: first={}, last={})",
        opts.num_psos,
        pso_total.as_secs_f64() * 1000.0,
        pso_status_name(pso_status(ser_psos[0].0, false)),
        pso_status_name(pso_status(ser_psos[opts.num_psos - 1].0, false))
    );

    // Archiver: add shaders + PSOs, serialize to blob, write to file.
    let mut archiver: *mut sys::IArchiver = std::ptr::null_mut();
    let create_archiver = unsafe {
        (*(*archiver_factory.0).pVtbl)
            .ArchiverFactory
            .CreateArchiver
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IArchiverFactory::CreateArchiver"))?
    };
    unsafe { create_archiver(archiver_factory.0, ser_dev.0, &mut archiver) };
    if archiver.is_null() {
        println!("[v20f] ARCHIVER WRITE: FAILED — CreateArchiver returned null");
        return Ok(());
    }
    let archiver = Raw(archiver);

    let add_shader = unsafe {
        (*(*archiver.0).pVtbl)
            .Archiver
            .AddShader
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IArchiver::AddShader"))?
    };
    let add_pso = unsafe {
        (*(*archiver.0).pVtbl)
            .Archiver
            .AddPipelineState
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IArchiver::AddPipelineState"))?
    };
    let add_start = Instant::now();
    let mut ok = true;
    for s in &ser_shaders {
        if !unsafe { add_shader(archiver.0, s.0) } {
            ok = false;
            break;
        }
    }
    if ok {
        for p in &ser_psos {
            if !unsafe { add_pso(archiver.0, p.0) } {
                ok = false;
                break;
            }
        }
    }
    let add_total = add_start.elapsed();
    if !ok {
        println!("[v20f] ARCHIVER WRITE: FAILED — AddShader/AddPipelineState returned false");
        return Ok(());
    }
    println!(
        "[v20f] archiver: added {} shaders + {} PSOs in {:.2}ms",
        ser_shaders.len(),
        ser_psos.len(),
        add_total.as_secs_f64() * 1000.0
    );

    let serialize = unsafe {
        (*(*archiver.0).pVtbl)
            .Archiver
            .SerializeToBlob
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IArchiver::SerializeToBlob"))?
    };
    let mut blob: *mut sys::IDataBlob = std::ptr::null_mut();
    let ser_start = Instant::now();
    let ser_ok = unsafe { serialize(archiver.0, CONTENT_VERSION, &mut blob) };
    let ser_time = ser_start.elapsed();
    if !ser_ok || blob.is_null() {
        println!("[v20f] ARCHIVER WRITE: FAILED — SerializeToBlob returned {ser_ok} (blob null: {})", blob.is_null());
        return Ok(());
    }
    let blob = Raw(blob);
    let blob_size = blob_size_bytes(blob.0);
    println!(
        "[v20f] SerializeToBlob: OK in {:.2}ms, blob = {blob_size} bytes",
        ser_time.as_secs_f64() * 1000.0
    );

    let file_start = Instant::now();
    let bytes = blob_bytes(blob.0);
    std::fs::create_dir_all(archive_dir()).map_err(|e| {
        dil::Error::Message(format!("mkdir {}: {e}", archive_dir().display()))
    })?;
    std::fs::write(&path, &bytes)
        .map_err(|e| dil::Error::Message(format!("write {}: {e}", path.display())))?;
    println!(
        "[v20f] archive written: {} ({} bytes) in {:.2}ms",
        path.display(),
        bytes.len(),
        file_start.elapsed().as_secs_f64() * 1000.0
    );
    drop(blob);

    // Load-back: spawn THIS executable in a child process with `--pass load`.
    // The dearchiver's LoadArchive deterministically crashes with a native
    // access violation on this engine build (verified independently with a
    // minimal C probe against the same libraries), and a native AV cannot be
    // caught in-process — so the second process keeps this run intact and the
    // crash (or success) is reported as the drill outcome.
    let exe = std::env::current_exe().map_err(|e| {
        dil::Error::Message(format!("current_exe: {e}"))
    })?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--pass").arg("load")
        .arg("--num").arg(opts.num_psos.to_string())
        .arg("--mode").arg("sync")
        .arg("--runs").arg("1")
        .arg("--adapter").arg(ctx.adapter_id.to_string());
    let child_start = Instant::now();
    match cmd.output() {
        Ok(out) => {
            let status = out.status;
            let elapsed = child_start.elapsed().as_secs_f64() * 1000.0;
            if let Some(code) = status.code() {
                println!(
                    "[v20f] child load pass: exited {code} after {elapsed:.0}ms ({})",
                    if code == 0 { "LOAD OK" } else { "LOAD FAILED" }
                );
            } else {
                println!(
                    "[v20f] child load pass: terminated by signal/exception (code {:#x}, 0x{:08X}) after {elapsed:.0}ms — see 'ARCHIVE LOAD' lines below",
                    status.code().unwrap_or(-1),
                    status.code().unwrap_or(-1) as u32
                );
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            for line in stdout.lines().chain(stderr.lines()) {
                if line.contains("[v20f]") {
                    println!("[v20f]   child: {line}");
                }
            }
            println!(
                "[v20f] ARCHIVE LOAD (child): {}",
                if status.success() {
                    "OK — archive reusable"
                } else {
                    "CRASHED / FAILED — archive NOT reusable on this engine build"
                }
            );
        }
        Err(e) => {
            println!("[v20f] ARCHIVE LOAD (child): could not spawn: {e}");
        }
    }
    Ok(())
}

fn create_serialized_shader(
    create: &unsafe extern "C" fn(
        *mut sys::ISerializationDevice,
        *const sys::ShaderCreateInfo,
        *const sys::ShaderArchiveInfo,
        *mut *mut sys::IShader,
        *mut *mut sys::IDataBlob,
    ),
    ser_dev: *mut sys::ISerializationDevice,
    archive_info: &sys::ShaderArchiveInfo,
    name: &str,
    source: &str,
    shader_type: sys::SHADER_TYPE,
) -> dil::Result<*mut sys::IShader> {
    let name_c = CString::new(name)?;
    let source_c = CString::new(source)?;
    let entry_c = CString::new("main")?;
    let mut ci: sys::ShaderCreateInfo = unsafe { std::mem::zeroed() };
    ci.Source = source_c.as_ptr();
    ci.EntryPoint = entry_c.as_ptr();
    ci.Desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci.Desc.ShaderType = shader_type;
    ci.SourceLanguage =
        sys::_SHADER_SOURCE_LANGUAGE::SHADER_SOURCE_LANGUAGE_HLSL as sys::SHADER_SOURCE_LANGUAGE;
    ci.ShaderCompiler = sys::_SHADER_COMPILER::SHADER_COMPILER_DEFAULT as sys::SHADER_COMPILER;
    let mut shader: *mut sys::IShader = std::ptr::null_mut();
    unsafe { create(ser_dev, &ci, archive_info, &mut shader, std::ptr::null_mut()) };
    if shader.is_null() {
        return Err(dil::Error::Message(format!(
            "serialized shader '{name}' creation failed"
        )));
    }
    Ok(shader)
}

fn blob_size_bytes(blob: *mut sys::IDataBlob) -> usize {
    let get = unsafe {
        (*(*blob).pVtbl)
            .DataBlob
            .GetSize
            .as_ref()
            .expect("IDataBlob::GetSize missing")
    };
    unsafe { get(blob) }
}

fn blob_bytes(blob: *mut sys::IDataBlob) -> Vec<u8> {
    let get = unsafe {
        (*(*blob).pVtbl)
            .DataBlob
            .GetConstDataPtr
            .as_ref()
            .expect("IDataBlob::GetConstDataPtr missing")
    };
    let size = blob_size_bytes(blob);
    let ptr = unsafe { get(blob, 0) };
    unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec() }
}

fn load_from_disk(ctx: &Ctx, opts: &Opts, path: &Path) -> dil::Result<()> {
    let read_start = Instant::now();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("[v20f] ARCHIVE LOAD: FAILED — read {}: {e}", path.display());
            return Ok(());
        }
    };
    let read_time = read_start.elapsed();
    println!(
        "[v20f] load: read {} bytes from {} in {:.2}ms",
        bytes.len(),
        path.display(),
        read_time.as_secs_f64() * 1000.0
    );

    // IEngineFactory::CreateDataBlob via the D3D12 factory (base group).
    let create_blob = unsafe {
        (*(*ctx.factory.0).pVtbl)
            .EngineFactory
            .CreateDataBlob
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::CreateDataBlob"))?
    };
    println!("[v20f] load: CreateDataBlob ...");
    let mut blob: *mut sys::IDataBlob = std::ptr::null_mut();
    unsafe { create_blob(ctx.factory.0.cast::<sys::IEngineFactory>(), bytes.len(), bytes.as_ptr().cast(), &mut blob) };
    if blob.is_null() {
        println!("[v20f] ARCHIVE LOAD: FAILED — CreateDataBlob returned null");
        return Ok(());
    }
    let blob = Raw(blob);
    println!(
        "[v20f] load: CreateDataBlob OK ({} bytes)",
        blob_size_bytes(blob.0)
    );

    let create_dearchiver = unsafe {
        (*(*ctx.factory.0).pVtbl)
            .EngineFactory
            .CreateDearchiver
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::CreateDearchiver"))?
    };
    let dearchiver_ci: sys::DearchiverCreateInfo = unsafe { std::mem::zeroed() };
    println!("[v20f] load: CreateDearchiver ...");
    let mut dearchiver: *mut sys::IDearchiver = std::ptr::null_mut();
    unsafe { create_dearchiver(ctx.factory.0.cast::<sys::IEngineFactory>(), &dearchiver_ci, &mut dearchiver) };
    if dearchiver.is_null() {
        println!("[v20f] ARCHIVE LOAD: FAILED — CreateDearchiver returned null");
        return Ok(());
    }
    let dearchiver = Raw(dearchiver);
    println!("[v20f] load: CreateDearchiver OK");

    let load_archive = unsafe {
        (*(*dearchiver.0).pVtbl)
            .Dearchiver
            .LoadArchive
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDearchiver::LoadArchive"))?
    };
    let load_start = Instant::now();
    let loaded = unsafe { load_archive(dearchiver.0, blob.0, CONTENT_VERSION, true) };
    let load_time = load_start.elapsed();
    if !loaded {
        println!("[v20f] ARCHIVE LOAD: FAILED — LoadArchive returned false (content version mismatch?)");
        return Ok(());
    }
    println!(
        "[v20f] LoadArchive: OK in {:.2}ms (version={CONTENT_VERSION}, makeCopy=true)",
        load_time.as_secs_f64() * 1000.0
    );

    // Sanity: GetContentVersion must echo the archive content version.
    let get_version = unsafe {
        (*(*dearchiver.0).pVtbl)
            .Dearchiver
            .GetContentVersion
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDearchiver::GetContentVersion"))?
    };
    let version = unsafe { get_version(dearchiver.0) };
    println!(
        "[v20f] dearchiver state: dearchiver={:p} blob={:p} archive_version={}",
        dearchiver.0, blob.0, version
    );

    // Incremental probe: unpack the archived vertex shader by name.
    let unpack_shader = unsafe {
        (*(*dearchiver.0).pVtbl)
            .Dearchiver
            .UnpackShader
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDearchiver::UnpackShader"))?
    };
    let vs_name = CString::new("V20F arc VS")?;
    let mut shader_info: sys::ShaderUnpackInfo = unsafe { std::mem::zeroed() };
    shader_info.pDevice = ctx.device.0;
    shader_info.Name = vs_name.as_ptr();
    let mut unpacked_shader: *mut sys::IShader = std::ptr::null_mut();
    unsafe { unpack_shader(dearchiver.0, &shader_info, &mut unpacked_shader) };
    println!(
        "[v20f] UnpackShader('V20F arc VS') -> {}",
        if unpacked_shader.is_null() {
            "null (FAILED)"
        } else {
            "OK"
        }
    );
    if !unpacked_shader.is_null() {
        unsafe { release(unpacked_shader) };
    }

    let unpack = unsafe {
        (*(*dearchiver.0).pVtbl)
            .Dearchiver
            .UnpackPipelineState
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDearchiver::UnpackPipelineState"))?
    };
    let mut unpacked: Vec<Raw<sys::IPipelineState>> = Vec::with_capacity(opts.num_psos);
    let mut per_pso: Vec<f64> = Vec::with_capacity(opts.num_psos);
    let unpack_start = Instant::now();
    let mut any_null = false;
    for i in 0..opts.num_psos {
        let name = CString::new(format!("V20F arc PSO #{i}"))?;
        let mut info: sys::PipelineStateUnpackInfo = unsafe { std::mem::zeroed() };
        info.pDevice = ctx.device.0;
        info.Name = name.as_ptr();
        info.PipelineType = sys::_PIPELINE_TYPE::PIPELINE_TYPE_GRAPHICS as sys::PIPELINE_TYPE;
        info.SRBAllocationGranularity = 1;
        info.ImmediateContextMask = 1;
        let t0 = Instant::now();
        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        unsafe { unpack(dearchiver.0, &info, &mut pso) };
        per_pso.push(t0.elapsed().as_secs_f64() * 1000.0);
        if pso.is_null() {
            println!("[v20f] ARCHIVE LOAD: FAILED — UnpackPipelineState #{i} returned null");
            any_null = true;
            break;
        }
        unpacked.push(Raw(pso));
    }
    let unpack_total = unpack_start.elapsed();
    if any_null {
        return Ok(());
    }
    print_stats("load unpack per-PSO", &per_pso);
    println!(
        "[v20f] unpacked {n} PSOs in {total:.2}ms total; statuses: first={first}, last={last}",
        n = opts.num_psos,
        total = unpack_total.as_secs_f64() * 1000.0,
        first = pso_status_name(pso_status(unpacked[0].0, false)),
        last = pso_status_name(pso_status(unpacked[opts.num_psos - 1].0, false))
    );
    let ready = unpacked
        .iter()
        .filter(|p| {
            let s = pso_status(p.0, false);
            s == PSO_READY || s == PSO_FAILED
        })
        .count();
    println!(
        "[v20f] load status poll: {ready}/{} PSOs terminal immediately after unpack",
        opts.num_psos
    );
    drop(unpacked);
    Ok(())
}

// ---- main ----------------------------------------------------------------------

fn main() -> dil::Result<()> {
    let opts = parse_opts();
    println!(
        "[v20f] V20 full verification: {} PSOs, mode={:?}, pass={:?}, warm runs={}",
        opts.num_psos, opts.mode, opts.pass, opts.warm_runs
    );

    let ctx = setup(&opts)?;

    let mut cold_sync_total = 0.0f64;
    let mut cold_async_total = 0.0f64;

    if opts.mode == Mode::Sync || opts.mode == Mode::Both {
        let r = cold_sync(&ctx, opts.num_psos, "cold")?;
        cold_sync_total = r.total.as_secs_f64() * 1000.0;
    }
    if opts.mode == Mode::Async || opts.mode == Mode::Both {
        let r = cold_async(&ctx, opts.num_psos, "cold")?;
        cold_async_total = r.total.as_secs_f64() * 1000.0;
        let _ = r.statuses;
    }

    granularity_probe(&ctx, 3)?;
    warm_runs(&ctx, &opts)?;

    if opts.pass != Pass::None {
        serialize_archive_drill(&ctx, &opts)?;
    }

    // ---- summary ----
    println!("\n[v20f] ================= SUMMARY =================");
    println!("[v20f] adapter: {} (id {}, driver {})", ctx.adapter_name, ctx.adapter_id, ctx.driver_version);
    println!(
        "[v20f] AsyncShaderCompilation feature: {}",
        feature_state_name(ctx.async_feature)
    );
    if opts.mode == Mode::Sync || opts.mode == Mode::Both {
        println!("[v20f] SYNC  cold start ({} PSOs): {cold_sync_total:.2}ms", opts.num_psos);
    }
    if opts.mode == Mode::Async || opts.mode == Mode::Both {
        println!("[v20f] ASYNC cold start ({} PSOs): {cold_async_total:.2}ms", opts.num_psos);
    }
    if cold_sync_total > 0.0 && cold_async_total > 0.0 {
        println!(
            "[v20f] ratio sync/async total: {:.2}x",
            cold_sync_total / cold_async_total
        );
    }
    println!("[v20f] archive key: {} -> {}", archive_key(&ctx), archive_path(&ctx).display());
    println!("[v20f] ==============================================");
    Ok(())
}
