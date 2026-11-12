// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Safe half of the AOT runtime: the process-global booted
//! evaluator, primitive dispatch through the interpreter's own
//! natives, and the shared error/print rendering.
//!
//! Zero semantic drift by construction: every helper calls THE
//! SAME [`NativeImpl`](clj_eval::NativeImpl) the interpreter
//! calls, resolved from a booted evaluator's `clojure.core`
//! (comparison chains, `quot` zero rules, `nth`/`get`/`conj`
//! edges, arity texts — all shared, never reimplemented).
//!
//! Concurrency: one [`Mutex`] guards the global evaluator (unit
//! tests share the process; built binaries are single-threaded).
//! A single lock with no nesting cannot deadlock, and the panic
//! catch sits INSIDE the lock scope so the guard always drops on
//! the normal path — a panicking native never poisons the lock.
//! (That single-lock argument is the whole model: there is no lock
//! ordering to check, so a loom model would be vacuous.)

use std::cell::Cell;
use std::ffi::CStr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use clj_eval::coll::{nth_str, str_utf16_len};
use clj_eval::core::nth_index;
use clj_eval::{Evaluator, NativeError, NativeImpl, as_native, boot};
use clj_value::{Keyword, List, ThrowClass, Value, Vector, print_to_string};

/// `clojure.core` names in [`Prim`](clj_eval::Prim) declaration order: the table
/// index IS the discriminant (`prim as usize`). The order test
/// below pins the pairing; each FFI differential test pins its
/// shim to its native end to end.
pub(crate) const PRIM_NAMES: [&str; 19] = [
    "+",
    "-",
    "*",
    "quot",
    "<",
    "<=",
    ">",
    ">=",
    "=",
    "count",
    "nth",
    "get",
    "conj",
    "long",
    "int",
    "transient",
    "conj!",
    "assoc!",
    "persistent!",
];

/// The global runtime: one booted evaluator plus the resolved
/// primitive implementations, indexed by [`Prim`](clj_eval::Prim) discriminant.
struct Runtime {
    eval: Evaluator,
    prims: [Arc<NativeImpl>; 19],
}

impl Runtime {
    /// Boot once: a stock evaluator, then each [`PRIM_NAMES`]
    /// entry resolved exactly like the interpreter resolves it
    /// (evaluating the qualified symbol and taking its native).
    fn boot() -> Self {
        let mut eval = boot();
        let prims = Self::resolve_all(&mut eval);
        Self { eval, prims }
    }

    /// Every [`PRIM_NAMES`] native, resolved through `eval`.
    fn resolve_all(eval: &mut Evaluator) -> [Arc<NativeImpl>; 19] {
        PRIM_NAMES.map(|name| Self::resolve(eval, name))
    }

    /// Resolve one `clojure.core` native through the booted
    /// evaluator (the `expect`s name boot bugs — every core prim
    /// resolves, pinned by the smoke test).
    fn resolve(eval: &mut Evaluator, name: &str) -> Arc<NativeImpl> {
        let probe = format!("clojure.core/{name}");
        let value = eval
            .eval_source(&probe)
            .expect("boot resolves every core prim")
            .into_iter()
            .next()
            .expect("a symbol evaluates to one value");
        Arc::clone(&as_native(&value).expect("core prims are natives").func)
    }
}

/// The process-global runtime, booted on first helper call — only
/// by pure-subset binaries: mixed programs run natives inside an
/// [`ActiveScope`] on the program's own evaluator (issue #258).
static RUNTIME: LazyLock<Mutex<Runtime>> = LazyLock::new(|| Mutex::new(Runtime::boot()));

/// The prim natives resolved from the first active evaluator (the
/// natives are the same `clojure.core` fns in every evaluator).
static SHARED_PRIMS: OnceLock<[Arc<NativeImpl>; 19]> = OnceLock::new();

thread_local! {
    /// The evaluator the current native call runs on (null outside
    /// any native call made from interpreted code).
    static ACTIVE: Cell<*mut Evaluator> = const { Cell::new(ptr::null_mut()) };
}

/// Publishes the interpreter's evaluator to the prim shims for the
/// extent of one native call, restoring the enclosing one after
/// (nested native calls through callbacks stack correctly).
pub(crate) struct ActiveScope {
    prev: *mut Evaluator,
}

impl ActiveScope {
    /// Enter: the caller must not touch `eval` until the scope drops.
    pub(crate) fn enter(eval: &mut Evaluator) -> Self {
        let ptr: *mut Evaluator = eval;
        Self {
            prev: ACTIVE.with(|a| a.replace(ptr)),
        }
    }
}

impl Drop for ActiveScope {
    fn drop(&mut self) {
        ACTIVE.with(|a| a.set(self.prev));
    }
}

/// Call primitive `index` (a [`Prim`](clj_eval::Prim) discriminant) on owned
/// `args`: the SAME native the interpreter would call, with the
/// SAME [`NativeError`] on failure.
pub(crate) fn call_prim(index: usize, args: Vec<Value>) -> Result<Value, NativeError> {
    let active = ACTIVE.with(Cell::get);
    if !active.is_null() {
        // SAFETY: `ACTIVE` is non-null only inside an `ActiveScope`,
        // which holds the `&mut Evaluator` the interpreter handed to
        // the native call (it is not used again until the scope
        // ends); this reborrow ends before the shim returns.
        let eval = unsafe { &mut *active };
        let func = Arc::clone(&SHARED_PRIMS.get_or_init(|| Runtime::resolve_all(eval))[index]);
        return catch_panic(AssertUnwindSafe(|| func(eval, &args)));
    }
    let mut rt = RUNTIME.lock().expect("clj-rt evaluator lock");
    let func = Arc::clone(&rt.prims[index]);
    catch_panic(AssertUnwindSafe(|| func(&mut rt.eval, &args)))
}

/// Run a native call, converting a panic (an internal bug — the
/// subset natives return [`NativeError`], never panic) into a
/// `java.lang.Error` so native unwinding never crosses the FFI
/// boundary; the shim prints it and returns null like any error.
/// (Two impossible-by-construction paths still could: a
/// `LazyLock`-init panic on boot failure, or a `prims[index]`
/// OOB panic on a bad codegen index. Both fail stop — boot is
/// smoke-pinned, indices are macro literals — so no silent
/// wrong-code hides behind them. R1 finding 9.)
pub(crate) fn catch_panic(
    call: AssertUnwindSafe<impl FnOnce() -> Result<Value, NativeError>>,
) -> Result<Value, NativeError> {
    match catch_unwind(call) {
        Ok(outcome) => outcome,
        // `as_ref` (NOT `&payload`): unsized coercion would wrap
        // the Box itself as the `Any`, hiding the real payload.
        Err(payload) => Err(NativeError::new(
            ThrowClass::Error,
            panic_text(payload.as_ref()),
        )),
    }
}

/// Render a caught panic payload (String, &str, or unknown).
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return format!("internal error: {text}");
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return format!("internal error: {text}");
    }
    "internal error: native panicked".to_owned()
}

/// Fast path: `count` of a string in UTF-16 units through the
/// shared scan cache — `None` unless `arg` is a string (the shim
/// delegates everything else to the shared native, so arity and
/// non-string errors stay shared, never reimplemented). Skips the
/// mutex, the `Arc` clone, and the evaluator roundtrip; the units
/// themselves are the shared [`str_utf16_len`].
pub(crate) fn count_str_fast(arg: &Value) -> Option<Value> {
    match arg {
        Value::Str(s) => Some(Value::Int(str_utf16_len(s) as i64)),
        _ => None,
    }
}

/// Fast path: `nth` over a string — `None` unless `coll` is a
/// string (the shim delegates). The index coerces FIRST through
/// the shared [`nth_index`], exactly like the shared native (the
/// nil-NPE quirk, tower truncation, and range errors all shared);
/// the lookup is the shared [`nth_str`] (scan cache, `not-found`,
/// and `StringIndexOutOfBounds` text all shared).
pub(crate) fn nth_str_fast(
    coll: &Value,
    idx: &Value,
    not_found: Option<&Value>,
) -> Option<Result<Value, NativeError>> {
    let Value::Str(s) = coll else {
        return None;
    };
    Some(nth_index(idx).and_then(|n| nth_str(s, n, not_found)))
}

/// Borrow-test a box for the unboxed-i64 path: `1` when the box
/// holds an `Int`, else `0` (the specialization guard — the
/// `rt_is_int` shim borrows, this decides, so the predicate pins
/// without crossing the FFI boundary).
pub(crate) fn is_int_flag(value: &Value) -> u8 {
    u8::from(matches!(value, Value::Int(_)))
}

/// Read an `Int` box as a raw `i64` (the unboxed-i64 entry — the
/// `rt_unbox_int` shim borrows, this reads; non-`Int` degrades to
/// `0`, the `rt_vec_push` precedent — guards make it unreachable).
pub(crate) fn unbox_int(value: &Value) -> i64 {
    match value {
        Value::Int(n) => *n,
        _ => 0,
    }
}

/// The helper error line: `path:line:col: Class: message` (spans
/// baked at the call site; the class/message come straight from
/// the shared [`NativeError`]).
pub(crate) fn error_line(path: &str, line: u32, col: u32, err: &NativeError) -> String {
    format!(
        "{path}:{line}:{col}: {}: {}",
        err.class.jvm_name(),
        err.message
    )
}

/// The `print` intrinsic's output: space-joined
/// human-readable, unquoted strings, NO trailing newline.
pub(crate) fn print_join(args: &[Value]) -> String {
    args.iter()
        .map(print_to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Box a fresh owned value for the caller.
pub(crate) fn give(value: Value) -> *mut Value {
    Box::into_raw(Box::new(value))
}

/// The fallible-helper failure: print `path:line:col: Class:
/// message` and return null (the generated code null-checks and
/// exits 1).
pub(crate) fn fail(path: &str, line: u32, col: u32, err: &NativeError) -> *mut Value {
    // Inside a native call from embedded code (issue #256), the
    // wrapper rethrows instead: the interpreter decides whether
    // it is caught or printed.
    if !crate::mixed::stash(err) {
        eprintln!("{}", error_line(path, line, col, err));
    }
    ptr::null_mut()
}

/// The shared primitive body over owned values: call the shared
/// native, box the result — or fail (the shims take the boxes,
/// this decides; every `def_prim` shim funnels through here).
pub(crate) fn prim_worker(
    index: usize,
    owned: Vec<Value>,
    path: &str,
    line: u32,
    col: u32,
) -> *mut Value {
    match call_prim(index, owned) {
        Ok(value) => give(value),
        Err(err) => fail(path, line, col, &err),
    }
}

/// `count` over owned values: a single string arg counts
/// natively, every other shape delegates to the shared native
/// (the shim takes the boxes and borrows the path; the owned
/// boxes drop on the fast arm after the result computes).
pub(crate) fn count_worker(owned: Vec<Value>, path: &str, line: u32, col: u32) -> *mut Value {
    let fast = match owned.as_slice() {
        [arg] => count_str_fast(arg),
        _ => None,
    };
    match fast {
        Some(value) => give(value),
        None => prim_worker(9, owned, path, line, col),
    }
}

/// `nth` over owned values: a string collection reads natively,
/// every other shape delegates (same contract as
/// [`count_worker`]; the `not-found` borrow rides the slice
/// pattern, so no arity recheck).
pub(crate) fn nth_worker(owned: Vec<Value>, path: &str, line: u32, col: u32) -> *mut Value {
    let fast = match owned.as_slice() {
        [coll, idx] => nth_str_fast(coll, idx, None),
        [coll, idx, nf] => nth_str_fast(coll, idx, Some(nf)),
        _ => None,
    };
    match fast {
        Some(Ok(value)) => give(value),
        Some(Err(err)) => fail(path, line, col, &err),
        None => prim_worker(10, owned, path, line, col),
    }
}

/// The `*command-line-args*` value over borrowed C strings:
/// `argv[0]` already skipped by the shim; no entries is nil,
/// else a list of lossy-decoded strings (the JVM shape).
pub(crate) fn argv_worker(entries: &[&CStr]) -> *mut Value {
    let mut items = Vec::new();
    for entry in entries {
        let text = entry.to_string_lossy().into_owned();
        items.push(Value::Str(Arc::from(text.as_str())));
    }
    if items.is_empty() {
        give(Value::Nil)
    } else {
        give(Value::List(List::from_vec(items)))
    }
}

/// Push `elem` onto `old`: a vector grows in place, anything
/// else degrades to a fresh single-element vector (a non-vector
/// `vec` is a caller bug — the degrade keeps the line
/// meaningful).
pub(crate) fn vec_push_worker(old: Value, elem: Value) -> Value {
    match old {
        Value::Vector(items) => Value::Vector(items.push_back(elem)),
        _ => Value::Vector(Vector::from_vec(vec![elem])),
    }
}

/// One keyword box over decoded parts (`None` is unqualified).
pub(crate) fn keyword_worker(name: &str, ns: Option<&str>) -> *mut Value {
    give(Value::Keyword(Keyword::new(ns, name)))
}

/// One string box over a decoded byte string.
pub(crate) fn str_worker(text: &str) -> *mut Value {
    give(Value::Str(Arc::from(text)))
}

/// The `print` intrinsic over owned values: space-joined
/// rendering with NO trailing newline, stdout flushed, nil.
pub(crate) fn print_worker(owned: &[Value]) -> *mut Value {
    print!("{}", print_join(owned));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    give(Value::Nil)
}

/// Clone a borrowed box into a fresh owned box.
pub(crate) fn clone_worker(value: &Value) -> *mut Value {
    give((*value).clone())
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE, ActiveScope, PRIM_NAMES, RUNTIME, argv_worker, call_prim, catch_panic,
        clone_worker, count_str_fast, count_worker, error_line, is_int_flag, keyword_worker,
        nth_str_fast, nth_worker, prim_worker, print_join, print_worker, str_worker, unbox_int,
        vec_push_worker,
    };
    use clj_eval::{NativeError, Prim};
    use clj_value::{ThrowClass, Value, print_to_string};
    use std::panic::AssertUnwindSafe;

    #[test]
    fn active_scope_runs_prims_on_the_callers_evaluator_258() {
        let mut outer = clj_eval::boot();
        let mut inner = clj_eval::boot();
        let outer_ptr: *mut clj_eval::Evaluator = &mut outer;
        {
            let _a = ActiveScope::enter(&mut outer);
            let sum = call_prim(Prim::Add as usize, vec![Value::Int(1), Value::Int(2)]);
            assert_eq!(sum.ok(), Some(Value::Int(3)));
            {
                let _b = ActiveScope::enter(&mut inner);
                let cmp = call_prim(Prim::Lt as usize, vec![Value::Int(1), Value::Int(2)]);
                assert_eq!(cmp.ok(), Some(Value::Bool(true)));
            }
            assert_eq!(ACTIVE.with(std::cell::Cell::get), outer_ptr);
        }
        assert!(ACTIVE.with(std::cell::Cell::get).is_null());
    }

    /// Take a worker return back (frees the box).
    ///
    /// # Safety
    ///
    /// `ptr` must be a non-null box from a worker return.
    unsafe fn unbox(ptr: *mut Value) -> Value {
        unsafe { *Box::from_raw(ptr) }
    }

    /// Render a worker return to its `print` string (frees the box).
    fn render(ptr: *mut Value) -> String {
        let value = unsafe { unbox(ptr) };
        print_to_string(&value)
    }

    #[test]
    fn prim_names_follow_prim_order() {
        let pairs = [
            (Prim::Add, "+"),
            (Prim::Sub, "-"),
            (Prim::Mul, "*"),
            (Prim::Quot, "quot"),
            (Prim::Lt, "<"),
            (Prim::Le, "<="),
            (Prim::Gt, ">"),
            (Prim::Ge, ">="),
            (Prim::Eq, "="),
            (Prim::Count, "count"),
            (Prim::Nth, "nth"),
            (Prim::Get, "get"),
            (Prim::Conj, "conj"),
            (Prim::Long, "long"),
            (Prim::Int, "int"),
            (Prim::Transient, "transient"),
            (Prim::ConjBang, "conj!"),
            (Prim::AssocBang, "assoc!"),
            (Prim::PersistentBang, "persistent!"),
        ];
        for (prim, name) in pairs {
            assert_eq!(
                PRIM_NAMES.iter().position(|n| *n == name),
                Some(prim as usize),
                "{name}"
            );
            assert_eq!(PRIM_NAMES[prim as usize], name);
        }
    }

    #[test]
    fn boot_resolves_every_prim() {
        let rt = RUNTIME.lock().expect("test lock");
        assert_eq!(rt.prims.len(), PRIM_NAMES.len());
    }

    #[test]
    fn call_prim_reaches_the_shared_native() {
        let out = call_prim(Prim::Add as usize, vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(format!("{out:?}"), "Ok(Int(3))");
        let out = call_prim(Prim::Sub as usize, Vec::new());
        assert!(out.is_err());
    }

    #[test]
    fn error_line_pins() {
        assert_eq!(
            error_line("f.clj", 1, 2, &NativeError::arity("user/f", 0),),
            "f.clj:1:2: clojure.lang.ArityException: Wrong number of args (0) passed to: user/f"
        );
        assert_eq!(
            error_line(
                "g.clj",
                12,
                30,
                &NativeError::new(ThrowClass::IndexOutOfBoundsException, String::new()),
            ),
            "g.clj:12:30: java.lang.IndexOutOfBoundsException: "
        );
        assert_eq!(
            error_line(
                "h.clj",
                7,
                1,
                &NativeError::new(ThrowClass::Error, "internal error: boom".to_owned()),
            ),
            "h.clj:7:1: java.lang.Error: internal error: boom"
        );
    }

    #[test]
    fn catch_panic_passes_success_through() {
        let out = catch_panic(AssertUnwindSafe(|| Ok(Value::Int(7))));
        assert_eq!(format!("{out:?}"), "Ok(Int(7))");
        let out: Result<Value, NativeError> =
            catch_panic(AssertUnwindSafe(|| Err(NativeError::arity("user/f", 2))));
        assert!(out.is_err());
    }

    #[test]
    fn catch_panic_renders_every_payload() {
        let out = catch_panic(AssertUnwindSafe(|| -> Result<Value, NativeError> {
            std::panic::panic_any("boom-12".to_owned());
        }));
        assert!(
            matches!(&out, Err(e) if e.message == "internal error: boom-12" && e.class == ThrowClass::Error)
        );
        let out = catch_panic(AssertUnwindSafe(|| -> Result<Value, NativeError> {
            std::panic::panic_any("static");
        }));
        assert!(matches!(&out, Err(e) if e.message == "internal error: static"));
        // Real `panic!`s land in `internal error` text too (the
        // exact payload type is toolchain-version-dependent, so
        // these pin the prefix; `panic_any` above pins each arm
        // exactly, version-proof).
        let out = catch_panic(AssertUnwindSafe(|| -> Result<Value, NativeError> {
            panic!("boom");
        }));
        assert!(matches!(&out, Err(e) if e.message.starts_with("internal error: ")));
        let out = catch_panic(AssertUnwindSafe(|| -> Result<Value, NativeError> {
            panic!("boom-{}", 12);
        }));
        assert!(matches!(&out, Err(e) if e.message.starts_with("internal error: ")));
        let out = catch_panic(AssertUnwindSafe(|| -> Result<Value, NativeError> {
            std::panic::panic_any(7_i32);
        }));
        assert!(matches!(&out, Err(e) if e.message == "internal error: native panicked"));
    }

    #[test]
    fn count_str_fast_pins_utf16_units() {
        // Astral chars count 2 (surrogate pair); BMP 1.
        assert_eq!(
            format!(
                "{:?}",
                count_str_fast(&Value::Str("a𝄞é".into())).expect("strings hit")
            ),
            "Int(4)"
        );
        assert_eq!(
            format!(
                "{:?}",
                count_str_fast(&Value::Str("".into())).expect("empty hits")
            ),
            "Int(0)"
        );
        assert!(count_str_fast(&Value::Int(1)).is_none());
        assert!(count_str_fast(&Value::Nil).is_none());
    }

    #[test]
    fn nth_str_fast_pins_chars_and_shared_errors() {
        let at = |s: &str, i: Value, nf: Option<&Value>| {
            nth_str_fast(&Value::Str(s.into()), &i, nf).expect("strings hit")
        };
        assert_eq!(
            format!("{:?}", at("hello", Value::Int(1), None)),
            "Ok(Char('e'))"
        );
        // The index coerces first (double truncation, like the
        // shared native); `not-found` answers OOB and negatives.
        assert_eq!(
            format!("{:?}", at("hello", Value::Double(1.9), None)),
            "Ok(Char('e'))"
        );
        let dflt = Value::Int(7);
        assert_eq!(
            format!("{:?}", at("hello", Value::Int(9), Some(&dflt))),
            "Ok(Int(7))"
        );
        assert_eq!(
            format!("{:?}", at("hello", Value::Int(-1), Some(&dflt))),
            "Ok(Int(7))"
        );
        // Without `not-found` the shared `StringIndexOutOfBounds`
        // text survives; a nil index trips the shared NPE quirk.
        let err = at("hello", Value::Int(9), None).expect_err("OOB fails");
        assert_eq!(
            error_line("f.clj", 3, 9, &err),
            "f.clj:3:9: java.lang.StringIndexOutOfBoundsException: Index 9 out of bounds for length 5"
        );
        let err = at("hello", Value::Nil, None).expect_err("nil index fails");
        assert!(matches!(err.class, ThrowClass::NullPointerException));
        assert!(nth_str_fast(&Value::Int(1), &Value::Int(0), None).is_none());
    }

    #[test]
    fn is_int_flag_pins_ints() {
        assert_eq!(is_int_flag(&Value::Int(0)), 1);
        assert_eq!(is_int_flag(&Value::Int(i64::MAX)), 1);
        assert_eq!(is_int_flag(&Value::Int(i64::MIN)), 1);
        assert_eq!(is_int_flag(&Value::Str("1".into())), 0);
        assert_eq!(is_int_flag(&Value::Double(1.0)), 0);
        assert_eq!(is_int_flag(&Value::Bool(true)), 0);
        assert_eq!(is_int_flag(&Value::Nil), 0);
    }

    #[test]
    fn unbox_int_roundtrips_and_degrades() {
        for n in [0, 1, -1, 42, i64::MAX, i64::MIN] {
            assert_eq!(unbox_int(&Value::Int(n)), n);
        }
        assert_eq!(unbox_int(&Value::Str("x".into())), 0);
        assert_eq!(unbox_int(&Value::Bool(true)), 0);
        assert_eq!(unbox_int(&Value::Nil), 0);
    }

    #[test]
    fn prim_worker_dispatches_and_fails_null() {
        let out = prim_worker(
            Prim::Add as usize,
            vec![Value::Int(1), Value::Int(2)],
            "f.clj",
            3,
            9,
        );
        assert_eq!(render(out), "3");
        let out = prim_worker(Prim::Sub as usize, Vec::new(), "f.clj", 3, 9);
        assert!(out.is_null());
    }

    #[test]
    fn count_worker_fast_and_slow() {
        let out = count_worker(vec![Value::Str("héllo".into())], "f.clj", 3, 9);
        assert_eq!(render(out), "5");
        let out = count_worker(vec![Value::Int(1), Value::Int(2)], "f.clj", 3, 9);
        assert!(out.is_null());
    }

    #[test]
    fn nth_worker_fast_slow_and_fail() {
        let out = nth_worker(
            vec![Value::Str("hello".into()), Value::Int(1)],
            "f.clj",
            3,
            9,
        );
        assert_eq!(render(out), "e");
        let out = nth_worker(
            vec![Value::Str("hello".into()), Value::Int(9)],
            "f.clj",
            3,
            9,
        );
        assert!(out.is_null());
        let out = nth_worker(vec![Value::Int(1)], "f.clj", 3, 9);
        assert!(out.is_null());
    }

    #[test]
    fn argv_worker_shapes() {
        // Post-skip entries (the shim drops argv[0]); empty is nil.
        let a = c"a";
        let b = c"b";
        let bad = c"a\xffb";
        assert_eq!(render(argv_worker(&[])), "nil");
        assert_eq!(render(argv_worker(&[a, b])), "(a b)");
        assert_eq!(render(argv_worker(&[bad])), "(a�b)");
    }

    #[test]
    fn vec_push_worker_pushes_and_degrades() {
        use clj_value::Vector;
        let out = vec_push_worker(
            Value::Vector(Vector::from_vec(vec![Value::Int(1)])),
            Value::Int(2),
        );
        assert_eq!(print_to_string(&out), "[1 2]");
        let out = vec_push_worker(Value::Int(9), Value::Int(1));
        assert_eq!(print_to_string(&out), "[1]");
    }

    #[test]
    fn keyword_worker_plain_and_namespaced() {
        assert_eq!(render(keyword_worker("miss", None)), ":miss");
        assert_eq!(render(keyword_worker("id", Some("user"))), ":user/id");
    }

    #[test]
    fn str_worker_boxes() {
        assert_eq!(render(str_worker("héllo")), "héllo");
        assert_eq!(render(str_worker("")), "");
    }

    #[test]
    fn clone_worker_copies() {
        let out = clone_worker(&Value::Str("v".into()));
        assert_eq!(render(out), "v");
    }

    #[test]
    fn print_worker_renders_and_nils() {
        // stdout goes to the test capture; the return is nil.
        let out = print_worker(&[Value::Int(1), Value::Str("a b".into()), Value::Bool(true)]);
        assert_eq!(render(out), "nil");
    }

    #[test]
    fn print_join_pins() {
        assert_eq!(print_join(&[]), "");
        assert_eq!(print_join(&[Value::Int(1)]), "1");
        assert_eq!(
            print_join(&[Value::Int(1), Value::Str("a b".into()), Value::Bool(true)]),
            "1 a b true"
        );
    }
}
