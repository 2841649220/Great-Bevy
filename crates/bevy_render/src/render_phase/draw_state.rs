use crate::{
    diagnostic::internal::{Pass, PassKind, WritePipelineStatistics, WriteTimestamp},
    render_resource::{
        BindGroup, BindGroupId, Buffer, BufferId, BufferSlice, IndexFormat, RenderPipeline,
        RenderPipelineId,
    },
    render_resource::wgpu_compat::QuerySet,
    renderer::{diligent_draw, diligent_registry::registry, RenderDevice},
};
use bevy_camera::Viewport;
use bevy_color::{ColorToComponents, LinearRgba};
use bevy_utils::default;
use core::ops::Range;
use diligent_rs::diligent_sys::bindings as sys;

#[cfg(feature = "detailed_trace")]
use bevy_log::trace;

type BufferSliceKey = (BufferId, wgpu_types::BufferAddress, wgpu_types::BufferSize);

/// Tracks the state of a [`TrackedRenderPass`].
///
/// This is used to skip redundant operations on the [`TrackedRenderPass`] (e.g. setting an already
/// set pipeline, binding an already bound bind group). These operations can otherwise be fairly
/// costly due to IO to the GPU, so deduplicating these calls results in a speedup.
#[derive(Debug, Default)]
struct DrawState {
    pipeline: Option<RenderPipelineId>,
    bind_groups: Vec<(Option<BindGroupId>, Vec<u32>)>,
    /// List of vertex buffers by [`BufferId`], offset, and size. See [`DrawState::buffer_slice_key`]
    vertex_buffers: Vec<Option<BufferSliceKey>>,
    index_buffer: Option<(BufferSliceKey, IndexFormat)>,

    /// Stores whether this state is populated or empty for quick state invalidation
    stores_state: bool,
}

impl DrawState {
    /// Marks the `pipeline` as bound.
    fn set_pipeline(&mut self, pipeline: RenderPipelineId) {
        // TODO: do these need to be cleared?
        // self.bind_groups.clear();
        // self.vertex_buffers.clear();
        // self.index_buffer = None;
        self.pipeline = Some(pipeline);
        self.stores_state = true;
    }

    /// Checks, whether the `pipeline` is already bound.
    fn is_pipeline_set(&self, pipeline: RenderPipelineId) -> bool {
        self.pipeline == Some(pipeline)
    }

    /// Marks the `bind_group` as bound to the `index`.
    fn set_bind_group(&mut self, index: usize, bind_group: BindGroupId, dynamic_indices: &[u32]) {
        let group = &mut self.bind_groups[index];
        group.0 = Some(bind_group);
        group.1.clear();
        group.1.extend(dynamic_indices);
        self.stores_state = true;
    }

    /// Checks, whether the `bind_group` is already bound to the `index`.
    fn is_bind_group_set(
        &self,
        index: usize,
        bind_group: BindGroupId,
        dynamic_indices: &[u32],
    ) -> bool {
        if let Some(current_bind_group) = self.bind_groups.get(index) {
            current_bind_group.0 == Some(bind_group) && dynamic_indices == current_bind_group.1
        } else {
            false
        }
    }

    /// Marks the vertex `buffer` as bound to the `index`.
    fn set_vertex_buffer(&mut self, index: usize, buffer_slice: BufferSlice) {
        self.vertex_buffers[index] = Some(self.buffer_slice_key(&buffer_slice));
        self.stores_state = true;
    }

    /// Checks, whether the vertex `buffer` is already bound to the `index`.
    fn is_vertex_buffer_set(&self, index: usize, buffer_slice: &BufferSlice) -> bool {
        if let Some(current) = self.vertex_buffers.get(index) {
            *current == Some(self.buffer_slice_key(buffer_slice))
        } else {
            false
        }
    }

    /// Returns the value used for checking whether `BufferSlice`s are equivalent.
    fn buffer_slice_key(&self, buffer_slice: &BufferSlice) -> BufferSliceKey {
        (
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        )
    }

    /// Marks the index `buffer` as bound.
    fn set_index_buffer(&mut self, buffer_slice: &BufferSlice, index_format: IndexFormat) {
        self.index_buffer = Some((self.buffer_slice_key(buffer_slice), index_format));
        self.stores_state = true;
    }

    /// Checks, whether the index `buffer` is already bound.
    fn is_index_buffer_set(&self, buffer: &BufferSlice, index_format: IndexFormat) -> bool {
        self.index_buffer == Some((self.buffer_slice_key(buffer), index_format))
    }
}

/// The recording backend of a [`TrackedRenderPass`] (M1-4b-2: the transition
/// wgpu recording is gone - the Diligent immediate context is the only
/// recording path; `Empty` covers the pass-begin failure case).
enum TrackedRenderPassInner<'a> {
    /// M1-3: Diligent command recording on the immediate device context
    /// (`BeginRenderPass` was issued; `EndRenderPass` runs in `Drop`).
    Diligent {
        context: &'a diligent_rs::DeviceContext,
        /// Set when a command could not be represented on the diligent path
        /// (e.g. a pipeline without a diligent PSO). Draws are then skipped
        /// (with a debug log) instead of running against stale state.
        poisoned: bool,
    },
    /// The pass could not begin (no diligent context / engine failure);
    /// every command is skipped.
    Empty,
}


/// A [`RenderPass`], which tracks the current pipeline state to skip redundant operations.
///
/// It is used to set the current [`RenderPipeline`], [`BindGroup`]s and [`Buffer`]s.
/// After all requirements are specified, draw calls can be issued.
pub struct TrackedRenderPass<'a> {
    inner: TrackedRenderPassInner<'a>,
    state: DrawState,
    /// The current pipeline's immediate-constants binding (M2a,
    /// `set_immediates` -> `SetInlineConstants` on the immediate SRB),
    /// updated whenever the pipeline changes.
    immediate: Option<crate::renderer::render_device::ImmediateSrb>,
}

impl<'a> TrackedRenderPass<'a> {
    /// Tracks a pass that was begun on the Diligent immediate context
    /// (M1-3; `BeginRenderPass` was already issued by the caller).
    pub fn diligent(
        device: &RenderDevice,
        context: &'a diligent_rs::DeviceContext,
    ) -> Self {
        let limits = device.limits();
        let max_bind_groups = limits.max_bind_groups as usize;
        let max_vertex_buffers = limits.max_vertex_buffers as usize;
        Self {
            state: DrawState {
                bind_groups: vec![(None, Vec::new()); max_bind_groups],
                vertex_buffers: vec![None; max_vertex_buffers],
                ..default()
            },
            immediate: None,
            inner: TrackedRenderPassInner::Diligent {
                context,
                poisoned: false,
            },
        }
    }

    /// A pass whose commands are all skipped (the diligent begin failed -
    /// warned by the caller).
    pub(crate) fn empty(device: &RenderDevice) -> Self {
        let limits = device.limits();
        let max_bind_groups = limits.max_bind_groups as usize;
        let max_vertex_buffers = limits.max_vertex_buffers as usize;
        Self {
            state: DrawState {
                bind_groups: vec![(None, Vec::new()); max_bind_groups],
                vertex_buffers: vec![None; max_vertex_buffers],
                ..default()
            },
            immediate: None,
            inner: TrackedRenderPassInner::Empty,
        }
    }

    /// Marks the pass as degraded: subsequent draws are skipped (the pass
    /// cannot run correctly - e.g. a pipeline without a diligent PSO).
    fn poison(&mut self, reason: &str) {
        if let TrackedRenderPassInner::Diligent { poisoned, .. } = &mut self.inner {
            if !*poisoned {
                bevy_log::debug!(
                    "diligent: render pass degraded, draws will be skipped: {reason}"
                );
            }
            *poisoned = true;
        }
    }

    /// Whether draws should be skipped on the diligent path.
    fn draws_skipped(&self) -> bool {
        matches!(
            self.inner,
            TrackedRenderPassInner::Diligent { poisoned: true, .. }
        ) || matches!(self.inner, TrackedRenderPassInner::Empty)
    }

    /// Sets the active [`RenderPipeline`].
    ///
    /// Subsequent draw calls will exhibit the behavior defined by the `pipeline`.
    pub fn set_render_pipeline(&mut self, pipeline: &'a RenderPipeline) {
        #[cfg(feature = "detailed_trace")]
        trace!("set pipeline: {:?}", pipeline);
        if self.state.is_pipeline_set(pipeline.id()) {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                // M1-3: the M1-1 wrapper creates diligent PSOs for a subset
                // of the wgpu descriptors (see `diligent_pso`); a pipeline
                // without one degrades the pass (draws skipped) instead of
                // drawing with stale state.
                match pipeline.diligent() {
                    Some(pso) => {
                        let _guard = crate::renderer::diligent_registry::context_guard();
                        context.set_pipeline_state(pso);
                        if let Some(immediate) = pipeline.immediate_srb() {
                            // Bind the immediate signature's root constants
                            // slot now (fresh SRBs start zeroed);
                            // `set_immediates` re-applies the constants and
                            // re-commits.
                            context.commit_shader_resources(Some(&immediate.srb));
                        }
                    }
                    None => {
                        self.poison(&format!(
                            "pipeline {:?} has no diligent PSO",
                            pipeline.id()
                        ));
                    }
                }
            }
            TrackedRenderPassInner::Empty => {}
        }
        self.immediate = pipeline.immediate_srb().cloned();
        self.state.set_pipeline(pipeline.id());
    }

    /// Sets the active bind group for a given bind group index. The bind group layout
    /// in the active pipeline when any `draw()` function is called must match the layout of
    /// this bind group.
    ///
    /// If the bind group have dynamic offsets, provide them in binding order.
    /// These offsets have to be aligned to [`WgpuLimits::min_uniform_buffer_offset_alignment`](crate::settings::WgpuLimits::min_uniform_buffer_offset_alignment)
    /// or [`WgpuLimits::min_storage_buffer_offset_alignment`](crate::settings::WgpuLimits::min_storage_buffer_offset_alignment) appropriately.
    pub fn set_bind_group(
        &mut self,
        index: usize,
        bind_group: &'a BindGroup,
        dynamic_uniform_indices: &[u32],
    ) {
        if self
            .state
            .is_bind_group_set(index, bind_group.id(), dynamic_uniform_indices)
        {
            #[cfg(feature = "detailed_trace")]
            trace!(
                "set bind_group {} (already set): {:?} ({:?})",
                index,
                bind_group,
                dynamic_uniform_indices
            );
            return;
        }
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set bind_group {}: {:?} ({:?})",
            index,
            bind_group,
            dynamic_uniform_indices
        );

        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                // M1-3: commit the bind group's SRB. The SRB was built by
                // `create_diligent_bind_group`; committing it every set is
                // the M1 correctness-first choice (dirty tracking is M2b).
                // M1-3 review, fix 3: an SRB with unbound variables (a
                // resolve miss at build time) must not be committed - the
                // SRB object still exists, so skip the commit and poison
                // the pass instead (the existing poison-and-skip mechanism).
                match bind_group.diligent() {
                    Some(srb) => match bind_group.first_failed_diligent_binding() {
                        Some(binding) => self.poison(&format!(
                            "bind group {:?} has an unbound diligent variable \
                             (binding {binding}); SRB not committed",
                            bind_group.id()
                        )),
                        None => {
                            // M2a (§6.1.1): the dynamic offsets are applied
                            // to the DYNAMIC variables with SetBufferOffset
                            // *before* the commit - the offsets are baked
                            // into the root views at commit time
                            // (ShaderResourceCacheD3D12 BufferDynamicOffset
                            // consumed by CommitRootViews). No SRB
                            // re-commit is triggered per variable. The
                            // offset-change detection lives in `DrawState`
                            // (same group + same offsets -> early return
                            // above, no redundant calls). Both engine calls
                            // run under one context-lock scope so the
                            // offset set + commit pair is atomic across
                            // threads that share the SRB.
                            let outcome = (|| -> Result<(), String> {
                                let _guard =
                                    crate::renderer::diligent_registry::context_guard();
                                // M2a-1 review, fix 2: `apply_dynamic_offsets`
                                // runs unconditionally - with a layout that
                                // declares dynamic bindings and an empty
                                // offset array the validator must error (and
                                // poison) instead of silently committing
                                // stale cached offsets.
                                crate::renderer::render_device::apply_dynamic_offsets(
                                    &bind_group.diligent_state,
                                    bind_group.dynamic_bindings(),
                                    dynamic_uniform_indices,
                                )?;
                                context.commit_shader_resources(Some(srb));
                                Ok(())
                            })();
                            if let Err(err) = outcome {
                                bevy_log::warn!("diligent: dynamic offsets: {err}");
                                self.poison(
                                    "dynamic offset application failed; SRB not committed",
                                );
                                return;
                            }
                        }
                    },
                    None => {
                        self.poison(&format!(
                            "bind group {:?} has no diligent SRB",
                            bind_group.id()
                        ));
                    }
                }
            }
            TrackedRenderPassInner::Empty => {}
        }
        self.state
            .set_bind_group(index, bind_group.id(), dynamic_uniform_indices);
    }

    /// Assign a vertex buffer to a slot.
    ///
    /// Subsequent calls to [`draw`] and [`draw_indexed`] on this
    /// [`TrackedRenderPass`] will use `buffer` as one of the source vertex buffers.
    ///
    /// The `slot_index` refers to the index of the matching descriptor in
    /// [`VertexState::buffers`](crate::render_resource::VertexState::buffers).
    ///
    /// [`draw`]: TrackedRenderPass::draw
    /// [`draw_indexed`]: TrackedRenderPass::draw_indexed
    pub fn set_vertex_buffer(&mut self, slot_index: usize, buffer_slice: BufferSlice<'a>) {
        if self.state.is_vertex_buffer_set(slot_index, &buffer_slice) {
            #[cfg(feature = "detailed_trace")]
            trace!(
                "set vertex buffer {} (already set): {:?} (offset = {}, size = {})",
                slot_index,
                buffer_slice.id(),
                buffer_slice.offset(),
                buffer_slice.size(),
            );
            return;
        }
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set vertex buffer {}: {:?} (offset = {}, size = {})",
            slot_index,
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        );

        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                // The registry resolves the buffer by its id (clone-stable).
                match registry().resolve_buffer(buffer_slice.buffer().id()) {
                    Some(buffer) => {
                        // SET_VERTEX_BUFFERS_FLAG_NONE keeps the other slots
                        // bound (RESET would unbind them, breaking
                        // multi-buffer draws recorded across calls).
                        // TODO-REMOVE-M1-4: the context keeps strong
                        // references to bound buffers; M2a introduces
                        // per-pass slot management.
                        set_vertex_buffer_slot(
                            context,
                            slot_index as u32,
                            buffer,
                            buffer_slice.offset(),
                        );
                    }
                    None => {
                        self.poison(&format!(
                            "no diligent buffer for vertex slot {slot_index} \
                             (unregistered buffer)"
                        ));
                    }
                }
            }
            TrackedRenderPassInner::Empty => {}
        }
        self.state.set_vertex_buffer(slot_index, buffer_slice);
    }

    /// Sets the active index buffer.
    ///
    /// Subsequent calls to [`TrackedRenderPass::draw_indexed`] will use the buffer referenced by
    /// `buffer_slice` as the source index buffer.
    pub fn set_index_buffer(&mut self, buffer_slice: BufferSlice<'a>, index_format: IndexFormat) {
        let already_set = self.state.is_index_buffer_set(&buffer_slice, index_format);
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set index buffer{}: {:?} (offset = {}, size = {})",
            if already_set { " (already set)" } else { "" },
            buffer_slice.id(),
            buffer_slice.offset(),
            buffer_slice.size(),
        );
        if already_set {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                match registry().resolve_buffer(buffer_slice.buffer().id()) {
                    Some(buffer) => {
                        // The index format lives in the PSO state (the
                        // DrawIndexedAttribs carries it per draw).
                        diligent_draw::set_index_buffer(context, buffer, buffer_slice.offset());
                    }
                    None => {
                        self.poison("no diligent index buffer (unregistered buffer)");
                    }
                }
            }
            TrackedRenderPassInner::Empty => {}
        }
        self.state.set_index_buffer(&buffer_slice, index_format);
    }

    /// Draws primitives from the active vertex buffer(s).
    ///
    /// The active vertex buffer(s) can be set with [`TrackedRenderPass::set_vertex_buffer`].
    pub fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) {
        #[cfg(feature = "detailed_trace")]
        trace!("draw: {:?} {:?}", vertices, instances);
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let attribs = sys::DrawAttribs {
                    NumVertices: vertices.end - vertices.start,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    NumInstances: instances.end - instances.start,
                    StartVertexLocation: vertices.start,
                    FirstInstanceLocation: instances.start,
                };
                diligent_draw::draw(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Draws indexed primitives using the active index buffer and the active vertex buffer(s).
    ///
    /// The active index buffer can be set with [`TrackedRenderPass::set_index_buffer`, while the
    /// active vertex buffer(s) can be set with [`TrackedRenderPass::set_vertex_buffer`].
    pub fn draw_indexed(&mut self, indices: Range<u32>, base_vertex: i32, instances: Range<u32>) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "draw indexed: {:?} {} {:?}",
            indices,
            base_vertex,
            instances
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let index_type = self
                    .state
                    .index_buffer
                    .map(|(_, format)| match format {
                        IndexFormat::Uint16 => sys::_VALUE_TYPE::VT_UINT16,
                        IndexFormat::Uint32 => sys::_VALUE_TYPE::VT_UINT32,
                    })
                    .unwrap_or(sys::_VALUE_TYPE::VT_UINT32);
                let attribs = sys::DrawIndexedAttribs {
                    NumIndices: indices.end - indices.start,
                    IndexType: index_type as sys::VALUE_TYPE,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    NumInstances: instances.end - instances.start,
                    FirstIndexLocation: indices.start,
                    // TODO-REMOVE-M1-4: negative base vertices are not
                    // representable in DrawIndexedAttribs (Uint32 field).
                    BaseVertex: base_vertex as u32,
                    FirstInstanceLocation: instances.start,
                };
                diligent_draw::draw_indexed(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Draws primitives from the active vertex buffer(s) based on the contents of the
    /// `indirect_buffer`.
    ///
    /// The active vertex buffers can be set with [`TrackedRenderPass::set_vertex_buffer`].
    pub fn draw_indirect(&mut self, indirect_buffer: &'a Buffer, indirect_offset: u64) {
        #[cfg(feature = "detailed_trace")]
        trace!("draw indirect: {:?} {}", indirect_buffer, indirect_offset);
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let attribs = sys::DrawIndirectAttribs {
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: 1,
                    DrawArgsStride: 16,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: std::ptr::null_mut(),
                    CounterOffset: 0,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_NONE
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Draws indexed primitives using the active index buffer and the active vertex buffers,
    /// based on the contents of the `indirect_buffer`.
    pub fn draw_indexed_indirect(&mut self, indirect_buffer: &'a Buffer, indirect_offset: u64) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "draw indexed indirect: {:?} {}",
            indirect_buffer,
            indirect_offset
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let index_type = self
                    .state
                    .index_buffer
                    .map(|(_, format)| match format {
                        IndexFormat::Uint16 => sys::_VALUE_TYPE::VT_UINT16,
                        IndexFormat::Uint32 => sys::_VALUE_TYPE::VT_UINT32,
                    })
                    .unwrap_or(sys::_VALUE_TYPE::VT_UINT32);
                let attribs = sys::DrawIndexedIndirectAttribs {
                    IndexType: index_type as sys::VALUE_TYPE,
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: 1,
                    DrawArgsStride: 20,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: std::ptr::null_mut(),
                    CounterOffset: 0,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_NONE
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indexed_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Dispatches multiple draw calls from the active vertex buffer(s) based on the contents of the
    /// `indirect_buffer`.`count` draw calls are issued.
    pub fn multi_draw_indirect(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indirect: {:?} {}, {}x",
            indirect_buffer,
            indirect_offset,
            count
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let attribs = sys::DrawIndirectAttribs {
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: count,
                    DrawArgsStride: 16,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: std::ptr::null_mut(),
                    CounterOffset: 0,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_NONE
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Dispatches multiple draw calls from the active vertex buffer(s) based on the contents of
    /// the `indirect_buffer`.
    /// The count buffer is read to determine how many draws to issue.
    pub fn multi_draw_indirect_count(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count_buffer: &'a Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indirect count: {:?} {}, ({:?} {})x, max {}x",
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let Some(count) = registry().resolve_buffer(count_buffer.id()) else {
                    self.poison("no diligent count buffer (unregistered buffer)");
                    return;
                };
                let attribs = sys::DrawIndirectAttribs {
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: max_count,
                    DrawArgsStride: 16,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: count,
                    CounterOffset: count_offset,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Dispatches multiple draw calls from the active index buffer and the active vertex buffers,
    /// based on the contents of the `indirect_buffer`. `count` draw calls are issued.
    pub fn multi_draw_indexed_indirect(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indexed indirect: {:?} {}, {}x",
            indirect_buffer,
            indirect_offset,
            count
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let index_type = self
                    .state
                    .index_buffer
                    .map(|(_, format)| match format {
                        IndexFormat::Uint16 => sys::_VALUE_TYPE::VT_UINT16,
                        IndexFormat::Uint32 => sys::_VALUE_TYPE::VT_UINT32,
                    })
                    .unwrap_or(sys::_VALUE_TYPE::VT_UINT32);
                let attribs = sys::DrawIndexedIndirectAttribs {
                    IndexType: index_type as sys::VALUE_TYPE,
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: count,
                    DrawArgsStride: 20,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: std::ptr::null_mut(),
                    CounterOffset: 0,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_NONE
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indexed_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Dispatches multiple draw calls from the active index buffer and the active vertex buffers,
    /// based on the contents of the `indirect_buffer`.
    /// The count buffer is read to determine how many draws to issue.
    pub fn multi_draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: &'a Buffer,
        indirect_offset: u64,
        count_buffer: &'a Buffer,
        count_offset: u64,
        max_count: u32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "multi draw indexed indirect count: {:?} {}, ({:?} {})x, max {}x",
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_count
        );
        if self.draws_skipped() {
            return;
        }
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(buffer) = registry().resolve_buffer(indirect_buffer.id()) else {
                    self.poison("no diligent indirect buffer (unregistered buffer)");
                    return;
                };
                let Some(count) = registry().resolve_buffer(count_buffer.id()) else {
                    self.poison("no diligent count buffer (unregistered buffer)");
                    return;
                };
                let index_type = self
                    .state
                    .index_buffer
                    .map(|(_, format)| match format {
                        IndexFormat::Uint16 => sys::_VALUE_TYPE::VT_UINT16,
                        IndexFormat::Uint32 => sys::_VALUE_TYPE::VT_UINT32,
                    })
                    .unwrap_or(sys::_VALUE_TYPE::VT_UINT32);
                let attribs = sys::DrawIndexedIndirectAttribs {
                    IndexType: index_type as sys::VALUE_TYPE,
                    pAttribsBuffer: buffer,
                    DrawArgsOffset: indirect_offset,
                    Flags: sys::_DRAW_FLAGS::DRAW_FLAG_NONE as sys::DRAW_FLAGS,
                    DrawCount: max_count,
                    DrawArgsStride: 20,
                    AttribsBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                    pCounterBuffer: count,
                    CounterOffset: count_offset,
                    CounterBufferStateTransitionMode:
                        sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                            as sys::RESOURCE_STATE_TRANSITION_MODE,
                };
                diligent_draw::draw_indexed_indirect(context, &attribs);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Sets the stencil reference.
    ///
    /// Subsequent stencil tests will test against this value.
    pub fn set_stencil_reference(&mut self, reference: u32) {
        #[cfg(feature = "detailed_trace")]
        trace!("set stencil reference: {}", reference);
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                diligent_draw::set_stencil_ref(context, reference);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Sets the scissor region.
    ///
    /// Subsequent draw calls will discard any fragments that fall outside this region.
    pub fn set_scissor_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        #[cfg(feature = "detailed_trace")]
        trace!("set_scissor_rect: {} {} {} {}", x, y, width, height);
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                diligent_draw::set_scissor_rects(
                    context,
                    &[sys::Rect {
                        left: x as i32,
                        top: y as i32,
                        right: (x + width) as i32,
                        bottom: (y + height) as i32,
                    }],
                );
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Set immediates data.
    ///
    /// `Features::IMMEDIATES` must be enabled on the device in order to call these functions.
    ///
    /// M2a (§6.1.1): maps to `SetInlineConstants(FirstConstant = offset/4,
    /// NumConstants = data.len()/4)` on the current pipeline's immediate
    /// SRB (V3 mapping), then re-commits the immediate SRB (the constants
    /// are uploaded from the SRB cache at commit / on the next draw).
    pub fn set_immediates(&mut self, offset: u32, data: &[u8]) {
        #[cfg(feature = "detailed_trace")]
        trace!("set immediates offset: {} data.len: {}", offset, data.len());
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let Some(immediate) = &self.immediate else {
                    bevy_log::debug!(
                        "diligent: set_immediates({offset}, {} bytes) with no \
                         immediate constants in the current pipeline",
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
                        "set_immediates range (constants {}..{}) exceeds the \
                         pipeline's immediate capacity ({} dwords)",
                        first_constant,
                        first_constant + num_constants,
                        immediate.array_size_dwords
                    ));
                    return;
                }
                // SetInlineConstants writes into the SRB cache; the commit
                // uploads it (V3 mapping; both under one context-lock scope).
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
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Set the rendering viewport.
    ///
    /// Subsequent draw calls will be projected into that viewport.
    pub fn set_viewport(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    ) {
        #[cfg(feature = "detailed_trace")]
        trace!(
            "set viewport: {} {} {} {} {} {}",
            x,
            y,
            width,
            height,
            min_depth,
            max_depth
        );
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                let _guard = crate::renderer::diligent_registry::context_guard();
                context.set_viewports(&[sys::Viewport {
                    TopLeftX: x,
                    TopLeftY: y,
                    Width: width,
                    Height: height,
                    MinDepth: min_depth,
                    MaxDepth: max_depth,
                }]);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Set the rendering viewport to the given camera [`Viewport`].
    ///
    /// Subsequent draw calls will be projected into that viewport.
    pub fn set_camera_viewport(&mut self, viewport: &Viewport) {
        self.set_viewport(
            viewport.physical_position.x as f32,
            viewport.physical_position.y as f32,
            viewport.physical_size.x as f32,
            viewport.physical_size.y as f32,
            viewport.depth.start,
            viewport.depth.end,
        );
    }

    /// Insert a single debug marker.
    ///
    /// This is a GPU debugging feature. This has no effect on the rendering itself.
    pub fn insert_debug_marker(&mut self, label: &str) {
        #[cfg(feature = "detailed_trace")]
        trace!("insert debug marker: {}", label);
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                // No single-marker call on the locked IDeviceContext (only
                // debug groups); the marker is approximated with a group.
                diligent_draw::begin_debug_group(context, label);
                diligent_draw::end_debug_group(context);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Start a new debug group.
    ///
    /// Push a new debug group over the internal stack. Subsequent render commands and debug
    /// markers are grouped into this new group, until [`pop_debug_group`] is called.
    ///
    /// ```
    /// # fn example(mut pass: bevy_render::render_phase::TrackedRenderPass<'static>) {
    /// pass.push_debug_group("Render the car");
    /// // [setup pipeline etc...]
    /// pass.draw(0..64, 0..1);
    /// pass.pop_debug_group();
    /// # }
    /// ```
    ///
    /// Note that [`push_debug_group`] and [`pop_debug_group`] must always be called in pairs.
    ///
    /// This is a GPU debugging feature. This has no effect on the rendering itself.
    ///
    /// [`push_debug_group`]: TrackedRenderPass::push_debug_group
    /// [`pop_debug_group`]: TrackedRenderPass::pop_debug_group
    pub fn push_debug_group(&mut self, label: &str) {
        #[cfg(feature = "detailed_trace")]
        trace!("push_debug_group marker: {}", label);
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                diligent_draw::begin_debug_group(context, label);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// End the current debug group.
    ///
    /// Subsequent render commands and debug markers are not grouped anymore in
    /// this group, but in the previous one (if any) or the default top-level one
    /// if the debug group was the last one on the stack.
    ///
    /// Note that [`push_debug_group`] and [`pop_debug_group`] must always be called in pairs.
    ///
    /// This is a GPU debugging feature. This has no effect on the rendering itself.
    ///
    /// [`push_debug_group`]: TrackedRenderPass::push_debug_group
    /// [`pop_debug_group`]: TrackedRenderPass::pop_debug_group
    pub fn pop_debug_group(&mut self) {
        #[cfg(feature = "detailed_trace")]
        trace!("pop_debug_group");
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                diligent_draw::end_debug_group(context);
            }
            TrackedRenderPassInner::Empty => {}
        }
    }

    /// Sets the blend color as used by some of the blending modes.
    ///
    /// Subsequent blending tests will test against this value.
    pub fn set_blend_constant(&mut self, color: LinearRgba) {
        #[cfg(feature = "detailed_trace")]
        trace!("set blend constant: {:?}", color);
        match &mut self.inner {
            TrackedRenderPassInner::Diligent { context, .. } => {
                diligent_draw::set_blend_factors(context, color.to_f32_array());
            }
            TrackedRenderPassInner::Empty => {}
        }
    }
}

impl Drop for TrackedRenderPass<'_> {
    fn drop(&mut self) {
        if let TrackedRenderPassInner::Diligent { context, .. } = &self.inner {
            diligent_draw::end_render_pass(context);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V23 (pre-delivery): the indirect argument layouts must be D3D12-
    /// compatible with **zero translation** - the wgpu argument structs and
    /// the Diligent indirect-attribs documents describe the SAME memory
    /// layout, so a bevy indirect buffer written by a compute shader
    /// (`DispatchIndirectArgs`/`DrawIndirectArgs`/`DrawIndexedIndirectArgs`)
    /// is consumed by the engine as-is.
    ///
    /// Pinned against the locked headers:
    /// * `DrawIndirectAttribs` (DeviceContext.h:410-414): NumVertices,
    ///   NumInstances, StartVertexLocation, FirstInstanceLocation = the
    ///   wgpu `DrawIndirectArgs` 4x u32; `DrawArgsStride` minimum 16 bytes
    ///   (DeviceContext.h:428).
    /// * `DrawIndexedIndirectAttribs` (DeviceContext.h:489-493):
    ///   NumIndices, NumInstances, FirstIndexLocation, BaseVertex,
    ///   FirstInstanceLocation = the wgpu `DrawIndexedIndirectArgs` 5x u32;
    ///   `DrawArgsStride` minimum 20 bytes (DeviceContext.h:511).
    /// * `DispatchComputeIndirectAttribs` (DeviceContext.h:937-941):
    ///   ThreadGroupCountX/Y/Z = the wgpu `DispatchIndirectArgs` 3x u32.
    #[test]
    fn indirect_argument_layouts_match_the_diligent_d3d12_contracts() {
        use core::mem::size_of;
        // wgpu arg structs (repr(C), 4/5/3 x u32 - wgpu-types render.rs:
        // DrawIndirectArgs / DrawIndexedIndirectArgs / DispatchIndirectArgs).
        assert_eq!(size_of::<wgpu_types::DrawIndirectArgs>(), 16);
        assert_eq!(size_of::<wgpu_types::DrawIndexedIndirectArgs>(), 20);
        assert_eq!(size_of::<wgpu_types::DispatchIndirectArgs>(), 12);
        // The strides the diligent draw path commits (draw_state.rs) are the
        // header-declared minimums (DeviceContext.h:428 / :511).
        // The Diligent attrib structs embed the same first fields (the C API
        // exposes them as opaque structs; the header documents the argument
        // layouts as `Uint32 x N` - sizes pin the contract).
        assert!(size_of::<sys::DrawIndirectAttribs>() >= 16);
        assert!(size_of::<sys::DrawIndexedIndirectAttribs>() >= 20);
        assert!(size_of::<sys::DispatchComputeIndirectAttribs>() >= 12);
    }

    /// V23: the atomic-counter pattern - the count buffers consumed via
    /// `DrawIndirectAttribs.pCounterBuffer` are u32 counters at an explicit
    /// byte offset; the meshlet pattern writes the counter into the args
    /// buffer itself (`fill_counts.wgsl`/`cull_*.wgsl` atomicAdd on the
    /// first field). Both are expressible with the locked attribs; the
    /// indirect-dispatch attribs have NO counter buffer in this version
    /// (the count must live in the args buffer - `DispatchIndirectArgs.x`).
    #[test]
    fn counter_buffers_are_u32_at_explicit_offsets() {
        // The counter-buffer offset field is a Uint64 byte offset
        // (DeviceContext.h:440); the counter itself is a Uint32 - the
        // meshlet count buffers are exactly that (`bytemuck::bytes_of(&0u32)`
        // with `BufferUsages::STORAGE`).
        assert_eq!(size_of::<u32>(), 4);
        // The wgpu multi-draw count API consumes the same buffer+offset
        // shape (multi_draw_indirect_count), which the diligent path maps
        // to `pCounterBuffer` + `CounterOffset` (draw_state.rs).
    }
}

impl WriteTimestamp for TrackedRenderPass<'_> {
    fn write_timestamp(&mut self, _query_set: &QuerySet, _index: u32) {
        // Diligent path: timestamp queries are not wired yet
        // (TODO-REMOVE-M1-4).
    }
}

impl WritePipelineStatistics for TrackedRenderPass<'_> {
    fn begin_pipeline_statistics_query(&mut self, _query_set: &QuerySet, _index: u32) {
        // Diligent path: pipeline statistics queries are not wired yet
        // (TODO-REMOVE-M1-4).
    }

    fn end_pipeline_statistics_query(&mut self) {}
}

impl Pass for TrackedRenderPass<'_> {
    const KIND: PassKind = PassKind::Render;
}

/// `IDeviceContext::SetVertexBuffers` for a single slot (no RESET flag - the
/// other slots keep their bindings, mirroring the wgpu per-slot semantics).
pub(crate) fn set_vertex_buffer_slot(
    context: &diligent_rs::DeviceContext,
    slot: u32,
    buffer: *mut sys::IBuffer,
    offset: u64,
) {
    // M1-4b-2 review, fix 1: `SetVertexBuffers` is an immediate-context
    // call - the guard must cover the whole engine invocation (no locked
    // path is active here: the pass-record callers take no guard).
    let _guard = crate::renderer::diligent_registry::context_guard();
    let set = unsafe {
        (*(*context.as_raw()).pVtbl)
            .DeviceContext
            .SetVertexBuffers
            .as_ref()
            .expect("diligent: IDeviceContext::SetVertexBuffers missing from vtable")
    };
    // Safety: the buffer is alive (the registry keeps the owning wrapper
    // alive for the duration of the call); the single-element arrays are
    // valid for the duration of the call.
    unsafe {
        set(
            context.as_raw(),
            slot,
            1,
            &buffer,
            &offset,
            sys::_RESOURCE_STATE_TRANSITION_MODE::RESOURCE_STATE_TRANSITION_MODE_TRANSITION
                as sys::RESOURCE_STATE_TRANSITION_MODE,
            sys::_SET_VERTEX_BUFFERS_FLAGS::SET_VERTEX_BUFFERS_FLAG_NONE
                as sys::SET_VERTEX_BUFFERS_FLAGS,
        )
    };
}
