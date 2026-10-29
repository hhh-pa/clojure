// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Multimethods (S-typechain stage 3, issue #33; hierarchies
//! S13, issue #57): the `defmulti` value — dispatch fn, default
//! dispatch value, method table, preference table, and
//! hierarchy source.
//!
//! Values are immutable snapshots: `defmethod` / `remove-method` /
//! `prefer-method` install a new root in the multifn's var under the
//! port's var-alter lock (read-modify-install, like protocols), so a
//! multifn value captured before an update keeps its old table (a
//! documented divergence — the JVM mutates the one object). Ordinary
//! calls resolve the var fresh, so only explicit captures go stale.
//! Dispatch consults the resolved hierarchy: exact entry, then the
//! best `isa?` match with preference edges breaking ties, then the
//! default entry — unrelated competing methods are the JVM
//! ambiguity error.

use std::sync::Arc;

use crate::{LookupFail, Map, MethodLookup, PrintOpts, Value, best_method, pr_to_string};

/// A multifn value: the `def`ined root of `defmulti`, invoked like a
/// function (dispatch, then the winning method runs on the call
/// args).
#[derive(Clone, Debug)]
pub struct MultiFn {
    /// The `defmulti` name, as written (unqualified — error texts
    /// quote it verbatim, oracle `e-nomethod`).
    pub name: Arc<str>,
    /// The dispatch fn value (any `IFn` — keywords included, oracle
    /// `a-kw`).
    pub dispatch: Value,
    /// The fallback dispatch value (`:default` unless overridden).
    pub default: Value,
    /// Dispatch value → method fn (the `:default` entry rides here
    /// like any other, oracle `b-default-entry`).
    pub methods: Map,
    /// Preferred dispatch value → dispreferred set (breaks ties
    /// between unrelated matching methods).
    pub prefers: Map,
    /// Explicit `:hierarchy` (`None` is the live global
    /// hierarchy): a var/atom/agent dereferenced at dispatch,
    /// or nil (which NPEs on dereference, oracle `x-hier-nil`).
    pub hierarchy: Option<Value>,
}

/// Look up the method fn for a dispatch value against the
/// resolved hierarchy (split for the complexity budget):
/// the exact entry, else the best `isa?` match, else the
/// default entry, else no method. Ambiguity is the JVM
/// error text (challenger first, probe `n-amb-order`).
pub fn method_for(
    mf: &MultiFn,
    dispatch_val: &Value,
    hierarchy: &Value,
) -> Result<Option<Value>, LookupFail> {
    if let Some(f) = mf.methods.get(dispatch_val) {
        return Ok(Some(f.clone()));
    }
    match best_method(&mf.methods, &mf.prefers, hierarchy, dispatch_val)
        .map_err(LookupFail::Hier)?
    {
        MethodLookup::Found(f) => Ok(Some(f)),
        MethodLookup::Missing => Ok(mf.methods.get(&mf.default).cloned()),
        MethodLookup::Ambiguous { challenger, best } => Err(LookupFail::Ambiguous(ambiguity_msg(
            &mf.name,
            dispatch_val,
            &challenger,
            &best,
        ))),
    }
}

/// The JVM ambiguity text (`DV -> challenger and best`).
fn ambiguity_msg(name: &str, dispatch_val: &Value, challenger: &Value, best: &Value) -> String {
    format!(
        "Multiple methods in multimethod '{name}' match dispatch value: {} -> {} and {}, and neither is preferred",
        pr_to_string(dispatch_val, &PrintOpts::pr()),
        pr_to_string(challenger, &PrintOpts::pr()),
        pr_to_string(best, &PrintOpts::pr()),
    )
}

/// Deterministic identity digits for the `#object[…]` and `Name@H`
/// slots (the JVM prints the identity hash — same shape, stable
/// digits here, derived from the name).
#[must_use]
pub fn multifn_hex(name: &str) -> String {
    format!("{:x}", crate::hasheq(&Value::Str(Arc::from(name))) as u32)
}

/// The `str`/`toString` form (no `toString` methods on multifns —
/// always the `clojure.lang.MultiFn@hex` fallback).
#[must_use]
pub fn multifn_str(name: &str) -> String {
    format!("clojure.lang.MultiFn@{}", multifn_hex(name))
}
