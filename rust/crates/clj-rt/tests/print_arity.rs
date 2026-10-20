// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `print` and arity-error helpers through the real FFI boundary
//! (no LLVM): return-shape pins here, exact stdout/stderr bytes in
//! the S8D corpus replay.

mod support;

use clj_value::Value;
use cljrt::{rt_arity_error, rt_print};
use support::{inbox, render};

#[test]
fn print_returns_nil() {
    let empty: Vec<*mut Value> = Vec::new();
    assert_eq!(render(unsafe { rt_print(empty.as_ptr(), 0) }), "nil");
    let one = [inbox(Value::Int(1))];
    assert_eq!(render(unsafe { rt_print(one.as_ptr(), one.len()) }), "nil");
    let three = [
        inbox(Value::Int(1)),
        inbox(Value::Str("x".into())),
        inbox(Value::Bool(false)),
    ];
    assert_eq!(
        render(unsafe { rt_print(three.as_ptr(), three.len()) }),
        "nil"
    );
}

#[test]
fn arity_error_returns_null() {
    let display = b"user/f";
    let path = b"f.clj";
    let out = unsafe {
        rt_arity_error(
            display.as_ptr(),
            display.len(),
            0,
            path.as_ptr(),
            path.len(),
            1,
            16,
        )
    };
    assert!(out.is_null());
}
