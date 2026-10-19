// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5B natives: var roots (`alter-var-root`, `var-get`, `var-set`),
//! predicates (`bound?`, `thread-bound?`), thread-binding frames
//! (`push-`/`pop-`/`get-thread-bindings`, `with-bindings*`), and
//! metadata (`meta`, `with-meta`, `vary-meta`, `alter-meta!`,
//! `reset-meta!`). Oracle: probe37; rules: `docs/t1174/vars-contract.md`.

use std::sync::Arc;

use clj_value::{Map, ThrowClass, Value, Var, new_map};

use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ext::{Closure, NativeFn, as_closure, as_native, as_ns};
use crate::ns::{NativeFnPtr, cast_err, check_arity, npe, signal_to_native, sym_str, var_value};

const VAR_CLASS: &str = "clojure.lang.Var";
const ASSOC_CLASS: &str = "clojure.lang.Associative";
const IOBJ_CLASS: &str = "clojure.lang.IObj";
const IMAP_CLASS: &str = "clojure.lang.IPersistentMap";
const IREF_CLASS: &str = "clojure.lang.IReference";
const IENTRY_CLASS: &str = "clojure.lang.IMapEntry";

/// Reading `.seq()` off a nil bindings map (probe37 `push-nil`).
const NIL_SEQ_NPE: &str =
    "Cannot invoke \"clojure.lang.Associative.seq()\" because \"bindings\" is null";

/// Calling `.withMeta` off nil (probe37 `wm-nil-obj`).
const NIL_WITHMETA_NPE: &str = "Cannot invoke \"clojure.lang.IObj.withMeta(clojure.lang.IPersistentMap)\" because \"x\" is null";

/// A strict var argument (every S5B var-taking native casts here).
fn as_var(v: &Value) -> Result<&Var, NativeError> {
    match v {
        Value::Var(v) => Ok(v),
        _ => Err(cast_err(v, VAR_CLASS)),
    }
}

/// Arity with a minimum (`alter-var-root`, `with-bindings*`,
/// `vary-meta`, `alter-meta!` take trailing args).
pub(crate) fn check_min_arity(
    display: &str,
    args: &[Value],
    min: usize,
) -> Result<(), NativeError> {
    if args.len() >= min {
        Ok(())
    } else {
        Err(NativeError::arity(display, args.len()))
    }
}

/// Whether a var is bound: a non-`Unbound` root or a thread entry
/// (`Var.isBound`).
fn is_bound(eval: &Evaluator, id: u64) -> bool {
    eval.registry.has_root(id) || eval.is_thread_bound(id)
}

fn native_alter_var_root(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/alter-var-root", args, 2)?;
    let var = as_var(&args[0])?.clone();
    let current = eval.registry.root_or_unbound(var.id, &var.sym);
    let mut f_args = Vec::with_capacity(args.len() - 1);
    f_args.push(current);
    f_args.extend(args[2..].iter().cloned());
    // A throwing `f` leaves the root untouched (the install runs
    // only on success, probe37 `avr-fail-keeps`).
    let next = eval.call(&args[1], &f_args).map_err(signal_to_native)?;
    if let Some(st) = eval.registry.state_mut(var.id) {
        st.root = Some(next.clone());
    }
    Ok(next)
}

fn native_bound(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    // `every?` semantics: true on empty, short-circuit on false,
    // each arg cast in order (probe37 `bound-*`).
    for arg in args {
        let var = as_var(arg)?;
        if !is_bound(eval, var.id) {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn native_thread_bound(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    for arg in args {
        let var = as_var(arg)?;
        if !eval.is_thread_bound(var.id) {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn native_var_get(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/var-get", args, &[1])?;
    let var = as_var(&args[0])?;
    Ok(eval.deref_var(var.id, &var.sym))
}

fn native_var_set(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/var-set", args, &[2])?;
    let var = as_var(&args[0])?.clone();
    if eval.set_binding_top(var.id, args[1].clone()) {
        Ok(args[1].clone())
    } else {
        Err(NativeError::new(
            ThrowClass::IllegalStateException,
            format!(
                "Can't change/establish root binding of: {} with set",
                var.sym.name()
            ),
        ))
    }
}

/// Validate a `push-thread-bindings`/`with-bindings*` map into
/// frame entries (the whole map validates before anything pushes,
/// probe37 `push-atomic`). Vectors are `Associative`s of map
/// entries: empty pushes nothing, non-empty fails the first
/// element's `IMapEntry` cast (JVM `Frame` ctor).
pub(crate) fn push_entries(
    eval: &Evaluator,
    bindings: &Value,
) -> Result<Vec<(Var, Value)>, NativeError> {
    match bindings {
        Value::Nil => Err(npe(NIL_SEQ_NPE)),
        Value::Map(m) => {
            let mut entries = Vec::with_capacity(m.size());
            for (k, v) in m.iter() {
                let var = as_var(k)?.clone();
                let dynamic = eval.registry.state(var.id).is_some_and(|s| s.dynamic);
                if !dynamic {
                    return Err(NativeError::new(
                        ThrowClass::IllegalStateException,
                        format!(
                            "Can't dynamically bind non-dynamic var: {}",
                            sym_str(&var.sym)
                        ),
                    ));
                }
                entries.push((var, v.clone()));
            }
            Ok(entries)
        }
        Value::Vector(x) => match x.iter().next() {
            None => Ok(Vec::new()),
            Some(first) => Err(cast_err(first, IENTRY_CLASS)),
        },
        _ => Err(cast_err(bindings, ASSOC_CLASS)),
    }
}

fn native_push_thread_bindings(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/push-thread-bindings", args, &[1])?;
    let entries = push_entries(eval, &args[0])?;
    eval.push_frame(entries);
    Ok(Value::Nil)
}

fn native_pop_thread_bindings(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/pop-thread-bindings", args, &[0])?;
    match eval.pop_frame() {
        Some(_) => Ok(Value::Nil),
        None => Err(NativeError::new(
            ThrowClass::IllegalStateException,
            "Pop without matching push".to_owned(),
        )),
    }
}

fn native_get_thread_bindings(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/get-thread-bindings", args, &[0])?;
    // Oldest-first fold: the topmost entry wins per var (a fresh
    // map per call, probe37 `gtb-snapshot`).
    let mut out = new_map();
    for (var, value) in eval.binding_pairs() {
        out = out.insert(var_value(var.id, &var.sym), value.clone());
    }
    Ok(Value::Map(out))
}

fn native_with_bindings_star(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/with-bindings*", args, 2)?;
    let entries = push_entries(eval, &args[0])?;
    eval.push_frame(entries);
    // A `finally` pop: the frame leaves on throw too (probe37
    // `wbs-throw`).
    let out = eval.call(&args[1], &args[2..]).map_err(signal_to_native);
    eval.pop_frame();
    out
}

/// The metadata of any value (`meta`): vars (nil once cleared),
/// namespaces (nil when fresh), symbols, collections, and
/// functions; everything else reads nil.
pub(crate) fn meta_of(eval: &Evaluator, v: &Value) -> Option<Map> {
    match v {
        Value::Var(var) => eval.registry.state(var.id)?.meta.clone(),
        Value::Symbol(s) => s.meta().cloned(),
        Value::List(l) => l.meta().cloned(),
        Value::Vector(x) => x.meta().cloned(),
        Value::Map(m) => m.meta().cloned(),
        Value::Set(s) => s.meta().cloned(),
        Value::Extension(_) => {
            if let Some(c) = as_closure(v) {
                return c.meta.clone();
            }
            if let Some(n) = as_native(v) {
                return n.meta.clone();
            }
            if let Some(ns) = as_ns(v) {
                return eval.registry.ns_live_or_removed(&ns.name)?.meta().cloned();
            }
            None
        }
        _ => None,
    }
}

fn native_meta(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/meta", args, &[1])?;
    Ok(meta_of(eval, &args[0]).map_or(Value::Nil, Value::Map))
}

/// Attach `meta` (a map or nil) to an `IObj` (the object checks
/// before the meta arg, JVM receiver first).
fn with_meta_val(obj: &Value, meta: &Value) -> Result<Value, NativeError> {
    let meta = match meta {
        Value::Nil => None,
        Value::Map(m) => Some(m.clone()),
        _ => return Err(cast_err(meta, IMAP_CLASS)),
    };
    match obj {
        Value::Nil => Err(npe(NIL_WITHMETA_NPE)),
        Value::Symbol(s) => Ok(Value::Symbol(s.clone().with_meta(meta))),
        Value::List(l) => Ok(Value::List(l.clone().with_meta(meta))),
        Value::Vector(x) => Ok(Value::Vector(x.clone().with_meta(meta))),
        Value::Map(m) => Ok(Value::Map(m.clone().with_meta(meta))),
        Value::Set(s) => Ok(Value::Set(s.clone().with_meta(meta))),
        Value::Extension(e) => {
            if let Some(c) = e.as_any().downcast_ref::<Closure>() {
                let mut next = c.clone();
                next.meta = meta;
                return Ok(Value::Extension(Arc::new(next)));
            }
            if let Some(n) = e.as_any().downcast_ref::<NativeFn>() {
                let mut next = n.clone();
                next.meta = meta;
                return Ok(Value::Extension(Arc::new(next)));
            }
            Err(cast_err(obj, IOBJ_CLASS))
        }
        _ => Err(cast_err(obj, IOBJ_CLASS)),
    }
}

fn native_with_meta(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/with-meta", args, &[2])?;
    with_meta_val(&args[0], &args[1])
}

fn native_vary_meta(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/vary-meta", args, 2)?;
    let current = meta_of(eval, &args[0]).map_or(Value::Nil, Value::Map);
    let mut f_args = Vec::with_capacity(args.len() - 1);
    f_args.push(current);
    f_args.extend(args[2..].iter().cloned());
    let next = eval.call(&args[1], &f_args).map_err(signal_to_native)?;
    with_meta_val(&args[0], &next)
}

/// An `IReference` target (vars and namespaces; atoms don't
/// exist yet).
enum RefTarget {
    Var(u64),
    Ns(String),
}

fn as_ref_target(v: &Value) -> Result<RefTarget, NativeError> {
    match v {
        Value::Var(var) => Ok(RefTarget::Var(var.id)),
        Value::Extension(_) => match as_ns(v) {
            Some(ns) => Ok(RefTarget::Ns(ns.name.to_string())),
            None => Err(cast_err(v, IREF_CLASS)),
        },
        _ => Err(cast_err(v, IREF_CLASS)),
    }
}

fn install_ref_meta(eval: &mut Evaluator, target: &RefTarget, meta: Option<Map>) {
    match target {
        RefTarget::Var(id) => {
            if let Some(st) = eval.registry.state_mut(*id) {
                st.meta = meta;
            }
        }
        RefTarget::Ns(name) => {
            if let Some(ns) = eval.registry.ns_live_or_removed_mut(name) {
                ns.set_meta(meta);
            }
        }
    }
}

fn ref_meta(eval: &Evaluator, target: &RefTarget) -> Option<Map> {
    match target {
        RefTarget::Var(id) => eval.registry.state(*id)?.meta.clone(),
        RefTarget::Ns(name) => eval.registry.ns_live_or_removed(name)?.meta().cloned(),
    }
}

/// A meta value (a map or nil) for `alter-meta!`/`reset-meta!`.
fn as_meta_value(meta: &Value) -> Result<Option<Map>, NativeError> {
    match meta {
        Value::Nil => Ok(None),
        Value::Map(m) => Ok(Some(m.clone())),
        _ => Err(cast_err(meta, IMAP_CLASS)),
    }
}

fn native_alter_meta(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/alter-meta!", args, 2)?;
    let target = as_ref_target(&args[0])?;
    let current = ref_meta(eval, &target).map_or(Value::Nil, Value::Map);
    let mut f_args = Vec::with_capacity(args.len() - 1);
    f_args.push(current);
    f_args.extend(args[2..].iter().cloned());
    // A throwing `f` changes nothing (compute, then install).
    let next = eval.call(&args[1], &f_args).map_err(signal_to_native)?;
    let meta = as_meta_value(&next)?;
    install_ref_meta(eval, &target, meta.clone());
    Ok(meta.map_or(Value::Nil, Value::Map))
}

fn native_reset_meta(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reset-meta!", args, &[2])?;
    let target = as_ref_target(&args[0])?;
    let meta = as_meta_value(&args[1])?;
    install_ref_meta(eval, &target, meta.clone());
    Ok(meta.map_or(Value::Nil, Value::Map))
}

/// The S5B natives installed into `clojure.core` by [`boot`](crate::ns::boot).
pub(crate) const VAR_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("alter-var-root", native_alter_var_root),
    ("bound?", native_bound),
    ("thread-bound?", native_thread_bound),
    ("var-get", native_var_get),
    ("var-set", native_var_set),
    ("push-thread-bindings", native_push_thread_bindings),
    ("pop-thread-bindings", native_pop_thread_bindings),
    ("get-thread-bindings", native_get_thread_bindings),
    ("with-bindings*", native_with_bindings_star),
    ("meta", native_meta),
    ("with-meta", native_with_meta),
    ("vary-meta", native_vary_meta),
    ("alter-meta!", native_alter_meta),
    ("reset-meta!", native_reset_meta),
];

#[cfg(test)]
mod tests {
    use super::*;
    use clj_value::Keyword;

    /// Natives carry meta like closures (`deref` doesn't exist
    /// until S5D, so no source form can reach a native value yet —
    /// this pins it at the Rust level instead).
    #[test]
    fn native_fn_meta_roundtrip() {
        let mut eval = Evaluator::new_bare("test");
        let f = eval.install_native(
            "user",
            "dbl",
            Arc::new(|_, args| {
                check_arity("test/dbl", args, &[0])?;
                Ok(Value::Int(7))
            }),
        );
        assert!(meta_of(&eval, &f).is_none());
        let meta = new_map().insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1));
        let wrapped = with_meta_val(&f, &Value::Map(meta)).unwrap();
        assert_eq!(meta_of(&eval, &wrapped).unwrap().size(), 1);
        // Still callable under the new meta.
        assert_eq!(eval.call(&wrapped, &[]).unwrap(), Value::Int(7));
    }
}
