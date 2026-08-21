//! Bevy 2D Graphical Engine Test Demo
//!
//! 专属于 2D 的图形化渲染与物理粒子交互 Demo：
//! 包含 2D 视口相机、多层级几何图元渲染（中心发光核、公转行星环带、几何卫星）、
//! 动态引力轨迹微扰、绚丽程序化发光粒子流以及屏幕实时 HUD 遥测信息看板。

use bevy::{
    app::AppExit,
    color::palettes::css,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin, SystemInformationDiagnosticsPlugin},
    prelude::*,
};

fn main() {
    println!("================================================================================");
    println!("  [BEVY 2D DEMO] - Launching 2D Graphical Window & Rendering Subsystems");
    println!("================================================================================");

    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Bevy Engine - 2D Graphics & Particle Dynamics Demo".into(),
                    resolution: (1280, 720).into(),
                    present_mode: bevy::window::PresentMode::AutoVsync,
                    ..default()
                }),
                ..default()
            }),
            FrameTimeDiagnosticsPlugin::default(),
            SystemInformationDiagnosticsPlugin,
        ))
        .init_resource::<Demo2dState>()
        .add_systems(Startup, (setup_2d_scene, setup_ui_hud))
        .add_systems(
            Update,
            (
                animate_2d_orbiters_and_spinners,
                emit_2d_luminous_particles,
                update_and_despawn_particles,
                update_ui_hud,
                handle_input_controls,
            ),
        )
        .run();
}

/// 2D 状态与运行统计
#[derive(Resource)]
struct Demo2dState {
    frame_count: u64,
    total_particles_spawned: u64,
    paused: bool,
}

impl Default for Demo2dState {
    fn default() -> Self {
        Self {
            frame_count: 0,
            total_particles_spawned: 0,
            paused: false,
        }
    }
}

/// 2D 自转物体组件
#[derive(Component)]
struct Spinner2d {
    angular_speed: f32,
}

/// 2D 公转环绕物体组件
#[derive(Component)]
struct Orbiter2d {
    radius: f32,
    angular_speed: f32,
    current_angle: f32,
}

/// 2D 动力学粒子
#[derive(Component)]
struct Particle2d {
    velocity: Vec2,
    lifetime: Timer,
}

/// HUD 文本标记组件
#[derive(Component)]
struct HudText;

/// 初始化 2D 图形场景
fn setup_2d_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    println!("[2D SETUP] Initializing 2D Camera, Vector Geometry & Shaded Materials...");

    // 1. 2D 摄像机
    commands.spawn(Camera2d);

    // 2. 空间轨道环线（几何参考背景）
    for r in [120.0, 220.0, 310.0] {
        commands.spawn((
            Mesh2d(meshes.add(Annulus::new(r - 1.0, r + 1.0))),
            MeshMaterial2d(materials.add(ColorMaterial::from_color(Color::srgba(
                0.2, 0.35, 0.6, 0.25,
            )))),
            Transform::from_xyz(0.0, 0.0, -1.0),
        ));
    }

    // 3. 中心核心恒星 (Glowing Central Star)
    let core_mesh = meshes.add(Circle::new(45.0));
    let core_mat = materials.add(ColorMaterial::from_color(Color::srgb(1.0, 0.82, 0.15)));
    let core = commands
        .spawn((
            Mesh2d(core_mesh),
            MeshMaterial2d(core_mat),
            Spinner2d { angular_speed: 0.8 },
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();

    // 恒星光晕外环
    commands.spawn((
        Mesh2d(meshes.add(Annulus::new(50.0, 62.0))),
        MeshMaterial2d(materials.add(ColorMaterial::from_color(Color::srgba(
            1.0, 0.6, 0.1, 0.4,
        )))),
        Transform::from_xyz(0.0, 0.0, -0.5),
        ChildOf(core),
    ));

    // 4. 内圈卫星 A (Terra Cyan - 正六边形)
    let sat_a_mesh = meshes.add(RegularPolygon::new(20.0, 6));
    let sat_a_mat = materials.add(ColorMaterial::from_color(Color::srgb(0.1, 0.85, 0.95)));
    commands.spawn((
        Mesh2d(sat_a_mesh),
        MeshMaterial2d(sat_a_mat),
        Orbiter2d {
            radius: 120.0,
            angular_speed: 1.8,
            current_angle: 0.0,
        },
        Spinner2d { angular_speed: -2.5 },
        Transform::from_xyz(120.0, 0.0, 1.0),
        ChildOf(core),
    ));

    // 5. 中圈卫星 B (Ares Crimson - 胶囊体)
    let sat_b_mesh = meshes.add(Capsule2d::new(14.0, 36.0));
    let sat_b_mat = materials.add(ColorMaterial::from_color(Color::srgb(0.95, 0.25, 0.45)));
    commands.spawn((
        Mesh2d(sat_b_mesh),
        MeshMaterial2d(sat_b_mat),
        Orbiter2d {
            radius: 220.0,
            angular_speed: 1.1,
            current_angle: std::f32::consts::FRAC_PI_2,
        },
        Spinner2d { angular_speed: 3.2 },
        Transform::from_xyz(0.0, 220.0, 1.0),
        ChildOf(core),
    ));

    // 6. 外圈逆行卫星 C (Halley Emerald - 正八边形)
    let sat_c_mesh = meshes.add(RegularPolygon::new(16.0, 8));
    let sat_c_mat = materials.add(ColorMaterial::from_color(Color::srgb(0.2, 0.95, 0.55)));
    commands.spawn((
        Mesh2d(sat_c_mesh),
        MeshMaterial2d(sat_c_mat),
        Orbiter2d {
            radius: 310.0,
            angular_speed: -0.7,
            current_angle: std::f32::consts::PI,
        },
        Spinner2d { angular_speed: -1.2 },
        Transform::from_xyz(-310.0, 0.0, 1.0),
        ChildOf(core),
    ));

    println!("[2D SETUP] Graphical 2D scene hierarchy created successfully.");
}

/// 创建 HUD 界面文本
fn setup_ui_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(16.0),
        TextColor(css::WHITE.into()),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(16.0),
            left: Val::Px(16.0),
            padding: UiRect::all(Val::Px(12.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.08, 0.12, 0.85)),
        HudText,
    ));
}

/// 2D 自转与公转驱动系统
fn animate_2d_orbiters_and_spinners(
    time: Res<Time>,
    state: Res<Demo2dState>,
    mut spin_query: Query<(&Spinner2d, &mut Transform), Without<Orbiter2d>>,
    mut orbit_query: Query<(&mut Orbiter2d, &Spinner2d, &mut Transform)>,
) {
    if state.paused {
        return;
    }

    let dt = time.delta_secs();

    // 更新自转
    for (spin, mut transform) in &mut spin_query {
        transform.rotate_z(spin.angular_speed * dt);
    }

    // 更新公转与自身旋转
    for (mut orbit, spin, mut transform) in &mut orbit_query {
        orbit.current_angle += orbit.angular_speed * dt;
        transform.translation.x = orbit.radius * orbit.current_angle.cos();
        transform.translation.y = orbit.radius * orbit.current_angle.sin();
        transform.rotate_z(spin.angular_speed * dt);
    }
}

/// 发射发光 2D 粒子流
fn emit_2d_luminous_particles(
    mut commands: Commands,
    mut state: ResMut<Demo2dState>,
    orbit_query: Query<(&Orbiter2d, &Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if state.paused {
        return;
    }

    state.frame_count += 1;
    let p_mesh = meshes.add(Circle::new(3.5));

    for (orbit, transform) in &orbit_query {
        let pos = transform.translation.truncate();
        let tangent = Vec2::new(-orbit.current_angle.sin(), orbit.current_angle.cos());
        let vel = -tangent * orbit.angular_speed.abs() * 35.0;

        let col = if orbit.angular_speed > 1.5 {
            Color::srgba(0.1, 0.85, 0.95, 0.8)
        } else if orbit.angular_speed > 0.0 {
            Color::srgba(0.95, 0.25, 0.45, 0.8)
        } else {
            Color::srgba(0.2, 0.95, 0.55, 0.8)
        };

        commands.spawn((
            Mesh2d(p_mesh.clone()),
            MeshMaterial2d(materials.add(ColorMaterial::from_color(col))),
            Particle2d {
                velocity: vel,
                lifetime: Timer::from_seconds(0.65, TimerMode::Once),
            },
            Transform::from_xyz(pos.x, pos.y, 0.5),
        ));
        state.total_particles_spawned += 1;
    }
}

/// 更新与回收粒子
fn update_and_despawn_particles(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<Demo2dState>,
    mut query: Query<(Entity, &mut Particle2d, &mut Transform)>,
) {
    if state.paused {
        return;
    }

    let dt = time.delta_secs();
    for (entity, mut particle, mut transform) in &mut query {
        particle.lifetime.tick(time.delta());
        transform.translation.x += particle.velocity.x * dt;
        transform.translation.y += particle.velocity.y * dt;
        particle.velocity *= 0.96;

        let scale = 1.0 - (particle.lifetime.fraction());
        transform.scale = Vec3::splat(scale.max(0.01));

        if particle.lifetime.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// 更新 HUD 文本显示
fn update_ui_hud(
    time: Res<Time>,
    state: Res<Demo2dState>,
    active_particles: Query<&Particle2d>,
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<HudText>>,
) {
    let elapsed = time.elapsed_secs();
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(60.0);

    let active_p = active_particles.iter().count();

    for mut text in &mut query {
        **text = format!(
            "====================================================\n\
             BEVY 2D GRAPHICAL ENGINE & DYNAMICS DEMO\n\
             ====================================================\n\
             • Uptime          : {:.1}s\n\
             • Render FrameRate: {:.1} FPS\n\
             • Total Frames    : {}\n\
             • Active Particles: {}\n\
             • Total Particles : {}\n\
             • Simulation State: {}\n\
             ----------------------------------------------------\n\
             [Space] Toggle Pause | [Esc] Exit Demo\n\
             ====================================================",
            elapsed,
            fps,
            state.frame_count,
            active_p,
            state.total_particles_spawned,
            if state.paused { "PAUSED" } else { "RUNNING" }
        );
    }
}

/// 键盘输入交互
fn handle_input_controls(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<Demo2dState>,
    mut exit_writer: MessageWriter<AppExit>,
) {
    if keyboard_input.just_pressed(KeyCode::Escape) {
        println!("[2D DEMO] Escape key pressed. Exiting cleanly.");
        exit_writer.write(AppExit::Success);
    }
    if keyboard_input.just_pressed(KeyCode::Space) {
        state.paused = !state.paused;
        println!(
            "[2D DEMO] Simulation {}",
            if state.paused { "PAUSED" } else { "RESUMED" }
        );
    }
}
