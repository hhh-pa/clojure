// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D IO: `*out*`-bindable writers, the `print` family, `newline`,
//! `flush`, `slurp`/`spit`, and `read-string`.
//!
//! Printing honors the bound `*out*` writer (stdout, stderr, or an
//! in-memory buffer); file natives only speak UTF-8 paths.

use std::cell::Cell;
use std::io::Write;
use std::sync::{Arc, Mutex};

use clj_reader::{EvalReadError, ReadCond, Reader, ReaderError, ReaderOpts};
use clj_value::{ExtensionValue, ThrowClass, Value};

use crate::core::{cce_err, cce_message, print_opts};
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::lazy::realize_deep;
use crate::ns::{
    NativeFnPtr, check_arity, deref_core, signal_to_native, str_of, throw_class_for,
    throwable_to_native,
};

/// A `*out*`-bindable writer: process stdout/stderr or an
/// in-memory buffer. Identity is the sequence `id`.
#[derive(Debug)]
pub(crate) struct Writer {
    id: u64,
    kind: WriterKind,
}

/// The writer target. Staged for `*out*`/`*err*`/`print`
/// (S5D).
#[derive(Debug)]
#[allow(dead_code)]
enum WriterKind {
    Stdout,
    Stderr,
    Buffer(Arc<Mutex<String>>),
}

impl Writer {
    /// The JVM class name for a writer kind.
    fn class_of(kind: &WriterKind) -> &'static str {
        match kind {
            WriterKind::Stdout => "java.io.OutputStreamWriter",
            WriterKind::Stderr => "java.io.PrintWriter",
            WriterKind::Buffer(_) => "java.io.StringWriter",
        }
    }
}

impl ExtensionValue for Writer {
    fn class_name(&self) -> String {
        Self::class_of(&self.kind).to_owned()
    }

    fn print(&self) -> String {
        let class = Self::class_of(&self.kind);
        format!("#object[{class} 0x{:x} \"{class}@{:x}\"]", self.id, self.id)
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Writer>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a writer, if it is one.
fn as_writer(v: &Value) -> Option<&Writer> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<Writer>(),
        _ => None,
    }
}

/// A fresh stdout writer value. Staged for `*out*` (S5D).
#[allow(dead_code)]
pub(crate) fn stdout_writer(eval: &mut Evaluator) -> Value {
    Value::Extension(Arc::new(Writer {
        id: eval.next_id(),
        kind: WriterKind::Stdout,
    }))
}

/// A fresh stderr writer value. Staged for `*err*` (S5D).
#[allow(dead_code)]
pub(crate) fn stderr_writer(eval: &mut Evaluator) -> Value {
    Value::Extension(Arc::new(Writer {
        id: eval.next_id(),
        kind: WriterKind::Stderr,
    }))
}

/// A fresh in-memory string writer value. Staged for
/// `with-out-str` (S5D).
#[allow(dead_code)]
pub(crate) fn string_writer(eval: &mut Evaluator) -> Value {
    Value::Extension(Arc::new(Writer {
        id: eval.next_id(),
        kind: WriterKind::Buffer(Arc::new(Mutex::new(String::new()))),
    }))
}

/// The accumulated contents of a string writer (`None` for any
/// other value). Staged for `with-out-str` (S5D).
#[allow(dead_code)]
pub(crate) fn writer_contents(v: &Value) -> Option<String> {
    let w = as_writer(v)?;
    match &w.kind {
        WriterKind::Buffer(buf) => buf.lock().ok().map(|s| s.clone()),
        _ => None,
    }
}

/// An IO failure as an `IOException` carrying the OS message.
fn io_err(e: std::io::Error) -> NativeError {
    NativeError::new(ThrowClass::IOException, e.to_string())
}

/// Write a string to the bound `*out*` writer.
pub(crate) fn write_out(eval: &mut Evaluator, s: &str) -> Result<(), NativeError> {
    let out = deref_core(eval, "*out*");
    write_to_writer(&out, s)
}

/// Write a string to a writer value (nil → the `write` NPE,
/// non-writer → the `Writer` CCE — shared by `write_out`,
/// `print-simple`, `print-ctor`, and the print driver).
pub(crate) fn write_to_writer(w: &Value, s: &str) -> Result<(), NativeError> {
    if matches!(w, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null".to_owned(),
        ));
    }
    let Some(w) = as_writer(w) else {
        return Err(NativeError::new(
            ThrowClass::ClassCastException,
            cce_message(&jvm_class_of(w), "java.io.Writer"),
        ));
    };
    match &w.kind {
        WriterKind::Stdout => std::io::stdout()
            .lock()
            .write_all(s.as_bytes())
            .map_err(io_err),
        WriterKind::Stderr => std::io::stderr()
            .lock()
            .write_all(s.as_bytes())
            .map_err(io_err),
        WriterKind::Buffer(buf) => {
            buf.lock().unwrap_or_else(|e| e.into_inner()).push_str(s);
            Ok(())
        }
    }
}

/// A writer value's validity without writing (nil → the `write`
/// NPE, non-writer → the `Writer` CCE — the upfront `*out*`
/// validation and the `print-simple`/`print-ctor` writer checks).
pub(crate) fn check_writer(w: &Value) -> Result<(), NativeError> {
    if matches!(w, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null".to_owned(),
        ));
    }
    if as_writer(w).is_none() {
        return Err(NativeError::new(
            ThrowClass::ClassCastException,
            cce_message(&jvm_class_of(w), "java.io.Writer"),
        ));
    }
    Ok(())
}

/// Flush `*out*` (stdout flushes, anything else is a no-op) and
/// count the flush for the `*flush-on-newline*` test hook.
pub(crate) fn flush_out(eval: &mut Evaluator) -> Result<(), NativeError> {
    eval.bump_flush_count();
    if as_writer(&deref_core(eval, "*out*")).is_some_and(|w| matches!(w.kind, WriterKind::Stdout)) {
        std::io::stdout().lock().flush().map_err(io_err)?;
    }
    Ok(())
}

/// Serializes whole-print emissions across threads. Pre-S8
/// `print_impl` rendered the full line and issued one write (one
/// lock acquisition — atomic); the per-arg `pr-on` driver issues
/// several writes, so racing `println`s interleave mid-line without
/// this (`T5-LINE-0T0-LINE-0`, msrv `run_joins_drainers_at_exit`).
/// Re-entry safe: nested prints on the same thread (a user
/// `print-method` re-entering `print`, pinned to overflow like the
/// JVM) skip the mutex and emit inline, while other threads block.
static PRINT_EMISSION_LOCK: Mutex<()> = Mutex::new(());
thread_local! {
    static PRINT_EMISSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Held across one top-level print emission (`print_impl`,
/// `newline`, `print-simple`, `print-ctor`). Depth-tracked so the
/// same thread never deadlocks on re-entry; the mutex is only ever
/// uncontended-then-held (no lock ordering: emission never waits on
/// another thread while holding it).
pub(crate) struct EmissionGuard {
    _lock: Option<std::sync::MutexGuard<'static, ()>>,
}

impl EmissionGuard {
    pub(crate) fn acquire() -> Self {
        let nested = PRINT_EMISSION_DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n
        });
        let lock = if nested == 0 {
            Some(
                PRINT_EMISSION_LOCK
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
            )
        } else {
            None
        };
        Self { _lock: lock }
    }
}

impl Drop for EmissionGuard {
    fn drop(&mut self) {
        PRINT_EMISSION_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// `print`/`println`/`pr`/`prn`: space-joined (no trailing space);
/// `ln` appends a newline. `readably` overrides the binding when
/// given (`print` binds false, `pr` honors). Every arg renders
/// through the `pr-on` driver (user `print-method`/`print-dup`
/// entries fire); `*out*` re-reads per arg, and the `ln` newline
/// flushes iff `*flush-on-newline*` is truthy. The whole emission
/// holds the emission lock (atomic lines across threads).
fn print_impl(
    eval: &mut Evaluator,
    args: &[Value],
    readably: Option<bool>,
    ln: bool,
) -> Result<Value, NativeError> {
    // Eager validation first (limits CCE before anything realizes
    // or writes, as before).
    print_opts(eval, readably)?;
    let mut vals = Vec::with_capacity(args.len());
    for arg in args {
        vals.push(realize_deep(eval, arg).map_err(signal_to_native)?);
    }
    // The `readably` override binds like the JVM's `print` (the
    // driver defaults read the binding back).
    let depth = crate::printsys::maybe_bind_print_readably(eval, readably);
    // Upfront `*out*` validation (stock NPE/CCE) whenever output
    // follows; bare `pr`/`print` write nothing (not even a check).
    if !vals.is_empty() || ln {
        let out = deref_core(eval, "*out*");
        if let Err(e) = check_writer(&out) {
            eval.pop_bindings(depth);
            return Err(e);
        }
    }
    // One emission, one lock-hold: the arg writes, the `ln`
    // newline, and its flush serialize against other threads'
    // emissions (pre-S8 single-write atomicity).
    let _emission = EmissionGuard::acquire();
    let result = print_args(eval, &vals, ln);
    eval.pop_bindings(depth);
    result?;
    if ln && deref_core(eval, "*flush-on-newline*").is_truthy() {
        flush_out(eval)?;
    }
    Ok(Value::Nil)
}

/// One `pr-on` per arg (separators between, `*out*` re-read per
/// arg) plus the `ln` newline on the current `*out*`.
fn print_args(eval: &mut Evaluator, vals: &[Value], ln: bool) -> Result<(), NativeError> {
    for (i, v) in vals.iter().enumerate() {
        let out = deref_core(eval, "*out*");
        check_writer(&out)?;
        if i > 0 {
            write_to_writer(&out, " ")?;
        }
        crate::printsys::pr_on(eval, v, &out).map_err(signal_to_native)?;
    }
    if ln {
        let out = deref_core(eval, "*out*");
        write_to_writer(&out, "\n")?;
    }
    Ok(())
}

fn native_print(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    print_impl(eval, args, Some(false), false)
}

fn native_println(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    print_impl(eval, args, Some(false), true)
}

fn native_pr(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    print_impl(eval, args, None, false)
}

fn native_prn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    print_impl(eval, args, None, true)
}

/// `time`: `prn` one prebuilt line (readable + newline, like the
/// JVM `(prn (str ...))`).
pub(crate) fn prn_str(eval: &mut Evaluator, line: &str) -> Result<(), NativeError> {
    print_impl(
        eval,
        std::slice::from_ref(&Value::Str(Arc::from(line))),
        None,
        true,
    )?;
    Ok(())
}

/// `newline`: writes `\n` to `*out*`. Serialized with print
/// emissions so it cannot land mid-`println` (pre-S8 both were
/// single writes through the same lock).
fn native_newline(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/newline", args, &[0])?;
    let _emission = EmissionGuard::acquire();
    write_out(eval, "\n")?;
    Ok(Value::Nil)
}

/// `flush`: flushes stdout when `*out*` is stdout, else a no-op.
fn native_flush(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/flush", args, &[0])?;
    flush_out(eval)?;
    Ok(Value::Nil)
}

/// The `:encoding` check: only UTF-8 (any case) is supported.
fn check_encoding(v: &Value) -> Result<(), NativeError> {
    match v {
        Value::Str(s) if s.eq_ignore_ascii_case("utf-8") => Ok(()),
        _ => Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            format!("unsupported encoding: {}", str_of(v)),
        )),
    }
}

/// File option pairs: odd counts name the last item, `:encoding`
/// must be UTF-8, unknown keys are ignored. Returns whether
/// `:append` is truthy (`:append` itself is ignored unless wanted).
fn file_opts(opts: &[Value], want_append: bool) -> Result<bool, NativeError> {
    if opts.len() % 2 == 1 {
        let last = &opts[opts.len() - 1];
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No value supplied for key: {}", str_of(last)),
        ));
    }
    let mut append = false;
    // (`chunks_exact`, not `as_chunks`: the latter is 1.88+,
    // past the 1.85 MSRV floor.)
    for pair in opts.chunks_exact(2) {
        let name = match &pair[0] {
            Value::Keyword(kw) if kw.namespace().is_none() => kw.name(),
            _ => continue,
        };
        match name {
            "encoding" => check_encoding(&pair[1])?,
            "append" if want_append => append = pair[1].is_truthy(),
            _ => {}
        }
    }
    Ok(append)
}

/// The OS message with any ` (os error N)` suffix stripped.
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => s[..i].to_owned(),
        _ => s,
    }
}

/// A file failure: missing files name the path plus the bare OS
/// message, anything else is the bare display.
fn file_err(path: &str, e: std::io::Error) -> NativeError {
    if e.kind() == std::io::ErrorKind::NotFound {
        NativeError::new(
            ThrowClass::FileNotFoundException,
            format!("{path} ({})", strerror(&e)),
        )
    } else {
        NativeError::new(ThrowClass::IOException, e.to_string())
    }
}

/// `slurp`: read a file as a UTF-8 string.
fn native_slurp(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/slurp", 0));
    }
    let Value::Str(path) = &args[0] else {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot open <{}> as an InputStream.", str_of(&args[0])),
        ));
    };
    file_opts(&args[1..], false)?;
    let path_str: &str = path;
    let text = std::fs::read_to_string(path_str).map_err(|e| file_err(path_str, e))?;
    Ok(Value::Str(Arc::from(text.as_str())))
}

/// `spit`: write a string to a file (appending with `:append`,
/// creating either way).
fn native_spit(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 {
        return Err(NativeError::arity("clojure.core/spit", args.len()));
    }
    let Value::Str(path) = &args[0] else {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot open <{}> as an OutputStream.", str_of(&args[0])),
        ));
    };
    let Value::Str(content) = &args[1] else {
        return Err(cce_err(&args[1], "java.lang.String"));
    };
    let append = file_opts(&args[2..], true)?;
    let path_str: &str = path;
    let content_str: &str = content;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    (|| -> std::io::Result<()> {
        let mut f = opts.open(path_str)?;
        f.write_all(content_str.as_bytes())
    })()
    .map_err(|e| file_err(path_str, e))?;
    Ok(Value::Nil)
}

/// `read-string`: read one form (no line numbers, like the JVM).
/// S7 gates run after the string checks (probes `g-rs-*`).
/// The 2-arity reuses the `read` opts parser (issue #40: every
/// `rs-opts-*` oracle row agrees with the `r-opts-*`/`r-feat-*`/
/// `r-rc-*` pins), except the string validates FIRST (probes
/// `rs-opts-order`, `rs-opts-order-nil`) and nil fails with the
/// `String.length` NPE (probe `rs-opts-nilstr`), not the 1-arity
/// bare NPE. Errors stay bare `RuntimeException`s (no
/// `ReaderException` wrapper — every `rs-opts-*` throw row).
fn native_read_string(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/read-string", args, &[1, 2])?;
    let s_arg = args.last().expect("arity 1..=2 has a last arg");
    if matches!(s_arg, Value::Nil) {
        let message = if args.len() == 2 {
            "Cannot invoke \"String.length()\" because \"s\" is null".to_owned()
        } else {
            String::new()
        };
        return Err(NativeError::new(ThrowClass::NullPointerException, message));
    }
    let Value::Str(s) = s_arg else {
        return Err(cce_err(s_arg, "java.lang.String"));
    };
    let seed = crate::reader::seed_read_gates(eval)?;
    // Syntax-quote and `::` resolve against the synced current
    // namespace (read-time, like the JVM); `line_numbers: false`
    // gives `read-string` behavior (bare EOF errors, no meta).
    let cur = eval.current_ns_name();
    let mut opts = ReaderOpts {
        current_ns: cur.clone(),
        line_numbers: false,
        read_cond: ReadCond::Deny,
        ..Default::default()
    };
    // The 1-arity keeps deny-everything defaults (probe
    // `rs-opts-none`); the 2-arity parses the opts map (the
    // `recursive` tail is `read`-only — there is no wrapper to
    // skip here).
    let (eof_error, eof_value) = if args.len() == 2 {
        let (eof_error, eof_value, _) = crate::reader::parse_read_opts(&args[0], &mut opts)?;
        (eof_error, eof_value)
    } else {
        (true, Value::Nil)
    };
    if let Some(ns) = eval.registry.get(&cur) {
        opts.aliases = ns.alias_map();
        opts.refers = ns.refer_map();
    }
    crate::core::snapshot_data_readers(eval, &mut opts);
    opts.read_eval = seed.read_eval;
    opts.suppress_read = seed.suppress_read;
    opts.resolver_bound = seed.resolver_bound;
    // `read_form(false)` separates clean EOF (`None` → the `:eof`
    // value, probes `rs-opts-eof*`) from truncated input (`Err` →
    // always throws, probes `rs-opts-eof-trunc*`); `read_str`
    // cannot (both are bare `Eof` without line numbers).
    let text: &str = s;
    let mut reader = Reader::new(text, &opts);
    let outcome = {
        let mut hooks = crate::reader::LiveReadHooks::new(eval);
        reader.read_form(false, &mut hooks)
    };
    let fired = reader.take_fired();
    // Effects precede every outcome (fired tags ran during the read
    // — probe `e-discard-eof` fires before the EOF), so drain first.
    crate::core::drain_fired_tags(eval, fired)?;
    match outcome {
        Ok(Some(v)) => crate::core::resolve_data_readers(eval, v),
        Ok(None) if !eof_error => Ok(eof_value),
        Ok(None) => Err(NativeError::new(
            ThrowClass::RuntimeException,
            "EOF while reading".to_owned(),
        )),
        Err(e) => Err(land_read_string_error(e)),
    }
}

/// A `read-string` failure as a native error: a `#=` or resolver
/// throwable surfaces raw — class, message, data, and cause intact
/// (S7 probes `c-*`, `d-*`, `f-sq-currentnil`); S6 consult errors
/// keep their JVM classes (a broken table is a `ClassCastException`,
/// invalid payloads throw the validator's class); everything else
/// stays the bare `RuntimeException` the JVM reports without line
/// numbers (S7 probes `e-trunc`, `c-eof`, `f-nilalias`).
fn land_read_string_error(e: ReaderError) -> NativeError {
    if let ReaderError::EvalFailed {
        error: EvalReadError::Failed(t),
        ..
    } = &e
    {
        return throwable_to_native(t);
    }
    match e {
        ReaderError::TableCce { .. } | ReaderError::TagInvoke { .. } => {
            NativeError::new(throw_class_for(e.jvm_class()), e.message())
        }
        _ => NativeError::new(ThrowClass::RuntimeException, e.message()),
    }
}

/// The S5D IO natives installed into `clojure.core`.
pub(crate) const IO_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("print", native_print),
    ("println", native_println),
    ("pr", native_pr),
    ("prn", native_prn),
    ("newline", native_newline),
    ("flush", native_flush),
    ("slurp", native_slurp),
    ("spit", native_spit),
    ("read-string", native_read_string),
];

#[cfg(test)]
mod tests {
    use std::io::Error;

    use super::*;

    #[test]
    fn writer_contents_non_buffer_is_none() {
        // Process writers carry no accumulated contents.
        let stdout = Value::Extension(Arc::new(Writer {
            id: 0,
            kind: WriterKind::Stdout,
        }));
        assert_eq!(writer_contents(&stdout), None);
        let stderr = Value::Extension(Arc::new(Writer {
            id: 1,
            kind: WriterKind::Stderr,
        }));
        assert_eq!(writer_contents(&stderr), None);
    }

    #[test]
    fn io_err_carries_os_message() {
        // Any IO failure formats as an IOException (a real failing
        // stdout write is not inducible in-process, so the
        // constructor is covered directly).
        let err = io_err(Error::other("boom"));
        assert_eq!(err.class, ThrowClass::IOException);
        assert_eq!(err.message, "boom");
    }

    #[test]
    fn strerror_keeps_suffixless_messages() {
        // Custom errors carry no ` (os error N)` suffix to strip.
        let custom = Error::other("custom");
        assert_eq!(strerror(&custom), "custom");
    }
}
