// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `alter-var-root` lock protocol (T1192 FX1) — loom mirror of
//! `env.rs` (`VarAlterLock`/`VarAlterGuard`): a mutex+condvar
//! reentrant lock. Properties: racing critical sections
//! serialize (no lost update), same-thread reentry nests,
//! waiters always wake (no lost wakeup — a missed notify hangs
//! the model and fails the test).
//!
//! Run with the rest of the suite (`cargo test -p clj-eval`);
//! loom explores every schedule of each model. The mirror must
//! track `env.rs` branch-for-branch; drift is a bug.

use loom::sync::{Condvar, Mutex};
use std::sync::Arc;

struct Owner {
    owner: Option<loom::thread::ThreadId>,
    depth: u32,
}

struct Lock {
    state: Mutex<Owner>,
    cvar: Condvar,
}

struct Guard {
    lock: Arc<Lock>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut owner = self.lock.state.lock().unwrap();
        owner.depth -= 1;
        if owner.depth == 0 {
            owner.owner = None;
            self.lock.cvar.notify_one();
        }
    }
}

impl Lock {
    fn new() -> Self {
        Self {
            state: Mutex::new(Owner {
                owner: None,
                depth: 0,
            }),
            cvar: Condvar::new(),
        }
    }

    fn acquire(lock: &Arc<Lock>) -> Guard {
        let current = loom::thread::current().id();
        let mut owner = lock.state.lock().unwrap();
        loop {
            if matches!(&owner.owner, Some(held) if held == &current) {
                owner.depth += 1;
                break;
            }
            if owner.owner.is_some() {
                owner = lock.cvar.wait(owner).unwrap();
            } else {
                owner.owner = Some(current);
                owner.depth = 1;
                break;
            }
        }
        drop(owner);
        Guard {
            lock: Arc::clone(lock),
        }
    }
}

/// Two racing read-modify-write critical sections serialize: the
/// counter ends at 2 on every schedule (no lost update).
#[test]
fn racing_critical_sections_serialize() {
    loom::model(|| {
        let lock = Arc::new(Lock::new());
        let counter = Arc::new(Mutex::new(0));
        let (l2, c2) = (lock.clone(), counter.clone());
        let h = loom::thread::spawn(move || {
            let _guard = Lock::acquire(&l2);
            let mut n = c2.lock().unwrap();
            *n += 1;
        });
        {
            let _guard = Lock::acquire(&lock);
            let mut n = counter.lock().unwrap();
            *n += 1;
        }
        h.join().unwrap();
        assert_eq!(*counter.lock().unwrap(), 2);
    });
}

/// Same-thread reentry nests (depth counts) and a waiter
/// acquires after the outermost release — no lost wakeup.
#[test]
fn reentry_nests_and_waiters_wake() {
    loom::model(|| {
        let lock = Arc::new(Lock::new());
        let first = Lock::acquire(&lock);
        let second = Lock::acquire(&lock);
        {
            let owner = lock.state.lock().unwrap();
            assert_eq!(owner.depth, 2);
        }
        let l2 = lock.clone();
        let h = loom::thread::spawn(move || {
            let _guard = Lock::acquire(&l2);
        });
        drop(first);
        {
            // Still held (depth 1): the waiter cannot have acquired.
            let owner = lock.state.lock().unwrap();
            assert_eq!(owner.depth, 1);
        }
        drop(second);
        h.join().unwrap();
    });
}
