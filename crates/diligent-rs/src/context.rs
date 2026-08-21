//! Immediate device context: state setting, draw commands, flushing and
//! CPU-GPU data transfer (buffer/texture updates, copies and readback maps).

use std::marker::PhantomData;

use diligent_sys::bindings as sys;

use crate::error::{Error, Result};
use crate::handle::{Handle, NonOwning};
use crate::resource::{Buffer, Fence, PipelineState, ShaderResourceBinding};
use crate::texture::Texture;

/// Owning handle to the immediate device context (`IDeviceContext`).
///
/// Single-threaded by design: all methods mutate the engine's command
/// recording state and must be called from the thread that created this
/// context (see the crate-level threading notes).
pub struct DeviceContext {
    handle: Handle<sys::IDeviceContext>,
}

impl DeviceContext {
    /// Wraps a device-context pointer returned by the engine.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `IDeviceContext` instance returned by
    /// `IEngineFactory::CreateDeviceAndContexts` (the engine AddRefs it);
    /// ownership is transferred to the wrapper, which releases it on drop.
    /// Only engine-returned pointers may be passed here - arbitrary pointers
    /// would be dereferenced on drop.
    pub unsafe fn from_raw(ptr: *mut sys::IDeviceContext) -> Self {
        Self {
            handle: Handle::from_raw(ptr),
        }
    }

    /// The raw context pointer (for escape hatches into the C API).
    pub fn as_raw(&self) -> *mut sys::IDeviceContext {
        self.handle.as_ptr()
    }

    fn vtbl(&self) -> &sys::IDeviceContextMethods {
        unsafe { &(*(*self.as_raw()).pVtbl).DeviceContext }
    }

    /// Binds the pipeline state for subsequent draws.
    pub fn set_pipeline_state(&self, pso: &PipelineState) {
        let set = self
            .vtbl()
            .SetPipelineState
            .expect("diligent-rs: IDeviceContext::SetPipelineState missing from vtable");
        // Safety: `pso` is a live pipeline state held by the caller.
        unsafe { set(self.as_raw(), pso.as_raw()) };
    }

    /// Commits shader resources of an SRB, transitioning them to the states
    /// required by the draw command.
    pub fn commit_shader_resources(&self, srb: Option<&ShaderResourceBinding>) {
        let commit = self
            .vtbl()
            .CommitShaderResources
            .expect("diligent-rs: IDeviceContext::CommitShaderResources missing from vtable");
        // Safety: `srb` is a live SRB held by the caller (or null, which the
        // engine allows when the pipeline has no shader resources).
        unsafe {
            commit(
                self.as_raw(),
                srb.map_or(std::ptr::null_mut(), |s| s.as_raw()),
                sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as sys::RESOURCE_STATE_TRANSITION_MODE,
            )
        };
    }

    /// Binds vertex buffers starting at `start_slot`, with one byte offset
    /// per buffer. The context keeps strong references to the buffers.
    pub fn set_vertex_buffers(&self, start_slot: u32, buffers: &[&Buffer], offsets: &[u64]) -> Result<()> {
        if buffers.len() != offsets.len() {
            return Err(Error::InvalidArgument(
                "buffers and offsets must have the same length",
            ));
        }
        let buffer_ptrs: Vec<*mut sys::IBuffer> = buffers.iter().map(|b| b.as_raw()).collect();
        let set = self
            .vtbl()
            .SetVertexBuffers
            .expect("diligent-rs: IDeviceContext::SetVertexBuffers missing from vtable");
        // Safety: the arrays are valid for the duration of the call and all
        // buffers are alive; the RESET flag unbinds previous buffers.
        unsafe {
            set(
                self.as_raw(),
                start_slot,
                buffer_ptrs.len() as u32,
                buffer_ptrs.as_ptr(),
                offsets.as_ptr(),
                sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as sys::RESOURCE_STATE_TRANSITION_MODE,
                sys::_SET_VERTEX_BUFFERS_FLAGS::SET_VERTEX_BUFFERS_FLAG_RESET
                    as sys::SET_VERTEX_BUFFERS_FLAGS,
            )
        };
        Ok(())
    }

    /// Binds render targets; render target size for viewports is derived
    /// from the bound targets (0, 0).
    pub fn set_render_targets(&self, rtvs: &[NonOwning<sys::ITextureView>]) {
        let view_ptrs: Vec<*mut sys::ITextureView> = rtvs.iter().map(|v| v.as_ptr()).collect();
        let set = self
            .vtbl()
            .SetRenderTargets
            .expect("diligent-rs: IDeviceContext::SetRenderTargets missing from vtable");
        // Safety: the array is valid for the duration of the call; the
        // views are borrowed from the swap chain, which is held by the
        // caller.
        unsafe {
            set(
                self.as_raw(),
                view_ptrs.len() as u32,
                view_ptrs.as_ptr().cast_mut(),
                std::ptr::null_mut(),
                sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as sys::RESOURCE_STATE_TRANSITION_MODE,
            )
        };
    }

    /// Sets the viewport array (DirectX convention).
    pub fn set_viewports(&self, viewports: &[sys::Viewport]) {
        let set = self
            .vtbl()
            .SetViewports
            .expect("diligent-rs: IDeviceContext::SetViewports missing from vtable");
        // Safety: the viewport array is valid for the duration of the call;
        // RT width/height 0 = derive from bound render target.
        unsafe {
            set(
                self.as_raw(),
                viewports.len() as u32,
                viewports.as_ptr(),
                0,
                0,
            )
        };
    }

    /// Clears the render target view to `color` (RGBA, 0..1).
    pub fn clear_render_target(&self, rtv: &NonOwning<sys::ITextureView>, color: [f32; 4]) {
        let clear = self
            .vtbl()
            .ClearRenderTarget
            .expect("diligent-rs: IDeviceContext::ClearRenderTarget missing from vtable");
        // Safety: `rtv` is a live render target view held (borrowed) by the
        // caller; the color array is valid for the duration of the call.
        unsafe {
            clear(
                self.as_raw(),
                rtv.as_ptr(),
                color.as_ptr().cast(),
                sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as sys::RESOURCE_STATE_TRANSITION_MODE,
            )
        };
    }

    /// Issues a non-indexed draw with `num_vertices` vertices, 1 instance.
    pub fn draw(&self, num_vertices: u32) -> Result<()> {
        if num_vertices == 0 {
            return Err(Error::InvalidArgument("num_vertices must be > 0"));
        }
        let attribs = sys::DrawAttribs {
            NumVertices: num_vertices,
            Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
            NumInstances: 1,
            StartVertexLocation: 0,
            FirstInstanceLocation: 0,
        };
        let draw = self
            .vtbl()
            .Draw
            .expect("diligent-rs: IDeviceContext::Draw missing from vtable");
        // Safety: `attribs` is a valid draw command description.
        unsafe { draw(self.as_raw(), &attribs) };
        Ok(())
    }

    /// Submits the current command list to the GPU.
    pub fn flush(&self) {
        let flush = self
            .vtbl()
            .Flush
            .expect("diligent-rs: IDeviceContext::Flush missing from vtable");
        // Safety: no arguments beyond the context itself.
        unsafe { flush(self.as_raw()) };
    }

    /// Notifies the engine the frame is complete so it can reclaim stale
    /// resources. Must be called once per frame.
    pub fn finish_frame(&self) {
        let finish = self
            .vtbl()
            .FinishFrame
            .expect("diligent-rs: IDeviceContext::FinishFrame missing from vtable");
        // Safety: no arguments beyond the context itself.
        unsafe { finish(self.as_raw()) };
    }

    /// Enqueues a GPU signal of `fence` to `value` after all previously
    /// submitted commands complete (`IDeviceContext::EnqueueSignal`,
    /// bindings.rs:12246). Pair with [`Fence::wait`] on the CPU to stall
    /// until a staging readback is ready.
    pub fn enqueue_signal(&self, fence: &Fence, value: u64) -> Result<()> {
        let signal = self
            .vtbl()
            .EnqueueSignal
            .ok_or(Error::MissingMethod("IDeviceContext::EnqueueSignal"))?;
        // Safety: `fence` is alive and was created as FENCE_TYPE_GENERAL.
        unsafe { signal(self.as_raw(), fence.as_raw(), value) };
        Ok(())
    }

    /// Updates the contents of a buffer in GPU memory
    /// (`IDeviceContext::UpdateBuffer` - the `write_buffer` equivalent).
    ///
    /// `offset + data.len()` must not exceed the buffer size. The update is
    /// recorded in command order at the point of the call.
    pub fn update_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) -> Result<()> {
        let size = buffer.size()?;
        let end = offset.checked_add(data.len() as u64).ok_or_else(|| {
            Error::InvalidArgument("buffer update range overflows u64")
        })?;
        if end > size {
            return Err(Error::Message(format!(
                "buffer update range {offset}..{end} exceeds the buffer size {size}"
            )));
        }
        let update = self
            .vtbl()
            .UpdateBuffer
            .ok_or(Error::MissingMethod("IDeviceContext::UpdateBuffer"))?;
        // Safety: `buffer` is alive, `data` is valid for the duration of the
        // call (the engine copies it synchronously) and the range was
        // validated against the buffer size above.
        unsafe {
            update(
                self.as_raw(),
                buffer.as_raw(),
                offset,
                data.len() as u64,
                data.as_ptr().cast(),
                sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as sys::RESOURCE_STATE_TRANSITION_MODE,
            )
        };
        Ok(())
    }

    /// Copies a byte range from one buffer into another
    /// (`IDeviceContext::CopyBuffer`). The copy is recorded in command
    /// order; for readback the destination is a staging buffer (see
    /// [`RenderDevice::create_staging_buffer`](crate::RenderDevice::create_staging_buffer)).
    pub fn copy_buffer(
        &self,
        src: &Buffer,
        src_offset: u64,
        dst: &Buffer,
        dst_offset: u64,
        size: u64,
    ) -> Result<()> {
        if size == 0 {
            return Err(Error::InvalidArgument("copy size must be > 0"));
        }
        let src_size = src.size()?;
        let dst_size = dst.size()?;
        if src_offset
            .checked_add(size)
            .is_none_or(|end| end > src_size)
        {
            return Err(Error::Message(format!(
                "copy source range {src_offset}..{} exceeds the source buffer size {src_size}",
                src_offset + size
            )));
        }
        if dst_offset
            .checked_add(size)
            .is_none_or(|end| end > dst_size)
        {
            return Err(Error::Message(format!(
                "copy destination range {dst_offset}..{} exceeds the destination buffer size {dst_size}",
                dst_offset + size
            )));
        }
        let copy = self
            .vtbl()
            .CopyBuffer
            .ok_or(Error::MissingMethod("IDeviceContext::CopyBuffer"))?;
        let transition =
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE;
        // Safety: both buffers are alive and the ranges were validated
        // against their sizes above.
        unsafe {
            copy(
                self.as_raw(),
                src.as_raw(),
                src_offset,
                transition,
                dst.as_raw(),
                dst_offset,
                size,
                transition,
            )
        };
        Ok(())
    }

    /// Copies a whole texture subresource into another texture's subresource
    /// (`IDeviceContext::CopyTexture` with a null source box = the entire
    /// subresource). For readback the destination is a staging texture of the
    /// same format and size (see
    /// [`RenderDevice::create_staging_texture`](crate::RenderDevice::create_staging_texture)).
    pub fn copy_texture(
        &self,
        src: &Texture,
        src_mip_level: u32,
        src_slice: u32,
        dst: &Texture,
        dst_mip_level: u32,
        dst_slice: u32,
    ) -> Result<()> {
        let mut attribs: sys::CopyTextureAttribs = unsafe { std::mem::zeroed() };
        attribs.pSrcTexture = src.as_raw();
        attribs.SrcMipLevel = src_mip_level;
        attribs.SrcSlice = src_slice;
        // Null source box: the engine copies the entire subresource.
        attribs.pSrcBox = std::ptr::null();
        attribs.SrcTextureTransitionMode =
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE;
        attribs.pDstTexture = dst.as_raw();
        attribs.DstMipLevel = dst_mip_level;
        attribs.DstSlice = dst_slice;
        attribs.DstX = 0;
        attribs.DstY = 0;
        attribs.DstZ = 0;
        attribs.DstTextureTransitionMode =
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE;
        let copy = self
            .vtbl()
            .CopyTexture
            .ok_or(Error::MissingMethod("IDeviceContext::CopyTexture"))?;
        // Safety: both textures are alive for the duration of the call; the
        // engine copies the subresource synchronously into the command
        // stream.
        unsafe { copy(self.as_raw(), &attribs) };
        Ok(())
    }

    /// Maps a buffer for CPU access (`IDeviceContext::MapBuffer`).
    ///
    /// Returns `Ok(None)` when `do_not_wait` is set and the GPU has not
    /// finished using the buffer yet (the cross-frame readback pattern:
    /// retry on a later frame); with `do_not_wait` false the call blocks
    /// until the data is available (the synchronous fallback).
    ///
    /// The returned [`MappedBuffer`] unmaps the buffer on drop; the map
    /// region covers the whole buffer.
    ///
    /// The caller must keep `buffer` alive until the returned
    /// [`MappedBuffer`] is dropped (it holds the buffer's raw pointer; the
    /// unmap would otherwise hit a released object).
    pub fn map_buffer(
        &self,
        buffer: &Buffer,
        map_type: sys::MAP_TYPE,
        do_not_wait: bool,
    ) -> Result<Option<MappedBuffer<'_>>> {
        let size = buffer.size()?;
        let map_flags = if do_not_wait {
            sys::_MAP_FLAGS::MAP_FLAG_DO_NOT_WAIT as sys::MAP_FLAGS
        } else {
            sys::_MAP_FLAGS::MAP_FLAG_NONE as sys::MAP_FLAGS
        };
        let map = self
            .vtbl()
            .MapBuffer
            .ok_or(Error::MissingMethod("IDeviceContext::MapBuffer"))?;
        let mut mapped: sys::PVoid = std::ptr::null_mut();
        // Safety: `buffer` is alive; `mapped` is an out param.
        unsafe {
            map(self.as_raw(), buffer.as_raw(), map_type, map_flags, &mut mapped)
        };
        if mapped.is_null() {
            return Ok(None);
        }
        Ok(Some(MappedBuffer {
            context: self.as_raw(),
            buffer: buffer.as_raw(),
            map_type,
            data: mapped.cast::<u8>(),
            size: size as usize,
            _marker: PhantomData,
        }))
    }

    /// Maps a texture subresource for CPU access
    /// (`IDeviceContext::MapTextureSubresource`; null region = the whole
    /// subresource - the staging-texture readback pattern).
    ///
    /// Returns `Ok(None)` when `do_not_wait` is set and the GPU has not
    /// finished using the subresource yet; with `do_not_wait` false the call
    /// blocks until the data is available.
    ///
    /// The returned [`MappedTexture`] unmaps the subresource on drop.
    pub fn map_texture_subresource(
        &self,
        texture: &Texture,
        mip_level: u32,
        array_slice: u32,
        map_type: sys::MAP_TYPE,
        do_not_wait: bool,
    ) -> Result<Option<MappedTexture<'_>>> {
        let map_flags = if do_not_wait {
            sys::_MAP_FLAGS::MAP_FLAG_DO_NOT_WAIT as sys::MAP_FLAGS
        } else {
            sys::_MAP_FLAGS::MAP_FLAG_NONE as sys::MAP_FLAGS
        };
        let map = self
            .vtbl()
            .MapTextureSubresource
            .ok_or(Error::MissingMethod("IDeviceContext::MapTextureSubresource"))?;
        let mut mapped: sys::MappedTextureSubresource = unsafe { std::mem::zeroed() };
        // Safety: `texture` is alive; `mapped` is an out param; the null map
        // region maps the entire subresource.
        unsafe {
            map(
                self.as_raw(),
                texture.as_raw(),
                mip_level,
                array_slice,
                map_type,
                map_flags,
                std::ptr::null(),
                &mut mapped,
            )
        };
        if mapped.pData.is_null() {
            return Ok(None);
        }
        Ok(Some(MappedTexture {
            context: self.as_raw(),
            texture: texture.as_raw(),
            mip_level,
            array_slice,
            data: mapped.pData.cast::<u8>(),
            stride: mapped.Stride as usize,
            depth_stride: mapped.DepthStride as usize,
            _marker: PhantomData,
        }))
    }
}

/// A buffer mapped for CPU access; unmaps on drop (`IDeviceContext::UnmapBuffer`
/// with the map type the buffer was mapped with).
///
/// The mapped buffer must outlive this object (it holds the buffer's raw
/// pointer, used by the unmap on drop).
pub struct MappedBuffer<'a> {
    context: *mut sys::IDeviceContext,
    buffer: *mut sys::IBuffer,
    map_type: sys::MAP_TYPE,
    data: *mut u8,
    size: usize,
    _marker: PhantomData<&'a DeviceContext>,
}

impl MappedBuffer<'_> {
    /// The mapped bytes (the whole buffer).
    pub fn as_slice(&self) -> &[u8] {
        // Safety: the map region covers the whole buffer (the wrapper only
        // ever maps whole buffers) and the mapping is alive until this
        // object is dropped.
        unsafe { std::slice::from_raw_parts(self.data, self.size) }
    }

    /// The raw mapped pointer (for escape hatches into the C API).
    pub fn as_ptr(&self) -> *mut u8 {
        self.data
    }

    /// The mapped size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for MappedBuffer<'_> {
    fn drop(&mut self) {
        let unmap = unsafe {
            (*(*self.context).pVtbl)
                .DeviceContext
                .UnmapBuffer
                .as_ref()
                .expect("diligent-rs: IDeviceContext::UnmapBuffer missing from vtable")
        };
        // Safety: the buffer was mapped with `map_type` by this context and
        // is unmapped exactly once here.
        unsafe { unmap(self.context, self.buffer, self.map_type) };
    }
}

/// A texture subresource mapped for CPU access; unmaps on drop
/// (`IDeviceContext::UnmapTextureSubresource` with the map type the
/// subresource was mapped with).
///
/// Rows are `stride` bytes apart (`MappedTextureSubresource::Stride`); the
/// last row may be shorter than `stride` when the texture width is not
/// row-pitch aligned.
pub struct MappedTexture<'a> {
    context: *mut sys::IDeviceContext,
    texture: *mut sys::ITexture,
    mip_level: u32,
    array_slice: u32,
    data: *mut u8,
    stride: usize,
    depth_stride: usize,
    _marker: PhantomData<&'a DeviceContext>,
}

impl MappedTexture<'_> {
    /// The row stride in bytes (`MappedTextureSubresource::Stride`).
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// The depth-slice stride in bytes (`MappedTextureSubresource::DepthStride`).
    pub fn depth_stride(&self) -> usize {
        self.depth_stride
    }

    /// The raw mapped pointer (for escape hatches into the C API).
    pub fn as_ptr(&self) -> *mut u8 {
        self.data
    }

    /// The pointer to row `row` (rows are `stride()` bytes apart).
    pub fn row(&self, row: usize) -> *mut u8 {
        // Safety: the mapping is alive until this object is dropped; the
        // caller must stay within the mapped region.
        unsafe { self.data.add(row * self.stride) }
    }
}

impl Drop for MappedTexture<'_> {
    fn drop(&mut self) {
        let unmap = unsafe {
            (*(*self.context).pVtbl)
                .DeviceContext
                .UnmapTextureSubresource
                .as_ref()
                .expect("diligent-rs: IDeviceContext::UnmapTextureSubresource missing from vtable")
        };
        // Safety: the subresource was mapped with `map_type` by this
        // context and is unmapped exactly once here.
        unsafe { unmap(self.context, self.texture, self.mip_level, self.array_slice) };
    }
}
