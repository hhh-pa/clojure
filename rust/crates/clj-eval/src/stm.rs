// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S19 STM: refs (`ref`, the history accessors), transactions
//! (`dosync`/`sync`/`io!`), the updaters (`alter`/`commute`/
//! `ref-set`/`ensure`), and the MVCC commit path. Oracle:
//! `docs/zero-oos-s19-stm/probes/`; rules: the S19 specification
//! (§1/§2 — the commit steps below cite theirs).
//!
//! The running transaction lives in the [`Evaluator`] (`tx`), never
//! in `Bindings` (§1h): same-thread `bound-fn` calls observe it
//! (the evaluator field survives `apply_closure`'s re-push), while
//! cross-thread conveyance (`fork_for_thread`) drops it. Retries
//! unwind with the internal [`StmRetry`] panic, which user
//! `try`/`catch` cannot intercept (unlike the JVM's catchable
//! `RetryEx` — a deliberate hardening).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use clj_value::{Agent, Keyword, Map, QueuedAction, Ref, RefState, ThrowClass, Value};

use crate::classemu::identical_values;
use crate::concurrency::{atom_options, enqueue_send_item, validate_new};
use crate::core::{cce_err, is_ifn, nth_index};
use crate::env::Env;
use crate::error::{NativeError, Signal};
use crate::eval::Evaluator;
use crate::expr::Expr;
use crate::ns::{NativeFnPtr, check_arity, npe, signal_to_native};
use crate::vars::check_min_arity;

/// Spin rounds before the commit barge (§1g step 4; §2c).
const MAX_SPIN_RETRIES: u32 = 64;

/// The commit clock (§1g): process-global, since refs are `Send +
/// Sync` values that cross evaluator lineages (agent state, promise
/// delivery) — a per-lineage clock would corrupt snapshot isolation.
/// Values are never user-visible, so parallel tests cannot observe
/// each other through it.
static COMMIT_CLOCK: AtomicU64 = AtomicU64::new(0);

/// Active-reader start points (§1g trim-retain rule): pushed at
/// begin, removed at end (one entry per open outermost transaction).
static ACTIVE_READERS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// The internal conflict restart: unwinds to the attempt loop (user
/// code cannot catch it — [`sync_begin`] is the only catcher).
#[derive(Debug)]
struct StmRetry;

/// Raise the internal `Retry`. The unwind may pass held commit
/// guards (poisoning them — every lock site recovers via
/// `into_inner`), which is why the attempt loop catches only this
/// payload and drops everything with it.
fn raise_retry() -> ! {
    std::panic::panic_any(StmRetry);
}

/// A running transaction (§1g): the snapshot point, the nesting
/// depth, the read versions, the tx-local writes, every touched ref
/// (a memo — commits lock through it), the commute records in issue
/// order, the pre-first-commute snapshots (a memo — the step-2 replay
/// base), the set-guard and ensure sets, and the deferred sends.
pub(crate) struct Transaction {
    start_point: u64,
    depth: u32,
    reads: BTreeMap<u64, u64>,
    writes: BTreeMap<u64, Value>,
    touched: BTreeMap<u64, Ref>,
    commutes: Vec<(u64, Value, Vec<Value>)>,
    commute_base: BTreeMap<u64, Value>,
    commuted: BTreeSet<u64>,
    ensures: BTreeSet<u64>,
    pub(crate) outbox: Vec<(Agent, QueuedAction)>,
}

impl Transaction {
    fn new(start_point: u64) -> Self {
        Self {
            start_point,
            depth: 0,
            reads: BTreeMap::new(),
            writes: BTreeMap::new(),
            touched: BTreeMap::new(),
            commutes: Vec::new(),
            commute_base: BTreeMap::new(),
            commuted: BTreeSet::new(),
            ensures: BTreeSet::new(),
            outbox: Vec::new(),
        }
    }
}

/// The running transaction (every commit helper runs under one —
/// `sync` joins or begins before any updater, commute re-run, or
/// validator can observe the evaluator).
fn tx_ref(eval: &Evaluator) -> &Transaction {
    eval.tx.as_ref().expect("STM runs under a transaction")
}

/// The running transaction, mutably (see [`tx_ref`]).
fn tx_mut(eval: &mut Evaluator) -> &mut Transaction {
    eval.tx.as_mut().expect("STM runs under a transaction")
}

/// Lock the readers registry (single-line recovery — held across no
/// user code, so poison is unreachable).
fn lock_readers() -> MutexGuard<'static, Vec<u64>> {
    ACTIVE_READERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Remove one reader entry (the opener's own — a duplicate start
/// point from another transaction survives).
fn remove_reader(start: u64) {
    let mut readers = lock_readers();
    if let Some(i) = readers.iter().position(|s| *s == start) {
        readers.swap_remove(i);
    }
}

/// The OTHER active readers' start points (one occurrence of the
/// committer's own start excluded — it is leaving).
fn other_readers(start: u64) -> Vec<u64> {
    let mut readers = lock_readers().clone();
    if let Some(i) = readers.iter().position(|s| *s == start) {
        readers.swap_remove(i);
    }
    readers
}

/// Open an attempt: a fresh start point, a registered reader, a fresh
/// transaction (the previous attempt's reads, writes, and outbox die
/// with it — §1g).
fn begin_attempt(eval: &mut Evaluator) {
    let start = COMMIT_CLOCK.load(Ordering::SeqCst);
    lock_readers().push(start);
    eval.tx = Some(Transaction::new(start));
}

/// Close an attempt on an abort path (retry, body throw, commit
/// throw, or a foreign panic): drop the transaction and unregister
/// its reader (committed attempts pop both at step 8 instead).
fn end_attempt(eval: &mut Evaluator) {
    if let Some(tx) = eval.tx.take() {
        remove_reader(tx.start_point);
    }
}

/// One attempt's outcome: committed, conflicted (retry with a fresh
/// start point), or failed (the signal escapes the `sync` form).
enum AttemptOutcome {
    Done(Value),
    Retry,
    Throw(Signal),
}

/// The transaction forms (split from [`eval`](Evaluator::eval)
/// for the complexity budget — `sync` and `io!` share one dispatch
/// arm; the analyzer-built shape picks the path).
pub(crate) fn eval_sync_or_io(
    eval: &mut Evaluator,
    expr: &Expr,
    body: &[Expr],
    env: &Env,
) -> Result<Value, Signal> {
    if matches!(expr, Expr::Sync { .. }) {
        eval_sync(eval, body, env)
    } else {
        eval_io(eval, body, env)
    }
}

/// `(sync flags body…)` (and `dosync`, which expands to it): join the
/// running transaction or begin one (§1b/§1g).
pub(crate) fn eval_sync(eval: &mut Evaluator, body: &[Expr], env: &Env) -> Result<Value, Signal> {
    if eval.tx.is_some() {
        sync_join(eval, body, env)
    } else {
        sync_begin(eval, body, env)
    }
}

/// The join path: one nesting level deeper, same transaction (one
/// commit, one watch round, one outbox — §1b).
fn sync_join(eval: &mut Evaluator, body: &[Expr], env: &Env) -> Result<Value, Signal> {
    if let Some(tx) = eval.tx.as_mut() {
        tx.depth += 1;
    }
    let out = eval.eval_body(body, env);
    if let Some(tx) = eval.tx.as_mut() {
        tx.depth -= 1;
    }
    out
}

/// The begin path: fresh attempts until one commits (retries are
/// unbounded in COUNT like the JVM — only the spin phase is bounded;
/// user throws and vetoes abort with the attempt's outbox). A retry
/// unwinds past the attempt's binding frames (`binding`,
/// `with-bindings`, conveyed closures), so each retry truncates the
/// bindings stack to the entry depth — otherwise stale frames would
/// leak into the next attempt (roots restore via the `with-redefs`
/// guard instead; `finally` runs at `eval_try`).
fn sync_begin(eval: &mut Evaluator, body: &[Expr], env: &Env) -> Result<Value, Signal> {
    let depth = eval.bindings_depth();
    loop {
        begin_attempt(eval);
        let outcome = catch_unwind(AssertUnwindSafe(|| run_attempt(eval, body, env)));
        match outcome {
            Ok(AttemptOutcome::Done(value)) => {
                end_attempt(eval);
                return Ok(value);
            }
            Ok(AttemptOutcome::Retry) => {
                end_attempt(eval);
                eval.pop_bindings(depth);
            }
            Ok(AttemptOutcome::Throw(signal)) => {
                end_attempt(eval);
                return Err(signal);
            }
            Err(payload) => {
                end_attempt(eval);
                if payload.is::<StmRetry>() {
                    eval.pop_bindings(depth);
                    continue;
                }
                resume_unwind(payload);
            }
        }
    }
}

/// One attempt: run the body, then commit (a body throw discards the
/// attempt — nothing commits, no watches fire, no sends dispatch).
fn run_attempt(eval: &mut Evaluator, body: &[Expr], env: &Env) -> AttemptOutcome {
    match eval.eval_body(body, env) {
        Ok(value) => commit(eval, value),
        Err(signal) => AttemptOutcome::Throw(signal),
    }
}

/// A commit-phase user call's failure as the attempt's outcome
/// (throws propagate unstamped, like body throws; `recur` is
/// impossible — the analyzer cages every one).
fn call_signal(eval: &mut Evaluator, out: Result<Value, Signal>) -> Result<Value, Signal> {
    match out {
        Err(s @ Signal::Recur(_)) => Err(eval.throw(signal_to_native(s))),
        other => other,
    }
}

/// The commit path (outermost `sync` exit only — §1g steps 1–11).
/// User code (commute re-runs, validators) runs BEFORE any guard is
/// held: resolving through `tx_deref` under held guards would block
/// on the transaction's own targets (a validator reading an
/// ensure-only ref self-deadlocks) or a concurrent committer's
/// (cross deadlock) — so the lock phase comes after, and the single
/// revalidation gates the install (a stale rerun base retries like
/// any conflict).
fn commit(eval: &mut Evaluator, body_value: Value) -> AttemptOutcome {
    debug_assert_eq!(tx_ref(eval).depth, 0, "commit runs at the outermost exit");
    if is_read_only(tx_ref(eval)) {
        // The read-only fast path skips the clock bump, not the
        // outbox: a send-only transaction commits successfully, so
        // §1f dispatches its sends (JVM-verified: a bare `(dosync
        // (send a f))` delivers).
        let outbox = pop_commit(eval);
        if let Err(signal) = dispatch_outbox(eval, outbox) {
            return AttemptOutcome::Throw(signal);
        }
        return AttemptOutcome::Done(body_value);
    }
    if let Err(signal) = rerun_commutes(eval) {
        return AttemptOutcome::Throw(signal);
    }
    let validate_ids = write_ids(tx_ref(eval));
    let validated = match validate_writes(eval, &validate_ids) {
        Ok(validated) => validated,
        Err(signal) => return AttemptOutcome::Throw(signal),
    };
    // Targets snapshot after the user-code steps, so rerun- and
    // validator-phase joins are locked members (never brief-lock
    // stragglers at install).
    let targets = commit_targets(tx_ref(eval));
    let mut guards = spin_acquire(&targets);
    if !revalidate(tx_ref(eval), &targets, &guards, &validated) {
        return AttemptOutcome::Retry;
    }
    let install_ids = write_ids(tx_ref(eval));
    let installed = install_writes(eval, &targets, &mut guards, &install_ids);
    let outbox = pop_commit(eval);
    drop(guards);
    if let Err(signal) = notify_refs(eval, &installed) {
        return AttemptOutcome::Throw(signal);
    }
    if let Err(signal) = dispatch_outbox(eval, outbox) {
        return AttemptOutcome::Throw(signal);
    }
    AttemptOutcome::Done(body_value)
}

/// Step 1's guard: no writes, commutes, or ensures (the fast path
/// skips the clock bump — unobservable, avoids contention).
fn is_read_only(tx: &Transaction) -> bool {
    tx.writes.is_empty() && tx.commutes.is_empty() && tx.ensures.is_empty()
}

/// Step 4's lock set: write-set + ensure-set refs in ascending id
/// order (the global order — deadlock-free).
fn commit_targets(tx: &Transaction) -> Vec<(u64, Ref)> {
    let mut ids: Vec<u64> = tx
        .writes
        .keys()
        .copied()
        .chain(tx.ensures.iter().copied())
        .collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|id| (id, tx.touched[&id].clone()))
        .collect()
}

/// The written ids in ascending order (commit order across refs is
/// ascending Ref id — validate + notify share it, §1e).
fn write_ids(tx: &Transaction) -> Vec<u64> {
    tx.writes.keys().copied().collect()
}

/// Step 4: spin up to `MAX_SPIN_RETRIES` full `try_lock` rounds over
/// the set (releasing all on any failure), then barge with blocking
/// acquisition in the same ascending order (§2c).
fn spin_acquire(targets: &[(u64, Ref)]) -> Vec<MutexGuard<'_, RefState>> {
    for _ in 0..MAX_SPIN_RETRIES {
        if let Some(guards) = try_round(targets) {
            return guards;
        }
    }
    barge_acquire(targets)
}

/// One full `try_lock` round (`None` unless every cell yields —
/// partial guards drop on the way out).
fn try_round(targets: &[(u64, Ref)]) -> Option<Vec<MutexGuard<'_, RefState>>> {
    let mut guards = Vec::with_capacity(targets.len());
    for (_, reff) in targets {
        guards.push(reff.try_lock()?);
    }
    Some(guards)
}

/// The barge phase: blocking acquisition in ascending-id order
/// (deadlock-free by the global order; some holder always commits
/// and releases, so every wait is bounded).
fn barge_acquire(targets: &[(u64, Ref)]) -> Vec<MutexGuard<'_, RefState>> {
    targets.iter().map(|(_, reff)| reff.lock()).collect()
}

/// A locked target's guard index (the targets ascend, so the search
/// is exact — `None` for read-only refs, which never lock).
fn locked_index(targets: &[(u64, Ref)], id: u64) -> Option<usize> {
    targets.binary_search_by_key(&id, |(known, _)| *known).ok()
}

/// Step 5: every recorded read's current point still equals the
/// recorded point (ensured refs ride the same check — the rerun
/// base pins through the reads its first commute recorded), and
/// every validated validator slot still holds the same fn; a
/// mismatch drops the locks and retries with a fresh start point.
fn revalidate(
    tx: &Transaction,
    targets: &[(u64, Ref)],
    guards: &[MutexGuard<RefState>],
    validated: &BTreeMap<u64, Option<Value>>,
) -> bool {
    for (id, point) in tx.reads.iter() {
        if current_point(tx, targets, guards, *id) != *point {
            return false;
        }
    }
    validators_unchanged(targets, guards, validated)
}

/// The step-3 validator snapshot still matches every slot (a
/// `set-validator!` landing between validation and install would
/// install a value its own validator never saw — retry instead).
/// Reads through the held guards; validated ids are always locked
/// members. Identity is per-kind (`identical?`): immutable slots by
/// value, closures by allocation.
fn validators_unchanged(
    targets: &[(u64, Ref)],
    guards: &[MutexGuard<RefState>],
    validated: &BTreeMap<u64, Option<Value>>,
) -> bool {
    for (id, expected) in validated.iter() {
        let i = locked_index(targets, *id).expect("validated ids are locked");
        let same = match (expected, &guards[i].validator) {
            (None, None) => true,
            (Some(a), Some(b)) => identical_values(a, b),
            _ => false,
        };
        if !same {
            return false;
        }
    }
    true
}

/// A read ref's current point (locked cells read through the held
/// guard; read-only refs try briefly — a held cell means a
/// concurrent committer owns it, so retry like any conflict
/// instead of blocking into a lock cycle).
fn current_point(
    tx: &Transaction,
    targets: &[(u64, Ref)],
    guards: &[MutexGuard<RefState>],
    id: u64,
) -> u64 {
    if let Some(i) = locked_index(targets, id) {
        return guards[i].point;
    }
    match tx.touched[&id].try_lock() {
        Some(state) => state.point,
        None => raise_retry(),
    }
}

/// Step 2: re-run each ref's commutes in issue order over (latest
/// committed ⊕ this txn's earlier non-commute writes) — i.e. over
/// the pre-first-commute snapshot, which the step-5 revalidation
/// pins to latest (a stale base retries like any conflict — §1c).
/// Runs lock-free, before any guard is held. A rerun fn that
/// commutes records tx-locally but the late commute never reruns
/// this commit — and does not survive even tx-locally past it
/// (DIVERGENCE, oracle `u-latecommute-cme`: the JVM fail-fasts
/// with `ConcurrentModificationException` — the port snapshots
/// instead of growing the shared `ThrowClass` for a pathological
/// corner).
fn rerun_commutes(eval: &mut Evaluator) -> Result<(), Signal> {
    let plan: Vec<(u64, Value, Vec<Value>)> = tx_ref(eval).commutes.clone();
    if plan.is_empty() {
        return Ok(());
    }
    let mut finals: BTreeMap<u64, Value> = BTreeMap::new();
    for (id, fun, args) in &plan {
        let base = rerun_base(eval, *id, &finals);
        let mut f_args = Vec::with_capacity(args.len() + 1);
        f_args.push(base);
        f_args.extend(args.iter().cloned());
        let out = eval.call(fun, &f_args);
        let next = call_signal(eval, out)?;
        finals.insert(*id, next);
    }
    let tx = tx_mut(eval);
    for (id, value) in finals {
        tx.writes.insert(id, value);
    }
    Ok(())
}

/// One rerun's base: the running final for a re-commuted ref, else
/// its pre-first-commute snapshot (recorded in-txn — always present
/// for a planned ref).
fn rerun_base(eval: &mut Evaluator, id: u64, finals: &BTreeMap<u64, Value>) -> Value {
    if let Some(value) = finals.get(&id) {
        return value.clone();
    }
    tx_ref(eval)
        .commute_base
        .get(&id)
        .cloned()
        .expect("rerun base recorded at first commute")
}

/// Step 3: validate every written ref's final value, ascending id (a
/// veto aborts — never retries), returning the validated validator
/// slots. Validators run tx-live with no guards held (lock-free
/// like the reruns); their writes install but skip validation (the
/// id list snapshots before the loop — oracle `u-valphase-*`). The
/// JVM validates under its commit locks; the port validates
/// lock-free (deadlock-freedom) and closes the swap window by
/// rechecking the snapshot at step 5 instead (a swap retries).
fn validate_writes(
    eval: &mut Evaluator,
    ids: &[u64],
) -> Result<BTreeMap<u64, Option<Value>>, Signal> {
    let mut validated = BTreeMap::new();
    for id in ids {
        let (candidate, validator) = write_candidate(eval, *id);
        if let Some(vf) = validator.clone() {
            if let Err(error) = validate_new(eval, &vf, &candidate) {
                return Err(eval.throw(error));
            }
        }
        validated.insert(*id, validator);
    }
    Ok(validated)
}

/// One validation's candidate plus its current validator (the slot
/// reads briefly — validation holds no guards).
fn write_candidate(eval: &mut Evaluator, id: u64) -> (Value, Option<Value>) {
    let candidate = tx_ref(eval)
        .writes
        .get(&id)
        .cloned()
        .expect("validated ids are written");
    let validator = tx_ref(eval)
        .touched
        .get(&id)
        .expect("validated ids are touched")
        .lock()
        .validator
        .clone();
    (candidate, validator)
}

/// One installed ref: the handle plus its old/new values and the
/// watch snapshot step 9 fires.
struct Installed {
    reff: Ref,
    old: Value,
    new: Value,
    watches: Map,
}

/// Steps 6–7: bump the clock once, install every new current with the
/// fresh point, push each old current onto its history, and trim to
/// the retain rule (ensure-only commits stamp nothing — no bump).
fn install_writes(
    eval: &mut Evaluator,
    targets: &[(u64, Ref)],
    guards: &mut [MutexGuard<RefState>],
    ids: &[u64],
) -> Vec<Installed> {
    if ids.is_empty() {
        return Vec::new();
    }
    let fresh = COMMIT_CLOCK.fetch_add(1, Ordering::SeqCst) + 1;
    let start = tx_ref(eval).start_point;
    let others = other_readers(start);
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(install_one(eval, targets, guards, *id, fresh, &others));
    }
    out
}

/// Install one ref through its held guard (every installed id is a
/// locked member — targets snapshot after the user-code steps, and
/// nothing joins between the snapshot and the install).
fn install_one(
    eval: &mut Evaluator,
    targets: &[(u64, Ref)],
    guards: &mut [MutexGuard<RefState>],
    id: u64,
    fresh: u64,
    others: &[u64],
) -> Installed {
    let new = tx_ref(eval)
        .writes
        .get(&id)
        .cloned()
        .expect("installed ids are written");
    let i = locked_index(targets, id).expect("installed ids are locked");
    let reff = targets[i].1.clone();
    let (old, watches) = install_state(&mut guards[i], new.clone(), fresh, others);
    Installed {
        reff,
        old,
        new,
        watches,
    }
}

/// Install one state: swap in the new current with the fresh point,
/// push the old current onto the history front, trim to the retain
/// rule — answering the old value plus the watch snapshot.
fn install_state(state: &mut RefState, new: Value, fresh: u64, others: &[u64]) -> (Value, Map) {
    let old = std::mem::replace(&mut state.value, new);
    let old_point = std::mem::replace(&mut state.point, fresh);
    state.history.push_front((old_point, old.clone()));
    trim_history(state, others);
    (old, state.watches.clone())
}

/// Step 7: drop oldest while longer than `min_history`, except a
/// sole-pinned entry at-or-under `max_history` is retained (past
/// `max_history` the drop is forced and the pinned reader retries).
fn trim_history(state: &mut RefState, others: &[u64]) {
    while state.history.len() as i64 > state.min_history {
        let Some(&(oldest, _)) = state.history.back() else {
            break;
        };
        if state.history.len() as i64 <= state.max_history
            && sole_pinned(&state.history, oldest, others)
        {
            break;
        }
        state.history.pop_back();
    }
}

/// Whether `oldest` is the sole remaining version at-or-before some
/// other active reader's start point (the current value always
/// postdates every reader — the fresh point postdates the clock).
fn sole_pinned(history: &VecDeque<(u64, Value)>, oldest: u64, others: &[u64]) -> bool {
    others
        .iter()
        .any(|s| oldest <= *s && second_newer_than(history, *s))
}

/// Whether every history entry but the oldest postdates `s` (the
/// oldest is then the sole version at-or-before `s` — the chain
/// runs newest-first, so the oldest goes first under reversal).
fn second_newer_than(history: &VecDeque<(u64, Value)>, start: u64) -> bool {
    history
        .iter()
        .rev()
        .skip(1)
        .all(|(point, _)| *point > start)
}

/// Step 8: pop the running transaction and its reader entry (locks
/// drop at the call site, before watches and sends run).
fn pop_commit(eval: &mut Evaluator) -> Vec<(Agent, QueuedAction)> {
    let tx = eval.tx.take().expect("commit pops its transaction");
    remove_reader(tx.start_point);
    tx.outbox
}

/// Step 9: fire watches in ascending Ref-id order, each ref's in
/// insertion order (like atoms; nil fns never fire).
fn notify_refs(eval: &mut Evaluator, installed: &[Installed]) -> Result<(), Signal> {
    for n in installed {
        fire_ref_watches(eval, &n.reff, &n.old, &n.new, &n.watches)?;
    }
    Ok(())
}

/// Fire one ref's watches as `[key ref old new]` (the atom shape —
/// failures propagate after installing, oracle `u-watch-throw`).
fn fire_ref_watches(
    eval: &mut Evaluator,
    reff: &Ref,
    old: &Value,
    new: &Value,
    watches: &Map,
) -> Result<(), Signal> {
    let reference = Value::Ref(reff.clone());
    for (key, fun) in watches.iter() {
        if matches!(fun, Value::Nil) {
            continue;
        }
        let args = [key.clone(), reference.clone(), old.clone(), new.clone()];
        let out = eval.call(fun, &args);
        call_signal(eval, out)?;
    }
    Ok(())
}

/// Step 10: dispatch the outbox in issue order through the normal
/// enqueue path (post-commit, like the JVM's deferred sends).
fn dispatch_outbox(eval: &mut Evaluator, outbox: Vec<(Agent, QueuedAction)>) -> Result<(), Signal> {
    for (agent, item) in outbox {
        enqueue_send_item(eval, &agent, item).map_err(|error| eval.throw(error))?;
    }
    Ok(())
}

/// `(io! body…)`: the gate fires before body evaluation (§1b).
pub(crate) fn eval_io(eval: &mut Evaluator, body: &[Expr], env: &Env) -> Result<Value, Signal> {
    if eval.tx.is_some() {
        return Err(eval.throw(NativeError::new(
            ThrowClass::IllegalStateException,
            "I/O in transaction".to_owned(),
        )));
    }
    eval.eval_body(body, env)
}

/// Refuse `await`/`await-for` inside a transaction (checked before
/// the in-action guard — the tx message wins, §1d).
pub(crate) fn check_await_not_in_tx(eval: &Evaluator, name: &str) -> Result<(), NativeError> {
    if eval.tx.is_some() {
        return Err(NativeError::new(
            ThrowClass::IllegalStateException,
            format!("{name} in transaction"),
        ));
    }
    Ok(())
}

/// `deref` on a ref: the tx-local value when this transaction wrote
/// it, else the newest committed version at-or-before the start
/// point (§1g read path); outside a transaction, the current value.
pub(crate) fn tx_deref(eval: &mut Evaluator, reff: &Ref) -> Value {
    match eval.tx.as_mut() {
        Some(tx) => tx_resolve(tx, reff),
        None => reff.lock().value.clone(),
    }
}

/// Resolve a ref inside a transaction (own writes win without
/// recording; snapshot resolves record first-read-wins; no version
/// at-or-before the start point raises the internal `Retry`).
fn tx_resolve(tx: &mut Transaction, reff: &Ref) -> Value {
    let id = reff.id();
    tx.touched.entry(id).or_insert_with(|| reff.clone());
    if let Some(value) = tx.writes.get(&id) {
        return value.clone();
    }
    let (point, value) = snapshot_at_or_retry(reff, tx.start_point);
    tx.reads.entry(id).or_insert(point);
    value
}

/// The newest committed version at-or-before `start` (the current
/// value first, then history newest-first); none raises `Retry` (only
/// a force-trim under contention strands a reader — §1g). The cell
/// lock drops before the raise (never poisoned by it).
fn snapshot_at_or_retry(reff: &Ref, start: u64) -> (u64, Value) {
    let found = {
        let state = reff.lock();
        if state.point <= start {
            Some((state.point, state.value.clone()))
        } else {
            state
                .history
                .iter()
                .find(|(point, _)| *point <= start)
                .map(|(point, value)| (*point, value.clone()))
        }
    };
    match found {
        Some(version) => version,
        None => raise_retry(),
    }
}

const ALTER_NIL: &str = "Cannot invoke \"clojure.lang.Ref.alter(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null";
const COMMUTE_NIL: &str = "Cannot invoke \"clojure.lang.Ref.commute(clojure.lang.IFn, clojure.lang.ISeq)\" because \"ref\" is null";
const REFSET_NIL: &str = "Cannot invoke \"clojure.lang.Ref.set(Object)\" because \"ref\" is null";
const ENSURE_NIL: &str = "Cannot invoke \"clojure.lang.Ref.touch()\" because \"ref\" is null";
const COUNT_NIL: &str =
    "Cannot invoke \"clojure.lang.Ref.getHistoryCount()\" because \"ref\" is null";
const MIN_NIL: &str = "Cannot invoke \"clojure.lang.Ref.getMinHistory()\" because \"ref\" is null";
const MAX_NIL: &str = "Cannot invoke \"clojure.lang.Ref.getMaxHistory()\" because \"ref\" is null";
const APPLY_TO_NIL: &str =
    "Cannot invoke \"clojure.lang.IFn.applyTo(clojure.lang.ISeq)\" because \"fn\" is null";

/// A `Ref` receiver: nil trips the call-shape NPE, anything else
/// non-ref the `Ref` cast.
fn as_stm_ref(value: &Value, nil_message: &str) -> Result<Ref, NativeError> {
    match value {
        Value::Ref(reff) => Ok(reff.clone()),
        Value::Nil => Err(npe(nil_message)),
        other => Err(cce_err(other, "clojure.lang.Ref")),
    }
}

/// The non-nil receiver check for `alter`/`commute` (nil dispatches
/// after the fn check — §1c).
fn check_ref_cc(value: &Value) -> Result<Option<Ref>, NativeError> {
    match value {
        Value::Ref(reff) => Ok(Some(reff.clone())),
        Value::Nil => Ok(None),
        other => Err(cce_err(other, "clojure.lang.Ref")),
    }
}

/// The `alter`/`commute` fn check: nil passes (it fails at apply time
/// with the `applyTo` NPE); non-`IFn`s fail the cast now (§1c).
fn check_alter_fn(fun: &Value) -> Result<(), NativeError> {
    if matches!(fun, Value::Nil) || is_ifn(fun) {
        Ok(())
    } else {
        Err(cce_err(fun, "clojure.lang.IFn"))
    }
}

/// Apply an `alter`/`commute` fn to the base plus args (a nil fn is
/// the JVM's `applyTo` NPE — nil passes the checkcast, then fails at
/// the call).
fn apply_alter_fn(
    eval: &mut Evaluator,
    fun: &Value,
    base: &Value,
    extra: &[Value],
) -> Result<Value, NativeError> {
    if matches!(fun, Value::Nil) {
        return Err(npe(APPLY_TO_NIL));
    }
    let mut f_args = Vec::with_capacity(extra.len() + 1);
    f_args.push(base.clone());
    f_args.extend(extra.iter().cloned());
    eval.call(fun, &f_args).map_err(signal_to_native)
}

/// The transaction gate (§1c): outside a transaction every updater
/// fails with the JVM text (after the arity/cast checks).
fn check_tx(eval: &Evaluator) -> Result<(), NativeError> {
    if eval.tx.is_none() {
        return Err(NativeError::new(
            ThrowClass::IllegalStateException,
            "No transaction running".to_owned(),
        ));
    }
    Ok(())
}

/// The §1c set-guard: no `alter`/`ref-set` after a `commute` on the
/// same ref (fires at the call — after the alter fn ran).
fn check_set_guard(tx: &Transaction, reff: &Ref) -> Result<(), NativeError> {
    if tx.commuted.contains(&reff.id()) {
        return Err(NativeError::new(
            ThrowClass::IllegalStateException,
            "Can't set after commute".to_owned(),
        ));
    }
    Ok(())
}

fn native_alter(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/alter", args, 2)?;
    let maybe = check_ref_cc(&args[0])?;
    check_alter_fn(&args[1])?;
    let reff = maybe.ok_or_else(|| npe(ALTER_NIL))?;
    check_tx(eval)?;
    alter_in_tx(eval, &reff, &args[1], &args[2..])
}

/// `alter` inside a transaction: read the base, apply the fn (which
/// runs even when the set-guard will throw — oracle
/// `u-setaftercommute-throwfn`), guard, install.
fn alter_in_tx(
    eval: &mut Evaluator,
    reff: &Ref,
    fun: &Value,
    extra: &[Value],
) -> Result<Value, NativeError> {
    let base = tx_deref(eval, reff);
    let next = apply_alter_fn(eval, fun, &base, extra)?;
    check_set_guard(tx_ref(eval), reff)?;
    tx_mut(eval).writes.insert(reff.id(), next.clone());
    Ok(next)
}

fn native_commute(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/commute", args, 2)?;
    let maybe = check_ref_cc(&args[0])?;
    check_alter_fn(&args[1])?;
    let reff = maybe.ok_or_else(|| npe(COMMUTE_NIL))?;
    check_tx(eval)?;
    commute_in_tx(eval, &reff, &args[1], &args[2..])
}

/// `commute` inside a transaction: apply the fn for the tx-local
/// value and record `(ref, fun, args)` for the commit re-run (§1c).
fn commute_in_tx(
    eval: &mut Evaluator,
    reff: &Ref,
    fun: &Value,
    extra: &[Value],
) -> Result<Value, NativeError> {
    let base = tx_deref(eval, reff);
    let next = apply_alter_fn(eval, fun, &base, extra)?;
    let tx = tx_mut(eval);
    tx.commute_base.entry(reff.id()).or_insert(base);
    tx.writes.insert(reff.id(), next.clone());
    tx.commuted.insert(reff.id());
    tx.commutes.push((reff.id(), fun.clone(), extra.to_vec()));
    Ok(next)
}

fn native_ref_set(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ref-set", args, &[2])?;
    let reff = as_stm_ref(&args[0], REFSET_NIL)?;
    check_tx(eval)?;
    check_set_guard(tx_ref(eval), &reff)?;
    // Blind (records no read — bargain semantics): last committer
    // wins after revalidation of what was actually read.
    let tx = tx_mut(eval);
    tx.touched.entry(reff.id()).or_insert_with(|| reff.clone());
    tx.writes.insert(reff.id(), args[1].clone());
    Ok(args[1].clone())
}

fn native_ensure(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ensure", args, &[1])?;
    let reff = as_stm_ref(&args[0], ENSURE_NIL)?;
    check_tx(eval)?;
    let value = tx_deref(eval, &reff);
    tx_mut(eval).ensures.insert(reff.id());
    Ok(value)
}

/// The §1a history-bound conversion: `RT.longCast` then checked
/// `long→int` — exactly [`nth_index`]'s ladder (nil quirk, `Number`
/// cast, NaN-is-zero, `Value out of range for long`, truncation
/// toward zero, `integer overflow`).
fn history_bound(value: &Value) -> Result<i64, NativeError> {
    nth_index(value).map(i64::from)
}

fn native_ref(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/ref", args, 1)?;
    let opts = atom_options(&args[1..])?;
    let reff = Ref::new(args[0].clone());
    install_ref_options(eval, &reff, &opts)?;
    Ok(Value::Ref(reff))
}

/// Install `:meta`/`:validator` (setup-reference order — meta first,
/// then the validator over the initial value) plus
/// `:min-history`/`:max-history` through the §1a conversion. Falsy
/// options never install; unknown keys are ignored.
fn install_ref_options(eval: &mut Evaluator, reff: &Ref, opts: &Map) -> Result<(), NativeError> {
    install_ref_meta(reff, opts)?;
    install_ref_validator(eval, reff, opts)?;
    install_ref_bounds(reff, opts)?;
    Ok(())
}

/// Install a ref's `:meta` option (a map or nothing — falsy never
/// installs).
fn install_ref_meta(reff: &Ref, opts: &Map) -> Result<(), NativeError> {
    let key = Value::Keyword(Keyword::new(None, "meta"));
    let Some(meta) = opts.get(&key).filter(|m| m.is_truthy()) else {
        return Ok(());
    };
    match meta {
        Value::Map(mm) => {
            reff.lock().meta = Some(mm.clone());
            Ok(())
        }
        _ => Err(cce_err(meta, "clojure.lang.IPersistentMap")),
    }
}

/// Install a ref's `:validator` option (validated over the initial
/// value, like atoms — a vetoing ctor throws).
fn install_ref_validator(eval: &mut Evaluator, reff: &Ref, opts: &Map) -> Result<(), NativeError> {
    let key = Value::Keyword(Keyword::new(None, "validator"));
    let Some(fun) = opts.get(&key) else {
        return Ok(());
    };
    if !fun.is_truthy() {
        return Ok(());
    }
    if !is_ifn(fun) {
        return Err(cce_err(fun, "clojure.lang.IFn"));
    }
    let initial = reff.lock().value.clone();
    validate_new(eval, fun, &initial)?;
    reff.lock().validator = Some(fun.clone());
    Ok(())
}

/// Install a ref's `:min-history`/`:max-history` options (no
/// cross-constraint, no lower bound — §1a).
fn install_ref_bounds(reff: &Ref, opts: &Map) -> Result<(), NativeError> {
    let min_key = Value::Keyword(Keyword::new(None, "min-history"));
    if let Some(n) = opts.get(&min_key) {
        reff.lock().min_history = history_bound(n)?;
    }
    let max_key = Value::Keyword(Keyword::new(None, "max-history"));
    if let Some(n) = opts.get(&max_key) {
        reff.lock().max_history = history_bound(n)?;
    }
    Ok(())
}

fn native_history_count(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ref-history-count", args, &[1])?;
    let reff = as_stm_ref(&args[0], COUNT_NIL)?;
    Ok(Value::Int(reff.lock().history.len() as i64))
}

fn native_min_history(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ref-min-history", args, &[1, 2])?;
    let reff = as_stm_ref(&args[0], MIN_NIL)?;
    if args.len() == 1 {
        return Ok(Value::Int(reff.lock().min_history));
    }
    reff.lock().min_history = history_bound(&args[1])?;
    Ok(Value::Ref(reff))
}

fn native_max_history(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ref-max-history", args, &[1, 2])?;
    let reff = as_stm_ref(&args[0], MAX_NIL)?;
    if args.len() == 1 {
        return Ok(Value::Int(reff.lock().max_history));
    }
    reff.lock().max_history = history_bound(&args[1])?;
    Ok(Value::Ref(reff))
}

/// The S19 natives installed into `clojure.core` by [`boot`](crate::ns::boot).
pub(crate) const STM_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("ref", native_ref),
    ("alter", native_alter),
    ("commute", native_commute),
    ("ref-set", native_ref_set),
    ("ensure", native_ensure),
    ("ref-history-count", native_history_count),
    ("ref-min-history", native_min_history),
    ("ref-max-history", native_max_history),
];

#[cfg(test)]
mod tests {
    use super::*;
    use clj_value::Symbol;
    use clj_value::{AgentAction, AgentPool, Throwable, Var};
    use std::sync::Arc;

    /// A ref with a fabricated point/history (the contention-only
    /// resolve paths are unreachable single-threaded, so the retry
    /// and history-hit arms pin through fabricated cells).
    fn fabricated(point: u64, history: Vec<(u64, Value)>) -> Ref {
        let reff = Ref::new(Value::Int(0));
        let mut state = reff.lock();
        state.point = point;
        state.history = history.into_iter().collect();
        drop(state);
        reff
    }

    #[test]
    fn resolve_hits_history_below_current() {
        // Current too new for the start point, history holds a
        // version at-or-before it: snapshot isolation walks back.
        let reff = fabricated(10, vec![(7, Value::Int(70)), (3, Value::Int(30))]);
        let mut tx = Transaction::new(8);
        assert_eq!(tx_resolve(&mut tx, &reff), Value::Int(70));
        assert_eq!(tx.reads.get(&reff.id()), Some(&7));
    }

    #[test]
    fn resolve_without_a_version_retries() {
        // Nothing at-or-before the start point (a force-trim under
        // contention stranded this reader): the internal `Retry`.
        let reff = fabricated(10, vec![(9, Value::Int(90))]);
        let mut tx = Transaction::new(8);
        let payload = std::panic::catch_unwind(AssertUnwindSafe(|| tx_resolve(&mut tx, &reff)))
            .expect_err("stranded resolve retries");
        assert!(payload.is::<StmRetry>());
        // The raise drops the cell lock first (no poison).
        assert!(reff.try_lock().is_some());
    }

    #[test]
    fn own_writes_win_without_recording() {
        let reff = Ref::new(Value::Int(0));
        let mut tx = Transaction::new(0);
        tx.touched.insert(reff.id(), reff.clone());
        tx.writes.insert(reff.id(), Value::Int(5));
        assert_eq!(tx_resolve(&mut tx, &reff), Value::Int(5));
        assert!(tx.reads.is_empty());
    }

    #[test]
    fn trim_retains_a_sole_pinned_version() {
        // Three entries over min 0 with another reader at 7: the
        // oldest drops (a newer version still covers the reader),
        // then the sole survivor retains.
        let reff = Ref::new(Value::Int(0));
        {
            let mut state = reff.lock();
            state.max_history = 10;
            state.history = vec![(8, Value::Int(8)), (6, Value::Int(6)), (5, Value::Int(5))]
                .into_iter()
                .collect();
            trim_history(&mut state, &[7]);
            let points: Vec<u64> = state.history.iter().map(|(p, _)| *p).collect();
            assert_eq!(points, vec![8, 6]);
        }
    }

    #[test]
    fn trim_force_drops_past_max() {
        // The oldest is sole-pinned but past `max_history`: forced
        // (the pinned reader retries on next access).
        let reff = Ref::new(Value::Int(0));
        {
            let mut state = reff.lock();
            state.min_history = 1;
            state.max_history = 1;
            state.history = vec![(8, Value::Int(8)), (5, Value::Int(5))]
                .into_iter()
                .collect();
            trim_history(&mut state, &[7]);
            let points: Vec<u64> = state.history.iter().map(|(p, _)| *p).collect();
            assert_eq!(points, vec![8]);
        }
    }

    #[test]
    fn trim_to_min_without_readers() {
        // No other readers: exactly trim-to-min (every
        // `r-history-*` row rests on this).
        let reff = Ref::new(Value::Int(0));
        {
            let mut state = reff.lock();
            state.min_history = 1;
            state.history = vec![(8, Value::Int(8)), (5, Value::Int(5))]
                .into_iter()
                .collect();
            trim_history(&mut state, &[]);
            assert_eq!(state.history.len(), 1);
        }
    }

    #[test]
    fn revalidate_retries_on_held_read_only_ref() {
        // A read-only ref held by a concurrent committer: the brief
        // try fails and the attempt retries (never blocks into a
        // lock cycle — R1 H1).
        let held = Ref::new(Value::Int(0));
        let written = Ref::new(Value::Int(1));
        let mut tx = Transaction::new(0);
        tx.touched.insert(held.id(), held.clone());
        tx.touched.insert(written.id(), written.clone());
        tx.reads.insert(held.id(), 0);
        tx.writes.insert(written.id(), Value::Int(2));
        let targets = vec![(written.id(), written.clone())];
        let guards = vec![written.lock()];
        let _foreign = held.lock();
        let payload = std::panic::catch_unwind(AssertUnwindSafe(|| {
            current_point(&tx, &targets, &guards, held.id())
        }))
        .expect_err("held read-only ref retries");
        assert!(payload.is::<StmRetry>());
    }

    /// A `user/s19-retry-once` native raising the internal retry
    /// exactly once (first call), then answering `:ok` — drives
    /// EXACTLY two attempts with no threads or timing (M1).
    fn install_retry_once(eval: &mut Evaluator) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let count = Arc::new(AtomicUsize::new(0));
        eval.install_native(
            "user",
            "s19-retry-once",
            Arc::new(move |_, _| {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    super::raise_retry();
                }
                Ok(Value::Keyword(Keyword::new(None, "ok")))
            }),
        );
    }

    /// Intern `user/<name>` with a root (M1 fixtures).
    fn def_test_var(eval: &mut Evaluator, name: &str, root: Value, dynamic: bool) {
        let sym = Symbol::new(Some("user"), name);
        let id = eval.registry.intern(&sym);
        eval.registry.with_state_mut(id, |st| {
            st.root = Some(root);
            st.dynamic = dynamic;
        });
    }

    /// M1: a retry unwinds past the attempt's `binding` frames —
    /// `sync_begin` truncates to the entry depth, so neither the
    /// next attempt nor the world after sees them.
    #[test]
    fn retry_truncates_binding_frames() {
        let mut eval = crate::ns::boot();
        def_test_var(&mut eval, "s19-tx", Value::Int(0), true);
        install_retry_once(&mut eval);
        let out = eval
            .eval_source("(dosync (binding [user/s19-tx 1] (user/s19-retry-once)))")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(out, Value::Keyword(Keyword::new(None, "ok")));
        // Attempt 1's frame is gone (a leak would still read 1).
        let after = eval.eval_source("user/s19-tx").unwrap().pop().unwrap();
        assert_eq!(after, Value::Int(0));
    }

    /// M1: a retry unwinding through `with-redefs` restores the
    /// saved roots (no global permanent leak).
    #[test]
    fn retry_restores_with_redefs_roots() {
        let mut eval = crate::ns::boot();
        def_test_var(&mut eval, "s19-rr", Value::Int(0), false);
        install_retry_once(&mut eval);
        let out = eval
            .eval_source("(dosync (with-redefs [user/s19-rr 9] (user/s19-retry-once)))")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(out, Value::Keyword(Keyword::new(None, "ok")));
        let after = eval.eval_source("user/s19-rr").unwrap().pop().unwrap();
        assert_eq!(after, Value::Int(0));
    }

    /// M1: `finally` runs when a retry unwinds through `try`
    /// (catches never see it) — once per attempt, like the JVM.
    #[test]
    fn retry_runs_finally() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let mut eval = crate::ns::boot();
        install_retry_once(&mut eval);
        let fins = Arc::new(AtomicUsize::new(0));
        let mark = Arc::clone(&fins);
        eval.install_native(
            "user",
            "s19-mark",
            Arc::new(move |_, _| {
                mark.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Keyword(Keyword::new(None, "marked")))
            }),
        );
        let out = eval
            .eval_source("(dosync (try (user/s19-retry-once) :ok (finally (user/s19-mark))))")
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(out, Value::Keyword(Keyword::new(None, "ok")));
        // Once for attempt 1's unwind, once for attempt 2's normal path.
        assert_eq!(fins.load(Ordering::SeqCst), 2);
    }

    /// M1: a `finally` that throws while a retry unwinds masks the
    /// retry like the JVM — attempt 2 never runs, the throw
    /// propagates out of `dosync` (covers the masking arm).
    #[test]
    fn retry_finally_throw_masks_retry() {
        let mut eval = crate::ns::boot();
        install_retry_once(&mut eval);
        eval.install_native(
            "user",
            "s19-boom",
            Arc::new(|_, _| {
                Err(NativeError::new(
                    ThrowClass::RuntimeException,
                    "boom".to_string(),
                ))
            }),
        );
        let err = eval
            .eval_source("(dosync (try (user/s19-retry-once) :ok (finally (user/s19-boom))))")
            .expect_err("the finally throw masks the retry");
        // Branch-free assertion (a `match` wildcard arm would itself
        // be an uncovered line): `Err` proves attempt 2 never ran,
        // `boom` proves the right throw propagated.
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("boom"),
            "expected the boom throw, got {rendered}"
        );
    }

    #[test]
    fn validators_unchanged_pins_slot_identity() {
        // N1: matching slots (absent or identical) pass; a swap or
        // a fresh install between validation and install fails.
        let plain = Ref::new(Value::Int(0));
        let vf = Value::Keyword(Keyword::new(None, "vf"));
        let checked = Ref::new(Value::Int(1));
        checked.lock().validator = Some(vf.clone());
        let targets = vec![(plain.id(), plain.clone()), (checked.id(), checked.clone())];
        let guards = vec![plain.lock(), checked.lock()];
        let mut snapshot = BTreeMap::new();
        snapshot.insert(plain.id(), None);
        snapshot.insert(checked.id(), Some(vf));
        assert!(validators_unchanged(&targets, &guards, &snapshot));
        drop(guards);
        // A swap lands: same snapshot, changed slots.
        plain.lock().validator = Some(Value::Keyword(Keyword::new(None, "late")));
        checked.lock().validator = None;
        let guards = vec![plain.lock(), checked.lock()];
        assert!(!validators_unchanged(&targets, &guards, &snapshot));
    }

    #[test]
    fn call_signal_reroutes_recur() {
        // The impossible arm: an escaping `recur` (the analyzer cages
        // every one) surfaces as an internal error, never a panic.
        let mut eval = Evaluator::new("test");
        let out = call_signal(&mut eval, Err(Signal::Recur(vec![])));
        assert!(matches!(out, Err(Signal::Throw(_))));
    }

    #[test]
    fn foreign_panics_unwind_past_attempts() {
        // A non-`Retry` panic (a port bug — user throws are signals,
        // not panics) still closes the attempt, then resumes.
        let mut eval = Evaluator::new("test");
        eval.install_native(
            "user",
            "s19-boom",
            Arc::new(|_, _| panic!("s19 foreign panic")),
        );
        let sym = Symbol::new(Some("user"), "s19-boom");
        let id = eval
            .registry
            .lookup(&sym)
            .expect("installed native resolves");
        let body = vec![Expr::Invoke {
            head: Box::new(Expr::Global(Var { id, sym })),
            args: Vec::new(),
            span: clj_reader::Span::new(1, 1),
        }];
        let env = Env::new();
        let before = lock_readers().len();
        let payload =
            std::panic::catch_unwind(AssertUnwindSafe(|| sync_begin(&mut eval, &body, &env)))
                .expect_err("foreign panic resumes");
        assert!(
            payload.downcast_ref::<String>().is_some() || payload.downcast_ref::<&str>().is_some()
        );
        assert!(eval.tx.is_none());
        assert_eq!(lock_readers().len(), before);
    }

    /// A never-run deferred action (the failed-agent dispatch
    /// tests abort before any action runs).
    struct DropAction;

    impl AgentAction for DropAction {
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

    /// A deferred send whose agent failed after deferral (pools
    /// stay shut so no worker threads spawn — the failure surfaces
    /// before any pool use).
    fn failed_outbox() -> (Agent, QueuedAction) {
        let agent = Agent::new(Value::Int(0));
        agent.complete_failed(Throwable {
            id: 0,
            class: ThrowClass::RuntimeException,
            message: Some(Arc::from("boom")),
            data: None,
            cause: None,
        });
        let item = QueuedAction {
            pool: AgentPool::Send,
            action: Box::new(DropAction),
        };
        (agent, item)
    }

    #[test]
    fn drop_action_honors_the_contract() {
        // The dispatch-failure stub never runs inside the abort
        // tests (enqueue fails first) — its trivial contract pins
        // directly, like `agent.rs`'s `StubAction`.
        let mut action = DropAction;
        let state = Value::Int(3);
        let out = action.run(&state);
        assert!(matches!(out, Ok(v) if v == state));
        assert!(
            action
                .fire_watches(&Value::Nil, &state, &state, &clj_value::new_map())
                .is_ok()
        );
        action.handle_error(
            &Value::Nil,
            &Value::Nil,
            &Throwable {
                id: 0,
                class: ThrowClass::RuntimeException,
                message: None,
                data: None,
                cause: None,
            },
        );
        assert!(action.take_outbox().is_empty());
    }

    #[test]
    fn failed_outbox_agent_aborts_read_only_commit() {
        // The agent was healthy at deferral, failed before dispatch:
        // the read-only fast path aborts with the agent error (§1f —
        // the outbox dispatches only on success).
        let mut eval = Evaluator::new("test");
        eval.shutdown_agents();
        let mut tx = Transaction::new(u64::MAX);
        lock_readers().push(u64::MAX);
        let (agent, item) = failed_outbox();
        tx.outbox.push((agent, item));
        eval.tx = Some(tx);
        assert!(matches!(
            commit(&mut eval, Value::Nil),
            AttemptOutcome::Throw(_)
        ));
        assert!(eval.tx.is_none());
    }

    #[test]
    fn failed_outbox_agent_aborts_writing_commit() {
        // Same, through the writing path: validators pass, the
        // install lands, then dispatch fails and the commit reports
        // the agent error.
        let mut eval = Evaluator::new("test");
        eval.shutdown_agents();
        let reff = Ref::new(Value::Int(0));
        let mut tx = Transaction::new(u64::MAX);
        lock_readers().push(u64::MAX);
        tx.touched.insert(reff.id(), reff.clone());
        tx.writes.insert(reff.id(), Value::Int(1));
        let (agent, item) = failed_outbox();
        tx.outbox.push((agent, item));
        eval.tx = Some(tx);
        assert!(matches!(
            commit(&mut eval, Value::Nil),
            AttemptOutcome::Throw(_)
        ));
        assert!(eval.tx.is_none());
        // The write installed before dispatch failed (dispatch
        // errors surface post-commit, like the JVM's).
        assert_eq!(reff.lock().value, Value::Int(1));
    }

    #[test]
    fn spin_then_barge_commits_under_contention() {
        // Two refs, the second held: every spin round fails
        // (partially — the first cell yields, then drops), the barge
        // blocks, the release commits. Deterministic: the hold spans
        // the whole spin phase (failed `try_lock`s cannot succeed
        // against a held mutex), so all 64 rounds fail by mechanism.
        let first = Ref::new(Value::Int(1));
        let second = Ref::new(Value::Int(2));
        let targets = vec![(first.id(), first.clone()), (second.id(), second.clone())];
        let held = second.lock();
        let worker = std::thread::spawn(move || {
            let guards = spin_acquire(&targets);
            assert_eq!(guards.len(), 2);
            guards[0].point + guards[1].point
        });
        // The hold spans the whole spin phase (50ms against
        // microsecond rounds): all 64 fail by mechanism, the barge
        // blocks, the release commits.
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(held);
        assert_eq!(worker.join().unwrap(), 0);
    }
}
