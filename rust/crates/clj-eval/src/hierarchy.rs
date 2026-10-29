// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Hierarchies (zero-oos S13, issue #57): `make-hierarchy` /
//! `derive` / `underive` / `isa?` / `parents` / `ancestors` /
//! `descendants` / `prefers`.
//!
//! The pure map engine lives in `clj-value` (`isa_in`,
//! `derive_in`, …); this module adds the evaluator surface:
//! arity guards, the private `#'clojure.core/global-hierarchy`
//! var behind the short arities (altered under the var-alter
//! lock, read fresh like any root), and error rendering
//! (casts take the house loader text).

use clj_value::{
    HierError, HierMessage, MultiFn, Set, Symbol, ThrowClass, Value, ancestors_in,
    assert_global_edge, derive_in, descendants_in, isa_in, make_hierarchy_map, parents_in,
    underive_in,
};

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::multi::multifn_arg;
use crate::ns::{NativeFnPtr, check_arity, core_var};

pub const HIERARCHY_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("make-hierarchy", native_make_hierarchy),
    ("derive", native_derive),
    ("underive", native_underive),
    ("isa?", native_isa),
    ("parents", native_parents),
    ("ancestors", native_ancestors),
    ("descendants", native_descendants),
    ("prefers", native_prefers),
];

/// Render a hierarchy failure (casts take the house loader
/// text; staged for `multi.rs` too).
pub(crate) fn hier_native_error(e: HierError) -> NativeError {
    match e.message {
        HierMessage::Text(m) => NativeError::new(e.class, m),
        HierMessage::Cast { from, to } => cce_err(&from, to),
    }
}

/// The private global-hierarchy var id (`None` when a user
/// unmapped it — the short arities then fail resolution).
fn global_id(eval: &Evaluator) -> Option<u64> {
    core_var(eval, "global-hierarchy")
}

/// The missing-var error (the house unresolvable-var text).
fn unresolvable_global() -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        "Unable to resolve var: global-hierarchy in this context".to_owned(),
    )
}

/// Read the global hierarchy root (fresh, like any deref;
/// staged for `multi.rs` dispatch).
pub(crate) fn read_global(eval: &Evaluator) -> Result<Value, NativeError> {
    let Some(id) = global_id(eval) else {
        return Err(unresolvable_global());
    };
    Ok(eval.deref_var(id, &Symbol::new(Some("clojure.core"), "global-hierarchy")))
}

/// Alter the global hierarchy under the var-alter lock
/// (global `derive` / `underive`): the root is re-read
/// after acquiring and the new map installed; answers nil.
fn alter_global(
    eval: &mut Evaluator,
    next: impl FnOnce(&Value) -> Result<clj_value::Map, HierError>,
) -> Result<Value, NativeError> {
    let Some(id) = global_id(eval) else {
        return Err(unresolvable_global());
    };
    let sym = Symbol::new(Some("clojure.core"), "global-hierarchy");
    let _guard = eval.registry.lock_var_for_alter(id);
    let current = eval.registry.root_or_unbound(id, &sym);
    let next = next(&current).map_err(hier_native_error)?;
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(Value::Map(next));
    });
    Ok(Value::Nil)
}

/// A fresh hierarchy map (split for budget): 0-arity.
fn native_make_hierarchy(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/make-hierarchy", args, &[0])?;
    Ok(Value::Map(make_hierarchy_map()))
}

/// Establish a parent/child edge (split for budget):
/// explicit-hierarchy 3-arity answers the new map, the
/// global 2-arity asserts namespaces, alters the root,
/// and answers nil.
fn native_derive(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/derive", args, &[2, 3])?;
    if args.len() == 3 {
        return derive_in(&args[0], &args[1], &args[2])
            .map(Value::Map)
            .map_err(hier_native_error);
    }
    assert_global_edge(&args[0], &args[1]).map_err(hier_native_error)?;
    let (tag, parent) = (args[0].clone(), args[1].clone());
    alter_global(eval, |h| derive_in(h, &tag, &parent))
}

/// Drop a parent/child edge (split for budget): same arities
/// as `derive` (the global arity asserts nothing, like the
/// JVM).
fn native_underive(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/underive", args, &[2, 3])?;
    if args.len() == 3 {
        return underive_in(&args[0], &args[1], &args[2])
            .map(Value::Map)
            .map_err(hier_native_error);
    }
    let (tag, parent) = (args[0].clone(), args[1].clone());
    alter_global(eval, |h| underive_in(h, &tag, &parent))
}

/// The `isa?` question (split for budget): explicit `h`
/// 3-arity or the global 2-arity.
fn native_isa(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/isa?", args, &[2, 3])?;
    let hit = if args.len() == 3 {
        isa_in(&args[0], &args[1], &args[2])
    } else {
        let h = read_global(eval)?;
        isa_in(&h, &args[0], &args[1])
    };
    hit.map(Value::Bool).map_err(hier_native_error)
}

/// A relation view over an explicit or global hierarchy
/// (split for budget): the set, or nil when empty.
fn native_view(
    eval: &mut Evaluator,
    name: &str,
    args: &[Value],
    view: fn(&Value, &Value) -> Result<Option<Set>, HierError>,
) -> Result<Value, NativeError> {
    check_arity(name, args, &[1, 2])?;
    let found = if args.len() == 2 {
        view(&args[0], &args[1])
    } else {
        let h = read_global(eval)?;
        view(&h, &args[0])
    };
    found
        .map(|s| s.map_or(Value::Nil, Value::Set))
        .map_err(hier_native_error)
}

/// Immediate parents (split for budget).
fn native_parents(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    native_view(eval, "clojure.core/parents", args, parents_in)
}

/// Transitive parents (split for budget).
fn native_ancestors(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    native_view(eval, "clojure.core/ancestors", args, ancestors_in)
}

/// Transitive children (split for budget).
fn native_descendants(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    native_view(eval, "clojure.core/descendants", args, descendants_in)
}

/// A multifn's prefer table (split for budget):
/// preferred value → dispreferred set.
fn native_prefers(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/prefers", args, &[1])?;
    let mf: std::sync::Arc<MultiFn> = multifn_arg(&args[0])?;
    Ok(Value::Map(mf.prefers.clone()))
}
