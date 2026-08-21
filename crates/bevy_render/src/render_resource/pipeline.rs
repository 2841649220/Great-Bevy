use crate::renderer::diligent_registry::DiligentHandle;
use crate::renderer::render_device::ImmediateSrb;
use bevy_utils::define_atomic_id;

define_atomic_id!(RenderPipelineId);

/// A [`RenderPipeline`] represents a graphics pipeline and its stages (shaders), bindings and vertex buffers.
///
/// The primary handle is the Diligent [`PipelineState`](diligent_rs::PipelineState)
/// (M1-4b-2: the transition wgpu handle is gone).
/// Can be created via [`RenderDevice::create_render_pipeline`](crate::renderer::RenderDevice::create_render_pipeline).
#[derive(Clone)]
pub struct RenderPipeline {
    pub(crate) id: RenderPipelineId,
    /// The Diligent graphics PSO (`None` when the Diligent creation failed -
    /// logged).
    pub(crate) value: Option<DiligentHandle<diligent_rs::PipelineState>>,
    /// The immediate-constants SRB of this pipeline's layout (M2a,
    /// §6.1.1: `set_immediates` -> `SetInlineConstants` on it; `None` when
    /// the layout has no immediate constants).
    pub(crate) immediate: Option<ImmediateSrb>,
}

impl core::fmt::Debug for RenderPipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RenderPipeline")
            .field("id", &self.id)
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

impl RenderPipeline {
    #[inline]
    pub fn id(&self) -> RenderPipelineId {
        self.id
    }

    /// The Diligent graphics PSO, when this instance has one (consumed by
    /// the M1-3 diligent render-pass recording).
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::PipelineState> {
        self.value.as_deref()
    }

    /// The immediate-constants SRB of this pipeline, when its layout
    /// declares immediates (M2a: the `set_immediates` target).
    pub(crate) fn immediate_srb(&self) -> Option<&ImmediateSrb> {
        self.immediate.as_ref()
    }

    /// Whether the Diligent PSO (if any) has finished compiling.
    ///
    /// M2a async PSO (task 6): a pipeline created with
    /// `create_render_pipeline_diligent_async` carries a PSO that is still
    /// compiling; the pipeline-cache state machine polls this until
    /// `true`, then promotes the pipeline to `Ok`. A `None` PSO (creation
    /// failed or no diligent device) counts as "ready" so the pass degrades
    /// immediately instead of stalling. A `FAILED` status is reported as
    /// ready-with-error through `diligent_pso_failed`.
    pub(crate) fn diligent_pso_ready(&self) -> bool {
        use diligent_rs::diligent_sys::bindings as sys;
        let Some(pso) = self.value.as_deref() else {
            return true;
        };
        match pso.status(false) {
            Ok(status) => {
                status
                    != sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_COMPILING
                        as sys::PIPELINE_STATE_STATUS
            }
            Err(_) => true,
        }
    }

    /// Whether the Diligent PSO finished with `FAILED` status (a real
    /// async-compile failure, distinct from "still compiling").
    pub(crate) fn diligent_pso_failed(&self) -> bool {
        use diligent_rs::diligent_sys::bindings as sys;
        let Some(pso) = self.value.as_deref() else {
            return false;
        };
        matches!(
            pso.status(false),
            Ok(status)
                if status
                    == sys::_PIPELINE_STATE_STATUS::PIPELINE_STATE_STATUS_FAILED
                        as sys::PIPELINE_STATE_STATUS
        )
    }
}

define_atomic_id!(ComputePipelineId);

/// A [`ComputePipeline`] represents a compute pipeline and its single shader stage.
///
/// The primary handle is the Diligent [`PipelineState`](diligent_rs::PipelineState)
/// (M1-4b-2: the transition wgpu handle is gone).
/// Can be created via [`RenderDevice::create_compute_pipeline`](crate::renderer::RenderDevice::create_compute_pipeline).
#[derive(Clone)]
pub struct ComputePipeline {
    pub(crate) id: ComputePipelineId,
    /// The Diligent compute PSO (`None` when the Diligent creation failed -
    /// logged).
    pub(crate) value: Option<DiligentHandle<diligent_rs::PipelineState>>,
    /// The immediate-constants SRB of this pipeline's layout (M2a,
    /// §6.1.1: `set_immediates` -> `SetInlineConstants` on it; `None` when
    /// the layout has no immediate constants).
    pub(crate) immediate: Option<ImmediateSrb>,
}

impl core::fmt::Debug for ComputePipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ComputePipeline")
            .field("id", &self.id)
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

impl ComputePipeline {
    /// Returns the [`ComputePipelineId`].
    #[inline]
    pub fn id(&self) -> ComputePipelineId {
        self.id
    }

    /// The Diligent compute PSO, when this instance has one.
    pub(crate) fn diligent(&self) -> Option<&diligent_rs::PipelineState> {
        self.value.as_deref()
    }

    /// The immediate-constants SRB of this pipeline, when its layout
    /// declares immediates (M2a: the `set_immediates` target).
    pub(crate) fn immediate_srb(&self) -> Option<&ImmediateSrb> {
        self.immediate.as_ref()
    }
}
