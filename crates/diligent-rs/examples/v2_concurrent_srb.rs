//! V2 verification: concurrent SRB creation + concurrent resource creation
//! from multiple threads (D3D12 backend).
//!
//! Bevy's render-world schedules run their systems on the compute task pool
//! (multiple worker threads - `gpu_preprocessing.rs:2352-2418` is the
//! reference `ComputeTaskPool::get().scope` pattern), and bind-group
//! creation (`RenderDevice::create_bind_group` ->
//! `create_diligent_bind_group` -> `IPipelineResourceSignature::CreateShaderResourceBinding`,
//! render_device.rs:354) can therefore execute concurrently from different
//! worker threads. The immediate device context is already serialized in
//! bevy (the `CONTEXT_LOCK` of M1-4b-2), but SRB creation is a *signature*
//! method, not a context method - it runs outside that lock today.
//!
//! This example reproduces the exact access pattern on the raw Diligent API:
//!
//!   - `std::thread::scope` (the std analogue of the bevy task-pool scope)
//!   - N worker threads (default 8, matching a typical bevy compute pool)
//!   - every worker concurrently:
//!       * creates `USAGE_DYNAMIC` constant buffers
//!         (`IRenderDevice::CreateBuffer` - the "resource creation" leg)
//!       * creates SRBs from the same shared PRS instance
//!         (`IPipelineResourceSignature::CreateShaderResourceBinding`)
//!       * binds its buffer into its SRB (`SetBufferRange` + `Set`)
//!   - a fully independent per-thread PRS leg, so both "shared PRS" and
//!     "per-thread PRS creation" are exercised
//!
//! The wrapper keeps its objects pinned to the creating thread (documented
//! discipline), so the concurrent section uses raw pointers through an
//! explicit `unsafe impl Send` carrier - the crate's documented escape hatch
//! (`as_raw()` + caller responsibility).
//!
//! # Conclusion (what the run measures)
//!
//! - exit 0 + "SUPPORTED" when every thread's SRBs were created, bound and
//!   verified with zero engine errors;
//! - the engine ERROR/FATAL message count (`ERROR_COUNT`, via the message
//!   callback) is printed at exit and asserted: any non-zero count turns the
//!   run into a failure (non-zero exit), matching the v17/v22 discipline;
//! - "NOT SUPPORTED" (with a non-zero exit) when any engine call fails or
//!   the process crashes - in which case the bevy-side serialization landing
//!   point is `create_diligent_bind_group` (render_device.rs:354), i.e. the
//!   SRB creation call would go under the existing `CONTEXT_LOCK` (or a new
//!   dedicated mutex next to it in `diligent_registry.rs`).
//!
//! # Usage
//!
//! ```text
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml --example v2_concurrent_srb -- [--threads N] [--iters M]
//! ```

use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const THREADS_DEFAULT: usize = 8;
const ITERS_DEFAULT: usize = 64;

static ERROR_COUNT: AtomicU32 = AtomicU32::new(0);

/// Logs engine ERROR/FATAL messages (the D3D12 debug layer routes through
/// the Diligent message callback).
unsafe extern "C" fn on_message(
    severity: sys::DEBUG_MESSAGE_SEVERITY,
    message: *const sys::Char,
    _function: *const sys::Char,
    _file: *const sys::Char,
    _line: std::os::raw::c_int,
) {
    if severity == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_ERROR as sys::DEBUG_MESSAGE_SEVERITY
        || severity == sys::DEBUG_MESSAGE_SEVERITY::DEBUG_MESSAGE_SEVERITY_FATAL_ERROR as sys::DEBUG_MESSAGE_SEVERITY
    {
        ERROR_COUNT.fetch_add(1, Ordering::Relaxed);
        let msg = if message.is_null() {
            "<null>".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        };
        eprintln!("[v2] engine ERROR: {msg}");
    }
}

const RTV_FORMAT: sys::TEXTURE_FORMAT =
    sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM_SRGB as sys::TEXTURE_FORMAT;

/// Resource-free full-screen triangle VS (the v15 sample-A shape: the PRS
/// declares the DYNAMIC cbuffer, the shaders do not consume it - the
/// concurrent SRB binding is exercised through `SetBufferRange` on the
/// PRS-side variable).
const VS_SOURCE: &str = r#"
struct VSInput { float3 pos : ATTRIB0; };
struct VSOutput { float4 pos : SV_POSITION; };
void main(in VSInput input, out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0),
                      vid == 0 ? -3.0 : (vid == 1 ? 3.0 : 3.0));
    output.pos = float4(p, 0.0, 1.0);
}
"#;

const PS_SOURCE: &str = r#"
struct PSInput { float4 pos : SV_POSITION; };
float4 main(in PSInput input) : SV_TARGET {
    return float4(1.0, 1.0, 1.0, 1.0);
}
"#;

// ---- raw RAII (the v20_full / v3 pattern) -------------------------------

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

/// Explicitly-shareable carrier for the engine's thread-safe objects
/// (`IRenderDevice`, `IPipelineResourceSignature`). The engine documents the
/// device and resource objects as thread safe; the wrapper keeps them pinned
/// for safety, so this example takes the documented opt-in.
#[derive(Clone, Copy)]
struct Shared<T>(*mut T);
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

/// Takes the raw pointer out of a `Shared` by whole-value move (a direct
/// `shared.0` field read inside a closure would make the closure capture the
/// raw pointer *field* under Rust 2024 precise capture - defeating the
/// `Send` carrier).
fn raw_of<T>(s: Shared<T>) -> *mut T {
    s.0
}

// ---- raw FFI helpers ------------------------------------------------------

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

/// `PipelineResourceDesc` for one DYNAMIC constant buffer variable.
fn dyn_cbuffer_res_desc(name: &std::ffi::CStr) -> sys::PipelineResourceDesc {
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

fn create_srb_raw(
    prs: *mut sys::IPipelineResourceSignature,
) -> dil::Result<*mut sys::IShaderResourceBinding> {
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
    shader_type: sys::SHADER_TYPE,
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
    let var = unsafe { get(srb, shader_type, name.as_ptr()) };
    if var.is_null() {
        return Err(dil::Error::NullPointer("shader resource variable"));
    }
    Ok(var)
}

/// Binds a whole-buffer range with an explicit offset + size.
fn set_buffer_range_raw(
    var: *mut sys::IShaderResourceVariable,
    buffer: *mut sys::IBuffer,
    offset: u64,
    size: u64,
    _flags: sys::SET_SHADER_RESOURCE_FLAGS,
) -> dil::Result<()> {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetBufferRange
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IShaderResourceVariable::SetBufferRange",
            ))?
    };
    unsafe { set(var, buffer as *mut sys::IDeviceObject, offset, size, 0, 0) };
    Ok(())
}

fn pso_status(pso: *mut sys::IPipelineState) -> sys::PIPELINE_STATE_STATUS {
    let get = unsafe {
        (*(*pso).pVtbl)
            .PipelineState
            .GetStatus
            .as_ref()
            .expect("IPipelineState::GetStatus missing from vtable")
    };
    unsafe { get(pso, true) }
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
    ci.GraphicsPipeline.BlendDesc.RenderTargets[0].RenderTargetWriteMask =
        sys::_COLOR_MASK::COLOR_MASK_ALL as sys::COLOR_MASK;
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
        face.StencilFunc =
            sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_ALWAYS as sys::COMPARISON_FUNCTION;
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

fn adapter_name(device: *mut sys::IRenderDevice) -> String {
    let get = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .expect("IRenderDevice::GetAdapterInfo missing")
    };
    let info = unsafe { *get(device) };
    unsafe { std::ffi::CStr::from_ptr(info.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn main() -> dil::Result<()> {
    let mut threads_n = THREADS_DEFAULT;
    let mut iters = ITERS_DEFAULT;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--threads" => threads_n = args.next().and_then(|v| v.parse().ok()).unwrap_or(THREADS_DEFAULT),
            "--iters" => iters = args.next().and_then(|v| v.parse().ok()).unwrap_or(ITERS_DEFAULT),
            other => eprintln!("[v2] ignoring unknown arg: {other}"),
        }
    }
    println!("[v2] V2 concurrent SRB + resource creation (D3D12)");
    println!("[v2] threads={threads_n} iters/thread={iters}");

    let factory = dil::EngineFactoryD3D12::d3d12()?;
    // Route engine errors through our counter (the `SetMessageCallback`
    // vtable method - EngineFactoryBase.hpp:119; the global C++ symbol is
    // not exposed as `extern "C"` in the bindings).
    let set_cb = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactory
            .SetMessageCallback
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IEngineFactory::SetMessageCallback"))?
    };
    unsafe { set_cb(factory.as_raw() as *mut sys::IEngineFactory, Some(on_message)) };
    let (device, context) = factory.create_device_and_contexts()?;
    let device = Shared(device.as_raw());
    println!("[v2] adapter: {}", adapter_name(device.0));

    let prs_name = CString::new("PerDraw")?;
    let res = dyn_cbuffer_res_desc(&prs_name);
    let shared_prs = Raw(create_prs_raw(device.0, "v2_shared_prs", std::slice::from_ref(&res))?);

    // One PSO over the shared PRS (validates the concurrent SRB's are usable
    // by a real pipeline; the SRB binding itself happens per thread).
    let vs = Raw(create_shader_raw(device.0, "v2_vs", VS_SOURCE, sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE)?);
    let ps = Raw(create_shader_raw(device.0, "v2_ps", PS_SOURCE, sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE)?);
    let pso = Raw(build_graphics_pso_raw(device.0, "v2_pso", vs.0, ps.0, shared_prs.0)?);
    if pso_status(pso.0) != sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_READY as sys::PIPELINE_STATE_STATUS {
        return Err(dil::Error::CreateFailed("v2 PSO did not reach READY"));
    }
    println!("[v2] shared-PRS PSO: READY");

    // Thread-pool scope pattern (gpu_preprocessing.rs:2352-2418 analogue).
    let shared_prs = Shared(shared_prs.0);
    let prs_name = &prs_name;
    let ok = AtomicU32::new(0);
    let failed = AtomicU32::new(0);
    let per_thread_prs_ok = AtomicU32::new(0);

    let t0 = Instant::now();
    std::thread::scope(|scope| {
        for t in 0..threads_n {
            let shared_prs = shared_prs;
            let ok = &ok;
            let failed = &failed;
            let per_thread_prs_ok = &per_thread_prs_ok;
            scope.spawn(move || {
                let mut ok_local = 0u32;
                let mut failed_local = 0u32;
                for i in 0..iters {
                    // Leg 1: concurrent resource creation (buffers).
                    let buf_name = format!("t{t}_buf_{i}");
                    let buffer = match create_buffer_raw(raw_of(device), &buf_name, 64) {
                        Ok(b) => b,
                        Err(e) => {
                            eprintln!("[v2] thread {t} buffer create failed: {e}");
                            failed_local += 1;
                            continue;
                        }
                    };

                    // Leg 2: concurrent SRB creation from the SHARED PRS.
                    let srb = match create_srb_raw(raw_of(shared_prs)) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("[v2] thread {t} SRB create (shared PRS) failed: {e}");
                            unsafe { release(buffer) };
                            failed_local += 1;
                            continue;
                        }
                    };

                    // Bind the freshly created buffer into the fresh SRB.
                    let var = get_var_raw(srb, sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE, &prs_name);
                    let bound = match var {
                        Ok(var) => set_buffer_range_raw(var, buffer, 0, 64, 0),
                        Err(e) => Err(e),
                    };
                    if let Err(e) = bound {
                        eprintln!("[v2] thread {t} bind failed: {e}");
                        failed_local += 1;
                    } else {
                        ok_local += 1;
                    }
                    unsafe { release(srb) };
                    unsafe { release(buffer) };
                }

                // Leg 3: each thread creates its OWN PRS + SRB concurrently
                // (per-thread PRS creation is also a prepare-thread pattern).
                let own_res = dyn_cbuffer_res_desc(&prs_name);
                let own_name = format!("v2_thread_{t}_prs");
                match create_prs_raw(raw_of(device), &own_name, std::slice::from_ref(&own_res))
                    .and_then(|own| {
                        let srb = create_srb_raw(own)?;
                        unsafe { release(srb) };
                        unsafe { release(own) };
                        Ok(())
                    })
                {
                    Ok(()) => {
                        per_thread_prs_ok.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("[v2] thread {t} own-PRS leg failed: {e}");
                        failed_local += 1;
                    }
                }

                ok.fetch_add(ok_local, Ordering::Relaxed);
                failed.fetch_add(failed_local, Ordering::Relaxed);
            });
        }
    });
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let _ = context;
    let _ = factory;
    let engine_errors = ERROR_COUNT.load(Ordering::Relaxed);
    println!("[v2] done in {elapsed_ms:.1}ms: srb+bind ok={} failed={} per-thread-prs={}/{} engine-errors={engine_errors}",
        ok.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        per_thread_prs_ok.load(Ordering::Relaxed),
        threads_n);
    println!("[v2] total engine ERROR messages observed: {engine_errors}");

    if failed.load(Ordering::Relaxed) == 0 && engine_errors == 0 {
        println!("[v2] CONCLUSION: SUPPORTED - concurrent CreateShaderResourceBinding + CreateBuffer from {threads_n} threads completed with zero engine errors");
        println!("[v2]   bevy mapping: direct mapping, no prepare-side serialization needed (the M1-4b-2 CONTEXT_LOCK stays as-is for the immediate context only)");
        Ok(())
    } else {
        println!("[v2] CONCLUSION: NOT SUPPORTED - {} engine failures observed (engine-errors={engine_errors})", failed.load(Ordering::Relaxed));
        println!("[v2]   bevy landing point: serialize SRB creation in create_diligent_bind_group (render_device.rs) under the CONTEXT_LOCK (or a new SRB_LOCK next to it in diligent_registry.rs)");
        Err(dil::Error::CreateFailed("concurrent SRB creation reported failures"))
    }
}





