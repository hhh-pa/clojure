// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #32 RNG natives: `rand` / `rand-int` mirror the
//! `Math/random` formulas exactly (including the `(rand-int -1)`
//! → `0` truncation); `shuffle` Fisher-Yates over the seqable
//! items and answers a vector. Oracle: the alpha10
//! `clojure.core` sources (`rand` = `Math/random`, `rand-int` =
//! `int` of that, `shuffle` accepts sequential collections).

use clj_value::{ThrowClass, Value, Vector};
use num_traits::ToPrimitive;
use rand::Rng;
use rand::seq::SliceRandom;

use crate::core::{Num, as_num, cce_err};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::{collect_all, is_seq_value};
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

/// The issue #32 RNG natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const RNG_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("rand", native_rand),
    ("rand-int", native_rand_int),
    ("shuffle", native_shuffle),
];

/// One `Math/random` draw: a `Double` in `[0, 1)`.
fn unit_random() -> f64 {
    rand::rng().random_range(0.0..1.0)
}

/// A `rand` / `rand-int` bound as an `f64` (`Int` widens, tower
/// narrows approximately — the product promotes to `Double`
/// either way, like the JVM's `Numbers.multiply` unboxing).
fn bound_f64(num: Num) -> f64 {
    match num {
        Num::Int(i) => i as f64,
        Num::BigInt(n) => n.to_f64().unwrap_or(f64::NAN),
        Num::Ratio(r) => r.to_f64().unwrap_or(f64::NAN),
        Num::BigDec(d) => d.to_f64(),
        Num::Double(d) => d,
    }
}

/// `rand`: a bare draw, or `n` scaled by one (`(* n (rand))` —
/// the `Number` cast and the nil NPE ride on [`as_num`]).
fn native_rand(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rand", args, &[0, 1])?;
    let draw = unit_random();
    if args.is_empty() {
        return Ok(Value::Double(draw));
    }
    Ok(Value::Double(bound_f64(as_num(&args[0])?) * draw))
}

/// `rand-int`: `(int (* n (rand)))` — truncation through the
/// same `long` stop as `int`, so a huge product fails `Value out
/// of range for long` exactly like the JVM (oracle `re-r2`:
/// `(rand-int 1e300)`); (`(rand-int -1)` is `0`, never `-1`).
fn native_rand_int(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rand-int", args, &[1])?;
    let scaled = bound_f64(as_num(&args[0])?) * unit_random();
    Ok(Value::Int(crate::core::double_long(scaled)?))
}

/// `shuffle`: Fisher-Yates over the seqable items (nil trips
/// the `ArrayList` copy NPE; strings, maps, and anything else
/// non-sequential fail the `Collection` cast — all
/// oracle-verbatim); answers a vector.
fn native_shuffle(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/shuffle", args, &[1])?;
    match &args[0] {
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.util.Collection.toArray()\" because \"c\" is null".to_owned(),
        )),
        Value::Vector(_) | Value::List(_) | Value::Set(_) | Value::MapEntry { .. } => {
            let mut items = collect_all(eval, &args[0]).map_err(signal_to_native)?;
            items.shuffle(&mut rand::rng());
            Ok(Value::Vector(Vector::from_vec(items)))
        }
        Value::Extension(_) if is_seq_value(&args[0]) => {
            let mut items = collect_all(eval, &args[0]).map_err(signal_to_native)?;
            items.shuffle(&mut rand::rng());
            Ok(Value::Vector(Vector::from_vec(items)))
        }
        other => Err(cce_err(other, "java.util.Collection")),
    }
}
