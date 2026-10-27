// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Stage-3 multimethods (S-typechain, issue #33): `defmulti` values
//! plus `defmethod` / `prefer-method` / `get-method` / `methods` /
//! `remove-method`.
//!
//! A multifn is an ordinary immutable value
//! ([`MultiFn`]: name, dispatch fn, default, method table,
//! preference table). Table updates install a new root in the
//! multifn's var under the port's var-alter lock (read-modify-install
//! serializes concurrent updates the way `alter-var-root` does), so
//! the mutating natives take a var (or a quoted symbol resolving to
//! one — anything else is the `ClassCastException`, oracle
//! `e-prefer-nonmulti`); the query natives and invocation take
//! values. Dispatch is exact-value plus the default entry (oracle
//! `a-default`, `b-get-miss`); hierarchy derivation is fenced, so
//! preferences are stored but never consulted.

use std::sync::Arc;

use clj_value::{Map, MultiFn, Symbol, ThrowClass, Value, method_for, new_map, new_set};

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity};

pub const MULTI_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("__multifn*", native_multifn),
    ("__defmethod*", native_defmethod),
    ("__remove_method*", native_remove_method),
    ("__prefer_method*", native_prefer),
    ("get-method", native_get_method),
    ("methods", native_methods),
];

/// The JVM multifn class (cast target for non-multifn values).
const MULTIFN_CLASS: &str = "clojure.lang.MultiFn";

/// Expect a multifn value (anything else is the oracle's
/// `ClassCastException`, `e-prefer-nonmulti`).
fn multifn_arg(v: &Value) -> Result<Arc<MultiFn>, NativeError> {
    match v {
        Value::MultiFn(m) => Ok(m.clone()),
        _ => Err(cce_err(v, MULTIFN_CLASS)),
    }
}

/// A multifn reference: the var id (for locked updates) plus the
/// current value. Macros emit a var (or quote a non-symbol through
/// to the cast error); a quoted symbol resolves at call time.
fn multi_ref(eval: &mut Evaluator, v: &Value) -> Result<(u64, Symbol, Arc<MultiFn>), NativeError> {
    match v {
        Value::Var(var) => {
            let root = eval.deref_var(var.id, &var.sym);
            match root {
                Value::MultiFn(m) => Ok((var.id, var.sym.clone(), m)),
                _ => Err(cce_err(&root, MULTIFN_CLASS)),
            }
        }
        Value::Symbol(sym) => {
            let cur = eval.current_ns_name();
            match eval.registry.lookup_in(&cur, sym) {
                Some(id) => {
                    let root = eval.deref_var(id, sym);
                    match root {
                        Value::MultiFn(m) => Ok((id, sym.clone(), m)),
                        _ => Err(cce_err(&root, MULTIFN_CLASS)),
                    }
                }
                None => Err(unresolvable(sym)),
            }
        }
        _ => Err(cce_err(v, MULTIFN_CLASS)),
    }
}

/// The `Unable to resolve symbol` runtime error (the analyzer's
/// text, without the compile wrapper).
fn unresolvable(sym: &Symbol) -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        format!("Unable to resolve symbol: {sym} in this context"),
    )
}

/// Build a multifn value (split for budget): args are
/// `[name-str dispatch-fn default]`; both tables start empty.
fn native_multifn(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__multifn*", args, &[3])?;
    let name = match &args[0] {
        Value::Str(s) => s.clone(),
        _ => return Err(cce_err(&args[0], "java.lang.String")),
    };
    Ok(Value::MultiFn(Arc::new(MultiFn {
        name,
        dispatch: args[1].clone(),
        default: args[2].clone(),
        methods: new_map(),
        prefers: new_map(),
    })))
}

/// One table update (split from the natives so the locked
/// install stays a plain call — and unit-testable without a
/// closure no coverage run could execute).
enum TableUpdate {
    Method(Value, Value),
    Remove(Value),
    Prefer(Value, Value),
}

/// Install an updated multifn under the var-alter lock (split for
/// budget): the root is re-read after acquiring, updated purely,
/// and installed only when still a multifn — concurrent disjoint
/// updates serialize with no lost update, like `alter-var-root`.
fn install_updated(
    eval: &mut Evaluator,
    id: u64,
    sym: &Symbol,
    update: TableUpdate,
) -> Result<Arc<MultiFn>, NativeError> {
    let _guard = eval.registry.lock_var_for_alter(id);
    let current = eval.registry.root_or_unbound(id, sym);
    let mf = multifn_arg(&current)?;
    let mut next = (*mf).clone();
    match update {
        TableUpdate::Method(k, f) => {
            next.methods = next.methods.clone().insert(k, f);
        }
        TableUpdate::Remove(k) => {
            next.methods = next.methods.clone().without(&k);
        }
        TableUpdate::Prefer(pref, other) => {
            next.prefers = prefer_insert(&next.prefers, &pref, &other);
        }
    }
    let next = Arc::new(next);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(Value::MultiFn(next.clone()));
    });
    Ok(next)
}

/// Add (or replace) one method entry (split for budget): args are
/// `[multifn-ref dispatch-val fn]`; answers the updated multifn.
fn native_defmethod(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__defmethod*", args, &[3])?;
    let (id, sym, _) = multi_ref(eval, &args[0])?;
    let update = TableUpdate::Method(args[1].clone(), args[2].clone());
    let next = install_updated(eval, id, &sym, update)?;
    Ok(Value::MultiFn(next))
}

/// Drop one method entry (split for budget): args are
/// `[multifn-ref dispatch-val]`; a missing entry is a no-op
/// (oracle `c-remove-miss`); answers the updated multifn.
fn native_remove_method(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__remove_method*", args, &[2])?;
    let (id, sym, _) = multi_ref(eval, &args[0])?;
    let next = install_updated(eval, id, &sym, TableUpdate::Remove(args[1].clone()))?;
    Ok(Value::MultiFn(next))
}

/// Record a preference (split for budget): args are
/// `[multifn-ref preferred other]`; the entry joins the preferred
/// value's dispreferred set (reserved for the hierarchy
/// follow-up); answers the updated multifn.
fn native_prefer(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__prefer_method*", args, &[3])?;
    let (id, sym, _) = multi_ref(eval, &args[0])?;
    let update = TableUpdate::Prefer(args[1].clone(), args[2].clone());
    let next = install_updated(eval, id, &sym, update)?;
    Ok(Value::MultiFn(next))
}

/// Insert one preference edge (split from [`native_prefer`] for the
/// complexity budget): the dispreferred value joins the preferred
/// value's set (created on demand).
fn prefer_insert(prefers: &Map, preferred: &Value, other: &Value) -> Map {
    let mut set = match prefers.get(preferred) {
        Some(Value::Set(s)) => s.clone(),
        _ => new_set(),
    };
    set = set.insert(other.clone());
    prefers.clone().insert(preferred.clone(), Value::Set(set))
}

/// Fetch one method fn (split for budget): the exact entry, else
/// the default entry (oracle `b-get-miss`), else `nil`.
fn native_get_method(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/get-method", args, &[2])?;
    let mf = multifn_arg(&args[0])?;
    Ok(method_for(&mf, &args[1]).unwrap_or(Value::Nil))
}

/// The whole method table (split for budget): dispatch value → fn,
/// `:default` entry included (oracle `b-methods`,
/// `b-default-entry`).
fn native_methods(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/methods", args, &[1])?;
    let mf = multifn_arg(&args[0])?;
    Ok(Value::Map(mf.methods.clone()))
}

/// Invoke a multifn value (split for budget): the dispatch fn runs
/// on the call args, the winning method runs on the same args; a
/// miss is the oracle's `No method` error (naming the multifn and
/// the dispatch value's print form, `e-nomethod` / `c-remove`).
pub(crate) fn invoke_multifn(
    eval: &mut Evaluator,
    mf: &Arc<MultiFn>,
    args: &[Value],
) -> Result<Value, crate::error::Signal> {
    let dispatch_val = eval.call(&mf.dispatch, args)?;
    match method_for(mf, &dispatch_val) {
        Some(func) => eval.call(&func, args),
        None => Err(eval.throw(no_method(mf, &dispatch_val))),
    }
}

/// The oracle's missing-method error.
fn no_method(mf: &MultiFn, dispatch_val: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!(
            "No method in multimethod '{}' for dispatch value: {}",
            mf.name,
            clj_value::pr_to_string(dispatch_val, &clj_value::PrintOpts::pr())
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_updated_rejects_non_multifn_root() {
        // The under-lock re-validation: a var whose root is not a
        // multifn errors instead of installing (covers the race
        // arm without threads — mirrors `install_impls`).
        let mut eval = crate::boot();
        eval.eval_source("(def P 5)").unwrap();
        let sym = Symbol::new(None, "P");
        let cur = eval.current_ns_name();
        let q = eval.registry.resolve_in(&cur, &sym).unwrap();
        let id = eval.registry.lookup(&q).unwrap();
        let err = install_updated(
            &mut eval,
            id,
            &sym,
            TableUpdate::Method(Value::Int(1), Value::Int(2)),
        )
        .unwrap_err();
        assert_eq!(err.class, ThrowClass::ClassCastException);
    }
}
