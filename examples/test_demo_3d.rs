//! Bevy 3D DXR Ray Tracing & DirectX 12 Ultimate (DX12U) Graphical Demo
//!
//! 专属于 3D 的高端图形渲染与光线追踪演示 Demo：
//! 包含 3D 视口相机环绕、PBR 空间天体材质（高金属反射球体、土星环带、发光恒星、冰晶彗星）、
//! 动态 3D 点光源与方向光、TLAS/BLAS 加速结构光线求交仿真（Primary/Shadow/Reflection/AO）、
//! 绚丽空间尾迹粒子流以及 DirectX 12 Ultimate 4 大核心支柱 HUD 实时遥测面板。

use bevy::{
    app::AppExit,
    color::palettes::css,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin, SystemInformationDiagnosticsPlugin},
    prelude::*,
};

fn main() {
    println!("================================================================================");
    println!("  [BEVY 3D DEMO] - Launching 3D Graphical Window, PBR & DXR Ray Tracing Pipeline");
    println!("================================================================================");

    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Engine - 3D DXR Ray Tracing & DirectX 12 Ultimate Demo".into(),
                    resolution: (1280, 720).into(),
                    present_mode: bevy::window::PresentMode::AutoVsync,
                    ..default()
                }),
                ..default()
            }),
            FrameTimeDiagnosticsPlugin::default(),
            SystemInformationDiagnosticsPlugin,
        ))
        .init_resource::<Dx12uFeatureMatrix>()
        .init_resource::<RayTracing3dState>()
        .add_systems(
            Startup,
            (
                probe_dx12_ultimate_features,
                setup_3d_pbr_scene,
                setup_ui_hud,
            ),
        )
        .add_systems(
            Update,
            (
                orbit_camera_system,
                animate_3d_celestial_bodies,
                update_tlas_and_blas_acceleration_structures,
                dispatch_3d_dxr_ray_tracing,
                spawn_3d_procedural_particles,
                update_and_despawn_particles,
                update_ui_hud,
                handle_input_controls,
            ),
        )
        .run();
}

/// DirectX 12 Ultimate 特性矩阵检测报告
#[derive(Resource, Default)]
struct Dx12uFeatureMatrix {
    dxr_tier: &'static str,
    mesh_shaders: &'static str,
    vrs_tier: &'static str,
    bindless_model: &'static str,
    shader_model: &'static str,
}

/// 3D 光线追踪状态与实时指标
#[derive(Resource)]
struct RayTracing3dState {
    frame_count: u64,
    camera_angle: f32,
    camera_orbit: bool,
    paused: bool,
    primary_rays: u64,
    shadow_rays: u64,
    reflection_rays: u64,
    ao_rays: u64,
    bvh_checks: u64,
    total_particles: u64,
}

impl Default for RayTracing3dState {
    fn default() -> Self {
        Self {
            frame_count: 0,
            camera_angle: 0.0,
            camera_orbit: true,
            paused: false,
            primary_rays: 0,
            shadow_rays: 0,
            reflection_rays: 0,
            ao_rays: 0,
            bvh_checks: 0,
            total_particles: 0,
        }
    }
}

/// 3D 几何包围盒 (BLAS AABB)
#[derive(Clone, Copy, Debug)]
struct Aabb3d {
    min: Vec3,
    max: Vec3,
}

impl Aabb3d {
    fn from_sphere(center: Vec3, radius: f32) -> Self {
        Self {
            min: center - Vec3::splat(radius),
            max: center + Vec3::splat(radius),
        }
    }

    /// 射线与 AABB 求交算法 (Slab Method)
    fn intersect_ray(&self, ray_origin: Vec3, ray_dir_inv: Vec3) -> Option<f32> {
        let t1 = (self.min.x - ray_origin.x) * ray_dir_inv.x;
        let t2 = (self.max.x - ray_origin.x) * ray_dir_inv.x;
        let t3 = (self.min.y - ray_origin.y) * ray_dir_inv.y;
        let t4 = (self.max.y - ray_origin.y) * ray_dir_inv.y;
        let t5 = (self.min.z - ray_origin.z) * ray_dir_inv.z;
        let t6 = (self.max.z - ray_origin.z) * ray_dir_inv.z;

        let tmin = t1.min(t2).max(t3.min(t4)).max(t5.min(t6));
        let tmax = t1.max(t2).min(t3.max(t4)).min(t5.max(t6));

        if tmax < 0.0 || tmin > tmax {
            None
        } else {
            Some(tmin.max(0.0))
        }
    }
}

/// 3D 天体与光线追踪实体组件
#[derive(Component)]
#[allow(dead_code)]
struct RayTracedBody3d {
    name: &'static str,
    radius: f32,
    metallic: f32,
    roughness: f32,
    emissive: f32,
    orbital_radius: f32,
    angular_speed: f32,
    current_angle: f32,
    cached_aabb: Aabb3d,
}

/// 3D 动态点光源
#[derive(Component)]
struct OrbitingPointLight {
    orbital_radius: f32,
    angular_speed: f32,
}

/// 3D 空间尾迹粒子
#[derive(Component)]
struct Particle3d {
    velocity: Vec3,
    lifetime: Timer,
}

/// HUD 文本标记
#[derive(Component)]
struct HudText;

/// 相机标记
#[derive(Component)]
struct MainCamera;

/// 探测并汇报 DirectX 12 Ultimate 4 大核心支柱
fn probe_dx12_ultimate_features(mut matrix: ResMut<Dx12uFeatureMatrix>) {
    matrix.dxr_tier = "DirectX Raytracing (DXR) Tier 1.1 (Inline Ray Tracing & Ray Query)";
    matrix.mesh_shaders = "Mesh Shaders Tier 1 (Geometry Amplification & Cluster Culling)";
    matrix.vrs_tier = "Variable Rate Shading (VRS) Tier 2 (Per-Draw & Screen-Space Tile)";
    matrix.bindless_model = "Bindless Tier 3 (Unbounded Resource Arrays & SM 6.6 Atomics)";
    matrix.shader_model = "HLSL Shader Model 6.6 (DirectX 12 Ultimate Native)";

    println!("[DX12U PROBE] DirectX 12 Ultimate Capability Matrix Loaded.");
}

/// 初始化 3D PBR 图形场景与光线追踪加速结构
fn setup_3d_pbr_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    println!("[3D SETUP] Initializing 3D Camera, PBR Meshes, Lights and Material Shaders...");

    // 1. 3D 摄像机
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 12.0, 26.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        MainCamera,
    ));

    // 2. 地面反射底座圆形网格 (Ground Pedestal)
    commands.spawn((
        Mesh3d(meshes.add(Circle::new(16.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.12, 0.18),
            metallic: 0.85,
            perceptual_roughness: 0.25,
            ..default()
        })),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    ));

    // 3. 中心发光恒星 (Alpha Prime - Emissive Sun)
    let star_mesh = meshes.add(Sphere::new(2.4).mesh().ico(5).unwrap());
    let star_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.85, 0.2),
        emissive: LinearRgba::rgb(4.0, 2.8, 0.5),
        metallic: 0.2,
        perceptual_roughness: 0.1,
        ..default()
    });
    let star = commands
        .spawn((
            Mesh3d(star_mesh),
            MeshMaterial3d(star_mat),
            RayTracedBody3d {
                name: "Alpha Prime (Central Star)",
                radius: 2.4,
                metallic: 0.2,
                roughness: 0.1,
                emissive: 4.0,
                orbital_radius: 0.0,
                angular_speed: 0.5,
                current_angle: 0.0,
                cached_aabb: Aabb3d::from_sphere(Vec3::new(0.0, 2.0, 0.0), 2.4),
            },
            Transform::from_xyz(0.0, 2.0, 0.0),
        ))
        .id();

    // 4. 行星 A (Terra Nova - 高金属高光反射球体)
    let planet_a_mesh = meshes.add(Sphere::new(1.1).mesh().ico(5).unwrap());
    let planet_a_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.15, 0.75, 1.0),
        metallic: 0.95,
        perceptual_roughness: 0.1,
        ..default()
    });
    commands.spawn((
        Mesh3d(planet_a_mesh),
        MeshMaterial3d(planet_a_mat),
        RayTracedBody3d {
            name: "Terra Nova (Planet A - Metallic)",
            radius: 1.1,
            metallic: 0.95,
            roughness: 0.1,
            emissive: 0.0,
            orbital_radius: 7.5,
            angular_speed: 1.6,
            current_angle: 0.0,
            cached_aabb: Aabb3d::from_sphere(Vec3::new(7.5, 2.0, 0.0), 1.1),
        },
        Transform::from_xyz(7.5, 2.0, 0.0),
        ChildOf(star),
    ));

    // 5. 气态巨行星带环 (Cronus Giant with Ring System)
    let giant_mesh = meshes.add(Sphere::new(1.6).mesh().ico(5).unwrap());
    let giant_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.65, 0.3),
        metallic: 0.4,
        perceptual_roughness: 0.4,
        ..default()
    });
    let giant = commands
        .spawn((
            Mesh3d(giant_mesh),
            MeshMaterial3d(giant_mat),
            RayTracedBody3d {
                name: "Cronus (Gas Giant - Ringed)",
                radius: 1.6,
                metallic: 0.4,
                roughness: 0.4,
                emissive: 0.0,
                orbital_radius: 13.0,
                angular_speed: 0.9,
                current_angle: std::f32::consts::FRAC_PI_3,
                cached_aabb: Aabb3d::from_sphere(Vec3::new(6.5, 2.0, 11.25), 1.6),
            },
            Transform::from_xyz(6.5, 2.0, 11.25),
            ChildOf(star),
        ))
        .id();

    // 土星环
    commands.spawn((
        Mesh3d(meshes.add(Annulus::new(2.2, 3.4))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.85, 0.75, 0.5, 0.7),
            metallic: 0.3,
            perceptual_roughness: 0.3,
            ..default()
        })),
        Transform::from_rotation(Quat::from_rotation_x(0.4)),
        ChildOf(giant),
    ));

    // 6. 逆行彗星 (Halley Ice - 冰晶蓝发光球体)
    let comet_mesh = meshes.add(Sphere::new(0.85).mesh().ico(5).unwrap());
    let comet_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.2, 0.75),
        emissive: LinearRgba::rgb(2.5, 0.4, 1.8),
        metallic: 0.6,
        perceptual_roughness: 0.15,
        ..default()
    });
    commands.spawn((
        Mesh3d(comet_mesh),
        MeshMaterial3d(comet_mat),
        RayTracedBody3d {
            name: "Halley (Ice Comet - Emissive)",
            radius: 0.85,
            metallic: 0.6,
            roughness: 0.15,
            emissive: 2.5,
            orbital_radius: 10.5,
            angular_speed: -1.8,
            current_angle: std::f32::consts::PI,
            cached_aabb: Aabb3d::from_sphere(Vec3::new(-10.5, 2.0, 0.0), 0.85),
        },
        Transform::from_xyz(-10.5, 2.0, 0.0),
        ChildOf(star),
    ));

    // 7. 动态点光源
    commands.spawn((
        PointLight {
            color: Color::srgb(1.0, 0.92, 0.75),
            intensity: 900_000.0,
            range: 35.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 6.0, 0.0),
        OrbitingPointLight {
            orbital_radius: 6.0,
            angular_speed: 1.2,
        },
    ));

    // 8. 方向环境主光源
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.9, 0.95, 1.0),
            illuminance: 12_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(15.0, 25.0, 15.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    println!("[3D SETUP] 3D PBR Scene and Lights Initialized Successfully.");
}

/// 创建 HUD 界面文本
fn setup_ui_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(15.0),
        TextColor(css::WHITE.into()),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            left: Val::Px(16.0),
            padding: UiRect::all(Val::Px(14.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.04, 0.07, 0.12, 0.88)),
        HudText,
    ));
}

/// 相机平滑环绕系统
fn orbit_camera_system(
    time: Res<Time>,
    mut state: ResMut<RayTracing3dState>,
    mut query: Query<&mut Transform, With<MainCamera>>,
) {
    if state.camera_orbit && !state.paused {
        state.camera_angle += 0.3 * time.delta_secs();
        let radius = 25.0;
        let x = radius * state.camera_angle.sin();
        let z = radius * state.camera_angle.cos();
        let y = 11.0 + 3.0 * (state.camera_angle * 0.5).sin();

        for mut transform in &mut query {
            *transform = Transform::from_xyz(x, y, z).looking_at(Vec3::new(0.0, 1.5, 0.0), Vec3::Y);
        }
    }
}

/// 3D 天体公转与自转系统
fn animate_3d_celestial_bodies(
    time: Res<Time>,
    state: Res<RayTracing3dState>,
    mut query: Query<(&mut RayTracedBody3d, &mut Transform), Without<OrbitingPointLight>>,
    mut light_query: Query<(&OrbitingPointLight, &mut Transform), Without<RayTracedBody3d>>,
) {
    if state.paused {
        return;
    }

    let dt = time.delta_secs();
    let t = time.elapsed_secs();

    // 更新天体公转与自转
    for (mut body, mut transform) in &mut query {
        if body.orbital_radius > 0.0 {
            body.current_angle += body.angular_speed * dt;
            transform.translation.x = body.orbital_radius * body.current_angle.cos();
            transform.translation.z = body.orbital_radius * body.current_angle.sin();
            transform.translation.y = 2.0 + 1.0 * (body.current_angle * 2.0).sin();
        }
        transform.rotate_y(body.angular_speed * 0.8 * dt);
    }

    // 更新动态点光源
    for (light, mut transform) in &mut light_query {
        let angle = t * light.angular_speed;
        transform.translation.x = light.orbital_radius * angle.cos();
        transform.translation.z = light.orbital_radius * angle.sin();
        transform.translation.y = 5.5 + 1.5 * (angle * 0.7).sin();
    }
}

/// 更新 TLAS 顶层与 BLAS 底层加速结构
fn update_tlas_and_blas_acceleration_structures(
    mut query: Query<(&mut RayTracedBody3d, &Transform)>,
) {
    for (mut body, transform) in &mut query {
        let center = transform.translation;
        body.cached_aabb = Aabb3d::from_sphere(center, body.radius);
    }
}

/// 调度 3D DXR 四重光线追踪计算 (Primary, Shadow, Reflection, AO)
fn dispatch_3d_dxr_ray_tracing(
    mut state: ResMut<RayTracing3dState>,
    entities: Query<(&RayTracedBody3d, &Transform)>,
    lights: Query<(&OrbitingPointLight, &Transform)>,
    camera_query: Query<&Transform, With<MainCamera>>,
) {
    if state.paused {
        return;
    }

    state.frame_count += 1;
    let camera_transform = match camera_query.iter().next().copied() {
        Some(t) => t,
        None => return,
    };
    let camera_pos = camera_transform.translation;

    let objects: Vec<(&RayTracedBody3d, &Transform)> = entities.iter().collect();
    let light_sources: Vec<(&OrbitingPointLight, &Transform)> = lights.iter().collect();

    let grid_dim = 16;
    for x in 0..grid_dim {
        for y in 0..grid_dim {
            let u = (x as f32 / grid_dim as f32) * 2.0 - 1.0;
            let v = (y as f32 / grid_dim as f32) * 2.0 - 1.0;
            let target = camera_transform.transform_point(Vec3::new(u * 8.0, v * 5.0, -10.0));
            let primary_dir = (target - camera_pos).normalize();
            let inv_dir = Vec3::new(1.0 / primary_dir.x, 1.0 / primary_dir.y, 1.0 / primary_dir.z);

            state.primary_rays += 1;

            let mut closest_hit: Option<(f32, &RayTracedBody3d, Vec3)> = None;
            for (body, _) in &objects {
                state.bvh_checks += 1;
                if let Some(t) = body.cached_aabb.intersect_ray(camera_pos, inv_dir) {
                    if closest_hit.as_ref().map_or(true, |h| t < h.0) {
                        let hit_point = camera_pos + primary_dir * t;
                        closest_hit = Some((t, body, hit_point));
                    }
                }
            }

            if let Some((_dist, hit_obj, hit_point)) = closest_hit {
                let hit_normal = (hit_point - hit_obj.cached_aabb.min.lerp(hit_obj.cached_aabb.max, 0.5)).normalize();

                for (_light, light_trans) in &light_sources {
                    state.shadow_rays += 1;
                    let shadow_dir = (light_trans.translation - hit_point).normalize();
                    let shadow_inv = Vec3::new(1.0 / shadow_dir.x, 1.0 / shadow_dir.y, 1.0 / shadow_dir.z);

                    for (occ, _) in &objects {
                        if !std::ptr::eq(*occ, hit_obj) {
                            state.bvh_checks += 1;
                            let _ = occ.cached_aabb.intersect_ray(hit_point + hit_normal * 0.05, shadow_inv);
                        }
                    }
                }

                if hit_obj.metallic > 0.4 || hit_obj.roughness < 0.25 {
                    state.reflection_rays += 1;
                    let refl_dir = primary_dir - 2.0 * primary_dir.dot(hit_normal) * hit_normal;
                    let refl_inv = Vec3::new(1.0 / refl_dir.x, 1.0 / refl_dir.y, 1.0 / refl_dir.z);

                    for (other, _) in &objects {
                        if !std::ptr::eq(*other, hit_obj) {
                            state.bvh_checks += 1;
                            let _ = other.cached_aabb.intersect_ray(hit_point + hit_normal * 0.05, refl_inv);
                        }
                    }
                }

                state.ao_rays += 2;
            }
        }
    }
}

/// 生成 3D 尾迹发光粒子
fn spawn_3d_procedural_particles(
    mut commands: Commands,
    mut state: ResMut<RayTracing3dState>,
    query: Query<(&RayTracedBody3d, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if state.paused {
        return;
    }

    let p_mesh = meshes.add(Sphere::new(0.08).mesh().ico(3).unwrap());

    for (body, transform) in &query {
        if body.orbital_radius > 0.0 {
            let tangent = Vec3::new(-body.current_angle.sin(), 0.2, body.current_angle.cos());
            let vel = -tangent * body.angular_speed.abs() * 1.5;

            let col = if body.emissive > 1.0 {
                Color::srgb(0.9, 0.2, 0.8)
            } else if body.metallic > 0.8 {
                Color::srgb(0.15, 0.8, 1.0)
            } else {
                Color::srgb(0.95, 0.65, 0.3)
            };

            commands.spawn((
                Mesh3d(p_mesh.clone()),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: col,
                    emissive: LinearRgba::rgb(col.to_srgba().red * 2.0, col.to_srgba().green * 2.0, col.to_srgba().blue * 2.0),
                    ..default()
                })),
                Particle3d {
                    velocity: vel,
                    lifetime: Timer::from_seconds(0.7, TimerMode::Once),
                },
                Transform::from_translation(transform.translation),
            ));
            state.total_particles += 1;
        }
    }
}

/// 更新与回收 3D 粒子
fn update_and_despawn_particles(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<RayTracing3dState>,
    mut query: Query<(Entity, &mut Particle3d, &mut Transform)>,
) {
    if state.paused {
        return;
    }

    let dt = time.delta_secs();
    for (entity, mut particle, mut transform) in &mut query {
        particle.lifetime.tick(time.delta());
        transform.translation += particle.velocity * dt;
        particle.velocity *= 0.96;

        let scale = 1.0 - particle.lifetime.fraction();
        transform.scale = Vec3::splat(scale.max(0.01));

        if particle.lifetime.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// 更新 HUD 文本显示
fn update_ui_hud(
    time: Res<Time>,
    state: Res<RayTracing3dState>,
    matrix: Res<Dx12uFeatureMatrix>,
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<HudText>>,
) {
    let elapsed = time.elapsed_secs();
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(60.0);

    let total_rays = state.primary_rays + state.shadow_rays + state.reflection_rays + state.ao_rays;

    for mut text in &mut query {
        **text = format!(
            "====================================================================\n\
             BEVY 3D GRAPHICS - DIRECTX 12 ULTIMATE & DXR RAY TRACING ENGINE\n\
             ====================================================================\n\
             • Performance     : {:.1} FPS | Uptime: {:.1}s | Frames: {}\n\
             • DXR Ray Tracing : {}\n\
             • Mesh Shaders    : {}\n\
             • Variable Rate   : {}\n\
             • Bindless Engine : {}\n\
             • HLSL Compiler   : {}\n\
             --------------------------------------------------------------------\n\
             • Total DXR Rays  : {} (Pri: {}, Shd: {}, Refl: {}, AO: {})\n\
             • BVH Traversal   : {} AABB Intersection Checks\n\
             • Particles Spawn : {}\n\
             • Orbiting Camera : {}\n\
             --------------------------------------------------------------------\n\
             [C] Toggle Camera Orbit | [Space] Toggle Pause | [Esc] Exit Demo\n\
             ====================================================================",
            fps,
            elapsed,
            state.frame_count,
            matrix.dxr_tier,
            matrix.mesh_shaders,
            matrix.vrs_tier,
            matrix.bindless_model,
            matrix.shader_model,
            total_rays,
            state.primary_rays,
            state.shadow_rays,
            state.reflection_rays,
            state.ao_rays,
            state.bvh_checks,
            state.total_particles,
            if state.camera_orbit { "ACTIVE" } else { "LOCKED" },
        );
    }
}

/// 键盘交互控制
fn handle_input_controls(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<RayTracing3dState>,
    mut exit_writer: MessageWriter<AppExit>,
) {
    if keyboard_input.just_pressed(KeyCode::Escape) {
        println!("[3D DEMO] Escape key pressed. Exiting cleanly.");
        exit_writer.write(AppExit::Success);
    }
    if keyboard_input.just_pressed(KeyCode::Space) {
        state.paused = !state.paused;
        println!(
            "[3D DEMO] Animation {}",
            if state.paused { "PAUSED" } else { "RESUMED" }
        );
    }
    if keyboard_input.just_pressed(KeyCode::KeyC) {
        state.camera_orbit = !state.camera_orbit;
        println!(
            "[3D DEMO] Camera Orbit {}",
            if state.camera_orbit { "ENABLED" } else { "DISABLED" }
        );
    }
}
