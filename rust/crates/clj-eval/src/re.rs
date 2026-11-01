// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #32 regex natives: `re-pattern` compiles (or passes a
//! `Pattern` through); `re-find` / `re-matches` / `re-seq` match
//! over the `Value::Regex` source with JVM result shapes (a bare
//! string vs a group vector, nil holes, `re-seq` nil-on-no-match).
//! Oracle: the alpha10 `clojure.core` sources (`re-matcher` casts,
//! `re-groups` shapes); the `regex` crate rejects look-around at
//! compile time (a documented divergence surfacing as
//! `PatternSyntaxException`, like the JVM).

use std::sync::Arc;

use clj_value::{SeqKind, ThrowClass, Value, Vector};
use regex::Regex;

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::realized_value;
use crate::ns::{NativeFnPtr, check_arity};

/// The §N regex natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const RE_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("re-pattern", native_re_pattern),
    ("re-find", native_re_find),
    ("re-matches", native_re_matches),
    ("re-seq", native_re_seq),
];

/// A `regex`-crate compile failure as the JVM's
/// `PatternSyntaxException` (the message is the crate report —
/// the JVM's own text is engine-specific).
fn pattern_syntax(err: regex::Error) -> NativeError {
    NativeError::new(ThrowClass::PatternSyntaxException, err.to_string())
}

/// Compile a pattern source (a stored `Pattern` source is always
/// compilable — only the reader mints `Value::Regex` — but a
/// stale value still reports `PatternSyntaxException`).
fn compile_source(source: &str) -> Result<Regex, NativeError> {
    Regex::new(source).map_err(pattern_syntax)
}

/// A `re-find`/`re-matches`/`re-seq` pattern: a stored
/// `Pattern` only (a `String` is NOT coerced — the JVM casts,
/// oracle `refind-strpat`).
fn pattern_source(v: &Value) -> Result<&str, NativeError> {
    match v {
        Value::Regex(source) => Ok(source),
        other => Err(cce_err(other, "java.util.regex.Pattern")),
    }
}

/// A `re-find`/`re-matches`/`re-seq` subject: `String` text (nil
/// trips the `Matcher.find` NPE, anything else the
/// `CharSequence` cast — both oracle-verbatim).
fn match_text(v: &Value) -> Result<&str, NativeError> {
    match v {
        Value::Str(s) => Ok(s),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
                .to_owned(),
        )),
        other => Err(cce_err(other, "java.lang.CharSequence")),
    }
}

/// One `re-groups`: the whole match when the pattern holds no
/// groups, else the `[full g1 ...]` vector (nil for unmatched
/// groups, like `Matcher.group` returning null).
fn groups_value(caps: &regex::Captures<'_>) -> Value {
    if caps.len() == 1 {
        let text = caps.get(0).map_or("", |m| m.as_str());
        return Value::Str(Arc::from(text));
    }
    let mut items = Vec::with_capacity(caps.len());
    for i in 0..caps.len() {
        match caps.get(i) {
            Some(m) => items.push(Value::Str(Arc::from(m.as_str()))),
            None => items.push(Value::Nil),
        }
    }
    Value::Vector(Vector::from_vec(items))
}

/// Split `text` around matches of the `source` pattern (the
/// JDK `Pattern.java#split` loop over the crate's `find_iter`,
/// whose empty-match advance matches `Matcher.find`): a
/// zero-width match at the start yields no empty leader, no
/// match at all yields the whole input, a positive `limit` caps
/// the parts (the last keeps the remainder), `0` drops trailing
/// empties, negative keeps everything. Bad patterns report
/// `PatternSyntaxException` (the crate text, like `re-pattern`).
/// The running state of the JDK split loop (split out so each
/// half stays under the complexity ratchet).
struct SplitAcc {
    parts: Vec<String>,
    index: usize,
    count: i64,
}

/// One `find_iter` step of the JDK split loop: matches before the
/// cap push the text before them (a zero-width match at the start
/// yields no empty leader); once a positive limit is reached, the
/// next match pushes the remainder instead and later matches are
/// dead (the `count == cap` arm fires exactly once).
fn split_step(acc: &mut SplitAcc, text: &str, m: regex::Match, limited: bool, cap: i64) {
    if !limited || acc.count < cap {
        let leader = acc.index == 0 && m.start() == 0 && m.is_empty();
        if !leader {
            acc.parts.push(text[acc.index..m.start()].to_owned());
            acc.index = m.end();
            acc.count += 1;
        }
    } else if acc.count == cap {
        acc.parts.push(text[acc.index..].to_owned());
        acc.index = m.end();
        acc.count += 1;
    }
}

pub(crate) fn pattern_split(text: &str, source: &str, limit: i32) -> Result<Value, NativeError> {
    let re = compile_source(source)?;
    let limited = limit > 0;
    let cap = i64::from(limit) - 1;
    let mut acc = SplitAcc {
        parts: Vec::new(),
        index: 0,
        count: 0,
    };
    for m in re.find_iter(text) {
        split_step(&mut acc, text, m, limited, cap);
    }
    if acc.index == 0 {
        return Ok(Value::Vector(Vector::from_vec(vec![Value::Str(
            Arc::from(text),
        )])));
    }
    if !limited || acc.count < i64::from(limit) {
        acc.parts.push(text[acc.index..].to_owned());
    }
    if limit == 0 {
        while acc.parts.last().is_some_and(|p| p.is_empty()) {
            acc.parts.pop();
        }
    }
    Ok(Value::Vector(Vector::from_vec(
        acc.parts
            .into_iter()
            .map(|p| Value::Str(Arc::from(p)))
            .collect(),
    )))
}

/// `re-pattern`: a `Pattern` passes through, a `String` compiles;
/// anything else fails the `String` cast (oracle `repat-long`:
/// the JVM compiles the `String` argument, so a `Long` fails
/// against `String`, not `Pattern`).
fn native_re_pattern(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-pattern", args, &[1])?;
    match &args[0] {
        Value::Regex(_) => Ok(args[0].clone()),
        Value::Str(s) => {
            compile_source(s)?;
            Ok(Value::Regex(Arc::clone(s)))
        }
        other => Err(cce_err(other, "java.lang.String")),
    }
}

/// `re-find`: the next match (`re-groups` shape, nil on no
/// match). The 1-arity takes a `Matcher`: an S12 handle advances
/// (owner-ACKED companion arm, issue #54); every other argument
/// fails the `Matcher` cast exactly like the JVM (oracles
/// `refind-arity`, `re-r2b`).
fn native_re_find(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-find", args, &[1, 2])?;
    if args.len() == 1 {
        if let Some(found) = crate::collmisc::match_handle_find(&args[0]) {
            return found;
        }
        return Err(cce_err(&args[0], "java.util.regex.Matcher"));
    }
    let re = compile_source(pattern_source(&args[0])?)?;
    let text = match_text(&args[1])?;
    Ok(match re.captures(text) {
        Some(caps) => groups_value(&caps),
        None => Value::Nil,
    })
}

/// The `re-matches` full-match anchor: the source wrapped so a
/// top-level `|` cannot escape the match (`\A(?:src)\z`, per the
/// slice contract).
fn anchored(source: &str) -> String {
    format!("\\A(?:{source})\\z")
}

/// `re-matches`: the whole string must match (`re-groups`
/// shape, nil otherwise).
fn native_re_matches(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-matches", args, &[2])?;
    let source = pattern_source(&args[0])?;
    let text = match_text(&args[1])?;
    let re = compile_source(&anchored(source))?;
    Ok(match re.captures(text) {
        Some(caps) => groups_value(&caps),
        None => Value::Nil,
    })
}

/// `re-seq`: successive `find` results (`re-groups` each — a
/// `Cons` like the JVM's, nil when nothing matches at all).
fn native_re_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-seq", args, &[2])?;
    let re = compile_source(pattern_source(&args[0])?)?;
    let text = match_text(&args[1])?;
    let items: Vec<Value> = re
        .captures_iter(text)
        .map(|caps| groups_value(&caps))
        .collect();
    if items.is_empty() {
        Ok(Value::Nil)
    } else {
        Ok(realized_value(items, false, false, false, SeqKind::Cons))
    }
}
