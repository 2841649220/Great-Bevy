//! Diligent command recording + render-pass/framebuffer creation (M1-3).
//!
//! This module is the Diligent side of the render-execution path:
//!
//! * raw `IDeviceContext` command helpers the M1-1 wrapper does not cover
//!   (draw / draw_indexed / indirect / multi-draw / index buffer / scissor /
//!   stencil / blend factors / `BeginRenderPass` / debug groups - every
//!   signature verified against the locked `DeviceContext.h` and the
//!   generated bindings);
//! * render-pass + framebuffer creation through the raw `IRenderDevice`
//!   vtables (`CreateRenderPass` / `CreateFramebuffer` - the diligent-rs
//!   wrapper has no RenderPass/Framebuffer support), with a bounded cache
//!   (render passes and framebuffers are engine objects; per-frame creation
//!   would churn the engine);
//! * on-demand derivation of attachment views of the right type
//!   (`TEXTURE_VIEW_RENDER_TARGET` / `TEXTURE_VIEW_DEPTH_STENCIL`): the M1-2
//!   view-type heuristic picks the bind-group-side view (SRV/UAV) when a
//!   texture is both sampled and rendered to, and Diligent needs a concrete
//!   view type per use;
//! * the present-mode -> `Present(SyncInterval)` mapping (V12 finding: this
//!   Diligent version has no `PRESENT_MODE`/`SWAP_CHAIN_STATUS`
//!   enumerations - `SwapChain.h` only exposes `Present(Uint32 SyncInterval)`
//!   plus `Resize`/`SetMaximumFrameLatency`).
//!
//! # Failure policy
//!
//! The begin path returns `Err(reason)` - the caller (`RenderContext`)
//! falls back to the transition wgpu encoder for that pass (warn, logged
//! once per reason). `None`-based degradation is used for per-command
//! lookups (unresolved buffer/view) with debug logs.
//!
//! # TODO-REMOVE-M1-4
//!
//! * MSAA resolve targets: the resolve target is wired into the subpass
//!   (`SubpassDesc.pResolveAttachments`; the engine emits a D3D12
//!   `D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_RESOLVE` - see
//!   DeviceContextD3D12Impl.cpp:1499-1543, `PreserveResolveSource` from the
//!   color attachment's store op). The M1-2 "renders into the sample
//!   texture, resolve ignored" note is obsolete since M1-4b-1 (MSAA textures
//!   are created with their real `SampleCount`).
//! * Attachment views derived here cover the whole texture (sub-range
//!   attachment views are not representable from the wgpu descriptor).
//! * The framebuffer cache keys on the RESOLVED diligent attachment-view
//!   pointers, not the wgpu view addresses (the swap chain re-registers a
//!   fresh back-buffer RTV under the same wgpu dummy address every frame -
//!   an address-keyed cache would silently reuse the first frame's
//!   framebuffer). Swap-chain-targeted framebuffers therefore miss the
//!   cache and are recreated every frame (bounded by the cache cap), while
//!   regular textures resolve to a stable pointer and keep hitting.
//! * Dynamic offsets / immediates / occlusion queries / timestamp writes /
//!   multiview are not expressible on this path (see the per-command notes).

use alloc::ffi::CString;
use bevy_window::PresentMode;
use core::ffi::c_void;
use diligent_rs::diligent_sys::bindings as sys;

use super::{diligent_registry, RenderDevice};

/// A raw Diligent interface pointer with ownership (calls `Release` on
/// drop). The engine keeps every interface in a COM-style refcounted object
/// whose vtable starts with the universal `IObjectMethods` block, so the
/// release can be performed generically.
///
/// # Safety
///
/// Only engine-returned (AddRef'ed) pointers may be wrapped. Engine resource
/// objects are thread-safe (same deliberate opt-in as
/// [`DiligentHandle`](diligent_registry::DiligentHandle)).
pub(crate) struct RawOwned<T> {
    ptr: *mut T,
}

// SAFETY: the wrapped engine objects are ref-counted and thread-safe at the
// engine level; the release on drop is thread-safe (same discipline as
// `DiligentHandle`).
unsafe impl<T> Send for RawOwned<T> {}
unsafe impl<T> Sync for RawOwned<T> {}

impl<T> RawOwned<T> {
    fn from_raw(ptr: *mut T) -> Self {
        assert!(!ptr.is_null(), "diligent: refusing to wrap a null pointer");
        Self { ptr }
    }

    pub(crate) fn as_ptr(&self) -> *mut T {
        self.ptr
    }
}

impl<T> Drop for RawOwned<T> {
    fn drop(&mut self) {
        // Safety: every Diligent interface vtable starts with the
        // IObjectMethods block, so casting any interface to IObject and
        // invoking the Release slot is valid; this handle owns exactly one
        // reference (the engine's AddRef from the creation call).
        release_interface(self.ptr.cast());
    }
}

/// Calls the universal `IObject::Release` vtable slot.
fn release_interface(ptr: *mut c_void) {
    // Safety: `ptr` is a live Diligent interface (see `RawOwned`).
    let obj = ptr.cast::<sys::IObject>();
    let vtbl = unsafe { &*(*obj).pVtbl };
    let release = vtbl
        .Object
        .Release
        .expect("diligent: IObject::Release missing from vtable");
    // Safety: the object is alive; the caller owns the released reference.
    unsafe { release(obj) };
}

// ---------------------------------------------------------------------------
// IDeviceContext command helpers (signatures verified against the locked
// DeviceContext.h / the generated bindings; the M1-1 wrapper does not cover
// these).
// ---------------------------------------------------------------------------

/// Returns the immediate-context method table together with the context
/// lock. Every helper MUST hold the returned guard for the WHOLE engine
/// call (M1-4b-2 review, fix 1: a guard that drops before the vtable
/// invocation serializes nothing - the render-world schedules are
/// multithreaded and the immediate context is not thread-safe, so a
/// concurrent access corrupts the D3D12 command list).
fn context_methods(
    ctx: &diligent_rs::DeviceContext,
) -> (std::sync::MutexGuard<'static, ()>, &sys::IDeviceContextMethods) {
    let guard = diligent_registry::context_guard();
    // Safety: `ctx` is alive for the duration of the call.
    let methods = unsafe { &(*(*ctx.as_raw()).pVtbl).DeviceContext };
    (guard, methods)
}

fn state_transition_mode() -> sys::RESOURCE_STATE_TRANSITION_MODE {
    sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
        as sys::RESOURCE_STATE_TRANSITION_MODE
}

/// `IDeviceContext::SetIndexBuffer` (DeviceContext.h:2788).
pub(crate) fn set_index_buffer(
    ctx: &diligent_rs::DeviceContext,
    buffer: *mut sys::IBuffer,
    byte_offset: u64,
) {
    let (_guard, methods) = context_methods(ctx);
    let set = methods
        .SetIndexBuffer
        .expect("diligent: IDeviceContext::SetIndexBuffer missing from vtable");
    // Safety: `buffer` is a live index buffer (the registry keeps the owning
    // wrapper alive for the duration of the call).
    unsafe { set(ctx.as_raw(), buffer, byte_offset, state_transition_mode()) };
}

/// `IDeviceContext::Draw` (DeviceContext.h:2929).
pub(crate) fn draw(ctx: &diligent_rs::DeviceContext, attribs: &sys::DrawAttribs) {
    let (_guard, methods) = context_methods(ctx);
    let draw = methods
        .Draw
        .expect("diligent: IDeviceContext::Draw missing from vtable");
    // Safety: `attribs` is a valid draw command description.
    unsafe { draw(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::DrawIndexed` (DeviceContext.h:2945).
pub(crate) fn draw_indexed(ctx: &diligent_rs::DeviceContext, attribs: &sys::DrawIndexedAttribs) {
    let (_guard, methods) = context_methods(ctx);
    let draw = methods
        .DrawIndexed
        .expect("diligent: IDeviceContext::DrawIndexed missing from vtable");
    // Safety: `attribs` is a valid draw command description.
    unsafe { draw(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::DrawIndirect` (DeviceContext.h:2978).
pub(crate) fn draw_indirect(ctx: &diligent_rs::DeviceContext, attribs: &sys::DrawIndirectAttribs) {
    let (_guard, methods) = context_methods(ctx);
    let draw = methods
        .DrawIndirect
        .expect("diligent: IDeviceContext::DrawIndirect missing from vtable");
    // Safety: `attribs` is a valid draw command description.
    unsafe { draw(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::DrawIndexedIndirect` (DeviceContext.h:3014).
pub(crate) fn draw_indexed_indirect(
    ctx: &diligent_rs::DeviceContext,
    attribs: &sys::DrawIndexedIndirectAttribs,
) {
    let (_guard, methods) = context_methods(ctx);
    let draw = methods
        .DrawIndexedIndirect
        .expect("diligent: IDeviceContext::DrawIndexedIndirect missing from vtable");
    // Safety: `attribs` is a valid draw command description.
    unsafe { draw(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::DispatchCompute` (DeviceContext.h:3037; M1-4b-2: the
/// compute dispatch path - the transition wgpu compute recording is gone).
pub(crate) fn dispatch_compute(ctx: &diligent_rs::DeviceContext, attribs: &sys::DispatchComputeAttribs) {
    let (_guard, methods) = context_methods(ctx);
    let dispatch = methods
        .DispatchCompute
        .expect("diligent: IDeviceContext::DispatchCompute missing from vtable");
    // Safety: `attribs` is a valid dispatch command description.
    unsafe { dispatch(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::SetScissorRects` (DeviceContext.h:2837) - one rect,
/// render-target size derived from the bound targets.
pub(crate) fn set_scissor_rects(ctx: &diligent_rs::DeviceContext, rects: &[sys::Rect]) {
    let (_guard, methods) = context_methods(ctx);
    let set = methods
        .SetScissorRects
        .expect("diligent: IDeviceContext::SetScissorRects missing from vtable");
    // Safety: `rects` is valid for the duration of the call.
    unsafe { set(ctx.as_raw(), rects.len() as u32, rects.as_ptr(), 0, 0) };
}

/// `IDeviceContext::SetStencilRef` (DeviceContext.h:2703).
pub(crate) fn set_stencil_ref(ctx: &diligent_rs::DeviceContext, reference: u32) {
    let (_guard, methods) = context_methods(ctx);
    let set = methods
        .SetStencilRef
        .expect("diligent: IDeviceContext::SetStencilRef missing from vtable");
    // Safety: no pointers involved.
    unsafe { set(ctx.as_raw(), reference) };
}

/// `IDeviceContext::SetBlendFactors` (DeviceContext.h:2717).
pub(crate) fn set_blend_factors(ctx: &diligent_rs::DeviceContext, factors: [f32; 4]) {
    let (_guard, methods) = context_methods(ctx);
    let set = methods
        .SetBlendFactors
        .expect("diligent: IDeviceContext::SetBlendFactors missing from vtable");
    // Safety: the factors array is valid for the duration of the call.
    unsafe { set(ctx.as_raw(), factors.as_ptr()) };
}

/// The subpass depth-stencil attachment state for a wgpu depth-stencil
/// attachment (M2a-2).
///
/// wgpu 29 marks an aspect read-only with `*_ops: None`; the locked Diligent
/// version expresses read-only depth-stencil as a single all-or-nothing
/// state (`RESOURCE_STATE_DEPTH_READ` - the framebuffer derives a
/// read-only DSV, FramebufferBase.hpp:127-165). Both aspects read-only maps
/// exactly; any other combination maps to the writable `DEPTH_WRITE` state
/// (see `begin_tracked_render_pass` for the mixed-aspect note).
pub(crate) fn depth_stencil_attachment_state(
    depth_read_only: bool,
    stencil_read_only: bool,
) -> sys::RESOURCE_STATE {
    if depth_read_only && stencil_read_only {
        sys::_RESOURCE_STATE::RESOURCE_STATE_DEPTH_READ as sys::RESOURCE_STATE
    } else {
        sys::_RESOURCE_STATE::RESOURCE_STATE_DEPTH_WRITE as sys::RESOURCE_STATE
    }
}

/// `IDeviceContext::DispatchComputeIndirect` (DeviceContext.h:3111).
pub(crate) fn dispatch_compute_indirect(
    ctx: &diligent_rs::DeviceContext,
    attribs: &sys::DispatchComputeIndirectAttribs,
) {
    let (_guard, methods) = context_methods(ctx);
    let dispatch = methods
        .DispatchComputeIndirect
        .expect("diligent: IDeviceContext::DispatchComputeIndirect missing from vtable");
    // Safety: `attribs` is a valid dispatch command description.
    unsafe { dispatch(ctx.as_raw(), attribs) };
}

/// Raw `IDeviceContext::BeginRenderPass` (DeviceContext.h:2901) vtable call.
///
/// M1-4b-2 review, fix 2: this variant does NOT take the context lock itself
/// - the caller must hold the guard for the whole call (see
/// `begin_tracked_render_pass`: the pass begin is serialized together with
/// the viewport reset under one guard; a locking variant can never run under
/// that guard - `CONTEXT_LOCK` is a non-reentrant `std::sync::Mutex` and a
/// nested acquisition self-deadlocks).
fn begin_render_pass_engine_call(
    ctx: &diligent_rs::DeviceContext,
    attribs: &sys::BeginRenderPassAttribs,
) {
    // Safety: `ctx` is alive for the duration of the call (the caller holds
    // the context lock, so no concurrent access can invalidate it).
    let methods = unsafe { &(*(*ctx.as_raw()).pVtbl).DeviceContext };
    let begin = methods
        .BeginRenderPass
        .expect("diligent: IDeviceContext::BeginRenderPass missing from vtable");
    // Safety: `attribs` references live render-pass/framebuffer objects and
    // clear values for the duration of the call.
    unsafe { begin(ctx.as_raw(), attribs) };
}

/// `IDeviceContext::EndRenderPass` (DeviceContext.h:2914).
pub(crate) fn end_render_pass(ctx: &diligent_rs::DeviceContext) {
    let (_guard, methods) = context_methods(ctx);
    let end = methods
        .EndRenderPass
        .expect("diligent: IDeviceContext::EndRenderPass missing from vtable");
    // Safety: no pointers involved.
    unsafe { end(ctx.as_raw()) };
}

/// `IDeviceContext::BeginDebugGroup` (DeviceContext.h, verified in the
/// bindings).
pub(crate) fn begin_debug_group(ctx: &diligent_rs::DeviceContext, label: &str) {
    let Ok(name) = CString::new(label) else {
        return;
    };
    let (_guard, methods) = context_methods(ctx);
    let begin = methods
        .BeginDebugGroup
        .expect("diligent: IDeviceContext::BeginDebugGroup missing from vtable");
    // Safety: `name` is a live C string for the duration of the call.
    unsafe { begin(ctx.as_raw(), name.as_ptr(), std::ptr::null()) };
}

/// `IDeviceContext::EndDebugGroup`.
pub(crate) fn end_debug_group(ctx: &diligent_rs::DeviceContext) {
    let (_guard, methods) = context_methods(ctx);
    let end = methods
        .EndDebugGroup
        .expect("diligent: IDeviceContext::EndDebugGroup missing from vtable");
    // Safety: no pointers involved.
    unsafe { end(ctx.as_raw()) };
}

/// `ISwapChain::SetMaximumFrameLatency` (SwapChain.h:99; D3D11/D3D12 only).
pub(crate) fn set_maximum_frame_latency(swap_chain: &diligent_rs::SwapChain, latency: u32) {
    let set = unsafe {
        (*(*swap_chain.as_raw()).pVtbl)
            .SwapChain
            .SetMaximumFrameLatency
            .as_ref()
            .expect("diligent: ISwapChain::SetMaximumFrameLatency missing from vtable")
    };
    // Safety: no pointers involved.
    unsafe { set(swap_chain.as_raw(), latency) };
}

// ---------------------------------------------------------------------------
// Render pass + framebuffer creation (raw IRenderDevice vtables; the
// diligent-rs wrapper has no RenderPass/Framebuffer support - the struct
// shapes are verified against RenderPass.h / Framebuffer.h and the generated
// bindings).
// ---------------------------------------------------------------------------

/// The maximum number of cached render pass + framebuffer pairs. Exceeding
/// the cap clears the whole cache (framebuffer views churn, e.g. swap-chain
/// back-buffer RTVs that flip per `Present`).
const MAX_CACHED_RENDER_PASSES: usize = 128;

/// One color/depth-stencil attachment as far as the render pass object is
/// concerned (format + ops; the clear value is per-begin, not per-object).
///
/// `read_only` marks the depth-stencil attachment reference of a fully
/// read-only depth pass (M2a-2: wgpu expresses read-only depth via
/// `depth_ops: None` + `stencil_ops: None`; Diligent expresses it via the
/// `RESOURCE_STATE_DEPTH_READ` subpass reference state, which makes the
/// framebuffer derive a read-only DSV - FramebufferBase.hpp:127-165). It is
/// part of the cache key: two passes that differ only in read-only-ness must
/// not share a render-pass object (the reference states are baked into it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PassAttachmentKey {
    pub(crate) format: i32,
    pub(crate) sample_count: u8,
    pub(crate) load_op: u8,
    pub(crate) store_op: u8,
    pub(crate) read_only: bool,
}

/// Cache key for a render pass + framebuffer pair: the attachment
/// descriptors plus the RESOLVED diligent attachment-view pointer identity
/// (a stable pointer for regular textures; per-frame fresh for swap-chain
/// back-buffer RTVs - see `resolved_view_keys`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PassCacheKey {
    attachments: Vec<PassAttachmentKey>,
    views: Vec<usize>,
}

struct RenderPassEntry {
    key: PassCacheKey,
    render_pass: RawOwned<sys::IRenderPass>,
    framebuffer: RawOwned<sys::IFramebuffer>,
}

/// Bounded cache of render pass + framebuffer engine objects plus the
/// derived attachment views created for them (all released together when the
/// cache is cleared).
pub(crate) struct RenderPassCache {
    entries: Vec<RenderPassEntry>,
    derived_views: Vec<RawOwned<sys::ITextureView>>,
}

impl RenderPassCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            derived_views: Vec::new(),
        }
    }

    /// Releases every cached object (also drops the derived attachment
    /// views; the framebuffers hold their own references, so nothing
    /// dangles). Called on swap-chain resize and on cap overflow.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.derived_views.clear();
    }

    /// The insertion gate: clears the cache when the cap is reached (the
    /// framebuffer views churn - swap-chain back-buffer RTVs flip per
    /// `Present` - so eviction is a full clear rather than an LRU).
    pub(crate) fn prepare_insert(&mut self) {
        if should_evict(self.entries.len()) {
            self.clear();
        }
    }
}

/// Whether the cache must be evicted at `entries_len` entries.
fn should_evict(entries_len: usize) -> bool {
    entries_len >= MAX_CACHED_RENDER_PASSES
}

impl Default for RenderPassCache {
    fn default() -> Self {
        Self::new()
    }
}

/// `ITextureView::GetDesc` via the universal DeviceObject slot (the C API
/// casts `IDeviceObject::GetDesc` to the concrete description type).
fn texture_view_desc(view: *mut sys::ITextureView) -> Result<sys::TextureViewDesc, String> {
    let get = unsafe {
        (*(*view).pVtbl)
            .DeviceObject
            .GetDesc
            .as_ref()
            .ok_or("ITextureView::GetDesc missing from vtable")?
    };
    // Safety: the engine returns a pointer to internal storage that is valid
    // while the view is alive; we copy the value out.
    let ptr = unsafe { get(view.cast::<sys::IDeviceObject>()) };
    Ok(unsafe { *ptr.cast::<sys::TextureViewDesc>() })
}

/// `ITextureView::GetTexture` (does not AddRef).
fn view_texture(view: *mut sys::ITextureView) -> Result<*mut sys::ITexture, String> {
    let get = unsafe {
        (*(*view).pVtbl)
            .TextureView
            .GetTexture
            .as_ref()
            .ok_or("ITextureView::GetTexture missing from vtable")?
    };
    // Safety: the returned pointer is owned by the view (no refcount
    // increment) and is valid while the view is alive.
    let texture = unsafe { get(view) };
    if texture.is_null() {
        Err("ITextureView::GetTexture returned null".into())
    } else {
        Ok(texture)
    }
}

/// The MSAA sample count of the texture behind a view
/// (`ITexture::GetDesc().SampleCount`; 1 for single-sample textures).
fn texture_sample_count(view: *mut sys::ITextureView) -> Result<u32, String> {
    let texture = view_texture(view)?;
    let get = unsafe {
        (*(*texture).pVtbl)
            .DeviceObject
            .GetDesc
            .as_ref()
            .ok_or("ITexture::GetDesc missing from vtable")?
    };
    // Safety: the engine returns a pointer to internal storage that is valid
    // while the texture is alive; the desc's first member is the
    // `DeviceObjectAttribs` the call returns.
    let ptr = unsafe { get(texture.cast::<sys::IDeviceObject>()) };
    if ptr.is_null() {
        return Err("ITexture::GetDesc returned null".into());
    }
    Ok(unsafe { (*ptr.cast::<sys::TextureDesc>()).SampleCount })
}

/// `ITexture::CreateView` (AddRefs the returned view).
fn texture_create_view(
    texture: *mut sys::ITexture,
    view_desc: &sys::TextureViewDesc,
) -> Result<*mut sys::ITextureView, String> {
    let create = unsafe {
        (*(*texture).pVtbl)
            .Texture
            .CreateView
            .as_ref()
            .ok_or("ITexture::CreateView missing from vtable")?
    };
    let mut view: *mut sys::ITextureView = std::ptr::null_mut();
    // Safety: `view_desc` is a valid FFI struct and `view` is an out param;
    // the engine AddRefs the returned view.
    unsafe { create(texture, view_desc, &mut view) };
    if view.is_null() {
        Err("ITexture::CreateView returned null".into())
    } else {
        Ok(view)
    }
}

/// Resolves the diligent view for a render-pass attachment.
///
/// The M1-2 view-type heuristic creates bind-group-side views
/// (SRV/UAV-first), so an attachment view of the required type may not
/// exist yet - in that case a new view of `required_type` is derived from
/// the texture (keeping the base view's sub-resource range) and cached in
/// the render-pass cache (released with it; framebuffers hold their own
/// references to the views, so the derived views never dangle).
fn resolve_attachment_view(
    cache: &mut RenderPassCache,
    view: &crate::render_resource::WgpuTextureView,
    required_type: sys::TEXTURE_VIEW_TYPE,
) -> Result<*mut sys::ITextureView, String> {
    let base = diligent_registry::registry()
        .resolve_texture_view(view.id())
        .ok_or("no diligent texture view registered for the attachment")?;
    let base_desc = texture_view_desc(base)?;
    if base_desc.ViewType == required_type {
        return Ok(base);
    }
    // Derive a view of the required type on the same texture, preserving the
    // base view's sub-resource range (TODO-REMOVE-M1-4: sub-range
    // attachments are a corner case - most bevy attachments are full-texture
    // views).
    let texture = view_texture(base)?;
    let format = match diligent_rs::format::to_diligent(view.format) {
        Ok(format) => format,
        Err(err) => return Err(format!("attachment view format: {err}")),
    };
    let mut view_desc = base_desc;
    view_desc.ViewType = required_type;
    // TEMP-DIAG-M2A2: log the derived view descriptor (crash bisection).
    bevy_log::debug!(
        "diligent: deriving attachment view: base {:?} -> type {:?} format {:?} dim {:?} mip {}..{} slices {}..{}",
        base_desc.ViewType,
        required_type,
        view_desc.Format,
        view_desc.TextureDim,
        view_desc.MostDetailedMip,
        view_desc.MostDetailedMip + view_desc.NumMipLevels,
        unsafe { view_desc.__bindgen_anon_1.FirstArraySlice },
        unsafe { view_desc.__bindgen_anon_1.FirstArraySlice + view_desc.__bindgen_anon_2.NumArraySlices },
    );
    // TODO-REMOVE-M1-4 (M1-3 review, fix 7): the wgpu view's format override
    // (e.g. a Depth24Plus texture viewed as Depth32Float) is discarded here -
    // the derived view uses the texture's own format. The engine validates
    // derived views against the texture, so a mismatch fails loudly rather
    // than silently; M1-5 re-checks whether the override must be honored.
    view_desc.Format = format;
    let derived = texture_create_view(texture, &view_desc)?;
    let ptr = derived;
    cache.derived_views.push(RawOwned::from_raw(derived));
    Ok(ptr)
}

/// Maps a wgpu color `Operations` to Diligent load/store ops.
///
/// Note: wgpu 29's `LoadOp` has no `Discard` variant - `DontCare` maps to
/// `ATTACHMENT_LOAD_OP_DISCARD` (the "contents undefined" semantics).
fn color_ops(
    ops: wgpu_types::Operations<wgpu_types::Color>,
) -> Result<(sys::ATTACHMENT_LOAD_OP, sys::ATTACHMENT_STORE_OP), String> {
    Ok((
        match ops.load {
            wgpu_types::LoadOp::Load => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD,
            wgpu_types::LoadOp::Clear(_) => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_CLEAR,
            wgpu_types::LoadOp::DontCare(_) => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD,
        } as sys::ATTACHMENT_LOAD_OP,
        match ops.store {
            wgpu_types::StoreOp::Store => sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE,
            wgpu_types::StoreOp::Discard => sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_DISCARD,
        } as sys::ATTACHMENT_STORE_OP,
    ))
}

/// Maps a wgpu depth/stencil `Operations` to Diligent load/store ops
/// (`None` = the attachment is bound but not touched -> DISCARD both).
fn depth_ops<T>(
    ops: Option<wgpu_types::Operations<T>>,
) -> Result<(sys::ATTACHMENT_LOAD_OP, sys::ATTACHMENT_STORE_OP), String> {
    match ops {
        Some(ops) => Ok((
            match ops.load {
                wgpu_types::LoadOp::Load => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD,
                wgpu_types::LoadOp::Clear(_) => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_CLEAR,
                wgpu_types::LoadOp::DontCare(_) => sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD,
            } as sys::ATTACHMENT_LOAD_OP,
            match ops.store {
                wgpu_types::StoreOp::Store => sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE,
                wgpu_types::StoreOp::Discard => sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_DISCARD,
            } as sys::ATTACHMENT_STORE_OP,
        )),
        None => Ok((
            sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD as sys::ATTACHMENT_LOAD_OP,
            sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_DISCARD as sys::ATTACHMENT_STORE_OP,
        )),
    }
}

/// `IRenderDevice::CreateRenderPass` for a single-subpass render pass
/// (AddRefs the returned object).
fn create_render_pass(
    device: &diligent_rs::RenderDevice,
    label: Option<&str>,
    attachments: &[sys::RenderPassAttachmentDesc],
    color_refs: &[sys::AttachmentReference],
    resolve_refs: &[sys::AttachmentReference],
    depth_ref: Option<&sys::AttachmentReference>,
) -> Result<RawOwned<sys::IRenderPass>, String> {
    let name = CString::new(label.unwrap_or("bevy_render_pass")).map_err(|e| e.to_string())?;
    let subpass = sys::SubpassDesc {
        InputAttachmentCount: 0,
        pInputAttachments: std::ptr::null(),
        RenderTargetAttachmentCount: color_refs.len() as u32,
        pRenderTargetAttachments: color_refs.as_ptr(),
        pResolveAttachments: if resolve_refs.is_empty() {
            std::ptr::null()
        } else {
            resolve_refs.as_ptr()
        },
        pDepthStencilAttachment: depth_ref.map_or(std::ptr::null(), |r| r as *const _),
        PreserveAttachmentCount: 0,
        pPreserveAttachments: std::ptr::null(),
        pShadingRateAttachment: std::ptr::null(),
    };
    let mut desc: sys::RenderPassDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name.as_ptr();
    desc.AttachmentCount = attachments.len() as u32;
    desc.pAttachments = attachments.as_ptr();
    desc.SubpassCount = 1;
    desc.pSubpasses = &subpass;
    desc.DependencyCount = 0;
    desc.pDependencies = std::ptr::null();

    let mut render_pass: *mut sys::IRenderPass = std::ptr::null_mut();
    let create = unsafe {
        (*(*device.as_raw()).pVtbl)
            .RenderDevice
            .CreateRenderPass
            .as_ref()
            .ok_or("IRenderDevice::CreateRenderPass missing from vtable")?
    };
    // Safety: `desc` points at live attachment/subpass arrays and a C string
    // for the duration of the call; `render_pass` is an out param.
    unsafe { create(device.as_raw(), &desc, &mut render_pass) };
    if render_pass.is_null() {
        return Err("IRenderDevice::CreateRenderPass returned null".into());
    }
    Ok(RawOwned::from_raw(render_pass))
}

/// `IRenderDevice::CreateFramebuffer` (AddRefs the returned object and the
/// attachment views / render pass it references - FramebufferBase.hpp:121).
/// Width/Height/NumArraySlices are derived from the attachments by the
/// engine when zero (FramebufferBase.hpp:72-100).
fn create_framebuffer(
    device: &diligent_rs::RenderDevice,
    label: Option<&str>,
    render_pass: *mut sys::IRenderPass,
    views: &[*mut sys::ITextureView],
) -> Result<RawOwned<sys::IFramebuffer>, String> {
    let name = CString::new(label.unwrap_or("bevy_framebuffer")).map_err(|e| e.to_string())?;
    let mut desc: sys::FramebufferDesc = unsafe { std::mem::zeroed() };
    desc._DeviceObjectAttribs.Name = name.as_ptr();
    desc.pRenderPass = render_pass;
    desc.AttachmentCount = views.len() as u32;
    desc.ppAttachments = views.as_ptr();
    desc.Width = 0;
    desc.Height = 0;
    desc.NumArraySlices = 0;

    let mut framebuffer: *mut sys::IFramebuffer = std::ptr::null_mut();
    let create = unsafe {
        (*(*device.as_raw()).pVtbl)
            .RenderDevice
            .CreateFramebuffer
            .as_ref()
            .ok_or("IRenderDevice::CreateFramebuffer missing from vtable")?
    };
    // Safety: `desc` points at live attachment views and a C string for the
    // duration of the call; `framebuffer` is an out param.
    unsafe { create(device.as_raw(), &desc, &mut framebuffer) };
    if framebuffer.is_null() {
        return Err("IRenderDevice::CreateFramebuffer returned null".into());
    }
    Ok(RawOwned::from_raw(framebuffer))
}

/// The cache-key view identities for the resolved attachment views: the
/// diligent view POINTERS, not the wgpu view addresses. The swap chain
/// re-registers a fresh back-buffer RTV under the same wgpu dummy address
/// every frame, so an address-keyed framebuffer would silently reuse the
/// first frame's back buffer; regular textures resolve to a stable pointer
/// and keep hitting the cache (M1-3 review, fix 1).
fn resolved_view_keys(views: &[*mut sys::ITextureView]) -> Vec<usize> {
    views.iter().map(|&view| view as usize).collect()
}

/// `IFramebuffer::GetDesc` via the universal DeviceObject slot - the engine
/// resolves `Width`/`Height` from the attachments at creation time
/// (FramebufferBase.hpp:72-100), so the desc carries the real framebuffer
/// size even though `CreateFramebuffer` was called with zeros.
fn framebuffer_size(framebuffer: *mut sys::IFramebuffer) -> Result<(u32, u32), String> {
    let get = unsafe {
        (*(*framebuffer).pVtbl)
            .DeviceObject
            .GetDesc
            .as_ref()
            .ok_or("IFramebuffer::GetDesc missing from vtable")?
    };
    // Safety: the engine returns a pointer to internal storage that is
    // valid while the framebuffer is alive; we copy the value out.
    let ptr = unsafe { get(framebuffer.cast::<sys::IDeviceObject>()) };
    let desc = unsafe { *ptr.cast::<sys::FramebufferDesc>() };
    Ok((desc.Width, desc.Height))
}

/// Begins a Diligent render pass for a wgpu render-pass descriptor:
/// resolves/derives the attachment views, creates (or reuses) the render
/// pass + framebuffer and issues `BeginRenderPass` with the descriptor's
/// clear values.
///
/// Returns `Err(reason)` when the pass cannot run on the diligent path (no
/// diligent device, an attachment without a diligent view, multiview /
/// queries / timestamps requested, no attachments at all) - the caller falls
/// back to the transition wgpu encoder for that pass.
pub(crate) fn begin_tracked_render_pass(
    render_device: &RenderDevice,
    context: &diligent_rs::DeviceContext,
    descriptor: &crate::render_resource::RenderPassDescriptor,
) -> Result<(), String> {
    if descriptor.multiview_mask.is_some() {
        return Err("multiview render passes are not supported on the diligent path \
                    (TODO-REMOVE-M1-4)"
            .into());
    }
    if descriptor.occlusion_query_set.is_some() {
        return Err("occlusion queries are not supported on the diligent path \
                    (TODO-REMOVE-M1-4)"
            .into());
    }
    if descriptor.timestamp_writes.is_some() {
        return Err("timestamp writes are not supported on the diligent path \
                    (TODO-REMOVE-M1-4)"
            .into());
    }
    let device = render_device
        .diligent_device()
        .ok_or("no diligent device")?;

    let mut attachments: Vec<sys::RenderPassAttachmentDesc> = Vec::new();
    let mut framebuffer_views: Vec<*mut sys::ITextureView> = Vec::new();
    let mut color_refs: Vec<sys::AttachmentReference> = Vec::new();
    let mut resolve_refs: Vec<sys::AttachmentReference> = Vec::new();
    let mut depth_ref: Option<sys::AttachmentReference> = None;
    let mut key_attachments: Vec<PassAttachmentKey> = Vec::new();

    let mut cache = render_device.render_pass_cache().lock().unwrap();

    for attachment in descriptor.color_attachments.iter() {
        let Some(attachment) = attachment else {
            // Empty attachment slot: the subpass reference marks it unused.
            color_refs.push(sys::AttachmentReference {
                AttachmentIndex: sys::ATTACHMENT_UNUSED,
                State: sys::_RESOURCE_STATE::RESOURCE_STATE_UNKNOWN as sys::RESOURCE_STATE,
            });
            resolve_refs.push(sys::AttachmentReference {
                AttachmentIndex: sys::ATTACHMENT_UNUSED,
                State: sys::_RESOURCE_STATE::RESOURCE_STATE_UNKNOWN as sys::RESOURCE_STATE,
            });
            continue;
        };
        let view = resolve_attachment_view(
            &mut cache,
            attachment.view,
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_RENDER_TARGET as sys::TEXTURE_VIEW_TYPE,
        )?;
        let format = diligent_rs::format::to_diligent(attachment.view.format)
            .map_err(|e| format!("color attachment format: {e}"))?;
        let (load_op, store_op) = color_ops(attachment.ops)?;
        // M1-4b-1: MSAA textures are created with their real sample count
        // (the wrapper's TextureDesc carries it through), so the render-pass
        // attachment must declare the texture's actual sample count and the
        // resolve target is wired into the subpass (the engine resolves at
        // pass end - D3D12_RENDER_PASS_ENDING_ACCESS_TYPE_RESOLVE).
        let sample_count = texture_sample_count(view)?;
        // TEMP-BISECT-M2A2: force the resolve off (M2a-1 behavior) to confirm
        // the resolve wiring is the TDR regression.
        let resolve_disabled = std::env::var_os("DILIGENT_RS_NO_RESOLVE").is_some();
        let (resolve_ref, _resolve_present) = if let Some(resolve_target) = attachment.resolve_target
            && !resolve_disabled
        {
            let resolve_view = resolve_attachment_view(
                &mut cache,
                resolve_target,
                sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_RENDER_TARGET as sys::TEXTURE_VIEW_TYPE,
            )?;
            let resolve_format = diligent_rs::format::to_diligent(resolve_target.format)
                .map_err(|e| format!("resolve attachment format: {e}"))?;
            if resolve_format != format {
                return Err(format!(
                    "resolve attachment format {resolve_format:?} does not match the color \
                     attachment format {format:?}"
                )
                .into());
            }
            let index = attachments.len() as u32;
            attachments.push(sys::RenderPassAttachmentDesc {
                Format: resolve_format,
                SampleCount: 1,
                LoadOp: sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD
                    as sys::ATTACHMENT_LOAD_OP,
                StoreOp: sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE
                    as sys::ATTACHMENT_STORE_OP,
                StencilLoadOp: sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD
                    as sys::ATTACHMENT_LOAD_OP,
                StencilStoreOp: sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE
                    as sys::ATTACHMENT_STORE_OP,
                InitialState: sys::_RESOURCE_STATE::RESOURCE_STATE_RESOLVE_DEST
                    as sys::RESOURCE_STATE,
                FinalState: sys::_RESOURCE_STATE::RESOURCE_STATE_RESOLVE_DEST as sys::RESOURCE_STATE,
            });
            key_attachments.push(PassAttachmentKey {
                format: resolve_format as i32,
                sample_count: 1,
                load_op: sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD as u8,
                store_op: sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE as u8,
                read_only: false,
            });
            framebuffer_views.push(resolve_view);
            (
                sys::AttachmentReference {
                    AttachmentIndex: index,
                    State: sys::_RESOURCE_STATE::RESOURCE_STATE_RESOLVE_DEST as sys::RESOURCE_STATE,
                },
                true,
            )
        } else {
            (
                sys::AttachmentReference {
                    AttachmentIndex: sys::ATTACHMENT_UNUSED,
                    State: sys::_RESOURCE_STATE::RESOURCE_STATE_UNKNOWN as sys::RESOURCE_STATE,
                },
                false,
            )
        };
        let _ = _resolve_present;
        let index = attachments.len() as u32;
        attachments.push(sys::RenderPassAttachmentDesc {
            Format: format,
            SampleCount: sample_count as u8,
            LoadOp: load_op,
            StoreOp: store_op,
            StencilLoadOp: sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD
                as sys::ATTACHMENT_LOAD_OP,
            StencilStoreOp: sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE
                as sys::ATTACHMENT_STORE_OP,
            InitialState: sys::_RESOURCE_STATE::RESOURCE_STATE_RENDER_TARGET as sys::RESOURCE_STATE,
            FinalState: sys::_RESOURCE_STATE::RESOURCE_STATE_RENDER_TARGET as sys::RESOURCE_STATE,
        });
        key_attachments.push(PassAttachmentKey {
            format: format as i32,
            sample_count: sample_count as u8,
            load_op: load_op as u8,
            store_op: store_op as u8,
            read_only: false,
        });
        framebuffer_views.push(view);
        color_refs.push(sys::AttachmentReference {
            AttachmentIndex: index,
            State: sys::_RESOURCE_STATE::RESOURCE_STATE_RENDER_TARGET as sys::RESOURCE_STATE,
        });
        resolve_refs.push(resolve_ref);
    }

    if let Some(depth_stencil) = &descriptor.depth_stencil_attachment {
        let view = resolve_attachment_view(
            &mut cache,
            depth_stencil.view,
            sys::_TEXTURE_VIEW_TYPE::TEXTURE_VIEW_DEPTH_STENCIL as sys::TEXTURE_VIEW_TYPE,
        )?;
        let format = diligent_rs::format::to_diligent(depth_stencil.view.format)
            .map_err(|e| format!("depth-stencil attachment format: {e}"))?;
        let sample_count = texture_sample_count(view)?;
        let (depth_load, depth_store) = depth_ops(depth_stencil.depth_ops)?;
        let (stencil_load, stencil_store) = depth_ops(depth_stencil.stencil_ops)?;
        // M2a-2: the read-only depth-stencil expression of this locked
        // version. wgpu 29 marks an aspect read-only with `*_ops: None`
        // (wgpu-core render.rs:1182 - `is_depth_read_only =
        // at.depth.is_readonly()`); Diligent has no per-aspect flag - the
        // read-only DSV covers both aspects
        // (D3DViewDescConversionImpl.hpp:187-194 sets
        // READ_ONLY_DEPTH | READ_ONLY_STENCIL) and is triggered by the
        // subpass depth attachment reference state == RESOURCE_STATE_DEPTH_READ
        // (RenderPassBase.cpp:268 validates it; FramebufferBase.hpp:127-165
        // derives the read-only view; DeviceContextBase.hpp:1275 binds it).
        // Fully read-only (both ops None) maps exactly; a MIXED aspect maps
        // to the writable DSV (semantically safe - the pipeline simply does
        // not write the read-only aspect; the only loss is D3D12's
        // read-only-DSV optimization that allows SRV-sampling the aspect in
        // the same pass. Bevy's current mixed uses are on depth-only
        // formats, where wgpu on D3D12 also produces a plain writable DSV).
        let fully_read_only = depth_stencil.depth_ops.is_none()
            && depth_stencil.stencil_ops.is_none();
        let attachment_state = depth_stencil_attachment_state(
            depth_stencil.depth_ops.is_none(),
            depth_stencil.stencil_ops.is_none(),
        );
        // A read-only DSV must not be DISCARDed: preserve the contents via
        // LOAD/STORE (wgpu's None ops are NO_ACCESS - preserve semantics).
        let (depth_load, depth_store) = if fully_read_only {
            (
                sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD
                    as sys::ATTACHMENT_LOAD_OP,
                sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE
                    as sys::ATTACHMENT_STORE_OP,
            )
        } else {
            (depth_load, depth_store)
        };
        let index = attachments.len() as u32;
        attachments.push(sys::RenderPassAttachmentDesc {
            Format: format,
            SampleCount: sample_count as u8,
            LoadOp: depth_load,
            StoreOp: depth_store,
            StencilLoadOp: stencil_load,
            StencilStoreOp: stencil_store,
            InitialState: attachment_state,
            FinalState: attachment_state,
        });
        key_attachments.push(PassAttachmentKey {
            format: format as i32,
            sample_count: sample_count as u8,
            load_op: depth_load as u8,
            store_op: depth_store as u8,
            read_only: fully_read_only,
        });
        framebuffer_views.push(view);
        depth_ref = Some(sys::AttachmentReference {
            AttachmentIndex: index,
            State: attachment_state,
        });
    }

    if attachments.is_empty() {
        return Err("render pass has no attachments (the engine cannot derive a framebuffer size)"
            .into());
    }

    let key = PassCacheKey {
        attachments: key_attachments,
        views: resolved_view_keys(&framebuffer_views),
    };
    let (render_pass, framebuffer) = if let Some(entry) = cache.entries.iter().find(|e| e.key == key)
    {
        (entry.render_pass.as_ptr(), entry.framebuffer.as_ptr())
    } else {
        cache.prepare_insert();
        let render_pass =
            create_render_pass(device, descriptor.label, &attachments, &color_refs, &resolve_refs, depth_ref.as_ref())?;
        let framebuffer = create_framebuffer(device, descriptor.label, render_pass.as_ptr(), &framebuffer_views)?;
        cache.entries.push(RenderPassEntry {
            key,
            render_pass,
            framebuffer,
        });
        let entry = cache
            .entries
            .last()
            .expect("entry pushed above");
        (entry.render_pass.as_ptr(), entry.framebuffer.as_ptr())
    };

    // Build the clear values: one per dense attachment, in attachment-index
    // order (resolve attachments get a default DISCARD entry - the engine
    // indexes the array by attachment number).
    let mut clear_values: Vec<sys::OptimizedClearValue> = Vec::with_capacity(attachments.len());
    let resolve_disabled = std::env::var_os("DILIGENT_RS_NO_RESOLVE").is_some();
    for attachment in descriptor.color_attachments.iter().flatten() {
        if attachment.resolve_target.is_some() && !resolve_disabled {
            clear_values.push(sys::OptimizedClearValue {
                Format: diligent_rs::format::to_diligent(attachment.view.format)
                    .map_err(|e| format!("color attachment format: {e}"))?,
                Color: [0.0; 4],
                DepthStencil: sys::DepthStencilClearValue {
                    Depth: 0.0,
                    Stencil: 0,
                },
            });
        }
        let mut clear = sys::OptimizedClearValue {
            Format: diligent_rs::format::to_diligent(attachment.view.format)
                .map_err(|e| format!("color attachment format: {e}"))?,
            Color: [0.0; 4],
            DepthStencil: sys::DepthStencilClearValue {
                Depth: 0.0,
                Stencil: 0,
            },
        };
        if let wgpu_types::LoadOp::Clear(color) = attachment.ops.load {
            clear.Color = [color.r as f32, color.g as f32, color.b as f32, color.a as f32];
        }
        clear_values.push(clear);
    }
    if let Some(depth_stencil) = &descriptor.depth_stencil_attachment {
        let mut clear = sys::OptimizedClearValue {
            Format: diligent_rs::format::to_diligent(depth_stencil.view.format)
                .map_err(|e| format!("depth-stencil attachment format: {e}"))?,
            Color: [0.0; 4],
            DepthStencil: sys::DepthStencilClearValue {
                Depth: 0.0,
                Stencil: 0,
            },
        };
        if let Some(ops) = depth_stencil.depth_ops
            && let wgpu_types::LoadOp::Clear(depth) = ops.load
        {
            clear.DepthStencil.Depth = depth;
        }
        if let Some(ops) = depth_stencil.stencil_ops
            && let wgpu_types::LoadOp::Clear(stencil) = ops.load
        {
            clear.DepthStencil.Stencil = stencil as u8;
        }
        clear_values.push(clear);
    }

    let mut attribs: sys::BeginRenderPassAttribs = unsafe { std::mem::zeroed() };
    attribs.pRenderPass = render_pass;
    attribs.pFramebuffer = framebuffer;
    attribs.ClearValueCount = clear_values.len() as u32;
    attribs.pClearValues = clear_values.as_mut_ptr();
    attribs.StateTransitionMode = state_transition_mode();

    // M1-3 review, fix 2: `BeginRenderPass` does NOT reset the viewport /
    // scissor state - it persists on the immediate context across passes
    // and frames, so a pass that never calls `set_viewport` could inherit a
    // previous frame's (or empty) viewport and rasterize nothing. wgpu
    // resets both to the full attachment rect at pass begin; issue the same
    // reset here, deterministically sized from the framebuffer's resolved
    // size (cheap: two state sets per pass).
    let (fb_width, fb_height) = framebuffer_size(framebuffer)?;

    // M1-4b-2 review, fix 2: the pass begin and the viewport reset are
    // immediate-context calls - they are serialized under ONE tightly-scoped
    // guard. The guard must NOT span any locking helper (`context_methods`
    // acquires the same non-reentrant `CONTEXT_LOCK`, so a helper called
    // under the guard would self-deadlock): only raw engine calls - the
    // BeginRenderPass vtable call and the wrapper `SetViewports` - run under
    // the lock, and `set_scissor_rects` (a locking helper) is issued only
    // after the guard is dropped.
    {
        let _guard = diligent_registry::context_guard();
        // TEMP-DIAG-M2A2: log the pass identity right before the engine
        // call (crash bisection for the null pRenderTargets[0] descriptor).
        bevy_log::debug!(
            "diligent: BeginRenderPass '{}' ({} attachments, {} color refs, depth {:?}, resolve refs {}, views {:?})",
            descriptor.label.unwrap_or("<unnamed>"),
            attachments.len(),
            color_refs.len(),
            depth_ref.map(|r| r.AttachmentIndex),
            resolve_refs.len(),
            framebuffer_views.iter().map(|v| *v as usize).collect::<Vec<_>>()
        );
        begin_render_pass_engine_call(context, &attribs);
        context.set_viewports(&[sys::Viewport {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: fb_width as f32,
            Height: fb_height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]);
    }
    set_scissor_rects(
        context,
        &[sys::Rect {
            left: 0,
            top: 0,
            right: fb_width as i32,
            bottom: fb_height as i32,
        }],
    );
    Ok(())
}

/// The `ISwapChain::Present` sync interval for a bevy_window `PresentMode`.
///
/// V12 finding: this Diligent version has no `PRESENT_MODE` enumeration and
/// no surface-capability query (`SwapChain.h` only exposes
/// `Present(Uint32 SyncInterval)`), so the wgpu fallback chain ("requested
/// mode not available -> fall back") reduces to the sync-interval choice:
/// vsync modes -> 1, no-vsync modes -> 0.
///
/// TODO-REMOVE-M1-4: `FifoRelaxed` (late tearing) and `Mailbox` (no tearing
/// without vsync) have no dedicated Diligent controls in this version -
/// `FifoRelaxed` falls back to plain vsync and `Mailbox` to no-vsync (the
/// engine's `SetMaximumFrameLatency`/DXGI flip controls could approximate
/// both; re-evaluate with the M2 present work).
pub(crate) fn present_mode_to_sync_interval(mode: PresentMode) -> u32 {
    match mode {
        PresentMode::Fifo | PresentMode::FifoRelaxed | PresentMode::AutoVsync => 1,
        PresentMode::Immediate | PresentMode::Mailbox | PresentMode::AutoNoVsync => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_mode_maps_to_the_sync_interval() {
        assert_eq!(present_mode_to_sync_interval(PresentMode::Fifo), 1);
        assert_eq!(present_mode_to_sync_interval(PresentMode::FifoRelaxed), 1);
        assert_eq!(present_mode_to_sync_interval(PresentMode::AutoVsync), 1);
        assert_eq!(present_mode_to_sync_interval(PresentMode::Immediate), 0);
        assert_eq!(present_mode_to_sync_interval(PresentMode::Mailbox), 0);
        assert_eq!(present_mode_to_sync_interval(PresentMode::AutoNoVsync), 0);
    }

    #[test]
    fn load_store_ops_map_to_attachment_ops() {
        let (load, store) = color_ops(wgpu_types::Operations {
            load: wgpu_types::LoadOp::Load,
            store: wgpu_types::StoreOp::Store,
        })
        .unwrap();
        assert_eq!(
            load,
            sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD as sys::ATTACHMENT_LOAD_OP
        );
        assert_eq!(
            store,
            sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE as sys::ATTACHMENT_STORE_OP
        );

        let (load, store) = color_ops(wgpu_types::Operations {
            load: wgpu_types::LoadOp::Clear(wgpu_types::Color::RED),
            store: wgpu_types::StoreOp::Discard,
        })
        .unwrap();
        assert_eq!(
            load,
            sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_CLEAR as sys::ATTACHMENT_LOAD_OP
        );
        assert_eq!(
            store,
            sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_DISCARD as sys::ATTACHMENT_STORE_OP
        );

        let (load, store) = depth_ops::<f32>(None).unwrap();
        assert_eq!(
            load,
            sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_DISCARD as sys::ATTACHMENT_LOAD_OP
        );
        assert_eq!(
            store,
            sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_DISCARD as sys::ATTACHMENT_STORE_OP
        );
    }

    #[test]
    fn pass_cache_evicts_after_the_cap() {
        // The creation path calls `prepare_insert` before inserting; the
        // eviction decision is pinned here (engine objects are device-bound,
        // so the cache itself is not exercised with real handles in tests).
        assert!(!should_evict(MAX_CACHED_RENDER_PASSES - 1));
        assert!(should_evict(MAX_CACHED_RENDER_PASSES));
        assert!(should_evict(MAX_CACHED_RENDER_PASSES + 4));
    }

    #[test]
    fn pass_cache_keys_on_resolved_diligent_view_identity() {
        // M1-3 review, fix 1: the cache key must be the RESOLVED diligent
        // view pointer identity, not the wgpu view address - the swap chain
        // re-registers a fresh back-buffer RTV under the same wgpu dummy
        // address every frame, so an address-keyed framebuffer would
        // silently target the first frame's back buffer. Engine objects are
        // device-bound, so the key-computation pure function is pinned here
        // instead of the full cache. The pointers below are never
        // dereferenced - only their identity is compared.
        let stable_view = 0x1_0000usize as *mut sys::ITextureView;
        let next_back_buffer = 0x2_0000usize as *mut sys::ITextureView;

        // A regular texture resolves to the same diligent pointer every
        // frame: the key repeats and the cache hits.
        let key_frame_1 = PassCacheKey {
            attachments: Vec::new(),
            views: resolved_view_keys(&[stable_view]),
        };
        let key_frame_2 = PassCacheKey {
            attachments: Vec::new(),
            views: resolved_view_keys(&[stable_view]),
        };
        assert_eq!(key_frame_1, key_frame_2);

        // A swap-chain attachment resolves to the CURRENT back-buffer RTV -
        // a fresh pointer per frame: the key misses and a framebuffer for
        // the current back buffer is created (bounded by the cache cap).
        let key_next_frame = PassCacheKey {
            attachments: Vec::new(),
            views: resolved_view_keys(&[next_back_buffer]),
        };
        assert_ne!(key_frame_1, key_next_frame);
    }

    /// M2a-2: the read-only depth-stencil expression. wgpu 29 marks an
    /// aspect read-only with `*_ops: None`; the locked Diligent version has
    /// a single all-or-nothing read-only state (both aspects) triggered by
    /// the subpass reference state `RESOURCE_STATE_DEPTH_READ` - only the
    /// both-None combination maps to it.
    #[test]
    fn depth_stencil_attachment_state_follows_the_read_only_rule() {
        let read = sys::_RESOURCE_STATE::RESOURCE_STATE_DEPTH_READ as sys::RESOURCE_STATE;
        let write = sys::_RESOURCE_STATE::RESOURCE_STATE_DEPTH_WRITE as sys::RESOURCE_STATE;
        // Both aspects read-only (`depth_ops: None` + `stencil_ops: None` -
        // the wgpu read-only expression) -> DEPTH_READ.
        assert_eq!(depth_stencil_attachment_state(true, true), read);
        // Any other combination (the pass writes at least one aspect) ->
        // DEPTH_WRITE. The mixed cases (depth write + stencil read-only and
        // vice versa) have no per-aspect read-only DSV in this version.
        assert_eq!(depth_stencil_attachment_state(false, true), write);
        assert_eq!(depth_stencil_attachment_state(true, false), write);
        assert_eq!(depth_stencil_attachment_state(false, false), write);
    }

    /// M2a-2: the read-only flag is part of the render-pass cache key - two
    /// passes that differ only in read-only-ness must not share a
    /// render-pass object (the subpass reference states are baked into it).
    #[test]
    fn pass_cache_key_distinguishes_read_only_depth() {
        let attachment = PassAttachmentKey {
            format: sys::_TEXTURE_FORMAT::TEX_FORMAT_D32_FLOAT as i32,
            sample_count: 1,
            load_op: sys::_ATTACHMENT_LOAD_OP::ATTACHMENT_LOAD_OP_LOAD as u8,
            store_op: sys::_ATTACHMENT_STORE_OP::ATTACHMENT_STORE_OP_STORE as u8,
            read_only: true,
        };
        let mut writable = attachment.clone();
        writable.read_only = false;
        assert_ne!(attachment, writable);
    }
}
