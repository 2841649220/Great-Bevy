//! V15: PRS ↔ shader reflection consistency verification (D3D12 backend).
//!
//! Tests the core design premise of the wgpu → Diligent backend replacement
//! (施工方案 §4.3.3): "the PRS descriptor is the source of truth — Diligent
//! does not infer the layout from shader reflection, and it rejects a
//! pipeline whose explicit resource signatures are inconsistent with the
//! shaders' declared resources."
//!
//! Eight creation attempts (control + 4 mismatch groups, the array group
//! in both directions plus an exact-match control):
//!
//! | # | PRS declares        | Shader declares            | Expected |
//! |---|---------------------|----------------------------|----------|
//! | 0 | X (VS, CB, size 1)  | VS uses X, PS none         | ACCEPT   |
//! | A | g_Unused (CB)       | VS/PS use nothing          | ACCEPT   |
//! | B | (nothing)           | PS uses Y (not in PRS)     | REJECT   |
//! | C0| texs[4]/texs[16]    | PS uses texs[4]/texs[16]   | ACCEPT   |
//! | C1| texs[8] (PS, SRV)   | PS uses Texture2D texs[4]  | REJECT   |
//! | C2| texs[8] (PS, SRV)   | PS uses Texture2D texs[16] | REJECT   |
//! | D | X (VS only, CB)     | VS uses X AND PS uses X    | REJECT   |
//!
//! Each attempt is isolated: engine messages are captured through
//! `IEngineFactory::SetMessageCallback` into a global buffer that is cleared
//! per attempt, so the printed error text is exactly what the engine produced
//! for that attempt. A failed `CreateGraphicsPipelineState` returns null
//! (the engine catches the validation exception, logs "Failed to create
//! Pipeline State ..." and leaves the out pointer null); one attempt can
//! never abort the process, so all four samples always run.
//!
//! Headless: no swap chain or window needed (PSO creation is device-only).
//! Build/run:
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml \
//!       --example v15_prs_reflection

use std::ffi::{CStr, CString};
use std::sync::Mutex;

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const RTV_FORMAT: sys::TEXTURE_FORMAT =
    sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB as sys::TEXTURE_FORMAT;
const DSV_UNKNOWN: sys::TEXTURE_FORMAT =
    sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;

const SHADER_TYPE_VS: sys::SHADER_TYPE =
    sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE;
const SHADER_TYPE_PS: sys::SHADER_TYPE =
    sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
const SHADER_TYPE_VS_PS: sys::SHADER_TYPE = (1 | 2) as sys::SHADER_TYPE;

const VAR_MUTABLE: sys::SHADER_RESOURCE_VARIABLE_TYPE =
    sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
        as sys::SHADER_RESOURCE_VARIABLE_TYPE;
const RES_CB: sys::SHADER_RESOURCE_TYPE =
    sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE;
const RES_TEX: sys::SHADER_RESOURCE_TYPE =
    sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV as sys::SHADER_RESOURCE_TYPE;

// ---------------------------------------------------------------------------
// Engine message capture (IEngineFactory::SetMessageCallback)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EngineMessage {
    severity: sys::DEBUG_MESSAGE_SEVERITY,
    message: String,
    function: String,
    file: String,
    line: i32,
}

static CAPTURED: Mutex<Vec<EngineMessage>> = Mutex::new(Vec::new());

/// Diligent `DebugMessageCallbackType` (C ABI, called by the engine for every
/// log message; Function/File/Line may be null for LOG_*_MESSAGE).
extern "C" fn on_debug_message(
    severity: sys::DEBUG_MESSAGE_SEVERITY,
    message: *const sys::Char,
    function: *const sys::Char,
    file: *const sys::Char,
    line: std::os::raw::c_int,
) {
    let to_string = |p: *const sys::Char| -> String {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let msg = EngineMessage {
        severity,
        message: to_string(message),
        function: to_string(function),
        file: to_string(file),
        line,
    };
    if let Ok(mut g) = CAPTURED.lock() {
        g.push(msg);
    }
}

fn clear_captured() {
    if let Ok(mut g) = CAPTURED.lock() {
        g.clear();
    }
}

fn severity_name(s: sys::DEBUG_MESSAGE_SEVERITY) -> &'static str {
    match s {
        sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_INFO => "INFO",
        sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_WARNING => "WARNING",
        sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_ERROR => "ERROR",
        sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_FATAL_ERROR => "FATAL",
    }
}

fn print_captured(tag: &str) {
    let msgs: Vec<EngineMessage> = CAPTURED.lock().map(|g| g.clone()).unwrap_or_default();
    if msgs.is_empty() {
        println!("[{tag}] (no engine messages captured)");
        return;
    }
    for m in &msgs {
        let loc = if m.function.is_empty() {
            String::new()
        } else {
            format!(" [{}:{}:{}]", m.function, m.file, m.line)
        };
        println!(
            "[{tag}] <{}> {}{}",
            severity_name(m.severity),
            m.message,
            loc
        );
    }
}

// ---------------------------------------------------------------------------
// RAII wrappers for raw Diligent interface pointers
// ---------------------------------------------------------------------------

struct RawDevice(*mut sys::IRenderDevice);
struct RawContext(*mut sys::IDeviceContext);
struct RawShader(*mut sys::IShader);
struct RawPrs(*mut sys::IPipelineResourceSignature);
struct RawPso(*mut sys::IPipelineState);

impl Drop for RawDevice {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}
impl Drop for RawContext {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}
impl Drop for RawShader {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}
impl Drop for RawPrs {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}
impl Drop for RawPso {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// Calls `IObject::Release` on any Diligent interface pointer (IObject is the
/// base of every interface; its vtable's first block holds `Release`).
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

// ---------------------------------------------------------------------------
// FFI helpers (same Vtbl-call style as examples/triangle.rs / v20_async_pso.rs)
// ---------------------------------------------------------------------------

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

fn create_prs_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    resources: &[sys::PipelineResourceDesc],
) -> dil::Result<*mut sys::IPipelineResourceSignature> {
    let name_c = CString::new(name)?;
    let mut desc: sys::PipelineResourceSignatureDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    desc.Resources = resources.as_ptr();
    desc.NumResources = resources.len() as u32;
    desc.BindingIndex = 0;
    desc.SRBAllocationGranularity = 1;

    let mut prs: *mut sys::IPipelineResourceSignature = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreatePipelineResourceSignature
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IRenderDevice::CreatePipelineResourceSignature",
            ))?
    };
    unsafe { create(device, &desc, &mut prs) };
    if prs.is_null() {
        return Err(dil::Error::CreateFailed("pipeline resource signature"));
    }
    Ok(prs)
}

/// One `PipelineResourceDesc` for a PRS (all C++ defaults except Name,
/// ShaderStages, ArraySize, ResourceType and VarType, which must be explicit
/// in the C API).
fn prs_resource(
    name: &CStr,
    stages: sys::SHADER_TYPE,
    array_size: u32,
    res_type: sys::SHADER_RESOURCE_TYPE,
) -> sys::PipelineResourceDesc {
    let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    r.Name = name.as_ptr();
    r.ShaderStages = stages;
    r.ArraySize = array_size;
    r.ResourceType = res_type;
    r.VarType = VAR_MUTABLE;
    r
}

#[allow(clippy::too_many_arguments)]
fn create_graphics_pso_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    layout: &[sys::LayoutElement],
    signatures: &[*mut sys::IPipelineResourceSignature],
) -> dil::Result<*mut sys::IPipelineState> {
    let name_c = CString::new(name)?;

    let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
    ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci._PipelineStateCreateInfo.PSODesc.SRBAllocationGranularity = 1;
    ci._PipelineStateCreateInfo.ResourceSignaturesCount = signatures.len() as u32;
    ci._PipelineStateCreateInfo.ppResourceSignatures = signatures.as_ptr().cast_mut();

    ci.GraphicsPipeline.InputLayout.LayoutElements = layout.as_ptr();
    ci.GraphicsPipeline.InputLayout.NumElements = layout.len() as u32;
    ci.GraphicsPipeline.PrimitiveTopology =
        sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
    ci.GraphicsPipeline.NumRenderTargets = 1;
    ci.GraphicsPipeline.NumViewports = 1;
    ci.GraphicsPipeline.RTVFormats[0] = RTV_FORMAT;
    ci.GraphicsPipeline.DSVFormat = DSV_UNKNOWN;
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
        v if v == P::PIPELINE_STATE_STATUS_UNINITIALIZED as sys::PIPELINE_STATE_STATUS => "UNINITIALIZED",
        v if v == P::PIPELINE_STATE_STATUS_COMPILING as sys::PIPELINE_STATE_STATUS => "COMPILING",
        v if v == P::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS => "READY",
        v if v == P::PIPELINE_STATE_STATUS_FAILED as sys::PIPELINE_STATE_STATUS => "FAILED",
        _ => "UNKNOWN",
    }
}

// ---------------------------------------------------------------------------
// Shader sources
// ---------------------------------------------------------------------------

/// Resource-free full-screen triangle VS (SV_VertexID, ATTRIB0 declared to
/// match the input layout element like examples/v20_async_pso.rs).
const VS_BASE: &str = r#"
struct VSInput { float3 pos : ATTRIB0; };
struct VSOutput { float4 pos : SV_POSITION; };
void main(in VSInput input, out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0),
                      vid == 0 ? -3.0 : (vid == 1 ? 3.0 : 3.0));
    output.pos = float4(p, 0.0, 1.0);
}
"#;

/// VS that reads constant buffer `X` (resource present in the PRS of the
/// control sample and of sample D).
const VS_USES_X: &str = r#"
struct VSInput { float3 pos : ATTRIB0; };
struct VSOutput { float4 pos : SV_POSITION; };
cbuffer X : register(b0) {
    float4 scale;
};
void main(in VSInput input, out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0),
                      vid == 0 ? -3.0 : (vid == 1 ? 3.0 : 3.0));
    output.pos = float4(p * scale.x, 0.0, 1.0);
}
"#;

/// Resource-free PS.
const PS_BASE: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
float4 main(in PSInput input) : SV_TARGET {
    return float4(1.0, 1.0, 1.0, 1.0);
}
"#;

/// PS that uses constant buffer `X` (declared in the PRS of sample D, but
/// only for the VS stage).
const PS_USES_X: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
cbuffer X : register(b0) {
    float4 tint;
};
float4 main(in PSInput input) : SV_TARGET {
    return tint;
}
"#;

/// PS that uses constant buffer `Y` (NOT declared in any PRS - sample B).
const PS_USES_Y: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
cbuffer Y : register(b0) {
    float4 tint;
};
float4 main(in PSInput input) : SV_TARGET {
    return tint;
}
"#;

/// PS that uses a 4-element `Texture2D` array `texs` (samples C1: PRS
/// declares texs[8], shader declares texs[4]).
const PS_TEXS_4: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
Texture2D texs[4] : register(t0);
float4 main(in PSInput input) : SV_TARGET {
    return texs[0].Load(uint3(0, 0, 0));
}
"#;

/// PS that uses a 16-element `Texture2D` array `texs` (samples C2: PRS
/// declares texs[8], shader declares texs[16]).
const PS_TEXS_16: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
Texture2D texs[16] : register(t0);
float4 main(in PSInput input) : SV_TARGET {
    return texs[0].Load(uint3(0, 0, 0));
}
"#;

// ---------------------------------------------------------------------------
// Sample runner
// ---------------------------------------------------------------------------

/// Runs one PSO creation attempt: prints the setup, the outcome
/// (ACCEPTED/REJECTED) and the engine messages captured for that attempt.
fn run_sample(
    device: *mut sys::IRenderDevice,
    layout: &[sys::LayoutElement],
    tag: &str,
    title: &str,
    prs_name: &str,
    resources: &[sys::PipelineResourceDesc],
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    pso_name: &str,
) {
    println!("\n[{tag}] ============================================================");
    println!("[{tag}] {title}");
    println!("[{tag}] PRS '{}': {} resource(s)", prs_name, resources.len());
    for (i, r) in resources.iter().enumerate() {
        let name = if r.Name.is_null() {
            "<null>".to_string()
        } else {
            unsafe { CStr::from_ptr(r.Name) }.to_string_lossy().into_owned()
        };
        println!(
            "[{tag}]   [{i}] name='{name}' stages=0x{:02x} array_size={} type={} var={}",
            r.ShaderStages, r.ArraySize, r.ResourceType, r.VarType
        );
    }

    // 1. Create the PRS.
    let prs = match create_prs_raw(device, prs_name, resources) {
        Ok(p) => p,
        Err(e) => {
            print_captured(tag);
            println!("[{tag}] PRS creation failed: {e} (sample aborted)");
            return;
        }
    };
    let prs = RawPrs(prs);

    // 2. Create the PSO (isolated attempt; engine messages captured per call).
    clear_captured();
    let attempt = create_graphics_pso_raw(device, pso_name, vs, ps, layout, &[prs.0]);
    match attempt {
        Ok(pso) => {
            let status = pso_status_name(pso_status(pso, false));
            drop(RawPso(pso));
            println!("[{tag}] RESULT: ACCEPTED (pipeline created, status = {status})");
            print_captured(tag);
        }
        Err(e) => {
            println!("[{tag}] RESULT: REJECTED ({e})");
            print_captured(tag);
        }
    }
}

fn main() -> dil::Result<()> {
    println!("[V15] PRS <-> shader reflection consistency verification (D3D12 backend)");
    println!("[V15] ===================================================================");

    // ---- 0. Factory + device + context ----
    let factory = dil::EngineFactoryD3D12::d3d12()?;

    // Install the engine message callback (global setting, applied through
    // the C interface IEngineFactory::SetMessageCallback; the standalone
    // SetDebugMessageCallback is a C++-mangled symbol and not linkable from
    // Rust).
    let set_cb = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactory
            .SetMessageCallback
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::SetMessageCallback"))?
    };
    unsafe { set_cb(factory.as_raw() as *mut sys::IEngineFactory, Some(on_debug_message)) };
    println!("[V15] message callback: IEngineFactory::SetMessageCallback installed");

    // ---- 1. Device + context (raw FFI with a custom EngineCI so the adapter
    //         can be overridden: DILIGENT_RS_ADAPTER=<n>, default 0) ----
    let adapter_id: u32 = std::env::var("DILIGENT_RS_ADAPTER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut engine_ci = dil::desc::engine_d3d12();
    engine_ci._EngineCreateInfo.AdapterId = adapter_id;

    let mut device_raw: *mut sys::IRenderDevice = std::ptr::null_mut();
    let mut context_raw: *mut sys::IDeviceContext = std::ptr::null_mut();
    let create_dev = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactoryD3D12
            .CreateDeviceAndContextsD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IEngineFactoryD3D12::CreateDeviceAndContextsD3D12",
            ))?
    };
    unsafe { create_dev(factory.as_raw(), &engine_ci, &mut device_raw, &mut context_raw) };
    if device_raw.is_null() {
        return Err(dil::Error::CreateFailed("render device (D3D12)"));
    }
    if context_raw.is_null() {
        unsafe { release(device_raw) };
        return Err(dil::Error::CreateFailed("immediate context (D3D12)"));
    }
    let device = RawDevice(device_raw);
    let context = RawContext(context_raw);
    println!(
        "[V15] factory/device: D3D12 created (API v{}, adapter id {adapter_id})",
        sys::DILIGENT_API_VERSION
    );

    let get_info = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .expect("IRenderDevice::GetAdapterInfo missing")
    };
    let adapter = unsafe { *get_info(device.0) };
    let adapter_name = unsafe { CStr::from_ptr(adapter.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    println!(
        "[V15] adapter: {adapter_name} (vendorId={}, deviceId={})",
        adapter.VendorId, adapter.DeviceId
    );

    // ---- 2. Shaders (compiled once, reused by all samples) ----
    let device_ptr = device.0;
    let vs_base = RawShader(create_shader_raw(device_ptr, "V15 VS_BASE", VS_BASE, SHADER_TYPE_VS)?);
    let vs_uses_x = RawShader(create_shader_raw(device_ptr, "V15 VS_USES_X", VS_USES_X, SHADER_TYPE_VS)?);
    let ps_base = RawShader(create_shader_raw(device_ptr, "V15 PS_BASE", PS_BASE, SHADER_TYPE_PS)?);
    let ps_uses_x = RawShader(create_shader_raw(device_ptr, "V15 PS_USES_X", PS_USES_X, SHADER_TYPE_PS)?);
    let ps_uses_y = RawShader(create_shader_raw(device_ptr, "V15 PS_USES_Y", PS_USES_Y, SHADER_TYPE_PS)?);
    let ps_texs_4 = RawShader(create_shader_raw(device_ptr, "V15 PS_TEXS_4", PS_TEXS_4, SHADER_TYPE_PS)?);
    let ps_texs_16 = RawShader(create_shader_raw(device_ptr, "V15 PS_TEXS_16", PS_TEXS_16, SHADER_TYPE_PS)?);
    println!("[V15] shaders: 7 HLSL shaders compiled (VS_BASE, VS_USES_X, PS_BASE, PS_USES_X, PS_USES_Y, PS_TEXS_4, PS_TEXS_16)");

    let attr = CString::new("ATTRIB")?;
    let layout = [dil::layout_element(
        &attr,
        0,
        0,
        3,
        sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE,
        false,
    )];

    let x_c = CString::new("X")?;
    let unused_c = CString::new("g_Unused")?;
    let texs_c = CString::new("texs")?;

    // ---- 3. Sample 0 (control): consistent PRS + shaders ----
    let res_x_vs = [prs_resource(&x_c, SHADER_TYPE_VS, 1, RES_CB)];
    let _ = run_sample(
        device_ptr,
        &layout,
        "CTRL",
        "Control: PRS declares X (VS, CB), VS uses X, PS uses nothing -> expect ACCEPT",
        "V15 CTRL PRS",
        &res_x_vs,
        vs_uses_x.0,
        ps_base.0,
        "V15 CTRL PSO",
    );

    // ---- 4. Sample A: PRS declares a variable the shaders never use ----
    let res_unused = [prs_resource(&unused_c, SHADER_TYPE_VS_PS, 1, RES_CB)];
    let _ = run_sample(
        device_ptr,
        &layout,
        "A",
        "A: PRS declares g_Unused (CB), shaders use NO resources -> expect ACCEPT (extra PRS var tolerated)",
        "V15 A PRS",
        &res_unused,
        vs_base.0,
        ps_base.0,
        "V15 A PSO",
    );

    // ---- 5. Sample B: shader uses a variable the PRS never declares ----
    let _ = run_sample(
        device_ptr,
        &layout,
        "B",
        "B: PRS declares NOTHING, PS uses Y -> expect REJECT",
        "V15 B PRS",
        &[],
        vs_base.0,
        ps_uses_y.0,
        "V15 B PSO",
    );

    // ---- 6. Sample C0 (control for the array group): exact array-size match
    //         (PRS texs[4] + shader texs[4], PRS texs[16] + shader texs[16]) ----
    let res_texs4 = [prs_resource(&texs_c, SHADER_TYPE_PS, 4, RES_TEX)];
    let res_texs16 = [prs_resource(&texs_c, SHADER_TYPE_PS, 16, RES_TEX)];
    let _ = run_sample(
        device_ptr,
        &layout,
        "C0a",
        "C0a: PRS declares texs[4], PS uses Texture2D texs[4] (exact match) -> expect ACCEPT",
        "V15 C0a PRS",
        &res_texs4,
        vs_base.0,
        ps_texs_4.0,
        "V15 C0a PSO",
    );
    let _ = run_sample(
        device_ptr,
        &layout,
        "C0b",
        "C0b: PRS declares texs[16], PS uses Texture2D texs[16] (exact match) -> expect ACCEPT",
        "V15 C0b PRS",
        &res_texs16,
        vs_base.0,
        ps_texs_16.0,
        "V15 C0b PSO",
    );

    // ---- 7. Sample C1: array size PRS=8 vs shader=4 (PRS larger) ----
    let res_texs8 = [prs_resource(&texs_c, SHADER_TYPE_PS, 8, RES_TEX)];
    let _ = run_sample(
        device_ptr,
        &layout,
        "C1",
        "C1: PRS declares texs[8], PS uses Texture2D texs[4] (PRS larger) -> expect REJECT",
        "V15 C1 PRS",
        &res_texs8,
        vs_base.0,
        ps_texs_4.0,
        "V15 C1 PSO",
    );

    // ---- 8. Sample C2: array size PRS=8 vs shader=16 (PRS smaller) ----
    let _ = run_sample(
        device_ptr,
        &layout,
        "C2",
        "C2: PRS declares texs[8], PS uses Texture2D texs[16] (PRS smaller) -> expect REJECT",
        "V15 C2 PRS",
        &res_texs8,
        vs_base.0,
        ps_texs_16.0,
        "V15 C2 PSO",
    );

    // ---- 9. Sample D: stage mismatch ----
    let _ = run_sample(
        device_ptr,
        &layout,
        "D",
        "D: PRS declares X with VS stage ONLY, VS uses X AND PS uses X -> expect REJECT (PS stage has no X)",
        "V15 D PRS",
        &res_x_vs,
        vs_uses_x.0,
        ps_uses_x.0,
        "V15 D PSO",
    );

    // ---- 10. Validation-only switch check (static analysis, header grep) ----
    println!("\n[V15] ============================================================");
    println!("[V15] validation-only switch: NONE");
    println!(
        "[V15]   * GraphicsTypes.h VALIDATION_FLAGS = {{NONE, CHECK_SHADER_BUFFER_SIZE (Vulkan only)}}"
    );
    println!(
        "[V15]   * D3D12_*_VALIDATION_FLAGS = {{BREAK_ON_ERROR, BREAK_ON_CORRUPTION, ENABLE_GPU_BASED_VALIDATION}}"
    );
    println!(
        "[V15]     -> all of these configure RUNTIME checks; none is a 'check layout only, do not create' switch"
    );
    println!(
        "[V15]   * the PRS <-> shader compatibility check runs unconditionally inside CreateGraphicsPipelineState"
    );
    println!(
        "[V15]     (D3D12 PipelineStateD3D12Impl::ValidateShaderResources, also in D3D11/GL/Vulkan/WebGPU backends)"
    );

    // ---- 11. Cleanup (drop order: shaders -> context -> device -> factory) ----
    drop(vs_base);
    drop(vs_uses_x);
    drop(ps_base);
    drop(ps_uses_x);
    drop(ps_uses_y);
    drop(ps_texs_4);
    drop(ps_texs_16);
    drop(context);
    drop(device);
    // factory (wrapper) dropped here

    println!("[V15] cleanup complete, exiting 0");
    Ok(())
}
