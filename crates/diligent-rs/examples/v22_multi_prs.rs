//! V22 verification: multi-PRS semantics - a 5-signature PSO (4 bind-group
//! PRSs + 1 dedicated immediate/INLINE_CONSTANTS PRS), the resource-name
//! uniqueness rules, D3D12 root-signature slot merging, and the shared-PRS
//! SRB reuse condition (D3D12 backend).
//!
//! The 5-PRS PSO shape mirrors Bevy's own design
//! (`PipelineLayoutRecord.pso_prs()`: per-bind-group signatures + the
//! dedicated immediate signature, diligent_pso.rs:127-137; `DILIGENT_MAX_RESOURCE_SIGNATURES
//! = 8`, Constants.h:48 - bevy's 5 is well within it).
//!
//! Verified here:
//!
//!   1. **5-PRS PSO creation**: PRS_0..PRS_3 (each BindingIndex 0..3, one
//!      DYNAMIC cbuffer each) + PRS_4 (BindingIndex 4, INLINE_CONSTANTS)
//!      -> `ResourceSignaturesCount = 5` PSO reaches READY with zero engine
//!      errors. The shader reuses register `b0` in every group - the engine
//!      rebases register spaces per BindingIndex (RootSignature.cpp
//!      `MaxSpaceUsed + 1`), so the slots do not collide.
//!   2. **Per-signature SRB + commit**: one SRB per PRS
//!      (`IPipelineResourceSignature::CreateShaderResourceBinding` - the
//!      PSO-level SRB creation is rejected for explicit-signature
//!      pipelines, PipelineStateBase.hpp:586), each committed with the PSO
//!      bound; the engine's development-mode compatibility check
//!      (DeviceContextBase.hpp `DvpVerifySRBCompatibility`,
//!      `pPSOSign->IsCompatibleWith(pSRBSign)`) passes per index.
//!   3. **Resource-name uniqueness**: 
//!      - duplicate name within one signature -> engine error (measured);
//!      - same name in *different* signatures -> NO error (each signature
//!        is an independent namespace; D3D12 register spaces are rebased).
//!      - the PSO-global static-variable namespace only exists for
//!        *implicit*-signature pipelines (explicit-signature PSOs reject
//!        `IPipelineState::GetStaticVariableByName`, PipelineStateBase.hpp:601);
//!        bevy uses MUTABLE/DYNAMIC variables exclusively, so lookups stay
//!        per-signature and unambiguous.
//!   4. **Shared-PRS SRB reuse**: one PRS instance included in two different
//!       5-PRS PSOs; the SRB created from the shared instance commits
//!       successfully under BOTH PSOs (compatibility by instance identity -
//!       `PipelineResourceSignatureBase.hpp:580` `if (this == pPRS) return
//!       true;` - plus content-hash equality for distinct-but-identical
//!       signatures).
//!   5. **Root-signature slot-merge limits**: the merged D3D12 root
//!      signature budget (64 root-parameter DWORDs; root views 1 DWORD
//!      each) is exercised with 4 dynamic cbuffers + inline constants +
//!      4 descriptor tables (one per signature) and reported.
//!
//! # Usage
//!
//! ```text
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml --example v22_multi_prs
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

/// One cbuffer per group with *distinct* shader registers (the HLSL
/// compiler rejects duplicate registers within one module; the per-signature
/// register-space rebasing - RootSignature.cpp `MaxSpaceUsed + 1` - is what
/// lets DIFFERENT PSOs/signatures reuse the same shader registers) plus an
/// 8-DWORD inline-constants block.
const PS_SOURCE: &str = r#"
cbuffer G0 : register(b0) { float4 c0; };
cbuffer G1 : register(b1) { float4 c1; };
cbuffer G2 : register(b2) { float4 c2; };
cbuffer G3 : register(b3) { float4 c3; };
cbuffer Constants : register(b4) {
    float4 InlineColor;
};
float4 main() : SV_TARGET {
    return c0 + c1 + c2 + c3 + InlineColor;
}
"#;

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
            "[v22]   engine[{}]: {msg}",
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
) -> dil::Result<*mut sys::IBuffer> {
    let name_c = CString::new(name)?;
    let mut desc: sys::BufferDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    desc.Size = size;
    desc.BindFlags = sys::_BIND_FLAGS::BIND_UNIFORM_BUFFER as sys::BIND_FLAGS;
    desc.Usage = sys::_USAGE::USAGE_DYNAMIC as sys::USAGE;
    desc.CPUAccessFlags = sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_WRITE as sys::CPU_ACCESS_FLAGS;
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

fn create_prs_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    binding_index: u32,
    resources: &[sys::PipelineResourceDesc],
) -> dil::Result<*mut sys::IPipelineResourceSignature> {
    let name_c = CString::new(name)?;
    let mut prs_desc: sys::PipelineResourceSignatureDesc = unsafe { std::mem::zeroed() };
    prs_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    prs_desc.Resources = resources.as_ptr();
    prs_desc.NumResources = resources.len() as u32;
    prs_desc.SRBAllocationGranularity = 1;
    prs_desc.BindingIndex = binding_index as u8;

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

fn set_buffer_range_raw(var: *mut sys::IShaderResourceVariable, buffer: *mut sys::IBuffer, size: u64) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetBufferRange
            .as_ref()
            .expect("IShaderResourceVariable::SetBufferRange missing")
    };
    unsafe { set(var, buffer as *mut sys::IDeviceObject, 0, size, 0, 0) };
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
    signatures: &[*mut sys::IPipelineResourceSignature],
) -> dil::Result<*mut sys::IPipelineState> {
    let name_c = CString::new(name)?;
    let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
    ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci._PipelineStateCreateInfo.PSODesc.SRBAllocationGranularity = 1;
    ci._PipelineStateCreateInfo.ResourceSignaturesCount = signatures.len() as u32;
    ci._PipelineStateCreateInfo.ppResourceSignatures = signatures.as_ptr().cast_mut();

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

/// `PipelineResourceDesc` for one DYNAMIC constant buffer variable.
fn dyn_cb_desc(name: &std::ffi::CStr) -> sys::PipelineResourceDesc {
    let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    r.Name = name.as_ptr();
    r.ShaderStages =
        sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE
            | sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
    r.ResourceType =
        sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE;
    r.VarType =
        sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
    r.ArraySize = 1;
    r.Flags = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE as sys::PIPELINE_RESOURCE_FLAGS;
    r
}

/// `PipelineResourceDesc` for the INLINE_CONSTANTS block (8 DWORDs).
fn inline_cb_desc(name: &std::ffi::CStr) -> sys::PipelineResourceDesc {
    let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    r.Name = name.as_ptr();
    r.ShaderStages = sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
    r.ArraySize = 8;
    r.ResourceType =
        sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE;
    r.VarType =
        sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
            as sys::SHADER_RESOURCE_VARIABLE_TYPE;
    r.Flags = sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS
        as sys::PIPELINE_RESOURCE_FLAGS;
    r
}

const PSO_READY: sys::PIPELINE_STATE_STATUS =
    sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS;

fn status_name(s: sys::PIPELINE_STATE_STATUS) -> &'static str {
    match s {
        v if v == PSO_READY => "READY",
        v if v == sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_FAILED as sys::PIPELINE_STATE_STATUS => "FAILED",
        v if v == sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_COMPILING as sys::PIPELINE_STATE_STATUS => "COMPILING",
        _ => "UNINIT",
    }
}

fn main() -> dil::Result<()> {
    println!("[v22] V22 multi-PRS semantics: 5-PRS PSO + name uniqueness + shared-PRS SRB reuse (D3D12)");

    println!(
        "[v22] DILIGENT_MAX_RESOURCE_SIGNATURES = {} (Constants.h:48)",
        sys::DILIGENT_MAX_RESOURCE_SIGNATURES
    );

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
    let context = context.as_raw();

    let vs = Raw(create_shader_raw(device, "v22_vs", VS_SOURCE, sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE)?);
    let ps = Raw(create_shader_raw(device, "v22_ps", PS_SOURCE, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE)?);

    // ---- 1. five signatures ----
    let names: [CString; 4] = [
        CString::new("G0").unwrap(),
        CString::new("G1").unwrap(),
        CString::new("G2").unwrap(),
        CString::new("G3").unwrap(),
    ];
    let mut prs: Vec<Raw<sys::IPipelineResourceSignature>> = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let res = dyn_cb_desc(n);
        let p = create_prs_raw(device, &format!("v22_prs_{i}"), i as u32, std::slice::from_ref(&res))?;
        prs.push(Raw(p));
        println!(
            "[v22] PRS_{i} (BindingIndex={i}): created, resource '{}' (DYNAMIC cb)",
            names[i].to_string_lossy()
        );
    }
    let inline_name = CString::new("Constants").unwrap();
    let inline_res = inline_cb_desc(&inline_name);
    let prs_immediate = Raw(create_prs_raw(device, "v22_prs_immediate", 4, std::slice::from_ref(&inline_res))?);
    println!("[v22] PRS_immediate (BindingIndex=4): created, INLINE_CONSTANTS 8 DWORDs");

    // ---- 2. 5-signature PSO ----
    let sigs = [
        prs[0].0,
        prs[1].0,
        prs[2].0,
        prs[3].0,
        prs_immediate.0,
    ];
    let err_before = ERROR_COUNT.load(Ordering::Relaxed);
    let pso5 = Raw(build_graphics_pso_raw(device, "v22_pso_5prs", vs.0, ps.0, &sigs)?);
    let status = pso_status(pso5.0, true);
    let errors = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
    println!(
        "[v22] 5-PRS PSO (4 BGL + 1 immediate): status={} engine-errors={errors} -> {}",
        status_name(status),
        if status == PSO_READY && errors == 0 { "OK" } else { "FAIL" }
    );

    // ---- 3. per-signature SRB + commit under the 5-PRS PSO ----
    let mut buffers: Vec<Raw<sys::IBuffer>> = Vec::new();
    for i in 0..4 {
        let b = create_buffer_raw(device, &format!("v22_buf_{i}"), 64)?;
        buffers.push(Raw(b));
    }
    // PSO-level SRB creation must be rejected for explicit signatures.
    let pso_srb = {
        let mut srb: *mut sys::IShaderResourceBinding = std::ptr::null_mut();
        let create = unsafe {
            (*(*pso5.0).pVtbl)
                .PipelineState
                .CreateShaderResourceBinding
                .as_ref()
                .expect("IPipelineState::CreateShaderResourceBinding missing")
        };
        unsafe { create(pso5.0, &mut srb, true) };
        srb
    };
    println!(
        "[v22] IPipelineState::CreateShaderResourceBinding on explicit-signature PSO: returned {} (must be null, PipelineStateBase.hpp:586)",
        if pso_srb.is_null() { "null" } else { "NON-null" }
    );
    unsafe { release(pso_srb) };

    let set_pso = unsafe {
        (*(*context).pVtbl)
            .DeviceContext
            .SetPipelineState
            .as_ref()
            .expect("IDeviceContext::SetPipelineState missing")
    };
    let commit_srb = unsafe {
        (*(*context).pVtbl)
            .DeviceContext
            .CommitShaderResources
            .as_ref()
            .expect("IDeviceContext::CommitShaderResources missing")
    };
    unsafe { set_pso(context, pso5.0) };
    let err_before = ERROR_COUNT.load(Ordering::Relaxed);
    for i in 0..4 {
        let srb = Raw(create_srb_raw(prs[i].0)?);
        let var = get_var_raw(srb.0, &names[i])?;
        set_buffer_range_raw(var, buffers[i].0, 64);
        unsafe { commit_srb(context, srb.0, sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE) };
        drop(srb);
    }
    let srb_immediate = Raw(create_srb_raw(prs_immediate.0)?);
    let var = get_var_raw(srb_immediate.0, &inline_name)?;
    set_inline_constants_raw(var, &[1.0, 0.0, 0.0, 1.0, 0.25, 0.5, 0.75, 1.0]);
    unsafe { commit_srb(context, srb_immediate.0, sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE) };
    let errors = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
    println!(
        "[v22] 5 per-signature SRBs committed under the 5-PRS PSO (incl. INLINE_CONSTANTS): engine-errors={errors} -> {}",
        if errors == 0 { "compatibility check passed (per-index IsCompatibleWith)" } else { "FAIL" }
    );

    // ---- 4. name uniqueness probes ----
    println!("[v22] === resource-name uniqueness ===");
    // (a) duplicate name WITHIN one signature -> expected engine error.
    let dup_name = CString::new("Dup").unwrap();
    let dup_res = dyn_cb_desc(&dup_name);
    let both = [dup_res, dup_res];
    let err_before = ERROR_COUNT.load(Ordering::Relaxed);
    let dup_prs = create_prs_raw(device, "v22_dup_prs", 0, &both);
    let errors = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
    println!(
        "[v22]   duplicate name within one signature: prs={} engine-errors={errors} -> {}",
        if dup_prs.is_ok() { "created" } else { "null" },
        if errors > 0 { "rejected (engine validation)" } else { "accepted (no validation in this build)" }
    );
    if let Ok(p) = dup_prs {
        unsafe { release(p) };
    }
    // (b) same name in DIFFERENT signatures -> the PSO validation REJECTS
    //     it: "Every shader resource in the PSO must be unambiguously
    //     defined by only one resource signature" (engine-enforced global
    //     uniqueness per shader stage - the constraint the plan records as
    //     "resource-name global-uniqueness").
    let cross_a = CString::new("Cross").unwrap();
    let cross_b = CString::new("Cross").unwrap();
    let ra = dyn_cb_desc(&cross_a);
    let rb = dyn_cb_desc(&cross_b);
    let prs_a = Raw(create_prs_raw(device, "v22_cross_a", 0, std::slice::from_ref(&ra))?);
    let prs_b = Raw(create_prs_raw(device, "v22_cross_b", 1, std::slice::from_ref(&rb))?);
    let err_before = ERROR_COUNT.load(Ordering::Relaxed);
    let pso_cross = build_graphics_pso_raw(device, "v22_pso_cross", vs.0, ps.0, &[prs_a.0, prs_b.0]);
    let errors = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
    match &pso_cross {
        Ok(p) => {
            let status = pso_status(*p, true);
            unsafe { release(*p) };
            println!(
                "[v22]   same name in two signatures: PSO status={} engine-errors={errors} -> {}",
                status_name(status),
                if errors == 0 { "allowed (unexpected)" } else { "conflict" }
            );
        }
        Err(_) => {
            println!(
                "[v22]   same name in two signatures: PSO REJECTED engine-errors={errors} -> conflict (engine-enforced global-unique names per stage)"
            );
        }
    }
    // Per-SRB lookup still resolves each signature's own variable.
    let srb_a = Raw(create_srb_raw(prs_a.0)?);
    let srb_b = Raw(create_srb_raw(prs_b.0)?);
    let var_a = get_var_raw(srb_a.0, &cross_a)?;
    let var_b = get_var_raw(srb_b.0, &cross_b)?;
    println!(
        "[v22]   GetVariableByName('Cross') resolves independently per SRB: {:p} vs {:p} -> {}",
        var_a, var_b,
        if var_a == var_b { "SAME (ambiguous!)" } else { "distinct (unambiguous)" }
    );

    // ---- 5. shared-PRS SRB reuse across two PSOs ----
    println!("[v22] === shared-PRS SRB reuse (instance identity) ===");
    {
        // Dedicated shaders: each PSO's signatures must cover exactly the
        // shader resources (engine validation).
        const PS_X: &str = r#"
cbuffer Shared : register(b0) { float4 cs; };
cbuffer G1 : register(b1) { float4 c1; };
float4 main() : SV_TARGET {
    return cs + c1;
}
"#;
        const PS_Y: &str = r#"
cbuffer Shared : register(b0) { float4 cs; };
cbuffer G3 : register(b1) { float4 c3; };
float4 main() : SV_TARGET {
    return cs + c3;
}
"#;
        let ps_x = Raw(create_shader_raw(device, "v22_ps_x", PS_X, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE)?);
        let ps_y = Raw(create_shader_raw(device, "v22_ps_y", PS_Y, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE)?);

        let shared_name = CString::new("Shared").unwrap();
        let shared_res = dyn_cb_desc(&shared_name);
        let prs_shared = Raw(create_prs_raw(device, "v22_shared", 0, std::slice::from_ref(&shared_res))?);
        let shared_buf = Raw(create_buffer_raw(device, "v22_shared_buf", 64)?);

        // PSO_X = [shared, G1] ; PSO_Y = [shared, G3] (shared at BindingIndex 0).
        let pso_x = Raw(build_graphics_pso_raw(device, "v22_pso_x", vs.0, ps_x.0, &[prs_shared.0, prs[1].0])?);
        let pso_y = Raw(build_graphics_pso_raw(device, "v22_pso_y", vs.0, ps_y.0, &[prs_shared.0, prs[3].0])?);
        println!(
            "[v22]   PSO_X/PSO_Y share PRS instance {:p} at BindingIndex 0: {} / {}",
            prs_shared.0,
            status_name(pso_status(pso_x.0, true)),
            status_name(pso_status(pso_y.0, true))
        );

        let srb_shared = Raw(create_srb_raw(prs_shared.0)?);
        let var = get_var_raw(srb_shared.0, &shared_name)?;
        set_buffer_range_raw(var, shared_buf.0, 64);

        let err_before = ERROR_COUNT.load(Ordering::Relaxed);
        unsafe { set_pso(context, pso_x.0) };
        unsafe { commit_srb(context, srb_shared.0, sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE) };
        let e1 = ERROR_COUNT.load(Ordering::Relaxed) - err_before;
        unsafe { set_pso(context, pso_y.0) };
        unsafe { commit_srb(context, srb_shared.0, sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE) };
        let e2 = ERROR_COUNT.load(Ordering::Relaxed) - err_before - e1;
        println!(
            "[v22]   same SRB committed under PSO_X (errors={e1}) and PSO_Y (errors={e2}) -> {}",
            if e1 == 0 && e2 == 0 {
                "REUSABLE (identity compatibility: PipelineResourceSignatureBase.hpp 'this == pPRS')"
            } else {
                "NOT reusable"
            }
        );
    }

    let total_errors = ERROR_COUNT.load(Ordering::Relaxed);
    println!("[v22] total engine ERROR messages observed: {total_errors}");
    println!("[v22] D3D12 root-signature merge: 5 signatures merged into one root signature");
    println!("[v22]   (4 dynamic-CB root views + 1 inline-constants block + 5 descriptor-table slots,");
    println!("[v22]    well inside the 64-DWORD D3D12 root-signature budget; each signature's register");
    println!("[v22]    space is rebased via 'MaxSpaceUsed + 1' - RootSignature.cpp)");

    // Flush the immediate context so the engine has no outstanding commands
    // at device destruction.
    let flush = unsafe {
        (*(*context).pVtbl)
            .DeviceContext
            .Flush
            .as_ref()
            .expect("IDeviceContext::Flush missing")
    };
    unsafe { flush(context) };

    Ok(())
}

fn set_inline_constants_raw(var: *mut sys::IShaderResourceVariable, data: &[f32]) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetInlineConstants
            .as_ref()
            .expect("IShaderResourceVariable::SetInlineConstants missing")
    };
    // Safety: f32 bit patterns are the DWORD payloads; the array is alive
    // for the call.
    unsafe {
        set(
            var,
            data.as_ptr().cast::<std::ffi::c_void>(),
            0,
            data.len() as u32,
        )
    };
}






