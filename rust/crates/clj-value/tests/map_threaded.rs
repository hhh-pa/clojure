// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Loop-threaded map pins (`Map::insert_threaded` / `deep_clone`):
//! threaded assoc chains equal regular `insert` chains entry for
//! entry, in iteration order, while the entry version stays intact
//! with a distinct identity.

use clj_value::{Map, Value, new_map};

fn int_map(n: i64) -> Map {
    let mut m = new_map();
    for i in 0..n {
        m = m.insert(Value::Int(i), Value::Int(i * 10));
    }
    m
}

fn entries(m: &Map) -> Vec<(Value, Value)> {
    m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Threaded chains equal `insert` chains (values, count, order).
#[test]
fn threaded_matches_insert() {
    let mut threaded = new_map().deep_clone();
    let mut plain = new_map();
    for i in 0..500 {
        threaded = threaded.insert_threaded(Value::Int(i), Value::Int(i * 2));
        plain = plain.insert(Value::Int(i), Value::Int(i * 2));
    }
    assert_eq!(entries(&threaded), entries(&plain));
    assert_eq!(threaded.get(&Value::Int(499)), Some(&Value::Int(998)));
    assert_eq!(threaded.get(&Value::Int(500)), None);
}

/// The entry version stays intact with a distinct identity.
#[test]
fn threaded_preserves_entry() {
    let entry = int_map(100);
    let entry_entries = entries(&entry);
    let entry_id = entry.id();
    let mut threaded = entry.deep_clone();
    for i in 100..200 {
        threaded = threaded.insert_threaded(Value::Int(i), Value::Int(i));
    }
    assert_eq!(entries(&entry), entry_entries);
    assert_eq!(entry.id(), entry_id);
    assert_ne!(threaded.id(), entry_id);
    assert_eq!(entries(&threaded).len(), 200);
    assert_eq!(threaded.get(&Value::Int(50)), Some(&Value::Int(500)));
}

/// Existing-key rounds keep values and refresh identity.
#[test]
fn threaded_existing_keys() {
    let entry = int_map(50);
    let entry_id = entry.id();
    let mut threaded = entry.deep_clone();
    for i in 0..50 {
        threaded = threaded.insert_threaded(Value::Int(i), Value::Int(-i));
    }
    assert_eq!(entries(&threaded).len(), 50);
    assert_eq!(threaded.get(&Value::Int(7)), Some(&Value::Int(-7)));
    assert_ne!(threaded.id(), entry_id);
    assert_eq!(entry.get(&Value::Int(7)), Some(&Value::Int(70)));
}

/// Shaped maps thread through the shaped arm (values equal, entry
/// intact).
#[test]
fn threaded_shaped() {
    let entry = int_map(5);
    assert!(entry.is_shaped());
    let mut threaded = entry.deep_clone();
    for i in 5..10 {
        threaded = threaded.insert_threaded(Value::Int(i), Value::Int(i));
    }
    assert_eq!(entries(&threaded).len(), 10);
    assert_eq!(entries(&entry).len(), 5);
    // Past 16 the threaded map promotes like `insert` does.
    for i in 10..20 {
        threaded = threaded.insert_threaded(Value::Int(i), Value::Int(i));
    }
    assert!(!threaded.is_shaped());
    assert_eq!(entries(&threaded).len(), 20);
    assert_eq!(threaded.get(&Value::Int(3)), Some(&Value::Int(30)));
}

/// The first threaded assoc detaches a shared sidecar (the entry
/// version keeps its identity and entries).
#[test]
fn threaded_detaches_shared_sidecar() {
    let entry = int_map(500);
    let entry_id = entry.id();
    let shared = entry.clone();
    let threaded = shared.insert_threaded(Value::Int(500), Value::Int(5000));
    assert_ne!(threaded.id(), entry_id);
    assert_eq!(entries(&entry).len(), 500);
    assert_eq!(threaded.get(&Value::Int(500)), Some(&Value::Int(5000)));
}

/// `deep_clone` preserves flags, metadata, and iteration order.
#[test]
fn deep_clone_preserves_shape() {
    let m = int_map(100).with_sort(Some(clj_value::SortSpec::Natural));
    assert!(m.is_sorted());
    let order: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
    let c = m.deep_clone();
    assert!(c.is_sorted());
    assert_eq!(c.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(), order);
    assert_ne!(c.id(), m.id());
    let meta = int_map(2);
    let m2 = int_map(30).with_meta(Some(meta));
    assert_eq!(entries(&m2.deep_clone()), entries(&m2));
}
