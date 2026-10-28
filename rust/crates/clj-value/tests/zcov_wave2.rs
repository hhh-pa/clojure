// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Wave-2 coverage closure for the value core: S9 scalar (`Uuid` /
//! `Instant` / `Uri`) equality, hashing, ordering, and printing;
//! primitive-vector participation in equality, hashing, `equiv`
//! duplicate checks, and ordering; array hashing and compare-error
//! naming; `Elem8` keywords and `BaseType` letters.

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use clj_value::{
    Array, ArrayDesc, ArrayStore, BaseType, Elem8, Instant, List, PrimitiveVector, PrintOpts, Set,
    Symbol, Uri, Uuid, Value, Vector, compare, hash_code, hasheq, java_debug, pr_to_string,
    print_to_string,
};

fn uuid(hi: u64, lo: u64) -> Value {
    Value::Uuid(Uuid::from_bits(hi, lo))
}

fn instant(millis: i64) -> Value {
    Value::Instant(Instant::from_millis(millis))
}

fn uri(text: &str) -> Value {
    Value::Uri(Box::new(Uri::parse(text).expect("test URI parses")))
}

fn pvec(elem: Elem8, items: Vec<Value>) -> Value {
    Value::PrimitiveVector(PrimitiveVector::new(elem, items, None))
}

fn vecv(items: Vec<Value>) -> Value {
    Value::Vector(Vector::from_vec(items))
}

fn listv(items: Vec<Value>) -> Value {
    Value::List(List::from_vec(items))
}

fn int_array() -> Value {
    Value::Array(Array::new(
        ArrayDesc::flat(BaseType::Int),
        ArrayStore::Ints(vec![1, 2]),
    ))
}

fn tagged(form: Value) -> Value {
    Value::TaggedLiteral {
        tag: Symbol::new(None, "t"),
        form: Box::new(form),
    }
}

fn hash_of(v: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

#[test]
fn s9_scalars_equate_by_content() {
    // Instants compare by millis, URIs by normalized content
    // (never the print id), UUIDs by bits.
    assert_eq!(instant(0), instant(0));
    assert_ne!(instant(0), instant(1));
    assert_eq!(uri("http://example.com/"), uri("http://example.com/"));
    assert_ne!(uri("http://example.com/"), uri("http://example.org/"));
    assert_eq!(uuid(1, 2), uuid(1, 2));
    assert_ne!(uuid(1, 2), uuid(1, 3));
}

#[test]
fn hash_stays_consistent_with_eq() {
    // Equal values hash equally: instants, URIs (scheme
    // case-blind, like `uri_eq`), primitive vectors (exactly
    // like their item vector), and array clones (same cell).
    assert_eq!(hash_of(&instant(5)), hash_of(&instant(5)));
    assert_eq!(
        hash_of(&uri("HTTP://example.com/")),
        hash_of(&uri("http://example.com/"))
    );
    assert_eq!(
        hash_of(&pvec(Elem8::Long, vec![Value::Int(1), Value::Int(2)])),
        hash_of(&vecv(vec![Value::Int(1), Value::Int(2)]))
    );
    let a = int_array();
    assert_eq!(hash_of(&a), hash_of(&a.clone()));
}

#[test]
fn set_literal_dup_check_equates_pvec_members() {
    // Primitive vectors participate in the `equiv` duplicate
    // check like vectors, in both orders and across List/pvec.
    let dup = pvec(Elem8::Long, vec![Value::Int(1), Value::Int(2)]);
    let err = Set::from_items_checked(vec![dup.clone(), dup.clone()])
        .expect_err("equal pvec members are dups");
    assert_eq!(err, dup);
    assert!(
        Set::from_items_checked(vec![
            dup.clone(),
            pvec(Elem8::Long, vec![Value::Int(1), Value::Int(3)])
        ])
        .is_ok()
    );
    let err = Set::from_items_checked(vec![listv(vec![Value::Int(1), Value::Int(2)]), dup.clone()])
        .expect_err("a pvec dups its item list");
    assert_eq!(err, dup);
    let err = Set::from_items_checked(vec![dup.clone(), listv(vec![Value::Int(1), Value::Int(2)])])
        .expect_err("order does not matter");
    assert_eq!(err, listv(vec![Value::Int(1), Value::Int(2)]));
}

#[test]
fn tagged_literal_equality_delegates_to_form_equals() {
    // Reader-form `.equals` reaches the S9 scalar rules and the
    // sequential family (a pvec form is its item list).
    assert_eq!(tagged(instant(1)), tagged(instant(1)));
    assert_ne!(tagged(instant(1)), tagged(instant(2)));
    assert_eq!(
        tagged(uri("http://example.com/")),
        tagged(uri("http://example.com/"))
    );
    assert_ne!(
        tagged(uri("http://example.com/")),
        tagged(uri("http://example.org/"))
    );
    assert_eq!(tagged(uuid(1, 2)), tagged(uuid(1, 2)));
    assert_ne!(tagged(uuid(1, 2)), tagged(uuid(1, 3)));
    assert_eq!(
        tagged(pvec(Elem8::Long, vec![Value::Int(1)])),
        tagged(listv(vec![Value::Int(1)]))
    );
    assert_ne!(
        tagged(pvec(Elem8::Long, vec![Value::Int(1)])),
        tagged(listv(vec![Value::Int(2)]))
    );
}

#[test]
fn s9_scalars_compare_within_kind() {
    // UUIDs order by signed words, instants by millis, URIs by
    // the JDK `compareTo` legs (host here).
    assert_eq!(compare(&uuid(0, 0), &uuid(1, 0)).unwrap(), Ordering::Less);
    assert_eq!(
        compare(&uuid(u64::MAX, 0), &uuid(0, 0)).unwrap(),
        Ordering::Less
    );
    assert_eq!(compare(&uuid(1, 2), &uuid(1, 2)).unwrap(), Ordering::Equal);
    assert_eq!(compare(&instant(1), &instant(2)).unwrap(), Ordering::Less);
    assert_eq!(
        compare(&instant(2), &instant(1)).unwrap(),
        Ordering::Greater
    );
    assert_eq!(
        compare(&uri("http://a/"), &uri("http://b/")).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        compare(&uri("http://b/"), &uri("http://a/")).unwrap(),
        Ordering::Greater
    );
    assert_eq!(
        compare(&uri("http://a/"), &uri("http://a/")).unwrap(),
        Ordering::Equal
    );
}

#[test]
fn pvec_compares_as_vector() {
    let p12 = pvec(Elem8::Long, vec![Value::Int(1), Value::Int(2)]);
    assert_eq!(
        compare(&p12, &vecv(vec![Value::Int(1), Value::Int(2)])).unwrap(),
        Ordering::Equal
    );
    assert_eq!(
        compare(&pvec(Elem8::Long, vec![Value::Int(1)]), &p12).unwrap(),
        Ordering::Less
    );
    assert_eq!(
        compare(
            &vecv(vec![Value::Int(2)]),
            &pvec(Elem8::Long, vec![Value::Int(1)])
        )
        .unwrap(),
        Ordering::Greater
    );
}

#[test]
fn compare_errors_name_s9_types() {
    assert_eq!(
        compare(&uuid(1, 2), &Value::Int(1)).unwrap_err().message(),
        "Cannot compare UUID to Long"
    );
    assert_eq!(
        compare(&Value::Int(1), &instant(0)).unwrap_err().message(),
        "Cannot compare Long to Instant"
    );
    assert_eq!(
        compare(&uri("http://example.com/"), &Value::Int(1))
            .unwrap_err()
            .message(),
        "Cannot compare URI to Long"
    );
}

#[test]
fn compare_errors_name_arrays_by_descriptor() {
    assert_eq!(
        compare(&int_array(), &Value::Int(1)).unwrap_err().message(),
        "Cannot compare [I to Long"
    );
}

#[test]
fn hasheq_covers_s9_pvec_and_array() {
    // `hasheq` on the S9 scalars is the JDK `.hashCode`
    // fallback (deterministic, content-based); primitive
    // vectors hash like their item vector; arrays by identity.
    assert_eq!(hasheq(&instant(7)), hasheq(&instant(7)));
    assert_ne!(hasheq(&instant(7)), hasheq(&instant(8)));
    assert_eq!(
        hasheq(&uri("http://example.com/")),
        hasheq(&uri("http://example.com/"))
    );
    assert_eq!(
        hasheq(&pvec(Elem8::Long, vec![Value::Int(1)])),
        hasheq(&vecv(vec![Value::Int(1)]))
    );
    let a = int_array();
    assert_eq!(hasheq(&a), hasheq(&a.clone()));
}

#[test]
fn hash_code_pins_s9_pvec_and_array() {
    // Date/UUID word-folding pins plus the `.hashCode`
    // contracts: pvec matches its item vector, equal arrays
    // (same cell) match.
    assert_eq!(hash_code(&instant(0)), 0);
    assert_eq!(hash_code(&instant(1)), 1);
    assert_eq!(hash_code(&instant(0x1_0000_0001)), 0);
    assert_eq!(hash_code(&uuid(0, 0)), 0);
    assert_eq!(hash_code(&uuid(1, 0)), 1);
    assert_eq!(
        hash_code(&uri("HTTP://example.com/")),
        hash_code(&uri("http://example.com/"))
    );
    assert_eq!(
        hash_code(&pvec(Elem8::Long, vec![Value::Int(1), Value::Int(2)])),
        hash_code(&vecv(vec![Value::Int(1), Value::Int(2)]))
    );
    let a = int_array();
    assert_eq!(hash_code(&a), hash_code(&a.clone()));
}

#[test]
fn s9_scalars_print() {
    assert_eq!(
        pr_to_string(&instant(0), &PrintOpts::pr()),
        "#inst \"1970-01-01T00:00:00.000-00:00\""
    );
    assert_eq!(
        print_to_string(&instant(0)),
        "#inst \"1970-01-01T00:00:00.000-00:00\""
    );
    assert_eq!(
        pr_to_string(&uuid(0, 0), &PrintOpts::pr()),
        "#uuid \"00000000-0000-0000-0000-000000000000\""
    );
    let pr = pr_to_string(&uri("http://example.com/"), &PrintOpts::pr());
    assert!(pr.starts_with("#object[java.net.URI 0x"), "{pr}");
    assert!(pr.ends_with(" \"http://example.com/\"]"), "{pr}");
    let human = print_to_string(&uri("http://example.com/"));
    assert!(human.starts_with("#object[java.net.URI 0x"), "{human}");
    assert!(human.ends_with(" http://example.com/]"), "{human}");
}

#[test]
fn s9_scalar_tostrings() {
    assert_eq!(java_debug(&instant(0)), "Thu Jan 01 00:00:00 UTC 1970");
    assert_eq!(
        java_debug(&uri("http://example.com/")),
        "http://example.com/"
    );
    assert_eq!(
        java_debug(&uuid(0, 0)),
        "00000000-0000-0000-0000-000000000000"
    );
}

#[test]
fn elem8_keywords() {
    assert_eq!(
        [
            Elem8::Boolean,
            Elem8::Byte,
            Elem8::Char,
            Elem8::Short,
            Elem8::Int,
            Elem8::Long,
            Elem8::Float,
            Elem8::Double,
        ]
        .map(Elem8::keyword),
        [
            ":boolean", ":byte", ":char", ":short", ":int", ":long", ":float", ":double",
        ]
    );
}

#[test]
fn base_type_letters() {
    use BaseType::{Boolean, Byte, Char, Double, Float, Int, Long, Object, Short};
    assert_eq!(
        [Boolean, Byte, Char, Short, Int, Long, Float, Double, Object].map(BaseType::letter),
        ["Z", "B", "C", "S", "I", "J", "F", "D", "Ljava.lang.Object;"]
    );
}
