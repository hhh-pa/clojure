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

use num_traits::{ToPrimitive, Zero};

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
        Value::Double(d) => Ok(take_double(*d)),
        // Tower counts like `double` counts (the JVM `pos?`/`dec`
        // loop behaves as `ceil`: oracle `index-tower`,
        // `index-big` with its `1.0E20` rendering).
        Value::BigInt(n) => Ok(take_double(n.to_f64().unwrap_or(f64::NAN))),
        Value::Ratio(r) => Ok(take_double(r.to_f64().unwrap_or(f64::NAN))),
        Value::BigDec(d) => Ok(take_double(d.to_f64())),
        Value::Nil => Err(npe_getclass()),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// A `take`/`drop` count from a `double`: `NaN` takes nothing,
/// infinities saturate, the rest rounds up.
fn take_double(d: f64) -> i64 {
    if d.is_nan() {
        0
    } else if d >= i64::MAX as f64 {
        i64::MAX
    } else if d <= i64::MIN as f64 {
        i64::MIN
    } else {
        d.ceil() as i64
    }
}

/// A `repeat` count: `intValue` truncation (`NaN` is zero,
/// infinities saturate; `nil` trips the `Repeat.create`
/// `doubleValue` NPE, anything else fails to `Number`).
pub(crate) fn repeat_count(v: &Value) -> Result<i64, NativeError> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Double(d) => repeat_double(*d),
        // Tower truncates like `intValue`, through the same
        // range check (the drop-path `double` rendering stays).
        Value::BigInt(n) => repeat_double(n.to_f64().unwrap_or(f64::NAN)),
        Value::Ratio(r) => repeat_double(r.to_f64().unwrap_or(f64::NAN)),
        Value::BigDec(d) => repeat_double(d.to_f64()),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null".to_owned(),
        )),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// A `repeat` count from a `double`: `intValue` truncation
/// (`NaN` is zero, past-`i64` fails `out of range for long`).
fn repeat_double(d: f64) -> Result<i64, NativeError> {
    if d.is_nan() {
        Ok(0)
    } else if d > i64::MAX as f64 || d < i64::MIN as f64 {
        Err(out_of_range("long", Num::Double(d)))
    } else {
        Ok(d as i64)
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
    // `drop`/`nthrest` validate `n` first, so only numbers arrive;
    // `as_num` keeps the identical failures total (arms covered).
    // Tower counts like `double` counts (the JVM `pos?`/`dec`
    // loop behaves as `ceil`, and huge counts render `1.0E20`:
    // oracle `index-tower`, `index-big`).
    match as_num(v)? {
        Num::Int(i) => int_exact_long(i),
        Num::Double(d) => int_cast_double(d.ceil()),
        Num::BigInt(n) => int_cast_double(n.to_f64().unwrap_or(f64::NAN).ceil()),
        Num::Ratio(r) => int_cast_double(r.to_f64().unwrap_or(f64::NAN).ceil()),
        Num::BigDec(d) => int_cast_double(d.to_f64().ceil()),
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
        // `nth` truncates toward zero first, then takes the `long`
        // range check (exact digits) and `toIntExact` (oracle
        // `nth-trunc2`, `nth-big`, `nth-intrange`); decimals
        // narrow through `double` first (like `int`).
        Value::BigInt(n) => tower_intcast(n),
        Value::Ratio(r) => tower_intcast(&trunc_ratio(r)),
        Value::BigDec(d) => bigdec_intcast(d),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// An `RT.intCast` index from a truncated tower integer:
/// past-`i64` fails `Value out of range for long` with the exact
/// digits, past-`i32` fails `integer overflow` (oracles
/// `nth-big`, `nth-intrange`, `conv-hugeint` — shared by `int`,
/// `nth`, and `subvec`).
fn tower_intcast(n: &num_bigint::BigInt) -> Result<i32, NativeError> {
    let l = n
        .to_i64()
        .ok_or_else(|| out_of_range("long", Num::BigInt(n.clone())))?;
    int_exact_long(l)
}

/// An `RT.intCast` index from a decimal: through the `long`
/// range check first (double rendering), then `toIntExact`
/// (oracle `conv-hugedecint`).
fn bigdec_intcast(d: &clj_value::BigDec) -> Result<i32, NativeError> {
    int_exact_long(double_long(d.to_f64())?)
}

/// A `long` from a `double`: `NaN` is zero, past-`i64` fails
/// `Value out of range for long` (double rendering), else
/// truncation toward zero.
pub(crate) fn double_long(d: f64) -> Result<i64, NativeError> {
    if d.is_nan() {
        Ok(0)
    } else if d >= 9223372036854775808.0 || d < -9223372036854775808.0 {
        Err(out_of_range("long", Num::Double(d)))
    } else {
        Ok(d.trunc() as i64)
    }
}

/// A `long` from a tower integer: past-`i64` fails `Value out of
/// range for long` with the exact digits (`short`/`byte` narrow
/// through here, so `(short 2^63N)` names `long`; oracle
/// `tower-r2e`).
/// Narrow a `long` into a smaller-width coercion target.
fn narrow_range(l: i64, lo: i64, hi: i64, kind: &'static str) -> Result<i64, NativeError> {
    if l < lo || l > hi {
        Err(out_of_range(kind, Num::Int(l)))
    } else {
        Ok(l)
    }
}

/// Narrow any number into a smaller-width coercion target:
/// doubles range-check directly (double rendering), unbounded
/// integers go through the `long` stop first, decimals through
/// `double` like `int` (oracles `tower-r2e`, `tower-r2f`).
fn narrow_num(n: &Num, lo: i64, hi: i64, kind: &'static str) -> Result<i64, NativeError> {
    match n {
        Num::Double(d) => {
            if d.is_nan() || *d > hi as f64 || *d < lo as f64 {
                return Err(out_of_range(kind, Num::Double(*d)));
            }
            Ok(d.trunc() as i64)
        }
        Num::Int(i) => narrow_range(*i, lo, hi, kind),
        Num::BigInt(n) => narrow_range(bigint_long(n)?, lo, hi, kind),
        Num::Ratio(r) => narrow_range(bigint_long(&trunc_ratio(r))?, lo, hi, kind),
        Num::BigDec(d) => narrow_range(double_long(d.to_f64())?, lo, hi, kind),
    }
}

/// Java `intValue`: low 32 bits, sign-extended (`BigDecimal`
/// `(char 1E300M)` needs the NUL, not a range error; oracle
/// `tower-r2f`).
fn bigint_int_value(n: &num_bigint::BigInt) -> i32 {
    let mut le = n.to_signed_bytes_le();
    le.resize(
        4,
        if n.sign() == num_bigint::Sign::Minus {
            0xFF
        } else {
            0
        },
    );
    i32::from_le_bytes([le[0], le[1], le[2], le[3]])
}

/// A `char` from an unbounded integer: the full value
/// range-checks (never the `long` stop — oracle `tower-r2g`),
/// then UTF-32 narrows (surrogates stay the documented
/// divergence).
fn char_from_bigint(n: &num_bigint::BigInt) -> Result<char, NativeError> {
    match n
        .to_u32()
        .filter(|&u| u <= 1114111)
        .and_then(char::from_u32)
    {
        Some(c) => Ok(c),
        None => Err(out_of_range("char", Num::BigInt(n.clone()))),
    }
}

/// A `char` from a decimal: `BigDecimal.intValue` first, then the
/// `char` range check.
fn char_from_bigdec(d: &clj_value::BigDec) -> Result<char, NativeError> {
    let i = bigint_int_value(&d.to_bigint_trunc());
    if !(0..=1114111).contains(&i) {
        return Err(out_of_range("char", Num::Int(i64::from(i))));
    }
    match char::from_u32(i as u32) {
        Some(c) => Ok(c),
        None => Err(out_of_range("char", Num::Int(i64::from(i)))),
    }
}

/// A `char` from any number: unbounded integers range-check
/// whole (never the `long` stop), ratios truncate first,
/// decimals go through `BigDecimal.intValue` (oracles
/// `tower-r2f`, `tower-r2g`).
fn char_of_num(n: &Num) -> Result<char, NativeError> {
    match n {
        Num::Int(i) => match char::from_u32(u32::try_from(*i).unwrap_or(u32::MAX)) {
            // Surrogates are unrepresentable (documented
            // divergence — the JVM accepts them, unprintably).
            Some(c) => Ok(c),
            None => Err(out_of_range("char", Num::Int(*i))),
        },
        Num::Double(d) => {
            if d.is_nan() || *d > 1114111.0 || *d < 0.0 {
                return Err(out_of_range("char", Num::Double(*d)));
            }
            match char::from_u32(d.trunc() as u32) {
                Some(c) => Ok(c),
                None => Err(out_of_range("char", Num::Double(*d))),
            }
        }
        Num::BigInt(n) => char_from_bigint(n),
        Num::Ratio(r) => char_from_bigint(&trunc_ratio(r)),
        Num::BigDec(d) => char_from_bigdec(d),
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
        | Value::MapEntry { .. }
        | Value::Map(_)
        | Value::Set(_)
        | Value::Var(_)
        | Value::Promise(_)
        | Value::MultiFn(_)
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
/// non-numeric the `Number` cast failure). Tower ranks for
/// promotion: `Int` < `BigInt` < `Ratio` < `BigDec` < `Double`
/// (oracle-pinned: `(+ 1/2 5N)` is a `Ratio`, `(+ 5N 5M)` a
/// `BigDec`, `(+ 5M 0.5)` a `Double`).
#[derive(Clone)]
pub(crate) enum Num {
    Int(i64),
    BigInt(num_bigint::BigInt),
    Ratio(num_rational::BigRational),
    BigDec(clj_value::BigDec),
    Double(f64),
}

pub(crate) fn as_num(v: &Value) -> Result<Num, NativeError> {
    match v {
        Value::Int(i) => Ok(Num::Int(*i)),
        Value::BigInt(n) => Ok(Num::BigInt(n.clone())),
        Value::Ratio(r) => Ok(Num::Ratio(r.clone())),
        Value::BigDec(d) => Ok(Num::BigDec(d.clone())),
        Value::Double(d) => Ok(Num::Double(*d)),
        Value::Nil => Err(npe_getclass()),
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

fn num_value(n: Num) -> Value {
    match n {
        Num::Int(i) => Value::Int(i),
        Num::BigInt(n) => Value::BigInt(n),
        Num::Ratio(r) => normalize_rational(r),
        Num::BigDec(d) => Value::BigDec(d),
        Num::Double(d) => Value::Double(d),
    }
}

/// A rational with denominator 1 is a `BigInt` (the reader and
/// `/` rely on this as well as the ops below).
fn normalize_rational(r: num_rational::BigRational) -> Value {
    if r.denom() == &num_bigint::BigInt::from(1) {
        Value::BigInt(r.numer().clone())
    } else {
        Value::Ratio(r)
    }
}

/// Promotion rank (`Double` > `BigDec` > `Ratio` > `BigInt` > `Int`).
fn num_rank(n: &Num) -> u8 {
    match n {
        Num::Int(_) => 0,
        Num::BigInt(_) => 1,
        Num::Ratio(_) => 2,
        Num::BigDec(_) => 3,
        Num::Double(_) => 4,
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

/// Promoted `+`: `Int`/`Int` keeps the checked errors;
/// otherwise the top rank present computes (`Double` > `BigDec` >
/// `Ratio` > `BigInt`, oracle `promo1`).
fn add(a: Num, b: Num) -> Result<Num, NativeError> {
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => x.checked_add(*y).map(Num::Int).ok_or_else(overflow),
        _ if either_double(&a, &b) => Ok(Num::Double(to_f64(a) + to_f64(b))),
        _ if num_rank(&a).max(num_rank(&b)) == 3 => {
            Ok(Num::BigDec(num_bigdec(&a)?.add(&num_bigdec(&b)?)))
        }
        _ => Ok(num_ratio(num_rational(&a) + num_rational(&b))),
    }
}

/// Promoted `-` (same lattice as [`add`]).
fn sub(a: Num, b: Num) -> Result<Num, NativeError> {
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => x.checked_sub(*y).map(Num::Int).ok_or_else(overflow),
        _ if either_double(&a, &b) => Ok(Num::Double(to_f64(a) - to_f64(b))),
        _ if num_rank(&a).max(num_rank(&b)) == 3 => {
            Ok(Num::BigDec(num_bigdec(&a)?.sub(&num_bigdec(&b)?)))
        }
        _ => Ok(num_ratio(num_rational(&a) - num_rational(&b))),
    }
}

/// Promoted `*` (same lattice as [`add`]).
fn mul(a: Num, b: Num) -> Result<Num, NativeError> {
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => x.checked_mul(*y).map(Num::Int).ok_or_else(overflow),
        _ if either_double(&a, &b) => Ok(Num::Double(to_f64(a) * to_f64(b))),
        _ if num_rank(&a).max(num_rank(&b)) == 3 => {
            Ok(Num::BigDec(num_bigdec(&a)?.mul(&num_bigdec(&b)?)))
        }
        _ => Ok(num_ratio(num_rational(&a) * num_rational(&b))),
    }
}

/// Promoted `/`: `Int`/`Int` keeps the wedge (exact when divisible,
/// else the `Double` division — C1 documents the ratio
/// deviation); tower division is exact (`(/ 5N 2)` → `5/2`, a
/// non-terminating decimal fails like `(bigdec 1/3)`).
fn div(a: Num, b: Num) -> Result<Num, NativeError> {
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => {
            if *y == 0 {
                return Err(divide_by_zero());
            }
            if x % y == 0 {
                x.checked_div(*y).map(Num::Int).ok_or_else(overflow)
            } else {
                // Inexact `Int` division is a reduced ratio, never a
                // `Double` (oracle `promo-mul`: `(/ 1 2)` is `1/2`).
                Ok(num_ratio(num_rational(&a) / num_rational(&b)))
            }
        }
        _ if either_double(&a, &b) => Ok(Num::Double(to_f64(a) / to_f64(b))),
        _ if num_rank(&a).max(num_rank(&b)) == 3 => num_bigdec(&a)?
            .div_exact(&num_bigdec(&b)?)
            .map(Num::BigDec)
            .map_err(|m| {
                NativeError::new(clj_value::ThrowClass::ArithmeticException, m.to_owned())
            }),
        _ => {
            let (x, y) = (num_rational(&a), num_rational(&b));
            if y.numer().is_zero() {
                return Err(divide_by_zero());
            }
            Ok(num_ratio(x / y))
        }
    }
}

/// A left-associative variadic fold (`+`/`-`/`*`/`/`; every caller
/// rejects empty args first).
fn fold(
    args: &[Value],
    op: fn(Num, Num) -> Result<Num, NativeError>,
) -> Result<Value, NativeError> {
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
    fold(args, add)
}

fn native_sub(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/-", 0));
    }
    if args.len() == 1 {
        return match as_num(&args[0])? {
            Num::Int(x) => x.checked_neg().map(Value::Int).ok_or_else(overflow),
            Num::BigInt(x) => Ok(Value::BigInt(-x)),
            Num::Ratio(x) => Ok(num_value(num_ratio(-x))),
            Num::BigDec(x) => Ok(Value::BigDec(x.neg())),
            Num::Double(x) => Ok(Value::Double(-x)),
        };
    }
    fold(args, sub)
}

fn native_mul(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Ok(Value::Int(1));
    }
    fold(args, mul)
}

fn native_div(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core//", 0));
    }
    if args.len() == 1 {
        return div(Num::Int(1), as_num(&args[0])?).map(num_value);
    }
    fold(args, div)
}

fn native_quot(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/quot", args, &[2])?;
    let (a, b) = (as_num(&args[0])?, as_num(&args[1])?);
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => {
            if *y == 0 {
                return Err(slash_by_zero());
            }
            x.checked_div(*y).map(Value::Int).ok_or_else(overflow)
        }
        _ if either_double(&a, &b) => quot_double(to_f64(a), to_f64(b)),
        // Tower truncating division (oracle `quotrem`/`quotmix`/
        // `quotdec`): any tower zero divisor is `Divide by zero`
        // (unlike the `Int` `/ by zero` wedge, which stays).
        _ => quot_tower(&a, &b),
    }
}

/// `quot` over a `Double` pair: zero first (even over `NaN`,
/// `:quot-nan-0`), then finiteness (either side, `:quot-nan-div`),
/// then the quotient's own finiteness (`:quot-ovf`); `-0.0`
/// normalizes to `+0.0` (`:quot-negfrac`, `:quot-2-neginf`).
fn quot_double(x: f64, y: f64) -> Result<Value, NativeError> {
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

/// `quot` over tower kinds: exact truncating division.
fn quot_tower(a: &Num, b: &Num) -> Result<Value, NativeError> {
    if num_rational(b).numer().is_zero() {
        return Err(divide_by_zero());
    }
    if num_rank(a).max(num_rank(b)) == 3 {
        Ok(Value::BigDec(num_bigdec(a)?.quot_trunc(&num_bigdec(b)?)))
    } else {
        let (x, y) = (num_rational(a), num_rational(b));
        Ok(num_value(num_ratio(trunc_div(&x, &y))))
    }
}

/// Truncating rational division toward zero (the `quot` rule for
/// `BigInt`/`Ratio` operands).
fn trunc_div(
    x: &num_rational::BigRational,
    y: &num_rational::BigRational,
) -> num_rational::BigRational {
    let q = x.numer() * y.denom() / (x.denom() * y.numer());
    num_rational::BigRational::from_integer(q)
}

fn native_rem(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rem", args, &[2])?;
    let (a, b) = (as_num(&args[0])?, as_num(&args[1])?);
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => {
            if *y == 0 {
                return Err(slash_by_zero());
            }
            // `MIN % -1` is the only overflow (0, but checked).
            x.checked_rem(*y).map(Value::Int).ok_or_else(overflow)
        }
        _ if either_double(&a, &b) => rem_double(to_f64(a), to_f64(b)),
        // Tower remainder (oracle `dec-each`): exact, in the
        // dividend's kind (`(rem 5M 2)` → `1M`, `(rem 1/2 1)` →
        // `1/2`).
        _ => rem_tower(&a, &b),
    }
}

/// `rem` over a `Double` pair: zero first (`:rem-nan-0`), then
/// finiteness (`:rem-nan-num`); an infinite divisor is `NaN`, NOT
/// `x` (`:rem-num-inf`, `:rem-num-neginf`).
fn rem_double(x: f64, y: f64) -> Result<Value, NativeError> {
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

/// `rem` over tower kinds: exact, in the dividend's kind.
fn rem_tower(a: &Num, b: &Num) -> Result<Value, NativeError> {
    if num_rational(b).numer().is_zero() {
        return Err(divide_by_zero());
    }
    if num_rank(a).max(num_rank(b)) == 3 {
        Ok(Value::BigDec(num_bigdec(a)?.rem(&num_bigdec(b)?)))
    } else {
        let (x, y) = (num_rational(a), num_rational(b));
        let q = trunc_div(&x, &y);
        Ok(num_value(num_ratio(x - q * y)))
    }
}

fn native_mod(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/mod", args, &[2])?;
    let (a, b) = (as_num(&args[0])?, as_num(&args[1])?);
    match (&a, &b) {
        (Num::Int(x), Num::Int(y)) => {
            if *y == 0 {
                return Err(divide_by_zero());
            }
            let r = x.checked_rem(*y).ok_or_else(overflow)?;
            Ok(Value::Int(if r == 0 || (r > 0) == (*y > 0) {
                r
            } else {
                r.checked_add(*y).ok_or_else(overflow)?
            }))
        }
        _ if either_double(&a, &b) => mod_double(to_f64(a), to_f64(b)),
        // Tower modulus (oracle `dec-each`): the remainder
        // adjusted to the divisor's sign, in the dividend's kind.
        _ => mod_tower(&a, &b),
    }
}

/// `mod` over a `Double` pair: zero first (`:mod-nan-0`), then
/// finiteness (`:mod-nan-num`); an infinite divisor falls out of
/// the formula as `NaN` (`:mod-num-inf`).
fn mod_double(x: f64, y: f64) -> Result<Value, NativeError> {
    if y == 0.0 {
        return Err(divide_by_zero());
    }
    if !x.is_finite() || y.is_nan() {
        return Err(infinite_or_nan());
    }
    Ok(Value::Double(((x % y) + y) % y))
}

/// `mod` over tower kinds: the remainder adjusted to the
/// divisor's sign, in the dividend's kind.
fn mod_tower(a: &Num, b: &Num) -> Result<Value, NativeError> {
    if num_rational(b).numer().is_zero() {
        return Err(divide_by_zero());
    }
    if num_rank(a).max(num_rank(b)) == 3 {
        let (x, y) = (num_bigdec(a)?, num_bigdec(b)?);
        Ok(Value::BigDec(bigdec_mod(&x, &y)))
    } else {
        mod_rational(&num_rational(a), &num_rational(b))
    }
}

/// `mod` over two exact rationals (sign-adjusted remainder,
/// folded).
fn mod_rational(
    x: &num_rational::BigRational,
    y: &num_rational::BigRational,
) -> Result<Value, NativeError> {
    let q = trunc_div(x, y);
    let y_sign = y.numer().sign();
    let r = x - q * y.clone();
    Ok(num_value(num_ratio(
        if r.numer().is_zero() || r.numer().sign() == y_sign {
            r
        } else {
            r + y.clone()
        },
    )))
}

/// `BigDec` modulus: the remainder adjusted to the divisor's
/// sign (mirrors the `Int` formula above).
fn bigdec_mod(x: &clj_value::BigDec, y: &clj_value::BigDec) -> clj_value::BigDec {
    let r = x.rem(y);
    if r.is_zero() || r.sign() == y.sign() {
        r
    } else {
        r.add(y)
    }
}

/// A number as `f64` (mixed/double paths; tower narrows
/// approximately, infinities included).
fn to_f64(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::BigInt(n) => n.to_f64().unwrap_or(f64::NAN),
        Num::Ratio(r) => r.to_f64().unwrap_or(f64::NAN),
        Num::BigDec(d) => d.to_f64(),
        Num::Double(d) => d,
    }
}

/// An exact rational for a number. `Double` never reaches here
/// (doubles always take the `f64` path); the `expect` documents
/// that invariant coverably.
fn num_rational(n: &Num) -> num_rational::BigRational {
    match n {
        Num::Int(i) => num_rational::BigRational::from_integer(num_bigint::BigInt::from(*i)),
        Num::BigInt(n) => num_rational::BigRational::from_integer(n.clone()),
        Num::Ratio(r) => r.clone(),
        Num::BigDec(d) => d.to_rational(),
        Num::Double(_) => {
            unreachable!("doubles take the f64 path, never the exact one")
        }
    }
}

/// Exact ordering without needless expansion: `BigDec` against
/// `BigDec`/`Int`/`BigInt` decides by sign and magnitude first
/// (astronomical scales never expand — `compareTo` parity);
/// everything else keeps the rational path.
fn compare_exact(a: &Num, b: &Num) -> std::cmp::Ordering {
    match (a, b) {
        (Num::BigDec(d), Num::BigDec(e)) => d.compare(e),
        (Num::BigDec(d), Num::Int(i)) => d.compare(&clj_value::BigDec::from_bigint((*i).into())),
        (Num::Int(i), Num::BigDec(e)) => clj_value::BigDec::from_bigint((*i).into()).compare(e),
        (Num::BigDec(d), Num::BigInt(n)) => d.compare(&clj_value::BigDec::from_bigint(n.clone())),
        (Num::BigInt(n), Num::BigDec(d)) => clj_value::BigDec::from_bigint(n.clone()).compare(d),
        _ => num_rational(a).cmp(&num_rational(b)),
    }
}

/// An exact `BigDec` for a non-`Double` number (a `Ratio` with no
/// terminating expansion fails like `(bigdec 1/3)`).
fn num_bigdec(n: &Num) -> Result<clj_value::BigDec, NativeError> {
    match n {
        Num::Int(i) => Ok(clj_value::BigDec::from_bigint(num_bigint::BigInt::from(*i))),
        Num::BigInt(n) => Ok(clj_value::BigDec::from_bigint(n.clone())),
        Num::BigDec(d) => Ok(d.clone()),
        Num::Ratio(r) => clj_value::BigDec::from_rational_exact(r).map_err(|m| {
            NativeError::new(clj_value::ThrowClass::ArithmeticException, m.to_owned())
        }),
        Num::Double(_) => {
            unreachable!("rank dispatch keeps doubles out of BigDec ops")
        }
    }
}

/// A rational as a `Num` (denominator 1 folds to `BigInt`).
fn num_ratio(r: num_rational::BigRational) -> Num {
    if r.denom() == &num_bigint::BigInt::from(1) {
        Num::BigInt(r.numer().clone())
    } else {
        Num::Ratio(r)
    }
}

/// An `Ordering` as the `f64` pair the chained comparisons take
/// (`<`/`>`/`<=`/`>=` all read correctly off `(ord, 0.0)`).
fn ord_num(ord: std::cmp::Ordering) -> f64 {
    match ord {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }
}

/// The sign of a number (tower via the exact rational, whose
/// denominator is always positive; `Double` keeps its own arms
/// at the call sites).
fn num_sign(n: &Num) -> num_bigint::Sign {
    num_rational(n).numer().sign()
}

/// Whether either side is a `Double` (the approximate path).
fn either_double(a: &Num, b: &Num) -> bool {
    matches!(a, Num::Double(_)) || matches!(b, Num::Double(_))
}

/// The rational `1` (tower steps).
fn rat_unit() -> num_rational::BigRational {
    num_rational::BigRational::from_integer(num_bigint::BigInt::from(1))
}

/// One step (tower kinds keep their kind: `(inc 5M)` → `6M`,
/// `(inc 1/2)` → `3/2`; `Int` still overflows — the wedge).
fn inc_one(n: Num) -> Result<Value, NativeError> {
    match n {
        Num::Int(x) => x.checked_add(1).map(Value::Int).ok_or_else(overflow),
        Num::BigInt(x) => Ok(Value::BigInt(x + 1)),
        Num::Ratio(x) => Ok(num_value(num_ratio(x + rat_unit()))),
        Num::BigDec(x) => Ok(Value::BigDec(
            x.add(&clj_value::BigDec::from_bigint(num_bigint::BigInt::from(1))),
        )),
        Num::Double(x) => Ok(Value::Double(x + 1.0)),
    }
}

/// One step down (mirror of [`inc_one`]).
fn dec_one(n: Num) -> Result<Value, NativeError> {
    match n {
        Num::Int(x) => x.checked_sub(1).map(Value::Int).ok_or_else(overflow),
        Num::BigInt(x) => Ok(Value::BigInt(x - 1)),
        Num::Ratio(x) => Ok(num_value(num_ratio(x - rat_unit()))),
        Num::BigDec(x) => Ok(Value::BigDec(
            x.sub(&clj_value::BigDec::from_bigint(num_bigint::BigInt::from(1))),
        )),
        Num::Double(x) => Ok(Value::Double(x - 1.0)),
    }
}

fn native_inc(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/inc", args, &[1])?;
    inc_one(as_num(&args[0])?)
}

fn native_dec(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/dec", args, &[1])?;
    dec_one(as_num(&args[0])?)
}

/// `inc'`: auto-promoting increment — identical to `inc` here
/// (`Int` still overflows instead of growing to `BigInt`; the
/// wedge stays, tower kinds pass through like `inc`).
fn native_inc_prime(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/inc'", args, &[1])?;
    inc_one(as_num(&args[0])?)
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
        // incumbent, which always survives (`:max-nan1`). Tower
        // pairs without a `Double` compare exactly.
        let keep = match (&acc, &n) {
            (Num::Int(a), Num::Int(b)) => {
                if want_max {
                    a > b
                } else {
                    a < b
                }
            }
            _ if either_double(&acc, &n) => {
                let (a, b) = (to_f64(acc.clone()), to_f64(n.clone()));
                if a.is_nan() {
                    true
                } else if want_max {
                    a > b
                } else {
                    a < b
                }
            }
            _ => {
                let ord = compare_exact(&acc, &n);
                if want_max {
                    ord == std::cmp::Ordering::Greater
                } else {
                    ord == std::cmp::Ordering::Less
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
fn eq_bool(eval: &mut Evaluator, args: &[Value]) -> Result<bool, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/=", 0));
    }
    let mut first = &args[0];
    for arg in &args[1..] {
        if !equiv_vals(eval, first, arg).map_err(signal_to_native)? {
            return Ok(false);
        }
        first = arg;
    }
    Ok(true)
}

fn native_eq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    Ok(Value::Bool(eq_bool(eval, args)?))
}

fn native_not_eq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    Ok(Value::Bool(!eq_bool(eval, args)?))
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
        // `Int`/`Int` is exact; `Double` pairs compare as `f64`
        // (`NaN` never is); tower pairs compare exactly.
        let same = match (&prev, &n) {
            (Num::Int(a), Num::Int(b)) => a == b,
            _ if either_double(&prev, &n) => to_f64(prev.clone()) == to_f64(n.clone()),
            _ => num_rational(&prev) == num_rational(&n),
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
        // `Int`/`Int` uses the integer predicate; `Double` pairs
        // use the `f64` one (`NaN` orders false everywhere);
        // tower pairs compare exactly, read off `(ord, 0.0)`.
        let holds = match (&prev, &n) {
            (Num::Int(a), Num::Int(b)) => int_op(*a, *b),
            _ if either_double(&prev, &n) => op(to_f64(prev.clone()), to_f64(n.clone())),
            _ => op(ord_num(compare_exact(&prev, &n)), 0.0),
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

/// The sign of a tower number as a `zero?`/`pos?`/`neg?`
/// answer (`Double` keeps its historic arms above).
fn tower_sign(n: &Num, want: num_bigint::Sign) -> bool {
    num_sign(n) == want
}

fn native_zero(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/zero?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x == 0,
        Num::Double(x) => x == 0.0,
        n => tower_sign(&n, num_bigint::Sign::NoSign),
    }))
}

fn native_pos(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/pos?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x > 0,
        Num::Double(x) => x > 0.0,
        n => tower_sign(&n, num_bigint::Sign::Plus),
    }))
}

fn native_neg(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/neg?", args, &[1])?;
    Ok(Value::Bool(match as_num(&args[0])? {
        Num::Int(x) => x < 0,
        Num::Double(x) => x < 0.0,
        n => tower_sign(&n, num_bigint::Sign::Minus),
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
    if let Some(ord) = compare_nil(x, y) {
        return Ok(ord);
    }
    if let Some(ord) = compare_numbers(x, y)? {
        return Ok(ord);
    }
    if let Some(ord) = compare_same_type(x, y)? {
        return Ok(ord);
    }
    Err(compare_cast_err(x, y))
}

/// Nils sort first (`nil` is less than everything, equal to itself).
fn compare_nil(x: &Value, y: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (x, y) {
        (Value::Nil, Value::Nil) => Some(Ordering::Equal),
        (Value::Nil, _) => Some(Ordering::Less),
        (_, Value::Nil) => Some(Ordering::Greater),
        _ => None,
    }
}

/// Numeric cross-compare (`None` when `x` is not numeric, so the
/// caller falls through; a numeric `x` against a non-numeric `y`
/// fails `y` to `Number`).
fn compare_numbers(x: &Value, y: &Value) -> Result<Option<std::cmp::Ordering>, NativeError> {
    use std::cmp::Ordering;
    if !is_number(x) {
        return Ok(None);
    }
    if !is_number(y) {
        return Err(cce_err(y, "java.lang.Number"));
    }
    let (a, b) = (as_num(x)?, as_num(y)?);
    // `Double` pairs compare as `f64` (the `<`/`>` pair, never
    // `total_cmp`: `NaN` ties); every other numeric pairing
    // compares exactly (oracle `cmpd`, `sort-mixed2`).
    if either_double(&a, &b) {
        let (a, b) = (to_f64(a), to_f64(b));
        if a < b {
            return Ok(Some(Ordering::Less));
        }
        if a > b {
            return Ok(Some(Ordering::Greater));
        }
        return Ok(Some(Ordering::Equal));
    }
    Ok(Some(compare_exact(&a, &b)))
}

/// Same-type scalars and vectors (`None` when the types differ).
fn compare_same_type(x: &Value, y: &Value) -> Result<Option<std::cmp::Ordering>, NativeError> {
    match (x, y) {
        (Value::Str(a), Value::Str(b)) => Ok(Some(compare_utf16(a, b))),
        (Value::Char(a), Value::Char(b)) => Ok(Some(a.cmp(b))),
        (Value::Bool(a), Value::Bool(b)) => Ok(Some(a.cmp(b))),
        (Value::Keyword(a), Value::Keyword(b)) => Ok(Some(compare_named(a, b))),
        (Value::Symbol(a), Value::Symbol(b)) => Ok(Some(compare_named(a, b))),
        (Value::Vector(a), Value::Vector(b)) => Ok(Some(compare_vectors(a, b)?)),
        // Entries order exactly like their 2-vectors (Oracle FX2
        // `compare`, `sort-entries`, `maxkey`).
        (Value::MapEntry { key, val }, Value::Vector(b)) => {
            let tmp = clj_value::Vector::from_vec(vec![(**key).clone(), (**val).clone()]);
            Ok(Some(compare_vectors(&tmp, b)?))
        }
        (Value::Vector(a), Value::MapEntry { key, val }) => {
            let tmp = clj_value::Vector::from_vec(vec![(**key).clone(), (**val).clone()]);
            Ok(Some(compare_vectors(a, &tmp)?))
        }
        (Value::MapEntry { key: k1, val: v1 }, Value::MapEntry { key: k2, val: v2 }) => {
            let a = clj_value::Vector::from_vec(vec![(**k1).clone(), (**v1).clone()]);
            let b = clj_value::Vector::from_vec(vec![(**k2).clone(), (**v2).clone()]);
            Ok(Some(compare_vectors(&a, &b)?))
        }
        _ => Ok(None),
    }
}

/// Lexicographic vector order (recursive element `compare`).
fn compare_vectors(
    a: &clj_value::Vector,
    b: &clj_value::Vector,
) -> Result<std::cmp::Ordering, NativeError> {
    use std::cmp::Ordering;
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

/// `x.compareTo(y)`: checkcast `y` to `x`'s class (vectors name
/// the interface); non-`Comparable` `x` fails itself.
fn compare_cast_err(x: &Value, y: &Value) -> NativeError {
    let target = match x {
        Value::Str(_) => "java.lang.String",
        Value::Char(_) => "java.lang.Character",
        Value::Bool(_) => "java.lang.Boolean",
        Value::Keyword(_) => "clojure.lang.Keyword",
        Value::Symbol(_) => "clojure.lang.Symbol",
        Value::Vector(_) | Value::MapEntry { .. } => "clojure.lang.IPersistentVector",
        _ => return cce_err(x, "java.lang.Comparable"),
    };
    cce_err(y, target)
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
    matches!(v, Value::Int(_) | Value::BigInt(_))
}
/// `int?` stays `Int`-only (oracle: `(int? 5N)` is false — the
/// pinned `(int? 1)` wedge for `Long` stays too).
fn is_int(v: &Value) -> bool {
    matches!(v, Value::Int(_))
}
fn is_double(v: &Value) -> bool {
    matches!(v, Value::Double(_))
}
fn is_number(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(_) | Value::Double(_) | Value::BigInt(_) | Value::BigDec(_) | Value::Ratio(_)
    )
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
        Value::List(_) | Value::Vector(_) | Value::MapEntry { .. } | Value::Map(_) | Value::Set(_)
    ) || as_subvector(v).is_some()
        || is_seq_value(v)
        || crate::types::is_host_record(v)
}
fn is_sequential(v: &Value) -> bool {
    matches!(v, Value::Vector(_) | Value::MapEntry { .. })
        || as_subvector(v).is_some()
        || is_seq_value(v)
}
fn is_associative(v: &Value) -> bool {
    matches!(v, Value::Map(_) | Value::Vector(_) | Value::MapEntry { .. })
        || as_subvector(v).is_some()
        || crate::types::is_host_record(v)
}
pub(crate) fn is_counted(v: &Value) -> bool {
    matches!(
        v,
        Value::List(_) | Value::Vector(_) | Value::MapEntry { .. } | Value::Map(_) | Value::Set(_)
    ) || crate::coll::is_transient(v)
        || as_subvector(v).is_some()
        || is_counted_seq(v)
        || crate::types::is_host_record(v)
}
fn is_list(v: &Value) -> bool {
    matches!(v, Value::List(_))
}
fn is_vector(v: &Value) -> bool {
    matches!(v, Value::Vector(_) | Value::MapEntry { .. }) || as_subvector(v).is_some()
}
fn is_map(v: &Value) -> bool {
    matches!(v, Value::Map(_)) || crate::types::is_host_record(v)
}
fn is_set(v: &Value) -> bool {
    matches!(v, Value::Set(_))
}
// Sorted maps and sets answer true (FT6); everything else answers
// false, matching the oracle for all constructible inputs.
fn is_sorted(v: &Value) -> bool {
    match v {
        Value::Map(m) => m.is_sorted(),
        Value::Set(s) => s.is_sorted(),
        _ => false,
    }
}
// `IPersistentVector` (vectors, map entries, subvectors) implements
// `clojure.lang.Reversible`, as do sorted maps and sets (FT6).
// Lists, hash maps, hash sets, queues, seqs (including `rseq`),
// and transients do not (`docs/t1192/probes/fx2b-reversible`).
fn is_reversible(v: &Value) -> bool {
    matches!(v, Value::Vector(_) | Value::MapEntry { .. })
        || as_subvector(v).is_some()
        || is_sorted(v)
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
            | Value::MapEntry { .. }
            | Value::Map(_)
            | Value::Set(_)
    ) || as_subvector(v).is_some()
        || is_seq_value(v)
        || crate::types::is_host_record(v)
}
// `record?` (S-typechain stage 1): true for record instances —
// including records with extension entries — and nothing else
// (deftype instances, descriptors, and plain maps all answer
// false).
fn is_record(v: &Value) -> bool {
    crate::types::is_host_record(v)
}

pred1!(native_nil, "clojure.core/nil?", is_nil);
pred1!(native_not, "clojure.core/not", is_falsy_val);
pred1!(native_boolean, "clojure.core/boolean", is_truthy_val);
pred1!(native_somep, "clojure.core/some?", is_some_val);
pred1!(native_truep, "clojure.core/true?", is_true);
pred1!(native_falsep, "clojure.core/false?", is_false);
pred1!(native_booleanp, "clojure.core/boolean?", is_boolean);
pred1!(native_integerp, "clojure.core/integer?", is_integer);
pred1!(native_intp, "clojure.core/int?", is_int);
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
pred1!(native_sortedp, "clojure.core/sorted?", is_sorted);
pred1!(
    native_reversiblep,
    "clojure.core/reversible?",
    is_reversible
);
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

/// Printer options from the bound `*print-*` vars (`readably`
/// overrides the binding when given — `print` passes false, `pr`
/// honors). Public for S7 drivers: the REPL prints results exactly
/// like `prn` (pass `None`).
pub fn print_opts(
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
        dup: deref_core(eval, "*print-dup*").is_truthy(),
    })
}

/// Whether the value is a JVM `LazySeq` (whose `toString` never
/// prints contents): a pending seq reporting the `LazySeq` class
/// (forced or not — memoizing never changes the class), or a
/// realized seq explicitly kinded `LazySeq`. Shared with
/// `print-simple`'s object half (single source).
pub(crate) fn is_lazy_seq_str(v: &Value) -> bool {
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
        _ if is_lazy_seq_str(v) => lazy_seq_str(eval, v),
        _ => str_of_realized(eval, v),
    }
}

/// A `LazySeq` renders `clojure.lang.LazySeq@<hex>` with the Java
/// `List` content hash over the fully forced items.
fn lazy_seq_str(eval: &mut Evaluator, v: &Value) -> Result<String, NativeError> {
    let items = collect_all(eval, v).map_err(signal_to_native)?;
    let mut h: i32 = 1;
    for item in &items {
        let r = realize_deep(eval, item).map_err(signal_to_native)?;
        h = h.wrapping_mul(31).wrapping_add(clj_value::hash_code(&r));
    }
    Ok(format!("clojure.lang.LazySeq@{:x}", h as u32))
}

/// `str` of a realized non-`LazySeq` value: seq objects and strict
/// collections print readably without limits, atoms/delays render
/// their `toString`, anything else prints plainly.
fn str_of_realized(eval: &mut Evaluator, v: &Value) -> Result<String, NativeError> {
    let r = realize_deep(eval, v).map_err(signal_to_native)?;
    Ok(match &r {
        // Port-defined types run a zero-arg `toString` method when
        // defined, else the `Name@hex` fallback (oracle `reify-str`,
        // `j-str-rec`); descriptors render their name.
        Value::HostType(h) => crate::types::host_str(eval, h, r.clone())?,
        // Multimethods render the `toString` fallback (no methods
        // run for them, oracle `d-prefer`).
        Value::MultiFn(m) => clj_value::multifn_str(&m.name),
        // Atoms, delays, futures, promises, and agents render their
        // `toString` (`Class@hex`), never the `#object[…]` print form.
        Value::Atom(_)
        | Value::Delay(_)
        | Value::Future(_)
        | Value::Promise(_)
        | Value::Agent(_) => clj_value::java_debug(&r),
        Value::List(_) | Value::Vector(_) | Value::Map(_) | Value::Set(_) | Value::Extension(_) => {
            clj_value::pr_to_string(&r, &clj_value::PrintOpts::pr())
        }
        _ => pr_to_string(&r, &PrintOpts::print()),
    })
}

/// `str`: concatenates the per-arg rendering (no spaces).
/// `str` concatenated (shared by `str` and the `throw-*!` natives).
fn str_concat(eval: &mut Evaluator, args: &[Value]) -> Result<String, NativeError> {
    let mut out = String::new();
    for arg in args {
        out.push_str(&str_of_one(eval, arg)?);
    }
    Ok(out)
}

pub(crate) fn native_str(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let s = str_concat(eval, args)?;
    Ok(Value::Str(Arc::from(s.as_str())))
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
        v => return Err(cce_err(v, "java.lang.String")),
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
/// Like the JVM's `with-out-str` + `pr`/`print`: `*out*` binds to
/// a fresh string writer (so user methods see it) and every arg
/// renders through the `pr-on` driver.
fn pr_print_str(
    eval: &mut Evaluator,
    args: &[Value],
    readably: Option<bool>,
) -> Result<Value, NativeError> {
    // Eager validation first (limits CCE before anything realizes,
    // as before).
    print_opts(eval, readably)?;
    let mut vals = Vec::with_capacity(args.len());
    for arg in args {
        vals.push(realize_deep(eval, arg).map_err(signal_to_native)?);
    }
    let sw = crate::io::string_writer(eval);
    let depth = eval.bindings_depth();
    crate::printsys::bind_core_var(eval, "*out*", sw.clone());
    crate::printsys::maybe_bind_print_readably(eval, readably);
    let result = pr_str_args(eval, &vals);
    eval.pop_bindings(depth);
    result?;
    let contents = crate::io::writer_contents(&sw).unwrap_or_default();
    Ok(Value::Str(Arc::from(contents.as_str())))
}

/// One `pr-on` per arg into the bound string writer (separators
/// between, `*out*` re-read per arg like the `pr` path).
fn pr_str_args(eval: &mut Evaluator, vals: &[Value]) -> Result<(), NativeError> {
    use crate::ns::deref_core;
    for (i, v) in vals.iter().enumerate() {
        let out = deref_core(eval, "*out*");
        crate::io::check_writer(&out)?;
        if i > 0 {
            crate::io::write_to_writer(&out, " ")?;
        }
        crate::printsys::pr_on(eval, v, &out).map_err(signal_to_native)?;
    }
    Ok(())
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

/// A `Ratio` truncated toward zero (`int`/`long` narrow from
/// there, like the JVM's `intValue`/`longValue`).
fn trunc_ratio(r: &num_rational::BigRational) -> num_bigint::BigInt {
    r.numer() / r.denom()
}

/// A truncated tower integer as a `long`: out-of-`i64` fails
/// `Value out of range for long` with the exact digits (oracle
/// `l1`/`l2` — no wrap, unlike `longValue`).
fn bigint_long(n: &num_bigint::BigInt) -> Result<i64, NativeError> {
    n.to_i64()
        .ok_or_else(|| out_of_range("long", Num::BigInt(n.clone())))
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
/// via `Double.toString`, ints raw (fixprobe10 `:int-huge`),
/// tower values exactly.
fn out_of_range(kind: &str, n: Num) -> NativeError {
    let x = match n {
        Num::Int(i) => i.to_string(),
        Num::Double(d) => clj_value::java_double_string(d),
        Num::BigInt(n) => n.to_string(),
        Num::Ratio(r) => format!("{r}"),
        Num::BigDec(d) => d.to_bigdec_string(),
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
        Value::Double(d) => double_int(*d)?,
        Value::Char(c) => *c as i64,
        // Tower integers take the `long` range check (exact
        // digits) then `toIntExact` (oracles `conv-i1`,
        // `conv-hugeint`); decimals narrow through `double`
        // first (oracle `conv-hugedecint`: `1.0E30`).
        Value::BigInt(n) => tower_intcast(n).map(i64::from)?,
        Value::Ratio(r) => tower_intcast(&trunc_ratio(r)).map(i64::from)?,
        Value::BigDec(d) => bigdec_intcast(d).map(i64::from)?,
        Value::Nil => return Err(npe_char_value()),
        v => return Err(cce_err(v, "java.lang.Character")),
    }))
}

/// An `int` from a `double`: `NaN` is 0, past-`i32` fails `Value
/// out of range for int` (checked BEFORE truncating), else
/// truncation toward zero.
fn double_int(d: f64) -> Result<i64, NativeError> {
    if d.is_nan() {
        Ok(0)
    } else if d > f64::from(i32::MAX) || d < f64::from(i32::MIN) {
        Err(out_of_range("int", Num::Double(d)))
    } else {
        Ok(d.trunc() as i64)
    }
}

fn native_long(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/long", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Int(i) => *i,
        Value::Double(d) => double_long(*d)?,
        Value::Char(c) => *c as i64,
        Value::BigInt(n) => bigint_long(n)?,
        Value::Ratio(r) => bigint_long(&trunc_ratio(r))?,
        // Decimals narrow through `double` (oracle `conv-declong`:
        // `(long 1E+30M)` fails `1.0E30`).
        Value::BigDec(d) => double_long(d.to_f64())?,
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

fn native_double(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/double", args, &[1])?;
    Ok(Value::Double(match &args[0] {
        Value::Int(i) => *i as f64,
        Value::Double(d) => *d,
        Value::BigInt(n) => n.to_f64().unwrap_or(f64::NAN),
        Value::Ratio(r) => r.to_f64().unwrap_or(f64::NAN),
        Value::BigDec(d) => d.to_f64(),
        Value::Nil => return Err(npe_double_value()),
        // Characters rejected (fixprobe11 `:dbl-ch`).
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

/// `bigint`: `BigInt` passes through; `Int` widens; `Double`
/// and `BigDec` truncate toward zero through the canonical
/// decimal (`NaN`/`Inf` fail like `valueOf`); `Ratio` truncates;
/// strings parse like `BigInteger(String)`; nil trips the
/// `String.length` NPE; anything else is the `No matching ctor`
/// `IllegalArgumentException` (all oracle-verbatim).
fn native_bigint(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/bigint", args, &[1])?;
    match &args[0] {
        Value::BigInt(n) => Ok(Value::BigInt(n.clone())),
        Value::Int(i) => Ok(Value::BigInt(num_bigint::BigInt::from(*i))),
        Value::Double(d) => {
            let dec = clj_value::BigDec::value_of_f64(*d).map_err(number_format)?;
            Ok(Value::BigInt(dec.to_bigint_trunc()))
        }
        Value::BigDec(d) => Ok(Value::BigInt(d.to_bigint_trunc())),
        Value::Ratio(r) => Ok(Value::BigInt(r.numer() / r.denom())),
        Value::Str(s) => clj_value::bigint_parse(s)
            .map(Value::BigInt)
            .map_err(number_format),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.length()\" because \"val\" is null".to_owned(),
        )),
        _ => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "No matching ctor found for class java.math.BigInteger".to_owned(),
        )),
    }
}

/// `bigdec`: `BigDec` passes through; `Int`/`Double` go through
/// `valueOf` (so `(bigdec 5)` is `5.0M`); `BigInt` widens;
/// `Ratio` must terminate exactly; strings parse like
/// `BigDecimal(String)`; nil trips the `toCharArray` NPE;
/// anything else is the `No matching ctor`
/// `IllegalArgumentException` (all oracle-verbatim).
fn native_bigdec(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/bigdec", args, &[1])?;
    match &args[0] {
        Value::BigDec(d) => Ok(Value::BigDec(d.clone())),
        Value::Int(i) => Ok(Value::BigDec(
            clj_value::BigDec::value_of_f64(*i as f64).map_err(number_format)?,
        )),
        Value::Double(d) => Ok(Value::BigDec(
            clj_value::BigDec::value_of_f64(*d).map_err(number_format)?,
        )),
        Value::BigInt(n) => Ok(Value::BigDec(clj_value::BigDec::from_bigint(n.clone()))),
        Value::Ratio(r) => clj_value::BigDec::from_rational_exact(r)
            .map(Value::BigDec)
            .map_err(arithmetic),
        Value::Str(s) => clj_value::BigDec::parse(s)
            .map(Value::BigDec)
            .map_err(number_format),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.toCharArray()\" because \"val\" is null".to_owned(),
        )),
        _ => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "No matching ctor found for class java.math.BigDecimal".to_owned(),
        )),
    }
}

/// `num`: numbers (tower included) pass through, nil is nil,
/// anything else fails the `Number` cast (oracle `num-str`).
fn native_num(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/num", args, &[1])?;
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::Int(_)
        | Value::Double(_)
        | Value::BigInt(_)
        | Value::BigDec(_)
        | Value::Ratio(_) => Ok(args[0].clone()),
        other => Err(cce_err(other, "java.lang.Number")),
    }
}

/// `rationalize`: integers pass through, nil is nil, doubles go
/// through `BigDecimal.valueOf` first (`(rationalize 0.1)` is
/// `1/10`, `(rationalize 1e100)` is exact `10N^100`;
/// `NaN`/`Inf` fail), integer-valued decimals fold to `BigInt`
/// (`(rationalize 5.0M)` → `5N`), anything else fails the
/// `Number` cast (all oracle-verbatim).
fn native_rationalize(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rationalize", args, &[1])?;
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::Int(_) | Value::BigInt(_) | Value::Ratio(_) => Ok(args[0].clone()),
        Value::Double(d) => {
            let dec = clj_value::BigDec::value_of_f64(*d).map_err(number_format)?;
            let r = dec.to_rational();
            if r.denom() == &num_bigint::BigInt::from(1) {
                Ok(Value::BigInt(r.numer().clone()))
            } else {
                Ok(Value::Ratio(r))
            }
        }
        Value::BigDec(d) => {
            let r = d.to_rational();
            if r.denom() == &num_bigint::BigInt::from(1) {
                Ok(Value::BigInt(r.numer().clone()))
            } else {
                Ok(Value::Ratio(r))
            }
        }
        other => Err(cce_err(other, "java.lang.Number")),
    }
}

/// A `NumberFormatException` for a message.
fn number_format(message: impl Into<String>) -> NativeError {
    NativeError::new(ThrowClass::NumberFormatException, message.into())
}

/// An `ArithmeticException` for a message.
pub(crate) fn arithmetic(message: impl Into<String>) -> NativeError {
    NativeError::new(ThrowClass::ArithmeticException, message.into())
}

fn native_float(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/float", args, &[1])?;
    // No `Float` exists: the float range is checked, then the
    // EXACT double is returned (no `f32` rounding — printing and
    // simple values match; the class/precision gap is documented).
    Ok(Value::Double(match &args[0] {
        Value::Int(i) => *i as f64,
        // `NaN` falls through (all comparisons false).
        Value::Double(d) => float_checked(*d, Num::Double(*d))?,
        // Tower narrows like `double`, then takes the same
        // range check (the class/precision gap is documented).
        Value::BigInt(n) => float_checked(n.to_f64().unwrap_or(f64::NAN), Num::BigInt(n.clone()))?,
        Value::Ratio(r) => float_checked(r.to_f64().unwrap_or(f64::NAN), Num::Ratio(r.clone()))?,
        Value::BigDec(d) => float_checked(d.to_f64(), Num::BigDec(d.clone()))?,
        Value::Nil => return Err(npe_double_value()),
        v => return Err(cce_err(v, "java.lang.Number")),
    }))
}

/// A `float` from a narrowed `f64` with its error rendering: past
/// the `f32` range fails `Value out of range for float`, else the
/// exact `double` (no `f32` rounding).
fn float_checked(f: f64, err: Num) -> Result<f64, NativeError> {
    if f > f64::from(f32::MAX) || f < -f64::from(f32::MAX) {
        Err(out_of_range("float", err))
    } else {
        Ok(f)
    }
}

fn native_short(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/short", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_double_value()),
        // Numbers narrow through `narrow_num` (doubles check
        // directly, tower through the `long` stop — oracles
        // `tower-r2e`, `tower-r2f`).
        v => narrow_num(&as_num(v)?, -32768, 32767, "short")?,
    }))
}

fn native_byte(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/byte", args, &[1])?;
    Ok(Value::Int(match &args[0] {
        Value::Char(c) => *c as i64,
        Value::Nil => return Err(npe_double_value()),
        // Numbers narrow through `narrow_num` (oracles `tower-r2e`,
        // `tower-r2f`).
        v => narrow_num(&as_num(v)?, -128, 127, "byte")?,
    }))
}

fn native_char(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/char", args, &[1])?;
    Ok(Value::Char(match &args[0] {
        Value::Char(c) => *c,
        Value::Nil => return Err(npe_long_value()),
        // Numbers narrow through `char_of_num` (oracles
        // `tower-r2f`, `tower-r2g`).
        v => char_of_num(&as_num(v)?)?,
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
    pub(crate) fn wrap(value: Value) -> Value {
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
    let s = str_concat(eval, args)?;
    Err(NativeError::new(ThrowClass::IllegalArgumentException, s))
}

/// `throw-npe!` (PRIVATE, port-internal): `doseq`'s invalid-keyword
/// path throws the JVM's `steppair` `NullPointerException` verbatim
/// (a JVM bug the corpus pins — `doseq` never validates modifiers).
fn native_throw_npe(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/throw-npe!", args, &[1])?;
    let s = str_concat(eval, args)?;
    Err(NativeError::new(ThrowClass::NullPointerException, s))
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

/// `clojure-version`: the port release string (S7 P3). A literal,
/// not `env!("CARGO_PKG_VERSION")`: versions live per-crate while
/// the release version belongs to the workspace — the literal keeps
/// every crate reporting the release (bump it with the release;
/// `clj-repl` tests pin the semver shape).
fn native_clojure_version(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/clojure-version", args, &[0])?;
    Ok(Value::Str(Arc::from("1.13.0-alpha9")))
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
    ("sorted?", native_sortedp),
    ("reversible?", native_reversiblep),
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
    ("bigint", native_bigint),
    ("bigdec", native_bigdec),
    ("num", native_num),
    ("rationalize", native_rationalize),
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
    ("clojure-version", native_clojure_version),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `num_rank` still orders `Double` top (only called past the
    /// `either_double` guards, so user code never reaches this
    /// arm — the pin keeps the promotion lattice total).
    #[test]
    fn num_rank_orders_double_top() {
        assert_eq!(num_rank(&Num::Double(0.5)), 4);
        assert_eq!(num_rank(&Num::Int(1)), 0);
    }

    /// The exact paths reject `Double` (callers dispatch on
    /// `either_double` first — the panic is the contract).
    #[test]
    #[should_panic(expected = "doubles take the f64 path, never the exact one")]
    fn num_rational_rejects_double() {
        let _ = num_rational(&Num::Double(0.5));
    }

    /// See [`num_rational_rejects_double`].
    #[test]
    #[should_panic(expected = "rank dispatch keeps doubles out of BigDec ops")]
    fn num_bigdec_rejects_double() {
        let _ = num_bigdec(&Num::Double(0.5));
    }

    /// A denominator-1 rational folds to `BigInt` (user code only
    /// builds reduced ratios, so this arm is the safety net).
    #[test]
    fn num_value_folds_unit_denominator() {
        let unreduced = num_rational::BigRational::new(
            num_bigint::BigInt::from(2),
            num_bigint::BigInt::from(2),
        );
        assert_eq!(
            num_value(Num::Ratio(unreduced)),
            Value::BigInt(num_bigint::BigInt::from(1))
        );
    }

    #[test]
    fn hash_code_post_realize_uoe() {
        // `hash-code` is private (unreachable from user code): like
        // `hash`, an infinite revealed by forcing throws bare UOE.
        let mut eval = crate::boot();
        let mut vs = eval
            .eval_source("(map identity [(repeat 1)])")
            .expect("shape evaluates");
        let v = vs.pop().expect("one form");
        let err = native_hash_code(&mut eval, &[v]).expect_err("post-check throws");
        assert_eq!(
            err.class,
            clj_value::ThrowClass::UnsupportedOperationException
        );
        assert!(err.message.is_empty());
    }
}
