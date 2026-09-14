//! Stress tests for `bevy_ecs` `MultiThreadedExecutor` and hotpatching synchronization.

extern crate alloc;

use alloc::sync::Arc;
use core::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicUsize, Ordering},
};
use std::panic::catch_unwind;

use bevy_ecs::{
    change_detection::ResMut,
    prelude::Resource,
    schedule::{IntoScheduleConfigs, MultiThreadedExecutor, Schedule},
    system::Commands,
    world::World,
};

#[derive(Resource, Default)]
struct CounterA(usize);

#[derive(Resource, Default)]
struct CounterB(usize);

#[derive(Resource, Default)]
struct CounterC(usize);

#[derive(Resource, Default)]
struct CounterD(usize);

#[derive(Resource, Default)]
struct SharedAtomicCounter(Arc<AtomicUsize>);

fn sys_a(mut a: ResMut<CounterA>) {
    a.0 += 1;
}

fn sys_b(mut b: ResMut<CounterB>) {
    b.0 += 2;
}

fn sys_c(mut c: ResMut<CounterC>) {
    c.0 += 3;
}

fn sys_d(mut d: ResMut<CounterD>) {
    d.0 += 4;
}

fn sys_atomic(counter: bevy_ecs::change_detection::Res<SharedAtomicCounter>) {
    counter.0.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn test_multi_threaded_executor_concurrent_disjoint_systems() {
    let mut world = World::new();
    world.init_resource::<CounterA>();
    world.init_resource::<CounterB>();
    world.init_resource::<CounterC>();
    world.init_resource::<CounterD>();
    let atomic_arc = Arc::new(AtomicUsize::new(0));
    world.insert_resource(SharedAtomicCounter(atomic_arc.clone()));

    let mut schedule = Schedule::default();
    schedule.set_executor(MultiThreadedExecutor::new());

    // Disjoint systems that can run in parallel
    schedule.add_systems((sys_a, sys_b, sys_c, sys_d, sys_atomic));

    for _ in 0..100 {
        schedule.run(&mut world);
    }

    assert_eq!(world.resource::<CounterA>().0, 100);
    assert_eq!(world.resource::<CounterB>().0, 200);
    assert_eq!(world.resource::<CounterC>().0, 300);
    assert_eq!(world.resource::<CounterD>().0, 400);
    assert_eq!(atomic_arc.load(Ordering::SeqCst), 100);
}

#[test]
fn test_multi_threaded_executor_panic_propagation_and_cleanup() {
    let mut world = World::new();
    world.init_resource::<CounterA>();

    let mut schedule = Schedule::default();
    schedule.set_executor(MultiThreadedExecutor::new());

    schedule.add_systems((sys_a, || {
        panic!("multi-threaded system intentional panic");
    }));

    for _ in 0..10 {
        let res = catch_unwind(AssertUnwindSafe(|| {
            schedule.run(&mut world);
        }));
        assert!(res.is_err(), "Schedule run should propagate system panic");
    }
}

#[cfg(feature = "hotpatching")]
#[test]
fn test_multi_threaded_executor_hotpatch_sync() {
    use bevy_ecs::HotPatchChanges;

    let mut world = World::new();
    world.init_resource::<CounterA>();
    world.init_resource::<CounterB>();
    world.init_resource::<HotPatchChanges>();

    let mut schedule = Schedule::default();
    schedule.set_executor(MultiThreadedExecutor::new());
    schedule.add_systems((sys_a, sys_b));

    for iter in 0..50 {
        // Mutate HotPatchChanges between schedule runs
        if iter % 5 == 0 {
            if let Some(mut hotpatch) = world.get_resource_mut::<HotPatchChanges>() {
                // Mutating marks it as changed
                let _ = &mut *hotpatch;
            }
        }
        schedule.run(&mut world);
    }

    assert_eq!(world.resource::<CounterA>().0, 50);
    assert_eq!(world.resource::<CounterB>().0, 100);
}

#[test]
fn test_multi_threaded_executor_heavy_chaining_and_barriers() {
    let mut world = World::new();
    let mut schedule = Schedule::default();
    schedule.set_executor(MultiThreadedExecutor::new());

    #[derive(Resource, Default)]
    struct StepTracker(Vec<usize>);
    world.init_resource::<StepTracker>();

    // 10 chained systems that record sequence
    schedule.add_systems(
        (
            |mut t: ResMut<StepTracker>| t.0.push(0),
            |mut t: ResMut<StepTracker>| t.0.push(1),
            |mut t: ResMut<StepTracker>| t.0.push(2),
            |mut t: ResMut<StepTracker>| t.0.push(3),
            |mut t: ResMut<StepTracker>| t.0.push(4),
            |mut commands: Commands| {
                commands.queue(|_: &mut World| {});
            },
            |mut t: ResMut<StepTracker>| t.0.push(5),
            |mut t: ResMut<StepTracker>| t.0.push(6),
        )
            .chain(),
    );

    schedule.run(&mut world);
    assert_eq!(world.resource::<StepTracker>().0, vec![0, 1, 2, 3, 4, 5, 6]);
}
