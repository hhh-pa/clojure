// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Shaped small-map pins (#221 suffix states): maps at or below 16
//! entries share an interned key-set plus a flat values vec (no
//! HAMT or sidecar allocs) and convert past 16. Representation is
//! behavior-transparent — `get`, `iter`, `=`, insertion order,
//! and the reported class must agree across the boundary.

use clj_value::{Value, new_map};

fn int_map(n: i64) -> clj_value::Map {
    let mut m = new_map();
    for i in 0..n {
        m = m.insert(Value::Int(i), Value::Int(i * 10));
    }
    m
}

/// Small maps stay shaped; the empty map allocates nothing.
#[test]
fn small_maps_stay_shaped() {
    let m = new_map();
    assert!(m.is_shaped());
    assert!(m.is_empty());
    let m = int_map(16);
    assert!(m.is_shaped());
    assert_eq!(m.size(), 16);
}

/// The 17th pair converts to the hash trie (never back).
#[test]
fn seventeenth_pair_promotes() {
    let m = int_map(16);
    assert!(m.is_shaped());
    let m = m.insert(Value::Int(16), Value::Int(160));
    assert!(!m.is_shaped());
    assert_eq!(m.size(), 17);
    assert_eq!(m.get(&Value::Int(0)), Some(&Value::Int(0)));
    assert_eq!(m.get(&Value::Int(16)), Some(&Value::Int(160)));
    // Shrinking never demotes the representation.
    let m = m.without(&Value::Int(16));
    assert!(!m.is_shaped());
    assert_eq!(m.size(), 16);
}

/// `get`/`contains_key` agree across the boundary, including misses.
#[test]
fn get_agrees_across_boundary() {
    for n in [1, 8, 15, 16, 17, 40] {
        let m = int_map(n);
        assert_eq!(m.is_shaped(), n <= 16);
        for i in 0..n {
            assert_eq!(
                m.get(&Value::Int(i)),
                Some(&Value::Int(i * 10)),
                "n={n} i={i}"
            );
            assert!(m.contains_key(&Value::Int(i)));
        }
        assert_eq!(m.get(&Value::Int(n)), None);
        assert!(!m.contains_key(&Value::Int(n)));
    }
}

/// Re-assoc keeps position and key; iteration order is insertion
/// order on both sides of the boundary.
#[test]
fn assoc_keeps_position_and_order() {
    for n in [3, 16, 17, 40] {
        let m = int_map(n);
        let m = m.insert(Value::Int(1), Value::Int(999));
        assert_eq!(m.size(), n as usize);
        assert_eq!(m.get(&Value::Int(1)), Some(&Value::Int(999)));
        let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
        let want: Vec<Value> = (0..n).map(Value::Int).collect();
        assert_eq!(keys, want, "n={n}");
    }
}

/// `dissoc` drops the pair and shifts positions; missing keys are a
/// no-op returning the same map.
#[test]
fn dissoc_shifts_and_misses_noop() {
    let m = int_map(5);
    let id_before = m.id();
    let m2 = m.clone().without(&Value::Int(99));
    assert_eq!(m2.id(), id_before);
    let m = m.without(&Value::Int(1));
    assert!(m.is_shaped());
    assert_eq!(m.size(), 4);
    let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(
        keys,
        vec![Value::Int(0), Value::Int(2), Value::Int(3), Value::Int(4)]
    );
    // Dissoc to empty stays shaped and shared-empty.
    let m = m
        .without(&Value::Int(0))
        .without(&Value::Int(2))
        .without(&Value::Int(3))
        .without(&Value::Int(4));
    assert!(m.is_shaped());
    assert!(m.is_empty());
}

/// Literals take the JVM class across the boundary: all-keyword
/// small maps stay array-classed, mixed/large go hash.
#[test]
fn literal_class_follows_jvm_rules() {
    let kws: Vec<(Value, Value)> = (0..9)
        .map(|i| {
            (
                Value::Keyword(clj_value::Keyword::new(None, &format!("k{i}"))),
                Value::Int(i),
            )
        })
        .collect();
    let m = clj_value::Map::from_kvs_checked(kws).unwrap();
    assert!(m.is_shaped());
    assert!(!m.is_hashy());
    let mixed: Vec<(Value, Value)> = (0..9).map(|i| (Value::Int(i), Value::Int(i))).collect();
    let m = clj_value::Map::from_kvs_checked(mixed).unwrap();
    assert!(m.is_shaped());
    assert!(m.is_hashy());
}

/// Big literals build the hash arm directly (one pass, same
/// entries, order, and class as the incremental path).
#[test]
fn big_literal_builds_hash_directly() {
    let pairs: Vec<(Value, Value)> = (0..40)
        .map(|i| (Value::Int(i), Value::Int(i * 10)))
        .collect();
    let m = clj_value::Map::from_kvs_checked(pairs).unwrap();
    assert!(!m.is_shaped());
    assert_eq!(m.size(), 40);
    assert_eq!(Value::Map(m.clone()), Value::Map(int_map(40)));
    let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys, (0..40).map(Value::Int).collect::<Vec<_>>());
}

/// Empty literals and empty rebuilds ride the shared-empty shape.
#[test]
fn empty_literal_and_rebuild_stay_shaped() {
    let m = clj_value::Map::from_kvs_checked(vec![]).unwrap();
    assert!(m.is_shaped());
    assert!(m.is_empty());
    let m = new_map().with_ordered_entries(vec![]);
    assert!(m.is_shaped());
    assert!(m.is_empty());
}

/// `dissoc` on a hashed map: missing keys return the same map
/// (identity preserved), present keys rebuild without demoting.
#[test]
fn hashed_dissoc_miss_noop_and_hit() {
    let m = int_map(20);
    assert!(!m.is_shaped());
    let id_before = m.id();
    let m2 = m.clone().without(&Value::Int(99));
    assert_eq!(m2.id(), id_before);
    let m = m.without(&Value::Int(5));
    assert!(!m.is_shaped());
    assert_eq!(m.size(), 19);
    assert_eq!(m.get(&Value::Int(5)), None);
}

/// Last-wins batch construction matches the `insert` loop exactly:
/// first occurrence sets the position, last sets the value.
#[test]
fn last_wins_matches_insert_loop() {
    let pairs = vec![
        (Value::Int(1), Value::Int(10)),
        (Value::Int(2), Value::Int(20)),
        (Value::Int(1), Value::Int(99)),
    ];
    let batched = clj_value::Map::from_kvs_last_wins(pairs.clone());
    let mut looped = new_map();
    for (k, v) in pairs {
        looped = looped.insert(k, v);
    }
    assert_eq!(Value::Map(batched.clone()), Value::Map(looped));
    assert_eq!(batched.get(&Value::Int(1)), Some(&Value::Int(99)));
    let keys: Vec<Value> = batched.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys, vec![Value::Int(1), Value::Int(2)]);
    // The class simulates the incremental rule (overwrites don't
    // advance it): 9 small-int keys promote, 9 keywords don't.
    let ints: Vec<(Value, Value)> = (0..9).map(|i| (Value::Int(i), Value::Int(i))).collect();
    let mut looped = new_map();
    for (k, v) in ints.clone() {
        looped = looped.insert(k, v);
    }
    let batched = clj_value::Map::from_kvs_last_wins(ints);
    assert_eq!(batched.is_hashy(), looped.is_hashy());
    assert!(batched.is_hashy());
    // Big last-wins literals build the hash arm directly.
    let big: Vec<(Value, Value)> = (0..20).map(|i| (Value::Int(i), Value::Int(i))).collect();
    let m = clj_value::Map::from_kvs_last_wins(big);
    assert!(!m.is_shaped());
    assert_eq!(m.size(), 20);
}

/// Identity rides the entries `Arc`: every writer (including
/// `with_meta` and empty construction) carries a fresh `Arc`,
/// `Clone` shares it, and no-ops return `self`.
#[test]
fn identity_follows_entries_arc() {
    // Distinct empties never identify (fresh `Arc` each).
    assert_ne!(new_map().id(), new_map().id());
    let m = int_map(3);
    // Clones identify; writers don't.
    assert_eq!(m.clone().id(), m.id());
    assert_ne!(m.clone().insert(Value::Int(1), Value::Int(2)).id(), m.id());
    assert_ne!(m.clone().without(&Value::Int(1)).id(), m.id());
    assert_ne!(m.clone().with_meta(Some(new_map())).id(), m.id());
    assert_eq!(m.clone().without(&Value::Int(99)).id(), m.id());
    // Same contract on the hash arm.
    let h = int_map(20);
    assert_eq!(h.clone().id(), h.id());
    assert_ne!(h.clone().insert(Value::Int(1), Value::Int(2)).id(), h.id());
    assert_ne!(
        h.clone().insert(Value::Int(99), Value::Int(99)).id(),
        h.id()
    );
    assert_ne!(h.clone().without(&Value::Int(1)).id(), h.id());
    assert_ne!(h.clone().with_meta(Some(new_map())).id(), h.id());
    assert_eq!(h.clone().without(&Value::Int(99)).id(), h.id());
}

/// `=` agrees between shaped and hashed twins with the same entries.
#[test]
fn equality_spans_representations() {
    let shaped = int_map(16);
    let mut hashed = int_map(17);
    hashed = hashed.without(&Value::Int(16));
    assert!(shaped.is_shaped());
    assert!(!hashed.is_shaped());
    assert_eq!(Value::Map(shaped), Value::Map(hashed));
}
