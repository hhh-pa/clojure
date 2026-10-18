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

use clj_value::{ExtensionValue, Value};

use crate::env::Env;
use crate::error::NativeError;
use crate::expr::Arity;

/// `Compiler.munge` for display names (`-` → `_`, `/` → `$`,
/// punctuation → `_NAME_`). Common names are exact; exotic ones
/// follow the same table.
#[must_use]
pub fn munge(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for c in display.chars() {
        match c {
            '-' => out.push('_'),
            '/' => out.push('$'),
            '.' => out.push('_'),
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
    out
}

/// An interpreted function value: display name, arities, captured
/// frame. Identity is the sequence `id`.
#[derive(Clone, Debug)]
pub struct Closure {
    pub id: u64,
    pub display: Arc<str>,
    pub arities: Vec<Arity>,
    pub env: Env,
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
/// call. Failures are [`NativeError`]s (the evaluator stamps the
/// identity when throwing).
pub type NativeImpl = dyn Fn(&[Value]) -> Result<Value, NativeError> + Send + Sync;

#[derive(Clone)]
pub struct NativeFn {
    pub id: u64,
    pub display: Arc<str>,
    pub func: Arc<NativeImpl>,
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
