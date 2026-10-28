// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S18 compiler surface (normative:
//! `docs/zero-oos-s18-compiler/specification.md`).
//!
//! This module owns the knob roots, `add-classpath`, `native_compile`,
//! the leading-`ns` strip, the §1.4 object path, and the injected AOT
//! driver hook (`DriveError` + `CompileDriver` + the guarded
//! registry). `clj-eval` cannot depend on `clj-aot` (which plans
//! through `plan_source`), so the real driver installs here from the
//! linking binary; the unset hook reports the CLI's exact unlinked
//! line (T11).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use clj_reader::{NoHooks, Reader, ReaderOpts, Span};
use clj_value::{ThrowClass, Value, Vector, new_map, seq_vec};

use crate::aot_subset::{PlanError, SubsetProgram, plan_source};
use crate::error::{CompileError, NativeError, span_of};
use crate::eval::{Evaluator, SourceError};
use crate::io::write_out;
use crate::ns::{
    cast_err, check_arity, core_var, core_var_handle, deref_core, load_one, load_paths, name_of,
    root_resource, signal_to_native, source_to_native, strict_sym,
};

/// The six slot-only knob roots (JVM `Var.intern(...).setDynamic()`:
/// dynamic slot, no `:dynamic` meta — `boot` installs these;
/// `*verbose-defrecords*` rides `core_vars` instead, like the other
/// `^:dynamic` vars).
pub(crate) fn knob_roots() -> [(&'static str, Value); 6] {
    [
        ("*compile-path*", Value::Str(Arc::from("classes"))),
        ("*compile-files*", Value::Bool(false)),
        ("*compiler-options*", Value::Nil),
        ("*source-path*", Value::Str(Arc::from("NO_SOURCE_FILE"))),
        ("*fn-loader*", Value::Nil),
        ("*use-context-classloader*", Value::Bool(true)),
    ]
}

/// `primitives-classnames`: the 8 spec pairs over class-name-string
/// keys (JVM keys are `Class` objects — N1-unrepresentable — so the
/// 8 `.getName` strings stand in, per the #33 convention).
pub(crate) fn prims_map() -> Value {
    let mut map = new_map();
    for (key, val) in [
        ("boolean", "Boolean/TYPE"),
        ("byte", "Byte/TYPE"),
        ("char", "Character/TYPE"),
        ("double", "Double/TYPE"),
        ("float", "Float/TYPE"),
        ("int", "Integer/TYPE"),
        ("long", "Long/TYPE"),
        ("short", "Short/TYPE"),
    ] {
        map = map.insert(Value::Str(Arc::from(key)), Value::Str(Arc::from(val)));
    }
    Value::Map(map)
}

/// `(add-classpath dir)`: the historical append contract over
/// `*load-path*` — arity (T1, no WARNING), then the verbatim WARNING
/// (printed even when the add then fails), then the String check
/// (CCE `String` — the port takes path strings only), then a
/// value-membership guarded `conj` onto the root (nil normalizes to
/// `[]` first, preserving the append invariant; non-vector colls get
/// `conj` semantics). Answers nil.
pub(crate) fn native_add_classpath(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity("clojure.core/add-classpath", args, &[1])?;
    write_out(eval, "WARNING: add-classpath is deprecated\n")?;
    let Value::Str(dir) = &args[0] else {
        return Err(cast_err(&args[0], "java.lang.String"));
    };
    let dir_val = Value::Str(dir.clone());
    let id = core_var(eval, "*load-path*").expect("boot installs *load-path*");
    let root = eval
        .registry
        .state(id)
        .and_then(|s| s.root)
        .unwrap_or(Value::Nil);
    let coll = match root {
        Value::Nil => Value::Vector(Vector::from_vec(Vec::new())),
        other => other,
    };
    let items = seq_vec(&coll).ok_or_else(|| cast_err(&coll, "clojure.lang.IPersistentVector"))?;
    if items.iter().any(|entry| entry == &dir_val) {
        return Ok(Value::Nil);
    }
    let conj = deref_core(eval, "conj");
    let next = eval
        .call(&conj, &[coll, dir_val])
        .map_err(signal_to_native)?;
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(next);
    });
    Ok(Value::Nil)
}

/// The CLI's exact unlinked-backend line (T11): the single source
/// shared by the unset hook and the binary's unlinked `build` arm,
/// so the two can never drift.
pub const UNLINKED_BUILD_LINE: &str = "build: LLVM backend not linked (built with --no-default-features; rebuild with default features to compile)";

/// Emission failures (LLVM-free): exactly `clj-aot`'s `CompileError`
/// minus `Plan` (planning stays caller-side, via `plan_source`)
/// minus `Link` (CLI-only — linking a library is meaningless).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveError {
    /// The host target machine failed (broken LLVM install).
    Target(String),
    /// Object emission failed (broken LLVM install).
    Emit(String),
    /// A file write failed.
    Io {
        /// The file that failed.
        path: PathBuf,
        /// The OS error text.
        message: String,
    },
}

/// Object emission across the crate cycle (`clj-aot` implements the
/// real driver; tests install fakes). The driver owns the write
/// (bytes + path); the caller owns nothing about directories (no
/// `mkdirs` — missing parents fail as [`DriveError::Io`]).
pub trait CompileDriver: Send + Sync {
    /// Emit `program` (planned from `source_path`'s text) as the
    /// object file at `obj_path`, creating no directories.
    fn emit(
        &self,
        program: &SubsetProgram,
        source_path: &str,
        obj_path: &Path,
    ) -> Result<(), DriveError>;
}

/// The installed driver (unset until the linking binary installs
/// the real one at startup).
static DRIVER: RwLock<Option<&'static dyn CompileDriver>> = RwLock::new(None);

/// Install the emission driver (`None` restores the unset hook —
/// the T11 test uses this to pin the unset shape end to end;
/// production installs once at startup).
pub fn install_driver(driver: Option<&'static dyn CompileDriver>) {
    *DRIVER.write().unwrap_or_else(|e| e.into_inner()) = driver;
}

/// The installed driver, if any (each flag-true root reads once).
fn current_driver() -> Option<&'static dyn CompileDriver> {
    *DRIVER.read().unwrap_or_else(|e| e.into_inner())
}

/// `(compile lib)`: `(binding [*compile-files* true] (load-one ...))`
/// answering `lib` — arity (T1), `name` (T2/T3), the unconditional
/// `true` binding (an outer `false` does not skip), the `load_one`
/// path (resolve, check, mark), and the original arg as the answer.
pub(crate) fn native_compile(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/compile", args, &[1])?;
    let name = name_of(&args[0])?;
    let id = core_var(eval, "*compile-files*").expect("boot installs *compile-files*");
    let depth = eval.push_binding(core_var_handle(id, "*compile-files*"), Value::Bool(true));
    let out = compile_load(eval, &args[0], &name);
    eval.pop_bindings(depth);
    out?;
    Ok(args[0].clone())
}

/// The compile load phase over the `load_one` path: symbol args take
/// `load_one` (load, check, mark); strings and keywords load by name
/// then fail the need-ns check over the ORIGINAL arg (T5 — the
/// check runs after emission, the mark after the check, so the mark
/// is unreachable here and `load_paths` loads without marking).
fn compile_load(eval: &mut Evaluator, arg: &Value, name: &str) -> Result<(), NativeError> {
    match arg {
        Value::Symbol(sym) => load_one(eval, sym, true),
        _ => {
            let root = root_resource(name);
            load_paths(
                eval,
                std::slice::from_ref(&Value::Str(Arc::from(root.as_str()))),
            )?;
            strict_sym(arg).map(|_| ())
        }
    }
}

/// A flag-true root's validated plan: the subset program plus its
/// §1.4 object path (the path is read once per root — evaluation
/// cannot move it). The loader holds this across evaluation, then
/// emits.
pub(crate) struct Planned {
    program: SubsetProgram,
    obj_path: PathBuf,
}

/// The per-root plan phase (§1.2 step 5): `None` when
/// `*compile-files*` is falsy (the load runs exactly as today);
/// otherwise validate `*compile-path*` (T7/T8), strip, and plan
/// (T6) — before any evaluation, so failures leave no artifact and
/// no mark (D3 atomicity).
pub(crate) fn maybe_plan_root(
    eval: &mut Evaluator,
    root: &str,
    file_star: &str,
    text: &str,
) -> Result<Option<Planned>, NativeError> {
    if !deref_core(eval, "*compile-files*").is_truthy() {
        return Ok(None);
    }
    let dir = compile_dir(eval)?;
    let stripped = strip_leading_ns(text);
    let program = plan_source(&stripped).map_err(|e| plan_failure(eval, file_star, e))?;
    Ok(Some(Planned {
        program,
        obj_path: obj_path_for(&dir, root),
    }))
}

/// The per-root emit phase (§1.2 step 7): drive the planned program
/// to its §1.4 object (T9–T11). Runs only after evaluation
/// succeeds, so eval failures (T12) leave no artifact.
pub(crate) fn emit_planned(file_star: &str, planned: &Planned) -> Result<(), NativeError> {
    match current_driver() {
        Some(driver) => driver
            .emit(&planned.program, file_star, &planned.obj_path)
            .map_err(drive_failure)?,
        None => return Err(unlinked_failure()),
    }
    Ok(())
}

/// The output dir for a flag-true root: nil is T7 (verbatim), a
/// non-string is T8 (the `String` cast).
fn compile_dir(eval: &Evaluator) -> Result<String, NativeError> {
    match deref_core(eval, "*compile-path*") {
        Value::Str(dir) => Ok(dir.to_string()),
        Value::Nil => Err(NativeError::new(
            ThrowClass::RuntimeException,
            "*compile-path* not set".to_owned(),
        )),
        other => Err(cast_err(&other, "java.lang.String")),
    }
}

/// A plan failure as a native error: reader/compile/uncaught reuse
/// the load's exact shape (`source_to_native` — T6a/T6b; the compile
/// error's source is re-attributed to the load's `*file*` candidate,
/// since planning runs on a sourceless throwaway);
/// subset/depth/Internal answer the plan diagnostic verbatim as a
/// causeless `CompilerException` (T6c — the CLI's verbatim arm;
/// `Internal` cannot arise in planning, so it shares the arm).
fn plan_failure(eval: &mut Evaluator, file_star: &str, err: PlanError) -> NativeError {
    match err {
        PlanError::Reader(e) => source_to_native(eval, file_star, SourceError::Reader(e)),
        PlanError::Compile(e) => {
            let fixed = CompileError {
                source: Arc::from(file_star),
                ..e
            };
            source_to_native(eval, file_star, SourceError::Compile(fixed))
        }
        PlanError::Uncaught(t) => source_to_native(eval, file_star, SourceError::Uncaught(t)),
        other => NativeError::new(ThrowClass::CompilerException, other.diagnostic(file_star)),
    }
}

/// A driver failure as a native error: target/emit print `build:
/// ...` (T9), io prints `build: path: message` (T10) — the CLI's
/// `build:` arms mapped to throws (exit codes do not transfer).
fn drive_failure(err: DriveError) -> NativeError {
    match err {
        DriveError::Target(message) | DriveError::Emit(message) => {
            NativeError::new(ThrowClass::IOException, format!("build: {message}"))
        }
        DriveError::Io { path, message } => NativeError::new(
            ThrowClass::IOException,
            format!("build: {}: {message}", path.display()),
        ),
    }
}

/// The unset-hook failure (T11): the CLI's exact unlinked line as an
/// `IOException`.
fn unlinked_failure() -> NativeError {
    NativeError::new(ThrowClass::IOException, UNLINKED_BUILD_LINE.to_owned())
}

/// The §1.4 object path: `<dir>/<root>.o`, from the load root
/// (never the evaluated namespace). Creates nothing.
fn obj_path_for(dir: &str, root: &str) -> PathBuf {
    Path::new(dir).join(format!("{root}.o"))
}

/// Blank the leading `ns` form's char range with spaces (newlines
/// preserved), so `plan_source` — which rejects `(ns ...)` — plans
/// the rest with original line/columns. Only the first form, only an
/// unqualified `ns` head; anything else (including read failures,
/// which planning surfaces as T6a) plans the text as-is. Evaluation
/// always runs the full unstripped text.
///
/// (The options match the planner's throwaway read
/// success-for-success: `data_readers: true` mirrors the per-form
/// `snapshot_data_readers`, and every other success-relevant field
/// agrees (read-eval true, suppress-read/resolver false, empty
/// table, no default fn, `user` ns, no aliases). Only `refers`
/// differs, and it never affects read success — so a strip/no-strip
/// decision always agrees with what the planner can read. A misread
/// fails closed either way: no strip means the planner reports the
/// subset diagnostic itself.)
fn strip_leading_ns(text: &str) -> String {
    let mut reader = Reader::new(
        text,
        &ReaderOpts {
            data_readers: true,
            ..Default::default()
        },
    );
    // `NoHooks`: only a plain `ns` head is ever detected here.
    // `#=` fails the read (no eval hook to run it); resolver
    // consults never fire (unbound). Anything unreadable plans
    // as-is per the contract above.
    let form = match reader.read_form(false, &mut NoHooks) {
        Ok(Some(form)) => form,
        _ => return text.to_owned(),
    };
    if !is_ns_head(&form) {
        return text.to_owned();
    }
    let start = span_of(&form, Span::new(1, 1));
    let end = Span::new(reader.line(), reader.col());
    blank_range(text, offset_of(text, start), offset_of(text, end))
}

/// Whether the form is a list headed by the unqualified symbol `ns`.
fn is_ns_head(form: &Value) -> bool {
    match form {
        Value::List(items) => matches!(
            items.iter().next(),
            Some(Value::Symbol(head)) if head.namespace().is_none() && head.name() == "ns"
        ),
        _ => false,
    }
}

/// Blank `[from, to)` with spaces, preserving newlines (later forms
/// keep their line/columns, so plan diagnostics blame truly).
fn blank_range(text: &str, from: usize, to: usize) -> String {
    text.char_indices()
        .map(|(at, c)| {
            if at >= from && at < to && c != '\n' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// The byte offset of a 1-based reader position: char-counted
/// columns over the reader's `\r\n`/`\r`→`\n` normalization (column
/// 0, observable after `unread('\n')`, is the line start); runs past
/// the end clamp to the text end.
fn offset_of(text: &str, target: Span) -> usize {
    let want_col = target.col.max(1);
    let mut line = 1u32;
    let mut col = 1u32;
    let mut skip_lf = false;
    for (index, ch) in text.char_indices() {
        if line == target.line && col == want_col {
            return index;
        }
        if skip_lf && ch == '\n' {
            skip_lf = false;
            continue;
        }
        skip_lf = false;
        if ch == '\r' {
            line += 1;
            col = 1;
            skip_lf = true;
        } else if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    text.len()
}
