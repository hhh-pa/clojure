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

use std::sync::Arc;
use std::time::Duration;

use clj_value::{
    Atom, Delay, DelayState, Future, FutureOutcome, FutureState, Keyword, Map, PrintOpts, Promise,
    ThrowClass, Throwable, Value, new_map, pr_to_string,
};

use crate::core::{cce_err, is_ifn};
use crate::error::{NativeError, Signal};
use crate::eval::Evaluator;
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

/// An `IRef` receiver (atoms in S6A): nil trips the call-shape NPE,
/// anything else non-atom the `IRef` cast.
fn as_iref(v: &Value, nil_message: &str) -> Result<Atom, NativeError> {
    match v {
        Value::Atom(a) => Ok(a.clone()),
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
    if let Some(m) = opts.get(&meta_key)
        && m.is_truthy()
    {
        match m {
            Value::Map(mm) => atom.lock().meta = Some(mm.clone()),
            _ => return Err(cce_err(m, "clojure.lang.IPersistentMap")),
        }
    }
    let validator_key = Value::Keyword(Keyword::new(None, "validator"));
    if let Some(f) = opts.get(&validator_key)
        && f.is_truthy()
    {
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
    match eval.call(validator, std::slice::from_ref(candidate)) {
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
    let atom = as_iref(
        &args[0],
        "Cannot invoke \"clojure.lang.IRef.addWatch(Object, clojure.lang.IFn)\" because \"reference\" is null",
    )?;
    // Nil fns install (stored, never fired); other non-`IFn`s fail
    // the cast.
    if !matches!(&args[2], Value::Nil) && !is_ifn(&args[2]) {
        return Err(cce_err(&args[2], "clojure.lang.IFn"));
    }
    {
        let mut state = atom.lock();
        state.watches = state
            .watches
            .clone()
            .insert(args[1].clone(), args[2].clone());
    }
    Ok(Value::Atom(atom))
}

fn native_remove_watch(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/remove-watch", args, &[2])?;
    let atom = as_iref(
        &args[0],
        "Cannot invoke \"clojure.lang.IRef.removeWatch(Object)\" because \"reference\" is null",
    )?;
    {
        let mut state = atom.lock();
        state.watches = state.watches.clone().without(&args[1]);
    }
    Ok(Value::Atom(atom))
}

/// A taken delay: either the cached outcome or the fn to run (the
/// cell now reads `Forcing`, so a re-entrant force errors instead of
/// recursing).
enum DelayTake {
    Done(Result<Value, Signal>),
    Thunk(Value),
}

/// Take a delay's cached outcome or its pending fn (leaving
/// `Forcing` behind; cached states restore themselves).
fn take_delay_thunk(eval: &mut Evaluator, delay: &Delay) -> DelayTake {
    let mut state = delay.lock();
    match std::mem::replace(&mut *state, DelayState::Forcing) {
        DelayState::Realized(v) => {
            *state = DelayState::Realized(v.clone());
            DelayTake::Done(Ok(v))
        }
        DelayState::Failed(t) => {
            *state = DelayState::Failed(t.clone());
            DelayTake::Done(Err(Signal::Throw(t)))
        }
        DelayState::Forcing => {
            *state = DelayState::Forcing;
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
    match eval.call(&thunk, &[]) {
        Ok(v) => {
            *delay.lock() = DelayState::Realized(v.clone());
            Ok(v)
        }
        Err(Signal::Throw(t)) => {
            *delay.lock() = DelayState::Failed(t.clone());
            Err(Signal::Throw(t))
        }
        Err(other) => {
            // A `recur` escaped its frame (the analyzer rejects
            // those — restore, so a retry re-runs).
            *delay.lock() = DelayState::Pending(thunk);
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
/// with an empty message (any arity, even with a timeout value).
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

/// `future-call`: convey the calling thread's bindings onto a forked
/// evaluator and spawn the zero-arg fn on its own thread. The fn is
/// never validated eagerly (like the JVM's `binding-conveyor-fn`
/// wrap: a non-function fails inside the body, surfacing as the
/// future's cached failure).
fn native_future_call(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/future-call", args, &[1])?;
    let future = Future::new();
    let cell = future.clone();
    let child = eval.fork_for_thread(eval.clone_bindings());
    let f = args[0].clone();
    let name = format!("clj-future-{}", future.id());
    let handle = std::thread::Builder::new()
        .name(name)
        .spawn(move || run_future_body(child, f, cell))
        .map_err(|_| {
            NativeError::new(
                ThrowClass::RuntimeException,
                "unable to create native thread".to_owned(),
            )
        })?;
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
    let outcome =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child.call(&f, &[]))) {
            Ok(Ok(v)) => FutureOutcome::Value(v),
            Ok(Err(Signal::Throw(t))) => FutureOutcome::Failed(t),
            Ok(Err(Signal::Recur(_))) => {
                FutureOutcome::Failed(error_throwable(&mut child, "recur escaped its loop or fn"))
            }
            Err(panicked) => {
                FutureOutcome::Failed(error_throwable(&mut child, &panic_message(&panicked)))
            }
        };
    // A cancellation wins over this late result (like `FutureTask`
    // after `cancel`).
    future.complete(outcome);
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
fn panic_message(panicked: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panicked.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panicked.downcast_ref::<String>() {
        s.clone()
    } else {
        "future body panicked".to_owned()
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

/// The S6A/S6B natives installed into `clojure.core` by [`boot`](crate::ns::boot).
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
];
