// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Executor cells (fence 3c): single-thread pools mirroring
//! `Executors/newSingleThreadExecutor` — the constructors `send-via`
//! honors. One worker thread per executor (`pool-N-thread-1`, the
//! JVM's naming over a process-wide pool counter), an unbounded job
//! queue, FIFO order. Jobs are opaque closures, so the pool needs no
//! evaluator types (agent actions arrive wrapped by the caller).
//!
//! Lifecycle (gap-96): `shutdown-agents` shuts the installed pool
//! fields (customs included, like the JVM's `Agent/shutdown` over
//! the current fields) — queued jobs still run, later submits
//! reject. A `send-via`-only executor is never installed, so it
//! never shuts down (fence 3c). Workers detach at spawn and nothing
//! ever joins them (daemon-like at process exit). Equality is
//! shared-pool identity; printing follows the cell `#object[…]`
//! convention under the classic delegated-service class (newer JDKs
//! report an `AutoShutdown` variant instead — port-fixed, like the
//! version-dependent cast texts).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard};

/// Fresh executor identities (print-only; equality is shared-pool).
static EXECUTOR_ID: AtomicU64 = AtomicU64::new(1);

/// The JVM's pool counter (`pool-N-thread-M` names every pool from
/// one process-wide sequence).
static EXECUTOR_POOL: AtomicU64 = AtomicU64::new(1);

/// One unit of executor work: an opaque closure (callers wrap
/// whatever runs — agent heads arrive as pump closures).
pub type ExecutorJob = Box<dyn FnOnce() + Send + 'static>;

/// The shared worker behind an [`Executor`]: the job queue's sender
/// (clones share the pool; `None` once shut down — the worker drains
/// the queued jobs, then exits).
#[derive(Debug)]
struct ExecutorPool {
    tx: Mutex<Option<Sender<ExecutorJob>>>,
}

impl ExecutorPool {
    /// Lock the sender slot (short critical sections only; the send
    /// itself never blocks — the queue is unbounded).
    fn lock(&self) -> MutexGuard<'_, Option<Sender<ExecutorJob>>> {
        self.tx.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A first-class executor value: an identity plus the shared worker.
/// Identity equality (same pool); `counted?` is false, like the JVM.
#[derive(Clone, Debug)]
pub struct Executor {
    id: u64,
    pool_no: u64,
    pool: Arc<ExecutorPool>,
}

impl Executor {
    /// A fresh single-thread executor (spawns its worker now, so
    /// submits never queue behind a lazy start). A spawn failure
    /// is an error (near-unreachable on supported targets — the
    /// pool workers pin the same failure the same way).
    pub fn new() -> Result<Self, std::io::Error> {
        let id = EXECUTOR_ID.fetch_add(1, Ordering::Relaxed);
        let pool_no = EXECUTOR_POOL.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<ExecutorJob>();
        let name = format!("pool-{pool_no}-thread-1");
        let spawned = std::thread::Builder::new().name(name).spawn(move || {
            while let Ok(job) = rx.recv() {
                job();
            }
        });
        Self::from_spawned(id, pool_no, tx, spawned)
    }

    /// Build over an explicit spawn result (takes the `Result` so
    /// tests can drive the failure, like the pool workers'
    /// `install_worker`). The worker handle detaches — nothing
    /// ever joins executor threads (daemon-like at process exit).
    fn from_spawned(
        id: u64,
        pool_no: u64,
        tx: Sender<ExecutorJob>,
        spawned: std::io::Result<std::thread::JoinHandle<()>>,
    ) -> Result<Self, std::io::Error> {
        spawned.map(|_| Self {
            id,
            pool_no,
            pool: Arc::new(ExecutorPool {
                tx: Mutex::new(Some(tx)),
            }),
        })
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The JVM pool number (the `N` in `pool-N-thread-1`).
    #[must_use]
    pub fn pool_no(&self) -> u64 {
        self.pool_no
    }

    /// Whether `other` shares this executor's worker.
    #[must_use]
    pub fn same_pool(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pool, &other.pool)
    }

    /// Queue `job` on the worker (never blocks — the queue is
    /// unbounded). `false` once shut down — the job never runs (like
    /// the JVM's `execute` rejection); a dead worker drops it all the
    /// same (unreachable — the worker outlives the sender).
    pub fn submit(&self, job: ExecutorJob) -> bool {
        match self.pool.lock().as_ref() {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        }
    }

    /// Shut the pool down (idempotent): queued jobs still run, later
    /// submits reject, and the worker exits once the queue drains —
    /// like the JVM's `ExecutorService/shutdown`, which
    /// `shutdown-agents` calls over the installed fields.
    pub fn shutdown(&self) {
        self.pool.lock().take();
    }

    /// Whether the pool is shut down.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.pool.lock().is_none()
    }

    /// The JVM class name (the classic delegated service — newer
    /// JDKs report the `AutoShutdown` variant; port-fixed).
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "java.util.concurrent.Executors$FinalizableDelegatedExecutorService"
    }
}

#[cfg(test)]
mod tests {
    use super::Executor;

    /// The worker runs jobs FIFO on its `pool-N-thread-1` thread.
    #[test]
    fn worker_runs_fifo_on_named_thread() {
        let ex = Executor::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        for i in 0..3 {
            let tx = tx.clone();
            assert!(ex.submit(Box::new(move || {
                let name = std::thread::current().name().unwrap_or("?").to_owned();
                tx.send((i, name)).unwrap();
            })));
        }
        drop(tx);
        let got: Vec<(i32, String)> = rx.into_iter().collect();
        assert_eq!(got.len(), 3);
        assert!(got.windows(2).all(|w| w[0].0 < w[1].0));
        let want = format!("pool-{}-thread-1", ex.pool_no());
        assert!(got.iter().all(|(_, n)| *n == want));
    }

    /// Clones share the pool; fresh executors do not.
    #[test]
    fn identity_is_shared_pool() {
        let a = Executor::new().unwrap();
        let b = a.clone();
        let c = Executor::new().unwrap();
        assert!(a.same_pool(&b));
        assert!(!a.same_pool(&c));
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
    }

    /// Executors hash by minted id (covers `id_cell_tag`'s arm).
    #[test]
    fn hash_uses_minted_id() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        fn hash_of(v: &crate::Value) -> u64 {
            let mut h = DefaultHasher::new();
            v.hash(&mut h);
            h.finish()
        }
        let a = Executor::new().unwrap();
        let (x, y) = (crate::Value::Executor(a.clone()), crate::Value::Executor(a));
        assert_eq!(hash_of(&x), hash_of(&y));
        let z = crate::Value::Executor(Executor::new().unwrap());
        assert_ne!(hash_of(&x), hash_of(&z));
    }

    /// A spawn failure is an error, not a dead pool (fence 3c N2).
    #[test]
    fn spawn_failure_is_an_error() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let err = std::io::Error::other("boom");
        assert!(super::Executor::from_spawned(1, 1, tx, Err(err)).is_err());
    }

    /// Shutdown drains the queued jobs, then rejects (gap-96).
    #[test]
    fn shutdown_drains_then_rejects() {
        let ex = Executor::new().unwrap();
        assert!(!ex.is_shutdown());
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(ex.submit(Box::new(move || tx.send(7).unwrap())));
        ex.shutdown();
        assert!(ex.is_shutdown());
        // The queued job still runs (the worker drains before
        // exiting); later submits reject.
        assert_eq!(rx.recv().unwrap(), 7);
        assert!(!ex.submit(Box::new(|| {})));
        // Idempotent (like the JVM's `shutdown`).
        ex.shutdown();
        assert!(ex.is_shutdown());
    }

    /// Shutdown is shared-pool state: clones shut together.
    #[test]
    fn shutdown_is_shared_pool_state() {
        let a = Executor::new().unwrap();
        let b = a.clone();
        a.shutdown();
        assert!(b.is_shutdown());
        assert!(!b.submit(Box::new(|| {})));
    }
}
