// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! VM intrinsics (#226): the three-argument hot `clojure.core`
//! calls (`nth`/`get` with a not-found value, `assoc!`) computed in
//! place, plus the shared fast-path helpers behind the
//! shape-specialized ops (`crate::vm_ops`).
//!
//! A direct call to one of these (still validated per site like any
//! direct native) reads its local and literal operands by reference
//! — no argument buffer, no clone of the collection — and answers
//! inline when every argument has a type the fast path covers. Each
//! covered case returns exactly what the native returns; anything
//! else (other types, out-of-range indexes, errors) runs the native
//! itself over the same values, so every error text and edge case
//! stays the native's.

use clj_value::{TransientKind, Value};

/// `mod` over `Int`s (`None` for a zero divisor): Java `%`
/// adjusted to the divisor's sign (`(mod MIN -1)` is 0).
pub(crate) fn java_mod(x: i64, y: i64) -> Option<i64> {
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

/// An index that every `int`-coercing reader takes as-is
/// (`0..=i32::MAX`).
pub(crate) fn index(v: &Value) -> Option<usize> {
    match v {
        Value::Int(n) if (0..=i64::from(i32::MAX)).contains(n) => usize::try_from(*n).ok(),
        _ => None,
    }
}

/// `nth` of a vector, transient vector, or string at an `int`
/// index: in range answers the item; with `not-found`, past either
/// end answers that (the native's `int` coercion fails wider
/// indexes).
pub(crate) fn nth_of(coll: &Value, idx: &Value, not_found: Option<&Value>) -> Option<Value> {
    let Value::Int(n) = idx else {
        return None;
    };
    let i = usize::try_from(i32::try_from(*n).ok()?).ok();
    match (nth_item(coll, i)?, not_found) {
        (Some(v), _) => Some(v),
        (None, Some(nf)) => Some(nf.clone()),
        (None, None) => None,
    }
}

fn nth_item(coll: &Value, i: Option<usize>) -> Option<Option<Value>> {
    match coll {
        Value::Vector(v) => Some(i.and_then(|i| v.get(i))),
        Value::Array(a) => {
            Some(i.and_then(|i| (i < a.len()).then(|| crate::arrays::array_read(a, i))))
        }
        Value::Str(s) => Some(i.and_then(|i| crate::coll::str_char_at(s, i).map(Value::Char))),
        Value::Transient(t) => transient_nth(t, i),
        Value::Extension(_) => crate::lazy::as_subvector(coll).map(|sv| i.and_then(|i| sv.nth(i))),
        _ => None,
    }
}

/// `nth` of a transient vector (`None` for other transients — the
/// native's error — or a frozen one).
fn transient_nth(t: &clj_value::Transient, i: Option<usize>) -> Option<Option<Value>> {
    match &*t.lock() {
        TransientKind::Vector(tv) => match i {
            Some(i) => tv.nth(i).ok(),
            None => Some(None),
        },
        _ => None,
    }
}

/// `get` of a map/set (scalar keys), vector (in-range `int` keys),
/// or string (`int` keys), `nil` included.
pub(crate) fn get_of(coll: &Value, key: &Value, not_found: Option<&Value>) -> Option<Value> {
    let found = match coll {
        Value::Nil => Some(None),
        Value::Map(m) => get_map(m, key),
        Value::Set(s) => get_set(s, key),
        Value::Vector(v) => index(key).map(|i| v.get(i)),
        Value::Str(s) => get_str(s, key),
        Value::Transient(t) => get_transient(t, key),
        Value::Extension(_) => get_subvec(coll, key),
        _ => None,
    }?;
    Some(found.unwrap_or_else(|| not_found.map_or(Value::Nil, Value::clone)))
}

fn get_map(m: &clj_value::Map, key: &Value) -> Option<Option<Value>> {
    (!m.is_sorted() && is_scalar_key(key)).then(|| m.get(key).cloned())
}

fn get_set(s: &clj_value::Set, key: &Value) -> Option<Option<Value>> {
    (!s.is_sorted() && is_scalar_key(key)).then(|| s.get(key).cloned())
}

fn get_subvec(coll: &Value, key: &Value) -> Option<Option<Value>> {
    let sv = crate::lazy::as_subvector(coll)?;
    Some(index(key).and_then(|i| sv.nth(i)))
}

fn get_str(s: &std::sync::Arc<str>, key: &Value) -> Option<Option<Value>> {
    let Value::Int(n) = key else {
        return None;
    };
    Some(
        usize::try_from(*n as i32)
            .ok()
            .and_then(|i| crate::coll::str_char_at(s, i).map(Value::Char)),
    )
}

fn get_transient(t: &clj_value::Transient, key: &Value) -> Option<Option<Value>> {
    if !is_scalar_key(key) {
        return None;
    }
    let guard = t.lock();
    let TransientKind::Map(tm) = &*guard else {
        return None;
    };
    Some(tm.get(key).ok()?.cloned())
}

/// Keys whose lookup needs no realization (equality is structural
/// and cheap).
pub(crate) fn is_scalar_key(k: &Value) -> bool {
    matches!(
        k,
        Value::Keyword(_)
            | Value::Int(_)
            | Value::Str(_)
            | Value::Char(_)
            | Value::Symbol(_)
            | Value::Bool(_)
            | Value::Nil
    )
}

/// `count` of `nil`, a vector, map, set, or string (UTF-16 units,
/// like the native).
/// `count` of a live transient, by kind (`None` once frozen, so
/// the caller falls back to the native error).
fn transient_len(t: &clj_value::Transient) -> Option<usize> {
    let guard = t.lock();
    match &*guard {
        TransientKind::Vector(tv) => tv.len().ok(),
        TransientKind::Map(tm) => tm.len().ok(),
        TransientKind::Set(ts) => ts.len().ok(),
    }
}

pub(crate) fn count_of(v: &Value) -> Option<Value> {
    let n = match v {
        Value::Nil => 0,
        Value::Vector(v) => v.len(),
        Value::Array(a) => a.len(),
        Value::Map(m) => m.size(),
        Value::Set(s) => s.size(),
        Value::Str(s) => crate::coll::str_utf16_len(s),
        Value::Transient(t) => transient_len(t)?,
        Value::Extension(_) => crate::lazy::as_subvector(v)?.len(),
        _ => return None,
    };
    i64::try_from(n).ok().map(Value::Int)
}

/// `contains?` of an unsorted map or set at a scalar key.
pub(crate) fn contains_test(coll: &Value, key: &Value) -> Option<bool> {
    match coll {
        Value::Map(m) if !m.is_sorted() && is_scalar_key(key) => Some(m.contains_key(key)),
        Value::Set(s) if !s.is_sorted() && is_scalar_key(key) => Some(s.contains(key)),
        _ => None,
    }
}

/// `=` over same-kind scalars (`None` for every other pair: numeric
/// categories, collections, and mixed kinds take the native).
#[inline]
pub(crate) fn scalar_eq(a: &Value, b: &Value) -> Option<bool> {
    Some(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => return None,
    })
}
