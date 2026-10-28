// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Refs (S19): STM shared cells mirroring `clojure.lang.Ref`
//! (`docs/zero-oos-s19-stm/specification.md` §1a).
//!
//! The cell holds the current value plus its commit point, the
//! history chain (`(point, value)` pairs, newest first — the MVCC
//! snapshot source), the `min_history`/`max_history` bounds (always
//! int-range by construction — the §1a conversion), the `:validator`
//! fn (if any), the `:meta` map (if any), and the watches map (`{key
//! fn}`, fired in insertion order). Equality is shared-cell identity
//! plus equal meta (like atoms); printing follows the transient
//! `#object[…]` convention (never the JVM's `{:status :ready, :val
//! …}` rendering — see the S19 spec §2f).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{Map, Value};

/// Fresh ref identities (print-only; equality is shared-cell).
static REF_ID: AtomicU64 = AtomicU64::new(1);

/// The mutable cell behind a [`Ref`]: the current value and its
/// commit point, the history chain, the history bounds, and the
/// validator, metadata, and watches.
#[derive(Clone, Debug)]
pub struct RefState {
    /// The current (latest committed) value.
    pub value: Value,
    /// The commit point of [`RefState::value`] (fresh refs read 0 —
    /// every start point resolves them without a retry).
    pub point: u64,
    /// The history chain, newest first (`(point, value)` pairs —
    /// snapshot reads walk this past the current value).
    pub history: VecDeque<(u64, Value)>,
    /// The retained history floor (the commit trim keeps this many).
    pub min_history: i64,
    /// The pinned-reader retention bound (past this the commit trim
    /// force-drops, and pinned readers retry).
    pub max_history: i64,
    /// The validator fn (`None` = none installed — falsy `:validator`
    /// options never install, like the JVM's `setup-reference`).
    pub validator: Option<Value>,
    /// The metadata map (`None` = none).
    pub meta: Option<Map>,
    /// Watch fns keyed opaquely (nil fns install but never fire, like
    /// the JVM's `notifyWatches` null check).
    pub watches: Map,
}

/// A first-class ref value: an identity plus the shared cell.
/// Identity equality (same cell, equal meta); `counted?` is false
/// and `ifn?` is true, like the JVM.
#[derive(Clone, Debug)]
pub struct Ref {
    id: u64,
    cell: Arc<Mutex<RefState>>,
}

impl Ref {
    /// A ref over `value` (fresh identity, point 0, empty history,
    /// default bounds 0/10, no validator, no meta, no watches).
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self {
            id: REF_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(Mutex::new(RefState {
                value,
                point: 0,
                history: VecDeque::new(),
                min_history: 0,
                max_history: 10,
                validator: None,
                meta: None,
                watches: crate::new_map(),
            })),
        }
    }

    /// The print identity (also the commit lock order).
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this ref's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections outside commit; the
    /// commit path holds the write set across its steps — poison is
    /// unreachable outside a panicking holder, and recovers).
    pub fn lock(&self) -> MutexGuard<'_, RefState> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Try the cell once for the commit spin phase (`None` on
    /// contention; a poisoned cell recovers like [`Ref::lock`]).
    pub fn try_lock(&self) -> Option<MutexGuard<'_, RefState>> {
        match self.cell.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(err)) => Some(err.into_inner()),
        }
    }

    /// The metadata map, if any (cloned from behind the lock, for
    /// `meta` and `*print-meta*`).
    #[must_use]
    pub fn meta(&self) -> Option<Map> {
        self.lock().meta.clone()
    }

    /// The JVM class name.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.lang.Ref"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ref_reads_defaults() {
        let r = Ref::new(Value::Int(1));
        let state = r.lock();
        assert_eq!(state.point, 0);
        assert!(state.history.is_empty());
        assert_eq!(state.min_history, 0);
        assert_eq!(state.max_history, 10);
        assert!(state.validator.is_none());
        assert!(state.meta.is_none());
        assert_eq!(r.jvm_class(), "clojure.lang.Ref");
        assert!(r.same_cell(&r));
        assert!(!r.same_cell(&Ref::new(Value::Int(1))));
    }

    #[test]
    fn try_lock_contention_reads_none() {
        // A held cell refuses the spin-phase try (the commit
        // spin-then-barge test in `clj-eval` covers the retry).
        let r = Ref::new(Value::Int(0));
        let _held = r.lock();
        assert!(r.try_lock().is_none());
    }

    #[test]
    fn poisoned_cells_recover() {
        // A panicking holder poisons the mutex; both lock paths
        // recover the cell (the `Retry` unwind path relies on this).
        let r = Ref::new(Value::Int(7));
        let poisoned = r.clone();
        let _ = std::panic::catch_unwind(move || {
            let _held = poisoned.lock();
            panic!("poison the ref cell");
        });
        assert_eq!(r.lock().value, Value::Int(7));
        assert_eq!(
            r.try_lock().expect("poisoned try recovers").value,
            Value::Int(7)
        );
    }
}
