// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Owned vector updates (#215): `set_nth_owned`/`drop_last_owned`
//! and the owned primitive-vector set mutate uniquely-owned
//! storage in place (the #212 pattern) while versions shared with
//! other handles still path-copy. Every construction mints a fresh
//! id; the `&self` forms stay for genuinely shared sites (the
//! subvector writers).

use clj_value::{Elem8, PrimitiveVector, Value, Vector};

fn ints(n: i64) -> Vec<Value> {
    (0..n).map(Value::Int).collect()
}

/// A generic-trie vector (the trailing `Nil` defeats packing).
fn generic(n: i64) -> Vector {
    let mut items = ints(n);
    items.push(Value::Nil);
    Vector::from_vec(items)
}

fn pv(items: Vec<Value>) -> PrimitiveVector {
    PrimitiveVector::new(Elem8::Long, items, None)
}

/// Owned `set_nth` on the generic trie: the answer carries the new
/// item under a fresh id, and a pre-existing clone still reads the
/// old item (shared nodes path-copied, never clobbered).
#[test]
fn set_nth_owned_generic_sets_and_preserves_sharing() {
    let v = generic(10);
    assert!(!v.is_packed());
    let snapshot = v.clone();
    let w = v.set_nth_owned(3, Value::Int(99)).unwrap();
    assert_ne!(w.id(), snapshot.id());
    assert_eq!(w.get(3), Some(Value::Int(99)));
    assert_eq!(w.len(), 11);
    assert_eq!(snapshot.get(3), Some(Value::Int(3)));
    assert_eq!(snapshot.len(), 11);
}

/// Owned `set_nth` out of bounds is `None` on the generic trie.
#[test]
fn set_nth_owned_generic_oob_is_none() {
    let v = generic(10);
    assert!(v.set_nth_owned(11, Value::Int(0)).is_none());
    let v = generic(10);
    assert!(v.set_nth_owned(999, Value::Int(0)).is_none());
}

/// Owned `set_nth` of an `Int` on packed storage stays packed.
#[test]
fn set_nth_owned_packed_int_stays_packed() {
    let v = Vector::from_vec(ints(10));
    let before = v.clone();
    let w = v.set_nth_owned(3, Value::Int(99)).unwrap();
    assert!(w.is_packed());
    assert_ne!(w.id(), before.id());
    assert_eq!(w.get(3), Some(Value::Int(99)));
    assert_eq!(before.get(3), Some(Value::Int(3)));
}

/// Owned `set_nth` of an `Int` past the packed end is `None`.
#[test]
fn set_nth_owned_packed_int_oob_is_none() {
    let v = Vector::from_vec(ints(10));
    assert!(v.set_nth_owned(10, Value::Int(0)).is_none());
}

/// Owned `set_nth` of a foreign value unpacks once, keeping every
/// item in order under a fresh id.
#[test]
fn set_nth_owned_packed_foreign_unpacks() {
    let v = Vector::from_vec(ints(10));
    let before = v.clone();
    let w = v.set_nth_owned(3, Value::Nil).unwrap();
    assert!(!w.is_packed());
    assert_ne!(w.id(), before.id());
    assert_eq!(w.len(), 10);
    assert_eq!(w.get(3), Some(Value::Nil));
    assert_eq!(w.get(4), Some(Value::Int(4)));
    assert!(before.is_packed());
    assert_eq!(before.get(3), Some(Value::Int(3)));
}

/// Owned `set_nth` of a foreign value past the packed end is `None`.
#[test]
fn set_nth_owned_packed_foreign_oob_is_none() {
    let v = Vector::from_vec(ints(10));
    assert!(v.set_nth_owned(999, Value::Nil).is_none());
}

/// Owned `set_nth` keeps metadata (moved, not cloned).
#[test]
fn set_nth_owned_keeps_meta() {
    let v = generic(4).with_meta(Some(clj_value::new_map()));
    assert!(v.meta().is_some());
    let w = v.set_nth_owned(0, Value::Int(7)).unwrap();
    assert!(w.meta().is_some());
    assert_eq!(w.get(0), Some(Value::Int(7)));
}

/// Owned `drop_last` on the generic trie: shorter answer under a
/// fresh id, pre-existing clone intact.
#[test]
fn drop_last_owned_generic_shortens_and_preserves_sharing() {
    let v = generic(4);
    let snapshot = v.clone();
    let w = v.drop_last_owned().unwrap();
    assert_ne!(w.id(), snapshot.id());
    assert_eq!(w.len(), 4);
    assert_eq!(w.get(3), Some(Value::Int(3)));
    assert_eq!(snapshot.len(), 5);
}

/// Owned `drop_last` on an empty generic trie is `None` (drained
/// from a one-item generic vector — fresh empties pack).
#[test]
fn drop_last_owned_generic_empty_is_none() {
    let one = Vector::from_vec(vec![Value::Nil]);
    assert!(!one.is_packed());
    let empty = one.drop_last_owned().unwrap();
    assert!(empty.is_empty());
    assert!(!empty.is_packed());
    assert!(empty.drop_last_owned().is_none());
}

/// Owned `drop_last` on packed storage stays packed.
#[test]
fn drop_last_owned_packed_stays_packed() {
    let v = Vector::from_vec(ints(10));
    let before = v.clone();
    let w = v.drop_last_owned().unwrap();
    assert!(w.is_packed());
    assert_ne!(w.id(), before.id());
    assert_eq!(w.len(), 9);
    assert_eq!(before.len(), 10);
}

/// Owned `drop_last` on an empty packed vector is `None`.
#[test]
fn drop_last_owned_packed_empty_is_none() {
    let empty = Vector::new_sync();
    assert!(empty.is_packed());
    assert!(empty.drop_last_owned().is_none());
}

/// The `&self` forms still serve shared sites: borrow, update, and
/// the original reads unchanged.
#[test]
fn shared_forms_still_serve_borrowed_vectors() {
    let v = generic(4);
    let w = v.set_nth(1, Value::Int(42)).unwrap();
    assert_eq!(w.get(1), Some(Value::Int(42)));
    assert_eq!(v.get(1), Some(Value::Int(1)));
    let t = v.drop_last().unwrap();
    assert_eq!(t.len(), 4);
    assert_eq!(v.len(), 5);
    assert_ne!(w.id(), v.id());
    assert_ne!(t.id(), v.id());
}

/// Owned primitive `push_back` appends under a fresh id.
#[test]
fn pv_push_back_appends() {
    let p = pv(vec![Value::Int(1), Value::Int(2)]);
    let id = p.id();
    let q = p.push_back(Value::Int(3));
    assert_ne!(q.id(), id);
    assert_eq!(q.len(), 3);
    assert_eq!(q.items(), vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
}

/// Owned primitive `set_nth`: in-bounds sets, OOB is `None`, and a
/// pre-existing clone still reads the old item.
#[test]
fn pv_set_nth_sets_and_preserves_sharing() {
    let p = pv(vec![Value::Int(1), Value::Int(2)]);
    let snapshot = p.clone();
    let q = p.set_nth(0, Value::Int(9)).unwrap();
    assert_ne!(q.id(), snapshot.id());
    assert_eq!(q.get(0), Some(&Value::Int(9)));
    assert_eq!(snapshot.get(0), Some(&Value::Int(1)));
    let p = pv(vec![Value::Int(1)]);
    assert!(p.set_nth(1, Value::Int(9)).is_none());
}

/// Owned primitive `drop_last`: non-empty shortens, empty is `None`.
#[test]
fn pv_drop_last_shortens_or_none() {
    let p = pv(vec![Value::Int(1), Value::Int(2)]);
    let id = p.id();
    let q = p.drop_last().unwrap();
    assert_ne!(q.id(), id);
    assert_eq!(q.len(), 1);
    let empty = PrimitiveVector::empty(Elem8::Long);
    assert!(empty.drop_last().is_none());
}
