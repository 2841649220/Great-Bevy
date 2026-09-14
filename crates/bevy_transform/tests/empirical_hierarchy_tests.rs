#![expect(
    missing_docs,
    unused_imports,
    reason = "Integration test binaries do not expose a documented public API."
)]

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_tasks::{ComputeTaskPool, TaskPool};
use bevy_transform::prelude::*;
use bevy_transform::systems::propagate_parent_transforms;

fn run_hierarchy_depth(depth: usize) {
    ComputeTaskPool::get_or_init(TaskPool::default);
    let mut app = App::new();
    app.insert_resource(StaticTransformOptimizations::default());
    app.add_systems(Update, propagate_parent_transforms);

    let mut parent = app
        .world_mut()
        .spawn((
            Transform::from_xyz(1.0, 0.0, 0.0),
            GlobalTransform::IDENTITY,
        ))
        .id();

    for _ in 1..depth {
        let child = app
            .world_mut()
            .spawn((
                Transform::from_xyz(1.0, 0.0, 0.0),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.world_mut().entity_mut(parent).add_children(&[child]);
        parent = child;
    }

    app.update();
}

#[test]
fn test_depth_950() {
    run_hierarchy_depth(950);
}

#[test]
fn test_depth_1000() {
    run_hierarchy_depth(1000);
}
