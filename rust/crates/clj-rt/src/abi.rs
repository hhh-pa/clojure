// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

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

use std::ffi::{CStr, c_char};

use clj_eval::{NativeError, Prim};
use clj_value::{Value, Vector};

use crate::mixed::Trampoline;
use crate::runtime::{
    argv_worker, clone_worker, count_worker, fail, give, is_int_flag, keyword_worker, nth_worker,
    prim_worker, print_worker, str_worker, unbox_int, vec_push_worker,
};

/// The `extern "C"` symbol per [`Prim`], indexed by discriminant
/// (S8C codegen's single source of truth — a table, not a match:
/// one arm per prim would trip the complexity gate; the pairing is
/// pinned by the `prim_symbol` drift tests in `clj-aot` and the FFI
/// suite instead).
const PRIM_SYMBOLS: [&str; 19] = [
    "rt_add",
    "rt_sub",
    "rt_mul",
    "rt_quot",
    "rt_lt",
    "rt_le",
    "rt_gt",
    "rt_ge",
    "rt_eq",
    "rt_count",
    "rt_nth",
    "rt_get",
    "rt_conj",
    "rt_long",
    "rt_int_cast",
    "rt_transient",
    "rt_conj_bang",
    "rt_assoc_bang",
    "rt_persistent_bang",
];

/// The `extern "C"` symbol lowering [`Prim`] calls to.
#[must_use]
pub fn prim_symbol(prim: Prim) -> &'static str {
    PRIM_SYMBOLS[prim as usize]
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
    // Safety: the caller guarantees validity (see module docs).
    str_worker(&unsafe { text(ptr, len) })
}

/// One keyword box: `name` is required; the `ns` slice is read
/// only when `has_ns` is nonzero (`0` is unqualified — the slice
/// must still be valid, but is never read).
///
/// # Safety
///
/// Both byte slices must be valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_keyword(
    name_ptr: *const u8,
    name_len: usize,
    ns_ptr: *const u8,
    ns_len: usize,
    has_ns: u8,
) -> *mut Value {
    // Safety: the caller guarantees validity (see module docs).
    let name = unsafe { text(name_ptr, name_len) };
    let ns = if has_ns != 0 {
        Some(unsafe { text(ns_ptr, ns_len) })
    } else {
        None
    };
    keyword_worker(&name, ns.as_deref())
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
    *slot = vec_push_worker(old, elem);
    vec
}

/// The `*command-line-args*` value from C `argc`/`argv` (T1192
/// FX1 D-argv): `argv[0]` (the program name) is skipped; no
/// remaining args is nil, else a list of strings — the JVM
/// shape (fx1-argv probe). Bytes decode lossy (like the JVM's
/// platform decode).
///
/// # Safety
///
/// `argv` must be valid for `argc` entries (`argc >= 0`), each a
/// valid NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_argv(argv: *const *const c_char, argc: i32) -> *mut Value {
    argv_worker(&unsafe { argv_entries(argv, argc) })
}

/// C argv minus the program name.
///
/// # Safety
///
/// `argv` must be valid for `argc` entries (`argc >= 0`), each a
/// valid NUL-terminated C string outliving `'a`.
unsafe fn argv_entries<'a>(argv: *const *const c_char, argc: i32) -> Vec<&'a CStr> {
    let mut entries = Vec::new();
    // Safety: the caller guarantees validity (see module docs).
    for i in 1..argc {
        entries.push(unsafe { CStr::from_ptr(*argv.offset(i as isize)) });
    }
    entries
}

/// Clone the borrowed box into a fresh owned box.
///
/// # Safety
///
/// `value` must be a valid box (borrowed, never freed here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_clone(value: *const Value) -> *mut Value {
    // Safety: the caller guarantees a valid box (see module docs).
    clone_worker(unsafe { &*value })
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

/// Consume the box and report Clojure truthiness: `0` for nil
/// and false, `1` for everything else (the `if` test).
///
/// # Safety
///
/// `value` must be a valid box, consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_is_truthy(value: *mut Value) -> u8 {
    u8::from(unsafe { take(value) }.is_truthy())
}

/// The `print` intrinsic: consume the boxes, write the
/// space-joined rendering with NO trailing newline, return nil.
///
/// # Safety
///
/// `args` must be a valid array of `argc` valid boxes, consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_print(args: *const *mut Value, argc: usize) -> *mut Value {
    // Safety: the caller guarantees a valid array (see module docs).
    print_worker(&unsafe { take_args(args, argc) })
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
    fail(&path, line, col, &err)
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
    // Safety: the caller guarantees validity (see module docs).
    prim_worker(
        index,
        unsafe { take_args(args, argc) },
        &unsafe { text(path, path_len) },
        line,
        col,
    )
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
def_prim!(rt_get, 11);
def_prim!(rt_conj, 12);
def_prim!(rt_long, 13);
// `rt_int` is the integer-literal constructor (a different
// signature), so the `int` cast shim takes the suffixed symbol.
def_prim!(rt_int_cast, 14);
def_prim!(rt_transient, 15);
def_prim!(rt_conj_bang, 16);
def_prim!(rt_assoc_bang, 17);
def_prim!(rt_persistent_bang, 18);

/// `count` with a native string fast path (one closed-set
/// primitive — see `prim_call` for the delegated shape): a single
/// string arg counts through the shared scan cache with no mutex
/// and no evaluator roundtrip; every other shape delegates to the
/// shared native, so errors stay shared.
///
/// # Safety
///
/// `args` must be a valid array of `argc` valid boxes, consumed;
/// the path slice must be valid for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_count(
    args: *const *mut Value,
    argc: usize,
    path: *const u8,
    path_len: usize,
    line: u32,
    col: u32,
) -> *mut Value {
    // Safety: the caller guarantees validity (see module docs).
    count_worker(
        unsafe { take_args(args, argc) },
        &unsafe { text(path, path_len) },
        line,
        col,
    )
}

/// `nth` with a native string fast path (one closed-set
/// primitive — see `prim_call` for the delegated shape): a string
/// collection coerces the index and reads the char through the
/// shared functions with no mutex and no evaluator roundtrip;
/// every other shape delegates to the shared native, so errors
/// stay shared.
///
/// # Safety
///
/// `args` must be a valid array of `argc` valid boxes, consumed;
/// the path slice must be valid for its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_nth(
    args: *const *mut Value,
    argc: usize,
    path: *const u8,
    path_len: usize,
    line: u32,
    col: u32,
) -> *mut Value {
    // Safety: the caller guarantees validity (see module docs).
    nth_worker(
        unsafe { take_args(args, argc) },
        &unsafe { text(path, path_len) },
        line,
        col,
    )
}

/// Borrow-test a box for the unboxed-i64 path: `1` when the box
/// holds an `Int`, else `0` (the specialization guard — generated
/// code tests before [`rt_unbox_int`] and takes the boxed helper
/// on `0`).
///
/// # Safety
///
/// `value` must be a valid box (borrowed, never freed here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_is_int(value: *const Value) -> u8 {
    // Safety: the caller guarantees a valid box (see module docs).
    is_int_flag(unsafe { &*value })
}

/// Borrow-read an `Int` box as a raw `i64` (the unboxed-i64
/// entry: loop-carried arithmetic runs on the raw value, `rt_int`
/// re-boxes on region exit).
///
/// # Safety
///
/// `value` must be a valid box holding an `Int` (borrowed, never
/// freed here); anything else is a caller bug and degrades to `0`
/// (the `rt_vec_push` precedent — guards make it unreachable).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_unbox_int(value: *const Value) -> i64 {
    // Safety: the caller guarantees a valid box (see module docs).
    unbox_int(unsafe { &*value })
}

/// Flag one checked unboxed step: `0` in `overflow` and the value
/// on success; `1` in `overflow` and the never-read zero slot on
/// overflow (the caller re-runs the matching boxed helper, which
/// reports the shared `long overflow`).
///
/// # Safety
///
/// `overflow` must be a valid writable byte.
unsafe fn i64_flagged(result: Option<i64>, overflow: *mut u8) -> i64 {
    let (value, flag) = match result {
        Some(n) => (n, 0),
        None => (0, 1),
    };
    // Safety: the caller guarantees a writable byte (see module docs).
    unsafe { *overflow = flag };
    value
}

macro_rules! def_i64 {
    ($shim:ident, $checked:ident, $prim:literal) => {
        #[doc = concat!(
            "Unboxed-i64 `",
            $prim,
            "`: the checked step with the overflow out-flag (see `i64_flagged`). ",
            "Clojure long arithmetic never wraps, so on `1` the caller re-runs the boxed helper."
        )]
        #[doc = ""]
        #[doc = " # Safety"]
        #[doc = ""]
        #[doc = " `overflow` must be a valid writable byte."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $shim(a: i64, b: i64, overflow: *mut u8) -> i64 {
            unsafe { i64_flagged(a.$checked(b), overflow) }
        }
    };
}

def_i64!(rt_i64_add, checked_add, "+");
def_i64!(rt_i64_sub, checked_sub, "-");
def_i64!(rt_i64_mul, checked_mul, "*");

/// Register native `index` of a mixed program (issue #256): its
/// `defn` name and arity plus the uniform trampoline the masked
/// source's `(clj.aot/n index)` interns.
///
/// # Safety
///
/// The name slice must be valid for its length; `entry` must obey
/// the [`Trampoline`] contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_register_native(
    index: usize,
    name: *const u8,
    name_len: usize,
    arity: usize,
    entry: Trampoline,
) {
    crate::mixed::register(index, unsafe { text(name, name_len) }, arity, entry);
}

/// Run a mixed program's embedded source as `run` runs `path`,
/// with `*command-line-args*` from argv (program name dropped).
/// Returns the process exit code (0, or 1 after the diagnostic).
///
/// # Safety
///
/// Both slices must be valid for their lengths; `argv` must be
/// valid for `argc` entries, each a NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_run_program(
    source: *const u8,
    source_len: usize,
    path: *const u8,
    path_len: usize,
    argv: *const *const c_char,
    argc: i32,
) -> i32 {
    let source = unsafe { text(source, source_len) };
    let path = unsafe { text(path, path_len) };
    let args: Vec<String> = unsafe { argv_entries(argv, argc) }
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    crate::mixed::run_program(&path, &source, &args)
}
