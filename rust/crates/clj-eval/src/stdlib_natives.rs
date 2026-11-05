// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Native fast paths for the embedded `clojure.string` fns nearly
//! every script calls (#226): `split`, `join`, `trim`, `blank?`,
//! `split-lines`. Each answers its common arguments (strings, a
//! regex) exactly as the upstream definition would through the
//! port's interop — the same `Pattern.split`, the same Java
//! whitespace predicate — and hands anything else (nil, other
//! `CharSequence`s, odd arities) to the interpreted upstream
//! definition, so errors and edge cases stay the file's own. The
//! upstream `string.clj` loads unchanged: its forms remain the
//! vars' metadata source and the slow path (`core_lazy`).

use std::sync::Arc;

use clj_value::{Symbol, ThrowClass, Value};

use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::{StepUnit, is_seqable, seq_value, step_unit};
use crate::ns::signal_to_native;

type Native = fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError>;

/// The native-backed `clojure.string` fns, by name.
pub(crate) const STRING_NATIVES: &[(&str, Native)] = &[
    ("split", native_split),
    ("split-lines", native_split_lines),
    ("join", native_join),
    ("trim", native_trim),
    ("blank?", native_blank),
];

/// Call the interpreted upstream definition of `clojure.string/name`
/// (the slow path for arguments outside a fast path). The var is
/// found live or in a `remove-ns` tombstone, so a captured native
/// keeps working like the JVM's self-contained fn; only an
/// `ns-unmap`ped name has no definition left to run.
fn upstream(eval: &mut Evaluator, name: &str, args: &[Value]) -> Result<Value, NativeError> {
    let sym = Symbol::new(Some("clojure.string"), name);
    let f = eval
        .registry
        .lookup_in_live_or_removed("clojure.string", &sym)
        .and_then(|id| crate::core_lazy::interpreted(eval, id));
    match f {
        Some(f) => eval.call(&f, args).map_err(signal_to_native),
        None => Err(NativeError::new(
            clj_value::ThrowClass::IllegalStateException,
            format!("No definition behind clojure.string/{name}"),
        )),
    }
}

/// `(split s re)` / `(split s re limit)`: `Pattern.split`, with
/// the compiled checks of `string.clj` in bytecode order — the
/// `^Pattern` and `^CharSequence` casts, the `limit` `Number` cast
/// plus `intCast`, then the null-target NPEs.
fn native_split(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (s, re, limit) = match args {
        [s, re] => (s, re, None),
        [s, re, limit] => (s, re, Some(limit)),
        _ => return upstream(eval, "split", args),
    };
    if !matches!(re, Value::Regex(_) | Value::Nil) {
        return Err(crate::core::cce_err(re, "java.util.regex.Pattern"));
    }
    let text = match s {
        Value::Nil => None,
        other => Some(
            crate::classemu::char_sequence_text(other)
                .ok_or_else(|| crate::core::cce_err(other, "java.lang.CharSequence"))?,
        ),
    };
    let n = limit.map(crate::core::intcast).transpose()?;
    let Value::Regex(re) = re else {
        let params = if n.is_some() {
            "java.lang.CharSequence, int"
        } else {
            "java.lang.CharSequence"
        };
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            format!(
                "Cannot invoke \"java.util.regex.Pattern.split({params})\" because \"re\" is null"
            ),
        ));
    };
    let Some(text) = text else {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
                .to_owned(),
        ));
    };
    crate::re::pattern_split(&text, re, n.unwrap_or(0))
}

/// `(split-lines s)`: `(split s #"\r?\n")`.
fn native_split_lines(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Str(s)] => crate::re::pattern_split(s, "\r?\n", 0),
        _ => upstream(eval, "split-lines", args),
    }
}

/// `(trim s)`: Java whitespace off both ends (whitespace is BMP-only
/// and surrogates never match, so trimming code points equals the
/// upstream `charAt` scan).
fn native_trim(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Str(s)] => Ok(Value::Str(Arc::from(
            s.trim_matches(crate::classemu::java_is_whitespace),
        ))),
        _ => upstream(eval, "trim", args),
    }
}

/// `(blank? s)`: true for nil/false or all-Java-whitespace strings.
fn native_blank(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Nil | Value::Bool(false)] => Ok(Value::Bool(true)),
        [Value::Str(s)] => Ok(Value::Bool(
            s.chars().all(crate::classemu::java_is_whitespace),
        )),
        _ => upstream(eval, "blank?", args),
    }
}

/// `(join sep coll)`: each item's `str` between `(str sep)`, walking
/// `coll` step by step like the upstream `first`/`next` loop (each
/// step realizes before the items it yields are stringified). A
/// separator whose `str` could run user code, a 1-arity call, or an
/// unseqable coll takes the upstream path.
fn native_join(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let [sep, coll] = args else {
        return upstream(eval, "join", args);
    };
    let sep = match sep {
        Value::Str(s) => s.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Nil => String::new(),
        _ => return upstream(eval, "join", args),
    };
    if !is_seqable(coll) {
        return upstream(eval, "join", args);
    }
    let mut out = String::new();
    let mut first = true;
    let mut cur = seq_value(eval, coll).map_err(signal_to_native)?;
    loop {
        let items: Vec<Value> = match step_unit(eval, &cur).map_err(signal_to_native)? {
            StepUnit::Empty => break,
            StepUnit::One { head, tail } => {
                cur = tail;
                vec![head]
            }
            StepUnit::Chunk { items, tail } => {
                cur = tail;
                items.to_vec()
            }
        };
        for item in &items {
            if !first {
                out.push_str(&sep);
            }
            first = false;
            out.push_str(&crate::core::str_of_one(eval, item)?);
        }
    }
    Ok(Value::Str(Arc::from(out)))
}
