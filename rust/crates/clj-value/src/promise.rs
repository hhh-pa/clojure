// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Promises (S6B): single-shot latches mirroring the `promise` reify
//! (`docs/t1174/concurrency-contract.md`, `docs/t1174/s6-pins.md` P4,
//! `/tmp/s6b-probe*.clj`).
//!
//! A promise starts [`PromiseState::Pending`]; the first
//! [`deliver`](Promise::deliver) publishes the value (even `nil` —
//! delivery is a state transition, not a value check) and releases
//! all `deref` waiters, while later deliveries are no-ops. Promises
//! are `IDeref`/`IBlockingDeref`/`IPending`/`IFn` (one arg) but NOT
//! `java.util.concurrent.Future`s: `future?` is false of them and
//! `future-done?`/`future-cancel` fail the `Future` cast. Equality
//! is shared-cell identity plus equal meta, and printing follows
//! the transient
//! `#object[…]` convention. Metadata is a plain per-handle field
//! (see [`Future`](crate::Future): `with-meta` shares the latch).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{Map, Value};

/// Fresh promise identities (print-only; equality is shared-cell).
static PROMISE_ID: AtomicU64 = AtomicU64::new(1);

/// The latch state: pending, or the delivered value (a delivered
/// `nil` is still delivered).
#[derive(Clone, Debug)]
pub enum PromiseState {
    /// No delivery yet.
    Pending,
    /// The first delivery won.
    Delivered(Value),
}

/// The shared cell behind a [`Promise`]: the latch state plus its
/// wait queue.
#[derive(Debug)]
struct PromiseCell {
    state: Mutex<PromiseState>,
    ready: Condvar,
}

/// A first-class promise value: an identity plus the shared latch
/// cell (and the per-handle metadata). Identity equality (same
/// cell, equal meta); `counted?` is false, like the JVM.
#[derive(Clone, Debug)]
pub struct Promise {
    id: u64,
    cell: Arc<PromiseCell>,
    meta: Option<Map>,
}

impl Promise {
    /// A pending promise (fresh identity, no metadata).
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: PROMISE_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(PromiseCell {
                state: Mutex::new(PromiseState::Pending),
                ready: Condvar::new(),
            }),
            meta: None,
        }
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this promise's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only — poison is
    /// unreachable).
    fn lock(&self) -> MutexGuard<'_, PromiseState> {
        self.cell.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Deliver `value` if still pending (first wins; a delivered
    /// `nil` still realizes). Returns whether this call delivered,
    /// waking all waiters on success.
    pub fn deliver(&self, value: Value) -> bool {
        let mut state = self.lock();
        if !matches!(*state, PromiseState::Pending) {
            return false;
        }
        *state = PromiseState::Delivered(value);
        self.cell.ready.notify_all();
        true
    }

    /// Whether a value was delivered.
    #[must_use]
    pub fn is_realized(&self) -> bool {
        matches!(*self.lock(), PromiseState::Delivered(_))
    }

    /// Block until a delivery arrives, returning the delivered
    /// value.
    pub fn get(&self) -> Value {
        let mut state = self.lock();
        while let PromiseState::Pending = *state {
            state = self
                .cell
                .ready
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        match state.clone() {
            PromiseState::Delivered(v) => v,
            PromiseState::Pending => unreachable!("waited out of pending"),
        }
    }

    /// Block up to `timeout` for a delivery: `Some` value when it
    /// arrives in time, `None` on timeout (the `deref` timeout
    /// value). A spurious wakeup re-waits only the remaining span
    /// (against one deadline), never a fresh full timeout.
    pub fn get_timeout(&self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while let PromiseState::Pending = *state {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (next, result) = self
                .cell
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
            if result.timed_out() && matches!(*state, PromiseState::Pending) {
                return None;
            }
        }
        match state.clone() {
            PromiseState::Delivered(v) => Some(v),
            PromiseState::Pending => unreachable!("waited out of pending"),
        }
    }

    /// The metadata map, if any (`with-meta` installs it; the latch
    /// cell is untouched).
    #[must_use]
    pub fn meta(&self) -> Option<Map> {
        self.meta.clone()
    }

    /// This handle with `meta` attached (`None` clears it): the
    /// identity and latch cell are shared (like the JVM reify's
    /// `withMeta` field copy).
    #[must_use]
    pub fn with_meta(&self, meta: Option<Map>) -> Self {
        Self {
            id: self.id,
            cell: Arc::clone(&self.cell),
            meta,
        }
    }

    /// The JVM class name (the `promise` reify class, minus its
    /// per-build numeric suffix).
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.core$promise$reify"
    }
}

impl Default for Promise {
    fn default() -> Self {
        Self::new()
    }
}
