// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The audited `unsafe` boundary: `extern "C"` helpers over opaque
//! `*mut Value` boxes (S8 plan D1/D2/D7).
//!
//! Contract (enforced by S8C codegen, documented per shim):
//! pointers are non-null and valid; helper arguments are consumed
//! (each box freed exactly once); the return is owned (null on
//! error — the generated code null-checks and exits 1); byte
//! slices are valid for their length. Violating the contract is a
//! caller bug and instant UB — the shims do not null-check (the
//! FFI boundary is internal to a built binary, not a public C
//! API), except [`rt_vec_push`]'s wrong-type arm, which degrades
//! to a fresh vector so the line stays meaningful.
//!
//! Every shim is `#[unsafe(no_mangle)]` so the platform link finds the
//! plain `rt_*` symbols in the static archive.

use std::ptr;
use std::sync::Arc;

use clj_eval::{NativeError, Prim};
use clj_value::{Value, Vector};

use crate::runtime::{call_prim, error_line, print_join};

/// The `extern "C"` symbol lowering [`Prim`] calls to (S8C
/// codegen's single source of truth — no parallel table).
#[must_use]
pub fn prim_symbol(prim: Prim) -> &'static str {
    match prim {
        Prim::Add => "rt_add",
        Prim::Sub => "rt_sub",
        Prim::Mul => "rt_mul",
        Prim::Quot => "rt_quot",
        Prim::Lt => "rt_lt",
        Prim::Le => "rt_le",
        Prim::Gt => "rt_gt",
        Prim::Ge => "rt_ge",
        Prim::Eq => "rt_eq",
        Prim::Count => "rt_count",
        Prim::Nth => "rt_nth",
        Prim::Get => "rt_get",
        Prim::Conj => "rt_conj",
    }
}

/// Borrow a caller byte slice.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    // Safety: the caller guarantees validity (see module docs).
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

/// Decode a caller byte string (lossy — codegen only ever passes
/// valid UTF-8, so the lossy arm never fires in practice).
unsafe fn text(ptr: *const u8, len: usize) -> String {
    String::from_utf8_lossy(unsafe { bytes(ptr, len) }).into_owned()
}

/// Box a fresh owned value for the caller.
fn give(value: Value) -> *mut Value {
    Box::into_raw(Box::new(value))
}

/// Take ownership of a caller box.
unsafe fn take(ptr: *mut Value) -> Value {
    // Safety: the caller transfers ownership (see module docs).
    unsafe { *Box::from_raw(ptr) }
}

/// Take ownership of every box in a caller array (the array
/// itself stays caller-owned — only the boxes move).
unsafe fn take_args(args: *const *mut Value, argc: usize) -> Vec<Value> {
    // Safety: the caller guarantees a valid array (see module docs).
    let items = unsafe { std::slice::from_raw_parts(args, argc) };
    items.iter().map(|ptr| unsafe { take(*ptr) }).collect()
}

/// One `nil` box.
#[unsafe(no_mangle)]
pub extern "C" fn rt_nil() -> *mut Value {
    give(Value::Nil)
}

/// One boolean box: `0` is false, anything else true.
#[unsafe(no_mangle)]
pub extern "C" fn rt_bool(flag: u8) -> *mut Value {
    give(Value::Bool(flag != 0))
}

/// One integer box.
#[unsafe(no_mangle)]
pub extern "C" fn rt_int(n: i64) -> *mut Value {
    give(Value::Int(n))
}

/// One double box.
#[unsafe(no_mangle)]
pub extern "C" fn rt_double(x: f64) -> *mut Value {
    give(Value::Double(x))
}

/// One string box decoding `len` bytes at `ptr`.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_str(ptr: *const u8, len: usize) -> *mut Value {
    give(Value::Str(Arc::from(unsafe { text(ptr, len) }.as_str())))
}

/// One empty vector box.
#[unsafe(no_mangle)]
pub extern "C" fn rt_vec_new() -> *mut Value {
    give(Value::Vector(Vector::from_vec(Vec::new())))
}

/// Push the owned `elem` box onto the owned `vec` box in place
/// and return `vec` for chaining (a non-vector `vec` is a caller
/// bug — it degrades to a fresh single-element vector).
///
/// # Safety
///
/// Both boxes must be valid; `elem` is consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_vec_push(vec: *mut Value, elem: *mut Value) -> *mut Value {
    let elem = unsafe { take(elem) };
    // Safety: the caller guarantees a valid box (see module docs).
    let slot = unsafe { &mut *vec };
    let old = std::mem::replace(slot, Value::Nil);
    *slot = match old {
        Value::Vector(items) => Value::Vector(items.push_back(elem)),
        _ => Value::Vector(Vector::from_vec(vec![elem])),
    };
    vec
}

/// Clone the borrowed box into a fresh owned box.
///
/// # Safety
///
/// `value` must be a valid box (borrowed, never freed here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_clone(value: *const Value) -> *mut Value {
    // Safety: the caller guarantees a valid box (see module docs).
    give(unsafe { (*value).clone() })
}

/// Free the owned box.
///
/// # Safety
///
/// `value` must be a valid box, freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_drop(value: *mut Value) {
    drop(unsafe { take(value) });
}

/// The `print` intrinsic: consume the boxes, write the
/// space-joined rendering with NO trailing newline, return nil.
///
/// # Safety
///
/// `args` must be a valid array of `argc` valid boxes, consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_print(args: *const *mut Value, argc: usize) -> *mut Value {
    let owned = unsafe { take_args(args, argc) };
    print!("{}", print_join(&owned));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    give(Value::Nil)
}

/// A sibling call with the wrong arg count: always fails with the
/// shared arity text over the stable `user/name` display.
///
/// # Safety
///
/// Both byte slices must be valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_arity_error(
    display: *const u8,
    display_len: usize,
    got: usize,
    path: *const u8,
    path_len: usize,
    line: u32,
    col: u32,
) -> *mut Value {
    let err = NativeError::arity(&unsafe { text(display, display_len) }, got);
    let path = unsafe { text(path, path_len) };
    eprintln!("{}", error_line(&path, line, col, &err));
    ptr::null_mut()
}

/// The shared primitive body: consume the boxes, call the shared
/// native, box the result — or print `path:line:col: Class:
/// message` and return null.
///
/// # Safety
///
/// `args` must be a valid array of `argc` valid boxes,
/// consumed; the path slice must be valid for its length.
unsafe fn prim_call(
    index: usize,
    args: *const *mut Value,
    argc: usize,
    path: *const u8,
    path_len: usize,
    line: u32,
    col: u32,
) -> *mut Value {
    let owned = unsafe { take_args(args, argc) };
    match call_prim(index, owned) {
        Ok(value) => give(value),
        Err(err) => {
            let path = unsafe { text(path, path_len) };
            eprintln!("{}", error_line(&path, line, col, &err));
            ptr::null_mut()
        }
    }
}

macro_rules! def_prim {
    ($shim:ident, $index:expr) => {
        #[doc = "One closed-set primitive (see `prim_call`): consume the boxes, call the shared native."]
        #[doc = ""]
        #[doc = " # Safety"]
        #[doc = ""]
        #[doc = " `args` must be a valid array of `argc` valid boxes, consumed;"]
        #[doc = " the path slice must be valid for its length."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $shim(
            args: *const *mut Value,
            argc: usize,
            path: *const u8,
            path_len: usize,
            line: u32,
            col: u32,
        ) -> *mut Value {
            unsafe { prim_call($index, args, argc, path, path_len, line, col) }
        }
    };
}

def_prim!(rt_add, 0);
def_prim!(rt_sub, 1);
def_prim!(rt_mul, 2);
def_prim!(rt_quot, 3);
def_prim!(rt_lt, 4);
def_prim!(rt_le, 5);
def_prim!(rt_gt, 6);
def_prim!(rt_ge, 7);
def_prim!(rt_eq, 8);
def_prim!(rt_count, 9);
def_prim!(rt_nth, 10);
def_prim!(rt_get, 11);
def_prim!(rt_conj, 12);
