//! Stress tests for `TaskPool` scoped panic safety.

#![cfg(feature = "multi_threaded")]

extern crate alloc;

use alloc::sync::Arc;
use core::{
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{panic::catch_unwind, thread};

use bevy_tasks::{TaskPool, TaskPoolBuilder};

#[test]
fn test_scope_panic_stack_uaf_oracle() {
    let pool = TaskPool::new();

    for _ in 0..50 {
        let scope_exited = AtomicBool::new(false);
        let uaf_detected = Arc::new(AtomicBool::new(false));

        let uaf_detected_clone = uaf_detected.clone();
        let scope_exited_ref = &scope_exited;

        let res = catch_unwind(AssertUnwindSafe(|| {
            pool.scope(|scope| {
                let uaf = uaf_detected_clone.clone();
                scope.spawn(async move {
                    thread::sleep(Duration::from_millis(10));
                    if scope_exited_ref.load(Ordering::Acquire) {
                        uaf.store(true, Ordering::Release);
                    }
                });

                let uaf2 = uaf_detected_clone.clone();
                scope.spawn_on_scope(async move {
                    thread::sleep(Duration::from_millis(5));
                    if scope_exited_ref.load(Ordering::Acquire) {
                        uaf2.store(true, Ordering::Release);
                    }
                    panic!("spawn_on_scope panicking");
                });

                panic!("scope closure panicking");
            });
        }));

        // Immediately mark scope as exited
        scope_exited.store(true, Ordering::Release);

        assert!(res.is_err(), "Scope should have panicked");
        assert!(
            !uaf_detected.load(Ordering::Acquire),
            "UAF detected! A background task executed after scope unwound!"
        );
    }
}

#[test]
fn test_scope_mixed_panicking_and_non_panicking_tasks() {
    let pool = TaskPool::new();

    for iter in 0..30 {
        let completed = Arc::new(AtomicUsize::new(0));
        let panicked = Arc::new(AtomicUsize::new(0));

        let comp = completed.clone();
        let pan = panicked.clone();

        let res = catch_unwind(AssertUnwindSafe(|| {
            pool.scope(|scope| {
                // Task 1: panics after short delay
                let p = pan.clone();
                scope.spawn(async move {
                    thread::sleep(Duration::from_millis(5));
                    p.fetch_add(1, Ordering::SeqCst);
                    panic!("task 1 panic");
                });

                // Task 2: completes successfully
                let c = comp.clone();
                scope.spawn(async move {
                    thread::sleep(Duration::from_millis(15));
                    c.fetch_add(1, Ordering::SeqCst);
                });

                // Task 3: panics immediately
                let p2 = pan.clone();
                scope.spawn(async move {
                    p2.fetch_add(1, Ordering::SeqCst);
                    panic!("task 3 immediate panic");
                });

                // Task 4: on scope, completes
                let c2 = comp.clone();
                scope.spawn_on_scope(async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                });

                // Task 5: on scope, panics
                let p3 = pan.clone();
                scope.spawn_on_scope(async move {
                    p3.fetch_add(1, Ordering::SeqCst);
                    panic!("task 5 on scope panic");
                });

                if iter % 2 == 0 {
                    panic!("closure intentional panic");
                }
            });
        }));

        assert!(res.is_err(), "Scope should have panicked");
        assert_eq!(
            comp.load(Ordering::SeqCst),
            2,
            "Both non-panicking tasks should complete"
        );
        assert_eq!(
            pan.load(Ordering::SeqCst),
            3,
            "All 3 panicking tasks should reach their execution"
        );
    }
}

#[test]
fn test_zero_threads_pool_scope_panic() {
    let pool = TaskPoolBuilder::new().num_threads(0).build();

    for _ in 0..20 {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = counter.clone();

        let res = catch_unwind(AssertUnwindSafe(|| {
            pool.scope(|scope| {
                let c1 = c.clone();
                scope.spawn(async move {
                    c1.fetch_add(1, Ordering::SeqCst);
                    panic!("zero-thread pool task panic");
                });

                let c2 = c.clone();
                scope.spawn_on_scope(async move {
                    c2.fetch_add(1, Ordering::SeqCst);
                });

                panic!("zero-thread closure panic");
            });
        }));

        assert!(res.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn test_scope_heavy_churn_stress() {
    let pool = TaskPool::new();

    for i in 0..100 {
        let task_count = (i % 15) + 1;
        let completed = Arc::new(AtomicUsize::new(0));

        let c = completed.clone();
        let _ = catch_unwind(AssertUnwindSafe(|| {
            pool.scope(|scope| {
                for task_idx in 0..task_count {
                    let c_inner = c.clone();
                    if task_idx % 3 == 0 {
                        scope.spawn(async move {
                            thread::sleep(Duration::from_millis(1));
                            c_inner.fetch_add(1, Ordering::SeqCst);
                            panic!("churn task panic");
                        });
                    } else if task_idx % 3 == 1 {
                        scope.spawn(async move {
                            c_inner.fetch_add(1, Ordering::SeqCst);
                        });
                    } else {
                        scope.spawn_on_scope(async move {
                            c_inner.fetch_add(1, Ordering::SeqCst);
                            if task_idx % 2 == 0 {
                                panic!("churn scope task panic");
                            }
                        });
                    }
                }

                if i % 2 == 0 {
                    panic!("churn closure panic");
                }
            });
        }));

        assert_eq!(
            completed.load(Ordering::SeqCst),
            task_count,
            "All tasks spawned in iteration {} must complete before return",
            i
        );
    }
}

#[test]
fn test_nested_scope_panics() {
    let pool = TaskPool::new();

    for _ in 0..20 {
        let inner_completed = Arc::new(AtomicBool::new(false));
        let outer_completed = Arc::new(AtomicBool::new(false));

        let ic = inner_completed.clone();
        let oc = outer_completed.clone();

        let res = catch_unwind(AssertUnwindSafe(|| {
            pool.scope(|outer_scope| {
                let ic_clone = ic.clone();
                outer_scope.spawn(async move {
                    let inner_res = catch_unwind(AssertUnwindSafe(|| {
                        let pool_inner = TaskPool::new();
                        pool_inner.scope(|inner_scope| {
                            inner_scope.spawn(async {
                                thread::sleep(Duration::from_millis(10));
                                panic!("inner scope task panic");
                            });
                        });
                    }));
                    assert!(inner_res.is_err());
                    ic_clone.store(true, Ordering::SeqCst);
                });

                let oc_clone = oc.clone();
                outer_scope.spawn(async move {
                    thread::sleep(Duration::from_millis(20));
                    oc_clone.store(true, Ordering::SeqCst);
                });

                panic!("outer closure panic");
            });
        }));

        assert!(res.is_err());
        assert!(ic.load(Ordering::SeqCst), "Inner scope must finish");
        assert!(oc.load(Ordering::SeqCst), "Outer task must finish");
    }
}
