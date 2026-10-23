// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Callable extension values: [`Closure`] (interpreted `fn`) and
//! [`NativeFn`] (Rust callables: S5 core fns plus test doubles).
//! Both print `#object[…]` with the munged display name and a
//! per-evaluator sequence id as the hex (deterministic for a fixed
//! program, §E10).

use std::any::Any;
use std::hash::Hasher;
use std::sync::Arc;

use clj_value::{ExtensionValue, Map, Value};

use crate::env::{Bindings, Env};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::expr::Arity;

/// `Compiler.munge` for display names (`-` → `_`, `/` → `$`,
/// punctuation → `_NAME_`). Common names are exact; exotic ones
/// follow the same table.
#[must_use]
pub fn munge(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for c in display.chars() {
        munge_char(c, &mut out);
    }
    out
}

/// First half of the `Compiler.munge` char table.
fn munge_char(c: char, out: &mut String) {
    match c {
        '-' => out.push('_'),
        '/' => out.push('$'),
        ':' => out.push_str("_COLON_"),
        '+' => out.push_str("_PLUS_"),
        '>' => out.push_str("_GT_"),
        '<' => out.push_str("_LT_"),
        '=' => out.push_str("_EQ_"),
        '~' => out.push_str("_TILDE_"),
        '!' => out.push_str("_BANG_"),
        '@' => out.push_str("_CIRCA_"),
        '#' => out.push_str("_SHARP_"),
        '\'' => out.push_str("_SINGLEQUOTE_"),
        other => munge_char_rest(other, out),
    }
}

/// Rest of the `Compiler.munge` char table. Unlisted chars pass through.
fn munge_char_rest(c: char, out: &mut String) {
    match c {
        '"' => out.push_str("_DOUBLEQUOTE_"),
        '%' => out.push_str("_PERCENT_"),
        '^' => out.push_str("_CARET_"),
        '&' => out.push_str("_AMPERSAND_"),
        '*' => out.push_str("_STAR_"),
        '|' => out.push_str("_BAR_"),
        '{' => out.push_str("_LBRACE_"),
        '}' => out.push_str("_RBRACE_"),
        '[' => out.push_str("_LBRACK_"),
        ']' => out.push_str("_RBRACK_"),
        '?' => out.push_str("_QMARK_"),
        _ => out.push(c),
    }
}

/// An interpreted function value: display name, arities, captured
/// frame, optional metadata (`with-meta`), and optional conveyed
/// thread bindings (`bound-fn` snapshots these at creation and
/// re-pushes them around every invocation). Identity is the
/// sequence `id`.
#[derive(Clone, Debug)]
pub struct Closure {
    pub id: u64,
    pub display: Arc<str>,
    pub arities: Vec<Arity>,
    pub env: Env,
    pub meta: Option<Map>,
    pub conveyed: Option<Bindings>,
}

impl ExtensionValue for Closure {
    fn class_name(&self) -> String {
        munge(&self.display)
    }

    fn print(&self) -> String {
        let munged = munge(&self.display);
        format!(
            "#object[{munged} 0x{:x} \"{munged}@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Closure>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A Rust callable: `display` names arity errors, `func` runs the
/// call with the live evaluator (natives can intern, resolve, and
/// eval §N vars). Failures are [`NativeError`]s (the evaluator
/// stamps the identity when throwing).
pub type NativeImpl = dyn Fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError> + Send + Sync;

#[derive(Clone)]
pub struct NativeFn {
    pub id: u64,
    pub display: Arc<str>,
    pub func: Arc<NativeImpl>,
    pub meta: Option<Map>,
}

impl std::fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeFn")
            .field("id", &self.id)
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

impl ExtensionValue for NativeFn {
    fn class_name(&self) -> String {
        munge(&self.display)
    }

    fn print(&self) -> String {
        let munged = munge(&self.display);
        format!(
            "#object[{munged} 0x{:x} \"{munged}@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<NativeFn>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Downcast a value to a closure, if it is one.
#[must_use]
pub fn as_closure(v: &Value) -> Option<&Closure> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<Closure>(),
        _ => None,
    }
}

/// Downcast a value to a native fn, if it is one.
#[must_use]
pub fn as_native(v: &Value) -> Option<&NativeFn> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<NativeFn>(),
        _ => None,
    }
}

/// A namespace handle: `name` plus the registry creation `id`.
/// Equality and hashing use the `id` only, like JVM identity — a
/// removed-then-recreated namespace is a new value.
#[derive(Clone, Debug)]
pub struct NsHandle {
    pub name: Arc<str>,
    pub id: u64,
}

impl ExtensionValue for NsHandle {
    fn class_name(&self) -> String {
        "clojure.lang.Namespace".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[clojure.lang.Namespace 0x{:x} \"{}\"]",
            self.id, self.name
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<NsHandle>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Downcast a value to a namespace handle, if it is one.
#[must_use]
pub fn as_ns(v: &Value) -> Option<&NsHandle> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<NsHandle>(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `CHAR_MAP` table, oracle-pinned (every arm matches
    /// `Compiler/munge`; dots pass through like the JVM; `/` is the
    /// display's namespace separator by design).
    #[test]
    fn munge_matches_compiler_table() {
        assert_eq!(
            munge(":+><=~!@#\"'%^&*|{}[]?"),
            "_COLON__PLUS__GT__LT__EQ__TILDE__BANG__CIRCA__SHARP__DOUBLEQUOTE__SINGLEQUOTE__PERCENT__CARET__AMPERSAND__STAR__BAR__LBRACE__RBRACE__LBRACK__RBRACK__QMARK_"
        );
        // Dots pass through (namespace separators, like
        // `Compiler/munge`); this is also `(pr-str +)`'s class.
        assert_eq!(munge("clojure.core/+"), "clojure.core$_PLUS_");
    }

    #[test]
    fn native_fn_debug_names_id_and_display() {
        let func: Arc<NativeImpl> = Arc::new(|_, _| Ok(Value::Nil));
        let f = NativeFn {
            id: 7,
            display: Arc::from("user/dbl"),
            func,
            meta: None,
        };
        let s = format!("{f:?}");
        assert!(s.contains("NativeFn"), "{s}");
        assert!(s.contains('7'), "{s}");
        assert!(s.contains("user/dbl"), "{s}");
    }

    #[test]
    fn downcasts_reject_other_values() {
        assert!(as_closure(&Value::Int(1)).is_none());
        assert!(as_native(&Value::Int(1)).is_none());
    }
}
