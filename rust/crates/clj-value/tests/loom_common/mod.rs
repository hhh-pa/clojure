// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Shared mirror of the S6 agent cell + runner protocol for loom models.
//! Mirrors (branch T1174-s6-concurrency):
//!   `rust/crates/clj-value/src/agent.rs` — enqueue / enqueue_nested /
//!       complete_ok / complete_failed / restart_publish (WITH the M3
//!       under-lock failed re-check: the pre-fix publish is what the
//!       round-1 loom models violated)
//!   `rust/crates/clj-eval/src/concurrency.rs` — send_action /
//!       native_restart_agent / execute_head publish discipline /
//!       pump_agent drain
//!
//! Simplifications (faithful to the synchronization structure):
//!   - action bodies are u32 ids; the "apply + blind publish" of execute_head
//!     is `value = value + 1` with the read and the replace in SEPARATE
//!     critical sections (as in the real code: the state clone and the
//!     replace bracket the lock-free action run).
//!   - dispatch to pool threads is inlined: the thread that receives a head
//!     runs it (same-thread runner). The real pools only add a handoff delay;
//!     any schedule with a pool thread is reproducible here because the runner
//!     runs strictly outside the cell lock with explicit preemption windows.
//!   - shutdown/reject paths are NOT modeled.

use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use std::collections::VecDeque;

/// Mirror of `AgentInner`: only the queue-discipline fields.
/// `value`/`error` are u32 stand-ins.
#[derive(Debug)]
pub struct Inner {
    pub value: u32,
    pub error: Option<u32>,
    pub queue: VecDeque<u32>,
    pub running: bool,
}

#[derive(Debug)]
pub struct Agent {
    pub cell: Arc<Mutex<Inner>>,
}

impl Agent {
    pub fn new(value: u32) -> Self {
        Self {
            cell: Arc::new(Mutex::new(Inner {
                value,
                error: None,
                queue: VecDeque::new(),
                running: false,
            })),
        }
    }

    pub fn failed(value: u32, err: u32, queued: Vec<u32>) -> Self {
        Self {
            cell: Arc::new(Mutex::new(Inner {
                value,
                error: Some(err),
                queue: VecDeque::from(queued),
                running: false,
            })),
        }
    }

    pub fn failed_err(&self) -> Option<u32> {
        self.cell.lock().unwrap().error
    }

    /// Mirror of `Agent::enqueue`: failed re-checked under the lock.
    pub fn enqueue(&self, item: u32) -> Result<Option<u32>, u32> {
        let mut inner = self.cell.lock().unwrap();
        if let Some(error) = inner.error {
            return Err(error);
        }
        inner.queue.push_back(item);
        if inner.running {
            return Ok(None);
        }
        inner.running = true;
        Ok(inner.queue.pop_front())
    }

    /// Mirror of `Agent::enqueue_nested`.
    pub fn enqueue_nested(&self, item: u32) -> Option<u32> {
        let mut inner = self.cell.lock().unwrap();
        inner.queue.push_back(item);
        if inner.running || inner.error.is_some() {
            return None;
        }
        inner.running = true;
        inner.queue.pop_front()
    }

    /// Mirror of `Agent::complete_ok`.
    pub fn complete_ok(&self) -> Option<u32> {
        let mut inner = self.cell.lock().unwrap();
        match inner.queue.pop_front() {
            Some(head) => Some(head),
            None => {
                inner.running = false;
                None
            }
        }
    }

    /// Mirror of `Agent::complete_failed`.
    pub fn complete_failed(&self, error: u32) {
        let mut inner = self.cell.lock().unwrap();
        inner.error = Some(error);
        inner.running = false;
    }

    /// Mirror of `Agent::restart_publish` WITH the M3 fix: the failed
    /// state re-checks under the lock, so a racing second restart
    /// observes the cleared cell and publishes nothing (`None`).
    /// Returns the head to run (`Some(Some)`) or nothing (`Some(None)`
    /// for clear/empty).
    pub fn restart_publish(&self, new_state: u32, clear: bool) -> Option<Option<u32>> {
        let mut inner = self.cell.lock().unwrap();
        inner.error.as_ref()?;
        inner.value = new_state;
        inner.error = None;
        if clear {
            inner.queue.clear();
            inner.running = false;
            return Some(None);
        }
        match inner.queue.pop_front() {
            Some(head) => {
                inner.running = true;
                Some(Some(head))
            }
            None => {
                inner.running = false;
                Some(None)
            }
        }
    }
}

/// Shared statistics / preemption probes for the models.
#[derive(Clone)]
pub struct Stats {
    /// Actions currently executing (outside the cell lock).
    pub inflight: Arc<AtomicUsize>,
    /// Execution log (head ids, in run order).
    pub log: Arc<Mutex<Vec<u32>>>,
    /// Dummy atomic: each access is a loom preemption point modeling a
    /// lock-free window (action body, evaluator fork, validation call).
    pub probe: Arc<AtomicUsize>,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            inflight: Arc::new(AtomicUsize::new(0)),
            log: Arc::new(Mutex::new(Vec::new())),
            probe: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn window(&self) {
        self.probe.fetch_add(1, Ordering::SeqCst);
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

/// Run one head outside the cell lock (mirrors execute_head's publish
/// discipline + the runner side of pump_agent), asserting mutual exclusion.
pub fn run_one(agent: &Agent, st: &Stats, head: u32) {
    let n = st.inflight.fetch_add(1, Ordering::SeqCst) + 1;
    assert!(n <= 1, "MUTUAL EXCLUSION: two actions running concurrently");
    st.window(); // action body runs outside the lock
    // Blind read-modify-publish across two critical sections, exactly like
    // execute_head's clone-then-replace around the action run.
    let v = agent.cell.lock().unwrap().value;
    st.window(); // read-to-publish window
    agent.cell.lock().unwrap().value = v + 1;
    st.log.lock().unwrap().push(head);
    st.inflight.fetch_sub(1, Ordering::SeqCst);
}

/// Run a head then drain successors via complete_ok (pump_agent).
pub fn run_and_drain(agent: &Agent, st: &Stats, head: u32) {
    run_one(agent, st, head);
    let mut next = agent.complete_ok();
    while let Some(h) = next {
        run_one(agent, st, h);
        next = agent.complete_ok();
    }
}

/// Mirror of the `send_action` submit path: the failed pre-check, the
/// lock-free fork window, then the atomic enqueue.
pub fn send(agent: &Agent, st: &Stats, item: u32) -> Result<(), u32> {
    if agent.failed_err().is_some() {
        return Err(999);
    }
    st.window(); // fork_for_thread: outside the lock
    match agent.enqueue(item) {
        Ok(Some(head)) => {
            run_and_drain(agent, st, head);
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Mirror of the `native_restart_agent` path: failed pre-check, the
/// options/validation window OUTSIDE the lock (arbitrary code must not
/// run under the non-reentrant cell mutex), then the publish whose
/// under-lock re-check (M3) serializes racing restarts.
pub fn restart(agent: &Agent, st: &Stats, v: u32, clear: bool) -> Result<(), &'static str> {
    if agent.failed_err().is_none() {
        return Err("does not need restart");
    }
    st.window(); // options parse + validator call: arbitrary code, lock-free
    st.window();
    match agent.restart_publish(v, clear) {
        None => Err("does not need restart"),
        Some(Some(head)) => {
            run_and_drain(agent, st, head);
            Ok(())
        }
        Some(None) => Ok(()),
    }
}
