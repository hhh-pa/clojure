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

use std::io::Write;
use std::sync::{Arc, Mutex};

use clj_reader::{ReaderOpts, read_str};
use clj_value::{ExtensionValue, ThrowClass, Value, pr_to_string};

use crate::core::{cce_err, cce_message, print_opts};
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::lazy::realize_deep;
use crate::ns::{NativeFnPtr, check_arity, deref_core, signal_to_native, str_of};

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
    if matches!(out, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null".to_owned(),
        ));
    }
    let Some(w) = as_writer(&out) else {
        return Err(NativeError::new(
            ThrowClass::ClassCastException,
            cce_message(&jvm_class_of(&out), "java.io.Writer"),
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

/// `print`/`println`/`pr`/`prn`: space-joined (no trailing space);
/// `ln` appends a newline. `readably` overrides the binding when
/// given (`print` binds false, `pr` honors).
fn print_impl(
    eval: &mut Evaluator,
    args: &[Value],
    readably: Option<bool>,
    ln: bool,
) -> Result<Value, NativeError> {
    let opts = print_opts(eval, readably)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        let v = realize_deep(eval, arg).map_err(signal_to_native)?;
        parts.push(pr_to_string(&v, &opts));
    }
    let mut out = parts.join(" ");
    if ln {
        out.push('\n');
    }
    // Bare `print` writes nothing (not even an empty write).
    if !out.is_empty() {
        write_out(eval, &out)?;
    }
    Ok(Value::Nil)
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

/// `newline`: writes `\n` to `*out*`.
fn native_newline(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/newline", args, &[0])?;
    write_out(eval, "\n")?;
    Ok(Value::Nil)
}

/// `flush`: flushes stdout when `*out*` is stdout, else a no-op.
fn native_flush(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/flush", args, &[0])?;
    if as_writer(&deref_core(eval, "*out*")).is_some_and(|w| matches!(w.kind, WriterKind::Stdout)) {
        std::io::stdout().lock().flush().map_err(io_err)?;
    }
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
fn native_read_string(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/read-string", args, &[1])?;
    if matches!(args[0], Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            String::new(),
        ));
    }
    let Value::Str(s) = &args[0] else {
        return Err(cce_err(&args[0], "java.lang.String"));
    };
    // Syntax-quote and `::` resolve against the synced current
    // namespace (read-time, like the JVM); `line_numbers: false`
    // gives `read-string` behavior (bare EOF errors, no meta).
    let cur = eval.current_ns_name();
    let mut opts = ReaderOpts {
        current_ns: cur.clone(),
        line_numbers: false,
        ..Default::default()
    };
    if let Some(ns) = eval.registry.get(&cur) {
        opts.aliases = ns.alias_map();
        opts.refers = ns.refer_map();
    }
    let text: &str = s;
    read_str(text, &opts).map_err(|e| NativeError::new(ThrowClass::RuntimeException, e.message()))
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
