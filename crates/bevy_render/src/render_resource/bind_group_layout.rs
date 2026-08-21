use crate::renderer::diligent_registry::DiligentHandle;
use bevy_utils::define_atomic_id;
use wgpu_types::BufferBindingType;

define_atomic_id!(BindGroupLayoutId);

/// Bind group layouts define the interface of resources (e.g. buffers, textures, samplers)
/// for a shader. The actual resource binding is done via a [`BindGroup`](super::BindGroup).
///
/// This is a lightweight thread-safe wrapper, which can be cloned as needed to workaround
/// lifetime management issues. The Diligent
/// [`PipelineResourceSignature`](diligent_rs::PipelineResourceSignature) carried here is the
/// primary handle - the SRB-side signature used by
/// [`RenderDevice::create_bind_group`](crate::renderer::RenderDevice::create_bind_group)
/// (M1-4b-2: the transition wgpu handle is gone).
///
/// Can be created via [`RenderDevice::create_bind_group_layout`](crate::renderer::RenderDevice::create_bind_group_layout).
#[derive(Clone)]
pub struct BindGroupLayout {
    pub(crate) id: BindGroupLayoutId,
    /// The SRB-side pipeline resource signature (`None` when the PRS
    /// creation failed - logged). Built from the layout descriptor with
    /// canonical binding names; compatible with the shader-named PSO-side
    /// signatures (V15 report; `IsCompatibleWith` disregards names).
    pub(crate) value: Option<DiligentHandle<diligent_rs::PipelineResourceSignature>>,
    /// (binding -> buffer binding type) captured at layout creation - drives
    /// the SRB `SetBufferRange` vs default-view `Set` branch (the engine only
    /// allows `SetBufferRange` for constant buffers).
    pub(crate) buffer_binding_types: std::collections::HashMap<u32, BufferBindingType>,
    /// The ascending binding indices of the buffer entries with
    /// `has_dynamic_offset` (M2a, §6.1.1): the `set_bind_group(.., &[u32])`
    /// dynamic-offset array maps to these SRB variables one-to-one, in
    /// binding order (the wgpu dynamic-offset contract).
    pub(crate) dynamic_bindings: alloc::sync::Arc<[u32]>,
    /// The `GetVariableByName` probe stages for SRBs built from this layout
    /// (M2a): the engine derives the signature's pipeline type from the
    /// union of the entries' visibility, and probing an invalid stage logs
    /// an engine warning per probe (see
    /// [`diligent_mapping::srb_variable_probe_stages`]).
    pub(crate) srb_probe_stages: wgpu_types::ShaderStages,
}

impl core::fmt::Debug for BindGroupLayout {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BindGroupLayout")
            .field("id", &self.id)
            .field("diligent", &self.value.is_some())
            .finish()
    }
}

impl PartialEq for BindGroupLayout {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for BindGroupLayout {}

impl core::hash::Hash for BindGroupLayout {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.id.0.hash(state);
    }
}

impl BindGroupLayout {
    /// Returns the [`BindGroupLayoutId`] representing the unique ID of the bind group layout.
    #[inline]
    pub fn id(&self) -> BindGroupLayoutId {
        self.id
    }

    /// The SRB-side Diligent signature, when this instance has one.
    pub(crate) fn prs(&self) -> Option<&DiligentHandle<diligent_rs::PipelineResourceSignature>> {
        self.value.as_ref()
    }

    /// The dynamic-offset buffer bindings of this layout, ascending by
    /// binding index (M2a, §6.1.1): the offset array of a matching
    /// `set_bind_group` call maps to these one-to-one.
    pub(crate) fn dynamic_bindings(&self) -> &[u32] {
        &self.dynamic_bindings
    }

    /// The `GetVariableByName` probe stages for SRBs built from this layout
    /// (M2a): only stages consistent with the signature's pipeline type -
    /// probing invalid stages makes the engine log a warning per probe
    /// (ShaderResourceBindingBase.hpp:185).
    pub(crate) fn srb_probe_stages(&self) -> wgpu_types::ShaderStages {
        self.srb_probe_stages
    }
}
