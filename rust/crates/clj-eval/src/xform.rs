// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! T1200 D1 transducers: transducer-returning arities for the seq
//! fns, plus the transducer core (`transduce`, `sequence`,
//! `eduction`, `completing`, `ensure-reduced`, `unreduced`,
//! `preserving-reduced`, `halt-when`, `cat`). Oracle:
//! `docs/t1200/probes/xform.out` (alpha10 jars); step contracts
//! cross-checked against `core.clj` in the alpha10 jar.
//!
//! A transducer is a [`NativeFn`] taking the downstream reducing
//! fn (`rf`) and returning the stepping fn (init/complete/step by
//! arity, exactly like the JVM's `(fn [rf] (fn ([] …) …))`).
//! Stateful transducers (`take`, `distinct`, …) keep per-application
//! state in an `Arc<Mutex<…>>` captured by the step closure —
//! `std::sync::Mutex` is the established interior-mutability
//! pattern (`concurrency.rs`, `clj-value` cells). State is
//! per-reduction and never shared across threads by user code, so
//! no loom model is needed (spec §1.2).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use clj_value::{ExtensionValue, Keyword, Value, new_map};

use crate::core::{Reduced, as_reduced, take_count};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ext::{NativeFn, NativeImpl};
use crate::ns::{NativeFnPtr, check_arity, deref_core, signal_to_native};
use crate::seqs::call_checked;

/// The `clojure.core` transducer natives plus the private helper
/// (installed by [`boot`](crate::ns::boot)).
pub(crate) const XFORM_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("transduce", native_transduce),
    ("sequence", native_sequence),
    ("eduction", native_eduction),
    ("completing", native_completing),
    ("ensure-reduced", native_ensure_reduced),
    ("unreduced", native_unreduced),
    ("halt-when", native_halt_when),
    ("cat", native_cat),
    ("preserving-reduced", native_preserving_reduced),
];

/// Installed then marked `^:private` (chained into
/// `boot_mark_private` like `FX4_PRIVATE`): unresolvable
/// unqualified, resolvable qualified, cross-ns calls rejected —
/// exactly the JVM's `^:private` shape (oracle
/// `resolve-preserving*`, `preserving-*`).
pub(crate) const XFORM_PRIVATE: &[&str] = &["preserving-reduced"];

/// A fresh [`NativeFn`] value capturing its environment (mirrors
/// `seqs.rs`, whose helper is module-private).
fn transducer(
    eval: &mut Evaluator,
    display: &str,
    func: impl Fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError> + Send + Sync + 'static,
) -> Value {
    Value::Extension(Arc::new(NativeFn {
        id: eval.next_id(),
        display: Arc::from(display),
        func: Arc::new(func) as Arc<NativeImpl>,
        meta: None,
    }))
}

/// The `::halt` key (`halt-when` wraps the tripping input as
/// `{::halt input}`, unwrapped by completion — oracle `halt-*`,
/// `core.clj` source).
fn halt_key() -> Value {
    Value::Keyword(Keyword::new(Some("clojure.core"), "halt"))
}

/// Apply a transducer builder to `rf`, yielding the stepping fn
/// (what `TransformerIterator` does per reduction).
pub(crate) fn apply_xf(
    eval: &mut Evaluator,
    xform: &Value,
    rf: Value,
) -> Result<Value, NativeError> {
    call_checked(eval, xform, &[rf])
}

/// An `eduction` value: the raw transducer plus its collection
/// (applications re-run per use, like the JVM's `Eduction`
/// — oracle `eduction-*`).
#[derive(Debug)]
pub(crate) struct Eduction {
    id: u64,
    xform: Value,
    coll: Value,
}

static EDUCTION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Eduction {
    pub(crate) fn of(xform: Value, coll: Value) -> Value {
        Value::Extension(Arc::new(Self {
            id: EDUCTION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            xform,
            coll,
        }))
    }

    /// The `(xform, coll)` pair, if a value is an eduction.
    pub(crate) fn parts(v: &Value) -> Option<(&Value, &Value)> {
        match v {
            Value::Extension(e) => e
                .as_any()
                .downcast_ref::<Self>()
                .map(|r| (&r.xform, &r.coll)),
            _ => None,
        }
    }
}

impl ExtensionValue for Eduction {
    fn class_name(&self) -> String {
        "clojure.core.Eduction".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[clojure.core.Eduction 0x{:x} \"clojure.core.Eduction@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `(completing f)` / `(completing f cf)`: `([] (f))`,
/// `([x] (cf x))`, `([x y] (f x y))` (oracle `completing-*`).
pub(crate) fn completing_fn(eval: &mut Evaluator, f: Value, cf: Option<Value>) -> Value {
    transducer(eval, "clojure.core/completing$fn", move |eval, args| {
        match args.len() {
            0 => call_checked(eval, &f, &[]),
            1 => {
                // `([f] (completing f identity))`: the default
                // completion is `identity`, never `f` (oracle
                // `err-completing-badf` never calls `f`).
                let done = cf.clone().unwrap_or_else(|| deref_core(eval, "identity"));
                call_checked(eval, &done, &[args[0].clone()])
            }
            2 => call_checked(eval, &f, &[args[0].clone(), args[1].clone()]),
            n => Err(NativeError::arity("clojure.core/completing$fn", n)),
        }
    })
}

fn native_completing(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/completing", args, &[1, 2])?;
    let cf = if args.len() == 2 {
        Some(args[1].clone())
    } else {
        None
    };
    Ok(completing_fn(eval, args[0].clone(), cf))
}

fn native_ensure_reduced(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ensure-reduced", args, &[1])?;
    if as_reduced(&args[0]).is_some() {
        return Ok(args[0].clone());
    }
    Ok(Reduced::wrap(args[0].clone()))
}

fn native_unreduced(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/unreduced", args, &[1])?;
    if let Some(inner) = as_reduced(&args[0]) {
        return Ok(inner.clone());
    }
    Ok(args[0].clone())
}

/// `(preserving-reduced rf)`: re-wraps a reduced step result so an
/// enclosing `reduce` keeps stopping (private; oracle
/// `preserving-*`, `core.clj` source). Takes exactly the step pair.
fn native_preserving_reduced(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/preserving-reduced", args, &[1])?;
    let rf = args[0].clone();
    Ok(transducer(
        eval,
        "clojure.core/preserving-reduced",
        move |eval, args| {
            check_arity("clojure.core/preserving-reduced$fn", args, &[2])?;
            let ret = call_checked(eval, &rf, &[args[0].clone(), args[1].clone()])?;
            if as_reduced(&ret).is_some() {
                return Ok(Reduced::wrap(ret));
            }
            Ok(ret)
        },
    ))
}

/// `(halt-when pred)` / `(halt-when pred retf)`: the tripping input
/// short-circuits as `{::halt …}`, unwrapped by completion without
/// calling `rf` (oracle `halt-*`, `core.clj` source). Two levels
/// like every builder: the outer fn takes `rf`, the inner steps.
fn native_halt_when(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/halt-when", args, &[1, 2])?;
    let pred = args[0].clone();
    let retf = if args.len() == 2 {
        Some(args[1].clone())
    } else {
        None
    };
    Ok(transducer(
        eval,
        "clojure.core/halt-when$fn",
        move |eval, args| {
            check_arity("clojure.core/halt-when$fn", args, &[1])?;
            let (pred, retf) = (pred.clone(), retf.clone());
            let rf = args[0].clone();
            Ok(transducer(
                eval,
                "clojure.core/halt-when$fn",
                move |eval, args| {
                    let (pred, retf, rf) = (pred.clone(), retf.clone(), rf.clone());
                    match args.len() {
                        0 => call_checked(eval, &rf, &[]),
                        1 => {
                            let result = &args[0];
                            if let Value::Map(m) = result {
                                if let Some(halt) = m.get(&halt_key()) {
                                    return Ok(halt.clone());
                                }
                            }
                            call_checked(eval, &rf, std::slice::from_ref(result))
                        }
                        2 => {
                            let (result, input) = (args[0].clone(), args[1].clone());
                            if call_checked(eval, &pred, std::slice::from_ref(&input))?.is_truthy()
                            {
                                let halted = match &retf {
                                    Some(retf) => {
                                        let completed = call_checked(eval, &rf, &[result])?;
                                        call_checked(eval, retf, &[completed, input])?
                                    }
                                    None => input,
                                };
                                let held = new_map().insert(halt_key(), halted);
                                return Ok(Reduced::wrap(Value::Map(held)));
                            }
                            call_checked(eval, &rf, &[result, input])
                        }
                        n => Err(NativeError::arity("clojure.core/halt-when$fn", n)),
                    }
                },
            ))
        },
    ))
}

/// The shared reduction: apply `xform` to `f`, walk `coll` through
/// the stepping fn (unwrapping `Reduced` to stop, like
/// `coll-reduce`), then complete — exactly `core.clj` `transduce`.
/// Used by `transduce`, 4-arg `into`, and 3-arg `reduce` over an
/// eduction (which wraps `f` in `completing`, per the source).
pub(crate) fn transduce_into(
    eval: &mut Evaluator,
    xform: &Value,
    f: &Value,
    init: Value,
    coll: &Value,
) -> Result<Value, NativeError> {
    let step = call_checked(eval, xform, std::slice::from_ref(f))?;
    let mut acc = init;
    let mut seq = crate::lazy::seq_value(eval, coll).map_err(signal_to_native)?;
    while !matches!(seq, Value::Nil) {
        let head = crate::lazy::first_value(eval, &seq).map_err(signal_to_native)?;
        acc = call_checked(eval, &step, &[acc, head])?;
        if let Some(inner) = as_reduced(&acc) {
            acc = inner.clone();
            break;
        }
        seq = crate::lazy::next_value(eval, &seq).map_err(signal_to_native)?;
    }
    call_checked(eval, &step, &[acc])
}

fn native_transduce(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/transduce", args, &[3, 4])?;
    let (xform, f) = (args[0].clone(), args[1].clone());
    let (init, coll) = if args.len() == 4 {
        (args[2].clone(), &args[3])
    } else {
        let init = call_checked(eval, &f, &[])?;
        (init, &args[2])
    };
    transduce_into(eval, &xform, &f, init, coll)
}

/// `(cat rf)`: concatenates each input collection into the
/// reduction (oracle `cat-*`, `core.clj` source).
fn native_cat(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/cat", args, &[1])?;
    let rf = args[0].clone();
    Ok(transducer(
        eval,
        "clojure.core/cat$fn",
        move |eval, args| {
            let rf = rf.clone();
            match args.len() {
                0 => call_checked(eval, &rf, &[]),
                1 => call_checked(eval, &rf, &[args[0].clone()]),
                2 => {
                    let preserving = preserving_step(eval, rf)?;
                    let mut acc = args[0].clone();
                    let mut seq =
                        crate::lazy::seq_value(eval, &args[1]).map_err(signal_to_native)?;
                    while !matches!(seq, Value::Nil) {
                        let head =
                            crate::lazy::first_value(eval, &seq).map_err(signal_to_native)?;
                        acc = call_checked(eval, &preserving, &[acc, head])?;
                        if let Some(inner) = as_reduced(&acc) {
                            acc = inner.clone();
                            break;
                        }
                        seq = crate::lazy::next_value(eval, &seq).map_err(signal_to_native)?;
                    }
                    Ok(acc)
                }
                n => Err(NativeError::arity("clojure.core/cat$fn", n)),
            }
        },
    ))
}

/// The `(preserving-reduced rf)` stepping fn as a value (shared by
/// `cat` and `mapcat`).
fn preserving_step(eval: &mut Evaluator, rf: Value) -> Result<Value, NativeError> {
    native_preserving_reduced(eval, &[rf])
}

/// Init/complete delegation shared by every stepping fn.
fn xf_init(eval: &mut Evaluator, rf: &Value) -> Result<Value, NativeError> {
    call_checked(eval, rf, &[])
}

/// Init/complete delegation shared by every stepping fn.
fn xf_complete(eval: &mut Evaluator, rf: &Value, result: Value) -> Result<Value, NativeError> {
    call_checked(eval, rf, &[result])
}

/// The outer `(fn [rf])` level shared by every seq-fn transducer:
/// checks the single `rf` arg, then builds the stepping fn (fresh
/// state per application, like the JVM's per-`volatile` capture).
fn xf_outer(
    eval: &mut Evaluator,
    display: &'static str,
    build: impl Fn(&mut Evaluator, Value) -> Result<Value, NativeError> + Send + Sync + 'static,
) -> Value {
    transducer(eval, display, move |eval, args| {
        check_arity(display, args, &[1])?;
        build(eval, args[0].clone())
    })
}

/// The `(map f)` stepping fn over a fixed downstream `rf`
/// (shared by `map` and `mapcat`, which composes it over `cat`).
fn map_step_value(eval: &mut Evaluator, display: &'static str, f: Value, rf: Value) -> Value {
    transducer(eval, display, move |eval, args| {
        let (f, rf) = (f.clone(), rf.clone());
        match args.len() {
            0 => xf_init(eval, &rf),
            1 => xf_complete(eval, &rf, args[0].clone()),
            2 => {
                let v = call_checked(eval, &f, &[args[1].clone()])?;
                call_checked(eval, &rf, &[args[0].clone(), v])
            }
            _ => {
                let mut call = Vec::with_capacity(args.len() - 1);
                for input in &args[1..] {
                    call.push(input.clone());
                }
                let v = call_checked(eval, &f, &call)?;
                call_checked(eval, &rf, &[args[0].clone(), v])
            }
        }
    })
}

/// `(map f)`: init/complete passthrough, step maps one (or, with
/// extra inputs, `apply`-style many) inputs (oracle `map-*`,
/// `core.clj` source — the only builder with an N-ary step arm).
pub(crate) fn map_xf(eval: &mut Evaluator, f: Value) -> Value {
    xf_outer(eval, "clojure.core/map$fn", move |eval, rf| {
        Ok(map_step_value(eval, "clojure.core/map$fn", f.clone(), rf))
    })
}

/// `(mapcat f)` is `(comp (map f) cat)`: the map step runs over the
/// cat step (oracle `mapcat-step`, `core.clj` source).
pub(crate) fn mapcat_xf(eval: &mut Evaluator, f: Value) -> Value {
    xf_outer(eval, "clojure.core/mapcat$fn", move |eval, rf| {
        let cat_step = native_cat(eval, &[rf])?;
        Ok(map_step_value(
            eval,
            "clojure.core/mapcat$fn",
            f.clone(),
            cat_step,
        ))
    })
}

/// `(filter pred)` / `(remove pred)`: passing inputs reduce,
/// dropped inputs return the result untouched (oracle
/// `filter-step-*`, `remove-step`).
pub(crate) fn filter_xf(
    eval: &mut Evaluator,
    display: &'static str,
    pred: Value,
    invert: bool,
) -> Value {
    xf_outer(eval, display, move |eval, rf| {
        let (pred, rf) = (pred.clone(), rf);
        Ok(transducer(eval, display, move |eval, args| {
            let (pred, rf) = (pred.clone(), rf.clone());
            match args.len() {
                0 => xf_init(eval, &rf),
                1 => xf_complete(eval, &rf, args[0].clone()),
                2 => {
                    let pass = call_checked(eval, &pred, &[args[1].clone()])?.is_truthy();
                    if pass != invert {
                        return call_checked(eval, &rf, &[args[0].clone(), args[1].clone()]);
                    }
                    Ok(args[0].clone())
                }
                n => Err(NativeError::arity(display, n)),
            }
        }))
    })
}

/// `(keep f)` / `(keep-indexed f)`: `nil` results are dropped
/// (oracle `keep-step-*`).
pub(crate) fn keep_xf(
    eval: &mut Evaluator,
    display: &'static str,
    f: Value,
    indexed: bool,
) -> Value {
    xf_outer(eval, display, move |eval, rf| {
        let (f, rf) = (f.clone(), rf);
        let idx = Arc::new(Mutex::new(-1i64));
        Ok(transducer(eval, display, move |eval, args| {
            let (f, rf) = (f.clone(), rf.clone());
            match args.len() {
                0 => xf_init(eval, &rf),
                1 => xf_complete(eval, &rf, args[0].clone()),
                2 => {
                    // The counter drops before the user call: locks
                    // never span calls (reentrant steps must not
                    // deadlock, unlike `swap!` on a volatile).
                    let v = if indexed {
                        let i = {
                            let mut guard = idx.lock().expect("xform counter is never poisoned");
                            *guard += 1;
                            *guard
                        };
                        call_checked(eval, &f, &[Value::Int(i), args[1].clone()])?
                    } else {
                        call_checked(eval, &f, &[args[1].clone()])?
                    };
                    if matches!(v, Value::Nil) {
                        return Ok(args[0].clone());
                    }
                    call_checked(eval, &rf, &[args[0].clone(), v])
                }
                n => Err(NativeError::arity(display, n)),
            }
        }))
    })
}

/// `(map-indexed f)`: the running index feeds `f` (oracle
/// `map-indexed-step`).
pub(crate) fn map_indexed_xf(eval: &mut Evaluator, f: Value) -> Value {
    xf_outer(eval, "clojure.core/map-indexed$fn", move |eval, rf| {
        let (f, rf) = (f.clone(), rf);
        let idx = Arc::new(Mutex::new(-1i64));
        Ok(transducer(
            eval,
            "clojure.core/map-indexed$fn",
            move |eval, args| {
                let (f, rf) = (f.clone(), rf.clone());
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let i = {
                            let mut guard = idx.lock().expect("xform counter is never poisoned");
                            *guard += 1;
                            *guard
                        };
                        let v = call_checked(eval, &f, &[Value::Int(i), args[1].clone()])?;
                        call_checked(eval, &rf, &[args[0].clone(), v])
                    }
                    n => Err(NativeError::arity("clojure.core/map-indexed$fn", n)),
                }
            },
        ))
    })
}

/// `(take n)`: open windows reduce, exhaustion wraps the result in
/// `Reduced` (oracle `take-step`, `core.clj` source).
pub(crate) fn take_xf(eval: &mut Evaluator, n: Value) -> Result<Value, NativeError> {
    let n = take_count(&n)?;
    Ok(xf_outer(eval, "clojure.core/take$fn", move |eval, rf| {
        let rf = rf;
        let left = Arc::new(Mutex::new(n));
        Ok(transducer(
            eval,
            "clojure.core/take$fn",
            move |eval, args| {
                let rf = rf.clone();
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let (open, spent) = {
                            let mut guard = left.lock().expect("xform counter is never poisoned");
                            let open = *guard;
                            *guard = guard.saturating_sub(1);
                            (open, *guard)
                        };
                        let result = if open > 0 {
                            call_checked(eval, &rf, &[args[0].clone(), args[1].clone()])?
                        } else {
                            args[0].clone()
                        };
                        if spent <= 0 {
                            return Ok(Reduced::wrap(result));
                        }
                        Ok(result)
                    }
                    n => Err(NativeError::arity("clojure.core/take$fn", n)),
                }
            },
        ))
    }))
}

/// `(drop n)`: skipped inputs return the result untouched (oracle
/// `drop-*`, `core.clj` source).
pub(crate) fn drop_xf(eval: &mut Evaluator, n: Value) -> Result<Value, NativeError> {
    let n = take_count(&n)?;
    Ok(xf_outer(eval, "clojure.core/drop$fn", move |eval, rf| {
        let rf = rf;
        let left = Arc::new(Mutex::new(n));
        Ok(transducer(
            eval,
            "clojure.core/drop$fn",
            move |eval, args| {
                let rf = rf.clone();
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let skip = {
                            let mut guard = left.lock().expect("xform counter is never poisoned");
                            let skip = *guard;
                            *guard = guard.saturating_sub(1);
                            skip
                        };
                        if skip > 0 {
                            return Ok(args[0].clone());
                        }
                        call_checked(eval, &rf, &[args[0].clone(), args[1].clone()])
                    }
                    n => Err(NativeError::arity("clojure.core/drop$fn", n)),
                }
            },
        ))
    }))
}

/// `(take-while pred)`: the first failing input wraps the result in
/// `Reduced` (oracle `take-while-step`).
pub(crate) fn take_while_xf(eval: &mut Evaluator, pred: Value) -> Value {
    xf_outer(eval, "clojure.core/take-while$fn", move |eval, rf| {
        let (pred, rf) = (pred.clone(), rf);
        Ok(transducer(
            eval,
            "clojure.core/take-while$fn",
            move |eval, args| {
                let (pred, rf) = (pred.clone(), rf.clone());
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        if call_checked(eval, &pred, &[args[1].clone()])?.is_truthy() {
                            return call_checked(eval, &rf, &[args[0].clone(), args[1].clone()]);
                        }
                        Ok(Reduced::wrap(args[0].clone()))
                    }
                    n => Err(NativeError::arity("clojure.core/take-while$fn", n)),
                }
            },
        ))
    })
}

/// `(drop-while pred)`: dropping inputs return the result until the
/// first failure flips the latch (oracle `drop-while-step`).
pub(crate) fn drop_while_xf(eval: &mut Evaluator, pred: Value) -> Value {
    xf_outer(eval, "clojure.core/drop-while$fn", move |eval, rf| {
        let (pred, rf) = (pred.clone(), rf);
        let dropping = Arc::new(Mutex::new(true));
        Ok(transducer(
            eval,
            "clojure.core/drop-while$fn",
            move |eval, args| {
                let (pred, rf) = (pred.clone(), rf.clone());
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        // Locks never span the predicate call.
                        let still_dropping =
                            *dropping.lock().expect("xform latch is never poisoned");
                        if still_dropping {
                            if call_checked(eval, &pred, &[args[1].clone()])?.is_truthy() {
                                return Ok(args[0].clone());
                            }
                            *dropping.lock().expect("xform latch is never poisoned") = false;
                        }
                        call_checked(eval, &rf, &[args[0].clone(), args[1].clone()])
                    }
                    n => Err(NativeError::arity("clojure.core/drop-while$fn", n)),
                }
            },
        ))
    })
}

/// `(take-nth n)`: every nth input reduces (oracle `take-nth-step`).
pub(crate) fn take_nth_xf(eval: &mut Evaluator, n: Value) -> Result<Value, NativeError> {
    let n = take_count(&n)?;
    Ok(xf_outer(
        eval,
        "clojure.core/take-nth$fn",
        move |eval, rf| {
            let rf = rf;
            let idx = Arc::new(Mutex::new(-1i64));
            Ok(transducer(
                eval,
                "clojure.core/take-nth$fn",
                move |eval, args| {
                    let rf = rf.clone();
                    match args.len() {
                        0 => xf_init(eval, &rf),
                        1 => xf_complete(eval, &rf, args[0].clone()),
                        2 => {
                            let take = {
                                let mut guard =
                                    idx.lock().expect("xform counter is never poisoned");
                                *guard += 1;
                                *guard % n == 0
                            };
                            if take {
                                return call_checked(
                                    eval,
                                    &rf,
                                    &[args[0].clone(), args[1].clone()],
                                );
                            }
                            Ok(args[0].clone())
                        }
                        n => Err(NativeError::arity("clojure.core/take-nth$fn", n)),
                    }
                },
            ))
        },
    ))
}

/// `(distinct)`: seen inputs are dropped (oracle `distinct-step`).
pub(crate) fn distinct_xf(eval: &mut Evaluator) -> Value {
    xf_outer(eval, "clojure.core/distinct$fn", move |eval, rf| {
        let rf = rf;
        let seen = Arc::new(Mutex::new(HashSet::new()));
        Ok(transducer(
            eval,
            "clojure.core/distinct$fn",
            move |eval, args| {
                let rf = rf.clone();
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let fresh = {
                            seen.lock()
                                .expect("xform set is never poisoned")
                                .insert(args[1].clone())
                        };
                        if fresh {
                            return call_checked(eval, &rf, &[args[0].clone(), args[1].clone()]);
                        }
                        Ok(args[0].clone())
                    }
                    n => Err(NativeError::arity("clojure.core/distinct$fn", n)),
                }
            },
        ))
    })
}

/// `(dedupe)`: runs against the last kept input are dropped (oracle
/// `dedupe-step`; `None` is the `::none` sentinel, so a leading
/// `nil` still emits).
pub(crate) fn dedupe_xf(eval: &mut Evaluator) -> Value {
    xf_outer(eval, "clojure.core/dedupe$fn", move |eval, rf| {
        let rf = rf;
        let prior: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        Ok(transducer(
            eval,
            "clojure.core/dedupe$fn",
            move |eval, args| {
                let rf = rf.clone();
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let dup = {
                            let mut guard = prior.lock().expect("xform slot is never poisoned");
                            let dup = guard.as_ref().is_some_and(|p| *p == args[1]);
                            *guard = Some(args[1].clone());
                            dup
                        };
                        if dup {
                            return Ok(args[0].clone());
                        }
                        call_checked(eval, &rf, &[args[0].clone(), args[1].clone()])
                    }
                    n => Err(NativeError::arity("clojure.core/dedupe$fn", n)),
                }
            },
        ))
    })
}

/// `(interpose sep)`: the separator reduces between inputs; a
/// reduced separator step propagates without touching the input
/// (oracle `interpose-step`, `core.clj` source).
pub(crate) fn interpose_xf(eval: &mut Evaluator, sep: Value) -> Value {
    xf_outer(eval, "clojure.core/interpose$fn", move |eval, rf| {
        let (sep, rf) = (sep.clone(), rf);
        let started = Arc::new(Mutex::new(false));
        Ok(transducer(
            eval,
            "clojure.core/interpose$fn",
            move |eval, args| {
                let (sep, rf) = (sep.clone(), rf.clone());
                match args.len() {
                    0 => xf_init(eval, &rf),
                    1 => xf_complete(eval, &rf, args[0].clone()),
                    2 => {
                        let first = {
                            let mut guard = started.lock().expect("xform latch is never poisoned");
                            let first = !*guard;
                            *guard = true;
                            first
                        };
                        if first {
                            return call_checked(eval, &rf, &[args[0].clone(), args[1].clone()]);
                        }
                        let sepr = call_checked(eval, &rf, &[args[0].clone(), sep.clone()])?;
                        if as_reduced(&sepr).is_some() {
                            return Ok(sepr);
                        }
                        call_checked(eval, &rf, &[sepr, args[1].clone()])
                    }
                    n => Err(NativeError::arity("clojure.core/interpose$fn", n)),
                }
            },
        ))
    })
}

/// `(sequence coll)`: the collection as a seq (`(or (seq coll)
/// ())`, oracle `sequence-1*`). With a transducer, the lazy
/// transform (`sequence-*`, chunked like the port's lazy seqs).
fn native_sequence(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() == 1 {
        let s = crate::lazy::seq_value(eval, &args[0]).map_err(signal_to_native)?;
        if matches!(s, Value::Nil) {
            return Ok(crate::lazy::empty_list_value());
        }
        return Ok(s);
    }
    crate::vars::check_min_arity("clojure.core/sequence", args, 2)?;
    sequence_of(eval, &args[0], &args[1..])
}

/// `(sequence xform coll & colls)`: lockstep multi-source transform
/// (oracle `sequence-multi`).
pub(crate) fn sequence_of(
    eval: &mut Evaluator,
    xform: &Value,
    colls: &[Value],
) -> Result<Value, NativeError> {
    // JVM order (`TransformerIterator/create`): apply the transducer
    // first, then validate (`RT.iter`) before any realization.
    let conj = deref_core(eval, "conj");
    let step = call_checked(eval, xform, &[conj])?;
    for coll in colls {
        crate::lazy::check_seqable(eval, coll).map_err(signal_to_native)?;
    }
    Ok(crate::lazy::sequence_thunk(step, colls.to_vec()))
}

/// `(into to xform from)`: exactly `core.clj` — transients reduce
/// through a `persistent!`/`conj!` rf over `(transient to)`, so
/// transient-of-transient takes the pre-existing transient cast
/// (oracle `into-to-transient`); everything else transduces with
/// `conj` (oracle `into-xf`, `into-halt`).
pub(crate) fn into_xform(
    eval: &mut Evaluator,
    to: Value,
    xform: &Value,
    from: &Value,
) -> Result<Value, NativeError> {
    // `IEditableCollection` ≈ persistent vector/map/set — plus
    // transients, which take the `(transient to)` call below and
    // throw there, exactly like the JVM (oracle
    // `into-to-transient`).
    if matches!(
        to,
        Value::Vector(_) | Value::Map(_) | Value::Set(_) | Value::Transient(_)
    ) {
        let transient_to = call_checked(eval, &deref_core(eval, "transient"), &[to])?;
        let rf = into_transient_rf(eval);
        return transduce_into(eval, xform, &rf, transient_to, from);
    }
    transduce_into(eval, xform, &deref_core(eval, "conj"), to, from)
}

/// The transient `into` reducing fn: completion persists,
/// stepping conjoins (`core.clj` source). Internal to
/// `transduce_into` (1/2-arg only); other arities report against
/// `into` and are pinned by the unit test below.
fn into_transient_rf(eval: &mut Evaluator) -> Value {
    transducer(eval, "clojure.core/into$fn", move |eval, args| {
        match args.len() {
            1 => call_checked(eval, &deref_core(eval, "persistent!"), &[args[0].clone()]),
            2 => call_checked(
                eval,
                &deref_core(eval, "conj!"),
                &[args[0].clone(), args[1].clone()],
            ),
            n => Err(NativeError::arity("clojure.core/into$fn", n)),
        }
    })
}

/// `(eduction xform* coll)`: the raw pair as a reducible/seqable
/// value — applications re-run per use (oracle `eduction-*`,
/// `core.clj` source: variadic, `(apply comp (butlast …))` over the
/// leading forms).
fn native_eduction(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let identity = deref_core(eval, "identity");
    if args.is_empty() {
        return Ok(Eduction::of(identity, Value::Nil));
    }
    let (heads, last) = args.split_at(args.len() - 1);
    let mut composed = identity;
    if !heads.is_empty() {
        composed = call_checked(eval, &deref_core(eval, "comp"), heads)?;
    }
    Ok(Eduction::of(composed, last[0].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_transient_rf_rejects_other_arities() {
        let mut eval = crate::ns::boot();
        let rf = into_transient_rf(&mut eval);
        for args in [Vec::new(), vec![Value::Nil, Value::Nil, Value::Nil]] {
            match crate::seqs::call_checked(&mut eval, &rf, &args) {
                Err(e) => assert!(
                    format!("{e:?}").contains("clojure.core/into$fn"),
                    "unexpected error: {e:?}"
                ),
                Ok(v) => panic!("expected an arity error, got {v:?}"),
            }
        }
    }
}
