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

use std::sync::{Arc, OnceLock};

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
            Some(Seq::Lazy { .. }) => unreachable!("forced above"),
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
            Seq::Lazy { .. } => unreachable!("forced above"),
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
            Seq::Lazy { .. } => unreachable!("forced above"),
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
            _ => None,
        }
    }
}
