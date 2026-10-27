// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S-instream readers: `*in*`-bindable stream readers plus the
//! `read`/`read-line` natives (issue #36).
//!
//! Both reader kinds print as
//! `clojure.lang.LineNumberingPushbackReader` (the JVM class of the
//! `*in*` root and the `with-in-str` binding alike); the id is
//! port-owned like `Writer`'s. `read` and `read-line`
//! share one persistent parser, so interleaving matches the JVM
//! and line numbers survive across calls.

#[cfg(test)]
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Stdin};
use std::sync::{Arc, Mutex};

use clj_reader::{ReadCond, Reader, ReaderError, ReaderOpts, Span};
use clj_value::{ExtensionValue, Keyword, Map, ThrowClass, Throwable, Value, new_map};

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity, deref_core, throw_class_for};

/// The JVM class of both the `*in*` root and the `with-in-str`
/// bindings (probes `in-class`, `wis-class`).
const READER_CLASS: &str = "clojure.lang.LineNumberingPushbackReader";

/// A `*in*`-bindable stream reader: process stdin or an in-memory
/// string. Identity is the sequence `id`.
pub(crate) struct StreamReader {
    id: u64,
    state: Arc<Mutex<StreamState>>,
}

impl std::fmt::Debug for StreamReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StreamReader({})", self.id)
    }
}

/// The shared read position: one persistent parser plus the live
/// stdin source (`None` for string readers; stdin stays `Some`
/// and EOF is detected per-read).
struct StreamState {
    parser: Reader,
    stdin: Option<BufReader<Stdin>>,
    /// Scripted input lines (tests only): drained before live
    /// stdin, so retry tests never touch process stdin.
    #[cfg(test)]
    script_lines: VecDeque<String>,
}

impl ExtensionValue for StreamReader {
    fn class_name(&self) -> String {
        READER_CLASS.to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[{READER_CLASS} 0x{:x} \"{READER_CLASS}@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<StreamReader>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a stream reader, if it is one.
fn as_reader(v: &Value) -> Option<&StreamReader> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<StreamReader>(),
        _ => None,
    }
}

/// A fresh stdin reader value (the `*in*` root): an empty parser
/// fed line by line from process stdin.
pub(crate) fn stdin_reader(eval: &mut Evaluator) -> Value {
    Value::Extension(Arc::new(StreamReader {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(StreamState {
            parser: Reader::new("", &ReaderOpts::default()),
            stdin: Some(BufReader::new(std::io::stdin())),
            #[cfg(test)]
            script_lines: VecDeque::new(),
        })),
    }))
}

/// A fresh string reader value (`with-in-str`): the parser starts
/// over `s` (line numbers on, for ex-data spans and form meta).
pub(crate) fn string_reader(eval: &mut Evaluator, s: &str) -> Value {
    Value::Extension(Arc::new(StreamReader {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(StreamState {
            parser: Reader::new(s, &ReaderOpts::default()),
            stdin: None,
            #[cfg(test)]
            script_lines: VecDeque::new(),
        })),
    }))
}

/// The locked stream state (a poisoned mutex still yields its
/// data, like `Writer`'s buffer).
fn lock_state(reader: &StreamReader) -> std::sync::MutexGuard<'_, StreamState> {
    reader.state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read one raw line from `src`; `None` at EOF (generic so unit
/// tests drive it with in-memory buffers instead of process
/// stdin).
fn pull_chunk<R: BufRead>(src: &mut R) -> Result<Option<String>, NativeError> {
    let mut chunk = String::new();
    let n = src
        .read_line(&mut chunk)
        .map_err(|e| NativeError::new(ThrowClass::IOException, e.to_string()))?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(chunk))
}

/// Pull one line from `src` into the parser; `false` at EOF (the
/// stdin feed for `read-line`, which never needs the read-retry
/// snapshot).
fn pull_line<R: BufRead>(src: &mut R, parser: &mut Reader) -> Result<bool, NativeError> {
    match pull_chunk(src)? {
        Some(chunk) => {
            parser.extend(&chunk);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Whether another line may still arrive (live stdin; tests also
/// feed scripted lines). `run_read` snapshots the parser only
/// then, so string reads stay zero-overhead.
fn may_grow(state: &StreamState) -> bool {
    #[cfg(test)]
    if !state.script_lines.is_empty() {
        return true;
    }
    state.stdin.is_some()
}

/// Fetch the next input line without touching the parser:
/// scripted test lines first, then live stdin when present, `None`
/// at EOF (string readers and exhausted stdin). The `read` retry
/// restores its snapshot before extending with this, so a
/// truncated head survives.
fn next_chunk(state: &mut StreamState) -> Result<Option<String>, NativeError> {
    #[cfg(test)]
    if let Some(line) = state.script_lines.pop_front() {
        return Ok(Some(line));
    }
    match state.stdin.as_mut() {
        None => Ok(None),
        Some(src) => pull_chunk(src),
    }
}

/// Feed one stdin line into the parser when stdin is live;
/// `false` for string readers and exhausted stdin (the `map`
/// closure shares its line with the chain, so string-reader tests
/// cover the dispatch while `pull_line` unit tests cover the feed
/// itself). Tests drain scripted lines first.
fn more_input(state: &mut StreamState) -> Result<bool, NativeError> {
    #[cfg(test)]
    if let Some(line) = state.script_lines.pop_front() {
        state.parser.extend(&line);
        return Ok(true);
    }
    Ok(state
        .stdin
        .as_mut()
        .map(|src| pull_line(src, &mut state.parser))
        .transpose()?
        .unwrap_or(false))
}

/// Whether a read failure is an EOF (clean or truncated): stdin
/// readers feed another line and retry these, and throw the rest.
fn is_eof_error(e: &ReaderError) -> bool {
    matches!(
        e,
        ReaderError::Eof { .. }
            | ReaderError::EofDelimited { .. }
            | ReaderError::EofChar { .. }
            | ReaderError::EofString { .. }
            | ReaderError::EofRegex { .. }
    )
}

/// A read failure as a native error: `recursive?` rethrows the raw
/// root (probe `r-recursive-outer`); otherwise a `ReaderException`
/// (`{root-class}: {root-message}`, ex-data line/column from the
/// span, the root attached as the cause).
fn map_reader_error(eval: &mut Evaluator, e: &ReaderError, recursive: bool) -> NativeError {
    let class = throw_class_for(e.jvm_class());
    let message = e.message();
    if recursive {
        return NativeError::new(class, message);
    }
    let span = e.span();
    let mut outer = NativeError::new(
        ThrowClass::ReaderException,
        format!("{}: {message}", class.jvm_name()),
    );
    let line_key = Value::Keyword(Keyword::new(Some("clojure.error"), "line"));
    let col_key = Value::Keyword(Keyword::new(Some("clojure.error"), "column"));
    let data = new_map()
        .insert(line_key, Value::Int(i64::from(span.line)))
        .insert(col_key, Value::Int(i64::from(span.col)));
    outer.data = Some(Value::Map(data));
    outer.cause = Some(Throwable {
        id: eval.next_id(),
        class,
        message: Some(Arc::from(message.as_str())),
        data: None,
        cause: None,
    });
    outer
}

/// The `:read-cond` option: `:allow`/`:preserve` opt in, anything
/// else (absent included) denies (probes `r-readcond-allow`,
/// `r-readcond-preserve`, `r-cond-opts-absent`, `r-rc-bogus`,
/// `r-rc-nil`).
fn apply_read_cond(m: &Map, opts: &mut ReaderOpts) {
    let key = Value::Keyword(Keyword::new(None, "read-cond"));
    let mode = match m.get(&key) {
        Some(Value::Keyword(kw)) if kw.namespace().is_none() && kw.name() == "allow" => {
            ReadCond::Allow
        }
        Some(Value::Keyword(kw)) if kw.namespace().is_none() && kw.name() == "preserve" => {
            ReadCond::Preserve
        }
        _ => ReadCond::Deny,
    };
    opts.read_cond = mode;
}

/// The `:features` option: a set of keywords (only keywords are
/// kept — anything else can never match a conditional feature);
/// nil/absent keeps the default; anything else fails the
/// `IPersistentSet` cast (probes `r-feat-vec`, `r-feat-nil`).
fn apply_features(m: &Map, opts: &mut ReaderOpts) -> Result<(), NativeError> {
    let key = Value::Keyword(Keyword::new(None, "features"));
    match m.get(&key) {
        None | Some(Value::Nil) => Ok(()),
        Some(Value::Set(s)) => {
            opts.features = s
                .iter()
                .filter_map(|v| match v {
                    Value::Keyword(kw) => Some(kw.clone()),
                    _ => None,
                })
                .collect();
            Ok(())
        }
        Some(other) => Err(cce_err(other, "clojure.lang.IPersistentSet")),
    }
}

/// The `[opts stream]` options: `:read-cond`/`:features` onto the
/// reader options, `:eof` presence (even nil) selecting the
/// no-throw value (probes `r-opts-*`). Nil opts mean defaults;
/// non-map opts fail the `IPersistentMap` cast (probe
/// `r-opts-nonmap`).
fn parse_read_opts(
    opts_arg: &Value,
    opts: &mut ReaderOpts,
) -> Result<(bool, Value, bool), NativeError> {
    if matches!(opts_arg, Value::Nil) {
        return Ok((true, Value::Nil, false));
    }
    let Value::Map(m) = opts_arg else {
        return Err(cce_err(opts_arg, "clojure.lang.IPersistentMap"));
    };
    apply_read_cond(m, opts);
    apply_features(m, opts)?;
    let eof_key = Value::Keyword(Keyword::new(None, "eof"));
    match m.get(&eof_key) {
        Some(v) => Ok((false, v.clone(), false)),
        None => Ok((true, Value::Nil, false)),
    }
}

/// The stream operand: `*in*` at 0-arity, the second arg at
/// 2-arity (`[opts stream]` — there is no `[stream eof-error?]`
/// arity), the first arg otherwise.
fn pick_stream(eval: &Evaluator, args: &[Value]) -> Value {
    match args.len() {
        0 => deref_core(eval, "*in*"),
        2 => args[1].clone(),
        _ => args[0].clone(),
    }
}

/// The `PushbackReader` cast: non-nil non-readers fail up front
/// (probe `r-str-stream`); nil streams wait for the `Reader.read()`
/// NPE, after opts validation (probe `r-2arity-boolstream`,
/// plus `r-optsbad-nilstream`/`r-eofopts-nilstream` for the
/// ordering).
fn check_stream_type(stream_value: &Value) -> Result<(), NativeError> {
    if !matches!(stream_value, Value::Nil) && as_reader(stream_value).is_none() {
        return Err(cce_err(stream_value, "java.io.PushbackReader"));
    }
    Ok(())
}

/// The EOF/recursive behavior plus reader options: 0/1-arity throw
/// at EOF, 2-arity parses the opts map, 3/4-arity take
/// truthiness-tested flags (probes `r-opts-*`, `r-eoferr-*`).
fn parse_read_tail(
    args: &[Value],
    opts: &mut ReaderOpts,
) -> Result<(bool, Value, bool), NativeError> {
    match args.len() {
        0 | 1 => Ok((true, Value::Nil, false)),
        2 => parse_read_opts(&args[0], opts),
        3 => Ok((args[1].is_truthy(), args[2].clone(), false)),
        _ => Ok((args[1].is_truthy(), args[2].clone(), args[3].is_truthy())),
    }
}

/// Syntax-quote and `::` resolve against the synced current
/// namespace (read-time, like the JVM), mirroring
/// `native_read_string`.
fn sync_reader_ns(eval: &Evaluator, opts: &mut ReaderOpts) {
    let cur = eval.current_ns_name();
    opts.current_ns = cur.clone();
    if let Some(ns) = eval.registry.get(&cur) {
        opts.aliases = ns.alias_map();
        opts.refers = ns.refer_map();
    }
}

/// Run one `read`: fresh options per call, stdin fed line by line
/// while the parser reports EOF, clean EOF honoring `eof-error?`
/// (the synthesized error sits at the live tracker position,
/// probe `r-exdata-eof`). A truncated head survives retries: the
/// parser is snapshotted before each attempt while more input may
/// arrive, then restored and extended with the next line (`Eof*`
/// only — anything else throws without feeding).
fn run_read(
    eval: &mut Evaluator,
    reader: &StreamReader,
    eof_error: bool,
    eof_value: Value,
    recursive: bool,
    opts: &ReaderOpts,
) -> Result<Value, NativeError> {
    let mut state = lock_state(reader);
    state.parser.set_opts(opts);
    loop {
        // Snapshot before the attempt consumes the head (string
        // readers never grow, so they never pay for the clone).
        let snapshot = may_grow(&state).then(|| state.parser.clone());
        match state.parser.read_form(false) {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => match next_chunk(&mut state)? {
                Some(chunk) => state.parser.extend(&chunk),
                None => {
                    if eof_error {
                        let at = Span::new(state.parser.line(), state.parser.col());
                        let clean = ReaderError::Eof { span: at };
                        return Err(map_reader_error(eval, &clean, recursive));
                    }
                    return Ok(eof_value.clone());
                }
            },
            Err(e) => {
                if !is_eof_error(&e) {
                    return Err(map_reader_error(eval, &e, recursive));
                }
                match next_chunk(&mut state)? {
                    Some(chunk) => {
                        if let Some(parser) = snapshot {
                            state.parser = parser;
                        }
                        state.parser.extend(&chunk);
                    }
                    None => return Err(map_reader_error(eval, &e, recursive)),
                }
            }
        }
    }
}

/// `read`: all five JVM arities over the bound reader (probes
/// `r-*`, `r-eof-outer`, `r-trunc-outer`, `r-recursive-outer`,
/// `r-exdata*`, `r-excause`).
fn native_read(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/read", args, &[0, 1, 2, 3, 4])?;
    let stream_value = pick_stream(eval, args);
    check_stream_type(&stream_value)?;
    let mut opts = ReaderOpts {
        read_cond: ReadCond::Deny,
        ..ReaderOpts::default()
    };
    let (eof_error, eof_value, recursive) = parse_read_tail(args, &mut opts)?;
    sync_reader_ns(eval, &mut opts);
    if matches!(stream_value, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.Reader.read()\" because \"r\" is null".to_owned(),
        ));
    }
    let reader = as_reader(&stream_value).expect("nil and non-readers fail above");
    run_read(eval, reader, eof_error, eof_value, recursive, &opts)
}

/// `read-line`: one line from `*in*` (nil at EOF). Nil `*in*` is
/// the verbatim `Var.get()` NPE (probe `rl-nil-in`); non-readers
/// fail the `BufferedReader` cast (probes `rl-str-in`,
/// `rl-num-in`).
fn native_read_line(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/read-line", args, &[0])?;
    let stream_value = deref_core(eval, "*in*");
    if matches!(stream_value, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.BufferedReader.readLine()\" because the return value of \"clojure.lang.Var.get()\" is null".to_owned(),
        ));
    }
    let Some(reader) = as_reader(&stream_value) else {
        return Err(cce_err(&stream_value, "java.io.BufferedReader"));
    };
    let mut state = lock_state(reader);
    if let Some(line) = state.parser.read_line() {
        return Ok(Value::Str(Arc::from(line.as_str())));
    }
    // Stdin readers feed one line and retry; string readers (and
    // exhausted stdin) observe the same `None` again.
    more_input(&mut state)?;
    Ok(state
        .parser
        .read_line()
        .map(|line| Value::Str(Arc::from(line.as_str())))
        .unwrap_or(Value::Nil))
}

/// The S-instream natives installed into `clojure.core`.
pub(crate) const READER_NATIVES: &[(&str, NativeFnPtr)] =
    &[("read", native_read), ("read-line", native_read_line)];

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    use std::io::Cursor;

    use super::*;
    use clj_value::{PrintOpts, pr_to_string};

    /// A scratch evaluator (ids only — the readers under test never
    /// evaluate).
    fn scratch() -> Evaluator {
        Evaluator::new_bare("user")
    }

    fn pr(v: &Value) -> String {
        pr_to_string(v, &PrintOpts::pr())
    }

    /// Run `run_read` over a string reader with scripted stdin lines
    /// (the stdin retry path without process stdin).
    fn run_scripted(
        eval: &mut Evaluator,
        first: &str,
        script: &[&str],
    ) -> Result<Value, NativeError> {
        let value = string_reader(eval, first);
        let reader = as_reader(&value).expect("a reader");
        lock_state(reader)
            .script_lines
            .extend(script.iter().map(|s| (*s).to_owned()));
        let opts = ReaderOpts {
            read_cond: ReadCond::Deny,
            ..ReaderOpts::default()
        };
        run_read(eval, reader, true, Value::Nil, false, &opts)
    }

    #[test]
    fn constructors_pin_kinds() {
        // The only stdin-kind test: construction alone (driving it
        // would block on process stdin). The stdin parser starts
        // empty with line numbers on; the string parser starts over
        // its input.
        let mut eval = scratch();
        let stdin_value = stdin_reader(&mut eval);
        let stdin_ref = as_reader(&stdin_value).expect("a reader");
        let stdin_state = lock_state(stdin_ref);
        assert!(stdin_state.stdin.is_some());
        assert_eq!(
            (stdin_state.parser.line(), stdin_state.parser.col()),
            (1, 1)
        );
        drop(stdin_state);
        let str_value = string_reader(&mut eval, "a\n");
        let str_ref = as_reader(&str_value).expect("a reader");
        let mut str_state = lock_state(str_ref);
        assert!(str_state.stdin.is_none());
        assert_eq!(str_state.parser.read_line(), Some("a".to_owned()));
        assert!(as_reader(&Value::Nil).is_none());
    }

    #[test]
    fn pull_line_feeds_and_reports_eof() {
        // The stdin feed over in-memory buffers: lines extend the
        // parser (normalizing), EOF reports `false`.
        let mut eval = scratch();
        let value = string_reader(&mut eval, "");
        let reader = as_reader(&value).expect("a reader");
        let mut state = lock_state(reader);
        let mut src = Cursor::new(b"a\r\nb" as &[u8]);
        assert!(pull_line(&mut src, &mut state.parser).unwrap());
        assert!(pull_line(&mut src, &mut state.parser).unwrap());
        assert!(!pull_line(&mut src, &mut state.parser).unwrap());
        assert_eq!(state.parser.read_line(), Some("a".to_owned()));
        assert_eq!(state.parser.read_line(), Some("b".to_owned()));
        assert_eq!(state.parser.read_line(), None);
    }

    #[test]
    fn stdin_retry_keeps_truncated_head() {
        // A stdin form split across lines completes with its head
        // intact (the retry restores the snapshot, then extends).
        let mut eval = scratch();
        let v = run_scripted(&mut eval, "", &["(a\n", "b)\n"]).unwrap();
        assert_eq!(pr(&v), "(a b)");
    }

    #[test]
    fn stdin_retry_exhausted_throws_truncated_span() {
        // A partial head with no more lines throws the truncated
        // error (ex-data at the bumped EOF position, probe
        // `r-exdata` shape over fed input).
        let mut eval = scratch();
        let err = run_scripted(&mut eval, "", &["(a\n"]).unwrap_err();
        assert_eq!(err.class, ThrowClass::ReaderException);
        assert_eq!(
            err.message,
            "java.lang.RuntimeException: EOF while reading, starting at line 1"
        );
        let Value::Map(data) = err.data.clone().expect("ex-data") else {
            panic!("ex-data is a map");
        };
        let line_key = Value::Keyword(Keyword::new(Some("clojure.error"), "line"));
        let col_key = Value::Keyword(Keyword::new(Some("clojure.error"), "column"));
        assert_eq!(data.get(&line_key), Some(&Value::Int(2)));
        assert_eq!(data.get(&col_key), Some(&Value::Int(1)));
    }

    #[test]
    fn stdin_retry_clean_eof_feeds_then_reads() {
        // Clean `None`s (empty, then whitespace-only) feed lines
        // until a form completes.
        let mut eval = scratch();
        let v = run_scripted(&mut eval, "", &["\n", "(b)\n"]).unwrap();
        assert_eq!(pr(&v), "(b)");
    }

    #[test]
    fn pull_line_rejects_non_utf8() {
        // Invalid UTF-8 is an `IOException` carrying the OS message.
        let mut eval = scratch();
        let value = string_reader(&mut eval, "");
        let reader = as_reader(&value).expect("a reader");
        let mut state = lock_state(reader);
        let mut src = Cursor::new(b"\xff" as &[u8]);
        let err = pull_line(&mut src, &mut state.parser).unwrap_err();
        assert_eq!(err.class, ThrowClass::IOException);
        assert!(
            err.message.contains("stream did not contain valid UTF-8"),
            "{}",
            err.message
        );
    }

    #[test]
    fn reader_debug_names_id() {
        // The `Debug` impl (an `ExtensionValue` bound) names the id.
        let mut eval = scratch();
        let value = string_reader(&mut eval, "x");
        let reader = as_reader(&value).expect("a reader");
        let id = reader.id;
        assert_eq!(format!("{reader:?}"), format!("StreamReader({id})"));
    }

    #[test]
    fn reader_eq_and_hash_use_id() {
        // Identity semantics, mirroring `Writer` (behavior tests
        // pin `=`/`distinct?`; the direct calls pin the trait fns).
        let mut eval = scratch();
        let first = string_reader(&mut eval, "x");
        let second = string_reader(&mut eval, "x");
        let a = as_reader(&first).expect("a reader");
        let b = as_reader(&second).expect("a reader");
        assert!(a.dyn_eq(a));
        assert!(!a.dyn_eq(b));
        let mut ha = DefaultHasher::new();
        a.dyn_hash(&mut ha);
        let mut hb = DefaultHasher::new();
        b.dyn_hash(&mut hb);
        assert_ne!(ha.finish(), hb.finish());
    }
}
