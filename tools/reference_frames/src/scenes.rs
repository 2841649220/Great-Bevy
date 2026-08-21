//! Scene registry: re-creations of selected `examples/3d/` scenes, driven with
//! fixed camera rigs and no user input, so captures are reproducible.
//!
//! Implemented scenes are procedural (no asset files), built through public
//! Bevy APIs. They are simplified static versions of the originals — the
//! composition and lighting match the reference examples' core look; camera
//! controllers, temporal effects (TAA) and animation are removed for
//! determinism. Scene-by-scene deviations are listed in
//! `tests/reference-frames/README.md`.

use bevy::{
    camera::Hdr,
    core_pipeline::tonemapping::Tonemapping,
    math::Vec3,
    pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel},
    post_process::bloom::Bloom,
    prelude::*,
};

/// Fixed camera rig for a scene (position + look-at target).
#[derive(Clone, Copy, Debug)]
pub struct CameraRig {
    pub position: Vec3,
    pub target: Vec3,
}

impl CameraRig {
    pub const fn new(position: Vec3, target: Vec3) -> Self {
        Self { position, target }
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position).looking_at(self.target, Vec3::Y)
    }
}

pub type SceneSetup = fn(&mut App, CameraRig);

pub struct SceneSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub implemented: bool,
    pub needs_assets: bool,
    pub required_features: &'static [&'static str],
    pub camera: CameraRig,
    pub setup: Option<SceneSetup>,
    pub notes: &'static str,
}

pub const SCENES: &[SceneSpec] = &[
    SceneSpec {
        id: "3d_scene",
        title: "3D Scene (basic shapes + lighting)",
        implemented: true,
        needs_assets: false,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(-2.5, 4.5, 9.0), Vec3::ZERO),
        setup: Some(setup_3d_scene),
        notes: "circle base + blue cube + shadow-casting point light; camera matches examples/3d/3d_scene.rs",
    },
    SceneSpec {
        id: "pbr",
        title: "Physically Based Rendering (metallic/roughness grid)",
        implemented: true,
        needs_assets: false,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(4.0, 3.0, 6.0), Vec3::new(0.0, 0.5, 0.0)),
        setup: Some(setup_pbr),
        notes: "4x4 metallic x roughness sphere grid + plane + dir/point lights (static re-creation of examples/3d/pbr.rs)",
    },
    SceneSpec {
        id: "lighting",
        title: "Lighting (dir/point/spot)",
        implemented: true,
        needs_assets: false,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(0.0, 3.0, 8.0), Vec3::new(0.0, 0.5, 0.0)),
        setup: Some(setup_lighting),
        notes: "plane + spheres under directional, colored point and spot lights (static re-creation of examples/3d/lighting.rs)",
    },
    SceneSpec {
        id: "bloom_3d",
        title: "3D Bloom (HDR emissive spheres)",
        implemented: true,
        needs_assets: false,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(-2.0, 2.5, 5.0), Vec3::ZERO),
        setup: Some(setup_bloom_3d),
        notes: "emissive sphere field + Bloom::NATURAL; camera matches examples/3d/bloom_3d.rs; bounce animation removed (static)",
    },
    SceneSpec {
        id: "ssao",
        title: "Screen Space Ambient Occlusion",
        implemented: true,
        needs_assets: false,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(-2.0, 2.0, -2.0), Vec3::ZERO),
        setup: Some(setup_ssao),
        notes: "matches examples/3d/ssao.rs; TAA removed (temporal, not deterministic); Hdr + Msaa::Off + ScreenSpaceAmbientOcclusion",
    },
    SceneSpec {
        id: "lightmaps",
        title: "Lightmaps",
        implemented: false,
        needs_assets: true,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(-2.5, 4.5, 9.0), Vec3::ZERO),
        setup: None,
        notes: "follow-up: re-create with baked lightmaps from assets/ (glTF scene); not in first round",
    },
    SceneSpec {
        id: "irradiance_volumes",
        title: "Irradiance Volumes",
        implemented: false,
        needs_assets: true,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(4.0, 3.0, 6.0), Vec3::ZERO),
        setup: None,
        notes: "follow-up: needs irradiance volume asset pipeline; not in first round",
    },
    SceneSpec {
        id: "deferred_rendering",
        title: "Deferred Rendering",
        implemented: false,
        needs_assets: true,
        required_features: &[],
        camera: CameraRig::new(Vec3::new(-2.5, 4.5, 9.0), Vec3::ZERO),
        setup: None,
        notes: "follow-up: re-create deferred pipeline showcase; not in first round",
    },
    SceneSpec {
        id: "meshlet",
        title: "Meshlet (dense high-poly)",
        implemented: false,
        needs_assets: true,
        required_features: &["meshlet", "https", "free_camera"],
        camera: CameraRig::new(Vec3::new(10.0, 10.0, 10.0), Vec3::ZERO),
        setup: None,
        notes: "follow-up: requires root features meshlet/https/free_camera + downloaded meshes; pressure tier (see large_scenes: bistro/caldera_hotel)",
    },
];

pub fn get(id: &str) -> Option<&'static SceneSpec> {
    SCENES.iter().find(|spec| spec.id == id)
}

/// examples/3d/3d_scene.rs re-creation (camera fixed, no input).
fn setup_3d_scene(app: &mut App, rig: CameraRig) {
    app.add_systems(
        Startup,
        move |mut commands: Commands,
              mut meshes: ResMut<Assets<Mesh>>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            commands.spawn((
                Mesh3d(meshes.add(Circle::new(4.0))),
                MeshMaterial3d(materials.add(Color::WHITE)),
                Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            ));
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
                MeshMaterial3d(materials.add(Color::srgb_u8(124, 144, 255))),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            commands.spawn((
                PointLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(4.0, 8.0, 4.0),
            ));
            commands.spawn((Camera3d::default(), rig.transform()));
        },
    );
}

/// Static re-creation of the core of examples/3d/pbr.rs: a metallic x
/// roughness sphere grid on a reflective plane under directional + point light.
fn setup_pbr(app: &mut App, rig: CameraRig) {
    app.add_systems(
        Startup,
        move |mut commands: Commands,
              mut meshes: ResMut<Assets<Mesh>>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            let plane = materials.add(StandardMaterial {
                base_color: Color::srgb(0.1, 0.1, 0.12),
                perceptual_roughness: 0.1,
                ..default()
            });
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(10.0)))),
                MeshMaterial3d(plane),
            ));

            let sphere_mesh = meshes.add(Sphere::new(0.5).mesh().uv(72, 36));
            for (i, metallic) in [0.0f32, 0.3, 0.6, 0.95].into_iter().enumerate() {
                for (j, roughness) in [0.05f32, 0.2, 0.5, 0.9].into_iter().enumerate() {
                    let mat = materials.add(StandardMaterial {
                        base_color: Color::srgb(0.6, 0.35, 0.25),
                        metallic,
                        perceptual_roughness: roughness,
                        ..default()
                    });
                    commands.spawn((
                        Mesh3d(sphere_mesh.clone()),
                        MeshMaterial3d(mat),
                        Transform::from_xyz(
                            (i as f32 - 1.5) * 1.8,
                            0.5,
                            (j as f32 - 1.5) * 1.8,
                        ),
                    ));
                }
            }

            commands.spawn((
                DirectionalLight {
                    illuminance: 20_000.0,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(
                    EulerRot::ZYX,
                    0.0,
                    -1.0,
                    -0.6,
                )),
            ));
            commands.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.6, 0.3),
                    intensity: 10_000_000.0,
                    range: 30.0,
                    ..default()
                },
                Transform::from_xyz(2.0, 4.0, 1.0),
            ));

            commands.spawn((Camera3d::default(), rig.transform()));
        },
    );
}

/// Static re-creation of the core of examples/3d/lighting.rs.
fn setup_lighting(app: &mut App, rig: CameraRig) {
    app.add_systems(
        Startup,
        move |mut commands: Commands,
              mut meshes: ResMut<Assets<Mesh>>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            let plane = materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.2, 0.22),
                ..default()
            });
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(8.0)))),
                MeshMaterial3d(plane),
            ));

            let sphere = meshes.add(Sphere::new(0.6).mesh().uv(72, 36));
            let white = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                ..default()
            });
            commands.spawn((
                Mesh3d(sphere.clone()),
                MeshMaterial3d(white.clone()),
                Transform::from_xyz(-1.5, 0.6, 0.0),
            ));
            commands.spawn((
                Mesh3d(sphere.clone()),
                MeshMaterial3d(white.clone()),
                Transform::from_xyz(1.5, 0.6, 0.0),
            ));
            commands.spawn((
                Mesh3d(sphere),
                MeshMaterial3d(white),
                Transform::from_xyz(0.0, 0.6, -1.0),
            ));

            commands.spawn((
                DirectionalLight {
                    illuminance: 10_000.0,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, -1.0, -0.5)),
            ));
            commands.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.2, 0.2),
                    intensity: 5_000_000.0,
                    range: 20.0,
                    ..default()
                },
                Transform::from_xyz(-2.0, 3.0, 2.0),
            ));
            commands.spawn((
                PointLight {
                    color: Color::srgb(0.2, 0.4, 1.0),
                    intensity: 5_000_000.0,
                    range: 20.0,
                    ..default()
                },
                Transform::from_xyz(2.0, 3.0, 2.0),
            ));
            commands.spawn((
                SpotLight {
                    color: Color::srgb(0.8, 0.8, 0.2),
                    intensity: 5_000_000.0,
                    range: 25.0,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_xyz(0.0, 5.0, -2.0)
                    .looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
            ));

            commands.spawn((Camera3d::default(), rig.transform()));
        },
    );
}

/// examples/3d/bloom_3d.rs re-creation: deterministic emissive sphere field
/// under `Bloom::NATURAL`; bounce animation removed for determinism.
fn setup_bloom_3d(app: &mut App, rig: CameraRig) {
    app.add_systems(
        Startup,
        move |mut commands: Commands,
              mut meshes: ResMut<Assets<Mesh>>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            commands.spawn((
                Camera3d::default(),
                Camera {
                    clear_color: ClearColorConfig::Custom(Color::BLACK),
                    ..default()
                },
                Tonemapping::TonyMcMapface,
                rig.transform(),
                Bloom::NATURAL,
            ));

            let material_emissive1 = materials.add(StandardMaterial {
                emissive: LinearRgba::rgb(0.0, 0.0, 150.0),
                ..default()
            });
            let material_emissive2 = materials.add(StandardMaterial {
                emissive: LinearRgba::rgb(1000.0, 1000.0, 1000.0),
                ..default()
            });
            let material_emissive3 = materials.add(StandardMaterial {
                emissive: LinearRgba::rgb(50.0, 0.0, 0.0),
                ..default()
            });
            let material_non_emissive = materials.add(StandardMaterial {
                base_color: Color::BLACK,
                ..default()
            });

            let mesh = meshes.add(Sphere::new(0.4).mesh().ico(5).unwrap());
            use std::hash::{Hash, Hasher};
            for x in -5..5 {
                for z in -5..5 {
                    // Same deterministic hash-seed pattern as the original
                    // example (DefaultHasher over (x, z)).
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    (x, z).hash(&mut hasher);
                    let rand = (hasher.finish() + 3) % 6;
                    let (material, scale) = match rand {
                        0 => (material_emissive1.clone(), 0.5),
                        1 => (material_emissive2.clone(), 0.1),
                        2 => (material_emissive3.clone(), 1.0),
                        3..=5 => (material_non_emissive.clone(), 1.5),
                        _ => unreachable!(),
                    };
                    commands.spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material),
                        Transform::from_xyz(x as f32 * 1.5, 0.0, z as f32 * 1.5)
                            .with_scale(Vec3::splat(scale)),
                    ));
                }
            }
        },
    );
}

/// examples/3d/ssao.rs re-creation. TAA is deliberately removed (temporal
/// jitter breaks determinism); SSAO + Hdr + Msaa::Off match the original.
fn setup_ssao(app: &mut App, rig: CameraRig) {
    app.insert_resource(GlobalAmbientLight {
        brightness: 1000.0,
        ..default()
    });
    app.add_systems(
        Startup,
        move |mut commands: Commands,
              mut meshes: ResMut<Assets<Mesh>>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            commands.spawn((
                Camera3d::default(),
                rig.transform(),
                Hdr,
                Msaa::Off,
                ScreenSpaceAmbientOcclusion {
                    quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Low,
                    ..default()
                },
            ));

            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                ..default()
            });
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::default())),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(0.0, 0.0, 1.0),
            ));
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::default())),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(0.0, -1.0, 0.0),
            ));
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::default())),
                MeshMaterial3d(material),
                Transform::from_xyz(1.0, 0.0, 0.0),
            ));
            commands.spawn((
                Mesh3d(meshes.add(Sphere::new(0.4).mesh().uv(72, 36))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.4, 0.4, 0.4),
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })),
            ));

            commands.spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(
                    EulerRot::ZYX,
                    0.0,
                    std::f32::consts::PI * -0.15,
                    std::f32::consts::PI * -0.15,
                )),
            ));
        },
    );
}
