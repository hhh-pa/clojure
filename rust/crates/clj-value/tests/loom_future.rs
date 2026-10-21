// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Loom model of the future/promise outcome-cell protocol (`Future`
//! in `src/future.rs`, `Promise` in `src/promise.rs`): the
//! single-lock first-wins publish discipline (`complete` vs
//! `cancel`, racing `deliver`s), the blocking `get` wait loop, and
//! the `get_timeout` wake-not-timeout back-edge — the
//! `timed_out() && pending` re-check (`future.rs`) / loop-head
//! delivery re-test (`promise.rs`) that keeps a woken waiter from
//! reporting a timeout, pinned here by construction instead of the
//! sleeper timing in `woken_wait_returns_delivery`.
//!
//! Loom has no clock, so the timeout leg is a racing timer thread:
//! expiry is a flag set under the cell lock plus a wakeup, exactly
//! like a publish. That preserves the race skeleton the back-edge
//! pins (publish vs expiry vs the waiter's check order) while the
//! real deadline mechanics stay covered by the in-module tests
//! (`zero_timeout_returns_none_at_once`,
//! `timeout_on_pending_promise_returns_none`). The back-edge
//! property reads linearization order off tickets taken under the
//! lock: a `None` return must linearize before any winning publish.
//! The expiry-first variant (returning `None` on expiry without
//! re-checking state) violates that property on the
//! publish-then-expire-then-check schedule.
//!
//! Run with the rest of the suite (`cargo test -p clj-value
//! --offline`); loom explores every schedule of each model.

use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;

/// Mirror of `FutureState` (values simplified to `u64` — payloads
/// never steer the protocol; `PromiseState` is this without
/// `Cancelled`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// No outcome yet.
    Pending,
    /// The published outcome.
    Done(u64),
    /// Cancelled while pending.
    Cancelled,
}

/// Mirror of `FutureCell`/`PromiseCell`: the outcome state plus its
/// wait queue.
struct Cell {
    state: Mutex<State>,
    ready: Condvar,
}

impl Cell {
    fn pending() -> Self {
        Self {
            state: Mutex::new(State::Pending),
            ready: Condvar::new(),
        }
    }

    /// Mirror of `Future::complete` / `Promise::deliver`: first
    /// publisher wins, waking every waiter.
    fn publish(&self, value: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if !matches!(*state, State::Pending) {
            return false;
        }
        *state = State::Done(value);
        self.ready.notify_all();
        true
    }

    /// Mirror of `Future::cancel`: cancels while pending, waking
    /// every waiter.
    fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if !matches!(*state, State::Pending) {
            return false;
        }
        *state = State::Cancelled;
        self.ready.notify_all();
        true
    }

    /// Mirror of `Future::get` / `Promise::get`: the predicate wait
    /// loop (a missed wakeup would deadlock a schedule, failing the
    /// model).
    fn get(&self) -> State {
        let mut state = self.state.lock().unwrap();
        while matches!(*state, State::Pending) {
            state = self.ready.wait(state).unwrap();
        }
        state.clone()
    }
}

/// A publish that never happened (the waiter's initial view).
const NEVER: u64 = u64::MAX;

/// Mirror of the `get_timeout` race skeleton: the outcome, the
/// expiry flag, and the linearization tickets — all under the one
/// cell lock, exactly like the real `FutureCell`.
struct TimedState {
    /// The published outcome (`None` while pending).
    outcome: Option<u64>,
    /// The timer thread fired (the model's clock).
    expired: bool,
    /// Ticket of the winning publish (`NEVER` if none yet).
    published_at: u64,
    /// Next linearization ticket.
    next_ticket: u64,
    /// The waiter's final pre-return ticket.
    check_at: u64,
    /// `published_at` as of that check.
    check_saw: u64,
}

/// Mirror of the cell behind `get_timeout`.
struct TimeoutCell {
    state: Mutex<TimedState>,
    ready: Condvar,
}

impl TimeoutCell {
    fn pending() -> Self {
        Self {
            state: Mutex::new(TimedState {
                outcome: None,
                expired: false,
                published_at: NEVER,
                next_ticket: 0,
                check_at: NEVER,
                check_saw: NEVER,
            }),
            ready: Condvar::new(),
        }
    }

    /// Mirror of the publish half of `get_timeout`'s race: the
    /// ticket linearizes the publish against the waiter's checks.
    fn publish(&self, value: u64) {
        let mut state = self.state.lock().unwrap();
        state.published_at = state.next_ticket;
        state.next_ticket += 1;
        state.outcome = Some(value);
        self.ready.notify_all();
    }

    /// The model's clock: raise expiry and wake the waiter (a real
    /// deadline expiry surfaces as a `timed_out()` wakeup).
    fn expire(&self) {
        self.state.lock().unwrap().expired = true;
        self.ready.notify_all();
    }

    /// Mirror of the fixed `get_timeout` wait loop: every iteration
    /// re-checks the outcome BEFORE consulting expiry (the
    /// wake-not-timeout back-edge), recording the final check's
    /// ticket for the linearization property.
    fn get_timeout(&self) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(value) = state.outcome {
                return Some(value);
            }
            state.check_at = state.next_ticket;
            state.next_ticket += 1;
            state.check_saw = state.published_at;
            if state.expired {
                return None;
            }
            state = self.ready.wait(state).unwrap();
        }
    }
}

/// A blocking waiter always observes the publish (no lost wakeup on
/// any schedule — a missed notify would deadlock the model).
#[test]
fn blocking_waiter_observes_publish() {
    loom::model(|| {
        let cell = Arc::new(Cell::pending());
        let waiter = Arc::clone(&cell);
        let waiter = thread::spawn(move || waiter.get());
        let publisher = Arc::clone(&cell);
        let publisher = thread::spawn(move || publisher.publish(11));
        assert!(publisher.join().unwrap());
        assert_eq!(waiter.join().unwrap(), State::Done(11));
    });
}

/// `complete` and `cancel` race under the one lock: exactly one
/// wins, the loser observes the winner, and the waiter sees the
/// settled state.
#[test]
fn complete_cancel_first_wins_loser_observes() {
    loom::model(|| {
        let cell = Arc::new(Cell::pending());
        let waiter = Arc::clone(&cell);
        let waiter = thread::spawn(move || waiter.get());
        let completer = Arc::clone(&cell);
        let completer = thread::spawn(move || completer.publish(13));
        let canceller = Arc::clone(&cell);
        let canceller = thread::spawn(move || canceller.cancel());
        let completed = completer.join().unwrap();
        let cancelled = canceller.join().unwrap();
        assert!(completed != cancelled, "exactly one wins");
        let seen = waiter.join().unwrap();
        if completed {
            assert_eq!(seen, State::Done(13));
        } else {
            assert_eq!(seen, State::Cancelled);
        }
    });
}

/// Racing deliveries: the first wins, the second is a no-op, and
/// the waiter shares the winner's value.
#[test]
fn racing_deliveries_first_wins() {
    loom::model(|| {
        let cell = Arc::new(Cell::pending());
        let waiter = Arc::clone(&cell);
        let waiter = thread::spawn(move || waiter.get());
        let first = Arc::clone(&cell);
        let first = thread::spawn(move || first.publish(1));
        let second = Arc::clone(&cell);
        let second = thread::spawn(move || second.publish(2));
        let won_first = first.join().unwrap();
        let won_second = second.join().unwrap();
        assert!(won_first != won_second, "exactly one wins");
        let seen = waiter.join().unwrap();
        if won_first {
            assert_eq!(seen, State::Done(1));
        } else {
            assert_eq!(seen, State::Done(2));
        }
    });
}

/// The wake-not-timeout back-edge: a `None` (timeout) return
/// linearizes strictly before any winning publish — a publish the
/// waiter already passed never comes back as a timeout.
#[test]
fn timeout_never_loses_publish_before_final_check() {
    loom::model(|| {
        let cell = Arc::new(TimeoutCell::pending());
        let waiter = Arc::clone(&cell);
        let waiter = thread::spawn(move || waiter.get_timeout());
        let publisher = Arc::clone(&cell);
        let publisher = thread::spawn(move || publisher.publish(7));
        let timer = Arc::clone(&cell);
        let timer = thread::spawn(move || timer.expire());
        let result = waiter.join().unwrap();
        publisher.join().unwrap();
        timer.join().unwrap();
        let state = cell.state.lock().unwrap();
        assert_eq!(state.outcome, Some(7));
        match result {
            Some(value) => assert_eq!(value, 7),
            None => assert!(
                state.check_saw == NEVER || state.check_saw > state.check_at,
                "lost a publish that linearized before the final check"
            ),
        }
    });
}
