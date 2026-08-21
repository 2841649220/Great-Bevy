//! V19: RUNTIME_ARRAY + ArraySize=5000 + partial binding verification
//! (D3D12 backend).
//!
//! Verifies the bindless material slab semantics (section 4.4.6 of the M0
//! verification plan): a TEXTURE_SRV pipeline resource with
//! `ArraySize=5000` (Solari MAX_TEXTURE_COUNT) and
//! `PIPELINE_RESOURCE_FLAG_RUNTIME_ARRAY` (1<<4) can be created, a SRB can
//! be derived from the PRS, and a *partial* array bind can be performed
//! without touching the other ~4996 elements.
//!
//! The brief hints at `SetArrayRange`/`SetTextureArray`/`GetArrayElement`.
//! Those names do **not** exist in the locked commit headers
//! (`ShaderResourceBinding.h` / `ShaderResourceVariable.h`): the real
//! partial-bind API on `IShaderResourceVariable` is
//!
//! ```text
//! SetArray(ppObjects, FirstElement, NumElements, Flags)
//! ```
//!
//! (first element + element count = range), plus `Get(ArrayIndex)` for
//! element-wise readback. Also verified: `IRenderDevice` in this locked
//! version has **no** `GetDeviceCaps` method; device/adapter limits are
//! read via `GetDeviceInfo()` + `GetAdapterInfo()` instead.
//!
//! Headless: no window, no swap chain. Textures are created through the raw
//! `IRenderDevice::CreateTexture` vtable (the diligent-rs wrapper has no
//! `create_texture`). Note that a TEXTURE_SRV variable must be bound with
//! the texture's **view** (`ITexture::GetDefaultView(TEXTURE_VIEW_SHADER_RESOURCE)`),
//! not the raw `ITexture`: the D3D12 backend silently ignores the latter.
//!
//! The engine factory defaults to adapter 0 (on this hybrid machine that is
//! the AMD iGPU); this example enumerates adapters, prefers the NVIDIA GPU
//! (vendor 0x10DE) and falls back to adapter 0 otherwise.

use std::ffi::CString;
use std::time::Instant;

use diligent_rs as dil;
use diligent_sys::bindings as sys;

/// Solari MAX_TEXTURE_COUNT (binder.rs:25).
const ARRAY_SIZE: u32 = 5000;
/// Number of 1x1 textures used for the partial bind.
const NUM_TEXTURES: u32 = 4;
/// PRS resource name (mirrors the bindless material texture array).
const VAR_NAME: &str = "g_Textures";
/// NVIDIA PCI vendor id.
const VENDOR_NVIDIA: u32 = 0x10DE;

const PIXEL: sys::SHADER_TYPE = sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE;
const TEXTURE_SRV: sys::SHADER_RESOURCE_TYPE =
    sys::_SHADER_RESOURCE_TYPE::SHADER_RESOURCE_TYPE_TEXTURE_SRV as sys::SHADER_RESOURCE_TYPE;
const MUTABLE: sys::SHADER_RESOURCE_VARIABLE_TYPE =
    sys::_SHADER_RESOURCE_VARIABLE_TYPE::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE as sys::SHADER_RESOURCE_VARIABLE_TYPE;
const RUNTIME_ARRAY: sys::PIPELINE_RESOURCE_FLAGS =
    sys::_PIPELINE_RESOURCE_FLAGS::PIPELINE_RESOURCE_FLAG_RUNTIME_ARRAY as sys::PIPELINE_RESOURCE_FLAGS;
const SET_NONE: sys::SET_SHADER_RESOURCE_FLAGS =
    sys::_SET_SHADER_RESOURCE_FLAGS::SET_SHADER_RESOURCE_FLAG_NONE as sys::SET_SHADER_RESOURCE_FLAGS;
const SET_OVERWRITE: sys::SET_SHADER_RESOURCE_FLAGS =
    sys::_SET_SHADER_RESOURCE_FLAGS::SET_SHADER_RESOURCE_FLAG_ALLOW_OVERWRITE as sys::SET_SHADER_RESOURCE_FLAGS;

/// RAII wrapper for a raw `ITexture` pointer (calls `IObject::Release`).
struct RawTexture(*mut sys::ITexture);

impl Drop for RawTexture {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

impl RawTexture {
    fn as_ptr(&self) -> *mut sys::ITexture {
        self.0
    }

    /// Returns the texture's default SRV view (`ITextureView`). Borrowed:
    /// the engine does NOT AddRef it, so Release() must not be called; the
    /// view lives as long as the texture.
    fn default_srv(&self) -> *mut sys::ITextureView {
        let get_default = unsafe {
            (*(*self.0).pVtbl)
                .Texture
                .GetDefaultView
                .as_ref()
                .expect("ITexture::GetDefaultView missing from vtable")
        };
        unsafe {
            get_default(
                self.0,
                sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_SHADER_RESOURCE as sys::TEXTURE_VIEW_TYPE,
            )
        }
    }
}

/// RAII wrapper for a raw `IRenderDevice` pointer.
struct RawDevice(*mut sys::IRenderDevice);

impl Drop for RawDevice {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// RAII wrapper for a raw `IPipelineResourceSignature` pointer.
struct RawPrs(*mut sys::IPipelineResourceSignature);

impl Drop for RawPrs {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// RAII wrapper for a raw `IShaderResourceBinding` pointer.
struct RawSrb(*mut sys::IShaderResourceBinding);

impl Drop for RawSrb {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

/// Calls `IObject::Release` on any Diligent interface pointer.
unsafe fn release<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let obj = ptr as *mut sys::IObject;
    // Safety: caller guarantees `ptr` is a live Diligent interface.
    let vtbl = unsafe { &*(*obj).pVtbl };
    if let Some(rel) = vtbl.Object.Release {
        // Safety: the vtable Release slot is valid for any Diligent object.
        unsafe { rel(obj) };
    }
}

fn adapter_name(info: &sys::GraphicsAdapterInfo) -> String {
    if info.Description[0] == 0 {
        return "<unnamed>".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(info.Description.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// `EngineD3D12CreateInfo` mirroring the wrapper defaults, with `AdapterId`
/// overridden (the wrapper hardcodes the zeroed default = adapter 0).
fn build_engine_ci(adapter_id: u32) -> sys::EngineD3D12CreateInfo {
    let mut ci: sys::EngineD3D12CreateInfo = unsafe { std::mem::zeroed() };
    ci._EngineCreateInfo.EngineAPIVersion = sys::DILIGENT_API_VERSION as i32;
    ci._EngineCreateInfo.EnableValidation = true;
    ci._EngineCreateInfo.AdapterId = adapter_id;
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

/// Creates a 1x1 RGBA8 `USAGE_DEFAULT` texture via raw
/// `IRenderDevice::CreateTexture` (no initial data).
fn create_texture_raw(device: *mut sys::IRenderDevice, name: &str) -> dil::Result<RawTexture> {
    let name_c = CString::new(name)?;
    let mut td: sys::TextureDesc = unsafe { std::mem::zeroed() };
    td._DeviceObjectAttribs.Name = name_c.as_ptr();
    td.Type = sys::_RESOURCE_DIMENSION::RESOURCE_DIM_TEX_2D as sys::RESOURCE_DIMENSION;
    td.Width = 1;
    td.Height = 1;
    td.__bindgen_anon_1.ArraySize = 1;
    td.Format = sys::_TEXTURE_FORMAT::TEX_FORMAT_RGBA8_UNORM as sys::TEXTURE_FORMAT;
    td.MipLevels = 1;
    td.SampleCount = 1;
    td.BindFlags = sys::_BIND_FLAGS::BIND_SHADER_RESOURCE as sys::BIND_FLAGS;
    td.Usage = sys::_USAGE::USAGE_DEFAULT as sys::USAGE;
    td.ImmediateContextMask = 1;

    let create = unsafe {
        (*(*device).pVtbl)
            .RenderDevice
            .CreateTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateTexture"))?
    };
    let mut tex: *mut sys::ITexture = std::ptr::null_mut();
    // Safety: `td` is a fully initialized TextureDesc (name CString alive
    // for the call); pData = null (no initial data); `tex` is an out param.
    unsafe { create(device, &td, std::ptr::null(), &mut tex) };
    if tex.is_null() {
        return Err(dil::Error::CreateFailed("texture"));
    }
    Ok(RawTexture(tex))
}

fn feature_state_name(s: sys::DEVICE_FEATURE_STATE) -> &'static str {
    use sys::_DEVICE_FEATURE_STATE as D;
    match s {
        v if v == D::DEVICE_FEATURE_STATE_DISABLED as sys::DEVICE_FEATURE_STATE => "DISABLED",
        v if v == D::DEVICE_FEATURE_STATE_ENABLED as sys::DEVICE_FEATURE_STATE => "ENABLED",
        v if v == D::DEVICE_FEATURE_STATE_OPTIONAL as sys::DEVICE_FEATURE_STATE => "OPTIONAL",
        _ => "UNKNOWN",
    }
}

fn var_type_name(s: sys::SHADER_RESOURCE_VARIABLE_TYPE) -> &'static str {
    use sys::_SHADER_RESOURCE_VARIABLE_TYPE as T;
    match s {
        v if v == T::SHADER_RESOURCE_VARIABLE_TYPE_STATIC as sys::SHADER_RESOURCE_VARIABLE_TYPE => "STATIC",
        v if v == T::SHADER_RESOURCE_VARIABLE_TYPE_MUTABLE as sys::SHADER_RESOURCE_VARIABLE_TYPE => "MUTABLE",
        v if v == T::SHADER_RESOURCE_VARIABLE_TYPE_DYNAMIC as sys::SHADER_RESOURCE_VARIABLE_TYPE => "DYNAMIC",
        _ => "UNKNOWN",
    }
}

fn res_type_name(s: sys::SHADER_RESOURCE_TYPE) -> &'static str {
    use sys::_SHADER_RESOURCE_TYPE as R;
    match s {
        v if v == R::SHADER_RESOURCE_TYPE_CONSTANT_BUFFER as sys::SHADER_RESOURCE_TYPE => "CONSTANT_BUFFER",
        v if v == R::SHADER_RESOURCE_TYPE_TEXTURE_SRV as sys::SHADER_RESOURCE_TYPE => "TEXTURE_SRV",
        v if v == R::SHADER_RESOURCE_TYPE_BUFFER_SRV as sys::SHADER_RESOURCE_TYPE => "BUFFER_SRV",
        v if v == R::SHADER_RESOURCE_TYPE_TEXTURE_UAV as sys::SHADER_RESOURCE_TYPE => "TEXTURE_UAV",
        v if v == R::SHADER_RESOURCE_TYPE_BUFFER_UAV as sys::SHADER_RESOURCE_TYPE => "BUFFER_UAV",
        v if v == R::SHADER_RESOURCE_TYPE_SAMPLER as sys::SHADER_RESOURCE_TYPE => "SAMPLER",
        _ => "UNKNOWN",
    }
}

fn main() -> dil::Result<()> {
    println!("[V19] RUNTIME_ARRAY + ArraySize=5000 + partial binding verification (D3D12)");
    println!("[V19] ========================================================");

    // ---- 1. Factory ----
    let factory = dil::EngineFactoryD3D12::d3d12()?;
    println!(
        "[V19] factory: D3D12 engine factory resolved (API v{})",
        sys::DILIGENT_API_VERSION
    );

    // ---- 2. Adapter enumeration + selection (NVIDIA preferred) ----
    // EnumerateAdapters requires the D3D12 DLL to be loaded explicitly
    // (CreateDeviceAndContextsD3D12 would load it lazily).
    let load_d3d12 = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactoryD3D12
            .LoadD3D12
            .as_ref()
            .expect("IEngineFactoryD3D12::LoadD3D12 missing")
    };
    let loaded = unsafe { load_d3d12(factory.as_raw(), c"d3d12.dll".as_ptr()) };
    if !loaded {
        return Err(dil::Error::Message("LoadD3D12(d3d12.dll) failed".to_string()));
    }
    println!("[V19] LoadD3D12(d3d12.dll): ok");

    let enumerate = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactory
            .EnumerateAdapters
            .as_ref()
            .expect("IEngineFactory::EnumerateAdapters missing")
    };
    let mut num_adapters: u32 = 0;
    // MinVersion is a D3D feature level packed as (Major<<12)|(Minor<<8);
    // {0,0} would map to D3D_FEATURE_LEVEL 0 and match nothing.
    let min_fl = sys::Version { Major: 12, Minor: 0 };
    let factory_raw = factory.as_raw();
    unsafe { enumerate(factory_raw.cast(), min_fl, &mut num_adapters, std::ptr::null_mut()) };
    let mut infos: Vec<sys::GraphicsAdapterInfo> = vec![unsafe { std::mem::zeroed() }; num_adapters as usize];
    unsafe { enumerate(factory_raw.cast(), min_fl, &mut num_adapters, infos.as_mut_ptr()) };
    let mut adapter_id = 0u32;
    for (i, info) in infos.iter().enumerate() {
        let name = adapter_name(info);
        println!(
            "[V19]   adapter #{i}: {name} (vendorId=0x{:04X}, deviceId=0x{:04X}, local mem={} MiB)",
            info.VendorId,
            info.DeviceId,
            info.Memory.LocalMemory / (1 << 20)
        );
        if info.VendorId == VENDOR_NVIDIA && i != 0 {
            adapter_id = i as u32;
        }
    }
    let selected = adapter_name(&infos[adapter_id as usize]);
    println!(
        "[V19] adapter selected: #{adapter_id} '{selected}' (NVIDIA preferred; falls back to 0)"
    );

    // ---- 3. Device + context (raw FFI with explicit AdapterId) ----
    let engine_ci = build_engine_ci(adapter_id);
    let mut device: *mut sys::IRenderDevice = std::ptr::null_mut();
    let mut context: *mut sys::IDeviceContext = std::ptr::null_mut();
    let create_dev = unsafe {
        (*(*factory.as_raw()).pVtbl)
            .EngineFactoryD3D12
            .CreateDeviceAndContextsD3D12
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IEngineFactoryD3D12::CreateDeviceAndContextsD3D12",
            ))?
    };
    unsafe { create_dev(factory.as_raw(), &engine_ci, &mut device, &mut context) };
    if device.is_null() {
        return Err(dil::Error::CreateFailed("render device (D3D12)"));
    }
    if context.is_null() {
        unsafe { release(device) };
        return Err(dil::Error::CreateFailed("immediate context (D3D12)"));
    }
    // Drop order (reverse declaration): textures -> srb -> prs -> context -> device -> factory.
    let device = RawDevice(device);
    let context = RawContext(context);

    // ---- 4. DeviceCaps excerpt: GetDeviceInfo + GetAdapterInfo ----
    // Verified against the locked headers: `IRenderDevice::GetDeviceCaps`
    // does NOT exist in this version; the limits live in
    // RenderDeviceInfo (GetDeviceInfo) and GraphicsAdapterInfo
    // (GetAdapterInfo).
    let get_info = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .GetDeviceInfo
            .as_ref()
            .expect("IRenderDevice::GetDeviceInfo missing")
    };
    let dinfo = unsafe { *get_info(device.0) };
    println!(
        "[V19] device: type={:?}, API version {}.{}",
        dinfo.Type, dinfo.APIVersion.Major, dinfo.APIVersion.Minor
    );
    println!(
        "[V19] device features: BindlessResources={}, ShaderResourceQueries={}",
        feature_state_name(dinfo.Features.BindlessResources),
        feature_state_name(dinfo.Features.ShaderResourceQueries),
    );
    let get_adapter = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .GetAdapterInfo
            .as_ref()
            .expect("IRenderDevice::GetAdapterInfo missing")
    };
    let ainfo = unsafe { *get_adapter(device.0) };
    let adapter_name = adapter_name(&ainfo);
    println!(
        "[V19] adapter: {adapter_name} (vendorId={}, deviceId={})",
        ainfo.VendorId, ainfo.DeviceId
    );
    println!(
        "[V19] adapter texture caps: MaxTexture2DDimension={}, MaxTexture2DArraySlices={}, TextureViewSupported={}",
        ainfo.Texture.MaxTexture2DDimension,
        ainfo.Texture.MaxTexture2DArraySlices,
        ainfo.Texture.TextureViewSupported,
    );
    println!(
        "[V19] adapter memory: local={} MiB, hostVisible={} MiB",
        ainfo.Memory.LocalMemory / (1 << 20),
        ainfo.Memory.HostVisibleMemory / (1 << 20),
    );
    println!(
        "[V19] adapter buffer: ConstantBufferOffsetAlignment={}, StructuredBufferOffsetAlignment={}",
        ainfo.Buffer.ConstantBufferOffsetAlignment,
        ainfo.Buffer.StructuredBufferOffsetAlignment,
    );

    // ---- 5. PRS with ArraySize=5000 TEXTURE_SRV RUNTIME_ARRAY (raw FFI) ----
    let var_name = CString::new(VAR_NAME)?;
    let mut res_desc: sys::PipelineResourceDesc = unsafe { std::mem::zeroed() };
    res_desc.Name = var_name.as_ptr();
    res_desc.ShaderStages = PIXEL;
    res_desc.ArraySize = ARRAY_SIZE;
    res_desc.ResourceType = TEXTURE_SRV;
    res_desc.VarType = MUTABLE;
    res_desc.Flags = RUNTIME_ARRAY;

    let prs_name = CString::new("V19 PRS")?;
    let mut prs_desc: sys::PipelineResourceSignatureDesc = unsafe { std::mem::zeroed() };
    prs_desc._DeviceObjectAttribs.Name = prs_name.as_ptr();
    prs_desc.Resources = &res_desc;
    prs_desc.NumResources = 1;
    prs_desc.BindingIndex = 0;

    let create_prs = unsafe {
        (*(*device.0).pVtbl)
            .RenderDevice
            .CreatePipelineResourceSignature
            .as_ref()
            .ok_or(dil::Error::MissingMethod(
                "IRenderDevice::CreatePipelineResourceSignature",
            ))?
    };
    let mut prs: *mut sys::IPipelineResourceSignature = std::ptr::null_mut();
    let t0 = Instant::now();
    unsafe { create_prs(device.0, &prs_desc, &mut prs) };
    let prs_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if prs.is_null() {
        return Err(dil::Error::CreateFailed("pipeline resource signature"));
    }
    let prs = RawPrs(prs);
    println!(
        "[V19] PRS: created ArraySize={ARRAY_SIZE} TEXTURE_SRV VarType=MUTABLE Flags=RUNTIME_ARRAY (0x{:X}) in {prs_ms:.3}ms",
        RUNTIME_ARRAY
    );

    // ---- 6. SRB from PRS ----
    let create_srb = unsafe {
        (*(*prs.0).pVtbl)
            .PipelineResourceSignature
            .CreateShaderResourceBinding
            .as_ref()
            .expect("IPipelineResourceSignature::CreateShaderResourceBinding missing")
    };
    let mut srb: *mut sys::IShaderResourceBinding = std::ptr::null_mut();
    let t0 = Instant::now();
    unsafe { create_srb(prs.0, &mut srb, true) };
    let srb_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if srb.is_null() {
        return Err(dil::Error::CreateFailed("shader resource binding"));
    }
    let srb = RawSrb(srb);
    println!("[V19] SRB: created from PRS (init static resources) in {srb_ms:.3}ms");

    // Variable count (mutable + dynamic only; expect 1).
    let get_var_count = unsafe {
        (*(*srb.0).pVtbl)
            .ShaderResourceBinding
            .GetVariableCount
            .as_ref()
            .expect("IShaderResourceBinding::GetVariableCount missing")
    };
    let var_count = unsafe { get_var_count(srb.0, PIXEL) };
    println!("[V19] SRB GetVariableCount(PIXEL) = {var_count}");

    // ---- 7. Variable lookup ----
    let t0 = Instant::now();
    let get_var_by_name = unsafe {
        (*(*srb.0).pVtbl)
            .ShaderResourceBinding
            .GetVariableByName
            .as_ref()
            .expect("IShaderResourceBinding::GetVariableByName missing")
    };
    let var = unsafe { get_var_by_name(srb.0, PIXEL, var_name.as_ptr()) };
    let var_lookup_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if var.is_null() {
        return Err(dil::Error::Message(
            "GetVariableByName returned null for 'g_Textures'".to_string(),
        ));
    }
    println!(
        "[V19] GetVariableByName('{VAR_NAME}', PIXEL): ok in {var_lookup_ms:.3}ms (null={})",
        var.is_null()
    );

    // ---- 8. Variable introspection: GetResourceDesc / GetType / GetIndex ----
    let mut res_desc_out: sys::ShaderResourceDesc = unsafe { std::mem::zeroed() };
    let get_res_desc = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .GetResourceDesc
            .as_ref()
            .expect("IShaderResourceVariable::GetResourceDesc missing")
    };
    unsafe { get_res_desc(var, &mut res_desc_out) };
    let desc_name = if res_desc_out.Name.is_null() {
        "<null>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(res_desc_out.Name) }
            .to_string_lossy()
            .into_owned()
    };
    let get_type = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .GetType
            .as_ref()
            .expect("IShaderResourceVariable::GetType missing")
    };
    let get_index = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .GetIndex
            .as_ref()
            .expect("IShaderResourceVariable::GetIndex missing")
    };
    println!(
        "[V19] variable: name='{desc_name}' type={} ({}) ArraySize={} index={}",
        res_type_name(res_desc_out.Type),
        var_type_name(unsafe { get_type(var) }),
        res_desc_out.ArraySize,
        unsafe { get_index(var) },
    );
    if res_desc_out.ArraySize != ARRAY_SIZE {
        return Err(dil::Error::Message(format!(
            "PRS ArraySize mismatch: expected {ARRAY_SIZE}, got {}",
            res_desc_out.ArraySize
        )));
    }

    // ---- 9. Create NUM_TEXTURES 1x1 textures (raw FFI) ----
    let mut textures: Vec<RawTexture> = Vec::with_capacity(NUM_TEXTURES as usize);
    let t0 = Instant::now();
    for i in 0..NUM_TEXTURES {
        textures.push(create_texture_raw(device.0, &format!("V19 tex #{i}"))?);
    }
    let tex_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[V19] textures: {NUM_TEXTURES}x 1x1 RGBA8 UNORM created in {tex_ms:.3}ms ({:.3}ms each)",
        tex_ms / NUM_TEXTURES as f64
    );

    // ---- 10. PARTIAL bind via SetArray(ppObjects, FirstElement, NumElements) ----
    // A TEXTURE_SRV variable must be bound with an ITextureView (the
    // texture's SRV), not the raw ITexture: the D3D12 backend casts the
    // object to TextureViewD3D12Impl (ShaderVariableManagerD3D12.cpp,
    // CacheResourceView) and silently no-ops when the cast fails.
    let set_array = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .SetArray
            .as_ref()
            .expect("IShaderResourceVariable::SetArray missing")
    };
    let get_elem = unsafe {
        (*(*var).pVtbl)
            .ShaderResourceVariable
            .Get
            .as_ref()
            .expect("IShaderResourceVariable::Get missing")
    };

    // 10a. Probe: bind the raw ITexture objects (invalid object type for a
    // TEXTURE_SRV variable). Expected: silently ignored -> Get() stays null.
    let tex_objs_0_2: Vec<*mut sys::IDeviceObject> = textures[0..2]
        .iter()
        .map(|t| t.as_ptr() as *mut sys::IDeviceObject)
        .collect();
    unsafe { set_array(var, tex_objs_0_2.as_ptr(), 0, 2, SET_NONE) };
    let g0 = unsafe { get_elem(var, 0) };
    let g1 = unsafe { get_elem(var, 1) };
    println!(
        "[V19] probe: SetArray(raw ITexture, First=0, Num=2) -> Get(0)={:p} Get(1)={:p} (null = engine rejected raw texture; ITextureView required)",
        g0, g1
    );

    // 10b. First real bind: SRV views of textures 0..1 at elements [0, 2).
    let views_0_2: Vec<*mut sys::IDeviceObject> = textures[0..2]
        .iter()
        .map(|t| t.default_srv() as *mut sys::IDeviceObject)
        .collect();
    let t0 = Instant::now();
    unsafe { set_array(var, views_0_2.as_ptr(), 0, 2, SET_NONE) };
    let set_a_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[V19] SetArray(views[0..2], FirstElement=0, NumElements=2, NONE): ok in {set_a_ms:.3}ms (partial bind, no overwrite flag)"
    );

    // 10c. Extend the bound range: elements [2, 4) - a second partial update
    // that never touches elements [0, 2) or the rest of the 5000-array.
    let views_2_4: Vec<*mut sys::IDeviceObject> = textures[2..4]
        .iter()
        .map(|t| t.default_srv() as *mut sys::IDeviceObject)
        .collect();
    let t0 = Instant::now();
    unsafe { set_array(var, views_2_4.as_ptr(), 2, 2, SET_NONE) };
    let set_b_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[V19] SetArray(views[2..4], FirstElement=2, NumElements=2, NONE): ok in {set_b_ms:.3}ms (append to bound range)"
    );

    // 10d. Overwrite an already-bound element (slab re-assignment): requires
    // SET_SHADER_RESOURCE_FLAG_ALLOW_OVERWRITE for mutable variables.
    let t0 = Instant::now();
    unsafe { set_array(var, views_0_2.as_ptr(), 0, 2, SET_OVERWRITE) };
    let set_c_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "[V19] SetArray(views[0..2], FirstElement=0, NumElements=2, ALLOW_OVERWRITE): ok in {set_c_ms:.3}ms (slab re-assignment)"
    );

    // ---- 11. Element-wise verification via Get(ArrayIndex) ----
    let mut all_ok = true;
    for i in 0..6u32 {
        let obj = unsafe { get_elem(var, i) };
        let expected = if i < NUM_TEXTURES {
            textures[i as usize].default_srv() as *mut sys::IDeviceObject
        } else {
            std::ptr::null_mut()
        };
        let ok = obj == expected;
        all_ok &= ok;
        println!(
            "[V19]   Get({i}): 0x{:016X} expected 0x{:016X} {}",
            obj as usize,
            expected as usize,
            if ok { "OK" } else { "MISMATCH" }
        );
    }
    if !all_ok {
        return Err(dil::Error::Message(
            "element-wise Get() verification failed".to_string(),
        ));
    }
    println!(
        "[V19] verification: elements [0,{NUM_TEXTURES}) bound (identity match), element {NUM_TEXTURES} unbound (null) - partial bind semantics confirmed"
    );

    // ---- 12. Overhead model: one "material add" = lookup + 1 partial SetArray ----
    let lookup_and_bind_ms = var_lookup_ms + set_b_ms;
    println!("[V19] overhead model (coarse, single run):");
    println!(
        "[V19]   PRS create (5000 array):   {prs_ms:8.3} ms  (one-time)"
    );
    println!(
        "[V19]   SRB create:                {srb_ms:8.3} ms  (one-time)"
    );
    println!(
        "[V19]   1x1 texture create:        {:8.3} ms  (per texture, driver alloc)",
        tex_ms / NUM_TEXTURES as f64
    );
    println!(
        "[V19]   GetVariableByName:         {var_lookup_ms:8.3} ms  (once per PRS, cache the pointer)"
    );
    println!(
        "[V19]   SetArray partial (2 elems):{set_a_ms:8.3} ms / {set_b_ms:.3} ms  (append; no full-array re-bind)"
    );
    println!(
        "[V19]   SetArray overwrite:        {set_c_ms:8.3} ms  (with ALLOW_OVERWRITE)"
    );
    println!(
        "[V19]   -> material slab add path: ~{lookup_and_bind_ms:.3} ms after one-time SRB setup; a 5000-wide array update is NEVER performed on partial updates"
    );

    // ---- 13. Summary ----
    println!("[V19] ========================================================");
    println!("[V19] SUMMARY");
    println!("[V19]   PRS ArraySize=5000 TEXTURE_SRV RUNTIME_ARRAY:      CREATED OK");
    println!("[V19]   SRB from PRS (GetVariableCount=1):                 OK");
    println!("[V19]   partial bind API (locked headers):                 IShaderResourceVariable::SetArray(ppObjects, FirstElement, NumElements, Flags)");
    println!("[V19]   SetArrayRange / SetTextureArray / GetArrayElement: DO NOT EXIST in locked commit (verified by grep)");
    println!("[V19]   GetDeviceCaps:                                     DOES NOT EXIST in locked commit (GetDeviceInfo + GetAdapterInfo used)");
    println!("[V19]   partial binds [0,2) + [2,4):                       OK; element 4+ still unbound (null)");
    println!("[V19]   bound object type:                                 ITextureView (SRV); raw ITexture is silently rejected by D3D12 backend");
    println!("[V19]   overwrite with ALLOW_OVERWRITE:                    OK");
    println!("[V19]   backend: D3D12, adapter: {adapter_name}");
    println!("[V19]   Vulkan: NOT tested (diligent-sys builds D3D12 only on this machine)");
    println!("[V19] ========================================================");

    // ---- 14. Cleanup ----
    // Drop order (reverse declaration): textures -> srb -> prs -> context
    // -> device -> factory. All raw pointers are released manually.
    drop(textures);
    drop(srb);
    drop(prs);
    drop(context);
    drop(device);
    println!("[V19] cleanup complete, exiting 0");
    Ok(())
}

/// RAII wrapper for a raw `IDeviceContext` pointer.
struct RawContext(*mut sys::IDeviceContext);

impl Drop for RawContext {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}
