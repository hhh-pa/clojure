// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Lazy seqs (S5D, C4): chunked (32), memoized, nil-punning.
//!
//! [`PendingSeq`] is a seq value whose tail needs the evaluator:
//! forcing runs one [`Thunk`] step (a whole input chunk for
//! chunked inputs, one item otherwise — the JVM's observable
//! side-effect order), memoizes the chunk in the shared cell, and
//! chains the tail value (which may be pending again). Realization
//! sees the ambient bindings, never the creation ones (probed).
//! Thunk errors restore the pending state, so a retry re-runs the
//! step like the JVM's `LazySeq`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use clj_value::{
    CHUNK_SIZE, ConsValue, CycleValue, ExtensionValue, IterateValue, KeySeqValue, List, Map,
    RangeNum, RangeValue, RealizedSeq, RepeatValue, Seq, SeqKind, SubVectorValue, ThrowClass,
    ValSeqValue, Value, Vector,
};

use crate::core::Num;
use crate::env::Env;
use crate::error::{NativeError, Signal, jvm_class_of};
use crate::eval::Evaluator;
use crate::expr::Expr;
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

/// Which lazy class a [`PendingSeq`] reports: almost everything is
/// a `LazySeq`, but `rest` of a forced chunk is a `ChunkedCons`
/// (like the JVM's `more`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LazyKind {
    LazySeq,
    ChunkedCons,
}

impl LazyKind {
    fn jvm_name(self) -> &'static str {
        match self {
            LazyKind::LazySeq => "clojure.lang.LazySeq",
            LazyKind::ChunkedCons => "clojure.lang.ChunkedCons",
        }
    }
}

/// A seq value with an evaluator-backed tail: forcing needs
/// [`force_pending`]. Prints best-effort from memoized state
/// (natives realize first, so this is out-of-band only).
#[derive(Debug)]
pub struct PendingSeq {
    state: Mutex<SeqState>,
    meta: Option<Map>,
    kind: LazyKind,
}

/// The memoized state: pending thunks become realized chunks.
#[derive(Clone, Debug)]
enum SeqState {
    Pending(Box<Thunk>),
    Chunk {
        items: Arc<[Value]>,
        pos: usize,
        tail: Value,
    },
    Empty,
}

/// One lazy step: the captured op plus its (advanced) inputs.
/// Chunks hold at most [`CHUNK_SIZE`] items.
#[derive(Clone, Debug)]
pub(crate) enum Thunk {
    Map {
        f: Value,
        inputs: Vec<Value>,
    },
    Filter {
        f: Value,
        input: Value,
    },
    Remove {
        f: Value,
        input: Value,
    },
    Take {
        n: Value,
        input: Value,
    },
    Concat {
        inputs: Vec<Value>,
    },
    MapCat {
        outer: Value,
    },
    Chain {
        head: Vec<Value>,
        tail: Value,
    },
    // The `lazy-seq*` control form (S5D): an unevaluated body
    // plus its lexical env (dynamic bindings stay force-time, like
    // the JVM closure).
    Thunk0 {
        body: Arc<Vec<Expr>>,
        env: Env,
    },
    Repeatedly {
        f: Value,
    },
    Interleave {
        queue: Vec<Value>,
        inputs: Vec<Value>,
    },
    Interpose {
        sep: Value,
        input: Value,
        first: bool,
    },
    TakeWhile {
        f: Value,
        input: Value,
    },
    DropWhile {
        f: Value,
        input: Value,
    },
    Drop {
        n: Value,
        input: Value,
    },
    TakeNth {
        n: Value,
        input: Value,
    },
    Partition {
        n: Value,
        step: Value,
        pad: Option<Value>,
        input: Value,
    },
    PartitionAll {
        n: Value,
        step: Value,
        input: Value,
    },
    PartitionBy {
        f: Value,
        input: Value,
    },
    PartitionByAdvance {
        f: Value,
        run: Value,
        s: Value,
    },
    PartitionRun {
        f: Value,
        fv: Value,
        input: Value,
    },
    Distinct {
        seen: HashSet<Value>,
        input: Value,
    },
    Dedupe {
        last: Option<Value>,
        input: Value,
    },
    Keep {
        f: Value,
        input: Value,
    },
    KeepIndexed {
        f: Value,
        input: Value,
        idx: i64,
    },
    MapIndexed {
        f: Value,
        input: Value,
        idx: i64,
    },
    Reductions {
        f: Value,
        acc: Option<Value>,
        has_init: bool,
        input: Value,
        started: bool,
    },
    Replace {
        smap: Value,
        input: Value,
    },
}

/// A forced step: empty, or a chunk plus its tail value.
enum Forced {
    Empty,
    Chunk { items: Arc<[Value]>, tail: Value },
}

/// One input step: empty, one head plus tail, or a chunk plus tail.
pub(crate) enum StepUnit {
    Empty,
    One { head: Value, tail: Value },
    Chunk { items: Arc<[Value]>, tail: Value },
}

impl PendingSeq {
    /// A pending seq value from a thunk (metadata-free).
    fn pending(thunk: Thunk) -> Value {
        Self::pending_kind(thunk, LazyKind::LazySeq)
    }

    /// A pending seq value with an explicit lazy class.
    fn pending_kind(thunk: Thunk, kind: LazyKind) -> Value {
        Value::Extension(Arc::new(Self {
            state: Mutex::new(SeqState::Pending(Box::new(thunk))),
            meta: None,
            kind,
        }))
    }

    /// The metadata map, if any.
    pub(crate) fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// A metadata-carrying clone with a FRESH memo cell (like the
    /// JVM, whose `withMeta` re-wraps the fn). Staged for
    /// `with-meta` (S5D).
    #[allow(dead_code)]
    pub(crate) fn with_meta_fresh(&self, meta: Option<Map>) -> Value {
        let state = match self.state.lock().ok().map(|s| s.clone()) {
            Some(SeqState::Pending(t)) => SeqState::Pending(t),
            // Forced states re-wrap as chains (already realized, so
            // no recomputation either way).
            Some(SeqState::Chunk { items, pos, tail }) => {
                SeqState::Pending(Box::new(Thunk::Chain {
                    head: items[pos..].to_vec(),
                    tail,
                }))
            }
            _ => SeqState::Empty,
        };
        Value::Extension(Arc::new(Self {
            state: Mutex::new(state),
            meta,
            kind: self.kind,
        }))
    }
}

impl ExtensionValue for PendingSeq {
    fn class_name(&self) -> String {
        self.kind.jvm_name().to_owned()
    }

    fn print(&self) -> String {
        // Best-effort over memoized state (natives realize first):
        // forced chunks print their known prefix, pending prints
        // `(...)`, empties `()`.
        let Ok(state) = self.state.lock() else {
            return "(...)".to_owned();
        };
        match &*state {
            SeqState::Empty => "()".to_owned(),
            SeqState::Pending(_) => "(...)".to_owned(),
            SeqState::Chunk { items, pos, tail } => {
                let parts: Vec<String> = items[*pos..]
                    .iter()
                    .map(|v| clj_value::pr_to_string(v, &clj_value::PrintOpts::pr()))
                    .collect();
                let open = parts.join(" ");
                let tail_empty =
                    matches!(tail, Value::Nil) || matches!(tail, Value::List(l) if l.is_empty());
                if tail_empty && !open.is_empty() {
                    format!("({open})")
                } else if open.is_empty() {
                    "(...)".to_owned()
                } else {
                    format!("({open} ...)")
                }
            }
        }
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        // Unrealized identity only (`=` realizes first, so deep
        // equality never reaches here).
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

/// Downcast a seq value to its pending cell, if any.
pub(crate) fn as_pending(v: &Value) -> Option<&PendingSeq> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<PendingSeq>(),
        _ => None,
    }
}

/// Downcast a seq value to its realized seq, if any.
pub(crate) fn as_realized(v: &Value) -> Option<&RealizedSeq> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<RealizedSeq>(),
        _ => None,
    }
}

macro_rules! seq_downcast {
    ($name:ident, $t:ty, $doc:literal) => {
        #[doc = $doc]
        pub(crate) fn $name(v: &Value) -> Option<&$t> {
            match v {
                Value::Extension(e) => e.as_any().downcast_ref::<$t>(),
                _ => None,
            }
        }
    };
}

seq_downcast!(
    as_range,
    RangeValue,
    "Downcast a seq value to its range, if any."
);
seq_downcast!(
    as_repeat,
    RepeatValue,
    "Downcast a seq value to its repeat, if any."
);
seq_downcast!(
    as_cycle,
    CycleValue,
    "Downcast a seq value to its cycle, if any."
);
seq_downcast!(
    as_iterate,
    IterateValue,
    "Downcast a seq value to its iterate, if any."
);
seq_downcast!(
    as_cons_value,
    ConsValue,
    "Downcast a seq value to its cons, if any."
);
seq_downcast!(
    as_keys,
    KeySeqValue,
    "Downcast a seq value to its key view, if any."
);
seq_downcast!(
    as_vals,
    ValSeqValue,
    "Downcast a seq value to its value view, if any."
);
seq_downcast!(
    as_subvector,
    SubVectorValue,
    "Downcast a value to its subvector view, if any."
);

/// Whether the value is a seq (a list or any seq object).
pub(crate) fn is_seq_value(v: &Value) -> bool {
    matches!(v, Value::List(_))
        || as_pending(v).is_some()
        || as_realized(v).is_some()
        || as_range(v).is_some()
        || as_repeat(v).is_some()
        || as_cycle(v).is_some()
        || as_iterate(v).is_some()
        || as_cons_value(v).is_some()
        || as_keys(v).is_some()
        || as_vals(v).is_some()
}

/// The `ISeq` coercion error.
fn iseq_err(v: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Don't know how to create ISeq from: {}", jvm_class_of(v)),
    )
}

/// Whether `seq` accepts the value without forcing it (mirrors
/// `RT.canSeq` over our value types; transients and throwables
/// are not seqable). Staged for `seqable?` (S5D).
#[allow(dead_code)]
pub(crate) fn is_seqable(v: &Value) -> bool {
    match v {
        Value::Nil
        | Value::List(_)
        | Value::Vector(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::Str(_) => true,
        Value::Extension(e) => {
            let any = e.as_any();
            any.downcast_ref::<PendingSeq>().is_some()
                || any.downcast_ref::<RealizedSeq>().is_some()
                || any.downcast_ref::<RangeValue>().is_some()
                || any.downcast_ref::<RepeatValue>().is_some()
                || any.downcast_ref::<CycleValue>().is_some()
                || any.downcast_ref::<IterateValue>().is_some()
                || any.downcast_ref::<ConsValue>().is_some()
                || any.downcast_ref::<KeySeqValue>().is_some()
                || any.downcast_ref::<ValSeqValue>().is_some()
        }
        _ => false,
    }
}

/// Force a pending cell one step (memoized; errors restore the
/// pending state so retries re-run). The lock never spans
/// evaluation.
fn force_pending(eval: &mut Evaluator, p: &PendingSeq) -> Result<Forced, Signal> {
    let thunk = {
        let mut state = p.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            SeqState::Empty => return Ok(Forced::Empty),
            SeqState::Chunk { items, pos, tail } => {
                return Ok(Forced::Chunk {
                    items: Arc::from(&items[*pos..]),
                    tail: tail.clone(),
                });
            }
            SeqState::Pending(_) => {}
        }
        match std::mem::replace(&mut *state, SeqState::Empty) {
            SeqState::Pending(t) => t,
            // Unreachable: matched pending above (single-threaded).
            other => {
                *state = other;
                return Ok(Forced::Empty);
            }
        }
    };
    match step_thunk(eval, (*thunk).clone()) {
        Ok(forced @ (Forced::Empty | Forced::Chunk { .. })) => {
            let mut state = p.state.lock().unwrap_or_else(|e| e.into_inner());
            *state = match &forced {
                Forced::Empty => SeqState::Empty,
                Forced::Chunk { items, tail } => SeqState::Chunk {
                    items: Arc::clone(items),
                    pos: 0,
                    tail: tail.clone(),
                },
            };
            Ok(forced)
        }
        Err(e) => {
            let mut state = p.state.lock().unwrap_or_else(|e| e.into_inner());
            *state = SeqState::Pending(thunk);
            Err(e)
        }
    }
}

/// Step one input: empty, one head, or a whole chunk (chunked
/// inputs yield their current chunk up to [`CHUNK_SIZE`]).
pub(crate) fn step_unit(eval: &mut Evaluator, v: &Value) -> Result<StepUnit, Signal> {
    match v {
        Value::Nil => Ok(StepUnit::Empty),
        Value::Extension(_) => {
            if let Some(p) = as_pending(v) {
                return match force_pending(eval, p)? {
                    Forced::Empty => Ok(StepUnit::Empty),
                    Forced::Chunk { items, tail } => {
                        if items.is_empty() {
                            step_unit(eval, &tail)
                        } else {
                            Ok(StepUnit::Chunk { items, tail })
                        }
                    }
                };
            }
            if let Some(s) = as_realized(v) {
                return step_seq(s.seq(), s.is_counted(), s.is_idrop(), s.kind(), s.meta());
            }
            if let Some(r) = as_range(v) {
                return step_range_value(eval, r);
            }
            if let Some(r) = as_repeat(v) {
                return step_repeat_value(r);
            }
            if let Some(c) = as_cycle(v) {
                return step_cycle_value(eval, c);
            }
            if let Some(it) = as_iterate(v) {
                return step_iterate_value(eval, it);
            }
            if let Some(c) = as_cons_value(v) {
                return step_cons_value(eval, c);
            }
            if let Some(k) = as_keys(v) {
                return step_keys_unit(eval, k.underlying(), true);
            }
            if let Some(k) = as_vals(v) {
                return step_keys_unit(eval, k.underlying(), false);
            }
            if let Some(sv) = as_subvector(v) {
                return step_realized(&sv.items(), false).map_or_else(
                    || Ok(StepUnit::Empty),
                    |s| step_seq(&s, true, false, SeqKind::Subvec, None),
                );
            }
            Err(eval.throw(iseq_err(v)))
        }
        Value::List(l) => match l.first() {
            None => Ok(StepUnit::Empty),
            Some(head) => {
                let tail = l
                    .drop_first()
                    .map(|t| Value::List(t.with_meta(None)))
                    .unwrap_or_else(|| Value::List(List::new_sync()));
                Ok(StepUnit::One {
                    head: head.clone(),
                    tail,
                })
            }
        },
        Value::Vector(x) => step_realized(&x.iter().cloned().collect::<Vec<_>>(), true)
            .map_or_else(
                || Ok(StepUnit::Empty),
                |s| step_seq(&s, true, true, SeqKind::Chunked, None),
            ),
        Value::Map(m) => {
            let entries: Vec<Value> = m
                .iter()
                .map(|(k, val)| Value::Vector(Vector::from_vec(vec![k.clone(), val.clone()])))
                .collect();
            let small = !m.is_hashy();
            let kind = if small { SeqKind::Map } else { SeqKind::Key };
            step_realized(&entries, false).map_or_else(
                || Ok(StepUnit::Empty),
                |s| step_seq(&s, small, small, kind, None),
            )
        }
        Value::Set(s) => step_realized(&s.iter().cloned().collect::<Vec<_>>(), false).map_or_else(
            || Ok(StepUnit::Empty),
            |s| step_seq(&s, false, false, SeqKind::Key, None),
        ),
        Value::Str(s) => {
            let chars: Vec<Value> = s.chars().map(Value::Char).collect();
            step_realized(&chars, false).map_or_else(
                || Ok(StepUnit::Empty),
                |s| step_seq(&s, true, true, SeqKind::Str, None),
            )
        }
        _ => Err(eval.throw(iseq_err(v))),
    }
}

/// Step a pure seq: chunked seqs yield the current chunk, anything
/// else one head. Tails inherit the JVM interfaces and class
/// (`more` preserves them); only `StringSeq` tails keep metadata
/// (like `StringSeq.next` — every other `next` drops it).
fn step_seq(
    s: &Seq,
    counted: bool,
    idrop: bool,
    kind: SeqKind,
    meta: Option<&Map>,
) -> Result<StepUnit, Signal> {
    if let Some((chunk, rest)) = s.chunk_next() {
        if chunk.is_empty() {
            return step_seq(&rest, counted, idrop, kind, meta);
        }
        let tail = Value::Extension(Arc::new(seq_tail(rest, counted, idrop, kind, meta)));
        return Ok(StepUnit::Chunk {
            items: Arc::from(chunk),
            tail,
        });
    }
    match s.first() {
        None => Ok(StepUnit::Empty),
        Some(head) => {
            let tail = Value::Extension(Arc::new(seq_tail(s.more(), counted, idrop, kind, meta)));
            Ok(StepUnit::One {
                head: head.clone(),
                tail,
            })
        }
    }
}

/// A stepped tail seq (metadata kept only for `StringSeq`).
fn seq_tail(s: Seq, counted: bool, idrop: bool, kind: SeqKind, meta: Option<&Map>) -> RealizedSeq {
    let keep = if matches!(kind, SeqKind::Str) {
        meta.cloned()
    } else {
        None
    };
    RealizedSeq::flagged(s, counted, idrop, kind).with_meta(keep)
}

/// Build a pure seq from items (chunked for vectors, eager
/// otherwise), or `None` when empty.
fn step_realized(items: &[Value], chunked: bool) -> Option<Seq> {
    if chunked {
        Seq::chunked(items.to_vec())
    } else {
        Seq::eager(items.to_vec())
    }
}

/// Step a range: up to [`CHUNK_SIZE`] verbatim heads plus the
/// shifted range (nil tail when done). An `Int`+`Int` overflow
/// ends the walk (like `Range.forceChunk`, whose promoting
/// `Numbers.addP` always lands out of bounds past `i64`).
fn step_range_value(_eval: &mut Evaluator, r: &RangeValue) -> Result<StepUnit, Signal> {
    let mut items = Vec::new();
    let mut cur = r.clone();
    let mut tail = Value::Nil;
    while items.len() < CHUNK_SIZE {
        let Some((head, next)) = cur.stepped() else {
            break;
        };
        items.push(head);
        match next {
            Some(nxt) => {
                tail = Value::Extension(Arc::new(nxt.clone()));
                cur = nxt;
            }
            None => {
                tail = Value::Nil;
                break;
            }
        }
    }
    if items.is_empty() {
        return Ok(StepUnit::Empty);
    }
    if cur.done() {
        tail = Value::Nil;
    }
    Ok(StepUnit::Chunk {
        items: Arc::from(items),
        tail,
    })
}

/// Step a repeat: one head plus the decremented repeat (infinite
/// repeats return themselves; tails drop metadata like
/// `Repeat.next`).
fn step_repeat_value(r: &RepeatValue) -> Result<StepUnit, Signal> {
    match r.n() {
        Some(n) if n <= 0 => Ok(StepUnit::Empty),
        Some(n) => Ok(StepUnit::One {
            head: r.x().clone(),
            tail: Value::Extension(Arc::new(RepeatValue::new(r.x().clone(), Some(n - 1)))),
        }),
        None => Ok(StepUnit::One {
            head: r.x().clone(),
            tail: Value::Extension(Arc::new(RepeatValue::new(r.x().clone(), None))),
        }),
    }
}

/// Step a cycle: the head of `current`, restarting from `prev`
/// when it runs out (tails drop metadata like `Cycle.next`).
fn step_cycle_value(eval: &mut Evaluator, c: &CycleValue) -> Result<StepUnit, Signal> {
    if let Some((head, tail)) = step_head(eval, c.current())? {
        let next = CycleValue::new(c.prev().clone(), tail);
        return Ok(StepUnit::One {
            head,
            tail: Value::Extension(Arc::new(next)),
        });
    }
    if matches!(c.prev(), Value::Nil) {
        return Ok(StepUnit::Empty);
    }
    if let Some((head, tail)) = step_head(eval, c.prev())? {
        let next = CycleValue::new(c.prev().clone(), tail);
        return Ok(StepUnit::One {
            head,
            tail: Value::Extension(Arc::new(next)),
        });
    }
    Ok(StepUnit::Empty)
}

/// Read an iterate head, realizing the seed at most once (like
/// `Iterate.first`, memoized). A nil step fn is the JVM's field
/// NPE (construction only checks non-nil callees, like
/// `Iterate.create`'s reflectively-found `IFn` cast).
fn iterate_seed(eval: &mut Evaluator, it: &IterateValue) -> Result<Value, Signal> {
    if let Some(seed) = it.seed_opt() {
        return Ok(seed);
    }
    if matches!(it.f(), Value::Nil) {
        return Err(eval.throw(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"this.f\" is null"
                .to_owned(),
        )));
    }
    crate::core::check_ifn(eval, it.f())?;
    let seed = eval.call(it.f(), std::slice::from_ref(it.prev()))?;
    it.store_seed(seed.clone());
    Ok(seed)
}

/// Step an iterate: realize this head (at most one `f` call, like
/// `next` calling `first`) plus a deferred tail (no call, like
/// `next`'s unrealized seed). Tails drop metadata.
fn step_iterate_value(eval: &mut Evaluator, it: &IterateValue) -> Result<StepUnit, Signal> {
    let head = iterate_seed(eval, it)?;
    let tail = IterateValue::deferred(it.f().clone(), head.clone());
    Ok(StepUnit::One {
        head,
        tail: Value::Extension(Arc::new(tail)),
    })
}

/// Step a cons: the head plus the tail untouched (like
/// `Cons.more`, which never seqs — `cons` already did).
fn step_cons_value(_eval: &mut Evaluator, c: &ConsValue) -> Result<StepUnit, Signal> {
    Ok(StepUnit::One {
        head: c.head().clone(),
        tail: c.tail().clone(),
    })
}

/// Extract one entry projection (keys take index 0, vals index 1).
/// Map entries read as 2-vectors; anything else fails like the
/// JVM's `Map.Entry` cast.
fn entry_of(item: &Value, keys: bool) -> Result<Value, NativeError> {
    match item {
        Value::Vector(v) if v.len() == 2 => {
            let idx = usize::from(!keys);
            Ok(v.iter().nth(idx).cloned().unwrap_or(Value::Nil))
        }
        _ => Err(crate::core::cce_err(item, "java.util.Map$Entry")),
    }
}

/// Step an entry view: extract the head projection (casting, like
/// `KeySeq.first`) plus the rewrapped underlying tail.
fn step_keys_unit(
    eval: &mut Evaluator,
    underlying: &Value,
    keys: bool,
) -> Result<StepUnit, Signal> {
    let Some((head, tail)) = step_head(eval, underlying)? else {
        return Ok(StepUnit::Empty);
    };
    let head = entry_of(&head, keys).map_err(|e| eval.throw(e))?;
    // Provably-spent underlyings nil-pun (like `KeySeq.create`).
    let tail = if keys_empty(&tail) {
        Value::Nil
    } else if keys {
        Value::Extension(Arc::new(KeySeqValue::new(tail)))
    } else {
        Value::Extension(Arc::new(ValSeqValue::new(tail)))
    };
    Ok(StepUnit::One { head, tail })
}

/// An entry-view head (nil for spent underlyings; casting, like
/// `KeySeq.first`).
fn keys_first(eval: &mut Evaluator, underlying: &Value, keys: bool) -> Result<Value, Signal> {
    match step_head(eval, underlying)? {
        None => Ok(Value::Nil),
        Some((head, _)) => entry_of(&head, keys).map_err(|e| eval.throw(e)),
    }
}

/// An entry-view rest (advancing never casts, like
/// `KeySeq.next`; spent underlyings rest to `()`).
fn keys_rest(eval: &mut Evaluator, underlying: &Value, keys: bool) -> Result<Value, Signal> {
    let mut cur = underlying.clone();
    let tail = loop {
        match step_unit(eval, &cur)? {
            StepUnit::Empty => break None,
            StepUnit::One { tail, .. } => break Some(tail),
            StepUnit::Chunk { items, tail } => {
                if items.len() > 1 {
                    break Some(chain_tail(&items[1..], &tail));
                }
                if items.is_empty() {
                    cur = tail;
                } else {
                    break Some(tail);
                }
            }
        }
    };
    match tail {
        None => Ok(empty_list_value()),
        Some(t) if keys_empty(&t) => Ok(empty_list_value()),
        Some(t) if keys => Ok(Value::Extension(Arc::new(KeySeqValue::new(t)))),
        Some(t) => Ok(Value::Extension(Arc::new(ValSeqValue::new(t)))),
    }
}

/// A realized seq value from items (nil-punning when empty),
/// with explicit JVM interfaces and class.
pub(crate) fn realized_value(
    items: Vec<Value>,
    chunked: bool,
    counted: bool,
    idrop: bool,
    kind: SeqKind,
) -> Value {
    match step_realized(&items, chunked) {
        Some(s) => Value::Extension(Arc::new(RealizedSeq::flagged(s, counted, idrop, kind))),
        None => Value::Nil,
    }
}

/// Run one thunk step: empty, or a chunk (at most [`CHUNK_SIZE`]
/// items) plus its tail value.
fn step_thunk(eval: &mut Evaluator, thunk: Thunk) -> Result<Forced, Signal> {
    match thunk {
        Thunk::Map { f, inputs } => step_map(eval, &f, inputs),
        Thunk::Filter { f, input } => step_filter(eval, &f, input, false),
        Thunk::Remove { f, input } => step_filter(eval, &f, input, true),
        Thunk::Take { n, input } => step_take(eval, &n, input),
        Thunk::Concat { inputs } => step_concat(eval, inputs),
        Thunk::MapCat { outer } => step_mapcat(eval, outer),
        Thunk::Chain { head, tail } => {
            if head.is_empty() {
                step_value_forced(eval, &tail)
            } else {
                Ok(Forced::Chunk {
                    items: Arc::from(head),
                    tail,
                })
            }
        }
        Thunk::Thunk0 { body, env } => step_thunk0(eval, &body, &env),
        Thunk::Repeatedly { f } => step_repeatedly(eval, &f),
        Thunk::Interleave { queue, inputs } => step_interleave(eval, queue, inputs),
        Thunk::Interpose { sep, input, first } => step_interpose(eval, sep, input, first),
        Thunk::TakeWhile { f, input } => step_take_while(eval, &f, input),
        Thunk::DropWhile { f, input } => step_drop_while(eval, &f, input),
        Thunk::Drop { n, input } => step_drop(eval, &n, input),
        Thunk::TakeNth { n, input } => step_take_nth(eval, &n, input),
        Thunk::Partition {
            n,
            step,
            pad,
            input,
        } => step_partition_collect(eval, &n, &step, pad, input),
        Thunk::PartitionAll { n, step, input } => {
            step_partition_all_collect(eval, &n, &step, input)
        }
        Thunk::PartitionBy { f, input } => step_partition_by(eval, &f, input),
        Thunk::PartitionByAdvance { f, run, s } => step_partition_by_advance(eval, &f, &run, s),
        Thunk::PartitionRun { f, fv, input } => step_partition_run(eval, &f, &fv, input),
        Thunk::Distinct { seen, input } => step_distinct(eval, seen, input),
        Thunk::Dedupe { last, input } => step_dedupe(eval, last, input),
        Thunk::Keep { f, input } => step_keep(eval, &f, input),
        Thunk::KeepIndexed { f, input, idx } => step_keep_indexed(eval, &f, input, idx),
        Thunk::MapIndexed { f, input, idx } => step_map_indexed(eval, &f, input, idx),
        Thunk::Reductions {
            f,
            acc,
            has_init,
            input,
            started,
        } => step_reductions(eval, &f, acc, has_init, input, started),
        Thunk::Replace { smap, input } => step_replace(eval, &smap, input),
    }
}

/// Step a value into a forced chunk (for chain tails).
fn step_value_forced(eval: &mut Evaluator, v: &Value) -> Result<Forced, Signal> {
    match step_unit(eval, v)? {
        StepUnit::Empty => Ok(Forced::Empty),
        StepUnit::One { head, tail } => Ok(Forced::Chunk {
            items: Arc::from(vec![head]),
            tail,
        }),
        StepUnit::Chunk { items, tail } => Ok(Forced::Chunk { items, tail }),
    }
}

/// Step one head plus its remainder (chunk remainders re-chain
/// ahead of the tail; empty chunks fall through to the tail).
fn step_head(eval: &mut Evaluator, input: &Value) -> Result<Option<(Value, Value)>, Signal> {
    let mut cur = input.clone();
    loop {
        match step_unit(eval, &cur)? {
            StepUnit::Empty => return Ok(None),
            StepUnit::One { head, tail } => return Ok(Some((head, tail))),
            StepUnit::Chunk { items, tail } => match items.split_first() {
                Some((head, rest)) => {
                    return Ok(Some((head.clone(), chain_tail(rest, &tail))));
                }
                None => cur = tail,
            },
        }
    }
}

/// Whether `drop`/`nthrest` take the eager `IDrop` path: vectors,
/// small maps, `LongRange`, `Repeat`, and the `IDrop` realized
/// seqs (`ChunkedSeq`, `StringSeq`, small-map seqs).
pub(crate) fn is_idrop(v: &Value) -> bool {
    match v {
        Value::Vector(_) => true,
        Value::Map(m) => !m.is_hashy(),
        Value::Extension(_) => {
            if let Some(r) = as_range(v) {
                return r.is_long();
            }
            if as_repeat(v).is_some() {
                return true;
            }
            if let Some(s) = as_realized(v) {
                return matches!(s.kind(), SeqKind::Chunked | SeqKind::Str | SeqKind::Map);
            }
            false
        }
        _ => false,
    }
}

/// Eager `IDrop` shift by a positive `i32` count (metadata always
/// dropped, like every `.drop`; `None` past the end, like the
/// JVM's null). Callers only pass `IDrop` values.
fn idrop_shift(v: &Value, n: i32) -> Option<Value> {
    let n64 = i64::from(n);
    match v {
        Value::Vector(x) => {
            if n64 < 0 || n64 >= x.len() as i64 {
                return None;
            }
            let rest: Vec<Value> = x.iter().skip(n as usize).cloned().collect();
            Some(realized_value(rest, true, true, true, SeqKind::Chunked))
        }
        Value::Map(m) if !m.is_hashy() => {
            let entries: Vec<Value> = m
                .iter()
                .map(|(k, val)| Value::Vector(Vector::from_vec(vec![k.clone(), val.clone()])))
                .collect();
            if n64 < 0 || n64 >= entries.len() as i64 {
                return None;
            }
            Some(realized_value(
                entries[n as usize..].to_vec(),
                false,
                true,
                true,
                SeqKind::Map,
            ))
        }
        Value::Extension(_) => {
            if let Some(r) = as_range(v) {
                if !r.is_long() {
                    return None;
                }
                let (RangeNum::Int(start), RangeNum::Int(step)) = (r.cur(), r.step()) else {
                    return None;
                };
                let count = r.count().unwrap_or(0);
                if n64 >= count {
                    return None;
                }
                let shifted = RangeValue::new(
                    RangeNum::Int(start.wrapping_add(step.wrapping_mul(n64))),
                    r.end(),
                    RangeNum::Int(step),
                    true,
                    Some(count - n64),
                );
                return Some(Value::Extension(Arc::new(shifted)));
            }
            if let Some(r) = as_repeat(v) {
                match r.n() {
                    // Infinite drops return `this` (metadata kept).
                    None => return Some(v.clone()),
                    Some(c) if c - n64 > 0 => {
                        return Some(Value::Extension(Arc::new(RepeatValue::new(
                            r.x().clone(),
                            Some(c - n64),
                        ))));
                    }
                    _ => return None,
                }
            }
            if let Some(s) = as_realized(v) {
                if !matches!(s.kind(), SeqKind::Chunked | SeqKind::Str | SeqKind::Map) {
                    return None;
                }
                let mut cur = s.seq().clone();
                for _ in 0..n64 {
                    cur = cur.next()?;
                }
                if cur.is_empty() {
                    return None;
                }
                return Some(Value::Extension(Arc::new(RealizedSeq::flagged(
                    cur,
                    s.is_counted(),
                    s.is_idrop(),
                    s.kind(),
                ))));
            }
            None
        }
        _ => None,
    }
}

/// `(drop n coll)`: `IDrop` inputs validate `n` (like `pos?`) and
/// drop eagerly — non-positive counts `seq` eagerly, past-end
/// drops answer `()`; everything else stays a lazy `Drop` thunk
/// (counts validated when forced).
pub(crate) fn drop_value(eval: &mut Evaluator, n: &Value, coll: Value) -> Result<Value, Signal> {
    if !is_idrop(&coll) {
        return Ok(drop_thunk(n.clone(), coll));
    }
    let count = crate::core::take_count(n).map_err(|e| eval.throw(e))?;
    if count <= 0 {
        return match seq_value(eval, &coll)? {
            Value::Nil => Ok(empty_list_value()),
            s => Ok(s),
        };
    }
    let n32 = crate::core::idrop_coerce(n).map_err(|e| eval.throw(e))?;
    Ok(idrop_shift(&coll, n32).unwrap_or_else(empty_list_value))
}

/// `nthrest`: `(pos? n)` first — eager, before coll dispatch (nil
/// is the `getClass` NPE, non-numbers the `Number` CCE, on vectors
/// AND lists). Non-positive `n` returns the coll ITSELF (never
/// seqed); `IDrop` colls shift eagerly (out-of-`i32` fails, like
/// `drop`); anything else walks the JVM's strict
/// `(loop [n n xs coll] ...)` — huge `n` walks to the end silently
/// (no `AE`/`IAE`), and the `(or … ())` maps empty to `()`.
pub(crate) fn nthrest_value(eval: &mut Evaluator, coll: Value, n: &Value) -> Result<Value, Signal> {
    let mut cur = match n {
        Value::Int(i) => *i as f64,
        Value::Double(d) => *d,
        Value::Nil => return Err(eval.throw(crate::core::npe_getclass())),
        _ => return Err(eval.throw(crate::core::cce_err(n, "java.lang.Number"))),
    };
    if cur <= 0.0 || cur.is_nan() {
        return Ok(coll);
    }
    if is_idrop(&coll) {
        let n32 = crate::core::idrop_coerce(n).map_err(|e| eval.throw(e))?;
        return Ok(idrop_shift(&coll, n32).unwrap_or_else(empty_list_value));
    }
    // The strict walk (`(dec n)` per step as `f64`: exact for small
    // counts; huge counts on finite colls exit via exhaustion, on
    // infinite colls hang — both like the JVM loop).
    let mut xs = coll;
    loop {
        if cur <= 0.0 || cur.is_nan() {
            break;
        }
        let s = seq_value(eval, &xs)?;
        if matches!(s, Value::Nil) {
            xs = Value::Nil;
            break;
        }
        cur -= 1.0;
        xs = rest_value(eval, &s)?;
    }
    match seq_value(eval, &xs)? {
        Value::Nil => Ok(empty_list_value()),
        s => Ok(s),
    }
}

/// `map`: step every input; the first input's unit sets the width
/// (whole chunks for chunked first inputs, one item otherwise);
/// other inputs walk to the same width; the shortest wins.
fn step_map(eval: &mut Evaluator, f: &Value, inputs: Vec<Value>) -> Result<Forced, Signal> {
    if inputs.len() == 1 {
        return step_map_one(eval, f, inputs.into_iter().next().unwrap_or(Value::Nil));
    }
    if inputs.is_empty() {
        return Ok(Forced::Empty);
    }
    // Multi-input `map` is one call per step (the arities `cons`
    // per item; only the single-input arity chunks).
    let mut heads = Vec::with_capacity(inputs.len());
    let mut tails = Vec::with_capacity(inputs.len());
    for input in &inputs {
        let Some((head, tail)) = step_head(eval, input)? else {
            return Ok(Forced::Empty);
        };
        heads.push(head);
        tails.push(tail);
    }
    crate::core::check_ifn(eval, f)?;
    let out = eval.call(f, &heads)?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![out]),
        tail: PendingSeq::pending(Thunk::Map {
            f: f.clone(),
            inputs: tails,
        }),
    })
}

/// Single-input `map`: chunked inputs apply the whole chunk per
/// step, anything else one item.
fn step_map_one(eval: &mut Evaluator, f: &Value, input: Value) -> Result<Forced, Signal> {
    let (items, tail) = match step_unit(eval, &input)? {
        StepUnit::Empty => return Ok(Forced::Empty),
        StepUnit::One { head, tail } => (vec![head], tail),
        StepUnit::Chunk { items, tail } => (items.to_vec(), tail),
    };
    crate::core::check_ifn(eval, f)?;
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        out.push(eval.call(f, std::slice::from_ref(item))?);
    }
    Ok(Forced::Chunk {
        items: Arc::from(out),
        tail: PendingSeq::pending(Thunk::Map {
            f: f.clone(),
            inputs: vec![tail],
        }),
    })
}

/// Re-chain an unconsumed remainder ahead of its tail.
fn chain_tail(remainder: &[Value], tail: &Value) -> Value {
    if remainder.is_empty() {
        tail.clone()
    } else {
        PendingSeq::pending(Thunk::Chain {
            head: remainder.to_vec(),
            tail: tail.clone(),
        })
    }
}

/// `filter`/`remove`: consume whole units, testing each item in
/// order, until a non-empty chunk (or the end).
fn step_filter(
    eval: &mut Evaluator,
    f: &Value,
    mut input: Value,
    invert: bool,
) -> Result<Forced, Signal> {
    loop {
        let (items, tail) = match step_unit(eval, &input)? {
            StepUnit::Empty => return Ok(Forced::Empty),
            StepUnit::One { head, tail } => (vec![head], tail),
            StepUnit::Chunk { items, tail } => (items.to_vec(), tail),
        };
        crate::core::check_ifn(eval, f)?;
        let mut kept = Vec::new();
        for item in &items {
            let pass = eval.call(f, std::slice::from_ref(item))?.is_truthy() != invert;
            if pass {
                kept.push(item.clone());
            }
        }
        if kept.is_empty() {
            input = tail;
        } else {
            let next = if invert {
                Thunk::Remove {
                    f: f.clone(),
                    input: tail,
                }
            } else {
                Thunk::Filter {
                    f: f.clone(),
                    input: tail,
                }
            };
            return Ok(Forced::Chunk {
                items: Arc::from(kept),
                tail: PendingSeq::pending(next),
            });
        }
    }
}

/// `take`: one step consumes the head of the current unit.
fn step_take(eval: &mut Evaluator, n: &Value, input: Value) -> Result<Forced, Signal> {
    // `(pos? n)` per step (the count re-validates every force);
    // one item per step (the body `cons`es).
    let n = crate::core::take_count(n).map_err(|e| eval.throw(e))?;
    if n <= 0 {
        return Ok(Forced::Empty);
    }
    let Some((head, tail)) = step_head(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    Ok(Forced::Chunk {
        items: Arc::from(vec![head]),
        tail: PendingSeq::pending(Thunk::Take {
            n: Value::Int(n - 1),
            input: tail,
        }),
    })
}

/// `concat`: pass the first non-empty input's unit through,
/// chaining its tail back ahead of the rest.
fn step_concat(eval: &mut Evaluator, mut inputs: Vec<Value>) -> Result<Forced, Signal> {
    while let Some(first) = inputs.first() {
        match step_unit(eval, first)? {
            StepUnit::Empty => {
                inputs.remove(0);
            }
            StepUnit::One { head, tail } => {
                inputs[0] = tail;
                return Ok(Forced::Chunk {
                    items: Arc::from(vec![head]),
                    tail: PendingSeq::pending(Thunk::Concat { inputs }),
                });
            }
            StepUnit::Chunk { items, tail } => {
                inputs[0] = tail;
                return Ok(Forced::Chunk {
                    items,
                    tail: PendingSeq::pending(Thunk::Concat { inputs }),
                });
            }
        }
    }
    Ok(Forced::Empty)
}

/// Step a `mapcat` outer seq: each outer item is seq'd and
/// spliced (`(mapcat f xs)` ≡ `(apply concat (map f xs))`), so
/// nil/empty items vanish and non-seqables fail exactly like
/// `concat` would. Chunks peel one item and re-chain the rest.
fn step_mapcat(eval: &mut Evaluator, outer: Value) -> Result<Forced, Signal> {
    let mut outer = outer;
    loop {
        match step_unit(eval, &outer)? {
            StepUnit::Empty => return Ok(Forced::Empty),
            StepUnit::One { head, tail } => {
                let s = seq_value(eval, &head)?;
                if matches!(s, Value::Nil) {
                    outer = tail;
                } else {
                    let cont = pending_value(Thunk::MapCat { outer: tail });
                    return splice_first(eval, &s, cont);
                }
            }
            StepUnit::Chunk { items, tail } => {
                let mut it = items.iter();
                let Some(head) = it.next() else {
                    outer = tail;
                    continue;
                };
                let rest: Vec<Value> = it.cloned().collect();
                let cont = pending_value(Thunk::Chain { head: rest, tail });
                let s = seq_value(eval, head)?;
                if matches!(s, Value::Nil) {
                    outer = cont;
                } else {
                    let cont = pending_value(Thunk::MapCat { outer: cont });
                    return splice_first(eval, &s, cont);
                }
            }
        }
    }
}

/// Yield the first unit of a spliced seq, chaining its tail ahead
/// of the mapcat continuation.
fn splice_first(eval: &mut Evaluator, s: &Value, cont: Value) -> Result<Forced, Signal> {
    match step_unit(eval, s)? {
        StepUnit::Empty => step_value_forced(eval, &cont),
        StepUnit::One { head, tail } => Ok(Forced::Chunk {
            items: Arc::from(vec![head]),
            tail: PendingSeq::pending(Thunk::Concat {
                inputs: vec![tail, cont],
            }),
        }),
        StepUnit::Chunk { items, tail } => Ok(Forced::Chunk {
            items,
            tail: PendingSeq::pending(Thunk::Concat {
                inputs: vec![tail, cont],
            }),
        }),
    }
}

/// Step one unit into an item vector plus its tail (empty inputs
/// end the step).
fn step_items(eval: &mut Evaluator, input: &Value) -> Result<Option<(Vec<Value>, Value)>, Signal> {
    match step_unit(eval, input)? {
        StepUnit::Empty => Ok(None),
        StepUnit::One { head, tail } => Ok(Some((vec![head], tail))),
        StepUnit::Chunk { items, tail } => Ok(Some((items.to_vec(), tail))),
    }
}

/// per realization).
fn step_repeatedly(eval: &mut Evaluator, f: &Value) -> Result<Forced, Signal> {
    // Always infinite here (the `n` arity routes through `take`).
    crate::core::check_ifn(eval, f)?;
    let item = eval.call(f, &[])?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![item]),
        tail: PendingSeq::pending(Thunk::Repeatedly { f: f.clone() }),
    })
}

/// `interleave`: one round (one head per input); any exhausted
/// input ends the whole seq.
fn step_interleave(
    eval: &mut Evaluator,
    mut queue: Vec<Value>,
    inputs: Vec<Value>,
) -> Result<Forced, Signal> {
    // One item per step: drain the queued round first, else step
    // every input together (any exhaustion ends the round).
    if !queue.is_empty() {
        let head = queue.remove(0);
        return Ok(Forced::Chunk {
            items: Arc::from(vec![head]),
            tail: PendingSeq::pending(Thunk::Interleave { queue, inputs }),
        });
    }
    if inputs.is_empty() {
        return Ok(Forced::Empty);
    }
    let mut heads = Vec::with_capacity(inputs.len());
    let mut next = Vec::with_capacity(inputs.len());
    for input in &inputs {
        let Some((head, tail)) = step_head(eval, input)? else {
            return Ok(Forced::Empty);
        };
        heads.push(head);
        next.push(tail);
    }
    let mut rounds = heads.into_iter();
    let Some(head) = rounds.next() else {
        return Ok(Forced::Empty);
    };
    Ok(Forced::Chunk {
        items: Arc::from(vec![head]),
        tail: PendingSeq::pending(Thunk::Interleave {
            queue: rounds.collect(),
            inputs: next,
        }),
    })
}

/// `interpose`: the head alone first, then one item per step
/// (a separator, then its head — no trailing separator).
fn step_interpose(
    eval: &mut Evaluator,
    sep: Value,
    input: Value,
    first: bool,
) -> Result<Forced, Signal> {
    let Some((head, tail)) = step_head(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    if first {
        return Ok(Forced::Chunk {
            items: Arc::from(vec![head]),
            tail: PendingSeq::pending(Thunk::Interpose {
                sep,
                input: tail,
                first: false,
            }),
        });
    }
    Ok(Forced::Chunk {
        items: Arc::from(vec![sep.clone()]),
        tail: chain_tail(
            std::slice::from_ref(&head),
            &PendingSeq::pending(Thunk::Interpose {
                sep,
                input: tail,
                first: false,
            }),
        ),
    })
}

/// `take-while`: one test per step (the JVM tests per item);
/// failure drops the remainder.
fn step_take_while(eval: &mut Evaluator, f: &Value, input: Value) -> Result<Forced, Signal> {
    let Some((head, tail)) = step_head(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    crate::core::check_ifn(eval, f)?;
    if !eval.call(f, std::slice::from_ref(&head))?.is_truthy() {
        return Ok(Forced::Empty);
    }
    Ok(Forced::Chunk {
        items: Arc::from(vec![head]),
        tail: PendingSeq::pending(Thunk::TakeWhile {
            f: f.clone(),
            input: tail,
        }),
    })
}

/// `drop-while`: skip passing heads (every test runs in the first
/// step, like the JVM's walk), then pass the failing head and
/// its remainder through.
fn step_drop_while(eval: &mut Evaluator, f: &Value, mut input: Value) -> Result<Forced, Signal> {
    loop {
        let Some((head, tail)) = step_head(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        crate::core::check_ifn(eval, f)?;
        if !eval.call(f, std::slice::from_ref(&head))?.is_truthy() {
            return Ok(Forced::Chunk {
                items: Arc::from(vec![head]),
                tail,
            });
        }
        input = tail;
    }
}

/// `drop`: validate the count when forced, skip that many heads,
/// then pass the remainder through.
fn step_drop(eval: &mut Evaluator, n: &Value, mut input: Value) -> Result<Forced, Signal> {
    let mut skip = crate::core::take_count(n)
        .map_err(|e| eval.throw(e))?
        .max(0);
    while skip > 0 {
        let Some((_, tail)) = step_head(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        input = tail;
        skip -= 1;
    }
    step_value_forced(eval, &input)
}

/// `take-nth`: every step seqs, takes the head, and evaluates
/// `(drop n s)` for the tail — exactly the JVM body, so `IDrop`
/// inputs validate eagerly on the first force (non-positive `n`
/// replays the head forever, like the JVM's zero-drop loop).
fn step_take_nth(eval: &mut Evaluator, n: &Value, input: Value) -> Result<Forced, Signal> {
    let s = seq_value(eval, &input)?;
    let Some((head, _)) = step_head(eval, &s)? else {
        return Ok(Forced::Empty);
    };
    let dropped = drop_value(eval, n, s)?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![head]),
        tail: PendingSeq::pending(Thunk::TakeNth {
            n: n.clone(),
            input: dropped,
        }),
    })
}

/// The eager `(nthrest s step)` past a collected window (the
/// `cons` arg evaluates on the SAME force as the window, so the
/// step validates here): non-positive steps replay the window
/// start, overlaps re-chain the window tail, gaps skip ahead.
/// `IDrop` inputs also run the `.drop` count coercion (out of
/// `i32` range fails, like `nthrest` on a vector).
fn partition_next(
    eval: &mut Evaluator,
    window: &[Value],
    rest: Value,
    s_idrop: bool,
    step: &Value,
) -> Result<Value, Signal> {
    let step_count = crate::core::take_count(step).map_err(|e| eval.throw(e))?;
    if step_count <= 0 {
        return Ok(chain_tail(window, &rest));
    }
    if s_idrop {
        // The `.drop` count coercion on the RAW step (huge doubles
        // fail the long range check, like `nthrest`).
        match step {
            Value::Int(i) => {
                crate::core::int_exact_long(*i).map_err(|e| eval.throw(e))?;
            }
            Value::Double(d) => {
                crate::core::int_cast_double(d.ceil()).map_err(|e| eval.throw(e))?;
            }
            // Unreachable: `take_count` above already rejected
            // non-numbers.
            _ => {}
        }
    }
    let step_usize = step_count as usize;
    if step_usize >= window.len() {
        let mut skip = step_usize - window.len();
        let mut tail = rest;
        while skip > 0 {
            let Some((_, rest)) = step_head(eval, &tail)? else {
                break;
            };
            tail = rest;
            skip -= 1;
        }
        Ok(tail)
    } else {
        Ok(chain_tail(&window[step_usize..], &rest))
    }
}

/// A `partition` window: a realized lazy seq (the JVM's `doall`
/// of `take`), never a list.
fn lazy_window(eval: &mut Evaluator, items: Vec<Value>) -> Result<Value, Signal> {
    let pending = PendingSeq::pending(Thunk::Chain {
        head: items,
        tail: Value::Nil,
    });
    let Some(p) = as_pending(&pending) else {
        return Ok(pending);
    };
    force_pending(eval, p)?;
    Ok(pending)
}

/// Whether a collected window counts as full: the JVM compares
/// the RAW `n` with `=` (so `2.5` never fills, `3.0` does).
fn window_full(n: &Value, len: usize) -> bool {
    match n {
        Value::Int(i) => *i == len as i64,
        Value::Double(d) => *d == len as f64,
        _ => false,
    }
}

/// `partition`: seq-check, collect one window, and advance
/// eagerly (the `(nthrest s step)` arg evaluates on the SAME
/// force, so the step validates with the first window). Fullness
/// compares the raw `n`, so zero widths emit infinite empties on
/// non-empty input. Short windows pad when a pad was given —
/// `(take n (concat p pad))`, ending the run — and drop
/// otherwise (never validating the step).
fn step_partition_collect(
    eval: &mut Evaluator,
    n: &Value,
    step: &Value,
    pad: Option<Value>,
    input: Value,
) -> Result<Forced, Signal> {
    let Some(s) = seq_opt(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    let s_idrop = is_idrop(&s);
    let width = crate::core::take_count(n)
        .map_err(|e| eval.throw(e))?
        .max(0) as usize;
    let mut window = Vec::with_capacity(width.min(CHUNK_SIZE));
    let mut rest = s;
    while window.len() < width {
        let Some((head, tail)) = step_head(eval, &rest)? else {
            break;
        };
        window.push(head);
        rest = tail;
    }
    if !window_full(n, window.len()) {
        let Some(pad) = pad else {
            return Ok(Forced::Empty);
        };
        let padded = PendingSeq::pending(Thunk::Take {
            n: n.clone(),
            input: PendingSeq::pending(Thunk::Concat {
                inputs: vec![lazy_window(eval, window)?, pad],
            }),
        });
        return Ok(Forced::Chunk {
            items: Arc::from(vec![padded]),
            tail: Value::Nil,
        });
    }
    let next = partition_next(eval, &window, rest, s_idrop, step)?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![lazy_window(eval, window.clone())?]),
        tail: PendingSeq::pending(Thunk::Partition {
            n: n.clone(),
            step: step.clone(),
            pad,
            input: next,
        }),
    })
}

/// `partition-all`: collect one window (short windows kept — even
/// empty ones, so non-positive widths emit empties until the
/// input runs out) and advance eagerly on the same force.
fn step_partition_all_collect(
    eval: &mut Evaluator,
    n: &Value,
    step: &Value,
    input: Value,
) -> Result<Forced, Signal> {
    let Some(s) = seq_opt(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    let s_idrop = is_idrop(&s);
    let width = crate::core::take_count(n)
        .map_err(|e| eval.throw(e))?
        .max(0) as usize;
    let mut window = Vec::with_capacity(width.min(CHUNK_SIZE));
    let mut rest = s;
    while window.len() < width {
        let Some((head, tail)) = step_head(eval, &rest)? else {
            break;
        };
        window.push(head);
        rest = tail;
    }
    if window.len() < width {
        return Ok(Forced::Chunk {
            items: Arc::from(vec![lazy_window(eval, window)?]),
            tail: Value::Nil,
        });
    }
    let next = partition_next(eval, &window, rest, s_idrop, step)?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![lazy_window(eval, window.clone())?]),
        tail: PendingSeq::pending(Thunk::PartitionAll {
            n: n.clone(),
            step: step.clone(),
            input: next,
        }),
    })
}

/// `partition-by`: one run per step, exactly the JVM shape: the
/// run is a cons of the head over a lazy take-while tail, and
/// the advance drops the (fully forced) run length. Boundary
/// keys recompute (never cached), matching the JVM call counts.
fn step_partition_by(eval: &mut Evaluator, f: &Value, input: Value) -> Result<Forced, Signal> {
    let Some(s) = seq_opt(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    let Some((fst, next)) = step_head(eval, &s)? else {
        return Ok(Forced::Empty);
    };
    crate::core::check_ifn(eval, f)?;
    let fv = eval.call(f, std::slice::from_ref(&fst))?;
    let run_tail = PendingSeq::pending(Thunk::PartitionRun {
        f: f.clone(),
        fv,
        input: next,
    });
    let run = Value::Extension(Arc::new(ConsValue::new(fst, run_tail)));
    Ok(Forced::Chunk {
        items: Arc::from(vec![run.clone()]),
        tail: PendingSeq::pending(Thunk::PartitionByAdvance {
            f: f.clone(),
            run,
            s,
        }),
    })
}

/// `partition-by` advance: `(drop (count run) s)`, evaluated here
/// (the count forces the run, like the JVM's strict `count`).
fn step_partition_by_advance(
    eval: &mut Evaluator,
    f: &Value,
    run: &Value,
    s: Value,
) -> Result<Forced, Signal> {
    let mut k: i64 = 0;
    let mut rest = run.clone();
    while let Some((_, tail)) = step_head(eval, &rest)? {
        k += 1;
        rest = tail;
    }
    let dropped = drop_value(eval, &Value::Int(k), s)?;
    step_partition_by(eval, f, dropped)
}

/// A `partition-by` run tail: `(take-while #(= fv (f %)))`.
fn step_partition_run(
    eval: &mut Evaluator,
    f: &Value,
    fv: &Value,
    input: Value,
) -> Result<Forced, Signal> {
    let Some((head, tail)) = step_head(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    crate::core::check_ifn(eval, f)?;
    let got = eval.call(f, std::slice::from_ref(&head))?;
    if !equiv_vals(eval, fv, &got)? {
        return Ok(Forced::Empty);
    }
    Ok(Forced::Chunk {
        items: Arc::from(vec![head]),
        tail: PendingSeq::pending(Thunk::PartitionRun {
            f: f.clone(),
            fv: fv.clone(),
            input: tail,
        }),
    })
}

/// `seq` as an option (`None` for empty inputs; unseqable
/// inputs still error).
fn seq_opt(eval: &mut Evaluator, v: &Value) -> Result<Option<Value>, Signal> {
    match seq_value(eval, v)? {
        Value::Nil => Ok(None),
        s => Ok(Some(s)),
    }
}

/// `distinct`: one fresh item per step (the body destructures
/// first/rest, never chunks).
#[allow(clippy::mutable_key_type)]
fn step_distinct(
    eval: &mut Evaluator,
    mut seen: HashSet<Value>,
    mut input: Value,
) -> Result<Forced, Signal> {
    loop {
        let Some((head, tail)) = step_head(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        if seen.insert(head.clone()) {
            return Ok(Forced::Chunk {
                items: Arc::from(vec![head]),
                tail: PendingSeq::pending(Thunk::Distinct { seen, input: tail }),
            });
        }
        input = tail;
    }
}

/// `dedupe`: whole units, suppressing runs against the last kept.
fn step_dedupe(
    eval: &mut Evaluator,
    mut last: Option<Value>,
    mut input: Value,
) -> Result<Forced, Signal> {
    loop {
        let Some((items, tail)) = step_items(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        let mut out = Vec::new();
        for item in &items {
            if last.as_ref() != Some(item) {
                out.push(item.clone());
            }
            last = Some(item.clone());
        }
        if out.is_empty() {
            input = tail;
        } else {
            return Ok(Forced::Chunk {
                items: Arc::from(out),
                tail: PendingSeq::pending(Thunk::Dedupe { last, input: tail }),
            });
        }
    }
}

/// `keep`: whole units, dropping nil results.
fn step_keep(eval: &mut Evaluator, f: &Value, mut input: Value) -> Result<Forced, Signal> {
    loop {
        let Some((items, tail)) = step_items(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        crate::core::check_ifn(eval, f)?;
        let mut kept = Vec::new();
        for item in &items {
            let kept_item = eval.call(f, std::slice::from_ref(item))?;
            if !matches!(kept_item, Value::Nil) {
                kept.push(kept_item);
            }
        }
        if kept.is_empty() {
            input = tail;
        } else {
            return Ok(Forced::Chunk {
                items: Arc::from(kept),
                tail: PendingSeq::pending(Thunk::Keep {
                    f: f.clone(),
                    input: tail,
                }),
            });
        }
    }
}

/// `keep-indexed`: whole units with running indices, dropping nil
/// results.
fn step_keep_indexed(
    eval: &mut Evaluator,
    f: &Value,
    mut input: Value,
    mut idx: i64,
) -> Result<Forced, Signal> {
    loop {
        let Some((items, tail)) = step_items(eval, &input)? else {
            return Ok(Forced::Empty);
        };
        crate::core::check_ifn(eval, f)?;
        let mut kept = Vec::new();
        for item in &items {
            let kept_item = eval.call(f, &[Value::Int(idx), item.clone()])?;
            idx += 1;
            if !matches!(kept_item, Value::Nil) {
                kept.push(kept_item);
            }
        }
        if kept.is_empty() {
            input = tail;
        } else {
            return Ok(Forced::Chunk {
                items: Arc::from(kept),
                tail: PendingSeq::pending(Thunk::KeepIndexed {
                    f: f.clone(),
                    input: tail,
                    idx,
                }),
            });
        }
    }
}

/// `map-indexed`: whole units with running indices.
fn step_map_indexed(
    eval: &mut Evaluator,
    f: &Value,
    input: Value,
    mut idx: i64,
) -> Result<Forced, Signal> {
    let Some((items, tail)) = step_items(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    crate::core::check_ifn(eval, f)?;
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        out.push(eval.call(f, &[Value::Int(idx), item.clone()])?);
        idx += 1;
    }
    Ok(Forced::Chunk {
        items: Arc::from(out),
        tail: PendingSeq::pending(Thunk::MapIndexed {
            f: f.clone(),
            input: tail,
            idx,
        }),
    })
}

/// `reductions`: one item per step (unstarted with-init emits the
/// init; unstarted no-init seeds from the head, or calls `(f)` on
/// empty inputs).
fn step_reductions(
    eval: &mut Evaluator,
    f: &Value,
    acc: Option<Value>,
    has_init: bool,
    input: Value,
    started: bool,
) -> Result<Forced, Signal> {
    if !started {
        if has_init {
            let Some(init) = acc.clone() else {
                return Ok(Forced::Empty);
            };
            return Ok(Forced::Chunk {
                items: Arc::from(vec![init]),
                tail: PendingSeq::pending(Thunk::Reductions {
                    f: f.clone(),
                    acc,
                    has_init,
                    input,
                    started: true,
                }),
            });
        }
        match step_head(eval, &input)? {
            None => {
                crate::core::check_ifn(eval, f)?;
                let seed = eval.call(f, &[])?;
                return Ok(Forced::Chunk {
                    items: Arc::from(vec![seed]),
                    tail: Value::Nil,
                });
            }
            Some((head, tail)) => {
                return Ok(Forced::Chunk {
                    items: Arc::from(vec![head.clone()]),
                    tail: PendingSeq::pending(Thunk::Reductions {
                        f: f.clone(),
                        acc: Some(head),
                        has_init: false,
                        input: tail,
                        started: true,
                    }),
                });
            }
        }
    }
    let Some((head, tail)) = step_head(eval, &input)? else {
        return Ok(Forced::Empty);
    };
    crate::core::check_ifn(eval, f)?;
    let Some(acc) = acc else {
        return Ok(Forced::Empty);
    };
    let next = eval.call(f, &[acc, head])?;
    Ok(Forced::Chunk {
        items: Arc::from(vec![next.clone()]),
        tail: PendingSeq::pending(Thunk::Reductions {
            f: f.clone(),
            acc: Some(next),
            has_init,
            input: tail,
            started: true,
        }),
    })
}

/// A `lazy-seq` body: evaluate, seq the result, and share its first
/// unit (pending bodies adopt the inner cell's chunk). An empty
/// `recur` re-invokes the body (the zero-arg `fn` boundary, like the
/// JVM closure); nested loops consume their own first.
fn step_thunk0(eval: &mut Evaluator, body: &[Expr], env: &Env) -> Result<Forced, Signal> {
    loop {
        let mut value = Value::Nil;
        let mut recurred = false;
        for expr in body {
            match eval.eval(expr, env) {
                Ok(v) => value = v,
                Err(Signal::Recur(args)) if args.is_empty() => {
                    recurred = true;
                    break;
                }
                Err(other) => return Err(other),
            }
        }
        if !recurred {
            let forced = seq_value(eval, &value)?;
            return step_value_forced(eval, &forced);
        }
    }
}

/// The empty list value (`()`).
pub(crate) fn empty_list_value() -> Value {
    Value::List(List::new_sync())
}

/// Pure emptiness for entry-view underlyings (nil, empty lists
/// and realized seqs, done ranges, spent repeats — pending and
/// other strict seqs count as non-empty without forcing, like
/// the JVM's `.seq()`, which never forces).
fn keys_empty(u: &Value) -> bool {
    match u {
        Value::Nil => true,
        Value::List(l) => l.is_empty(),
        Value::Extension(_) => {
            if let Some(s) = as_realized(u) {
                return s.seq().is_empty();
            }
            if let Some(r) = as_range(u) {
                return r.done();
            }
            if let Some(r) = as_repeat(u) {
                return r.n().is_some_and(|n| n <= 0);
            }
            false
        }
        _ => false,
    }
}

/// Nil-punning `seq`: nil for empties, the seq value otherwise.
/// Pending seqs force one step; strict seqs decide emptiness from
/// their bounds (never stepping, like the JVM's `.seq()` on
/// `ISeq`s); vectors chunk, maps/sets/strings go eager; lists
/// return directly.
pub(crate) fn seq_value(eval: &mut Evaluator, v: &Value) -> Result<Value, Signal> {
    match v {
        Value::Nil => Ok(Value::Nil),
        Value::Extension(_) => {
            if let Some(p) = as_pending(v) {
                return match force_pending(eval, p)? {
                    Forced::Empty => Ok(Value::Nil),
                    Forced::Chunk { .. } => Ok(v.clone()),
                };
            }
            if let Some(s) = as_realized(v) {
                return Ok(if s.seq().is_empty() {
                    Value::Nil
                } else {
                    v.clone()
                });
            }
            if let Some(r) = as_range(v) {
                return Ok(if r.done() { Value::Nil } else { v.clone() });
            }
            if let Some(r) = as_repeat(v) {
                let empty = r.n().is_some_and(|n| n <= 0);
                return Ok(if empty { Value::Nil } else { v.clone() });
            }
            if let Some(c) = as_cycle(v) {
                let empty = matches!(c.prev(), Value::Nil) && matches!(c.current(), Value::Nil);
                return Ok(if empty { Value::Nil } else { v.clone() });
            }
            if as_iterate(v).is_some() || as_cons_value(v).is_some() {
                return Ok(v.clone());
            }
            if let Some(k) = as_keys(v) {
                return Ok(if keys_empty(k.underlying()) {
                    Value::Nil
                } else {
                    v.clone()
                });
            }
            if let Some(k) = as_vals(v) {
                return Ok(if keys_empty(k.underlying()) {
                    Value::Nil
                } else {
                    v.clone()
                });
            }
            if let Some(sv) = as_subvector(v) {
                return Ok(realized_value(
                    sv.items(),
                    false,
                    true,
                    false,
                    SeqKind::Subvec,
                ));
            }
            Err(eval.throw(iseq_err(v)))
        }
        Value::List(l) => Ok(if l.is_empty() { Value::Nil } else { v.clone() }),
        Value::Vector(x) => Ok(realized_value(
            x.iter().cloned().collect::<Vec<_>>(),
            true,
            true,
            true,
            SeqKind::Chunked,
        )),
        Value::Map(m) => {
            let small = !m.is_hashy();
            let kind = if small { SeqKind::Map } else { SeqKind::Key };
            Ok(realized_value(
                m.iter()
                    .map(|(k, val)| Value::Vector(Vector::from_vec(vec![k.clone(), val.clone()])))
                    .collect::<Vec<_>>(),
                false,
                small,
                small,
                kind,
            ))
        }
        Value::Set(s) => Ok(realized_value(
            s.iter().cloned().collect::<Vec<_>>(),
            false,
            false,
            false,
            SeqKind::Key,
        )),
        Value::Str(s) => Ok(realized_value(
            s.chars().map(Value::Char).collect::<Vec<_>>(),
            false,
            true,
            true,
            SeqKind::Str,
        )),
        _ => Err(eval.throw(iseq_err(v))),
    }
}

/// The first item (nil for empties/unseqable-nils; direct for
/// collections, stepping only for seq values). Iterate heads
/// read without stepping (never calling `f` for realized seeds);
/// entry views cast.
pub(crate) fn first_value(eval: &mut Evaluator, v: &Value) -> Result<Value, Signal> {
    match v {
        Value::Nil => Ok(Value::Nil),
        Value::List(l) => Ok(l.first().cloned().unwrap_or(Value::Nil)),
        Value::Vector(x) => Ok(x.iter().next().cloned().unwrap_or(Value::Nil)),
        Value::Map(m) => Ok(m
            .iter()
            .next()
            .map(|(k, val)| Value::Vector(Vector::from_vec(vec![k.clone(), val.clone()])))
            .unwrap_or(Value::Nil)),
        Value::Set(s) => Ok(s.iter().next().cloned().unwrap_or(Value::Nil)),
        Value::Str(s) => Ok(s.chars().next().map(Value::Char).unwrap_or(Value::Nil)),
        Value::Extension(_) if as_subvector(v).is_some() => Ok(as_subvector(v)
            .and_then(|sv| sv.iter().next().cloned())
            .unwrap_or(Value::Nil)),
        Value::Extension(_) if is_seq_value(v) => {
            if let Some(it) = as_iterate(v) {
                return iterate_seed(eval, it);
            }
            if let Some(k) = as_keys(v) {
                return keys_first(eval, k.underlying(), true);
            }
            if let Some(k) = as_vals(v) {
                return keys_first(eval, k.underlying(), false);
            }
            match step_unit(eval, v)? {
                StepUnit::Empty => Ok(Value::Nil),
                StepUnit::One { head, .. } => Ok(head),
                StepUnit::Chunk { items, .. } => Ok(items.first().cloned().unwrap_or(Value::Nil)),
            }
        }
        _ => Err(eval.throw(iseq_err(v))),
    }
}

/// A range shifted past its head (`()` when spent; metadata
/// dropped, like `Range.next`/`LongRange.next`).
fn range_rest(r: &RangeValue) -> Value {
    if r.done() {
        return empty_list_value();
    }
    match r.advance() {
        None => empty_list_value(),
        Some(nxt) => {
            let shifted = RangeValue::new(
                nxt,
                r.end(),
                r.step(),
                r.is_long(),
                r.count().map(|c| c - 1),
            );
            if shifted.done() {
                empty_list_value()
            } else {
                Value::Extension(Arc::new(shifted))
            }
        }
    }
}

/// `rest` of a seqed value (never nil): lists drop their head,
/// realized seqs shift (preserving kind, `StringSeq` keeping
/// metadata), ranges shift, one-step strict seqs step, entry
/// views advance without casting, and forced multi-item chunks
/// rest to a `ChunkedCons` (like the JVM's `more`).
fn rest_of_seq(eval: &mut Evaluator, s: &Value) -> Result<Value, Signal> {
    match s {
        Value::Nil => Ok(empty_list_value()),
        Value::List(l) => Ok(l
            .drop_first()
            .map(|t| Value::List(t.with_meta(None)))
            .unwrap_or_else(empty_list_value)),
        Value::Extension(_) => {
            if let Some(p) = as_pending(s) {
                return match force_pending(eval, p)? {
                    Forced::Empty => Ok(empty_list_value()),
                    Forced::Chunk { items, tail } => {
                        if items.len() > 1 {
                            Ok(PendingSeq::pending_kind(
                                Thunk::Chain {
                                    head: items[1..].to_vec(),
                                    tail,
                                },
                                LazyKind::ChunkedCons,
                            ))
                        } else if items.is_empty() {
                            let s = seq_value(eval, &tail)?;
                            rest_of_seq(eval, &s)
                        } else {
                            match seq_value(eval, &tail)? {
                                Value::Nil => Ok(empty_list_value()),
                                t => Ok(t),
                            }
                        }
                    }
                };
            }
            if let Some(r) = as_realized(s) {
                let more = r.seq().more();
                if more.is_empty() {
                    return Ok(empty_list_value());
                }
                return Ok(Value::Extension(Arc::new(seq_tail(
                    more,
                    r.is_counted(),
                    r.is_idrop(),
                    r.kind(),
                    r.meta(),
                ))));
            }
            if let Some(r) = as_range(s) {
                return Ok(range_rest(r));
            }
            if let Some(k) = as_keys(s) {
                return keys_rest(eval, k.underlying(), true);
            }
            if let Some(k) = as_vals(s) {
                return keys_rest(eval, k.underlying(), false);
            }
            match step_unit(eval, s)? {
                StepUnit::Empty => Ok(empty_list_value()),
                StepUnit::One { tail, .. } | StepUnit::Chunk { tail, .. } => match &tail {
                    Value::Nil => Ok(empty_list_value()),
                    Value::List(l) if l.is_empty() => Ok(empty_list_value()),
                    _ => {
                        if as_realized(&tail).is_some_and(|r| r.seq().is_empty()) {
                            return Ok(empty_list_value());
                        }
                        Ok(tail)
                    }
                },
            }
        }
        _ => {
            let s = seq_value(eval, s)?;
            rest_of_seq(eval, &s)
        }
    }
}

/// `rest`: a seq value, never nil (empty lists print `()`).
pub(crate) fn rest_value(eval: &mut Evaluator, v: &Value) -> Result<Value, Signal> {
    let s = seq_value(eval, v)?;
    rest_of_seq(eval, &s)
}

/// `next`: like `rest` but nil at the end (forcing one step).
pub(crate) fn next_value(eval: &mut Evaluator, v: &Value) -> Result<Value, Signal> {
    let rest = rest_value(eval, v)?;
    match &rest {
        Value::List(l) if l.is_empty() => Ok(Value::Nil),
        _ => {
            if as_realized(&rest).is_some_and(|s| s.seq().is_empty()) {
                return Ok(Value::Nil);
            }
            if let Some(p) = as_pending(&rest)
                && matches!(force_pending(eval, p)?, Forced::Empty)
            {
                return Ok(Value::Nil);
            }
            Ok(rest)
        }
    }
}

/// Realize every item (for `apply` spreads, `vec`, `sort`,
/// `count`, printing, and `=`).
pub(crate) fn collect_all(eval: &mut Evaluator, v: &Value) -> Result<Vec<Value>, Signal> {
    let mut out = Vec::new();
    let mut cur = v.clone();
    loop {
        match step_unit(eval, &cur)? {
            StepUnit::Empty => return Ok(out),
            StepUnit::One { head, tail } => {
                out.push(head);
                cur = tail;
            }
            StepUnit::Chunk { items, tail } => {
                out.extend(items.iter().cloned());
                cur = tail;
            }
        }
    }
}

/// Whether hashing throws `UnsupportedOperationException` (the
/// infinite `Cycle`/`Iterate`/`Repeat`, like the JVM's `hasheq`
/// overrides — empty message).
pub(crate) fn is_hash_uoe(v: &Value) -> bool {
    if let Some(r) = as_repeat(v) {
        return r.n().is_none();
    }
    as_cycle(v).is_some() || as_iterate(v).is_some()
}

/// The `UnsupportedOperationException` (empty message) for
/// unhashable infinite seqs.
fn uoe() -> NativeError {
    NativeError::new(ThrowClass::UnsupportedOperationException, String::new())
}

/// Whether `=` walks the value pairwise (lists, vectors, and every
/// seq object — like `Sequential`).
fn is_sequential(v: &Value) -> bool {
    matches!(v, Value::Vector(_)) || as_subvector(v).is_some() || is_seq_value(v)
}

/// Whether a seq value is `Counted` (probed): long ranges, conses,
/// and flagged realized seqs; repeats/cycles/iterates, entry views,
/// and pending seqs never are.
pub(crate) fn is_counted_seq(v: &Value) -> bool {
    if let Some(r) = as_range(v) {
        return r.is_long();
    }
    // `Cons` is never `Counted` — not even over a vector
    // (fixprobe7 `:cc-cons` reverses the old pin).
    if as_cons_value(v).is_some() {
        return false;
    }
    if let Some(s) = as_realized(v) {
        return s.is_counted();
    }
    false
}

/// Whether a half-walked side is spent (buffer drained, tail
/// empty).
fn equiv_drained(eval: &mut Evaluator, tail: &Value) -> Result<bool, Signal> {
    let mut cur = tail.clone();
    loop {
        match step_unit(eval, &cur)? {
            StepUnit::Empty => return Ok(true),
            StepUnit::One { .. } => return Ok(false),
            StepUnit::Chunk { items, tail } => {
                if items.is_empty() {
                    cur = tail;
                } else {
                    return Ok(false);
                }
            }
        }
    }
}

/// Fill a walk buffer from its tail (`Ok(false)` when spent).
fn equiv_fill(
    eval: &mut Evaluator,
    buf: &mut Vec<Value>,
    tail: &mut Value,
) -> Result<bool, Signal> {
    loop {
        match step_unit(eval, tail)? {
            StepUnit::Empty => return Ok(false),
            StepUnit::One { head, tail: rest } => {
                *buf = vec![head];
                *tail = rest;
                return Ok(true);
            }
            StepUnit::Chunk { items, tail: rest } => {
                if items.is_empty() {
                    *tail = rest;
                } else {
                    *buf = items.to_vec();
                    *tail = rest;
                    return Ok(true);
                }
            }
        }
    }
}

/// Pairwise sequential walk with early exit (like
/// `Util.doEquiv`): finite-vs-infinite answers `false` instead of
/// hanging, and errors surface from whichever side forces first.
fn equiv_seq(eval: &mut Evaluator, a: &Value, b: &Value) -> Result<bool, Signal> {
    let mut ta = a.clone();
    let mut tb = b.clone();
    let (mut ba, mut bb) = (Vec::new(), Vec::new());
    let (mut pa, mut pb) = (0usize, 0usize);
    loop {
        if pa >= ba.len() {
            if !equiv_fill(eval, &mut ba, &mut ta)? {
                if pb < bb.len() {
                    return Ok(false);
                }
                return equiv_drained(eval, &tb);
            }
            pa = 0;
        }
        if pb >= bb.len() {
            if !equiv_fill(eval, &mut bb, &mut tb)? {
                return Ok(false);
            }
            pb = 0;
        }
        if !equiv_vals(eval, &ba[pa], &bb[pb])? {
            return Ok(false);
        }
        pa += 1;
        pb += 1;
    }
}

/// `Util.equiv`: sequential pairs walk pairwise (early exit);
/// maps compare by size plus per-key recursion (missing keys fail
/// even for nil values, like the JVM's sentinel lookup);
/// unhashable-infinite members throw like `hasheq` does during
/// the JVM's hash lookups; anything else falls back to `==`.
pub(crate) fn equiv_vals(eval: &mut Evaluator, a: &Value, b: &Value) -> Result<bool, Signal> {
    if is_sequential(a) && is_sequential(b) {
        return equiv_seq(eval, a, b);
    }
    if let (Value::Map(x), Value::Map(y)) = (a, b) {
        if x.size() != y.size() {
            return Ok(false);
        }
        for (k, v) in x.iter() {
            if is_hash_uoe(k) {
                return Err(eval.throw(uoe()));
            }
            let Some(w) = y.get(k) else {
                return Ok(false);
            };
            if !equiv_vals(eval, v, w)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let (Value::Set(x), Value::Set(y)) = (a, b) {
        if x.size() != y.size() {
            return Ok(false);
        }
        for v in x.iter() {
            if is_hash_uoe(v) {
                return Err(eval.throw(uoe()));
            }
            let mut found = false;
            for w in y.iter() {
                if equiv_vals(eval, v, w)? {
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    Ok(a == b)
}

/// Deep-realize pending seqs into realized ones (for printing,
/// `str`, and `=`); realized seqs rebuild with realized items and
/// preserved metadata.
pub(crate) fn realize_deep(eval: &mut Evaluator, v: &Value) -> Result<Value, Signal> {
    match v {
        Value::Extension(_) => {
            if let Some(p) = as_pending(v) {
                let items = collect_all(eval, v)?;
                let mut seq = RealizedSeq::new(Seq::eager(items).unwrap_or_else(Seq::empty));
                seq = seq.with_meta(p.meta().cloned());
                return Ok(Value::Extension(Arc::new(seq)));
            }
            if let Some(s) = as_realized(v) {
                let mut items = Vec::new();
                for item in s.items() {
                    items.push(realize_deep(eval, &item)?);
                }
                let mut seq = RealizedSeq::new(Seq::eager(items).unwrap_or_else(Seq::empty));
                seq = seq.with_meta(s.meta().cloned());
                return Ok(Value::Extension(Arc::new(seq)));
            }
            if as_range(v).is_some()
                || as_repeat(v).is_some()
                || as_cycle(v).is_some()
                || as_iterate(v).is_some()
                || as_cons_value(v).is_some()
                || as_keys(v).is_some()
                || as_vals(v).is_some()
            {
                let items = collect_all(eval, v)?;
                let mut seq = RealizedSeq::new(Seq::eager(items).unwrap_or_else(Seq::empty));
                seq = seq.with_meta(v.meta().cloned());
                return Ok(Value::Extension(Arc::new(seq)));
            }
            if let Some(sv) = as_subvector(v) {
                let mut items = Vec::with_capacity(sv.len());
                for item in sv.iter() {
                    items.push(realize_deep(eval, item)?);
                }
                return Ok(Value::Extension(Arc::new(sv.with_window(items))));
            }
            Ok(v.clone())
        }
        Value::List(l) => {
            let mut items = Vec::with_capacity(l.len());
            for item in l.iter() {
                items.push(realize_deep(eval, item)?);
            }
            Ok(Value::List(
                List::from_vec(items).with_meta(l.meta().cloned()),
            ))
        }
        Value::Vector(x) => {
            let mut items = Vec::with_capacity(x.len());
            for item in x.iter() {
                items.push(realize_deep(eval, item)?);
            }
            Ok(Value::Vector(
                Vector::from_vec(items).with_meta(x.meta().cloned()),
            ))
        }
        Value::Map(m) => {
            let mut pairs = Vec::with_capacity(m.size());
            for (k, val) in m.iter() {
                pairs.push((realize_deep(eval, k)?, realize_deep(eval, val)?));
            }
            let mut out = clj_value::new_map();
            for (k, val) in pairs {
                out = out.insert(k, val);
            }
            Ok(Value::Map(out.with_meta(m.meta().cloned())))
        }
        Value::Set(s) => {
            let mut out = clj_value::new_set();
            for item in s.iter() {
                out = out.insert(realize_deep(eval, item)?);
            }
            Ok(Value::Set(out.with_meta(s.meta().cloned())))
        }
        _ => Ok(v.clone()),
    }
}

/// Build a pending seq from a thunk (for the seq natives).
pub(crate) fn pending_value(thunk: Thunk) -> Value {
    PendingSeq::pending(thunk)
}

/// A `Thunk::Map` tail value (for `map`).
pub(crate) fn map_thunk(f: Value, inputs: Vec<Value>) -> Value {
    pending_value(Thunk::Map { f, inputs })
}

/// A `Thunk::MapCat` tail value (for `mapcat`, over the map outer).
pub(crate) fn mapcat_thunk(f: Value, inputs: Vec<Value>) -> Value {
    pending_value(Thunk::MapCat {
        outer: map_thunk(f, inputs),
    })
}

/// A `Thunk::Replace` tail value (for `replace` over non-vectors).
pub(crate) fn replace_thunk(smap: Value, input: Value) -> Value {
    pending_value(Thunk::Replace { smap, input })
}

/// `replace` over a non-vector: swap members found in `smap` (a
/// `find` per item; missing keys pass through; an unsupported
/// `smap` fails on the first item).
fn step_replace(eval: &mut Evaluator, smap: &Value, input: Value) -> Result<Forced, Signal> {
    let (items, tail) = match step_unit(eval, &input)? {
        StepUnit::Empty => return Ok(Forced::Empty),
        StepUnit::One { head, tail } => (vec![head], tail),
        StepUnit::Chunk { items, tail } => (items.to_vec(), tail),
    };
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        let found = crate::coll::find_entry(smap, item).map_err(|e| eval.throw(e))?;
        out.push(found.map_or_else(|| item.clone(), |(_, v)| v));
    }
    Ok(Forced::Chunk {
        items: Arc::from(out),
        tail: PendingSeq::pending(Thunk::Replace {
            smap: smap.clone(),
            input: tail,
        }),
    })
}

/// A `Thunk::Filter`/`Remove` tail value.
pub(crate) fn filter_thunk(f: Value, input: Value, invert: bool) -> Value {
    pending_value(if invert {
        Thunk::Remove { f, input }
    } else {
        Thunk::Filter { f, input }
    })
}

/// A `Thunk::Take` tail value (the count stays raw — invalid
/// counts fail when forced, like the JVM's lazy `pos?`).
pub(crate) fn take_thunk(n: Value, input: Value) -> Value {
    pending_value(Thunk::Take { n, input })
}

/// A `Thunk::Concat` tail value.
pub(crate) fn concat_thunk(inputs: Vec<Value>) -> Value {
    pending_value(Thunk::Concat { inputs })
}

/// `int?` over our numbers (fixed-precision integers only).
fn is_int_value(v: &Value) -> bool {
    matches!(v, Value::Int(_))
}

/// A bound as a number pair (`Int`/`Int` compare exactly, anything
/// else as `f64` — like `Numbers`). Bad sides fail x-first (nil is
/// the numbers NPE, anything else the `Number` cast failure).
fn num_pair(a: &Value, b: &Value) -> Result<(Num, Num), NativeError> {
    Ok((crate::core::as_num(a)?, crate::core::as_num(b)?))
}

/// `Numbers.gt` over our numbers.
fn num_gt(a: &Value, b: &Value) -> Result<bool, NativeError> {
    match num_pair(a, b)? {
        (Num::Int(x), Num::Int(y)) => Ok(x > y),
        (x, y) => Ok(num_f64(x) > num_f64(y)),
    }
}

/// `Numbers.equiv` over our numbers (`NaN` never, cross-type by
/// value — like `Range.create`'s emptiness check).
fn num_equiv(a: &Value, b: &Value) -> Result<bool, NativeError> {
    match num_pair(a, b)? {
        (Num::Int(x), Num::Int(y)) => Ok(x == y),
        (x, y) => Ok(num_f64(x) == num_f64(y)),
    }
}

/// `Numbers.isPos`/`isNeg` over our numbers (`sign > 0` / `< 0`).
fn num_sign(v: &Value, positive: bool) -> Result<bool, NativeError> {
    match crate::core::as_num(v)? {
        Num::Int(i) => Ok(if positive { i > 0 } else { i < 0 }),
        Num::Double(d) => Ok(if positive { d > 0.0 } else { d < 0.0 }),
    }
}

/// `Numbers.isZero` over our numbers.
fn num_is_zero(v: &Value) -> Result<bool, NativeError> {
    match crate::core::as_num(v)? {
        Num::Int(i) => Ok(i == 0),
        Num::Double(d) => Ok(d == 0.0),
    }
}

/// A `Num` as `f64`.
fn num_f64(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Double(d) => d,
    }
}

/// A numeric value as a range bound (only reached for numbers —
/// the `create` checks reject the rest first).
fn range_num(v: &Value) -> RangeNum {
    match v {
        Value::Int(i) => RangeNum::Int(*i),
        Value::Double(d) => RangeNum::Double(*d),
        _ => RangeNum::Int(0),
    }
}

/// `LongRange.rangeCount` (checked; `None` overflows like the
/// JVM's `ArithmeticException`, routing to the double path).
fn range_count(start: i64, end: i64, step: i64) -> Option<i64> {
    let total = end
        .checked_sub(start)?
        .checked_add(step)?
        .checked_add(if step > 0 { -1 } else { 1 })?;
    total.checked_div(step)
}

/// `LongRange.create` (all-`Int` args): empties answer `()`,
/// zero steps repeat the start, overflows fall back to the double
/// path with `Int` bounds.
fn long_range(s: i64, e: i64, st: i64) -> Value {
    if (st > 0 && e <= s) || (st < 0 && e >= s) || (st == 0 && e == s) {
        return empty_list_value();
    }
    if st == 0 {
        return Value::Extension(Arc::new(RepeatValue::new(Value::Int(s), None)));
    }
    match range_count(s, e, st).and_then(|c| i32::try_from(c).ok()) {
        Some(count) => Value::Extension(Arc::new(RangeValue::new(
            RangeNum::Int(s),
            Some(RangeNum::Int(e)),
            RangeNum::Int(st),
            true,
            Some(i64::from(count)),
        ))),
        None => Value::Extension(Arc::new(RangeValue::new(
            RangeNum::Int(s),
            Some(RangeNum::Int(e)),
            RangeNum::Int(st),
            false,
            None,
        ))),
    }
}

/// `Range.create` (any non-`Int` arg): bound checks answer `()`,
/// zero steps repeat the start, else a double-path `Range`.
fn double_range(start: Value, end: Value, step: Value) -> Result<Value, NativeError> {
    if (num_sign(&step, true)? && num_gt(&start, &end)?)
        || (num_sign(&step, false)? && num_gt(&end, &start)?)
        || num_equiv(&start, &end)?
    {
        return Ok(empty_list_value());
    }
    if num_is_zero(&step)? {
        return Ok(Value::Extension(Arc::new(RepeatValue::new(start, None))));
    }
    Ok(Value::Extension(Arc::new(RangeValue::new(
        range_num(&start),
        Some(range_num(&end)),
        range_num(&step),
        false,
        None,
    ))))
}

/// The 1-arity `(range end)`: `int?` dispatches to
/// `LongRange.create`, else `Range.create` (whose `isPos` errors
/// on non-numbers).
pub(crate) fn range1(end: Value) -> Result<Value, NativeError> {
    if is_int_value(&end) {
        let Value::Int(e) = end else {
            unreachable!("checked int above");
        };
        return Ok(long_range(0, e, 1));
    }
    if num_sign(&end, true)? {
        Ok(Value::Extension(Arc::new(RangeValue::new(
            RangeNum::Int(0),
            Some(range_num(&end)),
            RangeNum::Int(1),
            false,
            None,
        ))))
    } else {
        Ok(empty_list_value())
    }
}

/// The 2-arity `(range start end)`.
pub(crate) fn range2(start: Value, end: Value) -> Result<Value, NativeError> {
    if is_int_value(&start) && is_int_value(&end) {
        let (Value::Int(s), Value::Int(e)) = (start, end) else {
            unreachable!("checked ints above");
        };
        return Ok(long_range(s, e, 1));
    }
    double_range(start, end, Value::Int(1))
}

/// The 3-arity `(range start end step)`.
pub(crate) fn range3(start: Value, end: Value, step: Value) -> Result<Value, NativeError> {
    if is_int_value(&start) && is_int_value(&end) && is_int_value(&step) {
        let (Value::Int(s), Value::Int(e), Value::Int(st)) = (start, end, step) else {
            unreachable!("checked ints above");
        };
        return Ok(long_range(s, e, st));
    }
    double_range(start, end, step)
}

/// A `Thunk::Thunk0` tail value (the `lazy-seq*` body).
pub(crate) fn thunk0_value(body: Arc<Vec<Expr>>, env: Env) -> Value {
    pending_value(Thunk::Thunk0 { body, env })
}

/// A `repeat` value: non-positive counts answer `()` (like
/// `Repeat.create`), else a strict `Repeat` (`None` is infinite).
pub(crate) fn repeat_thunk(x: Value, n: Option<i64>) -> Value {
    if n.is_some_and(|n| n <= 0) {
        return empty_list_value();
    }
    Value::Extension(Arc::new(RepeatValue::new(x, n)))
}

/// An infinite `repeatedly` tail value (the `n` arity routes
/// through `take`, so counts stay lazy there).
pub(crate) fn repeatedly_thunk(f: Value) -> Value {
    pending_value(Thunk::Repeatedly { f })
}

/// An `iterate` value (the seed emits free, like
/// `Iterate.create` — non-`IFn` callees fail at the call site).
pub(crate) fn iterate_thunk(f: Value, x: Value) -> Value {
    Value::Extension(Arc::new(IterateValue::new(f, x)))
}

/// A `cycle` value: the input seqed eagerly, `()` for empties
/// (like `Cycle.create`), else a strict `Cycle` replaying it.
pub(crate) fn cycle_thunk(eval: &mut Evaluator, input: Value) -> Result<Value, Signal> {
    match seq_value(eval, &input)? {
        Value::Nil => Ok(empty_list_value()),
        s => Ok(Value::Extension(Arc::new(CycleValue::new(s.clone(), s)))),
    }
}

/// A `Thunk::Interleave` tail value (empty round queue).
pub(crate) fn interleave_thunk(inputs: Vec<Value>) -> Value {
    pending_value(Thunk::Interleave {
        queue: Vec::new(),
        inputs,
    })
}

/// A `keys`/`vals` entry view over the seqed input (`None` for
/// empties, like `KeySeq.create`).
pub(crate) fn keys_thunk(eval: &mut Evaluator, input: Value, keys: bool) -> Result<Value, Signal> {
    match seq_value(eval, &input)? {
        Value::Nil => Ok(Value::Nil),
        s => Ok(if keys {
            Value::Extension(Arc::new(KeySeqValue::new(s)))
        } else {
            Value::Extension(Arc::new(ValSeqValue::new(s)))
        }),
    }
}

/// A `Thunk::Interpose` tail value.
pub(crate) fn interpose_thunk(sep: Value, input: Value) -> Value {
    pending_value(Thunk::Interpose {
        sep,
        input,
        first: true,
    })
}

/// A `Thunk::TakeWhile` tail value.
pub(crate) fn take_while_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::TakeWhile { f, input })
}

/// A `Thunk::DropWhile` tail value.
pub(crate) fn drop_while_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::DropWhile { f, input })
}

/// A `Thunk::Drop` tail value (the count stays raw — validated
/// when forced).
pub(crate) fn drop_thunk(n: Value, input: Value) -> Value {
    pending_value(Thunk::Drop { n, input })
}

/// A `Thunk::TakeNth` tail value (the count stays raw —
/// validated per force through the body's `drop`).
pub(crate) fn take_nth_thunk(n: Value, input: Value) -> Value {
    pending_value(Thunk::TakeNth { n, input })
}

/// A `Thunk::Partition` tail value (counts stay raw; `None` pad
/// drops short windows, `Some` pads and keeps them).
pub(crate) fn partition_thunk(n: Value, step: Value, pad: Option<Value>, input: Value) -> Value {
    pending_value(Thunk::Partition {
        n,
        step,
        pad,
        input,
    })
}

/// A `Thunk::PartitionAll` tail value (counts stay raw).
pub(crate) fn partition_all_thunk(n: Value, step: Value, input: Value) -> Value {
    pending_value(Thunk::PartitionAll { n, step, input })
}

/// A `Thunk::PartitionBy` tail value (runs key off the head's
/// value per step, like the JVM body).
pub(crate) fn partition_by_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::PartitionBy { f, input })
}

/// A `Thunk::Distinct` tail value (nothing seen yet).
pub(crate) fn distinct_thunk(input: Value) -> Value {
    pending_value(Thunk::Distinct {
        seen: HashSet::new(),
        input,
    })
}

/// A `Thunk::Dedupe` tail value (no last item yet).
pub(crate) fn dedupe_thunk(input: Value) -> Value {
    pending_value(Thunk::Dedupe { last: None, input })
}

/// A `Thunk::Keep` tail value.
pub(crate) fn keep_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::Keep { f, input })
}

/// A `Thunk::KeepIndexed` tail value (indices from zero).
pub(crate) fn keep_indexed_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::KeepIndexed { f, input, idx: 0 })
}

/// A `Thunk::MapIndexed` tail value (indices from zero).
pub(crate) fn map_indexed_thunk(f: Value, input: Value) -> Value {
    pending_value(Thunk::MapIndexed { f, input, idx: 0 })
}

/// A `Thunk::Reductions` tail value (`None` init seeds from the head).
pub(crate) fn reductions_thunk(f: Value, init: Option<Value>, input: Value) -> Value {
    pending_value(Thunk::Reductions {
        f,
        acc: init.clone(),
        has_init: init.is_some(),
        input,
        started: false,
    })
}

fn native_seq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/seq", args, &[1])?;
    seq_value(eval, &args[0]).map_err(signal_to_native)
}

fn native_first(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/first", args, &[1])?;
    first_value(eval, &args[0]).map_err(signal_to_native)
}

fn native_rest(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/rest", args, &[1])?;
    rest_value(eval, &args[0]).map_err(signal_to_native)
}

fn native_next(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/next", args, &[1])?;
    next_value(eval, &args[0]).map_err(signal_to_native)
}

pub(crate) fn native_cons(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/cons", args, &[2])?;
    // Like `RT.cons`: nil tails answer a one-list, seq tails pass
    // through untouched (never forced), anything else seqs eagerly
    // (unseqables fail now).
    if matches!(args[1], Value::Nil) {
        return Ok(Value::List(List::from_vec(vec![args[0].clone()])));
    }
    if is_seq_value(&args[1]) {
        return Ok(Value::Extension(Arc::new(ConsValue::new(
            args[0].clone(),
            args[1].clone(),
        ))));
    }
    let tail = seq_value(eval, &args[1]).map_err(signal_to_native)?;
    Ok(Value::Extension(Arc::new(ConsValue::new(
        args[0].clone(),
        tail,
    ))))
}

/// The S5D seq-access natives installed into `clojure.core`.
pub(crate) const LAZY_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("seq", native_seq),
    ("first", native_first),
    ("rest", native_rest),
    ("next", native_next),
    ("cons", native_cons),
];
