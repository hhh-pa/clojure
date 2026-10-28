// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S17 JDK/IO adapters: opaque `Enumeration`/`Iterator`/`Stream`/
//! `ResultSet`/`File` handles plus `enumeration-seq`,
//! `iterator-seq`, `stream-seq!`, `stream-reduce!`,
//! `stream-transduce!`, `stream-into!`, `resultset-seq`, and
//! `file-seq` (`xml-seq` is a `core.clj` port).
//!
//! There is no JVM to hand the port a handle, so embedding
//! Rust code builds them through the `pub` constructors
//! below — the only construction surface — and Clojure code
//! consumes what it is given. Oracle:
//! `docs/zero-oos-s17-jdkio/probes/jdkio.out`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use clj_value::{ConsValue, ExtensionValue, Keyword, RealizedSeq, Seq, ThrowClass, Value, new_map};

use crate::core::{as_reduced, cce_err};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::{Thunk, pending_value};
use crate::ns::{NativeFnPtr, check_arity, deref_core, str_of};
use crate::seqs::call_checked;

/// A null deref with the oracle's verbatim message.
fn npe(message: &str) -> NativeError {
    NativeError::new(ThrowClass::NullPointerException, message.to_owned())
}

/// A second terminal op on a stream (the exact JDK text,
/// oracle `b-reuse`).
fn stream_reuse() -> NativeError {
    NativeError::new(
        ThrowClass::IllegalStateException,
        "stream has already been operated upon or closed".to_owned(),
    )
}

/// Shared pull state: items plus the advanced position.
#[derive(Debug)]
pub(crate) struct ItemsState {
    items: Arc<[Value]>,
    pos: usize,
}

/// An emulated `java.util.Enumeration`: identity plus shared
/// pull state. Built only by [`enumeration`].
#[derive(Debug)]
pub struct JdkEnum {
    id: u64,
    state: Arc<Mutex<ItemsState>>,
}

impl ExtensionValue for JdkEnum {
    fn class_name(&self) -> String {
        "java.util.Enumeration".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.util.Enumeration 0x{:x} \"java.util.Enumeration@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JdkEnum>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An emulated `java.util.Iterator`. Built only by
/// [`iterator`].
#[derive(Debug)]
pub struct JdkIter {
    id: u64,
    state: Arc<Mutex<ItemsState>>,
}

impl ExtensionValue for JdkIter {
    fn class_name(&self) -> String {
        "java.util.Iterator".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.util.Iterator 0x{:x} \"java.util.Iterator@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JdkIter>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Shared stream state: items, the advanced position, and the
/// terminal-op latch.
#[derive(Debug)]
pub(crate) struct StreamState {
    items: Arc<[Value]>,
    pos: usize,
    consumed: bool,
}

/// An emulated `java.util.stream.Stream`: single-use (every
/// terminal op latches `consumed`). Built only by [`stream`].
#[derive(Debug)]
pub struct JdkStream {
    id: u64,
    state: Arc<Mutex<StreamState>>,
}

impl ExtensionValue for JdkStream {
    fn class_name(&self) -> String {
        "java.util.stream.Stream".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.util.stream.Stream 0x{:x} \"java.util.stream.Stream@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JdkStream>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
/// Shared row state: label-width rows plus the advanced row.
#[derive(Debug)]
pub(crate) struct RowsState {
    rows: Arc<[Arc<[Value]>]>,
    pos: usize,
}

/// An emulated `java.sql.ResultSet`: column labels plus
/// shared row state. Built only by [`result_set`].
#[derive(Debug)]
pub struct JdkResultSet {
    id: u64,
    labels: Arc<[String]>,
    state: Arc<Mutex<RowsState>>,
}

impl ExtensionValue for JdkResultSet {
    fn class_name(&self) -> String {
        "java.sql.ResultSet".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.sql.ResultSet 0x{:x} \"java.sql.ResultSet@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JdkResultSet>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An emulated `java.io.File`: identity plus its path.
/// Immutable (there is no cursor to share). Built only by
/// [`file`].
#[derive(Debug)]
pub struct JdkFile {
    id: u64,
    path: Arc<str>,
}

impl ExtensionValue for JdkFile {
    fn class_name(&self) -> String {
        "java.io.File".to_owned()
    }

    fn print(&self) -> String {
        format!("#object[java.io.File 0x{:x} \"{}\"]", self.id, self.path)
    }

    fn print_str(&self) -> Option<String> {
        Some(self.path.to_string())
    }

    // Path equality like `java.io.File.equals` (the other
    // handles keep identity — their JVM types override nothing).
    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JdkFile>()
            .is_some_and(|o| o.path == self.path)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write(self.path.as_bytes());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to an emulated enumeration, if it is one.
fn as_enum(v: &Value) -> Option<&JdkEnum> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JdkEnum>(),
        _ => None,
    }
}

/// Downcast a value to an emulated iterator, if it is one.
fn as_iter(v: &Value) -> Option<&JdkIter> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JdkIter>(),
        _ => None,
    }
}

/// Downcast a value to an emulated stream, if it is one.
fn as_stream(v: &Value) -> Option<&JdkStream> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JdkStream>(),
        _ => None,
    }
}

/// Downcast a value to an emulated result set, if it is one.
fn as_result_set(v: &Value) -> Option<&JdkResultSet> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JdkResultSet>(),
        _ => None,
    }
}

/// Downcast a value to an emulated file, if it is one.
fn as_file(v: &Value) -> Option<&JdkFile> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JdkFile>(),
        _ => None,
    }
}

/// An emulated `java.util.Enumeration` over `items` (the only
/// construction surface — Clojure code consumes handles it is
/// given).
#[must_use]
pub fn enumeration(eval: &mut Evaluator, items: Vec<Value>) -> Value {
    Value::Extension(Arc::new(JdkEnum {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(ItemsState {
            items: Arc::from(items),
            pos: 0,
        })),
    }))
}

/// An emulated `java.util.Iterator` over `items`.
#[must_use]
pub fn iterator(eval: &mut Evaluator, items: Vec<Value>) -> Value {
    Value::Extension(Arc::new(JdkIter {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(ItemsState {
            items: Arc::from(items),
            pos: 0,
        })),
    }))
}

/// An emulated `java.util.stream.Stream` over `items`
/// (single-use: the first terminal op latches it).
#[must_use]
pub fn stream(eval: &mut Evaluator, items: Vec<Value>) -> Value {
    Value::Extension(Arc::new(JdkStream {
        id: eval.next_id(),
        state: Arc::new(Mutex::new(StreamState {
            items: Arc::from(items),
            pos: 0,
            consumed: false,
        })),
    }))
}

/// An emulated `java.sql.ResultSet`: `labels` are the column
/// labels (lowercased to keywords per row), `rows` the row
/// values. Short rows read nil past their end, long rows
/// truncate (the embedding owns well-formedness; the native
/// stays total either way).
#[must_use]
pub fn result_set(eval: &mut Evaluator, labels: Vec<String>, rows: Vec<Vec<Value>>) -> Value {
    let width = labels.len();
    let mut fixed = Vec::with_capacity(rows.len());
    for mut row in rows {
        row.resize(width, Value::Nil);
        fixed.push(Arc::from(row));
    }
    Value::Extension(Arc::new(JdkResultSet {
        id: eval.next_id(),
        labels: Arc::from(labels),
        state: Arc::new(Mutex::new(RowsState {
            rows: Arc::from(fixed),
            pos: 0,
        })),
    }))
}

/// An emulated `java.io.File` naming `path`.
#[must_use]
pub fn file(eval: &mut Evaluator, path: &str) -> Value {
    Value::Extension(Arc::new(JdkFile {
        id: eval.next_id(),
        path: Arc::from(path),
    }))
}

/// Items (or rows) still pullable from a handle (`None` for
/// files and non-handles). Test-only: pins pull counts.
#[must_use]
pub fn remaining(v: &Value) -> Option<usize> {
    if let Some(h) = as_enum(v) {
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        return Some(st.items.len().saturating_sub(st.pos));
    }
    if let Some(h) = as_iter(v) {
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        return Some(st.items.len().saturating_sub(st.pos));
    }
    if let Some(h) = as_stream(v) {
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        return Some(st.items.len().saturating_sub(st.pos));
    }
    if let Some(h) = as_result_set(v) {
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        return Some(st.rows.len().saturating_sub(st.pos));
    }
    None
}

/// Whether a stream handle has latched (`None` for
/// non-streams). Test-only: pins single-use.
#[must_use]
pub fn stream_consumed(v: &Value) -> Option<bool> {
    as_stream(v).map(|h| h.state.lock().unwrap_or_else(|e| e.into_inner()).consumed)
}

/// One directory listing in a file walk: entries in
/// `read_dir` order plus the advanced index.
#[derive(Clone, Debug)]
pub(crate) struct FileFrame {
    entries: Arc<[Arc<str>]>,
    idx: usize,
}

/// A shared pull cursor behind `Thunk::JdkPull`: item and
/// stream handles share their handle's cell (forcing
/// advances the handle); row cursors share the row cell;
/// file walks own their stack (`root` is `Some` until the
/// first pull checks and emits it — the lazy `tree-seq`
/// error shape).
#[derive(Clone, Debug)]
pub(crate) enum JdkCursor {
    Items {
        state: Arc<Mutex<ItemsState>>,
    },
    Stream {
        state: Arc<Mutex<StreamState>>,
    },
    Rows {
        labels: Arc<[Keyword]>,
        state: Arc<Mutex<RowsState>>,
    },
    Files {
        root: Option<Value>,
        stack: Vec<FileFrame>,
    },
}

/// A directory's children in `read_dir` order (like the
/// JVM's `listFiles` order: OS-defined). Failures answer no
/// children — the JVM's null return is silent.
fn list_dir(path: &str) -> Vec<Arc<str>> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(path) else {
        return out;
    };
    for entry in read {
        let Ok(entry) = entry else { continue };
        out.push(Arc::from(entry.path().to_string_lossy().as_ref()));
    }
    out
}

/// One row as a column-order map (structmaps print exactly as
/// maps — oracle `c-rows`).
fn row_map(labels: &[Keyword], row: &[Value]) -> Value {
    let mut map = new_map();
    for (label, cell) in labels.iter().zip(row.iter()) {
        map = map.insert(Value::Keyword(label.clone()), cell.clone());
    }
    Value::Map(map)
}

/// Pull up to `n` values through a cursor (fewer at the end;
/// empty only when exhausted). File pulls mint child `File`
/// values from the evaluator sequence.
pub(crate) fn pull_cursor(
    eval: &mut Evaluator,
    cursor: &mut JdkCursor,
    n: usize,
) -> Result<Vec<Value>, NativeError> {
    match cursor {
        JdkCursor::Items { state } => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            let end = (st.pos + n).min(st.items.len());
            let out = st.items[st.pos..end].to_vec();
            st.pos = end;
            Ok(out)
        }
        JdkCursor::Stream { state } => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            let end = (st.pos + n).min(st.items.len());
            let out = st.items[st.pos..end].to_vec();
            st.pos = end;
            Ok(out)
        }
        JdkCursor::Rows { labels, state } => {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            let end = (st.pos + n).min(st.rows.len());
            let mut out = Vec::with_capacity(end - st.pos);
            for row in &st.rows[st.pos..end] {
                out.push(row_map(labels, row));
            }
            st.pos = end;
            Ok(out)
        }
        JdkCursor::Files { root, stack } => pull_files(eval, root, stack, n),
    }
}

/// Pull up to `n` nodes of a preorder file walk. The root
/// checks (and emits) first; directories push their listing
/// ahead of their siblings.
fn pull_files(
    eval: &mut Evaluator,
    root: &mut Option<Value>,
    stack: &mut Vec<FileFrame>,
    n: usize,
) -> Result<Vec<Value>, NativeError> {
    let mut out = Vec::new();
    if let Some(node) = root.take() {
        out.push(check_file_root(&node)?);
        push_children(stack, &node);
    }
    while out.len() < n {
        let Some(frame) = stack.last_mut() else {
            break;
        };
        if frame.idx >= frame.entries.len() {
            stack.pop();
            continue;
        }
        let path = frame.entries[frame.idx].clone();
        frame.idx += 1;
        let node = file(eval, &path);
        push_children(stack, &node);
        out.push(node);
    }
    Ok(out)
}

/// The lazy root check: nil is the oracle NPE, non-files the
/// oracle CCE (both fire on first force, like `tree-seq`).
fn check_file_root(node: &Value) -> Result<Value, NativeError> {
    if matches!(node, Value::Nil) {
        return Err(npe(
            "Cannot invoke \"java.io.File.isDirectory()\" because \"f\" is null",
        ));
    }
    if as_file(node).is_none() {
        return Err(cce_err(node, "java.io.File"));
    }
    Ok(node.clone())
}

/// Check a terminal-op stream argument: nil is the oracle
/// NPE (naming `site`), non-streams the oracle CCE, latched
/// streams the reuse error — then latch it. Answers the
/// shared state.
fn check_stream_arg(s: &Value, site: &str) -> Result<Arc<Mutex<StreamState>>, NativeError> {
    if matches!(s, Value::Nil) {
        return Err(npe(&format!(
            "Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"{site}\" is null"
        )));
    }
    let Some(h) = as_stream(s) else {
        return Err(cce_err(s, "java.util.stream.BaseStream"));
    };
    let mut st = h.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.consumed {
        return Err(stream_reuse());
    }
    st.consumed = true;
    Ok(Arc::clone(&h.state))
}

/// Pull one stream item (the terminal-op loop unit).
fn pull_one_stream(state: &Arc<Mutex<StreamState>>) -> Option<Value> {
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    if st.pos >= st.items.len() {
        return None;
    }
    let out = st.items[st.pos].clone();
    st.pos += 1;
    Some(out)
}

/// `iterator-reduce!` over pulled stream items: `None` init
/// pulls the first item (empty calls `(f)`); `reduced`
/// unwraps.
fn stream_reduce_core(
    eval: &mut Evaluator,
    f: &Value,
    init: Option<Value>,
    state: &Arc<Mutex<StreamState>>,
) -> Result<Value, NativeError> {
    let mut acc = match init {
        Some(v) => v,
        None => match pull_one_stream(state) {
            Some(v) => v,
            None => return call_checked(eval, f, &[]),
        },
    };
    while let Some(x) = pull_one_stream(state) {
        acc = call_checked(eval, f, &[acc, x])?;
        if let Some(inner) = as_reduced(&acc) {
            return Ok(inner.clone());
        }
    }
    Ok(acc)
}

/// `stream-transduce!` over pulled stream items: the xform
/// applies first, `reduced` unwraps mid-loop, the completion
/// always runs.
fn stream_transduce_core(
    eval: &mut Evaluator,
    xform: &Value,
    f: &Value,
    init: Value,
    s: &Value,
) -> Result<Value, NativeError> {
    let step = call_checked(eval, xform, std::slice::from_ref(f))?;
    let state = check_stream_arg(s, "s")?;
    let mut acc = init;
    while let Some(x) = pull_one_stream(&state) {
        acc = call_checked(eval, &step, &[acc, x])?;
        if let Some(inner) = as_reduced(&acc) {
            acc = inner.clone();
            break;
        }
    }
    call_checked(eval, &step, &[acc])
}

/// Push a node's children when it is a directory (silently
/// none otherwise — the JVM's null `listFiles`).
fn push_children(stack: &mut Vec<FileFrame>, node: &Value) {
    let Some(f) = as_file(node) else {
        return;
    };
    let path: &str = &f.path;
    if !std::path::Path::new(path).is_dir() {
        return;
    }
    stack.push(FileFrame {
        entries: Arc::from(list_dir(path)),
        idx: 0,
    });
}

/// `enumeration-seq`: strict spine (the JVM's `EnumerationSeq`
/// is not `IPending`). Empty-at-construction answers nil;
/// otherwise the remaining items answer as one realized seq.
fn native_enumeration_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/enumeration-seq", args, &[1])?;
    if matches!(args[0], Value::Nil) {
        return Err(npe(
            "Cannot invoke \"java.util.Enumeration.hasMoreElements()\" because \"iter\" is null",
        ));
    }
    let Some(h) = as_enum(&args[0]) else {
        return Err(cce_err(&args[0], "java.util.Enumeration"));
    };
    let mut st = h.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.pos >= st.items.len() {
        return Ok(Value::Nil);
    }
    let rest = st.items[st.pos..].to_vec();
    st.pos = st.items.len();
    Ok(Value::Extension(Arc::new(RealizedSeq::new(Seq::Eager {
        items: Arc::from(rest),
        pos: 0,
    }))))
}

/// `iterator-seq`: lazy chunked-32 seq over the shared
/// handle. Empty-at-construction answers nil.
fn native_iterator_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/iterator-seq", args, &[1])?;
    if matches!(args[0], Value::Nil) {
        return Err(npe(
            "Cannot invoke \"java.util.Iterator.hasNext()\" because \"iter\" is null",
        ));
    }
    let Some(h) = as_iter(&args[0]) else {
        return Err(cce_err(&args[0], "java.util.Iterator"));
    };
    {
        let st = h.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.pos >= st.items.len() {
            return Ok(Value::Nil);
        }
    }
    Ok(pending_value(Thunk::JdkPull {
        cursor: JdkCursor::Items {
            state: Arc::clone(&h.state),
        },
        chunked: true,
    }))
}

/// `stream-seq!`: terminal `iterator-seq` — latches the
/// stream at construction, then pulls chunked-32.
fn native_stream_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/stream-seq!", args, &[1])?;
    let state = check_stream_arg(&args[0], "stream")?;
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    if st.pos >= st.items.len() {
        return Ok(Value::Nil);
    }
    drop(st);
    Ok(pending_value(Thunk::JdkPull {
        cursor: JdkCursor::Stream { state },
        chunked: true,
    }))
}

/// `stream-reduce!`: 2 + 3 arities over `iterator-reduce!`.
fn native_stream_reduce(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/stream-reduce!", args, &[2, 3])?;
    let (f, init, s) = if args.len() == 3 {
        (&args[0], Some(args[1].clone()), &args[2])
    } else {
        (&args[0], None, &args[1])
    };
    let state = check_stream_arg(s, "s")?;
    stream_reduce_core(eval, f, init, &state)
}

/// `stream-transduce!`: 3 + 4 arities; 3-arity evaluates
/// `(f)` for init before applying the xform.
fn native_stream_transduce(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/stream-transduce!", args, &[3, 4])?;
    if args.len() == 4 {
        return stream_transduce_core(eval, &args[0], &args[1], args[2].clone(), &args[3]);
    }
    let init = call_checked(eval, &args[1], &[])?;
    stream_transduce_core(eval, &args[0], &args[1], init, &args[2])
}

/// `stream-into!`: 2 + 3 arities mirroring `into_xform`'s
/// editable dispatch (transients throw at the `(transient
/// to)` call, exactly like `into`).
fn native_stream_into(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/stream-into!", args, &[2, 3])?;
    let to = args[0].clone();
    // `IEditableCollection` ≈ persistent vector/map/set — plus
    // transients, which take the `(transient to)` call below
    // and throw there, exactly like the JVM (mirrors
    // `into_xform`).
    let editable = matches!(
        to,
        Value::Vector(_) | Value::Map(_) | Value::Set(_) | Value::Transient(_)
    );
    if !editable {
        if args.len() == 3 {
            let s = args[2].clone();
            let conj = deref_core(eval, "conj");
            return stream_transduce_core(eval, &args[1], &conj, to, &s);
        }
        let s = args[1].clone();
        let conj = deref_core(eval, "conj");
        let state = check_stream_arg(&s, "s")?;
        return stream_reduce_core(eval, &conj, Some(to), &state);
    }
    let transient_to = call_checked(eval, &deref_core(eval, "transient"), &[to])?;
    if args.len() == 3 {
        let s = args[2].clone();
        let conj_bang = deref_core(eval, "conj!");
        let reduced = stream_transduce_core(eval, &args[1], &conj_bang, transient_to, &s)?;
        return call_checked(eval, &deref_core(eval, "persistent!"), &[reduced]);
    }
    let s = args[1].clone();
    let conj_bang = deref_core(eval, "conj!");
    let state = check_stream_arg(&s, "s")?;
    let reduced = stream_reduce_core(eval, &conj_bang, Some(transient_to), &state)?;
    call_checked(eval, &deref_core(eval, "persistent!"), &[reduced])
}

/// Lowercased column labels as keywords, duplicate-checked
/// (the JVM's eager `Exception` — oracle `c-dupkeys`).
#[allow(clippy::mutable_key_type)]
fn result_labels(labels: &[String]) -> Result<Vec<Keyword>, NativeError> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let key = Keyword::new(None, &label.to_lowercase());
        if !seen.insert(Value::Keyword(key.clone())) {
            return Err(NativeError::new(
                ThrowClass::Exception,
                "ResultSet must have unique column labels".to_owned(),
            ));
        }
        out.push(key);
    }
    Ok(out)
}

/// `resultset-seq`: eager head (nil when rowless), lazy
/// one-row-per-step tail.
fn native_resultset_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/resultset-seq", args, &[1])?;
    if matches!(args[0], Value::Nil) {
        return Err(npe(
            "Cannot invoke \"java.sql.ResultSet.getMetaData()\" because \"rs\" is null",
        ));
    }
    let Some(h) = as_result_set(&args[0]) else {
        return Err(cce_err(&args[0], "java.sql.ResultSet"));
    };
    let labels = result_labels(&h.labels)?;
    let mut st = h.state.lock().unwrap_or_else(|e| e.into_inner());
    if st.pos >= st.rows.len() {
        return Ok(Value::Nil);
    }
    let head = row_map(&labels, &st.rows[st.pos]);
    st.pos += 1;
    let tail = pending_value(Thunk::JdkPull {
        cursor: JdkCursor::Rows {
            labels: Arc::from(labels),
            state: Arc::clone(&h.state),
        },
        chunked: false,
    });
    Ok(Value::Extension(Arc::new(ConsValue::new(head, tail))))
}

/// `file-seq`: lazy preorder walk. File handles ride the
/// pull cursor (the root is NOT checked here — nil throws
/// the oracle NPE on first force, like `tree-seq`);
/// string roots walk as paths (the S12 §1.7 divergence
/// survives the wave-2 union — strings stay valid roots,
/// so the JVM's CCE-on-string has no live call site).
/// Other roots keep S12's eager slurp-shaped IAE.
fn native_file_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/file-seq", args, &[1])?;
    // S12 union: string roots walk (strings in and out).
    if let Value::Str(path) = &args[0] {
        return Ok(pending_value(Thunk::FileSeq {
            stack: vec![path.to_string()],
        }));
    }
    // S12 union: non-string, non-File, non-nil roots throw
    // the eager slurp-shaped IAE (S12 pins `(file-seq 42)`;
    // S17 pins no other-type root).
    if !matches!(args[0], Value::Nil) && as_file(&args[0]).is_none() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot open <{}> as a directory.", str_of(&args[0])),
        ));
    }
    Ok(pending_value(Thunk::JdkPull {
        cursor: JdkCursor::Files {
            root: Some(args[0].clone()),
            stack: Vec::new(),
        },
        chunked: false,
    }))
}

/// The S17 natives installed into `clojure.core` (`xml-seq`
/// ships as a `core.clj` port instead).
pub(crate) const JDKIO_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("enumeration-seq", native_enumeration_seq),
    ("iterator-seq", native_iterator_seq),
    ("stream-seq!", native_stream_seq),
    ("stream-reduce!", native_stream_reduce),
    ("stream-transduce!", native_stream_transduce),
    ("stream-into!", native_stream_into),
    ("resultset-seq", native_resultset_seq),
    ("file-seq", native_file_seq),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_children_ignores_non_files() {
        // Walks only ever push `File` values (the root is
        // checked first), so the silent-ignore arm is covered
        // directly.
        let mut stack = Vec::new();
        push_children(&mut stack, &Value::Int(1));
        assert!(stack.is_empty());
    }
}
