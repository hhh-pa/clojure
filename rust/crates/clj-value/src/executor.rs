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
//! Lifecycle is port-fixed: workers run until process exit (there is
//! no upstream shutdown surface outside interop, so none is mirrored
//! — `shutdown-agents` touches the two agent pools only, like the
//! JVM). Equality is shared-pool identity; printing follows the cell
//! `#object[…]` convention under the classic delegated-service class
//! (newer JDKs report an `AutoShutdown` variant instead —
//! port-fixed, like the version-dependent cast texts).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};

/// Fresh executor identities (print-only; equality is shared-pool).
static EXECUTOR_ID: AtomicU64 = AtomicU64::new(1);

/// The JVM's pool counter (`pool-N-thread-M` names every pool from
/// one process-wide sequence).
static EXECUTOR_POOL: AtomicU64 = AtomicU64::new(1);

/// One unit of executor work: an opaque closure (callers wrap
/// whatever runs — agent heads arrive as pump closures).
pub type ExecutorJob = Box<dyn FnOnce() + Send + 'static>;

/// The shared worker behind an [`Executor`]: the job queue's sender
/// (clones share the pool; the worker exits only when every sender
/// drops, i.e. never before process exit in practice).
#[derive(Debug)]
struct ExecutorPool {
    tx: Sender<ExecutorJob>,
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
    /// submits never queue behind a lazy start).
    #[must_use]
    pub fn new() -> Self {
        let id = EXECUTOR_ID.fetch_add(1, Ordering::Relaxed);
        let pool_no = EXECUTOR_POOL.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<ExecutorJob>();
        let name = format!("pool-{pool_no}-thread-1");
        // A spawn failure leaves a sender with no receiver: submits
        // drop, like the JVM's rejected `execute` — and the pool
        // constructors already pin spawn failure as unreachable on
        // supported targets, so no error path is exposed here.
        let _ = std::thread::Builder::new().name(name).spawn(move || {
            while let Ok(job) = rx.recv() {
                job();
            }
        });
        Self {
            id,
            pool_no,
            pool: Arc::new(ExecutorPool { tx }),
        }
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
    /// unbounded; a dead worker drops it, like a rejection).
    pub fn submit(&self, job: ExecutorJob) {
        let _ = self.pool.tx.send(job);
    }

    /// The JVM class name (the classic delegated service — newer
    /// JDKs report the `AutoShutdown` variant; port-fixed).
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "java.util.concurrent.Executors$FinalizableDelegatedExecutorService"
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Executor;

    /// The worker runs jobs FIFO on its `pool-N-thread-1` thread.
    #[test]
    fn worker_runs_fifo_on_named_thread() {
        let ex = Executor::new();
        let (tx, rx) = std::sync::mpsc::channel();
        for i in 0..3 {
            let tx = tx.clone();
            ex.submit(Box::new(move || {
                let name = std::thread::current().name().unwrap_or("?").to_owned();
                tx.send((i, name)).unwrap();
            }));
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
        let a = Executor::new();
        let b = a.clone();
        let c = Executor::new();
        assert!(a.same_pool(&b));
        assert!(!a.same_pool(&c));
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
    }
}
