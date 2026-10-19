// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Agent queue discipline (S6C) — loom model over the shared cell
//! mirror (`loom_common`, mirroring `agent.rs` and the
//! submit/complete/restart paths of `concurrency.rs`).
//!
//! Properties: at-most-one runner, FIFO drain, failed-agent
//! rejection, nested-send queuing, racing restarts serialize (the M3
//! regression: the pre-fix publish without the under-lock re-check
//! violates the restart properties on multiple schedules).
//!
//! Run with the rest of the suite (`cargo test -p clj-value
//! --offline`); loom explores every schedule of each model.

#[path = "loom_common/mod.rs"]
mod loom_common;

use loom::sync::atomic::Ordering;
use loom_common::{Agent, Stats, restart, run_and_drain, send};
use std::sync::Arc;

/// P3a: two concurrent sends — each action runs exactly once, the agent
/// ends idle (running=false, queue empty), no lost update.
#[test]
fn concurrent_sends_single_runner_each_once() {
    loom::model(|| {
        let agent = Arc::new(Agent::new(0));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || send(&a2, &s2, 1));
        let r1 = send(&agent, &st, 2);
        let r2 = h.join().unwrap();
        assert_eq!(Ok(()), r1);
        assert_eq!(Ok(()), r2);

        let log = st.log.lock().unwrap();
        assert_eq!(2, log.len(), "each action runs exactly once: {log:?}");
        assert!(log.contains(&1) && log.contains(&2));
        let inner = agent.cell.lock().unwrap();
        assert!(!inner.running, "runner idles after drain");
        assert!(inner.queue.is_empty());
        assert_eq!(2, inner.value, "no lost update across handoff");
    });
}

/// P3b: one sender, three actions — FIFO drain order is preserved through
/// the complete_ok handoff chain.
#[test]
fn single_sender_fifo_order() {
    loom::model(|| {
        let agent = Agent::new(0);
        let st = Stats::new();
        // Enqueue three directly (queue discipline only; runner drains).
        let h1 = agent.enqueue(1).unwrap();
        assert!(h1.is_some());
        assert_eq!(Ok(None), agent.enqueue(2));
        assert_eq!(Ok(None), agent.enqueue(3));
        run_and_drain(&agent, &st, h1.unwrap());
        assert_eq!(vec![1, 2, 3], *st.log.lock().unwrap());
        let inner = agent.cell.lock().unwrap();
        assert!(!inner.running);
        assert!(inner.queue.is_empty());
        assert_eq!(3, inner.value);
    });
}

/// P3c: concurrent nested sends onto an idle clean agent — both actions
/// run exactly once (sequential full drains are benign; mutual
/// exclusion is asserted inside run_one on every schedule).
#[test]
fn concurrent_nested_start_single_runner() {
    loom::model(|| {
        let agent = Arc::new(Agent::new(0));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || {
            let r = a2.enqueue_nested(1);
            if let Some(head) = r {
                run_and_drain(&a2, &s2, head);
            }
            r.is_some()
        });
        let r1 = agent.enqueue_nested(2);
        if let Some(head) = r1 {
            run_and_drain(&agent, &st, head);
        }
        let started2 = h.join().unwrap();
        let _ = (r1.is_some(), started2);
        let log = st.log.lock().unwrap();
        assert_eq!(2, log.len(), "both run exactly once: {log:?}");
        let inner = agent.cell.lock().unwrap();
        assert!(!inner.running);
        assert!(inner.queue.is_empty());
        assert_eq!(2, inner.value, "no lost update");
    });
}

/// P3d: nested sends onto a FAILED agent queue but never start execution.
#[test]
fn nested_onto_failed_queues_without_running() {
    loom::model(|| {
        let agent = Arc::new(Agent::new(0));
        agent.complete_failed(99);
        agent.enqueue_nested(1); // pre-existing held action
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || {
            let r = a2.enqueue_nested(2);
            let _ = &s2;
            r.is_some()
        });
        let r1 = agent.enqueue_nested(3);
        let started2 = h.join().unwrap();
        assert!(r1.is_none() && !started2, "no runner starts on failure");
        let inner = agent.cell.lock().unwrap();
        assert_eq!(vec![1u32, 2, 3].len(), inner.queue.len());
        assert!(!inner.running);
        assert_eq!(Some(99), inner.error);
        assert!(st.log.lock().unwrap().is_empty(), "nothing ran");
    });
}

/// P3e: a failed agent rejects direct sends (enqueue re-checks under the
/// lock even though send_action pre-checked outside it).
#[test]
fn failed_agent_rejects_sends() {
    loom::model(|| {
        let agent = Arc::new(Agent::new(0));
        agent.complete_failed(99);
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || send(&a2, &s2, 1));
        let r1 = send(&agent, &st, 2);
        let r2 = h.join().unwrap();
        assert!(r1.is_err() && r2.is_err(), "both rejected: {r1:?} {r2:?}");
        assert!(st.log.lock().unwrap().is_empty());
    });
}

/// P3f: complete_failed parks the queue; a single restart resumes it in
/// FIFO order (restart state + one publish per held head, no lost update).
#[test]
fn single_restart_resumes_fifo() {
    loom::model(|| {
        let agent = Agent::failed(0, 99, vec![1, 2]);
        let st = Stats::new();
        assert_eq!(Ok(()), restart(&agent, &st, 10, false));
        let log = st.log.lock().unwrap();
        assert_eq!(vec![1, 2], *log);
        let inner = agent.cell.lock().unwrap();
        assert!(!inner.running);
        assert!(inner.queue.is_empty());
        assert_eq!(None, inner.error);
        assert_eq!(12, inner.value, "restart state + two publishes");
    });
}

/// P3g: restart with :clear-actions drops the held queue and runs nothing.
#[test]
fn restart_clear_drops_queue() {
    loom::model(|| {
        let agent = Agent::failed(0, 99, vec![1, 2]);
        let st = Stats::new();
        assert_eq!(Ok(()), restart(&agent, &st, 10, true));
        assert!(st.log.lock().unwrap().is_empty(), "nothing ran");
        let inner = agent.cell.lock().unwrap();
        assert!(!inner.running);
        assert!(inner.queue.is_empty());
        assert_eq!(None, inner.error);
        assert_eq!(10, inner.value, "restart state only");
    });
}

/// P3h (M3 regression): TWO concurrent restarts on a failed agent with
/// a 2-deep queue. Exactly one publishes (results are exactly one Ok
/// and one does-not-need-restart); the queue drains once, FIFO, with
/// mutual exclusion (asserted inside run_one) and no lost update.
#[test]
fn concurrent_restarts_serialize_single_runner() {
    loom::model(|| {
        let agent = Arc::new(Agent::failed(0, 99, vec![1, 2]));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || restart(&a2, &s2, 10, false));
        let r1 = restart(&agent, &st, 10, false);
        let r2 = h.join().unwrap();
        let mut rs = [r1, r2];
        rs.sort_by_key(|r| r.is_err());
        assert_eq!([Ok(()), Err("does not need restart")], rs);
        let log = st.log.lock().unwrap();
        assert_eq!(vec![1, 2], *log, "each queued action runs once, FIFO");
        assert_eq!(0, st.inflight.load(Ordering::SeqCst));
        assert_eq!(12, agent.cell.lock().unwrap().value, "no lost update");
    });
}

/// P3h2 (M3 regression, overlap shape): same setup with ONLY the
/// mutual-exclusion assert live — no schedule runs two actions
/// concurrently.
#[test]
fn concurrent_restarts_mutual_exclusion() {
    loom::model(|| {
        let agent = Arc::new(Agent::failed(0, 99, vec![1, 2]));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let h = loom::thread::spawn(move || restart(&a2, &s2, 10, false));
        let r1 = restart(&agent, &st, 10, false);
        let r2 = h.join().unwrap();
        let _ = (r1, r2);
        // Only run_one's `inflight <= 1` assert is live here.
    });
}

/// P3i (M3 regression): two concurrent restarts on a 1-deep queue plus
/// a racing send. The restarts serialize (one Ok, one rejected); the
/// send either lands while failed (rejected, held head runs once) or
/// after the clear (both run once, no lost update).
#[test]
fn concurrent_restart_plus_send_serializes() {
    // Three threads explore unbounded for hours; bound 3 keeps every
    // schedule that matters (verified: the pre-fix double-publish
    // still fails at bound 3 in 0.1s, the fix passes in 0.2s).
    let mut explore = loom::model::Builder::new();
    explore.preemption_bound = Some(3);
    explore.check(|| {
        let agent = Arc::new(Agent::failed(0, 99, vec![1]));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let (a3, s3) = (agent.clone(), st.clone());
        let h1 = loom::thread::spawn(move || restart(&a2, &s2, 10, false));
        let h2 = loom::thread::spawn(move || send(&a3, &s3, 2));
        let r1 = restart(&agent, &st, 10, false);
        let (r2, rs) = (h1.join().unwrap(), h2.join().unwrap());
        let mut rrs = [r1, r2];
        rrs.sort_by_key(|r| r.is_err());
        assert_eq!([Ok(()), Err("does not need restart")], rrs);
        let log = st.log.lock().unwrap();
        let mut sorted = log.clone();
        sorted.sort();
        if rs.is_err() {
            assert_eq!(vec![1], sorted, "held head runs once: {log:?}");
        } else {
            assert_eq!(vec![1, 2], sorted, "both actions run once: {log:?}");
            assert_eq!(
                12,
                agent.cell.lock().unwrap().value,
                "no lost update (send accepted)"
            );
        }
    });
}

/// P3i2 (M3 regression, overlap shape): P3i with ONLY the
/// mutual-exclusion assert live.
#[test]
fn concurrent_restart_plus_send_mutual_exclusion() {
    // Same bound as above (same 3-thread schedule space).
    let mut explore = loom::model::Builder::new();
    explore.preemption_bound = Some(3);
    explore.check(|| {
        let agent = Arc::new(Agent::failed(0, 99, vec![1]));
        let st = Stats::new();
        let (a2, s2) = (agent.clone(), st.clone());
        let (a3, s3) = (agent.clone(), st.clone());
        let h1 = loom::thread::spawn(move || restart(&a2, &s2, 10, false));
        let h2 = loom::thread::spawn(move || send(&a3, &s3, 2));
        let r1 = restart(&agent, &st, 10, false);
        let (r2, rs) = (h1.join().unwrap(), h2.join().unwrap());
        let _ = (r1, r2, rs);
        // Only run_one's `inflight <= 1` assert is live here.
    });
}
