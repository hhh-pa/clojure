// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Loom model of the delay force protocol (`DelayState` in
//! `src/delay.rs`, taken by `take_delay_thunk` in `clj-eval`): the
//! state machine below mirrors the real one transition for
//! transition — `Pending` takes, `Forcing`-by-me errors,
//! `Forcing`-by-other condvar-waits then re-takes, and every
//! publish wakes all waiters. The one deliberate gap: the owner is
//! the forcing thread's model id (`u8`), not a `ThreadId`, because
//! loom multiplexes every model thread onto one real thread.
//!
//! Run with the rest of the suite (`cargo test -p clj-value
//! --offline`); loom explores every schedule of each model.

use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;

/// Mirror of `DelayState` (values simplified to `u64`/unit — the
/// outcomes' payloads never steer the protocol).
#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Unforced.
    Pending,
    /// A force is in progress, owned by this model thread.
    Forcing(u8),
    /// The forced value.
    Realized(u64),
    /// The cached failure (rethrown on every force).
    Failed,
}

/// Mirror of the delay cell: the force state plus its wait queue.
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
}

/// Mirror of `take_delay_thunk`'s `DelayTake`: the cached outcome
/// or the pending body to run.
enum Take {
    Done(Result<u64, ForceError>),
    Thunk,
}

/// Why a take finished without running the body: same-thread
/// re-entry (`IllegalStateException`) or the cached failure.
#[derive(Debug, PartialEq, Eq)]
enum ForceError {
    Reentrant,
    Failed,
}

/// Mirror of `take_delay_thunk`: cached states restore themselves,
/// `Pending` takes (leaving `Forcing(me)`), `Forcing(me)`
/// errors, and `Forcing(other)` condvar-waits then re-takes.
fn take(cell: &Cell, me: u8) -> Take {
    let mut state = cell.state.lock().unwrap();
    loop {
        let fellow_forcing = match &*state {
            State::Forcing(owner) => *owner != me,
            _ => false,
        };
        if !fellow_forcing {
            break;
        }
        state = cell.ready.wait(state).unwrap();
    }
    match std::mem::replace(&mut *state, State::Forcing(me)) {
        State::Realized(v) => {
            *state = State::Realized(v);
            Take::Done(Ok(v))
        }
        State::Failed => {
            *state = State::Failed;
            Take::Done(Err(ForceError::Failed))
        }
        State::Forcing(owner) => {
            *state = State::Forcing(owner);
            Take::Done(Err(ForceError::Reentrant))
        }
        State::Pending => Take::Thunk,
    }
}

/// Mirror of the publish half of `finish_delay_force`: install the
/// value and wake every waiter.
fn publish(cell: &Cell, value: u64) {
    *cell.state.lock().unwrap() = State::Realized(value);
    cell.ready.notify_all();
}

/// Mirror of the failure half of `finish_delay_force`: cache the
/// failure and wake every waiter.
fn publish_failed(cell: &Cell) {
    *cell.state.lock().unwrap() = State::Failed;
    cell.ready.notify_all();
}

/// Force to completion, running the body at most once per cell:
/// the take/hold/publish shape of `force_delay`.
fn force_once(cell: &Cell, runs: &Mutex<u32>, me: u8) -> u64 {
    match take(cell, me) {
        Take::Done(Ok(v)) => v,
        Take::Done(Err(_)) => unreachable!("no failures or re-entry in this model"),
        Take::Thunk => {
            *runs.lock().unwrap() += 1;
            publish(cell, 42);
            42
        }
    }
}

/// Two threads race one force: exactly one body runs, and both
/// threads share its outcome (the waiter blocks on the condvar,
/// never errors).
#[test]
fn racing_forcers_share_one_outcome() {
    loom::model(|| {
        let cell = Arc::new(Cell::pending());
        let runs = Arc::new(Mutex::new(0u32));
        let first = Arc::clone(&cell);
        let first_runs = Arc::clone(&runs);
        let t0 = thread::spawn(move || force_once(&first, &first_runs, 0));
        let second = Arc::clone(&cell);
        let second_runs = Arc::clone(&runs);
        let t1 = thread::spawn(move || force_once(&second, &second_runs, 1));
        assert_eq!(t0.join().unwrap(), 42);
        assert_eq!(t1.join().unwrap(), 42);
        assert_eq!(*runs.lock().unwrap(), 1);
    });
}

/// The owner re-enters while a fellow thread waits: the re-entry
/// errors, the waiter blocks, and the publish settles both.
#[test]
fn reentrant_force_errors_while_fellow_waits() {
    loom::model(|| {
        let cell = Arc::new(Cell::pending());
        assert!(matches!(take(&cell, 0), Take::Thunk));
        let waiter = Arc::clone(&cell);
        let t = thread::spawn(move || match take(&waiter, 1) {
            Take::Done(Ok(v)) => v,
            Take::Done(Err(_)) => unreachable!("a fellow force waits, never errors"),
            Take::Thunk => unreachable!("the thunk is already taken"),
        });
        assert!(matches!(
            take(&cell, 0),
            Take::Done(Err(ForceError::Reentrant))
        ));
        publish(&cell, 5);
        assert_eq!(t.join().unwrap(), 5);
    });
}

/// A cached failure rethrows the same failure on every later force
/// (and still counts as realized — no re-run).
#[test]
fn cached_failure_rethrows() {
    loom::model(|| {
        let cell = Cell::pending();
        assert!(matches!(take(&cell, 0), Take::Thunk));
        publish_failed(&cell);
        assert!(matches!(
            take(&cell, 0),
            Take::Done(Err(ForceError::Failed))
        ));
        assert!(matches!(
            take(&cell, 1),
            Take::Done(Err(ForceError::Failed))
        ));
    });
}
