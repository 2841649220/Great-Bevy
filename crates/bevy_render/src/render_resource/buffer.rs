use crate::renderer::diligent_registry::DiligentHandle;
use alloc::sync::Arc;
use bevy_utils::define_atomic_id;
use core::ops::{Deref, RangeBounds};
use std::sync::Mutex;
use wgpu_types::{BufferAddress, BufferUsages};

define_atomic_id!(BufferId);

/// A GPU-accessible buffer.
///
/// The primary handle is a Diligent [`IBuffer`](diligent_rs::Buffer)
/// (M1-4b-2: the transition wgpu buffer is gone - `Deref` no longer exists;
/// size/usage/as_entire_binding are inherent methods).
///
/// Can be created via [`RenderDevice::create_buffer`](crate::renderer::RenderDevice::create_buffer).
#[derive(Clone)]
pub struct Buffer {
    pub(crate) id: BufferId,
    /// The Diligent buffer. `None` only when the Diligent creation failed
    /// (logged).
    pub(crate) value: Option<DiligentHandle<diligent_rs::Buffer>>,
    /// The size of the buffer allocation in bytes.
    pub(crate) size: u64,
    /// The usages the buffer was created with.
    pub(crate) usage: BufferUsages,
    /// The diligent immediate context of the creating device (used by the
    /// blocking map paths; the handle is `Send + Sync`).
    pub(crate) context_handle: Option<DiligentHandle<diligent_rs::DeviceContext>>,
    /// The mapped readback data (set by `RenderDevice::map_buffer` /
    /// `CommandEncoder::map_buffer_on_submit`; cleared by `unmap`).
    pub(crate) mapped: Arc<Mutex<Option<Vec<u8>>>>,
    /// A pending texture readback (set by `CommandEncoder::copy_texture_to_buffer`;
    /// executed by `BufferSlice::map_async` - the diligent copy API has no
    /// texture-to-buffer direction, so the readback stages through a
    /// same-format texture).
    pub(crate) pending_readback: Arc<Mutex<Option<crate::texture::TextureReadbackPending>>>,
}

impl core::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Buffer")
            .field("id", &self.id)
            .field("diligent", &self.value.is_some())
            .field("size", &self.size)
            .finish()
    }
}

impl Buffer {
    #[inline]
    pub fn id(&self) -> BufferId {
        self.id
    }

    /// Returns a [`BufferSlice`] referring to the portion of `self`'s
    /// contents indicated by `bounds`, in bytes.
    #[track_caller]
    pub fn slice(&self, bounds: impl RangeBounds<BufferAddress>) -> BufferSlice<'_> {
        let start = match bounds.start_bound() {
            core::ops::Bound::Included(&b) => b,
            core::ops::Bound::Excluded(&b) => b + 1,
            core::ops::Bound::Unbounded => 0,
        };
        let end = match bounds.end_bound() {
            core::ops::Bound::Included(&b) => b + 1,
            core::ops::Bound::Excluded(&b) => b,
            core::ops::Bound::Unbounded => self.size,
        };
        assert!(
            end > start && end <= self.size,
            "slice {}..{} is out of range for buffer of size {}",
            start,
            end,
            self.size
        );
        let size = end - start;
        BufferSlice {
            inner: WgpuBufferSlice {
                buffer: self,
                offset: start,
                size: core::num::NonZeroU64::new(size).unwrap(),
            },
        }
    }

    /// Gains read access to the mapped bytes of the buffer (the buffer must
    /// have been mapped via map_buffer_on_submit/RenderDevice::map_buffer).
    #[track_caller]
    pub fn get_mapped_range(&self) -> BufferView {
        self.slice(..).get_mapped_range()
    }

    /// Unmaps the buffer from host memory (discards the mapped readback
    /// data).
    pub fn unmap(&self) {
        *self.mapped.lock().unwrap() = None;
    }

    /// Returns the length of the buffer allocation in bytes.
    pub fn size(&self) -> BufferAddress {
        self.size
    }

    /// Returns the usages this buffer was created with.
    pub fn usage(&self) -> BufferUsages {
        self.usage
    }

    /// Returns the binding view of the entire buffer.
    pub fn as_entire_binding(&self) -> crate::render_resource::BindingResource<'_> {
        crate::render_resource::BindingResource::Buffer(self.as_entire_buffer_binding())
    }

    /// Returns the binding view of the entire buffer.
    pub fn as_entire_buffer_binding(&self) -> crate::render_resource::BufferBinding<'_> {
        crate::render_resource::BufferBinding {
            buffer: self,
            offset: 0,
            size: None,
        }
    }

    /// Stores the mapped readback data (consumed by `get_mapped_range`).
    pub(crate) fn store_mapped(&self, data: Vec<u8>) {
        *self.mapped.lock().unwrap() = Some(data);
    }

    /// The Diligent buffer, when this instance has one.
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::Buffer> {
        self.value.as_deref()
    }

    /// The diligent immediate context, when this buffer was created by a
    /// device with an engine (used by the blocking map paths).
    pub(crate) fn context(&self) -> Option<&diligent_rs::DeviceContext> {
        self.context_handle.as_deref()
    }
}

/// A slice of a [`Buffer`], to be used for vertex or index data or to be
/// mapped.
#[derive(Clone, Copy, Debug)]
pub struct BufferSlice<'a> {
    inner: WgpuBufferSlice<'a>,
}

/// The deref target of [`BufferSlice`] (mirrors the wgpu `BufferSlice` shape:
/// consumer code that derefs a bevy `BufferSlice` - e.g. `*buffer.slice(..)` -
/// receives a value of this type, which converts back to a [`BufferSlice`]).
#[derive(Clone, Copy, Debug)]
pub struct WgpuBufferSlice<'a> {
    buffer: &'a Buffer,
    offset: BufferAddress,
    size: core::num::NonZeroU64,
}

impl<'a> Deref for BufferSlice<'a> {
    type Target = WgpuBufferSlice<'a>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> From<WgpuBufferSlice<'a>> for BufferSlice<'a> {
    fn from(value: WgpuBufferSlice<'a>) -> Self {
        Self { inner: value }
    }
}

impl<'a> From<BufferSlice<'a>> for WgpuBufferSlice<'a> {
    fn from(value: BufferSlice<'a>) -> Self {
        value.inner
    }
}

impl<'a> BufferSlice<'a> {
    /// Returns the [`BufferId`] of the buffer this is a slice of.
    #[inline]
    pub fn id(&self) -> BufferId {
        self.inner.buffer.id
    }

    /// Returns the buffer this is a slice of.
    pub fn buffer(&self) -> &'a Buffer {
        self.inner.buffer
    }

    /// Returns the offset in the buffer this slice starts at.
    pub fn offset(&self) -> BufferAddress {
        self.inner.offset
    }

    /// Returns the size of this slice.
    pub fn size(&self) -> crate::render_resource::BufferSize {
        self.inner.size
    }

    /// Gains read access to the mapped bytes of the buffer (the buffer must
    /// have been mapped via `map_buffer_on_submit`/`RenderDevice::map_buffer`).
    #[track_caller]
    pub fn get_mapped_range(&self) -> BufferView {
        let data = self
            .inner
            .buffer
            .mapped
            .lock()
            .unwrap()
            .clone()
            .expect("tried to call get_mapped_range on an unmapped buffer");
        BufferView {
            data: data
                [self.inner.offset as usize..self.inner.offset as usize + self.inner.size.get() as usize]
                .to_vec(),
        }
    }

    /// Maps the buffer to host memory and invokes the callback with the
    /// result (blocking map: the diligent readback executes immediately).
    ///
    /// When a texture readback was recorded for this buffer via
    /// `CommandEncoder::copy_texture_to_buffer`, the readback is executed
    /// here (the copy is recorded on the immediate context in command order,
    /// after the work that produced the texture contents).
    pub fn map_async(
        &self,
        mode: crate::render_resource::MapMode,
        callback: impl FnOnce(Result<(), crate::render_resource::BufferAsyncError>) + Send + 'static,
    ) {
        let result = self.map_async_blocking(mode);
        callback(result);
    }

    fn map_async_blocking(
        &self,
        mode: crate::render_resource::MapMode,
    ) -> Result<(), crate::render_resource::BufferAsyncError> {
        let buffer = self.inner.buffer;
        if let Some(pending) = buffer.pending_readback.lock().unwrap().take() {
            return crate::texture::execute_texture_readback(&pending, buffer);
        }
        let Some(context) = buffer.context() else {
            return Err(crate::render_resource::BufferAsyncError);
        };
        let Some(diligent) = buffer.diligent() else {
            return Err(crate::render_resource::BufferAsyncError);
        };
        let map_type = match mode {
            crate::render_resource::MapMode::Read => {
                diligent_rs::diligent_sys::bindings::_MAP_TYPE::MAP_READ as _
            }
            crate::render_resource::MapMode::Write => {
                diligent_rs::diligent_sys::bindings::_MAP_TYPE::MAP_WRITE as _
            }
        };
        // M1-4b-2 review, fix 1: the direct `MapBuffer` is an
        // immediate-context call - serialize it (the texture-readback path
        // above takes its own guard inside `execute_texture_readback`, so
        // this guard must stay scoped to the direct map).
        let _guard = crate::renderer::diligent_registry::context_guard();
        let mapped = context
            .map_buffer(diligent, map_type, false)
            .map_err(|_| crate::render_resource::BufferAsyncError)?;
        let Some(mapped) = mapped else {
            return Err(crate::render_resource::BufferAsyncError);
        };
        let data = mapped.as_slice().to_vec();
        buffer.store_mapped(data);
        Ok(())
    }
}

impl<'a> From<BufferSlice<'a>> for crate::render_resource::BufferBinding<'a> {
    fn from(value: BufferSlice<'a>) -> Self {
        let inner = value.inner;
        crate::render_resource::BufferBinding {
            buffer: inner.buffer,
            offset: inner.offset,
            size: Some(inner.size),
        }
    }
}

impl<'a> From<BufferSlice<'a>> for crate::render_resource::BindingResource<'a> {
    fn from(value: BufferSlice<'a>) -> Self {
        crate::render_resource::BindingResource::Buffer(value.into())
    }
}

/// A read-only view of a mapped buffer's bytes.
#[derive(Debug)]
pub struct BufferView {
    data: Vec<u8>,
}

impl Deref for BufferView {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        &self.data
    }
}

impl AsRef<[u8]> for BufferView {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}
