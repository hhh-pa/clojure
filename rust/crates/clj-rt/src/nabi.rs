// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The `extern "C"` surface of natively compiled programs (issue
//! #259, `rt_n_*`). The audited `unsafe` boundary: generated code
//! passes 32-byte `Value` slots by pointer. Contract for every shim:
//! input slots are valid and only borrowed; output slots hold a valid
//! `Value` (generated code initializes every slot to nil), which the
//! shim replaces (dropping the old value). Fallible shims return 0
//! on success and 1 with the throw pending ([`crate::native`]).
//! Each shim converts pointers through the four helpers below and
//! delegates to safe code.

use std::ffi::{CStr, c_char};

use clj_eval::native_abi;
use clj_value::{Keyword, Symbol, Value};

use crate::native::{self, ArityDesc, Code, FnDesc, settle};
use crate::runtime::with_active;

/// Borrow a slot.
///
/// # Safety
/// `p` points to a valid `Value` that outlives `'a`.
unsafe fn slot<'a>(p: *const Value) -> &'a Value {
    unsafe { &*p }
}

/// Replace a slot's value (dropping the old one).
///
/// # Safety
/// `p` points to a valid, writable `Value`.
unsafe fn put(p: *mut Value, v: Value) {
    unsafe { *p = v };
}

/// Borrow `n` contiguous slots (`p` may be null when `n == 0`).
///
/// # Safety
/// `p` points to `n` valid `Value`s outliving `'a`.
unsafe fn slots<'a>(p: *const Value, n: usize) -> &'a [Value] {
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(p, n) }
}

/// UTF-8 text of `len` bytes (lossy).
///
/// # Safety
/// `p` is valid for `len` bytes (`p` may dangle when `len == 0`).
unsafe fn text(p: *const u8, len: usize) -> String {
    if len == 0 {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, len) }).into_owned()
}

/// Write a step's value and return its status.
///
/// # Safety
/// As [`put`].
unsafe fn finish(out: *mut Value, step: native_abi::Step) -> i32 {
    let (status, v) = settle(step);
    unsafe { put(out, v) };
    status
}

/// "No index" marker for optional table indices.
const NONE: u64 = u64::MAX;

fn meta_of(k: u64) -> Option<clj_value::Map> {
    if k == NONE {
        return None;
    }
    match native::constant(k as usize) {
        Value::Map(m) => Some(m.clone()),
        _ => None,
    }
}

// ---- program tables --------------------------------------------------

/// Declare var `k` as `ns/name`.
///
/// # Safety
/// Both texts valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_declare_var(
    k: u64,
    ns: *const u8,
    ns_len: usize,
    name: *const u8,
    name_len: usize,
) {
    native::declare_var(k as usize, unsafe { text(ns, ns_len) }, unsafe {
        text(name, name_len)
    });
}

/// Declare compiled fn `k`: its display name and `n` arities.
///
/// # Safety
/// The text and the three arrays are valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_declare_fn(
    k: u64,
    display: *const u8,
    display_len: usize,
    n: usize,
    fixed: *const u64,
    variadic: *const u8,
    codes: *const Code,
) {
    let mut arities = Vec::with_capacity(n);
    for i in 0..n {
        let (f, v, c) = unsafe { (*fixed.add(i), *variadic.add(i), *codes.add(i)) };
        arities.push(ArityDesc {
            fixed: f as usize,
            variadic: v != 0,
            code: c,
        });
    }
    native::declare_fn(
        k as usize,
        FnDesc {
            display: std::sync::Arc::from(unsafe { text(display, display_len) }),
            arities: arities.into(),
        },
    );
}

/// Store constant `k` (constant initialization only).
///
/// # Safety
/// `v` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_set_const(k: u64, v: *const Value) {
    native::set_const(k as usize, unsafe { slot(v) }.clone());
}

/// Run the program (see [`native::run_main`]); returns the exit code.
///
/// # Safety
/// `path` valid for `path_len` bytes; `argv` valid for `argc` C
/// strings; `tops` valid for `n` code pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_main(
    path: *const u8,
    path_len: usize,
    argv: *const *const c_char,
    argc: i32,
    init: Code,
    tops: *const Code,
    n: usize,
) -> i32 {
    let path = unsafe { text(path, path_len) };
    let mut args = Vec::new();
    for i in 1..argc.max(0) as usize {
        let arg = unsafe { CStr::from_ptr(*argv.add(i)) };
        args.push(arg.to_string_lossy().into_owned());
    }
    let tops: Vec<Code> = (0..n).map(|i| unsafe { *tops.add(i) }).collect();
    native::run_main(&path, &args, init, tops)
}

// ---- constant builders -------------------------------------------------

/// `out = nil`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_nil(out: *mut Value) {
    unsafe { put(out, Value::Nil) };
}

/// `out = n`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_int(out: *mut Value, n: i64) {
    unsafe { put(out, Value::Int(n)) };
}

/// `out = d`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_double(out: *mut Value, d: f64) {
    unsafe { put(out, Value::Double(d)) };
}

/// `out = b`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_bool(out: *mut Value, b: u8) {
    unsafe { put(out, Value::Bool(b != 0)) };
}

/// `out = "…"`.
///
/// # Safety
/// `out` valid; text valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_str(out: *mut Value, p: *const u8, len: usize) {
    let s = unsafe { text(p, len) };
    unsafe { put(out, Value::Str(std::sync::Arc::from(s.as_str()))) };
}

/// `out = :ns/name` (`ns_len == u64::MAX`: no namespace).
///
/// # Safety
/// `out` valid; texts valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_kw(
    out: *mut Value,
    ns: *const u8,
    ns_len: u64,
    name: *const u8,
    name_len: usize,
) {
    let ns = (ns_len != NONE).then(|| unsafe { text(ns, ns_len as usize) });
    let name = unsafe { text(name, name_len) };
    unsafe { put(out, Value::Keyword(Keyword::new(ns.as_deref(), &name))) };
}

/// `out = 'ns/name` (no metadata).
///
/// # Safety
/// As [`rt_n_kw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_sym(
    out: *mut Value,
    ns: *const u8,
    ns_len: u64,
    name: *const u8,
    name_len: usize,
) {
    let ns = (ns_len != NONE).then(|| unsafe { text(ns, ns_len as usize) });
    let name = unsafe { text(name, name_len) };
    unsafe { put(out, Value::Symbol(Symbol::new(ns.as_deref(), &name))) };
}

/// `out = #"…"` (the pattern source).
///
/// # Safety
/// `out` valid; text valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_regex(out: *mut Value, p: *const u8, len: usize) {
    let s = unsafe { text(p, len) };
    unsafe { put(out, Value::Regex(std::sync::Arc::from(s.as_str()))) };
}

/// `out =` static field `class/field` (nil when unknown).
///
/// # Safety
/// `out` valid; texts valid for their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_static_field(
    out: *mut Value,
    class: *const u8,
    class_len: usize,
    field: *const u8,
    field_len: usize,
) {
    let (class, field) = unsafe { (text(class, class_len), text(field, field_len)) };
    let v = native_abi::static_field(&class, &field).unwrap_or(Value::Nil);
    unsafe { put(out, v) };
}

/// `out =` the constant printed as `p[..len]` (reader fallback).
///
/// # Safety
/// `out` valid; text valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_read(out: *mut Value, p: *const u8, len: usize) -> i32 {
    let src = unsafe { text(p, len) };
    let step = native_abi::read_constant(&src).map_err(|msg| {
        with_active(|eval| {
            native_abi::native_throw(
                eval,
                clj_eval::NativeError::new(
                    clj_value::ThrowClass::Error,
                    format!("internal: bad constant: {msg}"),
                ),
            )
        })
    });
    unsafe { finish(out, step) }
}

// ---- values ------------------------------------------------------------

/// `out = clone(src)`.
///
/// # Safety
/// Both are valid slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_clone(out: *mut Value, src: *const Value) {
    let v = unsafe { slot(src) }.clone();
    unsafe { put(out, v) };
}

/// Reset a slot to nil (dropping its value).
///
/// # Safety
/// `v` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_drop(v: *mut Value) {
    unsafe { put(v, Value::Nil) };
}

/// Clojure truthiness.
///
/// # Safety
/// `v` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_truthy(v: *const Value) -> u8 {
    u8::from(unsafe { slot(v) }.is_truthy())
}

/// `out = constant k`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_const(out: *mut Value, k: u64) {
    unsafe { put(out, native::constant(k as usize).clone()) };
}

/// `out = @var k`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_deref(out: *mut Value, k: u64) {
    let var = native::var(k as usize);
    let v = with_active(|eval| native_abi::deref(eval, &var));
    unsafe { put(out, v) };
}

/// `out = #'var k`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_varref(out: *mut Value, k: u64) {
    unsafe { put(out, Value::Var(native::var(k as usize))) };
}

/// `out = (f args…)`.
///
/// # Safety
/// `out`, `f` valid; `args` valid for `n` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_invoke(
    out: *mut Value,
    f: *const Value,
    args: *const Value,
    n: usize,
) -> i32 {
    let (f, args) = unsafe { (slot(f), slots(args, n)) };
    let step = with_active(|eval| native_abi::invoke(eval, f, args));
    unsafe { finish(out, step) }
}

// ---- forms -------------------------------------------------------------

/// `(def …)` first half: returns the var id for [`rt_n_def_finish`].
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_def_begin(sym_k: u64) -> u64 {
    let Value::Symbol(sym) = native::constant(sym_k as usize) else {
        return 0;
    };
    with_active(|eval| native_abi::def_begin(eval, sym))
}

/// `(def …)` second half; `out = the var`.
///
/// # Safety
/// `out` valid; `value` null or valid; `meta` valid for `n_meta`
/// slots unless `n_meta == u64::MAX`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_def_finish(
    out: *mut Value,
    id: u64,
    sym_k: u64,
    doc_k: u64,
    value: *const Value,
    meta: *const Value,
    n_meta: u64,
    line: u32,
    col: u32,
) -> i32 {
    let Value::Symbol(sym) = native::constant(sym_k as usize) else {
        return 0;
    };
    let doc = match (doc_k != NONE).then(|| native::constant(doc_k as usize)) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    };
    let value = (!value.is_null()).then(|| unsafe { slot(value) }.clone());
    let kvs = (n_meta != NONE).then(|| unsafe { slots(meta, n_meta as usize) });
    let step = with_active(|eval| {
        let name_meta = match kvs {
            Some(kvs) => Some(native_abi::map_pairs(eval, kvs)?),
            None => sym.meta().cloned(),
        };
        Ok(native_abi::def_finish(
            eval, id, sym, doc, value, name_meta, line, col,
        ))
    });
    unsafe { finish(out, step) }
}

/// `out = [items…]` with optional constant metadata `meta_k`.
///
/// # Safety
/// `out` valid; `items` valid for `n` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_vec_lit(out: *mut Value, items: *const Value, n: usize, meta_k: u64) {
    let v = native_abi::vec_lit(unsafe { slots(items, n) }.to_vec(), meta_of(meta_k));
    unsafe { put(out, v) };
}

/// `out = {k v …}` over `n` slots (keys and values interleaved).
///
/// # Safety
/// As [`rt_n_vec_lit`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_map_lit(
    out: *mut Value,
    kvs: *const Value,
    n: usize,
    meta_k: u64,
) -> i32 {
    let kvs = unsafe { slots(kvs, n) };
    let step = with_active(|eval| native_abi::map_lit(eval, kvs, meta_of(meta_k)));
    unsafe { finish(out, step) }
}

/// `out = #{items…}`.
///
/// # Safety
/// As [`rt_n_vec_lit`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_set_lit(
    out: *mut Value,
    items: *const Value,
    n: usize,
    meta_k: u64,
) -> i32 {
    let items = unsafe { slots(items, n) };
    let step = with_active(|eval| native_abi::set_lit(eval, items, meta_of(meta_k)));
    unsafe { finish(out, step) }
}

/// `(throw v)`: always 1 (pending).
///
/// # Safety
/// `v` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_throw(v: *const Value) -> i32 {
    let v = unsafe { slot(v) }.clone();
    native::fail(with_active(|eval| native_abi::throw(eval, v)))
}

/// `out =` a fresh throwable of class constant `class_k` (a string)
/// with message constant `msg_k`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_new_throwable(out: *mut Value, class_k: u64, msg_k: u64) {
    let class = match native::constant(class_k as usize) {
        Value::Str(s) => native_abi::throw_class(s),
        _ => clj_value::ThrowClass::Exception,
    };
    let msg = match native::constant(msg_k as usize) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    };
    let v = with_active(|eval| native_abi::new_throwable(eval, class, &msg));
    unsafe { put(out, v) };
}

/// `out =` a closure over compiled fn `k` capturing `n` slots.
///
/// # Safety
/// `out` valid; `captured` valid for `n` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_make_fn(out: *mut Value, k: u64, captured: *const Value, n: usize) {
    let captured = unsafe { slots(captured, n) }.to_vec();
    let v = with_active(|eval| native::make_fn(eval, k as usize, captured));
    unsafe { put(out, v) };
}

/// `try` over a compiled body fn, `n` catches (class names in the
/// constant vector `classes_k`, handler fns in `handlers`), and an
/// optional finally fn (`fin` null when absent).
///
/// # Safety
/// `out`, `body` valid; `handlers` valid for `n`; `fin` null or valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_try(
    out: *mut Value,
    body: *const Value,
    classes_k: u64,
    handlers: *const Value,
    n: usize,
    fin: *const Value,
) -> i32 {
    let classes: Vec<String> = match native::constant(classes_k as usize) {
        Value::Vector(v) => v
            .iter()
            .map(|c| match c {
                Value::Str(s) => s.to_string(),
                _ => String::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let handlers = unsafe { slots(handlers, n) };
    let catches: Vec<(String, Value)> = classes.into_iter().zip(handlers.iter().cloned()).collect();
    let body = unsafe { slot(body) };
    let fin = (!fin.is_null()).then(|| unsafe { slot(fin) });
    let step = with_active(|eval| native_abi::try_call(eval, body, &catches, fin));
    unsafe { finish(out, step) }
}

/// `binding` of vars `ks` (`n` table indices) to `values` around the
/// body fn.
///
/// # Safety
/// `out`, `body` valid; `ks` and `values` valid for `n`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_binding(
    out: *mut Value,
    ks: *const u64,
    values: *const Value,
    n: usize,
    body: *const Value,
) -> i32 {
    let values = unsafe { slots(values, n) };
    let pairs: Vec<_> = (0..n)
        .map(|i| {
            (
                native::var(unsafe { *ks.add(i) } as usize),
                values[i].clone(),
            )
        })
        .collect();
    let body = unsafe { slot(body) };
    let step = with_active(|eval| native_abi::binding_call(eval, pairs, body));
    unsafe { finish(out, step) }
}

/// `(set! var-k v)`.
///
/// # Safety
/// `out`, `v` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_set_var(out: *mut Value, k: u64, v: *const Value) -> i32 {
    let var = native::var(k as usize);
    let v = unsafe { slot(v) }.clone();
    let step = with_active(|eval| native_abi::set_var(eval, &var, v));
    unsafe { finish(out, step) }
}

/// `(declare …)` of the symbols in constant vector `syms_k`.
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_declare(out: *mut Value, syms_k: u64, line: u32, col: u32) {
    let syms: Vec<Symbol> = match native::constant(syms_k as usize) {
        Value::Vector(v) => v
            .iter()
            .filter_map(|s| match s {
                Value::Symbol(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let v = with_active(|eval| native_abi::declare(eval, &syms, line, col));
    unsafe { put(out, v) };
}

/// `(ns …)` described by constant `ns_k` (see `native::decode_ns`).
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_ns(out: *mut Value, ns_k: u64) -> i32 {
    let step = native::run_ns(native::constant(ns_k as usize));
    unsafe { finish(out, step) }
}

/// `case*` dispatch for constant descriptor `case_k`: `*sel` gets -1
/// (default), `i` (take case i), or `i | 1 << 32` (take case i if its
/// test equivs `*realized`).
///
/// # Safety
/// `sel` writable; `realized`, `v` valid slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_case(
    sel: *mut i64,
    realized: *mut Value,
    v: *const Value,
    case_k: u64,
) -> i32 {
    let v = unsafe { slot(v) };
    match native::case_select(native::constant(case_k as usize), v) {
        Ok((s, r)) => {
            unsafe {
                *sel = s;
                put(realized, r);
            }
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `*out = (= a b)` as 0/1.
///
/// # Safety
/// `out` writable; `a`, `b` valid slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_equiv(out: *mut u8, a: *const Value, b: *const Value) -> i32 {
    let (a, b) = unsafe { (slot(a), slot(b)) };
    match with_active(|eval| native_abi::equiv(eval, a, b)) {
        Ok(eq) => {
            unsafe { *out = u8::from(eq) };
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `out = (nth coll i nil)` (positional destructuring).
///
/// # Safety
/// `out`, `coll` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_nth_nil(out: *mut Value, coll: *const Value, i: u64) -> i32 {
    let coll = unsafe { slot(coll) };
    let step = with_active(|eval| native_abi::nth_or_nil(eval, coll, i as usize));
    unsafe { finish(out, step) }
}

/// `out = gseq(v)` (`&` destructuring).
///
/// # Safety
/// `out`, `v` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_gseq(out: *mut Value, v: *const Value) -> i32 {
    let v = unsafe { slot(v) };
    let step = with_active(|eval| native_abi::gseq(eval, v));
    unsafe { finish(out, step) }
}

/// `out = (first v)`.
///
/// # Safety
/// `out`, `v` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_first(out: *mut Value, v: *const Value) -> i32 {
    let v = unsafe { slot(v) };
    let step = with_active(|eval| native_abi::first(eval, v));
    unsafe { finish(out, step) }
}

/// `out = (next v)`.
///
/// # Safety
/// `out`, `v` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_next(out: *mut Value, v: *const Value) -> i32 {
    let v = unsafe { slot(v) };
    let step = with_active(|eval| native_abi::next(eval, v));
    unsafe { finish(out, step) }
}

/// Destructure `v` against encoded pattern constant `k` (with `nd`
/// evaluated `:or` defaults) into the `n` slots at `out`.
///
/// # Safety
/// `out` valid for `n` slots; `v` valid; `defaults` valid for `nd`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_destructure(
    out: *mut Value,
    n: usize,
    v: *const Value,
    k: u64,
    defaults: *const Value,
    nd: usize,
) -> i32 {
    let (v, defaults) = unsafe { (slot(v), slots(defaults, nd)) };
    let enc = native::constant(k as usize);
    match with_active(|eval| native_abi::destructure_encoded(eval, enc, v, defaults)) {
        Ok(vals) => {
            for (i, val) in vals.into_iter().take(n).enumerate() {
                unsafe { put(out.add(i), val) };
            }
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `out =` the class named `p[..len]` (folded class constants).
///
/// # Safety
/// `out` valid; text valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_class(out: *mut Value, p: *const u8, len: usize) {
    let name = unsafe { text(p, len) };
    let v = with_active(|eval| native_abi::class_named(eval, &name));
    unsafe { put(out, v) };
}

/// `out =` a fresh `letfn` cell (a volatile holding nil).
///
/// # Safety
/// `out` is a valid slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_cell_new(out: *mut Value) {
    unsafe { put(out, Value::Volatile(clj_value::Volatile::new(Value::Nil))) };
}

/// Store `v` into the cell `cell`.
///
/// # Safety
/// `cell`, `v` valid slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_cell_set(cell: *const Value, v: *const Value) {
    if let Value::Volatile(c) = unsafe { slot(cell) } {
        *c.lock() = unsafe { slot(v) }.clone();
    }
}

/// `out =` the cell's value.
///
/// # Safety
/// `out`, `cell` valid slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_n_cell_get(out: *mut Value, cell: *const Value) {
    let v = match unsafe { slot(cell) } {
        Value::Volatile(c) => c.lock().clone(),
        other => other.clone(),
    };
    unsafe { put(out, v) };
}
