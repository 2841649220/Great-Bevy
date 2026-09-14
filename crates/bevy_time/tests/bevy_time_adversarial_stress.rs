//! Adversarial stress test harness for `bevy_time` subsystem logic:
//! 1. `Timer::remaining` & `Timer::almost_finish` pathological inputs & underflow safety
//! 2. `DelayedCommands` chronological ordering & determinism under identical/varying `submit_at` values

extern crate alloc;

use alloc::vec::Vec;
use core::panic::AssertUnwindSafe;
use core::time::Duration;
use std::panic::catch_unwind;

use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_time::{
    DelayedCommandsExt, Time, TimePlugin, TimeUpdateStrategy, Timer, TimerMode, Virtual,
};

// ============================================================================
// 1. Timer::remaining and Timer::almost_finish Stress Tests
// ============================================================================

#[test]
fn test_timer_remaining_and_almost_finish_zero_duration() {
    for mode in [TimerMode::Once, TimerMode::Repeating] {
        let mut t = Timer::new(Duration::ZERO, mode);
        assert_eq!(t.remaining(), Duration::ZERO);
        assert_eq!(t.duration(), Duration::ZERO);
        assert_eq!(t.elapsed(), Duration::ZERO);

        // Calling almost_finish on zero-duration timer must not underflow or panic
        let res = catch_unwind(AssertUnwindSafe(|| {
            t.almost_finish();
        }));
        assert!(
            res.is_ok(),
            "almost_finish panicked on zero-duration timer ({mode:?})"
        );
        assert_eq!(t.remaining(), Duration::ZERO);

        // Repeated calls must be safe and idempotent
        for _ in 0..100 {
            t.almost_finish();
            assert_eq!(t.remaining(), Duration::ZERO);
        }
    }
}

#[test]
fn test_timer_remaining_when_elapsed_equals_duration() {
    for mode in [TimerMode::Once, TimerMode::Repeating] {
        let dur = Duration::from_secs(5);
        let mut t = Timer::new(dur, mode);
        t.set_elapsed(dur);

        assert_eq!(t.elapsed(), dur);
        assert_eq!(t.remaining(), Duration::ZERO);

        let res = catch_unwind(AssertUnwindSafe(|| {
            t.almost_finish();
        }));
        assert!(
            res.is_ok(),
            "almost_finish panicked when elapsed == duration ({mode:?})"
        );
    }
}

#[test]
fn test_timer_remaining_when_elapsed_far_exceeds_duration() {
    let pathological_elapsed = [
        Duration::from_secs(10),
        Duration::from_secs(1_000_000),
        Duration::from_secs(u32::MAX as u64),
        Duration::from_secs(u64::MAX / 2),
        Duration::MAX,
    ];

    for mode in [TimerMode::Once, TimerMode::Repeating] {
        for &elapsed in &pathological_elapsed {
            let mut t = Timer::new(Duration::from_secs(1), mode);
            t.set_elapsed(elapsed);

            assert!(t.elapsed() > t.duration());
            // remaining must saturate to ZERO and never panic
            assert_eq!(
                t.remaining(),
                Duration::ZERO,
                "remaining() must be ZERO when elapsed ({elapsed:?}) > duration"
            );

            // almost_finish must not panic
            let res = catch_unwind(AssertUnwindSafe(|| {
                t.almost_finish();
            }));
            assert!(
                res.is_ok(),
                "almost_finish panicked when elapsed ({elapsed:?}) >> duration for mode {mode:?}"
            );
        }
    }
}

#[test]
fn test_timer_duration_max_pathological_boundaries() {
    for mode in [TimerMode::Once, TimerMode::Repeating] {
        let mut t = Timer::new(Duration::MAX, mode);
        assert_eq!(t.remaining(), Duration::MAX);

        // almost_finish should tick Duration::MAX - 1ns
        let res = catch_unwind(AssertUnwindSafe(|| {
            t.almost_finish();
        }));
        assert!(
            res.is_ok(),
            "almost_finish panicked on Duration::MAX timer ({mode:?})"
        );
        assert_eq!(t.remaining(), Duration::from_nanos(1));
        assert!(!t.is_finished());

        // Ticking 1ns finishes the timer
        t.tick(Duration::from_nanos(1));
        assert!(t.is_finished());
        match mode {
            TimerMode::Once => {
                // A finished non-repeating timer is clamped to its duration, so nothing remains.
                assert_eq!(t.elapsed(), Duration::MAX);
                assert_eq!(t.remaining(), Duration::ZERO);
            }
            TimerMode::Repeating => {
                // A repeating timer wraps around on the tick it finishes: `elapsed` is reset to the
                // remainder of the division (`Duration::MAX % Duration::MAX == 0`), so the remaining
                // time is the full duration of the *next* cycle, not zero.
                assert_eq!(t.elapsed(), Duration::ZERO);
                assert_eq!(t.remaining(), Duration::MAX);
            }
        }

        // Calling almost_finish on finished Duration::MAX timer
        let res2 = catch_unwind(AssertUnwindSafe(|| {
            t.almost_finish();
        }));
        assert!(
            res2.is_ok(),
            "almost_finish panicked on already-finished Duration::MAX timer"
        );
    }
}

#[test]
fn test_timer_huge_delta_ticks_stress_matrix() {
    let durations = [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_nanos(100),
        Duration::from_millis(10),
        Duration::from_secs(1),
        Duration::from_secs(3600),
        Duration::MAX,
    ];

    let tick_deltas = [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_micros(500),
        Duration::from_secs(1),
        Duration::from_secs(100_000),
        Duration::from_secs(u64::MAX / 4),
        Duration::MAX,
    ];

    for mode in [TimerMode::Once, TimerMode::Repeating] {
        for &dur in &durations {
            for &delta in &tick_deltas {
                let mut t = Timer::new(dur, mode);
                let res = catch_unwind(AssertUnwindSafe(|| {
                    t.tick(delta);
                    let _rem = t.remaining();
                    let _frac = t.fraction();
                    let _frac_rem = t.fraction_remaining();
                    t.almost_finish();
                }));
                assert!(
                    res.is_ok(),
                    "Stress matrix failed on dur={dur:?}, delta={delta:?}, mode={mode:?}"
                );
            }
        }
    }
}

#[test]
fn test_timer_almost_finish_repeated_stability() {
    // Non-repeating timer
    let mut once = Timer::from_seconds(2.0, TimerMode::Once);
    once.almost_finish();
    assert_eq!(once.remaining(), Duration::from_nanos(1));
    assert!(!once.is_finished());

    // Calling almost_finish again should be stable at 1ns remaining
    once.almost_finish();
    assert_eq!(once.remaining(), Duration::from_nanos(1));
    assert!(!once.is_finished());

    // Ticking 1ns finishes it
    once.tick(Duration::from_nanos(1));
    assert!(once.is_finished());
    assert_eq!(once.remaining(), Duration::ZERO);

    // Repeating timer
    let mut rep = Timer::from_seconds(2.0, TimerMode::Repeating);
    rep.almost_finish();
    assert_eq!(rep.remaining(), Duration::from_nanos(1));
    assert!(!rep.is_finished());

    // Repeating timer should finish and wrap when 1ns is ticked
    rep.tick(Duration::from_nanos(1));
    assert!(rep.just_finished());
    assert_eq!(rep.times_finished_this_tick(), 1);
    assert_eq!(rep.remaining(), Duration::from_secs(2));

    // almost_finish again prepares the second period
    rep.almost_finish();
    assert_eq!(rep.remaining(), Duration::from_nanos(1));
}

#[test]
fn test_timer_paused_almost_finish() {
    let mut t = Timer::from_seconds(5.0, TimerMode::Once);
    t.pause();
    assert!(t.is_paused());

    // almost_finish on paused timer should not advance elapsed time
    t.almost_finish();
    assert_eq!(t.elapsed(), Duration::ZERO);
    assert_eq!(t.remaining(), Duration::from_secs(5));

    t.unpause();
    t.almost_finish();
    assert_eq!(t.remaining(), Duration::from_nanos(1));
}

// ============================================================================
// 2. DelayedCommands Deterministic Chronological Ordering Tests
// ============================================================================

#[derive(Resource, Default)]
struct TestLog(Vec<usize>);

#[test]
fn test_delayed_commands_identical_submit_at_same_frame_fifo() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));

    // Frame 0: setup
    app.update();

    // Enqueue 50 separate delayed command queues all with identical 50ms delay
    const COUNT: usize = 50;
    {
        let mut commands = app.world_mut().commands();
        for i in 0..COUNT {
            commands
                .delayed()
                .duration(Duration::from_millis(50))
                .queue(move |world: &mut World| {
                    world.resource_mut::<TestLog>().0.push(i);
                });
        }
    }

    // Frame 1: elapsed = 100ms -> all 50 queues mature simultaneously (each is due at 50ms)
    app.update();

    let log = &app.world().resource::<TestLog>().0;
    let expected: Vec<usize> = (0..COUNT).collect();
    assert_eq!(
        log, &expected,
        "Queues with identical submit_at in the same frame must execute in strict FIFO order"
    );
}

#[test]
fn test_delayed_commands_identical_submit_at_across_different_frames() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )));

    // Frame 0: elapsed = 0ms
    app.update();

    // Between frame 0 and frame 1 (elapsed = 0ms): delay 150ms -> submit_at = 150ms (Tag 1)
    {
        let mut commands = app.world_mut().commands();
        commands
            .delayed()
            .duration(Duration::from_millis(150))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(1);
            });
    }
    app.update();

    // Between frame 1 and frame 2 (elapsed = 50ms): delay 100ms -> submit_at = 150ms (Tag 2)
    {
        let mut commands = app.world_mut().commands();
        commands
            .delayed()
            .duration(Duration::from_millis(100))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(2);
            });
    }
    app.update();

    // After frame 2 (elapsed = 100ms) nothing can have matured yet: every queue is due at 150ms.
    // Note that queues submitted between frames are spawned before that frame's `Time` update, so
    // their `submit_at` is measured from the elapsed time of the *previous* frame.
    assert_eq!(app.world().resource::<TestLog>().0, Vec::<usize>::new());

    // Between frame 2 and frame 3 (elapsed = 100ms): delay 50ms -> submit_at = 150ms (Tag 3)
    {
        let mut commands = app.world_mut().commands();
        commands
            .delayed()
            .duration(Duration::from_millis(50))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(3);
            });
    }
    app.update();

    // All three queues mature simultaneously at submit_at = 150ms during this frame.
    let log = &app.world().resource::<TestLog>().0;
    assert_eq!(
        log,
        &[1, 2, 3],
        "Queues maturing at the exact same submit_at across different frames must preserve deterministic order"
    );
}

#[test]
fn test_delayed_commands_scrambled_varying_submit_at_ordering() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            1000,
        )));

    // `Time<Virtual>` clamps large frame deltas to `max_delta` (250ms by default), which would
    // spread these queues over multiple frames. Raise the clamp so the full 1000ms step is applied
    // in a single frame and every queue matures at once.
    app.world_mut()
        .resource_mut::<Time<Virtual>>()
        .set_max_delta(Duration::from_secs(10));

    // Frame 0: setup
    app.update();

    // Enqueue 20 commands with wildly shuffled delay durations
    let delays_ms = [
        450, 10, 800, 50, 200, 10, 999, 150, 300, 50, 700, 20, 600, 100, 850, 5, 400, 500, 900, 250,
    ];

    // Compute expected order: sort by (delay, original_index)
    let mut indexed_delays: Vec<(usize, usize)> = delays_ms.into_iter().enumerate().collect();
    // stable sort by delay value
    indexed_delays.sort_by_key(|&(_, delay)| delay);
    let expected_order: Vec<usize> = indexed_delays.iter().map(|&(idx, _)| idx).collect();

    {
        let mut commands = app.world_mut().commands();
        for (idx, &delay) in delays_ms.iter().enumerate() {
            commands
                .delayed()
                .duration(Duration::from_millis(delay as u64))
                .queue(move |world: &mut World| {
                    world.resource_mut::<TestLog>().0.push(idx);
                });
        }
    }

    // Frame 1: advance by 1000ms -> all mature in the same frame
    app.update();

    let log = &app.world().resource::<TestLog>().0;
    assert_eq!(
        log, &expected_order,
        "Scrambled delayed commands must execute in strictly ascending chronological order"
    );
}

#[test]
fn test_delayed_commands_zero_duration_immediate_execution() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )));

    app.update(); // Frame 0

    // Schedule a zero-duration delayed command along with a 20ms and 50ms command
    {
        let mut commands = app.world_mut().commands();
        commands
            .delayed()
            .duration(Duration::from_millis(50))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(50);
            });
        commands
            .delayed()
            .duration(Duration::ZERO)
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(0);
            });
        commands
            .delayed()
            .duration(Duration::from_millis(20))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(20);
            });
    }

    // Frame 1: advances time by 50ms
    app.update();

    let log = &app.world().resource::<TestLog>().0;
    assert_eq!(
        log,
        &[0, 20, 50],
        "Zero-duration delayed commands must execute ahead of positive-duration commands"
    );
}

#[test]
fn test_delayed_commands_nested_cascading_submissions() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )));

    app.update(); // Frame 0

    // Enqueue command at T + 50ms which enqueues another at T' + 50ms
    {
        let mut commands = app.world_mut().commands();
        commands
            .delayed()
            .duration(Duration::from_millis(50))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(1);
                let mut commands = world.commands();
                commands
                    .delayed()
                    .duration(Duration::from_millis(50))
                    .queue(|world: &mut World| {
                        world.resource_mut::<TestLog>().0.push(2);
                    });
            });
    }

    // Frame 1: time advances to 50ms -> command 1 executes and queues command 2
    app.update();
    assert_eq!(app.world().resource::<TestLog>().0, &[1]);

    // Frame 2: time advances to 100ms -> command 2 executes
    app.update();
    assert_eq!(app.world().resource::<TestLog>().0, &[1, 2]);
}

#[test]
fn test_delayed_commands_single_delayed_builder_multiple_queues() {
    let mut app = App::new();
    app.add_plugins(TimePlugin)
        .init_resource::<TestLog>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )));

    app.update(); // Frame 0

    // Use single commands.delayed() builder to register multiple commands on same and different durations
    {
        let mut commands = app.world_mut().commands();
        let mut delayed = commands.delayed();

        // 30ms queue
        delayed
            .duration(Duration::from_millis(30))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(30);
            });

        // 10ms queue - two commands
        let mut ten = delayed.duration(Duration::from_millis(10));
        ten.queue(|world: &mut World| {
            world.resource_mut::<TestLog>().0.push(10);
        });
        ten.queue(|world: &mut World| {
            world.resource_mut::<TestLog>().0.push(11);
        });

        // 20ms queue
        delayed
            .duration(Duration::from_millis(20))
            .queue(|world: &mut World| {
                world.resource_mut::<TestLog>().0.push(20);
            });
    }

    app.update();

    let log = &app.world().resource::<TestLog>().0;
    assert_eq!(
        log,
        &[10, 11, 20, 30],
        "Single builder registering varying durations must execute in strict chronological order"
    );
}
