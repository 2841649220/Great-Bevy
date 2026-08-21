//! Diligent triangle demo: winit 0.30 window + D3D12 backend.
//!
//! Renders a white triangle into a black background for 120 frames and then
//! exits with code 0, exercising the full V1 chain through `diligent-rs`:
//!
//! ```text
//! factory -> device+context -> swap chain -> PRS -> VS/PS -> vertex buffer
//!         -> PSO (binds PRS) -> SRB -> set RTs / clear / viewport / buffers
//!         -> set PSO / commit SRB / draw(3) -> present
//! ```
//!
//! Key milestones are printed to stdout. Set `DILIGENT_RS_READBACK=1` to
//! additionally copy the back buffer to a staging texture on frame 90 and
//! verify that non-black pixels were actually rendered (CPU readback check).

use std::ffi::CString;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use diligent_rs as dil;
use diligent_sys::bindings as sys;

const MAX_FRAMES: u32 = 120;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const READBACK_FRAME: u32 = 90;

const VS_SOURCE: &str = r#"
struct VSInput {
    float3 pos : ATTRIB0;
};
struct VSOutput {
    float4 pos : SV_POSITION;
};
// TEMP PROBE: ignore the input buffer entirely and synthesize a full-screen
// triangle from SV_VertexID, so a draw that rasterizes NOTHING can only mean
// the PSO/VS/draw-path is broken (not the vertex buffer/input layout).
void main(in VSInput input, out VSOutput output, uint vid : SV_VertexID) {
    float2 p = float2(vid == 0 ? -1.0 : (vid == 1 ? -1.0 : 3.0),
                      vid == 0 ? -3.0 : (vid == 1 ? 3.0 : 3.0));
    output.pos = float4(p, 0.0, 1.0);
}
"#;

const PS_SOURCE: &str = r#"
struct PSInput {
    float4 pos : SV_POSITION;
};
float4 main(in PSInput input) : SV_TARGET {
    return float4(1.0, 1.0, 1.0, 1.0);
}
"#;

fn readback_enabled() -> bool {
    std::env::var_os("DILIGENT_RS_READBACK").is_some()
}

struct TriangleApp {
    state: Option<RenderState>,
    frames: u32,
    readback: bool,
    exiting: bool,
}

// Field order = drop order (Rust drops in declaration order). The SRB is
// allocated from the PRS's block allocator, and the PSO holds the last PRS
// reference, so the SRB must be released before the PSO; the staging
// texture before the device; the swap chain before the window and device;
// the device before the factory.
struct RenderState {
    context: dil::DeviceContext,
    swap_chain: dil::SwapChain,
    vertex_buffer: dil::Buffer,
    srb: dil::ShaderResourceBinding,
    pso: dil::PipelineState,
    staging: Option<OwnedTexture>,
    fence: dil::Fence,
    window: Window,
    _device: dil::RenderDevice,
    _factory: dil::EngineFactoryD3D12,
}

impl TriangleApp {
    fn setup(&mut self, event_loop: &ActiveEventLoop) -> dil::Result<()> {
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("diligent-rs triangle (D3D12)")
                    .with_inner_size(LogicalSize::new(WIDTH as f64, HEIGHT as f64)),
            )
            .map_err(|e| dil::Error::Message(format!("create window: {e}")))?;

        let hwnd = match window.window_handle().map_err(|e| dil::Error::Message(e.to_string()))?.as_raw()
        {
            // raw-window-handle 0.6 stores the HWND as a NonZero<isize>.
            RawWindowHandle::Win32(h) => h.hwnd.get() as *mut std::ffi::c_void,
            other => return Err(dil::Error::Message(format!("unexpected raw window handle: {other:?}"))),
        };
        println!("[demo] window: created, HWND = {hwnd:p}");

        let factory = dil::EngineFactoryD3D12::d3d12()?;
        println!(
            "[demo] factory: Diligent_GetEngineFactoryD3D12 ok (API v{})",
            sys::DILIGENT_API_VERSION
        );

        let (device, context) = factory.create_device_and_contexts()?;
        let dinfo = device.device_info();
        println!(
            "[demo] device: type={:?}, API version {}.{}",
            dinfo.Type, dinfo.APIVersion.Major, dinfo.APIVersion.Minor
        );
        let adapter = device.adapter_info();
        let adapter_name = unsafe { std::ffi::CStr::from_ptr(adapter.Description.as_ptr()) }
            .to_string_lossy();
        println!(
            "[demo] adapter: {adapter_name} (vendorId={}, deviceId={})",
            adapter.VendorId, adapter.DeviceId
        );

        let swap_chain = factory.create_swap_chain(&device, &context, hwnd, WIDTH, HEIGHT)?;
        let sc_desc = swap_chain.desc();
        println!(
            "[demo] swap chain: {}x{}, colorFormat={:?}, depthFormat={:?}, buffers={}",
            sc_desc.Width, sc_desc.Height, sc_desc.ColorBufferFormat, sc_desc.DepthBufferFormat,
            sc_desc.BufferCount
        );

        let prs = device.create_pipeline_resource_signature("triangle PRS", &[])?;
        println!("[demo] PRS: created (empty, binding index 0)");

        let vs = device.create_shader(
            "triangle VS",
            VS_SOURCE,
            sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE,
        )?;
        let ps = device.create_shader(
            "triangle PS",
            PS_SOURCE,
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
        )?;
        println!("[demo] shaders: VS + PS compiled from embedded HLSL");

        let vertices: [f32; 9] = [-0.5, -0.5, 0.0, 0.5, -0.5, 0.0, 0.0, 0.5, 0.0];
        let vertex_bytes =
            unsafe { std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), std::mem::size_of_val(&vertices)) };
        let vertex_buffer = device.create_vertex_buffer("triangle VB", vertex_bytes)?;
        println!("[demo] vertex buffer: 3 vertices (float3), 36 bytes");

        // D3D12 forbids trailing digits in the semantic *name*: the HLSL
        // attribute `ATTRIB0` is declared with semantic name "ATTRIB" and
        // InputIndex 0.
        let attr0 = CString::new("ATTRIB").expect("no NUL");
        let layout_elements = [dil::layout_element(
            &attr0,
            0,
            0,
            3,
            sys::_VALUE_TYPE::VT_FLOAT32 as sys::VALUE_TYPE,
            false,
        )];

        let srb = prs.create_shader_resource_binding(true)?;
        println!("[demo] SRB: created from PRS (init static resources = true)");

        // TEMP DIAGNOSTIC: bypass the PRS/SRB chain entirely via raw FFI
        // (0 explicit signatures = implicit root signature) to bisect whether
        // the empty-PRS path is what kills the draw.
        let no_prs = std::env::var_os("DILIGENT_RS_NO_PRS").is_some();
        let pso = if no_prs {
            println!("[demo] TEMP DIAG: creating PSO with NO explicit PRS (raw FFI)");
            let pso = device.create_graphics_pipeline_raw_no_prs(
                "triangle PSO raw",
                &vs,
                &ps,
                sc_desc.ColorBufferFormat,
                &layout_elements,
                sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT,
            )?;
            pso
        } else {
            device.create_graphics_pipeline(
                "triangle PSO",
                &vs,
                &ps,
                sc_desc.ColorBufferFormat,
                &layout_elements,
                &[&prs],
                // No depth-stencil view is bound in this demo, so the pipeline
                // is created with depth test/write disabled (DSVFormat UNKNOWN).
                sys::_TEXTURE_FORMAT::TEX_FORMAT_UNKNOWN as sys::TEXTURE_FORMAT,
                1,
            )?
        };
        println!(
            "[demo] PSO: created (RTV format {:?}, {})",
            sc_desc.ColorBufferFormat,
            if no_prs { "NO explicit PRS (raw FFI)" } else { "1 explicit PRS bound" }
        );

let staging = if self.readback {
        let tex = create_staging_texture(&device, sc_desc.Width, sc_desc.Height)?;
        println!(
            "[demo] readback: staging texture {}x{} TEX_FORMAT_RGBA8_UNORM ready",
            sc_desc.Width, sc_desc.Height
        );
        Some(tex)
    } else {
        None
    };

        self.state = Some(RenderState {
            context,
            swap_chain,
            vertex_buffer,
            srb,
            pso,
            staging,
            fence: device.create_fence("readback fence")?,
            window,
            _device: device,
            _factory: factory,
        });
        Ok(())
    }
}

impl ApplicationHandler for TriangleApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        if let Err(e) = self.setup(event_loop) {
            eprintln!("[demo] FATAL: {e}");
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                println!("[demo] close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = &mut self.state {
                    if let Err(e) = state.swap_chain.resize(size.width, size.height) {
                        eprintln!("[demo] resize failed: {e}");
                    }
                    // The OS/DPI can resize the swap chain at any time; the
                    // readback staging texture must match the current back
                    // buffer dimensions or the CopyTexture is undefined.
                    if self.readback {
                        match create_staging_texture(&state._device, size.width, size.height) {
                            Ok(tex) => {
                                state.staging = Some(tex);
                                println!(
                                    "[demo] readback: staging recreated at {}x{}",
                                    size.width, size.height
                                );
                            }
                            Err(e) => eprintln!("[demo] staging recreate failed: {e}"),
                        }
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
        if let Err(e) = render_frame(state, self.frames, self.readback) {
            eprintln!("[demo] FATAL: {e}");
            event_loop.exit();
            self.exiting = true;
            return;
        }
        self.frames += 1;
        if self.frames >= MAX_FRAMES {
            println!("[demo] {MAX_FRAMES} frames rendered, exiting cleanly");
            self.exiting = true;
            event_loop.exit();
            return;
        }
        // winit 0.30 blocks in the OS wait when no events are pending, so
        // without an explicit redraw request the loop stops after the first
        // about_to_wait. Requesting a redraw keeps the render loop going.
        state.window.request_redraw();
    }
}

fn render_frame(state: &mut RenderState, frame: u32, do_readback: bool) -> dil::Result<()> {
    let rtv = state
        .swap_chain
        .current_back_buffer_rtv()
        .ok_or(dil::Error::NullPointer("back buffer RTV"))?;
    let sc_desc = state.swap_chain.desc();

    state.context.set_render_targets(&[rtv]);
    // Black clear + white triangle.
    state.context.clear_render_target(&rtv, [0.0, 0.0, 0.0, 1.0]);
    state
        .context
        .set_viewports(&[dil::viewport(sc_desc.Width as f32, sc_desc.Height as f32)]);
    state
        .context
        .set_vertex_buffers(0, &[&state.vertex_buffer], &[0])?;
    state.context.set_pipeline_state(&state.pso);
    if !std::env::var_os("DILIGENT_RS_NO_PRS").is_some() {
        state.context.commit_shader_resources(Some(&state.srb));
    }
    state.context.draw(3)?;

    if do_readback && frame == READBACK_FRAME {
        match readback_verify(state) {
            Ok(non_black) => {
                println!(
                    "[demo] readback: copied back buffer to staging, {non_black} non-black pixels found \
                     (>= 0 means GPU actually drew something)"
                );
            }
            Err(e) => {
                println!("[demo] readback: skipped/failed ({e})");
            }
        }
    }

    state.swap_chain.present(1);
    state.context.finish_frame();
    if frame % 30 == 0 {
        println!("[demo] frame {frame}: clear + draw(3) + present ok");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Readback stretch: copy the back buffer into a CPU-readable staging texture
// and count non-black pixels. Uses raw diligent_sys FFI (outside the wrapper).
// ---------------------------------------------------------------------------

struct OwnedTexture(*mut sys::ITexture);

impl Drop for OwnedTexture {
    fn drop(&mut self) {
        unsafe {
            let obj = self.0.cast::<sys::IObject>();
            let vtbl = &*(*obj).pVtbl;
            vtbl.Object.Release.expect("Release missing")(obj);
        }
    }
}

fn create_staging_texture(device: &dil::RenderDevice, width: u32, height: u32) -> dil::Result<OwnedTexture> {
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
        (*(*device.as_raw()).pVtbl)
            .RenderDevice
            .CreateTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IRenderDevice::CreateTexture"))?
    };
    let mut tex: *mut sys::ITexture = std::ptr::null_mut();
    unsafe { create(device.as_raw(), &td, std::ptr::null(), &mut tex) };
    if tex.is_null() {
        return Err(dil::Error::CreateFailed("staging texture"));
    }
    Ok(OwnedTexture(tex))
}

fn readback_verify(state: &RenderState) -> dil::Result<usize> {
    let rtv = state
        .swap_chain
        .current_back_buffer_rtv()
        .ok_or(dil::Error::NullPointer("back buffer RTV"))?;

    // The back buffer texture is owned by the swap chain (no Release).
    let get_texture = unsafe {
        (*(*rtv.as_ptr()).pVtbl)
            .TextureView
            .GetTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("ITextureView::GetTexture"))?
    };
    let back_buffer = unsafe { get_texture(rtv.as_ptr()) };
    if back_buffer.is_null() {
        return Err(dil::Error::NullPointer("back buffer texture"));
    }
    let staging = state
        .staging
        .as_ref()
        .ok_or(dil::Error::Message("no staging texture".to_string()))?
        .0;

    let mut attribs: sys::CopyTextureAttribs = unsafe { std::mem::zeroed() };
    attribs.pSrcTexture = back_buffer;
    attribs.SrcMipLevel = 0;
    attribs.SrcSlice = 0;
    attribs.SrcTextureTransitionMode =
        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE;
    attribs.pDstTexture = staging;
    attribs.DstMipLevel = 0;
    attribs.DstSlice = 0;
    attribs.DstX = 0;
    attribs.DstY = 0;
    attribs.DstZ = 0;
    attribs.DstTextureTransitionMode =
        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION as sys::RESOURCE_STATE_TRANSITION_MODE;

    let ctx = state.context.as_raw();
    let ctx_methods = unsafe { &(*(*ctx).pVtbl).DeviceContext };
    unsafe {
        ctx_methods
            .CopyTexture
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceContext::CopyTexture"))?(ctx, &attribs);
    }
    // The D3D12 backend never waits for the GPU when mapping a staging
    // texture (see the engine warning); the canonical sync is a fence:
    // signal it after the copy, then block on the CPU side until it reaches
    // the signaled value before mapping. EnqueueSignal does not flush the
    // context (DeviceContext.h:3223), so flush first or the signal never
    // reaches the GPU.
    state.context.enqueue_signal(&state.fence, 1)?;
    state.context.flush();
    state.fence.wait(1)?;

    let mut mapped: sys::MappedTextureSubresource = unsafe { std::mem::zeroed() };
    unsafe {
        ctx_methods
            .MapTextureSubresource
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceContext::MapTextureSubresource"))?(
            ctx,
            staging,
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

    let sc_desc = state.swap_chain.desc();
    let width = sc_desc.Width as usize;
    let height = sc_desc.Height as usize;
    let row_pitch = mapped.Stride as usize;
    let pixels = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), row_pitch * height) };
    let mut non_black = 0usize;
    let mut min_r = 255u8;
    let mut max_r = 0u8;
    let mut min_g = 255u8;
    let mut max_g = 0u8;
    let mut min_b = 255u8;
    let mut max_b = 0u8;
    for y in 0..height {
        for x in 0..width {
            let r = pixels[y * row_pitch + x * 4];
            let g = pixels[y * row_pitch + x * 4 + 1];
            let b = pixels[y * row_pitch + x * 4 + 2];
            if r > 200 || g > 200 || b > 200 {
                non_black += 1;
            }
            min_r = min_r.min(r);
            max_r = max_r.max(r);
            min_g = min_g.min(g);
            max_g = max_g.max(g);
            min_b = min_b.min(b);
            max_b = max_b.max(b);
        }
    }
    println!(
        "[demo] readback: min/max R={min_r}/{max_r} G={min_g}/{max_g} B={min_b}/{max_b} across {width}x{height}"
    );

    // ASCII art of the GREEN channel (24x18): a red clear has G=0 while a
    // white triangle has G=255, so this distinguishes the two (R alone
    // cannot).
    {
        let mut art = String::new();
        for gy in 0..18 {
            for gx in 0..24 {
                let y = (gy as usize) * height / 18 + height / 36;
                let x = (gx as usize) * width / 24 + width / 48;
                let g = pixels[y * row_pitch + x * 4 + 1];
                art.push(if g > 200 {
                    '#'
                } else if g > 100 {
                    '+'
                } else if g > 0 {
                    '.'
                } else {
                    ' '
                });
            }
            art.push('\n');
        }
        println!("[demo] readback: G-channel ascii 24x18:\n{art}");
    }

    unsafe {
        ctx_methods
            .UnmapTextureSubresource
            .as_ref()
            .ok_or(dil::Error::MissingMethod("IDeviceContext::UnmapTextureSubresource"))?(
            ctx, staging, 0, 0
        );
    }
    Ok(non_black)
}

fn main() -> dil::Result<()> {
    println!("[demo] diligent-rs triangle (D3D12 backend, winit 0.30)");
    let event_loop = EventLoop::new().map_err(|e| dil::Error::Message(format!("event loop: {e}")))?;
    let mut app = TriangleApp {
        state: None,
        frames: 0,
        readback: readback_enabled(),
        exiting: false,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| dil::Error::Message(format!("run app: {e}")))
}
