//! Shape-equivalent replacements for the wgpu 29.0.4 runtime types that
//! `render_resource` re-exports (M1-4b-2: the wgpu runtime is gone from the
//! tree; wgpu-types 29.0.4 provides the pure-type layer, and the types below
//! mirror the wgpu 29.0.4 API shapes - field names / field types / lifetimes -
//! so consumer crates compile and construct them unchanged).
//!
//! Every type here was verified against the wgpu 29.0.4 sources in the cargo
//! registry cache (see the M1-4b-2 report §2 for the per-type provenance):
//!
//! * type aliases over wgpu-types generic descriptors reproduce the wgpu
//!   aliases exactly (e.g. `BufferDescriptor<'a>` =
//!   `wgt::BufferDescriptor<Label<'a>>` with `Label<'a>` = `Option<&'a str>`);
//! * the handle types (`ShaderModule`, `PipelineLayout`, `Blas`, `Tlas`,
//!   `WgpuTextureView`, `WgpuSampler`, `WgpuBindGroup`, `QuerySet`) are
//!   self-authored: the diligent-side objects are the only GPU state;
//! * the descriptor families (`BindGroupDescriptor`, `RenderPassDescriptor`,
//!   `RawRenderPipelineDescriptor`, ...) reference bevy's own leaf types
//!   through the same `&`-shape wgpu's referenced its handles with (consumer
//!   code passing bevy objects compiles via direct references or the
//!   `Deref` bridges the leaf wrappers expose to these handle types);
//! * `CommandEncoder`/`RenderPass`/`ComputePass`/`CommandBuffer` record
//!   directly on the Diligent immediate context at encode time (the render
//!   graph records there anyway; `finish`/`submit` become ordering no-ops).
//!
//! The `ExternalTexture` variant payload of [`BindingResource`] is a hidden
//! marker (bevy never had an `ExternalTexture` type; consumers never name
//! the variant's payload type).

use crate::render_resource::{
    BindGroupLayout, Buffer, BufferSize, BufferSlice, ColorTargetState, DepthStencilState,
    IndexFormat, MultisampleState, PrimitiveState, TextureFormat,
};
use crate::renderer::diligent_registry::DiligentHandle;
use alloc::borrow::Cow;
use alloc::sync::Arc;
use bevy_color::ColorToComponents;
use bevy_utils::define_atomic_id;
use core::num::NonZeroU32;
use wgpu_types::{BufferAddress, Extent3d};

/// Object debugging label (the wgpu `Label<'a>` shape).
pub type Label<'a> = Option<&'a str>;

/// The index of a submission.
///
/// Shape deviation (M1-4b-2 review, fix 2): wgpu 29.0.4 defines this as a
/// newtype struct (`api/queue.rs:45`), not a `u64` alias, so
/// `PollType = wgpu_types::PollType<SubmissionIndex>` matches wgpu's shape
/// only if the newtype is reproduced. The field is crate-internal: nothing
/// constructs or reads a submission index (the diligent submit is an
/// ordering no-op that returns `SubmissionIndex(0)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubmissionIndex(pub(crate) u64);

define_atomic_id!(ShaderModuleId);
define_atomic_id!(PipelineLayoutId);

// ---------------------------------------------------------------------------
// Type aliases over the wgpu-types generic descriptors. In wgpu 29.0.4 these
// are `pub type X<'a> = wgt::X<Label<'a>>` aliases; reproducing the alias
// over the identical wgpu-types generic keeps the shapes byte-for-byte.
// ---------------------------------------------------------------------------

pub type BufferDescriptor<'a> = wgpu_types::BufferDescriptor<Label<'a>>;
pub type CommandEncoderDescriptor<'a> = wgpu_types::CommandEncoderDescriptor<Label<'a>>;
pub type TextureDescriptor<'a> = wgpu_types::TextureDescriptor<Label<'a>, &'a [TextureFormat]>;
pub type TextureViewDescriptor<'a> = wgpu_types::TextureViewDescriptor<Label<'a>>;
pub type SamplerDescriptor<'a> = wgpu_types::SamplerDescriptor<Label<'a>>;
pub type CreateBlasDescriptor<'a> = wgpu_types::CreateBlasDescriptor<Label<'a>>;
pub type CreateTlasDescriptor<'a> = wgpu_types::CreateTlasDescriptor<Label<'a>>;
pub type PollType = wgpu_types::PollType<SubmissionIndex>;

/// The wgpu `TexelCopyBufferInfo` alias shape: the buffer handle the info
/// references is bevy's [`Buffer`].
pub type TexelCopyBufferInfo<'a> = wgpu_types::TexelCopyBufferInfo<&'a Buffer>;

/// The wgpu `TexelCopyTextureInfo` alias shape: the texture handle the info
/// references is bevy's [`Texture`].
pub type TexelCopyTextureInfo<'a> = wgpu_types::TexelCopyTextureInfo<&'a crate::render_resource::Texture>;

// ---------------------------------------------------------------------------
// Buffer mapping
// ---------------------------------------------------------------------------

/// Error occurred when trying to async map a buffer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BufferAsyncError;

impl core::fmt::Display for BufferAsyncError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Error occurred when trying to async map a buffer")
    }
}

impl core::error::Error for BufferAsyncError {}

/// Type of buffer mapping.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MapMode {
    /// Map only for reading
    Read,
    /// Map only for writing
    Write,
}

// ---------------------------------------------------------------------------
// Bind groups
// ---------------------------------------------------------------------------

/// Resource to be bound by a [`BindGroup`](crate::render_resource::BindGroup)
/// for use with a pipeline.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum BindingResource<'a> {
    /// Binding is backed by a buffer.
    Buffer(BufferBinding<'a>),
    /// Binding is backed by an array of buffers.
    BufferArray(&'a [BufferBinding<'a>]),
    /// Binding is a sampler.
    Sampler(&'a WgpuSampler),
    /// Binding is backed by an array of samplers.
    SamplerArray(&'a [&'a WgpuSampler]),
    /// Binding is backed by a texture.
    TextureView(&'a WgpuTextureView),
    /// Binding is backed by an array of textures.
    TextureViewArray(&'a [&'a WgpuTextureView]),
    /// Binding is backed by a top level acceleration structure.
    AccelerationStructure(&'a Tlas),
    /// Binding is backed by an array of top level acceleration structures.
    AccelerationStructureArray(&'a [&'a Tlas]),
    /// Binding is backed by an external texture (unused by bevy; kept for
    /// the wgpu 29.0.4 variant set).
    ExternalTexture(&'a ExternalTexture),
}

/// Describes the segment of a buffer to bind.
#[derive(Clone, Debug)]
pub struct BufferBinding<'a> {
    /// The buffer to bind.
    pub buffer: &'a Buffer,
    /// Base offset of the buffer, in bytes.
    pub offset: BufferAddress,
    /// Size of the binding in bytes, or `None` for using the rest of the buffer.
    pub size: Option<BufferSize>,
}

/// An element of a [`BindGroupDescriptor`], consisting of a bindable resource
/// and the slot to bind it to.
#[derive(Clone, Debug)]
pub struct BindGroupEntry<'a> {
    /// Slot for which binding provides resource.
    pub binding: u32,
    /// Resource to attach to the binding.
    pub resource: BindingResource<'a>,
}

/// Describes a group of bindings and the resources to be bound.
#[derive(Clone, Debug)]
pub struct BindGroupDescriptor<'a> {
    /// Debug label of the bind group.
    pub label: Label<'a>,
    /// The [`BindGroupLayout`] that corresponds to this bind group.
    pub layout: &'a BindGroupLayout,
    /// The resources to bind to this bind group.
    pub entries: &'a [BindGroupEntry<'a>],
}

/// Marker for the `ExternalTexture` variant payload (bevy never constructs
/// it; the type exists so the variant set matches wgpu 29.0.4).
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ExternalTexture;

/// The handle type wgpu named `BindGroup`. Bevy's [`BindGroup`](crate::render_resource::BindGroup)
/// wrapper dereferences to this (the diligent SRB carrier).
#[derive(Clone)]
pub struct WgpuBindGroup {
    /// The Diligent shader resource binding (`None` when the SRB creation
    /// failed - logged).
    pub(crate) value: Option<DiligentHandle<diligent_rs::ShaderResourceBinding>>,
    /// Shared diligent-side SRB state: the first binding that failed to bind
    /// when the SRB was built, if any.
    pub(crate) diligent_state: Arc<crate::render_resource::BindGroupDiligentState>,
    /// The ascending binding indices of the layout's dynamic-offset buffer
    /// entries (M2a, §6.1.1): the `set_bind_group` offset array maps to the
    /// SRB variables one-to-one, in binding order.
    pub(crate) dynamic_bindings: Arc<[u32]>,
}

impl core::fmt::Debug for WgpuBindGroup {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WgpuBindGroup")
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

/// The handle type wgpu named `TextureView`. Bevy's [`TextureView`](crate::render_resource::TextureView)
/// wrapper dereferences to this (the diligent view carrier).
#[derive(Clone)]
pub struct WgpuTextureView {
    /// The registry id of the view (matches the wrapping `TextureView`'s id).
    pub(crate) id: crate::render_resource::TextureViewId,
    /// The Diligent texture view (`None` when the diligent creation failed).
    pub(crate) value: Option<DiligentHandle<diligent_rs::TextureView>>,
    /// The format of the underlying texture (drives derived attachment views).
    pub(crate) format: TextureFormat,
    /// The size of the underlying texture.
    pub(crate) size: Extent3d,
    /// The dimension of the view.
    pub(crate) dimension: wgpu_types::TextureViewDimension,
}

impl WgpuTextureView {
    /// Returns the registry id of the view.
    pub(crate) fn id(&self) -> crate::render_resource::TextureViewId {
        self.id
    }
}

impl core::fmt::Debug for WgpuTextureView {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WgpuTextureView")
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

/// The handle type wgpu named `Sampler`. Bevy's [`Sampler`](crate::render_resource::Sampler)
/// wrapper dereferences to this (the diligent sampler carrier).
#[derive(Clone)]
pub struct WgpuSampler {
    /// The registry id of the sampler (matches the wrapping `Sampler`'s id).
    pub(crate) id: crate::render_resource::SamplerId,
    /// The Diligent sampler (`None` when the diligent creation failed).
    pub(crate) value: Option<DiligentHandle<diligent_rs::Sampler>>,
}

impl WgpuSampler {
    /// Returns the registry id of the sampler.
    pub(crate) fn id(&self) -> crate::render_resource::SamplerId {
        self.id
    }
}

impl core::fmt::Debug for WgpuSampler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WgpuSampler")
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Pipeline layout
// ---------------------------------------------------------------------------

/// Describes the layout of bind groups for a pipeline.
#[derive(Clone, Debug, Default)]
pub struct PipelineLayoutDescriptor<'a> {
    /// Debug label of the pipeline layout.
    pub label: Label<'a>,
    /// Bind groups that this pipeline uses.
    pub bind_group_layouts: &'a [Option<&'a BindGroupLayout>],
    /// The number of bytes of immediate data allocated for use in the shader.
    pub immediate_size: u32,
}

// ---------------------------------------------------------------------------
// Shader modules
// ---------------------------------------------------------------------------

/// The source for a shader module.
#[cfg_attr(feature = "shader_format_spirv", expect(clippy::large_enum_variant))]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ShaderSource<'a> {
    /// SPIR-V module represented as a slice of words.
    #[cfg(feature = "shader_format_spirv")]
    SpirV(Cow<'a, [u32]>),
    /// WGSL module as a string slice.
    Wgsl(Cow<'a, str>),
    /// Naga module.
    Naga(Cow<'static, naga::Module>),
}

/// Descriptor for use with `RenderDevice::create_shader_module`.
#[derive(Clone, Debug)]
pub struct ShaderModuleDescriptor<'a> {
    /// Debug label of the shader module.
    pub label: Label<'a>,
    /// Source code for the shader.
    pub source: ShaderSource<'a>,
}

/// A compiled shader module handle (M1-4b-2: carries the naga module the
/// Diligent per-stage shaders are compiled from; the wgpu module is gone).
#[derive(Clone, Debug)]
pub struct ShaderModule {
    pub(crate) id: ShaderModuleId,
    /// The naga module parsed from the source (`None` when parsing failed -
    /// the error is latched for `pipeline_cache::load_module`).
    pub(crate) naga: Option<Arc<naga::Module>>,
}

impl ShaderModule {
    /// Returns the [`ShaderModuleId`] representing the unique ID of the
    /// shader module.
    #[inline]
    pub fn id(&self) -> ShaderModuleId {
        self.id
    }

    /// The registry key of the module (the id as a plain integer).
    pub(crate) fn registry_key(&self) -> u32 {
        u32::from(core::num::NonZero::<u32>::from(self.id))
    }

    /// The naga module compiled from the source, when parsing succeeded.
    pub(crate) fn naga_module(&self) -> Option<&naga::Module> {
        self.naga.as_deref()
    }
}

/// A pipeline layout handle (M1-4b-2: the wgpu layout is gone; the layout
/// record with the PRS array lives in the pipeline cache and is registered
/// by this handle's id).
#[derive(Clone, Debug)]
pub struct PipelineLayout {
    pub(crate) id: PipelineLayoutId,
}

impl PipelineLayout {
    /// Returns the [`PipelineLayoutId`] representing the unique ID of the
    /// pipeline layout.
    #[inline]
    pub fn id(&self) -> PipelineLayoutId {
        self.id
    }

    /// The registry key of the layout (the id as a plain integer).
    pub(crate) fn registry_key(&self) -> u32 {
        u32::from(core::num::NonZero::<u32>::from(self.id))
    }
}

// ---------------------------------------------------------------------------
// Command recording
// ---------------------------------------------------------------------------

/// A command encoder that records directly on the Diligent immediate device
/// context (the render graph records there anyway; `finish` produces an
/// ordering marker that `RenderQueue::submit` drains).
pub struct CommandEncoder {
    /// The diligent device + immediate context the commands record onto.
    render_device: crate::renderer::RenderDevice,
    context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
    /// Buffer maps requested via `map_buffer_on_submit`, executed (blocking)
    /// when the encoder is finished.
    pending_maps: Vec<PendingBufferMap>,
}

struct PendingBufferMap {
    buffer: Buffer,
    size: u64,
    callback: std::sync::Mutex<Box<dyn FnOnce(Result<(), BufferAsyncError>) + Send + 'static>>,
}

/// The finished form of a [`CommandEncoder`]: the command list has already
/// been recorded on the diligent context; this is an ordering token.
pub struct CommandBuffer {
    _private: (),
}

impl core::fmt::Debug for CommandBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CommandBuffer").finish()
    }
}

impl CommandEncoder {
    pub(crate) fn new(
        render_device: crate::renderer::RenderDevice,
        context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
    ) -> Self {
        Self {
            render_device,
            context,
            pending_maps: Vec::new(),
        }
    }

    /// The diligent immediate context, when the engine is available.
    fn context(&self) -> Option<&diligent_rs::DeviceContext> {
        self.context.as_deref()
    }

    /// Finishes recording: runs the pending buffer maps (blocking reads) and
    /// returns a submission token.
    pub fn finish(mut self) -> CommandBuffer {
        let pending_maps = core::mem::take(&mut self.pending_maps);
        for pending in pending_maps {
            let result = self.run_pending_map(&pending);
            let callback = pending.callback.into_inner().unwrap_or_else(|e| e.into_inner());
            callback(result);
        }
        CommandBuffer { _private: () }
    }

    /// Performs the blocking map + read of `pending.buffer` (the copy was
    /// already recorded on the immediate context; the map blocks until the
    /// GPU work finishes) and stores the bytes for `get_mapped_range`.
    fn run_pending_map(&self, pending: &PendingBufferMap) -> Result<(), BufferAsyncError> {
        let Some(context) = self.context() else {
            return Err(BufferAsyncError);
        };
        let Some(buffer) = pending.buffer.diligent() else {
            return Err(BufferAsyncError);
        };
        let _guard = crate::renderer::diligent_registry::context_guard();
        let mapped = context
            .map_buffer(
                buffer,
                sys_map_type_read(),
                false, // blocking: the recorded copy must complete first
            )
            .map_err(|_| BufferAsyncError)?;
        let Some(mapped) = mapped else {
            return Err(BufferAsyncError);
        };
        let data = mapped.as_slice()[..pending.size as usize].to_vec();
        pending.buffer.store_mapped(data);
        Ok(())
    }

    /// Begins recording of a render pass.
    pub fn begin_render_pass<'encoder>(
        &'encoder mut self,
        desc: &RenderPassDescriptor<'_>,
    ) -> RenderPass<'encoder> {
        let context = self.context();
        let (context, began) = match context {
            Some(context) => {
                match crate::renderer::diligent_draw::begin_tracked_render_pass(
                    &self.render_device,
                    context,
                    desc,
                ) {
                    Ok(()) => (Some(context), true),
                    Err(err) => {
                        bevy_log::debug!(
                            "diligent: render pass '{:?}' could not begin: {err}",
                            desc.label
                        );
                        (Some(context), false)
                    }
                }
            }
            None => (None, false),
        };
        RenderPass {
            context,
            began,
            poisoned: !began,
            index_format: None,
            immediate: None,
            bind_groups: Vec::new(),
        }
    }

    /// Begins recording of a compute pass.
    pub fn begin_compute_pass<'encoder>(
        &'encoder mut self,
        _desc: &ComputePassDescriptor<'_>,
    ) -> ComputePass<'encoder> {
        ComputePass {
            context: self.context(),
            poisoned: false,
            immediate: None,
            bind_groups: Vec::new(),
        }
    }

    /// Schedules a buffer map to run when the encoder is finished.
    pub fn map_buffer_on_submit(
        &mut self,
        buffer: &Buffer,
        _mode: MapMode,
        _bounds: impl core::ops::RangeBounds<BufferAddress>,
        callback: impl FnOnce(Result<(), BufferAsyncError>) + Send + 'static,
    ) {
        let size = buffer.size();
        self.pending_maps.push(PendingBufferMap {
            buffer: buffer.clone(),
            size,
            callback: std::sync::Mutex::new(Box::new(callback)),
        });
    }

    /// Copies the bytes of `copy_size` bytes from `source` at
    /// `source_offset` to `destination` at `destination_offset`.
    pub fn copy_buffer_to_buffer(
        &mut self,
        source: &Buffer,
        source_offset: BufferAddress,
        destination: &Buffer,
        destination_offset: BufferAddress,
        copy_size: impl Into<Option<BufferAddress>>,
    ) {
        let Some(context) = self.context() else {
            return;
        };
        let size = copy_size.into().unwrap_or_else(|| {
            let src_remaining = source.size().saturating_sub(source_offset);
            let dst_remaining = destination.size().saturating_sub(destination_offset);
            src_remaining.min(dst_remaining)
        });
        let (Some(src), Some(dst)) = (source.diligent(), destination.diligent()) else {
            return;
        };
        let _guard = crate::renderer::diligent_registry::context_guard();
        if let Err(err) = context.copy_buffer(src, source_offset, dst, destination_offset, size) {
            bevy_log::warn!("diligent: copy_buffer_to_buffer failed: {err}");
        }
    }

    /// Copies `copy_size` bytes from a texture to a buffer.
    ///
    /// M1-4b-2: the diligent copy API has no texture-to-buffer direction -
    /// the copy is recorded as a pending readback on the destination buffer,
    /// which `BufferSlice::map_async` executes through a same-format staging
    /// texture (in command order on the immediate context).
    pub fn copy_texture_to_buffer(
        &mut self,
        source: TexelCopyTextureInfo<'_>,
        destination: TexelCopyBufferInfo<'_>,
        _copy_size: Extent3d,
    ) {
        let _ = _copy_size;
        *destination.buffer.pending_readback.lock().unwrap() =
            Some(crate::texture::TextureReadbackPending {
                device: self.render_device.clone(),
                source: source.texture.clone(),
                mip_level: source.mip_level,
                array_slice: source.origin.z,
            });
    }

    /// Copies `copy_size` bytes from a buffer to a texture.
    pub fn copy_buffer_to_texture(
        &mut self,
        _source: &TexelCopyBufferInfo<'_>,
        _destination: &TexelCopyTextureInfo<'_>,
        _copy_size: Extent3d,
    ) {
        // Same as `copy_texture_to_buffer`: uploads go through
        // `RenderQueue::write_texture` (UpdateTexture).
        bevy_log::debug!("diligent: copy_buffer_to_texture is not wired (TODO-REMOVE-M1-4)");
    }

    /// Copies the sub-resource range from one texture to another.
    pub fn copy_texture_to_texture(
        &mut self,
        source: TexelCopyTextureInfo<'_>,
        destination: TexelCopyTextureInfo<'_>,
        _copy_size: Extent3d,
    ) {
        let Some(context) = self.context() else {
            return;
        };
        let (Some(src), Some(dst)) = (source.texture.diligent(), destination.texture.diligent())
        else {
            return;
        };
        let _guard = crate::renderer::diligent_registry::context_guard();
        if let Err(err) = context.copy_texture(
            src,
            source.mip_level,
            source.origin.z,
            dst,
            destination.mip_level,
            destination.origin.z,
        ) {
            bevy_log::warn!("diligent: copy_texture_to_texture failed: {err}");
        }
    }

    /// Clears the given texture sub-resource to zero.
    pub fn clear_texture(
        &mut self,
        _texture: &crate::render_resource::Texture,
        _subresource_range: &wgpu_types::ImageSubresourceRange,
    ) {
        bevy_log::debug!("diligent: clear_texture is not wired (TODO-REMOVE-M1-4)");
    }

    /// Clears the buffer to zero.
    pub fn clear_buffer(
        &mut self,
        _buffer: &Buffer,
        _offset: BufferAddress,
        _size: Option<BufferAddress>,
    ) {
        bevy_log::debug!("diligent: clear_buffer is not wired (TODO-REMOVE-M1-4)");
    }

    /// Records a timestamp write (no-op on the diligent path).
    pub fn write_timestamp(&mut self, _query_set: &QuerySet, _index: u32) {}

    /// Starts a new debug group (recorded on the diligent context).
    pub fn push_debug_group(&mut self, label: &str) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::begin_debug_group(context, label);
        }
    }

    /// Ends the current debug group.
    pub fn pop_debug_group(&mut self) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::end_debug_group(context);
        }
    }

    /// Resolves a query set into a buffer (no-op on the diligent path - no
    /// query support).
    pub fn resolve_query_set(
        &mut self,
        _query_set: &QuerySet,
        _query_range: core::ops::Range<u32>,
        _destination: &Buffer,
        _destination_offset: BufferAddress,
    ) {
    }

    /// Builds bottom- and top-level acceleration structures (no-op on the
    /// diligent path - RT lands with the M4a solari port).
    pub fn build_acceleration_structures<'a>(
        &mut self,
        blas: impl IntoIterator<Item = &'a BlasBuildEntry<'a>>,
        tlas: impl IntoIterator<Item = &'a Tlas>,
    ) {
        let (blas_count, tlas_count) = (blas.into_iter().count(), tlas.into_iter().count());
        if blas_count + tlas_count > 0 {
            bevy_log::debug!(
                "diligent: acceleration structure builds ({blas_count} BLAS, {tlas_count} TLAS) \
                 are not supported on the diligent path (TODO-REMOVE-M1-4: M4a)"
            );
        }
    }
}

fn sys_map_type_read() -> diligent_rs::diligent_sys::bindings::MAP_TYPE {
    diligent_rs::diligent_sys::bindings::_MAP_TYPE::MAP_READ
        as diligent_rs::diligent_sys::bindings::MAP_TYPE
}

/// A render pass recording directly on the diligent immediate context.
pub struct RenderPass<'encoder> {
    context: Option<&'encoder diligent_rs::DeviceContext>,
    began: bool,
    poisoned: bool,
    /// The index format of the bound index buffer (per-draw in
    /// `DrawIndexedAttribs`/`DrawIndexedIndirectAttribs`).
    index_format: Option<IndexFormat>,
    /// The current pipeline's immediate-constants binding (M2a,
    /// `set_immediates` -> `SetInlineConstants` on the immediate SRB).
    immediate: Option<crate::renderer::render_device::ImmediateSrb>,
    /// Per-slot (bind group SRB, offsets) cache: repeated `set_bind_group`
    /// calls for the same group and offsets skip the commit (the
    /// offset-change detection of §6.1.1; cleared on pipeline changes).
    bind_groups: Vec<Option<(usize, Vec<u32>)>>,
}

impl RenderPass<'_> {
    fn context(&self) -> Option<&diligent_rs::DeviceContext> {
        self.context
    }

    /// Marks the pass as degraded: subsequent draws are skipped.
    fn poison(&mut self, reason: &str) {
        if !self.poisoned {
            bevy_log::debug!(
                "diligent: render pass degraded, draws will be skipped: {reason} \
                 (TODO-REMOVE-M1-4)"
            );
        }
        self.poisoned = true;
    }

    /// Sets the active [`RenderPipeline`](crate::render_resource::RenderPipeline).
    pub fn set_pipeline(&mut self, pipeline: &crate::render_resource::RenderPipeline) {
        let Some(context) = self.context() else {
            self.poison("no diligent context");
            return;
        };
        match pipeline.diligent() {
            Some(pso) => {
                let _guard = crate::renderer::diligent_registry::context_guard();
                context.set_pipeline_state(pso);
                if let Some(immediate) = pipeline.immediate_srb() {
                    // Bind the immediate signature's root constants slot now
                    // (fresh SRBs start zeroed); `set_immediates` re-applies
                    // the constants and re-commits.
                    context.commit_shader_resources(Some(&immediate.srb));
                }
            }
            None => self.poison("pipeline has no diligent PSO"),
        }
        // Any pipeline change invalidates the per-slot commit cache (the
        // committed root parameters belong to the previous PSO).
        self.bind_groups.clear();
        self.immediate = pipeline.immediate_srb().cloned();
    }

    /// Sets the active bind group for a given bind group index.
    pub fn set_bind_group<'a, BG>(
        &mut self,
        index: u32,
        bind_group: BG,
        dynamic_offsets: &[u32],
    ) where
        Option<&'a WgpuBindGroup>: From<BG>,
    {
        let Some(bind_group) = bind_group.into() else {
            return;
        };
        let Some(srb) = &bind_group.value else {
            self.poison("bind group has no diligent SRB");
            return;
        };
        if bind_group.diligent_state.first_failed_binding().is_some() {
            self.poison("bind group has an unbound diligent variable");
            return;
        }
        // Offset-change detection (§6.1.1): repeated (group, offsets) sets
        // skip both the `SetBufferOffset` calls and the commit.
        let slot = index as usize;
        if slot >= self.bind_groups.len() {
            self.bind_groups.resize(slot + 1, None);
        }
        let key = (srb.as_raw() as usize, dynamic_offsets.to_vec());
        if self.bind_groups[slot].as_ref() == Some(&key) {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        // Both engine calls run under one context-lock scope (the offset set
        // + commit pair is atomic across threads that share the SRB).
        let outcome = (|| -> Result<(), String> {
            let _guard = crate::renderer::diligent_registry::context_guard();
            // M2a-1 review, fix 2: `apply_dynamic_offsets` runs
            // unconditionally - a layout with dynamic bindings and an empty
            // offset array must error (and poison) instead of committing
            // stale cached offsets.
            crate::renderer::render_device::apply_dynamic_offsets(
                &bind_group.diligent_state,
                &bind_group.dynamic_bindings,
                dynamic_offsets,
            )?;
            context.commit_shader_resources(Some(srb));
            Ok(())
        })();
        if let Err(err) = outcome {
            bevy_log::warn!("diligent: dynamic offsets: {err}");
            self.poison("dynamic offset application failed");
            return;
        }
        self.bind_groups[slot] = Some(key);
    }

    /// Assign a vertex buffer to a slot.
    pub fn set_vertex_buffer<'a>(&mut self, slot_index: u32, buffer_slice: impl Into<BufferSlice<'a>>) {
        let buffer_slice = buffer_slice.into();
        let Some(context) = self.context() else {
            return;
        };
        let Some(buffer) = crate::renderer::diligent_registry::registry()
            .resolve_buffer(buffer_slice.buffer().id())
        else {
            self.poison("no diligent buffer for vertex slot");
            return;
        };
        crate::render_phase::draw_state::set_vertex_buffer_slot(
            context,
            slot_index,
            buffer,
            buffer_slice.offset(),
        );
    }

    /// Sets the active index buffer.
    pub fn set_index_buffer<'a>(
        &mut self,
        buffer_slice: impl Into<BufferSlice<'a>>,
        index_format: IndexFormat,
    ) {
        let buffer_slice = buffer_slice.into();
        let Some(context) = self.context() else {
            return;
        };
        let Some(buffer) = crate::renderer::diligent_registry::registry()
            .resolve_buffer(buffer_slice.buffer().id())
        else {
            self.poison("no diligent index buffer");
            return;
        };
        crate::renderer::diligent_draw::set_index_buffer(context, buffer, buffer_slice.offset());
        self.index_format = Some(index_format);
    }

    fn index_type(&self) -> diligent_rs::diligent_sys::bindings::VALUE_TYPE {
        match self.index_format {
            Some(IndexFormat::Uint16) => {
                diligent_rs::diligent_sys::bindings::_VALUE_TYPE::VT_UINT16 as _
            }
            _ => diligent_rs::diligent_sys::bindings::_VALUE_TYPE::VT_UINT32 as _,
        }
    }

    /// Draws primitives from the active vertex buffer(s).
    pub fn draw(&mut self, vertices: core::ops::Range<u32>, instances: core::ops::Range<u32>) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let attribs = diligent_rs::diligent_sys::bindings::DrawAttribs {
            NumVertices: vertices.end - vertices.start,
            Flags: 0,
            NumInstances: instances.end - instances.start,
            StartVertexLocation: vertices.start,
            FirstInstanceLocation: instances.start,
        };
        crate::renderer::diligent_draw::draw(context, &attribs);
    }

    /// Draws indexed primitives using the active index buffer.
    pub fn draw_indexed(
        &mut self,
        indices: core::ops::Range<u32>,
        base_vertex: i32,
        instances: core::ops::Range<u32>,
    ) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let attribs = diligent_rs::diligent_sys::bindings::DrawIndexedAttribs {
            NumIndices: indices.end - indices.start,
            IndexType: self.index_type(),
            Flags: 0,
            NumInstances: instances.end - instances.start,
            FirstIndexLocation: indices.start,
            BaseVertex: base_vertex as u32,
            FirstInstanceLocation: instances.start,
        };
        crate::renderer::diligent_draw::draw_indexed(context, &attribs);
    }

    /// Draws primitives from the active vertex buffer(s) based on the
    /// contents of the `indirect_buffer`.
    pub fn draw_indirect(&mut self, indirect_buffer: &Buffer, indirect_offset: u64) {
        self.draw_indirect_impl(indirect_buffer, indirect_offset, None, 1);
    }

    /// Draws indexed primitives using the active index buffer, based on the
    /// contents of the `indirect_buffer`.
    pub fn draw_indexed_indirect(&mut self, indirect_buffer: &Buffer, indirect_offset: u64) {
        self.draw_indexed_indirect_impl(indirect_buffer, indirect_offset, None, 1);
    }

    /// Dispatches multiple draw calls from the active vertex buffer(s).
    pub fn multi_draw_indirect(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        self.draw_indirect_impl(indirect_buffer, indirect_offset, None, count);
    }

    /// Dispatches multiple draw calls, count read from a buffer.
    pub fn multi_draw_indirect_count(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count_buffer: &Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        self.draw_indirect_impl(
            indirect_buffer,
            indirect_offset,
            Some((count_buffer, count_offset)),
            max_count,
        );
    }

    fn draw_indirect_impl(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count: Option<(&Buffer, u64)>,
        max_count: u32,
    ) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let Some(buffer) = crate::renderer::diligent_registry::registry()
            .resolve_buffer(indirect_buffer.id())
        else {
            self.poison("no diligent indirect buffer");
            return;
        };
        let (counter, counter_offset, counter_mode) = match count {
            Some((count_buffer, count_offset)) => {
                match crate::renderer::diligent_registry::registry().resolve_buffer(count_buffer.id())
                {
                    Some(counter) => (counter, count_offset, transition_mode()),
                    None => {
                        self.poison("no diligent count buffer");
                        return;
                    }
                }
            }
            None => (core::ptr::null_mut(), 0, none_mode()),
        };
        let attribs = diligent_rs::diligent_sys::bindings::DrawIndirectAttribs {
            pAttribsBuffer: buffer,
            DrawArgsOffset: indirect_offset,
            Flags: 0,
            DrawCount: max_count,
            DrawArgsStride: 16,
            AttribsBufferStateTransitionMode: transition_mode(),
            pCounterBuffer: counter,
            CounterOffset: counter_offset,
            CounterBufferStateTransitionMode: counter_mode,
        };
        crate::renderer::diligent_draw::draw_indirect(context, &attribs);
    }

    fn draw_indexed_indirect_impl(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count: Option<(&Buffer, u64)>,
        max_count: u32,
    ) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let Some(buffer) = crate::renderer::diligent_registry::registry()
            .resolve_buffer(indirect_buffer.id())
        else {
            self.poison("no diligent indirect buffer");
            return;
        };
        let (counter, counter_offset, counter_mode) = match count {
            Some((count_buffer, count_offset)) => {
                match crate::renderer::diligent_registry::registry().resolve_buffer(count_buffer.id())
                {
                    Some(counter) => (counter, count_offset, transition_mode()),
                    None => {
                        self.poison("no diligent count buffer");
                        return;
                    }
                }
            }
            None => (core::ptr::null_mut(), 0, none_mode()),
        };
        let attribs = diligent_rs::diligent_sys::bindings::DrawIndexedIndirectAttribs {
            IndexType: self.index_type(),
            pAttribsBuffer: buffer,
            DrawArgsOffset: indirect_offset,
            Flags: 0,
            DrawCount: max_count,
            DrawArgsStride: 20,
            AttribsBufferStateTransitionMode: transition_mode(),
            pCounterBuffer: counter,
            CounterOffset: counter_offset,
            CounterBufferStateTransitionMode: counter_mode,
        };
        crate::renderer::diligent_draw::draw_indexed_indirect(context, &attribs);
    }

    /// Dispatches multiple indexed draw calls from the active index buffer.
    pub fn multi_draw_indexed_indirect(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        self.draw_indexed_indirect_impl(indirect_buffer, indirect_offset, None, count);
    }

    /// Dispatches multiple indexed draw calls, count read from a buffer.
    pub fn multi_draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
        count_buffer: &Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        self.draw_indexed_indirect_impl(
            indirect_buffer,
            indirect_offset,
            Some((count_buffer, count_offset)),
            max_count,
        );
    }

    /// Sets the stencil reference.
    pub fn set_stencil_reference(&mut self, reference: u32) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::set_stencil_ref(context, reference);
        }
    }

    /// Sets the scissor region.
    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::set_scissor_rects(
                context,
                &[diligent_rs::diligent_sys::bindings::Rect {
                    left: x as i32,
                    top: y as i32,
                    right: (x + width) as i32,
                    bottom: (y + height) as i32,
                }],
            );
        }
    }

    /// Sets the immediate-constants data: `SetInlineConstants` on the
    /// current pipeline's immediate SRB (M2a, §6.1.1; V3 mapping:
    /// `FirstConstant = offset / 4`, `NumConstants = data.len() / 4`).
    pub fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        let Some(context) = self.context() else {
            return;
        };
        let Some(immediate) = &self.immediate else {
            bevy_log::debug!(
                "diligent: set_immediates({offset}, {} bytes) with no immediate \
                 constants in the current pipeline",
                data.len()
            );
            return;
        };
        if offset % 4 != 0 || data.len() % 4 != 0 {
            self.poison("set_immediates offset/data is not 4-byte aligned");
            return;
        }
        let first_constant = offset / 4;
        let num_constants = (data.len() / 4) as u32;
        if first_constant + num_constants > immediate.array_size_dwords {
            self.poison(&format!(
                "set_immediates range (constants {}..{}) exceeds the pipeline's \
                 immediate capacity ({} dwords)",
                first_constant,
                first_constant + num_constants,
                immediate.array_size_dwords
            ));
            return;
        }
        // SetInlineConstants writes into the SRB cache; the commit uploads
        // it (V3 mapping; both under one context-lock scope).
        let outcome = (|| -> Result<(), String> {
            let _guard = crate::renderer::diligent_registry::context_guard();
            crate::renderer::render_device::set_inline_constants(
                immediate.variable,
                data,
                first_constant,
                num_constants,
            )?;
            context.commit_shader_resources(Some(&immediate.srb));
            Ok(())
        })();
        if let Err(err) = outcome {
            bevy_log::warn!("diligent: SetInlineConstants: {err}");
            self.poison("SetInlineConstants failed");
        }
    }

    /// Sets the rendering viewport.
    pub fn set_viewport(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) {
        if let Some(context) = self.context() {
            let _guard = crate::renderer::diligent_registry::context_guard();
            context.set_viewports(&[diligent_rs::diligent_sys::bindings::Viewport {
                TopLeftX: x,
                TopLeftY: y,
                Width: width,
                Height: height,
                MinDepth: min_depth,
                MaxDepth: max_depth,
            }]);
        }
    }

    /// Sets the rendering viewport to the given camera viewport.
    pub fn set_camera_viewport(&mut self, viewport: &bevy_camera::Viewport) {
        self.set_viewport(
            viewport.physical_position.x as f32,
            viewport.physical_position.y as f32,
            viewport.physical_size.x as f32,
            viewport.physical_size.y as f32,
            viewport.depth.start,
            viewport.depth.end,
        );
    }

    /// Inserts a single debug marker (approximated with a debug group).
    pub fn insert_debug_marker(&mut self, label: &str) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::begin_debug_group(context, label);
            crate::renderer::diligent_draw::end_debug_group(context);
        }
    }

    /// Starts a new debug group.
    pub fn push_debug_group(&mut self, label: &str) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::begin_debug_group(context, label);
        }
    }

    /// Ends the current debug group.
    pub fn pop_debug_group(&mut self) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::end_debug_group(context);
        }
    }

    /// Sets the blend color.
    pub fn set_blend_constant(&mut self, color: bevy_color::LinearRgba) {
        if let Some(context) = self.context() {
            crate::renderer::diligent_draw::set_blend_factors(context, color.to_f32_array());
        }
    }

    /// Writes a timestamp (no-op on the diligent path).
    pub fn write_timestamp(&mut self, _query_set: &QuerySet, _index: u32) {}

    /// Begins a pipeline statistics query (no-op on the diligent path).
    pub fn begin_pipeline_statistics_query(&mut self, _query_set: &QuerySet, _index: u32) {}

    /// Ends a pipeline statistics query (no-op on the diligent path).
    pub fn end_pipeline_statistics_query(&mut self) {}
}

impl Drop for RenderPass<'_> {
    fn drop(&mut self) {
        if self.began {
            if let Some(context) = self.context() {
                crate::renderer::diligent_draw::end_render_pass(context);
            }
        }
    }
}

/// A compute pass recording directly on the diligent immediate context.
pub struct ComputePass<'encoder> {
    context: Option<&'encoder diligent_rs::DeviceContext>,
    poisoned: bool,
    /// The current pipeline's immediate-constants binding (M2a,
    /// `set_immediates` -> `SetInlineConstants` on the immediate SRB).
    immediate: Option<crate::renderer::render_device::ImmediateSrb>,
    /// Per-slot (bind group SRB, offsets) cache: repeated `set_bind_group`
    /// calls for the same group and offsets skip the commit (the
    /// offset-change detection of §6.1.1; cleared on pipeline changes).
    bind_groups: Vec<Option<(usize, Vec<u32>)>>,
}

impl ComputePass<'_> {
    fn context(&self) -> Option<&diligent_rs::DeviceContext> {
        self.context
    }

    /// Sets the active [`ComputePipeline`](crate::render_resource::ComputePipeline).
    pub fn set_pipeline(&mut self, pipeline: &crate::render_resource::ComputePipeline) {
        let Some(context) = self.context() else {
            return;
        };
        match pipeline.diligent() {
            Some(pso) => {
                let _guard = crate::renderer::diligent_registry::context_guard();
                context.set_pipeline_state(pso);
                if let Some(immediate) = pipeline.immediate_srb() {
                    // Bind the immediate signature's root constants slot now
                    // (fresh SRBs start zeroed); `set_immediates` re-applies
                    // the constants and re-commits.
                    context.commit_shader_resources(Some(&immediate.srb));
                }
            }
            None => {
                if !self.poisoned {
                    bevy_log::debug!(
                        "diligent: compute pipeline has no diligent PSO, dispatches will be skipped"
                    );
                }
                self.poisoned = true;
            }
        }
        // Any pipeline change invalidates the per-slot commit cache.
        self.bind_groups.clear();
        self.immediate = pipeline.immediate_srb().cloned();
    }

    /// Sets the active bind group for a given bind group index.
    pub fn set_bind_group<'a, BG>(
        &mut self,
        index: u32,
        bind_group: BG,
        dynamic_offsets: &[u32],
    ) where
        Option<&'a WgpuBindGroup>: From<BG>,
    {
        let Some(bind_group) = bind_group.into() else {
            return;
        };
        let Some(srb) = &bind_group.value else {
            self.poisoned = true;
            return;
        };
        if bind_group.diligent_state.first_failed_binding().is_some() {
            self.poisoned = true;
            return;
        }
        // Offset-change detection (§6.1.1): repeated (group, offsets) sets
        // skip both the `SetBufferOffset` calls and the commit.
        let slot = index as usize;
        if slot >= self.bind_groups.len() {
            self.bind_groups.resize(slot + 1, None);
        }
        let key = (srb.as_raw() as usize, dynamic_offsets.to_vec());
        if self.bind_groups[slot].as_ref() == Some(&key) {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        // Both engine calls run under one context-lock scope (the offset set
        // + commit pair is atomic across threads that share the SRB).
        let outcome = (|| -> Result<(), String> {
            let _guard = crate::renderer::diligent_registry::context_guard();
            // M2a-1 review, fix 2: `apply_dynamic_offsets` runs
            // unconditionally - a layout with dynamic bindings and an empty
            // offset array must error (and poison) instead of committing
            // stale cached offsets.
            crate::renderer::render_device::apply_dynamic_offsets(
                &bind_group.diligent_state,
                &bind_group.dynamic_bindings,
                dynamic_offsets,
            )?;
            context.commit_shader_resources(Some(srb));
            Ok(())
        })();
        if let Err(err) = outcome {
            bevy_log::warn!("diligent: dynamic offsets: {err}");
            self.poisoned = true;
            return;
        }
        self.bind_groups[slot] = Some(key);
    }

    /// Dispatches workgroups.
    pub fn dispatch_workgroups(&mut self, x: u32, y: u32, z: u32) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let attribs = diligent_rs::diligent_sys::bindings::DispatchComputeAttribs {
            ThreadGroupCountX: x,
            ThreadGroupCountY: y,
            ThreadGroupCountZ: z,
            MtlThreadGroupSizeX: 0,
            MtlThreadGroupSizeY: 0,
            MtlThreadGroupSizeZ: 0,
        };
        crate::renderer::diligent_draw::dispatch_compute(context, &attribs);
    }

    /// Dispatches workgroups based on the contents of the indirect buffer.
    ///
    /// M2a-2: maps to `DispatchComputeIndirect` with the wgpu
    /// `DispatchIndirectArgs` `{x, y, z}` layout - the locked
    /// `DispatchComputeIndirectAttribs` documents the same 3x u32 layout
    /// (ThreadGroupCountX/Y/Z at `DispatchArgsByteOffset`, DeviceContext.h:933-944),
    /// so the translation is a zero-copy offset pass-through (V23). Note:
    /// this version has NO `pCounterBuffer` on the indirect-dispatch
    /// attribs (unlike `DrawIndirectAttribs`), so count-buffer-driven
    /// indirect dispatches are not expressible - the meshlet pattern writes
    /// the count into the args buffer itself (atomic counter as the first
    /// arg field, `fill_counts.wgsl`).
    pub fn dispatch_workgroups_indirect(
        &mut self,
        indirect_buffer: &Buffer,
        indirect_offset: u64,
    ) {
        if self.poisoned {
            return;
        }
        let Some(context) = self.context() else {
            return;
        };
        let Some(buffer) = crate::renderer::diligent_registry::registry()
            .resolve_buffer(indirect_buffer.id())
        else {
            bevy_log::debug!(
                "diligent: no diligent indirect buffer for dispatch (unregistered buffer)"
            );
            self.poisoned = true;
            return;
        };
        let attribs = diligent_rs::diligent_sys::bindings::DispatchComputeIndirectAttribs {
            pAttribsBuffer: buffer,
            AttribsBufferStateTransitionMode:
                diligent_rs::diligent_sys::bindings::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                    as diligent_rs::diligent_sys::bindings::RESOURCE_STATE_TRANSITION_MODE,
            DispatchArgsByteOffset: indirect_offset,
            MtlThreadGroupSizeX: 0,
            MtlThreadGroupSizeY: 0,
            MtlThreadGroupSizeZ: 0,
        };
        crate::renderer::diligent_draw::dispatch_compute_indirect(context, &attribs);
    }

    /// Sets the immediate-constants data: `SetInlineConstants` on the
    /// current pipeline's immediate SRB (M2a, §6.1.1; V3 mapping:
    /// `FirstConstant = offset / 4`, `NumConstants = data.len() / 4`).
    pub fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        let Some(context) = self.context() else {
            return;
        };
        let Some(immediate) = &self.immediate else {
            bevy_log::debug!(
                "diligent: set_immediates({offset}, {} bytes) with no immediate \
                 constants in the current pipeline",
                data.len()
            );
            return;
        };
        if offset % 4 != 0 || data.len() % 4 != 0 {
            bevy_log::warn!(
                "diligent: compute set_immediates offset/data is not 4-byte aligned \
                 (offset {offset}, {} bytes)",
                data.len()
            );
            self.poisoned = true;
            return;
        }
        let first_constant = offset / 4;
        let num_constants = (data.len() / 4) as u32;
        if first_constant + num_constants > immediate.array_size_dwords {
            bevy_log::warn!(
                "diligent: set_immediates range (constants {}..{}) exceeds the pipeline's \
                 immediate capacity ({} dwords)",
                first_constant,
                first_constant + num_constants,
                immediate.array_size_dwords
            );
            self.poisoned = true;
            return;
        }
        // SetInlineConstants writes into the SRB cache; the commit uploads
        // it (V3 mapping; both under one context-lock scope).
        let outcome = (|| -> Result<(), String> {
            let _guard = crate::renderer::diligent_registry::context_guard();
            crate::renderer::render_device::set_inline_constants(
                immediate.variable,
                data,
                first_constant,
                num_constants,
            )?;
            context.commit_shader_resources(Some(&immediate.srb));
            Ok(())
        })();
        if let Err(err) = outcome {
            bevy_log::warn!("diligent: SetInlineConstants: {err}");
            self.poisoned = true;
        }
    }

    /// Writes a timestamp (no-op on the diligent path).
    pub fn write_timestamp(&mut self, _query_set: &QuerySet, _index: u32) {}

    /// Begins a pipeline statistics query (no-op on the diligent path).
    pub fn begin_pipeline_statistics_query(&mut self, _query_set: &QuerySet, _index: u32) {}

    /// Ends a pipeline statistics query (no-op on the diligent path).
    pub fn end_pipeline_statistics_query(&mut self) {}
}

// ---------------------------------------------------------------------------
// Render pass descriptors
// ---------------------------------------------------------------------------

/// Describes the timestamp writes of a render pass.
#[derive(Clone, Debug)]
pub struct RenderPassTimestampWrites<'a> {
    /// The query set to write to.
    pub query_set: &'a QuerySet,
    /// The index of the query set at which a start timestamp of this pass is written, if any.
    pub beginning_of_pass_write_index: Option<u32>,
    /// The index of the query set at which an end timestamp of this pass is written, if any.
    pub end_of_pass_write_index: Option<u32>,
}

/// Describes a color attachment to a [`RenderPass`].
#[derive(Clone, Debug)]
pub struct RenderPassColorAttachment<'tex> {
    /// The view to use as an attachment.
    pub view: &'tex WgpuTextureView,
    /// The depth slice index of a 3D view. It must not be provided if the view is not 3D.
    pub depth_slice: Option<u32>,
    /// The view that will receive the resolved output if multisampling is used.
    pub resolve_target: Option<&'tex WgpuTextureView>,
    /// What operations will be performed on this color attachment.
    pub ops: wgpu_types::Operations<wgpu_types::Color>,
}

/// Describes a depth/stencil attachment to a [`RenderPass`].
#[derive(Clone, Debug)]
pub struct RenderPassDepthStencilAttachment<'tex> {
    /// The view to use as an attachment.
    pub view: &'tex WgpuTextureView,
    /// What operations will be performed on the depth part of the attachment.
    pub depth_ops: Option<wgpu_types::Operations<f32>>,
    /// What operations will be performed on the stencil part of the attachment.
    pub stencil_ops: Option<wgpu_types::Operations<u32>>,
}

/// Describes the attachments of a render pass.
#[derive(Clone, Debug, Default)]
pub struct RenderPassDescriptor<'a> {
    /// Debug label of the render pass.
    pub label: Label<'a>,
    /// The color attachments of the render pass.
    pub color_attachments: &'a [Option<RenderPassColorAttachment<'a>>],
    /// The depth and stencil attachment of the render pass, if any.
    pub depth_stencil_attachment: Option<RenderPassDepthStencilAttachment<'a>>,
    /// Defines which timestamp values will be written for this pass, and where to write them to.
    pub timestamp_writes: Option<RenderPassTimestampWrites<'a>>,
    /// Defines where the occlusion query results will be stored for this pass.
    pub occlusion_query_set: Option<&'a QuerySet>,
    /// The mask of multiview image layers to use for this render pass.
    pub multiview_mask: Option<NonZeroU32>,
}

/// Describes the timestamp writes of a compute pass.
#[derive(Clone, Debug)]
pub struct ComputePassTimestampWrites<'a> {
    /// The query set to write to.
    pub query_set: &'a QuerySet,
    /// The index of the query set at which a start timestamp of this pass is written, if any.
    pub beginning_of_pass_write_index: Option<u32>,
    /// The index of the query set at which an end timestamp of this pass is written, if any.
    pub end_of_pass_write_index: Option<u32>,
}

/// Describes the attachments of a compute pass.
#[derive(Clone, Default, Debug)]
pub struct ComputePassDescriptor<'a> {
    /// Debug label of the compute pass.
    pub label: Label<'a>,
    /// Defines which timestamp values will be written for this pass, and where to write them to.
    pub timestamp_writes: Option<ComputePassTimestampWrites<'a>>,
}

/// Marker for GPU query sets (the diligent path has no timestamp/occlusion
/// query support; the diagnostic queries are no-ops).
#[derive(Debug, Clone)]
pub struct QuerySet;

// ---------------------------------------------------------------------------
// Pipeline compilation options + raw pipeline descriptors
// ---------------------------------------------------------------------------

/// Advanced options for use when a pipeline is compiled.
#[derive(Clone, Debug)]
pub struct PipelineCompilationOptions<'a> {
    /// Specifies the values of pipeline-overridable constants in the shader module.
    pub constants: &'a [(&'a str, f64)],
    /// Whether workgroup scoped memory will be initialized with zero values for this stage.
    pub zero_initialize_workgroup_memory: bool,
}

impl Default for PipelineCompilationOptions<'_> {
    fn default() -> Self {
        Self {
            constants: Default::default(),
            zero_initialize_workgroup_memory: true,
        }
    }
}

/// The wgpu `PipelineCache` marker used by raw pipeline descriptors (bevy
/// never populates the `cache` field).
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct PipelineCacheMarker;

/// Describes the vertex buffer layout of a vertex state.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct VertexBufferLayout<'a> {
    /// The stride, in bytes, between elements of this buffer.
    pub array_stride: BufferAddress,
    /// How often this vertex buffer is "stepped" forward.
    pub step_mode: wgpu_types::VertexStepMode,
    /// The list of attributes which comprise a single vertex.
    pub attributes: &'a [wgpu_types::VertexAttribute],
}

/// Describes the vertex processing in a render pipeline.
#[derive(Clone, Debug)]
pub struct VertexState<'a> {
    /// The compiled shader module for this stage.
    pub module: &'a ShaderModule,
    /// The name of the entry point in the compiled shader to use.
    pub entry_point: Option<&'a str>,
    /// Advanced options for when this pipeline is compiled.
    pub compilation_options: PipelineCompilationOptions<'a>,
    /// The format of any vertex buffers used with this pipeline.
    pub buffers: &'a [VertexBufferLayout<'a>],
}

/// Describes the fragment processing in a render pipeline.
#[derive(Clone, Debug)]
pub struct FragmentState<'a> {
    /// The compiled shader module for this stage.
    pub module: &'a ShaderModule,
    /// The name of the entry point in the compiled shader to use.
    pub entry_point: Option<&'a str>,
    /// Advanced options for when this pipeline is compiled.
    pub compilation_options: PipelineCompilationOptions<'a>,
    /// The color state of the render targets.
    pub targets: &'a [Option<ColorTargetState>],
}

/// Describes a render (graphics) pipeline.
#[derive(Clone, Debug)]
pub struct RenderPipelineDescriptor<'a> {
    /// Debug label of the pipeline.
    pub label: Label<'a>,
    /// The layout of bind groups for this pipeline.
    pub layout: Option<&'a PipelineLayout>,
    /// The compiled vertex stage, its entry point, and the input buffers layout.
    pub vertex: VertexState<'a>,
    /// The properties of the pipeline at the primitive assembly and rasterization level.
    pub primitive: PrimitiveState,
    /// The effect of draw calls on the depth and stencil aspects of the output target, if any.
    pub depth_stencil: Option<DepthStencilState>,
    /// The multi-sampling properties of the pipeline.
    pub multisample: MultisampleState,
    /// The compiled fragment stage, its entry point, and the color targets.
    pub fragment: Option<FragmentState<'a>>,
    /// If the pipeline will be used with a multiview render pass, this indicates what multiview
    /// mask the render pass will be used with.
    pub multiview_mask: Option<NonZeroU32>,
    /// The pipeline cache to use when creating this pipeline.
    pub cache: Option<&'a PipelineCacheMarker>,
}

/// Describes a compute pipeline.
#[derive(Clone, Debug)]
pub struct ComputePipelineDescriptor<'a> {
    /// Debug label of the pipeline.
    pub label: Label<'a>,
    /// The layout of bind groups for this pipeline.
    pub layout: Option<&'a PipelineLayout>,
    /// The compiled shader module for this stage.
    pub module: &'a ShaderModule,
    /// The name of the entry point in the compiled shader to use.
    pub entry_point: Option<&'a str>,
    /// Advanced options for when this pipeline is compiled.
    pub compilation_options: PipelineCompilationOptions<'a>,
    /// The pipeline cache to use when creating this pipeline.
    pub cache: Option<&'a PipelineCacheMarker>,
}

// ---------------------------------------------------------------------------
// Bottom/top level acceleration structures (RT handle family; the diligent
// path has no ray tracing - the M4a solari port is the runtime landing spot).
// ---------------------------------------------------------------------------

/// Error occurred when trying to asynchronously prepare a blas for compaction.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlasAsyncError;

/// Safe instance for a [`Tlas`].
#[derive(Debug, Clone)]
pub struct TlasInstance {
    blas: Arc<()>,
    /// Affine transform matrix 3x4 (rows x columns, row major order).
    pub transform: [f32; 12],
    /// Custom index for the instance used inside the shader.
    pub custom_data: u32,
    /// Mask for the instance used inside the shader to filter instances.
    pub mask: u8,
}

impl TlasInstance {
    /// Constructs a `TlasInstance`.
    pub fn new(blas: &Blas, transform: [f32; 12], custom_data: u32, mask: u8) -> Self {
        Self {
            blas: Arc::clone(&blas.inner),
            transform,
            custom_data,
            mask,
        }
    }

    /// Sets the bottom level acceleration structure.
    pub fn set_blas(&mut self, blas: &Blas) {
        self.blas = Arc::clone(&blas.inner);
    }
}

/// Bottom Level Acceleration Structure (BLAS) handle (RT family; the
/// diligent path has no ray tracing - compaction never completes).
#[derive(Debug, Clone)]
pub struct Blas {
    /// The shared keep-alive token (instances reference it; the diligent
    /// path carries no engine state, so the token is all the lifetime
    /// machinery needs).
    pub(crate) inner: Arc<()>,
}

impl Blas {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(()),
        }
    }

    /// Raw handle to the acceleration structure, used inside raw instance
    /// buffers (never available on the diligent path).
    pub fn handle(&self) -> Option<u64> {
        None
    }

    /// Requests asynchronous compaction preparation (no-op on the diligent
    /// path: the callback reports the unsupported operation immediately).
    pub fn prepare_compaction_async(
        &self,
        callback: impl FnOnce(Result<(), BlasAsyncError>) + Send + 'static,
    ) {
        callback(Err(BlasAsyncError));
    }

    /// Whether the BLAS is ready to be compacted (always false on the
    /// diligent path - `compact_raytracing_blas` keeps retrying harmlessly).
    pub fn ready_for_compaction(&self) -> bool {
        false
    }
}

/// Definition for a triangle geometry for a Bottom Level Acceleration Structure (BLAS).
#[derive(Debug)]
pub struct BlasTriangleGeometry<'a> {
    /// Sub descriptor for the size defining attributes of a triangle geometry.
    pub size: &'a wgpu_types::BlasTriangleGeometrySizeDescriptor,
    /// Vertex buffer.
    pub vertex_buffer: &'a Buffer,
    /// Offset into the vertex buffer as a factor of the vertex stride.
    pub first_vertex: u32,
    /// Vertex stride in bytes.
    pub vertex_stride: BufferAddress,
    /// Index buffer (optional).
    pub index_buffer: Option<&'a Buffer>,
    /// Number of indexes to skip in the index buffer (optional, required if index buffer is present).
    pub first_index: Option<u32>,
    /// Transform buffer containing 3x4 affine transform matrices (optional).
    pub transform_buffer: Option<&'a Buffer>,
    /// Transform buffer offset in bytes (optional, required if transform buffer is present).
    pub transform_buffer_offset: Option<BufferAddress>,
}

/// Contains the sets of geometry that go into a [`Blas`].
pub enum BlasGeometries<'a> {
    /// Triangle geometry variant.
    TriangleGeometries(Vec<BlasTriangleGeometry<'a>>),
}

/// Builds the given sets of geometry into the given [`Blas`].
pub struct BlasBuildEntry<'a> {
    /// Reference to the acceleration structure.
    pub blas: &'a Blas,
    /// Geometries.
    pub geometry: BlasGeometries<'a>,
}

/// Top Level Acceleration Structure (TLAS) handle (RT family; the diligent
/// path has no ray tracing - instances are kept CPU-side only).
#[derive(Debug, Clone)]
pub struct Tlas {
    instances: Vec<Option<TlasInstance>>,
}

impl Tlas {
    pub(crate) fn new(max_instances: u32) -> Self {
        Self {
            instances: vec![None; max_instances as usize],
        }
    }

    /// The instances stored in this TLAS.
    pub fn get(&self) -> &[Option<TlasInstance>] {
        &self.instances
    }

    /// Mutable access to a range of the instances.
    pub fn get_mut_slice(&mut self, range: core::ops::Range<usize>) -> Option<&mut [Option<TlasInstance>]> {
        self.instances.get_mut(range)
    }

    /// Mutable access to a single instance slot.
    pub fn get_mut_single(&mut self, index: usize) -> Option<&mut Option<TlasInstance>> {
        self.instances.get_mut(index)
    }

    /// Returns a binding resource for this TLAS.
    pub fn as_binding(&self) -> BindingResource<'_> {
        BindingResource::AccelerationStructure(self)
    }
}

// ---------------------------------------------------------------------------
// Device / Adapter facades (the transition wgpu Instance/Adapter/Device are
// gone; these carry the diligent-derived capability data)
// ---------------------------------------------------------------------------

/// The wgpu-29-compatible device facade served by
/// `RenderDevice::wgpu_device()` (bevy_pbr reads `features()`; bevy_solari
/// creates BLAS/TLAS handles through it).
#[derive(Clone)]
pub struct Device {
    features: wgpu_types::Features,
    limits: wgpu_types::Limits,
}

impl Device {
    pub(crate) fn new(features: wgpu_types::Features, limits: wgpu_types::Limits) -> Self {
        Self { features, limits }
    }

    /// The features of this device.
    pub fn features(&self) -> wgpu_types::Features {
        self.features
    }

    /// The limits of this device.
    pub fn limits(&self) -> wgpu_types::Limits {
        self.limits.clone()
    }

    /// Creates a bottom level acceleration structure (no-op handle - the
    /// diligent path has no ray tracing).
    pub fn create_blas(
        &self,
        _desc: &CreateBlasDescriptor<'_>,
        _sizes: wgpu_types::BlasGeometrySizeDescriptors,
    ) -> Blas {
        Blas::new()
    }

    /// Creates a top level acceleration structure (no-op handle).
    pub fn create_tlas(&self, desc: &CreateTlasDescriptor<'_>) -> Tlas {
Tlas::new(desc.max_instances)
    }
}

/// The wgpu-29-compatible adapter facade carried by the `RenderAdapter`
/// resource (consumers read `get_info`/`features`/`limits`/
/// `get_downlevel_capabilities` through the `Deref`).
#[derive(Debug, Clone)]
pub struct Adapter {
    info: wgpu_types::AdapterInfo,
    features: wgpu_types::Features,
    limits: wgpu_types::Limits,
    downlevel_capabilities: wgpu_types::DownlevelCapabilities,
}

impl Adapter {
    pub(crate) fn new(
        info: wgpu_types::AdapterInfo,
        features: wgpu_types::Features,
        limits: wgpu_types::Limits,
        downlevel_capabilities: wgpu_types::DownlevelCapabilities,
    ) -> Self {
        Self {
            info,
            features,
            limits,
            downlevel_capabilities,
        }
    }

    /// The adapter information.
    pub fn get_info(&self) -> wgpu_types::AdapterInfo {
        self.info.clone()
    }

    /// The features of the adapter.
    pub fn features(&self) -> wgpu_types::Features {
        self.features
    }

    /// The limits of the adapter.
    pub fn limits(&self) -> wgpu_types::Limits {
        self.limits.clone()
    }

    /// The downlevel capabilities of the adapter.
    pub fn get_downlevel_capabilities(&self) -> wgpu_types::DownlevelCapabilities {
        self.downlevel_capabilities.clone()
    }

    /// The texture format features for a format on this adapter (derived
    /// from the wgpu-types guaranteed feature computation).
    pub fn get_texture_format_features(
        &self,
        format: TextureFormat,
    ) -> wgpu_types::TextureFormatFeatures {
        format.guaranteed_format_features(self.features)
    }
}

/// Marker for the renderer instance resource (the wgpu `Instance` is gone;
/// the diligent engine factory is the instance-equivalent and lives in the
/// `RenderDevice`).
#[derive(Debug, Clone, Default)]
pub struct Instance;

/// A write-only view for `RenderQueue::write_buffer_with`: the bytes are
/// written into an owned buffer that is uploaded to the diligent context when
/// the view is dropped.
pub struct QueueWriteBufferView {
    buffer: Buffer,
    offset: u64,
    data: Vec<u8>,
    context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
}

impl QueueWriteBufferView {
    pub(crate) fn new(
        buffer: Buffer,
        offset: u64,
        data: Vec<u8>,
        context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
    ) -> Self {
        Self {
            buffer,
            offset,
            data,
            context,
        }
    }

    /// Returns a write-only slice over the bytes to be uploaded.
    pub fn slice(
        &mut self,
        bounds: impl core::ops::RangeBounds<usize>,
    ) -> wgpu_types::WriteOnly<'_, [u8]> {
        wgpu_types::WriteOnly::from_mut(&mut self.data[..]).into_slice(bounds)
    }
}

impl Drop for QueueWriteBufferView {
    fn drop(&mut self) {
        if let (Some(context), Some(buffer)) = (self.context.as_deref(), self.buffer.diligent()) {
            let _guard = crate::renderer::diligent_registry::context_guard();
            if let Err(err) = context.update_buffer(buffer, self.offset, &self.data) {
                bevy_log::warn!("diligent: write_buffer_with upload failed: {err}");
            }
        }
    }
}

/// `IDeviceContext` resource-state transition mode helpers (mirrors
/// `diligent_draw`'s `state_transition_mode`).
fn transition_mode() -> diligent_rs::diligent_sys::bindings::RESOURCE_STATE_TRANSITION_MODE {
    diligent_rs::diligent_sys::bindings::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
        as diligent_rs::diligent_sys::bindings::RESOURCE_STATE_TRANSITION_MODE
}

fn none_mode() -> diligent_rs::diligent_sys::bindings::RESOURCE_STATE_TRANSITION_MODE {
    diligent_rs::diligent_sys::bindings::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_NONE
        as diligent_rs::diligent_sys::bindings::RESOURCE_STATE_TRANSITION_MODE
}
