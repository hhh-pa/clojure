// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Shaped small-map pins (#221 suffix states): maps at or below 16
//! entries share an interned key-set plus a flat values vec (no
//! HAMT or sidecar allocs) and convert past 16. Representation is
//! behavior-transparent — `get`, `=`, and the reported class agree
//! across the boundary; iteration is insertion order while shaped
//! and the trie's content-determined order once hashed (never
//! insertion — the Bug 4 fix), comparator order when sorted.

use clj_value::{SortSpec, Value, new_map};

fn int_map(n: i64) -> clj_value::Map {
    let mut m = new_map();
    for i in 0..n {
        m = m.insert(Value::Int(i), Value::Int(i * 10));
    }
    m
}

/// The same content inserted in reverse (pins that hashed
/// iteration order is content-determined, not insertion order).
fn int_map_rev(n: i64) -> clj_value::Map {
    let mut m = new_map();
    for i in (0..n).rev() {
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
/// order while shaped, and the trie's content-determined order
/// (never insertion) once hashed.
#[test]
fn assoc_keeps_position_and_order() {
    for n in [3, 16] {
        let m = int_map(n);
        let m = m.insert(Value::Int(1), Value::Int(999));
        assert_eq!(m.size(), n as usize);
        assert_eq!(m.get(&Value::Int(1)), Some(&Value::Int(999)));
        let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
        let want: Vec<Value> = (0..n).map(Value::Int).collect();
        assert_eq!(keys, want, "n={n}");
    }
    for n in [17, 40] {
        let m = int_map(n);
        let m = m.insert(Value::Int(1), Value::Int(999));
        assert_eq!(m.size(), n as usize);
        assert_eq!(m.get(&Value::Int(1)), Some(&Value::Int(999)));
        // Same content built in reverse insertion order iterates
        // identically (content-determined, like the JVM's hash
        // order) — and not in insertion order.
        let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
        let rev: Vec<Value> = int_map_rev(n)
            .insert(Value::Int(1), Value::Int(999))
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(keys, rev, "n={n}");
        assert_ne!(keys, (0..n).map(Value::Int).collect::<Vec<_>>(), "n={n}");
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
/// entries, class, and trie iteration order as the incremental
/// path).
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
    let looped: Vec<Value> = int_map(40).iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys, looped);
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

/// Mass shape churn stays correct under the adaptive cleanup
/// cadence (#235): resident shapes defer cleanup until the scan
/// amortizes, then dropped shapes clean out — maps read back
/// exactly throughout.
#[test]
fn adaptive_cleanup_survives_churn() {
    // 200 live single-key maps (distinct resident shapes).
    let mut live = Vec::new();
    for i in 0..200_i64 {
        live.push(new_map().insert(Value::Int(i), Value::Int(i)));
    }
    // Fresh-key churn past both cadence arms (defer, then clean).
    for i in 200..600_i64 {
        let m = new_map().insert(Value::Int(i), Value::Int(i));
        assert_eq!(m.get(&Value::Int(i)), Some(&Value::Int(i)));
    }
    // Live maps still read back exactly.
    for (i, m) in live.iter().enumerate() {
        assert_eq!(m.get(&Value::Int(i as i64)), Some(&Value::Int(i as i64)));
    }
    // Drop the residents; churn again so dead entries clean out.
    drop(live);
    for i in 1000..1400_i64 {
        let m = new_map().insert(Value::Int(i), Value::Int(i));
        assert_eq!(m.get(&Value::Int(i)), Some(&Value::Int(i)));
    }
}

/// Sorted maps keep the comparator-order sidecar on every arm:
/// rebuilds restore the given order, `dissoc` prunes it (the
/// `sorted_without_map` path), and direct `insert` appends to it
/// (in-repo sorted writes position through `with_ordered_entries`
/// instead — this pins the escape hatch, not the funnel).
#[test]
fn sorted_large_maps_keep_comparator_order() {
    let entries: Vec<(Value, Value)> = (0..20).map(|i| (Value::Int(i), Value::Int(i))).collect();
    let m = new_map()
        .with_sort(Some(SortSpec::Natural))
        .with_ordered_entries(entries);
    assert!(!m.is_shaped());
    assert!(m.is_sorted());
    let want: Vec<Value> = (0..20).map(Value::Int).collect();
    assert_eq!(m.ordered_keys(), want);
    let itered: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(itered, want);
    // `dissoc` prunes the sidecar alongside the trie.
    let m = m.without(&Value::Int(5));
    let want: Vec<Value> = (0..20).filter(|i| *i != 5).map(Value::Int).collect();
    assert_eq!(m.ordered_keys(), want);
    // Direct `insert` appends (the funnel re-sorts instead).
    let m = m.insert(Value::Int(99), Value::Int(99));
    let mut want = want;
    want.push(Value::Int(99));
    assert_eq!(m.ordered_keys(), want);
    // Promotion carries the sidecar over.
    let small = new_map()
        .with_sort(Some(SortSpec::Natural))
        .with_ordered_entries((0..16).map(|i| (Value::Int(i), Value::Int(i))).collect());
    let m = small.insert(Value::Int(16), Value::Int(16));
    assert!(!m.is_shaped());
    assert_eq!(
        m.ordered_keys(),
        (0..17).map(Value::Int).collect::<Vec<_>>()
    );
}

/// Plain large rebuilds ignore the given order (trie order wins),
/// and `ordered_keys` matches iteration exactly.
#[test]
fn plain_large_rebuild_ignores_given_order() {
    let entries: Vec<(Value, Value)> = (0..20).map(|i| (Value::Int(i), Value::Int(i))).collect();
    let m = new_map().with_ordered_entries(entries);
    assert!(!m.is_shaped());
    let via_iter: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(m.ordered_keys(), via_iter);
    // Same content, same trie order as the incremental build.
    let looped: Vec<Value> = int_map(20).iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(via_iter, looped);
}

#[test]
fn take_value_shaped_and_trie() {
    // Uniquely-owned shaped map
    let mut m = int_map(3);
    assert!(m.is_shaped());
    assert_eq!(m.take_value(&Value::Int(1)), Some(Value::Int(10)));
    assert_eq!(m.get(&Value::Int(1)), Some(&Value::Nil));
    assert_eq!(m.take_value(&Value::Int(99)), None);

    // Shared shaped map (Arc refcount > 1)
    let mut m_shared = int_map(3);
    let clone = m_shared.clone();
    assert_eq!(m_shared.take_value(&Value::Int(1)), Some(Value::Int(10)));
    assert_eq!(clone.get(&Value::Int(1)), Some(&Value::Int(10)));

    // Large trie map (> 16 keys)
    let mut big = int_map(20);
    assert!(!big.is_shaped());
    assert_eq!(big.take_value(&Value::Int(5)), Some(Value::Int(50)));
    assert_eq!(big.take_value(&Value::Int(999)), None);
}

#[test]
fn shape_registry_mru_exact_keys() {
    let char_pairs = vec![
        (Value::Char('a'), Value::Int(1)),
        (Value::Char('b'), Value::Int(2)),
    ];
    let m1 = clj_value::Map::from_kvs_last_wins(char_pairs.clone());
    let m2 = clj_value::Map::from_kvs_last_wins(char_pairs);
    assert!(m1.is_shaped());
    assert!(m2.is_shaped());

    let str_pairs = vec![(Value::Str(std::sync::Arc::from("k")), Value::Int(1))];
    let m3 = clj_value::Map::from_kvs_last_wins(str_pairs.clone());
    let m4 = clj_value::Map::from_kvs_last_wins(str_pairs);
    assert!(m3.is_shaped());
    assert!(m4.is_shaped());

    let sym_pairs = vec![(
        Value::Symbol(clj_value::Symbol::new(None, "sym")),
        Value::Int(1),
    )];
    let m5 = clj_value::Map::from_kvs_last_wins(sym_pairs.clone());
    let m6 = clj_value::Map::from_kvs_last_wins(sym_pairs);
    assert!(m5.is_shaped());
    assert!(m6.is_shaped());

    let bool_nil_pairs = vec![
        (Value::Bool(true), Value::Int(1)),
        (Value::Nil, Value::Int(2)),
    ];
    let m7 = clj_value::Map::from_kvs_last_wins(bool_nil_pairs.clone());
    let m8 = clj_value::Map::from_kvs_last_wins(bool_nil_pairs);
    assert!(m7.is_shaped());
    assert!(m8.is_shaped());

    for _ in 0..10 {
        let a = clj_value::Map::from_kvs_last_wins(vec![(Value::Char('x'), Value::Int(1))]);
        let b = clj_value::Map::from_kvs_last_wins(vec![(Value::Char('y'), Value::Int(2))]);
        assert_eq!(a.get(&Value::Char('x')), Some(&Value::Int(1)));
        assert_eq!(b.get(&Value::Char('y')), Some(&Value::Int(2)));
    }
}

#[test]
fn map_shape_ascii_map_lookups() {
    let mut m = clj_value::Map::from_kvs_last_wins(vec![
        (Value::Char('a'), Value::Int(10)),
        (Value::Char('\u{100}'), Value::Int(20)),
        (
            Value::Keyword(clj_value::Keyword::new(None, "k")),
            Value::Int(30),
        ),
    ]);
    assert!(m.is_shaped());

    // ASCII char lookup
    assert_eq!(m.get(&Value::Char('a')), Some(&Value::Int(10)));
    assert!(m.contains_key(&Value::Char('a')));
    assert_eq!(m.get(&Value::Char('b')), None);
    assert!(!m.contains_key(&Value::Char('b')));

    // Non-ASCII char lookup
    assert_eq!(m.get(&Value::Char('\u{100}')), Some(&Value::Int(20)));
    assert!(m.contains_key(&Value::Char('\u{100}')));
    assert_eq!(m.get(&Value::Char('\u{101}')), None);
    assert!(!m.contains_key(&Value::Char('\u{101}')));

    // Non-char lookup
    let kw = Value::Keyword(clj_value::Keyword::new(None, "k"));
    let kw_missing = Value::Keyword(clj_value::Keyword::new(None, "missing"));
    assert_eq!(m.get(&kw), Some(&Value::Int(30)));
    assert!(m.contains_key(&kw));
    assert_eq!(m.get(&kw_missing), None);
    assert!(!m.contains_key(&kw_missing));

    // without on existing and missing ASCII / non-ASCII keys
    let m2 = m.clone().without(&Value::Char('b'));
    assert_eq!(m2.size(), 3);
    let m3 = m.clone().without(&Value::Char('a'));
    assert_eq!(m3.size(), 2);
    assert_eq!(m3.get(&Value::Char('a')), None);

    // take_value on ASCII char
    assert_eq!(m.take_value(&Value::Char('a')), Some(Value::Int(10)));
}

#[test]
fn shape_registry_capacity_cap() {
    let mut maps = Vec::with_capacity(600);
    for i in 0..600 {
        let m = clj_value::Map::from_kvs_last_wins(vec![(
            Value::Keyword(clj_value::Keyword::new(None, &format!("k{i}"))),
            Value::Int(i),
        )]);
        assert!(m.is_shaped());
        maps.push(m);
    }
}
