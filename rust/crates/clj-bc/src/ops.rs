// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Pure fast-path operations compiled into the `clj-bc` LLVM bitcode.

use std::cell::Cell;
use std::sync::{Arc, Weak};

use clj_value::{TransientKind, Value, ValueRef};

use crate::{OutSlot, clone_fast, is_transient_vec, transient_vec_len};

/// Maximum string byte length scanned inline for `is_ascii` without TLS.
const MAX_INLINE_STR_BYTES: usize = 128;

/// Unary bitcode opcodes.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcOp1 {
    Inc = 0,
    Dec = 1,
    Neg = 2,
    BitNot = 3,
    ZeroP = 4,
    PosP = 5,
    NegP = 6,
    EvenP = 7,
    OddP = 8,
    Not = 9,
    NilP = 10,
    SomeP = 11,
    StringP = 12,
    IntCast = 13,
    LongCast = 14,
    DoubleCast = 15,
    Count = 16,
    EmptyP = 17,
    Peek = 18,
    First = 19,
    PopBang = 20,
}

/// Binary bitcode opcodes.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcOp2 {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Lt = 3,
    Lte = 4,
    Gt = 5,
    Gte = 6,
    NumEq = 7,
    Eq = 8,
    NotEq = 9,
    Quot = 10,
    Rem = 11,
    Mod = 12,
    BitAnd = 13,
    BitOr = 14,
    BitXor = 15,
    Shl = 16,
    Shr = 17,
    Ushr = 18,
    Min = 19,
    Max = 20,
    Nth = 21,
    Get = 22,
    Contains = 23,
}

/// Ternary bitcode opcodes.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcOp3 {
    AssocBang = 0,
    Nth3 = 1,
    Get3 = 2,
}

pub(crate) const OP1_SYMBOLS: &[(&str, &str)] = &[
    ("inc", "rt_bc_1_inc"),
    ("inc'", "rt_bc_1_inc"),
    ("dec", "rt_bc_1_dec"),
    ("dec'", "rt_bc_1_dec"),
    ("-", "rt_bc_1_neg"),
    ("-'", "rt_bc_1_neg"),
    ("bit-not", "rt_bc_1_bit_not"),
    ("zero?", "rt_bc_1_zero_p"),
    ("pos?", "rt_bc_1_pos_p"),
    ("neg?", "rt_bc_1_neg_p"),
    ("even?", "rt_bc_1_even_p"),
    ("odd?", "rt_bc_1_odd_p"),
    ("not", "rt_bc_1_not"),
    ("nil?", "rt_bc_1_nil_p"),
    ("some?", "rt_bc_1_some_p"),
    ("string?", "rt_bc_1_string_p"),
    ("int", "rt_bc_1_int"),
    ("long", "rt_bc_1_long"),
    ("double", "rt_bc_1_double"),
    ("count", "rt_bc_1_count"),
    ("empty?", "rt_bc_1_empty_p"),
    ("peek", "rt_bc_1_peek"),
    ("first", "rt_bc_1_first"),
    ("pop!", "rt_bc_1_pop_bang"),
];

pub(crate) const OP2_SYMBOLS: &[(&str, &str)] = &[
    ("+", "rt_bc_2_add"),
    ("+'", "rt_bc_2_add"),
    ("-", "rt_bc_2_sub"),
    ("-'", "rt_bc_2_sub"),
    ("*", "rt_bc_2_mul"),
    ("*'", "rt_bc_2_mul"),
    ("<", "rt_bc_2_lt"),
    ("<=", "rt_bc_2_lte"),
    (">", "rt_bc_2_gt"),
    (">=", "rt_bc_2_gte"),
    ("==", "rt_bc_2_num_eq"),
    ("=", "rt_bc_2_eq"),
    ("not=", "rt_bc_2_not_eq"),
    ("quot", "rt_bc_2_quot"),
    ("rem", "rt_bc_2_rem"),
    ("mod", "rt_bc_2_mod"),
    ("bit-and", "rt_bc_2_bit_and"),
    ("bit-or", "rt_bc_2_bit_or"),
    ("bit-xor", "rt_bc_2_bit_xor"),
    ("bit-shift-left", "rt_bc_2_shl"),
    ("bit-shift-right", "rt_bc_2_shr"),
    ("unsigned-bit-shift-right", "rt_bc_2_ushr"),
    ("min", "rt_bc_2_min"),
    ("max", "rt_bc_2_max"),
    ("nth", "rt_bc_2_nth"),
    ("get", "rt_bc_2_get"),
    ("contains?", "rt_bc_2_contains"),
];

pub(crate) const OP3_SYMBOLS: &[(&str, &str)] = &[
    ("assoc!", "rt_bc_3_assoc_bang"),
    ("nth", "rt_bc_3_nth"),
    ("get", "rt_bc_3_get"),
];

/// Map `(clojure.core name, arity)` to its bitcode entrypoint symbol, if covered.
#[must_use]
pub fn bc_op_symbol(name: &str, arity: usize) -> Option<&'static str> {
    let table = match arity {
        1 => OP1_SYMBOLS,
        2 => OP2_SYMBOLS,
        3 => OP3_SYMBOLS,
        _ => return None,
    };
    table.iter().find(|(n, _)| *n == name).map(|(_, sym)| *sym)
}

/// Effect-position (`do` discarded result) transient bitcode symbol, skipping
/// the redundant transient `Arc` clone/drop when mutation succeeds.
#[must_use]
pub fn bc_fx_symbol(name: &str, arity: usize) -> Option<&'static str> {
    match (name, arity) {
        ("pop!", 1) => Some("rt_bc_1_pop_bang_fx"),
        ("assoc!", 3) => Some("rt_bc_3_assoc_bang_fx"),
        _ => None,
    }
}

type AsciiSlot = Cell<Option<(Weak<str>, bool)>>;

thread_local! {
    static ASCII_STRS: [AsciiSlot; 4] = const {
        [
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
            Cell::new(None),
        ]
    };
}

fn probe_ascii_slot(slots: &[AsciiSlot; 4], ptr: *const u8) -> Option<bool> {
    for (i, slot) in slots.iter().enumerate() {
        let Some(entry) = slot.take() else {
            continue;
        };
        let hit = entry.0.as_ptr().cast::<u8>() == ptr;
        let ascii = entry.1;
        if hit {
            if i > 0 {
                let front = slots[0].take();
                slots[0].set(Some(entry));
                slot.set(front);
            } else {
                slots[0].set(Some(entry));
            }
            return Some(ascii);
        }
        slot.set(Some(entry));
    }
    None
}

#[inline]
fn is_ascii_long(s: &Arc<str>) -> bool {
    let ptr = Arc::as_ptr(s).cast::<u8>();
    ASCII_STRS.with(|slots| {
        if let Some(ascii) = probe_ascii_slot(slots, ptr) {
            return ascii;
        }
        let ascii = s.is_ascii();
        let mut cur = Some((Arc::downgrade(s), ascii));
        for slot in slots {
            cur = slot.replace(cur);
        }
        ascii
    })
}

#[inline]
fn is_ascii_cached(s: &Arc<str>) -> bool {
    if s.len() <= MAX_INLINE_STR_BYTES {
        return s.is_ascii();
    }
    is_ascii_long(s)
}

#[inline]
fn write_val<O: OutSlot>(out: &mut O, val: Value) -> bool {
    out.write_value(val);
    true
}

#[inline(always)]
fn finish_bang<O: OutSlot>(out: &mut O, ok: bool, tval: &Value, keep_ret: bool) -> bool {
    if !ok {
        return false;
    }
    if keep_ret {
        clone_fast(out, tval)
    } else {
        write_val(out, Value::Nil)
    }
}

#[inline]
fn write_val_ref<O: OutSlot>(out: &mut O, vr: ValueRef<'_>) -> bool {
    match vr {
        ValueRef::Int(n) => write_val(out, Value::Int(n)),
        ValueRef::Char(c) => write_val(out, Value::Char(c)),
        ValueRef::Ref(v) => clone_fast(out, v),
    }
}

#[inline]
fn coll_count(v: &Value) -> Option<usize> {
    match v {
        Value::Nil => Some(0),
        Value::Vector(v) => Some(v.len()),
        Value::List(l) => Some(l.len()),
        Value::Map(m) => m.len_opt(),
        Value::Set(s) => Some(s.size()),
        Value::Str(s) if is_ascii_cached(s) => Some(s.len()),
        Value::Transient(t) => transient_vec_len(t),
        _ => None,
    }
}

#[inline(always)]
pub(crate) fn apply1<O: OutSlot>(out: &mut O, op: BcOp1, a: &Value) -> bool {
    match op {
        BcOp1::Inc | BcOp1::Dec | BcOp1::Neg => apply1_arith(out, op, a),
        BcOp1::ZeroP | BcOp1::PosP | BcOp1::NegP | BcOp1::EvenP | BcOp1::OddP => {
            apply1_num_pred(out, op, a)
        }
        BcOp1::IntCast | BcOp1::LongCast | BcOp1::DoubleCast => apply1_cast(out, op, a),
        BcOp1::BitNot => match a {
            Value::Int(x) => write_val(out, Value::Int(!x)),
            _ => false,
        },
        BcOp1::Not => write_val(
            out,
            Value::Bool(matches!(a, Value::Nil | Value::Bool(false))),
        ),
        BcOp1::NilP => write_val(out, Value::Bool(matches!(a, Value::Nil))),
        BcOp1::SomeP => write_val(out, Value::Bool(!matches!(a, Value::Nil))),
        BcOp1::StringP => write_val(out, Value::Bool(matches!(a, Value::Str(_)))),
        BcOp1::Count | BcOp1::EmptyP | BcOp1::Peek | BcOp1::First | BcOp1::PopBang => {
            apply1_coll(out, op, a)
        }
    }
}

#[inline(always)]
fn apply1_coll<O: OutSlot>(out: &mut O, op: BcOp1, a: &Value) -> bool {
    match op {
        BcOp1::Count => coll_count(a)
            .and_then(|n| i64::try_from(n).ok())
            .is_some_and(|n| write_val(out, Value::Int(n))),
        BcOp1::EmptyP => match a {
            Value::Str(s) => write_val(out, Value::Bool(s.is_empty())),
            _ => coll_count(a).is_some_and(|n| write_val(out, Value::Bool(n == 0))),
        },
        BcOp1::Peek => apply1_peek(out, a),
        BcOp1::First => apply1_first(out, a),
        _ => pop_bang_fast(out, a, true),
    }
}

#[inline]
fn apply1_arith<O: OutSlot>(out: &mut O, op: BcOp1, a: &Value) -> bool {
    match (op, a) {
        (BcOp1::Inc, Value::Int(x)) => x
            .checked_add(1)
            .is_some_and(|r| write_val(out, Value::Int(r))),
        (BcOp1::Inc, Value::Double(x)) => write_val(out, Value::Double(x + 1.0)),
        (BcOp1::Dec, Value::Int(x)) => x
            .checked_sub(1)
            .is_some_and(|r| write_val(out, Value::Int(r))),
        (BcOp1::Dec, Value::Double(x)) => write_val(out, Value::Double(x - 1.0)),
        (BcOp1::Neg, Value::Int(x)) => x
            .checked_neg()
            .is_some_and(|r| write_val(out, Value::Int(r))),
        (BcOp1::Neg, Value::Double(x)) => write_val(out, Value::Double(-x)),
        _ => false,
    }
}

#[inline]
fn apply1_num_pred<O: OutSlot>(out: &mut O, op: BcOp1, a: &Value) -> bool {
    let ans = match (op, a) {
        (BcOp1::ZeroP, Value::Int(x)) => *x == 0,
        (BcOp1::ZeroP, Value::Double(x)) => *x == 0.0,
        (BcOp1::PosP, Value::Int(x)) => *x > 0,
        (BcOp1::PosP, Value::Double(x)) => *x > 0.0,
        (BcOp1::NegP, Value::Int(x)) => *x < 0,
        (BcOp1::NegP, Value::Double(x)) => *x < 0.0,
        (BcOp1::EvenP, Value::Int(x)) => (*x & 1) == 0,
        (BcOp1::OddP, Value::Int(x)) => (*x & 1) != 0,
        _ => return false,
    };
    write_val(out, Value::Bool(ans))
}

#[inline]
fn apply1_cast<O: OutSlot>(out: &mut O, op: BcOp1, a: &Value) -> bool {
    match (op, a) {
        (BcOp1::IntCast, Value::Int(x)) if i32::try_from(*x).is_ok() => {
            write_val(out, Value::Int(*x))
        }
        (BcOp1::IntCast, Value::Char(c)) => write_val(out, Value::Int(i64::from(u32::from(*c)))),
        (BcOp1::LongCast, Value::Int(x)) => write_val(out, Value::Int(*x)),
        (BcOp1::LongCast, Value::Char(c)) => write_val(out, Value::Int(i64::from(u32::from(*c)))),
        (BcOp1::DoubleCast, Value::Int(x)) => write_val(out, Value::Double(*x as f64)),
        (BcOp1::DoubleCast, Value::Double(x)) => write_val(out, Value::Double(*x)),
        _ => false,
    }
}

#[inline]
fn apply1_peek<O: OutSlot>(out: &mut O, a: &Value) -> bool {
    match a {
        Value::Nil => write_val(out, Value::Nil),
        Value::List(l) => match l.iter().next() {
            Some(v) => clone_fast(out, v),
            None => write_val(out, Value::Nil),
        },
        // Queues ride on metadata-marked vectors (FIFO peek in clj-eval); bail to fallback.
        Value::Vector(v) if v.meta().is_none() => {
            match v.len().checked_sub(1).and_then(|last| v.get_val_ref(last)) {
                Some(vr) => write_val_ref(out, vr),
                None => write_val(out, Value::Nil),
            }
        }
        _ => false,
    }
}

#[inline]
fn apply1_first<O: OutSlot>(out: &mut O, a: &Value) -> bool {
    match a {
        Value::Nil => write_val(out, Value::Nil),
        Value::List(l) => match l.iter().next() {
            Some(v) => clone_fast(out, v),
            None => write_val(out, Value::Nil),
        },
        Value::Vector(v) => match v.get_val_ref(0) {
            Some(vr) => write_val_ref(out, vr),
            None => write_val(out, Value::Nil),
        },
        Value::Str(s) => match s.chars().next() {
            Some(c) if (c as u32) < 0x1_0000 => write_val(out, Value::Char(c)),
            None => write_val(out, Value::Nil),
            _ => false,
        },
        _ => false,
    }
}

#[inline]
pub(crate) fn pop_bang_fast<O: OutSlot>(out: &mut O, a: &Value, keep_ret: bool) -> bool {
    let Some(t) = is_transient_vec(a) else {
        return false;
    };
    let ok = {
        let mut guard = t.lock();
        if let TransientKind::Vector(tv) = &mut *guard {
            tv.pop_scalar_opt()
        } else {
            false
        }
    };
    finish_bang(out, ok, a, keep_ret)
}

#[inline(always)]
pub(crate) fn apply2<O: OutSlot>(out: &mut O, op: BcOp2, a: &Value, b: &Value) -> bool {
    match op {
        BcOp2::Add | BcOp2::Sub | BcOp2::Mul | BcOp2::Min | BcOp2::Max => {
            apply2_arith(out, op, a, b)
        }
        BcOp2::Lt | BcOp2::Lte | BcOp2::Gt | BcOp2::Gte | BcOp2::NumEq => apply2_cmp(out, op, a, b),
        BcOp2::Eq | BcOp2::NotEq => apply2_eq(out, op == BcOp2::NotEq, a, b),
        BcOp2::Quot
        | BcOp2::Rem
        | BcOp2::Mod
        | BcOp2::BitAnd
        | BcOp2::BitOr
        | BcOp2::BitXor
        | BcOp2::Shl
        | BcOp2::Shr
        | BcOp2::Ushr => apply2_int_ops(out, op, a, b),
        BcOp2::Nth => nth_fast(out, a, b, None),
        BcOp2::Get => get_fast(out, a, b, None),
        BcOp2::Contains => contains_fast(out, a, b),
    }
}

#[inline(always)]
fn apply2_arith<O: OutSlot>(out: &mut O, op: BcOp2, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            let res = match op {
                BcOp2::Add => x.checked_add(*y),
                BcOp2::Sub => x.checked_sub(*y),
                BcOp2::Mul => x.checked_mul(*y),
                BcOp2::Min => Some((*x).min(*y)),
                _ => Some((*x).max(*y)),
            };
            res.is_some_and(|r| write_val(out, Value::Int(r)))
        }
        (Value::Double(x), Value::Double(y)) => {
            float_arith(op, *x, *y).is_some_and(|v| write_val(out, Value::Double(v)))
        }
        (Value::Int(x), Value::Double(y)) => {
            float_arith(op, *x as f64, *y).is_some_and(|v| write_val(out, Value::Double(v)))
        }
        (Value::Double(x), Value::Int(y)) => {
            float_arith(op, *x, *y as f64).is_some_and(|v| write_val(out, Value::Double(v)))
        }
        _ => false,
    }
}

#[inline]
fn float_arith(op: BcOp2, x: f64, y: f64) -> Option<f64> {
    match op {
        BcOp2::Add => Some(x + y),
        BcOp2::Sub => Some(x - y),
        BcOp2::Mul => Some(x * y),
        // NaN semantics match Java `Math.min` / `Math.max` via fallback.
        BcOp2::Min if !x.is_nan() && !y.is_nan() => Some(if x <= y { x } else { y }),
        BcOp2::Max if !x.is_nan() && !y.is_nan() => Some(if x >= y { x } else { y }),
        _ => None,
    }
}

#[inline]
fn apply2_cmp<O: OutSlot>(out: &mut O, op: BcOp2, a: &Value, b: &Value) -> bool {
    let ans = match (a, b) {
        (Value::Int(x), Value::Int(y)) => cmp_ord(op, x, y),
        (Value::Double(x), Value::Double(y)) => cmp_f64(op, *x, *y),
        (Value::Int(x), Value::Double(y)) => cmp_f64(op, *x as f64, *y),
        (Value::Double(x), Value::Int(y)) => cmp_f64(op, *x, *y as f64),
        _ => return false,
    };
    write_val(out, Value::Bool(ans))
}

#[inline]
fn cmp_ord(op: BcOp2, x: &i64, y: &i64) -> bool {
    match op {
        BcOp2::Lt => x < y,
        BcOp2::Lte => x <= y,
        BcOp2::Gt => x > y,
        BcOp2::Gte => x >= y,
        _ => x == y,
    }
}

#[inline]
fn cmp_f64(op: BcOp2, x: f64, y: f64) -> bool {
    match op {
        BcOp2::Lt => x < y,
        BcOp2::Lte => x <= y,
        BcOp2::Gt => x > y,
        BcOp2::Gte => x >= y,
        _ => x == y,
    }
}

#[inline]
fn apply2_eq<O: OutSlot>(out: &mut O, invert: bool, a: &Value, b: &Value) -> bool {
    let eq = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        (Value::Double(x), Value::Double(y)) if !x.is_nan() && !y.is_nan() => x == y,
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (
            Value::Int(_) | Value::Char(_) | Value::Bool(_) | Value::Nil,
            Value::Nil | Value::Bool(_),
        )
        | (Value::Nil | Value::Bool(_), Value::Int(_) | Value::Char(_)) => false,
        _ => return false,
    };
    write_val(out, Value::Bool(eq ^ invert))
}

#[inline]
fn apply2_int_ops<O: OutSlot>(out: &mut O, op: BcOp2, a: &Value, b: &Value) -> bool {
    let (Value::Int(x), Value::Int(y)) = (a, b) else {
        return false;
    };
    let shift = (*y as u64) as u32 & 63;
    let res = match op {
        BcOp2::Quot => x.checked_div(*y),
        BcOp2::Rem => x.checked_rem(*y),
        BcOp2::Mod => java_mod(*x, *y),
        BcOp2::BitAnd => Some(x & y),
        BcOp2::BitOr => Some(x | y),
        BcOp2::BitXor => Some(x ^ y),
        BcOp2::Shl => Some(x.wrapping_shl(shift)),
        BcOp2::Shr => Some(x.wrapping_shr(shift)),
        _ => Some(((*x as u64).wrapping_shr(shift)) as i64),
    };
    res.is_some_and(|r| write_val(out, Value::Int(r)))
}

#[inline]
fn java_mod(x: i64, y: i64) -> Option<i64> {
    if y == 0 {
        return None;
    }
    let r = x.wrapping_rem(y);
    if r == 0 || (r > 0) == (y > 0) {
        Some(r)
    } else {
        r.checked_add(y)
    }
}

#[inline]
fn index_i32(v: &Value) -> Option<usize> {
    match v {
        Value::Int(n) => i32::try_from(*n).ok().and_then(|i| usize::try_from(i).ok()),
        _ => None,
    }
}

#[inline]
fn nth_str_fast(s: &Arc<str>, idx: usize) -> Option<Option<char>> {
    if is_ascii_cached(s) {
        return Some(s.as_bytes().get(idx).copied().map(char::from));
    }
    None
}

#[inline]
fn nth_fast<O: OutSlot>(out: &mut O, coll: &Value, idx: &Value, nf: Option<&Value>) -> bool {
    let Value::Int(n) = idx else {
        return false;
    };
    let Ok(i32_n) = i32::try_from(*n) else {
        return false;
    };
    let ok = usize::try_from(i32_n).ok();
    match coll {
        Value::Vector(v) => match ok.and_then(|i| v.get_val_ref(i)) {
            Some(vr) => write_val_ref(out, vr),
            None => nf.is_some_and(|d| clone_fast(out, d)),
        },
        Value::Str(s) => nth_str_slot(out, s, ok, nf),
        Value::Transient(t) => nth_transient(out, t, ok, nf),
        _ => false,
    }
}

#[inline]
fn nth_str_slot<O: OutSlot>(
    out: &mut O,
    s: &Arc<str>,
    ok: Option<usize>,
    nf: Option<&Value>,
) -> bool {
    match ok {
        Some(i) => match nth_str_fast(s, i) {
            Some(Some(c)) => write_val(out, Value::Char(c)),
            Some(None) => nf.is_some_and(|d| clone_fast(out, d)),
            None => false,
        },
        None => nf.is_some_and(|d| clone_fast(out, d)),
    }
}

#[inline]
fn nth_transient<O: OutSlot>(
    out: &mut O,
    t: &clj_value::Transient,
    ok: Option<usize>,
    nf: Option<&Value>,
) -> bool {
    let guard = t.lock();
    let TransientKind::Vector(tv) = &*guard else {
        return false;
    };
    match ok {
        Some(i) => match tv.get_val_ref(i) {
            Some(vr) => write_val_ref(out, vr),
            None if tv.len().is_ok() => nf.is_some_and(|d| clone_fast(out, d)),
            None => false,
        },
        None => tv.len().is_ok() && nf.is_some_and(|d| clone_fast(out, d)),
    }
}

#[inline]
fn write_not_found<O: OutSlot>(out: &mut O, nf: Option<&Value>) -> bool {
    match nf {
        Some(d) => clone_fast(out, d),
        None => write_val(out, Value::Nil),
    }
}

#[inline]
fn get_str_fast<O: OutSlot>(out: &mut O, s: &Arc<str>, key: &Value, nf: Option<&Value>) -> bool {
    let Value::Int(n) = key else {
        return false;
    };
    match usize::try_from(*n).ok() {
        Some(i) => match nth_str_fast(s, i) {
            Some(Some(c)) => write_val(out, Value::Char(c)),
            Some(None) => write_not_found(out, nf),
            None => false,
        },
        None => write_not_found(out, nf),
    }
}

#[inline]
fn get_fast<O: OutSlot>(out: &mut O, coll: &Value, key: &Value, nf: Option<&Value>) -> bool {
    match coll {
        Value::Nil => write_not_found(out, nf),
        Value::Vector(v) => match index_i32(key) {
            Some(i) => match v.get_val_ref(i) {
                Some(vr) => write_val_ref(out, vr),
                None => write_not_found(out, nf),
            },
            None => false,
        },
        Value::Map(m) => match m.get_small_scalar(key) {
            Some(Some(val)) => clone_fast(out, val),
            Some(None) => write_not_found(out, nf),
            None => false,
        },
        Value::Str(s) => get_str_fast(out, s, key, nf),
        _ => false,
    }
}

#[inline]
fn contains_fast<O: OutSlot>(out: &mut O, coll: &Value, key: &Value) -> bool {
    let ans = match coll {
        Value::Nil => false,
        Value::Map(m) => match m.get_small_scalar(key) {
            Some(found) => found.is_some(),
            None => return false,
        },
        Value::Vector(v) => match key {
            Value::Int(n) => usize::try_from(*n).is_ok_and(|idx| idx < v.len()),
            _ => return false,
        },
        _ => return false,
    };
    write_val(out, Value::Bool(ans))
}

#[inline(always)]
pub(crate) fn apply3<O: OutSlot>(out: &mut O, op: BcOp3, a: &Value, b: &Value, c: &Value) -> bool {
    match op {
        BcOp3::Nth3 => nth_fast(out, a, b, Some(c)),
        BcOp3::Get3 => get_fast(out, a, b, Some(c)),
        BcOp3::AssocBang => assoc_bang_fast(out, a, b, c, true),
    }
}

#[inline]
pub(crate) fn assoc_bang_fast<O: OutSlot>(
    out: &mut O,
    tval: &Value,
    key: &Value,
    val: &Value,
    keep_ret: bool,
) -> bool {
    let Some(t) = is_transient_vec(tval) else {
        return false;
    };
    let Some(idx) = index_i32(key) else {
        return false;
    };
    let Value::Int(n) = val else {
        return false;
    };
    let ok = {
        let mut guard = t.lock();
        if let TransientKind::Vector(tv) = &mut *guard {
            tv.assoc_int_in_bounds(idx, *n)
        } else {
            false
        }
    };
    finish_bang(out, ok, tval, keep_ret)
}
