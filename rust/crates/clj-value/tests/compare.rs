// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Compare tests (S3 TDD, C2): the full probe17 matrix, including
//! NaN-is-0, `-0.0`-is-0, list/map/set CCE, and message pins.

use std::cmp::Ordering;
use std::sync::Arc;

use clj_value::{Keyword, List, Map, Set, Symbol, Value, Vector, compare};

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn dbl(d: f64) -> Value {
    Value::Double(d)
}

fn ord(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap()
}

#[test]
fn numerics_compare_across_types() {
    assert_eq!(ord(&int(1), &dbl(1.0)), Ordering::Equal);
    assert_eq!(ord(&dbl(1.5), &int(2)), Ordering::Less);
    assert_eq!(ord(&int(2), &dbl(1.5)), Ordering::Greater);
    assert_eq!(ord(&int(1), &int(1)), Ordering::Equal);
    // Int/Int stays exact past f64 precision (2^53+1 vs 2^53+2).
    assert_eq!(
        ord(&int(9_007_199_254_740_993), &int(9_007_199_254_740_994)),
        Ordering::Less
    );
    // Anything touching Double compares as f64 (double-rounded).
    assert_eq!(
        ord(&int(9_007_199_254_740_993), &dbl(9_007_199_254_740_992.0)),
        Ordering::Equal
    );
}

#[test]
fn nan_and_signed_zero_rules() {
    let nan = dbl(f64::NAN);
    assert_eq!(ord(&nan, &int(1)), Ordering::Equal);
    assert_eq!(ord(&int(1), &nan), Ordering::Equal);
    assert_eq!(ord(&nan, &nan), Ordering::Equal);
    assert_eq!(ord(&dbl(-0.0), &dbl(0.0)), Ordering::Equal);
    assert_eq!(ord(&dbl(0.0), &dbl(-0.0)), Ordering::Equal);
}

#[test]
fn scalars_compare_within_type() {
    assert_eq!(ord(&Value::Char('a'), &Value::Char('b')), Ordering::Less);
    assert_eq!(ord(&Value::Bool(false), &Value::Bool(true)), Ordering::Less);
    assert_eq!(
        ord(&Value::Str(Arc::from("a")), &Value::Str(Arc::from("b"))),
        Ordering::Less
    );
    assert_eq!(
        ord(
            &Value::Keyword(Keyword::new(None, "a")),
            &Value::Keyword(Keyword::new(None, "b"))
        ),
        Ordering::Less
    );
    assert_eq!(
        ord(
            &Value::Symbol(Symbol::new(None, "a")),
            &Value::Symbol(Symbol::new(None, "b"))
        ),
        Ordering::Less
    );
    // Unqualified sorts before qualified, then namespace, then name.
    assert_eq!(
        ord(
            &Value::Keyword(Keyword::new(None, "a")),
            &Value::Keyword(Keyword::new(Some("b"), "x"))
        ),
        Ordering::Less
    );
    assert_eq!(
        ord(
            &Value::Keyword(Keyword::new(Some("b"), "x")),
            &Value::Keyword(Keyword::new(None, "a"))
        ),
        Ordering::Greater
    );
    assert_eq!(
        ord(
            &Value::Symbol(Symbol::new(Some("a"), "b")),
            &Value::Symbol(Symbol::new(Some("a"), "c"))
        ),
        Ordering::Less
    );
}

#[test]
fn vectors_compare_lexicographically() {
    let v1 = Value::Vector(Vector::from_vec(vec![int(1)]));
    let v2 = Value::Vector(Vector::from_vec(vec![int(1), int(2)]));
    assert_eq!(ord(&v1, &v2), Ordering::Less);
    assert_eq!(ord(&v2, &v1), Ordering::Greater);
    assert_eq!(ord(&v1, &v1), Ordering::Equal);
    // Element errors propagate.
    let bad = Value::Vector(Vector::from_vec(vec![Value::Keyword(Keyword::new(
        None, "a",
    ))]));
    assert!(compare(&v1, &bad).is_err());
}

#[test]
fn astral_strings_follow_utf16_order() {
    // JVM: "\uFFFF" sorts AFTER astral chars (0xFFFF > 0xD800 lead);
    // byte order says the opposite.
    let ffff = Value::Str(Arc::from("\u{FFFF}"));
    let astral = Value::Str(Arc::from("\u{10000}"));
    assert_eq!(ord(&ffff, &astral), Ordering::Greater);
    assert_eq!(ord(&astral, &ffff), Ordering::Less);
    // Astral-astral still code-point ordered.
    let grin = Value::Str(Arc::from("\u{1F601}"));
    assert_eq!(ord(&astral, &grin), Ordering::Less);
    // Same rule inside symbols and keywords.
    let s1 = Value::Symbol(Symbol::new(None, "\u{FFFF}"));
    let s2 = Value::Symbol(Symbol::new(None, "\u{10000}"));
    assert_eq!(ord(&s1, &s2), Ordering::Greater);
    let k1 = Value::Keyword(Keyword::new(None, "\u{FFFF}"));
    let k2 = Value::Keyword(Keyword::new(None, "\u{10000}"));
    assert_eq!(ord(&k1, &k2), Ordering::Greater);
}

#[test]
fn nil_orders_first() {
    assert_eq!(ord(&Value::Nil, &Value::Nil), Ordering::Equal);
    assert_eq!(ord(&Value::Nil, &int(1)), Ordering::Less);
    assert_eq!(ord(&int(1), &Value::Nil), Ordering::Greater);
}

#[test]
fn cross_type_compares_fail_with_stable_message() {
    for (a, b) in [
        (int(1), Value::Keyword(Keyword::new(None, "a"))),
        (
            Value::Keyword(Keyword::new(None, "a")),
            Value::Symbol(Symbol::new(None, "a")),
        ),
        (int(1), Value::Bool(true)),
        (Value::Bool(true), int(1)),
        (
            Value::Str(Arc::from("a")),
            Value::Keyword(Keyword::new(None, "a")),
        ),
    ] {
        assert!(compare(&a, &b).is_err(), "{a:?} vs {b:?}");
    }
    let err = compare(&int(1), &Value::Keyword(Keyword::new(None, "a"))).unwrap_err();
    assert_eq!(err.message(), "Cannot compare Long to Keyword");
    assert_eq!(err.jvm_class(), "java.lang.ClassCastException");
}

#[test]
fn lists_maps_sets_regexes_never_compare() {
    let list = Value::List(List::from_vec(vec![int(1)]));
    let vec = Value::Vector(Vector::from_vec(vec![int(1)]));
    assert!(compare(&list, &vec).is_err());
    assert!(compare(&vec, &list).is_err());
    assert!(compare(&list, &list).is_err());
    let map = Value::Map(Map::from_kvs_checked(vec![(int(1), int(2))]).unwrap());
    assert!(compare(&map, &map).is_err());
    let set = Value::Set(Set::from_items_checked(vec![int(1)]).unwrap());
    assert!(compare(&set, &set).is_err());
    let re = Value::Regex(Arc::from("a"));
    assert!(compare(&re, &re).is_err());
}
