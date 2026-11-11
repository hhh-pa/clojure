// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Mixed AOT programs (issue #256): the binary registers its
//! native `defn`s, then loads the embedded source through `run`'s
//! own frame ([`clj_eval::script`]). The masked source calls
//! `(clj.aot/n K)` where native `K`'s `defn` stood, which interns
//! it at exactly that load position, under its name, in the
//! current namespace.
//!
//! The program runs on its OWN evaluator, never the global
//! [`RUNTIME`](crate::runtime) one the primitive shims lock. A
//! native called from embedded code therefore re-enters the shims
//! without deadlock (spec D6). While a native runs, shim failures
//! are stashed instead of printed, and the wrapper rethrows them
//! as the [`NativeError`] the interpreter would have raised. That
//! keeps `try`/`catch` around native calls and the final `run`
//! diagnostic identical.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use clj_eval::script::{run_diagnostic, run_script};
use clj_eval::{Evaluator, MIXED_INTERN_NS, NativeError, boot, load_text_as_file};
use clj_value::{ThrowClass, Value};

use crate::runtime::give;

/// A native `defn`'s uniform entry: the generated trampoline loads
/// `argc` owned boxes from `args`, calls the lowered fn (which
/// consumes them), and returns its owned result, or null after a
/// stashed failure.
pub type Trampoline = extern "C" fn(args: *mut *mut Value, argc: usize) -> *mut Value;

/// One registered native.
#[derive(Clone)]
struct Native {
    name: String,
    arity: usize,
    entry: Trampoline,
}

/// Natives by index, registered by the generated `main` before
/// the program loads.
static NATIVES: Mutex<Vec<Option<Native>>> = Mutex::new(Vec::new());

thread_local! {
    /// Nesting depth of native calls made from embedded code (> 0:
    /// shim failures stash instead of print).
    static STASH_DEPTH: Cell<usize> = const { Cell::new(0) };
    /// The failure a shim stashed during the current native call.
    static PENDING: RefCell<Option<NativeError>> = const { RefCell::new(None) };
}

/// Record native `index` (re-registration replaces).
pub(crate) fn register(index: usize, name: String, arity: usize, entry: Trampoline) {
    let mut natives = NATIVES.lock().expect("clj-rt natives lock");
    if natives.len() <= index {
        natives.resize(index + 1, None);
    }
    natives[index] = Some(Native { name, arity, entry });
}

/// Stash `err` when a native call is in flight (returns `true`),
/// else leave it to the caller to print.
pub(crate) fn stash(err: &NativeError) -> bool {
    if STASH_DEPTH.with(Cell::get) == 0 {
        return false;
    }
    PENDING.with(|p| *p.borrow_mut() = Some(err.clone()));
    true
}

/// Increments [`STASH_DEPTH`] for its lifetime (panic-safe).
struct StashScope;

impl StashScope {
    fn enter() -> Self {
        STASH_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}

impl Drop for StashScope {
    fn drop(&mut self) {
        STASH_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

/// Call a native with interpreter arguments: arity-checked with
/// the interpreter's own text, boxes handed over, failures
/// rethrown from the stash.
fn invoke(native: &Native, display: &str, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() != native.arity {
        return Err(NativeError::arity(display, args.len()));
    }
    let mut boxes: Vec<*mut Value> = args.iter().cloned().map(give).collect();
    let ret = {
        let _scope = StashScope::enter();
        (native.entry)(boxes.as_mut_ptr(), boxes.len())
    };
    if ret.is_null() {
        let pending = PENDING.with(|p| p.borrow_mut().take());
        return Err(pending.unwrap_or_else(|| {
            NativeError::new(
                ThrowClass::Error,
                "internal error: native failed silently".to_owned(),
            )
        }));
    }
    // SAFETY: a non-null trampoline return is a fresh owned box
    // (the S8 D7 ownership contract), reclaimed exactly once here.
    Ok(*unsafe { Box::from_raw(ret) })
}

/// `(clj.aot/n K)`: intern native `K` into the current namespace
/// (the masked `defn`'s position) and return its fn value.
fn intern_native(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let index = match args {
        [Value::Int(k)] => usize::try_from(*k).ok(),
        _ => None,
    };
    let native = index.and_then(|k| {
        let natives = NATIVES.lock().expect("clj-rt natives lock");
        natives.get(k).cloned().flatten()
    });
    let Some(native) = native else {
        return Err(NativeError::new(
            ThrowClass::Error,
            "internal: bad AOT native index".to_owned(),
        ));
    };
    let ns = eval.current_ns_name();
    let display = format!("{ns}/{}", native.name);
    let name = native.name.clone();
    let func = Arc::new(move |_: &mut Evaluator, args: &[Value]| invoke(&native, &display, args));
    Ok(eval.install_native(&ns, &name, func))
}

/// Run the embedded program as `run` would run `path`: same frame,
/// same diagnostics, same exit codes (0, or 1 after printing the
/// failure).
pub(crate) fn run_program(path: &str, source: &str, args: &[String]) -> i32 {
    let mut eval = boot();
    let program: Arc<str> = Arc::from(source);
    let file: Arc<str> = Arc::from(path);
    eval.install_native(MIXED_INTERN_NS, "n", Arc::new(intern_native));
    eval.install_native(
        MIXED_INTERN_NS,
        "load",
        Arc::new(move |eval: &mut Evaluator, _: &[Value]| load_text_as_file(eval, &file, &program)),
    );
    let script = run_script(args, &format!("({MIXED_INTERN_NS}/load)"));
    let code = match eval.eval_source(&script) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{}", run_diagnostic(path, &e));
            1
        }
    };
    // `run`'s exit: drained agents and futures print first, and
    // the runtime is not torn down var by var (#226).
    eval.join_drainers();
    std::mem::forget(eval);
    // The binary's C `main` returns straight to libc `exit`, which
    // never runs std's at-exit flush: a trailing `print` without a
    // newline would stay in the line buffer.
    let _flushed = std::io::Write::flush(&mut std::io::stdout());
    code
}

#[cfg(test)]
mod tests {
    use super::{Trampoline, register, run_program, stash};
    use clj_eval::NativeError;
    use clj_value::{ThrowClass, Value};

    use crate::runtime::{fail, give};

    /// `(fn [x] (+ x 1))` over boxes, failing on non-ints the way a
    /// prim shim does (stash through [`fail`]).
    extern "C" fn inc_tramp(args: *mut *mut Value, argc: usize) -> *mut Value {
        assert_eq!(argc, 1);
        // SAFETY: test trampoline under the Trampoline contract.
        let arg = *unsafe { Box::from_raw(*args) };
        match arg {
            Value::Int(n) => give(Value::Int(n + 1)),
            other => fail(
                "t.clj",
                1,
                1,
                &NativeError::new(
                    ThrowClass::ClassCastException,
                    format!("bad {}", clj_value::print_to_string(&other)),
                ),
            ),
        }
    }

    /// A broken trampoline: null without stashing anything.
    extern "C" fn silent_tramp(_: *mut *mut Value, _: usize) -> *mut Value {
        std::ptr::null_mut()
    }

    fn program(index: usize, entry: Trampoline, src: &str) -> i32 {
        register(index, format!("f{index}"), 1, entry);
        run_program("t.clj", src, &[])
    }

    #[test]
    fn interned_native_runs_and_rethrows() {
        // Index 10+ keep the parallel tests' registrations apart.
        let src = "(clj.aot/n 10)\n\
                   (assert (= 3 (f10 2)))\n\
                   (assert (= \"bad :k\" (try (f10 :k) (catch ClassCastException e (.getMessage e)))))\n\
                   (assert (= \"Wrong number of args (2) passed to: user/f10\" \
                     (try (f10 1 2) (catch clojure.lang.ArityException e (.getMessage e)))))\n";
        assert_eq!(program(10, inc_tramp, src), 0);
    }

    #[test]
    fn failures_render_like_run() {
        assert_eq!(program(11, inc_tramp, "(clj.aot/n 11)\n(f11 :k)\n"), 1);
        assert_eq!(program(12, silent_tramp, "(clj.aot/n 12)\n(f12 1)\n"), 1);
        assert_eq!(run_program("t.clj", "(clj.aot/n 99)\n", &[]), 1);
        assert_eq!(run_program("t.clj", "(clj.aot/n :x)\n", &[]), 1);
    }

    #[test]
    fn frame_matches_run() {
        let src = "(assert (= *command-line-args* '(\"a\")))\n\
                   (assert (= \"x\" (clojure.string/trim \" x \")))\n\
                   (assert (.endsWith *file* \"t.clj\"))\n";
        assert_eq!(run_program("t.clj", src, &["a".to_owned()]), 0);
    }

    #[test]
    fn stash_only_inside_native_calls() {
        let err = NativeError::new(ThrowClass::Error, "x".to_owned());
        assert!(!stash(&err));
    }
}
