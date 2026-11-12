// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The `extern "C"` surface of natively compiled programs (issue
//! #259, `rt_n_*`).
//!
//! Generated code passes 32-byte `Value` slots by pointer. Single
//! slots arrive as references (`&Value` borrowed, `&mut Value`
//! output, `Option<&Value>` nullable) — ABI-identical to pointers, so
//! the FFI contract ("valid, aligned, initialized, not aliased with
//! the output") is what the generated code guarantees and the shim
//! bodies stay safe. Every output slot holds a valid `Value` (the
//! generated code nil-initializes slots) that the shim replaces.
//! Arrays (slot runs, byte strings, index tables, C argv) arrive as
//! [`CSlice`] headers, whose two accessors (plus the argv `CStr` read)
//! are the whole audited `unsafe` boundary.
//!
//! Fallible shims return 0 on success and 1 with the throw pending
//! ([`crate::native`]).

use std::ffi::{CStr, c_char};

use clj_eval::native_abi;
use clj_value::{Keyword, Symbol, Value};

use crate::native::{self, ArityDesc, Code, FnDesc, settle};
use crate::runtime::with_active;

/// `len` contiguous `T`s at `ptr`: the `{ptr, len}` header generated
/// code passes by reference for every array argument (texts, slot
/// runs, index tables, argv).
///
/// The fields are private and nothing in Rust constructs one: a
/// `CSlice` only ever arrives from generated code, which guarantees
/// `ptr` is valid (aligned, initialized) for `len` items for the
/// duration of the call and that a `&mut CSlice`'s items alias nothing
/// else the shim receives. That makes the two accessors below the
/// whole audited boundary for arrays.
#[repr(C)]
pub struct CSlice<T> {
    ptr: *mut T,
    len: usize,
}

impl<T> CSlice<T> {
    /// The items (empty when `len == 0`, whatever `ptr` is).
    fn get(&self) -> &[T] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the type-level contract above.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// The items, mutably (the shim moves values out of them).
    fn get_mut(&mut self) -> &mut [T] {
        if self.len == 0 {
            return &mut [];
        }
        // SAFETY: the type-level contract above (exclusive items).
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl CSlice<u8> {
    /// The bytes as (lossy) UTF-8 text.
    fn text(&self) -> String {
        String::from_utf8_lossy(self.get()).into_owned()
    }
}

/// One arity of a compiled fn as generated code describes it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ArityC {
    fixed: u64,
    variadic: u64,
    code: Code,
}

/// Write a step's value and return its status.
fn finish(out: &mut Value, step: native_abi::Step) -> i32 {
    let (status, v) = settle(step);
    *out = v;
    status
}

/// "No index" marker for optional table indices.
const NONE: u64 = u64::MAX;

fn meta_of(k: u64) -> Option<clj_value::Map> {
    match (k != NONE).then(|| native::constant(k as usize)) {
        Some(Value::Map(m)) => Some(m.clone()),
        _ => None,
    }
}

fn str_const(k: u64) -> String {
    match native::constant(k as usize) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
}

fn sym_const(k: u64) -> Symbol {
    match native::constant(k as usize) {
        Value::Symbol(s) => s.clone(),
        _ => Symbol::new(None, "_"),
    }
}

fn var_pairs(ks: &CSlice<u64>, values: &CSlice<Value>) -> Vec<(clj_value::Var, Value)> {
    ks.get()
        .iter()
        .zip(values.get())
        .map(|(k, v)| (native::var(*k as usize).clone(), v.clone()))
        .collect()
}

/// The C `argv` strings after the program name.
fn argv_strings(argv: &CSlice<*const c_char>) -> Vec<String> {
    argv.get()
        .iter()
        .skip(1)
        .map(|a| {
            // SAFETY: `argv` is the C runtime's `main` vector (via
            // generated code): every entry is a NUL-terminated string.
            unsafe { CStr::from_ptr(*a) }.to_string_lossy().into_owned()
        })
        .collect()
}

// ---- program tables --------------------------------------------------

/// Declare var `k` as `ns/name`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_declare_var(k: u64, ns: &CSlice<u8>, name: &CSlice<u8>) {
    native::declare_var(k as usize, ns.text(), name.text());
}

/// Declare compiled fn `k`: its display name and arities.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_declare_fn(k: u64, display: &CSlice<u8>, arities: &CSlice<ArityC>) {
    let arities: Vec<ArityDesc> = arities
        .get()
        .iter()
        .map(|a| ArityDesc {
            fixed: a.fixed as usize,
            variadic: a.variadic != 0,
            code: a.code,
        })
        .collect();
    native::declare_fn(
        k as usize,
        FnDesc {
            display: std::sync::Arc::from(display.text()),
            arities: arities.into(),
        },
    );
}

/// Store constant `k` (constant initialization only).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_set_const(k: u64, v: &Value) {
    native::set_const(k as usize, v.clone());
}

/// Run the program (see [`native::run_main`]); returns the exit code.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_main(
    path: &CSlice<u8>,
    argv: &CSlice<*const c_char>,
    init: Code,
    tops: &CSlice<Code>,
) -> i32 {
    native::run_main(&path.text(), &argv_strings(argv), init, tops.get().to_vec())
}

// ---- constant builders -------------------------------------------------

/// `out = nil`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_nil(out: &mut Value) {
    *out = Value::Nil;
}

/// `out = n`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_int(out: &mut Value, n: i64) {
    *out = Value::Int(n);
}

/// `out = d`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_double(out: &mut Value, d: f64) {
    *out = Value::Double(d);
}

/// `out = b`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_bool(out: &mut Value, b: u8) {
    *out = Value::Bool(b != 0);
}

/// `out = "…"`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_str(out: &mut Value, s: &CSlice<u8>) {
    *out = Value::Str(std::sync::Arc::from(s.text().as_str()));
}

/// `out = :ns/name` (`ns` null: no namespace).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_kw(out: &mut Value, ns: Option<&CSlice<u8>>, name: &CSlice<u8>) {
    let ns = ns.map(CSlice::text);
    *out = Value::Keyword(Keyword::new(ns.as_deref(), &name.text()));
}

/// `out = 'ns/name` (no metadata; `ns` null: no namespace).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_sym(out: &mut Value, ns: Option<&CSlice<u8>>, name: &CSlice<u8>) {
    let ns = ns.map(CSlice::text);
    *out = Value::Symbol(Symbol::new(ns.as_deref(), &name.text()));
}

/// `out = #"…"` (the pattern source).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_regex(out: &mut Value, s: &CSlice<u8>) {
    *out = Value::Regex(std::sync::Arc::from(s.text().as_str()));
}

/// `out =` static field `class/field` (nil when unknown).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_static_field(out: &mut Value, class: &CSlice<u8>, field: &CSlice<u8>) {
    *out = native_abi::static_field(&class.text(), &field.text()).unwrap_or(Value::Nil);
}

/// `out =` the class named `name` (folded class constants).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_class(out: &mut Value, name: &CSlice<u8>) {
    let name = name.text();
    *out = with_active(|eval| native_abi::class_named(eval, &name));
}

/// `out =` the constant printed as `src` (reader fallback).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_read(out: &mut Value, src: &CSlice<u8>) -> i32 {
    let step = native_abi::read_constant(&src.text()).map_err(|msg| {
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
    finish(out, step)
}

// ---- values ------------------------------------------------------------

/// `out = clone(src)`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_clone(out: &mut Value, src: &Value) {
    *out = src.clone();
}

/// Reset a slot to nil (dropping its value).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_drop(v: &mut Value) {
    *v = Value::Nil;
}

/// `out = constant k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_const(out: &mut Value, k: u64) {
    *out = native::constant(k as usize).clone();
}

/// `out = @var k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_deref(out: &mut Value, k: u64) {
    *out = with_active(|eval| native::root(eval, k as usize));
}

/// `out = #'var k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_varref(out: &mut Value, k: u64) {
    *out = Value::Var(native::var(k as usize).clone());
}

/// `out = (f args…)`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_invoke(out: &mut Value, f: &Value, args: &CSlice<Value>) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::invoke(eval, f, args.get())),
    )
}

/// Move the argument slots out (each left nil).
fn take_args(args: &mut CSlice<Value>) -> clj_eval::SmallArgs {
    args.get_mut()
        .iter_mut()
        .map(|a| std::mem::replace(a, Value::Nil))
        .collect()
}

/// `out = (var-k args…)` moving the argument slots in (each left nil).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_native_mv(out: &mut Value, k: u64, args: &mut CSlice<Value>) -> i32 {
    let moved = take_args(args);
    let step = with_active(|eval| {
        let callee = native::root(eval, k as usize);
        native_abi::call_value_owned(eval, &callee, moved)
    });
    finish(out, step)
}

/// [`rt_n_native_mv`] for a core var of a program that cannot rebind
/// it: the native entry is resolved once and called directly.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_core_mv(out: &mut Value, k: u64, args: &mut CSlice<Value>) -> i32 {
    let moved = take_args(args);
    let step = with_active(|eval| match native::core_entry(eval, k as usize) {
        Some(entry) => native_abi::call_core(eval, entry, moved),
        None => {
            let callee = native::root(eval, k as usize);
            native_abi::call_value_owned(eval, &callee, moved)
        }
    });
    finish(out, step)
}

/// A covered-case core intrinsic (`native_abi::intrinsic` op) over
/// borrowed args; uncovered cases run var `k`'s native (args cloned).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_intr(out: &mut Value, op: u32, k: u64, args: &CSlice<Value>) -> i32 {
    let args = args.get();
    if let Some(v) = native_abi::intrinsic(op, args) {
        *out = v;
        return 0;
    }
    let step = with_active(|eval| match native::core_entry(eval, k as usize) {
        Some(entry) => native_abi::call_core(eval, entry, args.iter().cloned().collect()),
        None => {
            let callee = native::root(eval, k as usize);
            native_abi::call_value(eval, &callee, args)
        }
    });
    finish(out, step)
}

/// Enter a direct call of compiled `defn` `desc` through var `k`: 1
/// when the var still holds that fn and the call depth/stack allow
/// (the caller then calls the code and [`rt_n_leave`]), else 0 (take
/// the general invoke).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_enter_fn(k: u64, desc: u64) -> u8 {
    with_active(|eval| {
        u8::from(
            native::root_is_fn(eval, k as usize, desc as usize) && native_abi::enter_call(eval),
        )
    })
}

/// Leave a call entered by [`rt_n_enter_fn`].
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_leave() {
    with_active(native_abi::leave_call);
}

// ---- forms -------------------------------------------------------------

/// `(def …)` first half: returns the var id for [`rt_n_def_finish`].
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_def_begin(sym_k: u64) -> u64 {
    let sym = sym_const(sym_k);
    with_active(|eval| native_abi::def_begin(eval, &sym))
}

/// `(def …)` second half; `out = the var`. `value` is null without an
/// init; `meta` (interleaved keys and values) is null without
/// evaluated name metadata.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_def_finish(
    out: &mut Value,
    id: u64,
    sym_k: u64,
    doc_k: u64,
    value: Option<&Value>,
    meta: Option<&CSlice<Value>>,
    line: u32,
    col: u32,
) -> i32 {
    let sym = sym_const(sym_k);
    let doc = (doc_k != NONE).then(|| str_const(doc_k));
    let value = value.cloned();
    let step = with_active(|eval| {
        let name_meta = match meta {
            Some(kvs) => Some(native_abi::map_pairs(eval, kvs.get())?),
            None => sym.meta().cloned(),
        };
        Ok(native_abi::def_finish(
            eval, id, &sym, doc, value, name_meta, line, col,
        ))
    });
    finish(out, step)
}

/// `out = [items…]` with optional constant metadata `meta_k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_lit(out: &mut Value, items: &CSlice<Value>, meta_k: u64) {
    *out = native_abi::vec_lit(items.get().to_vec(), meta_of(meta_k));
}

/// `out = {k v …}` (keys and values interleaved).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_map_lit(out: &mut Value, kvs: &CSlice<Value>, meta_k: u64) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::map_lit(eval, kvs.get(), meta_of(meta_k))),
    )
}

/// `out = #{items…}`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_set_lit(out: &mut Value, items: &CSlice<Value>, meta_k: u64) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::set_lit(eval, items.get(), meta_of(meta_k))),
    )
}

/// `(throw v)`: always 1 (pending).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_throw(v: &Value) -> i32 {
    let v = v.clone();
    native::fail(with_active(|eval| native_abi::throw(eval, v)))
}

/// `out =` a fresh throwable of class constant `class_k` (a string)
/// with message constant `msg_k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_new_throwable(out: &mut Value, class_k: u64, msg_k: u64) {
    let class = native_abi::throw_class(&str_const(class_k));
    let msg = str_const(msg_k);
    *out = with_active(|eval| native_abi::new_throwable(eval, class, &msg));
}

/// `out =` a closure over compiled fn `k` capturing the slots.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_make_fn(out: &mut Value, k: u64, captured: &CSlice<Value>) {
    let captured = captured.get().to_vec();
    *out = with_active(|eval| native::make_fn(eval, k as usize, captured));
}

/// `try` over a compiled body fn, catches (class names in the constant
/// vector `classes_k`, handler fns in `handlers`), and an optional
/// finally fn (`fin` null when absent).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_try(
    out: &mut Value,
    body: &Value,
    classes_k: u64,
    handlers: &CSlice<Value>,
    fin: Option<&Value>,
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
    let catches: Vec<(String, Value)> = classes
        .into_iter()
        .zip(handlers.get().iter().cloned())
        .collect();
    finish(
        out,
        with_active(|eval| native_abi::try_call(eval, body, &catches, fin)),
    )
}

/// `binding` of vars `ks` (table indices) to `values` around the body
/// fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_binding(
    out: &mut Value,
    ks: &CSlice<u64>,
    values: &CSlice<Value>,
    body: &Value,
) -> i32 {
    let pairs = var_pairs(ks, values);
    finish(
        out,
        with_active(|eval| native_abi::binding_call(eval, pairs, body)),
    )
}

/// `(set! var-k v)`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_set_var(out: &mut Value, k: u64, v: &Value) -> i32 {
    let var = native::var(k as usize);
    let v = v.clone();
    finish(out, with_active(|eval| native_abi::set_var(eval, var, v)))
}

/// `(declare …)` of the symbols in constant vector `syms_k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_declare(out: &mut Value, syms_k: u64, line: u32, col: u32) {
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
    *out = with_active(|eval| native_abi::declare(eval, &syms, line, col));
}

/// `(ns …)` described by constant `ns_k` (see `native::run_ns`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_ns(out: &mut Value, ns_k: u64) -> i32 {
    finish(out, native::run_ns(native::constant(ns_k as usize)))
}

/// `case*` dispatch for constant descriptor `case_k`: `*sel` gets -1
/// (default), `i` (take case i), or `i | 1 << 32` (take case i if its
/// test equivs `*realized`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_case(sel: &mut i64, realized: &mut Value, v: &Value, case_k: u64) -> i32 {
    match native::case_select(native::constant(case_k as usize), v) {
        Ok((s, r)) => {
            *sel = s;
            *realized = r;
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `*out = (= a b)` as 0/1.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_equiv(out: &mut u8, a: &Value, b: &Value) -> i32 {
    match with_active(|eval| native_abi::equiv(eval, a, b)) {
        Ok(eq) => {
            *out = u8::from(eq);
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `out = (nth coll i nil)` (positional destructuring).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_nth_nil(out: &mut Value, coll: &Value, i: u64) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::nth_or_nil(eval, coll, i as usize)),
    )
}

/// `out = gseq(v)` (`&` destructuring).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_gseq(out: &mut Value, v: &Value) -> i32 {
    finish(out, with_active(|eval| native_abi::gseq(eval, v)))
}

/// `out = (first v)`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_first(out: &mut Value, v: &Value) -> i32 {
    finish(out, with_active(|eval| native_abi::first(eval, v)))
}

/// `v = (next v)` in place.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_next_mut(v: &mut Value) -> i32 {
    let step = with_active(|eval| native_abi::next(eval, v));
    finish(v, step)
}

/// Destructure `v` against encoded pattern constant `k` (with
/// evaluated `:or` defaults) into the `out` slots.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_destructure(
    out: &mut CSlice<Value>,
    v: &Value,
    k: u64,
    defaults: &CSlice<Value>,
) -> i32 {
    let enc = native::constant(k as usize);
    match with_active(|eval| native_abi::destructure_encoded(eval, enc, v, defaults.get())) {
        Ok(vals) => {
            for (slot, val) in out.get_mut().iter_mut().zip(vals) {
                *slot = val;
            }
            0
        }
        Err(signal) => native::fail(signal),
    }
}

/// `out =` a fresh `letfn` cell (a volatile holding nil).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cell_new(out: &mut Value) {
    *out = Value::Volatile(clj_value::Volatile::new(Value::Nil));
}

/// Store `v` into the cell `cell`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cell_set(cell: &Value, v: &Value) {
    if let Value::Volatile(c) = cell {
        *c.lock() = v.clone();
    }
}

/// `out =` the cell's value.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cell_get(out: &mut Value, cell: &Value) {
    *out = match cell {
        Value::Volatile(c) => c.lock().clone(),
        other => other.clone(),
    };
}

// ---- extent forms (bodies arrive as compiled zero-arg fns) -------------

/// `with-redefs` of vars `ks` to `values` around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_with_redefs(
    out: &mut Value,
    ks: &CSlice<u64>,
    values: &CSlice<Value>,
    body: &Value,
) -> i32 {
    let vals = var_pairs(ks, values);
    finish(
        out,
        with_active(|eval| native_abi::with_redefs_call(eval, vals, body)),
    )
}

/// `with-bindings*` of `map` around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_with_bindings(out: &mut Value, map: &Value, body: &Value) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::with_bindings_call(eval, map, body)),
    )
}

/// `time` around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_time(out: &mut Value, body: &Value) -> i32 {
    finish(out, with_active(|eval| native_abi::time_call(eval, body)))
}

/// `with-out-str` (out var symbol constant `sym_k`) around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_with_out_str(out: &mut Value, sym_k: u64, body: &Value) -> i32 {
    let sym = sym_const(sym_k);
    finish(
        out,
        with_active(|eval| native_abi::with_out_str_call(eval, &sym, body)),
    )
}

/// `with-in-str` (input var symbol constant `sym_k`) of `s` around the
/// body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_with_in_str(out: &mut Value, sym_k: u64, s: &Value, body: &Value) -> i32 {
    let sym = sym_const(sym_k);
    finish(
        out,
        with_active(|eval| native_abi::with_in_str_call(eval, &sym, s, body)),
    )
}

/// `locking` on `target` around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_locking(out: &mut Value, target: &Value, body: &Value) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::locking_call(eval, target, body)),
    )
}

/// `sync` (`io` when `io != 0`) around the body fn.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_sync(out: &mut Value, io: u8, body: &Value) -> i32 {
    let step = with_active(|eval| {
        if io == 0 {
            native_abi::sync_call(eval, body)
        } else {
            native_abi::io_call(eval, body)
        }
    });
    finish(out, step)
}

/// `out =` a `lazy-seq` (`kind` 0), `delay` (1), or `bound-fn` (2)
/// over the compiled fn `f`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_wrap_fn(out: &mut Value, kind: u8, f: &Value) {
    let f = f.clone();
    *out = match kind {
        0 => native_abi::lazy_seq(f),
        1 => native_abi::delay(f),
        _ => with_active(|eval| native_abi::bound_fn(eval, f)),
    };
}

/// `defmacro` described by constant `k` (`[sym doc attr arglists
/// line col]`) over the compiled macro fn `f`; `out =` the var.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_defmacro(out: &mut Value, k: u64, f: &Value) -> i32 {
    finish(
        out,
        native::run_defmacro(native::constant(k as usize), f.clone()),
    )
}

/// A failed `assert` (`msg` null when absent; the form text constant
/// `form_k`): always 1 (pending).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_assert_fail(msg: Option<&Value>, form_k: u64) -> i32 {
    let msg = msg.cloned();
    let form = str_const(form_k);
    native::fail(with_active(|eval| {
        native_abi::assert_failed(eval, msg, &form)
    }))
}

/// Every `rt_n_*` shim by symbol name (the in-process JIT maps a
/// native module's declarations to these).
#[must_use]
pub fn native_symbols() -> Vec<(&'static str, usize)> {
    macro_rules! syms {
        ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
    }
    syms![
        rt_n_declare_var,
        rt_n_declare_fn,
        rt_n_set_const,
        rt_n_main,
        rt_n_nil,
        rt_n_int,
        rt_n_double,
        rt_n_bool,
        rt_n_str,
        rt_n_kw,
        rt_n_sym,
        rt_n_regex,
        rt_n_static_field,
        rt_n_class,
        rt_n_read,
        rt_n_clone,
        rt_n_drop,
        rt_n_const,
        rt_n_deref,
        rt_n_varref,
        rt_n_invoke,
        rt_n_native_mv,
        rt_n_core_mv,
        rt_n_intr,
        rt_n_enter_fn,
        rt_n_leave,
        rt_n_def_begin,
        rt_n_def_finish,
        rt_n_vec_lit,
        rt_n_map_lit,
        rt_n_set_lit,
        rt_n_throw,
        rt_n_new_throwable,
        rt_n_make_fn,
        rt_n_try,
        rt_n_binding,
        rt_n_set_var,
        rt_n_declare,
        rt_n_ns,
        rt_n_case,
        rt_n_equiv,
        rt_n_nth_nil,
        rt_n_gseq,
        rt_n_first,
        rt_n_next_mut,
        rt_n_destructure,
        rt_n_cell_new,
        rt_n_cell_set,
        rt_n_cell_get,
        rt_n_with_redefs,
        rt_n_with_bindings,
        rt_n_time,
        rt_n_with_out_str,
        rt_n_with_in_str,
        rt_n_locking,
        rt_n_sync,
        rt_n_wrap_fn,
        rt_n_defmacro,
        rt_n_assert_fail,
    ]
}
