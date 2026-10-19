// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Delays (S6A): run-once thunks mirroring `clojure.lang.Delay`
//! (`docs/t1174/concurrency-contract.md`, `s6-pins.md` P1).
//!
//! A delay starts [`DelayState::Pending`] over its zero-arg fn (the
//! `delay*` evaluator form traps the body as a closure, so forcing is
//! a plain call — no evaluator types cross this crate). The first
//! force runs the body and caches the outcome: values realize and
//! failures cache too (throwing still realizes, and rethrows the
//! same failure on every later force — never re-running). Equality
//! is shared-cell identity, and printing follows the transient
//! `#object[…]` convention (never forcing, unlike the JVM's
//! `deref-as-map` rendering — see the concurrency contract).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{Throwable, Value};

/// Fresh delay identities (print-only; equality is shared-cell).
static DELAY_ID: AtomicU64 = AtomicU64::new(1);

/// The memoized state: a pending fn becomes a cached value or failure.
#[derive(Clone, Debug)]
pub enum DelayState {
    /// Unforced: the zero-arg fn value to call.
    Pending(Value),
    /// A force is in progress on this thread (the body re-entered
    /// its own delay — the JVM recurses into `StackOverflowError`;
    /// the port reports a clean `IllegalStateException` instead).
    Forcing,
    /// The forced value.
    Realized(Value),
    /// The cached failure (rethrown on every force; still realized).
    Failed(Throwable),
}

/// A first-class delay value: an identity plus the shared cell.
/// Identity equality (same cell).
#[derive(Clone, Debug)]
pub struct Delay {
    id: u64,
    cell: Arc<Mutex<DelayState>>,
}

impl Delay {
    /// A delay over the zero-arg fn value `f` (fresh identity,
    /// unrealized).
    #[must_use]
    pub fn new(f: Value) -> Self {
        Self {
            id: DELAY_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(Mutex::new(DelayState::Pending(f))),
        }
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this delay's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only; the body runs
    /// outside the lock — poison is unreachable).
    pub fn lock(&self) -> MutexGuard<'_, DelayState> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the delay has produced its outcome — a value OR a
    /// cached failure (throwing still realizes, like the JVM's
    /// nulled lock).
    #[must_use]
    pub fn is_realized(&self) -> bool {
        matches!(
            *self.lock(),
            DelayState::Realized(_) | DelayState::Failed(_)
        )
    }

    /// The JVM class name.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.lang.Delay"
    }
}
