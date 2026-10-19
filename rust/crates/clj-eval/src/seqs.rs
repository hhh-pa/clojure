// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D seq + higher-order natives: lazy transformers over
//! [`crate::lazy`] thunks, eager consumers, and the fn-returning
//! combinators (built as [`NativeFn`] closures).
//!
//! Transducer (`xform`) arities are out of scope: single-argument
//! `map`/`filter`/etc. raise arity errors (their JVM selves
//! return transducer fns), asserted as divergences by the
//! differential corpus.

use std::cmp::Ordering;
use std::sync::Arc;

use clj_value::{List, Value, Vector, new_map};

use crate::core::{as_reduced, cce_err, check_ifn, jvm_compare, repeat_count, take_count};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ext::{NativeFn, NativeImpl};
use crate::lazy::{
    collect_all, concat_thunk, cycle_thunk, dedupe_thunk, distinct_thunk, drop_value,
    drop_while_thunk, empty_list_value, filter_thunk, first_value, interleave_thunk,
    interpose_thunk, iterate_thunk, keep_indexed_thunk, keep_thunk, keys_thunk, map_indexed_thunk,
    map_thunk, mapcat_thunk, next_value, nthrest_value, partition_all_thunk, partition_by_thunk,
    partition_thunk, range1, range2, range3, realized_value, reductions_thunk, repeat_thunk,
    repeatedly_thunk, seq_value, take_nth_thunk, take_thunk, take_while_thunk,
};
use crate::ns::{NativeFnPtr, check_arity, deref_core, signal_to_native};
use crate::vars::check_min_arity;

/// Call a user fn with the `IFn` check first (bad callees fail
/// with the JVM `checkcast` text, matching what the JVM's call
/// sites raise).
fn call_checked(eval: &mut Evaluator, f: &Value, args: &[Value]) -> Result<Value, NativeError> {
    check_ifn(eval, f).map_err(signal_to_native)?;
    eval.call(f, args).map_err(signal_to_native)
}

/// A fresh [`NativeFn`] value capturing its environment.
fn native_fn(
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

/// Head-by-head seq walking (short-circuitable; errors map to
/// native errors at each step).
enum Walk {
    Continue,
    Stop,
}

fn walk_seq(
    eval: &mut Evaluator,
    coll: &Value,
    mut visit: impl FnMut(&mut Evaluator, &Value) -> Result<Walk, NativeError>,
) -> Result<(), NativeError> {
    let mut seq = seq_value(eval, coll).map_err(signal_to_native)?;
    while !matches!(seq, Value::Nil) {
        let head = first_value(eval, &seq).map_err(signal_to_native)?;
        if matches!(visit(eval, &head)?, Walk::Stop) {
            return Ok(());
        }
        seq = next_value(eval, &seq).map_err(signal_to_native)?;
    }
    Ok(())
}

/// Force one step plus `n` heads (mirrors `dorun`'s walk: the
/// initial `seq` always realizes one step, then `n` `next`s).
fn force_n(eval: &mut Evaluator, coll: &Value, n: i64) -> Result<(), NativeError> {
    let mut seq = seq_value(eval, coll).map_err(signal_to_native)?;
    for _ in 0..n.max(0) {
        if matches!(seq, Value::Nil) {
            return Ok(());
        }
        seq = next_value(eval, &seq).map_err(signal_to_native)?;
    }
    Ok(())
}

fn native_map(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/map", args, 2)?;
    Ok(map_thunk(args[0].clone(), args[1..].to_vec()))
}

fn native_filter(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/filter", args, &[2])?;
    Ok(filter_thunk(args[0].clone(), args[1].clone(), false))
}

fn native_remove(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/remove", args, &[2])?;
    Ok(filter_thunk(args[0].clone(), args[1].clone(), true))
}

fn native_take(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/take", args, &[2])?;
    Ok(take_thunk(args[0].clone(), args[1].clone()))
}

fn native_drop(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/drop", args, &[2])?;
    drop_value(eval, &args[0], args[1].clone()).map_err(signal_to_native)
}

fn native_nthrest(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/nthrest", args, &[2])?;
    nthrest_value(eval, args[0].clone(), &args[1]).map_err(signal_to_native)
}

fn native_take_while(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/take-while", args, &[2])?;
    Ok(take_while_thunk(args[0].clone(), args[1].clone()))
}

fn native_drop_while(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/drop-while", args, &[2])?;
    Ok(drop_while_thunk(args[0].clone(), args[1].clone()))
}

fn native_concat(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    Ok(concat_thunk(args.to_vec()))
}

fn native_cycle(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/cycle", args, &[1])?;
    cycle_thunk(eval, args[0].clone()).map_err(signal_to_native)
}

fn native_interleave(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Ok(empty_list_value());
    }
    Ok(interleave_thunk(args.to_vec()))
}

fn native_interpose(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/interpose", args, &[2])?;
    Ok(interpose_thunk(args[0].clone(), args[1].clone()))
}

fn native_repeat(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/repeat", args, &[1, 2])?;
    if args.len() == 1 {
        return Ok(repeat_thunk(args[0].clone(), None));
    }
    let n = repeat_count(&args[0])?;
    Ok(repeat_thunk(args[1].clone(), Some(n)))
}

fn native_repeatedly(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/repeatedly", args, &[1, 2])?;
    if args.len() == 1 {
        return Ok(repeatedly_thunk(args[0].clone()));
    }
    // `(take n (repeatedly f))`: the count stays lazy (validating
    // per force through `take`, ceil rule included).
    Ok(take_thunk(
        args[0].clone(),
        repeatedly_thunk(args[1].clone()),
    ))
}

fn native_iterate(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/iterate", args, &[2])?;
    // Like `Iterate.create`'s `IFn` cast: nil passes (failing on
    // first advance instead), non-callables fail now.
    if !matches!(&args[0], Value::Nil) {
        check_ifn(eval, &args[0]).map_err(signal_to_native)?;
    }
    Ok(iterate_thunk(args[0].clone(), args[1].clone()))
}

fn native_range(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args.len() {
        // `(iterate inc' 0)` (class `Iterate`, not a range).
        0 => Ok(iterate_thunk(deref_core(eval, "inc'"), Value::Int(0))),
        1 => range1(args[0].clone()),
        2 => range2(args[0].clone(), args[1].clone()),
        3 => range3(args[0].clone(), args[1].clone(), args[2].clone()),
        _ => Err(NativeError::arity("clojure.core/range", args.len())),
    }
}

fn native_mapcat(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/mapcat", args, 2)?;
    Ok(mapcat_thunk(args[0].clone(), args[1..].to_vec()))
}

fn native_keep(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/keep", args, &[2])?;
    Ok(keep_thunk(args[0].clone(), args[1].clone()))
}

fn native_keep_indexed(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/keep-indexed", args, &[2])?;
    Ok(keep_indexed_thunk(args[0].clone(), args[1].clone()))
}

fn native_map_indexed(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/map-indexed", args, &[2])?;
    Ok(map_indexed_thunk(args[0].clone(), args[1].clone()))
}

fn native_distinct(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/distinct", args, &[1])?;
    Ok(distinct_thunk(args[0].clone()))
}

fn native_dedupe(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/dedupe", args, &[1])?;
    Ok(dedupe_thunk(args[0].clone()))
}

fn native_partition(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/partition", args, &[2, 3, 4])?;
    let (n, step, pad, coll) = match args.len() {
        2 => (args[0].clone(), args[0].clone(), None, args[1].clone()),
        3 => (args[0].clone(), args[1].clone(), None, args[2].clone()),
        _ => (
            args[0].clone(),
            args[1].clone(),
            Some(args[2].clone()),
            args[3].clone(),
        ),
    };
    Ok(partition_thunk(n, step, pad, coll))
}

fn native_partition_all(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/partition-all", args, &[2, 3])?;
    let (n, step, coll) = match args.len() {
        2 => (args[0].clone(), args[0].clone(), args[1].clone()),
        _ => (args[0].clone(), args[1].clone(), args[2].clone()),
    };
    Ok(partition_all_thunk(n, step, coll))
}

fn native_partition_by(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/partition-by", args, &[2])?;
    Ok(partition_by_thunk(args[0].clone(), args[1].clone()))
}

fn native_take_nth(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/take-nth", args, &[2])?;
    Ok(take_nth_thunk(args[0].clone(), args[1].clone()))
}

fn native_reductions(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reductions", args, &[2, 3])?;
    if args.len() == 2 {
        return Ok(reductions_thunk(args[0].clone(), None, args[1].clone()));
    }
    if let Some(inner) = as_reduced(&args[1]) {
        return Ok(Value::List(List::from_vec(vec![inner.clone()])));
    }
    Ok(reductions_thunk(
        args[0].clone(),
        Some(args[1].clone()),
        args[2].clone(),
    ))
}

fn native_split_at(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/split-at", args, &[2])?;
    let dropped = drop_value(eval, &args[0], args[1].clone()).map_err(signal_to_native)?;
    Ok(Value::Vector(Vector::from_vec(vec![
        take_thunk(args[0].clone(), args[1].clone()),
        dropped,
    ])))
}

fn native_split_with(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/split-with", args, &[2])?;
    Ok(Value::Vector(Vector::from_vec(vec![
        take_while_thunk(args[0].clone(), args[1].clone()),
        drop_while_thunk(args[0].clone(), args[1].clone()),
    ])))
}

fn native_reduce(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reduce", args, &[2, 3])?;
    let f = args[0].clone();
    let (mut acc, has_acc, coll) = if args.len() == 3 {
        (args[1].clone(), true, &args[2])
    } else {
        (Value::Nil, false, &args[1])
    };
    let mut first = true;
    let mut empty = true;
    walk_seq(eval, coll, |eval, head| {
        empty = false;
        if !has_acc && first {
            first = false;
            acc = head.clone();
            return Ok(Walk::Continue);
        }
        first = false;
        acc = call_checked(eval, &f, &[acc.clone(), head.clone()])?;
        if let Some(inner) = as_reduced(&acc) {
            acc = inner.clone();
            return Ok(Walk::Stop);
        }
        Ok(Walk::Continue)
    })?;
    if empty && !has_acc {
        return call_checked(eval, &f, &[]);
    }
    Ok(acc)
}

fn native_take_last(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/take-last", args, &[2])?;
    // Exactly the `loop`: `(seq (drop n coll))` validates `n`
    // eagerly, then both sides walk `next` in lockstep (which keeps
    // the result kind-preserving).
    let mut s = seq_value(eval, &args[1]).map_err(signal_to_native)?;
    let dropped = drop_value(eval, &args[0], args[1].clone()).map_err(signal_to_native)?;
    let mut lead = seq_value(eval, &dropped).map_err(signal_to_native)?;
    while !matches!(lead, Value::Nil) {
        s = next_value(eval, &s).map_err(signal_to_native)?;
        lead = next_value(eval, &lead).map_err(signal_to_native)?;
    }
    Ok(s)
}

fn native_drop_last(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/drop-last", args, &[1, 2])?;
    let (n, coll) = if args.len() == 1 {
        (Value::Int(1), args[0].clone())
    } else {
        (args[0].clone(), args[1].clone())
    };
    let project = native_fn(eval, "clojure.core/drop-last", |_, args: &[Value]| {
        Ok(args.first().cloned().unwrap_or(Value::Nil))
    });
    let dropped = drop_value(eval, &n, coll.clone()).map_err(signal_to_native)?;
    Ok(map_thunk(project, vec![coll, dropped]))
}

fn native_butlast(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/butlast", args, &[1])?;
    let items = collect_all(eval, &args[0]).map_err(signal_to_native)?;
    if items.len() < 2 {
        return Ok(Value::Nil);
    }
    // `(seq ret)`: a vector seq (`ChunkedSeq`, counted).
    Ok(realized_value(
        items[..items.len() - 1].to_vec(),
        true,
        true,
        true,
        clj_value::SeqKind::Chunked,
    ))
}

fn native_keys(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/keys", args, &[1])?;
    keys_thunk(eval, args[0].clone(), true).map_err(signal_to_native)
}

fn native_vals(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/vals", args, &[1])?;
    keys_thunk(eval, args[0].clone(), false).map_err(signal_to_native)
}

fn native_reverse(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reverse", args, &[1])?;
    let mut items = collect_all(eval, &args[0]).map_err(signal_to_native)?;
    items.reverse();
    Ok(Value::List(List::from_vec(items)))
}

/// Interpret a sort-comparator result the way `AFunction.compare`
/// does: booleans order (false consults the reversed call),
/// numbers truncate to their sign, anything else fails to
/// `Number`.
fn afunction_order(
    r: Value,
    rev: impl FnOnce() -> Result<Value, NativeError>,
) -> Result<Ordering, NativeError> {
    match r {
        Value::Bool(true) => Ok(Ordering::Less),
        Value::Bool(false) => Ok(if rev()?.is_truthy() {
            Ordering::Greater
        } else {
            Ordering::Equal
        }),
        Value::Int(n) => Ok((n as i32).cmp(&0)),
        Value::Double(d) => {
            let t = d.trunc();
            Ok(if t > 0.0 {
                Ordering::Greater
            } else if t < 0.0 {
                Ordering::Less
            } else {
                Ordering::Equal
            })
        }
        _ => Err(cce_err(&r, "java.lang.Number")),
    }
}

/// A fallible `sort_by` (first failure aborts the order).
fn sort_by_checked(
    items: &mut [Value],
    mut cmp: impl FnMut(&Value, &Value) -> Result<Ordering, NativeError>,
) -> Result<(), NativeError> {
    let mut err: Option<NativeError> = None;
    items.sort_by(|a, b| {
        if err.is_some() {
            return Ordering::Equal;
        }
        match cmp(a, b) {
            Ok(o) => o,
            Err(e) => {
                err = Some(e);
                Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(())
}

fn native_sort(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/sort", args, &[1, 2])?;
    let (comp, coll) = if args.len() == 1 {
        (None, &args[0])
    } else {
        (Some(args[0].clone()), &args[1])
    };
    let mut items = collect_all(eval, coll).map_err(signal_to_native)?;
    match comp {
        None => sort_by_checked(&mut items, jvm_compare)?,
        Some(comp) => {
            sort_by_checked(&mut items, |a, b| {
                let r = call_checked(eval, &comp, &[a.clone(), b.clone()])?;
                afunction_order(r, || call_checked(eval, &comp, &[b.clone(), a.clone()]))
            })?;
        }
    }
    // `ArraySeq` (counted, never `IDrop`, no metadata);
    // empty/nil inputs answer `()`.
    if items.is_empty() {
        return Ok(empty_list_value());
    }
    Ok(realized_value(
        items,
        false,
        true,
        false,
        clj_value::SeqKind::Array,
    ))
}

fn native_sort_by(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/sort-by", args, &[2, 3])?;
    let keyfn = args[0].clone();
    let (comp, coll) = if args.len() == 2 {
        (deref_core(eval, "compare"), &args[1])
    } else {
        (args[1].clone(), &args[2])
    };
    let mut items = collect_all(eval, coll).map_err(signal_to_native)?;
    sort_by_checked(&mut items, |a, b| {
        // `(comp (keyfn x) (keyfn y))` directly (the keyfn runs per
        // comparison; no int mapping).
        let ka = call_checked(eval, &keyfn, std::slice::from_ref(a))?;
        let kb = call_checked(eval, &keyfn, std::slice::from_ref(b))?;
        let r = call_checked(eval, &comp, &[ka.clone(), kb.clone()])?;
        afunction_order(r, || call_checked(eval, &comp, &[kb, ka]))
    })?;
    if items.is_empty() {
        return Ok(empty_list_value());
    }
    Ok(realized_value(
        items,
        false,
        true,
        false,
        clj_value::SeqKind::Array,
    ))
}

fn native_dorun(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/dorun", args, &[1, 2])?;
    if args.len() == 1 {
        walk_seq(eval, &args[0], |_, _| Ok(Walk::Continue))?;
        return Ok(Value::Nil);
    }
    let n = take_count(&args[0])?;
    force_n(eval, &args[1], n)?;
    Ok(Value::Nil)
}

fn native_doall(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/doall", args, &[1, 2])?;
    if args.len() == 1 {
        walk_seq(eval, &args[0], |_, _| Ok(Walk::Continue))?;
        return Ok(args[0].clone());
    }
    let n = take_count(&args[0])?;
    force_n(eval, &args[1], n)?;
    Ok(args[1].clone())
}

fn native_mapv(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/mapv", args, 2)?;
    let mapped = map_thunk(args[0].clone(), args[1..].to_vec());
    let items = collect_all(eval, &mapped).map_err(signal_to_native)?;
    Ok(Value::Vector(Vector::from_vec(items)))
}

fn native_filterv(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/filterv", args, &[2])?;
    let filtered = filter_thunk(args[0].clone(), args[1].clone(), false);
    let items = collect_all(eval, &filtered).map_err(signal_to_native)?;
    Ok(Value::Vector(Vector::from_vec(items)))
}

fn native_group_by(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/group-by", args, &[2])?;
    let f = args[0].clone();
    let mut groups = new_map();
    walk_seq(eval, &args[1], |eval, head| {
        let key = call_checked(eval, &f, std::slice::from_ref(head))?;
        let group = match groups.get(&key) {
            Some(Value::Vector(v)) => v.clone(),
            _ => Vector::from_vec(Vec::new()),
        };
        let taken = std::mem::replace(&mut groups, new_map());
        groups = taken.insert(key, Value::Vector(group.push_back(head.clone())));
        Ok(Walk::Continue)
    })?;
    Ok(Value::Map(groups))
}

fn native_frequencies(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/frequencies", args, &[1])?;
    let mut counts = new_map();
    walk_seq(eval, &args[0], |_, head| {
        let count = match counts.get(head) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let taken = std::mem::replace(&mut counts, new_map());
        counts = taken.insert(head.clone(), Value::Int(count + 1));
        Ok(Walk::Continue)
    })?;
    Ok(Value::Map(counts))
}

fn native_every_pred(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/every-pred", args, 1)?;
    let preds = args.to_vec();
    Ok(native_fn(
        eval,
        "clojure.core/every-pred",
        move |eval, args| {
            if args.is_empty() {
                return Ok(Value::Bool(true));
            }
            let fixed = args.len().min(3);
            for p in &preds {
                for a in &args[..fixed] {
                    if !call_checked(eval, p, std::slice::from_ref(a))?.is_truthy() {
                        return Ok(Value::Bool(false));
                    }
                }
            }
            if preds.len() <= 3 {
                for a in &args[fixed..] {
                    for p in &preds {
                        if !call_checked(eval, p, std::slice::from_ref(a))?.is_truthy() {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
            } else {
                for p in &preds {
                    for a in &args[fixed..] {
                        if !call_checked(eval, p, std::slice::from_ref(a))?.is_truthy() {
                            return Ok(Value::Bool(false));
                        }
                    }
                }
            }
            Ok(Value::Bool(true))
        },
    ))
}

fn native_some_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/some-fn", args, 1)?;
    let preds = args.to_vec();
    Ok(native_fn(
        eval,
        "clojure.core/some-fn",
        move |eval, args| {
            if args.is_empty() {
                return Ok(Value::Nil);
            }
            let fixed = args.len().min(3);
            for p in &preds {
                for a in &args[..fixed] {
                    let r = call_checked(eval, p, std::slice::from_ref(a))?;
                    if r.is_truthy() {
                        return Ok(r);
                    }
                }
            }
            if preds.len() <= 3 {
                for a in &args[fixed..] {
                    for p in &preds {
                        let r = call_checked(eval, p, std::slice::from_ref(a))?;
                        if r.is_truthy() {
                            return Ok(r);
                        }
                    }
                }
            } else {
                for p in &preds {
                    for a in &args[fixed..] {
                        let r = call_checked(eval, p, std::slice::from_ref(a))?;
                        if r.is_truthy() {
                            return Ok(r);
                        }
                    }
                }
            }
            Ok(Value::Nil)
        },
    ))
}

fn native_max_key(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/max-key", args, 2)?;
    let k = args[0].clone();
    let mut best = args[1].clone();
    let mut kbest = call_checked(eval, &k, std::slice::from_ref(&best))?;
    for cand in &args[2..] {
        let kcand = call_checked(eval, &k, std::slice::from_ref(cand))?;
        let ge = deref_core(eval, ">=");
        if call_checked(eval, &ge, &[kcand.clone(), kbest.clone()])?.is_truthy() {
            best = cand.clone();
            kbest = kcand;
        }
    }
    Ok(best)
}

fn native_min_key(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/min-key", args, 2)?;
    let k = args[0].clone();
    let mut best = args[1].clone();
    let mut kbest = call_checked(eval, &k, std::slice::from_ref(&best))?;
    for cand in &args[2..] {
        let kcand = call_checked(eval, &k, std::slice::from_ref(cand))?;
        let le = deref_core(eval, "<=");
        if call_checked(eval, &le, &[kcand.clone(), kbest.clone()])?.is_truthy() {
            best = cand.clone();
            kbest = kcand;
        }
    }
    Ok(best)
}

fn native_comp(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Ok(deref_core(eval, "identity"));
    }
    if args.len() == 1 {
        return Ok(args[0].clone());
    }
    let fns = args.to_vec();
    Ok(native_fn(eval, "clojure.core/comp", move |eval, args| {
        let mut val = call_checked(eval, &fns[fns.len() - 1], args)?;
        for f in fns[..fns.len() - 1].iter().rev() {
            val = call_checked(eval, f, &[val])?;
        }
        Ok(val)
    }))
}

fn native_partial(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/partial", args, 1)?;
    if args.len() == 1 {
        return Ok(args[0].clone());
    }
    let f = args[0].clone();
    let bound = args[1..].to_vec();
    Ok(native_fn(
        eval,
        "clojure.core/partial",
        move |eval, args| {
            let mut full = bound.clone();
            full.extend(args.iter().cloned());
            call_checked(eval, &f, &full)
        },
    ))
}

fn native_complement(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/complement", args, &[1])?;
    let f = args[0].clone();
    Ok(native_fn(
        eval,
        "clojure.core/complement",
        move |eval, args| Ok(Value::Bool(!call_checked(eval, &f, args)?.is_truthy())),
    ))
}

fn native_juxt(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/juxt", args, 1)?;
    let fns = args.to_vec();
    Ok(native_fn(eval, "clojure.core/juxt", move |eval, args| {
        let mut out = Vec::with_capacity(fns.len());
        for f in &fns {
            out.push(call_checked(eval, f, args)?);
        }
        Ok(Value::Vector(Vector::from_vec(out)))
    }))
}

fn native_constantly(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/constantly", args, &[1])?;
    let x = args[0].clone();
    Ok(native_fn(eval, "clojure.core/constantly", move |_, _| {
        Ok(x.clone())
    }))
}

fn native_identity(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/identity", args, &[1])?;
    Ok(args[0].clone())
}

fn native_fnil(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/fnil", args, &[2, 3, 4])?;
    let f = args[0].clone();
    let defaults = args[1..].to_vec();
    let min = if defaults.len() >= 2 { 2 } else { 1 };
    Ok(native_fn(eval, "clojure.core/fnil", move |eval, args| {
        if args.len() < min {
            return Err(NativeError::arity("clojure.core/fnil", args.len()));
        }
        let filled: Vec<Value> = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                if matches!(a, Value::Nil) {
                    defaults.get(i).cloned().unwrap_or(Value::Nil)
                } else {
                    a.clone()
                }
            })
            .collect();
        call_checked(eval, &f, &filled)
    }))
}

fn native_every_p(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/every?", args, &[2])?;
    let pred = args[0].clone();
    let mut all = true;
    walk_seq(eval, &args[1], |eval, head| {
        if !call_checked(eval, &pred, std::slice::from_ref(head))?.is_truthy() {
            all = false;
            return Ok(Walk::Stop);
        }
        Ok(Walk::Continue)
    })?;
    Ok(Value::Bool(all))
}

fn native_some(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/some", args, &[2])?;
    let pred = args[0].clone();
    let mut found = Value::Nil;
    walk_seq(eval, &args[1], |eval, head| {
        let r = call_checked(eval, &pred, std::slice::from_ref(head))?;
        if r.is_truthy() {
            found = r;
            return Ok(Walk::Stop);
        }
        Ok(Walk::Continue)
    })?;
    Ok(found)
}

fn native_not_any_p(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/not-any?", args, &[2])?;
    let pred = args[0].clone();
    let mut none = true;
    walk_seq(eval, &args[1], |eval, head| {
        if call_checked(eval, &pred, std::slice::from_ref(head))?.is_truthy() {
            none = false;
            return Ok(Walk::Stop);
        }
        Ok(Walk::Continue)
    })?;
    Ok(Value::Bool(none))
}

fn native_not_every_p(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/not-every?", args, &[2])?;
    let pred = args[0].clone();
    let mut all = true;
    walk_seq(eval, &args[1], |eval, head| {
        if !call_checked(eval, &pred, std::slice::from_ref(head))?.is_truthy() {
            all = false;
            return Ok(Walk::Stop);
        }
        Ok(Walk::Continue)
    })?;
    Ok(Value::Bool(!all))
}

/// The S5D seq + higher-order natives installed into
/// `clojure.core` by [`boot`](crate::ns::boot) (`concat` here
/// shadows the eager S5C install with the lazy seq).
pub(crate) const SEQS_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("map", native_map),
    ("filter", native_filter),
    ("remove", native_remove),
    ("take", native_take),
    ("drop", native_drop),
    ("nthrest", native_nthrest),
    ("take-while", native_take_while),
    ("drop-while", native_drop_while),
    ("take-last", native_take_last),
    ("drop-last", native_drop_last),
    ("butlast", native_butlast),
    ("split-at", native_split_at),
    ("split-with", native_split_with),
    ("concat", native_concat),
    ("cycle", native_cycle),
    ("interleave", native_interleave),
    ("interpose", native_interpose),
    ("repeat", native_repeat),
    ("repeatedly", native_repeatedly),
    ("iterate", native_iterate),
    ("range", native_range),
    ("mapcat", native_mapcat),
    ("keep", native_keep),
    ("keep-indexed", native_keep_indexed),
    ("map-indexed", native_map_indexed),
    ("distinct", native_distinct),
    ("dedupe", native_dedupe),
    ("partition", native_partition),
    ("partition-all", native_partition_all),
    ("partition-by", native_partition_by),
    ("take-nth", native_take_nth),
    ("reductions", native_reductions),
    ("keys", native_keys),
    ("vals", native_vals),
    ("reduce", native_reduce),
    ("reverse", native_reverse),
    ("sort", native_sort),
    ("sort-by", native_sort_by),
    ("dorun", native_dorun),
    ("doall", native_doall),
    ("mapv", native_mapv),
    ("filterv", native_filterv),
    ("group-by", native_group_by),
    ("frequencies", native_frequencies),
    ("every-pred", native_every_pred),
    ("some-fn", native_some_fn),
    ("max-key", native_max_key),
    ("min-key", native_min_key),
    ("comp", native_comp),
    ("partial", native_partial),
    ("complement", native_complement),
    ("juxt", native_juxt),
    ("constantly", native_constantly),
    ("identity", native_identity),
    ("fnil", native_fnil),
    ("every?", native_every_p),
    ("some", native_some),
    ("not-any?", native_not_any_p),
    ("not-every?", native_not_every_p),
];
