//! Class-Lumen-style software raytraced GI (M6a, tasks.md 18).
//!
//! SDF voxelization + software ray marching + SDF AO/soft shadows +
//! single-bounce GI with a last-frame irradiance cache. Pure compute + WGSL,
//! consistent across D3D12/Vulkan/GLES, no hardware RT required (spec §5.10,
//! construction §10).
//!
//! Milestone structure:
//! - **18.1 scene SDF** ([`sdf`] CPU reference + `voxelize.wgsl` compute
//!   pass): dynamic objects re-voxelize a local region; static geometry uses
//!   the precomputed field.
//! - **18.2 ray marching** (`ray_march.wgsl`): per-pixel/probe sphere
//!   tracing; hit distance → SDF AO + soft shadows (works on no-RT devices).
//! - **18.3 single-bounce GI** (`irradiance.wgsl`): one-bounce sampling +
//!   last-frame irradiance cache (low resolution + temporal/spatial filter).
//!
//! This crate ships the CPU reference of the math (`sdf.rs`, unit-tested
//! without a GPU) and the compute-pipeline skeleton. The GPU passes are
//! wired into the render schedule when a device with compute support is
//! present; full visual validation happens on real hardware (Android no-RT
//! devices are first-class, spec §5.10.4).

use bevy_app::{App, Plugin};
use bevy_camera::Hdr;
use bevy_core_pipeline::{
    core_3d::main_opaque_pass_3d,
    prepass::{DeferredPrepass, DepthPrepass, MotionVectorPrepass},
    schedule::{Core3d, Core3dSystems},
};
use bevy_ecs::{component::Component, reflect::ReflectComponent, schedule::IntoScheduleConfigs};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    renderer::RenderDevice, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
};
use tracing::warn;

mod extract;
mod node;
mod prepare;
pub mod sdf;

use extract::extract_sdf_gi;
use node::{init_sdf_gi_pipelines, sdf_gi_compute};
use prepare::prepare_sdf_gi_resources;

/// SDF software GI plugin (M6a-core: AO / soft shadows / single-bounce GI).
///
/// Requires compute support (any D3D12/Vulkan/GLES device); explicitly does
/// **not** require hardware ray tracing. Add this plugin and put
/// [`SdfGi`] on a camera to enable it.
pub struct SdfGiPlugin;

impl Plugin for SdfGiPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        let render_app = app.sub_app_mut(RenderApp);
        let render_device = render_app.world().resource::<RenderDevice>();
        if !render_device
            .features()
            .contains(crate::SolariPlugins::required_wgpu_features())
        {
            warn!(
                "SdfGiPlugin not loaded. GPU lacks support for required features."
            );
            return;
        }

        render_app
            .init_resource::<prepare::SdfGiSceneBuffers>()
            .add_systems(RenderStartup, init_sdf_gi_pipelines)
            .add_systems(ExtractSchedule, extract_sdf_gi)
            .add_systems(
                Render,
                prepare_sdf_gi_resources.in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                sdf_gi_compute
                    .after(main_opaque_pass_3d)
                    .in_set(Core3dSystems::PostProcess),
            );
    }
}

/// Camera component to enable SDF software GI (M6a).
///
/// Must be used with `CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)`
/// and `Msaa::Off` (the GI composite writes into a storage texture).
#[derive(Component, Reflect, Clone)]
#[reflect(Component, Default, Clone)]
#[require(Hdr, DeferredPrepass, DepthPrepass, MotionVectorPrepass)]
pub struct SdfGi {
    /// Set to true to reset the irradiance cache (temporal history).
    ///
    /// Useful after sudden camera cuts / scene changes. Automatically
    /// cleared at the end of the frame.
    pub reset: bool,
}

impl Default for SdfGi {
    fn default() -> Self {
        Self {
            reset: true, // No temporal history on the first frame
        }
    }
}