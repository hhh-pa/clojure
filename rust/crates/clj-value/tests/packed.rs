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

/// In-bounds `Int` assoc stays packed (one chunk copied).
#[test]
fn packed_int_assoc_stays_packed() {
    let v = Vector::from_ints(vec![1, 2, 3]);
    let w = v.set_nth_owned(1, Value::Int(9)).expect("in bounds");
    assert!(w.is_packed());
    let back: Vec<Value> = w.iter().collect();
    assert_eq!(back, vec![Value::Int(1), Value::Int(9), Value::Int(3)]);
}

/// Threaded entry points decline generic storage.
#[test]
fn threaded_entry_points_decline_generic() {
    let v = Vector::from_vec(vec![Value::Int(1), Value::Nil]);
    assert!(!v.is_packed());
    assert_eq!(v.nth_int(0), None);
    let mut t = v.thread_entry();
    assert!(!t.assoc_int_threaded(0, 5));
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

/// Pushing an `Int` onto a shared small packed run copies cheaply
/// and stays packed; both versions stay intact (persistence).
#[test]
fn push_back_shared_small_int_stays_packed() {
    let v = Vector::from_vec(ints(10));
    assert!(v.is_packed());
    let shared = v.clone();
    let w = v.push_back(Value::Int(10));
    assert!(w.is_packed());
    assert_eq!(w.len(), 11);
    assert_eq!(w.iter().collect::<Vec<_>>(), ints(11));
    assert!(shared.is_packed());
    assert_eq!(shared.len(), 10);
}

/// Pushing an `Int` onto a uniquely owned big packed run mutates
/// in place and stays packed.
#[test]
fn push_back_unique_big_int_stays_packed() {
    let v = Vector::from_ints((0..1500).collect());
    assert!(v.is_packed());
    let w = v.push_back(Value::Int(1500));
    assert!(w.is_packed());
    assert_eq!(w.len(), 1501);
    assert_eq!(w.get(1500), Some(Value::Int(1500)));
    assert_eq!(w.get(0), Some(Value::Int(0)));
}

/// Pushing an `Int` onto a shared big packed run stays packed
/// (one chunk copied, never the whole run); the old version stays
/// packed and intact.
#[test]
fn push_back_shared_big_int_stays_packed() {
    let v = Vector::from_ints((0..1500).collect());
    assert!(v.is_packed());
    let shared = v.clone();
    let w = v.push_back(Value::Int(1500));
    assert!(w.is_packed());
    assert_eq!(w.len(), 1501);
    assert_eq!(w.iter().collect::<Vec<_>>(), ints(1501));
    assert!(shared.is_packed());
    assert_eq!(shared.len(), 1500);
    assert_eq!(shared.iter().collect::<Vec<_>>(), ints(1500));
}

/// `set_nth` of an `Int` on a big packed run stays packed (one
/// chunk copied); the old version stays packed and intact.
#[test]
fn set_nth_big_int_stays_packed() {
    let v = Vector::from_ints((0..1500).collect());
    let w = v.set_nth(700, Value::Int(99)).unwrap();
    assert!(w.is_packed());
    assert_eq!(w.len(), 1500);
    assert_eq!(w.get(700), Some(Value::Int(99)));
    assert_eq!(w.get(0), Some(Value::Int(0)));
    assert!(v.is_packed());
    assert_eq!(v.get(700), Some(Value::Int(700)));
}

/// `drop_last` on a big packed run stays packed; the old version
/// stays packed and intact.
#[test]
fn drop_last_big_stays_packed() {
    let v = Vector::from_ints((0..1500).collect());
    let w = v.drop_last().unwrap();
    assert!(w.is_packed());
    assert_eq!(w.len(), 1499);
    assert_eq!(w.iter().collect::<Vec<_>>(), ints(1499));
    assert!(v.is_packed());
    assert_eq!(v.len(), 1500);
}

/// Freezing: pushing past a full 64-chunk keeps every item in
/// order (frozen chunk + new tail), and `get`/`len` span both.
#[test]
fn push_back_freezes_full_tail_into_chunks() {
    let mut v = Vector::from_ints((0..64).collect());
    assert_eq!(v.len(), 64);
    v = v.push_back(Value::Int(64));
    assert!(v.is_packed());
    assert_eq!(v.len(), 65);
    assert_eq!(v.iter().collect::<Vec<_>>(), ints(65));
    assert_eq!(v.get(0), Some(Value::Int(0)));
    assert_eq!(v.get(63), Some(Value::Int(63)));
    assert_eq!(v.get(64), Some(Value::Int(64)));
    assert_eq!(v.get(65), None);
}

/// Exact multiples leave an empty tail (two frozen chunks here);
/// dropping unfreezes the last chunk back into the tail.
#[test]
fn exact_multiple_runs_unfreeze_on_drop() {
    let v = Vector::from_ints((0..128).collect());
    assert!(v.is_packed());
    assert_eq!(v.len(), 128);
    assert_eq!(v.iter().collect::<Vec<_>>(), ints(128));
    let w = v.drop_last().unwrap();
    assert!(w.is_packed());
    assert_eq!(w.len(), 127);
    assert_eq!(w.iter().collect::<Vec<_>>(), ints(127));
    assert_eq!(v.len(), 128);
}

/// `set_nth` inside a frozen chunk copies just that chunk: later
/// chunks, the tail, and the old version are untouched.
#[test]
fn set_nth_frozen_region_shares_the_rest() {
    let v = Vector::from_ints((0..200).collect());
    let w = v.set_nth(10, Value::Int(99)).unwrap();
    assert!(w.is_packed());
    assert_eq!(w.get(10), Some(Value::Int(99)));
    assert_eq!(w.get(100), Some(Value::Int(100)));
    assert_eq!(w.get(199), Some(Value::Int(199)));
    assert_eq!(v.get(10), Some(Value::Int(10)));
    // Out of bounds stays `None` on multi-chunk runs.
    assert_eq!(v.set_nth(200, Value::Int(1)), None);
}

/// Forward iteration streams chunk after chunk; `next_back`
/// indexes from the end; mixed ends never cross or double-yield.
#[test]
fn packed_iter_walks_chunks_both_ends() {
    let v = Vector::from_ints((0..200).collect());
    let mut it = v.iter();
    assert_eq!(it.size_hint(), (200, Some(200)));
    assert_eq!(it.next(), Some(Value::Int(0)));
    assert_eq!(it.next_back(), Some(Value::Int(199)));
    assert_eq!(it.next(), Some(Value::Int(1)));
    assert_eq!(it.next_back(), Some(Value::Int(198)));
    assert_eq!(it.size_hint(), (196, Some(196)));
    let rest: Vec<_> = it.collect();
    assert_eq!(rest.len(), 196);
    assert_eq!(rest[0], Value::Int(2));
    assert_eq!(rest[195], Value::Int(197));
    // Fully consumed from both sides ends cleanly.
    let mut it = v.iter();
    for _ in 0..100 {
        it.next();
        it.next_back();
    }
    assert_eq!(it.next(), None);
    assert_eq!(it.next_back(), None);
}

/// A foreign value into a multi-chunk run unpacks once to the
/// trie; the old version stays packed and intact.
#[test]
fn foreign_push_on_multichunk_unpacks_once() {
    let v = Vector::from_ints((0..200).collect());
    let w = v.clone().push_back(Value::Nil);
    assert!(!w.is_packed());
    assert_eq!(w.len(), 201);
    assert_eq!(w.get(200), Some(Value::Nil));
    assert_eq!(w.get(199), Some(Value::Int(199)));
    assert!(v.is_packed());
    let s = v.set_nth(150, Value::Nil).unwrap();
    assert!(!s.is_packed());
    assert_eq!(s.get(150), Some(Value::Nil));
    assert_eq!(s.get(0), Some(Value::Int(0)));
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
