//! Prepare step for the M6a SDF software GI: uploads the scene SDF and
//! creates the irradiance-cache storage buffers the compute passes read and
//! write (tasks.md 18.1/18.3).

use super::extract::SdfGiSceneResources;
use bevy_ecs::{
    prelude::*,
    system::{Commands, Res, ResMut},
};
use bevy_render::{
    render_resource::{Buffer, BufferDescriptor, BufferInitDescriptor, BufferUsages},
    renderer::{RenderDevice, RenderQueue},
};

/// GPU-side buffers for the SDF GI compute passes (task 18.1/18.3).
#[derive(Resource, Default)]
pub struct SdfGiSceneBuffers {
    /// Storage buffer with the SDF distance samples (`voxelize`/`ray_march`).
    /// `None` until the scene SDF is non-empty.
    pub sdf_samples: Option<Buffer>,
    /// Storage texture/read-write buffer for the irradiance cache (task 18.3).

    pub irradiance: Option<Buffer>,
    /// The grid metadata the shaders need (origin / size / cell size).
    pub grid_info: Option<Buffer>,
    /// SDF AO output buffer (18.2), one `f32` per probe/pixel.
    pub ao_output: Option<Buffer>,
    /// SDF soft-shadow output buffer (18.2), one `f32` per probe/pixel.
    pub shadow_output: Option<Buffer>,
    /// Probe grid dimensions (width, height) for the ray-march dispatch.
    pub probe_size: Option<Buffer>,
}

/// The default probe resolution (width, height) used when no view-derived
/// resolution is available yet. Kept small: the AO/soft-shadow pass runs at
/// probe resolution, the full-res composite comes in 18.3.
pub const DEFAULT_PROBE_SIZE: [u32; 2] = [64, 64];

/// Rebuilds the GPU buffers when the scene SDF changed (task 18.1 prepare).
pub fn prepare_sdf_gi_resources(
    mut commands: Commands,
    scene: Res<SdfGiSceneResources>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    buffers: Option<ResMut<SdfGiSceneBuffers>>,
) {
    let Some(sdf) = &scene.scene_sdf else {
        return;
    };
    let Some(mut buffers) = buffers else {
        commands.init_resource::<SdfGiSceneBuffers>();
        return;
    };

    // Upload the distance samples (mirrors the voxelize.wgsl output layout).
    let bytes = bytemuck::cast_slice(&sdf.samples);
    match &buffers.sdf_samples {
        Some(_) => render_queue.write_buffer(
            buffers.sdf_samples.as_ref().expect("matched above"),
            0,
            bytes,
        ),
        None => {
            buffers.sdf_samples = Some(render_device.create_buffer_with_data(
                &BufferInitDescriptor {
                    label: Some("sdf_gi_samples"),
                    contents: bytes,
                    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                },
            ));
        }
    }

    // Grid metadata (origin/size/cell) as a Vec4 triple.
    let meta: [[f32; 4]; 3] = [
        [sdf.origin.x, sdf.origin.y, sdf.origin.z, 0.0],
        [
            sdf.size[0] as f32,
            sdf.size[1] as f32,
            sdf.size[2] as f32,
            0.0,
        ],
        [sdf.cell_size, 0.0, 0.0, 0.0],
    ];
    let meta_bytes = bytemuck::cast_slice(&meta);
    match &buffers.grid_info {
        Some(_) => render_queue.write_buffer(
            buffers.grid_info.as_ref().expect("matched above"),
            0,
            meta_bytes,
        ),
        None => {
            buffers.grid_info = Some(render_device.create_buffer_with_data(
                &BufferInitDescriptor {
                    label: Some("sdf_gi_grid_info"),
                    contents: meta_bytes,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                },
            ));
        }
    }

    // 18.2: ray-march output buffers (one f32 per probe) + probe dims.
    let probe_count = (DEFAULT_PROBE_SIZE[0] as usize) * (DEFAULT_PROBE_SIZE[1] as usize);
    if buffers.ao_output.is_none() {
        buffers.ao_output = Some(render_device.create_buffer(
            &BufferDescriptor {
                label: Some("sdf_gi_ao_output"),
                size: (probe_count * 4) as u64,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            },
        ));
    }
    if buffers.shadow_output.is_none() {
        buffers.shadow_output = Some(render_device.create_buffer(
            &BufferDescriptor {
                label: Some("sdf_gi_shadow_output"),
                size: (probe_count * 4) as u64,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            },
        ));
    }
    if buffers.probe_size.is_none() {
        // xy = probe dims, z = reset flag (0), w unused. Shared by
        // `ray_march.wgsl` and `irradiance.wgsl`.
        let probe_info: [u32; 4] = [DEFAULT_PROBE_SIZE[0], DEFAULT_PROBE_SIZE[1], 0, 0];
        buffers.probe_size = Some(render_device.create_buffer_with_data(
            &BufferInitDescriptor {
                label: Some("sdf_gi_probe_size"),
                contents: bytemuck::cast_slice(&probe_info),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            },
        ));
    }
    // 18.3: irradiance cache (one vec4 per probe: RGB irradiance + alpha 1).
    if buffers.irradiance.is_none() {
        buffers.irradiance = Some(render_device.create_buffer(
            &BufferDescriptor {
                label: Some("sdf_gi_irradiance"),
                size: (probe_count * 16) as u64,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        ));
    }
}
