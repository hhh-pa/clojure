// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Shared box helpers for the `clj-rt` FFI tests.

use clj_value::{Value, print_to_string};

/// Box a value for a helper call (the helper consumes it).
pub fn inbox(value: Value) -> *mut Value {
    Box::into_raw(Box::new(value))
}

/// Take a helper return back (frees the box).
///
/// # Safety
///
/// `ptr` must be a non-null box from a helper return.
pub unsafe fn unbox(ptr: *mut Value) -> Value {
    unsafe { *Box::from_raw(ptr) }
}

/// Render a helper return to its `print` string (frees the box).
pub fn render(ptr: *mut Value) -> String {
    let value = unsafe { unbox(ptr) };
    print_to_string(&value)
}
