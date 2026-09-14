//! Stress tests for `edge_executor` and `task_pool` overflow handling
//! under single-threaded / non-default features (`--no-default-features --features bevy_platform/std`).

extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use bevy_tasks::{ParallelSlice, ParallelSliceMut, TaskPool};
use core::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn test_edge_executor_exact_capacity_64() {
    let pool = TaskPool::new();
    let n = 64;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i * 2 });
        }
    });

    assert_eq!(results.len(), n, "All 64 tasks must produce results");
    let mut sorted = results;
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).map(|i| i * 2).collect();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_overflow_65() {
    let pool = TaskPool::new();
    let n = 65;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i * 3 });
        }
    });

    assert_eq!(results.len(), n, "All 65 tasks must produce results");
    let mut sorted = results;
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).map(|i| i * 3).collect();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_overflow_100() {
    let pool = TaskPool::new();
    let n = 100;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i + 10 });
        }
    });

    assert_eq!(results.len(), n, "All 100 tasks must produce results");
    let mut sorted = results;
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).map(|i| i + 10).collect();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_overflow_200() {
    let pool = TaskPool::new();
    let n = 200;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i * i });
        }
    });

    assert_eq!(results.len(), n, "All 200 tasks must produce results");
    let mut sorted = results;
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).map(|i| i * i).collect();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_overflow_500() {
    let pool = TaskPool::new();
    let n = 500;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i ^ 0xAA });
        }
    });

    assert_eq!(results.len(), n, "All 500 tasks must produce results");
    let mut sorted = results;
    sorted.sort_unstable();
    let mut expected: Vec<usize> = (0..n).map(|i| i ^ 0xAA).collect();
    expected.sort_unstable();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_overflow_1000() {
    let pool = TaskPool::new();
    let n = 1000;
    let results = pool.scope(|scope| {
        for i in 0..n {
            scope.spawn(async move { i });
        }
    });

    assert_eq!(
        results.len(),
        n,
        "All 1000 tasks must produce results without task loss"
    );
    let mut sorted = results;
    sorted.sort_unstable();
    let expected: Vec<usize> = (0..n).collect();
    assert_eq!(sorted, expected);
}

#[test]
fn test_edge_executor_yielding_tasks_overflow() {
    let pool = TaskPool::new();
    let n = 200;
    let completed = Arc::new(AtomicUsize::new(0));

    let results = pool.scope(|scope| {
        for i in 0..n {
            let completed = completed.clone();
            scope.spawn(async move {
                // Yield 5 times to force multiple reschedules between queue and overflow
                for _ in 0..5 {
                    futures_lite::future::yield_now().await;
                }
                completed.fetch_add(1, Ordering::SeqCst);
                i
            });
        }
    });

    assert_eq!(
        completed.load(Ordering::SeqCst),
        n,
        "All yielding tasks must complete"
    );
    assert_eq!(results.len(), n);
}

#[test]
fn test_edge_executor_par_chunk_map_overflow() {
    let pool = TaskPool::new();
    // 20,000 items in chunks of 100 -> 200 tasks spawned concurrently
    let data: Vec<u32> = (0..20_000).collect();
    let mapped = data.par_chunk_map(&pool, 100, |_idx, chunk| {
        chunk.iter().map(|x| x * 2).collect::<Vec<u32>>()
    });

    let flattened: Vec<u32> = mapped.into_iter().flatten().collect();
    let expected: Vec<u32> = (0..20_000).map(|x| x * 2).collect();
    assert_eq!(
        flattened, expected,
        "par_chunk_map results must match exactly"
    );
}

#[test]
fn test_edge_executor_par_chunk_map_mut_overflow() {
    let pool = TaskPool::new();
    // 20,000 items in chunks of 100 -> 200 tasks spawned concurrently
    let mut data: Vec<u32> = (0..20_000).collect();
    let mapped = data.par_chunk_map_mut(&pool, 100, |_idx, chunk| {
        for val in chunk.iter_mut() {
            *val += 10;
        }
        chunk.len()
    });

    assert_eq!(mapped.len(), 200);
    assert_eq!(data[0], 10);
    assert_eq!(data[19_999], 20_009);
    let expected: Vec<u32> = (10..20_010).collect();
    assert_eq!(
        data, expected,
        "par_chunk_map_mut must mutate in place correctly"
    );
}

#[test]
fn test_edge_executor_successive_bursts() {
    let pool = TaskPool::new();
    for iteration in 0..10 {
        let n = 200;
        let results = pool.scope(|scope| {
            for i in 0..n {
                scope.spawn(async move { (iteration, i) });
            }
        });
        assert_eq!(
            results.len(),
            n,
            "Burst {iteration} failed to complete all tasks"
        );
    }
}
