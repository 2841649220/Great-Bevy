//! V3 INLINE_CONSTANTS verification (Diligent D3D12 backend).
//!
//! Confirms that Diligent's D3D12 backend supports inline constants (D3D12
//! "root constants") through `PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS`, and
//! that the API maps 1:1 to Bevy's `set_immediates(offset_bytes, data)`:
//!
//! ```text
//! Bevy   : set_immediates(offset, data)
//!   ->   Diligent: IShaderResourceVariable::SetInlineConstants(
//!          pConstants    = data.as_ptr(),
//!          FirstConstant = offset / 4,   // bytes -> 32-bit DWORDs
//!          NumConstants  = data.len() / 4)
//! ```
//!
//! Verification steps:
//! 1. Build a PRS with one `INLINE_CONSTANTS` resource: 8 DWORDs (32 bytes,
//!    Bevy's max immediate size), `ResourceType=CONSTANT_BUFFER`,
//!    `VarType=MUTABLE` (per-draw).
//! 2. Build a PSO from the PRS + a full-screen VS and a PS that reads two
//!    `float4`s from the inline cbuffer and outputs their sum.
//! 3. Each frame issues TWO partial `SetInlineConstants` calls into disjoint
//!    DWORD ranges (FirstConstant=0/NumConstants=4 then FirstConstant=4/
//!    NumConstants=4) and draws.
//! 4. A GPU readback (staging copy + fence) confirms the partial writes reach
//!    the shader independently:
//!      frame 0 -> Color0=red,  Color1=black => output red
//!      frame 1 -> Color0=red,  Color1=green => output yellow (red+green)
//!
//! Exit code 0 when every frame's readback matches the expected color.
//!
//! # Adapter selection (raw FFI)
//!
//! This example creates the device through the raw C API rather than the
//! wrapper's default-adapter path: the wrapper's `create_device_and_contexts`
//! hard-codes `AdapterId = 0`, and on this machine D3D12 enumerates the AMD
//! iGPU (display) first, with the discrete NVIDIA GPU at index 1. Enumerating
//! adapters and explicitly preferring the NVIDIA discrete GPU keeps the
//! verification deterministic across GPU configurations.
//!
//! # Historical wrapper gap (raw FFI kept as historical workaround)
//!
//! `BlendStateDesc::RenderTargets[].RenderTargetWriteMask` has no C++ default
//! in the C API: the C++ default is `COLOR_MASK_ALL`, but a zeroed C struct
//! leaves it 0, and the D3D12 backend maps it straight into
//! `D3D12_RENDER_TARGET_BLEND_DESC::RenderTargetWriteMask` -- a 0 mask
//! silently discards every pixel the PS writes (clears still work). At the
//! time this example was written the wrapper's `create_graphics_pipeline` did
//! not set RenderTargetWriteMask, so wrapper PSOs drew nothing; the defect
//! was since fixed (device.rs, fix-writemask task, 2026-08-05). This example
//! intentionally keeps the raw-FFI path as a historical workaround; do not
//! re-debug.

use std::ffi::CString;
use std::os::raw::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const TOTAL_FRAMES: u32 = 2;
/// Inline constant capacity in 32-bit DWORDs (8 * 4 = 32 bytes, Bevy's max
/// immediate size). D3D12 allows up to 64 DWORDs (256 bytes) of root
/// constants, so 8 is well within the limit.
const INLINE_DWORDS: u32 = 8;

const VS_SOURCE: &str = r#"
struct VSOutput { float4 pos : SV_POSITION; };
// Full-screen triangle synthesized from SV_VertexID (no vertex data needed).
void main(out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? 3.0 : -1.0),
                      vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0));
    output.pos = float4(p, 0.0, 1.0);
}
"#;

const PS_SOURCE: &str = r#"
// 8-DWORD inline constant buffer (root constants). Bound through the PRS
// resource "Constants" flagged PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS.
cbuffer Constants {
    float4 Color0; // DWORDs 0..3 (FirstConstant 0)
    float4 Color1; // DWORDs 4..7 (FirstConstant 4)
};
float4 main() : SV_TARGET {
    return Color0 + Color1;
}
"#;

// ---------------------------------------------------------------------------
// Raw RAII wrappers (the safe wrapper does not expose device creation with
// an explicit adapter, inline-constant methods, or the readback path).
// ---------------------------------------------------------------------------

/// Calls `IObject::Release` on any Diligent interface pointer (the universal
/// first vtable slot; the same trick `diligent-rs`' `Handle` uses).
unsafe fn release<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let obj = ptr.cast::<sys::IObject>();
    let vtbl = unsafe { &*(*obj).pVtbl };
    if let Some(rel) = vtbl.Object.Release {
        unsafe { rel(obj) };
    }
}

/// Owning wrapper: calls `Release` on drop. `Owned(ptr)` never null.
struct Owned<T>(*mut T);

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

impl<T> Owned<T> {
    fn as_raw(&self) -> *mut T {
        self.0
    }
}

/// Resolves a mutable/dynamic shader resource variable by name on the SRB.
/// The returned pointer is non-owning (the engine does not AddRef) and stays
/// valid for the lifetime of the SRB.
fn get_variable_by_name(
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
    // Safety: `srb` is alive and `name` is a live NUL-terminated C string.
    let var = unsafe { get(srb, shader_type, name.as_ptr()) };
    if var.is_null() {
        return Err(dil::Error::NullPointer(
            "inline constant variable 'Constants'",
        ));
    }
    Ok(var)
}

/// Sets inline constants at a byte offset, encoding the Bevy mapping:
///   `FirstConstant = byte_offset / 4`  (bytes -> 32-bit DWORDs)
///   `NumConstants  = data.len()`        (number of DWORDs)
///
/// Returns the `(FirstConstant, NumConstants)` pair actually passed to the
/// engine so callers can log/verify the alignment.
fn set_inline_constants(
    var: *mut sys::IShaderResourceVariable,
    byte_offset: u32,
    data: &[u32],
) -> dil::Result<(u32, u32)> {
    let first_constant = byte_offset / 4;
    let num_constants = data.len() as u32;
    let set = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetInlineConstants
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IShaderResourceVariable::SetInlineConstants",
            ))?
    };
    // Safety: `var` is a live variable pointer; `data` is a valid DWORD array
    // alive for the duration of the call. The engine copies the values into
    // the SRB cache synchronously and uploads them to the command list on
    // CommitShaderResources / the next draw.
    unsafe { set(var, data.as_ptr().cast::<c_void>(), first_constant, num_constants) };
    Ok((first_constant, num_constants))
}

/// Packs an RGBA float4 into four 32-bit DWORDs (the inline-constant unit).
fn f4(r: f32, g: f32, b: f32, a: f32) -> [u32; 4] {
    [r.to_bits(), g.to_bits(), b.to_bits(), a.to_bits()]
}

// ---------------------------------------------------------------------------
// Raw device / context / swap chain / shader / PRS / PSO / SRB creation.
// ---------------------------------------------------------------------------

/// Enumerates the D3D12 adapters the engine can use.
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
    // MinVersion {12, 1}: the feature level the device will be created at
    // (Version {0, 0} maps to an invalid D3D feature level and yields 0
    // compatible adapters).
    let min_version = sys::Version {
        Major: 12,
        Minor: 1,
    };
    // Safety: first call with a null out-array returns the adapter count.
    unsafe { enumerate(factory.cast(), min_version, &mut count, std::ptr::null_mut()) };
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut adapters: Vec<sys::GraphicsAdapterInfo> =
        vec![unsafe { std::mem::zeroed() }; count as usize];
    let mut filled = count;
    // Safety: `adapters` has `count` slots, matching the capacity contract.
    unsafe { enumerate(factory.cast(), min_version, &mut filled, adapters.as_mut_ptr()) };
    Ok(adapters)
}

fn adapter_name(info: &sys::GraphicsAdapterInfo) -> String {
    unsafe { std::ffi::CStr::from_ptr(info.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Picks the adapter to use: prefer the NVIDIA discrete GPU (the known-good
/// configuration for this build), fall back to the first adapter.
fn choose_adapter_id(
    factory: *mut sys::IEngineFactoryD3D12,
) -> dil::Result<u32> {
    let adapters = enumerate_adapters(factory)?;
    println!("[v3] adapters: {}", adapters.len());
    for (i, a) in adapters.iter().enumerate() {
        println!(
            "[v3]   adapter {i}: {} (vendorId={}, deviceId={}, outputs={})",
            adapter_name(a),
            a.VendorId,
            a.DeviceId,
            a.NumOutputs
        );
    }
    let preferred = adapters
        .iter()
        .position(|a| {
            let name = adapter_name(a);
            name.contains("NVIDIA") || name.contains("RTX")
        })
        .unwrap_or(0) as u32;
    println!("[v3] using adapter {preferred}");
    Ok(preferred)
}

/// Creates the render device + immediate context with an explicit adapter.
fn create_device_and_contexts_raw(
    factory: *mut sys::IEngineFactoryD3D12,
    adapter_id: u32,
) -> dil::Result<(Owned<sys::IRenderDevice>, Owned<sys::IDeviceContext>)> {
    // Reuse the wrapper's engine defaults (heap sizes etc.), only override
    // the adapter id and keep validation on for diagnostics.
    let mut ci = dil::desc::engine_d3d12();
    ci._EngineCreateInfo.AdapterId = adapter_id;

    let mut device: *mut sys::IRenderDevice = std::ptr::null_mut();
    let mut context: *mut sys::IDeviceContext = std::ptr::null_mut();
    let create = unsafe {
        (*(*factory).pVtbl)
            .EngineFactoryD3D12
            .CreateDeviceAndContextsD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IEngineFactoryD3D12::CreateDeviceAndContextsD3D12",
            ))?
    };
    // Safety: `device`/`context` are out params; the engine AddRefs them.
    unsafe { create(factory, &ci, &mut device, &mut context) };
    if device.is_null() {
        return Err(dil::Error::CreateFailed("render device (D3D12)"));
    }
    if context.is_null() {
        unsafe { release(device) };
        return Err(dil::Error::CreateFailed("immediate device context (D3D12)"));
    }
    Ok((Owned(device), Owned(context)))
}

fn create_swap_chain_raw(
    factory: *mut sys::IEngineFactoryD3D12,
    device: *mut sys::IRenderDevice,
    context: *mut sys::IDeviceContext,
    hwnd: *mut c_void,
    width: u32,
    height: u32,
) -> dil::Result<Owned<sys::ISwapChain>> {
    let sc_desc = dil::desc::swap_chain(width, height);
    let fs_desc: sys::FullScreenModeDesc = unsafe { std::mem::zeroed() };
    let window = sys::NativeWindow { hWnd: hwnd };
    let mut swap_chain: *mut sys::ISwapChain = std::ptr::null_mut();
    let create = unsafe {
        (*(*factory).pVtbl)
            .EngineFactoryD3D12
            .CreateSwapChainD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IEngineFactoryD3D12::CreateSwapChainD3D12",
            ))?
    };
    // Safety: `window` carries the caller-owned HWND; `swap_chain` is an out
    // param; the descs are valid FFI structs alive for the call.
    unsafe {
        create(
            factory,
            device,
            context,
            &sc_desc,
            &fs_desc,
            &window,
            &mut swap_chain,
        )
    };
    if swap_chain.is_null() {
        return Err(dil::Error::CreateFailed("swap chain"));
    }
    Ok(Owned(swap_chain))
}

fn create_shader_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    source: &str,
    shader_type: sys::SHADER_TYPE,
) -> dil::Result<Owned<sys::IShader>> {
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
    // Safety: all `ci` strings are live CStrings for the duration of the call.
    unsafe { create(device, &ci, &mut shader, std::ptr::null_mut()) };
    if shader.is_null() {
        return Err(dil::Error::CreateFailed("shader"));
    }
    Ok(Owned(shader))
}

fn create_prs_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    resources: &[sys::PipelineResourceDesc],
) -> dil::Result<Owned<sys::IPipelineResourceSignature>> {
    let name_c = CString::new(name)?;
    let mut prs_desc: sys::PipelineResourceSignatureDesc = unsafe { std::mem::zeroed() };
    prs_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    prs_desc.Resources = resources.as_ptr();
    prs_desc.NumResources = resources.len() as u32;
    prs_desc.BindingIndex = 0;

    let mut signature: *mut sys::IPipelineResourceSignature = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreatePipelineResourceSignature
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IRenderDevice::CreatePipelineResourceSignature",
            ))?
    };
    // Safety: `prs_desc` points at live CStrings and the caller-owned
    // resource array; `signature` is an out param.
    unsafe { create(device, &prs_desc, &mut signature) };
    if signature.is_null() {
        return Err(dil::Error::CreateFailed("pipeline resource signature"));
    }
    Ok(Owned(signature))
}

fn create_graphics_pso_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    vs: *mut sys::IShader,
    ps: *mut sys::IShader,
    rtv_format: sys::TEXTURE_FORMAT,
    layout_elements: &[sys::LayoutElement],
    prs: *mut sys::IPipelineResourceSignature,
    dsv_format: sys::TEXTURE_FORMAT,
) -> dil::Result<Owned<sys::IPipelineState>> {
    let name_c = CString::new(name)?;
    let signature_ptrs: [*mut sys::IPipelineResourceSignature; 1] = [prs];

    let mut ci: sys::GraphicsPipelineStateCreateInfo = unsafe { std::mem::zeroed() };
    ci._PipelineStateCreateInfo.PSODesc._DeviceObjectAttribs.Name = name_c.as_ptr();
    ci._PipelineStateCreateInfo.ResourceSignaturesCount = 1;
    ci._PipelineStateCreateInfo.ppResourceSignatures = signature_ptrs.as_ptr().cast_mut();

    ci.GraphicsPipeline.InputLayout.LayoutElements = layout_elements.as_ptr();
    ci.GraphicsPipeline.InputLayout.NumElements = layout_elements.len() as u32;
    ci.GraphicsPipeline.PrimitiveTopology =
        sys::_PRIMITIVE_TOPOLOGY::PRIMITIVE_TOPOLOGY_TRIANGLE_LIST as sys::PRIMITIVE_TOPOLOGY;
    ci.GraphicsPipeline.NumRenderTargets = 1;
    ci.GraphicsPipeline.NumViewports = 1;
    ci.GraphicsPipeline.RTVFormats[0] = rtv_format;
    ci.GraphicsPipeline.DSVFormat = dsv_format;

    // SampleMask/SmplDesc have no C++ defaults (see diligent-rs desc.rs):
    // a zeroed SampleMask discards every pixel; SmplDesc.Count = 0 is
    // E_INVALIDARG on D3D12.
    ci.GraphicsPipeline.SampleMask = dil::desc::DEFAULT_SAMPLE_MASK;
    ci.GraphicsPipeline.SmplDesc.Count = 1;
    ci.GraphicsPipeline.SmplDesc.Quality = 0;

    // BlendStateDesc::RenderTargets[].RenderTargetWriteMask has NO C++
    // default in the C API either: the C++ default is COLOR_MASK_ALL, but a
    // zeroed struct leaves it 0, and the D3D12 backend maps it straight into
    // D3D12_RENDER_TARGET_BLEND_DESC::RenderTargetWriteMask -- a 0 mask
    // discards every pixel the PS writes (clears still work, so the failure
    // is silent). Always set it explicitly. All other blend fields only
    // matter when BlendEnable=true (opaque write otherwise).
    ci.GraphicsPipeline.BlendDesc.RenderTargets[0].RenderTargetWriteMask =
        sys::_COLOR_MASK::COLOR_MASK_ALL as sys::COLOR_MASK;

    // RasterizerStateDesc / DepthStencilStateDesc C++ defaults.
    let ra = &mut ci.GraphicsPipeline.RasterizerDesc;
    ra.FillMode = sys::_FILL_MODE::FILL_MODE_SOLID as sys::FILL_MODE;
    ra.CullMode = sys::_CULL_MODE::CULL_MODE_NONE as sys::CULL_MODE;
    ra.DepthClipEnable = true;
    let ds = &mut ci.GraphicsPipeline.DepthStencilDesc;
    ds.DepthEnable = dsv_format != sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT;
    ds.DepthWriteEnable = ds.DepthEnable;
    ds.DepthFunc = sys::_COMPARISON_FUNCTION::COMPARISON_FUNC_LESS as sys::COMPARISON_FUNCTION;
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
    // Safety: all pointers in `ci` are live for the duration of the call.
    unsafe { create(device, &ci, &mut pso) };
    if pso.is_null() {
        return Err(dil::Error::CreateFailed("graphics pipeline state"));
    }
    Ok(Owned(pso))
}

fn create_srb_raw(
    prs: *mut sys::IPipelineResourceSignature,
) -> dil::Result<Owned<sys::IShaderResourceBinding>> {
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
    // Safety: `srb` is an out param; init static resources = true.
    unsafe { create(prs, &mut srb, true) };
    if srb.is_null() {
        return Err(dil::Error::CreateFailed("shader resource binding"));
    }
    Ok(Owned(srb))
}

fn create_fence_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
) -> dil::Result<Owned<sys::IFence>> {
    let name_c = CString::new(name)?;
    let mut desc: sys::FenceDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    let mut fence: *mut sys::IFence = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateFence
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateFence"))?
    };
    // Safety: `desc` is valid; `fence` is an out param.
    unsafe { create(device, &desc, &mut fence) };
    if fence.is_null() {
        return Err(dil::Error::CreateFailed("fence"));
    }
    Ok(Owned(fence))
}

fn create_vertex_buffer_raw(
    device: *mut sys::IRenderDevice,
    name: &str,
    vertices: &[f32],
) -> dil::Result<Owned<sys::IBuffer>> {
    let name_c = CString::new(name)?;
    let bytes = unsafe {
        std::slice::from_raw_parts(
            vertices.as_ptr().cast::<u8>(),
            std::mem::size_of_val(vertices),
        )
    };
    let desc = dil::desc::buffer(
        bytes.len() as u64,
        sys::_BIND_FLAGS::BIND_VERTEX_BUFFER as sys::BIND_FLAGS,
        sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE,
        0,
    );
    let mut buf_desc = desc;
    buf_desc._DeviceObjectAttribs.Name = name_c.as_ptr();
    let data = dil::desc::buffer_data(bytes);
    let mut buffer: *mut sys::IBuffer = std::ptr::null_mut();
    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateBuffer
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateBuffer"))?
    };
    // Safety: `buf_desc`/`data` are valid FFI structs; the engine copies the
    // initial data synchronously; `buffer` is an out param.
    unsafe { create(device, &buf_desc, &data, &mut buffer) };
    if buffer.is_null() {
        return Err(dil::Error::CreateFailed("vertex buffer"));
    }
    Ok(Owned(buffer))
}

// ---------------------------------------------------------------------------
// Raw context / swap chain calls.
// ---------------------------------------------------------------------------

fn ctx_vtbl(ctx: *mut sys::IDeviceContext) -> &'static sys::IDeviceContextMethods {
    unsafe { &(*(*ctx).pVtbl).DeviceContext }
}

fn set_render_targets(ctx: *mut sys::IDeviceContext, mut rtv: *mut sys::ITextureView) {
    let set = ctx_vtbl(ctx).SetRenderTargets.as_ref().expect("SetRenderTargets");
    // Safety: `rtv` is a live render target view borrowed from the swap chain.
    unsafe {
        set(
            ctx,
            1,
            &mut rtv,
            std::ptr::null_mut(),
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        )
    };
}

fn clear_render_target(ctx: *mut sys::IDeviceContext, rtv: *mut sys::ITextureView, color: [f32; 4]) {
    let clear = ctx_vtbl(ctx).ClearRenderTarget.as_ref().expect("ClearRenderTarget");
    // Safety: `rtv` is a live view; `color` is valid for the call.
    unsafe {
        clear(
            ctx,
            rtv,
            color.as_ptr().cast(),
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        )
    };
}

fn set_viewports(ctx: *mut sys::IDeviceContext, viewports: &[sys::Viewport]) {
    let set = ctx_vtbl(ctx).SetViewports.as_ref().expect("SetViewports");
    // Safety: the viewport array is valid for the duration of the call.
    unsafe { set(ctx, viewports.len() as u32, viewports.as_ptr(), 0, 0) };
}

fn set_vertex_buffers(ctx: *mut sys::IDeviceContext, mut buffer: *mut sys::IBuffer) {
    let set = ctx_vtbl(ctx).SetVertexBuffers.as_ref().expect("SetVertexBuffers");
    let offsets = [0u64];
    // Safety: `buffer` is alive; single-slot binding with reset flag.
    unsafe {
        set(
            ctx,
            0,
            1,
            &mut buffer,
            offsets.as_ptr(),
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
            sys::_SET_VERTEX_BUFFERS_FLAGS::SET_VERTEX_BUFFERS_FLAG_RESET
                as sys::SET_VERTEX_BUFFERS_FLAGS,
        )
    };
}

fn set_pipeline_state(ctx: *mut sys::IDeviceContext, pso: *mut sys::IPipelineState) {
    let set = ctx_vtbl(ctx).SetPipelineState.as_ref().expect("SetPipelineState");
    // Safety: `pso` is alive.
    unsafe { set(ctx, pso) };
}

fn commit_shader_resources(ctx: *mut sys::IDeviceContext, srb: *mut sys::IShaderResourceBinding) {
    let commit = ctx_vtbl(ctx).CommitShaderResources.as_ref().expect("CommitShaderResources");
    // Safety: `srb` is a live binding (or null is allowed when the pipeline
    // has no shader resources; here it is always alive).
    unsafe {
        commit(
            ctx,
            srb,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
        )
    };
}

fn draw(ctx: *mut sys::IDeviceContext, num_vertices: u32) {
    let draw = ctx_vtbl(ctx).Draw.as_ref().expect("Draw");
    let attribs = sys::DrawAttribs {
        NumVertices: num_vertices,
        Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
        NumInstances: 1,
        StartVertexLocation: 0,
        FirstInstanceLocation: 0,
    };
    // Safety: `attribs` is a valid draw command description.
    unsafe { draw(ctx, &attribs) };
}

fn enqueue_signal(ctx: *mut sys::IDeviceContext, fence: *mut sys::IFence, value: u64) {
    let signal = ctx_vtbl(ctx).EnqueueSignal.as_ref().expect("EnqueueSignal");
    // Safety: `fence` is alive; EnqueueSignal does not flush (the caller
    // flushes explicitly before blocking on the fence).
    unsafe { signal(ctx, fence, value) };
}

fn flush_ctx(ctx: *mut sys::IDeviceContext) {
    let flush = ctx_vtbl(ctx).Flush.as_ref().expect("Flush");
    unsafe { flush(ctx) };
}

fn finish_frame(ctx: *mut sys::IDeviceContext) {
    let finish = ctx_vtbl(ctx).FinishFrame.as_ref().expect("FinishFrame");
    unsafe { finish(ctx) };
}

fn fence_wait(fence: *mut sys::IFence, value: u64) -> dil::Result<()> {
    let wait = unsafe {
        (*(*fence).pVtbl)
            .Fence
            .Wait
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IFence::Wait"))?
    };
    // Safety: `fence` is alive; blocks until the GPU reaches `value`.
    unsafe { wait(fence, value) };
    Ok(())
}

fn swap_chain_desc(sc: *mut sys::ISwapChain) -> sys::SwapChainDesc {
    let get = unsafe {
        (*(*sc).pVtbl)
            .SwapChain
            .GetDesc
            .as_ref()
            .expect("ISwapChain::GetDesc")
    };
    // Safety: the engine owns the returned desc; copy it out immediately.
    unsafe { *get(sc) }
}

fn current_back_buffer_rtv(sc: *mut sys::ISwapChain) -> *mut sys::ITextureView {
    let get = unsafe {
        (*(*sc).pVtbl)
            .SwapChain
            .GetCurrentBackBufferRTV
            .as_ref()
            .expect("ISwapChain::GetCurrentBackBufferRTV")
    };
    // Safety: non-owning pointer; valid for the lifetime of the swap chain.
    unsafe { get(sc) }
}

fn present(sc: *mut sys::ISwapChain, sync_interval: u32) {
    let present = unsafe {
        (*(*sc).pVtbl)
            .SwapChain
            .Present
            .as_ref()
            .expect("ISwapChain::Present")
    };
    unsafe { present(sc, sync_interval) };
}

fn resize_swap_chain(sc: *mut sys::ISwapChain, width: u32, height: u32) {
    let resize = unsafe {
        (*(*sc).pVtbl)
            .SwapChain
            .Resize
            .as_ref()
            .expect("ISwapChain::Resize")
    };
    unsafe {
        resize(
            sc,
            width,
            height,
            sys::_SURFACE_TRANSFORM::SURFACE_TRANSFORM_OPTIMAL as sys::SURFACE_TRANSFORM,
        )
    };
}

// ---------------------------------------------------------------------------
// GPU readback (raw FFI, adapted from examples/triangle.rs).
// ---------------------------------------------------------------------------

struct ReadbackResult {
    width: u32,
    height: u32,
    red_count: usize,
    yellow_count: usize,
    black_count: usize,
    min_r: u8,
    max_r: u8,
    min_g: u8,
    max_g: u8,
    min_b: u8,
    max_b: u8,
}

fn create_staging_texture(
    device: *mut sys::IRenderDevice,
    width: u32,
    height: u32,
) -> dil::Result<Owned<sys::ITexture>> {
    let mut td: sys::TextureDesc = unsafe { std::mem::zeroed() };
    td.Type = sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_2D as sys::RESOURCE_DIMENSION;
    td.Width = width;
    td.Height = height;
    td.__bindgen_anon_1.ArraySize = 1;
    td.Format = sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM as sys::TEXTURE_FORMAT;
    td.MipLevels = 1;
    td.SampleCount = 1;
    td.Usage = sys::_USAGE::USAGE_STAGING as sys::USAGE;
    td.CPUAccessFlags = sys::_CPU_ACCESS_FLAGS::CPU_ACCESS_READ as sys::CPU_ACCESS_FLAGS;
    td.ImmediateContextMask = 1;

    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateTexture"))?
    };
    let mut tex: *mut sys::ITexture = std::ptr::null_mut();
    // Safety: `td` is a valid texture description; `tex` is an out param.
    unsafe { create(device, &td, std::ptr::null(), &mut tex) };
    if tex.is_null() {
        return Err(dil::Error::CreateFailed("staging texture"));
    }
    Ok(Owned(tex))
}

/// Copies `src_texture` (w x h) into a fresh staging texture, blocks on a
/// fence until the copy is complete, then scans the pixels. The staging
/// texture is created and released per call.
fn readback_texture(
    device: *mut sys::IRenderDevice,
    ctx: *mut sys::IDeviceContext,
    src_texture: *mut sys::ITexture,
    width: u32,
    height: u32,
    fence: *mut sys::IFence,
    fence_value: u64,
) -> dil::Result<ReadbackResult> {
    let staging = create_staging_texture(device, width, height)?;

    let mut attribs: sys::CopyTextureAttribs = unsafe { std::mem::zeroed() };
    attribs.pSrcTexture = src_texture;
    attribs.SrcMipLevel = 0;
    attribs.SrcSlice = 0;
    attribs.SrcTextureTransitionMode =
        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE;
    attribs.pDstTexture = staging.as_raw();
    attribs.DstMipLevel = 0;
    attribs.DstSlice = 0;
    attribs.DstX = 0;
    attribs.DstY = 0;
    attribs.DstZ = 0;
    attribs.DstTextureTransitionMode =
        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE;

    unsafe {
        ctx_vtbl(ctx)
            .CopyTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceContext::CopyTexture"))?(ctx, &attribs);
    }
    // EnqueueSignal does not flush the context, so flush first or the signal
    // never reaches the GPU; then block on the CPU until the copy is done.
    enqueue_signal(ctx, fence, fence_value);
    flush_ctx(ctx);
    fence_wait(fence, fence_value)?;

    let mut mapped: sys::MappedTextureSubresource = unsafe { std::mem::zeroed() };
    unsafe {
        ctx_vtbl(ctx)
            .MapTextureSubresource
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceContext::MapTextureSubresource"))?(
            ctx,
            staging.as_raw(),
            0,
            0,
            sys::_MAP_TYPE::MAP_READ as sys::MAP_TYPE,
            sys::_MAP_FLAGS::MAP_FLAG_DO_NOT_WAIT as sys::MAP_FLAGS,
            std::ptr::null(),
            &mut mapped,
        );
    }
    if mapped.pData.is_null() {
        return Err(dil::Error::Message("map failed: null data".to_string()));
    }

    let width = width as usize;
    let height = height as usize;
    let row_pitch = mapped.Stride as usize;
    let pixels =
        unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), row_pitch * height) };

    let mut res = ReadbackResult {
        width: width as u32,
        height: height as u32,
        red_count: 0,
        yellow_count: 0,
        black_count: 0,
        min_r: 255,
        max_r: 0,
        min_g: 255,
        max_g: 0,
        min_b: 255,
        max_b: 0,
    };
    for y in 0..height {
        for x in 0..width {
            let r = pixels[y * row_pitch + x * 4];
            let g = pixels[y * row_pitch + x * 4 + 1];
            let b = pixels[y * row_pitch + x * 4 + 2];
            if r > 200 && g < 60 && b < 60 {
                res.red_count += 1;
            } else if r > 200 && g > 200 && b < 60 {
                res.yellow_count += 1;
            } else if r < 10 && g < 10 && b < 10 {
                res.black_count += 1;
            }
            res.min_r = res.min_r.min(r);
            res.max_r = res.max_r.max(r);
            res.min_g = res.min_g.min(g);
            res.max_g = res.max_g.max(g);
            res.min_b = res.min_b.min(b);
            res.max_b = res.max_b.max(b);
        }
    }

    unsafe {
        ctx_vtbl(ctx)
            .UnmapTextureSubresource
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IDeviceContext::UnmapTextureSubresource",
            ))?(ctx, staging.as_raw(), 0, 0);
    }
    Ok(res)
}

// ---------------------------------------------------------------------------
// Application.
// ---------------------------------------------------------------------------

// Field order = drop order. The SRB is released before the PSO (the PSO holds
// the last PRS reference); the swap chain before the window and device; the
// device before the factory. `inline_var` is a non-owning pointer borrowed
// from the SRB (no Release) and `fence_value` is plain data.
struct RenderState {
    context: Owned<sys::IDeviceContext>,
    swap_chain: Owned<sys::ISwapChain>,
    vertex_buffer: Owned<sys::IBuffer>,
    srb: Owned<sys::IShaderResourceBinding>,
    pso: Owned<sys::IPipelineState>,
    inline_var: *mut sys::IShaderResourceVariable,
    fence: Owned<sys::IFence>,
    fence_value: u64,
    window: Window,
    _device: Owned<sys::IRenderDevice>,
    _factory: Owned<sys::IEngineFactoryD3D12>,
}

struct V3App {
    state: Option<RenderState>,
    frames: u32,
    exiting: bool,
    /// (frame index, readback pass) collected for the final verdict.
    results: Vec<(u32, bool)>,
}

impl V3App {
    fn setup(&mut self, event_loop: &ActiveEventLoop) -> dil::Result<()> {
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("diligent-rs v3 inline constants (D3D12)")
                    .with_inner_size(LogicalSize::new(WIDTH as f64, HEIGHT as f64)),
            )
            .map_err(|e| dil::Error::Message(format!("create window: {e}")))?;

        let hwnd = match window
            .window_handle()
            .map_err(|e| dil::Error::Message(e.to_string()))?
            .as_raw()
        {
            RawWindowHandle::Win32(h) => h.hwnd.get() as *mut c_void,
            other => {
                return Err(dil::Error::Message(format!(
                    "unexpected raw window handle: {other:?}"
                )))
            }
        };
        println!("[v3] window: HWND = {hwnd:p}");

        let factory = Owned(unsafe { sys::Diligent_GetEngineFactoryD3D12() });
        println!(
            "[v3] factory: Diligent_GetEngineFactoryD3D12 ok (API v{})",
            sys::DILIGENT_API_VERSION
        );

        // EnumerateAdapters (and device creation) require the D3D12 library
        // to be loaded first.
        let load_d3d12 = unsafe {
            (*(*factory.as_raw()).pVtbl)
                .EngineFactoryD3D12
                .LoadD3D12
                .as_ref()
                .ok_or(dil::Error::MissingMethod("IEngineFactoryD3D12::LoadD3D12"))?
        };
        let loaded = unsafe { load_d3d12(factory.as_raw(), c"d3d12.dll".as_ptr()) };
        println!("[v3] LoadD3D12(\"d3d12.dll\") = {loaded}");

        // --- Adapter selection: prefer the NVIDIA discrete GPU (the AMD
        // iGPU is enumerated first on this machine and renders black with
        // this Diligent build). ---
        let adapter_id = choose_adapter_id(factory.as_raw())?;

        let (device, context) = create_device_and_contexts_raw(factory.as_raw(), adapter_id)?;
        let dinfo = unsafe { *device_info(device.as_raw())? };
        println!(
            "[v3] device: type={:?}, API {}.{}",
            dinfo.Type, dinfo.APIVersion.Major, dinfo.APIVersion.Minor
        );
        let adapter = unsafe { *adapter_info(device.as_raw())? };
        let adapter_name = unsafe { std::ffi::CStr::from_ptr(adapter.Description.as_ptr()) }
            .to_string_lossy();
        println!(
            "[v3] adapter: {adapter_name} (vendorId={}, deviceId={})",
            adapter.VendorId, adapter.DeviceId
        );

        let swap_chain = create_swap_chain_raw(
            factory.as_raw(),
            device.as_raw(),
            context.as_raw(),
            hwnd,
            WIDTH,
            HEIGHT,
        )?;
        let sc_desc = swap_chain_desc(swap_chain.as_raw());
        println!(
            "[v3] swap chain: {}x{}, colorFormat={:?}",
            sc_desc.Width, sc_desc.Height, sc_desc.ColorBufferFormat
        );

        // --- PRS with one INLINE_CONSTANTS resource (8 DWORDs = 32 bytes). ---
        let constants_name = CString::new("Constants")?;
        let inline_flag =
            sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS as sys::PIPELINE_RESOURCE_FLAGS;
        let mut inline_res: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
        inline_res.Name = constants_name.as_ptr();
        inline_res.ShaderStages = sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
        inline_res.ArraySize = INLINE_DWORDS;
        inline_res.ResourceType =
            sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE;
        inline_res.VarType =
            sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE as sys::SHADER_RESOURCE_VARIABLE_TYPE;
        inline_res.Flags = inline_flag;
        println!(
            "[v3] PRS resource: name='Constants' stages=PIXEL ArraySize={} DWORDs ({} bytes) \
             type=CONSTANT_BUFFER var=MUTABLE Flags=INLINE_CONSTANTS(value={})",
            INLINE_DWORDS,
            INLINE_DWORDS * 4,
            inline_flag
        );

        let prs = create_prs_raw(device.as_raw(), "v3 inline PRS", &[inline_res])?;
        println!("[v3] PRS: created OK (INLINE_CONSTANTS resource accepted)");

        let vs = create_shader_raw(
            device.as_raw(),
            "v3 VS",
            VS_SOURCE,
            sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE,
        )?;
        let ps = create_shader_raw(
            device.as_raw(),
            "v3 PS",
            PS_SOURCE,
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
        )?;
        println!(
            "[v3] shaders: VS+PS compiled (VS status={}, PS status={})",
            shader_status(vs.as_raw()),
            shader_status(ps.as_raw())
        );

        // Dummy vertex buffer + layout: the VS uses SV_VertexID, but the
        // input assembler expects a bound buffer for a declared element.
        let vertex_buffer = create_vertex_buffer_raw(device.as_raw(), "v3 dummy VB", &[0.0; 9])?;
        let attr0 = CString::new("ATTRIB")?;
        let layout_elements = [dil::desc::layout_element(
            &attr0,
            0,
            0,
            3,
            sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE,
            false,
        )];

        let srb = create_srb_raw(prs.as_raw())?;
        println!("[v3] SRB: created OK (init static resources = true)");

        let pso = create_graphics_pso_raw(
            device.as_raw(),
            "v3 inline PSO",
            vs.as_raw(),
            ps.as_raw(),
            sc_desc.ColorBufferFormat,
            &layout_elements,
            prs.as_raw(),
            sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT,
        )?;
        println!(
            "[v3] PSO: created OK (1 explicit PRS, RTV format {:?}, status={})",
            sc_desc.ColorBufferFormat,
            pso_status(pso.as_raw())
        );

        // Resolve the mutable inline-constant variable on the SRB.
        let inline_var = get_variable_by_name(
            srb.as_raw(),
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
            &constants_name,
        )?;
        println!("[v3] inline variable: 'Constants' resolved on SRB (ptr = {inline_var:p})");

        self.state = Some(RenderState {
            context,
            swap_chain,
            vertex_buffer,
            srb,
            pso,
            inline_var,
            fence: create_fence_raw(device.as_raw(), "v3 readback fence")?,
            fence_value: 0,
            window,
            _device: device,
            _factory: factory,
        });
        Ok(())
    }
}

fn shader_status(shader: *mut sys::IShader) -> sys::SHADER_STATUS {
    let get = unsafe {
        (*(*shader).pVtbl)
            .Shader
            .GetStatus
            .as_ref()
            .expect("IShader::GetStatus")
    };
    // Safety: `shader` is alive; may block until compilation completes.
    unsafe { get(shader, true) }
}

fn pso_status(pso: *mut sys::IPipelineState) -> sys::PIPELINE_STATE_STATUS {
    let get = unsafe {
        (*(*pso).pVtbl)
            .PipelineState
            .GetStatus
            .as_ref()
            .expect("IPipelineState::GetStatus")
    };
    // Safety: `pso` is alive; may block until compilation completes.
    unsafe { get(pso, true) }
}

/// Returns the device info via `IRenderDevice::GetDeviceInfo`.
fn device_info(device: *mut sys::IRenderDevice) -> dil::Result<*const sys::RenderDeviceInfo> {
    let get = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .GetDeviceInfo
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::GetDeviceInfo"))?
    };
    Ok(unsafe { get(device) })
}

/// Returns the adapter info via `IRenderDevice::GetAdapterInfo`.
fn adapter_info(device: *mut sys::IRenderDevice) -> dil::Result<*const sys::GraphicsAdapterInfo> {
    let get = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::GetAdapterInfo"))?
    };
    Ok(unsafe { get(device) })
}

impl ApplicationHandler for V3App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        if let Err(e) = self.setup(event_loop) {
            eprintln!("[v3] FATAL setup: {e}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("[v3] close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = &mut self.state {
                    if size.width > 0 && size.height > 0 {
                        resize_swap_chain(state.swap_chain.as_raw(), size.width, size.height);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if self.exiting {
            return;
        }
        let frame = self.frames;
        match render_frame(state, frame) {
            Ok(pass) => self.results.push((frame, pass)),
            Err(e) => {
                eprintln!("[v3] FATAL render frame {frame}: {e}");
                self.exiting = true;
                event_loop.exit();
                return;
            }
        }
        self.frames += 1;
        if self.frames >= TOTAL_FRAMES {
            println!("[v3] {TOTAL_FRAMES} frames rendered, exiting cleanly");
            self.exiting = true;
            event_loop.exit();
            return;
        }
        // winit 0.30 blocks in the OS wait when idle; request a redraw to keep
        // the render loop going for the next frame.
        state.window.request_redraw();
    }
}

/// Renders one frame: two partial inline-constant writes + draw + readback.
/// Returns `Ok(true)` when the readback matches the expected color.
fn render_frame(state: &mut RenderState, frame: u32) -> dil::Result<bool> {
    let ctx = state.context.as_raw();
    let sc = state.swap_chain.as_raw();
    let rtv = current_back_buffer_rtv(sc);
    if rtv.is_null() {
        return Err(dil::Error::NullPointer("back buffer RTV"));
    }
    let sc_desc = swap_chain_desc(sc);

    set_render_targets(ctx, rtv);
    clear_render_target(ctx, rtv, [0.0, 0.0, 0.0, 1.0]);
    let vp = [dil::desc::viewport(sc_desc.Width as f32, sc_desc.Height as f32)];
    set_viewports(ctx, &vp);
    set_vertex_buffers(ctx, state.vertex_buffer.as_raw());
    set_pipeline_state(ctx, state.pso.as_raw());
    // Commit the SRB first so the root signature is bound; the inline (root)
    // constants are re-uploaded from the SRB cache on the next draw because
    // the draw does not use DRAW_FLAG_INLINE_CONSTANTS_INTACT.
    commit_shader_resources(ctx, state.srb.as_raw());

    // Two partial writes into disjoint DWORD ranges of the 8-DWORD capacity.
    //   write 1 -> DWORDs 0..3 : byte offset 0  -> FirstConstant 0
    //   write 2 -> DWORDs 4..7 : byte offset 16 -> FirstConstant 4
    let red = f4(1.0, 0.0, 0.0, 1.0);
    let green = f4(0.0, 1.0, 0.0, 1.0);
    let black = f4(0.0, 0.0, 0.0, 0.0);

    let (first_color, second_color, expected) = if frame == 0 {
        (red, black, "red")
    } else {
        (red, green, "yellow")
    };

    let (fc1, nc1) = set_inline_constants(state.inline_var, 0, &first_color)?;
    let (fc2, nc2) = set_inline_constants(state.inline_var, 16, &second_color)?;
    println!(
        "[v3] frame {frame}: partial writes \
         SetInlineConstants(FirstConstant={fc1}, NumConstants={nc1}) \
         + SetInlineConstants(FirstConstant={fc2}, NumConstants={nc2}) \
         [offset 0/4=0, offset 16/4=4]",
    );

    draw(ctx, 3);

    // GPU readback: confirm the partial writes reached the shader.
    state.fence_value += 1;
    let src_rtv = current_back_buffer_rtv(sc);
    if src_rtv.is_null() {
        return Err(dil::Error::NullPointer("back buffer RTV"));
    }
    let get_texture = unsafe {
        (*(*src_rtv).pVtbl)
            .TextureView
            .GetTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("ITextureView::GetTexture"))?
    };
    let back_buffer = unsafe { get_texture(src_rtv) };
    if back_buffer.is_null() {
        return Err(dil::Error::NullPointer("back buffer texture"));
    }
    let rb = readback_texture(
        state._device.as_raw(),
        ctx,
        back_buffer,
        sc_desc.Width,
        sc_desc.Height,
        state.fence.as_raw(),
        state.fence_value,
    )?;
    let total = (rb.width as usize) * (rb.height as usize);
    let dominant = if frame == 0 { rb.red_count } else { rb.yellow_count };
    let pass = dominant * 2 > total;
    println!(
        "[v3] frame {frame}: readback {}x{} ({}px) expected={expected} | \
         red={} yellow={} black={} | R[{},{}] G[{},{}] B[{},{}] | \
         dominant={dominant} ({:.0}%) => {}",
        rb.width,
        rb.height,
        total,
        rb.red_count,
        rb.yellow_count,
        rb.black_count,
        rb.min_r,
        rb.max_r,
        rb.min_g,
        rb.max_g,
        rb.min_b,
        rb.max_b,
        (dominant as f64) * 100.0 / total as f64,
        if pass { "PASS" } else { "FAIL" },
    );

    present(sc, 1);
    finish_frame(ctx);
    Ok(pass)
}

fn main() -> dil::Result<()> {
    println!("[v3] diligent-rs INLINE_CONSTANTS verification (D3D12 backend, winit 0.30)");
    let event_loop =
        EventLoop::new().map_err(|e| dil::Error::Message(format!("event loop: {e}")))?;
    let mut app = V3App {
        state: None,
        frames: 0,
        exiting: false,
        results: Vec::new(),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| dil::Error::Message(format!("run app: {e}")))?;

    println!("[v3] ===== V3 INLINE_CONSTANTS SUMMARY =====");
    println!(
        "[v3] PRS resource: Flags=PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS ({}), \
         ResourceType=SHADER_RESOURCE_TYPE_CONSTANT_BUFFER, ArraySize={} DWORDs ({} bytes)",
        sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_INLINE_CONSTANTS as u32,
        INLINE_DWORDS,
        INLINE_DWORDS * 4,
    );
    println!(
        "[v3] SetInlineConstants signature: (const void* pConstants, Uint32 FirstConstant, Uint32 NumConstants)"
    );
    println!(
        "[v3] alignment: FirstConstant = byte_offset / 4  (offset 0 -> {}, offset 16 -> {})",
        0u32 / 4,
        16u32 / 4,
    );
    println!(
        "[v3] capacity: {} DWORDs (32 bytes) within D3D12 root-constant limit (64 DWORDs / 256 bytes)",
        INLINE_DWORDS
    );
    println!(
        "[v3] update frequency: MUTABLE variable, set per-draw after CommitShaderResources, no SRB recommit"
    );
    for (f, p) in &app.results {
        println!("[v3]   frame {f}: {}", if *p { "PASS" } else { "FAIL" });
    }

    let all_pass = !app.results.is_empty() && app.results.iter().all(|&(_, p)| p);
    if all_pass {
        println!("[v3] V3 INLINE_CONSTANTS: VERIFIED - Bevy set_immediates(offset, data) maps to SetInlineConstants(FirstConstant=offset/4, NumConstants)");
        Ok(())
    } else {
        println!("[v3] V3 INLINE_CONSTANTS: VERIFICATION FAILED");
        Err(dil::Error::Message(
            "inline constants verification failed".to_string(),
        ))
    }
}
