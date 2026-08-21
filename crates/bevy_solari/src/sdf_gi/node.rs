//! Node step for the M6a SDF software GI: pipeline registration
//! (`init_sdf_gi_pipelines`) and the per-frame compute dispatches
//! (`sdf_gi_compute`). Task 18.2 wires the ray-march pass (SDF AO + soft
//! shadows); the voxelize (18.1) and single-bounce GI + irradiance blend
//! (18.3) passes share the same bind-group layout.

use super::prepare::{SdfGiSceneBuffers, DEFAULT_PROBE_SIZE};
use bevy_asset::{load_embedded_asset, AssetServer, Handle};
use bevy_ecs::prelude::*;
use bevy_render::{
    render_resource::{
        binding_types::{storage_buffer_sized, uniform_buffer},
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
        CachedComputePipelineId, ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache,
        ShaderStages,
    },
    renderer::{RenderContext, RenderDevice},
};
use bevy_shader::Shader;
use bevy_utils::default;

/// Resource holding the SDF GI pipeline configuration (task 18.1-18.3).
#[derive(Resource)]
pub struct SdfGiPipelines {
    /// Bind group layout shared by the voxelize / ray-march / irradiance
    /// compute passes.

    bind_group_layout: BindGroupLayoutDescriptor,
    /// Voxelize compute pipeline (18.1).
    #[expect(dead_code, reason = "consumed by the 18.1 voxelize dispatch")]
    voxelize_pipeline: CachedComputePipelineId,
    /// Ray-march compute pipeline: SDF AO + soft shadows (18.2).
    ray_march_pipeline: CachedComputePipelineId,
    /// Single-bounce GI + irradiance blend (18.3).

    irradiance_pipeline: CachedComputePipelineId,
}

/// Startup system: loads the WGSL assets and registers the compute
/// pipelines (tasks.md 18.1-18.3 pipeline init).
pub fn init_sdf_gi_pipelines(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: ResMut<PipelineCache>,
) {
    // The layout mirrors `ray_march.wgsl` @group(0) bindings 0-5.
    let bind_group_layout = BindGroupLayoutDescriptor::new(
        "sdf_gi_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                // 0: SDF sample storage buffer (read).
                storage_buffer_sized(false, None),
                // 1: grid metadata uniform (origin/size/cell).
                uniform_buffer::<[f32; 4]>(false),
                // 2: irradiance cache storage buffer (read-write, 18.3).
                storage_buffer_sized(true, None),
                // 3: SDF AO output (read-write).
                storage_buffer_sized(true, None),
                // 4: soft-shadow output (read-write).
                storage_buffer_sized(true, None),
                // 5: probe grid dims uniform (ray_march.wgsl `probe_size`).
                uniform_buffer::<[u32; 2]>(false),
            ),
        ),
    );

    let create_pipeline = |label: &'static str,
                           entry_point: &'static str,
                           shader: Handle<Shader>| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![bind_group_layout.clone()],
            immediate_size: 8,
            shader,
            shader_defs: vec![],
            entry_point: Some(entry_point.into()),
            ..default()
        })
    };

    let voxelize_pipeline = create_pipeline(
        "sdf_gi_voxelize",
        "main",
        load_embedded_asset!(asset_server.as_ref(), "voxelize.wgsl"),
    );
    let ray_march_pipeline = create_pipeline(
        "sdf_gi_ray_march",
        "main",
        load_embedded_asset!(asset_server.as_ref(), "ray_march.wgsl"),
    );
    let irradiance_pipeline = create_pipeline(
        "sdf_gi_irradiance",
        "main",
        load_embedded_asset!(asset_server.as_ref(), "irradiance.wgsl"),
    );

    commands.insert_resource(SdfGiPipelines {
        bind_group_layout,
        voxelize_pipeline,
        ray_march_pipeline,
        irradiance_pipeline,
    });
}

/// Per-frame SDF GI compute dispatch (tasks.md 18.2: ray marching → SDF AO +
/// soft shadows; 18.3: single-bounce GI + irradiance blend).
///
/// Runs at probe resolution ([`DEFAULT_PROBE_SIZE`]); the full-resolution
/// composite is produced by the 18.3 pass. Guarded: every pipeline/buffer
/// must be present, otherwise the pass no-ops (graceful degradation on
/// devices where the module was not fully initialized).
pub fn sdf_gi_compute(
    pipelines: Option<Res<SdfGiPipelines>>,
    pipeline_cache: Res<PipelineCache>,
    buffers: Option<Res<SdfGiSceneBuffers>>,
    render_device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let Some(pipelines) = pipelines else {
        return;
    };
    let Some(buffers) = buffers else {
        return;
    };
    let (
        Some(ray_march_pipeline),
        Some(irradiance_pipeline),
        Some(sdf_samples),
        Some(grid_info),
        Some(ao_output),
        Some(shadow_output),
        Some(irradiance),
        Some(probe_size),
    ) = (
        pipeline_cache.get_compute_pipeline(pipelines.ray_march_pipeline),
        pipeline_cache.get_compute_pipeline(pipelines.irradiance_pipeline),
        buffers.sdf_samples.as_ref(),
        buffers.grid_info.as_ref(),
        buffers.ao_output.as_ref(),
        buffers.shadow_output.as_ref(),
        buffers.irradiance.as_ref(),
        buffers.probe_size.as_ref(),
    )
    else {
        return;
    };

    let bind_group = render_device.create_bind_group(
        "sdf_gi_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipelines.bind_group_layout),
        &BindGroupEntries::sequential((
            sdf_samples.as_entire_binding(),
            grid_info.as_entire_binding(),
            irradiance.as_entire_binding(),
            ao_output.as_entire_binding(),
            shadow_output.as_entire_binding(),
            probe_size.as_entire_binding(),
        )),
    );

    let dx = DEFAULT_PROBE_SIZE[0].div_ceil(8);
    let dy = DEFAULT_PROBE_SIZE[1].div_ceil(8);

    let command_encoder = ctx.command_encoder();
    let mut pass = command_encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("sdf_gi_ray_march"),
        timestamp_writes: None,
    });
    pass.set_bind_group(0, &bind_group, &[]);
    pass.set_pipeline(ray_march_pipeline);
    pass.dispatch_workgroups(dx, dy, 1);
    // 18.3: single-bounce GI + irradiance temporal/spatial blend, at the
    // same probe resolution (reads the AO/shadow outputs as input).
    pass.set_pipeline(irradiance_pipeline);
    pass.dispatch_workgroups(dx, dy, 1);
    drop(pass);
}
