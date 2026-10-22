// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S6A concurrency natives: atoms (`atom`, `deref`, `swap!`, `reset!`,
//! `compare-and-set!`, `add-watch`/`remove-watch`), delays (`force`,
//! `realized?` — `delay` itself is a `core.clj` macro over the
//! `delay*` evaluator form, mirroring `lazy-seq*`; `memoize` is a
//! `core.clj` port). Oracle: `/tmp/s6a-probe*.clj`; rules:
//! `docs/t1174/concurrency-contract.md`, `docs/t1174/s6-pins.md` P1/P2.
//!
//! S6B adds the first threads: `future-call` (plus the `future`
//! macro in `core.clj`), `future?`/`future-done?`/`future-cancel`,
//! `promise`/`deliver`, and the `deref` timeout arity (oracles:
//! `/tmp/s6b-probe*.clj`; pins P3/P4).
//!
//! S6C adds agents: `agent` (state + `:meta`/`:validator`/
//! `:error-handler`/`:error-mode`), `send` (fixed pool) / `send-off`
//! (unbounded pool) / `send-via` (routes to `send-off` — pinned
//! divergence, no custom executors), `await`/`await-for` (barrier
//! rendezvous over the `send` pool, like the JVM's `count-down`),
//! `agent-error`, `restart-agent`, and `shutdown-agents` (joins the
//! pools and the future threads). Per-agent FIFO ordering comes from
//! the cell queue ([`Agent::enqueue`](clj_value::Agent::enqueue)):
//! nested sends from an action run after it (no self-deadlock).
//! Oracles: `/tmp/s6c-probe*.clj`; rules:
//! `docs/t1174/concurrency-contract.md`, `docs/t1174/s6-pins.md` P5.
//!
//! S6D adds `set-error-handler!`/`set-error-mode!` and the `locking`
//! monitor (one global reentrant lock; oracles: `/tmp/s6d-probe*.clj`;
//! pins P6).
//!
//! Publish discipline: validators run BEFORE publishing (vetoes and
//! `f` failures leave the state untouched and fire no watches);
//! watches fire AFTER publishing, lock-free, even when the value is
//! unchanged (only a lost `compare-and-set!` fires nothing). The cell
//! lock never spans a `swap!` fn/validator/watch call, so watches may
//! publish again.
//!
//! Threading (D1): one `std::thread` per future over a forked
//! evaluator (shared lineage registry, conveyed bindings — see
//! [`Evaluator::fork_for_thread`](crate::eval::Evaluator::fork_for_thread)).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use clj_value::{
    Agent, AgentAction, AgentPool, Atom, Delay, DelayState, ErrorMode, Future, FutureOutcome,
    FutureState, Keyword, Map, PrintOpts, Promise, QueuedAction, Symbol, ThrowClass, Throwable,
    Value, Var, new_map, pr_to_string,
};

use crate::core::{cce_err, is_ifn};
use crate::error::{NativeError, Signal, internal_throwable};
use crate::eval::Evaluator;
use crate::ext::NativeFn;
use crate::lazy::as_pending;
use crate::ns::{NativeFnPtr, check_arity, npe, signal_to_native, str_of};
use crate::vars::check_min_arity;

/// `Invalid reference state` (every validator veto, JVM `ARef`).
fn invalid_reference_state() -> NativeError {
    NativeError::new(
        ThrowClass::IllegalStateException,
        "Invalid reference state".to_owned(),
    )
}

/// An `IAtom` receiver with a fixed nil shape (`reset!`,
/// `compare-and-set!`): nil trips the call-shape NPE, anything else
/// non-atom the `IAtom` cast.
fn as_atom_fixed(v: &Value, nil_message: &str) -> Result<Atom, NativeError> {
    match v {
        Value::Atom(a) => Ok(a.clone()),
        Value::Nil => Err(npe(nil_message)),
        other => Err(cce_err(other, "clojure.lang.IAtom")),
    }
}

/// An `IRef` receiver (atoms in S6A, agents in S6C).
enum WatchedRef {
    Atom(Atom),
    Agent(Agent),
}

/// The `IRef` receiver: nil trips the call-shape NPE, anything else
/// non-atom non-agent the `IRef` cast.
fn as_iref(v: &Value, nil_message: &str) -> Result<WatchedRef, NativeError> {
    match v {
        Value::Atom(a) => Ok(WatchedRef::Atom(a.clone())),
        Value::Agent(a) => Ok(WatchedRef::Agent(a.clone())),
        Value::Nil => Err(npe(nil_message)),
        other => Err(cce_err(other, "clojure.lang.IRef")),
    }
}

/// The `atom` options as a map (`(apply hash-map options)`:
/// dup-last-wins, unknown keys ignored by the installers below).
fn atom_options(args: &[Value]) -> Result<Map, NativeError> {
    if args.len() % 2 == 1 {
        let last = args.last().unwrap_or(&Value::Nil);
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No value supplied for key: {}", str_of(last)),
        ));
    }
    let mut out = new_map();
    for pair in args.chunks(2) {
        out = out.insert(pair[0].clone(), pair[1].clone());
    }
    Ok(out)
}

/// Install `:meta`/`:validator` on a fresh atom (`setup-reference`:
/// meta first, then the validator — which runs over the initial
/// value, so a vetoing ctor throws). Falsy options never install.
fn install_atom_options(eval: &mut Evaluator, atom: &Atom, opts: &Map) -> Result<(), NativeError> {
    let meta_key = Value::Keyword(Keyword::new(None, "meta"));
    if let Some(m) = opts.get(&meta_key).filter(|m| m.is_truthy()) {
        match m {
            Value::Map(mm) => atom.lock().meta = Some(mm.clone()),
            _ => return Err(cce_err(m, "clojure.lang.IPersistentMap")),
        }
    }
    let validator_key = Value::Keyword(Keyword::new(None, "validator"));
    if let Some(f) = opts.get(&validator_key) {
        if !f.is_truthy() {
            return Ok(());
        }
        if !is_ifn(f) {
            return Err(cce_err(f, "clojure.lang.IFn"));
        }
        let initial = atom.lock().value.clone();
        validate_new(eval, f, &initial)?;
        atom.lock().validator = Some(f.clone());
    }
    Ok(())
}

fn native_atom(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/atom", args, 1)?;
    let opts = atom_options(&args[1..])?;
    let atom = Atom::new(args[0].clone());
    install_atom_options(eval, &atom, &opts)?;
    Ok(Value::Atom(atom))
}

/// Run an atom validator over a candidate value: a falsy verdict
/// vetoes with `Invalid reference state`; a throwing validator
/// propagates `RuntimeException`s/`Error`s as-is and wraps checked
/// failures the same way (JVM `ARef.validate`).
fn validate_new(
    eval: &mut Evaluator,
    validator: &Value,
    candidate: &Value,
) -> Result<(), NativeError> {
    validation_outcome(eval.call(validator, std::slice::from_ref(candidate)))
}

/// Map a validator call's outcome to its verdict (a falsy verdict
/// vetoes; anything but a throw funnels through the shared signal
/// mapping — `recur` cannot reach here, the analyzer cages it).
fn validation_outcome(out: Result<Value, Signal>) -> Result<(), NativeError> {
    match out {
        Ok(v) => {
            if v.is_truthy() {
                Ok(())
            } else {
                Err(invalid_reference_state())
            }
        }
        Err(Signal::Throw(t)) => Err(validate_throw(t)),
        Err(other) => Err(signal_to_native(other)),
    }
}

/// A validator's own failure: checked exceptions (`Exception`,
/// `IOException` and its `FileNotFoundException` child — the only
/// checked classes this runtime throws) wrap with the cause;
/// everything else propagates (JVM `ARef.validate` catch order).
fn validate_throw(t: Throwable) -> NativeError {
    if matches!(
        t.class,
        ThrowClass::Exception | ThrowClass::IOException | ThrowClass::FileNotFoundException
    ) {
        let mut err = invalid_reference_state();
        err.cause = Some(t);
        err
    } else {
        signal_to_native(Signal::Throw(t))
    }
}

/// Fire an atom's watches as `[key ref old new]` (map order, like
/// the JVM's unspecified order; nil fns are stored but skipped,
/// like the JVM's `notifyWatches` null check). Runs after
/// publishing, lock-free, so watches may publish again.
fn fire_watches(
    eval: &mut Evaluator,
    atom: &Atom,
    old: &Value,
    new: &Value,
    watches: &Map,
) -> Result<(), NativeError> {
    let reference = Value::Atom(atom.clone());
    for (k, f) in watches.iter() {
        if matches!(f, Value::Nil) {
            continue;
        }
        let args = [k.clone(), reference.clone(), old.clone(), new.clone()];
        eval.call(f, &args).map_err(signal_to_native)?;
    }
    Ok(())
}

/// Publish `new` iff the state still equals `old` (the CAS: `=`
/// stands in for the JVM's reference comparison — values here have
/// no object identity to compare). Success snapshots the watches
/// (fired by the caller, lock-free); `None` retries the `swap!` loop.
fn try_publish(atom: &Atom, old: &Value, new: &Value) -> Option<Map> {
    let mut state = atom.lock();
    if state.value != *old {
        return None;
    }
    state.value = new.clone();
    Some(state.watches.clone())
}

/// The `swap!` CAS loop: apply `f`, validate, publish iff the state
/// is unchanged (else retry — `f` published meanwhile). Validation
/// vetoes and `f` failures leave the state untouched and fire no
/// watches.
fn swap_loop(
    eval: &mut Evaluator,
    atom: &Atom,
    f: &Value,
    extra: &[Value],
) -> Result<Value, NativeError> {
    loop {
        let old = atom.lock().value.clone();
        let mut f_args = Vec::with_capacity(extra.len() + 1);
        f_args.push(old.clone());
        f_args.extend(extra.iter().cloned());
        let new = eval.call(f, &f_args).map_err(signal_to_native)?;
        let validator = atom.lock().validator.clone();
        if let Some(vf) = validator {
            validate_new(eval, &vf, &new)?;
        }
        if let Some(watches) = try_publish(atom, &old, &new) {
            fire_watches(eval, atom, &old, &new, &watches)?;
            return Ok(new);
        }
    }
}

/// The `IAtom.swap` overload name for the nil-receiver NPE (the JVM
/// dispatches 2/3/4/5+-arg calls to distinct overloads).
fn swap_nil_message(argc: usize) -> String {
    let sig = match argc {
        2 => "clojure.lang.IAtom.swap(clojure.lang.IFn)",
        3 => "clojure.lang.IAtom.swap(clojure.lang.IFn, Object)",
        4 => "clojure.lang.IAtom.swap(clojure.lang.IFn, Object, Object)",
        _ => "clojure.lang.IAtom.swap(clojure.lang.IFn, Object, Object, clojure.lang.ISeq)",
    };
    format!("Cannot invoke \"{sig}\" because \"atom\" is null")
}

/// The `swap!` fn: nil trips the call-shape NPE (`invoke` arities,
/// `applyTo` past 4 args), non-`IFn`s the cast.
fn check_swap_fn(f: &Value, argc: usize) -> Result<(), NativeError> {
    if matches!(f, Value::Nil) {
        let sig = match argc {
            2 => "clojure.lang.IFn.invoke(Object)",
            3 => "clojure.lang.IFn.invoke(Object, Object)",
            4 => "clojure.lang.IFn.invoke(Object, Object, Object)",
            _ => "clojure.lang.IFn.applyTo(clojure.lang.ISeq)",
        };
        return Err(npe(&format!(
            "Cannot invoke \"{sig}\" because \"f\" is null"
        )));
    }
    if !is_ifn(f) {
        return Err(cce_err(f, "clojure.lang.IFn"));
    }
    Ok(())
}

fn native_swap(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/swap!", args, 2)?;
    let atom = match &args[0] {
        Value::Atom(a) => a.clone(),
        Value::Nil => return Err(npe(&swap_nil_message(args.len()))),
        other => return Err(cce_err(other, "clojure.lang.IAtom")),
    };
    check_swap_fn(&args[1], args.len())?;
    swap_loop(eval, &atom, &args[1], &args[2..])
}

fn native_reset(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reset!", args, &[2])?;
    let atom = as_atom_fixed(
        &args[0],
        "Cannot invoke \"clojure.lang.IAtom.reset(Object)\" because \"atom\" is null",
    )?;
    if let Some(vf) = atom.lock().validator.clone() {
        validate_new(eval, &vf, &args[1])?;
    }
    let (old, watches) = {
        let mut state = atom.lock();
        let old = std::mem::replace(&mut state.value, args[1].clone());
        (old, state.watches.clone())
    };
    fire_watches(eval, &atom, &old, &args[1], &watches)?;
    Ok(args[1].clone())
}

fn native_cas(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/compare-and-set!", args, &[3])?;
    let atom = as_atom_fixed(
        &args[0],
        "Cannot invoke \"clojure.lang.IAtom.compareAndSet(Object, Object)\" because \"atom\" is null",
    )?;
    // Validates BEFORE comparing (even on mismatch — probed).
    if let Some(vf) = atom.lock().validator.clone() {
        validate_new(eval, &vf, &args[2])?;
    }
    // `=` stands in for the JVM's reference comparison (port
    // values have no object identity, so ABA state changes
    // compare equal — only `f` side-effect counts can observe
    // the difference).
    let outcome = {
        let mut state = atom.lock();
        if state.value == args[1] {
            let old = std::mem::replace(&mut state.value, args[2].clone());
            Some((old, state.watches.clone()))
        } else {
            None
        }
    };
    match outcome {
        Some((old, watches)) => {
            fire_watches(eval, &atom, &old, &args[2], &watches)?;
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn native_add_watch(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/add-watch", args, &[3])?;
    let target = as_iref(
        &args[0],
        "Cannot invoke \"clojure.lang.IRef.addWatch(Object, clojure.lang.IFn)\" because \"reference\" is null",
    )?;
    // Nil fns install (stored, never fired); other non-`IFn`s fail
    // the cast.
    if !matches!(&args[2], Value::Nil) && !is_ifn(&args[2]) {
        return Err(cce_err(&args[2], "clojure.lang.IFn"));
    }
    // Both fns return the ref itself.
    match target {
        WatchedRef::Atom(atom) => {
            {
                let mut state = atom.lock();
                state.watches = state
                    .watches
                    .clone()
                    .insert(args[1].clone(), args[2].clone());
            }
            Ok(Value::Atom(atom))
        }
        WatchedRef::Agent(agent) => {
            {
                let mut inner = agent.lock();
                inner.watches = inner
                    .watches
                    .clone()
                    .insert(args[1].clone(), args[2].clone());
            }
            Ok(Value::Agent(agent))
        }
    }
}

fn native_remove_watch(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/remove-watch", args, &[2])?;
    let target = as_iref(
        &args[0],
        "Cannot invoke \"clojure.lang.IRef.removeWatch(Object)\" because \"reference\" is null",
    )?;
    match target {
        WatchedRef::Atom(atom) => {
            {
                let mut state = atom.lock();
                state.watches = state.watches.clone().without(&args[1]);
            }
            Ok(Value::Atom(atom))
        }
        WatchedRef::Agent(agent) => {
            {
                let mut inner = agent.lock();
                inner.watches = inner.watches.clone().without(&args[1]);
            }
            Ok(Value::Agent(agent))
        }
    }
}

/// A taken delay: either the cached outcome or the fn to run (the
/// cell now reads `Forcing` owned by this thread, so a re-entrant
/// force errors instead of recursing while fellow threads wait).
enum DelayTake {
    Done(Result<Value, Signal>),
    Thunk(Value),
}

/// Take a delay's cached outcome or its pending fn (leaving
/// `Forcing` behind; cached states restore themselves). A force
/// owned by a fellow thread blocks on the cell condvar until it
/// settles, then shares the outcome (like the JVM's synchronized
/// force); only same-thread re-entry errors.
fn take_delay_thunk(eval: &mut Evaluator, delay: &Delay) -> DelayTake {
    let mut state = delay.lock();
    loop {
        let fellow_forcing = match &*state {
            DelayState::Forcing(owner) => *owner != thread::current().id(),
            _ => false,
        };
        if !fellow_forcing {
            break;
        }
        state = delay.wait_for_settle(state);
    }
    let me = thread::current().id();
    match std::mem::replace(&mut *state, DelayState::Forcing(me)) {
        DelayState::Realized(v) => {
            *state = DelayState::Realized(v.clone());
            DelayTake::Done(Ok(v))
        }
        DelayState::Failed(t) => {
            *state = DelayState::Failed(t.clone());
            DelayTake::Done(Err(Signal::Throw(t)))
        }
        DelayState::Forcing(owner) => {
            debug_assert_eq!(owner, me, "a fellow force waits above");
            *state = DelayState::Forcing(owner);
            DelayTake::Done(Err(eval.throw(NativeError::new(
                ThrowClass::IllegalStateException,
                "Delay is already being forced".to_owned(),
            ))))
        }
        DelayState::Pending(f) => DelayTake::Thunk(f),
    }
}

/// Run a taken thunk and cache the outcome — values AND failures
/// (throwing still realizes; the same failure rethrows on every
/// later force, never re-running).
fn finish_delay_force(eval: &mut Evaluator, delay: &Delay, thunk: Value) -> Result<Value, Signal> {
    delay_outcome(eval.call(&thunk, &[]), delay, thunk)
}

/// Cache a taken thunk's outcome — values AND failures (throwing
/// still realizes; a `recur`, which the analyzer rejects, restores
/// the thunk so a retry re-runs).
fn delay_outcome(out: Result<Value, Signal>, delay: &Delay, thunk: Value) -> Result<Value, Signal> {
    match out {
        Ok(v) => {
            *delay.lock() = DelayState::Realized(v.clone());
            delay.notify_settled();
            Ok(v)
        }
        Err(Signal::Throw(t)) => {
            *delay.lock() = DelayState::Failed(t.clone());
            delay.notify_settled();
            Err(Signal::Throw(t))
        }
        Err(other) => {
            *delay.lock() = DelayState::Pending(thunk);
            delay.notify_settled();
            Err(other)
        }
    }
}

/// Force a delay to its cached-or-fresh value (failures cache and
/// rethrow the same failure).
fn force_delay(eval: &mut Evaluator, delay: &Delay) -> Result<Value, Signal> {
    match take_delay_thunk(eval, delay) {
        DelayTake::Done(outcome) => outcome,
        DelayTake::Thunk(thunk) => finish_delay_force(eval, delay, thunk),
    }
}

fn native_deref(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/deref", args, &[1, 3])?;
    if args.len() == 3 {
        return deref_timeout(&args[0], &args[1], &args[2]);
    }
    match &args[0] {
        Value::Atom(a) => Ok(a.lock().value.clone()),
        Value::Agent(a) => Ok(a.lock().value.clone()),
        Value::Delay(d) => force_delay(eval, d).map_err(signal_to_native),
        Value::Future(f) => future_outcome(&f.get()),
        Value::Promise(p) => Ok(p.get()),
        Value::Var(v) => Ok(eval.deref_var(v.id, &v.sym)),
        Value::Nil => Err(npe(
            "Cannot invoke \"java.util.concurrent.Future.get()\" because \"fut\" is null",
        )),
        other => Err(cce_err(other, "java.util.concurrent.Future")),
    }
}

/// A settled future as a `deref` result: values pass through,
/// cached failures rethrow — freshly wrapped per `deref` over the
/// cached cause — and cancellations throw `CancellationException`.
fn future_outcome(state: &FutureState) -> Result<Value, NativeError> {
    match state {
        FutureState::Done(FutureOutcome::Value(v)) => Ok(v.clone()),
        FutureState::Done(FutureOutcome::Failed(t)) => Err(execution_wrap(t)),
        FutureState::Cancelled => Err(cancelled()),
        // The cell only settles settled states (waiters loop on pending).
        FutureState::Pending => unreachable!("deref of an unsettled future"),
    }
}

/// A fresh `ExecutionException` over a cached cause (the JVM's
/// `FutureTask.get` constructs one per call: wrappers are never
/// `identical?`, their causes always are). The message is the
/// cause's `toString`; the cause chain survives for `ex-cause`.
fn execution_wrap(cause: &Throwable) -> NativeError {
    let mut err = NativeError::new(ThrowClass::ExecutionException, throwable_string(cause));
    err.cause = Some(cause.clone());
    err
}

/// `Throwable.toString`: `Class`, or `Class: message` — except
/// `ExceptionInfo`, which appends its data map (`ExceptionInfo.java`
/// renders `"clojure.lang.ExceptionInfo: " + getMessage() + " " +
/// data`; a nil message prints `null`, a nil data the empty map).
fn throwable_string(t: &Throwable) -> String {
    let class = t.class.jvm_name();
    if t.class == ThrowClass::ExceptionInfo {
        let message = t.message.as_deref().unwrap_or("null");
        let data = t
            .data
            .as_deref()
            .map_or_else(|| "{}".to_owned(), |d| pr_to_string(d, &PrintOpts::pr()));
        return format!("{class}: {message} {data}");
    }
    match &t.message {
        Some(m) => format!("{class}: {m}"),
        None => class.to_owned(),
    }
}

/// A cancelled future's `deref` failure: `CancellationException`
/// with no message (`ex-message` → nil, like the JVM's no-arg
/// throw — any arity, even with a timeout value).
fn cancelled() -> NativeError {
    NativeError::new(ThrowClass::CancellationException, String::new())
}

/// The 3-arity `deref`: `IBlockingDeref`s (futures and promises)
/// wait up to the timeout; everything else runs `deref-future`,
/// whose `Future` checkcast fails BEFORE the timeout converts
/// (atoms, delays, vars, and the rest never reach the timeout).
fn deref_timeout(
    ref_v: &Value,
    timeout_ms: &Value,
    timeout_val: &Value,
) -> Result<Value, NativeError> {
    match ref_v {
        Value::Future(f) => {
            let timeout = Duration::from_millis(timeout_millis(timeout_ms)?);
            match f.get_timeout(timeout) {
                Some(state) => future_outcome(&state),
                None => Ok(timeout_val.clone()),
            }
        }
        Value::Promise(p) => {
            let timeout = Duration::from_millis(timeout_millis(timeout_ms)?);
            Ok(p.get_timeout(timeout)
                .unwrap_or_else(|| timeout_val.clone()))
        }
        Value::Nil => Err(npe(
            "Cannot invoke \"java.util.concurrent.Future.get(long, java.util.concurrent.TimeUnit)\" because \"fut\" is null",
        )),
        other => Err(cce_err(other, "java.util.concurrent.Future")),
    }
}

/// A `deref` timeout in milliseconds: `Int` as-is, `Double`
/// truncated like `(long) doubleValue` (NaN is zero, infinities
/// saturate, negatives wait nothing). `nil` trips the `Number`
/// NPE; anything else non-numeric fails the `Number` cast (even
/// characters — the `.get` conversion has no `Character` branch).
fn timeout_millis(v: &Value) -> Result<u64, NativeError> {
    match v {
        Value::Int(n) => Ok(u64::try_from(*n).unwrap_or(0)),
        Value::Double(d) => Ok(u64::try_from(*d as i64).unwrap_or(0)),
        Value::Nil => Err(npe(
            "Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null",
        )),
        other => Err(cce_err(other, "java.lang.Number")),
    }
}

fn native_realized(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/realized?", args, &[1])?;
    match &args[0] {
        Value::Delay(d) => Ok(Value::Bool(d.is_realized())),
        Value::Future(f) => Ok(Value::Bool(f.is_done())),
        Value::Promise(p) => Ok(Value::Bool(p.is_realized())),
        Value::Nil => Err(npe(
            "Cannot invoke \"clojure.lang.IPending.isRealized()\" because \"x\" is null",
        )),
        other => {
            if let Some(p) = as_pending(other) {
                return Ok(Value::Bool(p.is_realized()));
            }
            Err(cce_err(other, "clojure.lang.IPending"))
        }
    }
}

/// A thread-spawn failure (the OS refused a new thread — the
/// JVM's `unable to create native thread`, untriggerable under
/// test; pinned by direct call).
fn spawn_failed() -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        "unable to create native thread".to_owned(),
    )
}

/// `future-call`: convey the calling thread's bindings onto a forked
/// evaluator and spawn the zero-arg fn on its own thread. The fn is
/// never validated eagerly (like the JVM's `binding-conveyor-fn`
/// wrap: a non-function fails inside the body, surfacing as the
/// future's cached failure).
fn native_future_call(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/future-call", args, &[1])?;
    // Post-shutdown submits reject like the JVM's pool `execute`
    // (T1192 FX1); the message is port-fixed — the JVM's embeds
    // pool identity.
    if eval.agent_system_shutdown() {
        return Err(NativeError::new(
            ThrowClass::RejectedExecutionException,
            "future rejected after shutdown".to_owned(),
        ));
    }
    let future = Future::new();
    let cell = future.clone();
    let child = eval.fork_for_thread(eval.clone_bindings());
    let f = args[0].clone();
    let name = format!("clj-future-{}", future.id());
    let handle = std::thread::Builder::new()
        .name(name)
        .spawn(move || run_future_body(child, f, cell))
        .map_err(|_| spawn_failed())?;
    eval.push_future_thread(handle);
    Ok(Value::Future(future))
}

/// A future body's whole thread: call the zero-arg fn (bindings
/// already conveyed), publishing the value or caching the failure.
/// A Rust panic becomes an `Error` failure and an escaping `recur`
/// an internal `Error` (both unreachable in practice — the
/// analyzer cages every `recur` — but waiters must always settle,
/// never hang).
fn run_future_body(mut child: Evaluator, f: Value, future: Future) {
    let outcome = future_body_outcome(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child.call(&f, &[]))),
        &mut child,
    );
    // A cancellation wins over this late result (like `FutureTask`
    // after `cancel`).
    future.complete(outcome);
}

/// Map a caught future-body outcome to its published outcome (the
/// `Recur` arm is defense in depth — the analyzer cages every
/// `recur` inside its loop/fn).
fn future_body_outcome(
    out: Result<Result<Value, Signal>, Box<dyn std::any::Any + Send>>,
    child: &mut Evaluator,
) -> FutureOutcome {
    match out {
        Ok(Ok(v)) => FutureOutcome::Value(v),
        Ok(Err(Signal::Throw(t))) => FutureOutcome::Failed(t),
        Ok(Err(Signal::Recur(_))) => {
            FutureOutcome::Failed(error_throwable(child, "recur escaped its loop or fn"))
        }
        Err(panicked) => FutureOutcome::Failed(error_throwable(
            child,
            &panic_message(&panicked, "future body panicked"),
        )),
    }
}

/// An `Error` throwable stamped from the shared identity sequence
/// (a VM-level body failure, never a user value).
fn error_throwable(child: &mut Evaluator, message: &str) -> Throwable {
    Throwable {
        id: child.next_id(),
        class: ThrowClass::Error,
        message: Some(Arc::from(message)),
        data: None,
        cause: None,
    }
}

/// A Rust panic's message, best-effort (`&str`/`String` payloads,
/// else a fixed tag — panics are port bugs, never user failures).
fn panic_message(panicked: &Box<dyn std::any::Any + Send>, fallback: &str) -> String {
    if let Some(s) = panicked.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panicked.downcast_ref::<String>() {
        s.clone()
    } else {
        fallback.to_owned()
    }
}

fn native_future_pred(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/future?", args, &[1])?;
    Ok(Value::Bool(matches!(&args[0], Value::Future(_))))
}

fn native_future_done(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/future-done?", args, &[1])?;
    match &args[0] {
        Value::Future(f) => Ok(Value::Bool(f.is_done())),
        Value::Nil => Err(npe(
            "Cannot invoke \"java.util.concurrent.Future.isDone()\" because \"f\" is null",
        )),
        other => Err(cce_err(other, "java.util.concurrent.Future")),
    }
}

fn native_future_cancel(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/future-cancel", args, &[1])?;
    match &args[0] {
        // No thread interruption exists here (pinned divergence:
        // the JVM cancels with interrupt): a running body runs to
        // completion while its late result is discarded.
        Value::Future(f) => Ok(Value::Bool(f.cancel())),
        Value::Nil => Err(npe(
            "Cannot invoke \"java.util.concurrent.Future.cancel(boolean)\" because \"f\" is null",
        )),
        other => Err(cce_err(other, "java.util.concurrent.Future")),
    }
}

fn native_promise(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/promise", args, &[0])?;
    Ok(Value::Promise(Promise::new()))
}

/// `deliver` is an invocation (`core.clj`: `[promise val] (promise
/// val)`): promises deliver (first wins), other `IFn`s apply, and
/// non-functions fail exactly like a call.
fn native_deliver(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/deliver", args, &[2])?;
    eval.call(&args[0], std::slice::from_ref(&args[1]))
        .map_err(signal_to_native)
}

fn native_force(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/force", args, &[1])?;
    match &args[0] {
        Value::Delay(d) => force_delay(eval, d).map_err(signal_to_native),
        other => Ok(other.clone()),
    }
}

/// The lineage's agent system: lazily built pools plus the
/// shutdown latch (shared by every forked evaluator, like the
/// future-thread list). Sends create the pools on first use;
/// `shutdown-agents` latches shutdown (later sends fail) and
/// parks the worker handles for the exit join — the call itself
/// never blocks (T1192 FX1).
pub struct AgentSystem {
    inner: Mutex<AgentSystemInner>,
}

struct AgentSystemInner {
    pools: Option<Arc<AgentPools>>,
    shutdown: bool,
    parked: Vec<std::thread::JoinHandle<()>>,
}

impl AgentSystem {
    /// An unstarted system (pools arrive on the first send).
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(AgentSystemInner {
                pools: None,
                shutdown: false,
                parked: Vec::new(),
            }),
        }
    }

    /// Whether `shutdown-agents` has latched (post-shutdown sends
    /// drop; post-shutdown futures reject).
    pub(crate) fn is_shutdown(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shutdown
    }

    /// The pools for a send, creating them on first use. `None`
    /// after shutdown: callers drop the head (like the JVM's
    /// `execute`, whose rejection the agent swallows after
    /// notifying the error handler — the send still returns the
    /// agent, the action never runs).
    pub(crate) fn pools_for_send(&self) -> Result<Option<Arc<AgentPools>>, NativeError> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.shutdown {
            return Ok(None);
        }
        match inner.pools.clone() {
            Some(pools) => Ok(Some(pools)),
            None => {
                let pools = AgentPools::create()?;
                inner.pools = Some(pools.clone());
                Ok(Some(pools))
            }
        }
    }

    /// Latch shutdown and park the worker handles for the exit
    /// join (in-flight heads finish; queued-but-unstarted heads
    /// drop like the JVM's rejected `execute`; later sends
    /// enqueue-then-drop). Returns at once, like the JVM's
    /// `shutdown` — draining happens on the workers themselves,
    /// which exit once the dropped queue runs dry (T1192 FX1).
    pub(crate) fn shutdown(&self) {
        // Take the pools under this lock, then release it before
        // touching the pools lock (workers lock the other way
        // around on nested sends — never nest).
        let pools = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.shutdown = true;
            inner.pools.take()
        };
        if let Some(pools) = pools {
            let handles = pools.begin_shutdown();
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .parked
                .extend(handles);
        }
    }

    /// Join every parked worker handle (the process-exit join —
    /// mirrors the JVM, whose non-daemon pool threads keep the
    /// runtime alive past main). Idempotent.
    pub(crate) fn join_parked(&self) {
        let parked =
            std::mem::take(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()).parked);
        join_all(parked);
    }
}

/// One submitted agent action: the agent plus its dequeued head (the
/// head remembers its own pool, like the JVM's per-`Action` executor).
struct AgentWork {
    agent: Agent,
    head: QueuedAction,
}

/// The agent thread pools: a fixed `send` pool
/// (`available_parallelism`, else 4) over one `mpsc` queue, plus a
/// `send-off` pool spawning one thread per action (unbounded, like
/// the JVM's cached pool; `send-via` routes here — pinned divergence,
/// no custom executors). Finished send-off threads are reaped on the
/// next spawn; shutdown drops the `send` queue (workers exit after
/// draining) and joins every thread.
pub struct AgentPools {
    inner: Mutex<AgentPoolsInner>,
    sendoff_seq: AtomicU64,
}

struct AgentPoolsInner {
    /// The `send` queue (`None` once shut down — dispatches run
    /// inline instead, so queued actions still complete).
    send_tx: Option<mpsc::Sender<AgentWork>>,
    send_handles: Vec<JoinHandle<()>>,
    sendoff_handles: Vec<JoinHandle<()>>,
}

impl AgentPools {
    /// Start the pools (the `send` workers share one queue). A worker
    /// spawn failure tears the partial pool down and errors the send
    /// instead of hanging it.
    fn create() -> Result<Arc<Self>, NativeError> {
        let count = thread::available_parallelism().map_or(4, |n| n.get());
        let (send_tx, send_rx) = mpsc::channel::<AgentWork>();
        let pools = Arc::new(Self {
            inner: Mutex::new(AgentPoolsInner {
                send_tx: Some(send_tx),
                send_handles: Vec::new(),
                sendoff_handles: Vec::new(),
            }),
            sendoff_seq: AtomicU64::new(0),
        });
        let send_rx = Arc::new(Mutex::new(send_rx));
        let mut handles = Vec::with_capacity(count);
        for i in 0..count {
            let worker_pools = Arc::clone(&pools);
            let worker_rx = Arc::clone(&send_rx);
            let spawned = thread::Builder::new()
                .name(format!("clojure-agent-send-pool-{i}"))
                .spawn(move || send_worker(&worker_pools, &worker_rx));
            Self::install_worker(&pools, &mut handles, spawned)?;
        }
        pools
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_handles = handles;
        Ok(pools)
    }

    /// Install one spawned `send` worker: a spawn failure poisons
    /// the pool's sender (late sends fail loudly instead of
    /// hanging), joins the workers that did start, and reports the
    /// failure (the caller's `Arc` releases on the ensuing return).
    /// Takes the spawn `Result` so tests can drive the failure.
    fn install_worker(
        pools: &Self,
        handles: &mut Vec<JoinHandle<()>>,
        spawned: std::io::Result<JoinHandle<()>>,
    ) -> Result<(), NativeError> {
        match spawned {
            Ok(handle) => {
                handles.push(handle);
                Ok(())
            }
            Err(_) => {
                pools
                    .inner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send_tx = None;
                for handle in handles.drain(..) {
                    let _ = handle.join();
                }
                Err(spawn_failed())
            }
        }
    }

    /// Whether shutdown dropped the `send` queue (dispatches and
    /// completions run inline from here on).
    fn is_shutdown(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_tx
            .is_none()
    }

    /// Stop the pools and join every thread (the `send` workers drain
    /// first; send-off threads drain their agent inline).
    /// Drop the `send` queue (workers exit after draining) and
    /// hand over every worker handle for the exit join. Never
    /// blocks — the join happens in `join_parked` (T1192 FX1).
    fn begin_shutdown(&self) -> Vec<std::thread::JoinHandle<()>> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.send_tx = None;
        let mut handles = std::mem::take(&mut inner.send_handles);
        handles.extend(std::mem::take(&mut inner.sendoff_handles));
        handles
    }
}

/// Join every handle except the current thread's own (defense in
/// depth for joins requested off the main thread — the exit join
/// normally runs there). A skipped handle detaches — its thread
/// runs on and publishes normally.
pub(crate) fn join_all(handles: Vec<JoinHandle<()>>) {
    let me = thread::current().id();
    for handle in handles {
        if handle.thread().id() == me {
            continue;
        }
        let _ = handle.join();
    }
}

/// Run one `send`-pool worker: heads arrive over the shared queue
/// until shutdown drops it (the lock spans the blocking `recv` — one
/// head wakes exactly one worker).
fn send_worker(pools: &Arc<AgentPools>, rx: &Mutex<mpsc::Receiver<AgentWork>>) {
    loop {
        let work = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        match work {
            Ok(work) => pump_agent(pools, &work.agent, work.head),
            Err(_) => break,
        }
    }
}

/// Drop a head the pools refuse (shut down, queue gone, workers
/// dead, spawn failed): notify the agent's error handler with the
/// rejection first, like the JVM's `execute` catch — the send that
/// queued it already returned the agent.
fn reject_head(agent: &Agent, mut head: QueuedAction) {
    let handler = agent.lock().error_handler.clone();
    if let Some(handler) = handler {
        let rejection = Throwable {
            id: 0,
            class: ThrowClass::RejectedExecutionException,
            // The JVM's message embeds pool identity hashes
            // (unpinnable); the class is the pinned part.
            message: Some(Arc::from("agent action rejected after shutdown")),
            data: None,
            cause: None,
        };
        head.action
            .handle_error(&handler, &Value::Agent(agent.clone()), &rejection);
    }
}

/// Dispatch a dequeued head to its own pool (each action remembers
/// its pool). Refused dispatches (shut down, queue gone, workers
/// dead) drop the head after notifying the handler — never inline.
fn pools_dispatch(pools: &Arc<AgentPools>, agent: &Agent, head: QueuedAction) {
    if head.pool == AgentPool::Send {
        // One under-lock read: `None` means shut down (the old
        // pre-check raced the queue drop; both rejected alike).
        let tx = pools
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_tx
            .clone();
        match tx {
            // The receiver outlives every sender clone (workers exit
            // only once all senders drop), so this send cannot fail.
            Some(tx) => {
                let _ = tx.send(AgentWork {
                    agent: agent.clone(),
                    head,
                });
            }
            None => reject_head(agent, head),
        }
    } else {
        dispatch_sendoff(pools, agent, head);
    }
}

/// Complete a `send-off` spawn: publish the handle and hand the
/// head to the new thread — or reject the head when the spawn
/// failed (like the JVM's `execute` catch). Takes the spawn
/// `Result` so tests can drive the failure.
fn complete_sendoff(
    mut inner: std::sync::MutexGuard<'_, AgentPoolsInner>,
    head_tx: mpsc::Sender<QueuedAction>,
    agent: &Agent,
    head: QueuedAction,
    spawned: std::io::Result<JoinHandle<()>>,
) {
    match spawned {
        Ok(handle) => {
            inner.sendoff_handles.push(handle);
            let _ = head_tx.send(head);
        }
        Err(_) => {
            drop(inner);
            reject_head(agent, head);
        }
    }
}

/// Dispatch a head to the unbounded pool: one thread per action.
/// Refused dispatches (shut down, spawn failed) drop the head after
/// notifying the handler, like the JVM's `execute` catch. Finished
/// threads are reaped on the next spawn.
fn dispatch_sendoff(pools: &Arc<AgentPools>, agent: &Agent, head: QueuedAction) {
    let seq = pools.sendoff_seq.fetch_add(1, Ordering::Relaxed);
    let mut inner = pools.inner.lock().unwrap_or_else(|e| e.into_inner());
    if inner.send_tx.is_none() {
        drop(inner);
        reject_head(agent, head);
        return;
    }
    inner.sendoff_handles.retain(|h| !h.is_finished());
    let child_pools = Arc::clone(pools);
    let child_agent = agent.clone();
    // The head travels over a channel (an unbounded send never
    // blocks): a spawn failure rejects it here instead of dropping
    // it silently inside the unborn closure.
    let (head_tx, head_rx) = mpsc::channel::<QueuedAction>();
    let name = format!("clojure-agent-send-off-pool-{seq}");
    let spawned = thread::Builder::new().name(name).spawn(move || {
        if let Ok(head) = head_rx.recv() {
            pump_agent(&child_pools, &child_agent, head);
        }
    });
    complete_sendoff(inner, head_tx, agent, head, spawned);
}

/// Run a head, then dispatch the next head to its pool — or drop it
/// after shutdown (an explicit loop: the queue is unbounded, so
/// recursion could overflow).
fn pump_agent(pools: &Arc<AgentPools>, agent: &Agent, head: QueuedAction) {
    if let Some(next) = execute_head(agent, head.action, pools) {
        if pools.is_shutdown() {
            reject_head(agent, next);
        } else {
            pools_dispatch(pools, agent, next);
        }
    }
}

/// Release buffered nested sends (the JVM's `releasePendingSends`):
/// each enqueues unchecked and starts its runner when idle and
/// clean; refused dispatches drop like any other submit failure.
fn release_outbox(outbox: Vec<(Agent, QueuedAction)>, pools: &Arc<AgentPools>) {
    for (target, item) in outbox {
        if let Some(head) = target.enqueue_nested(item) {
            pools_dispatch(pools, &target, head);
        }
    }
}

/// Run one dequeued head: apply over the current state, publish, fire
/// watches (all inside the action's error boundary — a throwing watch
/// fails the action after publishing, like the JVM), release the
/// nested-send outbox on success (discarded on failure, like the
/// JVM's unreleased `nested` buffer), then hand over the next head
/// (or park the queue on `:fail`). Never panics: every action
/// callback contains its own unwinds, so pool threads survive.
fn execute_head(
    agent: &Agent,
    mut action: Box<dyn AgentAction>,
    pools: &Arc<AgentPools>,
) -> Option<QueuedAction> {
    let state = agent.lock().value.clone();
    let agent_val = Value::Agent(agent.clone());
    match action.run(&state) {
        Ok(new) => {
            let (old, watches) = {
                let mut inner = agent.lock();
                let old = std::mem::replace(&mut inner.value, new.clone());
                (old, inner.watches.clone())
            };
            match action.fire_watches(&agent_val, &old, &new, &watches) {
                Ok(()) => {
                    release_outbox(action.take_outbox(), pools);
                    agent.complete_ok()
                }
                Err(error) => {
                    action.take_outbox();
                    fail_head(agent, action.as_mut(), &agent_val, error)
                }
            }
        }
        Err(error) => fail_head(agent, action.as_mut(), &agent_val, error),
    }
}

/// Fail one head: the handler runs first (ignoring its failures),
/// then `:continue` clears and proceeds while `:fail` latches the
/// error and parks the queue.
fn fail_head(
    agent: &Agent,
    action: &mut dyn AgentAction,
    agent_val: &Value,
    error: Throwable,
) -> Option<QueuedAction> {
    let (mode, handler) = {
        let inner = agent.lock();
        (inner.error_mode, inner.error_handler.clone())
    };
    if let Some(handler) = handler {
        action.handle_error(&handler, agent_val, &error);
    }
    match mode {
        ErrorMode::Continue => agent.complete_ok(),
        ErrorMode::Fail => {
            agent.complete_failed(error);
            None
        }
    }
}

/// One evaluated agent action: the fn plus its extra args over a
/// forked evaluator (bindings conveyed at send time, `*agent*` bound
/// around the fn call, the watches, and the error handler — only
/// the validator runs bare, like the JVM's popped conveyor). The
/// validator is captured at send (no setter exists, so it never
/// changes).
struct EvalAction {
    child: Evaluator,
    agent: Value,
    f: Value,
    args: Vec<Value>,
    validator: Option<Value>,
}

impl AgentAction for EvalAction {
    fn run(&mut self, state: &Value) -> Result<Value, Throwable> {
        let bound = bind_agent(&mut self.child, &self.agent);
        // Arm the nested-send outbox for the fn AND the watches (the
        // JVM's `nested` buffer spans both): failures below discard
        // it, success leaves it for release after the watches.
        self.child.agent_outbox = Some(Vec::new());
        let mut f_args = Vec::with_capacity(self.args.len() + 1);
        f_args.push(state.clone());
        f_args.extend(self.args.iter().cloned());
        let out = child_call(&mut self.child, &self.f, &f_args);
        if bound {
            self.child.pop_frame();
        }
        let candidate = match out {
            Ok(v) => v,
            Err(e) => {
                self.child.agent_outbox = None;
                return Err(e);
            }
        };
        let validated = match self.validator.clone() {
            Some(validator) => validate_action(&mut self.child, &validator, &candidate),
            None => Ok(()),
        };
        if let Err(e) = validated {
            self.child.agent_outbox = None;
            return Err(e);
        }
        Ok(candidate)
    }

    fn take_outbox(&mut self) -> Vec<(Agent, QueuedAction)> {
        self.child.agent_outbox.take().unwrap_or_default()
    }

    fn fire_watches(
        &mut self,
        agent: &Value,
        old: &Value,
        new: &Value,
        watches: &Map,
    ) -> Result<(), Throwable> {
        // `*agent*` stays bound over the watches (like the fn call —
        // the JVM's conveyor is still installed there).
        let bound = bind_agent(&mut self.child, agent);
        let mut out = Ok(());
        for (k, f) in watches.iter() {
            if matches!(f, Value::Nil) {
                continue;
            }
            let args = [k.clone(), agent.clone(), old.clone(), new.clone()];
            if let Err(error) = child_call(&mut self.child, f, &args) {
                out = Err(error);
                break;
            }
        }
        if bound {
            self.child.pop_frame();
        }
        out
    }

    fn handle_error(&mut self, handler: &Value, agent: &Value, error: &Throwable) {
        // `*agent*` stays bound over the handler too (only the
        // validator runs bare).
        let bound = bind_agent(&mut self.child, agent);
        let args = [agent.clone(), Value::Throwable(error.clone())];
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.child.call(handler, &args)
        }));
        if bound {
            self.child.pop_frame();
        }
    }
}

/// Call through a forked evaluator, catching everything — throws,
/// escaping `recur`s, and Rust panics — into a throwable (pool
/// threads never die).
fn child_call(child: &mut Evaluator, f: &Value, args: &[Value]) -> Result<Value, Throwable> {
    child_outcome(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child.call(f, args))),
        child,
    )
}

/// Map a caught child-call outcome to a value-or-throwable (the
/// `Recur` arm is defense in depth — the analyzer cages every
/// `recur` inside its loop/fn).
fn child_outcome(
    out: Result<Result<Value, Signal>, Box<dyn std::any::Any + Send>>,
    child: &mut Evaluator,
) -> Result<Value, Throwable> {
    match out {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(Signal::Throw(t))) => Err(t),
        Ok(Err(Signal::Recur(_))) => Err(error_throwable(child, "recur escaped its loop or fn")),
        Err(panicked) => Err(error_throwable(
            child,
            &panic_message(&panicked, "agent action panicked"),
        )),
    }
}

/// Run an action validator over a candidate (a falsy verdict vetoes,
/// like `swap!` — stamped with an identity for the error latch).
/// Unwind-contained like every other action callback, so a panicking
/// native surfaces as an action error instead of killing the worker.
fn validate_action(
    child: &mut Evaluator,
    validator: &Value,
    candidate: &Value,
) -> Result<(), Throwable> {
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validate_new(child, validator, candidate)
    }));
    match out {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(stamp_native(child, err)),
        Err(panicked) => Err(error_throwable(
            child,
            &panic_message(&panicked, "agent action panicked"),
        )),
    }
}

/// Stamp a native error with an identity (the error latch stores
/// throwables, like `agent-error` returns).
fn stamp_native(child: &mut Evaluator, err: NativeError) -> Throwable {
    unwrap_thrown(child.throw(err))
}

/// A constructed throw (the [`throw`](Evaluator::throw)
/// constructor only builds `Throw`; a `Recur` here is a port bug
/// and surfaces as an internal error, never a panic).
fn unwrap_thrown(s: Signal) -> Throwable {
    match s {
        Signal::Throw(t) => t,
        Signal::Recur(_) => internal_throwable("throw returned a recur signal".to_owned()),
    }
}

/// The `clojure.core/*agent*` var, if the lineage defines it.
fn agent_var(eval: &Evaluator) -> Option<Var> {
    let sym = Symbol::new(Some("clojure.core"), "*agent*");
    eval.registry.lookup(&sym).map(|id| Var { id, sym })
}

/// Whether `*agent*` is bound (await refuses inside actions).
fn agent_is_bound(eval: &Evaluator) -> bool {
    agent_var(eval).is_some_and(|v| eval.deref_var(v.id, &v.sym).is_truthy())
}

/// Bind `*agent*` around an action fn (skipped when the lineage has
/// no such var); returns whether a frame was pushed.
fn bind_agent(child: &mut Evaluator, agent: &Value) -> bool {
    match agent_var(child) {
        Some(v) => {
            child.push_frame(vec![(v, agent.clone())]);
            true
        }
        None => false,
    }
}

/// The `send` family's nil-agent NPE (`send-via`'s `dispatch` call).
const DISPATCH_NIL: &str = "Cannot invoke \"clojure.lang.Agent.dispatch(clojure.lang.IFn, clojure.lang.ISeq, java.util.concurrent.Executor)\" because \"a\" is null";

/// An `Agent` receiver: nil trips the call-shape NPE, anything else
/// non-agent the `Agent` cast.
fn as_agent(v: &Value, nil_message: &str) -> Result<Agent, NativeError> {
    match v {
        Value::Agent(a) => Ok(a.clone()),
        Value::Nil => Err(npe(nil_message)),
        other => Err(cce_err(other, "clojure.lang.Agent")),
    }
}

/// A send/await on a failed agent: `RuntimeException` with the
/// latched failure as the cause (the JVM's `dispatch` shape).
fn agent_failed(error: &Throwable) -> NativeError {
    let mut err = NativeError::new(
        ThrowClass::RuntimeException,
        "Agent is failed, needs restart".to_owned(),
    );
    err.cause = Some(error.clone());
    err
}

/// Install an agent's `:meta` option (a map or nothing — falsy
/// never installs).
fn install_agent_meta(agent: &Agent, opts: &Map) -> Result<(), NativeError> {
    let meta_key = Value::Keyword(Keyword::new(None, "meta"));
    let Some(m) = opts.get(&meta_key) else {
        return Ok(());
    };
    if !m.is_truthy() {
        return Ok(());
    }
    match m {
        Value::Map(mm) => {
            agent.lock().meta = Some(mm.clone());
            Ok(())
        }
        _ => Err(cce_err(m, "clojure.lang.IPersistentMap")),
    }
}

/// Install an agent's `:validator` option (validated over the initial
/// value, like atoms — a vetoing ctor throws).
fn install_agent_validator(
    eval: &mut Evaluator,
    agent: &Agent,
    opts: &Map,
) -> Result<(), NativeError> {
    let validator_key = Value::Keyword(Keyword::new(None, "validator"));
    let Some(f) = opts.get(&validator_key) else {
        return Ok(());
    };
    if !f.is_truthy() {
        return Ok(());
    }
    if !is_ifn(f) {
        return Err(cce_err(f, "clojure.lang.IFn"));
    }
    let initial = agent.lock().value.clone();
    validate_new(eval, f, &initial)?;
    agent.lock().validator = Some(f.clone());
    Ok(())
}

/// Install an agent's `:error-handler` option, returning whether one
/// installed (the mode default keys off the raw option).
fn install_agent_handler(agent: &Agent, opts: &Map) -> Result<bool, NativeError> {
    let handler_key = Value::Keyword(Keyword::new(None, "error-handler"));
    let Some(h) = opts.get(&handler_key).filter(|h| h.is_truthy()) else {
        return Ok(false);
    };
    if !is_ifn(h) {
        return Err(cce_err(h, "clojure.lang.IFn"));
    }
    agent.lock().error_handler = Some(h.clone());
    Ok(true)
}

/// Install an agent's `:error-mode` option (defaulting to `:continue`
/// with a handler and `:fail` without). A truthy non-keyword mode
/// fails the `Keyword` cast, while any other keyword behaves as
/// `:fail` (the JVM compares against `CONTINUE`).
fn install_agent_mode(agent: &Agent, opts: &Map, has_handler: bool) -> Result<(), NativeError> {
    let mode_key = Value::Keyword(Keyword::new(None, "error-mode"));
    let mode_value = match opts.get(&mode_key).filter(|m| m.is_truthy()) {
        Some(m) => {
            if !matches!(m, Value::Keyword(_)) {
                return Err(cce_err(m, "clojure.lang.Keyword"));
            }
            m.clone()
        }
        None => Value::Keyword(Keyword::new(
            None,
            if has_handler { "continue" } else { "fail" },
        )),
    };
    agent.lock().error_mode = if mode_value == Value::Keyword(Keyword::new(None, "continue")) {
        ErrorMode::Continue
    } else {
        ErrorMode::Fail
    };
    Ok(())
}

/// Install `agent` options over a fresh cell (`setup-reference` runs
/// first — meta, then the validator — then the error handler, then
/// the mode).
fn install_agent_options(
    eval: &mut Evaluator,
    agent: &Agent,
    opts: &Map,
) -> Result<(), NativeError> {
    install_agent_meta(agent, opts)?;
    install_agent_validator(eval, agent, opts)?;
    let has_handler = install_agent_handler(agent, opts)?;
    install_agent_mode(agent, opts, has_handler)
}

fn native_agent(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/agent", args, 1)?;
    let opts = atom_options(&args[1..])?;
    let agent = Agent::new(args[0].clone());
    install_agent_options(eval, &agent, &opts)?;
    Ok(Value::Agent(agent))
}

/// Dispatch an action: inside an action fn or its watches the send
/// buffers into the running action's outbox (released on success,
/// discarded on failure, like the JVM's thread-local `nested`);
/// otherwise the failed check beats shutdown (like the JVM's
/// `dispatch`), the fn is never validated eagerly (a bad fn fails
/// inside the action), an idle agent's head submits (dropped after
/// shutdown), and a running agent's completion picks the rest up
/// (nested sends run after the enclosing action — no self-deadlock).
fn send_action(
    eval: &mut Evaluator,
    pool: AgentPool,
    agent: &Agent,
    f: &Value,
    extra: &[Value],
) -> Result<Value, NativeError> {
    let validator = agent.lock().validator.clone();
    let child = eval.fork_for_thread(eval.clone_bindings());
    let action = EvalAction {
        child,
        agent: Value::Agent(agent.clone()),
        f: f.clone(),
        args: extra.to_vec(),
        validator,
    };
    let item = QueuedAction {
        pool,
        action: Box::new(action),
    };
    // Nested sends (inside an action fn or its watches) buffer into
    // the running action's outbox — unchecked, like the JVM's
    // thread-local `nested` — for release on success.
    if let Some(outbox) = eval.agent_outbox.as_mut() {
        outbox.push((agent.clone(), item));
        return Ok(Value::Agent(agent.clone()));
    }
    // No failed pre-check: `enqueue` rejects failed agents under its
    // own lock with the same latched error (a restart racing the
    // send linearizes as success either way).
    let pools = eval.agent_pools_for_send()?;
    match agent.enqueue(item) {
        Ok(Some(head)) => match pools.as_ref() {
            Some(pools) => pools_dispatch(pools, agent, head),
            // Shut down: the head drops (the JVM's rejected
            // `execute`); the queue wedges, the send returns.
            None => reject_head(agent, head),
        },
        Ok(None) => {}
        Err(error) => return Err(agent_failed(&error)),
    }
    Ok(Value::Agent(agent.clone()))
}

fn native_send(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/send", args, 2)?;
    let agent = as_agent(&args[0], DISPATCH_NIL)?;
    send_action(eval, AgentPool::Send, &agent, &args[1], &args[2..])
}

fn native_send_off(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/send-off", args, 2)?;
    let agent = as_agent(&args[0], DISPATCH_NIL)?;
    send_action(eval, AgentPool::SendOff, &agent, &args[1], &args[2..])
}

fn native_send_via(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/send-via", args, 3)?;
    let agent = as_agent(&args[1], DISPATCH_NIL)?;
    // The executor is accepted and ignored (PINNED DIVERGENCE: the
    // port has no custom executors, so every `send-via` routes to
    // the `send-off` pool).
    send_action(eval, AgentPool::SendOff, &agent, &args[2], &args[3..])
}

/// Refuse `await`/`await-for` inside an agent action (the JVM's
/// guard, checked before anything else — even with no agents).
fn check_not_in_action(eval: &Evaluator) -> Result<(), NativeError> {
    if agent_is_bound(eval) {
        return Err(NativeError::new(
            ThrowClass::Exception,
            "Can't await in agent action".to_owned(),
        ));
    }
    Ok(())
}

/// The `await` barrier fn: counts one latch down and returns the
/// state unchanged (like the JVM's `count-down` — it validates and
/// fires watches as a normal action).
fn barrier_fn(eval: &mut Evaluator, tx: mpsc::Sender<()>) -> Value {
    Value::Extension(Arc::new(NativeFn {
        id: eval.next_id(),
        display: Arc::from("clojure.core/await-count-down"),
        func: Arc::new(move |_, args| {
            let _ = tx.send(());
            Ok(args.first().cloned().unwrap_or(Value::Nil))
        }),
        meta: None,
    }))
}

/// Send one barrier per agent over the `send` pool (like the JVM's
/// `doseq`), validating each agent first — a failed agent throws
/// before anything waits.
fn send_barriers(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<(Vec<Agent>, mpsc::Receiver<()>), NativeError> {
    let (tx, rx) = mpsc::channel();
    let mut agents = Vec::with_capacity(args.len());
    for arg in args {
        let agent = as_agent(arg, DISPATCH_NIL)?;
        let count_down = barrier_fn(eval, tx.clone());
        send_action(eval, AgentPool::Send, &agent, &count_down, &[])?;
        agents.push(agent);
    }
    Ok((agents, rx))
}

/// Throw when any awaited agent is failed (checked while waiting, so
/// a failure racing the wait surfaces instead of hanging).
fn check_await_errors(agents: &[Agent]) -> Result<(), NativeError> {
    for agent in agents {
        if let Some(error) = agent.error() {
            return Err(agent_failed(&error));
        }
    }
    Ok(())
}

/// Wait for every barrier (an `await` rendezvous): each loop polls
/// the latch and the agents' errors, so an agent that fails after its
/// barrier queued throws instead of hanging (DIVERGENCE: the JVM
/// blocks forever there; `await-for` likewise throws on observed
/// failure instead of waiting out the deadline for `false`).
/// A deadline bounds the wait (`false` on
/// expiry); an expired deadline waits nothing (like the JVM's latch,
/// which checks once without waiting). A dropped barrier — a
/// `:clear-actions` restart racing the wait — ends it early
/// (DIVERGENCE: the JVM never returns).
fn await_latches(
    agents: &[Agent],
    rx: &mpsc::Receiver<()>,
    timeout: Option<Duration>,
) -> Result<bool, NativeError> {
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut remaining = agents.len();
    while remaining > 0 {
        let wait = match deadline {
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(1)),
            None => Duration::from_millis(1),
        };
        if !wait.is_zero() {
            match rx.recv_timeout(wait) {
                Ok(()) => {
                    remaining -= 1;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    check_await_errors(agents)?;
                    return Ok(true);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        check_await_errors(agents)?;
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn native_await(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_not_in_action(eval)?;
    let (agents, rx) = send_barriers(eval, args)?;
    await_latches(&agents, &rx, None)?;
    Ok(Value::Nil)
}

fn native_await_for(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/await-for", args, 1)?;
    check_not_in_action(eval)?;
    // Barriers validate and queue BEFORE the timeout converts (the
    // JVM's `doseq` precedes the latch wait).
    let (agents, rx) = send_barriers(eval, &args[1..])?;
    let timeout = Duration::from_millis(timeout_millis(&args[0])?);
    Ok(Value::Bool(await_latches(&agents, &rx, Some(timeout))?))
}

fn native_agent_error(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/agent-error", args, &[1])?;
    let agent = as_agent(
        &args[0],
        "Cannot invoke \"clojure.lang.Agent.getError()\" because \"a\" is null",
    )?;
    Ok(agent.error().map_or(Value::Nil, Value::Throwable))
}

fn native_restart_agent(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/restart-agent", args, 2)?;
    let agent = as_agent(
        &args[0],
        "Cannot invoke \"clojure.lang.Agent.restart(Object, boolean)\" because \"a\" is null",
    )?;
    // Options parse before the failed check (the JVM's `let`
    // binds them first), which precedes validation (an unfailed
    // agent with a bad state still reports the restart error).
    let opts = atom_options(&args[2..])?;
    if agent.error().is_none() {
        return Err(restart_not_needed());
    }
    let clear_key = Value::Keyword(Keyword::new(None, "clear-actions"));
    let clear = opts.get(&clear_key).is_some_and(|v| v.is_truthy());
    // A veto leaves the agent failed with its old state and error
    // (the publish below never runs); watchers never fire. The
    // validator runs OUTSIDE the publish lock (which is
    // non-reentrant) — a racing restart that clears the error first
    // makes this one lose under the lock instead.
    if let Some(validator) = agent.lock().validator.clone() {
        validate_new(eval, &validator, &args[1])?;
    }
    let head = agent
        .restart_publish(args[1].clone(), clear)
        .ok_or_else(restart_not_needed)?;
    if let Some(head) = head {
        match eval.agent_pools_for_send()? {
            Some(pools) => pools_dispatch(&pools, &agent, head),
            // Shut down: the resumed head drops (the JVM's rejected
            // re-`execute`); the restart itself still succeeded.
            None => reject_head(&agent, head),
        }
    }
    Ok(args[1].clone())
}

/// A restart on an unfailed agent (the pre-check and the
/// under-lock re-check in [`Agent::restart_publish`] share this).
fn restart_not_needed() -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        "Agent does not need a restart".to_owned(),
    )
}

fn native_shutdown_agents(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/shutdown-agents", args, &[0])?;
    eval.shutdown_agents();
    Ok(Value::Nil)
}

/// `set-error-handler!`: install (or clear, with nil) the cell's
/// handler. The JVM's single-overload call compiles to a checkcast
/// (probed: a bad handler is the `IFn` CCE, not a Reflector
/// mismatch), and the `void` setter returns nil.
fn native_set_error_handler(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/set-error-handler!", args, &[2])?;
    let agent = as_agent(
        &args[0],
        "Cannot invoke \"clojure.lang.Agent.setErrorHandler(clojure.lang.IFn)\" because \"a\" is null",
    )?;
    if !matches!(&args[1], Value::Nil) && !is_ifn(&args[1]) {
        return Err(cce_err(&args[1], "clojure.lang.IFn"));
    }
    agent.lock().error_handler = match &args[1] {
        Value::Nil => None,
        handler => Some(handler.clone()),
    };
    Ok(Value::Nil)
}

/// `set-error-mode!`: install the cell's mode (`:continue` continues
/// past the next failure, anything else — including nil, which the
/// JVM's checkcast passes through — latches it, since the worker
/// compares against `CONTINUE`). The `void` setter returns nil.
fn native_set_error_mode(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/set-error-mode!", args, &[2])?;
    let agent = as_agent(
        &args[0],
        "Cannot invoke \"clojure.lang.Agent.setErrorMode(clojure.lang.Keyword)\" because \"a\" is null",
    )?;
    if !matches!(&args[1], Value::Nil) && !matches!(&args[1], Value::Keyword(_)) {
        return Err(cce_err(&args[1], "clojure.lang.Keyword"));
    }
    agent.lock().error_mode = if args[1] == Value::Keyword(Keyword::new(None, "continue")) {
        ErrorMode::Continue
    } else {
        ErrorMode::Fail
    };
    Ok(Value::Nil)
}

/// The `locking` monitor: one process-global reentrant lock (every
/// forked evaluator shares it, so futures and agent actions mutually
/// exclude; per-object monitors would be observably identical — only
/// timing differs — and `std::Mutex` is non-reentrant, so ownership
/// is tracked explicitly for nested `locking` on one thread).
struct LockingMonitor {
    state: Mutex<LockingState>,
}

struct LockingState {
    owner: Option<thread::ThreadId>,
    count: u64,
}

impl LockingMonitor {
    const fn new() -> Self {
        Self {
            state: Mutex::new(LockingState {
                owner: None,
                count: 0,
            }),
        }
    }
}

static LOCKING_MONITOR: LockingMonitor = LockingMonitor::new();

/// One held `locking` monitor: released on drop (Rust-side, so body
/// throws and panics both release — no `finally` needed).
pub(crate) struct LockingGuard {
    _held: (),
}

impl Drop for LockingGuard {
    fn drop(&mut self) {
        let mut state = LOCKING_MONITOR
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.count -= 1;
        if state.count == 0 {
            state.owner = None;
        }
    }
}

/// Hold the `locking` monitor (reentrant on the calling thread;
/// contended threads yield-spin — critical sections are short).
pub(crate) fn locking_acquire() -> LockingGuard {
    let me = thread::current().id();
    loop {
        {
            let mut state = LOCKING_MONITOR
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if state.owner.is_none_or(|owner| owner == me) {
                state.owner = Some(me);
                state.count += 1;
                return LockingGuard { _held: () };
            }
        }
        thread::yield_now();
    }
}

/// The S6A/S6B/S6C natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const CONC_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("atom", native_atom),
    ("deref", native_deref),
    ("swap!", native_swap),
    ("reset!", native_reset),
    ("compare-and-set!", native_cas),
    ("add-watch", native_add_watch),
    ("remove-watch", native_remove_watch),
    ("realized?", native_realized),
    ("force", native_force),
    ("future-call", native_future_call),
    ("future?", native_future_pred),
    ("future-done?", native_future_done),
    ("future-cancel", native_future_cancel),
    ("promise", native_promise),
    ("deliver", native_deliver),
    ("agent", native_agent),
    ("send", native_send),
    ("send-off", native_send_off),
    ("send-via", native_send_via),
    ("await", native_await),
    ("await-for", native_await_for),
    ("agent-error", native_agent_error),
    ("restart-agent", native_restart_agent),
    ("shutdown-agents", native_shutdown_agents),
    ("set-error-handler!", native_set_error_handler),
    ("set-error-mode!", native_set_error_mode),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_all_skips_the_calling_thread() {
        // T1192 FX1: the exit join normally runs on main, but a
        // join requested off-thread must skip its own handle
        // instead of hanging on itself. The worker receives its
        // own handle over a channel (its spawner owns it first).
        let (send_handle, recv_handle) = std::sync::mpsc::channel();
        let (send_done, recv_done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let own = recv_handle.recv().expect("own handle");
            join_all(vec![own]);
            send_done.send(()).expect("done");
        });
        send_handle.send(worker).expect("hand over");
        recv_done
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("join_all returned");
    }

    #[test]
    fn unwrap_thrown_recur_is_internal() {
        // Defense in depth: a `Recur` from the `throw`
        // constructor (impossible — it only builds `Throw`)
        // surfaces as an internal error.
        let t = unwrap_thrown(Signal::Recur(vec![]));
        assert_eq!(t.class, ThrowClass::Error);
        assert!(
            t.message
                .as_deref()
                .unwrap_or("")
                .contains("throw returned a recur signal")
        );
    }

    #[test]
    fn locking_monitor_starts_unowned() {
        let monitor = LockingMonitor::new();
        let state = monitor.state.lock().expect("fresh monitor is unpoisoned");
        assert!(state.owner.is_none());
        assert_eq!(state.count, 0);
    }

    #[test]
    fn bind_agent_without_var_is_idle() {
        let mut eval = Evaluator::new_bare("user");
        assert!(!bind_agent(&mut eval, &Value::Nil));
    }

    #[test]
    #[should_panic(expected = "deref of an unsettled future")]
    fn future_outcome_pending_panics() {
        let _ = future_outcome(&FutureState::Pending);
    }

    #[test]
    fn child_outcome_maps_every_caught_shape() {
        let mut child = Evaluator::new_bare("user");
        assert!(matches!(
            child_outcome(Ok(Ok(Value::Int(1))), &mut child),
            Ok(Value::Int(1))
        ));
        let thrown = Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert!(child_outcome(Ok(Err(Signal::Throw(thrown))), &mut child).is_err());
        // The analyzer-impossible case: pins the helper's total
        // contract directly.
        let err = child_outcome(Ok(Err(Signal::Recur(vec![]))), &mut child).unwrap_err();
        assert_eq!(err.class, ThrowClass::Error);
        assert_eq!(err.message.as_deref(), Some("recur escaped its loop or fn"));
        let err = child_outcome(Err(Box::new("x")), &mut child).unwrap_err();
        assert_eq!(err.class, ThrowClass::Error);
        assert_eq!(err.message.as_deref(), Some("x"));
    }

    #[test]
    fn future_body_outcome_maps_every_caught_shape() {
        let mut child = Evaluator::new_bare("user");
        assert!(matches!(
            future_body_outcome(Ok(Ok(Value::Int(1))), &mut child),
            FutureOutcome::Value(Value::Int(1))
        ));
        let thrown = Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert!(matches!(
            future_body_outcome(Ok(Err(Signal::Throw(thrown))), &mut child),
            FutureOutcome::Failed(_)
        ));
        let out = future_body_outcome(Ok(Err(Signal::Recur(vec![]))), &mut child);
        assert!(
            matches!(out, FutureOutcome::Failed(t) if t.message.as_deref() == Some("recur escaped its loop or fn"))
        );
        let out = future_body_outcome(Err(Box::new("x")), &mut child);
        assert!(matches!(out, FutureOutcome::Failed(t) if t.message.as_deref() == Some("x")));
    }

    #[test]
    fn delay_outcome_caches_or_restores() {
        let delay = Delay::new(Value::Int(0));
        assert!(matches!(
            delay_outcome(Ok(Value::Int(1)), &delay, Value::Int(0)),
            Ok(Value::Int(1))
        ));
        assert!(matches!(*delay.lock(), DelayState::Realized(Value::Int(1))));
        let delay = Delay::new(Value::Int(0));
        let thrown = Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert!(matches!(
            delay_outcome(Err(Signal::Throw(thrown)), &delay, Value::Int(0)),
            Err(Signal::Throw(_))
        ));
        assert!(matches!(*delay.lock(), DelayState::Failed(_)));
        // The analyzer-impossible case: the thunk is restored, so a
        // retry re-runs.
        let delay = Delay::new(Value::Int(0));
        assert!(matches!(
            delay_outcome(Err(Signal::Recur(vec![])), &delay, Value::Int(7)),
            Err(Signal::Recur(_))
        ));
        assert!(matches!(*delay.lock(), DelayState::Pending(Value::Int(7))));
    }

    #[test]
    fn check_await_errors_reports_latched_failure() {
        // Direct pin: a racing failure mid-wait is timing, but the
        // check itself runs on any failed cell.
        let failed = Agent::new(Value::Int(0));
        failed.complete_failed(Throwable {
            id: 1,
            class: ThrowClass::RuntimeException,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        });
        assert!(check_await_errors(&[failed]).is_err());
        let clean = Agent::new(Value::Int(0));
        assert!(check_await_errors(&[clean]).is_ok());
    }

    #[test]
    fn validation_outcome_maps_every_verdict() {
        assert!(validation_outcome(Ok(Value::Bool(true))).is_ok());
        assert!(validation_outcome(Ok(Value::Bool(false))).is_err());
        let thrown = Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert!(validation_outcome(Err(Signal::Throw(thrown))).is_err());
        // The analyzer-impossible case: funnels through the shared
        // signal mapping like any other non-throw.
        assert!(validation_outcome(Err(Signal::Recur(vec![]))).is_err());
    }

    #[test]
    fn spawn_failed_reports_native_thread_error() {
        let err = spawn_failed();
        assert_eq!(err.class, ThrowClass::RuntimeException);
        assert_eq!(err.message, "unable to create native thread");
    }

    #[test]
    fn install_worker_pushes_started_handle() {
        let pools = AgentPools::create().expect("pool starts");
        let mut handles = Vec::new();
        let spawned = thread::spawn(|| {});
        AgentPools::install_worker(&pools, &mut handles, Ok(spawned)).expect("install");
        assert_eq!(handles.len(), 1);
        for handle in handles.drain(..) {
            let _ = handle.join();
        }
    }

    #[test]
    fn install_worker_spawn_failure_tears_down() {
        let pools = AgentPools::create().expect("pool starts");
        // A live handle proves the teardown joins what did start.
        let mut handles = vec![thread::spawn(|| {})];
        let err =
            AgentPools::install_worker(&pools, &mut handles, Err(std::io::Error::other("boom")))
                .expect_err("spawn failure errors");
        assert_eq!(err.class, ThrowClass::RuntimeException);
        assert_eq!(err.message, "unable to create native thread");
        assert!(handles.is_empty());
        assert!(
            pools
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send_tx
                .is_none()
        );
    }

    /// A rejection probe: records the handler invocation.
    struct RecordReject {
        fired: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl AgentAction for RecordReject {
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

        fn handle_error(&mut self, _handler: &Value, _agent: &Value, _error: &Throwable) {
            self.fired.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn record_reject_runs_and_fires() {
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut probe = RecordReject { fired };
        let state = Value::Int(7);
        let out = probe.run(&state).expect("run ok");
        assert_eq!(out, state);
        probe
            .fire_watches(&Value::Nil, &Value::Nil, &Value::Nil, &new_map())
            .expect("fires ok");
    }

    #[test]
    fn complete_sendoff_spawn_failure_rejects_head() {
        let pools = AgentPools::create().expect("pool starts");
        let agent = Agent::new(Value::Int(0));
        agent.lock().error_handler = Some(Value::Nil);
        let inner = pools.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (head_tx, _head_rx) = mpsc::channel::<QueuedAction>();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let head = QueuedAction {
            pool: AgentPool::SendOff,
            action: Box::new(RecordReject {
                fired: std::sync::Arc::clone(&fired),
            }),
        };
        complete_sendoff(
            inner,
            head_tx,
            &agent,
            head,
            Err(std::io::Error::other("boom")),
        );
        assert!(fired.load(Ordering::Relaxed));
    }
}
