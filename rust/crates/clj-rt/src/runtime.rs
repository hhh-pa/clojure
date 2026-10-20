// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

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

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, LazyLock, Mutex};

use clj_eval::{Evaluator, NativeError, NativeImpl, as_native, boot};
use clj_value::{ThrowClass, Value, print_to_string};

/// `clojure.core` names in [`Prim`](clj_eval::Prim) declaration order: the table
/// index IS the discriminant (`prim as usize`). The order test
/// below pins the pairing; each FFI differential test pins its
/// shim to its native end to end.
pub(crate) const PRIM_NAMES: [&str; 13] = [
    "+", "-", "*", "quot", "<", "<=", ">", ">=", "=", "count", "nth", "get", "conj",
];

/// The global runtime: one booted evaluator plus the resolved
/// primitive implementations, indexed by [`Prim`](clj_eval::Prim) discriminant.
struct Runtime {
    eval: Evaluator,
    prims: [Arc<NativeImpl>; 13],
}

impl Runtime {
    /// Boot once: a stock evaluator, then each [`PRIM_NAMES`]
    /// entry resolved exactly like the interpreter resolves it
    /// (evaluating the qualified symbol and taking its native).
    fn boot() -> Self {
        let mut eval = boot();
        let prims = PRIM_NAMES.map(|name| Self::resolve(&mut eval, name));
        Self { eval, prims }
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

/// The process-global runtime, booted on first helper call.
static RUNTIME: LazyLock<Mutex<Runtime>> = LazyLock::new(|| Mutex::new(Runtime::boot()));

/// Call primitive `index` (a [`Prim`](clj_eval::Prim) discriminant) on owned
/// `args`: the SAME native the interpreter would call, with the
/// SAME [`NativeError`] on failure.
pub(crate) fn call_prim(index: usize, args: Vec<Value>) -> Result<Value, NativeError> {
    let mut rt = RUNTIME.lock().expect("clj-rt evaluator lock");
    let func = Arc::clone(&rt.prims[index]);
    catch_panic(AssertUnwindSafe(|| func(&mut rt.eval, &args)))
}

/// Run a native call, converting a panic (an internal bug — the
/// subset natives return [`NativeError`], never panic) into a
/// `java.lang.Error` so no unwinding ever crosses the FFI
/// boundary; the shim prints it and returns null like any error.
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

#[cfg(test)]
mod tests {
    use super::{PRIM_NAMES, RUNTIME, call_prim, catch_panic, error_line, print_join};
    use clj_eval::{NativeError, Prim};
    use clj_value::{ThrowClass, Value};
    use std::panic::AssertUnwindSafe;

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
    fn print_join_pins() {
        assert_eq!(print_join(&[]), "");
        assert_eq!(print_join(&[Value::Int(1)]), "1");
        assert_eq!(
            print_join(&[Value::Int(1), Value::Str("a b".into()), Value::Bool(true)]),
            "1 a b true"
        );
    }
}
