// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S12 collections misc (issue #54): structmaps, chunks,
//! `hash-combine`/`mix-collection-hash`, `seque`, `iteration`,
//! `file-seq`, and the `re-matcher`/`re-groups` handle pair.
//! Oracle: `docs/zero-oos-s12-collmisc/probes/*.out` (199 pins).

use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::{Arc, Mutex, mpsc};

use clj_value::{ExtensionValue, HostDef, HostInst, HostKind, HostVal, ThrowClass, Value, Vector};

use crate::core::{cce_err, int_cast_value, number_int_cast};
use crate::error::{NativeError, Signal};
use crate::eval::Evaluator;
use crate::lazy::{
    PendingSeq, Thunk, check_seqable, chunked_parts, empty_list_value, is_chunked_seq_value,
    pending_value, realize_deep,
};
use crate::ns::{NativeFnPtr, check_arity, deref_core, signal_to_native, str_of};
use crate::vars::check_min_arity;

/// The S12 natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const COLLMISC_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("create-struct", native_create_struct),
    ("struct-map", native_struct_map),
    ("struct", native_struct),
    ("accessor", native_accessor),
    ("chunk-buffer", native_chunk_buffer),
    ("chunk-append", native_chunk_append),
    ("chunk", native_chunk),
    ("chunk-cons", native_chunk_cons),
    ("chunk-first", native_chunk_first),
    ("chunk-rest", native_chunk_rest),
    ("chunk-next", native_chunk_next),
    ("chunked-seq?", native_chunked_seq_q),
    ("hash-combine", native_hash_combine),
    ("mix-collection-hash", native_mix_collection_hash),
    ("seque", native_seque),
    ("iteration", native_iteration),
    ("file-seq", native_file_seq),
    ("re-matcher", native_re_matcher),
    ("re-groups", native_re_groups),
];

/// The JVM struct classes (bases report the `$Def` member class).
const STRUCT_CLASS: &str = "clojure.lang.PersistentStructMap";
const STRUCT_DEF_CLASS: &str = "clojure.lang.PersistentStructMap$Def";
const ACCESSOR_CLASS: &str = "clojure.lang.PersistentStructMap$1";
/// The JVM chunk classes.
const CHUNK_CLASS: &str = "clojure.lang.ArrayChunk";
const CHUNK_BUFFER_CLASS: &str = "clojure.lang.ChunkBuffer";
const ICHUNKEDSEQ_CLASS: &str = "clojure.lang.IChunkedSeq";
/// The JVM matcher class.
const MATCHER_CLASS: &str = "java.util.regex.Matcher";

/// Downcast a value to a struct accessor, if it is one.
pub(crate) fn as_accessor(v: &Value) -> Option<&AccessorValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<AccessorValue>(),
        _ => None,
    }
}

/// Downcast a value to a chunk, if it is one.
pub(crate) fn as_chunk(v: &Value) -> Option<&ChunkValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<ChunkValue>(),
        _ => None,
    }
}

/// Downcast a value to a chunk buffer, if it is one.
fn as_chunk_buffer(v: &Value) -> Option<&ChunkBufferValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<ChunkBufferValue>(),
        _ => None,
    }
}

/// Downcast a value to an iteration stepper, if it is one.
pub(crate) fn as_iteration(v: &Value) -> Option<&IterationValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<IterationValue>(),
        _ => None,
    }
}

/// Downcast a value to a matcher handle, if it is one.
fn as_matcher(v: &Value) -> Option<&MatcherHandle> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<MatcherHandle>(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Structmaps
// ---------------------------------------------------------------------------

/// A struct basis: a `HostKind::Struct` descriptor, or the `$Def`
/// cast (oracles `sm-map-nonbasis`, `sm-struct-nonbasis`,
/// `sm-accessor-nonbasis`).
fn struct_def(v: &Value) -> Result<Arc<HostDef>, NativeError> {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) if d.kind == HostKind::Struct => Ok(d.clone()),
            _ => Err(cce_err(v, STRUCT_DEF_CLASS)),
        },
        _ => Err(cce_err(v, STRUCT_DEF_CLASS)),
    }
}

/// `create-struct`: a basis over the given keys (non-empty —
/// oracle `sm-empty-basis`).
fn native_create_struct(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Must supply keys".to_owned(),
        ));
    }
    let def = HostDef {
        name: Arc::from(STRUCT_CLASS),
        kind: HostKind::Struct,
        fields: Vec::new(),
        methods: HashMap::new(),
        interfaces: Vec::new(),
        prefix: None,
        impl_ns: None,
        sigs: HashMap::new(),
        takes_this: true,
        struct_keys: args.to_vec(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Def(Arc::new(def)))))
}

/// `struct-map`: an instance with the inits applied (basis keys
/// first in basis order with nil defaults, then extras in init
/// order — oracles `sm-keys`, `sm-vals`, `sm-extra`).
fn native_struct_map(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/struct-map", args, 1)?;
    let def = struct_def(&args[0])?;
    let inits = &args[1..];
    if inits.len() % 2 == 1 {
        let last = inits.last().expect("odd inits have a last item");
        return Err(crate::coll::odd_keyvals_err(last));
    }
    let mut content = clj_value::new_map();
    for k in def.struct_keys.iter() {
        content = content.insert(k.clone(), Value::Nil);
    }
    for pair in inits.chunks_exact(2) {
        content = content.insert(pair[0].clone(), pair[1].clone());
    }
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def,
        content,
        meta: None,
    }))))
}

/// `struct`: the positional constructor (short positionals
/// default the tail to nil — oracle `sm-struct-short`; extras
/// throw — oracle `sm-struct-extra`).
fn native_struct(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/struct", args, 1)?;
    let def = struct_def(&args[0])?;
    let vals = &args[1..];
    if vals.len() > def.struct_keys.len() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Too many arguments to struct constructor".to_owned(),
        ));
    }
    let mut content = clj_value::new_map();
    for (i, k) in def.struct_keys.iter().enumerate() {
        let v = vals.get(i).cloned().unwrap_or(Value::Nil);
        content = content.insert(k.clone(), v);
    }
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def,
        content,
        meta: None,
    }))))
}

/// `accessor`: a reader for one basis key (a non-basis key
/// throws — oracle `sm-accessor-miss`).
fn native_accessor(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/accessor", args, &[2])?;
    let def = struct_def(&args[0])?;
    if !def.struct_keys.iter().any(|b| b == &args[1]) {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Not a key of struct".to_owned(),
        ));
    }
    Ok(Value::Extension(Arc::new(AccessorValue {
        key: args[1].clone(),
    })))
}

/// A struct accessor: the basis key it reads. Callable (`IFn`)
/// but not `fn?`, like the JVM's `PersistentStructMap$1`
/// (oracles `sm-accessor`, `sm-accessor-fn?`).
#[derive(Clone, Debug)]
pub struct AccessorValue {
    key: Value,
}

impl AccessorValue {
    /// The basis key this accessor reads.
    pub(crate) fn key(&self) -> &Value {
        &self.key
    }
}

impl ExtensionValue for AccessorValue {
    fn class_name(&self) -> String {
        ACCESSOR_CLASS.to_owned()
    }

    fn print(&self) -> String {
        let hex = clj_value::hasheq(&self.key) as u32;
        format!("#object[{ACCESSOR_CLASS} 0x{hex:x} \"{ACCESSOR_CLASS}@{hex:x}\"]")
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(self, o))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_usize(self as *const Self as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Chunks
// ---------------------------------------------------------------------------

/// A single-use chunk-buffer state: the capacity-sized `slots`,
/// the appended `len`, and `taken` (flipped when `chunk` takes
/// the array — oracles `ch-use-once`, `ch-chunk-twice`).
#[derive(Debug)]
struct ChunkBufferState {
    slots: Vec<Value>,
    len: usize,
    taken: bool,
}

/// A `chunk-buffer` value (aliases share the state, like the
/// JVM's mutable buffer object).
#[derive(Clone, Debug)]
pub struct ChunkBufferValue {
    state: Arc<Mutex<ChunkBufferState>>,
}

impl ExtensionValue for ChunkBufferValue {
    fn class_name(&self) -> String {
        CHUNK_BUFFER_CLASS.to_owned()
    }

    fn print(&self) -> String {
        let id = Arc::as_ptr(&self.state) as usize;
        format!("#object[{CHUNK_BUFFER_CLASS} 0x{id:x} \"{CHUNK_BUFFER_CLASS}@{id:x}\"]")
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| Arc::ptr_eq(&self.state, &o.state))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_usize(Arc::as_ptr(&self.state) as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// An immutable chunk: the capacity-sized `slots` plus the
/// appended `count` (`None` slots is the degenerate second chunk
/// of a spent buffer — oracle `ch-chunk-twice`).
#[derive(Clone, Debug)]
pub struct ChunkValue {
    slots: Option<Arc<[Value]>>,
    count: usize,
}

impl ChunkValue {
    /// The appended items (never the nil holes).
    pub(crate) fn items_arc(&self) -> Arc<[Value]> {
        match &self.slots {
            Some(slots) => Arc::from(&slots[..self.count]),
            None => Arc::from([]),
        }
    }
}

impl ExtensionValue for ChunkValue {
    fn class_name(&self) -> String {
        CHUNK_CLASS.to_owned()
    }

    fn print(&self) -> String {
        // (Slice pointers are fat — hash the thin data pointer.)
        let id = self.slots.as_ref().map_or(0, |s| s.as_ptr() as usize);
        format!("#object[{CHUNK_CLASS} 0x{id:x} \"{CHUNK_CLASS}@{id:x}\"]")
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| match (&self.slots, &o.slots) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => std::ptr::eq(self, o),
                _ => false,
            })
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        match &self.slots {
            Some(slots) => state.write_usize(slots.as_ptr() as usize),
            None => state.write_usize(self as *const Self as usize),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A head chunk over stepped items (nil-padded to 32, like the
/// JVM's `ArrayChunk` arrays behind seq chunks).
pub(crate) fn head_chunk(mut items: Vec<Value>) -> ChunkValue {
    let count = items.len();
    if items.len() < clj_value::CHUNK_SIZE {
        items.resize(clj_value::CHUNK_SIZE, Value::Nil);
    }
    ChunkValue {
        slots: Some(Arc::from(items)),
        count,
    }
}

/// The array-store NPE (append after `chunk` — oracle
/// `ch-use-once`).
fn store_npe() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot store to object array because \"this.buffer\" is null".to_owned(),
    )
}

/// The array-load NPE (count/nth on a degenerate chunk — oracle
/// `ch-chunk-twice`).
fn load_npe() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot load from object array because \"this.array\" is null".to_owned(),
    )
}

/// The JVM array-index message (oracles `ch-overflow`,
/// `ch-zero-append`, `ch-nth-cap-oob`).
fn aioobe(index: i32, len: usize) -> NativeError {
    NativeError::new(
        ThrowClass::ArrayIndexOutOfBoundsException,
        format!("Index {index} out of bounds for length {len}"),
    )
}

/// `chunk-buffer`: a single-use builder over a capacity-sized
/// array (negative capacities throw — oracle `ch-neg-cap`).
fn native_chunk_buffer(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-buffer", args, &[1])?;
    let cap = int_cast_value(&args[0])?;
    if cap < 0 {
        return Err(NativeError::new(
            ThrowClass::NegativeArraySizeException,
            cap.to_string(),
        ));
    }
    let cap = cap as usize;
    Ok(Value::Extension(Arc::new(ChunkBufferValue {
        state: Arc::new(Mutex::new(ChunkBufferState {
            slots: vec![Value::Nil; cap],
            len: 0,
            taken: false,
        })),
    })))
}

/// `chunk-append`: append one item, answering nil (past capacity
/// is the array-store failure — oracle `ch-overflow`; a spent
/// buffer is the store NPE — oracle `ch-use-once`).
fn native_chunk_append(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-append", args, &[2])?;
    let Some(b) = as_chunk_buffer(&args[0]) else {
        return Err(cce_err(&args[0], CHUNK_BUFFER_CLASS));
    };
    let mut state = b.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.taken {
        return Err(store_npe());
    }
    if state.len >= state.slots.len() {
        return Err(aioobe(state.len as i32, state.slots.len()));
    }
    let at = state.len;
    state.slots[at] = args[1].clone();
    state.len = at + 1;
    Ok(Value::Nil)
}

/// `chunk`: take the buffer's array (killing the buffer); a
/// second take answers the degenerate chunk (oracle
/// `ch-chunk-twice`).
fn native_chunk(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk", args, &[1])?;
    let Some(b) = as_chunk_buffer(&args[0]) else {
        return Err(cce_err(&args[0], CHUNK_BUFFER_CLASS));
    };
    let mut state = b.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.taken {
        return Ok(Value::Extension(Arc::new(ChunkValue {
            slots: None,
            count: 0,
        })));
    }
    state.taken = true;
    Ok(Value::Extension(Arc::new(ChunkValue {
        slots: Some(Arc::from(state.slots.clone())),
        count: state.len,
    })))
}

/// `count` of a chunk: the appended items (a degenerate chunk
/// raises its NPE — oracle `ch-chunk-twice`).
pub(crate) fn chunk_count(c: &ChunkValue) -> Result<i64, NativeError> {
    match &c.slots {
        Some(_) => Ok(c.count as i64),
        None => Err(load_npe()),
    }
}

/// `nth` of a chunk: the 3-arity bounds against the count (like
/// `RT.nthFrom` on `Indexed`); the 2-arity reads the raw array
/// (nil holes below capacity — oracles `ch-nth-oob`,
/// `ch-nth-cap`).
pub(crate) fn chunk_nth(
    c: &ChunkValue,
    n: i32,
    not_found: Option<&Value>,
) -> Result<Value, NativeError> {
    let Some(slots) = &c.slots else {
        return Err(load_npe());
    };
    if let Some(nf) = not_found {
        if n >= 0 && (n as usize) < c.count {
            return Ok(slots[n as usize].clone());
        }
        return Ok(nf.clone());
    }
    if n < 0 || (n as usize) >= slots.len() {
        return Err(aioobe(n, slots.len()));
    }
    Ok(slots[n as usize].clone())
}

/// `chunk-cons`: the chunk's items ahead of the tail (an empty
/// chunk answers the tail identically — oracle `ch-cons-empty`).
fn native_chunk_cons(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-cons", args, &[2])?;
    let Some(c) = as_chunk(&args[0]) else {
        return Err(cce_err(&args[0], "clojure.lang.IChunk"));
    };
    // The count reads first (a degenerate chunk raises its NPE,
    // like the JVM probing `.count`).
    if chunk_count(c)? == 0 {
        return Ok(args[1].clone());
    }
    check_seqable(eval, &args[1]).map_err(signal_to_native)?;
    Ok(PendingSeq::pending_chunked_cons(Thunk::ChunkHead {
        chunk: c.clone(),
        tail: args[1].clone(),
    }))
}

/// `chunk-first`: the head chunk (anything else takes the
/// `IChunkedSeq` cast — oracles `ch-first-list`, `ch-first-str`).
fn native_chunk_first(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-first", args, &[1])?;
    match chunked_parts(eval, &args[0]).map_err(signal_to_native)? {
        Some((chunk, _)) => Ok(Value::Extension(Arc::new(chunk))),
        None => Err(cce_err(&args[0], ICHUNKEDSEQ_CLASS)),
    }
}

/// `chunk-rest`: the seq after the head chunk (`()` at the end —
/// oracle `ch-rest-end`).
fn native_chunk_rest(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-rest", args, &[1])?;
    match chunked_parts(eval, &args[0]).map_err(signal_to_native)? {
        Some((_, tail)) => Ok(tail.unwrap_or_else(empty_list_value)),
        None => Err(cce_err(&args[0], ICHUNKEDSEQ_CLASS)),
    }
}

/// `chunk-next`: the seq after the head chunk (nil at the end —
/// oracle `ch-next-end`).
fn native_chunk_next(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunk-next", args, &[1])?;
    match chunked_parts(eval, &args[0]).map_err(signal_to_native)? {
        Some((_, tail)) => Ok(tail.unwrap_or(Value::Nil)),
        None => Err(cce_err(&args[0], ICHUNKEDSEQ_CLASS)),
    }
}

/// `chunked-seq?`: true for bounded ranges, realized chunked
/// seqs, and the emulated chunked cons (§1.3 battery).
fn native_chunked_seq_q(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/chunked-seq?", args, &[1])?;
    Ok(Value::Bool(is_chunked_seq_value(&args[0])))
}

// ---------------------------------------------------------------------------
// Hash combine and mix
// ---------------------------------------------------------------------------

/// `mix-collection-hash`: `Murmur3.mixCollHash` over int-checked
/// longs (oracles `mh-mix-*`).
fn native_mix_collection_hash(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/mix-collection-hash", args, &[2])?;
    let h = number_int_cast(&args[0])?;
    let n = number_int_cast(&args[1])?;
    Ok(Value::Int(i64::from(clj_value::mix_coll_hash(h, n))))
}

/// `hash-combine`: `Util.hashCombine` over `Util.hash` (never
/// murmur — oracles `mh-util-spot`, `mh-comb-formula`). Same
/// realize + infinite-UOE contract as `hash` (the JVM walks the
/// value).
fn native_hash_combine(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/hash-combine", args, &[2])?;
    let seed = number_int_cast(&args[0])?;
    if clj_value::seq_hash_unsupported(&args[1]) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    let y = realize_deep(eval, &args[1]).map_err(signal_to_native)?;
    if clj_value::seq_hash_unsupported(&y) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    Ok(Value::Int(i64::from(clj_value::hash_combine(
        seed,
        clj_value::hash_code(&y),
    ))))
}

// ---------------------------------------------------------------------------
// Seque
// ---------------------------------------------------------------------------

/// The receiving end of a `seque` feeder queue: `Some` items flow
/// until the `None` end-of-stream sentinel (nil elements survive
/// — the sentinel is a distinct variant); a dropped feeder reads
/// as end-of-stream too.
#[derive(Clone)]
pub struct SequeRx(Arc<Mutex<mpsc::Receiver<Option<Value>>>>);

impl std::fmt::Debug for SequeRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SequeRx(..)")
    }
}

impl SequeRx {
    fn new(rx: mpsc::Receiver<Option<Value>>) -> Self {
        Self(Arc::new(Mutex::new(rx)))
    }

    /// Lock the receiver for one blocking take.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, mpsc::Receiver<Option<Value>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A thread-spawn failure (mirrors the `future` constructor —
/// the OS refused a new thread).
fn spawn_failed() -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        "unable to create native thread".to_owned(),
    )
}

/// `seque`: a queued seq (default capacity 100). Capacities route
/// through the `(int x)` cast (oracles `sq-cap-str`,
/// `sq-cap-float`); non-positive capacities throw bare
/// (oracles `sq-zero-cap`, `sq-neg-cap`).
fn native_seque(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/seque", args, &[1, 2])?;
    let (cap, input) = if args.len() == 1 {
        (100i64, &args[0])
    } else {
        (int_cast_value(&args[0])?, &args[1])
    };
    if cap <= 0 {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            String::new(),
        ));
    }
    let (tx, rx) = mpsc::sync_channel(cap as usize);
    let child = eval.fork_for_thread(eval.clone_bindings());
    let input = input.clone();
    let name = format!("clj-seque-{}", eval.next_id());
    std::thread::Builder::new()
        .name(name)
        .spawn(move || seque_feeder(child, input, tx))
        .map_err(|_| spawn_failed())?;
    Ok(pending_value(Thunk::Seque {
        rx: SequeRx::new(rx),
    }))
}

/// A `seque` feeder thread: step the input on the forked
/// evaluator, backpressuring on the bounded queue. End-of-input
/// sends the eos sentinel; a producer failure drops the sender
/// (the drain reads that as end-of-stream — producer-error
/// propagation is racy by design and unpinned, R2).
/// The thread is detached, never joined: an infinite producer
/// with a bounded consumer parks on `send` forever (like the
/// JVM's daemon feeder), so joining at shutdown could hang.
fn seque_feeder(mut child: Evaluator, input: Value, tx: mpsc::SyncSender<Option<Value>>) {
    use crate::lazy::{first_value, next_value, seq_value};
    let mut cur = match seq_value(&mut child, &input) {
        Ok(s) => s,
        Err(_) => return,
    };
    loop {
        if matches!(cur, Value::Nil) {
            let _ = tx.send(None);
            return;
        }
        let head = match first_value(&mut child, &cur) {
            Ok(h) => h,
            Err(_) => return,
        };
        if tx.send(Some(head)).is_err() {
            return;
        }
        cur = match next_value(&mut child, &cur) {
            Ok(n) => n,
            Err(_) => return,
        };
    }
}

// ---------------------------------------------------------------------------
// Iteration
// ---------------------------------------------------------------------------

/// An `iteration` stepper: the step fn plus resolved opts.
/// Re-seqable (every `seq` re-steps from `:initk` — oracle
/// `it-twice`).
#[derive(Clone, Debug)]
pub struct IterationValue {
    pub(crate) step: Value,
    pub(crate) somef: Value,
    pub(crate) vf: Value,
    pub(crate) kf: Value,
    pub(crate) initk: Value,
}

impl ExtensionValue for IterationValue {
    fn class_name(&self) -> String {
        "clojure.core$iteration".to_owned()
    }

    fn print(&self) -> String {
        let id = self as *const Self as usize;
        format!("#object[clojure.core$iteration 0x{id:x} \"clojure.core$iteration@{id:x}\"]")
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(self, o))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_usize(self as *const Self as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// One collected `iteration` option (known keys only — unknown
/// opts are ignored, oracle `it-unknown-opt`).
struct IterationOpts {
    somef: Option<Value>,
    vf: Option<Value>,
    kf: Option<Value>,
    initk: Option<Value>,
}

/// Collect `iteration` opts from flat keyvals (a nil `:somef` /
/// `:vf` / `:kf` takes the default, like `:or`).
fn iteration_opts(opts: &[Value]) -> Result<IterationOpts, NativeError> {
    if opts.len() % 2 == 1 {
        let last = opts.last().expect("odd opts have a last item");
        return Err(crate::coll::odd_keyvals_err(last));
    }
    let mut out = IterationOpts {
        somef: None,
        vf: None,
        kf: None,
        initk: None,
    };
    for pair in opts.chunks_exact(2) {
        let Value::Keyword(kw) = &pair[0] else {
            continue;
        };
        if kw.namespace().is_some() {
            continue;
        }
        let slot = match kw.name() {
            "somef" => &mut out.somef,
            "vf" => &mut out.vf,
            "kf" => &mut out.kf,
            "initk" => &mut out.initk,
            _ => continue,
        };
        *slot = Some(pair[1].clone());
    }
    Ok(out)
}

/// A collected opt, defaulting nils (like `:or`).
fn opt_or(opt: Option<Value>, default: Value) -> Value {
    match opt {
        Some(v) if !matches!(v, Value::Nil) => v,
        _ => default,
    }
}

/// `iteration`: a seqable/reducible stepper (defaults: `:somef`
/// `some?`, `:vf`/`:kf` `identity`, `:initk` nil).
fn native_iteration(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core/iteration", args, 1)?;
    let opts = iteration_opts(&args[1..])?;
    Ok(Value::Extension(Arc::new(IterationValue {
        step: args[0].clone(),
        somef: opt_or(opts.somef, deref_core(eval, "some?")),
        vf: opt_or(opts.vf, deref_core(eval, "identity")),
        kf: opt_or(opts.kf, deref_core(eval, "identity")),
        initk: opts.initk.unwrap_or(Value::Nil),
    })))
}

/// One `iteration` step at `k`: `None` when `:somef` rejects the
/// return (which `:vf` never sees — oracle `it-somef-skips`);
/// else the emitted value plus its tail (a nil `:kf` stops the
/// walk — oracle `it-kf-stop`).
pub(crate) fn iteration_advance(
    eval: &mut Evaluator,
    step: &Value,
    somef: &Value,
    vf: &Value,
    kf: &Value,
    k: &Value,
) -> Result<Option<(Value, Value)>, Signal> {
    let ret = eval.call(step, std::slice::from_ref(k))?;
    let ok = eval.call(somef, std::slice::from_ref(&ret))?;
    if !ok.is_truthy() {
        return Ok(None);
    }
    let v = eval.call(vf, std::slice::from_ref(&ret))?;
    let next_k = eval.call(kf, std::slice::from_ref(&ret))?;
    let tail = if matches!(next_k, Value::Nil) {
        empty_list_value()
    } else {
        pending_value(Thunk::Iteration {
            step: step.clone(),
            somef: somef.clone(),
            vf: vf.clone(),
            kf: kf.clone(),
            k: next_k,
        })
    };
    Ok(Some((v, tail)))
}

/// `seq` of an `iteration` value: one step eagerly (oracle
/// `it-step-once`), the rest lazy per cell; a rejected first
/// return seqs to nil (oracle `it-empty`).
pub(crate) fn seq_iteration(eval: &mut Evaluator, it: &IterationValue) -> Result<Value, Signal> {
    match iteration_advance(eval, &it.step, &it.somef, &it.vf, &it.kf, &it.initk)? {
        None => Ok(Value::Nil),
        Some((v, tail)) => Ok(pending_value(Thunk::Chain {
            head: vec![v],
            tail,
        })),
    }
}

// ---------------------------------------------------------------------------
// File-seq
// ---------------------------------------------------------------------------

/// `file-seq`: a lazy seq of path strings (root first; non-dir
/// and missing roots walk a singleton). The port takes and
/// yields strings — the JVM takes and yields `java.io.File`
/// (documented divergence, §1.7).
fn native_file_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/file-seq", args, &[1])?;
    let Value::Str(path) = &args[0] else {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot open <{}> as a directory.", str_of(&args[0])),
        ));
    };
    Ok(pending_value(Thunk::FileSeq {
        stack: vec![path.to_string()],
    }))
}

// ---------------------------------------------------------------------------
// Re-matcher
// ---------------------------------------------------------------------------

/// A matcher handle's mutable state: the next search offset plus
/// the last match groups (full text first, one slot per group —
/// `None` for unmatched groups, like `Matcher.group` returning
/// null).
#[derive(Clone, Debug)]
struct MatcherState {
    next: usize,
    last: Option<Vec<Option<String>>>,
}

/// A stateful regex matcher handle (`re-matcher`): the compiled
/// pattern, the subject text, the next search offset, and the
/// last match (or none). `re-find` advances it (an empty match
/// advances one char — oracle `rm-empty-advance`); `re-groups`
/// reads the last match.
#[derive(Clone, Debug)]
pub struct MatcherHandle {
    re: regex::Regex,
    text: Arc<str>,
    state: Arc<Mutex<MatcherState>>,
}

impl MatcherHandle {
    /// Advance one `find` (the `re-groups` shape, nil on no
    /// match — an exhausted handle stays nil, oracle
    /// `rm-exhaust`).
    fn find_next(&self) -> Result<Value, NativeError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let text: &str = &self.text;
        if state.next > text.len() {
            state.last = None;
            return Ok(Value::Nil);
        }
        let Some(caps) = self.re.captures_at(text, state.next) else {
            state.last = None;
            return Ok(Value::Nil);
        };
        let m = caps.get(0).expect("a match holds group 0");
        state.next = if m.start() == m.end() {
            m.end() + text[m.end()..].chars().next().map_or(1, |c| c.len_utf8())
        } else {
            m.end()
        };
        let groups: Vec<Option<String>> = (0..caps.len())
            .map(|i| caps.get(i).map(|g| g.as_str().to_owned()))
            .collect();
        state.last = Some(groups.clone());
        Ok(groups_value_owned(&groups))
    }

    /// The last match groups, if any.
    fn last_groups(&self) -> Option<Vec<Option<String>>> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last
            .clone()
    }
}

impl ExtensionValue for MatcherHandle {
    fn class_name(&self) -> String {
        MATCHER_CLASS.to_owned()
    }

    fn print(&self) -> String {
        let id = Arc::as_ptr(&self.state) as usize;
        let last = self
            .last_groups()
            .and_then(|g| g.into_iter().next().flatten())
            .unwrap_or_default();
        format!(
            "#object[{MATCHER_CLASS} 0x{id:x} \"{MATCHER_CLASS}[pattern={} region=0,{} lastmatch={last}]\"]",
            self.re.as_str(),
            self.text.chars().count()
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| Arc::ptr_eq(&self.state, &o.state))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_usize(Arc::as_ptr(&self.state) as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// One `re-groups` over owned group texts (mirrors `re.rs`): the
/// whole match when the pattern holds no groups, else the
/// `[full g1 ...]` vector (nil for unmatched groups).
fn groups_value_owned(groups: &[Option<String>]) -> Value {
    if groups.len() == 1 {
        return Value::Str(Arc::from(groups[0].as_deref().unwrap_or("")));
    }
    Value::Vector(Vector::from_vec(
        groups
            .iter()
            .map(|g| match g {
                Some(s) => Value::Str(Arc::from(s.as_str())),
                None => Value::Nil,
            })
            .collect(),
    ))
}

/// A `regex`-crate compile failure as the JVM's
/// `PatternSyntaxException` (mirrors `re.rs` — only the reader
/// mints `Value::Regex`, so this is a stale-value defense).
fn pattern_syntax(err: regex::Error) -> NativeError {
    NativeError::new(ThrowClass::PatternSyntaxException, err.to_string())
}

/// `re-matcher`: a stateful handle over a pattern and a subject
/// (a non-pattern fails the `Pattern` cast, a non-string the
/// `CharSequence` cast — oracles `rm-bad-pattern`,
/// `rm-bad-subject`; nil trips the `length` NPE — oracle
/// `rm-nil-subject`).
fn native_re_matcher(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-matcher", args, &[2])?;
    let Value::Regex(source) = &args[0] else {
        return Err(cce_err(&args[0], "java.util.regex.Pattern"));
    };
    match &args[1] {
        Value::Str(text) => {
            let re = regex::Regex::new(source).map_err(pattern_syntax)?;
            Ok(Value::Extension(Arc::new(MatcherHandle {
                re,
                text: Arc::clone(text),
                state: Arc::new(Mutex::new(MatcherState {
                    next: 0,
                    last: None,
                })),
            })))
        }
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
                .to_owned(),
        )),
        other => Err(cce_err(other, "java.lang.CharSequence")),
    }
}

/// `re-groups`: the last match of a handle (no match is
/// `IllegalStateException` — oracles `rm-groups-nomatch`,
/// `rm-groups-fresh`; a non-handle fails the `Matcher` cast —
/// oracle `rm-groups-bad`).
fn native_re_groups(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-groups", args, &[1])?;
    let Some(h) = as_matcher(&args[0]) else {
        return Err(cce_err(&args[0], MATCHER_CLASS));
    };
    match h.last_groups() {
        Some(groups) => Ok(groups_value_owned(&groups)),
        None => Err(NativeError::new(
            ThrowClass::IllegalStateException,
            "No match found".to_owned(),
        )),
    }
}

/// The S12 `re-find` 1-arity companion: advance a matcher handle
/// (`None` for every other value — the caller keeps the #32
/// `Matcher` cast).
pub(crate) fn match_handle_find(v: &Value) -> Option<Result<Value, NativeError>> {
    as_matcher(v).map(MatcherHandle::find_next)
}
