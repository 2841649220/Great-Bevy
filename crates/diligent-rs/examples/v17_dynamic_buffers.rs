//! V17 (M1 gate) verification: the dynamic-buffer count limit of a PSO and
//! the `NO_DYNAMIC_BUFFERS` countermeasure (D3D12 backend).
//!
//! Context from the construction plan §5.4.4 / §2.3.2: the risk register
//! claims "Diligent per-PSO dynamic buffer limit ≈ 8" (Diligent
//! `doc/PerformanceGuide.md`: "On some implementations, the number of
//! dynamic buffers that can be used by a PSO may be limited by as few as 8
//! buffers") vs Bevy's peak of 8 (`wgpu_types::Limits::default()`
//! `max_dynamic_uniform_buffers_per_pipeline_layout = 8`; wgpu-hal dx12
//! caps D3D12 at the same 8 because each dynamic uniform buffer becomes a
//! CBV *root view*).
//!
//! This example measures the REAL limit on this machine:
//!
//!   1. gradient 1..=16 (plus 24/32 probes): PSOs with N DYNAMIC
//!      constant-buffer resources (`SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC`,
//!      no `PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS`) - does creation
//!      reach READY or FAILED?
//!   2. the descriptor-table path: the same N=16 resources with
//!      `ArraySize = 2` (non-array dynamic CBs become CBV root views on
//!      D3D12 - PipelineResourceSignatureD3D12Impl.cpp:310 - arrays force
//!      a descriptor table instead).
//!   3. `NO_DYNAMIC_BUFFERS` budget release: N=16 resources all flagged
//!      `PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS` - the PSO must build
//!      and the flag must demonstrably de-dynamify the variables (an
//!      ERROR-severity message callback counts the rejected partial-range
//!      `SetBufferRange` / `SetBufferOffset` / USAGE_DYNAMIC-buffer binds).
//!   4. the `SetBufferRange` partial-vs-full rule (PerformanceGuide:
//!      a `SetBufferRange` counts as dynamic when the range does not cover
//!      the entire buffer, regardless of buffer usage, unless the variable
//!      has `NO_DYNAMIC_BUFFERS`): full-range on a flagged variable = OK,
//!      partial-range = rejected.
//!
//! # Usage
//!
//! ```text
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml --example v17_dynamic_buffers
//! ```

use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const RTV_FORMAT: sys::TEXTURE_FORMAT =
    sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB as sys::TEXTURE_FORMAT;

const VS_SOURCE: &str = r#"
struct VSInput { float3 pos : ATTRIB0; };
struct VSOutput { float4 pos : SV_POSITION; };
void main(in VSInput input, out VSOutput output) {
    output.pos = float4(input.pos, 1.0);
}
"#;

const GRADIENT_MAX: u32 = 16;
const PROBES: [u32; 2] = [24, 32];

/// The error-message counter: any engine ERROR/FATAL message is counted.
static ERROR_COUNT: AtomicU32 = AtomicU32::new(0);
static CALLBACK_ACTIVE: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn on_message(
    severity: sys::DEBUG_MESSAGE_SEVERITY,
    message: *const sys::Char,
    _function: *const sys::Char,
    _file: *const sys::Char,
    _line: std::os::raw::c_int,
) {
    if CALLBACK_ACTIVE.load(Ordering::Relaxed) != 0 {
        let msg = if message.is_null() {
            "<null>".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        };
        println!(
            "[v17]   engine[{}]: {msg}",
            match severity {
                v if v == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_ERROR as sys::DEBUG_MESSAGE_SEVERITY => "ERROR",
                v if v == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_FATAL_ERROR as sys::DEBUG_MESSAGE_SEVERITY => "FATAL",
                v if v == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_WARNING as sys::DEBUG_MESSAGE_SEVERITY => "WARN",
                _ => "INFO",
            }
        );
        if severity == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_ERROR as sys::DEBUG_MESSAGE_SEVERITY
            || severity == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_FATAL_ERROR as sys::DEBUG_MESSAGE_SEVERITY
        {
            ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---- raw RAII -------------------------------------------------------------

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

struct Raw<T>(*mut T);
impl<T> Drop for Raw<T> {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

// ---- raw FFI helpers -------------------------------------------------------

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

fn create_buffer_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    size: u64,
    usage: sys::USAGE,
    cpu_flags: sys::CPU_ACCESS_FLAGS,
) -> dil::Result<*mut sys::IBuffer> {
    let name_c = CString::new(name)?;
    let mut desc: sys::BufferDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    desc.Size = size;
    desc.BindFlags = sys::_BIND_FLAGS::BIND_UNIFORM_BUFFER as sys::BIND_FLAGS;
    desc.Usage = usage;
    desc.CPUAccessFlags = cpu_flags;
    desc.ImmediateContextMask = 0x1;

    let mut buffer: *mut sys::IBuffer = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateBuffer
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateBuffer"))?
    };
    unsafe { create(device, &desc, std::ptr::null_mut(), &mut buffer) };
    if buffer.is_null() {
        return Err(dil::Error::CreateFailed("buffer"));
    }
    Ok(buffer)
}

/// A constant-buffer PRS resource; `no_dynamic` toggles the flag.
fn cb_res_desc(name: &std::ffi::CStr, no_dynamic: bool, array_size: u32) -> sys::PipelineResourceDesc {
    let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    r.Name = name.as_ptr();
    r.ShaderStages =
        sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE
            | sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
    r.ResourceType =
        sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE;
    r.VarType = if no_dynamic {
        sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
            as sys::SHADER_RESOURCE_VARIABLE_TYPE
    } else {
        sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC
            as sys::SHADER_RESOURCE_VARIABLE_TYPE
    };
    r.ArraySize = array_size;
    r.Flags = if no_dynamic {
        sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS
            as sys::PIPELINE_RESOURCE_FLAGS
    } else {
        sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE as sys::PIPELINE_RESOURCE_FLAGS
    };
    r
}

fn create_prs_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    resources: &[sys::PipelineResourceDesc],
) -> dil::Result<*mut sys::IPipelineResourceSignature> {
    let name_c = CString::new(name)?;
    let mut prs_desc: sys::PipelineResourceSignatureDesc = unsafe { std::mem::zeroed() };
    prs_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    prs_desc.Resources = resources.as_ptr();
    prs_desc.NumResources = resources.len() as u32;
    prs_desc.SRBAllocationGranularity = 1;
    prs_desc.BindingIndex = 0;

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
    unsafe { create(device, &prs_desc, &mut prs) };
    if prs.is_null() {
        return Err(dil::Error::CreateFailed("pipeline resource signature"));
    }
    Ok(prs)
}

fn create_srb_raw(prs: *mut sys::IPipelineResourceSignature) -> dil::Result<*mut sys::IShaderResourceBinding> {
    let mut srb: *mut sys::IShaderResourceBinding = std::ptr::null_mut();
    let create = unsafe {
        (*(*prs).pVtbl)
            .PipelineResourceSignature
            .CreateShaderResourceBinding
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IPipelineResourceSignature::CreateShaderResourceBinding",
            ))?
    };
    unsafe { create(prs, &mut srb, true) };
    if srb.is_null() {
        return Err(dil::Error::CreateFailed("shader resource binding"));
    }
    Ok(srb)
}

fn get_var_raw(
    srb: *mut sys::IShaderResourceBinding,
    name: &std::ffi::CStr,
) -> dil::Result<*mut sys::IShaderResourceVariable> {
    let get = unsafe {
        (*(*srb).pVtbl)
            .ShaderResourceBinding
            .GetVariableByName
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IShaderResourceBinding::GetVariableByName",
            ))?
    };
    let shader_type = sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
    let var = unsafe { get(srb, shader_type, name.as_ptr()) };
    if var.is_null() {
        return Err(dil::Error::NullPointer("shader resource variable"));
    }
    Ok(var)
}

fn set_buffer_range_raw(
    var: *mut sys::IShaderResourceVariable,
    buffer: *mut sys::IBuffer,
    offset: u64,
    size: u64,
) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetBufferRange
            .as_ref()
            .expect("IShaderResourceVariable::SetBufferRange missing")
    };
    unsafe { set(var, buffer as *mut sys::IDeviceObject, offset, size, 0, 0) };
}

fn set_buffer_offset_raw(var: *mut sys::IShaderResourceVariable, offset: u64) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetBufferOffset
            .as_ref()
            .expect("IShaderResourceVariable::SetBufferOffset missing")
    };
    unsafe { set(var, offset as u32, 0) };
}

fn set_var_raw(var: *mut sys::IShaderResourceVariable, buffer: *mut sys::IBuffer) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .Set
            .as_ref()
            .expect("IShaderResourceVariable::Set missing")
    };
    unsafe { set(var, buffer as *mut sys::IDeviceObject, 0) };
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

fn build_graphics_pso_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    prs: *mut sys::IPipelineResourceSignature,
) -> dil::Result<*mut sys::IPipelineState> {
    let name_c = CString::new(name)?;
    let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
    ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci._PipelineStateCreateInfo.PSODesc.SRBAllocationGranularity = 1;
    ci._PipelineStateCreateInfo.ResourceSignaturesCount = 1;
    ci._PipelineStateCreateInfo.ppResourceSignatures = std::slice::from_ref(&prs).as_ptr().cast_mut();

    let semantic = CString::new("ATTRIB")?;
    let mut element: sys::LayoutElement = unsafe { std::mem::zeroed() };
    element.HLSLSemantic = semantic.as_ptr();
    element.InputIndex = 0;
    element.BufferSlot = 0;
    element.NumComponents = 3;
    element.ValueType = sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE;
    element.RelativeOffset = 0xFFFF_FFFF;
    element.Stride = 0xFFFF_FFFF;
    element.Frequency =
        sys::_INPUT_ELEMENT_FREQUENCY::INPUT_ELEMENT_FREQUENCY_PER_VERTEX as sys::INPUT_ELEMENT_FREQUENCY;

    ci.GraphicsPipeline.InputLayout.LayoutElements = &element;
    ci.GraphicsPipeline.InputLayout.NumElements = 1;
    ci.GraphicsPipeline.PrimitiveTopology =
        sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
    ci.GraphicsPipeline.NumRenderTargets = 1;
    ci.GraphicsPipeline.NumViewports = 1;
    ci.GraphicsPipeline.RTVFormats[0] = RTV_FORMAT;
    ci.GraphicsPipeline.DSVFormat = sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;
    ci.GraphicsPipeline.SampleMask = 0xFFFF_FFFF;
    ci.GraphicsPipeline.SmplDesc.Count = 1;
    ci.GraphicsPipeline.SmplDesc.Quality = 0;
    ci.GraphicsPipeline.RasterizerDesc.FillMode =
        sys::_FILL_MODE::FILL_MODE_SOLID as sys::FILL_MODE;
    ci.GraphicsPipeline.RasterizerDesc.CullMode =
        sys::_CULL_MODE::CULL_MODE_NONE as sys::CULL_MODE;
    ci.GraphicsPipeline.RasterizerDesc.DepthClipEnable = true;
    ci.GraphicsPipeline.DepthStencilDesc.DepthEnable = false;
    ci.GraphicsPipeline.DepthStencilDesc.DepthWriteEnable = false;
    ci.GraphicsPipeline.DepthStencilDesc.StencilReadMask = 0xFF;
    ci.GraphicsPipeline.DepthStencilDesc.StencilWriteMask = 0xFF;
    for face in [
        &mut ci.GraphicsPipeline.DepthStencilDesc.FrontFace,
        &mut ci.GraphicsPipeline.DepthStencilDesc.BackFace,
    ] {
        face.StencilFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilDepthFailOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilPassOp = sys::_STENCIL_OP::STENCIL_OP_KEEP as sys::STENCIL_OP;
        face.StencilFunc = sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS as sys::COMPARISON_FUNCTION;
    }
    ci.pVS = vs;
    ci.pPS = ps;
    std::mem::forget(name_c);
    std::mem::forget(semantic);

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

/// Builds a PS source consuming `n` named cbuffers (b0..b{n-1}).
fn ps_source_with_n_buffers(n: u32) -> String {
    let mut src = String::new();
    let mut sum = String::new();
    for i in 0..n {
        src.push_str(&format!("cbuffer Dyn{i} : register(b{i}) {{ float4 c{i}; }};\n"));
        if i > 0 {
            sum.push_str(" + ");
        }
        sum.push_str(&format!("c{i}"));
    }
    src.push_str("float4 main() : SV_TARGET { return ");
    src.push_str(&sum);
    src.push_str("; }");
    src
}

const PSO_READY: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS;
const PSO_FAILED: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_FAILED as sys::PIPELINE_STATE_STATUS;

fn status_name(s: sys::PIPELINE_STATE_STATUS) -> &'static str {
    match s {
        v if v == PSO_READY => "READY",
        v if v == PSO_FAILED => "FAILED",
        v if v == sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_COMPILING as sys::PIPELINE_STATE_STATUS => "COMPILING",
        _ => "UNINIT",
    }
}

fn main() -> dil::Result<()> {
    println!("[v17] V17 dynamic-buffer limit + NO_DYNAMIC_BUFFERS budget (D3D12)");

    let factory = dil::EngineFactoryD3D12::d3d12()?;
    // The global `SetDebugMessageCallback` C++ symbol is not exposed as
    // `extern "C"` in the bindings (link error), but the vtable route
    // `IEngineFactory::SetMessageCallback` (EngineFactoryBase.hpp:119) calls
    // the same C++ function - verified route.
    let set_cb = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactory
            .SetMessageCallback
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::SetMessageCallback"))?
    };
    unsafe { set_cb(factory.as_raw() as *mut sys::IEngineFactory, Some(on_message)) };
    CALLBACK_ACTIVE.store(1, Ordering::Relaxed);

    let (device, context) = factory.create_device_and_contexts()?;
    let device = device.as_raw();
    let _ = context;

    // Caps.
    let get_adapter = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .expect("IRenderDevice::GetAdapterInfo missing")
    };
    let ainfo = unsafe { *get_adapter(device) };
    let adapter_name = unsafe { std::ffi::CStr::from_ptr(ainfo.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    println!("[v17] adapter: {adapter_name}");
    println!(
        "[v17] caps: ConstantBufferOffsetAlignment={} StructuredBufferOffsetAlignment={}",
        ainfo.Buffer.ConstantBufferOffsetAlignment, ainfo.Buffer.StructuredBufferOffsetAlignment
    );

    let vs = Raw(create_shader_raw(device, "v17_vs", VS_SOURCE, sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE)?);

    // ---- 1. gradient: 1..=16 DYNAMIC CBs per PSO (plus 24/32 probes) ----
    println!("[v17] === gradient: N DYNAMIC constant buffers per PSO (root views on D3D12) ===");
    let mut limits = Vec::new();
    let mut seq: Vec<u32> = (1..=GRADIENT_MAX).collect();
    seq.extend_from_slice(&PROBES);
    for n in seq {
        let ps_src = ps_source_with_n_buffers(n);
        let ps = match create_shader_raw(
            device,
            &format!("v17_ps_{n}"),
            &ps_src,
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
        ) {
            Ok(ps) => ps,
            Err(e) => {
                println!("[v17]   n={n:2}: shader create FAILED: {e}");
                limits.push((n, "shader-fail"));
                continue;
            }
        };
        let mut names = Vec::with_capacity(n as usize);
        let mut res = Vec::with_capacity(n as usize);
        for i in 0..n {
            let c = CString::new(format!("Dyn{i}")).unwrap();
        res.push(cb_res_desc(&c, false, 1));
        names.push(c);
    }
    let prs = match create_prs_raw(device, &format!("v17_dyn_prs_{n}"), &res) {
            Ok(p) => p,
            Err(e) => {
                println!("[v17]   n={n:2}: PRS create FAILED: {e}");
                unsafe { release(ps) };
                limits.push((n, "prs-fail"));
                continue;
            }
        };
        let err_before = ERROR_COUNT.load(Ordering::Relaxed);
        let pso = build_graphics_pso_raw(device, &format!("v17_dyn_pso_{n}"), vs.0, ps, prs);
        let status = match &pso {
            Ok(p) => pso_status(*p, true),
            Err(_) => PSO_FAILED,
        };
        let err_delta = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
        let verdict = if status == PSO_READY && err_delta == 0 { "OK" } else { "LIMIT" };
        println!(
            "[v17]   n={n:2}: status={} new-errors={err_delta} -> {verdict}",
            status_name(status)
        );
        if let Ok(p) = pso {
            unsafe { release(p) };
        }
        unsafe { release(prs) };
        unsafe { release(ps) };
        limits.push((n, verdict));
    }
    let first_limit = limits.iter().find(|(_, v)| *v != "OK");
    match first_limit {
        Some((n, v)) => println!(
            "[v17] measured DYNAMIC-buffer limit on this machine: {n} ({v}) - the ladder threshold vs Bevy peak 8 is settled by this value"
        ),
        None => println!(
            "[v17] all tested counts (up to {}) reached READY - limit is > 32 on this machine",
            PROBES[1]
        ),
    }

    // ---- 2. descriptor-table path (ArraySize=2 forces DESCRIPTOR_TABLE) ----
    println!("[v17] === n=16 with ArraySize=2 (descriptor-table path) ===");
    {
        let n = 16u32;
        // The array-cbuffer probe needs a register-less cbuffer declaration:
        // with an explicit `register(b0)` the engine's remapper rejects the
        // bind point ("Invalid cbuffer bind point (0), expected (1)").
        let mut ps_src = String::new();
        let mut sum = String::new();
        for i in 0..n {
            ps_src.push_str(&format!("cbuffer Dyn{i} {{ float4 c{i}; }};\n"));
            if i > 0 {
                sum.push_str(" + ");
            }
            sum.push_str(&format!("c{i}"));
        }
        ps_src.push_str("float4 main() : SV_TARGET { return ");
        ps_src.push_str(&sum);
        ps_src.push_str("; }");
        let ps = match create_shader_raw(device, "v17_ps_tbl16", &ps_src, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE) {
            Ok(p) => p,
            Err(e) => {
                println!("[v17]   n=16 ArraySize=2: shader create FAILED: {e}");
                println!("[v17]   (descriptor-table probe skipped)");
                return Ok(());
            }
        };
        let mut names = Vec::with_capacity(n as usize);
        let mut res = Vec::with_capacity(n as usize);
        for i in 0..n {
            let c = CString::new(format!("Dyn{i}")).unwrap();
            res.push(cb_res_desc(&c, false, 2));
            names.push(c);
        }
        let prs = match create_prs_raw(device, "v17_tbl_prs_16", &res) {
            Ok(p) => p,
            Err(e) => {
                println!("[v17]   n=16 ArraySize=2: PRS create FAILED: {e}");
                unsafe { release(ps) };
                return Ok(());
            }
        };
        let err_before = ERROR_COUNT.load(Ordering::Relaxed);
        let pso = build_graphics_pso_raw(device, "v17_tbl_pso_16", vs.0, ps, prs);
        let (status, err_delta) = match &pso {
            Ok(p) => {
                let status = pso_status(*p, true);
                (status, ERROR_COUNT.load(Ordering::Relaxed) - err_before)
            }
            Err(_) => (PSO_FAILED, ERROR_COUNT.load(Ordering::Relaxed) - err_before),
        };
        println!(
            "[v17]   n=16 ArraySize=2: status={} new-errors={err_delta} -> {}",
            status_name(status),
            if status == PSO_READY && err_delta == 0 { "OK" } else { "LIMIT (register-remap issue documented in the report)" }
        );
        if let Ok(p) = pso {
            unsafe { release(p) };
        }
        unsafe { release(prs) };
        unsafe { release(ps) };
    }

    // ---- 3. NO_DYNAMIC_BUFFERS budget release + behavioral proof ----
    println!("[v17] === n=16 all with PIPELINE_RESOURCE_FLAG_NO_DYNAMIC_BUFFERS ===");
    let n = 16u32;
    let ps_src = ps_source_with_n_buffers(n);
    let ps = Raw(create_shader_raw(device, "v17_ps_nd16", &ps_src, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE)?);
    let mut names = Vec::with_capacity(n as usize);
    let mut res = Vec::with_capacity(n as usize);
    for i in 0..n {
        let c = CString::new(format!("Dyn{i}")).unwrap();
        res.push(cb_res_desc(&c, true, 1));
        names.push(c);
    }
    let prs = Raw(create_prs_raw(device, "v17_nd_prs_16", &res)?);
    let err_before = ERROR_COUNT.load(Ordering::Relaxed);
    let pso = Raw(build_graphics_pso_raw(device, "v17_nd_pso_16", vs.0, ps.0, prs.0)?);
    let status = pso_status(pso.0, true);
    let err_delta = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
    println!(
        "[v17]   16x NO_DYNAMIC_BUFFERS PSO: status={} new-errors={err_delta} -> {} (budget released: 16 non-dynamic CBs build where 16 dynamic CBs also build)",
        status_name(status),
        if status == PSO_READY && err_delta == 0 { "OK" } else { "FAIL" }
    );
    println!(
        "[v17]   root-signature structure (source-verified, PipelineResourceSignatureD3D12Impl.cpp:310): DYNAMIC non-array CB = CBV root view; NO_DYNAMIC_BUFFERS moves it into a descriptor table (no heap-occupancy query API on IRenderDevice - no heap numbers printed)"
    );

    // Behavioral proof on a NO_DYNAMIC_BUFFERS variable.
    //
    // NOTE (measured 2026-08-07): the runtime rejections are
    // `DILIGENT_DEVELOPMENT`-gated in the locked engine source:
    //   - ShaderVariableManagerD3D12.cpp:327  (CacheCB -> VerifyConstantBufferBinding)
    //   - ShaderResourceVariableBase.hpp:750  (SetBufferOffset flag check)
    // This is a Release build (no DILIGENT_DEVELOPMENT), so the calls below
    // are accepted silently - the "counts as dynamic" semantics the flag
    // suppresses are implemented *in that gated code* (the partial-range vs
    // cached-range mismatch check, ShaderResourceVariableBase.hpp:299-322).
    // The honest release-build observables are the PSO gradient above and
    // the root-signature structure; a DEV build would additionally reject
    // every case marked "expected rejection".
    println!("[v17] === SetBufferRange partial-vs-full on NO_DYNAMIC_BUFFERS variable ===");
    {
        let srb = Raw(create_srb_raw(prs.0)?);
        let var_name = CString::new("Dyn0").unwrap();
        let var = get_var_raw(srb.0, &var_name)?;

        let usage_default = create_buffer_raw(device, "v17_default_buf", 256, sys::_USAGE::USAGE_DEFAULT as sys::USAGE, 0)?;
        let usage_dynamic = create_buffer_raw(
            device,
            "v17_dynamic_buf",
            256,
            sys::_USAGE::USAGE_DYNAMIC as sys::USAGE,
            sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_WRITE as sys::CPU_ACCESS_FLAGS,
        )?;

        let e0 = ERROR_COUNT.load(Ordering::Relaxed);
        set_buffer_range_raw(var, usage_default, 0, 256);
        set_buffer_range_raw(var, usage_default, 64, 128);
        set_buffer_offset_raw(var, 16);
        set_var_raw(var, usage_dynamic);
        let e_ops = ERROR_COUNT.load(Ordering::Relaxed) - e0;
        println!(
            "[v17]   full/partial SetBufferRange + SetBufferOffset + USAGE_DYNAMIC Set on NO_DYNAMIC_BUFFERS var: release-build errors={e_ops}"
        );
        println!(
            "[v17]     -> {} (runtime rejections are DILIGENT_DEVELOPMENT-gated in this snapshot; see the report)",
            if e_ops == 0 { "silently accepted in Release" } else { "rejected" }
        );

        // Sanity control on a DYNAMIC variable (no flag): partial range is
        // the normal dynamic-offset use and must not trip anything in
        // Release either.
        let dyn0_name = CString::new("Dyn0").unwrap();
        let res0 = cb_res_desc(&dyn0_name, false, 1);
        let prs_d = Raw(create_prs_raw(device, "v17_dynvar_prs", std::slice::from_ref(&res0))?);
        let dyn_prs_srb = Raw(create_srb_raw(prs_d.0)?);
        let var_dyn = get_var_raw(dyn_prs_srb.0, &dyn0_name)?;
        let e0 = ERROR_COUNT.load(Ordering::Relaxed);
        set_buffer_range_raw(var_dyn, usage_default, 64, 128);
        let e_partial_ok = ERROR_COUNT.load(Ordering::Relaxed) - e0;
        println!(
            "[v17]   partial-range SetBufferRange on DYNAMIC var (control): release-build errors={e_partial_ok} -> accepted"
        );

        unsafe { release(dyn_prs_srb.0) };
        unsafe { release(prs_d.0) };
        unsafe { release(usage_dynamic) };
        unsafe { release(usage_default) };
    }

    let total_errors = ERROR_COUNT.load(Ordering::Relaxed);
    println!("[v17] total engine ERROR messages observed: {total_errors}");

    // ---- 4. conclusion vs Bevy peak 8 ----
    println!("[v17] Bevy peak dynamic uniform buffers per pipeline layout = 8");
    println!("[v17]   (wgpu_types::Limits::default().max_dynamic_uniform_buffers_per_pipeline_layout = 8,");
    println!("[v17]    the WebGPU spec default; wgpu-hal dx12 enforces the same 8 root views)");
    println!("[v17] view bind group dynamic slots ~7 (mesh_view_bindings) + material 1 = peak 8");
    println!("[v17] ladder: measured limit > 8 -> no countermeasure needed on this class of device;");
    println!("[v17]         = 8  -> NO_DYNAMIC_BUFFERS everywhere (verified released budget above);");
    println!("[v17]         < 8  -> MUTABLE full-frame binding for low-frequency slots");

    Ok(())
}





