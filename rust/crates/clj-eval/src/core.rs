// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D scalar natives: arithmetic (C1), comparison (C2), predicates,
//! the `str` family, `format` (US-locale subset), the `ex-info`
//! family, `trampoline`, and `reduced`. Oracle: probe39; rules:
//! `docs/t1174/s5d-contract.md`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use clj_value::{
    ExtensionValue, Keyword, PrintOpts, Symbol, ThrowClass, Throwable, Value, pr_to_string,
};

use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::lazy::{
    as_pending, as_realized, as_subvector, collect_all, equiv_vals, is_counted_seq, is_seq_value,
    realize_deep,
};
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

/// `Cannot invoke "Object.getClass()" because "x" is null`: the
/// numbers-`nil` failure (every arithmetic/comparison op probes it).
pub(crate) fn npe_getclass() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"Object.getClass()\" because \"x\" is null".to_owned(),
    )
}

/// A JVM `ClassCastException` text with JDK module verbiage: same
/// modules combine (`A and B are in M`), different modules split
/// (`A is in M1; B is in M2`) — byte-pinned by probe39.
pub(crate) fn cce_message(from_jvm: &str, to_jvm: &str) -> String {
    fn module_of(cls: &str) -> &'static str {
        if cls.starts_with("java.") {
            "module java.base of loader 'bootstrap'"
        } else {
            "unnamed module of loader 'app'"
        }
    }
    let from_mod = module_of(from_jvm);
    let to_mod = module_of(to_jvm);
    if from_mod == to_mod {
        format!(
            "class {from_jvm} cannot be cast to class {to_jvm} ({from_jvm} and {to_jvm} are in {from_mod})"
        )
    } else {
        format!(
            "class {from_jvm} cannot be cast to class {to_jvm} ({from_jvm} is in {from_mod}; {to_jvm} is in {to_mod})"
        )
    }
}

/// A cast failure to `to_jvm` (NPEs never reach here — nils fail
/// loudly at each call site instead).
pub(crate) fn cce_err(v: &Value, to_jvm: &str) -> NativeError {
    NativeError::new(
        ThrowClass::ClassCastException,
        cce_message(&jvm_class_of(v), to_jvm),
    )
}

/// A `take`/`drop` count: `Int` as-is, `Double` rounded up (the
/// JVM `pos?`/`dec` loop takes `ceil(n)`), `NaN` takes nothing,
/// infinities take everything-or-nothing, `nil` is the numbers
/// NPE, anything else fails to `Number`.
pub(crate) fn take_count(v: &Value) -> Result<i64, NativeError> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Double(d) => {
            if d.is_nan() {
                Ok(0)
            } else if *d >= i64::MAX as f64 {
                Ok(i64::MAX)
            } else if *d <= i64::MIN as f64 {
                Ok(i64::MIN)
            } else {
                Ok(d.ceil() as i64)
            }
        }
        Value::Nil => Err(npe_getclass()),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// A `repeat` count: `intValue` truncation (`NaN` is zero,
/// infinities saturate; `nil` trips the `Repeat.create`
/// `doubleValue` NPE, anything else fails to `Number`).
pub(crate) fn repeat_count(v: &Value) -> Result<i64, NativeError> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Double(d) => {
            if d.is_nan() {
                Ok(0)
            } else if *d >= i64::MAX as f64 {
                Ok(i64::MAX)
            } else if *d <= i64::MIN as f64 {
                Ok(i64::MIN)
            } else {
                Ok(*d as i64)
            }
        }
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null".to_owned(),
        )),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// `Math.toIntExact` over a `long` (out of `i32` range is
/// `ArithmeticException "integer overflow"`).
pub(crate) fn int_exact_long(l: i64) -> Result<i32, NativeError> {
    i32::try_from(l).map_err(|_| {
        NativeError::new(
            ThrowClass::ArithmeticException,
            "integer overflow".to_owned(),
        )
    })
}

/// `RT.longCast` + `intCast` over a `double` (out of `i64` range
/// is `IllegalArgumentException "Value out of range for long: X"`
/// with Java double rendering, then the `int` range check).
pub(crate) fn int_cast_double(d: f64) -> Result<i32, NativeError> {
    if d < i64::MIN as f64 || d > i64::MAX as f64 {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!(
                "Value out of range for long: {}",
                clj_value::java_double_string(d)
            ),
        ));
    }
    int_exact_long(d as i64)
}

/// The `drop`/`nthrest` `IDrop` count coercion: `Int` narrows
/// exactly, `Double` ceils through the long range check. Callers
/// `pos?`-check first, so nil/non-numbers are unreachable — but
/// validated identically for totality.
pub(crate) fn idrop_coerce(v: &Value) -> Result<i32, NativeError> {
    match v {
        Value::Int(i) => int_exact_long(*i),
        Value::Double(d) => int_cast_double(d.ceil()),
        Value::Nil => Err(npe_getclass()),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// An `nth`/`subvec`/`subs` index through the `RT.intCast`
/// shape: `nil` trips the `charValue` NPE quirk, numbers narrow
/// checked (long range then `int` range), anything else fails to
/// `Number`. Staged for `coll.rs` (S5D).
#[allow(dead_code)]
pub(crate) fn nth_index(v: &Value) -> Result<i32, NativeError> {
    if matches!(v, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null".to_owned(),
        ));
    }
    match v {
        Value::Int(i) => int_exact_long(*i),
        Value::Double(d) => int_cast_double(*d),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// Whether the value is callable (`fn?` plus the `IFn` collections,
/// keywords, symbols, and vars; `Unbound` passes through to the
/// caller's unbound error).
pub(crate) fn is_ifn(v: &Value) -> bool {
    match v {
        Value::Extension(e) => {
            e.as_any().downcast_ref::<crate::ext::Closure>().is_some()
                || e.as_any().downcast_ref::<crate::ext::NativeFn>().is_some()
                || as_subvector(v).is_some()
        }
        Value::Keyword(_)
        | Value::Symbol(_)
        | Value::Vector(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::Var(_)
        | Value::Unbound(_) => true,
        _ => false,
    }
}

/// Whether the value is a `fn?` (closures and natives only).
pub(crate) fn is_fn_val(v: &Value) -> bool {
    match v {
        Value::Extension(e) => {
            e.as_any().downcast_ref::<crate::ext::Closure>().is_some()
                || e.as_any().downcast_ref::<crate::ext::NativeFn>().is_some()
        }
        _ => false,
    }
}

/// Check a higher-order callee: nil is the nil-invoke `NPE`,
/// non-`IFn`s are the `IFn` cast failure.
pub(crate) fn check_ifn(eval: &mut Evaluator, f: &Value) -> Result<(), crate::error::Signal> {
    if matches!(f, Value::Nil) {
        return Err(eval.throw(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke nil as a function".to_owned(),
        )));
    }
    if !is_ifn(f) {
        return Err(eval.throw(NativeError::new(
            ThrowClass::ClassCastException,
            cce_message(&jvm_class_of(f), "clojure.lang.IFn"),
        )));
    }
    Ok(())
}

/// A numeric operand (nil is the numbers NPE, anything else
/// non-numeric the `Number` cast failure).
#[derive(Clone, Copy)]
pub(crate) enum Num {
    Int(i64),
    Double(f64),
}

pub(crate) fn as_num(v: &Value) -> Result<Num, NativeError> {
    match v {
        Value::Int(i) => Ok(Num::Int(*i)),
        Value::Double(d) => Ok(Num::Double(*d)),
        Value::Nil => Err(npe_getclass()),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

fn num_value(n: Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i),
        Num::Double(d) => Value::Double(d),
    }
}

/// The checked `long overflow` (C1: always checked, never wraps).
fn overflow() -> NativeError {
    NativeError::new(ThrowClass::ArithmeticException, "long overflow".to_owned())
}

/// The integer `Divide by zero` (`/`, `mod`, and the
/// `quot`/`rem`/`mod` double-zero paths; plain `/` on doubles goes
/// to `Inf`/`NaN` instead).
fn divide_by_zero() -> NativeError {
    NativeError::new(ThrowClass::ArithmeticException, "Divide by zero".to_owned())
}

/// The integer `/ by zero` (`quot`/`rem` int paths only — the
/// JVM's dedicated integer-division text, fixprobe `:quot-i0`).
fn slash_by_zero() -> NativeError {
    NativeError::new(ThrowClass::ArithmeticException, "/ by zero".to_owned())
}

/// The `Infinite or NaN` (`quot`/`rem`/`mod` double paths with a
/// non-finite operand or quotient, fixprobe `:quot-nan-div`).
fn infinite_or_nan() -> NativeError {
    NativeError::new(
        ThrowClass::NumberFormatException,
        "Infinite or NaN".to_owned(),
    )
}

fn add(a: Num, b: Num) -> Result<Num, NativeError> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x.checked_add(y).map(Num::Int).ok_or_else(overflow),
        (Num::Int(x), Num::Double(y)) => Ok(Num::Double(x as f64 + y)),
        (Num::Double(x), Num::Int(y)) => Ok(Num::Double(x + y as f64)),
        (Num::Double(x), Num::Double(y)) => Ok(Num::Double(x + y)),
    }
}

fn sub(a: Num, b: Num) -> Result<Num, NativeError> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x.checked_sub(y).map(Num::Int).ok_or_else(overflow),
        (Num::Int(x), Num::Double(y)) => Ok(Num::Double(x as f64 - y)),
        (Num::Double(x), Num::Int(y)) => Ok(Num::Double(x - y as f64)),
        (Num::Double(x), Num::Double(y)) => Ok(Num::Double(x - y)),
    }
}

fn mul(a: Num, b: Num) -> Result<Num, NativeError> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x.checked_mul(y).map(Num::Int).ok_or_else(overflow),
        (Num::Int(x), Num::Double(y)) => Ok(Num::Double(x as f64 * y)),
        (Num::Double(x), Num::Int(y)) => Ok(Num::Double(x * y as f64)),
        (Num::Double(x), Num::Double(y)) => Ok(Num::Double(x * y)),
    }
}

/// `/` on two numbers: exact only when divisible (C1 documents the
/// ratio deviation — the port divides doubles instead).
fn div(a: Num, b: Num) -> Result<Num, NativeError> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => {
            if y == 0 {
                return Err(divide_by_zero());
            }
            if x % y == 0 {
                x.checked_div(y).map(Num::Int).ok_or_else(overflow)
            } else {
                Ok(Num::Double(x as f64 / y as f64))
            }
        }
        (Num::Int(x), Num::Double(y)) => Ok(Num::Double(x as f64 / y)),
        (Num::Double(x), Num::Int(y)) => Ok(Num::Double(x / y as f64)),
        (Num::Double(x), Num::Double(y)) => Ok(Num::Double(x / y)),
    }
}

/// A left-associative variadic fold (`+`/`-`/`*`/`/`).
fn fold(
    display: &str,
    args: &[Value],
    op: fn(Num, Num) -> Result<Num, NativeError>,
) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity(display, 0));
    }
    if args.len() == 1 {
        // Single args pass through unchecked (probe39 shapes).
        return Ok(args[0].clone());
    }
    let mut acc = as_num(&args[0])?;
    for arg in &args[1..] {
        acc = op(acc, as_num(arg)?)?;
    }
    Ok(num_value(acc))
}

fn native_add(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Ok(Value::Int(0));
    }
    fold("clojure.core/+", args, add)
}

fn native_sub(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/-", 0));
    }
    if args.len() == 1 {
        return match as_num(&args[0])? {
            Num::Int(x) => x.checked_neg().map(Value::Int).ok_or_else(overflow),
            Num::Double(x) => Ok(Value::Double(-x)),
        };
    }
    fold("clojure.core/-", args, sub)
}

fn native_mul(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Ok(Value::Int(1));
    }
    fold("clojure.core/*", args, mul)
}

fn native_div(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core//", 0));
    }
    if args.len() == 1 {
        return div(Num::Int(1), as_num(&args[0])?).map(num_value);
    }
    fold("clojure.core//", args, div)
}

fn native_quot(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/quot", args, &[2])?;
    match (as_num(&args[0])?, as_num(&args[1])?) {
        (Num::Int(x), Num::Int(y)) => {
            if y == 0 {
                return Err(slash_by_zero());
            }
            x.checked_div(y).map(Value::Int).ok_or_else(overflow)
        }
        (x, y) => {
            let (x, y) = (to_f64(x), to_f64(y));
            // Zero first (even over `NaN`, `:quot-nan-0`), then
            // finiteness (either side, `:quot-nan-div`), then the
            // quotient's own finiteness (`:quot-ovf`); `-0.0`
            // normalizes to `+0.0` (`:quot-negfrac`, `:quot-2-neginf`).
            if y == 0.0 {
                return Err(divide_by_zero());
            }
            if !x.is_finite() || y.is_nan() {
                return Err(infinite_or_nan());
            }
            let q = (x / y).trunc();
            if !q.is_finite() {
                return Err(infinite_or_nan());
            }
            Ok(Value::Double(if q == 0.0 { 0.0 } else { q }))
        }
    }
}

fn native_rem(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rem", args, &[2])?;
    match (as_num(&args[0])?, as_num(&args[1])?) {
        (Num::Int(x), Num::Int(y)) => {
            if y == 0 {
                return Err(slash_by_zero());
            }
            // `MIN % -1` is the only overflow (0, but checked).
            x.checked_rem(y).map(Value::Int).ok_or_else(overflow)
        }
        (x, y) => {
            let (x, y) = (to_f64(x), to_f64(y));
            // Zero first (`:rem-nan-0`), then finiteness
            // (`:rem-nan-num`); an infinite divisor is `NaN`, NOT
            // `x` (`:rem-num-inf`, `:rem-num-neginf`).
            if y == 0.0 {
                return Err(divide_by_zero());
            }
            if !x.is_finite() || y.is_nan() {
                return Err(infinite_or_nan());
            }
            if !y.is_finite() {
                return Ok(Value::Double(f64::NAN));
            }
            Ok(Value::Double(x % y))
        }
    }
}

fn native_mod(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/mod", args, &[2])?;
    match (as_num(&args[0])?, as_num(&args[1])?) {
        (Num::Int(x), Num::Int(y)) => {
            if y == 0 {
                return Err(divide_by_zero());
            }
            let r = x.checked_rem(y).ok_or_else(overflow)?;
            Ok(Value::Int(if r == 0 || (r > 0) == (y > 0) {
                r
            } else {
                r.checked_add(y).ok_or_else(overflow)?
            }))
        }
        (x, y) => {
            let (x, y) = (to_f64(x), to_f64(y));
            // Zero first (`:mod-nan-0`), then finiteness
            // (`:mod-nan-num`); an infinite divisor falls out of
            // the formula as `NaN` (`:mod-num-inf`).
            if y == 0.0 {
                return Err(divide_by_zero());
            }
            if !x.is_finite() || y.is_nan() {
                return Err(infinite_or_nan());
            }
            Ok(Value::Double(((x % y) + y) % y))
        }
    }
}

/// A number as `f64` (mixed/double paths).
fn to_f64(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Double(d) => d,
    }
}

fn native_inc(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/inc", args, &[1])?;
    match as_num(&args[0])? {
        Num::Int(x) => x.checked_add(1).map(Value::Int).ok_or_else(overflow),
        Num::Double(x) => Ok(Value::Double(x + 1.0)),
    }
}

fn native_dec(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/dec", args, &[1])?;
    match as_num(&args[0])? {
        Num::Int(x) => x.checked_sub(1).map(Value::Int).ok_or_else(overflow),
        Num::Double(x) => Ok(Value::Double(x - 1.0)),
    }
}

/// `inc'`: auto-promoting increment — identical to `inc` here
/// (our `Int` has no `BigInt` to promote to, so the top end
/// overflows instead of growing; `(range)` only needs small
/// steps).
fn native_inc_prime(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/inc'", args, &[1])?;
    match as_num(&args[0])? {
        Num::Int(x) => x.checked_add(1).map(Value::Int).ok_or_else(overflow),
        Num::Double(x) => Ok(Value::Double(x + 1.0)),
    }
}

/// `max`/`min`: ties keep the LATTER (`(max 1 1.0)` → `1.0`),
/// and `NaN` poisons from either side (`(max 1 ##NaN)` and `(max
/// ##NaN 1)` are both `##NaN`); single args pass through unchecked
/// (fixprobe `:max-tie`, `:max-nan1/2`).
fn minmax(display: &str, args: &[Value], want_max: bool) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity(display, 0));
    }
    if args.len() == 1 {
        return Ok(args[0].clone());
    }
    let mut acc = as_num(&args[0])?;
    for arg in &args[1..] {
        let n = as_num(arg)?;
        // The incumbent survives only on a strict win; anything
        // else (a tie, or a latter-side `NaN` since every `NaN`
        // comparison is false) takes the latter — except a `NaN`
        // incumbent, which always survives (`:max-nan1`).
        let keep = match (acc, n) {
            (Num::Int(a), Num::Int(b)) => {
                if want_max {
                    a > b
                } else {
                    a < b
                }
            }
            _ => {
                let (a, b) = (to_f64(acc), to_f64(n));
                if a.is_nan() {
                    true
                } else if want_max {
                    a > b
                } else {
                    a < b
                }
            }
        };
        if !keep {
            acc = n;
        }
    }
    Ok(num_value(acc))
}

fn native_max(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    minmax("clojure.core/max", args, true)
}

fn native_min(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    minmax("clojure.core/min", args, false)
}

/// `=` as `Util.equiv` (sequential pairs walk pairwise with
/// early exit, so finite-vs-infinite answers `false` instead of
/// hanging; maps/sets recurse; the rest is `==`).
fn native_eq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/=", 0));
    }
    let mut first = &args[0];
    for arg in &args[1..] {
        if !equiv_vals(eval, first, arg).map_err(signal_to_native)? {
            return Ok(Value::Bool(false));
        }
        first = arg;
    }
    Ok(Value::Bool(true))
}

fn native_not_eq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match native_eq(eval, args)? {
        Value::Bool(b) => Ok(Value::Bool(!b)),
        _ => unreachable!("native_eq returns bools"),
    }
}

/// Cross-category numeric equivalence (`(== 1 1.0)` is true;
/// `NaN` never is, not even to itself).
fn native_num_eq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/==", 0));
    }
    if args.len() == 1 {
        // Single args pass unchecked (`(== :a)` → `true`,
        // fixprobe `:eqeq-1kw`).
        return Ok(Value::Bool(true));
    }
    let mut prev = as_num(&args[0])?;
    for arg in &args[1..] {
        let n = as_num(arg)?;
        let same = match (prev, n) {
            (Num::Int(a), Num::Int(b)) => a == b,
            _ => to_f64(prev) == to_f64(n),
        };
        if !same {
            return Ok(Value::Bool(false));
        }
        prev = n;
    }
    Ok(Value::Bool(true))
}

/// A chained numeric comparison (`<`/`>`/`<=`/`>=`, IEEE for
/// doubles, so `NaN` orders false everywhere).
fn chain_cmp(
    display: &str,
    args: &[Value],
    op: fn(f64, f64) -> bool,
    int_op: fn(i64, i64) -> bool,
) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity(display, 0));
    }
    if args.len() == 1 {
        // Single args pass unchecked (`(< "s")` → `true`,
        // fixprobe `:lt-1str`).
        return Ok(Value::Bool(true));
    }
    let mut prev = as_num(&args[0])?;
    for arg in &args[1..] {
        let n = as_num(arg)?;
        let holds = match (prev, n) {
            (Num::Int(a), Num::Int(b)) => int_op(a, b),
            _ => op(to_f64(prev), to_f64(n)),
        };
        if !holds {
            return Ok(Value::Bool(false));
        }
        prev = n;
    }
    Ok(Value::Bool(true))
}

fn native_lt(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    chain_cmp("clojure.core/<", args, |a, b| a < b, |a, b| a < b)
}

fn native_gt(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    chain_cmp("clojure.core/>", args, |a, b| a > b, |a, b| a > b)
}

fn native_lte(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    chain_cmp("clojure.core/<=", args, |a, b| a <= b, |a, b| a <= b)
}

fn native_gte(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    chain_cmp("clojure.core/>=", args, |a, b| a >= b, |a, b| a >= b)
}

fn native_zero(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/zero?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x == 0,
        Num::Double(x) => x == 0.0,
    }))
}

fn native_pos(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/pos?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x > 0,
        Num::Double(x) => x > 0.0,
    }))
}

fn native_neg(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/neg?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x < 0,
        Num::Double(x) => x < 0.0,
    }))
}

/// JVM `compare` with exact cast failures (C2): nils sort first,
/// numbers cross-compare (the `<`/`>` pair, so `NaN` ties
/// everything), vectors compare lexicographically (recursively),
/// and anything else is `x.compareTo(y)` — the checkcast of `y`
/// to `x`'s class, except numbers (whose target is `Number`) and
/// non-`Comparable`s (whose own class fails against
/// `Comparable`). All probed.
pub(crate) fn jvm_compare(x: &Value, y: &Value) -> Result<std::cmp::Ordering, NativeError> {
    use std::cmp::Ordering;
    match (x, y) {
        (Value::Nil, Value::Nil) => return Ok(Ordering::Equal),
        (Value::Nil, _) => return Ok(Ordering::Less),
        (_, Value::Nil) => return Ok(Ordering::Greater),
        _ => {}
    }
    if matches!(x, Value::Int(_) | Value::Double(_))
        && matches!(y, Value::Int(_) | Value::Double(_))
    {
        let (a, b) = (to_f64(as_num(x)?), to_f64(as_num(y)?));
        // The `<`/`>` pair (never `total_cmp`): `NaN` ties.
        if a < b {
            return Ok(Ordering::Less);
        }
        if a > b {
            return Ok(Ordering::Greater);
        }
        return Ok(Ordering::Equal);
    }
    if matches!(x, Value::Int(_) | Value::Double(_)) {
        return Err(cce_err(y, "java.lang.Number"));
    }
    match (x, y) {
        (Value::Str(a), Value::Str(b)) => {
            return Ok(compare_utf16(a, b));
        }
        (Value::Char(a), Value::Char(b)) => return Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => return Ok(a.cmp(b)),
        (Value::Keyword(a), Value::Keyword(b)) => return Ok(compare_named(a, b)),
        (Value::Symbol(a), Value::Symbol(b)) => return Ok(compare_named(a, b)),
        (Value::Vector(a), Value::Vector(b)) => {
            let mut ai = a.iter();
            let mut bi = b.iter();
            loop {
                match (ai.next(), bi.next()) {
                    (None, None) => return Ok(Ordering::Equal),
                    (None, _) => return Ok(Ordering::Less),
                    (_, None) => return Ok(Ordering::Greater),
                    (Some(u), Some(v)) => match jvm_compare(u, v)? {
                        Ordering::Equal => {}
                        ord => return Ok(ord),
                    },
                }
            }
        }
        _ => {}
    }
    // `x.compareTo(y)`: checkcast `y` to `x`'s class (vectors name
    // the interface); non-`Comparable` `x` fails itself.
    let target = match x {
        Value::Str(_) => "java.lang.String",
        Value::Char(_) => "java.lang.Character",
        Value::Bool(_) => "java.lang.Boolean",
        Value::Keyword(_) => "clojure.lang.Keyword",
        Value::Symbol(_) => "clojure.lang.Symbol",
        Value::Vector(_) => "clojure.lang.IPersistentVector",
        _ => return Err(cce_err(x, "java.lang.Comparable")),
    };
    Err(cce_err(y, target))
}

/// UTF-16 code-unit order (the JVM's `String.compareTo`).
fn compare_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Keyword/symbol order (namespace, then name — like the JVM's
/// `compareTo`, which orders the composite).
fn compare_named<A, B>(a: &A, b: &B) -> std::cmp::Ordering
where
    A: NamedParts,
    B: NamedParts,
{
    (a.ns(), a.nm()).cmp(&(b.ns(), b.nm()))
}

/// Namespace/name parts for ordering.
trait NamedParts {
    fn ns(&self) -> Option<&str>;
    fn nm(&self) -> &str;
}

impl NamedParts for Keyword {
    fn ns(&self) -> Option<&str> {
        self.namespace()
    }
    fn nm(&self) -> &str {
        self.name()
    }
}

impl NamedParts for Symbol {
    fn ns(&self) -> Option<&str> {
        self.namespace()
    }
    fn nm(&self) -> &str {
        self.name()
    }
}

fn native_compare(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/compare", args, &[2])?;
    Ok(Value::Int(match jvm_compare(&args[0], &args[1])? {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }))
}

/// A 1-arity predicate native.
macro_rules! pred1 {
    ($name:ident, $display:literal, $test:expr) => {
        fn $name(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
            check_arity($display, args, &[1])?;
            let test: fn(&Value) -> bool = $test;
            Ok(Value::Bool(test(&args[0])))
        }
    };
}

fn is_nil(v: &Value) -> bool {
    matches!(v, Value::Nil)
}
fn is_some_val(v: &Value) -> bool {
    !is_nil(v)
}
fn is_truthy_val(v: &Value) -> bool {
    v.is_truthy()
}
fn is_falsy_val(v: &Value) -> bool {
    !v.is_truthy()
}
fn is_true(v: &Value) -> bool {
    matches!(v, Value::Bool(true))
}
fn is_false(v: &Value) -> bool {
    matches!(v, Value::Bool(false))
}
fn is_boolean(v: &Value) -> bool {
    matches!(v, Value::Bool(_))
}
fn is_integer(v: &Value) -> bool {
    matches!(v, Value::Int(_))
}
fn is_double(v: &Value) -> bool {
    matches!(v, Value::Double(_))
}
fn is_number(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Double(_))
}
fn is_string(v: &Value) -> bool {
    matches!(v, Value::Str(_))
}
fn is_char(v: &Value) -> bool {
    matches!(v, Value::Char(_))
}
fn is_keyword(v: &Value) -> bool {
    matches!(v, Value::Keyword(_))
}
fn is_symbol(v: &Value) -> bool {
    matches!(v, Value::Symbol(_))
}
fn is_ident(v: &Value) -> bool {
    is_keyword(v) || is_symbol(v)
}
fn ident_ns(v: &Value) -> Option<Option<&str>> {
    match v {
        Value::Keyword(k) => Some(k.namespace()),
        Value::Symbol(s) => Some(s.namespace()),
        _ => None,
    }
}
fn is_qualified_ident(v: &Value) -> bool {
    ident_ns(v).is_some_and(|ns| ns.is_some())
}
fn is_simple_ident(v: &Value) -> bool {
    ident_ns(v).is_some_and(|ns| ns.is_none())
}
fn is_qualified_keyword(v: &Value) -> bool {
    matches!(v, Value::Keyword(k) if k.namespace().is_some())
}
fn is_simple_keyword(v: &Value) -> bool {
    matches!(v, Value::Keyword(k) if k.namespace().is_none())
}
fn is_qualified_symbol(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.namespace().is_some())
}
fn is_simple_symbol(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.namespace().is_none())
}
fn is_coll(v: &Value) -> bool {
    matches!(
        v,
        Value::List(_) | Value::Vector(_) | Value::Map(_) | Value::Set(_)
    ) || as_subvector(v).is_some()
        || is_seq_value(v)
}
fn is_sequential(v: &Value) -> bool {
    matches!(v, Value::Vector(_)) || as_subvector(v).is_some() || is_seq_value(v)
}
fn is_associative(v: &Value) -> bool {
    matches!(v, Value::Map(_) | Value::Vector(_)) || as_subvector(v).is_some()
}
pub(crate) fn is_counted(v: &Value) -> bool {
    matches!(
        v,
        Value::List(_) | Value::Vector(_) | Value::Map(_) | Value::Set(_)
    ) || crate::coll::is_transient(v)
        || as_subvector(v).is_some()
        || is_counted_seq(v)
}
fn is_list(v: &Value) -> bool {
    matches!(v, Value::List(_))
}
fn is_vector(v: &Value) -> bool {
    matches!(v, Value::Vector(_)) || as_subvector(v).is_some()
}
fn is_map(v: &Value) -> bool {
    matches!(v, Value::Map(_))
}
fn is_set(v: &Value) -> bool {
    matches!(v, Value::Set(_))
}
fn is_var(v: &Value) -> bool {
    matches!(v, Value::Var(_))
}
fn is_seqable(v: &Value) -> bool {
    matches!(
        v,
        Value::Nil
            | Value::Str(_)
            | Value::List(_)
            | Value::Vector(_)
            | Value::Map(_)
            | Value::Set(_)
    ) || as_subvector(v).is_some()
        || is_seq_value(v)
}
fn is_record(_v: &Value) -> bool {
    false
}

pred1!(native_nil, "clojure.core/nil?", is_nil);
pred1!(native_not, "clojure.core/not", is_falsy_val);
pred1!(native_boolean, "clojure.core/boolean", is_truthy_val);
pred1!(native_somep, "clojure.core/some?", is_some_val);
pred1!(native_truep, "clojure.core/true?", is_true);
pred1!(native_falsep, "clojure.core/false?", is_false);
pred1!(native_booleanp, "clojure.core/boolean?", is_boolean);
pred1!(native_integerp, "clojure.core/integer?", is_integer);
pred1!(native_intp, "clojure.core/int?", is_integer);
pred1!(native_floatp, "clojure.core/float?", is_double);
pred1!(native_doublep, "clojure.core/double?", is_double);
pred1!(native_numberp, "clojure.core/number?", is_number);
pred1!(native_stringp, "clojure.core/string?", is_string);
pred1!(native_charp, "clojure.core/char?", is_char);
pred1!(native_keywordp, "clojure.core/keyword?", is_keyword);
pred1!(native_symbolp, "clojure.core/symbol?", is_symbol);
pred1!(native_identp, "clojure.core/ident?", is_ident);
pred1!(
    native_qualified_identp,
    "clojure.core/qualified-ident?",
    is_qualified_ident
);
pred1!(
    native_simple_identp,
    "clojure.core/simple-ident?",
    is_simple_ident
);
pred1!(
    native_qualified_keywordp,
    "clojure.core/qualified-keyword?",
    is_qualified_keyword
);
pred1!(
    native_simple_keywordp,
    "clojure.core/simple-keyword?",
    is_simple_keyword
);
pred1!(
    native_qualified_symbolp,
    "clojure.core/qualified-symbol?",
    is_qualified_symbol
);
pred1!(
    native_simple_symbolp,
    "clojure.core/simple-symbol?",
    is_simple_symbol
);
pred1!(native_collp, "clojure.core/coll?", is_coll);
pred1!(
    native_sequentialp,
    "clojure.core/sequential?",
    is_sequential
);
pred1!(
    native_associativep,
    "clojure.core/associative?",
    is_associative
);
pred1!(native_countedp, "clojure.core/counted?", is_counted);
pred1!(native_listp, "clojure.core/list?", is_list);
pred1!(native_vectorp, "clojure.core/vector?", is_vector);
pred1!(native_mapp, "clojure.core/map?", is_map);
pred1!(native_setp, "clojure.core/set?", is_set);
pred1!(native_fnp, "clojure.core/fn?", is_fn_val);
pred1!(native_ifnp, "clojure.core/ifn?", is_ifn);
pred1!(native_varp, "clojure.core/var?", is_var);
pred1!(native_seqp, "clojure.core/seq?", is_seq_value);
pred1!(native_seqablep, "clojure.core/seqable?", is_seqable);
pred1!(native_recordp, "clojure.core/record?", is_record);

/// Printer options from the bound `*print-*` vars (`readably`
/// overrides the binding when given — `print` binds false, `pr`
/// honors).
/// A `*print-length*`/`*print-level*` binding as a printer limit
/// (fixprobe9): nil, negative, fractional, and NaN are unlimited
/// (`None`) — except a non-positive level, which prints `#`
/// (`Some(0)`, via dec-first); integral doubles act as that int;
/// anything else CCEs to `Number`.
fn print_limit(v: &Value, is_level: bool) -> Result<Option<usize>, NativeError> {
    match v {
        Value::Nil => Ok(None),
        Value::Int(n) => {
            if *n < 0 {
                Ok(if is_level { Some(0) } else { None })
            } else {
                Ok(Some(*n as usize))
            }
        }
        Value::Double(d) => {
            if d.is_nan() || d.fract() != 0.0 {
                Ok(None)
            } else if *d < 0.0 {
                Ok(if is_level { Some(0) } else { None })
            } else {
                Ok(Some(*d as usize))
            }
        }
        v => Err(cce_err(v, "java.lang.Number")),
    }
}

pub(crate) fn print_opts(
    eval: &Evaluator,
    readably: Option<bool>,
) -> Result<clj_value::PrintOpts, NativeError> {
    use crate::ns::deref_core;
    Ok(clj_value::PrintOpts {
        length: print_limit(&deref_core(eval, "*print-length*"), false)?,
        level: print_limit(&deref_core(eval, "*print-level*"), true)?,
        namespace_maps: deref_core(eval, "*print-namespace-maps*").is_truthy(),
        readably: readably.unwrap_or_else(|| deref_core(eval, "*print-readably*").is_truthy()),
        meta: deref_core(eval, "*print-meta*").is_truthy(),
    })
}

/// Whether the value is a JVM `LazySeq` (whose `toString` never
/// prints contents): a pending seq reporting the `LazySeq` class
/// (forced or not — memoizing never changes the class), or a
/// realized seq explicitly kinded `LazySeq`.
fn is_lazy_seq_str(v: &Value) -> bool {
    if as_pending(v).is_some() {
        return jvm_class_of(v) == "clojure.lang.LazySeq";
    }
    if let Some(s) = as_realized(v) {
        return s.kind() == clj_value::SeqKind::Lazy;
    }
    false
}

/// `str` of one value (also `%s`): nil vanishes; strings/chars are
/// raw; a `LazySeq` renders `clojure.lang.LazySeq@<hex>` with the
/// Java `List` content hash over the fully forced items (the hash
/// walk realizes, so infinite lazies hang, like the JVM — callers
/// take-bound first); every other seq object and strict collection
/// prints readably without limits (like `RT.printString`); anything
/// else prints plainly.
pub(crate) fn str_of_one(eval: &mut Evaluator, v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Nil => Ok(String::new()),
        Value::Str(s) => Ok(s.to_string()),
        Value::Char(c) => Ok(c.to_string()),
        _ if is_lazy_seq_str(v) => {
            let items = collect_all(eval, v).map_err(signal_to_native)?;
            let mut h: i32 = 1;
            for item in &items {
                let r = realize_deep(eval, item).map_err(signal_to_native)?;
                h = h.wrapping_mul(31).wrapping_add(clj_value::hash_code(&r));
            }
            Ok(format!("clojure.lang.LazySeq@{:x}", h as u32))
        }
        _ => {
            let r = realize_deep(eval, v).map_err(signal_to_native)?;
            Ok(match &r {
                Value::Nil => String::new(),
                Value::Str(s) => s.to_string(),
                Value::Char(c) => c.to_string(),
                Value::List(_)
                | Value::Vector(_)
                | Value::Map(_)
                | Value::Set(_)
                | Value::Extension(_) => clj_value::pr_to_string(&r, &clj_value::PrintOpts::pr()),
                _ => pr_to_string(&r, &PrintOpts::print()),
            })
        }
    }
}

/// `str`: concatenates the per-arg rendering (no spaces).
pub(crate) fn native_str(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let mut out = String::new();
    for arg in args {
        out.push_str(&str_of_one(eval, arg)?);
    }
    Ok(Value::Str(Arc::from(out.as_str())))
}

/// `subs` (char-indexed; the JVM's UTF-16 units agree on the BMP —
/// unpinned astral divergence is documented).
fn native_subs(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(NativeError::arity("clojure.core/subs", args.len()));
    }
    let s = match &args[0] {
        Value::Str(s) => s.to_string(),
        Value::Nil => {
            let method = if args.len() == 2 {
                "String.substring(int)"
            } else {
                "String.substring(int, int)"
            };
            return Err(NativeError::new(
                ThrowClass::NullPointerException,
                format!("Cannot invoke \"{method}\" because \"s\" is null"),
            ));
        }
        v => {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!(
                    "No matching method found: substring for class {}",
                    jvm_class_of(v)
                ),
            ));
        }
    };
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    // The `nth`/`subvec` index path: doubles truncate, nil trips the
    // `charValue` NPE, anything else fails to `Number`.
    let begin = i64::from(nth_index(&args[1])?);
    let end = if args.len() == 3 {
        i64::from(nth_index(&args[2])?)
    } else {
        len
    };
    if begin < 0 || end < begin || end > len {
        return Err(NativeError::new(
            ThrowClass::StringIndexOutOfBoundsException,
            format!("Range [{begin}, {end}) out of bounds for length {len}"),
        ));
    }
    Ok(Value::Str(Arc::from(
        chars[begin as usize..end as usize]
            .iter()
            .collect::<String>(),
    )))
}

/// `pr-str`/`print-str`: space-joined (nothing for zero args).
fn pr_print_str(
    eval: &mut Evaluator,
    args: &[Value],
    readably: Option<bool>,
) -> Result<Value, NativeError> {
    let opts = print_opts(eval, readably)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        let v = realize_deep(eval, arg).map_err(signal_to_native)?;
        parts.push(clj_value::pr_to_string(&v, &opts));
    }
    Ok(Value::Str(Arc::from(parts.join(" ").as_str())))
}

fn native_pr_str(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    // `pr` honors the `*print-readably*` binding (root true).
    pr_print_str(eval, args, None)
}

fn native_print_str(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    // `print` binds `*print-readably*` false (probed).
    pr_print_str(eval, args, Some(false))
}

/// Split `ns/name` on the first `/` (empty sides stay empty, like
/// the JVM's `split`).
fn split_ns_name(s: &str) -> (Option<&str>, &str) {
    match s.find('/') {
        Some(i) => (Some(&s[..i]), &s[i + 1..]),
        None => (None, s),
    }
}

fn native_keyword(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() > 2 {
        return Err(NativeError::arity("clojure.core/keyword", args.len()));
    }
    if args.len() == 2 {
        let ns = match &args[0] {
            Value::Str(s) => s.to_string(),
            v => return Err(cce_err(v, "java.lang.String")),
        };
        let name = match &args[1] {
            Value::Str(s) => s.to_string(),
            v => return Err(cce_err(v, "java.lang.String")),
        };
        return Ok(Value::Keyword(Keyword::new(
            Some(ns.as_str()),
            name.as_str(),
        )));
    }
    check_arity("clojure.core/keyword", args, &[1])?;
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::Keyword(k) => Ok(Value::Keyword(k.clone())),
        Value::Str(s) => {
            let (ns, name) = split_ns_name(s);
            Ok(Value::Keyword(Keyword::new(ns, name)))
        }
        Value::Symbol(s) => Ok(Value::Keyword(Keyword::new(s.namespace(), s.name()))),
        // Anything else is nil (`(keyword 5)` → nil, fixprobe8
        // `:kw-num` — only the 2-arg arity casts to `String`).
        _ => Ok(Value::Nil),
    }
}

fn native_symbol(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() > 2 {
        return Err(NativeError::arity("clojure.core/symbol", args.len()));
    }
    if args.len() == 2 {
        let ns = match &args[0] {
            Value::Str(s) => s.to_string(),
            v => return Err(cce_err(v, "java.lang.String")),
        };
        let name = match &args[1] {
            Value::Str(s) => s.to_string(),
            v => return Err(cce_err(v, "java.lang.String")),
        };
        return Ok(Value::Symbol(Symbol::new(Some(ns.as_str()), name.as_str())));
    }
    check_arity("clojure.core/symbol", args, &[1])?;
    match &args[0] {
        Value::Str(s) => {
            let (ns, name) = split_ns_name(s);
            Ok(Value::Symbol(Symbol::new(ns, name)))
        }
        Value::Keyword(k) => Ok(Value::Symbol(Symbol::new(k.namespace(), k.name()))),
        Value::Symbol(s) => Ok(Value::Symbol(s.clone())),
        _ => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "no conversion to symbol".to_owned(),
        )),
    }
}

/// The `Named` name (`name` works on strings too).
fn native_name(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/name", args, &[1])?;
    match &args[0] {
        Value::Str(s) => Ok(Value::Str(Arc::clone(s))),
        Value::Keyword(k) => Ok(Value::Str(Arc::from(k.name()))),
        Value::Symbol(s) => Ok(Value::Str(Arc::from(s.name()))),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null".to_owned(),
        )),
        v => Err(cce_err(v, "clojure.lang.Named")),
    }
}

/// The `Named` namespace (nil for simple names; strings fail —
/// only `name` accepts them).
fn native_namespace(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/namespace", args, &[1])?;
    match &args[0] {
        Value::Keyword(k) => Ok(k
            .namespace()
            .map_or(Value::Nil, |ns| Value::Str(Arc::from(ns)))),
        Value::Symbol(s) => Ok(s
            .namespace()
            .map_or(Value::Nil, |ns| Value::Str(Arc::from(ns)))),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null".to_owned(),
        )),
        v => Err(cce_err(v, "clojure.lang.Named")),
    }
}

/// The `charValue` NPE (`int` of nil — `int` falls back to the
/// `Character` cast, fixprobe10 `:int-nil`).
fn npe_char_value() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null".to_owned(),
    )
}

/// The `doubleValue` NPE (`long`/`double`/`float`/`short`/`byte`
/// of nil).
fn npe_double_value() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null".to_owned(),
    )
}

/// The `longValue` NPE (`char` of nil).
fn npe_long_value() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"java.lang.Number.longValue()\" because \"x\" is null".to_owned(),
    )
}

/// `IAE | Value out of range for {kind}: {x}` — doubles render
/// via `Double.toString`, ints raw (fixprobe10 `:int-huge`).
fn out_of_range(kind: &str, n: Num) -> NativeError {
    let x = match n {
        Num::Int(i) => i.to_string(),
        Num::Double(d) => clj_value::java_double_string(d),
    };
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Value out of range for {kind}: {x}"),
    )
}

fn native_int(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/int", args, &[1])?;
    // `int` keeps the value a `Long` (no 32-bit ints exist), but
    // checks the int range exactly: longs past it are `AE |
    // integer overflow`, doubles past it `IAE | Value out of
    // range for int` (checked BEFORE truncating), `NaN` is 0,
    // and characters convert (fixprobe10/11).
    Ok(Value::Int(match &args[0] {
        Value::Int(i) => int_exact_long(*i).map(i64::from)?,
        Value::Double(d) => {
            if d.is_nan() {
                0
            } else if *d > f64::from(i32::MAX) || *d < f64::from(i32::MIN) {
                return Err(out_of_range("int", Num::Double(*d)));
            } else {
                d.trunc() as i64
            }
        }
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_char_value()),
        v => return Err(cce_err(v, "java.lang.Character")),
    }))
}

fn native_long(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/long", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Int(i) => *i,
        Value::Double(d) => {
            if d.is_nan() {
                0
            } else if *d >= 9223372036854775808.0 || *d < -9223372036854775808.0 {
                return Err(out_of_range("long", Num::Double(*d)));
            } else {
                d.trunc() as i64
            }
        }
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_double(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/double", args, &[1])?;
    Ok(Value::Double(match &args[0] {
        Value::Int(i) => *i as f64,
        Value::Double(d) => *d,
        Value::Nil => return Err(npe_double_value()),
        // Characters rejected (fixprobe11 `:dbl-ch`).
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_float(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/float", args, &[1])?;
    // No `Float` exists: the float range is checked, then the
    // EXACT double is returned (no `f32` rounding — printing and
    // simple values match; the class/precision gap is documented).
    Ok(Value::Double(match &args[0] {
        Value::Int(i) => *i as f64,
        Value::Double(d) => {
            // `NaN` falls through (all comparisons false).
            if *d > f64::from(f32::MAX) || *d < -f64::from(f32::MAX) {
                return Err(out_of_range("float", Num::Double(*d)));
            }
            *d
        }
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_short(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/short", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Int(i) => {
            if *i > 32767 || *i < -32768 {
                return Err(out_of_range("short", Num::Int(*i)));
            }
            *i
        }
        Value::Double(d) => {
            if d.is_nan() || *d > 32767.0 || *d < -32768.0 {
                return Err(out_of_range("short", Num::Double(*d)));
            }
            d.trunc() as i64
        }
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_byte(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/byte", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Int(i) => {
            if *i > 127 || *i < -128 {
                return Err(out_of_range("byte", Num::Int(*i)));
            }
            *i
        }
        Value::Double(d) => {
            if d.is_nan() || *d > 127.0 || *d < -128.0 {
                return Err(out_of_range("byte", Num::Double(*d)));
            }
            d.trunc() as i64
        }
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_char(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/char", args, &[1])?;
    Ok(Value::Char(match &args[0] {
        Value::Char(c) => *c,
        Value::Int(i) => match char::from_u32(u32::try_from(*i).unwrap_or(u32::MAX)) {
            // Surrogates are unrepresentable (documented
            // divergence — the JVM accepts them, unprintably).
            Some(c) => c,
            None => return Err(out_of_range("char", Num::Int(*i))),
        },
        Value::Double(d) => {
            if d.is_nan() || *d > 1114111.0 || *d < 0.0 {
                return Err(out_of_range("char", Num::Double(*d)));
            }
            match char::from_u32(d.trunc() as u32) {
                Some(c) => c,
                None => return Err(out_of_range("char", Num::Double(*d))),
            }
        }
        Value::Nil => return Err(npe_long_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

/// A `reduced` wrapper: `reduce` unwraps it for early exit.
/// Identity equality (the JVM never overrides `equals` here).
#[derive(Debug)]
pub struct Reduced {
    id: u64,
    pub value: Value,
}

/// Fresh `Reduced` identities (print-only; never compared).
static REDUCED_ID: AtomicU64 = AtomicU64::new(1);

impl Reduced {
    fn wrap(value: Value) -> Value {
        Value::Extension(Arc::new(Self {
            id: REDUCED_ID.fetch_add(1, Ordering::Relaxed),
            value,
        }))
    }
}

/// Unwrap a `reduced` value, if any (for `reduce`).
pub(crate) fn as_reduced(v: &Value) -> Option<&Value> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<Reduced>().map(|r| &r.value),
        _ => None,
    }
}

impl ExtensionValue for Reduced {
    fn class_name(&self) -> String {
        "clojure.lang.Reduced".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[clojure.lang.Reduced 0x{:x} \"clojure.lang.Reduced@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn native_reduced(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reduced", args, &[1])?;
    Ok(Reduced::wrap(args[0].clone()))
}

fn native_reducedp(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/reduced?", args, &[1])?;
    Ok(Value::Bool(as_reduced(&args[0]).is_some()))
}

/// `ex-info`: an `ExceptionInfo` value (nil messages stay nil).
fn native_ex_info(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(NativeError::arity("clojure.core/ex-info", args.len()));
    }
    let message = match &args[0] {
        Value::Nil => None,
        Value::Str(s) => Some(Arc::clone(s)),
        v => return Err(cce_err(v, "java.lang.String")),
    };
    let data = match &args[1] {
        Value::Nil => None,
        Value::Map(m) => Some(Box::new(Value::Map(m.clone()))),
        v => return Err(cce_err(v, "clojure.lang.IPersistentMap")),
    };
    let cause = if args.len() == 3 {
        match &args[2] {
            Value::Nil => None,
            Value::Throwable(t) => Some(Box::new(t.clone())),
            v => return Err(cce_err(v, "java.lang.Throwable")),
        }
    } else {
        None
    };
    Ok(Value::Throwable(Throwable {
        id: eval.next_id(),
        class: ThrowClass::ExceptionInfo,
        message,
        data,
        cause,
    }))
}

/// `ex-data`/`ex-message`/`ex-cause`: throwables project, anything
/// else (including nil) is nil.
fn native_ex_data(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ex-data", args, &[1])?;
    Ok(match &args[0] {
        Value::Throwable(t) => t.data.as_deref().cloned().unwrap_or(Value::Nil),
        _ => Value::Nil,
    })
}

fn native_ex_message(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ex-message", args, &[1])?;
    Ok(match &args[0] {
        Value::Throwable(t) => t
            .message
            .as_ref()
            .map_or(Value::Nil, |m| Value::Str(Arc::clone(m))),
        _ => Value::Nil,
    })
}

fn native_ex_cause(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ex-cause", args, &[1])?;
    Ok(match &args[0] {
        Value::Throwable(t) => t
            .cause
            .as_ref()
            .map_or(Value::Nil, |c| Value::Throwable((**c).clone())),
        _ => Value::Nil,
    })
}

/// `trampoline`: call, then bounce zero-arg while the result is a
/// `fn?` (the initial call takes arguments; anything non-`IFn` is
/// the `IFn` cast failure).
fn native_trampoline(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/trampoline", 0));
    }
    check_ifn(eval, &args[0]).map_err(signal_to_native)?;
    let mut result = eval.call(&args[0], &args[1..]).map_err(signal_to_native)?;
    while is_fn_val(&result) {
        result = eval.call(&result, &[]).map_err(signal_to_native)?;
    }
    Ok(result)
}

/// `hash`: `hasheq` after a full realize (like the JVM walk).
/// Unbounded `Repeat`/`Range`, zero-step `Range`, any
/// `Cycle`/`Iterate` — top-level, nested, or revealed by forcing —
/// throw bare `UnsupportedOperationException` (the JVM `hasheq`
/// overrides); forcing a genuinely infinite lazy thunk hangs, also
/// like the JVM.
fn native_hash(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/hash", args, &[1])?;
    if clj_value::seq_hash_unsupported(&args[0]) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    let v = realize_deep(eval, &args[0]).map_err(signal_to_native)?;
    if clj_value::seq_hash_unsupported(&v) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    Ok(Value::Int(i64::from(clj_value::hasheq(&v))))
}

/// `hash-code` (PRIVATE, port-internal): Java `.hashCode` for the
/// `case` macro's dispatch tables (`Util/hash`, not murmur —
/// `case-map`/`merge-hash-collisions` call it where the JVM calls
/// `clojure.lang.Util/hash`). Same realize + infinite-UOE contract
/// as `hash` (the JVM `hashCode` overrides throw too).
fn native_hash_code(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/hash-code", args, &[1])?;
    if clj_value::seq_hash_unsupported(&args[0]) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    let v = realize_deep(eval, &args[0]).map_err(signal_to_native)?;
    if clj_value::seq_hash_unsupported(&v) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    Ok(Value::Int(i64::from(clj_value::hash_code(&v))))
}

/// `throw-illegal-arg!` (PRIVATE, port-internal): `core.clj` macros
/// have no `new`, so `cond`/`assert-args`/`defn`/`for`/`case`/
/// `condp` expansion and no-match failures throw through here to keep
/// the exact JVM `IllegalArgumentException` class.
fn native_throw_iae(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/throw-illegal-arg!", args, &[1])?;
    let Value::Str(s) = native_str(eval, args)? else {
        unreachable!("str always returns a string")
    };
    Err(NativeError::new(
        ThrowClass::IllegalArgumentException,
        s.to_string(),
    ))
}

/// `throw-npe!` (PRIVATE, port-internal): `doseq`'s invalid-keyword
/// path throws the JVM's `steppair` `NullPointerException` verbatim
/// (a JVM bug the corpus pins — `doseq` never validates modifiers).
fn native_throw_npe(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/throw-npe!", args, &[1])?;
    let Value::Str(s) = native_str(eval, args)? else {
        unreachable!("str always returns a string")
    };
    Err(NativeError::new(
        ThrowClass::NullPointerException,
        s.to_string(),
    ))
}

/// `odd?`/`even?`: the low bit of an `Int` (two's complement, so
/// negatives work); anything else is `Argument must be an integer:
/// <str>`.
fn native_odd(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/odd?", args, &[1])?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Bool((n & 1) == 1)),
        v => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Argument must be an integer: {}", crate::ns::str_of(v)),
        )),
    }
}

fn native_even(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/even?", args, &[1])?;
    match &args[0] {
        Value::Int(n) => Ok(Value::Bool((n & 1) == 0)),
        v => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Argument must be an integer: {}", crate::ns::str_of(v)),
        )),
    }
}

/// The S5D scalar natives installed into `clojure.core`.
pub(crate) const CORE_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("+", native_add),
    ("-", native_sub),
    ("*", native_mul),
    ("/", native_div),
    ("quot", native_quot),
    ("rem", native_rem),
    ("mod", native_mod),
    ("inc", native_inc),
    ("inc'", native_inc_prime),
    ("dec", native_dec),
    ("max", native_max),
    ("min", native_min),
    ("=", native_eq),
    ("not=", native_not_eq),
    ("==", native_num_eq),
    ("<", native_lt),
    (">", native_gt),
    ("<=", native_lte),
    (">=", native_gte),
    ("zero?", native_zero),
    ("pos?", native_pos),
    ("neg?", native_neg),
    ("compare", native_compare),
    ("nil?", native_nil),
    ("not", native_not),
    ("boolean", native_boolean),
    ("some?", native_somep),
    ("true?", native_truep),
    ("false?", native_falsep),
    ("boolean?", native_booleanp),
    ("integer?", native_integerp),
    ("int?", native_intp),
    ("float?", native_floatp),
    ("double?", native_doublep),
    ("number?", native_numberp),
    ("string?", native_stringp),
    ("char?", native_charp),
    ("keyword?", native_keywordp),
    ("symbol?", native_symbolp),
    ("ident?", native_identp),
    ("qualified-ident?", native_qualified_identp),
    ("simple-ident?", native_simple_identp),
    ("qualified-keyword?", native_qualified_keywordp),
    ("simple-keyword?", native_simple_keywordp),
    ("qualified-symbol?", native_qualified_symbolp),
    ("simple-symbol?", native_simple_symbolp),
    ("coll?", native_collp),
    ("sequential?", native_sequentialp),
    ("associative?", native_associativep),
    ("counted?", native_countedp),
    ("list?", native_listp),
    ("vector?", native_vectorp),
    ("map?", native_mapp),
    ("set?", native_setp),
    ("fn?", native_fnp),
    ("ifn?", native_ifnp),
    ("var?", native_varp),
    ("seq?", native_seqp),
    ("seqable?", native_seqablep),
    ("record?", native_recordp),
    ("str", native_str),
    ("subs", native_subs),
    ("pr-str", native_pr_str),
    ("print-str", native_print_str),
    ("keyword", native_keyword),
    ("symbol", native_symbol),
    ("name", native_name),
    ("namespace", native_namespace),
    ("int", native_int),
    ("long", native_long),
    ("double", native_double),
    ("float", native_float),
    ("short", native_short),
    ("byte", native_byte),
    ("char", native_char),
    ("format", crate::fmt::native_format),
    ("reduced", native_reduced),
    ("reduced?", native_reducedp),
    ("ex-info", native_ex_info),
    ("ex-data", native_ex_data),
    ("ex-message", native_ex_message),
    ("ex-cause", native_ex_cause),
    ("trampoline", native_trampoline),
    ("hash", native_hash),
    ("hash-code", native_hash_code),
    ("throw-illegal-arg!", native_throw_iae),
    ("throw-npe!", native_throw_npe),
    ("odd?", native_odd),
    ("even?", native_even),
];
