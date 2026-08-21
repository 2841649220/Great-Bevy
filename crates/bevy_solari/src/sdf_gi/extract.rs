//! Extraction for the M6a SDF software GI: gathers the scene's static
//! geometry (for the precomputed SDF) and the dynamic-mesh set (for local
//! re-voxelization) into the render-world resource the prepare/node passes
//! consume.
//!
//! M6a-core (task 18.1) keeps the CPU-side `SceneSdf` reference available
//! here so the compute pipeline can be exercised headlessly (unit tests) and
//! validated against the shader output on real hardware.

use super::sdf::SceneSdf;
use bevy_ecs::{prelude::*, system::Commands};

/// Render-world resource carrying the SDF scene data (task 18.1).
///
/// `scene_sdf` is the precomputed distance field for static geometry. The
/// compute voxelizer (`voxelize.wgsl`) will produce the same field on the
/// GPU; this CPU reference is the validation twin.
#[derive(Resource, Default)]
pub struct SdfGiSceneResources {
    /// Precomputed scene SDF (static geometry).
    pub scene_sdf: Option<SceneSdf>,
}

/// Extracts the SDF scene resources from the main world into the render
/// world (task 18.1 extract step).
pub fn extract_sdf_gi(mut commands: Commands) {
    // Placeholder: static-geometry SDF extraction (mesh → primitives/grid)
    // lands with the first real scene integration. The resource stays
    // available so the compute skeleton compiles and runs empty.
    commands.insert_resource(SdfGiSceneResources::default());
}
