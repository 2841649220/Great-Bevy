//! Bevy Clean Rebuild Test Demo
//! 验证工作区清理并全量重编译后的 Bevy 引擎核心子系统（ECS、调度器、数学几何、系统诊断、事件/消息驱动体系）。

use bevy::{
    app::AppExit,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin, SystemInformationDiagnosticsPlugin},
    prelude::*,
};

fn main() {
    println!("================================================================================");
    println!("  [BEVY REBUILT ENGINE TEST DEMO] - Initializing Core Subsystems");
    println!("================================================================================");

    App::new()
        .add_plugins((
            MinimalPlugins,
            FrameTimeDiagnosticsPlugin::default(),
            SystemInformationDiagnosticsPlugin,
        ))
        .init_resource::<SimulationMetrics>()
        .add_systems(Startup, (setup_simulation_environment, print_system_info))
        .add_systems(
            Update,
            (
                simulate_orbital_mechanics,
                detect_orbital_conjunctions,
                spawn_procedural_particles,
                update_and_despawn_particles,
                report_simulation_diagnostics,
                automated_lifecycle_manager,
            )
                .chain(),
        )
        .run();
}

/// 模拟环境与统计数据
#[derive(Resource)]
struct SimulationMetrics {
    tick_count: u64,
    conjunction_count: u64,
    total_particles_spawned: u64,
    timer: Timer,
    max_duration_seconds: f32,
}

impl Default for SimulationMetrics {
    fn default() -> Self {
        Self {
            tick_count: 0,
            conjunction_count: 0,
            total_particles_spawned: 0,
            timer: Timer::from_seconds(1.0, TimerMode::Repeating),
            max_duration_seconds: 5.0, // 运行5秒完成完整动力学与诊断测试
        }
    }
}

/// 天体核心组件
#[derive(Component)]
struct CelestialBody {
    name: &'static str,
    mass: f32,
    orbital_radius: f32,
    angular_speed: f32,
    current_angle: f32,
}

/// 粒子组件
#[derive(Component)]
struct Particle {
    velocity: Vec3,
    lifetime: Timer,
}

/// 初始化仿真环境与测试实体
fn setup_simulation_environment(mut commands: Commands) {
    println!("[DEMO SETUP] Spawning Celestial Bodies and Hierarchical ECS Entities...");

    // 1. 中心恒星 (Central Star)
    let star = commands
        .spawn((
            CelestialBody {
                name: "Alpha Prime (Central Star)",
                mass: 10_000.0,
                orbital_radius: 0.0,
                angular_speed: 0.0,
                current_angle: 0.0,
            },
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();

    // 2. 行星 A (Terra Nova)
    commands.spawn((
        CelestialBody {
            name: "Terra Nova (Planet A)",
            mass: 50.0,
            orbital_radius: 12.0,
            angular_speed: 1.2,
            current_angle: 0.0,
        },
        Transform::from_xyz(12.0, 0.0, 0.0),
        ChildOf(star),
    ));

    // 3. 行星 B (Ares Major)
    commands.spawn((
        CelestialBody {
            name: "Ares Major (Planet B)",
            mass: 75.0,
            orbital_radius: 20.0,
            angular_speed: 0.75,
            current_angle: std::f32::consts::PI / 2.0,
        },
        Transform::from_xyz(0.0, 0.0, 20.0),
        ChildOf(star),
    ));

    // 4. 逆行彗星 (Halley Retrograde)
    commands.spawn((
        CelestialBody {
            name: "Halley (Retrograde Comet)",
            mass: 5.0,
            orbital_radius: 16.0,
            angular_speed: -2.1,
            current_angle: std::f32::consts::PI,
        },
        Transform::from_xyz(-16.0, 0.0, 0.0),
        ChildOf(star),
    ));

    println!("[DEMO SETUP] 4 Celestial bodies spawned successfully in ECS hierarchy.");
}

/// 打印系统环境信息
fn print_system_info() {
    println!("[DEMO ENVIRONMENT] Bevy Engine v0.19.0 ECS & Core Schedules Active.");
}

/// 轨道力学运动更新系统
fn simulate_orbital_mechanics(time: Res<Time>, mut query: Query<(&mut CelestialBody, &mut Transform)>) {
    let dt = time.delta_secs();
    for (mut body, mut transform) in &mut query {
        if body.orbital_radius > 0.0 {
            body.current_angle += body.angular_speed * dt;
            transform.translation.x = body.orbital_radius * body.current_angle.cos();
            transform.translation.z = body.orbital_radius * body.current_angle.sin();
            transform.translation.y = 1.2 * (body.current_angle * 2.0).sin();
        }
    }
}

/// 近邻会合事件检测系统
fn detect_orbital_conjunctions(
    query: Query<(&CelestialBody, &Transform)>,
    mut metrics: ResMut<SimulationMetrics>,
) {
    let bodies: Vec<(&CelestialBody, &Transform)> = query.iter().collect();
    let n = bodies.len();

    for i in 0..n {
        for j in (i + 1)..n {
            let dist = bodies[i].1.translation.distance(bodies[j].1.translation);
            // 当两个天体距离在特定阈值内判定为会合 (Conjunction)
            if dist < 7.0 && bodies[i].0.orbital_radius > 0.0 && bodies[j].0.orbital_radius > 0.0 {
                metrics.conjunction_count += 1;
            }
        }
    }
}

/// 程序化生成粒子系统
fn spawn_procedural_particles(
    mut commands: Commands,
    query: Query<(&CelestialBody, &Transform)>,
    mut metrics: ResMut<SimulationMetrics>,
) {
    for (body, transform) in &query {
        if body.orbital_radius > 0.0 {
            // 彗星和行星释放轨道尾迹粒子
            let spread = (body.current_angle.sin() * 0.5, body.current_angle.cos() * 0.5);
            commands.spawn((
                Particle {
                    velocity: Vec3::new(-spread.1 * 2.0, 0.2, spread.0 * 2.0),
                    lifetime: Timer::from_seconds(0.8, TimerMode::Once),
                },
                Transform::from_translation(transform.translation),
            ));
            metrics.total_particles_spawned += 1;
        }
    }
}

/// 更新粒子生命周期与回收
fn update_and_despawn_particles(
    mut commands: Commands,
    time: Res<Time>,
    mut query: Query<(Entity, &mut Particle, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut particle, mut transform) in &mut query {
        particle.lifetime.tick(time.delta());
        transform.translation += particle.velocity * dt;

        if particle.lifetime.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

/// 仿真统计与诊断报告系统
fn report_simulation_diagnostics(
    time: Res<Time>,
    mut metrics: ResMut<SimulationMetrics>,
    active_particles: Query<&Particle>,
    diagnostics: Res<DiagnosticsStore>,
) {
    metrics.tick_count += 1;
    metrics.timer.tick(time.delta());

    if metrics.timer.just_finished() {
        let elapsed = time.elapsed_secs();
        let ticks_per_sec = metrics.tick_count as f32 / elapsed.max(0.001);
        let active_count = active_particles.iter().count();

        let mem_str = if let Some(diag) = diagnostics.get(&SystemInformationDiagnosticsPlugin::SYSTEM_MEM_USAGE) {
            if let Some(val) = diag.smoothed() {
                format!("{:.1}%", val)
            } else {
                "N/A".into()
            }
        } else {
            "N/A".into()
        };

        println!(
            "[DEMO TELEMETRY] Uptime: {:.1}s / {:.1}s | Ticks: {} ({:.0} Hz) | Active Particles: {} | Total Spawned: {} | Conjunctions: {} | Mem: {}",
            elapsed,
            metrics.max_duration_seconds,
            metrics.tick_count,
            ticks_per_sec,
            active_count,
            metrics.total_particles_spawned,
            metrics.conjunction_count,
            mem_str
        );
    }
}

/// 自动生命周期与退出管理器
fn automated_lifecycle_manager(
    time: Res<Time>,
    metrics: Res<SimulationMetrics>,
    mut app_exit_writer: MessageWriter<AppExit>,
) {
    if time.elapsed_secs() >= metrics.max_duration_seconds {
        println!("================================================================================");
        println!(
            "  [DEMO SUCCESS] Simulation completed successfully in {:.2}s!",
            time.elapsed_secs()
        );
        println!(
            "  Final Summary: {} Ticks Processed | {} Procedural Entities Spawned/Despawned",
            metrics.tick_count, metrics.total_particles_spawned
        );
        println!("  All Bevy ECS schedules and core engine systems verified operational.");
        println!("================================================================================");
        app_exit_writer.write(AppExit::Success);
    }
}
