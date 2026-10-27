// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Multimethods (S-typechain stage 3, issue #33): the `defmulti`
//! value — dispatch fn, default dispatch value, method table, and
//! preference table.
//!
//! Values are immutable snapshots: `defmethod` / `remove-method` /
//! `prefer-method` install a new root in the multifn's var under the
//! port's var-alter lock (read-modify-install, like protocols), so a
//! multifn value captured before an update keeps its old table (a
//! documented divergence — the JVM mutates the one object). Ordinary
//! calls resolve the var fresh, so only explicit captures go stale.
//! Dispatch is exact-value plus the default entry; hierarchy
//! derivation (`derive`, `isa?`) is fenced as a follow-up, so the
//! preference table is stored but never consulted (and ambiguity
//! errors are unreachable without ancestors to be ambiguous over).

use std::sync::Arc;

use crate::{Map, Value};

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
    /// Preferred dispatch value → dispreferred set (recorded for
    /// the hierarchy follow-up; dispatch ignores it).
    pub prefers: Map,
}

/// Look up the method fn for a dispatch value (split for the
/// complexity budget): the exact entry, else the default entry,
/// else no method.
#[must_use]
pub fn method_for(mf: &MultiFn, dispatch_val: &Value) -> Option<Value> {
    if let Some(f) = mf.methods.get(dispatch_val) {
        return Some(f.clone());
    }
    mf.methods.get(&mf.default).cloned()
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
