// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Agents (S6C): shared cells mirroring `clojure.lang.Agent`
//! (`docs/t1174/concurrency-contract.md`, `docs/t1174/s6-pins.md` P5).
//!
//! The cell holds the current state, the `:validator` fn (if any),
//! the `:meta` map (if any), the watches map (`{key fn}`, fired in
//! insertion order), the error mode, the error handler (if any), the
//! latched failure (if any), and the FIFO action queue. At most one
//! action runs per agent at a time: [`enqueue`](Agent::enqueue)
//! hands the head to a pool thread only when nothing runs, and
//! completion pops the next head (or parks the queue on failure),
//! so sends from any thread serialize per agent exactly like the
//! JVM's head-chained `ActionQueue` (the explicit `running` flag
//! stands in for the JVM's still-queued running head, which Rust
//! cannot borrow across the unlock).
//!
//! Actions are evaluator callbacks behind [`AgentAction`] (the
//! forked evaluator lives in `clj-eval`, which this crate cannot
//! name): the queue moves them by value, and pool workers run them
//! outside the cell lock. Equality is shared-cell identity plus
//! equal meta (agents have no value equality on the JVM); printing
//! follows the transient `#object[…]` convention. Metadata lives
//! behind the
//! lock (mutable via `alter-meta!`/`reset-meta!`), so
//! [`Value::meta`](crate::Value::meta) — which borrows — cannot
//! serve agents; readers use [`Agent::meta`] (a clone) instead.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{Map, Throwable, Value};

/// Fresh agent identities (print-only; equality is shared-cell).
static AGENT_ID: AtomicU64 = AtomicU64::new(1);

/// Which pool runs an action: `send` (fixed) or `send-off`
/// (unbounded — `send-via` routes here, a pinned divergence, since
/// the port has no custom executors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPool {
    /// The fixed CPU-bound pool (`send`).
    Send,
    /// The unbounded pool (`send-off`, `send-via`).
    SendOff,
}

/// What a failed action does to its agent: `:continue` clears the
/// error and runs on; `:fail` latches the failure and holds the
/// queue until `restart-agent`. Anything but `:continue` behaves as
/// `:fail` (the JVM compares against `CONTINUE` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorMode {
    /// Keep the state, clear the error, run the next action.
    Continue,
    /// Latch the error, hold the queue, reject sends.
    Fail,
}

/// One agent action: apply to the agent's current state, returning
/// the new state or the action's failure. Implemented in `clj-eval`
/// (it owns the action's forked evaluator, which validators, watch
/// fns, and the error handler all run on); the queue moves actions
/// by value and workers run them outside the cell lock, so `run`
/// must never touch this agent's cell (a nested send from the action
/// fn enqueues through the cell's own lock instead, and the runner
/// picks it up on completion).
pub trait AgentAction: Send + Sync {
    /// Run the action fn over `state` (catching everything,
    /// including Rust panics, into the returned failure), then
    /// validate the candidate (a veto is an action failure, like the
    /// JVM's `setState` inside the action `try`).
    fn run(&mut self, state: &Value) -> Result<Value, Throwable>;

    /// Fire `watches` as `[key agent old new]` (nil fns skipped). A
    /// throwing watch fails the action — after the state published,
    /// like the JVM's `notifyWatches` inside the action `try`.
    fn fire_watches(
        &mut self,
        agent: &Value,
        old: &Value,
        new: &Value,
        watches: &Map,
    ) -> Result<(), Throwable>;

    /// Invoke `handler` as `[agent error]`; every failure (including
    /// Rust panics) is ignored, like the JVM's handler `try`.
    fn handle_error(&mut self, handler: &Value, agent: &Value, error: &Throwable);

    /// Take the nested sends this action buffered (released on
    /// success, discarded on failure); empty for actions that never
    /// buffer (barriers buffer nothing in practice, but share the
    /// mechanism).
    fn take_outbox(&mut self) -> Vec<(Agent, QueuedAction)> {
        Vec::new()
    }
}

/// A queued action plus the pool that runs it (each action
/// remembers its own pool, like the JVM's per-`Action` executor).
pub struct QueuedAction {
    /// The pool that runs this action.
    pub pool: AgentPool,
    /// The action itself (runs exactly once).
    pub action: Box<dyn AgentAction>,
}

impl std::fmt::Debug for QueuedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedAction")
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

/// The mutable cell behind an [`Agent`]: the reference state (value,
/// validator, metadata, watches — the `ARef` half) plus the error
/// mode, the error handler, the latched failure, and the FIFO
/// action queue with its at-most-one runner.
#[derive(Debug)]
pub struct AgentInner {
    /// The current value.
    pub value: Value,
    /// The validator fn (`None` = none installed — falsy `:validator`
    /// options never install, like the JVM's `setup-reference`).
    pub validator: Option<Value>,
    /// The metadata map (`None` = none).
    pub meta: Option<Map>,
    /// Watch fns keyed opaquely (nil fns install but never fire,
    /// like the JVM's `notifyWatches` null check).
    pub watches: Map,
    /// What a failed action does (`:continue` by default with a
    /// handler, else `:fail` — the `agent` fn resolves this).
    pub error_mode: ErrorMode,
    /// The error-handler fn (`None` = none installed).
    pub error_handler: Option<Value>,
    /// The latched failure (`Some` = failed: sends rejected, the
    /// queue held, until `restart-agent` clears it).
    error: Option<Throwable>,
    /// Actions waiting for (or under) the runner, head first. The
    /// running head is popped while it runs (unlike the JVM's
    /// still-queued head); `running` covers the gap.
    queue: VecDeque<QueuedAction>,
    /// Whether an action is currently running for this agent.
    running: bool,
}

/// A first-class agent value: an identity plus the shared cell.
/// Identity equality (same cell, equal meta); `counted?` is false,
/// like the JVM.
#[derive(Clone, Debug)]
pub struct Agent {
    id: u64,
    cell: Arc<Mutex<AgentInner>>,
}

impl Agent {
    /// An agent over `value` (fresh identity, no validator, no meta,
    /// no watches, no handler, `:fail` mode — the `agent` native
    /// installs the options over this).
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self {
            id: AGENT_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(Mutex::new(AgentInner {
                value,
                validator: None,
                meta: None,
                watches: crate::new_map(),
                error_mode: ErrorMode::Fail,
                error_handler: None,
                error: None,
                queue: VecDeque::new(),
                running: false,
            })),
        }
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this agent's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only; never across an
    /// action fn/validator/watch/error-handler call — pool workers
    /// run those outside the lock, so watches and handlers may send
    /// again. Poison is unreachable).
    pub fn lock(&self) -> MutexGuard<'_, AgentInner> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The metadata map, if any (cloned from behind the lock, for
    /// `meta` and `*print-meta*`).
    #[must_use]
    pub fn meta(&self) -> Option<Map> {
        self.lock().meta.clone()
    }

    /// The latched failure, if the agent is failed (cloned).
    #[must_use]
    pub fn error(&self) -> Option<Throwable> {
        self.lock().error.clone()
    }

    /// Outstanding actions: queued plus running (the JVM's
    /// `getQueueCount`, which `await1` consults before awaiting).
    #[must_use]
    pub fn queue_count(&self) -> usize {
        let inner = self.lock();
        inner.queue.len() + usize::from(inner.running)
    }

    /// The JVM class name.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.lang.Agent"
    }

    /// Enqueue `item`, returning the head to execute when this send
    /// starts the runner (the queue was idle). A failed agent
    /// rejects with its latched error (the `send` failure cause).
    pub fn enqueue(&self, item: QueuedAction) -> Result<Option<QueuedAction>, Throwable> {
        let mut inner = self.lock();
        if let Some(error) = &inner.error {
            return Err(error.clone());
        }
        inner.queue.push_back(item);
        if inner.running {
            return Ok(None);
        }
        inner.running = true;
        Ok(inner.queue.pop_front())
    }

    /// Enqueue a released nested send: unchecked (the JVM's
    /// `releasePendingSends` bypasses the failed check too), and the
    /// runner starts only when idle AND clean — onto a failed agent
    /// the send queues but never executes (until a restart without
    /// `:clear-actions` resumes it, like the JVM).
    pub fn enqueue_nested(&self, item: QueuedAction) -> Option<QueuedAction> {
        let mut inner = self.lock();
        inner.queue.push_back(item);
        if inner.running || inner.error.is_some() {
            return None;
        }
        inner.running = true;
        inner.queue.pop_front()
    }

    /// The runner finished an action cleanly: hand over the next
    /// head, or idle the runner when the queue drained.
    pub fn complete_ok(&self) -> Option<QueuedAction> {
        let mut inner = self.lock();
        match inner.queue.pop_front() {
            Some(head) => Some(head),
            None => {
                inner.running = false;
                None
            }
        }
    }

    /// The runner finished an action failed under `:fail`: latch
    /// `error`, idle the runner, hold the queue (the failed head is
    /// already out; the rest wait for `restart-agent`).
    pub fn complete_failed(&self, error: Throwable) {
        let mut inner = self.lock();
        inner.error = Some(error);
        inner.running = false;
    }

    /// Publish a restart: install `new_state` (already validated by
    /// the caller — a veto never reaches here, so the agent stays
    /// failed with its old state and error), clear the failure, and
    /// either drop the held queue (`clear`) or resume it (returning
    /// the head to execute). Watches never fire for restarts. The
    /// failure is re-checked under the lock, so two racing restarts
    /// serialize: the loser finds the error already cleared and gets
    /// `None` (the caller reports "does not need a restart").
    pub fn restart_publish(&self, new_state: Value, clear: bool) -> Option<Option<QueuedAction>> {
        let mut inner = self.lock();
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

#[cfg(test)]
mod tests {
    use super::{Agent, AgentAction, AgentPool, QueuedAction};
    use crate::{Map, ThrowClass, Throwable, Value};
    use std::sync::Arc;

    /// Minimal action that keeps the default `take_outbox`.
    struct StubAction;

    impl AgentAction for StubAction {
        fn run(&mut self, state: &Value) -> Result<Value, Throwable> {
            Ok(state.clone())
        }

        fn fire_watches(
            &mut self,
            _agent: &Value,
            _old: &Value,
            _new: &Value,
            _watches: &Map,
        ) -> Result<(), Throwable> {
            Ok(())
        }

        fn handle_error(&mut self, _handler: &Value, _agent: &Value, _error: &Throwable) {}
    }

    fn stub_queued(pool: AgentPool) -> QueuedAction {
        QueuedAction {
            pool,
            action: Box::new(StubAction),
        }
    }

    fn failure() -> Throwable {
        Throwable {
            id: 7,
            class: ThrowClass::RuntimeException,
            message: Some(Arc::from("boom")),
            data: None,
            cause: None,
        }
    }

    #[test]
    fn default_take_outbox_is_empty() {
        let mut action = StubAction;
        assert!(action.take_outbox().is_empty());
    }

    #[test]
    fn stub_action_runs_and_fires() {
        let mut action = StubAction;
        let state = Value::Int(1);
        assert!(matches!(action.run(&state), Ok(Value::Int(1))));
        assert!(
            action
                .fire_watches(&Value::Nil, &Value::Nil, &Value::Nil, &crate::new_map())
                .is_ok()
        );
        action.handle_error(&Value::Nil, &Value::Nil, &failure());
    }

    #[test]
    fn queued_action_debug_names_pool() {
        assert_eq!(
            format!("{:?}", stub_queued(AgentPool::Send)),
            "QueuedAction { pool: Send, .. }"
        );
        assert_eq!(
            format!("{:?}", stub_queued(AgentPool::SendOff)),
            "QueuedAction { pool: SendOff, .. }"
        );
    }

    #[test]
    fn enqueue_on_failed_agent_rejects_with_latched_error() {
        let agent = Agent::new(Value::Int(0));
        agent.complete_failed(failure());
        let err = agent
            .enqueue(stub_queued(AgentPool::Send))
            .expect_err("failed agent must reject sends");
        assert_eq!(err.id, 7);
        assert_eq!(err.class, ThrowClass::RuntimeException);
    }
}
