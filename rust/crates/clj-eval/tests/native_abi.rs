// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Issue #259 native ABI steps: the decoding fallbacks for malformed
//! encodings and the non-native call road (the native compiler only
//! emits well-formed encodings, so these pin the defensive defaults).

use clj_eval::{boot, native_abi};
use clj_value::Value;

#[test]
fn malformed_pattern_encodings_bind_nothing_useful() {
    let mut eval = boot();
    // Not a pattern at all: a nameless symbol binding the value.
    let vals = native_abi::destructure_encoded(&mut eval, &Value::Int(1), &Value::Int(7), &[])
        .expect("destructure");
    assert_eq!(vals, [Value::Int(7)]);
    // A vector pattern whose fixed part is not a vector: no fixed locals.
    let enc = Value::Vector(clj_value::Vector::from_vec(vec![
        Value::Keyword(clj_value::Keyword::new(None, "vec")),
        Value::Nil,
        Value::Nil,
        Value::Nil,
    ]));
    let vals =
        native_abi::destructure_encoded(&mut eval, &enc, &Value::Nil, &[]).expect("destructure");
    assert!(vals.is_empty());
}

#[test]
fn non_class_values_have_no_class_name_and_unknown_ops_no_intrinsic() {
    assert_eq!(native_abi::class_name_of(&Value::Nil), None);
    assert_eq!(native_abi::intrinsic(99, &[]), None);
}

#[test]
fn non_native_callees_take_the_general_call() {
    let mut eval = boot();
    let f = eval
        .eval_source("(fn [a b] (+ a b))")
        .expect("eval")
        .pop()
        .expect("fn");
    let args = [Value::Int(2), Value::Int(3)].into_iter().collect();
    let r = native_abi::call_value_owned(&mut eval, &f, args).expect("call");
    assert_eq!(r, Value::Int(5));
}
