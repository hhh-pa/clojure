// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S19 STM loom mirror: the commit lock order (step 4 — ascending
//! ids, spin-then-barge) and revalidation (step 3) under racing
//! committers, modeled branch-for-branch after `stm.rs` with the
//! spin bound as the model parameter (`MODEL_SPIN = 2`). Histories,
//! validators, watches, and sends are elided (single-version reads).
//! The racing test retries to commit like a real transaction (each
//! thread commits once, so attempts stay finite on every schedule —
//! every failure is covered by another thread's single commit); the
//! skew and barge tests run single attempts, like
//! `loom_var_lock.rs`. The real retry loop is additionally pinned by
//! `stm_retry_under_contention` in `tests/stm.rs`.

use std::collections::BTreeMap;

use loom::sync::Arc;
use loom::sync::Mutex;
use loom::sync::MutexGuard;
use loom::sync::atomic::{AtomicUsize, Ordering};

/// The model spin bound (the real path uses 64 — §2c; the mirror
/// pins the shape, over two threads and two refs).
const MODEL_SPIN: usize = 2;

/// A model ref: one current value plus its commit point (histories
/// elided — every read observes current).
struct ModelRef {
    value: usize,
    point: usize,
}

/// A model attempt: the snapshot reads, the computed writes, and the
/// ensured ids.
#[derive(Default)]
struct ModelTxn {
    reads: BTreeMap<usize, usize>,
    writes: BTreeMap<usize, usize>,
    ensures: Vec<usize>,
}

/// The model world: two refs, the commit clock, and the acquisition
/// log (each entry tags its thread and attempt — every
/// per-(thread, attempt) subsequence must ascend).
struct ModelWorld {
    refs: Vec<Arc<Mutex<ModelRef>>>,
    clock: AtomicUsize,
    log: Mutex<Vec<(usize, usize, usize)>>,
}

impl ModelWorld {
    fn new() -> Self {
        let cell = |value| Arc::new(Mutex::new(ModelRef { value, point: 0 }));
        Self {
            refs: vec![cell(0), cell(0)],
            clock: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
        }
    }
}

/// Read a ref into the transaction (single-version snapshot —
/// records first-read-wins, like `tx_resolve`).
fn model_read(world: &Arc<ModelWorld>, tx: &mut ModelTxn, id: usize) -> usize {
    let state = world.refs[id].lock().unwrap();
    let (value, point) = (state.value, state.point);
    drop(state);
    tx.reads.entry(id).or_insert(point);
    value
}

/// A computed write (records no read of its own).
fn model_write(tx: &mut ModelTxn, id: usize, value: usize) {
    tx.writes.insert(id, value);
}

/// Ensure (reads, then pins the id — like `native_ensure`).
fn model_ensure(world: &Arc<ModelWorld>, tx: &mut ModelTxn, id: usize) -> usize {
    let value = model_read(world, tx, id);
    tx.ensures.push(id);
    value
}

/// Step 4, mirrored (`spin_acquire`): ascending ids (asserted —
/// callers sort like `commit_targets`), `MODEL_SPIN` full `try_lock`
/// rounds, then the blocking barge in the same order. Guards return
/// aligned with `targets`.
fn model_acquire<'a>(
    log: &Mutex<Vec<(usize, usize, usize)>>,
    who: usize,
    seq: usize,
    targets: &[(usize, &'a Mutex<ModelRef>)],
) -> Vec<MutexGuard<'a, ModelRef>> {
    let mut ids: Vec<usize> = targets.iter().map(|(id, _)| *id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(ids.len(), sorted.len(), "duplicate lock target");
    ids.sort();
    assert_eq!(ids, sorted, "lock targets ascend");
    for _ in 0..MODEL_SPIN {
        let mut guards = Vec::with_capacity(targets.len());
        let mut complete = true;
        for (_, lock) in targets {
            match lock.try_lock() {
                Ok(guard) => guards.push(guard),
                Err(_) => {
                    complete = false;
                    break;
                }
            }
        }
        if complete {
            for (id, _) in targets {
                log.lock().unwrap().push((who, seq, *id));
            }
            return guards;
        }
    }
    targets
        .iter()
        .map(|(id, lock)| {
            log.lock().unwrap().push((who, seq, *id));
            lock.lock().unwrap()
        })
        .collect()
}

/// Steps 5+6, mirrored (`revalidate` + install): every recorded
/// read's current point still equals the recorded point, else abort
/// (guards drop); on success stamp one fresh point over every write.
fn model_commit(
    world: &Arc<ModelWorld>,
    tx: &ModelTxn,
    targets: &[(usize, &Mutex<ModelRef>)],
    guards: &mut [MutexGuard<ModelRef>],
) -> bool {
    for (id, point) in tx.reads.iter() {
        let current = match targets.iter().position(|(known, _)| known == id) {
            Some(i) => guards[i].point,
            None => world.refs[*id].lock().unwrap().point,
        };
        if current != *point {
            return false;
        }
    }
    let fresh = world.clock.fetch_add(1, Ordering::SeqCst) + 1;
    for (id, value) in tx.writes.iter() {
        let i = targets
            .iter()
            .position(|(known, _)| known == id)
            .expect("model writes lock");
        guards[i].value = *value;
        guards[i].point = fresh;
    }
    true
}

/// One attempt: lock the write+ensure set ascending, commit-or-abort.
fn attempt(
    world: &Arc<ModelWorld>,
    log: &Mutex<Vec<(usize, usize, usize)>>,
    who: usize,
    seq: usize,
    tx: &mut ModelTxn,
) -> bool {
    let mut ids: Vec<usize> = tx
        .writes
        .keys()
        .copied()
        .chain(tx.ensures.iter().copied())
        .collect();
    ids.sort();
    ids.dedup();
    let mutexes: Vec<&Mutex<ModelRef>> = ids.iter().map(|id| world.refs[*id].as_ref()).collect();
    let targets: Vec<(usize, &Mutex<ModelRef>)> = ids.into_iter().zip(mutexes).collect();
    let mut guards = model_acquire(log, who, seq, &targets);
    model_commit(world, tx, &targets, &mut guards)
}

/// Assert every (thread, attempt) acquisition subsequence ascends
/// (the global lock order — deadlock-freedom's mechanism).
fn assert_log_ascends(log: &Mutex<Vec<(usize, usize, usize)>>) {
    let log = log.lock().unwrap();
    let mut attempts: Vec<(usize, usize)> = log.iter().map(|(w, s, _)| (*w, *s)).collect();
    attempts.sort();
    attempts.dedup();
    for (who, seq) in attempts {
        let ids: Vec<usize> = log
            .iter()
            .filter_map(|(w, s, id)| {
                if (*w, *s) == (who, seq) {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "thread {who} attempt {seq} acquires ascending");
    }
}

/// One racing increment to commit: read both refs, write +1, retry
/// on abort (like `sync_begin` — the fresh attempt re-reads).
fn race_to_commit(world: &Arc<ModelWorld>, who: usize) {
    let mut seq = 0;
    loop {
        let mut tx = ModelTxn::default();
        let x = model_read(world, &mut tx, 0);
        let y = model_read(world, &mut tx, 1);
        model_write(&mut tx, 0, x + 1);
        model_write(&mut tx, 1, y + 1);
        if attempt(world, &world.log, who, seq, &mut tx) {
            return;
        }
        seq += 1;
    }
}

#[test]
fn stm_racing_increments_commit() {
    // Two threads increment both refs, retrying to commit: both
    // txns commit on every schedule (counter hits 2 — no lost
    // update, no deadlock).
    loom::model(|| {
        let world = Arc::new(ModelWorld::new());
        let first = world.clone();
        let second = world.clone();
        let a = loom::thread::spawn(move || race_to_commit(&first, 0));
        let b = loom::thread::spawn(move || race_to_commit(&second, 1));
        a.join().unwrap();
        b.join().unwrap();
        assert_log_ascends(&world.log);
        assert_eq!(world.refs[0].lock().unwrap().value, 2);
        assert_eq!(world.refs[1].lock().unwrap().value, 2);
    });
}

#[test]
fn stm_ensure_blocks_write_skew() {
    // The classic skew (A: `s = r + 1` ensuring `r`; B: `r = s + 1`
    // ensuring `s`, from (0, 0)): single attempts can commit one, the
    // other, both (serially), or neither — but never the skewed
    // (1, 1), every schedule.
    loom::model(|| {
        let world = Arc::new(ModelWorld::new());
        let first = world.clone();
        let second = world.clone();
        let a = loom::thread::spawn(move || {
            let mut tx = ModelTxn::default();
            let r = model_ensure(&first, &mut tx, 0);
            model_write(&mut tx, 1, r + 1);
            attempt(&first, &first.log, 0, 0, &mut tx);
        });
        let b = loom::thread::spawn(move || {
            let mut tx = ModelTxn::default();
            let s = model_ensure(&second, &mut tx, 1);
            model_write(&mut tx, 0, s + 1);
            attempt(&second, &second.log, 1, 0, &mut tx);
        });
        a.join().unwrap();
        b.join().unwrap();
        assert_log_ascends(&world.log);
        let r = world.refs[0].lock().unwrap().value;
        let s = world.refs[1].lock().unwrap().value;
        assert!((r, s) != (1, 1), "write skew slipped through: ({r}, {s})");
    });
}

#[test]
fn stm_contended_acquire_barges_then_proceeds() {
    // A commit racing a held cell spins out, barges, and proceeds
    // once the holder releases (the §2c shape).
    loom::model(|| {
        let world = Arc::new(ModelWorld::new());
        let holder = world.clone();
        let waiter = world.clone();
        let a = loom::thread::spawn(move || {
            // A nested critical section over both refs (the commit
            // shape): the waiter spins out against it, then barges.
            let first = holder.refs[0].lock().unwrap();
            let second = holder.refs[1].lock().unwrap();
            let _ = (first.value, second.value);
            drop(second);
            drop(first);
        });
        let b = loom::thread::spawn(move || {
            let mut tx = ModelTxn::default();
            let x = model_read(&waiter, &mut tx, 0);
            model_write(&mut tx, 0, x + 1);
            assert!(attempt(&waiter, &waiter.log, 1, 0, &mut tx));
        });
        a.join().unwrap();
        b.join().unwrap();
        assert_log_ascends(&world.log);
        assert_eq!(world.refs[0].lock().unwrap().value, 1);
        assert_eq!(world.refs[1].lock().unwrap().value, 0);
    });
}
