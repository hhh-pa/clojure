// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! FFI boundary tests for the mixed-program shims (issue #256):
//! `rt_register_native` + `rt_run_program`, called the way the
//! generated `main` calls them.

use std::ffi::{CString, c_char};

use clj_value::Value;
use cljrt::{rt_register_native, rt_run_program};

/// `(fn [x] (+ x 1))` over owned boxes, under the trampoline
/// contract (consumes the arg box, returns a fresh one).
extern "C" fn inc_entry(args: *mut *mut Value, argc: usize) -> *mut Value {
    assert_eq!(argc, 1);
    // SAFETY: the runtime hands `argc` valid owned boxes.
    let arg = *unsafe { Box::from_raw(*args) };
    let Value::Int(n) = arg else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(Value::Int(n + 1)))
}

#[test]
fn registered_native_runs_inside_the_run_frame() {
    let name = "inc1";
    // SAFETY: valid slice and a contract-abiding entry.
    unsafe { rt_register_native(0, name.as_ptr(), name.len(), 1, inc_entry) };
    let source = "(clj.aot/n 0)\n\
                  (assert (= 3 (inc1 2)))\n\
                  (assert (= *command-line-args* '(\"a\" \"b\")))\n";
    let path = "boundary.clj";
    let args: Vec<CString> = ["prog", "a", "b"]
        .iter()
        .map(|a| CString::new(*a).expect("cstring"))
        .collect();
    let argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
    // SAFETY: valid slices; argv holds 3 NUL-terminated strings.
    let code = unsafe {
        rt_run_program(
            source.as_ptr(),
            source.len(),
            path.as_ptr(),
            path.len(),
            argv.as_ptr(),
            3,
        )
    };
    assert_eq!(code, 0);
    // A failing program exits 1 after the `run` diagnostic.
    let failing = "(assert false)\n";
    // SAFETY: as above, argc 1.
    let code = unsafe {
        rt_run_program(
            failing.as_ptr(),
            failing.len(),
            path.as_ptr(),
            path.len(),
            argv.as_ptr(),
            1,
        )
    };
    assert_eq!(code, 1);
}
