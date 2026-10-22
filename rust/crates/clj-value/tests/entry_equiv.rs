// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Entry equiv pins (T1192 FX2 finding 15): entry keys route through
//! `jvm_equiv`/`coll_equiv` in the duplicate checks, and tagged entry
//! forms through the `plain_equals` entry arms — so every new arm is
//! pinned via the pub checked constructors below.

use clj_value::{Keyword, List, Map, Set, Symbol, Value, Vector, map_entry};

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn kw(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

fn vec_of(items: Vec<Value>) -> Value {
    Value::Vector(Vector::from_vec(items))
}

fn list_of(items: Vec<Value>) -> Value {
    Value::List(List::from_vec(items))
}

fn tagged(form: Value) -> Value {
    Value::TaggedLiteral {
        tag: Symbol::new(None, "inst"),
        form: Box::new(form),
    }
}

#[test]
fn entry_keys_dup_like_two_vectors() {
    // entry<->entry: the small-map scan reports the FIRST key.
    let first = map_entry(int(1), int(2));
    assert_eq!(
        Map::from_kvs_checked(vec![
            (first.clone(), kw("a")),
            (map_entry(int(1), int(2)), kw("b")),
        ]),
        Err(first)
    );
    // entry<->2-vector, both orders (coll_equiv crosses
    // List/Vector/MapEntry in every order).
    let e = map_entry(int(1), int(2));
    let v = vec_of(vec![int(1), int(2)]);
    assert_eq!(
        Map::from_kvs_checked(vec![(e.clone(), kw("a")), (v.clone(), kw("b"))]),
        Err(e.clone())
    );
    assert_eq!(
        Map::from_kvs_checked(vec![(v.clone(), kw("a")), (e.clone(), kw("b"))]),
        Err(v.clone())
    );
    // entry<->2-list, both orders.
    let l = list_of(vec![int(1), int(2)]);
    assert_eq!(
        Map::from_kvs_checked(vec![(e.clone(), kw("a")), (l.clone(), kw("b"))]),
        Err(e.clone())
    );
    assert_eq!(
        Map::from_kvs_checked(vec![(l.clone(), kw("a")), (e.clone(), kw("b"))]),
        Err(l.clone())
    );
    // Non-equal entries are distinct keys; entry vs scalar never dups.
    assert!(
        Map::from_kvs_checked(vec![
            (map_entry(int(1), int(2)), kw("a")),
            (map_entry(int(1), int(3)), kw("b")),
        ])
        .is_ok()
    );
    assert!(
        Map::from_kvs_checked(vec![
            (map_entry(int(1), int(2)), kw("a")),
            (int(1), kw("b"))
        ])
        .is_ok()
    );
}

#[test]
fn entry_items_use_equiv_not_bits() {
    // Nested doubles use `==`: 0.0 equiv -0.0 inside entries.
    let first = map_entry(Value::Double(0.0), int(1));
    assert_eq!(
        Map::from_kvs_checked(vec![
            (first.clone(), kw("a")),
            (map_entry(Value::Double(-0.0), int(1)), kw("b")),
        ]),
        Err(first)
    );
    // NaN equiv NaN (the shared ##NaN).
    assert!(
        Map::from_kvs_checked(vec![
            (map_entry(Value::Double(f64::NAN), int(1)), kw("a")),
            (map_entry(Value::Double(f64::NAN), int(1)), kw("b")),
        ])
        .is_err()
    );
}

#[test]
fn entry_members_dup_in_sets() {
    // Sets report the NEW item; an entry equivs its 2-vector member.
    let v = vec_of(vec![int(1), int(2)]);
    assert_eq!(
        Set::from_items_checked(vec![map_entry(int(1), int(2)), v.clone()]),
        Err(v)
    );
}

#[test]
fn tagged_entry_forms_use_plain_equals() {
    // The plain_equals entry arms: a tagged entry equivs a tagged
    // 2-vector (and 2-list) with the same tag.
    let first = tagged(map_entry(int(1), int(2)));
    assert_eq!(
        Map::from_kvs_checked(vec![
            (first.clone(), int(1)),
            (tagged(vec_of(vec![int(1), int(2)])), int(2)),
        ]),
        Err(first.clone())
    );
    assert_eq!(
        Map::from_kvs_checked(vec![
            (first.clone(), int(1)),
            (tagged(list_of(vec![int(1), int(2)])), int(2)),
        ]),
        Err(first.clone())
    );
    // Swapped order: the (2-list, entry) and (2-vector, entry)
    // `plain_equals` arms fire when the tagged collection is
    // the first key.
    assert_eq!(
        Map::from_kvs_checked(vec![
            (tagged(vec_of(vec![int(1), int(2)])), int(1)),
            (first.clone(), int(2)),
        ]),
        Err(tagged(vec_of(vec![int(1), int(2)])))
    );
    assert_eq!(
        Map::from_kvs_checked(vec![
            (tagged(list_of(vec![int(1), int(2)])), int(1)),
            (first.clone(), int(2)),
        ]),
        Err(tagged(list_of(vec![int(1), int(2)])))
    );
    // But doubles compare by bits there: 0.0 vs -0.0 reads fine.
    assert!(
        Map::from_kvs_checked(vec![
            (tagged(map_entry(Value::Double(0.0), int(1))), int(1)),
            (tagged(map_entry(Value::Double(-0.0), int(1))), int(2)),
        ])
        .is_ok()
    );
}
