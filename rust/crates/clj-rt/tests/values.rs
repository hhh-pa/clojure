// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Value-box helpers, called through the real FFI boundary (no
//! LLVM): every pin renders through `print` (the specified
//! observable — never derived `Debug` on collections).

mod support;

use clj_value::Value;
use cljrt::{
    rt_argv, rt_bool, rt_clone, rt_double, rt_drop, rt_int, rt_is_truthy, rt_nil, rt_str,
    rt_vec_new, rt_vec_push,
};
use support::{inbox, render};

#[test]
fn scalar_boxes_render() {
    assert_eq!(render(rt_nil()), "nil");
    assert_eq!(render(rt_bool(0)), "false");
    assert_eq!(render(rt_bool(1)), "true");
    assert_eq!(render(rt_bool(7)), "true");
    assert_eq!(render(rt_int(0)), "0");
    assert_eq!(render(rt_int(i64::MIN)), "-9223372036854775808");
    assert_eq!(render(rt_int(i64::MAX)), "9223372036854775807");
    assert_eq!(render(rt_double(1.5)), "1.5");
    assert_eq!(render(rt_double(f64::NAN)), "##NaN");
}

#[test]
fn string_boxes_decode_bytes() {
    let hello = "héllo";
    assert_eq!(
        render(unsafe { rt_str(hello.as_ptr(), hello.len()) }),
        "héllo"
    );
    assert_eq!(render(unsafe { rt_str("".as_ptr(), 0) }), "");
    let bad = b"a\xffb";
    assert_eq!(render(unsafe { rt_str(bad.as_ptr(), bad.len()) }), "a�b");
}

#[test]
fn vectors_build_in_order() {
    let vec = rt_vec_new();
    assert_eq!(render(unsafe { rt_clone(vec) }), "[]");
    let vec = unsafe { rt_vec_push(vec, inbox(Value::Int(1))) };
    let vec = unsafe { rt_vec_push(vec, inbox(Value::Str("s".into()))) };
    assert_eq!(render(vec), "[1 s]");
}

#[test]
fn argv_skips_program_name_and_nils_when_empty() {
    use std::ffi::CString;
    let prog = CString::new("prog").unwrap();
    // `argv[0]`-only (and the degenerate `argc = 0`) is nil.
    let argv = [prog.as_ptr()];
    assert_eq!(render(unsafe { rt_argv(argv.as_ptr(), 1) }), "nil");
    let empty: [*const std::ffi::c_char; 0] = [];
    assert_eq!(render(unsafe { rt_argv(empty.as_ptr(), 0) }), "nil");
    // Args arrive as a list of strings (the JVM shape, T1192
    // FX1 fx1-argv probe); bytes decode lossy.
    let a = CString::new("a").unwrap();
    let b = CString::new("b").unwrap();
    let argv = [prog.as_ptr(), a.as_ptr(), b.as_ptr()];
    assert_eq!(render(unsafe { rt_argv(argv.as_ptr(), 3) }), "(a b)");
    let bad = CString::new(b"a\xffb".to_vec()).unwrap();
    let argv = [prog.as_ptr(), bad.as_ptr()];
    assert_eq!(render(unsafe { rt_argv(argv.as_ptr(), 2) }), "(a�b)");
}

#[test]
fn vec_push_onto_non_vector_degrades_to_fresh() {
    let not_vec = inbox(Value::Int(9));
    let vec = unsafe { rt_vec_push(not_vec, inbox(Value::Int(1))) };
    assert_eq!(render(vec), "[1]");
}

#[test]
fn truthiness_consumes() {
    let cases = [
        (Value::Nil, 0),
        (Value::Bool(false), 0),
        (Value::Bool(true), 1),
        (Value::Int(0), 1),
        (Value::Str("".into()), 1),
    ];
    for (value, want) in cases {
        assert_eq!(unsafe { rt_is_truthy(inbox(value)) }, want);
    }
}

#[test]
fn clone_borrows_and_drop_frees() {
    let original = inbox(Value::Str("v".into()));
    let copy = unsafe { rt_clone(original) };
    assert_eq!(render(copy), "v");
    assert_eq!(render(original), "v");
    unsafe { rt_drop(inbox(Value::Int(1))) };
}
