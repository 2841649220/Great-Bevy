//! Adversarial stress tests for `EntityIndex` and `Entity` sparse set indexing
//! at boundary values, testing roundtrip invariants, panic safety, and `MockSparseArray` integration.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;
use std::panic::catch_unwind;

use bevy_ecs::entity::{Entity, EntityIndex};
use bevy_ecs::storage::SparseSetIndex;

#[derive(Default)]
struct MockSparseArray<I: SparseSetIndex, V> {
    values: Vec<Option<V>>,
    _marker: PhantomData<I>,
}

impl<I: SparseSetIndex, V> MockSparseArray<I, V> {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            _marker: PhantomData,
        }
    }

    fn insert(&mut self, index: I, value: V) {
        let idx = index.sparse_set_index();
        if idx >= self.values.len() {
            self.values.resize_with(idx + 1, || None);
        }
        self.values[idx] = Some(value);
    }

    fn get(&self, index: I) -> Option<&V> {
        let idx = index.sparse_set_index();
        self.values.get(idx).and_then(Option::as_ref)
    }

    fn contains(&self, index: I) -> bool {
        let idx = index.sparse_set_index();
        self.values.get(idx).is_some_and(Option::is_some)
    }

    fn remove(&mut self, index: I) -> Option<V> {
        let idx = index.sparse_set_index();
        self.values.get_mut(idx).and_then(Option::take)
    }

    fn iter(&self) -> impl Iterator<Item = (I, &V)> {
        self.values
            .iter()
            .enumerate()
            .filter_map(|(idx, val)| val.as_ref().map(|v| (I::get_sparse_set_index(idx), v)))
    }
}

#[test]
fn test_sparse_set_index_boundary_values() {
    let boundary_values: Vec<usize> = vec![
        0,
        1,
        2,
        3,
        4,
        7,
        8,
        15,
        16,
        31,
        32,
        63,
        64,
        127,
        128,
        255,
        256,
        511,
        512,
        1023,
        1024,
        4095,
        4096,
        32767,
        32768,
        65535,
        65536,
        (1 << 24) - 1,
        1 << 24,
        (1 << 30) - 1,
        1 << 30,
        (1 << 31) - 1,
        1 << 31,
        (u32::MAX - 3) as usize,
        (u32::MAX - 2) as usize,
        (u32::MAX - 1) as usize, // 0xFFFF_FFFE (maximum valid NonMaxU32)
    ];

    for &val in &boundary_values {
        // EntityIndex invariant
        let idx = EntityIndex::get_sparse_set_index(val);
        assert_eq!(
            idx.sparse_set_index(),
            val,
            "EntityIndex roundtrip failed for boundary value {val:#X}"
        );
        assert_eq!(
            idx.index(),
            val as u32,
            "EntityIndex index() mismatch for boundary value {val:#X}"
        );

        // Entity invariant
        let e = Entity::get_sparse_set_index(val);
        assert_eq!(
            e.sparse_set_index(),
            val,
            "Entity roundtrip failed for boundary value {val:#X}"
        );
        assert_eq!(
            e.index_u32(),
            val as u32,
            "Entity index_u32() mismatch for boundary value {val:#X}"
        );
        assert_eq!(
            e.index(),
            idx,
            "Entity index() must equal EntityIndex for boundary value {val:#X}"
        );
    }
}

#[test]
fn test_sparse_set_index_invalid_boundaries_panic_cleanly() {
    // 1. u32::MAX (0xFFFF_FFFF) is the niche value and must never be a valid EntityIndex
    assert_eq!(EntityIndex::from_raw_u32(u32::MAX), None);
    assert_eq!(Entity::from_raw_u32(u32::MAX), None);

    let panic_idx = catch_unwind(|| EntityIndex::get_sparse_set_index(u32::MAX as usize));
    assert!(
        panic_idx.is_err(),
        "EntityIndex::get_sparse_set_index(u32::MAX) must cleanly panic"
    );

    let panic_e = catch_unwind(|| Entity::get_sparse_set_index(u32::MAX as usize));
    assert!(
        panic_e.is_err(),
        "Entity::get_sparse_set_index(u32::MAX) must cleanly panic"
    );

    // 2. usize::MAX on 64-bit architectures truncates to u32::MAX when cast as u32,
    // which must also panic cleanly
    let panic_usize_max_idx = catch_unwind(|| EntityIndex::get_sparse_set_index(usize::MAX));
    assert!(
        panic_usize_max_idx.is_err(),
        "EntityIndex::get_sparse_set_index(usize::MAX) must cleanly panic"
    );

    let panic_usize_max_e = catch_unwind(|| Entity::get_sparse_set_index(usize::MAX));
    assert!(
        panic_usize_max_e.is_err(),
        "Entity::get_sparse_set_index(usize::MAX) must cleanly panic"
    );
}

#[test]
fn test_inverted_bits_regression_oracle() {
    // Under the old bug, EntityIndex::from_bits(1) produced inverted index !1 (0xFFFF_FFFE).
    // And EntityIndex::from_bits(0) panicked on 0.
    // Verify that the old bug cannot silently return:
    let idx0 = EntityIndex::get_sparse_set_index(0);
    assert_eq!(idx0.index(), 0);
    assert_eq!(idx0.sparse_set_index(), 0);

    let idx1 = EntityIndex::get_sparse_set_index(1);
    assert_eq!(idx1.index(), 1);
    assert_ne!(idx1.index(), !1u32);
    assert_eq!(idx1.sparse_set_index(), 1);

    let e0 = Entity::get_sparse_set_index(0);
    assert_eq!(e0.index_u32(), 0);
    assert_eq!(e0.sparse_set_index(), 0);

    let e1 = Entity::get_sparse_set_index(1);
    assert_eq!(e1.index_u32(), 1);
    assert_ne!(e1.index_u32(), !1u32);
    assert_eq!(e1.sparse_set_index(), 1);
}

#[test]
fn test_sparse_array_integration_with_entity_index() {
    let mut array = MockSparseArray::<EntityIndex, u64>::new();
    let test_indices = [0, 1, 2, 5, 10, 42, 100, 255, 1024, 4096, 20_000];

    for &idx_val in &test_indices {
        let idx = EntityIndex::get_sparse_set_index(idx_val);
        array.insert(idx, (idx_val as u64) * 100);
    }

    // Verify contains and get
    for &idx_val in &test_indices {
        let idx = EntityIndex::get_sparse_set_index(idx_val);
        assert!(array.contains(idx));
        assert_eq!(array.get(idx), Some(&((idx_val as u64) * 100)));
    }

    // Verify that non-existent indices return None and false
    for non_existent in [3, 4, 6, 99, 500, 10000] {
        let idx = EntityIndex::get_sparse_set_index(non_existent);
        assert!(!array.contains(idx));
        assert_eq!(array.get(idx), None);
    }

    // Verify iter() which uses SparseSetIndex::get_sparse_set_index internally
    let mut collected: BTreeMap<usize, u64> = BTreeMap::new();
    for (idx, val) in array.iter() {
        collected.insert(idx.sparse_set_index(), *val);
    }

    assert_eq!(collected.len(), test_indices.len());
    for &idx_val in &test_indices {
        assert_eq!(
            collected.get(&idx_val),
            Some(&((idx_val as u64) * 100)),
            "iter() must yield exact key for index {idx_val}"
        );
    }

    // Test removal
    let remove_target = EntityIndex::get_sparse_set_index(42);
    assert_eq!(array.remove(remove_target), Some(4200));
    assert!(!array.contains(remove_target));
    assert_eq!(array.get(remove_target), None);
}

#[test]
fn test_sparse_array_integration_with_entity() {
    let mut array = MockSparseArray::<Entity, String>::new();
    let test_indices = [0, 1, 7, 64, 128, 500, 10_000];

    for &idx_val in &test_indices {
        let e = Entity::get_sparse_set_index(idx_val);
        array.insert(e, format!("Entity_{idx_val}"));
    }

    for &idx_val in &test_indices {
        let e = Entity::get_sparse_set_index(idx_val);
        assert!(array.contains(e));
        assert_eq!(array.get(e), Some(&format!("Entity_{idx_val}")));
    }

    // Verify iter() yields proper Entity keys
    let mut count = 0;
    for (e, val) in array.iter() {
        assert_eq!(e.sparse_set_index(), e.index_u32() as usize);
        assert_eq!(val, &format!("Entity_{}", e.sparse_set_index()));
        count += 1;
    }
    assert_eq!(count, test_indices.len());
}

#[test]
fn test_randomized_roundtrip_oracle() {
    // LCG pseudo-random generator to test 10,000 diverse valid values without external rand crate
    let mut state: u64 = 0xDEADBEEF_CAFE1234;
    let max_valid = u32::MAX - 1;

    for _ in 0..10_000 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let raw = (state >> 32) as u32;
        let val = (raw % max_valid) as usize;

        let idx = EntityIndex::get_sparse_set_index(val);
        assert_eq!(idx.sparse_set_index(), val);
        assert_eq!(idx.index(), val as u32);

        let e = Entity::get_sparse_set_index(val);
        assert_eq!(e.sparse_set_index(), val);
        assert_eq!(e.index_u32(), val as u32);
    }
}
