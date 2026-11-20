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

use std::borrow::Cow;
use std::sync::Arc;

use clj_value::{Symbol, ThrowClass, Value};

use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::{StepUnit, is_seqable, seq_value, step_unit};
use crate::ns::{check_arity, signal_to_native};

type Native = fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError>;

/// The native-backed `clojure.string` fns, by name.
pub(crate) const STRING_NATIVES: &[(&str, Native)] = &[
    ("split", native_split),
    ("split-lines", native_split_lines),
    ("join", native_join),
    ("trim", native_trim),
    ("blank?", native_blank),
    ("starts-with?", native_starts_with),
    ("ends-with?", native_ends_with),
    ("includes?", native_includes),
];

/// Call the interpreted upstream definition of `clojure.string/name`
/// (the slow path for arguments outside a fast path). The var is
/// found live or in a `remove-ns` tombstone, so a captured native
/// keeps working like the JVM's self-contained fn; only an
/// `ns-unmap`ped name has no definition left to run.
/// Fallback to the interpreted `clojure.string` definition (or direct Lite
/// handling when `lazy_hooks` is absent in `BootMode::Lite`).
/// The `string.clj` definition behind a native's slow path. `run` (full
/// boot) calls the interpreted source itself; `BootMode::Lite` keeps no
/// source resident and runs `lite`, that definition's literal
/// transcription through the same interop entry points (pinned to `run`
/// input by input by the `lite_parity` suite, #271).
fn upstream(
    eval: &mut Evaluator,
    name: &str,
    args: &[Value],
    lite: impl FnOnce(&mut Evaluator) -> Result<Value, NativeError>,
) -> Result<Value, NativeError> {
    if eval.lazy_hooks.is_none() {
        return lite(eval);
    }
    let sym = Symbol::new(Some("clojure.string"), name);
    let f = eval
        .registry
        .lookup_in_live_or_removed("clojure.string", &sym)
        .and_then(|id| eval.lazy_interpreted(id));
    match f {
        Some(f) => eval.call(&f, args).map_err(signal_to_native),
        None => Err(NativeError::new(
            ThrowClass::IllegalStateException,
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
        _ => {
            return upstream(eval, "split", args, |_| {
                check_arity("clojure.string/split", args, &[2, 3]).map(|()| Value::Nil)
            });
        }
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
        // The escaped source (as `string.clj` spells it), so the split
        // takes the regex-free `\r?\n` byte cut instead of compiling a
        // pattern of literal CR/LF characters (#271).
        [Value::Str(s)] => crate::re::pattern_split(s, r"\r?\n", 0),
        _ => upstream(eval, "split-lines", args, |e| lite_split_lines(e, args)),
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
        _ => upstream(eval, "trim", args, |e| lite_trim(e, args)),
    }
}

/// `(blank? s)`: true for nil/false or all-Java-whitespace strings.
fn native_blank(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Nil | Value::Bool(false)] => Ok(Value::Bool(true)),
        [Value::Str(s)] => Ok(Value::Bool(
            s.chars().all(crate::classemu::java_is_whitespace),
        )),
        _ => upstream(eval, "blank?", args, |e| lite_blank(e, args)),
    }
}

/// `(starts-with? s substr)`: for two strings, the UTF-8 byte
/// prefix test equals the upstream UTF-16 unit test (a valid
/// string's unit prefix is its char prefix). Anything else (nil,
/// other `CharSequence`s, odd arities) takes the upstream path.
fn native_starts_with(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Str(s), Value::Str(p)] => Ok(Value::Bool(s.starts_with(p.as_ref()))),
        _ => upstream(eval, "starts-with?", args, |e| {
            lite_to_string_member(e, "starts-with?", "startsWith", args)
        }),
    }
}

/// `(ends-with? s suffix)`: byte suffix test, as `starts-with?`.
fn native_ends_with(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Str(s), Value::Str(p)] => Ok(Value::Bool(s.ends_with(p.as_ref()))),
        _ => upstream(eval, "ends-with?", args, |e| {
            lite_to_string_member(e, "ends-with?", "endsWith", args)
        }),
    }
}

/// `(includes? s substr)`: substring test, as `starts-with?`.
fn native_includes(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args {
        [Value::Str(s), Value::Str(p)] => Ok(Value::Bool(s.contains(p.as_ref()))),
        _ => upstream(eval, "includes?", args, |e| {
            lite_to_string_member(e, "includes?", "contains", args)
        }),
    }
}

pub(crate) fn fast_join_sep(sep: &Value) -> Option<Cow<'_, str>> {
    match sep {
        Value::Str(s) => Some(Cow::Borrowed(s)),
        Value::Char(c) => Some(Cow::Owned(c.to_string())),
        Value::Nil => Some(Cow::Borrowed("")),
        _ => None,
    }
}

/// Zero-copy fast path for joining a 1-element vector holding a single string.
fn single_str_join(v: &clj_value::Vector) -> Option<Value> {
    if v.len() != 1 {
        return None;
    }
    if let Some(xs) = v.as_flat_values() {
        if let Value::Str(s) = &xs[0] {
            return Some(Value::Str(Arc::clone(s)));
        }
    }
    None
}

fn exact_join_cap(v: &clj_value::Vector, sep_len: usize) -> Option<usize> {
    let mut total = v.len().saturating_sub(1).saturating_mul(sep_len);
    for item in v.iter() {
        let add = match item {
            Value::Str(s) => s.len(),
            Value::Char(c) => c.len_utf8(),
            Value::Nil => 0,
            _ => return None,
        };
        total = total.checked_add(add)?;
    }
    Some(total)
}

fn join_vector_buf(
    eval: &mut Evaluator,
    sep: &str,
    v: &clj_value::Vector,
) -> Result<String, NativeError> {
    if let Some(ps) = v.as_packed_strs() {
        return Ok(ps.join(sep));
    }
    let cap = if v.len() >= 64 {
        exact_join_cap(v, sep.len()).unwrap_or_else(|| v.len().saturating_mul(sep.len() + 4))
    } else {
        v.len().saturating_mul(sep.len() + 4)
    };
    let mut out = String::with_capacity(cap);
    let mut first = true;
    for item in v.iter() {
        if !first {
            out.push_str(sep);
        }
        first = false;
        crate::core::str_push(&mut out, eval, &item)?;
    }
    Ok(out)
}

fn join_seq(eval: &mut Evaluator, sep: &str, coll: &Value) -> Result<Value, NativeError> {
    let mut out = String::new();
    let mut first = true;
    let mut cur = seq_value(eval, coll).map_err(signal_to_native)?;
    loop {
        match step_unit(eval, &cur).map_err(signal_to_native)? {
            StepUnit::Empty => break,
            StepUnit::One { head, tail } => {
                cur = tail;
                if !first {
                    out.push_str(sep);
                }
                first = false;
                crate::core::str_push(&mut out, eval, &head)?;
            }
            StepUnit::Chunk { items, tail } => {
                cur = tail;
                for item in items.iter() {
                    if !first {
                        out.push_str(sep);
                    }
                    first = false;
                    crate::core::str_push(&mut out, eval, item)?;
                }
            }
        }
    }
    Ok(Value::Str(Arc::from(out)))
}

/// `(join sep coll)`: each item's `str` between `(str sep)`, walking
/// `coll` step by step like the upstream `first`/`next` loop (each
/// step realizes before the items it yields are stringified). A
/// separator whose `str` could run user code, a 1-arity call, or an
/// unseqable coll takes the upstream path.
fn native_join(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let [sep, coll] = args else {
        return upstream(eval, "join", args, |e| lite_join(e, args));
    };
    let Some(sep) = fast_join_sep(sep) else {
        return upstream(eval, "join", args, |e| lite_join(e, args));
    };
    if !is_seqable(coll) {
        return upstream(eval, "join", args, |e| lite_join(e, args));
    }
    if let Value::Vector(v) = coll {
        if let Some(s) = single_str_join(v) {
            return Ok(s);
        }
        return Ok(Value::Str(Arc::from(join_vector_buf(eval, &sep, v)?)));
    }
    join_seq(eval, &sep, coll)
}

/// Owned-args `clojure.string/join`: drops uniquely held large vectors
/// before allocating the result `Arc<str>` so peak RSS avoids holding
/// both `parts` and the joined output simultaneously.
pub(crate) fn native_join_owned(
    eval: &mut Evaluator,
    mut args: smallvec::SmallVec<[Value; 4]>,
) -> Result<Value, NativeError> {
    if args.len() == 2 {
        let coll = args.pop().unwrap_or(Value::Nil);
        let sep_val = args.pop().unwrap_or(Value::Nil);
        if let Value::Vector(v) = coll {
            if let Some(sep) = fast_join_sep(&sep_val) {
                if let Some(s) = single_str_join(&v) {
                    return Ok(s);
                }
                let out = match v.into_packed_strs() {
                    Ok(ps) => ps.into_joined(sep.as_ref()),
                    Err(v) => join_vector_buf(eval, sep.as_ref(), &v)?,
                };
                drop(sep);
                drop(sep_val);
                return Ok(Value::Str(Arc::from(out.as_str())));
            }
            return native_join(eval, &[sep_val, Value::Vector(v)]);
        }
        return native_join(eval, &[sep_val, coll]);
    }
    native_join(eval, &args)
}

/// `(.member target args…)` through `__meth*`, the reflector path every
/// interpreted `string.clj` body takes (misses, NPEs, and casts read
/// exactly as under `run`).
fn meth(
    eval: &mut Evaluator,
    member: &str,
    target: &Value,
    args: &[Value],
) -> Result<Value, NativeError> {
    let mut full = Vec::with_capacity(args.len() + 2);
    full.push(Value::Str(Arc::from(member)));
    full.push(target.clone());
    full.extend_from_slice(args);
    crate::types::native_meth(eval, &full)
}

/// `(Character/isWhitespace (.charAt s i))`.
fn char_at_whitespace(eval: &mut Evaluator, s: &Value, i: &Value) -> Result<bool, NativeError> {
    let c = meth(eval, "charAt", s, std::slice::from_ref(i))?;
    let ws = crate::classemu::static_character_is_whitespace(&[c])?;
    Ok(matches!(ws, Value::Bool(true)))
}

/// Lite `trim`: `(.length s)`, the right-hand `charAt` scan down to
/// `zero?`, the left-hand scan, then `(.. s (subSequence l r) toString)`.
fn lite_trim(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.string/trim", args, &[1])?;
    let s = &args[0];
    let mut rindex = meth(eval, "length", s, &[])?;
    loop {
        let zero = crate::core::native_zero(eval, std::slice::from_ref(&rindex))?;
        if matches!(zero, Value::Bool(true)) {
            return Ok(Value::Str(Arc::from("")));
        }
        let prev = crate::core::native_dec(eval, std::slice::from_ref(&rindex))?;
        if !char_at_whitespace(eval, s, &prev)? {
            break;
        }
        rindex = prev;
    }
    let mut lindex = Value::Int(0);
    while char_at_whitespace(eval, s, &lindex)? {
        lindex = crate::core::native_inc(eval, std::slice::from_ref(&lindex))?;
    }
    let sub = meth(eval, "subSequence", s, &[lindex, rindex])?;
    meth(eval, "toString", &sub, &[])
}

/// Lite `blank?` (nil and false answer on the fast path): each step
/// compares `(.length s)` with the index, then tests that `charAt`.
fn lite_blank(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.string/blank?", args, &[1])?;
    let s = &args[0];
    let mut index = Value::Int(0);
    loop {
        if meth(eval, "length", s, &[])? == index {
            return Ok(Value::Bool(true));
        }
        if !char_at_whitespace(eval, s, &index)? {
            return Ok(Value::Bool(false));
        }
        index = crate::core::native_inc(eval, std::slice::from_ref(&index))?;
    }
}

/// Lite `split-lines`: `(split s #"\r?\n")` through the `split` native.
fn lite_split_lines(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.string/split-lines", args, &[1])?;
    native_split(eval, &[args[0].clone(), Value::Regex(Arc::from(r"\r?\n"))])
}

/// Lite `starts-with?` / `ends-with?` / `includes?`:
/// `(.member (.toString s) substr)`.
fn lite_to_string_member(
    eval: &mut Evaluator,
    name: &str,
    member: &str,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity(&format!("clojure.string/{name}"), args, &[2])?;
    let s = meth(eval, "toString", &args[0], &[])?;
    meth(eval, member, &s, std::slice::from_ref(&args[1]))
}

/// Lite `join`: `(apply str coll)`, or `(str sep)` between each item's
/// `str` (the seq walk raises `seq`'s own errors; `str` runs no user
/// code under Lite, so stringifying the separator first is unobservable).
fn lite_join(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.string/join", args, &[1, 2])?;
    if args.len() == 1 {
        return join_seq(eval, "", &args[0]);
    }
    let mut sep = String::new();
    crate::core::str_push(&mut sep, eval, &args[0])?;
    join_seq(eval, &sep, &args[1])
}

#[cfg(test)]
mod tests {
    use super::{
        lite_blank, lite_trim, native_blank, native_includes, native_join, native_join_owned,
        native_trim,
    };
    use clj_value::{Value, Vector};
    use smallvec::SmallVec;
    use std::sync::Arc;

    #[test]
    fn join_owned_pre_sizes_large_and_handles_fallbacks() {
        let mut eval = crate::aot_boot::boot_lite(
            &[("clojure.string", "join"), ("clojure.core", "str")],
            false,
        );
        // Large string/char/nil vector hits `exact_join_cap` (`Some`), while mixed ints hit `None`.
        let mut strs: Vec<Value> = (0..70)
            .map(|i| Value::Str(Arc::from(format!("x{i}"))))
            .collect();
        strs[0] = Value::Char('Z');
        strs[1] = Value::Nil;
        let out = native_join_owned(
            &mut eval,
            SmallVec::from_iter([
                Value::Str(Arc::from(",")),
                Value::Vector(Vector::from_vec(strs)),
            ]),
        )
        .unwrap();
        assert!(format!("{out:?}").contains("Z,,x2"));

        let ints: Vec<Value> = (0..70).map(Value::Int).collect();
        let out_ints = native_join_owned(
            &mut eval,
            SmallVec::from_iter([Value::Nil, Value::Vector(Vector::from_vec(ints))]),
        )
        .unwrap();
        assert!(format!("{out_ints:?}").contains("0123"));

        // Small vector (`len < 64`) and `native_join` / `native_includes` fallbacks.
        let small = Value::Vector(Vector::from_vec(vec![Value::Int(1), Value::Int(2)]));
        assert_eq!(
            native_join(&mut eval, &[Value::Str(Arc::from(",")), small.clone()]).unwrap(),
            Value::Str(Arc::from("1,2"))
        );
        assert!(native_join(&mut eval, &[Value::Str(Arc::from(",")), Value::Int(1)]).is_err());
        assert!(native_includes(&mut eval, &[Value::Int(1)]).is_err());

        // Non-vector / char sep / 1-arg fallback in `native_join_owned`.
        let list = Value::List(clj_value::List::from_vec(vec![
            Value::Int(1),
            Value::Int(2),
        ]));
        let out_char_sep =
            native_join_owned(&mut eval, SmallVec::from_iter([Value::Char(':'), list])).unwrap();
        assert_eq!(out_char_sep, Value::Str(Arc::from("1:2")));

        let out_one = native_join_owned(&mut eval, SmallVec::from_iter([small])).unwrap();
        assert_eq!(out_one, Value::Str(Arc::from("12")));

        // single_str_join hits (both native_join and native_join_owned) and misses
        let one_str = Value::Vector(Vector::from_vec(vec![Value::Str(Arc::from("solo"))]));
        assert_eq!(
            native_join(&mut eval, &[Value::Str(Arc::from(",")), one_str.clone()]).unwrap(),
            Value::Str(Arc::from("solo"))
        );
        assert_eq!(
            native_join_owned(
                &mut eval,
                SmallVec::from_iter([Value::Str(Arc::from(",")), one_str])
            )
            .unwrap(),
            Value::Str(Arc::from("solo"))
        );
        let one_int = Value::Vector(Vector::from_vec(vec![Value::Int(42)]));
        assert_eq!(
            native_join(&mut eval, &[Value::Str(Arc::from(",")), one_int]).unwrap(),
            Value::Str(Arc::from("42"))
        );
        let one_bool = Value::Vector(Vector::from_vec(vec![Value::Bool(true)]));
        assert_eq!(
            native_join(&mut eval, &[Value::Str(Arc::from(",")), one_bool]).unwrap(),
            Value::Str(Arc::from("true"))
        );

        // native_join_owned on unique PackedStrings (into_joined) and shared PackedStrings (fallback)
        let mut ps_u = clj_value::PackedStrings::new();
        assert!(ps_u.try_push("foo"));
        assert!(ps_u.try_push("bar"));
        let v_ps = Vector::from_packed_strs(ps_u);
        assert_eq!(
            native_join_owned(
                &mut eval,
                SmallVec::from_iter([Value::Str(Arc::from(":")), Value::Vector(v_ps)])
            )
            .unwrap(),
            Value::Str(Arc::from("foo:bar"))
        );

        let mut ps_s = clj_value::PackedStrings::new();
        assert!(ps_s.try_push("p"));
        assert!(ps_s.try_push("q"));
        let v_shared = Vector::from_packed_strs(ps_s);
        let _keep = v_shared.clone();
        assert_eq!(
            native_join_owned(
                &mut eval,
                SmallVec::from_iter([Value::Str(Arc::from("-")), Value::Vector(v_shared)])
            )
            .unwrap(),
            Value::Str(Arc::from("p-q"))
        );

        // Non-fast separator with a Vector argument falls back through native_join
        let v_two = Value::Vector(Vector::from_vec(vec![Value::Int(1), Value::Int(2)]));
        assert_eq!(
            native_join_owned(&mut eval, SmallVec::from_iter([Value::Int(0), v_two])).unwrap(),
            Value::Str(Arc::from("102"))
        );
    }

    #[test]
    fn lite_transcriptions_agree_with_the_fast_paths_on_strings() {
        // The Lite slow paths replay `string.clj` through `__meth*`. No
        // non-String receiver gets past `.charAt` today (`StringBuilder`
        // and `File` answer `.length` only), but on a String every interop
        // step answers, so the transcriptions run end to end here and must
        // agree with the native fast paths, Java whitespace included
        // (U+2003 trims, NBSP does not).
        let mut eval = crate::aot_boot::boot_lite(&[], false);
        for s in ["", "   ", " a b ", "\t\nx\u{2003}", "abc", "\u{a0}a\u{a0}"] {
            let arg = [Value::Str(Arc::from(s))];
            let (lite, fast) = (lite_trim(&mut eval, &arg), native_trim(&mut eval, &arg));
            assert_eq!(lite.ok(), fast.ok(), "trim {s:?}");
            let (lite, fast) = (lite_blank(&mut eval, &arg), native_blank(&mut eval, &arg));
            assert_eq!(lite.ok(), fast.ok(), "blank? {s:?}");
        }
    }
}
