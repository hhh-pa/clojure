// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-120 slice 3: the `clojure.java.io` facsimile — `file`,
//! `reader`, `writer`, `input-stream`, `output-stream`, and
//! `delete-file` over `std::fs` on the `jdkio`/`io`/`reader`
//! bases. A native namespace (the portdeps shape): requiring
//! `clojure.java.io` installs the six natives, with no source
//! file — the upstream protocols have no port values to extend
//! (`URL`/`Socket` do not exist here), so the dispatch lives in
//! Rust and only the error texts travel over.
//!
//! Oracle: upstream `clojure/java/io.clj` source text (read,
//! never executed — no JVM at runtime, ever). Every error below
//! cites its upstream site; behavior the facsimile narrows is
//! pinned as a D-divergence in `tests/jio.rs`:
//!
//! - D0: opens are strict — the whole file reads at open. The
//!   `FileNotFoundException` timing matches (open-time, like the
//!   `FileInputStream` ctor); mid-read IO errors surface at open.
//! - D1: readers ARE the port's `LineNumberingPushbackReader`
//!   (so `read`/`read-line`/`line-seq` interoperate); `(class
//!   (reader f))` differs from upstream's `BufferedReader`.
//! - D2: no network — `http(s)` URIs and sockets are refused;
//!   strings resolve `file:` URIs first, then open literally.
//! - D3: `(file nil "c")` answers the child alone — exactly like
//!   the JDK (`File.java`: a null parent takes `normalize(child)`),
//!   so this pins parity, not a divergence (the empty-parent
//!   fs-root case matches too, via the join).
//! - D4: derived handles are independent — `(writer
//!   (output-stream f))` opens its own append handle (its close
//!   never propagates), and `(reader (input-stream f))`
//!   snapshots the full bytes (streams are non-positional).
//! - D5: relative-URI `as-file` reports `Not a file` (upstream:
//!   `MalformedURLException`); malformed `%` stays literal
//!   (upstream: `URLDecoder` throws).
//! - D6: long-tail charsets report UEE (inherits slice-2 N6);
//!   writer output encodes per the JDK `OutputStreamWriter`
//!   (replacement `?`, `UTF-16`/`UTF-32` BOMs).
//!
//! Out of scope (not interned): `as-file`, `as-url`,
//! `as-relative-path`, `copy`, `resource`, `make-parents`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use clj_value::{
    ArrayStore, ExtensionValue, PrintOpts, ThrowClass, Value, jvm_class_of, pr_to_string,
};

use crate::core::cce_err;
use crate::ctor::{Charset, decode, lookup_charset};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::io::{as_writer, file_err, file_writer, io_err, stream_closed};
use crate::jdkio::{file as jdk_file, file_path};
use crate::ns::{NativeFnPtr, check_arity, str_of};
use crate::reader::{as_reader, string_reader};

/// The facsimile namespace (native-only, no source file).
pub(crate) const JIO_NS: &str = "clojure.java.io";

/// Install the six facsimile natives (the portdeps shape —
/// never referred into `user`, never in `clojure.core`).
pub(crate) fn install_jio_ns(eval: &mut Evaluator) {
    eval.registry.ensure_ns(JIO_NS);
    for (name, f) in JIO_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(JIO_NS, name, func);
    }
}

/// An emulated `java.io.BufferedInputStream`: eager bytes plus
/// the closed latch. Built only by `input-stream` (and its byte
/// arm); `reader` decodes the bytes, `.close` latches.
#[derive(Debug)]
pub(crate) struct JioInputStream {
    id: u64,
    state: Arc<Mutex<JioInputState>>,
}

/// The shared input state: bytes plus the closed latch.
#[derive(Debug)]
struct JioInputState {
    bytes: Arc<[u8]>,
    closed: bool,
}

impl ExtensionValue for JioInputStream {
    fn class_name(&self) -> String {
        "java.io.BufferedInputStream".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.io.BufferedInputStream 0x{:x} \"java.io.BufferedInputStream@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JioInputStream>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An emulated `java.io.BufferedOutputStream`: its path (for the
/// derived-writer append handle, D4) plus the open file and the
/// closed latch. Built only by `output-stream`.
#[derive(Debug)]
pub(crate) struct JioOutputStream {
    id: u64,
    path: Arc<str>,
    state: Arc<Mutex<JioOutputState>>,
}

/// The shared output state: the open file plus the closed latch.
#[derive(Debug)]
struct JioOutputState {
    file: std::fs::File,
    closed: bool,
}

impl ExtensionValue for JioOutputStream {
    fn class_name(&self) -> String {
        "java.io.BufferedOutputStream".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.io.BufferedOutputStream 0x{:x} \"java.io.BufferedOutputStream@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JioOutputStream>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a facsimile input stream, if it is one.
fn as_input_stream(v: &Value) -> Option<&JioInputStream> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JioInputStream>(),
        _ => None,
    }
}

/// Downcast a value to a facsimile output stream, if it is one.
fn as_output_stream(v: &Value) -> Option<&JioOutputStream> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JioOutputStream>(),
        _ => None,
    }
}

/// A fresh input stream over eager bytes.
fn input_stream_value(eval: &mut Evaluator, bytes: Vec<u8>) -> Value {
    Value::Extension(Arc::new(JioInputStream {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(JioInputState {
            bytes: Arc::from(bytes),
            closed: false,
        })),
    }))
}

/// A fresh output stream over an open file.
fn output_stream_value(eval: &mut Evaluator, path: &str, file: std::fs::File) -> Value {
    Value::Extension(Arc::new(JioOutputStream {
        id: eval.next_id(),
        path: Arc::from(path),
        state: Arc::new(Mutex::new(JioOutputState {
            file,
            closed: false,
        })),
    }))
}

/// `close` on facsimile streams (`None` for anything else — the
/// miss shapes; void, so never a bean key). Output closes flush
/// and latch (second closes no-op); input closes only latch.
pub(crate) fn close_stream_value(
    e: &Arc<dyn ExtensionValue>,
) -> Option<Result<Value, NativeError>> {
    close_input_value(e).or_else(|| close_output_value(e))
}

/// `close` on a facsimile input stream (`None` for anything
/// else).
fn close_input_value(e: &Arc<dyn ExtensionValue>) -> Option<Result<Value, NativeError>> {
    let s = e.as_any().downcast_ref::<JioInputStream>()?;
    s.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
    Some(Ok(Value::Nil))
}

/// `close` on a facsimile output stream (`None` for anything
/// else).
fn close_output_value(e: &Arc<dyn ExtensionValue>) -> Option<Result<Value, NativeError>> {
    let s = e.as_any().downcast_ref::<JioOutputStream>()?;
    let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.closed {
        return Some(Ok(Value::Nil));
    }
    st.closed = true;
    Some(st.file.flush().map(|()| Value::Nil).map_err(io_err))
}

/// `flush` on a facsimile output stream (`None` for anything
/// else — readers and input streams have no `flush`, like the
/// JVM; void, so never a bean key).
pub(crate) fn flush_stream_value(
    e: &Arc<dyn ExtensionValue>,
) -> Option<Result<Value, NativeError>> {
    let s = e.as_any().downcast_ref::<JioOutputStream>()?;
    let mut st = s.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.closed {
        return Some(Err(stream_closed()));
    }
    Some(st.file.flush().map(|()| Value::Nil).map_err(io_err))
}

/// The `pr-str` of a value, for the `Cannot open <x>` texts
/// (upstream `pr-str`s — nil renders `nil`, never empty).
fn pr_of(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// `as-file`: nil stays nil, strings are literal (`(File. s)` —
/// no URL resolution, unlike the stream fns), files answer
/// their path, `file:` URIs decode; anything else is the
/// protocol-miss IAE (upstream extends `nil`/`String`/`File`/
/// `URL`/`URI` only).
fn as_file(v: &Value) -> Result<Option<String>, NativeError> {
    if matches!(v, Value::Nil) {
        return Ok(None);
    }
    if let Value::Str(s) = v {
        return Ok(Some(s.to_string()));
    }
    if let Some(path) = file_path(v) {
        return Ok(Some(path));
    }
    if let Value::Uri(u) = v {
        return uri_file_path(u.text()).map(Some);
    }
    Err(as_file_miss(v))
}

/// The `Coercions` miss text (upstream protocol shape).
fn as_file_miss(v: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!(
            "No implementation of method: :as-file of protocol: #'clojure.java.io/Coercions found for class: {}",
            jvm_class_of(v)
        ),
    )
}

/// Java `File.isAbsolute`: Rust's `is_absolute` under-reports on
/// Windows (a rooted-but-driveless `/abs` has no drive prefix, yet
/// Java counts any prefix — drive, UNC, or bare slash). Windows
/// checks prefix-or-root; anywhere else the two agree.
#[cfg(windows)]
fn file_is_absolute(path: &str) -> bool {
    let p = std::path::Path::new(path);
    matches!(p.components().next(), Some(std::path::Component::Prefix(_))) || p.has_root()
}

/// Unix: Rust and Java agree (leading slash).
#[cfg(not(windows))]
fn file_is_absolute(path: &str) -> bool {
    std::path::Path::new(path).is_absolute()
}

/// Upstream `as-relative-path`: nil hits the `isAbsolute` NPE
/// (the `check_file_root` shape), absolute paths the IAE,
/// otherwise the path itself.
fn as_relative_path(v: &Value) -> Result<String, NativeError> {
    match as_file(v)? {
        None => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.io.File.isAbsolute()\" because \"f\" is null".to_owned(),
        )),
        Some(path) if file_is_absolute(&path) => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("{path} is not a relative path"),
        )),
        Some(path) => Ok(path),
    }
}

/// `new File(parent, child)`: `None` parents answer the child
/// (D3 — the JDK does the same: a null parent takes
/// `normalize(child)`); trailing separators never double
/// (either slash — Windows joins accept both).
fn join_path(parent: Option<&str>, child: &str) -> String {
    match parent {
        None => child.to_owned(),
        Some(p) if p.ends_with('/') || p.ends_with('\\') => format!("{p}{child}"),
        Some(p) => format!("{p}{}{child}", std::path::MAIN_SEPARATOR),
    }
}

/// A `file:` URI's local path: the authority drops (the JDK
/// `URL.getFile` route keeps path+query only — even
/// `localhost` vanishes), a `/` ahead of a `X:/` drive drops on
/// Windows (the JDK `File(URI)` route), `/` takes the platform
/// separator, `%XX` decodes (`+` stays literal). Anything else is
/// the upstream `Not a file` IAE (D2/D5: no network, no
/// `MalformedURLException`).
fn uri_file_path(text: &str) -> Result<String, NativeError> {
    match strip_file_scheme(text) {
        Some(rest) => Ok(file_uri_local_path(rest)),
        None => Err(not_a_file(text)),
    }
}

/// The upstream `Not a file` IAE.
fn not_a_file(u: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Not a file: {u}"),
    )
}

/// The scheme-stripped remainder of a `file:` URI (`None` for
/// any other scheme — matched case-insensitively, like the
/// JDK).
fn strip_file_scheme(text: &str) -> Option<&str> {
    if text
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
    {
        text.get(5..)
    } else {
        None
    }
}

/// Drop a `file:` URI's `//authority` (see [`uri_file_path`]).
fn strip_authority(rest: &str) -> &str {
    let Some(after) = rest.strip_prefix("//") else {
        return rest;
    };
    match after.find('/') {
        Some(i) => &after[i..],
        None => "",
    }
}

/// Drop the `/` ahead of a `X:/` drive (`file:///C:/x` → `C:/x`).
/// Windows-only, like the JDK `File(URI)` route — anywhere else a
/// leading slash is a legitimate absolute path and survives.
#[cfg(windows)]
fn strip_drive_slash(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() > 2 && b[0] == b'/' && b[2] == b':' && b[1].is_ascii_alphabetic() {
        &s[1..]
    } else {
        s
    }
}

/// No drives off Windows: the remainder survives verbatim.
#[cfg(not(windows))]
fn strip_drive_slash(s: &str) -> &str {
    s
}

/// A `file:` URI's local path from its scheme-stripped remainder,
/// shared by the read/write/`file` routes: authority, drive slash,
/// separators, escapes, in that order (see [`uri_file_path`]).
fn file_uri_local_path(rest: &str) -> String {
    percent_decode(&separators(strip_drive_slash(strip_authority(rest))))
}

/// `/` becomes the platform separator (a Unix no-op scan —
/// branchless, so every line covers on every platform).
fn separators(s: &str) -> String {
    s.replace('/', std::path::MAIN_SEPARATOR_STR)
}

/// `%XX` decoding with `+` literal (upstream
/// `escaped-utf8-urlstring->str` shields `+` first); malformed
/// `%` stays literal (D5 — the JDK throws `IAE` there).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(b) = hex_pair(bytes, i) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The decoded byte at `%XX` (`None` when short or non-hex).
fn hex_pair(bytes: &[u8], i: usize) -> Option<u8> {
    let hi = bytes.get(i + 1).and_then(hex_val)?;
    let lo = bytes.get(i + 2).and_then(hex_val)?;
    Some(hi * 16 + lo)
}

/// One hex digit's value (`None` for anything else).
fn hex_val(b: &u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parsed stream opts: `:append` truthiness plus the raw
/// `:encoding` value (validated per-arm, after the open — the
/// upstream `InputStreamReader`/`OutputStreamWriter` ctor
/// order).
struct StreamOpts {
    append: bool,
    encoding: Value,
}

/// `(apply hash-map opts)`: odd counts name the last item
/// (`str`, like the JVM), later pairs win, unknown keys are
/// ignored.
fn parse_opts(opts: &[Value]) -> Result<StreamOpts, NativeError> {
    if opts.len() % 2 == 1 {
        let last = &opts[opts.len() - 1];
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No value supplied for key: {}", str_of(last)),
        ));
    }
    let mut out = StreamOpts {
        append: false,
        encoding: Value::Nil,
    };
    for pair in opts.chunks_exact(2) {
        let name = match &pair[0] {
            Value::Keyword(kw) if kw.namespace().is_none() => kw.name(),
            _ => continue,
        };
        match name {
            "append" => out.append = pair[1].is_truthy(),
            "encoding" => out.encoding = pair[1].clone(),
            _ => {}
        }
    }
    Ok(out)
}

/// The `:encoding` charset: nil/false default to UTF-8 (upstream
/// `(or (:encoding opts) "UTF-8")`), non-strings hit the
/// `^String` CCE, unknown names the UEE.
fn resolve_charset(v: &Value) -> Result<Charset, NativeError> {
    if matches!(v, Value::Nil | Value::Bool(false)) {
        return lookup_charset("UTF-8");
    }
    let Value::Str(name) = v else {
        return Err(cce_err(v, "java.lang.String"));
    };
    lookup_charset(name)
}

/// Flat byte arrays as bytes (`None` for anything else —
/// multi-dim arrays ride `Arrays` stores, so the store match
/// alone excludes them, exactly like upstream).
fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    let Value::Array(a) = v else {
        return None;
    };
    match &*a.lock() {
        ArrayStore::Bytes(v) => Some(v.iter().map(|b| *b as u8).collect()),
        _ => None,
    }
}

/// Flat char arrays as a string (`None` for anything else).
fn chars_of(v: &Value) -> Option<String> {
    let Value::Array(a) = v else {
        return None;
    };
    match &*a.lock() {
        ArrayStore::Chars(v) => Some(v.iter().collect()),
        _ => None,
    }
}

/// A stream fn's string input: `file:` URIs resolve first
/// (upstream `RT/toUrl`, minus the network — D2), anything
/// else opens literally.
fn string_path(s: &str) -> String {
    match strip_file_scheme(s) {
        Some(rest) => file_uri_local_path(rest),
        None => s.to_owned(),
    }
}

/// A path's raw bytes (missing names the path, like the JVM).
fn read_path_bytes(path: &str) -> Result<Vec<u8>, NativeError> {
    std::fs::read(path).map_err(|e| file_err(path, e))
}

/// Open a path for output (creating, truncating or appending).
fn open_output(path: &str, append: bool) -> Result<std::fs::File, NativeError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    opts.open(path).map_err(|e| file_err(path, e))
}

/// The bytes of an open input stream (closed reports the JDK
/// text — the facsimile reads eagerly, so the wrap is the
/// read).
fn open_input_bytes(s: &JioInputStream) -> Result<Arc<[u8]>, NativeError> {
    let st = s.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.closed {
        return Err(stream_closed());
    }
    Ok(Arc::clone(&st.bytes))
}

/// A writer over an open output stream: a fresh append handle
/// on the same path (D4 — its close never propagates back).
fn writer_over_output(
    eval: &mut Evaluator,
    s: &JioOutputStream,
    cs: Charset,
) -> Result<Value, NativeError> {
    let closed = s.state.lock().unwrap_or_else(|e| e.into_inner()).closed;
    if closed {
        return Err(stream_closed());
    }
    let path: &str = &s.path;
    let file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| file_err(path, e))?;
    Ok(file_writer(eval, file, cs))
}

/// `File.delete`: missing paths fail, plain dirs go too (like
/// the JVM — symlinks delete as links, via `symlink_metadata`),
/// anything else failing reports false.
fn delete_path(path: &str) -> bool {
    let is_plain_dir = std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false);
    if is_plain_dir {
        std::fs::remove_dir(path).is_ok()
    } else {
        std::fs::remove_file(path).is_ok()
    }
}

/// The `Cannot open <x>` IAE: nil takes the `nil_kind`
/// (`Reader`/`Writer` — the upstream nil extends), anything
/// else the routed kind (`InputStream`/`OutputStream` — the
/// upstream `default-streams-impl` fallthrough).
fn cannot_open(x: &Value, nil_kind: &str, route_kind: &str) -> NativeError {
    if matches!(x, Value::Nil) {
        cannot_open_routed(x, nil_kind)
    } else {
        cannot_open_routed(x, route_kind)
    }
}

/// The routed `Cannot open <x>` IAE (one text for nil and
/// beyond — upstream `default-streams-impl`).
fn cannot_open_routed(x: &Value, kind: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Cannot open <{}> as {kind}.", pr_of(x)),
    )
}

/// `reader`: readers pass through (opts ignored, like the
/// upstream `Reader` arm), input streams and byte arrays decode
/// with `:encoding`, char arrays wrap directly, paths open;
/// URIs route through `as-file` (D2). nil is the `Reader`
/// text; anything else falls through to the `InputStream` text.
fn native_reader(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.java.io/reader", 0));
    }
    let opts = parse_opts(&args[1..])?;
    reader_of(eval, &args[0], &opts)
}

/// Dispatch one `reader` input (split for the complexity gate).
fn reader_of(eval: &mut Evaluator, x: &Value, opts: &StreamOpts) -> Result<Value, NativeError> {
    if as_reader(x).is_some() {
        return Ok(x.clone());
    }
    if let Some(s) = as_input_stream(x) {
        let bytes = open_input_bytes(s)?;
        let text = decode(resolve_charset(&opts.encoding)?, &bytes);
        return Ok(string_reader(eval, &text));
    }
    if let Some(bytes) = bytes_of(x) {
        let text = decode(resolve_charset(&opts.encoding)?, &bytes);
        return Ok(string_reader(eval, &text));
    }
    if let Some(text) = chars_of(x) {
        return Ok(string_reader(eval, &text));
    }
    if let Value::Str(s) = x {
        return reader_of_path(eval, &string_path(s), opts);
    }
    if let Some(path) = file_path(x) {
        return reader_of_path(eval, &path, opts);
    }
    if let Value::Uri(u) = x {
        return reader_of_path(eval, &uri_file_path(u.text())?, opts);
    }
    Err(cannot_open(x, "a Reader", "an InputStream"))
}

/// Open a path for reading: bytes first (missing names the
/// path), then the charset — the upstream `FileInputStream` /
/// `InputStreamReader` ctor order.
fn reader_of_path(
    eval: &mut Evaluator,
    path: &str,
    opts: &StreamOpts,
) -> Result<Value, NativeError> {
    let bytes = read_path_bytes(path)?;
    let text = decode(resolve_charset(&opts.encoding)?, &bytes);
    Ok(string_reader(eval, &text))
}

/// `writer`: writers pass through (opts ignored), output
/// streams take a fresh append handle (D4), paths open
/// (creating); non-file URIs are the upstream `Can not write`
/// text. nil is the `Writer` text; anything else falls through
/// to the `OutputStream` text.
fn native_writer(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.java.io/writer", 0));
    }
    let opts = parse_opts(&args[1..])?;
    writer_of(eval, &args[0], &opts)
}

/// Dispatch one `writer` input (split for the complexity gate).
fn writer_of(eval: &mut Evaluator, x: &Value, opts: &StreamOpts) -> Result<Value, NativeError> {
    if as_writer(x).is_some() {
        return Ok(x.clone());
    }
    if let Some(s) = as_output_stream(x) {
        return writer_over_output(eval, s, resolve_charset(&opts.encoding)?);
    }
    if let Value::Str(s) = x {
        return writer_of_path(eval, &string_path(s), opts);
    }
    if let Some(path) = file_path(x) {
        return writer_of_path(eval, &path, opts);
    }
    if let Value::Uri(u) = x {
        return writer_of_path(eval, &uri_output_path(u.text())?, opts);
    }
    Err(cannot_open(x, "a Writer", "an OutputStream"))
}

/// Open a path for writing: the file first (creating), then the
/// charset — the upstream `FileOutputStream` /
/// `OutputStreamWriter` ctor order (a bad charset still leaves
/// the truncated file behind, like the JVM).
fn writer_of_path(
    eval: &mut Evaluator,
    path: &str,
    opts: &StreamOpts,
) -> Result<Value, NativeError> {
    let file = open_output(path, opts.append)?;
    let cs = resolve_charset(&opts.encoding)?;
    Ok(file_writer(eval, file, cs))
}

/// A write-side URI path: `file:` URIs decode, anything else is
/// the upstream `Can not write to non-file URL` text (two
/// words — verbatim).
fn uri_output_path(u: &str) -> Result<String, NativeError> {
    match strip_file_scheme(u) {
        Some(rest) => Ok(file_uri_local_path(rest)),
        None => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Can not write to non-file URL <{u}>"),
        )),
    }
}

/// `input-stream`: streams pass through, byte arrays wrap,
/// paths open; URIs route through `as-file` (D2 — the upstream
/// network open is refused as `Not a file`). `:encoding` never
/// consults (upstream neither — only the odd-count check runs).
fn native_input_stream(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.java.io/input-stream", 0));
    }
    parse_opts(&args[1..])?;
    input_stream_of(eval, &args[0])
}

/// Dispatch one `input-stream` input (split for the complexity
/// gate).
fn input_stream_of(eval: &mut Evaluator, x: &Value) -> Result<Value, NativeError> {
    if as_input_stream(x).is_some() {
        return Ok(x.clone());
    }
    if let Some(bytes) = bytes_of(x) {
        return Ok(input_stream_value(eval, bytes));
    }
    if let Value::Str(s) = x {
        return Ok(input_stream_value(eval, read_path_bytes(&string_path(s))?));
    }
    if let Some(path) = file_path(x) {
        return Ok(input_stream_value(eval, read_path_bytes(&path)?));
    }
    if let Value::Uri(u) = x {
        let path = uri_file_path(u.text())?;
        return Ok(input_stream_value(eval, read_path_bytes(&path)?));
    }
    Err(cannot_open_routed(x, "an InputStream"))
}

/// `output-stream`: streams pass through, paths open (creating,
/// honoring `:append`); non-file URIs are the upstream
/// `Can not write` text.
fn native_output_stream(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.java.io/output-stream", 0));
    }
    let opts = parse_opts(&args[1..])?;
    output_stream_of(eval, &args[0], &opts)
}

/// Dispatch one `output-stream` input (split for the complexity
/// gate).
fn output_stream_of(
    eval: &mut Evaluator,
    x: &Value,
    opts: &StreamOpts,
) -> Result<Value, NativeError> {
    if as_output_stream(x).is_some() {
        return Ok(x.clone());
    }
    if let Value::Str(s) = x {
        let path = string_path(s);
        return Ok(output_stream_value(
            eval,
            &path,
            open_output(&path, opts.append)?,
        ));
    }
    if let Some(path) = file_path(x) {
        return Ok(output_stream_value(
            eval,
            &path,
            open_output(&path, opts.append)?,
        ));
    }
    if let Value::Uri(u) = x {
        let path = uri_output_path(u.text())?;
        return Ok(output_stream_value(
            eval,
            &path,
            open_output(&path, opts.append)?,
        ));
    }
    Err(cannot_open_routed(x, "an OutputStream"))
}

/// `file`: 1-arity coerces (`as-file` — nil stays nil),
/// multi-arity joins the parent with each `as-relative-path`
/// child in turn (upstream `reduce`).
fn native_file(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.java.io/file", 0));
    }
    if args.len() == 1 {
        return match as_file(&args[0])? {
            Some(path) => Ok(jdk_file(eval, &path)),
            None => Ok(Value::Nil),
        };
    }
    let parent = as_file(&args[0])?;
    let mut acc = join_path(parent.as_deref(), &as_relative_path(&args[1])?);
    for child in &args[2..] {
        acc = join_path(Some(&acc), &as_relative_path(child)?);
    }
    Ok(jdk_file(eval, &acc))
}

/// `delete-file`: `(or (.delete (file f)) silently (throw))` —
/// nil hits the `delete` NPE, failures answer the truthy
/// `silently` value or throw `Couldn't delete` naming the
/// original arg.
fn native_delete_file(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.java.io/delete-file", args, &[1, 2])?;
    let path = match as_file(&args[0])? {
        None => {
            return Err(NativeError::new(
                ThrowClass::NullPointerException,
                "Cannot invoke \"java.io.File.delete()\" because \"f\" is null".to_owned(),
            ));
        }
        Some(path) => path,
    };
    if delete_path(&path) {
        return Ok(Value::Bool(true));
    }
    if args.len() == 2 && args[1].is_truthy() {
        return Ok(args[1].clone());
    }
    Err(NativeError::new(
        ThrowClass::IOException,
        format!("Couldn't delete {}", str_of(&args[0])),
    ))
}

/// The six facsimile natives installed into `clojure.java.io`.
pub(crate) const JIO_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("file", native_file),
    ("reader", native_reader),
    ("writer", native_writer),
    ("input-stream", native_input_stream),
    ("output-stream", native_output_stream),
    ("delete-file", native_delete_file),
];
