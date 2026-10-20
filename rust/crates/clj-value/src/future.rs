// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Futures (S6B): shared outcome cells mirroring the
//! `future-call` reify (`docs/t1174/concurrency-contract.md`,
//! `docs/t1174/s6-pins.md` P3, `/tmp/s6b-probe*.clj`).
//!
//! A future starts [`FutureState::Pending`]; its worker thread
//! publishes one [`FutureOutcome`] (a value or the body's cached
//! failure), and any thread may [`cancel`](Future::cancel) it while
//! pending. Publishing and cancelling race under one lock: the first
//! wins, the loser observes the winner (a cancelled future's late
//! body result is discarded, like `FutureTask` after `cancel`).
//! `deref` blocks on the condition variable (optionally with a
//! timeout); `future-done?`/`realized?` report done-ness, which —
//! like `Future.isDone` — includes cancellation.
//!
//! Only the body's failure is cached here; the `ExecutionException`
//! wrapper is rebuilt fresh on every `deref` (the JVM's `FutureTask`
//! constructs one per `get`, so wrappers are never `identical?`
//! while their causes are). Equality is shared-cell identity plus
//! equal meta, and printing follows the transient `#object[…]`
//! convention. Metadata
//! is a plain per-handle field (the reify's `IObj` meta is immutable
//! and per-object: `with-meta` returns a new handle sharing the
//! outcome cell, never mutating — futures are not `IReference`s, so
//! no in-place meta mutation exists).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{Map, Throwable, Value};

/// Fresh future identities (print-only; equality is shared-cell).
static FUTURE_ID: AtomicU64 = AtomicU64::new(1);

/// A future body's published outcome: its value, or its cached
/// failure (rethrown — freshly wrapped per `deref` — on every
/// later `deref`, never re-run).
#[derive(Clone, Debug)]
pub enum FutureOutcome {
    /// The body's value.
    Value(Value),
    /// The body's cached failure (the `ExecutionException` cause).
    Failed(Throwable),
}

/// The cell state: pending, completed, or cancelled (a late
/// completion never overwrites a cancellation).
#[derive(Clone, Debug)]
pub enum FutureState {
    /// The body has not published yet.
    Pending,
    /// The published outcome.
    Done(FutureOutcome),
    /// Cancelled while pending (a late body result is discarded).
    Cancelled,
}

/// The shared cell behind a [`Future`]: the outcome state plus its
/// wait queue.
#[derive(Debug)]
struct FutureCell {
    state: Mutex<FutureState>,
    ready: Condvar,
}

/// A first-class future value: an identity plus the shared outcome
/// cell (and the per-handle metadata). Identity equality (same
/// cell, equal meta); `counted?` is false, like the JVM.
#[derive(Clone, Debug)]
pub struct Future {
    id: u64,
    cell: Arc<FutureCell>,
    meta: Option<Map>,
}

impl Future {
    /// A pending future (fresh identity, no metadata).
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: FUTURE_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(FutureCell {
                state: Mutex::new(FutureState::Pending),
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

    /// Whether `other` shares this future's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only; the body runs
    /// outside the lock — poison is unreachable).
    fn lock(&self) -> MutexGuard<'_, FutureState> {
        self.cell.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish `outcome` unless already done or cancelled (a
    /// cancellation wins over a late body result). Returns whether
    /// this call published, waking all waiters on success.
    pub fn complete(&self, outcome: FutureOutcome) -> bool {
        let mut state = self.lock();
        if !matches!(*state, FutureState::Pending) {
            return false;
        }
        *state = FutureState::Done(outcome);
        self.cell.ready.notify_all();
        true
    }

    /// Cancel while pending (a completed or already-cancelled
    /// future stays as it is). Returns whether this call
    /// cancelled, waking all waiters on success.
    pub fn cancel(&self) -> bool {
        let mut state = self.lock();
        if !matches!(*state, FutureState::Pending) {
            return false;
        }
        *state = FutureState::Cancelled;
        self.cell.ready.notify_all();
        true
    }

    /// Whether the future is done — completed OR cancelled (like
    /// `Future.isDone`, which `future-done?` and `realized?`
    /// report).
    #[must_use]
    pub fn is_done(&self) -> bool {
        !matches!(*self.lock(), FutureState::Pending)
    }

    /// Block until the outcome or cancellation arrives, returning
    /// the settled state (never [`FutureState::Pending`]).
    pub fn get(&self) -> FutureState {
        let mut state = self.lock();
        while matches!(*state, FutureState::Pending) {
            state = self
                .cell
                .ready
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
        state.clone()
    }

    /// Block up to `timeout` for the outcome: `Some` settled state
    /// when it arrives in time (never [`FutureState::Pending`]),
    /// `None` on timeout (the `deref` timeout value). A spurious
    /// wakeup re-waits only the remaining span (against one
    /// deadline), never a fresh full timeout.
    pub fn get_timeout(&self, timeout: Duration) -> Option<FutureState> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        while matches!(*state, FutureState::Pending) {
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
            if result.timed_out() && matches!(*state, FutureState::Pending) {
                return None;
            }
        }
        Some(state.clone())
    }

    /// The metadata map, if any (`with-meta` installs it; the
    /// outcome cell is untouched).
    #[must_use]
    pub fn meta(&self) -> Option<Map> {
        self.meta.clone()
    }

    /// This handle with `meta` attached (`None` clears it): the
    /// identity and outcome cell are shared (like the JVM reify's
    /// `withMeta` field copy).
    #[must_use]
    pub fn with_meta(&self, meta: Option<Map>) -> Self {
        Self {
            id: self.id,
            cell: Arc::clone(&self.cell),
            meta,
        }
    }

    /// The JVM class name (the `future-call` reify class, minus its
    /// per-build numeric suffix).
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.core$future_call$reify"
    }
}

impl Default for Future {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Future;
    use std::time::Duration;

    #[test]
    fn default_is_a_pending_future() {
        let future = Future::default();
        assert!(!future.is_done());
    }

    #[test]
    fn zero_timeout_returns_none_at_once() {
        let future = Future::new();
        assert!(future.get_timeout(Duration::ZERO).is_none());
    }
}
