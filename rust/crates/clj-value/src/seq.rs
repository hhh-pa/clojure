// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Seqs (S3, C4): lazy, chunked (32), nil-punned sequences over
//! collections (`docs/t1174/probes/probe18.clj`).
//!
//! [`Seq::first`]/[`Seq::next`]/[`Seq::more`] mirror the JVM
//! (`next` yields `None` at the end, `more` the empty seq).
//! [`Seq::chunk_next`] exposes the current chunk plus the seq after
//! it, so chunk-aware consumers (S5's `map`/`filter`, the test
//! helpers) realize whole chunks exactly like the JVM. [`Seq::lazy`]
//! memoizes through [`OnceLock`], so thunks run once even under
//! contention, like `synchronized` `LazySeq`.

use std::sync::{Arc, Mutex, OnceLock};

use crate::{List, Map, Set, Value, Vector};

/// The JVM chunk size: 32 items per chunk.
pub const CHUNK_SIZE: usize = 32;

/// `seq` a value into items (`None` is unseqable — callers raise
/// the ISeq coercion error). `nil` seqs to empty, strings to chars,
/// maps to `[k v]` entries, like the JVM.
#[must_use]
pub fn seq_vec(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Nil => Some(Vec::new()),
        Value::Str(s) => Some(s.chars().map(Value::Char).collect()),
        Value::List(l) => Some(l.iter().cloned().collect()),
        Value::Vector(v) => Some(v.iter().cloned().collect()),
        Value::Map(m) => Some(
            m.iter()
                .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
                .collect(),
        ),
        Value::Set(s) => Some(s.iter().cloned().collect()),
        _ => None,
    }
}

/// A sequence over values. Clones are cheap (all `Arc`-backed).
#[derive(Clone)]
pub enum Seq {
    /// Eager sequential items (lists, maps, sets, `more`-end).
    Eager { items: Arc<[Value]>, pos: usize },
    /// Chunked items (vectors, chunk-preserving ops).
    Chunked {
        chunks: Arc<[Arc<[Value]>]>,
        chunk: usize,
        pos: usize,
    },
    /// A realized chunk head plus its (possibly lazy) tail.
    ChunkCons {
        chunk: Arc<[Value]>,
        pos: usize,
        tail: Box<Seq>,
    },
    /// One head plus its tail (`None` at the end).
    Cons { head: Value, tail: Option<Box<Seq>> },
    /// A memoized thunk (`None` forces to the empty seq).
    Lazy {
        thunk: Arc<dyn Fn() -> Option<Seq> + Send + Sync>,
        memo: Arc<OnceLock<Option<Seq>>>,
    },
}

impl std::fmt::Debug for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Seq::Eager { items, pos } => f
                .debug_struct("Eager")
                .field("len", &items.len())
                .field("pos", pos)
                .finish(),
            Seq::Chunked { chunks, chunk, pos } => f
                .debug_struct("Chunked")
                .field("chunks", &chunks.len())
                .field("chunk", chunk)
                .field("pos", pos)
                .finish(),
            Seq::ChunkCons { chunk, pos, .. } => f
                .debug_struct("ChunkCons")
                .field("len", &chunk.len())
                .field("pos", pos)
                .finish(),
            Seq::Cons { head, .. } => f.debug_tuple("Cons").field(head).finish(),
            Seq::Lazy { .. } => f.write_str("Lazy(..)"),
        }
    }
}

impl Seq {
    /// The empty seq (`more` at the end).
    #[must_use]
    pub fn empty() -> Self {
        Seq::Eager {
            items: Arc::from([]),
            pos: 0,
        }
    }

    /// An eager seq, or `None` when empty (nil-punning).
    #[must_use]
    pub fn eager(items: Vec<Value>) -> Option<Self> {
        if items.is_empty() {
            None
        } else {
            Some(Seq::Eager {
                items: Arc::from(items),
                pos: 0,
            })
        }
    }

    /// A chunked seq (32-item chunks), or `None` when empty.
    #[must_use]
    pub fn chunked(items: Vec<Value>) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let chunks: Vec<Arc<[Value]>> = items
            .chunks(CHUNK_SIZE)
            .map(|c| Arc::from(c.to_vec()))
            .collect();
        Some(Seq::Chunked {
            chunks: Arc::from(chunks),
            chunk: 0,
            pos: 0,
        })
    }

    /// A realized chunk head plus its tail (empty chunks collapse to
    /// the tail, keeping chunks non-empty).
    #[must_use]
    pub fn chunk_cons(chunk: Vec<Value>, tail: Seq) -> Self {
        if chunk.is_empty() {
            return tail;
        }
        Seq::ChunkCons {
            chunk: Arc::from(chunk),
            pos: 0,
            tail: Box::new(tail),
        }
    }

    /// One head plus its tail.
    #[must_use]
    pub fn cons(head: Value, tail: Option<Seq>) -> Self {
        Seq::Cons {
            head,
            tail: tail.map(Box::new),
        }
    }

    /// A memoized thunk (runs once, even under contention).
    pub fn lazy<F>(f: F) -> Self
    where
        F: Fn() -> Option<Seq> + Send + Sync + 'static,
    {
        Seq::Lazy {
            thunk: Arc::new(f),
            memo: Arc::new(OnceLock::new()),
        }
    }

    /// Seq over a list's items (sequential, like the JVM list seq).
    #[must_use]
    pub fn from_list(l: &List) -> Option<Self> {
        Self::eager(l.iter().cloned().collect())
    }

    /// Seq over a vector's items (chunked, like the JVM).
    #[must_use]
    pub fn from_vector(v: &Vector) -> Option<Self> {
        Self::chunked(v.iter().cloned().collect())
    }

    /// Seq over a map's entries as `[k v]` vectors, in seq order.
    #[must_use]
    pub fn from_map(m: &Map) -> Option<Self> {
        Self::eager(
            m.iter()
                .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
                .collect(),
        )
    }

    /// Seq over a set's members, in seq order.
    #[must_use]
    pub fn from_set(s: &Set) -> Option<Self> {
        Self::eager(s.iter().cloned().collect())
    }

    /// Force a lazy seq (memoized); non-lazy seqs return themselves.
    fn forced(&self) -> Option<&Seq> {
        match self {
            Seq::Lazy { thunk, memo } => memo.get_or_init(|| thunk()).as_ref(),
            other => Some(other),
        }
    }

    /// Whether this seq is exhausted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self.forced() {
            None => true,
            Some(Seq::Eager { items, pos }) => *pos >= items.len(),
            Some(Seq::Chunked { .. }) => false,
            Some(Seq::ChunkCons { chunk, pos, tail }) => *pos >= chunk.len() && tail.is_empty(),
            Some(Seq::Cons { .. }) => false,
            Some(l @ Seq::Lazy { .. }) => l.is_empty(),
        }
    }

    /// The first value, if any.
    #[must_use]
    pub fn first(&self) -> Option<&Value> {
        match self.forced()? {
            Seq::Eager { items, pos } => items.get(*pos),
            Seq::Chunked { chunks, chunk, pos } => chunks.get(*chunk).and_then(|c| c.get(*pos)),
            Seq::ChunkCons { chunk, pos, tail } => chunk.get(*pos).or_else(|| tail.first()),
            Seq::Cons { head, .. } => Some(head),
            l @ Seq::Lazy { .. } => l.first(),
        }
    }

    /// The seq after the first value (`None` at the end).
    #[must_use]
    pub fn next(&self) -> Option<Seq> {
        match self.forced()? {
            Seq::Eager { items, pos } => {
                if pos + 1 < items.len() {
                    Some(Seq::Eager {
                        items: Arc::clone(items),
                        pos: pos + 1,
                    })
                } else {
                    None
                }
            }
            Seq::Chunked { chunks, chunk, pos } => {
                let len = chunks.get(*chunk).map_or(0, |c| c.len());
                if pos + 1 < len {
                    Some(Seq::Chunked {
                        chunks: Arc::clone(chunks),
                        chunk: *chunk,
                        pos: pos + 1,
                    })
                } else if chunk + 1 < chunks.len() {
                    Some(Seq::Chunked {
                        chunks: Arc::clone(chunks),
                        chunk: chunk + 1,
                        pos: 0,
                    })
                } else {
                    None
                }
            }
            Seq::ChunkCons { chunk, pos, tail } => {
                if pos + 1 < chunk.len() {
                    Some(Seq::ChunkCons {
                        chunk: Arc::clone(chunk),
                        pos: pos + 1,
                        tail: tail.clone(),
                    })
                } else {
                    let t = (**tail).clone();
                    if t.is_empty() { None } else { Some(t) }
                }
            }
            Seq::Cons { tail, .. } => tail.clone().map(|t| *t),
            l @ Seq::Lazy { .. } => l.next(),
        }
    }

    /// The seq after the first value (empty, never `None`, at the end).
    #[must_use]
    pub fn more(&self) -> Seq {
        self.next().unwrap_or_else(Self::empty)
    }

    /// The current chunk plus the seq after it (`None` unless this
    /// seq is chunked here). The rest is the empty seq past the last
    /// chunk.
    #[must_use]
    pub fn chunk_next(&self) -> Option<(&[Value], Seq)> {
        match self.forced()? {
            Seq::Chunked { chunks, chunk, pos } => {
                let rest = if chunk + 1 < chunks.len() {
                    Seq::Chunked {
                        chunks: Arc::clone(chunks),
                        chunk: chunk + 1,
                        pos: 0,
                    }
                } else {
                    Seq::empty()
                };
                Some((&chunks[*chunk][*pos..], rest))
            }
            Seq::ChunkCons { chunk, pos, tail } => {
                if *pos < chunk.len() {
                    Some((&chunk[*pos..], (**tail).clone()))
                } else {
                    tail.chunk_next()
                }
            }
            // A forced lazy is a nested lazy: force through (like
            // `first`/`next` — the JVM forces recursively).
            Seq::Lazy { .. } => self.forced()?.chunk_next(),
            _ => None,
        }
    }
}

/// Which JVM seq class a [`RealizedSeq`] reports (S5D): the shape
/// alone cannot tell a string seq from a set seq, but cast errors
/// print the class, so values carry it explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeqKind {
    /// `PersistentVector$ChunkedSeq` (vectors, `butlast`, vector drops).
    Chunked,
    /// `ArraySeq` (`sort`/`sort-by` results).
    Array,
    /// `StringSeq` (string seqs and their tails).
    Str,
    /// `PersistentArrayMap$Seq` (small-map seqs and their tails).
    Map,
    /// `APersistentMap$KeySeq` (set and big-map seqs and their tails).
    Key,
    /// `ChunkedCons` (`take-last` of a lazy seq).
    ChunkedCons,
    /// `Cons` (pure `Seq::Cons` tails).
    Cons,
    /// `LazySeq` (pure `Seq::Lazy` tails).
    Lazy,
    /// `APersistentVector$Seq` (subvector seqs and their tails:
    /// counted but never `IDrop`).
    Subvec,
    /// `APersistentVector$RSeq` (`rseq` results: counted, never
    /// chunked, never `IDrop` — FX2 probes `rseq-vec-class`,
    /// `chunk-first-rseq`, `drop-rseq-class`).
    RSeq,
}

impl SeqKind {
    /// The JVM binary name.
    #[must_use]
    pub fn jvm_name(self) -> &'static str {
        match self {
            SeqKind::Chunked => "clojure.lang.PersistentVector$ChunkedSeq",
            SeqKind::Array => "clojure.lang.ArraySeq",
            SeqKind::Str => "clojure.lang.StringSeq",
            SeqKind::Map => "clojure.lang.PersistentArrayMap$Seq",
            SeqKind::Key => "clojure.lang.APersistentMap$KeySeq",
            SeqKind::ChunkedCons => "clojure.lang.ChunkedCons",
            SeqKind::Cons => "clojure.lang.Cons",
            SeqKind::Lazy => "clojure.lang.LazySeq",
            SeqKind::Subvec => "clojure.lang.APersistentVector$Seq",
            SeqKind::RSeq => "clojure.lang.APersistentVector$RSeq",
        }
    }

    /// The default kind for a pure shape (what `Seq::eager` /
    /// `Seq::chunked` tails report when nothing more specific is
    /// known).
    #[must_use]
    pub fn of_shape(seq: &Seq) -> Self {
        match seq {
            Seq::Chunked { .. } | Seq::ChunkCons { .. } => SeqKind::Chunked,
            Seq::Cons { .. } => SeqKind::Cons,
            Seq::Lazy { .. } => SeqKind::Lazy,
            Seq::Eager { .. } => SeqKind::Array,
        }
    }
}

/// A realized (thunk-free) seq as a value (S5D): `seq` on vectors
/// yields [`Seq::Chunked`], on maps/sets/strings [`Seq::Eager`]
/// (lists are already seqs, so `seq` returns them directly).
/// Prints as a list; sequential equality/hash cross `List`/`Vector`
/// (see `Value`'s impls). Carries metadata like the JVM's seqs.
/// `counted`/`idrop` mirror the JVM interfaces (`StringSeq` and
/// small-map seqs are both; set seqs are neither; `sort` results
/// are counted only), and `kind` is the reported JVM class.
#[derive(Clone, Debug)]
pub struct RealizedSeq {
    seq: Seq,
    meta: Option<Map>,
    counted: bool,
    idrop: bool,
    kind: SeqKind,
}

impl RealizedSeq {
    /// Wrap a realized seq (metadata-free, uncounted, non-`IDrop`,
    /// shape-derived kind).
    #[must_use]
    pub fn new(seq: Seq) -> Self {
        let kind = SeqKind::of_shape(&seq);
        Self {
            seq,
            meta: None,
            counted: false,
            idrop: false,
            kind,
        }
    }

    /// Wrap a realized seq with explicit JVM interfaces and class.
    #[must_use]
    pub fn flagged(seq: Seq, counted: bool, idrop: bool, kind: SeqKind) -> Self {
        Self {
            seq,
            meta: None,
            counted,
            idrop,
            kind,
        }
    }

    /// Whether the JVM seq is `Counted` (`counted?`).
    #[must_use]
    pub fn is_counted(&self) -> bool {
        self.counted
    }

    /// Whether the JVM seq is `IDrop` (`drop` validates eagerly).
    #[must_use]
    pub fn is_idrop(&self) -> bool {
        self.idrop
    }

    /// The reported JVM class.
    #[must_use]
    pub fn kind(&self) -> SeqKind {
        self.kind
    }

    /// The remaining items (for printing and sequential equality).
    #[must_use]
    pub fn items(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let mut cur = self.seq.clone();
        while let Some(head) = cur.first() {
            out.push(head.clone());
            match cur.next() {
                Some(rest) => cur = rest,
                None => break,
            }
        }
        out
    }

    /// The underlying seq (chunk-aware consumers step it directly).
    #[must_use]
    pub fn seq(&self) -> &Seq {
        &self.seq
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }
}

impl crate::ExtensionValue for RealizedSeq {
    fn class_name(&self) -> String {
        self.kind.jvm_name().to_owned()
    }

    fn print(&self) -> String {
        // The printer downcasts and prints with options instead;
        // this fallback prints readably without limits.
        let parts: Vec<String> = self
            .items()
            .iter()
            .map(|v| crate::pr_to_string(v, &crate::PrintOpts::pr()))
            .collect();
        format!("({})", parts.join(" "))
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.items() == o.items())
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Fallback (`Value::hash` downcasts and writes the
        // sequential family directly instead): digest through a
        // fresh state, since `dyn Hasher` cannot nest.
        let items = self.items();
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        items.len().hash(&mut h);
        for item in &items {
            item.hash(&mut h);
        }
        state.write_u64(h.finish());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A persistent subvector view (S5D): `APersistentVector$SubVector`
/// reporting that class — a window over a shared backing vector
/// (nested windows flatten at construction, like the JVM ctor).
/// Prints/counts/compares/calls as its window; `conj`/`assoc` stay
/// subvectors, popping to one element and `empty` fall back to plain
/// vectors. Seqs over the window report `APersistentVector$Seq`
/// ([`SeqKind::Subvec`]).
#[derive(Clone, Debug)]
pub struct SubVectorValue {
    vec: Vector,
    start: usize,
    end: usize,
    meta: Option<Map>,
}

impl SubVectorValue {
    /// A window over `vec` (`start..end`; the caller upholds
    /// `start <= end <= len`, like `RT.subvec` does).
    #[must_use]
    pub fn new(vec: Vector, start: usize, end: usize) -> Self {
        Self {
            vec,
            start,
            end,
            meta: None,
        }
    }

    /// The backing vector.
    #[must_use]
    pub fn vec(&self) -> &Vector {
        &self.vec
    }

    /// The window start (an index into the backing vector).
    #[must_use]
    pub fn start(&self) -> usize {
        self.start
    }

    /// The window end (exclusive).
    #[must_use]
    pub fn end(&self) -> usize {
        self.end
    }

    /// The window length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the window is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The window items, in order.
    #[must_use]
    pub fn items(&self) -> Vec<Value> {
        self.iter().cloned().collect()
    }

    /// Iterate the window items, in order.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.vec.iter().skip(self.start).take(self.len())
    }

    /// The window item at `index` (`None` past the end).
    #[must_use]
    pub fn nth(&self, index: usize) -> Option<&Value> {
        if index < self.len() {
            self.vec.iter().nth(self.start + index)
        } else {
            None
        }
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }

    /// Replace the window items (for deep realization): the new
    /// backing keeps the old head and tail, so `start`/`end` still
    /// delimit the window.
    #[must_use]
    pub fn with_window(&self, items: Vec<Value>) -> Self {
        let mut backing: Vec<Value> = self.vec.iter().take(self.start).cloned().collect();
        let end = self.start + items.len();
        backing.extend(items);
        backing.extend(self.vec.iter().skip(self.end).cloned());
        Self {
            vec: Vector::from_vec(backing),
            start: self.start,
            end,
            meta: self.meta.clone(),
        }
    }
}

impl crate::ExtensionValue for SubVectorValue {
    fn class_name(&self) -> String {
        "clojure.lang.APersistentVector$SubVector".to_owned()
    }

    fn print(&self) -> String {
        // The printer downcasts and prints with options instead;
        // this fallback prints readably without limits.
        let parts: Vec<String> = self
            .iter()
            .map(|v| crate::pr_to_string(v, &crate::PrintOpts::pr()))
            .collect();
        format!("[{}]", parts.join(" "))
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.items() == o.items())
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Fallback (`Value::hash` downcasts and writes the
        // sequential family directly instead): digest through a
        // fresh state, since `dyn Hasher` cannot nest.
        let items = self.items();
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        items.len().hash(&mut h);
        for item in &items {
            item.hash(&mut h);
        }
        state.write_u64(h.finish());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A range bound: all-`Int` ranges walk exactly, any `Double`
/// promotes the walk to `f64` (the start still yields verbatim).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RangeNum {
    /// An exact integer bound.
    Int(i64),
    /// An inexact floating bound.
    Double(f64),
}

impl RangeNum {
    /// The bound as a value (verbatim, never promoted).
    #[must_use]
    pub fn value(self) -> Value {
        match self {
            RangeNum::Int(i) => Value::Int(i),
            RangeNum::Double(d) => Value::Double(d),
        }
    }

    /// The bound as `f64` (double walks).
    #[must_use]
    pub fn to_f64(self) -> f64 {
        match self {
            RangeNum::Int(i) => i as f64,
            RangeNum::Double(d) => d,
        }
    }
}

/// `clojure.lang.LongRange` / `clojure.lang.Range` (S5D): a strict
/// numeric seq. All-`Int` ranges are `LongRange` (`Counted` +
/// `IDrop`, exact `count`); any-`Double` ranges are `Range`
/// (neither). Empty ranges never exist as values (`range`
/// returns `()` instead). Stepping lives in `clj-eval` (chunked,
/// 32 per step); printing/equality realize first, like `LazySeq`.
#[derive(Clone, Debug)]
pub struct RangeValue {
    cur: RangeNum,
    end: Option<RangeNum>,
    step: RangeNum,
    long: bool,
    count: Option<i64>,
    meta: Option<Map>,
}

impl RangeValue {
    /// A range value (`long` selects `LongRange` vs `Range`;
    /// `count` is the exact remaining count for `LongRange`).
    #[must_use]
    pub fn new(
        cur: RangeNum,
        end: Option<RangeNum>,
        step: RangeNum,
        long: bool,
        count: Option<i64>,
    ) -> Self {
        Self {
            cur,
            end,
            step,
            long,
            count,
            meta: None,
        }
    }

    /// The current bound (yields verbatim).
    #[must_use]
    pub fn cur(&self) -> RangeNum {
        self.cur
    }

    /// The exclusive end bound, if any.
    #[must_use]
    pub fn end(&self) -> Option<RangeNum> {
        self.end
    }

    /// The step bound.
    #[must_use]
    pub fn step(&self) -> RangeNum {
        self.step
    }

    /// Whether this is a `LongRange` (`Counted` + `IDrop`).
    #[must_use]
    pub fn is_long(&self) -> bool {
        self.long
    }

    /// The exact remaining count (`LongRange` only).
    #[must_use]
    pub fn count(&self) -> Option<i64> {
        self.count
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }

    /// Whether the walk is done: `LongRange` is count-bounded
    /// (exact, overflow-proof, like `LongRange.next`); double
    /// ranges bound-compare (`NaN` ends never finish, like the
    /// JVM's bounds check).
    #[must_use]
    pub fn done(&self) -> bool {
        if self.long {
            return self.count.is_none_or(|c| c <= 0);
        }
        match (self.cur, self.end, self.step) {
            (_, None, _) => false,
            (cur, Some(end), step) => {
                let (c, x, s) = (cur.to_f64(), end.to_f64(), step.to_f64());
                if s >= 0.0 { c >= x } else { c <= x }
            }
        }
    }

    /// Advance one step: `LongRange` adds unchecked (wrapping, like
    /// `LongRange.next` — the count bounds the walk); double walks
    /// check `Int`+`Int` (`None` ends the walk, like `forceChunk`
    /// landing out of bounds) and promote anything else to `f64`.
    pub fn advance(&self) -> Option<RangeNum> {
        match (self.cur, self.step) {
            (RangeNum::Int(c), RangeNum::Int(s)) if self.long => {
                Some(RangeNum::Int(c.wrapping_add(s)))
            }
            (RangeNum::Int(c), RangeNum::Int(s)) => c.checked_add(s).map(RangeNum::Int),
            (c, s) => Some(RangeNum::Double(c.to_f64() + s.to_f64())),
        }
    }

    /// The head plus the range after one step (`None` when done;
    /// the tail is `None` on `Int`+`Int` overflow).
    #[must_use]
    pub fn stepped(&self) -> Option<(Value, Option<RangeValue>)> {
        if self.done() {
            return None;
        }
        let head = self.cur.value();
        // Tails drop metadata (like `Range.next`/`LongRange.next`,
        // whose fresh objects carry none).
        let next = self.advance().map(|nxt| RangeValue {
            cur: nxt,
            end: self.end,
            step: self.step,
            long: self.long,
            count: self.count.map(|c| c - 1),
            meta: None,
        });
        Some((head, next))
    }

    /// Call `f` per item (pure walk; infinite ranges never return,
    /// like the JVM's `hashCode` walk; a `long overflow` ends the
    /// walk early — the `hash` fn realizes first and reports it).
    pub fn for_each(&self, f: &mut dyn FnMut(&Value)) {
        let mut cur = self.clone();
        while let Some((head, next)) = cur.stepped() {
            f(&head);
            match next {
                Some(nxt) => cur = nxt,
                None => break,
            }
        }
    }

    /// The item count (`LongRange` reads its exact count; double
    /// ranges walk — infinite ones never return).
    #[must_use]
    pub fn walked_len(&self) -> usize {
        if let Some(c) = self.count {
            return usize::try_from(c.max(0)).unwrap_or(usize::MAX);
        }
        let mut n = 0usize;
        self.for_each(&mut |_| n += 1);
        n
    }
}

impl crate::ExtensionValue for RangeValue {
    fn class_name(&self) -> String {
        if self.long {
            "clojure.lang.LongRange".to_owned()
        } else {
            "clojure.lang.Range".to_owned()
        }
    }

    fn print(&self) -> String {
        // Out-of-band only (natives realize first): the JVM would
        // stream items, which needs the evaluator for chunking.
        "(...)".to_owned()
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        // Unrealized identity only (`=` realizes first).
        other.as_any().downcast_ref::<Self>().is_some_and(|o| {
            self.cur == o.cur && self.end == o.end && self.step == o.step && self.long == o.long
        })
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Fallback (`Value::hash` downcasts and writes the
        // sequential family directly instead): digest through a
        // fresh state, since `dyn Hasher` cannot nest.
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        self.walked_len().hash(&mut h);
        self.for_each(&mut |v| v.hash(&mut h));
        state.write_u64(h.finish());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `clojure.lang.Repeat` (S5D): a strict constant seq (`IDrop`,
/// never `Counted`, unchunked — one item per step). `n` is the
/// remaining count (`None` is infinite). Empty repeats never exist
/// as values (`repeat` returns `()` instead).
#[derive(Clone, Debug)]
pub struct RepeatValue {
    x: Value,
    n: Option<i64>,
    meta: Option<Map>,
}

impl RepeatValue {
    /// A repeat value (`None` is infinite).
    #[must_use]
    pub fn new(x: Value, n: Option<i64>) -> Self {
        Self { x, n, meta: None }
    }

    /// The repeated value.
    #[must_use]
    pub fn x(&self) -> &Value {
        &self.x
    }

    /// The remaining count (`None` is infinite).
    #[must_use]
    pub fn n(&self) -> Option<i64> {
        self.n
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }

    /// Call `f` per item (infinite repeats never return).
    pub fn for_each(&self, f: &mut dyn FnMut(&Value)) {
        match self.n {
            Some(n) => {
                for _ in 0..n.max(0) {
                    f(&self.x);
                }
            }
            None => loop {
                f(&self.x);
            },
        }
    }

    /// The item count (infinite repeats never return).
    #[must_use]
    pub fn walked_len(&self) -> usize {
        match self.n {
            Some(n) => usize::try_from(n.max(0)).unwrap_or(usize::MAX),
            None => {
                let mut n = 0usize;
                self.for_each(&mut |_| n += 1);
                n
            }
        }
    }
}

impl crate::ExtensionValue for RepeatValue {
    fn class_name(&self) -> String {
        "clojure.lang.Repeat".to_owned()
    }

    fn print(&self) -> String {
        "(...)".to_owned()
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.x == o.x && self.n == o.n)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Fallback (`Value::hash` downcasts and writes the
        // sequential family directly instead).
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        self.walked_len().hash(&mut h);
        self.for_each(&mut |v| v.hash(&mut h));
        state.write_u64(h.finish());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `clojure.lang.Cycle` (S5D): a strict cycling seq. `prev` is the
/// seqed input (replayed forever), `current` the remaining tail.
/// Empty inputs never reach here (`cycle` returns `()` instead, like
/// `Cycle.create`).
#[derive(Clone, Debug)]
pub struct CycleValue {
    prev: Value,
    current: Value,
    meta: Option<Map>,
}

impl CycleValue {
    /// A cycle value over the seqed input.
    #[must_use]
    pub fn new(prev: Value, current: Value) -> Self {
        Self {
            prev,
            current,
            meta: None,
        }
    }

    /// The seqed input (replayed forever).
    #[must_use]
    pub fn prev(&self) -> &Value {
        &self.prev
    }

    /// The remaining tail.
    #[must_use]
    pub fn current(&self) -> &Value {
        &self.current
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }
}

impl crate::ExtensionValue for CycleValue {
    fn class_name(&self) -> String {
        "clojure.lang.Cycle".to_owned()
    }

    fn print(&self) -> String {
        "(...)".to_owned()
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(self, o))
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_usize(self as *const Self as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `clojure.lang.Iterate` (S5D): a strict iteration seq with a
/// lazily realized seed (like the JVM's `_seed`/`prevSeed`):
/// `new` holds a realized head (free, no call), `deferred` holds
/// the previous seed (one `f` call on first read, memoized).
/// Stepping never calls `f` (like `next`, which defers); only the
/// head read realizes (like `first`). The call itself lives in
/// `clj-eval` (it needs the evaluator); this type only owns the
/// memo cell.
#[derive(Debug)]
pub struct IterateValue {
    f: Value,
    prev: Value,
    seed: Mutex<Option<Value>>,
    meta: Option<Map>,
}

impl Clone for IterateValue {
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone(),
            prev: self.prev.clone(),
            seed: Mutex::new(self.seed.lock().ok().and_then(|s| s.clone())),
            meta: self.meta.clone(),
        }
    }
}

impl IterateValue {
    /// An iterate value with a realized head.
    #[must_use]
    pub fn new(f: Value, x: Value) -> Self {
        Self {
            f,
            prev: Value::Nil,
            seed: Mutex::new(Some(x)),
            meta: None,
        }
    }

    /// An iterate value whose head is `(f prev)` (realized on first
    /// read, memoized).
    #[must_use]
    pub fn deferred(f: Value, prev: Value) -> Self {
        Self {
            f,
            prev,
            seed: Mutex::new(None),
            meta: None,
        }
    }

    /// The step function.
    #[must_use]
    pub fn f(&self) -> &Value {
        &self.f
    }

    /// The previous seed (the `f` argument while unrealized).
    #[must_use]
    pub fn prev(&self) -> &Value {
        &self.prev
    }

    /// The memoized head, if realized.
    #[must_use]
    pub fn seed_opt(&self) -> Option<Value> {
        self.seed.lock().ok().and_then(|s| s.clone())
    }

    /// Memoize a realized head.
    pub fn store_seed(&self, v: Value) {
        if let Ok(mut seed) = self.seed.lock() {
            *seed = Some(v);
        }
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }
}

impl crate::ExtensionValue for IterateValue {
    fn class_name(&self) -> String {
        "clojure.lang.Iterate".to_owned()
    }

    fn print(&self) -> String {
        "(...)".to_owned()
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(self, o))
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_usize(self as *const Self as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `clojure.lang.Cons` (S5D): a strict head-plus-tail seq. `tail`
/// is the seq-or-nil tail (`cons` seqs non-seq tails eagerly, like
/// `RT.cons`; `rest` returns it untouched, preserving its kind).
#[derive(Clone, Debug)]
pub struct ConsValue {
    head: Value,
    tail: Value,
    meta: Option<Map>,
}

impl ConsValue {
    /// A cons value.
    #[must_use]
    pub fn new(head: Value, tail: Value) -> Self {
        Self {
            head,
            tail,
            meta: None,
        }
    }

    /// The head item.
    #[must_use]
    pub fn head(&self) -> &Value {
        &self.head
    }

    /// The raw tail value.
    #[must_use]
    pub fn tail(&self) -> &Value {
        &self.tail
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Attach a metadata map.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta;
        self
    }
}

impl crate::ExtensionValue for ConsValue {
    fn class_name(&self) -> String {
        "clojure.lang.Cons".to_owned()
    }

    fn print(&self) -> String {
        "(...)".to_owned()
    }

    fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(self, o))
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_usize(self as *const Self as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

macro_rules! entry_seq {
    ($t:ident, $class:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug)]
        pub struct $t {
            underlying: Value,
            meta: Option<Map>,
        }

        impl $t {
            #[doc = "An entry view over the seqed input (entries as 2-vectors)."]
            #[must_use]
            pub fn new(underlying: Value) -> Self {
                Self {
                    underlying,
                    meta: None,
                }
            }

            #[doc = "The seqed input (entries as 2-vectors)."]
            #[must_use]
            pub fn underlying(&self) -> &Value {
                &self.underlying
            }

            #[doc = "The metadata map, if any."]
            #[must_use]
            pub fn meta(&self) -> Option<&Map> {
                self.meta.as_ref()
            }

            #[doc = "Attach a metadata map."]
            #[must_use]
            pub fn with_meta(mut self, meta: Option<Map>) -> Self {
                self.meta = meta;
                self
            }
        }

        impl crate::ExtensionValue for $t {
            fn class_name(&self) -> String {
                $class.to_owned()
            }

            fn print(&self) -> String {
                "(...)".to_owned()
            }

            fn dyn_eq(&self, other: &dyn crate::ExtensionValue) -> bool {
                other
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|o| std::ptr::eq(self, o))
            }

            fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
                state.write_usize(self as *const Self as usize);
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    };
}

entry_seq!(
    KeySeqValue,
    "clojure.lang.APersistentMap$KeySeq",
    "`clojure.lang.APersistentMap$KeySeq` (S5D): a strict view mapping entries to keys."
);
entry_seq!(
    ValSeqValue,
    "clojure.lang.APersistentMap$ValSeq",
    "`clojure.lang.APersistentMap$ValSeq` (S5D): a strict view mapping entries to values."
);

#[cfg(test)]
mod tests {
    use super::{
        ConsValue, CycleValue, IterateValue, KeySeqValue, RangeNum, RangeValue, RealizedSeq,
        RepeatValue, Seq, SeqKind, SubVectorValue, ValSeqValue,
    };
    use crate::{ExtensionValue, Value, Vector};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::Arc;

    fn int(i: i64) -> Value {
        Value::Int(i)
    }

    fn hash_ext(e: &dyn ExtensionValue) -> u64 {
        let mut h = DefaultHasher::new();
        e.dyn_hash(&mut h);
        h.finish()
    }

    fn hash_val(v: &Value) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    #[test]
    fn seq_debug_names_shapes() {
        assert_eq!(format!("{:?}", Seq::empty()), "Eager { len: 0, pos: 0 }");
        let chunked = Seq::chunked(vec![int(1), int(2), int(3)]).expect("chunked");
        assert_eq!(
            format!("{:?}", chunked),
            "Chunked { chunks: 1, chunk: 0, pos: 0 }"
        );
        let headed = Seq::chunk_cons(vec![int(1), int(2)], Seq::empty());
        assert_eq!(format!("{:?}", headed), "ChunkCons { len: 2, pos: 0 }");
        assert_eq!(format!("{:?}", Seq::cons(int(1), None)), "Cons(Int(1))");
        let lazy = Seq::lazy(|| None::<Seq>);
        assert_eq!(format!("{:?}", lazy), "Lazy(..)");
    }

    #[test]
    fn seq_chunk_cons_empty_collapses() {
        let tail = Seq::eager(vec![int(1)]).expect("tail");
        let s = Seq::chunk_cons(Vec::new(), tail);
        assert_eq!(s.first(), Some(&int(1)));
        assert!(s.next().is_none());
    }

    #[test]
    fn seq_lazy_none_is_empty() {
        let s = Seq::lazy(|| None::<Seq>);
        assert!(s.is_empty());
        assert!(s.first().is_none());
        assert!(s.next().is_none());
        assert!(s.more().is_empty());
    }

    #[test]
    fn seq_nested_lazy_forces() {
        let inner = Seq::lazy(|| Seq::eager(vec![int(1), int(2)]));
        let outer = Seq::lazy(move || Some(inner.clone()));
        assert!(!outer.is_empty());
        assert_eq!(outer.first(), Some(&int(1)));
        let tail = outer.next().expect("tail");
        assert_eq!(tail.first(), Some(&int(2)));
        assert!(tail.next().is_none());
        assert_eq!(outer.more().first(), Some(&int(2)));
    }

    #[test]
    fn seq_chunk_next_nested_lazy_delegates() {
        // `chunk_next` forces through a nested lazy (like
        // `first`/`next` — the JVM forces recursively).
        let items: Vec<Value> = (0i64..40).map(int).collect();
        let chunked = Seq::chunked(items).expect("chunked");
        let inner = Seq::lazy(move || Some(chunked.clone()));
        let outer = Seq::lazy(move || Some(inner.clone()));
        let (head, rest) = outer.chunk_next().expect("delegate");
        assert_eq!(head.len(), 32);
        assert_eq!(rest.first(), Some(&int(32)));
    }

    #[test]
    fn seq_chunk_cons_first_next() {
        let tail = Seq::eager(vec![int(3)]).expect("tail");
        let s = Seq::chunk_cons(vec![int(1), int(2)], tail);
        assert_eq!(s.first(), Some(&int(1)));
        let mid = s.next().expect("mid");
        assert_eq!(mid.first(), Some(&int(2)));
        let rest = mid.next().expect("rest");
        assert_eq!(rest.first(), Some(&int(3)));
        assert!(rest.next().is_none());
    }

    #[test]
    fn seq_next_crosses_chunks() {
        let items: Vec<Value> = (0i64..33).map(int).collect();
        let mut cur = Seq::chunked(items).expect("chunked");
        for _ in 0..32 {
            cur = cur.next().expect("step");
        }
        assert_eq!(cur.first(), Some(&int(32)));
        assert!(cur.next().is_none());
    }

    #[test]
    fn seq_chunk_next_exhausted_delegates() {
        let items: Vec<Value> = (0i64..40).map(int).collect();
        let tail = Seq::chunked(items).expect("tail");
        let s = Seq::ChunkCons {
            chunk: Arc::from(vec![int(1)]),
            pos: 1,
            tail: Box::new(tail),
        };
        let (head, rest) = s.chunk_next().expect("delegate");
        assert_eq!(head.len(), 32);
        assert_eq!(rest.first(), Some(&int(32)));
    }

    #[test]
    fn seq_kind_jvm_names() {
        assert_eq!(
            SeqKind::Chunked.jvm_name(),
            "clojure.lang.PersistentVector$ChunkedSeq"
        );
        assert_eq!(SeqKind::Array.jvm_name(), "clojure.lang.ArraySeq");
        assert_eq!(SeqKind::Str.jvm_name(), "clojure.lang.StringSeq");
        assert_eq!(
            SeqKind::Map.jvm_name(),
            "clojure.lang.PersistentArrayMap$Seq"
        );
        assert_eq!(
            SeqKind::Key.jvm_name(),
            "clojure.lang.APersistentMap$KeySeq"
        );
        assert_eq!(SeqKind::ChunkedCons.jvm_name(), "clojure.lang.ChunkedCons");
        assert_eq!(SeqKind::Cons.jvm_name(), "clojure.lang.Cons");
        assert_eq!(SeqKind::Lazy.jvm_name(), "clojure.lang.LazySeq");
        assert_eq!(
            SeqKind::Subvec.jvm_name(),
            "clojure.lang.APersistentVector$Seq"
        );
    }

    #[test]
    fn seq_kind_of_shape() {
        let chunked = Seq::chunked(vec![int(1)]).expect("chunked");
        assert_eq!(RealizedSeq::new(chunked).kind(), SeqKind::Chunked);
        assert_eq!(
            RealizedSeq::new(Seq::chunk_cons(vec![int(1)], Seq::empty())).kind(),
            SeqKind::Chunked
        );
        assert_eq!(
            RealizedSeq::new(Seq::cons(int(1), None)).kind(),
            SeqKind::Cons
        );
        assert_eq!(
            RealizedSeq::new(Seq::lazy(|| None::<Seq>)).kind(),
            SeqKind::Lazy
        );
        let eager = Seq::eager(vec![int(1)]).expect("eager");
        assert_eq!(RealizedSeq::new(eager).kind(), SeqKind::Array);
        assert_eq!(RealizedSeq::new(Seq::empty()).kind(), SeqKind::Array);
    }

    #[test]
    fn realized_seq_fallbacks() {
        let seq = RealizedSeq::new(Seq::eager(vec![int(1), int(2)]).expect("seq"));
        assert_eq!(seq.print(), "(1 2)");
        let same = RealizedSeq::new(Seq::eager(vec![int(1), int(2)]).expect("same"));
        assert!(seq.dyn_eq(&same));
        let diff = RealizedSeq::new(Seq::eager(vec![int(1)]).expect("diff"));
        assert!(!seq.dyn_eq(&diff));
        let range = RangeValue::new(
            RangeNum::Int(0),
            Some(RangeNum::Int(2)),
            RangeNum::Int(1),
            true,
            Some(2),
        );
        assert!(!seq.dyn_eq(&range));
        assert_eq!(hash_ext(&seq), hash_ext(&same));
    }

    #[test]
    fn subvector_helpers_and_fallbacks() {
        let backing = Vector::from_vec(vec![int(1), int(2), int(3)]);
        assert!(SubVectorValue::new(backing.clone(), 1, 1).is_empty());
        let sv = SubVectorValue::new(backing, 0, 2);
        assert!(!sv.is_empty());
        assert_eq!(sv.print(), "[1 2]");
        let same = SubVectorValue::new(Vector::from_vec(vec![int(1), int(2)]), 0, 2);
        assert!(sv.dyn_eq(&same));
        let diff = SubVectorValue::new(Vector::from_vec(vec![int(1)]), 0, 1);
        assert!(!sv.dyn_eq(&diff));
        let seq = RealizedSeq::new(Seq::eager(vec![int(1), int(2)]).expect("seq"));
        assert!(!sv.dyn_eq(&seq));
        assert_eq!(hash_ext(&sv), hash_ext(&same));
    }

    #[test]
    fn range_value_meta_done_overflow() {
        let r = RangeValue::new(
            RangeNum::Int(0),
            Some(RangeNum::Int(2)),
            RangeNum::Int(1),
            true,
            Some(2),
        );
        let v = Value::Extension(Arc::new(r));
        assert!(v.meta().is_none());
        let meta = crate::new_map();
        let tagged = v.clone().with_meta(Some(meta.clone()));
        assert_eq!(tagged.meta(), Some(&meta));
        let open = RangeValue::new(RangeNum::Int(0), None, RangeNum::Int(1), false, None);
        assert!(!open.done());
        // Overflow break: the bounds are `f64`-distinct (so
        // `done` is false) but `start + step` overflows `i64` (so
        // the first advance ends the walk). Production builds the
        // `long` path for this count — the shape is constructed
        // directly to pin the break (oracled walk: 1 item).
        let edge = RangeValue::new(
            RangeNum::Int(9223372036854773807),
            Some(RangeNum::Int(9223372036854775807)),
            RangeNum::Int(3000),
            false,
            None,
        );
        assert_eq!(edge.walked_len(), 1);
        let mut out = Vec::new();
        edge.for_each(&mut |item| out.push(item.clone()));
        assert_eq!(out, vec![int(9223372036854773807)]);
    }

    #[test]
    fn range_value_fallbacks() {
        let r = RangeValue::new(
            RangeNum::Double(0.0),
            Some(RangeNum::Double(1.5)),
            RangeNum::Double(0.5),
            false,
            None,
        );
        assert_eq!(r.class_name(), "clojure.lang.Range");
        let same = RangeValue::new(
            RangeNum::Double(0.0),
            Some(RangeNum::Double(1.5)),
            RangeNum::Double(0.5),
            false,
            None,
        );
        assert!(r.dyn_eq(&same));
        let diff = RangeValue::new(
            RangeNum::Double(0.0),
            Some(RangeNum::Double(1.5)),
            RangeNum::Double(1.0),
            false,
            None,
        );
        assert!(!r.dyn_eq(&diff));
        let rep = RepeatValue::new(int(1), Some(2));
        assert!(!r.dyn_eq(&rep));
        assert_eq!(hash_ext(&r), hash_ext(&same));
    }

    #[test]
    fn repeat_value_gaps() {
        let v = Value::Extension(Arc::new(RepeatValue::new(int(7), Some(3))));
        assert!(v.meta().is_none());
        let meta = crate::new_map();
        let tagged = v.clone().with_meta(Some(meta.clone()));
        assert_eq!(tagged.meta(), Some(&meta));
        let r = RepeatValue::new(int(7), Some(3));
        assert_eq!(r.class_name(), "clojure.lang.Repeat");
        assert_eq!(r.print(), "(...)");
        let same = RepeatValue::new(int(7), Some(3));
        assert!(r.dyn_eq(&same));
        let fewer = RepeatValue::new(int(7), Some(2));
        assert!(!r.dyn_eq(&fewer));
        let other = RepeatValue::new(int(8), Some(3));
        assert!(!r.dyn_eq(&other));
        assert_eq!(hash_ext(&r), hash_ext(&same));
    }

    #[test]
    #[should_panic(expected = "stop after two")]
    fn repeat_for_each_infinite_iterates() {
        // The infinite arm never returns (matching the JVM, whose
        // walks hang the same way): iterate twice, then stop with a
        // panic so the loop mechanics genuinely run and the test ends.
        let r = RepeatValue::new(int(1), None);
        let mut count = 0;
        r.for_each(&mut |_| {
            count += 1;
            assert!(count < 3, "stop after two iterations");
        });
    }

    #[test]
    fn repeat_walked_len_infinite_diverges() {
        use std::thread;
        use std::time::Duration;

        // The infinite arm never returns and offers no injection
        // point (its closure is internal): run it on a thread the
        // test abandons, so the arm executes and the process still
        // exits to flush counters. The arm is entered within
        // microseconds of thread start; 50ms is pure margin.
        let r = RepeatValue::new(int(1), None);
        let _handle = thread::spawn(move || r.walked_len());
        thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn cycle_value_gaps() {
        let v = Value::Extension(Arc::new(CycleValue::new(int(1), int(2))));
        assert!(v.meta().is_none());
        let meta = crate::new_map();
        let tagged = v.clone().with_meta(Some(meta.clone()));
        assert_eq!(tagged.meta(), Some(&meta));
        let c = CycleValue::new(int(1), int(2));
        assert_eq!(c.class_name(), "clojure.lang.Cycle");
        assert_eq!(c.print(), "(...)");
    }

    #[test]
    fn iterate_value_gaps() {
        let v = Value::Extension(Arc::new(IterateValue::new(int(0), int(1))));
        assert!(v.meta().is_none());
        let meta = crate::new_map();
        let tagged = v.clone().with_meta(Some(meta.clone()));
        assert_eq!(tagged.meta(), Some(&meta));
        let it = IterateValue::new(int(0), int(1));
        let fresh = it.clone();
        assert_eq!(fresh.seed_opt(), Some(int(1)));
        it.store_seed(int(9));
        let copied = it.clone();
        assert_eq!(copied.seed_opt(), Some(int(9)));
        let pending = IterateValue::deferred(int(0), int(1));
        assert!(pending.clone().seed_opt().is_none());
        assert_eq!(it.class_name(), "clojure.lang.Iterate");
        assert_eq!(it.print(), "(...)");
    }

    #[test]
    fn cons_value_gaps() {
        let v = Value::Extension(Arc::new(ConsValue::new(int(1), int(2))));
        assert!(v.meta().is_none());
        let meta = crate::new_map();
        let tagged = v.clone().with_meta(Some(meta.clone()));
        assert_eq!(tagged.meta(), Some(&meta));
        let c = ConsValue::new(int(1), int(2));
        assert_eq!(c.print(), "(...)");
        assert!(c.dyn_eq(&c));
        let twin = ConsValue::new(int(1), int(2));
        assert!(!c.dyn_eq(&twin));
        let hashed = Value::Extension(Arc::new(c));
        assert_eq!(hash_val(&hashed), hash_val(&hashed));
    }

    #[test]
    fn entry_seq_gaps() {
        let k = Value::Extension(Arc::new(KeySeqValue::new(int(1))));
        assert!(k.meta().is_none());
        let meta = crate::new_map();
        assert_eq!(k.clone().with_meta(Some(meta.clone())).meta(), Some(&meta));
        let key = KeySeqValue::new(int(1));
        assert_eq!(key.class_name(), "clojure.lang.APersistentMap$KeySeq");
        assert_eq!(key.print(), "(...)");
        assert!(key.dyn_eq(&key));
        let other_key = KeySeqValue::new(int(1));
        assert!(!key.dyn_eq(&other_key));
        assert_eq!(hash_val(&k), hash_val(&k));
        let g = Value::Extension(Arc::new(ValSeqValue::new(int(1))));
        assert_eq!(g.clone().with_meta(Some(meta.clone())).meta(), Some(&meta));
        let val = ValSeqValue::new(int(1));
        assert_eq!(val.class_name(), "clojure.lang.APersistentMap$ValSeq");
        assert_eq!(val.print(), "(...)");
        assert!(val.dyn_eq(&val));
        let other_val = ValSeqValue::new(int(1));
        assert!(!val.dyn_eq(&other_val));
        assert_eq!(hash_val(&g), hash_val(&g));
    }
}
