use super::{
    diligent_draw, diligent_features, diligent_mapping, diligent_pso, diligent_registry,
    RenderQueue,
};
use crate::render_resource::{
    BindGroup, BindGroupDiligentState, BindGroupLayout, Buffer, BufferSlice, ComputePipeline,
    RawRenderPipelineDescriptor, RenderPipeline, Sampler, ShaderModule, ShaderModuleDescriptor,
    ShaderSource, Texture, WgpuSampler,
};
use crate::render_resource::wgpu_compat::WgpuBindGroup;
use crate::render_resource::{BindGroupEntry, BufferAsyncError};
use crate::renderer::{diligent_registry::DiligentHandle, WgpuWrapper};
use alloc::ffi::CString;
use alloc::sync::Arc;
use bevy_ecs::resource::Resource;
use bevy_utils::default;
use core::ffi::CStr;
use diligent_rs::diligent_sys::bindings as sys;
use std::sync::{Mutex, Weak};
use wgpu_types::{BindGroupLayoutEntry, BufferBindingType, PollError, PollStatus};

/// This GPU device is responsible for the creation of most rendering and compute resources.
///
/// M1b: the device wraps a Diligent [`RenderDevice`](diligent_rs::RenderDevice) +
/// immediate [`DeviceContext`](diligent_rs::DeviceContext) (the primary handles).
/// M1-4b-2: the transition wgpu device is gone; `wgpu_device()` serves the
/// self-authored capability facade ([`Device`](super::Device)).
#[derive(Resource, Clone)]
pub struct RenderDevice {
    /// The Diligent render device (`None` when the engine could not be
    /// initialized).
    diligent_device: Option<DiligentHandle<diligent_rs::RenderDevice>>,
    /// The Diligent immediate context (single context; TODO-REMOVE-M1-4:
    /// M2a introduces the binding model / deferred contexts).
    diligent_context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
    /// The engine factory the diligent device/context were created with
    /// (M1-3: the swap-chain creation entry point; the process-wide
    /// singleton, kept alive for the device's lifetime).
    diligent_factory: Option<DiligentHandle<diligent_rs::EngineFactoryD3D12>>,
    /// The backend the diligent device was created for (drives the shader
    /// compilation target: HLSL on D3D12, SPIR-V on Vulkan).
    backend: diligent_pso::DiligentBackend,
    /// The wgpu-29-compatible device facade (`wgpu_device()` accessor:
    /// features/limits + BLAS/TLAS handle creation).
    device: WgpuWrapper<super::Device>,
    /// (shader module id) -> shader record, so the public create_*_pipeline
    /// entry points can resolve the Diligent objects from the descriptors
    /// (M1-4b-2: keyed by the module handle's id).
    shader_modules: Arc<Mutex<std::collections::HashMap<u32, Weak<diligent_pso::ShaderModuleRecord>>>>,
    /// (pipeline layout id) -> layout record (PRS array + immediate_size).
    pipeline_layouts:
        Arc<Mutex<std::collections::HashMap<u32, Weak<diligent_pso::PipelineLayoutRecord>>>>,
    /// M1-3: bounded cache of render-pass + framebuffer engine objects and
    /// the derived attachment views (see `diligent_draw::RenderPassCache`).
    render_passes: Arc<Mutex<diligent_draw::RenderPassCache>>,
    /// M1-4a: the device capability set derived from the diligent
    /// `GetDeviceInfo`/`GetAdapterInfo` queries (see
    /// `diligent_features::DiligentCaps`). `features()`/`limits()` serve
    /// these when present.
    diligent_caps: Option<diligent_features::DiligentCaps>,
    /// M1-4b-2: the latched shader validation error (the wgpu
    /// `push_error_scope`/`pop` replacement - see `create_shader_module`).
    shader_validation_error: Arc<Mutex<Option<String>>>,
    /// M3b §8.10: the in-memory pipeline-state cache (LOAD_STORE). PSO
    /// creation feeds its pointer into `pPSOCache` so a same-named PSO
    /// reuses the driver blob and skips recompilation; `None` on devices
    /// without PSO-cache support (D3D11/OpenGL).
    pso_cache: Option<DiligentHandle<diligent_rs::PipelineStateCache>>,
}

impl RenderDevice {
    /// Builds the device from the diligent engine bootstrap (created by
    /// `initialize_renderer`) plus the derived capability data.
    pub(crate) fn from_parts(
        diligent_factory: Option<DiligentHandle<diligent_rs::EngineFactoryD3D12>>,
        diligent_device: Option<DiligentHandle<diligent_rs::RenderDevice>>,
        diligent_context: Option<DiligentHandle<diligent_rs::DeviceContext>>,
        backend: diligent_pso::DiligentBackend,
        diligent_caps: Option<diligent_features::DiligentCaps>,
        device: super::Device,
        pso_cache: Option<DiligentHandle<diligent_rs::PipelineStateCache>>,
    ) -> Self {
        Self {
            diligent_device,
            diligent_context,
            diligent_factory,
            backend,
            device: WgpuWrapper::new(device),
            shader_modules: default(),
            pipeline_layouts: default(),
            render_passes: default(),
            diligent_caps,
            shader_validation_error: default(),
            pso_cache,
        }
    }

    /// The Diligent render device (None when the engine failed to
    /// initialize).
    pub(crate) fn diligent_device(&self) -> Option<&diligent_rs::RenderDevice> {
        self.diligent_device.as_deref()
    }

    /// The native `ID3D12Device` handle of the underlying D3D12 device
    /// (M5a, task 16.1 escape hatch for vendor SDKs: NGX DLSS, FSR, XeSS,
    /// DirectSR).
    ///
    /// Returns the borrowed raw `ID3D12Device*` (do not Release it; it is
    /// owned by the engine for the device's lifetime). `None` when the
    /// engine failed to initialize or the device is not D3D12 (e.g. a
    /// Vulkan device). The `unsafe` surface is minimized: callers get a raw
    /// pointer they may hand to a vendor SDK verbatim.
    pub fn native_d3d12_device(&self) -> Option<*mut diligent_rs::diligent_sys::bindings::ID3D12Device> {
        let diligent = self.diligent_device()?;
        match diligent.native_d3d12_device() {
            Ok(handle) => handle,
            Err(err) => {
                bevy_log::warn!("diligent: native ID3D12Device resolution failed ({err})");
                None
            }
        }
    }

    /// The in-memory pipeline-state cache (M3b §8.10; `None` when the
    /// device does not support PSO caches). PSO creation feeds its pointer
    /// into `pPSOCache`.
    pub(crate) fn pso_cache(&self) -> Option<&diligent_rs::PipelineStateCache> {
        self.pso_cache.as_deref()
    }

    /// The Diligent immediate device context.
    pub(crate) fn diligent_context(&self) -> Option<&diligent_rs::DeviceContext> {
        self.diligent_context.as_deref()
    }

    /// A shared handle to the Diligent immediate device context (the
    /// render-context state keeps one for pass recording).
    pub(crate) fn diligent_context_handle(
        &self,
    ) -> Option<DiligentHandle<diligent_rs::DeviceContext>> {
        self.diligent_context.clone()
    }

    /// The engine factory (stored, or the process-wide singleton when this
    /// device was constructed without one).
    pub(crate) fn engine_factory(&self) -> Option<DiligentHandle<diligent_rs::EngineFactoryD3D12>> {
        if let Some(factory) = &self.diligent_factory {
            return Some(factory.clone());
        }
        diligent_rs::EngineFactoryD3D12::d3d12()
            .ok()
            .map(|factory| DiligentHandle::new(Arc::new(factory)))
    }

    /// The render-pass/framebuffer cache (M1-3; see
    /// `diligent_draw::RenderPassCache`).
    pub(crate) fn render_pass_cache(&self) -> &Mutex<diligent_draw::RenderPassCache> {
        &self.render_passes
    }

    /// Drops every cached render pass / framebuffer (called when a swap
    /// chain is resized - the cached framebuffers reference the old back
    /// buffers).
    pub(crate) fn invalidate_render_passes(&self) {
        self.render_passes.lock().unwrap().clear();
    }

    /// The backend the diligent device was created for.
    pub(crate) fn diligent_backend(&self) -> diligent_pso::DiligentBackend {
        self.backend
    }

    /// Registers a shader record (keyed by the shader module handle's id -
    /// the reference `desc.vertex.module` carries), so the public
    /// `create_render_pipeline` / `create_compute_pipeline` entry points can
    /// resolve the Diligent objects from the descriptors.
    pub(crate) fn register_shader_module(&self, record: &Arc<diligent_pso::ShaderModuleRecord>) {
        self.shader_modules
            .lock()
            .unwrap()
            .insert(record.module.registry_key(), Arc::downgrade(record));
    }

    /// Registers a layout record (keyed by the pipeline layout handle's id -
    /// the reference `desc.layout` carries).
    pub(crate) fn register_pipeline_layout(
        &self,
        record: &Arc<diligent_pso::PipelineLayoutRecord>,
    ) {
        self.pipeline_layouts
            .lock()
            .unwrap()
            .insert(record.layout.registry_key(), Arc::downgrade(record));
    }

    fn shader_record(
        &self,
        module: &ShaderModule,
    ) -> Option<Arc<diligent_pso::ShaderModuleRecord>> {
        self.shader_modules
            .lock()
            .unwrap()
            .get(&module.registry_key())
            .and_then(Weak::upgrade)
    }

    fn layout_record(
        &self,
        layout: &crate::render_resource::PipelineLayout,
    ) -> Option<Arc<diligent_pso::PipelineLayoutRecord>> {
        self.pipeline_layouts
            .lock()
            .unwrap()
            .get(&layout.registry_key())
            .and_then(Weak::upgrade)
    }

    /// List all [`Features`](wgpu_types::Features) that may be used with this device.
    ///
    /// M1-4a: the mask derived from the Diligent capability queries
    /// (`DiligentFeatures`, see `diligent_features.rs`), intersected at
    /// construction with the `WgpuSettings` feature bits (which carry the
    /// `disabled_features`/`requested_features` settings).
    ///
    /// Functions may panic if you use unsupported features.
    #[inline]
    pub fn features(&self) -> wgpu_types::Features {
        self.diligent_caps
            .as_ref()
            .map_or_else(|| self.device.features(), |caps| caps.features().as_features())
    }

    /// List all [`Limits`](wgpu_types::Limits) that were requested of this device.
    ///
    /// M1-4a: the storage-resource limits (the `max_storage_*_per_shader_stage`
    /// pair - the `gpu_array_buffer.rs` path-selection input) are derived
    /// from the Diligent capability queries; the remaining fields are the
    /// wgpu default limits (the same hardware drives both paths).
    ///
    /// If any of these limits are exceeded, functions may panic.
    #[inline]
    pub fn limits(&self) -> wgpu_types::Limits {
        let mut limits = self.device.limits();
        if let Some(caps) = &self.diligent_caps {
            limits.max_storage_buffers_per_shader_stage = caps.max_storage_buffers_per_shader_stage();
            limits.max_storage_textures_per_shader_stage = caps.max_storage_textures_per_shader_stage();
        }
        limits
    }

    /// Compiles the shader source into a naga module.
    fn compile_shader_source(source: &ShaderSource) -> Result<naga::Module, String> {
        match source {
            ShaderSource::Naga(module) => Ok(module.as_ref().clone()),
            ShaderSource::Wgsl(src) => naga::front::wgsl::parse_str(src)
                .map_err(|e| format!("failed to parse WGSL shader: {e}")),
            #[cfg(feature = "shader_format_spirv")]
            ShaderSource::SpirV(words) => {
                naga::front::spv::Frontend::new(words.iter().copied(), &naga::front::spv::Options::default())
                    .parse()
                    .map_err(|e| format!("failed to parse SPIR-V shader: {e}"))
            }
        }
    }

    /// The error latch for the shader validation path (see
    /// `create_shader_module`/`create_and_validate_shader_module`).
    pub(crate) fn take_shader_validation_error(&self) -> Option<String> {
        self.shader_validation_error.lock().unwrap().take()
    }

    /// Creates a [`ShaderModule`] from either SPIR-V or WGSL source code.
    ///
    /// The source is compiled to a naga module (the Diligent per-stage
    /// shaders compile from it); compilation failures are latched for the
    /// pipeline cache's `load_module` (the wgpu error-scope replacement) and
    /// produce a module without a naga side, which degrades the diligent
    /// PSO creation gracefully.
    ///
    /// # Safety
    ///
    /// Creates a shader module with user-customizable runtime checks which allows shaders to
    /// perform operations which can lead to undefined behavior like indexing out of bounds,
    /// To avoid UB, ensure any unchecked shaders are sound!
    /// This method should never be called for user-supplied shaders.
    #[inline]
    pub unsafe fn create_shader_module(
        &self,
        desc: ShaderModuleDescriptor,
    ) -> ShaderModule {
        self.create_shader_module_inner(desc, true)
    }

    /// Creates and validates a [`ShaderModule`] from either SPIR-V or WGSL source code.
    ///
    /// See [`ValidateShader`](bevy_shader::ValidateShader) for more information on the tradeoffs involved with shader validation.
    #[inline]
    pub fn create_and_validate_shader_module(
        &self,
        desc: ShaderModuleDescriptor,
    ) -> ShaderModule {
        self.create_shader_module_inner(desc, true)
    }

    fn create_shader_module_inner(
        &self,
        desc: ShaderModuleDescriptor,
        latch_error: bool,
    ) -> ShaderModule {
        match Self::compile_shader_source(&desc.source) {
            Ok(module) => {
                if latch_error {
                    *self.shader_validation_error.lock().unwrap() = None;
                }
                ShaderModule {
                    id: crate::render_resource::wgpu_compat::ShaderModuleId::new(),
                    naga: Some(Arc::new(module)),
                }
            }
            Err(err) => {
                if latch_error {
                    *self.shader_validation_error.lock().unwrap() = Some(err);
                }
                ShaderModule {
                    id: crate::render_resource::wgpu_compat::ShaderModuleId::new(),
                    naga: None,
                }
            }
        }
    }

    /// Check for resource cleanups and mapping callbacks.
    ///
    /// M1b (brief point 9): advances the Diligent frame lifecycle via
    /// `FinishFrame` (the wgpu poll is gone - the diligent context is the
    /// only execution path).
    #[inline]
    pub fn poll(&self, _maintain: crate::render_resource::PollType) -> Result<PollStatus, PollError> {
        if let Some(context) = &self.diligent_context {
            let _guard = diligent_registry::context_guard();
            context.finish_frame();
        }
        Ok(PollStatus::Poll)
    }

    /// Creates an empty [`CommandEncoder`](crate::render_resource::CommandEncoder).
    #[inline]
    pub fn create_command_encoder(
        &self,
        _desc: &crate::render_resource::CommandEncoderDescriptor,
    ) -> crate::render_resource::CommandEncoder {
        crate::render_resource::CommandEncoder::new(
            self.clone(),
            self.diligent_context_handle(),
        )
    }

    /// Creates a new [`BindGroup`](crate::render_resource::BindGroup).
    ///
    /// M1b (brief point 6): creates a real Diligent
    /// [`ShaderResourceBinding`](diligent_rs::ShaderResourceBinding) from the
    /// layout's PRS and binds the mutable/static variables by their canonical
    /// `binding_{n}` names (deterministic per the M1-2 PRS design; immune to
    /// the per-stage variable table differences of mixed-visibility layouts).
    #[inline]
    pub fn create_bind_group<'a>(
        &self,
        _label: impl Into<crate::render_resource::wgpu_compat::Label<'a>>,
        layout: &'a BindGroupLayout,
        entries: &'a [BindGroupEntry<'a>],
    ) -> BindGroup {
        let diligent_state = Arc::new(BindGroupDiligentState::new());
        let value = self.create_diligent_bind_group(layout, entries, &diligent_state);
        BindGroup {
            id: crate::render_resource::BindGroupId::new(),
            inner: Arc::new(WgpuBindGroup {
                value,
                diligent_state,
                // M2a (§6.1.1): the layout's dynamic-offset bindings (the
                // `set_bind_group(.., &[u32])` offset array maps to these
                // SRB variables one-to-one, in binding order).
                dynamic_bindings: layout.dynamic_bindings().into(),
            }),
        }
    }

    /// Creates the Diligent SRB for a bind group and binds its variables.
    ///
    /// M1-3 review, fix 3: every binding failure is recorded in `state`
    /// (shared with the returned handle's `BindGroup`), so the SRB is never
    /// committed with unbound variables.
    fn create_diligent_bind_group(
        &self,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry<'_>],
        state: &BindGroupDiligentState,
    ) -> Option<DiligentHandle<diligent_rs::ShaderResourceBinding>> {
        let prs = layout.prs()?;
        let srb = match prs.create_shader_resource_binding(true) {
            Ok(srb) => srb,
            Err(err) => {
                bevy_log::warn!("diligent: SRB creation failed: {err}");
                return None;
            }
        };
        let srb = DiligentHandle::new(Arc::new(srb));
        let registry = diligent_registry::registry();
        for entry in entries {
            self.bind_srb_variable(&srb, registry, layout, entry, state);
        }
        Some(srb)
    }

    /// Binds one bind-group entry into the SRB.
    ///
    /// The variable is looked up by name (the canonical `binding_{n}` PRS
    /// resource names - deterministic per the M1-2 PRS design, and immune to
    /// the per-stage variable table differences that an index lookup breaks
    /// on mixed-visibility layouts). The stage used for the lookup is the
    /// entry's visibility; the per-stage variable lists are PRS-ordered and
    /// all managers share one resource cache, so a single stage's variable
    /// covers the resource everywhere (verified against
    /// ShaderResourceBindingBase.hpp:99).
    fn bind_srb_variable(
        &self,
        srb: &DiligentHandle<diligent_rs::ShaderResourceBinding>,
        registry: &diligent_registry::ResourceRegistry,
        layout: &BindGroupLayout,
        entry: &BindGroupEntry<'_>,
        state: &BindGroupDiligentState,
    ) {
        // M1-3 review, fix 3: every early return below records the failed
        // binding in the shared bind-group state, so
        // `set_bind_group`/`commit_shader_resources` skips the commit (the
        // SRB object still exists) instead of binding variables that are
        // unbound - garbage reads / engine errors.
        let resource = match &entry.resource {
            crate::render_resource::BindingResource::Buffer(binding) => {
                let Some(buffer) = registry.resolve_buffer(binding.buffer.id()) else {
                    bevy_log::debug!(
                        "diligent: no Diligent buffer for binding {} (unregistered buffer)",
                        entry.binding
                    );
                    state.record_failed_binding(entry.binding);
                    return;
                };
                let kind = match layout.buffer_binding_types.get(&entry.binding) {
                    Some(BufferBindingType::Uniform) => BufferBindKind::Uniform,
                    Some(BufferBindingType::Storage { read_only: true }) => {
                        BufferBindKind::StorageReadOnly
                    }
                    Some(BufferBindingType::Storage { read_only: false }) => {
                        BufferBindKind::StorageReadWrite
                    }
                    None => {
                        bevy_log::debug!(
                            "diligent: no buffer binding type for binding {}",
                            entry.binding
                        );
                        state.record_failed_binding(entry.binding);
                        return;
                    }
                };
                ResourceBinding::Buffer {
                    buffer,
                    offset: binding.offset,
                    size: binding.size.map(|s| s.get()),
                    kind,
                }
            }
            crate::render_resource::BindingResource::BufferArray(bindings) => {
                let kind = match layout.buffer_binding_types.get(&entry.binding) {
                    Some(BufferBindingType::Uniform) => BufferBindKind::Uniform,
                    Some(BufferBindingType::Storage { read_only: true }) => {
                        BufferBindKind::StorageReadOnly
                    }
                    Some(BufferBindingType::Storage { read_only: false }) => {
                        BufferBindKind::StorageReadWrite
                    }
                    None => {
                        bevy_log::debug!(
                            "diligent: no buffer binding type for binding array {}",
                            entry.binding
                        );
                        state.record_failed_binding(entry.binding);
                        return;
                    }
                };
                let mut resolved = Vec::with_capacity(bindings.len());
                for binding in bindings.iter() {
                    match registry.resolve_buffer(binding.buffer.id()) {
                        Some(buffer) => resolved.push(buffer),
                        None => {
                            bevy_log::debug!(
                                "diligent: no Diligent buffer for binding array {}",
                                entry.binding
                            );
                            state.record_failed_binding(entry.binding);
                            return;
                        }
                    }
                }
                if resolved.is_empty() {
                    state.record_failed_binding(entry.binding);
                    return;
                }
                ResourceBinding::BufferArray(resolved, kind)
            }
            crate::render_resource::BindingResource::TextureView(view) => {
                let Some(view) = registry.resolve_texture_view(view.id()) else {
                    bevy_log::debug!(
                        "diligent: no Diligent texture view for binding {}",
                        entry.binding
                    );
                    state.record_failed_binding(entry.binding);
                    return;
                };
                ResourceBinding::TextureView(view)
            }
            crate::render_resource::BindingResource::TextureViewArray(views) => {
                let mut resolved = Vec::with_capacity(views.len());
                for view in views.iter() {
                    match registry.resolve_texture_view(view.id()) {
                        Some(view) => resolved.push(view),
                        None => {
                            bevy_log::debug!(
                                "diligent: no Diligent texture view for binding array {}",
                                entry.binding
                            );
                            state.record_failed_binding(entry.binding);
                            return;
                        }
                    }
                }
                if resolved.is_empty() {
                    state.record_failed_binding(entry.binding);
                    return;
                }
                ResourceBinding::TextureViewArray(resolved)
            }
            crate::render_resource::BindingResource::Sampler(sampler) => {
                let Some(sampler) = registry.resolve_sampler(sampler.id()) else {
                    bevy_log::debug!(
                        "diligent: no Diligent sampler for binding {}",
                        entry.binding
                    );
                    state.record_failed_binding(entry.binding);
                    return;
                };
                ResourceBinding::Sampler(sampler)
            }
            crate::render_resource::BindingResource::SamplerArray(samplers) => {
                let mut resolved = Vec::with_capacity(samplers.len());
                for sampler in samplers.iter() {
                    match registry.resolve_sampler(sampler.id()) {
                        Some(sampler) => resolved.push(sampler),
                        None => {
                            bevy_log::debug!(
                                "diligent: no Diligent sampler for binding array {}",
                                entry.binding
                            );
                            state.record_failed_binding(entry.binding);
                            return;
                        }
                    }
                }
                if resolved.is_empty() {
                    state.record_failed_binding(entry.binding);
                    return;
                }
                ResourceBinding::SamplerArray(resolved)
            }
            crate::render_resource::BindingResource::AccelerationStructure(_)
            | crate::render_resource::BindingResource::AccelerationStructureArray(_) => {
                // TODO-REMOVE-M1-4: BLAS/TLAS SRB binding lands with the
                // solari RT port (M4a).
                bevy_log::debug!(
                    "diligent: acceleration structure binding {} not yet supported",
                    entry.binding
                );
                state.record_failed_binding(entry.binding);
                return;
            }
            // BindingResource is non_exhaustive (future variants).
            _ => {
                bevy_log::debug!(
                    "diligent: unsupported binding resource for binding {}",
                    entry.binding
                );
                state.record_failed_binding(entry.binding);
                return;
            }
        };
        let variable = match self.bind_srb_resource(
            srb,
            entry.binding,
            &resource,
            layout.srb_probe_stages(),
        ) {
            Ok(variable) => variable,
            Err(err) => {
                bevy_log::warn!(
                    "diligent: failed to bind SRB variable at index {}: {err}",
                    entry.binding
                );
                state.record_failed_binding(entry.binding);
                return;
            }
        };
        // M2a (§6.1.1): cache the resolved variable of the layout's
        // dynamic-offset bindings - the per-draw `SetBufferOffset` path
        // reuses the pointer (the engine documents that it never changes)
        // instead of re-resolving by name, which probes stages that are
        // invalid for the SRB's pipeline type (e.g. VERTEX/PIXEL on a
        // compute signature) and logs a warning per probe
        // (ShaderResourceBindingBase.hpp:185).
        if layout.dynamic_bindings.contains(&entry.binding) {
            state.cache_dynamic_variable(entry.binding, variable as usize);
        }
    }

    /// Binds one resolved binding resource into the SRB; returns the
    /// resolved `IShaderResourceVariable` (M2a: cached for the dynamic
    /// offset path).
    fn bind_srb_resource(
        &self,
        srb: &diligent_rs::ShaderResourceBinding,
        binding: u32,
        resource: &ResourceBinding,
        probe_stages: wgpu_types::ShaderStages,
    ) -> Result<*mut sys::IShaderResourceVariable, String> {
        // The SRB was created from the canonical PRS, whose resources are
        // named `binding_{n}` - resolve the variable by that deterministic
        // name (an index lookup breaks on mixed-visibility layouts, where
        // the per-stage variable tables only hold stage-intersecting
        // resources).
        let variable_name = CString::new(diligent_pso::canonical_prs_name(binding))
            .map_err(|e| format!("SRB variable name for binding {binding}: {e}"))?;
        let variable =
            srb_variable_by_name(srb, &variable_name, probe_stages)?;
        match resource {
            ResourceBinding::Buffer {
                buffer,
                offset,
                size,
                kind,
            } => match kind {
                BufferBindKind::Uniform => {
                    set_buffer(variable, *buffer, *offset, *size, binding)
                }
                // BUFFER_SRV / BUFFER_UAV: the engine's D3D12 variable cache
                // accepts *buffer views*, not raw buffers (CacheResourceView
                // QueryInterfaces the object for IID_BufferViewD3D12) - the
                // buffer's default SRV/UAV covers the whole buffer
                // (created by BufferBase::CreateDefaultViews for RAW-mode
                // buffers; the storage binding approximates the wgpu entry
                // offset/size with the whole-buffer view - the dynamic
                // offset path applies the per-draw offset on top).
                kind => {
                    let view = buffer_default_view(*buffer, *kind)?;
                    set_object(variable, view as *mut sys::IDeviceObject)
                }
            },
            ResourceBinding::BufferArray(buffers, kind) => {
                match kind {
                    BufferBindKind::Uniform => set_buffer_array(variable, buffers, binding),
                    // Storage arrays bind through each buffer's default
                    // SRV/UAV view (see `buffer_default_view`).
                    kind => {
                        let views: Vec<*mut sys::IDeviceObject> = buffers
                            .iter()
                            .map(|buffer| {
                                buffer_default_view(*buffer, *kind)
                                    .map(|view| view as *mut sys::IDeviceObject)
                            })
                            .collect::<Result<_, _>>()?;
                        set_object_array(variable, &views, binding)
                    }
                }
            }
            ResourceBinding::TextureView(view) => set_object(variable, *view as *mut sys::IDeviceObject),
            ResourceBinding::TextureViewArray(views) => {
                let objects: Vec<*mut sys::IDeviceObject> =
                    views.iter().map(|v| *v as *mut sys::IDeviceObject).collect();
                set_object_array(variable, &objects, binding)
            }
            ResourceBinding::Sampler(sampler) => {
                set_object(variable, *sampler as *mut sys::IDeviceObject)
            }
            ResourceBinding::SamplerArray(samplers) => {
                let objects: Vec<*mut sys::IDeviceObject> =
                    samplers.iter().map(|s| *s as *mut sys::IDeviceObject).collect();
                set_object_array(variable, &objects, binding)
            }
        }?;
        Ok(variable)
    }

    /// Creates a [`BindGroupLayout`](crate::render_resource::BindGroupLayout).
    ///
    /// M1b (brief §5.3.3-5 / point 6): additionally creates the SRB-side
    /// Diligent [`PipelineResourceSignature`](diligent_rs::PipelineResourceSignature)
    /// from the layout descriptor (canonical binding names; content-hash
    /// deduplication happens in `PipelineCache`).
    #[inline]
    pub fn create_bind_group_layout<'a>(
        &self,
        label: impl Into<crate::render_resource::wgpu_compat::Label<'a>>,
        entries: &'a [BindGroupLayoutEntry],
    ) -> BindGroupLayout {
        let label: crate::render_resource::wgpu_compat::Label<'a> = label.into();
        let value = match &self.diligent_device {
            Some(device) => {
                let descriptor = bevy_material::descriptor::BindGroupLayoutDescriptor {
                    label: alloc::borrow::Cow::Owned(
                        label.map_or_else(|| "bgl".to_string(), |l| l.to_string()),
                    ),
                    entries: entries.to_vec(),
                };
                match diligent_pso::create_canonical_prs(device, &descriptor) {
                    Ok(prs) => Some(prs),
                    Err(err) => {
                        bevy_log::warn!("diligent: bind group layout PRS: {err}");
                        None
                    }
                }
            }
            None => None,
        };
        let buffer_binding_types = entries
            .iter()
            .filter_map(|entry| match entry.ty {
                wgpu_types::BindingType::Buffer { ty, .. } => Some((entry.binding, ty)),
                _ => None,
            })
            .collect();
        // M2a (§6.1.1): the ascending dynamic-offset buffer bindings - the
        // mapping key for the `set_bind_group` offset array.
        let dynamic_bindings: Arc<[u32]> =
            diligent_mapping::dynamic_buffer_bindings(entries).into();
        // M2a: the `GetVariableByName` probe stages for the SRBs built from
        // this layout (pipeline-type-consistent stages only - invalid
        // probes make the engine log a warning per probe).
        let srb_probe_stages = diligent_mapping::srb_variable_probe_stages(entries);
        BindGroupLayout {
            id: crate::render_resource::BindGroupLayoutId::new(),
            value,
            buffer_binding_types,
            dynamic_bindings,
            srb_probe_stages,
        }
    }

    /// Creates a [`PipelineLayout`](crate::render_resource::PipelineLayout).
    ///
    /// M1-4b-2: the wgpu layout is gone - this creates the handle the
    /// descriptors reference; the Diligent PRS array is created by
    /// `PipelineCache`'s layout cache, which registers the
    /// [`PipelineLayoutRecord`](diligent_pso::PipelineLayoutRecord) under
    /// this handle's id.
    #[inline]
    pub fn create_pipeline_layout(
        &self,
        _desc: &crate::render_resource::PipelineLayoutDescriptor,
    ) -> crate::render_resource::PipelineLayout {
        crate::render_resource::PipelineLayout {
            id: crate::render_resource::wgpu_compat::PipelineLayoutId::new(),
        }
    }

    /// Creates a [`RenderPipeline`](crate::render_resource::RenderPipeline).
    ///
    /// M1b (brief point 7): the Diligent graphics PSO is created from the
    /// descriptor's shaders (compiled to HLSL/SPIR-V) and the layout record
    /// (PRS array + immediate size) registered by the pipeline cache.
    #[inline]
    pub fn create_render_pipeline(
        &self,
        desc: &RawRenderPipelineDescriptor,
    ) -> RenderPipeline {
        let (value, immediate) = self.create_diligent_render_pipeline(desc);
        RenderPipeline {
            id: crate::render_resource::RenderPipelineId::new(),
            value,
            immediate,
        }
    }

    fn create_diligent_render_pipeline(
        &self,
        desc: &RawRenderPipelineDescriptor,
    ) -> (
        Option<DiligentHandle<diligent_rs::PipelineState>>,
        Option<ImmediateSrb>,
    ) {
        let Some(vertex_record) = self.shader_record(desc.vertex.module) else {
            return (None, None);
        };
        let fragment_record = desc
            .fragment
            .as_ref()
            .and_then(|fragment| self.shader_record(fragment.module));
        let layout_record = desc.layout.and_then(|layout| self.layout_record(layout));
        let immediate = layout_record.as_deref().and_then(create_immediate_srb);
        let value = diligent_pso::create_graphics_pipeline(
            self,
            desc,
            &vertex_record,
            fragment_record.as_ref(),
            layout_record.as_deref(),
        );
        (value, immediate)
    }

    /// Creates a [`RenderPipeline`] whose Diligent PSO compiles
    /// asynchronously (M2a task 6, V20: dGPU async cold-start is 1.7-3.1x
    /// faster wall-clock than sync). The returned pipeline's PSO is polled
    /// by the pipeline-cache state machine via
    /// [`RenderPipeline::diligent_pso_ready`] before the pipeline is
    /// promoted to `Ok`; until then the pass degrades (draws skipped) like
    /// a missing PSO.
    #[inline]
    pub fn create_render_pipeline_diligent_async(
        &self,
        desc: &RawRenderPipelineDescriptor,
    ) -> RenderPipeline {
        let (value, immediate) = if let Some(vertex_record) = self.shader_record(desc.vertex.module)
        {
            let fragment_record = desc
                .fragment
                .as_ref()
                .and_then(|fragment| self.shader_record(fragment.module));
            let layout_record = desc.layout.and_then(|layout| self.layout_record(layout));
            let immediate = layout_record.as_deref().and_then(create_immediate_srb);
            let value = diligent_pso::create_graphics_pipeline_async(
                self,
                desc,
                &vertex_record,
                fragment_record.as_ref(),
                layout_record.as_deref(),
            );
            (value, immediate)
        } else {
            (None, None)
        };
        RenderPipeline {
            id: crate::render_resource::RenderPipelineId::new(),
            value,
            immediate,
        }
    }

    /// Creates a [`ComputePipeline`](crate::render_resource::ComputePipeline).
    #[inline]
    pub fn create_compute_pipeline(
        &self,
        desc: &crate::render_resource::RawComputePipelineDescriptor,
    ) -> ComputePipeline {
        let (value, immediate) = if let Some(module) = self.shader_record(desc.module) {
            let layout_record = desc.layout.and_then(|layout| self.layout_record(layout));
            let immediate = layout_record
                .as_deref()
                .and_then(create_immediate_srb);
            let value = diligent_pso::create_compute_pipeline(
                self,
                desc,
                &module,
                layout_record.as_deref(),
            );
            (value, immediate)
        } else {
            (None, None)
        };
        ComputePipeline {
            id: crate::render_resource::ComputePipelineId::new(),
            value,
            immediate,
        }
    }

    /// Creates a [`Buffer`](crate::render_resource::Buffer).
    ///
    /// M1b (brief point 3): maps the wgpu descriptor to a Diligent
    /// `BufferDesc` (BindFlags / USAGE / CPUAccessFlags from `BufferUsages`).
    pub fn create_buffer(&self, desc: &crate::render_resource::BufferDescriptor) -> Buffer {
        let value = self.create_diligent_buffer(desc, None);
        let id = crate::render_resource::BufferId::new();
        if let Some(buffer) = &value {
            diligent_registry::registry().register_buffer(id, buffer.as_raw());
        }
        Buffer {
            id,
            value,
            size: desc.size,
            usage: desc.usage,
            context_handle: self.diligent_context_handle(),
            mapped: default(),
            pending_readback: default(),
        }
    }

    /// Creates a [`Buffer`] and initializes it with the specified data.
    pub fn create_buffer_with_data(
        &self,
        desc: &crate::render_resource::util::BufferInitDescriptor,
    ) -> Buffer {
        let buffer_desc = crate::render_resource::BufferDescriptor {
            label: desc.label,
            size: desc.contents.len() as u64,
            usage: desc.usage,
            mapped_at_creation: false,
        };
        let value = self.create_diligent_buffer(&buffer_desc, Some(desc.contents));
        let id = crate::render_resource::BufferId::new();
        if let Some(buffer) = &value {
            diligent_registry::registry().register_buffer(id, buffer.as_raw());
        }
        Buffer {
            id,
            value,
            size: buffer_desc.size,
            usage: desc.usage,
            context_handle: self.diligent_context_handle(),
            mapped: default(),
            pending_readback: default(),
        }
    }

    /// Creates the Diligent buffer for a wgpu buffer descriptor (+ optional
    /// initial data).
    ///
    /// M1b (brief point 3): `BufferUsages` -> `BindFlags` / `USAGE` mapping
    /// via `diligent_mapping`. M1-4b-1: `MAP_READ`/`MAP_WRITE` buffers get
    /// `USAGE_STAGING` + the matching `CPUAccessFlags` so the readback paths
    /// can `MapBuffer(MAP_READ)` their staging side; everything else is
    /// `USAGE_DEFAULT` (updated via `UpdateBuffer`), or `USAGE_IMMUTABLE`
    /// when initial data is provided. The engine rejects CPU access flags on
    /// static/default buffers (BufferBase.cpp:95), so the access flags are
    /// only ever set on the staging usage.
    fn create_diligent_buffer(
        &self,
        desc: &crate::render_resource::BufferDescriptor,
        initial_data: Option<&[u8]>,
    ) -> Option<DiligentHandle<diligent_rs::Buffer>> {
        let device = self.diligent_device()?;
        let bind_flags = diligent_mapping::buffer_usage_to_bind_flags(desc.usage);
        let usage = if initial_data.is_some() {
            sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE
        } else {
            diligent_mapping::buffer_usage_to_usage(desc.usage)
        };
        let name = format!("bevy_{}", desc.label.map_or("buffer", |l| l));
        // M1-4b-1: the readback pool's COPY_DST|MAP_READ buffers (and the
        // diagnostic timestamp buffers) now create with USAGE_STAGING +
        // CPU_ACCESS_READ - the USAGE_DEFAULT + CPU_ACCESS_READ combination
        // the old code passed is rejected by the engine (BufferBase.cpp:95),
        // which silently killed every buffer readback. The staging usage
        // requires zero bind flags (BufferBase.cpp:105-106).
        let cpu_access = if usage == sys::_USAGE::USAGE_STAGING as sys::USAGE {
            diligent_mapping::buffer_usage_to_cpu_access(desc.usage)
        } else {
            0
        };
        match device.create_buffer(&name, desc.size, bind_flags, usage, cpu_access, initial_data) {
            Ok(buffer) => Some(DiligentHandle::new(Arc::new(buffer))),
            Err(err) => {
                bevy_log::warn!(
                    "diligent: buffer creation failed ({} bytes, usage {:?}): {err}",
                    desc.size,
                    desc.usage
                );
                None
            }
        }
    }

    /// Creates a new [`Texture`] and initializes it with the specified data.
    ///
    /// `desc` specifies the general format of the texture.
    /// `data` is the raw data.
    pub fn create_texture_with_data(
        &self,
        _render_queue: &RenderQueue,
        desc: &crate::render_resource::TextureDescriptor,
        _order: wgpu_types::TextureDataOrder,
        data: &[u8],
    ) -> Texture {
        // M1b (brief point 4): initial data is only representable for a
        // single-subresource texture (M1-1 wrapper contract - the wrapper
        // additionally rejects block-compressed formats); multi-mip / array
        // textures are created without data (upload via the context is M2a
        // work).
        let initial_data = if desc.mip_level_count == 1 && desc.size.depth_or_array_layers <= 1 {
            Some(data)
        } else {
            None
        };
        let value = self.create_diligent_texture(desc, initial_data);
        let id = crate::render_resource::TextureId::new();
        if let Some(texture) = &value {
            diligent_registry::registry().register_texture(id, texture.as_raw());
        }
        Texture {
            id,
            value,
            format: desc.format,
            size: desc.size,
            dimension: desc.dimension,
            mip_level_count: desc.mip_level_count,
            sample_count: desc.sample_count,
            usage: desc.usage,
            bind_flags: diligent_mapping::texture_usage_to_bind_flags(desc.usage),
        }
    }

    /// Creates a new [`Texture`](crate::render_resource::Texture).
    ///
    /// `desc` specifies the general format of the texture.
    pub fn create_texture(&self, desc: &crate::render_resource::TextureDescriptor) -> Texture {
        let value = self.create_diligent_texture(desc, None);
        let id = crate::render_resource::TextureId::new();
        if let Some(texture) = &value {
            diligent_registry::registry().register_texture(id, texture.as_raw());
        }
        Texture {
            id,
            value,
            format: desc.format,
            size: desc.size,
            dimension: desc.dimension,
            mip_level_count: desc.mip_level_count,
            sample_count: desc.sample_count,
            usage: desc.usage,
            bind_flags: diligent_mapping::texture_usage_to_bind_flags(desc.usage),
        }
    }

    /// Creates the Diligent texture for a wgpu texture descriptor.
    ///
    /// The M1-1 wrapper's `create_texture` carries the bind flags / usage
    /// and, since M1-4b-1, the `TextureDesc.SampleCount` (MSAA textures are
    /// created with the descriptor's sample count - the render-pass path
    /// derives the attachment sample count from the texture, see
    /// `diligent_draw`).
    fn create_diligent_texture(
        &self,
        desc: &crate::render_resource::TextureDescriptor,
        initial_data: Option<&[u8]>,
    ) -> Option<DiligentHandle<diligent_rs::Texture>> {
        let device = self.diligent_device()?;
        let format = match diligent_rs::format::to_diligent(desc.format) {
            Ok(format) => format,
            Err(err) => {
                bevy_log::warn!("diligent: texture format: {err}");
                return None;
            }
        };
        let mut bind_flags = diligent_mapping::texture_usage_to_bind_flags(desc.usage);
        // M2a-2: `RENDER_ATTACHMENT` maps to `BIND_RENDER_TARGET |
        // BIND_DEPTH_STENCIL` (usage-only table), but D3D12 rejects the
        // cross-kind flag: a COLOR texture with BIND_DEPTH_STENCIL and a
        // DEPTH texture with BIND_RENDER_TARGET both fail
        // CreateCommittedResource with E_INVALIDARG. This was the root
        // cause of every RENDER_ATTACHMENT texture failing on the diligent
        // path (and therefore of every render pass falling back to the
        // empty pass - "no diligent texture view registered"). The format
        // decides the attachment kind; strip the invalid counterpart.
        if desc.format.is_depth_stencil_format() {
            bind_flags &= !(sys::_BIND_FLAGS::BIND_RENDER_TARGET as sys::BIND_FLAGS);
        } else {
            bind_flags &= !(sys::_BIND_FLAGS::BIND_DEPTH_STENCIL as sys::BIND_FLAGS);
        }
        let (usage, mip_levels) = if initial_data.is_some() {
            (sys::_USAGE::USAGE_IMMUTABLE as sys::USAGE, 1)
        } else {
            (
                sys::_USAGE::USAGE_DEFAULT as sys::USAGE,
                desc.mip_level_count,
            )
        };
        let name = format!("bevy_{}", desc.label.map_or("texture", |l| l));
        let texture = match device.create_texture(
            &name,
            desc.size.width,
            desc.size.height,
            desc.size.depth_or_array_layers,
            mip_levels,
            format,
            bind_flags,
            usage,
            desc.sample_count,
            initial_data,
        ) {
            Ok(texture) => texture,
            Err(err) => {
                bevy_log::warn!("diligent: texture creation failed: {err}");
                return None;
            }
        };
        Some(DiligentHandle::new(Arc::new(texture)))
    }

    /// Creates a new [`Sampler`](crate::render_resource::Sampler).
    ///
    /// `desc` specifies the behavior of the sampler.
    pub fn create_sampler(&self, desc: &crate::render_resource::SamplerDescriptor) -> Sampler {
        let value = self.create_diligent_sampler(desc);
        let id = crate::render_resource::SamplerId::new();
        if let Some(sampler) = &value {
            diligent_registry::registry().register_sampler(id, sampler.as_raw());
        }
        Sampler {
            id,
            inner: Arc::new(WgpuSampler { id, value }),
        }
    }

    /// Creates the Diligent sampler for a wgpu sampler descriptor.
    fn create_diligent_sampler(
        &self,
        desc: &crate::render_resource::SamplerDescriptor,
    ) -> Option<DiligentHandle<diligent_rs::Sampler>> {
        let device = self.diligent_device()?;
        let comparison = desc.compare.is_some();
        let anisotropy = desc.anisotropy_clamp > 1;
        let mipmap_filter = match desc.mipmap_filter {
            wgpu_types::MipmapFilterMode::Nearest => wgpu_types::FilterMode::Nearest,
            wgpu_types::MipmapFilterMode::Linear => wgpu_types::FilterMode::Linear,
        };
        let sampler_desc = diligent_rs::desc::sampler(
            diligent_mapping::filter_type(desc.min_filter, comparison, anisotropy),
            diligent_mapping::filter_type(desc.mag_filter, comparison, anisotropy),
            diligent_mapping::filter_type(mipmap_filter, comparison, anisotropy),
            diligent_mapping::address_mode(desc.address_mode_u),
            diligent_mapping::address_mode(desc.address_mode_v),
            diligent_mapping::address_mode(desc.address_mode_w),
            desc.compare.map(diligent_mapping::comparison_function),
            desc.anisotropy_clamp as u32,
            desc.lod_min_clamp,
            desc.lod_max_clamp,
        );
        if desc.border_color.is_some() {
            // The locked SamplerDesc has no border color field; CLAMP_BORDER
            // without a color is the best the API offers (border colors are
            // rarely used by bevy).
            bevy_log::debug!("diligent: sampler border color is not representable");
        }
        let name = format!("bevy_{}", desc.label.map_or("sampler", |l| l));
        match device.create_sampler(&name, &sampler_desc) {
            Ok(sampler) => Some(DiligentHandle::new(Arc::new(sampler))),
            Err(err) => {
                bevy_log::warn!("diligent: sampler creation failed: {err}");
                None
            }
        }
    }

    /// Returns the wgpu-29-compatible device facade (the transition wgpu
    /// device is gone; this carries the diligent-derived features/limits and
    /// the BLAS/TLAS handle creation).
    pub fn wgpu_device(&self) -> &super::Device {
        &self.device
    }

    pub fn map_buffer(
        &self,
        buffer: &BufferSlice,
        map_mode: crate::render_resource::MapMode,
        callback: impl FnOnce(Result<(), BufferAsyncError>) + Send + 'static,
    ) {
        let result = self.map_buffer_inner(buffer, map_mode);
        callback(result);
    }

    fn map_buffer_inner(
        &self,
        buffer: &BufferSlice,
        map_mode: crate::render_resource::MapMode,
    ) -> Result<(), BufferAsyncError> {
        let Some(context) = self.diligent_context.as_deref() else {
            return Err(BufferAsyncError);
        };
        let Some(diligent) = buffer.buffer().diligent() else {
            return Err(BufferAsyncError);
        };
        let _guard = diligent_registry::context_guard();
        let map_type = match map_mode {
            crate::render_resource::MapMode::Read => sys::_MAP_TYPE::MAP_READ,
            crate::render_resource::MapMode::Write => sys::_MAP_TYPE::MAP_WRITE,
        } as sys::MAP_TYPE;
        let mapped = context
            .map_buffer(diligent, map_type, false)
            .map_err(|_| BufferAsyncError)?;
        let Some(mapped) = mapped else {
            return Err(BufferAsyncError);
        };
        let data = mapped.as_slice().to_vec();
        buffer.buffer().store_mapped(data);
        Ok(())
    }

    // Rounds up `row_bytes` to be a multiple of
    // [`wgpu_types::COPY_BYTES_PER_ROW_ALIGNMENT`].
    pub const fn align_copy_bytes_per_row(row_bytes: usize) -> usize {
        let align = wgpu_types::COPY_BYTES_PER_ROW_ALIGNMENT as usize;

        // If row_bytes is aligned calculate a value just under the next aligned value.
        // Otherwise calculate a value greater than the next aligned value.
        let over_aligned = row_bytes + align - 1;

        // Round the number *down* to the nearest aligned value.
        (over_aligned / align) * align
    }

    pub fn get_supported_read_only_binding_type(
        &self,
        buffers_per_shader_stage: u32,
    ) -> BufferBindingType {
        if self.limits().max_storage_buffers_per_shader_stage >= buffers_per_shader_stage {
            BufferBindingType::Storage { read_only: true }
        } else {
            BufferBindingType::Uniform
        }
    }
}

/// A resolved SRB binding resource (Diligent objects only).
enum ResourceBinding {
    Buffer {
        buffer: *mut sys::IBuffer,
        offset: u64,
        size: Option<u64>,
        /// The layout's buffer binding kind - drives the SRB binding branch
        /// (SetBufferRange for CONSTANT_BUFFER, the default SRV/UAV view for
        /// storage buffers).
        kind: BufferBindKind,
    },
    BufferArray(Vec<*mut sys::IBuffer>, BufferBindKind),
    TextureView(*mut sys::ITextureView),
    TextureViewArray(Vec<*mut sys::ITextureView>),
    Sampler(*mut sys::ISampler),
    SamplerArray(Vec<*mut sys::ISampler>),
}

/// The wgpu buffer binding kind of a layout entry (captured at
/// `create_bind_group_layout`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferBindKind {
    /// `BufferBindingType::Uniform` -> Diligent CONSTANT_BUFFER (bound via
    /// `SetBufferRange`; the engine only allows ranges for constant
    /// buffers - ShaderResourceVariableBase.hpp:743).
    Uniform,
    /// `BufferBindingType::Storage { read_only: true }` -> BUFFER_SRV (bound
    /// via the buffer's default shader-resource view).
    StorageReadOnly,
    /// `BufferBindingType::Storage { read_only: false }` -> BUFFER_UAV
    /// (bound via the buffer's default unordered-access view).
    StorageReadWrite,
}

/// The `IBuffer::GetDefaultView(BUFFER_VIEW_SHADER_RESOURCE /
/// BUFFER_VIEW_UNORDERED_ACCESS)` of a storage buffer.
///
/// M2a binding model: the engine's D3D12 SRB cache accepts **buffer views**
/// for BUFFER_SRV/BUFFER_UAV variables (`CacheResourceView`
/// QueryInterfaces the bound object for `IID_BufferViewD3D12` - a raw
/// `IBuffer` silently binds nothing on Release builds). The default views
/// are created by `BufferBase::CreateDefaultViews` for RAW/STRUCTURED-mode
/// buffers created with `BIND_SHADER_RESOURCE` / `BIND_UNORDERED_ACCESS`.
fn buffer_default_view(
    buffer: *mut sys::IBuffer,
    kind: BufferBindKind,
) -> Result<*mut sys::IBufferView, String> {
    let view_type = match kind {
        BufferBindKind::StorageReadOnly => sys::_BUFFER_VIEW_TYPE::BUFFER_VIEW_SHADER_RESOURCE,
        BufferBindKind::StorageReadWrite => {
            sys::_BUFFER_VIEW_TYPE::BUFFER_VIEW_UNORDERED_ACCESS
        }
        BufferBindKind::Uniform => unreachable!("uniform buffers bind via SetBufferRange"),
    } as sys::BUFFER_VIEW_TYPE;
    let get = unsafe {
        (*(*buffer).pVtbl)
            .Buffer
            .GetDefaultView
            .as_ref()
            .ok_or("IBuffer::GetDefaultView missing")?
    };
    // Safety: `buffer` is alive (the registry keeps the owning wrapper alive
    // for the duration of the call); the returned view is owned by the
    // buffer (no refcount increment) and stays valid with it.
    let view = unsafe { get(buffer, view_type) };
    if view.is_null() {
        Err(format!(
            "no default view of type {view_type} for storage buffer \
             (bind flags / buffer mode mismatch)"
        ))
    } else {
        Ok(view)
    }
}

/// `IShaderResourceBinding::GetVariableByName` with the canonical
/// `binding_{n}` name (the SRB-side PRS resources are named deterministically
/// per binding - see [`diligent_pso::canonical_prs_name`]). Name-based lookup
/// is immune to the per-stage variable table differences that an index
/// lookup breaks on mixed-visibility layouts.
///
/// `probe_stages` restricts the probes to the stages that are consistent
/// with the signature's pipeline type (derived from the BGL entries'
/// visibility - see [`diligent_mapping::srb_variable_probe_stages`]):
/// probing an invalid stage makes the engine log a warning per probe
/// (ShaderResourceBindingBase.hpp:185). The variable for a binding lives in
/// one of its visible stages' managers, so the restricted probe set is
/// complete.
fn srb_variable_by_name(
    srb: &diligent_rs::ShaderResourceBinding,
    name: &CStr,
    probe_stages: wgpu_types::ShaderStages,
) -> Result<*mut sys::IShaderResourceVariable, String> {
    let get = unsafe {
        (*(*srb.as_raw()).pVtbl)
            .ShaderResourceBinding
            .GetVariableByName
            .as_ref()
            .ok_or("IShaderResourceBinding::GetVariableByName missing")?
    };
    let stages = [
        (
            wgpu_types::ShaderStages::VERTEX,
            sys::_SHADER_TYPE::SHADER_TYPE_VERTEX as sys::SHADER_TYPE,
        ),
        (
            wgpu_types::ShaderStages::FRAGMENT,
            sys::_SHADER_TYPE::SHADER_TYPE_PIXEL as sys::SHADER_TYPE,
        ),
        (
            wgpu_types::ShaderStages::COMPUTE,
            sys::_SHADER_TYPE::SHADER_TYPE_COMPUTE as sys::SHADER_TYPE,
        ),
    ];
    for (wgpu_stage, diligent_stage) in stages {
        if !probe_stages.contains(wgpu_stage) {
            continue;
        }
        let variable = unsafe { get(srb.as_raw(), diligent_stage, name.as_ptr()) };
        if !variable.is_null() {
            return Ok(variable);
        }
    }
    Err(format!("no SRB variable named {}", name.to_string_lossy()))
}

/// `IShaderResourceVariable::Set` (the generic resource bind; the engine
/// runs the runtime type checks - a buffer is accepted for buffer variables).
fn set_object(
    variable: *mut sys::IShaderResourceVariable,
    object: *mut sys::IDeviceObject,
) -> Result<(), String> {
    let set = unsafe {
        (*(*variable).pVtbl)
            .ShaderResourceVariable
            .Set
            .as_ref()
            .ok_or("IShaderResourceVariable::Set missing")?
    };
    // Safety: `variable` is alive and `object` is a live engine object of a
    // compatible type (validated by the engine at runtime).
    unsafe { set(variable, object, 0) };
    Ok(())
}

/// `IShaderResourceVariable::SetArray` for arrays of engine objects.
fn set_object_array(
    variable: *mut sys::IShaderResourceVariable,
    objects: &[*mut sys::IDeviceObject],
    _binding: u32,
) -> Result<(), String> {
    let set = unsafe {
        (*(*variable).pVtbl)
            .ShaderResourceVariable
            .SetArray
            .as_ref()
            .ok_or("IShaderResourceVariable::SetArray missing")?
    };
    // Safety: `variable` is alive and `objects` is valid for the duration of
    // the call.
    unsafe {
        set(
            variable,
            objects.as_ptr(),
            0,
            objects.len() as u32,
            0,
        )
    };
    Ok(())
}

/// Binds one constant buffer into an SRB variable via `SetBufferRange`
/// (offset/size semantics; only legal for constant buffers -
/// `ShaderResourceVariableBase.hpp:743`). Size 0 = the whole buffer from
/// Offset (ShaderVariableManagerD3D12.cpp:354) - the exact wgpu
/// `Option<BufferSize>` semantics. Storage buffers bind through their
/// default SRV/UAV views instead (`buffer_default_view`).
///
/// M2a: dynamic uniform buffers get their per-draw offset through
/// `SetBufferOffset` (applied at `set_bind_group` time, on top of the base
/// range - the §6.1.1 high-frequency path).
fn set_buffer(
    variable: *mut sys::IShaderResourceVariable,
    buffer: *mut sys::IBuffer,
    offset: u64,
    size: Option<u64>,
    _binding: u32,
) -> Result<(), String> {
    let vtbl = unsafe { &(*(*variable).pVtbl).ShaderResourceVariable };
    let set_range = vtbl
        .SetBufferRange
        .as_ref()
        .ok_or("IShaderResourceVariable::SetBufferRange missing")?;
    // Safety: `variable` is alive, `buffer` is a live constant buffer and
    // the range fits the buffer (validated by the engine).
    unsafe {
        set_range(
            variable,
            buffer.cast(),
            offset,
            size.unwrap_or(0),
            0,
            0,
        )
    };
    Ok(())
}

/// `IShaderResourceVariable::SetArray` for arrays of buffers.
fn set_buffer_array(
    variable: *mut sys::IShaderResourceVariable,
    buffers: &[*mut sys::IBuffer],
    _binding: u32,
) -> Result<(), String> {
    let objects: Vec<*mut sys::IDeviceObject> =
        buffers.iter().map(|b| *b as *mut sys::IDeviceObject).collect();
    set_object_array(variable, &objects, _binding)
}

/// The immediate-constants binding of a pipeline: the dedicated immediate
/// SRB plus its resolved `IShaderResourceVariable` and capacity (the PRS
/// `ArraySize` in DWORDs - the `SetInlineConstants` bounds check).
///
/// M2a (§6.1.1): created per pipeline from the layout record's immediate
/// PRS; `set_immediates` writes `SetInlineConstants` into it and commits it
/// (V3 mapping: `FirstConstant = offset / 4`).
#[derive(Clone)]
pub(crate) struct ImmediateSrb {
    pub(crate) srb: DiligentHandle<diligent_rs::ShaderResourceBinding>,
    /// The resolved variable of the shader's `Immediate` global (the
    /// `SetInlineConstants` target). Resolved once at pipeline creation -
    /// the engine documents that the pointer never changes, so the
    /// per-call name lookup is skipped.
    pub(crate) variable: usize,
    pub(crate) array_size_dwords: u32,
}

/// Creates the immediate SRB for a layout record, when it has one (M2a).
fn create_immediate_srb(
    record: &diligent_pso::PipelineLayoutRecord,
) -> Option<ImmediateSrb> {
    let (prs, name) = record.immediate_prs.as_ref().zip(record.immediate_name.as_ref())?;
    let srb = match prs.create_shader_resource_binding(true) {
        Ok(srb) => DiligentHandle::new(Arc::new(srb)),
        Err(err) => {
            bevy_log::warn!("diligent: immediate SRB creation failed: {err}");
            return None;
        }
    };
    let variable = match srb_variable_by_name(
        &srb,
        name,
        // The immediate PRS's stages are the shaders' entry-point stages
        // (VS|PS for graphics pipelines, CS for compute) - probing only
        // those keeps the lookup silent on both pipeline types.
        record.immediate_probe_stages,
    ) {
        Ok(variable) => variable as usize,
        Err(err) => {
            bevy_log::warn!(
                "diligent: immediate SRB variable '{}': {err}",
                name.to_string_lossy()
            );
            return None;
        }
    };
    Some(ImmediateSrb {
        srb,
        variable,
        array_size_dwords: record.immediate_size.div_ceil(4),
    })
}

/// Pure validation of a `set_bind_group` offset array against the layout's
/// dynamic buffer bindings (wgpu's own contract: the array maps one-to-one,
/// in ascending binding order), plus the SetBufferOffset alignment contract
/// (M2a-1 review, fix 3: every offset must be a multiple of the device's
/// `ConstantBufferOffsetAlignment` - `GraphicsAdapterInfo.Buffer`, the
/// header rule for `IShaderResourceVariable::SetBufferOffset`).
fn validate_dynamic_offsets(
    dynamic_bindings: &[u32],
    offsets: &[u32],
    alignment: u32,
) -> Result<(), String> {
    if dynamic_bindings.len() != offsets.len() {
        return Err(format!(
            "dynamic offset count mismatch: the layout declares {} dynamic \
             buffer bindings but {} offsets were provided",
            dynamic_bindings.len(),
            offsets.len()
        ));
    }
    for &offset in offsets {
        if alignment == 0 || offset % alignment != 0 {
            return Err(format!(
                "dynamic offset {offset} is not a multiple of the constant \
                 buffer offset alignment ({alignment})"
            ));
        }
    }
    Ok(())
}

/// The device's `ConstantBufferOffsetAlignment`
/// (`GraphicsAdapterInfo.Buffer.ConstantBufferOffsetAlignment`, a.k.a. the
/// header rule for `SetBufferOffset`): latched once by
/// `initialize_renderer`, read by `apply_dynamic_offsets` on the draw path.
static CONSTANT_BUFFER_OFFSET_ALIGNMENT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// Records the device's constant buffer offset alignment (called by
/// `initialize_renderer`; the first call wins - it never changes per
/// device).
pub(crate) fn latch_constant_buffer_offset_alignment(alignment: u32) {
    let _ = CONSTANT_BUFFER_OFFSET_ALIGNMENT.set(alignment);
}

/// The latched constant buffer offset alignment. The 256-byte fallback
/// covers the device-less path (engine init failed - no pass can run, so
/// the value is never read in practice) and matches the D3D12 constant
/// buffer alignment / wgpu's default
/// `min_uniform_buffer_offset_alignment`.
fn constant_buffer_offset_alignment() -> u32 {
    *CONSTANT_BUFFER_OFFSET_ALIGNMENT.get().unwrap_or(&256)
}

/// Applies the per-draw dynamic offsets of a `set_bind_group` call to the
/// SRB (M2a §6.1.1 core semantic): the offset array maps to the layout's
/// dynamic buffer bindings one-to-one, in ascending binding order, and each
/// gets `IShaderResourceVariable::SetBufferOffset(offset, 0)`.
///
/// The variables come from the per-bind-group cache (`state`): they are
/// resolved once at bind-group creation (their pointers never change per
/// the engine docs), so the per-draw path makes no name lookups - a fresh
/// lookup probes stages that are invalid for the SRB's pipeline type and
/// logs a warning per probe on compute signatures
/// (ShaderResourceBindingBase.hpp:185).
///
/// The offsets are added to the variables' base bindings **at the next
/// commit** (ShaderResourceCacheD3D12 `BufferDynamicOffset` consumed by
/// CommitRootViews), so the caller must run this before
/// `CommitShaderResources` - the callers hold the immediate-context lock
/// across both (see `TrackedRenderPass::set_bind_group`).
///
/// No SRB re-commit is triggered per variable: the SRB is committed exactly
/// once by the caller, with all offsets applied.
pub(crate) fn apply_dynamic_offsets(
    state: &BindGroupDiligentState,
    dynamic_bindings: &[u32],
    offsets: &[u32],
) -> Result<(), String> {
    validate_dynamic_offsets(dynamic_bindings, offsets, constant_buffer_offset_alignment())?;
    for (&binding, &offset) in dynamic_bindings.iter().zip(offsets) {
        let variable = state
            .dynamic_variable(binding)
            .map(|v| v as *mut sys::IShaderResourceVariable)
            .ok_or_else(|| format!("no cached SRB variable for dynamic binding {binding}"))?;
        set_buffer_offset(variable, offset, binding)?;
    }
    Ok(())
}

/// `IShaderResourceVariable::SetBufferOffset(Uint32 Offset, Uint32 ArrayIndex)`
///
/// Offset unit = bytes (verified against the header, GraphicsTypesX.hpp:
/// `SetBufferOffset(Uint32 Offset, Uint32 ArrayIndex = 0)`; the engine
/// adds it to the base binding at commit time). ArrayIndex is always 0 here:
/// wgpu dynamic offsets apply to single bindings, not array elements.
fn set_buffer_offset(
    variable: *mut sys::IShaderResourceVariable,
    offset: u32,
    _binding: u32,
) -> Result<(), String> {
    let set = unsafe {
        (*(*variable).pVtbl)
            .ShaderResourceVariable
            .SetBufferOffset
            .as_ref()
            .ok_or("IShaderResourceVariable::SetBufferOffset missing")?
    };
    // Safety: `variable` is alive; the offset range is validated by the
    // engine against the bound buffer.
    unsafe { set(variable, offset, 0) };
    Ok(())
}

/// `IShaderResourceVariable::SetInlineConstants` on the immediate SRB
/// (M2a; V3 mapping: `FirstConstant = offset / 4`, `NumConstants =
/// data.len() / 4`). The caller commits the SRB afterwards (the constants
/// are uploaded from the SRB cache at commit / on the next draw).
///
/// `variable` is the immediate variable resolved once at pipeline creation
/// (its pointer never changes per the engine docs).
pub(crate) fn set_inline_constants(
    variable: usize,
    data: &[u8],
    first_constant: u32,
    num_constants: u32,
) -> Result<(), String> {
    let variable = variable as *mut sys::IShaderResourceVariable;
    let set = unsafe {
        (*(*variable).pVtbl)
            .ShaderResourceVariable
            .SetInlineConstants
            .as_ref()
            .ok_or("IShaderResourceVariable::SetInlineConstants missing")?
    };
    // Safety: `variable` is alive and `data` is a valid DWORD array alive
    // for the duration of the call (the engine copies synchronously).
    unsafe {
        set(
            variable,
            data.as_ptr().cast(),
            first_constant,
            num_constants,
        )
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_resource::{BufferId, PipelineLayout, ShaderModule};

    #[test]
    fn align_copy_bytes_per_row() {
        assert_eq!(RenderDevice::align_copy_bytes_per_row(0), 0);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(1), 256);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(255), 256);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(256), 256);
        assert_eq!(RenderDevice::align_copy_bytes_per_row(257), 512);
    }

    /// The buffer registry is keyed by the atomic `BufferId`: the register
    /// and lookup sides derive the same key from the id (stable across
    /// clones - the M1-4b-2 replacement for the address-keyed registry).
    #[test]
    fn buffer_registry_key_is_the_buffer_id() {
        let id = BufferId::new();
        let ptr = 0x1 as *mut sys::IBuffer;
        let registry = diligent_registry::ResourceRegistry::default();
        registry.register_buffer(id, ptr);
        assert_eq!(registry.resolve_buffer(id), Some(ptr));
        // A clone of the id resolves the same entry.
        let id_clone = id;
        assert_eq!(registry.resolve_buffer(id_clone), Some(ptr));
    }

    /// The pipeline layout registry is keyed by the layout id (the
    /// descriptor reference `desc.layout` carries the same key).
    #[test]
    fn pipeline_layout_registry_key_is_the_layout_id() {
        let layout = PipelineLayout {
            id: crate::render_resource::wgpu_compat::PipelineLayoutId::new(),
        };
        let clone = layout.clone();
        assert_eq!(layout.registry_key(), clone.registry_key());
        // Distinct layouts have distinct keys.
        let other = PipelineLayout {
            id: crate::render_resource::wgpu_compat::PipelineLayoutId::new(),
        };
        assert_ne!(layout.registry_key(), other.registry_key());
    }

    /// The shader module registry is keyed by the module id (the descriptor
    /// reference `desc.vertex.module` carries the same key).
    #[test]
    fn shader_registry_key_is_the_module_id() {
        let module = ShaderModule {
            id: crate::render_resource::wgpu_compat::ShaderModuleId::new(),
            naga: None,
        };
        let clone = module.clone();
        assert_eq!(module.registry_key(), clone.registry_key());
        let other = ShaderModule {
            id: crate::render_resource::wgpu_compat::ShaderModuleId::new(),
            naga: None,
        };
        assert_ne!(module.registry_key(), other.registry_key());
    }

    /// The registry round-trips the resource id through the consumer-facing
    /// accessors (the id a `BindGroupEntry` carries resolves the same entry
    /// that was registered).
    #[test]
    fn resource_registry_round_trips_via_consumer_reference() {
        let id = BufferId::new();
        let ptr = 0x2 as *mut sys::IBuffer;
        let registry = diligent_registry::ResourceRegistry::default();
        registry.register_buffer(id, ptr);
        assert_eq!(registry.resolve_buffer(id), Some(ptr));
        assert_eq!(registry.resolve_buffer(BufferId::new()), None);
    }

    /// M2a (§6.1.1): the `set_bind_group` offset array must map one-to-one
    /// onto the layout's dynamic buffer bindings (a layout without dynamic
    /// bindings pairs with an empty array; a count mismatch is an error the
    /// pass poisons on).
    #[test]
    fn dynamic_offset_count_mismatch_is_rejected() {
        // The view-group shape: 3 dynamic bindings (0/1/12) need exactly 3
        // offsets, in binding order. Alignment = 256 (the latched D3D12
        // constant buffer offset alignment), so the offsets are all
        // 256-multiples here - the count semantics stay isolated from the
        // alignment semantics.
        let bindings = [0u32, 1, 12];
        assert!(validate_dynamic_offsets(&bindings, &[256, 512, 768], 256).is_ok());
        // The empty layout pairs with the empty array (bevy sets the group
        // without offsets).
        assert!(validate_dynamic_offsets(&[], &[], 256).is_ok());
        // Count mismatches are rejected in both directions.
        assert!(validate_dynamic_offsets(&bindings, &[256, 512], 256).is_err());
        assert!(validate_dynamic_offsets(&bindings, &[256, 512, 768, 1024], 256).is_err());
        assert!(validate_dynamic_offsets(&bindings, &[], 256).is_err());
        assert!(validate_dynamic_offsets(&[], &[256], 256).is_err());
    }

    /// M2a-1 review, fix 3: every `SetBufferOffset` offset must be a
    /// multiple of the device's `ConstantBufferOffsetAlignment` (the header
    /// rule; `GraphicsAdapterInfo.Buffer.ConstantBufferOffsetAlignment`).
    #[test]
    fn dynamic_offset_misalignment_is_rejected() {
        let bindings = [0u32, 1, 12];
        // Zero and coarser multiples are fine.
        assert!(validate_dynamic_offsets(&bindings, &[0, 0, 0], 256).is_ok());
        assert!(validate_dynamic_offsets(&bindings, &[512, 1024, 1536], 256).is_ok());
        // A single misaligned offset fails the whole array (fail-fast
        // before any SetBufferOffset engine call).
        assert!(validate_dynamic_offsets(&bindings, &[256, 520, 768], 256).is_err());
        assert!(validate_dynamic_offsets(&bindings, &[257, 512, 768], 256).is_err());
        // The alignment is a device property: a coarser device alignment
        // rejects the same array; zero alignment is safely rejected without panic.
        assert!(validate_dynamic_offsets(&bindings, &[256, 512, 768], 1024).is_err());
        assert!(validate_dynamic_offsets(&bindings, &[256, 512, 768], 0).is_err());
        // The alignment latch feeds the validator through
        // `apply_dynamic_offsets`.
        latch_constant_buffer_offset_alignment(256);
        assert_eq!(constant_buffer_offset_alignment(), 256);
    }
}
