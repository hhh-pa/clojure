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

use std::borrow::Cow;
use std::ffi::{CStr, c_char};

use clj_eval::native_abi;
use clj_value::{FastVars, Keyword, Symbol, Value};

use crate::native::{self, ArityDesc, Code, items_of, settle, str_of, sym_of};
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
    /// The bytes borrowed as UTF-8 when valid, avoiding heap allocation on
    /// AOT string/symbol/keyword literals (the compiler emits only valid
    /// UTF-8: `from_utf8`'s word-at-a-time ASCII check is several times
    /// cheaper than the lossy decoder, which stays the fallback).
    fn as_str(&self) -> Cow<'_, str> {
        let bytes = self.get();
        std::str::from_utf8(bytes).map_or_else(|_| String::from_utf8_lossy(bytes), Cow::Borrowed)
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
    str_of(native::constant(k as usize))
}

fn sym_const(k: u64) -> Symbol {
    sym_of(native::constant(k as usize))
}

fn var_pairs(ks: &CSlice<u64>, values: &CSlice<Value>) -> Vec<(clj_value::Var, Value)> {
    ks.get()
        .iter()
        .zip(values.get())
        .map(|(k, v)| (native::var(*k as usize).clone(), v.clone()))
        .collect()
}

/// The C `argv` strings after the program name (skipping any null entry).
fn argv_strings(argv: &CSlice<*const c_char>) -> Vec<String> {
    argv.get()
        .iter()
        .skip(1)
        .filter(|&&a| !a.is_null())
        .map(|&a| {
            // SAFETY: `argv` is the C runtime's `main` vector (via
            // generated code): every non-null entry is a NUL-terminated string.
            unsafe { CStr::from_ptr(a) }.to_string_lossy().into_owned()
        })
        .collect()
}

// ---- program tables --------------------------------------------------

/// One declared var as generated code describes it: its namespace and
/// name as byte spans `(at, len)` of the program's names text, and its
/// Lite native hint.
#[repr(C)]
pub struct VarRow {
    ns_at: u32,
    ns_len: u32,
    at: u32,
    len: u32,
    hint: u32,
}

/// Declare the program's vars, in index order: `names` holds each
/// namespace once and every var name, and each row spans its var's
/// namespace and name in it (#271: the text is validated and copied
/// once, with no allocation per var).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_declare_vars(names: &CSlice<u8>, rows: &CSlice<VarRow>) {
    let text: std::sync::Arc<str> = std::sync::Arc::from(names.as_str().as_ref());
    native::declare_vars(
        text,
        rows.get()
            .iter()
            .map(|r| ((r.ns_at, r.ns_len), (r.at, r.len), r.hint)),
    );
}

/// One declared compiled fn as generated code describes it: its display
/// name span in the names text and its arities slice span in the shared
/// arities table (#271).
#[repr(C)]
pub struct FnRow {
    name_at: u32,
    name_len: u32,
    arity_at: u32,
    arity_len: u32,
}

/// Declare every compiled fn in index order and pre-size the constant
/// table (#271: one names text, one arities array, zero per-fn
/// allocations at startup).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_declare_fns(
    names: &CSlice<u8>,
    rows: &CSlice<FnRow>,
    arities: &CSlice<ArityC>,
    n_consts: u64,
) {
    let text: std::sync::Arc<str> = std::sync::Arc::from(names.as_str().as_ref());
    native::declare_fns(
        text,
        rows.get()
            .iter()
            .map(|r| ((r.name_at, r.name_len), (r.arity_at, r.arity_len))),
        arities.get().iter().map(|a| ArityDesc {
            fixed: a.fixed as usize,
            variadic: a.variadic != 0,
            code: a.code,
        }),
        n_consts as usize,
    );
}

/// Store constant `k`, moving it out of `v` (leaving nil; constant
/// initialization only, #271: avoids cloning and immediately dropping
/// every constant).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_set_const(k: u64, v: &mut Value) {
    native::set_const(k as usize, std::mem::replace(v, Value::Nil));
}

/// Run the program (see [`native::run_main`]); returns the exit code.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_main(
    path: &CSlice<u8>,
    argv: &CSlice<*const c_char>,
    init: Code,
    tops: &CSlice<Code>,
) -> i32 {
    native::run_main(
        &path.as_str(),
        &argv_strings(argv),
        init,
        tops.get().to_vec(),
    )
}

/// Run the program under `BootMode::Lite` (`need_stdin != 0` initializes `*in*`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_main_lite(
    path: &CSlice<u8>,
    argv: &CSlice<*const c_char>,
    init: Code,
    tops: &CSlice<Code>,
    need_stdin: u8,
) -> i32 {
    native::run_main_lite(
        &path.as_str(),
        &argv_strings(argv),
        init,
        tops.get(),
        need_stdin != 0,
    )
}

// ---- constant builders -------------------------------------------------

/// `out = "…"`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_str(out: &mut Value, s: &CSlice<u8>) {
    *out = Value::Str(std::sync::Arc::from(s.as_str().as_ref()));
}

/// `out = :ns/name` (`ns` null: no namespace).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_kw(out: &mut Value, ns: Option<&CSlice<u8>>, name: &CSlice<u8>) {
    let ns = ns.map(CSlice::as_str);
    *out = Value::Keyword(Keyword::new(ns.as_deref(), &name.as_str()));
}

/// `out = 'ns/name` (no metadata; `ns` null: no namespace).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_sym(out: &mut Value, ns: Option<&CSlice<u8>>, name: &CSlice<u8>) {
    let ns = ns.map(CSlice::as_str);
    *out = Value::Symbol(Symbol::new(ns.as_deref(), &name.as_str()));
}

/// `out = ^{…} ns/name`: a symbol with metadata (`kvs`: interleaved
/// keys and values), built as the reader would without reading (#271).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_meta_sym(
    out: &mut Value,
    ns: Option<&CSlice<u8>>,
    name: &CSlice<u8>,
    kvs: &CSlice<Value>,
) {
    let ns = ns.map(CSlice::as_str);
    *out = native_abi::meta_symbol(ns.as_deref(), &name.as_str(), kvs.get());
}

/// `out =` a metadata-free constant collection over `items` (`kind`: 0
/// list, 1 vector, 2 map of interleaved keys and values, else a set),
/// built without the reader (#271).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_coll(out: &mut Value, kind: u64, items: &CSlice<Value>) {
    *out = native_abi::const_coll(kind, items.get());
}

/// `out = #"…"` (the pattern source).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_regex(out: &mut Value, s: &CSlice<u8>) {
    *out = Value::Regex(std::sync::Arc::from(s.as_str().as_ref()));
}

/// `out =` static field `class/field` (nil when unknown).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_static_field(out: &mut Value, class: &CSlice<u8>, field: &CSlice<u8>) {
    *out = native_abi::static_field(&class.as_str(), &field.as_str()).unwrap_or(Value::Nil);
}

/// `out =` the class named `name` (folded class constants).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_class(out: &mut Value, name: &CSlice<u8>) {
    let name = name.as_str();
    *out = with_active(|eval| native_abi::class_named(eval, &name));
}

/// `out =` the constant printed as `src` (reader fallback).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_read(out: &mut Value, src: &CSlice<u8>) {
    // The build checked every such constant reads back to itself.
    *out = native_abi::read_constant(&src.as_str()).expect("pooled constants read back");
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

/// A compiled loop's iteration plan for `coll` (#271 Slice 4c): `plan` =
/// (count, start, step); returns 1 for a persistent vector, 2 for a long
/// range, and 0 when the loop must take the general call.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_iter_plan(coll: &Value, plan: &mut [i64; 3]) -> u8 {
    let Some((kind, n, start, step)) = native_abi::iter_plan(coll) else {
        return 0;
    };
    *plan = [n, start, step];
    kind
}

/// A compiled loop's direct plan for `(range a0 [a1 [a2]])` with all-long
/// args (#271 Slice 5l), without building the range value: `plan` =
/// (count, start, step); returns 2 for a finite long range (an empty
/// shape answers count 0) and 0 when the loop must take the general
/// call (a zero-step repeat, a count overflow).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_range_plan(argc: u8, a0: i64, a1: i64, a2: i64, plan: &mut [i64; 3]) -> u8 {
    let Some((kind, n, start, step)) = native_abi::range_plan(usize::from(argc), [a0, a1, a2])
    else {
        return 0;
    };
    *plan = [n, start, step];
    kind
}

/// `out = v[i]` (a persistent vector and an in-range index).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_item(out: &mut Value, v: &Value, i: u64) {
    *out = native_abi::vec_item(v, i as usize);
}

/// `out =` a fresh accumulator for a compiled `mapv` / `vec` loop (`mode == 0`:
/// `VecBuilder` pre-reserving `len`; `mode == 1/2`: zero-filled packed `Int`/
/// `Double` transient vector for unboxed scalar `mapv`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_new(out: &mut Value, len: i64, mode: u8) {
    *out = native_abi::vec_builder(len, mode);
}

/// `out =` accumulator `acc` frozen into a persistent vector.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_done(out: &mut Value, acc: &mut Value) {
    *out = native_abi::vec_done(acc);
}

/// Append `v` (taken: left nil) to accumulator `acc`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_push(acc: &mut Value, v: &mut Value) {
    native_abi::vec_push(acc, std::mem::replace(v, Value::Nil));
}

/// Append every item of `coll` (taken: left nil) to accumulator `acc`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_push_all(acc: &mut Value, coll: &mut Value) -> i32 {
    let coll = std::mem::replace(coll, Value::Nil);
    native::status(with_active(|eval| {
        native_abi::vec_push_all(eval, acc, coll)
    }))
}

/// Unwrap a `reduced` accumulator in place: 1 when it was one.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_unreduce(v: &mut Value) -> u8 {
    u8::from(native_abi::unreduce(v))
}

/// Fast path for `(mapv #(Long/parseLong %) coll)` (`mode`: 0 Long, 1 Int, 2 Double):
/// writes `out` and returns 1 on hit, 0 on miss.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_mapv_parse_num(out: &mut Value, coll: &Value, mode: u8) -> u8 {
    let Some(v) = native_abi::try_mapv_parse_num(coll, mode) else {
        return 0;
    };
    *out = v;
    1
}

/// Fast path for `(reduce + init (map * c1 c2))` over two numeric vectors:
/// writes `out` and returns 1 on hit, 0 on miss.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_dot(out: &mut Value, init: &Value, c1: &Value, c2: &Value) -> u8 {
    let Some(v) = native_abi::try_dot_product(init, c1, c2) else {
        return 0;
    };
    *out = v;
    1
}

/// Streaming fast path for `(println|print (clojure.string/join sep coll))`:
/// sets `*hit = 1` when streamed directly to `*out*`, `0` when fallback is needed;
/// returns `0` on `Ok` and `1` on runtime error.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_print_join(hit: &mut u8, sep: &Value, coll: &Value, flags: u8) -> i32 {
    native::status(with_active(|eval| {
        native_abi::try_print_join(eval, sep, coll, flags, hit)
    }))
}

/// Probe a packed character vector or subvector for a fused traversal.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_select_chars(out: &mut Value, coll: &Value, codepoint: u32, mode: u8) -> i8 {
    let Some(value) =
        char::from_u32(codepoint).and_then(|c| native_abi::select_chars(coll, c, mode))
    else {
        return 0;
    };
    *out = value;
    1
}

/// Render mapped strings separated by the compiler-supplied literal.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_join_mapped_str(out: &mut Value, sep: &CSlice<u8>, coll: &mut Value) -> i32 {
    let input = std::mem::replace(coll, Value::Nil);
    let text = sep.as_str();
    finish(
        out,
        with_active(|eval| native_abi::join_mapped_str(eval, &text, input)),
    )
}

/// Fold `res` (taken: left nil) into `acc` using `clojure.core/+` for a
/// compiled `(reduce + [init] (map F ...))` loop.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_add_acc(acc: &mut Value, res: &mut Value) -> i32 {
    let rhs = std::mem::replace(res, Value::Nil);
    native::status(with_active(|eval| native_abi::add_acc(eval, acc, rhs)))
}

/// The frozen constant table's first slot (#271): generated code borrows
/// `&consts[k]` in place of cloning constant `k` into a temporary.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_consts_base() -> *const Value {
    native::consts_base()
}

/// `out = @var k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_deref(out: &mut Value, k: u64) {
    *out = with_active(|eval| native::root(eval, k as usize));
}

/// Process-wide lock-free fast-var table (initialized at `freeze`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_fast_vars() -> &'static FastVars {
    native::fast_vars()
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

/// Enter a call of `f` with `argc` args when `f` is a compiled closure of
/// exactly that fixed arity (#271): writes its code address and captured
/// values for the call site to call directly (then `rt_n_leave`), as a
/// `defn`'s direct call does, and answers 1 past the call-depth check.
/// 0 (nothing entered) leaves the call to `rt_n_invoke`: other callees,
/// other arities, and the depth error.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_enter_closure(
    f: &Value,
    argc: u64,
    code: &mut usize,
    captured: &mut *const Value,
) -> u8 {
    let entry = clj_eval::as_native(f)
        .and_then(|n| n.compiled.as_ref())
        .filter(|e| e.fixed as u64 == argc);
    let Some(entry) = entry else {
        return 0;
    };
    if !with_active(native_abi::enter_call) {
        return 0;
    }
    *code = entry.code as usize;
    *captured = entry.captured.as_ptr();
    1
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

/// `out = (var-k args…)` over borrowed args, for a program that cannot
/// rebind core vars: the VM's inline op `op` (from
/// `native_abi::inline_op`; `u32::MAX` for none) when it covers the
/// case, else var `k`'s native entry with the args cloned. No
/// argument is cloned on the inline path.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_core_ref(out: &mut Value, op: u32, k: u64, args: &CSlice<&Value>) -> i32 {
    let args = args.get();
    match native_abi::apply_inline(op, args) {
        Some(v) => {
            out.assign(v);
            0
        }
        None => core_fallback(out, k, args),
    }
}

/// [`rt_n_core_ref`] for core ops without any inline VM op (`op == u32::MAX`),
/// keeping `APPLY1`/`APPLY2`/`APPLY3` unreferenced when a binary uses none.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_core_call(out: &mut Value, k: u64, args: &CSlice<&Value>) -> i32 {
    core_fallback(out, k, args.get())
}

/// Fallback of the `rt_n_op*` shims: var `k`'s native with the args
/// cloned. A native taking 1-3 args borrowed gets them in a stack array
/// and writes `out` directly: no argument buffer, and no `Step` on the
/// success path (#271: ~470 instructions per generic core call on
/// Linux, the native's own work often a few).
fn core_fallback(out: &mut Value, k: u64, args: &[&Value]) -> i32 {
    with_active(|eval| {
        // Direct core calls come only from programs that cannot rebind
        // core vars (a rebinding program calls each var's current root),
        // so var k holds the native the analyzer saw (#271).
        let entry = native::core_entry(eval, k as usize)
            .expect("a direct core call's var holds its native");
        match entry.call_refs(eval, args) {
            Ok(v) => {
                out.assign(v);
                0
            }
            Err(e) => {
                *out = Value::Nil;
                native::fail(native_abi::native_throw(eval, e))
            }
        }
    })
}

/// [`rt_n_core_ref`] for one borrowed arg (no argument array).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_op1(out: &mut Value, op: u32, k: u64, a: &Value) -> i32 {
    match native_abi::apply1(op, a) {
        Some(v) => {
            out.assign(v);
            0
        }
        None => core_fallback(out, k, &[a]),
    }
}

/// [`rt_n_core_ref`] for two borrowed args excluding `re-matches` (no regex
/// dependency for non-regex programs).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_op2(out: &mut Value, op: u32, k: u64, a: &Value, b: &Value) -> i32 {
    if op == native_abi::OP_ATOM_NTH {
        return op2_atom_nth(out, k, a, b);
    }
    match native_abi::apply2_no_re(op, a, b) {
        Some(v) => {
            out.assign(v);
            0
        }
        None => core_fallback(out, k, &[a, b]),
    }
}

/// `(nth @a i)` for [`rt_n_op2`], out of line so the common ops run
/// without its frame.
#[inline(never)]
fn op2_atom_nth(out: &mut Value, k: u64, a: &Value, b: &Value) -> i32 {
    if let Some(v) = native_abi::try_atom_nth(a, b) {
        *out = v;
        return 0;
    }
    let mut derefed = Value::Nil;
    let st = core_fallback(&mut derefed, k >> 32, &[a]);
    if st != 0 {
        return st;
    }
    core_fallback(out, k & 0xFFFF_FFFF, &[&derefed, b])
}

/// Dedicated inline `(re-matches pat s)` fast path so `rt_n_op2` stays free of
/// `regex_automata`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_op2_re(out: &mut Value, k: u64, a: &Value, b: &Value) -> i32 {
    match native_abi::apply_re_matches(a, b) {
        Some(v) => {
            *out = v;
            0
        }
        None => core_fallback(out, k, &[a, b]),
    }
}

/// The integer multiply/quotient path for a result narrowed to Long.
/// Returns 0 when the ordinary Clojure arithmetic must handle the operands.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_scaled_long(out: &mut Value, a: &Value, b: &Value, d: &Value) -> u8 {
    if let Some(n) = native_abi::try_scaled_long(a, b, d) {
        *out = Value::Int(n);
        1
    } else {
        0
    }
}

/// [`rt_n_core_ref`] for three borrowed args (no argument array).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_op3(
    out: &mut Value,
    op: u32,
    k: u64,
    a: &Value,
    b: &Value,
    c: &Value,
) -> i32 {
    if op == native_abi::OP_ATOM_NTH2 {
        return op3_atom_nth2(out, k, a, b, c);
    }
    match native_abi::apply3(op, a, b, c) {
        Some(v) => {
            out.assign(v);
            0
        }
        None => core_fallback(out, k, &[a, b, c]),
    }
}

/// `(nth (nth @a i) j)` for [`rt_n_op3`], out of line like
/// [`op2_atom_nth`].
#[inline(never)]
fn op3_atom_nth2(out: &mut Value, k: u64, a: &Value, b: &Value, c: &Value) -> i32 {
    if let Some(v) = native_abi::try_atom_nth2(a, b, c) {
        *out = v;
        return 0;
    }
    let mut derefed = Value::Nil;
    let st = core_fallback(&mut derefed, k >> 32, &[a]);
    if st != 0 {
        return st;
    }
    let mut row = Value::Nil;
    let st = core_fallback(&mut row, k & 0xFFFF_FFFF, &[&derefed, b]);
    if st != 0 {
        return st;
    }
    core_fallback(out, k & 0xFFFF_FFFF, &[&row, c])
}

/// Update a long at an index in an owned vector, preserving the input on a miss.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_assoc_nth_step(
    out: &mut Value,
    coll: &mut Value,
    idx: &Value,
    step: i64,
) -> u8 {
    let (Value::Vector(v), Value::Int(i)) = (&mut *coll, idx) else {
        return 0;
    };
    let Ok(i) = i32::try_from(*i).and_then(usize::try_from) else {
        return 0;
    };
    let old = match v.get_val_ref(i) {
        Some(clj_value::ValueRef::Int(n)) => n,
        Some(clj_value::ValueRef::Ref(Value::Int(n))) => *n,
        _ => return 0,
    };
    let Some(next) = old.checked_add(step) else {
        return 0;
    };
    let set = v.set_nth_mut(i, Value::Int(next));
    debug_assert!(set);
    *out = std::mem::replace(coll, Value::Nil);
    1
}

/// Three-argument `assoc` over owned slots.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_assoc_own3(out: &mut Value, k: u64, args: &mut [Value; 3]) -> i32 {
    if let Some(v) = native_abi::assoc_owned(args) {
        *out = v;
        return 0;
    }
    let moved = args
        .iter_mut()
        .map(|arg| std::mem::replace(arg, Value::Nil))
        .collect();
    finish(
        out,
        with_active(|eval| native::call_core_var(eval, k as usize, moved)),
    )
}

/// [`rt_n_core_ref`] over owned args (a local taken at its last use,
/// #259): the inline op over them when it covers the case, else var
/// `k`'s native with the args moved in (each slot left nil), so an
/// owned-args native updates a uniquely held collection in place.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_core_own(out: &mut Value, op: u32, k: u64, args: &mut CSlice<Value>) -> i32 {
    if let Some(v) = native_abi::apply_own(op, args.get_mut()) {
        *out = v;
        return 0;
    }
    let inline = match args.get() {
        [a] => native_abi::apply_inline(op, &[a]),
        [a, b] => native_abi::apply_inline(op, &[a, b]),
        [a, b, c] => native_abi::apply_inline(op, &[a, b, c]),
        _ => None,
    };
    if let Some(v) = inline {
        *out = v;
        return 0;
    }
    let mut moved = take_args(args);
    native_abi::repack_own_args(op, &mut moved);
    finish(
        out,
        with_active(|eval| native::call_core_var(eval, k as usize, moved)),
    )
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

/// Enter a direct call to a stable `defn` (#271): [`rt_n_enter_fn`]'s
/// call-depth check without its root check (the var holds that fn for
/// the rest of the run). 1 = call the code, 0 = take the general invoke.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_enter() -> u8 {
    with_active(|eval| u8::from(native_abi::enter_call(eval)))
}

/// Leave a call entered by [`rt_n_enter_fn`] or [`rt_n_enter`].
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
    def_finish_named(out, id, &sym_const(sym_k), doc_k, value, meta, (line, col))
}

/// `(def …)` first half for a plain qualified name: its declared var
/// slot `var_k` is the target (no symbol constant, no second intern).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_def_begin_var(var_k: u64) -> u64 {
    let var = native::var(var_k as usize);
    with_active(|eval| native_abi::def_begin_var(eval, var.id))
}

/// [`rt_n_def_finish`] for a [`rt_n_def_begin_var`] target (named by
/// its var slot).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_def_finish_var(
    out: &mut Value,
    id: u64,
    var_k: u64,
    doc_k: u64,
    value: Option<&Value>,
    meta: Option<&CSlice<Value>>,
    line: u32,
    col: u32,
) -> i32 {
    let sym = &native::var(var_k as usize).sym;
    def_finish_named(out, id, sym, doc_k, value, meta, (line, col))
}

/// The shared `(def …)` second half: the root, the name's metadata
/// (evaluated pairs, else the symbol's own), the doc, the span.
fn def_finish_named(
    out: &mut Value,
    id: u64,
    sym: &Symbol,
    doc_k: u64,
    value: Option<&Value>,
    meta: Option<&CSlice<Value>>,
    (line, col): (u32, u32),
) -> i32 {
    let value = value.cloned();
    let step = with_active(|eval| {
        // A program that keeps no var metadata never reads the doc
        // (#271: no string built per def).
        let doc = (doc_k != NONE && native_abi::keeps_var_meta(eval)).then(|| str_const(doc_k));
        let name_meta = match meta {
            Some(kvs) => Some(native_abi::map_pairs(eval, kvs.get())?),
            None => sym.meta().cloned(),
        };
        Ok(native_abi::def_finish(
            eval, id, sym, doc, value, name_meta, line, col,
        ))
    });
    finish(out, step)
}

/// `out = [items…]` with optional constant metadata `meta_k`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_vec_lit(out: &mut Value, items: &mut CSlice<Value>, meta_k: u64) {
    *out = native_abi::vec_lit_owned(items.get_mut(), meta_of(meta_k));
}

/// `out = {k v …}` (keys and values interleaved).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_map_lit(out: &mut Value, kvs: &CSlice<Value>, meta_k: u64) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::map_lit(eval, kvs.get(), meta_of(meta_k))),
    )
}

/// `out =` a keyword map literal (#271): the constant `template`'s keys
/// (a shaped `{k nil …}`) over the evaluated `vals`, sharing its shape.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_kw_map(out: &mut Value, template: &Value, vals: &CSlice<Value>) {
    *out = native_abi::kw_map(template, vals.get());
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
    let classes = items_of(native::constant(classes_k as usize));
    let catches: Vec<(String, Value)> = classes
        .iter()
        .map(str_of)
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
    let syms: Vec<Symbol> = items_of(native::constant(syms_k as usize))
        .iter()
        .map(sym_of)
        .collect();
    *out = with_active(|eval| native_abi::declare(eval, &syms, line, col));
}

/// `(ns …)` described by constant `ns_k` (see `native::run_ns`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_ns(out: &mut Value, ns_k: u64) -> i32 {
    finish(out, native::run_ns(native::constant(ns_k as usize)))
}

/// Lightweight `(ns name …)` transition in `BootMode::Lite`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_ns_lite(out: &mut Value, name: &CSlice<u8>) {
    let ns = name.as_str();
    with_active(|eval| clj_eval::aot_boot::ensure_ns_lite(eval, &ns));
    *out = Value::Nil;
}

/// The compiled Lite program can observe var metadata: its `def`s keep it
/// (#271; called before `rt_n_main_lite`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_var_meta() {
    native::declare_lite_var_meta();
}

/// Register `BootMode::Lite` I/O capability natives (`println`, `slurp`, …).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_io() {
    native::declare_lite_core_cap(clj_eval::aot_boot::LITE_CORE_IO, None, false);
}

/// Register `BootMode::Lite` core capability natives.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_core() {
    native::declare_lite_core_cap(
        clj_eval::aot_boot::LITE_CORE_CORE,
        Some(clj_eval::aot_boot::LITE_OWNED_CORE),
        false,
    );
}

/// Register `BootMode::Lite` sequence/lazy/transducer capability natives.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_seq() {
    native::declare_lite_core_cap(
        clj_eval::aot_boot::LITE_CORE_SEQ,
        Some(clj_eval::aot_boot::LITE_OWNED_SEQ),
        true,
    );
}

/// Register `BootMode::Lite` regex capability natives.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_re() {
    native::declare_lite_core_cap(clj_eval::aot_boot::LITE_CORE_RE, None, false);
}

/// Register `BootMode::Lite` host/concurrency/STM/protocol capability natives.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_host() {
    native::declare_lite_core_cap(clj_eval::aot_boot::LITE_CORE_HOST, None, false);
}

/// Register `BootMode::Lite` `clojure.string` capability natives.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_cap_string() {
    native::declare_lite_lib_cap(
        "clojure.string",
        clj_eval::aot_boot::LITE_CAP_STRING,
        clj_eval::aot_boot::LITE_OWNED_STRING,
    );
}

/// Lightweight `(require 'clojure.string)` / `(require '[clojure.string :as s])`
/// in `BootMode::Lite`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_require_string_lite(out: &mut Value, alias: &CSlice<u8>) {
    let text = alias.as_str();
    let alias_opt = (!text.is_empty()).then_some(text.as_ref());
    with_active(|eval| clj_eval::aot_boot::require_string_lite(eval, alias_opt));
    *out = Value::Nil;
}

/// `case*` dispatch for constant descriptor `case_k`: `*sel` gets -1
/// (default), `i` (take case i), or `i | 1 << 32` (take case i if its
/// test equivs `*realized`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_case(sel: &mut i64, realized: &mut Value, v: &Value, case_k: u64) -> i32 {
    native::status(
        native::case_select(native::constant(case_k as usize), v).map(|(s, r)| {
            *sel = s;
            *realized = r;
        }),
    )
}

/// `*out = (= a b)` as 0/1.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_equiv(out: &mut u8, a: &Value, b: &Value) -> i32 {
    native::status(with_active(|eval| native_abi::equiv(eval, a, b)).map(|eq| *out = u8::from(eq)))
}

/// `out = (nth coll i nil)` (positional destructuring).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_nth_nil(out: &mut Value, coll: &Value, i: u64) -> i32 {
    finish(
        out,
        with_active(|eval| native_abi::nth_or_nil(eval, coll, i as usize)),
    )
}

/// Two successive `nth` calls, without cloning a borrowed vector row.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_nth2(out: &mut Value, k: u64, coll: &Value, a: &Value, b: &Value) -> i32 {
    if let Some(value) = native_abi::try_nth2(coll, a, b) {
        *out = value;
        return 0;
    }
    let mut row = Value::Nil;
    let status = core_fallback(&mut row, k, &[coll, a]);
    if status != 0 {
        return status;
    }
    core_fallback(out, k, &[&row, b])
}

/// Index a vector's last item or run the original `peek` and `nth`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_peek_nth(out: &mut Value, k: u64, coll: &Value, index: &Value) -> i32 {
    if let Some(value) = native_abi::try_peek_nth(coll, index) {
        *out = value;
        return 0;
    }
    let mut row = Value::Nil;
    let status = core_fallback(&mut row, k >> 32, &[coll]);
    if status != 0 {
        return status;
    }
    core_fallback(out, k & 0xFFFF_FFFF, &[&row, index])
}

/// `out = gseq(v)` (`&` destructuring).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_gseq(out: &mut Value, v: &Value) -> i32 {
    finish(out, with_active(|eval| native_abi::gseq(eval, v)))
}

/// Evaluate a `seq` test without allocating a sequence for strict collections.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_seq_nonempty(out: &mut Value, v: &Value) -> i32 {
    if let Some(present) = native_abi::try_seq_nonempty(v) {
        *out = Value::Bool(present);
        return 0;
    }
    finish(
        out,
        with_active(|eval| native_abi::gseq(eval, v).map(|seq| Value::Bool(seq.is_truthy()))),
    )
}

/// `out = (first v)`.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_first(out: &mut Value, v: &Value) -> i32 {
    finish(out, with_active(|eval| native_abi::first(eval, v)))
}

/// Take the first item of a fresh lazy sequence.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_first_mapped(out: &mut Value, seq: &mut Value) -> i32 {
    let seq = std::mem::replace(seq, Value::Nil);
    finish(out, with_active(|eval| native_abi::first_mapped(eval, seq)))
}

/// Collect a `mapcat` into an empty vector.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_into_mapcat(out: &mut Value, f: &Value, coll: &mut Value) -> i32 {
    let coll = std::mem::replace(coll, Value::Nil);
    finish(
        out,
        with_active(|eval| native_abi::into_mapcat(eval, f, coll)),
    )
}

/// `(vec (sort ...))` over owned sort arguments.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_sort_vec(out: &mut Value, args: &mut CSlice<Value>) -> i32 {
    let args = take_args(args);
    finish(out, with_active(|eval| native_abi::sort_vec(eval, args)))
}

/// `v = (next v)` in place.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_next_mut(v: &mut Value) -> i32 {
    let step = with_active(|eval| native_abi::next(eval, v));
    finish(v, step)
}

/// `out =` the map-destructure source `v` as its bindings read it
/// (#271): a list converts to its map, anything else is itself.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_destr_map(out: &mut Value, v: &Value) -> i32 {
    if !matches!(v, Value::List(_)) {
        *out = v.clone();
        return 0;
    }
    let converted = with_active(|eval| native_abi::destructure_source(eval, v));
    native::status(converted.map(|m| *out = m))
}

/// `out = (f a)` / `(f a not-found)` when `f` is a map, set, keyword or
/// symbol (#271): the lookup the general invoke ends in, without it.
/// 0 leaves the call to `rt_n_invoke` (closures and other callees).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_lookup_call(
    out: &mut Value,
    f: &Value,
    a: &Value,
    not_found: Option<&Value>,
) -> u8 {
    match native_abi::lookup_value_call(f, a, not_found) {
        Some(v) => {
            *out = v;
            1
        }
        None => 0,
    }
}

/// `out =` one map-destructure binding (#271): `key` read from the
/// converted source `m` when present (a present `nil` stays `nil`),
/// else `default` when the binding has one, else nil.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_destr_get(out: &mut Value, m: &Value, key: &Value, default: Option<&Value>) {
    *out = native_abi::destructure_lookup(m, key)
        .or_else(|| default.cloned())
        .unwrap_or(Value::Nil);
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
    native::status(
        with_active(|eval| native_abi::destructure_encoded(eval, enc, v, defaults.get())).map(
            |vals| {
                for (slot, val) in out.get_mut().iter_mut().zip(vals) {
                    *slot = val;
                }
            },
        ),
    )
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
    if let Value::Volatile(c) = cell {
        *out = c.lock().clone();
    }
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

/// Fused `(assoc! t1 k (conj (nth t2 k) x))` in place: returns 1 on
/// hit, 0 for fallback (leaving inputs intact on 0).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_assoc_conj(
    t1: &Value,
    t2: &Value,
    k: &Value,
    x: &mut Value,
    take_x: u8,
) -> u8 {
    u8::from(native_abi::assoc_conj(t1, t2, k, x, take_x != 0))
}

/// Effect-only `(conj! coll x)`: returns 1 on hit, 0 for fallback.
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_conj_bang_fx(a: &Value, b: &Value) -> u8 {
    u8::from(native_abi::conj_bang_fx(a, b))
}

/// Fast C-ABI ASCII predicate for long strings called from `clj-bc` (`1` = ASCII).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_str_ascii(v: &Value) -> u8 {
    match v {
        Value::Str(s) if native_abi::str_is_ascii(s) => 1,
        _ => 0,
    }
}

fn parse_str_into<T>(
    out: &mut Value,
    v: &Value,
    parse: fn(&str) -> Option<T>,
    wrap: fn(T) -> Value,
) -> u8 {
    let Value::Str(s) = v else {
        return 0;
    };
    let Some(val) = parse(s) else {
        return 0;
    };
    *out = wrap(val);
    1
}

fn parse_int32_str(s: &str) -> Option<i64> {
    native_abi::parse_long_str(s)
        .and_then(|n| i32::try_from(n).ok())
        .map(i64::from)
}

/// Fast C-ABI `Long/parseLong` over a `String` (`1` = parsed, `0` = fall back to `__static*`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_parse_long(out: &mut Value, v: &Value) -> u8 {
    parse_str_into(out, v, native_abi::parse_long_str, Value::Int)
}

/// Fast C-ABI `Integer/parseInt` over a `String` (`1` = parsed, `0` = fall back to `__static*`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_parse_int(out: &mut Value, v: &Value) -> u8 {
    parse_str_into(out, v, parse_int32_str, Value::Int)
}

/// Fast C-ABI `Double/parseDouble` over a `String` (`1` = parsed, `0` = fall back to `__static*`).
#[unsafe(no_mangle)]
pub extern "C" fn rt_n_parse_double(out: &mut Value, v: &Value) -> u8 {
    parse_str_into(out, v, native_abi::parse_double_str, Value::Double)
}

macro_rules! def_math_unary {
    ($($fn_name:ident => $method:ident),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $fn_name(x: f64) -> f64 {
                x.$method()
            }
        )*
    };
}

macro_rules! def_math_binary {
    ($($fn_name:ident => $method:ident),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $fn_name(x: f64, y: f64) -> f64 {
                x.$method(y)
            }
        )*
    };
}

def_math_unary! {
    rt_n_math_exp => exp,
    rt_n_math_log => ln,
    rt_n_math_log10 => log10,
    rt_n_math_ceil => ceil,
    rt_n_math_floor => floor,
    rt_n_math_sin => sin,
    rt_n_math_cos => cos,
    rt_n_math_tan => tan,
    rt_n_math_asin => asin,
    rt_n_math_acos => acos,
    rt_n_math_atan => atan,
    rt_n_math_sinh => sinh,
    rt_n_math_cosh => cosh,
    rt_n_math_tanh => tanh,
    rt_n_math_cbrt => cbrt,
}

def_math_binary! {
    rt_n_math_pow => powf,
    rt_n_math_atan2 => atan2,
    rt_n_math_hypot => hypot,
}

/// Every `rt_n_*` shim by symbol name (the in-process JIT maps a
/// native module's declarations to these).
#[must_use]
pub fn native_symbols() -> Vec<(&'static str, usize)> {
    macro_rules! syms {
        ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
    }
    syms![
        rt_n_declare_vars,
        rt_n_def_begin_var,
        rt_n_def_finish_var,
        rt_n_declare_fns,
        rt_n_set_const,
        rt_n_main,
        rt_n_main_lite,
        rt_n_str,
        rt_n_kw,
        rt_n_sym,
        rt_n_meta_sym,
        rt_n_coll,
        rt_n_consts_base,
        rt_n_enter,
        rt_n_iter_plan,
        rt_n_range_plan,
        rt_n_cap_var_meta,
        rt_n_vec_item,
        rt_n_vec_new,
        rt_n_vec_push,
        rt_n_vec_push_all,
        rt_n_vec_done,
        rt_n_unreduce,
        rt_n_mapv_parse_num,
        rt_n_dot,
        rt_n_print_join,
        rt_n_select_chars,
        rt_n_join_mapped_str,
        rt_n_add_acc,
        rt_n_regex,
        rt_n_static_field,
        rt_n_class,
        rt_n_read,
        rt_n_clone,
        rt_n_drop,
        rt_n_const,
        rt_n_deref,
        rt_n_fast_vars,
        rt_n_varref,
        rt_n_invoke,
        rt_n_enter_closure,
        rt_n_native_mv,
        rt_n_core_ref,
        rt_n_core_call,
        rt_n_op1,
        rt_n_op2,
        rt_n_op2_re,
        rt_n_scaled_long,
        rt_n_op3,
        rt_n_core_own,
        rt_n_assoc_own3,
        rt_n_assoc_nth_step,
        rt_n_enter_fn,
        rt_n_leave,
        rt_n_def_begin,
        rt_n_def_finish,
        rt_n_vec_lit,
        rt_n_map_lit,
        rt_n_kw_map,
        rt_n_set_lit,
        rt_n_throw,
        rt_n_new_throwable,
        rt_n_make_fn,
        rt_n_try,
        rt_n_binding,
        rt_n_set_var,
        rt_n_declare,
        rt_n_ns,
        rt_n_ns_lite,
        rt_n_cap_io,
        rt_n_cap_core,
        rt_n_cap_seq,
        rt_n_cap_re,
        rt_n_cap_host,
        rt_n_cap_string,
        rt_n_require_string_lite,
        rt_n_case,
        rt_n_equiv,
        rt_n_nth_nil,
        rt_n_nth2,
        rt_n_peek_nth,
        rt_n_gseq,
        rt_n_seq_nonempty,
        rt_n_first,
        rt_n_first_mapped,
        rt_n_into_mapcat,
        rt_n_sort_vec,
        rt_n_next_mut,
        rt_n_destructure,
        rt_n_destr_map,
        rt_n_destr_get,
        rt_n_lookup_call,
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
        rt_n_assoc_conj,
        rt_n_conj_bang_fx,
        rt_n_str_ascii,
        rt_n_parse_long,
        rt_n_parse_int,
        rt_n_parse_double,
        rt_n_math_exp,
        rt_n_math_log,
        rt_n_math_log10,
        rt_n_math_ceil,
        rt_n_math_floor,
        rt_n_math_sin,
        rt_n_math_cos,
        rt_n_math_tan,
        rt_n_math_asin,
        rt_n_math_acos,
        rt_n_math_atan,
        rt_n_math_sinh,
        rt_n_math_cosh,
        rt_n_math_tanh,
        rt_n_math_cbrt,
        rt_n_math_pow,
        rt_n_math_atan2,
        rt_n_math_hypot,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn argv_strings_skips_prog_and_null_entries() {
        let prog = CString::new("prog").unwrap();
        let a = CString::new("alpha").unwrap();
        let b = CString::new("beta").unwrap();
        let mut ptrs = [
            prog.as_ptr().cast_mut(),
            std::ptr::null_mut(),
            a.as_ptr().cast_mut(),
            b.as_ptr().cast_mut(),
        ];
        let slice = CSlice {
            ptr: ptrs.as_mut_ptr(),
            len: ptrs.len(),
        };
        assert_eq!(argv_strings(&slice), vec!["alpha", "beta"]);
    }
}
