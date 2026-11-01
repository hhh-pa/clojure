// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Single-thread executor cell (#188) — loom model over a mirror of
//! `src/executor.rs` (`submit` / `shutdown` / `is_shutdown` plus the
//! worker drain loop).
//!
//! Properties: the worker runs jobs FIFO, shutdown drains the queued
//! jobs then rejects (idempotent), and a submit racing shutdown
//! either runs or rejects — never both, never neither.
//!
//! Mirror notes (faithful to the synchronization structure):
//!   - loom has no mpsc, so the unbounded job queue is a
//!     `Mutex<VecDeque>` + `Condvar` stand-in behind the same shape:
//!     the sender slot (`Option`, `None` once shut) under one lock,
//!     the worker draining head-first, exiting on drained-and-shut.
//!     The std mpsc internals themselves are OUT (std owns them —
//!     the TLA spec `executor` keeps the same boundary).
//!   - jobs are u64 ids; bodies are opaque closures in the real pool,
//!     so "running" a job is logging its id outside the pool lock.
//!   - thread naming (`pool-N-thread-1`), the process-wide pool
//!     counter, and worker detach are NOT modeled (pinned by the
//!     in-module tests).
//!
//! Run with the rest of the suite (`cargo test -p clj-value
//! --offline`); loom explores every schedule of each model.

use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;
use std::collections::VecDeque;

/// Mirror of `ExecutorPool`'s sender slot + the job queue: the
/// `shut` flag is the slot (`Some` while jobs flow, `None` once shut
/// — like the real `Mutex<Option<Sender>>`); the queue sits beside
/// it under the one pool lock (the mpsc stand-in).
#[derive(Debug)]
struct Pool {
    state: Mutex<PoolState>,
    work: Condvar,
    /// Run log (job ids, in run order) — appended outside the pool
    /// lock, like the real worker running bodies off-queue.
    log: Mutex<Vec<u64>>,
}

#[derive(Debug)]
struct PoolState {
    queue: VecDeque<u64>,
    shut: bool,
}

impl Pool {
    fn new() -> Self {
        Self {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                shut: false,
            }),
            work: Condvar::new(),
            log: Mutex::new(Vec::new()),
        }
    }

    /// Mirror of `Executor::submit`: queue under the slot lock (never
    /// blocks — the queue is unbounded); `false` once shut down.
    fn submit(&self, job: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.shut {
            return false;
        }
        state.queue.push_back(job);
        self.work.notify_one();
        true
    }

    /// Mirror of `Executor::shutdown` (idempotent): drop the sender
    /// slot; the worker drains the queued jobs, then exits.
    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.shut = true;
        self.work.notify_all();
    }

    /// Mirror of `Executor::is_shutdown`.
    fn is_shutdown(&self) -> bool {
        self.state.lock().unwrap().shut
    }

    /// Mirror of the worker loop (`while let Ok(job) = rx.recv()`):
    /// run head-first; exit on drained-and-shut (the recv loop ends
    /// when the drained queue meets the dropped sender).
    fn run_worker(&self) {
        loop {
            let job = {
                let mut state = self.state.lock().unwrap();
                while state.queue.is_empty() && !state.shut {
                    state = self.work.wait(state).unwrap();
                }
                match state.queue.pop_front() {
                    Some(job) => job,
                    None => return,
                }
            };
            self.log.lock().unwrap().push(job);
        }
    }
}

/// The worker runs every submitted job FIFO on its single thread.
#[test]
fn worker_runs_fifo() {
    loom::model(|| {
        let pool = Arc::new(Pool::new());
        let worker = Arc::clone(&pool);
        let h = thread::spawn(move || worker.run_worker());
        assert!(pool.submit(1));
        assert!(pool.submit(2));
        assert!(pool.submit(3));
        pool.shutdown();
        h.join().unwrap();
        assert_eq!(*pool.log.lock().unwrap(), vec![1, 2, 3]);
    });
}

/// Shutdown drains the queued jobs, then rejects; shutdown is
/// idempotent (the gap-96 contract).
#[test]
fn shutdown_drains_then_rejects() {
    loom::model(|| {
        let pool = Pool::new();
        assert!(!pool.is_shutdown());
        assert!(pool.submit(7));
        pool.shutdown();
        assert!(pool.is_shutdown());
        assert!(!pool.submit(8));
        pool.shutdown();
        assert!(pool.is_shutdown());
        // The queued job still runs (the worker drains before
        // exiting); the rejected one never does.
        pool.run_worker();
        assert_eq!(*pool.log.lock().unwrap(), vec![7]);
    });
}

/// A submit racing shutdown either runs or rejects — never both,
/// never neither (the first-wins pin on the sender slot).
#[test]
fn racing_submit_vs_shutdown_runs_xor_rejects() {
    loom::model(|| {
        let pool = Arc::new(Pool::new());
        let worker = Arc::clone(&pool);
        let h = thread::spawn(move || worker.run_worker());
        let racing = Arc::clone(&pool);
        let submitter = thread::spawn(move || racing.submit(5));
        pool.shutdown();
        let accepted = submitter.join().unwrap();
        h.join().unwrap();
        let log = pool.log.lock().unwrap();
        if accepted {
            assert_eq!(*log, vec![5]);
        } else {
            assert!(log.is_empty(), "rejected job ran: {log:?}");
        }
    });
}

/// Two racing submitters both land; the worker runs them in accept
/// (lock) order — whichever order the schedule chose.
#[test]
fn racing_submitters_both_run_once() {
    loom::model(|| {
        let pool = Arc::new(Pool::new());
        let worker = Arc::clone(&pool);
        let h = thread::spawn(move || worker.run_worker());
        let a = Arc::clone(&pool);
        let ha = thread::spawn(move || a.submit(1));
        let b = Arc::clone(&pool);
        let hb = thread::spawn(move || b.submit(2));
        assert!(ha.join().unwrap());
        assert!(hb.join().unwrap());
        pool.shutdown();
        h.join().unwrap();
        let log = pool.log.lock().unwrap();
        assert_eq!(2, log.len(), "each job runs once: {log:?}");
        assert!(log.contains(&1) && log.contains(&2));
    });
}
