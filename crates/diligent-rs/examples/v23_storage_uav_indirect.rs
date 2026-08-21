//! V23 (M2b pre-delivery) + storage-UAV end-to-end verification (M2a-2).
//!
//! Two capability claims are exercised against the REAL engine here:
//!
//!   1. **Storage read_write -> BUFFER_UAV end-to-end**: a compute pass
//!      writes a RAW-mode storage buffer through its default UAV view
//!      (`IBuffer::GetDefaultView(BUFFER_VIEW_UNORDERED_ACCESS)` - the same
//!      binding path bevy_render uses, render_device.rs `buffer_default_view`),
//!      a second pass consumes it, and the result is read back through a
//!      staging buffer + fence and asserted on the CPU.
//!
//!   2. **Indirect dispatch with the D3D12 3x u32 layout (zero translation)**:
//!      the first compute pass ALSO writes a `DispatchIndirectArgs`
//!      `{x, y, z}` (the wgpu/meshlet layout, `fill_counts.wgsl` in bevy_pbr)
//!      and the second pass is issued via `DispatchComputeIndirect` reading
//!      those args at byte offset 0 (`DispatchComputeIndirectAttribs` -
//!      DeviceContext.h:933-944 documents `ThreadGroupCountX/Y/Z`).
//!
//! The **atomic counter** claim of V23: this locked engine version has NO
//! `pCounterBuffer` on `DispatchComputeIndirectAttribs` (unlike
//! `DrawIndirectAttribs`), so count-driven indirect *dispatches* are not
//! expressible - the meshlet pattern writes the count into the args buffer
//! itself (`atomicAdd(&..._dispatch.x, 1u)`), which is exactly what this
//! example does. Count-buffer-driven indirect *draws* ARE expressible
//! (`DrawIndirectAttribs.pCounterBuffer`/`CounterOffset`, DeviceContext.h:435-443)
//! and are wired in bevy_render's `multi_draw_*_indirect_count`; a running
//! draw-path probe needs a windowed swap chain and is left to the bevy
//! smoke tests.
//!
//! # Usage
//!
//! ```text
//!   cargo run --manifest-path crates/diligent-rs/Cargo.toml --example v23_storage_uav_indirect
//! ```
//!
//! Exit code 0 = all assertions passed.

use std::ffi::CString;

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const COMPUTE: sys::SHADER_TYPE = sys::_SHADER_TYPE::SHADER_TYPE_COMPUTE as sys::SHADER_TYPE;
const UAV: sys::SHADER_RESOURCE_TYPE =
    sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_BUFFER_UAV as sys::SHADER_RESOURCE_TYPE;
const MUTABLE: sys::SHADER_RESOURCE_VARIABLE_TYPE =
    sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE
        as sys::SHADER_RESOURCE_VARIABLE_TYPE;
const FLAG_NONE: sys::PIPELINE_RESOURCE_FLAGS =
    sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_NONE as sys::PIPELINE_RESOURCE_FLAGS;
const TRANSITION: sys::RESOURCE_STATE_TRANSITION_MODE =
    sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
        as sys::RESOURCE_STATE_TRANSITION_MODE;

/// Pass A: writes three u32 values into `g_Data` and a `DispatchIndirectArgs`
/// `{1, 1, 1}` into `g_Args` (the D3D12-compatible 3x u32 layout).
const CS_WRITE: &str = r#"
RWByteAddressBuffer g_Args;
RWByteAddressBuffer g_Data;

[numthreads(1, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    g_Data.Store(0, 7u);
    g_Data.Store(4, 11u);
    g_Data.Store(8, 13u);
    // DispatchIndirectArgs { x, y, z } - the wgpu/meshlet layout
    // (bevy_pbr fill_counts.wgsl); zero translation to
    // DispatchComputeIndirectAttribs::ThreadGroupCountX/Y/Z.
    g_Args.Store(0, 1u);
    g_Args.Store(4, 1u);
    g_Args.Store(8, 1u);
}
"#;

/// Pass B: sums the three values (issued INDIRECTLY from `g_Args`).
const CS_SUM: &str = r#"
RWByteAddressBuffer g_Data;
RWByteAddressBuffer g_Out;

[numthreads(1, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    uint sum = g_Data.Load(0) + g_Data.Load(4) + g_Data.Load(8);
    g_Out.Store(0, sum);
}
"#;

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

/// A non-owning engine pointer (no Release on drop) - for pointers the
/// engine does NOT AddRef (e.g. `IBuffer::GetDefaultView`, which returns
/// `m_pDefaultUAV.get()`, BufferBase.hpp:143 - the view stays alive with the
/// buffer).
struct Borrowed<T>(*mut T);

// ---- raw FFI helpers -------------------------------------------------------

fn ctx_vtbl(ctx: *mut sys::IDeviceContext) -> &'static sys::IDeviceContextMethods {
    unsafe { &(*(*ctx).pVtbl).DeviceContext }
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

fn create_uav_buffer(
    device: *mut sys::IRenderDevice,
    name: &str,
    size: u64,
    extra_bind: sys::BIND_FLAGS,
) -> dil::Result<*mut sys::IBuffer> {
    let name_c = CString::new(name)?;
    let mut desc: sys::BufferDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    desc.Size = size;
    desc.BindFlags = sys::_BIND_FLAGS::BIND_UNORDERED_ACCESS as sys::BIND_FLAGS | extra_bind;
    desc.Usage = sys::_USAGE::USAGE_DEFAULT as sys::USAGE;
    desc.Mode = sys::_BUFFER_MODE::BUFFER_MODE_RAW as sys::BUFFER_MODE;
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
        return Err(dil::Error::CreateFailed("uav buffer"));
    }
    Ok(buffer)
}

/// `IBuffer::GetDefaultView(BUFFER_VIEW_UNORDERED_ACCESS)` - the same
/// default-view path bevy_render's storage binding uses
/// (`BufferBase::CreateDefaultViews` creates them for RAW-mode buffers).
fn default_uav_view(buffer: *mut sys::IBuffer) -> dil::Result<*mut sys::IBufferView> {
    let get = unsafe {
        (*(*buffer).pVtbl)
            .Buffer
            .GetDefaultView
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IBuffer::GetDefaultView"))?
    };
    let view = unsafe {
        get(
            buffer,
            sys::_BUFFER_VIEW_TYPE::BUFFER_VIEW_UNORDERED_ACCESS as sys::BUFFER_VIEW_TYPE,
        )
    };
    if view.is_null() {
        return Err(dil::Error::CreateFailed("default UAV view"));
    }
    Ok(view)
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
    let var = unsafe { get(srb, COMPUTE, name.as_ptr()) };
    if var.is_null() {
        return Err(dil::Error::NullPointer("shader resource variable"));
    }
    Ok(var)
}

fn set_var_raw(var: *mut sys::IShaderResourceVariable, view: *mut sys::IBufferView) {
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .Set
            .as_ref()
            .expect("IShaderResourceVariable::Set missing from vtable")
    };
    unsafe { set(var, view as *mut sys::IDeviceObject, 0) };
}

fn dispatch_compute_raw(ctx: *mut sys::IDeviceContext, x: u32, y: u32, z: u32) {
    let attribs = sys::DispatchComputeAttribs {
        ThreadGroupCountX: x,
        ThreadGroupCountY: y,
        ThreadGroupCountZ: z,
        MtlThreadGroupSizeX: 0,
        MtlThreadGroupSizeY: 0,
        MtlThreadGroupSizeZ: 0,
    };
    unsafe {
        ctx_vtbl(ctx)
            .DispatchCompute
            .expect("IDeviceContext::DispatchCompute missing from vtable")(ctx, &attribs);
    }
}

fn dispatch_compute_indirect_raw(ctx: *mut sys::IDeviceContext, buffer: *mut sys::IBuffer) {
    let attribs = sys::DispatchComputeIndirectAttribs {
        pAttribsBuffer: buffer,
        AttribsBufferStateTransitionMode: TRANSITION,
        DispatchArgsByteOffset: 0,
        MtlThreadGroupSizeX: 0,
        MtlThreadGroupSizeY: 0,
        MtlThreadGroupSizeZ: 0,
    };
    unsafe {
        ctx_vtbl(ctx)
            .DispatchComputeIndirect
            .expect("IDeviceContext::DispatchComputeIndirect missing from vtable")(ctx, &attribs);
    }
}

fn flush_ctx(ctx: *mut sys::IDeviceContext) {
    unsafe {
        ctx_vtbl(ctx)
            .Flush
            .expect("IDeviceContext::Flush missing from vtable")(ctx);
    }
}

fn enqueue_signal(ctx: *mut sys::IDeviceContext, fence: *mut sys::IFence, value: u64) {
    unsafe {
        ctx_vtbl(ctx)
            .EnqueueSignal
            .expect("IDeviceContext::EnqueueSignal missing from vtable")(ctx, fence, value);
    }
}

fn fence_wait(fence: *mut sys::IFence, value: u64) -> dil::Result<()> {
    let wait = unsafe {
        (*(*fence).pVtbl)
            .Fence
            .Wait
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IFence::Wait"))?
    };
    unsafe { wait(fence, value) };
    Ok(())
}

/// `IBuffer::GetDesc` via the universal DeviceObject slot.
fn buffer_desc(buffer: *mut sys::IBuffer) -> dil::Result<sys::BufferDesc> {
    let get = unsafe {
        (*(*buffer).pVtbl)
            .DeviceObject
            .GetDesc
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceObject::GetDesc"))?
    };
    let ptr = unsafe { get(buffer.cast::<sys::IDeviceObject>()) };
    if ptr.is_null() {
        return Err(dil::Error::NullPointer("buffer desc"));
    }
    Ok(unsafe { *ptr.cast::<sys::BufferDesc>() })
}

fn main() -> dil::Result<()> {
    println!("[v23] V23 indirect-args layout + storage-UAV end-to-end (D3D12)");

    let factory = dil::EngineFactoryD3D12::d3d12()?;
    let (device, context) = factory.create_device_and_contexts()?;
    let device = device.as_raw();
    let context = context.as_raw();

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
    println!("[v23] adapter: {adapter_name}");

    // ---- buffers ----------------------------------------------------------
    // g_Data: 3 u32 values, storage read_write (BUFFER_UAV).
    let data = Raw(create_uav_buffer(
        device,
        "v23_data",
        12,
        sys::_BIND_FLAGS::BIND_NONE as sys::BIND_FLAGS,
    )?);
    // g_Args: a DispatchIndirectArgs {x,y,z}; written by compute, consumed by
    // DispatchComputeIndirect (BIND_INDIRECT_DRAW_ARGS is required for
    // buffers consumed by indirect commands).
    let args = Raw(create_uav_buffer(
        device,
        "v23_args",
        12,
        sys::_BIND_FLAGS::BIND_INDIRECT_DRAW_ARGS as sys::BIND_FLAGS,
    )?);
    // g_Out: the verification result (1 u32).
    let out = Raw(create_uav_buffer(
        device,
        "v23_out",
        4,
        sys::_BIND_FLAGS::BIND_NONE as sys::BIND_FLAGS,
    )?);

    let data_view = Borrowed(default_uav_view(data.0)?);
    let args_view = Borrowed(default_uav_view(args.0)?);
    let out_view = Borrowed(default_uav_view(out.0)?);
    println!(
        "[v23] default UAV views: data={:p} args={:p} out={:p} (BufferBase::CreateDefaultViews, RAW mode)",
        data_view.0, args_view.0, out_view.0
    );

    // ---- shaders ----------------------------------------------------------
    let cs_write = Raw(create_shader_raw(device, "v23_cs_write", CS_WRITE, COMPUTE)?);
    let cs_sum = Raw(create_shader_raw(device, "v23_cs_sum", CS_SUM, COMPUTE)?);

    // ---- PRS + PSO + SRB (pass A: g_Args + g_Data) --------------------------
    fn uav_resources(names: &mut Vec<CString>) -> Vec<sys::PipelineResourceDesc> {
        ["g_Args", "g_Data"]
            .iter()
            .map(|name| {
                let c = CString::new(*name).unwrap();
                let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
                r.Name = c.as_ptr();
                r.ShaderStages = COMPUTE;
                r.ArraySize = 1;
                r.ResourceType = UAV;
                r.VarType = MUTABLE;
                r.Flags = FLAG_NONE;
                names.push(c);
                r
            })
            .collect()
    }
    let mut names_write = Vec::new();
    let res_write = uav_resources(&mut names_write);
    let prs_write = Raw(create_prs_raw(device, "v23_prs_write", &res_write)?);
    let pso_write = {
        let name_c = CString::new("v23_pso_write")?;
        let mut ci: sys::ComputePipelineStateCreateInfo = unsafe { std::mem::zeroed() };
        ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci._PipelineStateCreateInfo.PSODesc.PipelineType =
            sys::_PIPELINE_TYPE::PIPELINE_TYPE_COMPUTE as sys::PIPELINE_TYPE;
        ci._PipelineStateCreateInfo.ResourceSignaturesCount = 1;
        ci._PipelineStateCreateInfo.ppResourceSignatures =
            std::slice::from_ref(&prs_write.0).as_ptr().cast_mut();
        ci.pCS = cs_write.0;
        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        let create = unsafe {
            (*(*device).pVtbl)
                .RenderDevice
                .CreateComputePipelineState
                .as_ref()
                .ok_or(dil::Error::MissingMethod(
                    "IRenderDevice::CreateComputePipelineState",
                ))?
        };
        unsafe { create(device, &ci, &mut pso) };
        if pso.is_null() {
            return Err(dil::Error::CreateFailed("compute pipeline (write)"));
        }
        std::mem::forget(name_c);
        Raw(pso)
    };
    let srb_write = Raw(create_srb_raw(prs_write.0)?);
    set_var_raw(get_var_raw(srb_write.0, &CString::new("g_Args").unwrap())?, args_view.0);
    set_var_raw(get_var_raw(srb_write.0, &CString::new("g_Data").unwrap())?, data_view.0);

    // ---- pass B: g_Data + g_Out --------------------------------------------
    let mut names_sum = Vec::new();
    let res_sum: Vec<sys::PipelineResourceDesc> = ["g_Data", "g_Out"]
        .iter()
        .map(|name| {
            let c = CString::new(*name).unwrap();
            let mut r: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
            r.Name = c.as_ptr();
            r.ShaderStages = COMPUTE;
            r.ArraySize = 1;
            r.ResourceType = UAV;
            r.VarType = MUTABLE;
            r.Flags = FLAG_NONE;
            names_sum.push(c);
            r
        })
        .collect();
    let prs_sum = Raw(create_prs_raw(device, "v23_prs_sum", &res_sum)?);
    let pso_sum = {
        let name_c = CString::new("v23_pso_sum")?;
        let mut ci: sys::ComputePipelineStateCreateInfo = unsafe { std::mem::zeroed() };
        ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
        ci._PipelineStateCreateInfo.PSODesc.PipelineType =
            sys::_PIPELINE_TYPE::PIPELINE_TYPE_COMPUTE as sys::PIPELINE_TYPE;
        ci._PipelineStateCreateInfo.ResourceSignaturesCount = 1;
        ci._PipelineStateCreateInfo.ppResourceSignatures =
            std::slice::from_ref(&prs_sum.0).as_ptr().cast_mut();
        ci.pCS = cs_sum.0;
        let mut pso: *mut sys::IPipelineState = std::ptr::null_mut();
        let create = unsafe {
            (*(*device).pVtbl)
                .RenderDevice
                .CreateComputePipelineState
                .as_ref()
                .ok_or(dil::Error::MissingMethod(
                    "IRenderDevice::CreateComputePipelineState",
                ))?
        };
        unsafe { create(device, &ci, &mut pso) };
        if pso.is_null() {
            return Err(dil::Error::CreateFailed("compute pipeline (sum)"));
        }
        std::mem::forget(name_c);
        Raw(pso)
    };
    let srb_sum = Raw(create_srb_raw(prs_sum.0)?);
    set_var_raw(get_var_raw(srb_sum.0, &CString::new("g_Data").unwrap())?, data_view.0);
    set_var_raw(get_var_raw(srb_sum.0, &CString::new("g_Out").unwrap())?, out_view.0);

    // ---- execute ------------------------------------------------------------
    unsafe {
        ctx_vtbl(context)
            .SetPipelineState
            .expect("IDeviceContext::SetPipelineState missing from vtable")(context, pso_write.0);
        ctx_vtbl(context)
            .CommitShaderResources
            .expect("IDeviceContext::CommitShaderResources missing from vtable")(
            context,
            srb_write.0,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        );
    }
    dispatch_compute_raw(context, 1, 1, 1);
    println!("[v23] pass A: dispatch(1,1,1) wrote data[0..2] + DispatchIndirectArgs{{1,1,1}}");

    unsafe {
        ctx_vtbl(context)
            .SetPipelineState
            .expect("IDeviceContext::SetPipelineState missing from vtable")(context, pso_sum.0);
        ctx_vtbl(context)
            .CommitShaderResources
            .expect("IDeviceContext::CommitShaderResources missing from vtable")(
            context,
            srb_sum.0,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        );
    }
    dispatch_compute_indirect_raw(context, args.0);
    println!(
        "[v23] pass B: DispatchComputeIndirect(args @ offset 0) - the 3x u32 \
         DispatchIndirectArgs layout consumed as ThreadGroupCountX/Y/Z"
    );

    // ---- readback -----------------------------------------------------------
    let fence = Raw({
        let name_c = CString::new("v23_fence")?;
        let mut desc: sys::FenceDesc = unsafe { std::mem::zeroed() };
        desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        desc.Type = sys::_FENCE_TYPE::FENCE_TYPE_GENERAL as sys::FENCE_TYPE;
        let mut f: *mut sys::IFence = std::ptr::null_mut();
        let create = unsafe {
            (*(*device).pVtbl)
                .RenderDevice
                .CreateFence
                .as_ref()
                .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateFence"))?
        };
        unsafe { create(device, &desc, &mut f) };
        if f.is_null() {
            return Err(dil::Error::CreateFailed("fence"));
        }
        std::mem::forget(name_c);
        f
    });

    let staging = Raw({
        let name_c = CString::new("v23_staging")?;
        let mut desc: sys::BufferDesc = unsafe { std::mem::zeroed() };
        desc._DeviceObjectAttribs.Name = name_c.as_ptr();
        desc.Size = 4;
        desc.Usage = sys::_USAGE::USAGE_STAGING as sys::USAGE;
        desc.CPUAccessFlags = sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as sys::CPU_ACCESS_FLAGS;
        desc.ImmediateContextMask = 0x1;
        let mut b: *mut sys::IBuffer = std::ptr::null_mut();
        let create = unsafe {
            (*(*device).pVtbl)
                .RenderDevice
                .CreateBuffer
                .as_ref()
                .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateBuffer"))?
        };
        unsafe { create(device, &desc, std::ptr::null_mut(), &mut b) };
        if b.is_null() {
            return Err(dil::Error::CreateFailed("staging buffer"));
        }
        std::mem::forget(name_c);
        b
    });

    // copy out -> staging, flush + fence, map and assert.
    unsafe {
        ctx_vtbl(context)
            .CopyBuffer
            .expect("IDeviceContext::CopyBuffer missing from vtable")(
            context,
            out.0,
            0,
            TRANSITION,
            staging.0,
            0,
            4,
            TRANSITION,
        );
    }
    enqueue_signal(context, fence.0, 1);
    flush_ctx(context);
    fence_wait(fence.0, 1)?;
    println!("[v23] GPU fence reached; readback copy complete");

    let mut mapped: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        ctx_vtbl(context)
            .MapBuffer
            .expect("IDeviceContext::MapBuffer missing from vtable")(
            context,
            staging.0,
            sys::_MAP_TYPE::MAP_READ as sys::MAP_TYPE,
            sys::_MAP_FLAGS::MAP_FLAG_DO_NOT_WAIT as sys::MAP_FLAGS,
            &mut mapped,
        );
    }
    if mapped.is_null() {
        return Err(dil::Error::Message("MapBuffer failed: null data".to_string()));
    }
    let sum = unsafe { *(mapped as *const u32) };
    unsafe {
        ctx_vtbl(context)
            .UnmapBuffer
            .expect("IDeviceContext::UnmapBuffer missing from vtable")(
            context,
            staging.0,
            sys::_MAP_TYPE::MAP_READ as sys::MAP_TYPE,
        );
    }
    println!("[v23] readback g_Out = {sum} (expected 7 + 11 + 13 = 31)");
    if sum != 31 {
        return Err(dil::Error::Message(format!(
            "V23 verification FAILED: storage-UAV round trip produced {sum}, expected 31"
        )));
    }

    let _ = buffer_desc(data.0)?;
    println!("[v23] PASS: storage read_write (BUFFER_UAV, RAW mode) write -> read -> readback verified");
    println!("[v23] PASS: DispatchComputeIndirect consumed the 3x u32 DispatchIndirectArgs layout");
    println!("[v23] note: DispatchComputeIndirectAttribs has no pCounterBuffer in this engine version -");
    println!("[v23]       count-driven indirect dispatch is not expressible; the meshlet pattern");
    println!("[v23]       writes the count into the args buffer itself (atomic counter as arg field).");
    Ok(())
}
