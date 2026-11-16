// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Lock-free program var-root cache shared between `clj-rt` and
//! bitcode helpers (`clj-bc`).
//!
//! Each var slot tracks `(epoch, var_id, Value)`. When no live
//! thread-binding frame anywhere binds `var_id` and no root mutation
//! is active (`reg_epoch` even), compiled var loads (`deref`) clone
//! straight out of `OnceLock` without entering TLS or locks, even
//! while `*ns*`, `*file*`, or `*command-line-args*` remain bound.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::Value;

/// Dedicated exact counters for core + initial user var ids; larger
/// ids share a bounded tail so dynamic checks remain O(1) and lock-free.
const EXACT_BIND_SLOTS: usize = 4096;
const OVERFLOW_BIND_SLOTS: usize = 1024;
const TOTAL_BIND_SLOTS: usize = EXACT_BIND_SLOTS + OVERFLOW_BIND_SLOTS;

/// One binding-refcount table: a fixed-size array, so `slot_index`'s
/// masked indices need no bounds check (the bitcode helpers inline these
/// reads and must not reference `core`'s panic machinery, #260).
type BindSlots = [AtomicUsize; TOTAL_BIND_SLOTS];

/// Process-wide atomic refcounts of active thread bindings per `var_id`.
#[derive(Debug)]
pub struct BindCounts {
    slots: OnceLock<Box<BindSlots>>,
}

impl Default for BindCounts {
    fn default() -> Self {
        Self::new()
    }
}

impl BindCounts {
    /// Allocate an empty binding-refcount table; the 40 KB slot array is
    /// materialized lazily on the first dynamic binding push.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: OnceLock::new(),
        }
    }

    #[cold]
    fn alloc_slots() -> Box<BindSlots> {
        Box::new([const { AtomicUsize::new(0) }; TOTAL_BIND_SLOTS])
    }

    #[inline]
    fn slot_index(var_id: u64) -> usize {
        let id = var_id as usize;
        if id < EXACT_BIND_SLOTS {
            id
        } else {
            EXACT_BIND_SLOTS + (id & (OVERFLOW_BIND_SLOTS - 1))
        }
    }

    /// Whether any thread currently holds a binding entry for `var_id`.
    #[inline]
    #[must_use]
    pub fn is_bound(&self, var_id: u64) -> bool {
        let Some(slots) = self.slots.get() else {
            return false;
        };
        slots[Self::slot_index(var_id)].load(Ordering::Acquire) != 0
    }

    /// Record `n` added binding entries for `var_id`.
    #[inline]
    pub fn add(&self, var_id: u64, n: usize) {
        if n == 0 {
            return;
        }
        let slots = self.slots.get_or_init(Self::alloc_slots);
        slots[Self::slot_index(var_id)].fetch_add(n, Ordering::AcqRel);
    }

    /// Release `n` removed binding entries for `var_id`.
    #[inline]
    pub fn sub(&self, var_id: u64, n: usize) {
        if n == 0 {
            return;
        }
        if let Some(slots) = self.slots.get() {
            slots[Self::slot_index(var_id)].fetch_sub(n, Ordering::Release);
        }
    }
}

/// Cached snapshots of one var slot's root value.
#[derive(Default)]
pub struct FastVarSlot {
    first: OnceLock<(u64, u64, Value)>,
    latest: OnceLock<(u64, u64, Value)>,
}

impl FastVarSlot {
    #[inline]
    fn get(&self, epoch: u64) -> Option<(u64, &Value)> {
        if let Some((e, id, v)) = self.first.get() {
            if *e == epoch {
                return Some((*id, v));
            }
        }
        if let Some((e, id, v)) = self.latest.get() {
            if *e == epoch {
                return Some((*id, v));
            }
        }
        None
    }

    fn record(&self, epoch: u64, var_id: u64, val: &Value) {
        if let Some((e, _, _)) = self.first.get() {
            if *e == epoch {
                return;
            }
        }
        let entry = (epoch, var_id, val.clone());
        if let Err(entry) = self.first.set(entry) {
            let _ = self.latest.set(entry);
        }
    }
}

/// Program-wide fast var-root table leaked by `clj-rt` after constant
/// initialization freezes `Tables`.
pub struct FastVars {
    reg_epoch: Arc<AtomicU64>,
    bind_counts: Arc<BindCounts>,
    slots: Box<[FastVarSlot]>,
}

impl FastVars {
    /// Allocate a fast table covering `len` declared var slots.
    #[must_use]
    pub fn new(reg_epoch: Arc<AtomicU64>, bind_counts: Arc<BindCounts>, len: usize) -> Self {
        let slots = (0..len)
            .map(|_| FastVarSlot::default())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            reg_epoch,
            bind_counts,
            slots,
        }
    }

    /// Borrow slot `k`'s root when no dynamic binding covers that var
    /// and the cached root matches the current registry generation.
    #[inline]
    #[must_use]
    pub fn get(&self, k: usize) -> Option<&Value> {
        let epoch = self.reg_epoch.load(Ordering::Acquire);
        if epoch & 1 != 0 {
            return None;
        }
        let (var_id, val) = self.slots.get(k)?.get(epoch)?;
        if self.bind_counts.is_bound(var_id) {
            return None;
        }
        Some(val)
    }

    /// Record a stable root observed when that var is unbound and `epoch`
    /// held unchanged across the read.
    pub fn record(&self, k: usize, epoch: u64, var_id: u64, val: &Value) {
        if epoch & 1 != 0
            || matches!(val, Value::Unbound(_))
            || self.bind_counts.is_bound(var_id)
            || self.reg_epoch.load(Ordering::Acquire) != epoch
        {
            return;
        }
        if let Some(slot) = self.slots.get(k) {
            slot.record(epoch, var_id, val);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BindCounts, EXACT_BIND_SLOTS, FastVars};
    use crate::{Symbol, Unbound, Value};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn fast_vars_hits_and_invalidates_on_epoch_and_bindings() {
        let reg_epoch = Arc::new(AtomicU64::new(0));
        let bind_counts = Arc::new(BindCounts::default());
        let table = FastVars::new(Arc::clone(&reg_epoch), Arc::clone(&bind_counts), 2);

        assert_eq!(table.get(0), None);
        assert_eq!(table.get(99), None);
        table.record(99, 0, 7, &Value::Int(10));
        assert_eq!(table.get(99), None);

        // Odd epoch (mutation in progress) neither records nor serves.
        table.record(0, 1, 7, &Value::Int(10));
        reg_epoch.store(1, Ordering::Release);
        assert_eq!(table.get(0), None);

        // Unbound sentinel is never cached.
        reg_epoch.store(2, Ordering::Release);
        table.record(
            0,
            2,
            7,
            &Value::Unbound(Unbound {
                sym: Symbol::new(None, "u"),
                var_id: 7,
            }),
        );
        assert_eq!(table.get(0), None);

        // Normal recording hits at the same epoch, even when an unrelated
        // var (e.g. `*ns*`) has active bindings.
        bind_counts.add(99, 1);
        table.record(0, 2, 7, &Value::Int(10));
        assert_eq!(table.get(0), Some(&Value::Int(10)));
        // Duplicate record at the same epoch is a no-op (keeps `latest` free).
        table.record(0, 2, 7, &Value::Int(10));
        assert_eq!(table.get(0), Some(&Value::Int(10)));

        // Active dynamic binding on the cached var blocks recording & hits.
        bind_counts.add(7, 1);
        assert_eq!(table.get(0), None);
        table.record(1, 2, 8, &Value::Int(30));
        bind_counts.sub(7, 1);
        assert_eq!(table.get(0), Some(&Value::Int(10)));

        // Epoch change invalidates the old entry; `latest` absorbs one update.
        reg_epoch.store(4, Ordering::Release);
        assert_eq!(table.get(0), None);
        table.record(0, 4, 7, &Value::Int(20));
        assert_eq!(table.get(0), Some(&Value::Int(20)));
        // Stale epoch record is ignored.
        table.record(1, 2, 8, &Value::Int(30));
        assert_eq!(table.get(1), None);
        reg_epoch.store(6, Ordering::Release);
        assert_eq!(table.get(0), None);

        // Large var ids (>= EXACT_BIND_SLOTS) map into the overflow tail.
        let large_id = EXACT_BIND_SLOTS as u64 + 17;
        assert!(!bind_counts.is_bound(large_id));
        bind_counts.add(large_id, 1);
        assert!(bind_counts.is_bound(large_id));
        bind_counts.sub(large_id, 1);
        assert!(!bind_counts.is_bound(large_id));
    }

    #[test]
    fn bind_counts_lazy_and_noop_guards() {
        let bind_counts = BindCounts::new();
        // Zero-count and uninitialized sub are harmless no-ops that keep the
        // 40 KB table unallocated.
        bind_counts.add(3, 0);
        bind_counts.sub(3, 0);
        bind_counts.sub(3, 1);
        assert!(!bind_counts.is_bound(3));
        bind_counts.add(3, 2);
        assert!(bind_counts.is_bound(3));
        bind_counts.sub(3, 2);
        assert!(!bind_counts.is_bound(3));
    }
}
