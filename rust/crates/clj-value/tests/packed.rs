// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Packed-`Int` vector pins (#221 P1): all-`Int` vectors ride on 8
//! bytes per element instead of one 72-byte `Value` per trie leaf.
//! Packing is representation-only — `=`, `hash`, `identical?`, `nth`,
//! `count`, and iteration must agree with the generic twin.

use clj_value::{TransientVector, Value, Vector};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn hash_of(v: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn ints(n: i64) -> Vec<Value> {
    (0..n).map(Value::Int).collect()
}

/// All-`Int` construction packs; items round-trip exactly.
#[test]
fn all_int_from_vec_stays_packed() {
    let v = Vector::from_vec(ints(1000));
    assert!(v.is_packed());
    assert_eq!(v.len(), 1000);
    let back: Vec<Value> = v.iter().collect();
    assert_eq!(back, ints(1000));
}

/// The empty vector packs vacuously (no scan cost, no trie nodes).
#[test]
fn empty_from_vec_is_packed() {
    let v = Vector::from_vec(Vec::new());
    assert!(v.is_packed());
    assert!(v.is_empty());
}

/// One non-`Int` item keeps the generic trie.
#[test]
fn mixed_from_vec_unpacks() {
    let mut items = ints(100);
    items.push(Value::Nil);
    let v = Vector::from_vec(items.clone());
    assert!(!v.is_packed());
    let back: Vec<Value> = v.iter().collect();
    assert_eq!(back, items);
}

/// Pushing `Int`s keeps packing; pushing anything else unpacks once
/// and preserves every item in order.
#[test]
fn push_back_int_stays_packed_until_foreign_item() {
    let mut v = Vector::from_vec(ints(10));
    assert!(v.is_packed());
    v = v.push_back(Value::Int(10));
    assert!(v.is_packed());
    assert_eq!(v.len(), 11);
    v = v.push_back(Value::Str("x".into()));
    assert!(!v.is_packed());
    let back: Vec<Value> = v.iter().collect();
    let mut want = ints(11);
    want.push(Value::Str("x".into()));
    assert_eq!(back, want);
}

/// `=` and `hash` agree across the packing boundary.
#[test]
fn equiv_and_hash_agree_across_packing() {
    let packed = Value::Vector(Vector::from_vec(ints(500)));
    assert!(matches!(&packed, Value::Vector(v) if v.is_packed()));
    // Force the generic twin through one foreign push, then pop it
    // back off via subvec-free rebuild: build generic by pushing a
    // string and unpacking, then compare prefix equality instead.
    let mut generic = Vector::from_vec(ints(500));
    assert!(generic.is_packed());
    generic = generic.push_back(Value::Nil);
    assert!(!generic.is_packed());
    let generic_prefix: Vec<Value> = generic.iter().take(500).collect();
    assert_eq!(generic_prefix, ints(500));
    // Same items through from_ints hash identically to from_vec.
    let a = Value::Vector(Vector::from_ints((0..500).collect()));
    assert_eq!(packed, a);
    assert_eq!(hash_of(&packed), hash_of(&a));
}

/// `identical?` is allocation identity, not packing: two separately
/// built packed vectors are `=` but never `identical?`.
#[test]
fn identical_is_allocation_identity_not_packing() {
    let a = Vector::from_vec(ints(100));
    let b = Vector::from_vec(ints(100));
    assert!(a.is_packed() && b.is_packed());
    assert_eq!(Value::Vector(a.clone()), Value::Vector(b.clone()));
    assert_ne!(a.id(), b.id());
    // Cloning a handle shares the identity (same allocation).
    assert_eq!(a.id(), a.clone().id());
}

/// `nth`/`get`/`len`/`count` read packed storage directly.
#[test]
fn indexed_access_reads_packed_storage() {
    let v = Vector::from_vec(ints(100));
    assert!(v.is_packed());
    assert_eq!(v.len(), 100);
    assert_eq!(v.get(0), Some(Value::Int(0)));
    assert_eq!(v.get(99), Some(Value::Int(99)));
    assert_eq!(v.get(100), None);
}

/// Transient `conj!` of `Int`s stays packed; the first foreign
/// value unpacks once and preserves every item.
#[test]
fn transient_conj_int_stays_packed_until_foreign() {
    let mut tv = TransientVector::new();
    assert!(tv.is_packed());
    for i in 0..1000 {
        tv.conj(Value::Int(i)).unwrap();
    }
    assert!(tv.is_packed());
    assert_eq!(tv.len().unwrap(), 1000);
    assert_eq!(tv.nth(999).unwrap(), Some(Value::Int(999)));
    tv.conj(Value::Nil).unwrap();
    assert!(!tv.is_packed());
    assert_eq!(tv.nth(1000).unwrap(), Some(Value::Nil));
}

/// `persistent!` moves the packed run without copying (the frozen
/// vector is packed and `=` to the generic twin).
#[test]
fn transient_persistent_is_zero_copy_packed() {
    let mut tv = TransientVector::new();
    for i in 0..1000 {
        tv.conj(Value::Int(i)).unwrap();
    }
    let frozen = tv.persistent().unwrap();
    assert!(frozen.is_packed());
    assert_eq!(
        Value::Vector(frozen),
        Value::Vector(Vector::from_ints((0..1000).collect()))
    );
}

/// `transient` over a packed vector stays packed; over a generic
/// vector stays generic.
#[test]
fn transient_from_vector_preserves_packing() {
    let packed = Vector::from_vec(ints(100));
    assert!(packed.is_packed());
    let tv = TransientVector::from_vector(&packed);
    assert!(tv.is_packed());
    let mut items = ints(100);
    items.push(Value::Nil);
    let generic = Vector::from_vec(items);
    assert!(!generic.is_packed());
    let tv2 = TransientVector::from_vector(&generic);
    assert!(!tv2.is_packed());
}

/// `set_nth` on packed storage: in-bounds `Int`s stay packed,
/// foreign values unpack once, out-of-bounds is `None`.
#[test]
fn set_nth_packed_arms() {
    let v = Vector::from_vec(ints(10));
    let w = v.set_nth(3, Value::Int(99)).unwrap();
    assert!(w.is_packed());
    assert_eq!(w.get(3), Some(Value::Int(99)));
    let w = v.set_nth(3, Value::Nil).unwrap();
    assert!(!w.is_packed());
    assert_eq!(w.get(3), Some(Value::Nil));
    assert_eq!(w.len(), 10);
    assert_eq!(v.set_nth(10, Value::Int(0)), None);
    assert_eq!(v.set_nth(999, Value::Nil), None);
}

/// `drop_last` on packed storage (empty is `None`).
#[test]
fn drop_last_packed_arms() {
    let v = Vector::from_vec(ints(10));
    let w = v.drop_last().unwrap();
    assert!(w.is_packed());
    assert_eq!(w.len(), 9);
    let empty = Vector::from_vec(Vec::new());
    assert!(empty.is_packed());
    assert_eq!(empty.drop_last(), None);
}

/// Transient `pop`/`assoc`/`len` work on generic buffers too (after
/// the first unpack), and `Int`s keep pushing packed-style.
#[test]
fn transient_generic_arms() {
    let mut tv = TransientVector::new();
    for i in 0..10 {
        tv.conj(Value::Int(i)).unwrap();
    }
    // Packed `assoc`: set in place, append at `len`.
    tv.assoc(3, Value::Int(99)).unwrap();
    assert!(tv.is_packed());
    assert_eq!(tv.nth(3).unwrap(), Some(Value::Int(99)));
    tv.assoc(10, Value::Int(100)).unwrap();
    assert!(tv.is_packed());
    assert_eq!(tv.len().unwrap(), 11);
    // First foreign value unpacks; the second reuses generic.
    tv.conj(Value::Nil).unwrap();
    assert!(!tv.is_packed());
    tv.conj(Value::Bool(true)).unwrap();
    assert!(!tv.is_packed());
    // `Int`s conj onto generic buffers too.
    tv.conj(Value::Int(102)).unwrap();
    assert_eq!(tv.len().unwrap(), 14);
    // A foreign `assoc!` onto a packed buffer unpacks in place.
    let mut tv2 = TransientVector::new();
    tv2.conj(Value::Int(1)).unwrap();
    tv2.assoc(0, Value::Nil).unwrap();
    assert!(!tv2.is_packed());
    assert_eq!(tv2.nth(0).unwrap(), Some(Value::Nil));
    // `persistent!` on a generic buffer freezes generically.
    let frozen = tv2.persistent().unwrap();
    assert!(!frozen.is_packed());
    assert_eq!(frozen.len(), 1);
    // Generic `assoc`: set in place, append at `len`.
    tv.assoc(0, Value::Int(-1)).unwrap();
    assert_eq!(tv.nth(0).unwrap(), Some(Value::Int(-1)));
    tv.assoc(14, Value::Int(101)).unwrap();
    assert_eq!(tv.nth(14).unwrap(), Some(Value::Int(101)));
    tv.assoc(15, Value::Nil).unwrap();
    assert_eq!(tv.len().unwrap(), 16);
    tv.assoc(1, Value::Str("x".into())).unwrap();
    assert_eq!(tv.nth(1).unwrap(), Some(Value::Str("x".into())));
    // Generic `pop` drains to empty (then errors).
    for _ in 0..16 {
        tv.pop().unwrap();
    }
    assert!(tv.is_empty().unwrap());
    assert!(tv.pop().is_err());
}

/// Long runs stay packed (#226): packed storage is the same
/// persistent trie as generic, so a shared update path-copies in log
/// time at any length (the #227 whole-run clone, and the 1024 cap it
/// forced, are gone).
#[test]
fn long_runs_stay_packed() {
    let v = Vector::from_vec(ints(1025));
    assert!(v.is_packed());
    let back: Vec<Value> = v.iter().collect();
    assert_eq!(back, ints(1025));
    let w = Vector::from_ints((0..2000).collect());
    assert!(w.is_packed());
    assert_eq!(w.len(), 2000);
    assert_eq!(w.get(1999), Some(Value::Int(1999)));
}

/// `push_back` of an `Int` keeps a long vector packed, and shared
/// versions stay intact; a foreign value converts once.
#[test]
fn push_back_keeps_packed_and_persistent() {
    let mut v = Vector::from_vec(ints(1024));
    v = v.push_back(Value::Int(1024));
    assert!(v.is_packed());
    assert_eq!(v.len(), 1025);
    let back: Vec<Value> = v.iter().collect();
    assert_eq!(back, ints(1025));
    let shared = v.clone();
    let w = v.push_back(Value::Int(1025));
    assert_eq!(w.len(), 1026);
    assert_eq!(shared.len(), 1025);
    assert_eq!(shared.get(1024), Some(Value::Int(1024)));
    assert_eq!(w.get(1025), Some(Value::Int(1025)));
    let mixed = shared.push_back(Value::Nil);
    assert!(!mixed.is_packed());
    assert_eq!(mixed.get(1025), Some(Value::Nil));
    assert_eq!(mixed.get(1024), Some(Value::Int(1024)));
}

/// `assoc` on a long `Int` vector keeps every version intact (the
/// trie path-copies; no whole-run clone per update).
#[test]
fn set_nth_long_int_vector_persists() {
    let v = Vector::from_vec(ints(5000));
    assert!(v.is_packed());
    let w = v.set_nth(4999, Value::Int(7)).unwrap();
    assert_eq!(w.get(4999), Some(Value::Int(7)));
    assert_eq!(v.get(4999), Some(Value::Int(4999)));
    assert_eq!(w.len(), 5000);
    assert!(v.set_nth(5000, Value::Int(1)).is_none());
}

/// `persistent!` of a long packed transient freezes packed with
/// every item intact.
#[test]
fn transient_persistent_long_freezes_packed() {
    let mut tv = TransientVector::new();
    for i in 0..2000 {
        tv.conj(Value::Int(i)).unwrap();
    }
    assert!(tv.is_packed());
    let frozen = tv.persistent().unwrap();
    assert!(frozen.is_packed());
    assert_eq!(frozen.len(), 2000);
    let back: Vec<Value> = frozen.iter().collect();
    assert_eq!(back, ints(2000));
}

fn chars(s: &str) -> Vec<Value> {
    s.chars().map(Value::Char).collect()
}

/// A generic vector holding exactly `items` (packing never applies:
/// it grows from a generic empty vector).
fn generic_of(items: Vec<Value>) -> Vector {
    let empty = Vector::from_vec(vec![Value::Nil]).drop_last().unwrap();
    assert!(!empty.is_packed());
    items.into_iter().fold(empty, Vector::push_back)
}

/// All-char vectors pack (4 bytes per char — a vector of a string's
/// chars was 16x its text); packing stays invisible to `=`, `hash`,
/// `nth`, chunks and iteration.
#[test]
fn char_vectors_pack_invisibly() {
    let packed = Vector::from_vec(chars("hello, wörld"));
    assert!(packed.is_packed());
    let generic = generic_of(chars("hello, wörld"));
    assert!(!generic.is_packed());
    let (p, g) = (Value::Vector(packed.clone()), Value::Vector(generic));
    assert_eq!(p, g);
    assert_eq!(hash_of(&p), hash_of(&g));
    assert_eq!(packed.get(8), Some(Value::Char('ö')));
    assert_eq!(packed.iter().next_back(), Some(Value::Char('d')));
    assert_eq!(packed.chunk_from(10).len(), 2);
    assert_eq!(Vector::from_vec(ints(40)).chunk_from(3).len(), 29);
    // Writes: chars stay packed; a foreign value converts once.
    let w = packed.clone().push_back(Value::Char('!'));
    assert!(w.is_packed() && w.len() == 13);
    let s = packed.set_nth(0, Value::Char('H')).unwrap();
    assert!(s.is_packed() && s.get(0) == Some(Value::Char('H')));
    let f = packed.set_nth(0, Value::Int(1)).unwrap();
    assert!(!f.is_packed() && f.get(1) == Some(Value::Char('e')));
    let n = packed.clone().push_back(Value::Int(1));
    assert!(!n.is_packed() && n.get(12) == Some(Value::Int(1)));
    let d = packed.drop_last().unwrap();
    assert!(d.is_packed() && d.len() == 11);
    // Mixed input stays generic whichever kind comes first.
    assert!(!Vector::from_vec(vec![Value::Char('a'), Value::Int(1)]).is_packed());
    assert!(!Vector::from_vec(vec![Value::Int(1), Value::Char('a')]).is_packed());
}

/// An empty packed vector takes the type of its first item, either
/// way round.
#[test]
fn empty_packed_vectors_retype() {
    let c = Vector::new_sync().push_back(Value::Char('a'));
    assert!(c.is_packed() && c.get(0) == Some(Value::Char('a')));
    let i = c.drop_last().unwrap().push_back(Value::Int(7));
    assert!(i.is_packed() && i.get(0) == Some(Value::Int(7)));
    let g = Vector::new_sync().push_back(Value::Nil);
    assert!(!g.is_packed());
    let e = Vector::from_vec(Vec::new());
    assert!(e.is_packed() && e.is_empty());
}

/// Char transients pack and freeze packed; empties retype; foreign
/// values convert to generic, either way round.
#[test]
fn char_transients_pack() {
    let mut tv = TransientVector::new();
    tv.conj(Value::Char('a')).unwrap();
    tv.conj(Value::Char('b')).unwrap();
    tv.assoc(0, Value::Char('z')).unwrap();
    assert!(tv.is_packed());
    assert_eq!(tv.nth(0).unwrap(), Some(Value::Char('z')));
    tv.pop().unwrap();
    let frozen = tv.persistent().unwrap();
    assert!(frozen.is_packed() && frozen.len() == 1);
    // From a packed char vector, then a foreign value.
    let mut tv = TransientVector::from_vector(&Vector::from_vec(chars("ab")));
    assert!(tv.is_packed());
    tv.conj(Value::Int(1)).unwrap();
    assert!(!tv.is_packed());
    assert_eq!(tv.nth(2).unwrap(), Some(Value::Int(1)));
    // An `Int` buffer meeting a char converts too.
    let mut tv = TransientVector::from_vector(&Vector::from_vec(ints(2)));
    tv.conj(Value::Char('x')).unwrap();
    assert!(!tv.is_packed());
    // An emptied char buffer retypes to `Int`.
    let mut tv = TransientVector::from_vector(&Vector::from_vec(chars("a")));
    tv.pop().unwrap();
    tv.conj(Value::Int(3)).unwrap();
    assert!(tv.is_packed());
    assert_eq!(tv.persistent().unwrap().get(0), Some(Value::Int(3)));
}

/// Metadata rides alongside packed storage (vectors stay `IObj`).
#[test]
fn meta_survives_packing() {
    let v = Vector::from_vec(ints(10));
    assert!(v.is_packed());
    assert!(v.meta().is_none());
    let m = clj_value::new_map();
    let vm = v.with_meta(Some(m));
    assert!(vm.is_packed());
    assert!(vm.meta().is_some());
    let back: Vec<Value> = vm.iter().collect();
    assert_eq!(back, ints(10));
}

/// `from_vector_owned` unwraps packed ints, chars, and generic vectors.
#[test]
fn transient_from_vector_owned_arms() {
    let gv = Vector::from_vec(vec![Value::Nil, Value::Int(1)]);
    let tv = TransientVector::from_vector_owned(gv);
    assert!(!tv.is_packed());

    let cv = Vector::from_chars(vec!['a', 'b', 'c']);
    let tv = TransientVector::from_vector_owned(cv);
    assert!(tv.is_packed());

    let iv_flat = Vector::from_ints(vec![1, 2, 3]);
    let tv = TransientVector::from_vector_owned(iv_flat);
    assert!(tv.is_packed());

    let big_ints = (0..2000).collect::<Vec<i64>>();
    let iv_big = Vector::from_ints(big_ints);
    let tv = TransientVector::from_vector_owned(iv_big);
    assert!(tv.is_packed());
    assert_eq!(tv.len().unwrap(), 2000);
}

#[test]
fn vector_push_to_string() {
    let flat_cv = Vector::from_chars(vec!['h', 'e', 'l', 'l', 'o']);
    let mut s = String::new();
    assert!(flat_cv.push_to_string(&mut s));
    assert_eq!(s, "hello");

    let big_chars: Vec<char> = (0..2000)
        .map(|i| if i % 2 == 0 { 'a' } else { 'b' })
        .collect();
    let trie_cv = Vector::from_chars(big_chars.clone());
    let mut s2 = String::new();
    assert!(trie_cv.push_to_string(&mut s2));
    assert_eq!(s2.len(), 2000);
    assert_eq!(s2, big_chars.into_iter().collect::<String>());

    let iv = Vector::from_ints(vec![1, 2, 3]);
    let mut s3 = String::new();
    assert!(!iv.push_to_string(&mut s3));
    assert!(s3.is_empty());
}

#[test]
fn packed_flat_shared_set_and_pop_promotion_to_trie() {
    let big: Vec<i64> = (0..2000).collect();
    let v = Vector::from_flat_ints(big);
    // Clone to make strong_count > 1
    let v_clone1 = v.clone();
    assert_eq!(v_clone1.get(10), Some(Value::Int(10)));
    let updated = v_clone1.set_nth(10, Value::Int(999)).unwrap();
    assert_eq!(updated.get(10), Some(Value::Int(999)));

    let v_clone2 = v.clone();
    assert_eq!(v_clone2.len(), 2000);
    let popped = v_clone2.drop_last().unwrap();
    assert_eq!(popped.len(), 1999);
}

#[test]
fn transient_conj_int_arms() {
    let mut tv_ints = TransientVector::from_vector_owned(Vector::from_ints(vec![1, 2]));
    assert!(tv_ints.conj_int(3).is_ok());
    assert_eq!(tv_ints.len().unwrap(), 3);
    assert_eq!(tv_ints.nth(2).unwrap(), Some(Value::Int(3)));

    let mut tv_chars = TransientVector::from_vector_owned(Vector::from_chars(vec!['a', 'b']));
    assert!(tv_chars.conj_int(4).is_ok());
    assert_eq!(tv_chars.len().unwrap(), 3);
    assert_eq!(tv_chars.nth(2).unwrap(), Some(Value::Int(4)));
}

#[test]
fn vector_into_vec_and_array_try_take_objs() {
    let vi = Vector::from_ints(vec![1, 2, 3]);
    assert_eq!(
        vi.into_vec(),
        vec![Value::Int(1), Value::Int(2), Value::Int(3)]
    );

    let vc = Vector::from_chars(vec!['x', 'y']);
    assert_eq!(vc.into_vec(), vec![Value::Char('x'), Value::Char('y')]);

    let vg = Vector::from_vec(vec![Value::Bool(true), Value::Nil]);
    assert_eq!(vg.into_vec(), vec![Value::Bool(true), Value::Nil]);

    let arr_objs = clj_value::Array::new(
        clj_value::ArrayDesc::flat(clj_value::BaseType::Object),
        clj_value::ArrayStore::Objs(vec![Value::Int(42)]),
    );
    let arr_clone = arr_objs.clone();
    assert!(arr_objs.try_take_objs().is_none());
    drop(arr_clone);
    assert_eq!(arr_objs.try_take_objs(), Some(vec![Value::Int(42)]));

    let arr_ints = clj_value::Array::new(
        clj_value::ArrayDesc::flat(clj_value::BaseType::Int),
        clj_value::ArrayStore::Ints(vec![10, 20]),
    );
    assert!(arr_ints.try_take_objs().is_none());
}

/// `read_at` reads every storage in place: packed ints and chars
/// pass a stack copy, generic items are borrowed; past the end is
/// `None` (#254 scalar tier reads).
#[test]
fn read_at_matches_get_over_every_storage() {
    let some = |v: &Value| Some(v.clone());
    let packed = Vector::from_vec(ints(3));
    let chars = Vector::from_chars(vec!['x', 'y']);
    let mixed = Vector::from_vec(vec![Value::Int(1), Value::Bool(true)]);
    for v in [&packed, &chars, &mixed] {
        for i in 0..4 {
            assert_eq!(v.read_at(i, some), v.get(i), "{i}");
        }
    }
    assert_eq!(chars.read_at(1, some), Some(Value::Char('y')));
}
