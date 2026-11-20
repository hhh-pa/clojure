// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Hot-path runtime helpers compiled to LLVM bitcode (`clj-aot` LTO, #260).
//!
//! Every exported `rt_bc_*` entry takes an already-dropped output slot
//! (`&mut MaybeUninit<Value>`) and returns `1` (`u8` non-zero) when it
//! wrote the result inline, or `0` to fall back cleanly to the C-ABI
//! `rt_n_*` shim in `clj-rt`. Keeping slow paths, error formatting,
//! and heap drop glue out of this crate prevents cross-crate symbol
//! dependency issues while letting `default<O2>` inline hot operations
//! directly into compiled Clojure functions.

mod ops;

#[cfg(not(test))]
use std::mem::MaybeUninit;
use std::sync::Arc;

use clj_value::{FastVars, Transient, TransientKind, Value};

pub use ops::{BcOp1, BcOp2, BcOp3, StrAsciiFn, bc_fx_symbol, bc_ns_op_symbol, bc_op_symbol};

/// Output destination abstraction so bitcode entry points write
/// directly into `&mut MaybeUninit<Value>` without drop glue while unit
/// tests assert values via `&mut Option<Value>` cleanly without `unsafe`.
pub(crate) trait OutSlot {
    fn write_value(&mut self, v: Value);
}

#[cfg(not(test))]
impl OutSlot for MaybeUninit<Value> {
    #[inline(always)]
    fn write_value(&mut self, v: Value) {
        self.write(v);
    }
}

#[cfg(test)]
impl OutSlot for Option<Value> {
    fn write_value(&mut self, v: Value) {
        *self = Some(v);
    }
}

#[inline(always)]
fn clone_hot_scalar(src: &Value) -> Option<Value> {
    Some(match src {
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        Value::Int(n) => Value::Int(*n),
        Value::Double(d) => Value::Double(*d),
        Value::Char(c) => Value::Char(*c),
        Value::Str(s) => Value::Str(Arc::clone(s)),
        Value::Keyword(k) => Value::Keyword(k.clone()),
        Value::Symbol(s) => Value::Symbol(s.clone()),
        _ => return None,
    })
}

#[inline(always)]
fn clone_hot_coll(src: &Value) -> Option<Value> {
    Some(match src {
        Value::Vector(v) => Value::Vector(v.clone()),
        Value::List(l) => Value::List(l.clone()),
        Value::Map(m) => Value::Map(m.clone()),
        Value::Set(s) => Value::Set(s.clone()),
        _ => return None,
    })
}

#[inline(always)]
fn clone_rare_scalar(src: &Value) -> Option<Value> {
    Some(match src {
        Value::Uuid(u) => Value::Uuid(*u),
        Value::Instant(t) => Value::Instant(*t),
        Value::Regex(r) => Value::Regex(Arc::clone(r)),
        Value::BigInt(n) => Value::BigInt(Arc::clone(n)),
        Value::BigInteger(n) => Value::BigInteger(Arc::clone(n)),
        Value::BigDec(d) => Value::BigDec(Arc::clone(d)),
        Value::Ratio(r) => Value::Ratio(Arc::clone(r)),
        Value::Throwable(t) => Value::Throwable(Arc::clone(t)),
        Value::Extension(e) => Value::Extension(Arc::clone(e)),
        _ => return None,
    })
}

#[inline(never)]
fn clone_rare(src: &Value) -> Option<Value> {
    if let Some(v) = clone_rare_scalar(src) {
        return Some(v);
    }
    Some(match src {
        Value::Transient(t) => Value::Transient(t.clone()),
        Value::Var(v) => Value::Var(v.clone()),
        Value::Atom(a) => Value::Atom(a.clone()),
        Value::Delay(d) => Value::Delay(d.clone()),
        Value::Future(f) => Value::Future(f.clone()),
        Value::Promise(p) => Value::Promise(p.clone()),
        Value::Agent(a) => Value::Agent(a.clone()),
        Value::Volatile(v) => Value::Volatile(v.clone()),
        Value::Ref(r) => Value::Ref(r.clone()),
        _ => return None,
    })
}

/// Clone `src` into an owned [`Value`] when it is a common POD or
/// `Arc`-backed variant.
#[inline(always)]
pub(crate) fn clone_value(src: &Value) -> Option<Value> {
    clone_hot_scalar(src)
        .or_else(|| clone_hot_coll(src))
        .or_else(|| clone_rare(src))
}

/// Write `src` clone into `out` when `src` is a common POD or
/// `Arc`-backed variant. Returns `true` on success; `false` delegates
/// rare/boxed variants cleanly to `rt_n_clone`.
#[inline(always)]
pub(crate) fn clone_fast<O: OutSlot>(out: &mut O, src: &Value) -> bool {
    match clone_value(src) {
        Some(v) => {
            out.write_value(v);
            true
        }
        None => false,
    }
}

/// Clone `src` into an already-dropped `out` slot (`1` = handled, `0`
/// = fallback to `rt_n_clone`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_clone(out: &mut MaybeUninit<Value>, src: &Value) -> i8 {
    i8::from(clone_fast(out, src))
}

#[inline]
fn deref_fast<O: OutSlot>(out: &mut O, vars: &FastVars, k: u64) -> bool {
    let Some(val) = usize::try_from(k).ok().and_then(|idx| vars.get(idx)) else {
        return false;
    };
    clone_fast(out, val)
}

/// Fast var-root load from the frozen [`FastVars`] cache (`1` = hit
/// cloned into already-dropped `out`, `0` = miss -> fallback `rt_n_deref`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_deref(out: &mut MaybeUninit<Value>, vars: &FastVars, k: u64) -> i8 {
    i8::from(deref_fast(out, vars, k))
}

/// Whether `slot` is a POD value needing no heap drop (`1` = no drop
/// needed, `0` = heap drop via `rt_n_drop`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_is_pod(slot: &Value) -> i8 {
    i8::from(matches!(
        slot,
        Value::Nil
            | Value::Bool(_)
            | Value::Int(_)
            | Value::Double(_)
            | Value::Char(_)
            | Value::Uuid(_)
            | Value::Instant(_)
    ))
}

#[cfg(not(test))]
macro_rules! bc_ops1 {
    ($($sym:ident => $op:expr),* $(,)?) => {
        $(
            /// Dedicated 1-arg `clojure.core` bitcode fast-path entrypoint.
            #[unsafe(no_mangle)]
            pub extern "C" fn $sym(out: &mut MaybeUninit<Value>, a: &Value) -> i8 {
                i8::from(ops::apply1(out, $op, a, None))
            }
        )*
    };
}

#[cfg(not(test))]
macro_rules! bc_ops2 {
    ($($sym:ident => $op:expr),* $(,)?) => {
        $(
            /// Dedicated 2-arg `clojure.core` bitcode fast-path entrypoint.
            #[unsafe(no_mangle)]
            pub extern "C" fn $sym(
                out: &mut MaybeUninit<Value>,
                a: &Value,
                b: &Value,
            ) -> i8 {
                i8::from(ops::apply2(out, $op, a, b, None))
            }
        )*
    };
}

#[cfg(not(test))]
macro_rules! bc_ops3 {
    ($($sym:ident => $op:expr),* $(,)?) => {
        $(
            /// Dedicated 3-arg `clojure.core` bitcode fast-path entrypoint.
            #[unsafe(no_mangle)]
            pub extern "C" fn $sym(
                out: &mut MaybeUninit<Value>,
                a: &Value,
                b: &Value,
                c: &Value,
            ) -> i8 {
                i8::from(ops::apply3(out, $op, a, b, c, None))
            }
        )*
    };
}

#[cfg(not(test))]
bc_ops1! {
    rt_bc_1_inc => BcOp1::Inc,
    rt_bc_1_dec => BcOp1::Dec,
    rt_bc_1_neg => BcOp1::Neg,
    rt_bc_1_bit_not => BcOp1::BitNot,
    rt_bc_1_zero_p => BcOp1::ZeroP,
    rt_bc_1_pos_p => BcOp1::PosP,
    rt_bc_1_neg_p => BcOp1::NegP,
    rt_bc_1_even_p => BcOp1::EvenP,
    rt_bc_1_odd_p => BcOp1::OddP,
    rt_bc_1_not => BcOp1::Not,
    rt_bc_1_nil_p => BcOp1::NilP,
    rt_bc_1_some_p => BcOp1::SomeP,
    rt_bc_1_string_p => BcOp1::StringP,
    rt_bc_1_int => BcOp1::IntCast,
    rt_bc_1_long => BcOp1::LongCast,
    rt_bc_1_double => BcOp1::DoubleCast,
    rt_bc_1_empty_p => BcOp1::EmptyP,
    rt_bc_1_peek => BcOp1::Peek,
    rt_bc_1_first => BcOp1::First,
    rt_bc_1_pop_bang => BcOp1::PopBang,
}

/// Dedicated 1-arg `count` bitcode entrypoint using the host ASCII probe for long strings.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_1_count(
    out: &mut MaybeUninit<Value>,
    a: &Value,
    str_ascii: StrAsciiFn,
) -> i8 {
    i8::from(ops::apply1(out, BcOp1::Count, a, Some(str_ascii)))
}

#[cfg(not(test))]
bc_ops2! {
    rt_bc_2_add => BcOp2::Add,
    rt_bc_2_sub => BcOp2::Sub,
    rt_bc_2_mul => BcOp2::Mul,
    rt_bc_2_lt => BcOp2::Lt,
    rt_bc_2_lte => BcOp2::Lte,
    rt_bc_2_gt => BcOp2::Gt,
    rt_bc_2_gte => BcOp2::Gte,
    rt_bc_2_num_eq => BcOp2::NumEq,
    rt_bc_2_eq => BcOp2::Eq,
    rt_bc_2_not_eq => BcOp2::NotEq,
    rt_bc_2_quot => BcOp2::Quot,
    rt_bc_2_rem => BcOp2::Rem,
    rt_bc_2_mod => BcOp2::Mod,
    rt_bc_2_bit_and => BcOp2::BitAnd,
    rt_bc_2_bit_or => BcOp2::BitOr,
    rt_bc_2_bit_xor => BcOp2::BitXor,
    rt_bc_2_shl => BcOp2::Shl,
    rt_bc_2_shr => BcOp2::Shr,
    rt_bc_2_ushr => BcOp2::Ushr,
    rt_bc_2_min => BcOp2::Min,
    rt_bc_2_max => BcOp2::Max,
    rt_bc_2_contains => BcOp2::Contains,
    rt_bc_2_str_starts_with => BcOp2::StrStartsWith,
    rt_bc_2_str_ends_with => BcOp2::StrEndsWith,
    rt_bc_2_str_includes => BcOp2::StrIncludes,
    rt_bc_2_conj_bang => BcOp2::ConjBang,
}

#[cfg(not(test))]
macro_rules! bc_str_ops2 {
    ($($sym:ident => $op:expr),* $(,)?) => {
        $(
            /// Dedicated 2-arg string-aware bitcode entrypoint using the host ASCII probe.
            #[unsafe(no_mangle)]
            pub extern "C" fn $sym(
                out: &mut MaybeUninit<Value>,
                a: &Value,
                b: &Value,
                str_ascii: StrAsciiFn,
            ) -> i8 {
                i8::from(ops::apply2(out, $op, a, b, Some(str_ascii)))
            }
        )*
    };
}

#[cfg(not(test))]
bc_str_ops2! {
    rt_bc_2_nth => BcOp2::Nth,
    rt_bc_2_get => BcOp2::Get,
}

#[cfg(not(test))]
bc_ops3! {
    rt_bc_3_assoc_bang => BcOp3::AssocBang,
}

#[cfg(not(test))]
macro_rules! bc_str_ops3 {
    ($($sym:ident => $op:expr),* $(,)?) => {
        $(
            /// Dedicated 3-arg string-aware bitcode entrypoint using the host ASCII probe.
            #[unsafe(no_mangle)]
            pub extern "C" fn $sym(
                out: &mut MaybeUninit<Value>,
                a: &Value,
                b: &Value,
                c: &Value,
                str_ascii: StrAsciiFn,
            ) -> i8 {
                i8::from(ops::apply3(out, $op, a, b, c, Some(str_ascii)))
            }
        )*
    };
}

#[cfg(not(test))]
bc_str_ops3! {
    rt_bc_3_nth => BcOp3::Nth3,
    rt_bc_3_get => BcOp3::Get3,
}

/// Effect-position `pop!` entrypoint: returns `1` with `Value::Nil` on hit.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_1_pop_bang_fx(out: &mut MaybeUninit<Value>, a: &Value) -> i8 {
    i8::from(ops::pop_bang_fast(out, a, false))
}

/// Effect-position `assoc!` entrypoint: returns `1` with `Value::Nil` on hit.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_3_assoc_bang_fx(
    out: &mut MaybeUninit<Value>,
    a: &Value,
    b: &Value,
    c: &Value,
) -> i8 {
    i8::from(ops::assoc_bang_fast(out, a, b, c, false))
}

/// Effect-position `conj!` entrypoint: returns `1` with `Value::Nil` on hit.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_2_conj_bang_fx(out: &mut MaybeUninit<Value>, a: &Value, b: &Value) -> i8 {
    i8::from(ops::conj_bang_fast(out, a, b, false))
}

/// Probe `v` as an ASCII string (`1` on hit, writing `ptr_out` and `len_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_str(
    v: &Value,
    ptr_out: &mut *const u8,
    len_out: &mut i64,
    str_ascii: Option<StrAsciiFn>,
) -> i8 {
    i8::from(ops::scalar_str_probe(v, ptr_out, len_out, str_ascii))
}

/// Unboxed `count` entrypoint (`1` on hit, writing `len_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_count(
    v: &Value,
    len_out: &mut i64,
    str_ascii: Option<StrAsciiFn>,
) -> i8 {
    i8::from(ops::scalar_count(v, len_out, str_ascii))
}

/// Unboxed `nth` entrypoint (`1` on hit, writing `k_out` and `v_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_nth(
    coll: &Value,
    idx: i64,
    k_out: &mut u8,
    v_out: &mut i64,
    str_ascii: Option<StrAsciiFn>,
    allow_fp: u8,
) -> i8 {
    i8::from(ops::scalar_nth(
        coll,
        idx,
        k_out,
        v_out,
        str_ascii,
        allow_fp != 0,
    ))
}

/// Unboxed 2D `nth` entrypoint (`coll[idx][item]`, `1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_nth2(
    coll: &Value,
    idx: i64,
    item: i64,
    k_out: &mut u8,
    v_out: &mut i64,
    allow_fp: u8,
) -> i8 {
    i8::from(ops::scalar_nth2(
        coll,
        idx,
        item,
        k_out,
        v_out,
        allow_fp != 0,
    ))
}

/// Unboxed effect-position `assoc!` entrypoint (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_assoc(tval: &Value, idx: i64, val: i64) -> i8 {
    i8::from(ops::scalar_assoc(tval, idx, val))
}

/// Unboxed effect-position `Double` `assoc!` entrypoint (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_assoc_d(tval: &Value, idx: i64, bits: i64) -> i8 {
    i8::from(ops::scalar_assoc_d(tval, idx, bits))
}

/// Unboxed effect-position `conj!` entrypoint (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_conj(tval: &Value, val: i64) -> i8 {
    i8::from(ops::scalar_conj(tval, val))
}

/// Unboxed effect-position `Double` `conj!` entrypoint (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_conj_d(tval: &Value, bits: i64) -> i8 {
    i8::from(ops::scalar_conj_d(tval, bits))
}

/// Probe `v` as a flat packed integer vector (`1` on hit, writing `ptr_out` and `len_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_ivec(v: &Value, ptr_out: &mut *const i64, len_out: &mut i64) -> i8 {
    i8::from(ops::scalar_ivec_probe(v, ptr_out, len_out))
}

/// Probe `v` as a flat packed `f64` vector (`1` on hit, writing `ptr_out` and `len_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_dvec(v: &Value, ptr_out: &mut *const f64, len_out: &mut i64) -> i8 {
    i8::from(ops::scalar_dvec_probe(v, ptr_out, len_out))
}

/// Probe `v` as a flat generic [`Value`] vector (`1` on hit, writing `ptr_out` and `len_out`).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_gvec(
    v: &Value,
    ptr_out: &mut *const Value,
    len_out: &mut i64,
) -> i8 {
    i8::from(ops::scalar_gvec_probe(v, ptr_out, len_out))
}

/// Whether `a` and `b` alias the same transient buffer (`1` when equal).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_same_transient(a: &Value, b: &Value) -> i8 {
    i8::from(ops::scalar_same_transient(a, b))
}

/// Lock a packed-`Int` transient vector for a scalar loop (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_tvec_lock(
    v: &Value,
    ptr_out: &mut *mut i64,
    len_out: &mut i64,
) -> i8 {
    i8::from(ops::scalar_tvec_lock(v, ptr_out, len_out))
}

/// Lock a packed-`Double` transient vector for a scalar loop (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_dtvec_lock(
    v: &Value,
    ptr_out: &mut *mut f64,
    len_out: &mut i64,
) -> i8 {
    i8::from(ops::scalar_dtvec_lock(v, ptr_out, len_out))
}

/// Release a transient lock acquired by [`rt_bc_scalar_tvec_lock`].
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_tvec_unlock(v: &Value) {
    ops::scalar_tvec_unlock(v);
}

/// Unboxed effect-position `pop!` entrypoint (`1` on hit).
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rt_bc_scalar_pop(tval: &Value) -> i8 {
    i8::from(ops::scalar_pop(tval))
}

#[inline]
pub(crate) fn is_transient_vec(v: &Value) -> Option<&Transient> {
    match v {
        Value::Transient(t) => Some(t),
        _ => None,
    }
}

#[inline]
pub(crate) fn transient_vec_len(t: &Transient) -> Option<usize> {
    let guard = t.lock();
    match &*guard {
        TransientKind::Vector(tv) => tv.len().ok(),
        TransientKind::Map(tm) => tm.len().ok(),
        TransientKind::Set(ts) => ts.len().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{apply1, apply2, apply3};
    use clj_value::{
        Agent, Atom, BigDec, BindCounts, Delay, Future, Instant, Keyword, List, Map, Promise, Ref,
        Set, SubVectorValue, Symbol, ThrowClass, Throwable, TransientMap, TransientSet,
        TransientVector, Unbound, Uuid, Var, Vector, Volatile,
    };
    use num_bigint::BigInt;
    use num_rational::BigRational;
    use std::sync::atomic::AtomicU64;

    extern "C" fn test_str_ascii(v: &Value) -> u8 {
        match v {
            Value::Str(s) if s.is_ascii() => 1,
            _ => 0,
        }
    }

    fn run1(op: BcOp1, a: &Value) -> Option<Value> {
        let mut out = None;
        apply1(&mut out, op, a, Some(test_str_ascii)).then(|| out.unwrap())
    }

    fn run2(op: BcOp2, a: &Value, b: &Value) -> Option<Value> {
        let mut out = None;
        apply2(&mut out, op, a, b, Some(test_str_ascii)).then(|| out.unwrap())
    }

    fn run3(op: BcOp3, a: &Value, b: &Value, c: &Value) -> Option<Value> {
        let mut out = None;
        apply3(&mut out, op, a, b, c, Some(test_str_ascii)).then(|| out.unwrap())
    }

    fn str_val(s: &str) -> Value {
        Value::Str(Arc::from(s))
    }

    #[test]
    fn clone_deref_is_pod_and_op_lookup() {
        assert_eq!(bc_op_symbol("inc", 1), Some("rt_bc_1_inc"));
        assert_eq!(bc_op_symbol("+", 2), Some("rt_bc_2_add"));
        assert_eq!(bc_op_symbol("assoc!", 3), Some("rt_bc_3_assoc_bang"));
        assert_eq!(bc_op_symbol("conj!", 2), Some("rt_bc_2_conj_bang"));
        assert_eq!(bc_op_symbol("unknown", 1), None);
        assert_eq!(bc_op_symbol("unknown", 2), None);
        assert_eq!(bc_op_symbol("unknown", 3), None);
        assert_eq!(bc_op_symbol("inc", 4), None);
        assert_eq!(bc_fx_symbol("pop!", 1), Some("rt_bc_1_pop_bang_fx"));
        assert_eq!(bc_fx_symbol("conj!", 2), Some("rt_bc_2_conj_bang_fx"));
        assert_eq!(bc_fx_symbol("assoc!", 3), Some("rt_bc_3_assoc_bang_fx"));
        assert_eq!(bc_fx_symbol("inc", 1), None);

        let samples: Vec<Value> = vec![
            Value::Nil,
            Value::Bool(true),
            Value::Int(42),
            Value::Double(2.5),
            Value::Char('z'),
            Value::Uuid(Uuid::from_bits(1, 2)),
            Value::Instant(Instant::from_millis(1234)),
            str_val("hi"),
            Value::Regex(Arc::from("a+")),
            Value::BigInt(Arc::new(BigInt::from(7))),
            Value::BigInteger(Arc::new(BigInt::from(8))),
            Value::BigDec(Arc::new(BigDec::from_f64_exact(1.5).unwrap())),
            Value::Ratio(Arc::new(BigRational::new(BigInt::from(3), BigInt::from(4)))),
            Value::Throwable(Arc::new(Throwable {
                id: 9,
                class: ThrowClass::Exception,
                message: None,
                data: None,
                cause: None,
            })),
            Value::Extension(Arc::new(SubVectorValue::new(Vector::new_sync(), 0, 0))),
            Value::Keyword(Keyword::new(None, "k")),
            Value::Symbol(Symbol::new(None, "s")),
            Value::Vector(Vector::from_ints(vec![1, 2])),
            Value::List(List::from_vec(vec![Value::Int(1)])),
            Value::Map(Map::from_kvs_last_wins(vec![(
                Value::Int(1),
                Value::Int(2),
            )])),
            Value::Set(Set::from_items_checked(vec![Value::Int(1)]).unwrap()),
            Value::Transient(Transient::new(
                TransientKind::Vector(TransientVector::new()),
            )),
            Value::Var(Var {
                id: 10,
                sym: Symbol::new(None, "v"),
            }),
            Value::Atom(Atom::new(Value::Nil)),
            Value::Delay(Delay::new(Value::Nil)),
            Value::Future(Future::new()),
            Value::Promise(Promise::new()),
            Value::Agent(Agent::new(Value::Nil)),
            Value::Volatile(Volatile::new(Value::Nil)),
            Value::Ref(Ref::new(Value::Nil)),
        ];
        for v in &samples {
            let mut out = None;
            assert!(clone_fast(&mut out, v), "failed cloning {v:?}");
            assert_eq!(out.as_ref(), Some(v));
        }

        let unbound = Value::Unbound(Unbound {
            sym: Symbol::new(None, "u"),
            var_id: 1,
        });
        let mut out = None;
        assert!(!clone_fast(&mut out, &unbound));

        let reg_epoch = Arc::new(AtomicU64::new(0));
        let bind_counts = Arc::new(BindCounts::new());
        let vars = FastVars::new(reg_epoch, bind_counts, 2);
        vars.record(0, 0, 7, &Value::Int(77));
        let mut deref_out = None;
        assert!(deref_fast(&mut deref_out, &vars, 0));
        assert_eq!(deref_out, Some(Value::Int(77)));
        assert!(!deref_fast(&mut deref_out, &vars, 9));

        for (name, _) in ops::OP1_SYMBOLS {
            assert!(bc_op_symbol(name, 1).is_some(), "{name}");
        }
        for (name, _) in ops::OP2_SYMBOLS {
            assert!(bc_op_symbol(name, 2).is_some(), "{name}");
        }
        for (name, _) in ops::OP3_SYMBOLS {
            assert!(bc_op_symbol(name, 3).is_some(), "{name}");
        }
        for (name, _) in ops::STR2_SYMBOLS {
            assert!(
                bc_ns_op_symbol(Some("clojure.string"), name, 2).is_some(),
                "{name}"
            );
        }
        assert_eq!(
            bc_ns_op_symbol(Some("clojure.core"), "+", 2),
            Some("rt_bc_2_add")
        );
        assert_eq!(bc_ns_op_symbol(Some("clojure.string"), "split", 2), None);
        assert_eq!(
            bc_ns_op_symbol(Some("clojure.string"), "starts-with?", 1),
            None
        );
        assert_eq!(bc_ns_op_symbol(Some("user"), "+", 2), None);
    }

    #[test]
    fn unary_fast_ops_and_fallbacks() {
        assert_eq!(run1(BcOp1::Inc, &Value::Int(1)), Some(Value::Int(2)));
        assert_eq!(
            run1(BcOp1::Inc, &Value::Double(1.0)),
            Some(Value::Double(2.0))
        );
        assert_eq!(run1(BcOp1::Inc, &Value::Int(i64::MAX)), None);
        assert_eq!(run1(BcOp1::Inc, &Value::Nil), None);
        assert_eq!(run1(BcOp1::Dec, &Value::Int(2)), Some(Value::Int(1)));
        assert_eq!(
            run1(BcOp1::Dec, &Value::Double(2.0)),
            Some(Value::Double(1.0))
        );
        assert_eq!(run1(BcOp1::Neg, &Value::Int(5)), Some(Value::Int(-5)));
        assert_eq!(
            run1(BcOp1::Neg, &Value::Double(5.0)),
            Some(Value::Double(-5.0))
        );

        assert_eq!(run1(BcOp1::ZeroP, &Value::Int(0)), Some(Value::Bool(true)));
        assert_eq!(
            run1(BcOp1::ZeroP, &Value::Double(0.0)),
            Some(Value::Bool(true))
        );
        assert_eq!(run1(BcOp1::PosP, &Value::Int(1)), Some(Value::Bool(true)));
        assert_eq!(
            run1(BcOp1::PosP, &Value::Double(1.0)),
            Some(Value::Bool(true))
        );
        assert_eq!(run1(BcOp1::NegP, &Value::Int(-1)), Some(Value::Bool(true)));
        assert_eq!(
            run1(BcOp1::NegP, &Value::Double(-1.0)),
            Some(Value::Bool(true))
        );
        assert_eq!(run1(BcOp1::EvenP, &Value::Int(4)), Some(Value::Bool(true)));
        assert_eq!(run1(BcOp1::OddP, &Value::Int(3)), Some(Value::Bool(true)));
        assert_eq!(run1(BcOp1::EvenP, &Value::Nil), None);

        assert_eq!(run1(BcOp1::IntCast, &Value::Int(4)), Some(Value::Int(4)));
        assert_eq!(
            run1(BcOp1::IntCast, &Value::Char('A')),
            Some(Value::Int(65))
        );
        assert_eq!(run1(BcOp1::IntCast, &Value::Int(i64::MAX)), None);
        assert_eq!(run1(BcOp1::LongCast, &Value::Int(4)), Some(Value::Int(4)));
        assert_eq!(
            run1(BcOp1::LongCast, &Value::Char('A')),
            Some(Value::Int(65))
        );
        assert_eq!(
            run1(BcOp1::DoubleCast, &Value::Int(4)),
            Some(Value::Double(4.0))
        );
        assert_eq!(
            run1(BcOp1::DoubleCast, &Value::Double(2.5)),
            Some(Value::Double(2.5))
        );

        assert_eq!(run1(BcOp1::BitNot, &Value::Int(0)), Some(Value::Int(-1)));
        assert_eq!(run1(BcOp1::BitNot, &Value::Nil), None);
        assert_eq!(run1(BcOp1::Not, &Value::Nil), Some(Value::Bool(true)));
        assert_eq!(run1(BcOp1::NilP, &Value::Nil), Some(Value::Bool(true)));
        assert_eq!(run1(BcOp1::SomeP, &Value::Nil), Some(Value::Bool(false)));
        assert_eq!(run1(BcOp1::StringP, &str_val("a")), Some(Value::Bool(true)));

        let v = Value::Vector(Vector::from_ints(vec![10, 20]));
        let ve = Value::Vector(Vector::new_sync());
        let l = Value::List(List::from_vec(vec![Value::Int(1)]));
        let le = Value::List(List::from_vec(vec![]));
        let m = Value::Map(Map::from_kvs_last_wins(vec![(
            Value::Int(1),
            Value::Int(2),
        )]));
        let st = Value::Set(Set::from_items_checked(vec![Value::Int(1)]).unwrap());
        let s = str_val("ab");
        let sempty = str_val("");
        let inline_s = str_val(&"a".repeat(128));
        let long_s = str_val(&"a".repeat(200));
        let sup_s = str_val("\u{1F600}");

        let mut tv = TransientVector::new();
        assert!(tv.conj_int_opt(9));
        let tvec = Value::Transient(Transient::new(TransientKind::Vector(tv)));
        let tmap = Value::Transient(Transient::new(TransientKind::Map(TransientMap::new())));
        let tset = Value::Transient(Transient::new(TransientKind::Set(TransientSet::new())));

        assert_eq!(run1(BcOp1::Count, &Value::Nil), Some(Value::Int(0)));
        assert_eq!(run1(BcOp1::Count, &v), Some(Value::Int(2)));
        assert_eq!(run1(BcOp1::Count, &l), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::Count, &m), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::Count, &st), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::Count, &s), Some(Value::Int(2)));
        assert_eq!(run1(BcOp1::Count, &inline_s), Some(Value::Int(128)));
        assert_eq!(run1(BcOp1::Count, &sup_s), None);
        let mut no_cb_out = None;
        assert!(!apply1(&mut no_cb_out, BcOp1::Count, &long_s, None));
        assert_eq!(run1(BcOp1::Count, &long_s), Some(Value::Int(200)));
        assert_eq!(run1(BcOp1::Count, &str_val(&"é".repeat(100))), None);
        assert_eq!(run1(BcOp1::Count, &tvec), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::Count, &tmap), Some(Value::Int(0)));
        assert_eq!(run1(BcOp1::Count, &tset), Some(Value::Int(0)));
        assert_eq!(run1(BcOp1::Count, &Value::Int(1)), None);

        assert_eq!(run1(BcOp1::EmptyP, &sempty), Some(Value::Bool(true)));
        assert_eq!(run1(BcOp1::EmptyP, &ve), Some(Value::Bool(true)));

        assert_eq!(run1(BcOp1::Peek, &Value::Nil), Some(Value::Nil));
        assert_eq!(run1(BcOp1::Peek, &l), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::Peek, &le), Some(Value::Nil));
        let vq = Value::Vector(Vector::from_ints(vec![10, 20]).with_meta(Some(
            Map::from_kvs_last_wins(vec![(Value::Int(1), Value::Bool(true))]),
        )));
        assert_eq!(run1(BcOp1::Peek, &v), Some(Value::Int(20)));
        assert_eq!(run1(BcOp1::Peek, &ve), Some(Value::Nil));
        assert_eq!(run1(BcOp1::Peek, &vq), None);
        assert_eq!(run1(BcOp1::Peek, &Value::Int(1)), None);

        assert_eq!(run1(BcOp1::First, &Value::Nil), Some(Value::Nil));
        assert_eq!(run1(BcOp1::First, &l), Some(Value::Int(1)));
        assert_eq!(run1(BcOp1::First, &le), Some(Value::Nil));
        assert_eq!(run1(BcOp1::First, &v), Some(Value::Int(10)));
        assert_eq!(run1(BcOp1::First, &ve), Some(Value::Nil));
        assert_eq!(run1(BcOp1::First, &s), Some(Value::Char('a')));
        assert_eq!(run1(BcOp1::First, &sempty), Some(Value::Nil));
        assert_eq!(run1(BcOp1::First, &sup_s), None);
        assert_eq!(run1(BcOp1::First, &Value::Int(1)), None);

        assert!(run1(BcOp1::PopBang, &tvec).is_some());
        assert_eq!(run1(BcOp1::PopBang, &tvec), None);
        assert_eq!(run1(BcOp1::PopBang, &tmap), None);
        assert_eq!(run1(BcOp1::PopBang, &Value::Nil), None);

        let mut gtv = TransientVector::new();
        assert!(gtv.conj(Value::Vector(Vector::new_sync())).is_ok());
        let gtval = Value::Transient(Transient::new(TransientKind::Vector(gtv)));
        assert_eq!(run1(BcOp1::PopBang, &gtval), None);
    }

    #[test]
    fn binary_and_ternary_fast_ops_and_fallbacks() {
        for op in [BcOp2::Add, BcOp2::Sub, BcOp2::Mul, BcOp2::Min, BcOp2::Max] {
            assert!(run2(op, &Value::Int(6), &Value::Int(2)).is_some());
            assert!(run2(op, &Value::Double(6.0), &Value::Double(2.0)).is_some());
            assert!(run2(op, &Value::Int(6), &Value::Double(2.0)).is_some());
            assert!(run2(op, &Value::Double(6.0), &Value::Int(2)).is_some());
            assert_eq!(run2(op, &Value::Nil, &Value::Int(1)), None);
        }
        assert_eq!(
            run2(BcOp2::Min, &Value::Double(f64::NAN), &Value::Double(1.0)),
            None
        );
        assert_eq!(
            run2(BcOp2::Max, &Value::Double(f64::NAN), &Value::Double(1.0)),
            None
        );

        for op in [BcOp2::Lt, BcOp2::Lte, BcOp2::Gt, BcOp2::Gte, BcOp2::NumEq] {
            assert!(run2(op, &Value::Int(1), &Value::Int(2)).is_some());
            assert!(run2(op, &Value::Double(1.0), &Value::Double(2.0)).is_some());
            assert!(run2(op, &Value::Int(1), &Value::Double(2.0)).is_some());
            assert!(run2(op, &Value::Double(1.0), &Value::Int(2)).is_some());
            assert_eq!(run2(op, &Value::Nil, &Value::Int(1)), None);
        }

        let kw = Value::Keyword(Keyword::new(None, "x"));
        let s = str_val("x");
        assert_eq!(
            run2(BcOp2::Eq, &Value::Int(1), &Value::Int(1)),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::NotEq, &Value::Char('a'), &Value::Char('b')),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::Eq, &Value::Bool(true), &Value::Bool(false)),
            Some(Value::Bool(false))
        );
        assert_eq!(
            run2(BcOp2::Eq, &Value::Nil, &Value::Nil),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::Eq, &Value::Double(1.0), &Value::Double(1.0)),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::Eq, &Value::Double(f64::NAN), &Value::Double(1.0)),
            None
        );
        assert_eq!(run2(BcOp2::Eq, &kw, &kw), Some(Value::Bool(true)));
        assert_eq!(run2(BcOp2::Eq, &s, &s), Some(Value::Bool(true)));
        assert_eq!(
            run2(BcOp2::Eq, &Value::Int(1), &Value::Nil),
            Some(Value::Bool(false))
        );
        assert_eq!(
            run2(
                BcOp2::Eq,
                &Value::Vector(Vector::new_sync()),
                &Value::Vector(Vector::new_sync())
            ),
            None
        );

        for op in [
            BcOp2::Quot,
            BcOp2::Rem,
            BcOp2::Mod,
            BcOp2::BitAnd,
            BcOp2::BitOr,
            BcOp2::BitXor,
            BcOp2::Shl,
            BcOp2::Shr,
            BcOp2::Ushr,
        ] {
            assert!(run2(op, &Value::Int(7), &Value::Int(3)).is_some());
        }
        assert_eq!(
            run2(BcOp2::Mod, &Value::Int(-1), &Value::Int(5)),
            Some(Value::Int(4))
        );
        assert_eq!(
            run2(BcOp2::Mod, &Value::Int(6), &Value::Int(3)),
            Some(Value::Int(0))
        );
        assert_eq!(run2(BcOp2::Mod, &Value::Int(6), &Value::Int(0)), None);
        assert_eq!(run2(BcOp2::BitAnd, &Value::Int(1), &Value::Nil), None);

        let v = Value::Vector(Vector::from_ints(vec![10, 20]));
        let str_ok = str_val("hi");
        let str_inline = str_val(&"a".repeat(128));
        let str_short_utf8 = str_val("é");
        let str_long = str_val(&"a".repeat(200));
        let str_long2 = str_val(&"b".repeat(200));
        let str_long_utf8 = str_val(&"é".repeat(100));
        assert_eq!(run2(BcOp2::Nth, &v, &Value::Int(0)), Some(Value::Int(10)));
        assert_eq!(run2(BcOp2::Nth, &v, &Value::Int(9)), None);
        assert_eq!(
            run3(BcOp3::Nth3, &v, &Value::Int(9), &Value::Int(99)),
            Some(Value::Int(99))
        );
        assert_eq!(
            run2(BcOp2::Nth, &str_ok, &Value::Int(1)),
            Some(Value::Char('i'))
        );
        assert_eq!(run2(BcOp2::Nth, &str_ok, &Value::Int(9)), None);
        assert_eq!(
            run3(BcOp3::Nth3, &str_ok, &Value::Int(-1), &Value::Int(5)),
            Some(Value::Int(5))
        );
        assert_eq!(
            run2(BcOp2::Nth, &str_inline, &Value::Int(127)),
            Some(Value::Char('a'))
        );
        assert_eq!(run2(BcOp2::Nth, &str_short_utf8, &Value::Int(0)), None);
        assert_eq!(
            run2(BcOp2::Nth, &str_long, &Value::Int(0)),
            Some(Value::Char('a'))
        );
        let v_refs = Value::Vector(Vector::from_vec(vec![Value::Char('z'), kw.clone()]));
        assert_eq!(
            run2(BcOp2::Nth, &v_refs, &Value::Int(0)),
            Some(Value::Char('z'))
        );
        assert_eq!(run2(BcOp2::Nth, &v_refs, &Value::Int(1)), Some(kw.clone()));
        let v_ints = Value::Vector(Vector::from_ints(vec![42]));
        assert_eq!(
            run2(BcOp2::Nth, &v_ints, &Value::Int(0)),
            Some(Value::Int(42))
        );
        let v_chars = Value::Vector(Vector::from_chars(vec!['z']));
        assert_eq!(
            run2(BcOp2::Nth, &v_chars, &Value::Int(0)),
            Some(Value::Char('z'))
        );
        assert_eq!(run2(BcOp2::Nth, &str_ok, &Value::Int(i64::MAX)), None);
        assert_eq!(
            run2(BcOp2::Nth, &str_long2, &Value::Int(0)),
            Some(Value::Char('b'))
        );
        // Hit slot > 0 (moves str_long to slot 0) then hit slot 0.
        assert_eq!(
            run2(BcOp2::Nth, &str_long, &Value::Int(1)),
            Some(Value::Char('a'))
        );
        assert_eq!(
            run2(BcOp2::Nth, &str_long, &Value::Int(2)),
            Some(Value::Char('a'))
        );
        assert_eq!(run2(BcOp2::Nth, &str_long_utf8, &Value::Int(0)), None);
        assert_eq!(run2(BcOp2::Nth, &Value::Nil, &Value::Int(0)), None);
        assert_eq!(run2(BcOp2::Nth, &v, &Value::Nil), None);

        let mut tv = TransientVector::new();
        assert!(tv.conj_int_opt(5));
        let tval = Value::Transient(Transient::new(TransientKind::Vector(tv)));
        let tmap = Value::Transient(Transient::new(TransientKind::Map(TransientMap::new())));
        assert_eq!(run2(BcOp2::Nth, &tval, &Value::Int(0)), Some(Value::Int(5)));
        assert_eq!(
            run3(BcOp3::Nth3, &tval, &Value::Int(9), &Value::Int(4)),
            Some(Value::Int(4))
        );
        assert_eq!(
            run3(BcOp3::Nth3, &tval, &Value::Int(-1), &Value::Int(4)),
            Some(Value::Int(4))
        );
        assert_eq!(run2(BcOp2::Nth, &tmap, &Value::Int(0)), None);
        let mut t_inv = TransientVector::new();
        let _ = t_inv.persistent().unwrap();
        let t_inv_val = Value::Transient(Transient::new(TransientKind::Vector(t_inv)));
        assert_eq!(run2(BcOp2::Nth, &t_inv_val, &Value::Int(0)), None);

        let smap = Value::Map(Map::from_kvs_last_wins(vec![(kw.clone(), Value::Int(8))]));
        let lmap = Value::Map(Map::from_kvs_last_wins(
            (0..20).map(|i| (Value::Int(i), Value::Int(i))).collect(),
        ));
        assert_eq!(run2(BcOp2::Get, &Value::Nil, &kw), Some(Value::Nil));
        assert_eq!(
            run3(BcOp3::Get3, &Value::Nil, &kw, &Value::Int(7)),
            Some(Value::Int(7))
        );
        assert_eq!(run2(BcOp2::Get, &v, &Value::Int(1)), Some(Value::Int(20)));
        assert_eq!(run2(BcOp2::Get, &v, &Value::Int(9)), Some(Value::Nil));
        assert_eq!(run2(BcOp2::Get, &v, &Value::Nil), None);
        assert_eq!(run2(BcOp2::Get, &smap, &kw), Some(Value::Int(8)));
        assert_eq!(run2(BcOp2::Get, &smap, &Value::Int(1)), Some(Value::Nil));
        assert_eq!(run2(BcOp2::Get, &lmap, &Value::Int(0)), None);
        assert_eq!(
            run2(BcOp2::Get, &str_ok, &Value::Int(0)),
            Some(Value::Char('h'))
        );
        assert_eq!(run2(BcOp2::Get, &str_ok, &Value::Int(9)), Some(Value::Nil));
        assert_eq!(run2(BcOp2::Get, &str_ok, &Value::Int(-1)), Some(Value::Nil));
        assert_eq!(
            run2(BcOp2::Get, &str_inline, &Value::Int(0)),
            Some(Value::Char('a'))
        );
        assert_eq!(
            run2(BcOp2::Get, &str_long, &Value::Int(0)),
            Some(Value::Char('a'))
        );
        assert_eq!(run2(BcOp2::Get, &str_short_utf8, &Value::Int(0)), None);
        assert_eq!(run2(BcOp2::Get, &str_ok, &Value::Nil), None);
        assert_eq!(run2(BcOp2::Get, &Value::Int(1), &Value::Int(0)), None);

        assert_eq!(
            run2(BcOp2::Contains, &Value::Nil, &kw),
            Some(Value::Bool(false))
        );
        assert_eq!(run2(BcOp2::Contains, &smap, &kw), Some(Value::Bool(true)));
        assert_eq!(run2(BcOp2::Contains, &lmap, &Value::Int(0)), None);
        assert_eq!(
            run2(BcOp2::Contains, &v, &Value::Int(1)),
            Some(Value::Bool(true))
        );
        assert_eq!(run2(BcOp2::Contains, &v, &Value::Nil), None);
        assert_eq!(run2(BcOp2::Contains, &str_ok, &Value::Int(0)), None);

        let s_hello = str_val("hello");
        let s_abab = str_val("abab");
        let sw = str_val("hel");
        let ew = str_val("llo");
        let inc_s = str_val("ell");
        assert_eq!(
            run2(BcOp2::StrStartsWith, &s_hello, &sw),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::StrStartsWith, &s_hello, &ew),
            Some(Value::Bool(false))
        );
        assert_eq!(
            run2(BcOp2::StrEndsWith, &s_hello, &ew),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::StrIncludes, &s_hello, &inc_s),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::StrIncludes, &s_hello, &str_val("")),
            Some(Value::Bool(true))
        );
        assert_eq!(
            run2(BcOp2::StrIncludes, &s_hello, &str_val("hello!")),
            Some(Value::Bool(false))
        );
        assert_eq!(
            run2(BcOp2::StrIncludes, &s_abab, &str_val("ac")),
            Some(Value::Bool(false))
        );
        assert_eq!(run2(BcOp2::StrStartsWith, &Value::Nil, &sw), None);

        assert!(run3(BcOp3::AssocBang, &tval, &Value::Int(0), &Value::Int(99)).is_some());
        assert_eq!(run3(BcOp3::AssocBang, &tval, &Value::Int(0), &v), None);
        assert_eq!(
            run3(BcOp3::AssocBang, &tval, &Value::Int(5), &Value::Int(99)),
            None
        );
        assert_eq!(
            run3(BcOp3::AssocBang, &tval, &Value::Nil, &Value::Int(1)),
            None
        );
        assert_eq!(
            run3(BcOp3::AssocBang, &tmap, &Value::Int(0), &Value::Int(1)),
            None
        );
        assert_eq!(
            run3(
                BcOp3::AssocBang,
                &Value::Nil,
                &Value::Int(0),
                &Value::Int(1)
            ),
            None
        );
    }

    #[test]
    fn conj_bang_fast_paths() {
        let mut init = TransientVector::new();
        init.conj(Value::Int(0)).unwrap();
        let ctvec = Value::Transient(Transient::new(TransientKind::Vector(init)));
        let tmap = Value::Transient(Transient::new(TransientKind::Map(TransientMap::new())));
        assert!(run2(BcOp2::ConjBang, &ctvec, &Value::Int(7)).is_some());
        let mut fx_out = None;
        assert!(ops::conj_bang_fast(
            &mut fx_out,
            &ctvec,
            &Value::Int(8),
            false
        ));
        assert_eq!(fx_out, Some(Value::Nil));
        assert_eq!(run2(BcOp2::ConjBang, &ctvec, &str_val("x")), None);
        assert_eq!(run2(BcOp2::ConjBang, &Value::Nil, &Value::Int(1)), None);
        assert_eq!(run2(BcOp2::ConjBang, &tmap, &Value::Int(1)), None);
    }

    #[test]
    fn scalar_str_and_count_paths() {
        let s = str_val("hello");
        let mut ptr = std::ptr::null();
        let mut len = -1;
        assert!(ops::scalar_str_probe(&s, &mut ptr, &mut len, None));
        assert_eq!(len, 5);
        assert!(!ptr.is_null());
        assert!(!ops::scalar_str_probe(
            &Value::Nil,
            &mut ptr,
            &mut len,
            None
        ));
        let utf8 = str_val("é");
        assert!(!ops::scalar_str_probe(&utf8, &mut ptr, &mut len, None));

        assert!(ops::scalar_count(&s, &mut len, None));
        assert_eq!(len, 5);
        let sv = Value::Vector(clj_value::Vector::from_ints(vec![1, 2]));
        assert!(ops::scalar_count(&sv, &mut len, None));
        assert_eq!(len, 2);
        assert!(!ops::scalar_count(&Value::Int(1), &mut len, None));
    }

    #[test]
    fn scalar_nth_and_transient_paths() {
        let mut k = 99;
        let mut v = -1;
        let ivec = Value::Vector(clj_value::Vector::from_ints(vec![10, 20]));
        assert!(ops::scalar_nth(&ivec, 1, &mut k, &mut v, None, false));
        assert_eq!((k, v), (0, 20));
        assert!(!ops::scalar_nth(&ivec, -1, &mut k, &mut v, None, false));

        let cvec = Value::Vector(clj_value::Vector::from_chars(vec!['a']));
        assert!(ops::scalar_nth(&cvec, 0, &mut k, &mut v, None, false));
        assert_eq!((k, v), (1, i64::from(b'a')));

        let gvec = Value::Vector(clj_value::Vector::from_vec(vec![
            Value::Int(3),
            Value::Char('z'),
            Value::Bool(true),
            Value::Double(1.5),
            str_val("no"),
        ]));
        assert!(ops::scalar_nth(&gvec, 0, &mut k, &mut v, None, true));
        assert_eq!((k, v), (0, 3));
        assert!(ops::scalar_nth(&gvec, 1, &mut k, &mut v, None, true));
        assert_eq!((k, v), (1, i64::from(b'z')));
        assert!(ops::scalar_nth(&gvec, 2, &mut k, &mut v, None, true));
        assert_eq!((k, v), (2, 1));
        assert!(ops::scalar_nth(&gvec, 3, &mut k, &mut v, None, true));
        assert_eq!((k, v), (3, 1.5_f64.to_bits() as i64));
        assert!(!ops::scalar_nth(&gvec, 3, &mut k, &mut v, None, false));
        assert!(!ops::scalar_nth(&gvec, 4, &mut k, &mut v, None, true));

        let s = str_val("ab");
        assert!(ops::scalar_nth(&s, 1, &mut k, &mut v, None, false));
        assert_eq!((k, v), (1, i64::from(b'b')));
        assert!(!ops::scalar_nth(
            &Value::Nil,
            0,
            &mut k,
            &mut v,
            None,
            false
        ));

        let mut tv = TransientVector::new();
        tv.conj(Value::Int(5)).unwrap();
        let tval = Value::Transient(Transient::new(TransientKind::Vector(tv)));
        assert!(ops::scalar_nth(&tval, 0, &mut k, &mut v, None, false));
        assert_eq!((k, v), (0, 5));
        assert!(ops::scalar_conj(&tval, 6));
        assert!(ops::scalar_assoc(&tval, 1, 7));
        assert!(!ops::scalar_assoc(&tval, -1, 7));
        assert!(ops::scalar_pop(&tval));

        let tmap = Value::Transient(Transient::new(TransientKind::Map(TransientMap::new())));
        assert!(!ops::scalar_nth(&tmap, 0, &mut k, &mut v, None, false));
        assert!(!ops::scalar_assoc(&tmap, 0, 1));
        assert!(!ops::scalar_conj(&tmap, 1));
        assert!(!ops::scalar_pop(&tmap));
        assert!(!ops::scalar_assoc(&Value::Nil, 0, 1));
        assert!(!ops::scalar_conj(&Value::Nil, 1));
        assert!(!ops::scalar_pop(&Value::Nil));

        let nested = Value::Vector(clj_value::Vector::from_vec(vec![
            ivec.clone(),
            gvec.clone(),
            str_val("x"),
        ]));
        assert!(ops::scalar_nth2(&nested, 0, 1, &mut k, &mut v, false));
        assert_eq!((k, v), (0, 20));
        assert!(ops::scalar_nth2(&nested, 1, 3, &mut k, &mut v, true));
        assert_eq!((k, v), (3, 1.5_f64.to_bits() as i64));
        assert!(!ops::scalar_nth2(&nested, 1, 3, &mut k, &mut v, false));
        assert!(!ops::scalar_nth2(&nested, 0, 5, &mut k, &mut v, false));
        assert!(!ops::scalar_nth2(&nested, 2, 0, &mut k, &mut v, false));
        assert!(!ops::scalar_nth2(&nested, -1, 0, &mut k, &mut v, false));
        assert!(!ops::scalar_nth2(&nested, 0, -1, &mut k, &mut v, false));
        assert!(!ops::scalar_nth2(&Value::Nil, 0, 0, &mut k, &mut v, false));

        let mut iptr = std::ptr::null();
        let mut ilen = -1;
        assert!(ops::scalar_ivec_probe(&ivec, &mut iptr, &mut ilen));
        assert!(!iptr.is_null());
        assert_eq!(ilen, 2);
        assert!(!ops::scalar_ivec_probe(&cvec, &mut iptr, &mut ilen));
        assert!(!ops::scalar_ivec_probe(&Value::Nil, &mut iptr, &mut ilen));

        let mut gptr = std::ptr::null();
        let mut glen = -1;
        assert!(ops::scalar_gvec_probe(&gvec, &mut gptr, &mut glen));
        assert!(!gptr.is_null());
        assert_eq!(glen, 5);
        assert!(!ops::scalar_gvec_probe(&ivec, &mut gptr, &mut glen));
        assert!(!ops::scalar_gvec_probe(&Value::Nil, &mut gptr, &mut glen));

        assert!(ops::scalar_same_transient(&tval, &tval));
        assert!(!ops::scalar_same_transient(&tval, &tmap));
        assert!(!ops::scalar_same_transient(&tval, &Value::Nil));

        let mut mptr = std::ptr::null_mut();
        assert!(ops::scalar_tvec_lock(&tval, &mut mptr, &mut ilen));
        assert!(!mptr.is_null());
        assert_eq!(ilen, 1);
        ops::scalar_tvec_unlock(&tval);
        ops::scalar_tvec_unlock(&Value::Nil);
        assert!(!ops::scalar_tvec_lock(&tmap, &mut mptr, &mut ilen));
        assert!(!ops::scalar_tvec_lock(&Value::Nil, &mut mptr, &mut ilen));

        let mut ps = clj_value::PackedStrings::new();
        assert!(ps.try_push("hi"));
        let psv = Value::Vector(clj_value::Vector::from_packed_strs(ps));
        assert_eq!(run1(BcOp1::First, &psv), None);
        assert!(!ops::scalar_nth(&psv, 0, &mut k, &mut v, None, false));

        // Packed Double vector & transient vector paths.
        let dvec = Value::Vector(clj_value::Vector::from_doubles(vec![1.25, 2.5]));
        assert_eq!(run1(BcOp1::First, &dvec), Some(Value::Double(1.25)));
        assert_eq!(
            run2(BcOp2::Nth, &dvec, &Value::Int(1)),
            Some(Value::Double(2.5))
        );
        assert!(ops::scalar_nth(&dvec, 1, &mut k, &mut v, None, true));
        assert_eq!((k, v), (3, 2.5_f64.to_bits() as i64));
        assert!(!ops::scalar_nth(&dvec, 1, &mut k, &mut v, None, false));

        let mut dptr = std::ptr::null();
        let mut dlen = -1;
        assert!(ops::scalar_dvec_probe(&dvec, &mut dptr, &mut dlen));
        assert!(!dptr.is_null());
        assert_eq!(dlen, 2);
        assert!(!ops::scalar_dvec_probe(&ivec, &mut dptr, &mut dlen));
        assert!(!ops::scalar_dvec_probe(&Value::Nil, &mut dptr, &mut dlen));

        let mut dtv = TransientVector::from_vector(&clj_value::Vector::from_doubles(vec![1.5]));
        dtv.conj(Value::Double(2.5)).unwrap();
        let dtval = Value::Transient(Transient::new(TransientKind::Vector(dtv)));
        assert!(ops::scalar_nth(&dtval, 0, &mut k, &mut v, None, true));
        assert_eq!((k, v), (3, 1.5_f64.to_bits() as i64));
        assert!(!ops::scalar_nth(&dtval, 0, &mut k, &mut v, None, false));
        assert!(ops::scalar_assoc_d(&dtval, 1, 3.5_f64.to_bits() as i64));
        assert!(!ops::scalar_assoc_d(&dtval, -1, 3.5_f64.to_bits() as i64));
        assert!(!ops::scalar_assoc_d(&tmap, 0, 3.5_f64.to_bits() as i64));
        assert!(!ops::scalar_assoc_d(
            &Value::Nil,
            0,
            3.5_f64.to_bits() as i64
        ));
        assert!(ops::scalar_conj_d(&dtval, 4.5_f64.to_bits() as i64));
        assert!(!ops::scalar_conj_d(&tmap, 4.5_f64.to_bits() as i64));
        assert!(!ops::scalar_conj_d(&Value::Nil, 4.5_f64.to_bits() as i64));
        assert_eq!(
            run2(BcOp2::ConjBang, &dtval, &Value::Double(5.5)),
            Some(dtval.clone())
        );

        let mut dmptr = std::ptr::null_mut();
        assert!(ops::scalar_dtvec_lock(&dtval, &mut dmptr, &mut dlen));
        assert!(!dmptr.is_null());
        assert_eq!(dlen, 4);
        ops::scalar_tvec_unlock(&dtval);
        assert!(!ops::scalar_dtvec_lock(&tval, &mut dmptr, &mut dlen));
        assert!(!ops::scalar_dtvec_lock(&tmap, &mut dmptr, &mut dlen));
        assert!(!ops::scalar_dtvec_lock(&Value::Nil, &mut dmptr, &mut dlen));
    }
}
