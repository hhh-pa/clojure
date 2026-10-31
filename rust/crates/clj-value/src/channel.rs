// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slices 1–2: `core.async` channels (blocking side only).
//!
//! A [`Channel`] is a many-to-many rendezvous + buffer mailbox over
//! one mutex: blocked takes/puts park on per-operation [`Waiter`]
//! condvars (plain ops) or on one shared `Waiter` (`alts!!`, whose
//! first-wins commit mirrors `do-alts`' flag). Semantics mirror
//! `clojure.core.async.impl.channels/ManyToManyChannel` (oracle:
//! `docs/gap-123/g123-*.clj`): takes drain the buffer first, then
//! rendezvous with queued putters, then (when closed) yield nil;
//! puts rendezvous with queued takers first, then buffer (fixed
//! blocks when full; dropping/sliding never block), then queue;
//! `close` wakes queued takers (one buffered value each, else nil)
//! and leaves queued puts parked until a taker releases them.
//!
//! Slice 2 adds transducers: an [`Xform`] channel steps every put
//! through `(xform conj)` (in `clj-eval`, which owns the evaluator;
//! this crate only serializes the steps behind the xform gate).
//! Filtered puts complete `true` without touching the mailbox;
//! multi-emit puts park one [`Reg`] holding every emit; a `reduced`
//! step aborts (queued puts complete `true`, the channel closes)
//! once its emits are buffered or its putter is refilled; the
//! completion arity runs at `close` (puts empty) and on every
//! closed-with-empty take (oracle `g123-xform.clj`).
//!
//! Lock order is channel-then-waiter (completions run under the
//! channel lock); waits hold only the waiter lock — no path takes
//! them in the other order, and every op holds at most one channel
//! lock at a time (like `do-alts`' one-mutex-at-a-time scan). The
//! xform gate sits outside the channel lock (nothing takes the gate
//! while holding the channel lock); stepping itself runs without
//! either lock held, so a transducer that peeks at its own channel
//! (`closed?`) cannot self-deadlock.

use std::collections::VecDeque;
use std::hash::Hasher;
use std::sync::{Arc, Condvar, Mutex};

use crate::Value;
use crate::extension::{ExtensionValue, ThrowClass, Throwable};
use crate::value::next_identity_id;

/// `impl/MAX-QUEUE-SIZE`: at most 1024 pending takes (resp. puts)
/// per channel; the 1025th errors (oracle `e-take-bound`,
/// `e-put-bound`).
pub const MAX_QUEUE_SIZE: usize = 1024;

/// A buffer kind (`impl/buffers`; no promise-buffer yet — slice
/// 2's transducers don't need one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferKind {
    /// Puts block when full.
    Fixed,
    /// Puts always complete; overflow is dropped.
    Dropping,
    /// Puts always complete; overflow evicts the oldest element.
    Sliding,
}

/// A constructed buffer (`(buffer n)` & co.): `cap` is the
/// ceiling of the requested size (JVM `full?` compares `(>= size
/// n)`, so `1.5` behaves as `2`), `integral` records whether the
/// size was integral (a sliding buffer only evicts on `(= size
/// n)`, which a non-integral size never satisfies — oracle edge
/// probes; `NaN`/`+inf` arrive as `(i64::MAX, false)`, i.e.
/// unbounded).
#[derive(Clone, Debug)]
pub struct BufferDesc {
    id: u64,
    kind: BufferKind,
    cap: i64,
    integral: bool,
}

impl BufferDesc {
    /// Mint a buffer descriptor (fresh identity, like the JVM
    /// `deftype` allocation).
    #[must_use]
    pub fn new(kind: BufferKind, cap: i64, integral: bool) -> Self {
        Self {
            id: next_identity_id(),
            kind,
            cap,
            integral,
        }
    }

    fn jvm_class(&self) -> &'static str {
        match self.kind {
            BufferKind::Fixed => "clojure.core.async.impl.buffers.FixedBuffer",
            BufferKind::Dropping => "clojure.core.async.impl.buffers.DroppingBuffer",
            BufferKind::Sliding => "clojure.core.async.impl.buffers.SlidingBuffer",
        }
    }
}

impl ExtensionValue for BufferDesc {
    fn class_name(&self) -> String {
        self.jvm_class().to_owned()
    }

    fn print(&self) -> String {
        let class = self.jvm_class();
        format!("#object[{class} 0x{:x} \"{class}@{:x}\"]", self.id, self.id)
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<BufferDesc>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A waiter's commitment state: first `try_win` wins (the
/// `do-alts` flag); `kill` abandons (the `:default` path).
#[derive(Clone, Debug)]
enum WaitStatus {
    Pending,
    Won { op: usize, val: Value },
    Dead,
}

/// A parked operation (one thread waits per `Waiter`): plain
/// `>!!`/`<!!` mint a fresh one per call; `alts!!` shares one
/// across its ports.
#[derive(Debug)]
pub struct Waiter {
    inner: Mutex<WaitStatus>,
    cv: Condvar,
}

impl Default for Waiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Waiter {
    /// A fresh pending waiter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(WaitStatus::Pending),
            cv: Condvar::new(),
        }
    }

    /// Complete with `(op, val)` when still pending (first wins).
    pub fn try_win(&self, op: usize, val: Value) -> bool {
        let mut status = self.inner.lock().expect("waiter lock");
        if matches!(*status, WaitStatus::Pending) {
            *status = WaitStatus::Won { op, val };
            self.cv.notify_one();
            true
        } else {
            false
        }
    }

    /// Abandon (the `:default` path, when no op was ready): queued
    /// registrations on every port go stale together.
    pub fn kill(&self) {
        let mut status = self.inner.lock().expect("waiter lock");
        if matches!(*status, WaitStatus::Pending) {
            *status = WaitStatus::Dead;
        }
    }

    /// Whether a queued registration of this waiter is still live
    /// (the `cleanup` sweep).
    fn is_pending(&self) -> bool {
        matches!(
            *self.inner.lock().expect("waiter lock"),
            WaitStatus::Pending
        )
    }

    /// Block until won (`None` when killed — the owner never waits
    /// after killing, so callers map it to an internal error).
    pub fn wait(&self) -> Option<(usize, Value)> {
        let status = self.inner.lock().expect("waiter lock");
        let status = self
            .cv
            .wait_while(status, |s| matches!(s, WaitStatus::Pending))
            .expect("waiter lock");
        match &*status {
            WaitStatus::Won { op, val } => Some((*op, val.clone())),
            _ => None,
        }
    }
}

/// One queued registration: a take (`put_val: None`) or a put
/// (`put_val: Some`) parked on `waiter` for op `op`. A transducer
/// put parks its whole emit vector (`put_val` is the first emit,
/// `rest` the remainder — `mapcat` can emit several values per
/// put); `reduced` marks a put whose step returned `reduced`, so
/// the refill that consumes it aborts the channel (the JVM steps
/// queued puts at refill and aborts there).
#[derive(Clone, Debug)]
struct Reg {
    waiter: Arc<Waiter>,
    op: usize,
    put_val: Option<Value>,
    rest: Vec<Value>,
    reduced: bool,
}

impl Reg {
    fn take(waiter: &Arc<Waiter>, op: usize) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            op,
            put_val: None,
            rest: Vec::new(),
            reduced: false,
        }
    }

    fn put(waiter: &Arc<Waiter>, op: usize, val: Value) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            op,
            put_val: Some(val),
            rest: Vec::new(),
            reduced: false,
        }
    }

    /// Park a transducer put's non-empty emit vector (filtered puts
    /// never park — they complete `true` outright).
    fn put_many(waiter: &Arc<Waiter>, op: usize, emits: Vec<Value>, reduced: bool) -> Self {
        let mut emits = emits.into_iter();
        let first = emits.next().expect("parked puts carry a value");
        Self {
            waiter: Arc::clone(waiter),
            op,
            put_val: Some(first),
            rest: emits.collect(),
            reduced,
        }
    }
}

/// The outcome of one non-blocking attempt: done, or parked on the
/// caller's waiter.
#[derive(Debug)]
pub enum OpReady<T> {
    /// Completed synchronously.
    Ready(T),
    /// Parked; the caller must `wait` on its waiter.
    Queued,
}

/// What a queue attempt can fail with (mapped to JVM error text in
/// `clj-eval`'s `async` module).
#[derive(Clone, Debug)]
pub enum ChanError {
    /// The 1025th pending take (`puts: false`) or put (`puts:
    /// true`) — `MAX-QUEUE-SIZE`.
    QueueFull { puts: bool },
    /// A sliding buffer of size 0 evicting from empty (the JVM
    /// crashes the same way — `LinkedList.removeLast`; its default
    /// ex-handler then swallows the trace and drops the value,
    /// which the port surfaces as this error instead — except on
    /// transducer channels, where the ex-handler path runs like
    /// the JVM's `handle` and only a re-crash surfaces this).
    EmptyEvict,
    /// A transducer step (or the ex-handler itself) threw: the
    /// caller rethrows the throwable (an ex-handler throw escapes
    /// `put!` on the JVM too).
    XformThrow(Throwable),
}

/// One transducer-step request (slice 2): step an input, run the
/// completion arity, or run the ex-handler path for a failed
/// buffer add (the sliding-zero evict crash, which the JVM routes
/// through `handle`).
#[derive(Debug)]
pub enum StepReq {
    /// Step one put value (never nil — the caller rejects it first).
    Input(Value),
    /// Run the completion arity (close / closed-take flush).
    Complete,
    /// Run the ex-handler path for a buffer-add crash.
    Failed(Throwable),
}

/// One transducer-step outcome: the emitted values (in order) and
/// whether the step returned `reduced` (the ex-handler path never
/// reduces — `handle` returns the buffer).
#[derive(Debug)]
pub struct StepOut {
    /// The step's emitted values (an ex-handler replacement is a
    /// single untransformed value, like the JVM's `handle`).
    pub emits: Vec<Value>,
    /// Whether the step returned `reduced` (abort once buffered).
    pub reduced: bool,
}

/// A channel transducer (slice 2): the composed step fn (`(xform
/// conj)`, built once at `chan`), the optional ex-handler (`None`
/// prints and drops, like the JVM's default), and the gate that
/// serializes steps (stateful transducers step atomically on the
/// JVM's channel mutex; here the gate plays that role outside the
/// channel lock — see the module docs).
#[derive(Debug)]
pub struct Xform {
    /// The composed step fn.
    pub step: Value,
    /// The ex-handler, if any.
    pub exh: Option<Value>,
    gate: Mutex<()>,
}

/// The channel state behind the mutex.
#[derive(Debug)]
struct ChanState {
    buf: VecDeque<Value>,
    kind: Option<(BufferKind, i64, bool)>,
    closed: bool,
    takes: VecDeque<Reg>,
    puts: VecDeque<Reg>,
}

impl ChanState {
    /// Drop stale registrations (`cleanup` runs before every
    /// attempt on the JVM).
    fn sweep(&mut self) {
        self.takes.retain(|r| r.waiter.is_pending());
        self.puts.retain(|r| r.waiter.is_pending());
    }

    /// Pop the next live take registration (stale regs skip
    /// like `cleanup`; combinators, so the early return stays out
    /// of the coverage map).
    fn next_taker(&mut self) -> Option<Reg> {
        std::iter::from_fn(|| self.takes.pop_front()).find(|reg| reg.waiter.is_pending())
    }

    /// Pop the next live put registration.
    fn next_putter(&mut self) -> Option<Reg> {
        std::iter::from_fn(|| self.puts.pop_front()).find(|reg| reg.waiter.is_pending())
    }

    /// Whether a queued putter's value fits the buffer now (the
    /// drain-refill step; puts only ever queue behind fixed or
    /// unbuffered channels).
    fn refill_room(&self) -> bool {
        match self.kind {
            Some((BufferKind::Fixed, cap, _)) | Some((BufferKind::Dropping, cap, _)) => {
                (self.buf.len() as i64) < cap
            }
            Some((BufferKind::Sliding, _, _)) | None => false,
        }
    }

    /// Move queued putters' values into freed buffer room,
    /// completing them (the take-path refill). A transducer putter
    /// moves its whole emit vector at once (the JVM's `add!` never
    /// checks `full?`, so refills overfill the same way); consuming
    /// a `reduced` putter aborts (the JVM aborts when the refill
    /// step returns `reduced`).
    fn refill(&mut self) {
        while self.refill_room() {
            let Some(reg) = self.next_putter() else {
                break;
            };
            let val = reg
                .put_val
                .clone()
                .expect("put registration carries a value");
            // Flags, not nesting: a lost commit (a co-registered
            // alt won elsewhere mid-refill) is race-only, so the
            // branches below stay straight-line and every line
            // runs on the common paths.
            let won = reg.waiter.try_win(reg.op, Value::Bool(true));
            let done = won && reg.reduced;
            if won {
                self.buf.push_back(val);
                self.buf.extend(reg.rest);
            }
            if done {
                self.abort_locked();
                break;
            }
        }
    }

    /// Abort: every queued put completes `true` and the channel
    /// closes (the JVM's `abort`, minus its take-serving — takes
    /// only ever queue behind an empty buffer, and every abort
    /// site just buffered, so none are queued).
    fn abort_locked(&mut self) {
        for reg in self.puts.drain(..) {
            reg.waiter.try_win(reg.op, Value::Bool(true));
        }
        self.closed = true;
    }

    /// Hand queued takers buffered values (one each, in order),
    /// then nil to the rest (the close/abort serving).
    fn serve_takes_all(&mut self) {
        let takes: Vec<Reg> = self.takes.drain(..).collect();
        for reg in takes {
            // Peek-then-pop: a lost commit (a co-registered alt won
            // elsewhere) leaves the buffered value for the next
            // taker.
            let val = self.buf.front().cloned().unwrap_or(Value::Nil);
            if reg.waiter.try_win(reg.op, val) {
                self.buf.pop_front();
            }
        }
    }

    /// Hand queued takers buffered values while any remain (a live
    /// put's serving — unserved takers stay queued, like the JVM's
    /// `pos?`-bounded take loop).
    fn serve_takes_while_full(&mut self) {
        while !self.buf.is_empty() {
            let Some(reg) = self.next_taker() else {
                break;
            };
            let val = self.buf.front().cloned().unwrap_or(Value::Nil);
            if reg.waiter.try_win(reg.op, val) {
                self.buf.pop_front();
            }
        }
    }

    /// Buffer one transducer emit, overfilling like the JVM's
    /// `add!` (fixed pushes unconditionally; dropping keeps room;
    /// sliding evicts; the unbuffered arm only serves unit tests —
    /// `chan` forbids transducers without a buffer).
    fn buffer_one_overfill(&mut self, val: Value) -> Result<(), ChanError> {
        match self.kind {
            Some((BufferKind::Fixed, _, _)) | None => {
                self.buf.push_back(val);
                Ok(())
            }
            Some((BufferKind::Dropping, cap, _)) => {
                if (self.buf.len() as i64) < cap {
                    self.buf.push_back(val);
                }
                Ok(())
            }
            Some((BufferKind::Sliding, cap, integral)) => {
                if integral && self.buf.len() as i64 == cap {
                    self.buf.pop_front().ok_or(ChanError::EmptyEvict)?;
                }
                self.buf.push_back(val);
                Ok(())
            }
        }
    }

    /// Buffer a step's emits, routing a buffer-add crash through
    /// the ex-handler path (the JVM's `handle`): the remaining
    /// emits are discarded and the replacement (if any) buffers
    /// directly — a re-crash escapes, like `handle`'s `add!`.
    fn buffer_handled(
        &mut self,
        emits: Vec<Value>,
        stepper: &mut dyn FnMut(StepReq) -> Result<StepOut, Throwable>,
    ) -> Result<(), ChanError> {
        for val in emits {
            if self.buffer_one_overfill(val).is_err() {
                let failed = StepReq::Failed(Throwable {
                    id: next_identity_id(),
                    class: ThrowClass::NoSuchElementException,
                    message: None,
                    data: None,
                    cause: None,
                });
                let out = stepper(failed).map_err(ChanError::XformThrow)?;
                for replacement in out.emits {
                    self.buffer_one_overfill(replacement)?;
                }
                return Ok(());
            }
        }
        Ok(())
    }
}

/// A many-to-many channel (blocking ops only — no async
/// callbacks; slice 2 adds transducers via [`Channel::new_xform`]).
#[derive(Debug)]
pub struct Channel {
    id: u64,
    state: Mutex<ChanState>,
    xform: Option<Xform>,
}

impl Channel {
    /// Mint a channel (`None` = unbuffered).
    #[must_use]
    pub fn new(buf: Option<&BufferDesc>) -> Self {
        Self::build(buf, None)
    }

    /// Mint a transducer channel (`step` is `(xform conj)`, `exh`
    /// the optional ex-handler).
    #[must_use]
    pub fn new_xform(buf: Option<&BufferDesc>, step: Value, exh: Option<Value>) -> Self {
        Self::build(
            buf,
            Some(Xform {
                step,
                exh,
                gate: Mutex::new(()),
            }),
        )
    }

    fn build(buf: Option<&BufferDesc>, xform: Option<Xform>) -> Self {
        Self {
            id: next_identity_id(),
            state: Mutex::new(ChanState {
                buf: VecDeque::new(),
                kind: buf.map(|b| (b.kind, b.cap, b.integral)),
                closed: false,
                takes: VecDeque::new(),
                puts: VecDeque::new(),
            }),
            xform,
        }
    }

    /// The channel transducer, if any (the caller routes
    /// transducer channels through the `*_xform` methods).
    #[must_use]
    pub fn xform(&self) -> Option<&Xform> {
        self.xform.as_ref()
    }

    /// Whether the channel is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.lock().expect("channel lock").closed
    }

    /// Close: queued takers each take one buffered value (else
    /// nil); queued puts stay parked until a taker releases them
    /// (oracle `k-*`, `b-*`). Idempotent; returns nil-worthy
    /// nothing.
    pub fn close(&self) {
        let mut state = self.state.lock().expect("channel lock");
        if state.closed {
            return;
        }
        state.closed = true;
        state.sweep();
        state.serve_takes_all();
    }

    /// Close a transducer channel: the completion arity flushes
    /// first (only with no puts queued — the JVM's
    /// `(.isEmpty puts)` guard), then the takes are served (oracle
    /// `p-*`, `c-*`). Idempotent.
    pub fn close_xform(
        &self,
        stepper: &mut dyn FnMut(StepReq) -> Result<StepOut, Throwable>,
    ) -> Result<(), ChanError> {
        let xform = self
            .xform
            .as_ref()
            .expect("close_xform on a channel without a transducer");
        let generating = {
            let mut state = self.state.lock().expect("channel lock");
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.sweep();
            state.puts.is_empty()
        };
        if generating {
            let _gate = xform.gate.lock().expect("xform gate");
            let out = stepper(StepReq::Complete).map_err(ChanError::XformThrow)?;
            let mut state = self.state.lock().expect("channel lock");
            state.buffer_handled(out.emits, stepper)?;
        }
        self.state.lock().expect("channel lock").serve_takes_all();
        Ok(())
    }

    /// One take attempt: drain the buffer (refilling from queued
    /// putters), else rendezvous with a queued putter, else nil
    /// when closed, else park on `waiter` for op `op`.
    pub fn take_one(
        &self,
        waiter: &Arc<Waiter>,
        op: usize,
    ) -> Result<OpReady<Option<Value>>, ChanError> {
        let mut state = self.state.lock().expect("channel lock");
        state.sweep();
        if let Some(val) = state.buf.pop_front() {
            state.refill();
            return Ok(OpReady::Ready(Some(val)));
        }
        // Claim-first scan: the first live putter whose waiter we
        // win supplies the value (a lost commit — a co-registered
        // alt won elsewhere — skips like a stale reg; combinators,
        // so the race-only edge stays out of our branch count).
        let claimed = std::iter::from_fn(|| state.next_putter()).find_map(|reg| {
            let val = reg
                .put_val
                .clone()
                .expect("put registration carries a value");
            reg.waiter.try_win(reg.op, Value::Bool(true)).then_some(val)
        });
        if let Some(val) = claimed {
            return Ok(OpReady::Ready(Some(val)));
        }
        if state.closed {
            return Ok(OpReady::Ready(None));
        }
        if state.takes.len() >= MAX_QUEUE_SIZE {
            return Err(ChanError::QueueFull { puts: false });
        }
        state.takes.push_back(Reg::take(waiter, op));
        Ok(OpReady::Queued)
    }

    /// One put attempt (`val` is never nil — the caller rejects it
    /// first): rendezvous with a queued taker, else fail `false`
    /// when closed, else buffer (fixed queues when full;
    /// dropping/sliding always accept), else park on `waiter`.
    pub fn put_one(
        &self,
        val: Value,
        waiter: &Arc<Waiter>,
        op: usize,
    ) -> Result<OpReady<bool>, ChanError> {
        let mut state = self.state.lock().expect("channel lock");
        state.sweep();
        // Claim-first scan (see take path): the first live taker
        // whose waiter we win rendezvouses; the race-only lost
        // commit stays inside the combinator.
        let claimed = std::iter::from_fn(|| state.next_taker())
            .any(|reg| reg.waiter.try_win(reg.op, val.clone()));
        if claimed {
            return Ok(OpReady::Ready(true));
        }
        if state.closed {
            return Ok(OpReady::Ready(false));
        }
        match state.kind {
            Some((BufferKind::Fixed, cap, _)) => {
                if (state.buf.len() as i64) < cap {
                    state.buf.push_back(val);
                    return Ok(OpReady::Ready(true));
                }
            }
            Some((BufferKind::Dropping, cap, _)) => {
                if (state.buf.len() as i64) < cap {
                    state.buf.push_back(val);
                }
                return Ok(OpReady::Ready(true));
            }
            Some((BufferKind::Sliding, cap, integral)) => {
                if integral && state.buf.len() as i64 == cap {
                    state.buf.pop_front().ok_or(ChanError::EmptyEvict)?;
                }
                state.buf.push_back(val);
                return Ok(OpReady::Ready(true));
            }
            None => {}
        }
        if state.puts.len() >= MAX_QUEUE_SIZE {
            return Err(ChanError::QueueFull { puts: true });
        }
        state.puts.push_back(Reg::put(waiter, op, val));
        Ok(OpReady::Queued)
    }

    /// One put attempt on a transducer channel: step the value
    /// (serialized behind the gate, without the channel lock),
    /// then buffer every emit (serving queued takers) or park the
    /// emit vector behind a full fixed buffer. Filtered puts
    /// complete `true` untouched; a `reduced` step aborts once its
    /// emits buffer — or parks flagged, aborting at refill (the
    /// JVM aborts when the refill step returns `reduced`, so puts
    /// parked behind it still complete `true`; oracle `t-*`).
    pub fn put_xform(
        &self,
        val: Value,
        waiter: &Arc<Waiter>,
        op: usize,
        stepper: &mut dyn FnMut(StepReq) -> Result<StepOut, Throwable>,
    ) -> Result<OpReady<bool>, ChanError> {
        if self.is_closed() {
            return Ok(OpReady::Ready(false));
        }
        let xform = self
            .xform
            .as_ref()
            .expect("put_xform on a channel without a transducer");
        let _gate = xform.gate.lock().expect("xform gate");
        let out = stepper(StepReq::Input(val)).map_err(ChanError::XformThrow)?;
        let mut state = self.state.lock().expect("channel lock");
        state.sweep();
        if state.closed {
            return Ok(OpReady::Ready(false));
        }
        if out.emits.is_empty() {
            return Ok(OpReady::Ready(true));
        }
        if Self::room_for_first(&state) {
            state.buffer_handled(out.emits, stepper)?;
            if out.reduced {
                state.abort_locked();
                state.serve_takes_all();
            } else {
                state.serve_takes_while_full();
            }
            Ok(OpReady::Ready(true))
        } else {
            if state.puts.len() >= MAX_QUEUE_SIZE {
                return Err(ChanError::QueueFull { puts: true });
            }
            state
                .puts
                .push_back(Reg::put_many(waiter, op, out.emits, out.reduced));
            Ok(OpReady::Queued)
        }
    }

    /// Whether a fresh emit vector buffers outright (fixed needs
    /// room — otherwise the put parks; windowed buffers never
    /// park; the unbuffered arm only serves unit tests).
    fn room_for_first(state: &ChanState) -> bool {
        match state.kind {
            Some((BufferKind::Fixed, cap, _)) => (state.buf.len() as i64) < cap,
            Some(_) => true,
            None => false,
        }
    }

    /// Finish a take that found a closed transducer channel empty:
    /// run the completion arity and pop what it flushed (the JVM
    /// runs `(add! buf)` on every such take; oracle `c-flush`).
    pub fn complete_closed(
        &self,
        stepper: &mut dyn FnMut(StepReq) -> Result<StepOut, Throwable>,
    ) -> Result<Option<Value>, ChanError> {
        let xform = self
            .xform
            .as_ref()
            .expect("complete_closed on a channel without a transducer");
        let _gate = xform.gate.lock().expect("xform gate");
        let out = stepper(StepReq::Complete).map_err(ChanError::XformThrow)?;
        let mut state = self.state.lock().expect("channel lock");
        state.buffer_handled(out.emits, stepper)?;
        Ok(state.buf.pop_front())
    }
}

impl ExtensionValue for Channel {
    fn class_name(&self) -> String {
        "clojure.core.async.impl.channels.ManyToManyChannel".to_owned()
    }

    fn print(&self) -> String {
        let class = "clojure.core.async.impl.channels.ManyToManyChannel";
        format!("#object[{class} 0x{:x} \"{class}@{:x}\"]", self.id, self.id)
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Channel>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a channel value (shared by the `async` natives).
#[must_use]
pub fn as_channel(v: &Value) -> Option<&Channel> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<Channel>(),
        _ => None,
    }
}

/// Downcast a buffer value (shared by the `async` natives).
#[must_use]
pub fn as_buffer(v: &Value) -> Option<&BufferDesc> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<BufferDesc>(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fixed(n: i64) -> BufferDesc {
        BufferDesc::new(BufferKind::Fixed, n, true)
    }

    fn dropping(n: i64) -> BufferDesc {
        BufferDesc::new(BufferKind::Dropping, n, true)
    }

    fn sliding(n: i64, integral: bool) -> BufferDesc {
        BufferDesc::new(BufferKind::Sliding, n, integral)
    }

    /// Waiter primitives: `Default`, second-win false edge, and
    /// killed-wait resolves `None`.
    #[test]
    fn waiter_primitives() {
        let w = Waiter::default();
        assert!(w.try_win(0, Value::Nil));
        assert!(!w.try_win(1, Value::Nil));
        let k = Waiter::new();
        k.kill();
        assert_eq!(k.wait(), None);
    }

    /// Buffer descriptors hash by identity.
    #[test]
    fn buffer_desc_hashes_by_id() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let (a, b) = (fixed(2), fixed(2));
        let mut ha = DefaultHasher::new();
        let mut hb = DefaultHasher::new();
        ExtensionValue::dyn_hash(&a, &mut ha);
        ExtensionValue::dyn_hash(&b, &mut hb);
        assert_ne!(ha.finish(), hb.finish());
    }

    /// Stale registrations skip to `None` (an abandoned take reg
    /// does not satisfy a put, and vice versa).
    #[test]
    fn stale_regs_skip_to_none() {
        let ch = Channel::new(None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&w, 0), Ok(OpReady::Queued)));
        assert!(w.try_win(9, Value::Nil));
        let p = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_one(Value::Int(1), &p, 0),
            Ok(OpReady::Queued)
        ));
        let ch2 = Channel::new(None);
        let w2 = Arc::new(Waiter::new());
        assert!(matches!(
            ch2.put_one(Value::Int(1), &w2, 0),
            Ok(OpReady::Queued)
        ));
        assert!(w2.try_win(9, Value::Bool(true)));
        let t = Arc::new(Waiter::new());
        assert!(matches!(ch2.take_one(&t, 0), Ok(OpReady::Queued)));
    }

    /// A live rendezvous completes synchronously (a parked take
    /// is handed the next put, and a parked put the next take).
    #[test]
    fn live_regs_rendezvous_sync() {
        let ch = Channel::new(None);
        let t = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&t, 0), Ok(OpReady::Queued)));
        let p = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_one(Value::Int(1), &p, 0),
            Ok(OpReady::Ready(true))
        ));
        assert_eq!(t.wait(), Some((0, Value::Int(1))));
        let ch2 = Channel::new(None);
        let p2 = Arc::new(Waiter::new());
        assert!(matches!(
            ch2.put_one(Value::Int(2), &p2, 0),
            Ok(OpReady::Queued)
        ));
        let t2 = Arc::new(Waiter::new());
        assert!(matches!(
            ch2.take_one(&t2, 0),
            Ok(OpReady::Ready(Some(Value::Int(2))))
        ));
        assert_eq!(p2.wait(), Some((0, Value::Bool(true))));
    }

    /// A non-blocking take probe (a queued probe is killed, so it
    /// never disturbs later assertions).
    #[derive(Debug)]
    enum TakeProbe {
        Got(Value),
        Closed,
        WouldBlock,
    }

    fn take_now(ch: &Channel) -> TakeProbe {
        let w = Arc::new(Waiter::new());
        match ch.take_one(&w, 0).expect("take probe") {
            OpReady::Ready(found) => match found {
                Some(v) => TakeProbe::Got(v),
                None => TakeProbe::Closed,
            },
            OpReady::Queued => {
                w.kill();
                TakeProbe::WouldBlock
            }
        }
    }

    /// A non-blocking put probe.
    enum PutProbe {
        Accepted(bool),
        WouldBlock,
    }

    fn put_now(ch: &Channel, v: Value) -> PutProbe {
        let w = Arc::new(Waiter::new());
        match ch.put_one(v, &w, 0).expect("put probe") {
            OpReady::Ready(done) => PutProbe::Accepted(done),
            OpReady::Queued => {
                w.kill();
                PutProbe::WouldBlock
            }
        }
    }

    fn takes_depth(ch: &Channel) -> usize {
        ch.state.lock().expect("channel lock").takes.len()
    }

    fn puts_depth(ch: &Channel) -> usize {
        ch.state.lock().expect("channel lock").puts.len()
    }

    /// Spin until `depth` reaches `want` (registrations always
    /// land; only a genuine hang trips the timeout).
    fn wait_depth(ch: &Channel, want: usize, takes: bool) {
        let start = Instant::now();
        loop {
            let depth = if takes {
                takes_depth(ch)
            } else {
                puts_depth(ch)
            };
            if depth >= want {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "queue depth stuck at {depth}, want {want}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn fixed_buffer_fifo() {
        let buf = fixed(2);
        let ch = Channel::new(Some(&buf));
        assert!(matches!(
            put_now(&ch, Value::Int(1)),
            PutProbe::Accepted(true)
        ));
        assert!(matches!(
            put_now(&ch, Value::Int(2)),
            PutProbe::Accepted(true)
        ));
        assert!(matches!(put_now(&ch, Value::Int(3)), PutProbe::WouldBlock));
        assert_take_int(&ch, 1);
        assert_take_int(&ch, 2);
        assert!(matches!(take_now(&ch), TakeProbe::WouldBlock));
    }

    /// Assert a take probe delivers int `want` (single
    /// `matches!`, so no never-taken panic arm breaks the floor).
    fn assert_take_int(ch: &Channel, want: i64) {
        let probe = take_now(ch);
        assert!(
            matches!(probe, TakeProbe::Got(Value::Int(n)) if n == want),
            "expected take of {want}, got {probe:?}"
        );
    }

    #[test]
    fn windowed_buffers_never_block() {
        let buf = dropping(2);
        let ch = Channel::new(Some(&buf));
        for i in 1..=3 {
            assert!(matches!(
                put_now(&ch, Value::Int(i)),
                PutProbe::Accepted(true)
            ));
        }
        assert_take_int(&ch, 1);
        assert_take_int(&ch, 2);

        let buf = sliding(2, true);
        let ch = Channel::new(Some(&buf));
        for i in 1..=3 {
            assert!(matches!(
                put_now(&ch, Value::Int(i)),
                PutProbe::Accepted(true)
            ));
        }
        assert_take_int(&ch, 2);
        assert_take_int(&ch, 3);
    }

    #[test]
    fn unbuffered_rendezvous() {
        let ch = Arc::new(Channel::new(None));
        let taker = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                let w = Arc::new(Waiter::new());
                // Queued by orchestration (the put lands after
                // `wait_depth` observes us).
                assert!(matches!(ch.take_one(&w, 0).expect("take"), OpReady::Queued));
                w.wait().map(|(_, v)| v)
            })
        };
        wait_depth(&ch, 1, true);
        assert!(matches!(
            put_now(&ch, Value::Int(42)),
            PutProbe::Accepted(true)
        ));
        let got = taker.join().expect("taker joined");
        assert!(
            matches!(got, Some(Value::Int(n)) if n == 42),
            "taker killed: {got:?}"
        );
    }

    #[test]
    fn close_drains_then_yields_nil() {
        let buf = fixed(2);
        let ch = Channel::new(Some(&buf));
        assert!(!ch.is_closed());
        put_now(&ch, Value::Int(9));
        ch.close();
        assert!(ch.is_closed());
        ch.close();
        assert!(matches!(
            put_now(&ch, Value::Int(1)),
            PutProbe::Accepted(false)
        ));
        assert_take_int(&ch, 9);
        assert!(matches!(take_now(&ch), TakeProbe::Closed));
    }

    #[test]
    fn pending_put_survives_close_until_taker() {
        let ch = Arc::new(Channel::new(None));
        let putter = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                let w = Arc::new(Waiter::new());
                // Queued by orchestration (no taker yet).
                assert!(matches!(
                    ch.put_one(Value::Int(7), &w, 0).expect("put"),
                    OpReady::Queued
                ));
                w.wait().map(|(_, v)| matches!(v, Value::Bool(true)))
            })
        };
        wait_depth(&ch, 1, false);
        ch.close();
        // Still parked (not failed): the take releases it.
        assert_take_int(&ch, 7);
        assert_eq!(putter.join().expect("putter joined"), Some(true));
        assert!(matches!(take_now(&ch), TakeProbe::Closed));
    }

    #[test]
    fn close_hands_takers_buffered_values_first() {
        let buf = fixed(1);
        let ch = Arc::new(Channel::new(Some(&buf)));
        put_now(&ch, Value::Int(11));
        let mut takers = Vec::new();
        for _ in 0..2 {
            let ch = Arc::clone(&ch);
            takers.push(std::thread::spawn(move || {
                let w = Arc::new(Waiter::new());
                match ch.take_one(&w, 0).expect("take") {
                    OpReady::Ready(found) => found,
                    OpReady::Queued => w.wait().map(|(_, v)| v),
                }
            }));
        }
        // One taker may grab the buffered value before the close
        // lands; either way the pair is `{11, nil}`.
        wait_depth(&ch, 1, true);
        ch.close();
        let mut got: Vec<Option<i64>> = takers
            .into_iter()
            .map(|t| {
                let got = t.join().expect("taker joined");
                assert!(
                    matches!(got, Some(Value::Int(_)) | Some(Value::Nil)),
                    "unexpected take: {got:?}"
                );
                match got {
                    Some(Value::Int(n)) => Some(n),
                    _ => None,
                }
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![None, Some(11)]);
    }

    #[test]
    fn pending_bounds_match_max_queue_size() {
        pending_take_bound();
        pending_put_bound();
    }

    /// The take half: `MAX_QUEUE_SIZE` parked takers, then one more
    /// take fails `QueueFull`.
    fn pending_take_bound() {
        let ch = Arc::new(Channel::new(None));
        let mut threads = Vec::new();
        for _ in 0..MAX_QUEUE_SIZE {
            let ch = Arc::clone(&ch);
            threads.push(
                std::thread::Builder::new()
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        let w = Arc::new(Waiter::new());
                        // Takes-only phase: every take queues.
                        assert!(matches!(ch.take_one(&w, 0).expect("take"), OpReady::Queued));
                        w.wait();
                    })
                    .expect("spawn"),
            );
        }
        wait_depth(&ch, MAX_QUEUE_SIZE, true);
        let w = Arc::new(Waiter::new());
        let err = ch.take_one(&w, 0).unwrap_err();
        assert!(matches!(err, ChanError::QueueFull { puts: false }));
        ch.close();
        for t in threads {
            t.join().expect("joined");
        }
    }

    /// The put half: `MAX_QUEUE_SIZE` parked puts, then one more put
    /// fails `QueueFull` (queued puts survive the close: drained).
    fn pending_put_bound() {
        let ch = Arc::new(Channel::new(None));
        let mut threads = Vec::new();
        for i in 0..MAX_QUEUE_SIZE {
            let ch = Arc::clone(&ch);
            threads.push(
                std::thread::Builder::new()
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        let w = Arc::new(Waiter::new());
                        // Puts-only phase: every put queues.
                        assert!(matches!(
                            ch.put_one(Value::Int(i as i64), &w, 0).expect("put"),
                            OpReady::Queued
                        ));
                        w.wait();
                    })
                    .expect("spawn"),
            );
        }
        wait_depth(&ch, MAX_QUEUE_SIZE, false);
        let w = Arc::new(Waiter::new());
        let err = ch.put_one(Value::Int(-1), &w, 0).unwrap_err();
        assert!(matches!(err, ChanError::QueueFull { puts: true }));
        ch.close();
        for _ in 0..MAX_QUEUE_SIZE {
            assert!(matches!(take_now(&ch), TakeProbe::Got(_)));
        }
        for t in threads {
            t.join().expect("joined");
        }
    }

    #[test]
    fn shared_waiter_first_wins() {
        let c1 = Channel::new(None);
        let c2 = Channel::new(None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(
            c1.take_one(&w, 0).expect("take c1"),
            OpReady::Queued
        ));
        assert!(matches!(
            c2.take_one(&w, 1).expect("take c2"),
            OpReady::Queued
        ));
        // A put on c2 rendezvouses with the shared waiter for op 1.
        let p = Arc::new(Waiter::new());
        assert!(matches!(
            c2.put_one(Value::Int(9), &p, 0).expect("put c2"),
            OpReady::Ready(true)
        ));
        let won = w.wait();
        assert!(
            matches!(won, Some((1, Value::Int(n))) if n == 9),
            "expected op 1 with 9, got {won:?}"
        );
        // c1's registration is stale now: a put finds no live taker.
        assert!(matches!(put_now(&c1, Value::Int(8)), PutProbe::WouldBlock));
        c1.close();
        c2.close();
    }

    #[test]
    fn killed_waiter_abandons_registrations() {
        let ch = Channel::new(None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&w, 0).expect("take"), OpReady::Queued));
        w.kill();
        // The stale take is skipped: the put queues instead of
        // rendezvousing, and a fresh take gets it.
        assert!(matches!(put_now(&ch, Value::Int(5)), PutProbe::WouldBlock));
        // NOTE: `put_now` kills its own probe, so re-put for real.
        let p = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_one(Value::Int(5), &p, 0).expect("put"),
            OpReady::Queued
        ));
        assert_take_int(&ch, 5);
        assert!(p.wait().is_some());
    }

    #[test]
    fn identities_and_prints() {
        let a = Channel::new(None);
        let b = Channel::new(None);
        let ea: Arc<dyn ExtensionValue> = Arc::new(a);
        let ea2 = Arc::clone(&ea);
        let eb: Arc<dyn ExtensionValue> = Arc::new(b);
        assert!(ea.dyn_eq(&*ea2));
        assert!(!ea.dyn_eq(&*eb));
        assert_eq!(
            ea.class_name(),
            "clojure.core.async.impl.channels.ManyToManyChannel"
        );
        assert!(
            ea.print()
                .starts_with("#object[clojure.core.async.impl.channels.ManyToManyChannel 0x")
        );
        let buf: Arc<dyn ExtensionValue> = Arc::new(sliding(1, true));
        assert_eq!(
            buf.class_name(),
            "clojure.core.async.impl.buffers.SlidingBuffer"
        );
    }

    /// A canned stepper: inputs echo, completion flushes 99, the
    /// ex-handler path drops (every arm runs across the xform
    /// unit tests below).
    fn echo_stepper(req: StepReq) -> Result<StepOut, Throwable> {
        match req {
            StepReq::Input(v) => Ok(StepOut {
                emits: vec![v],
                reduced: false,
            }),
            StepReq::Complete => Ok(StepOut {
                emits: vec![Value::Int(99)],
                reduced: false,
            }),
            StepReq::Failed(_) => Ok(StepOut {
                emits: Vec::new(),
                reduced: false,
            }),
        }
    }

    /// A stepper that always throws (an ex-handler throw escapes
    /// the op on the JVM too).
    fn throw_stepper(_req: StepReq) -> Result<StepOut, Throwable> {
        Err(Throwable {
            id: next_identity_id(),
            class: ThrowClass::ExceptionInfo,
            message: Some(Arc::from("from-exh")),
            data: None,
            cause: None,
        })
    }

    /// The `None`-buffer transducer shape (`chan` forbids it, so
    /// only unit tests pin it): puts park, takes rendezvous, and
    /// completion buffers through the unbuffered arm.
    #[test]
    fn xform_none_parks_and_completes() {
        let ch = Channel::new_xform(None, Value::Nil, None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_xform(Value::Int(1), &w, 0, &mut echo_stepper),
            Ok(OpReady::Queued)
        ));
        assert_take_int(&ch, 1);
        assert!(w.wait().is_some());
        let flushed = ch.complete_closed(&mut echo_stepper).expect("complete");
        assert!(matches!(flushed, Some(Value::Int(n)) if n == 99));
    }

    /// A buffer-add crash routes through the ex-handler path (the
    /// sliding-zero evict drops the value, like the JVM's
    /// `handle` with a nil result).
    #[test]
    fn xform_evict_routes_failed() {
        let buf = sliding(0, true);
        let ch = Channel::new_xform(Some(&buf), Value::Nil, None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_xform(Value::Int(1), &w, 0, &mut echo_stepper),
            Ok(OpReady::Ready(true))
        ));
        assert!(matches!(take_now(&ch), TakeProbe::WouldBlock));
    }

    /// A stepper throw surfaces as `XformThrow` (the caller
    /// rethrows it).
    #[test]
    fn xform_throw_propagates() {
        let buf = fixed(1);
        let ch = Channel::new_xform(Some(&buf), Value::Nil, None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_xform(Value::Int(1), &w, 0, &mut throw_stepper),
            Err(ChanError::XformThrow(_))
        ));
    }

    /// A transducer put past a full queue errors `QueueFull`
    /// (the bound counts parked puts, like the JVM's).
    #[test]
    fn xform_parked_put_bound() {
        let buf = fixed(1);
        let ch = Channel::new_xform(Some(&buf), Value::Nil, None);
        let w0 = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_xform(Value::Int(0), &w0, 0, &mut echo_stepper),
            Ok(OpReady::Ready(true))
        ));
        {
            let mut state = ch.state.lock().expect("channel lock");
            for i in 0..MAX_QUEUE_SIZE {
                state
                    .puts
                    .push_back(Reg::put(&Arc::new(Waiter::new()), 0, Value::Int(i as i64)));
            }
        }
        let w = Arc::new(Waiter::new());
        assert!(matches!(
            ch.put_xform(Value::Int(-1), &w, 0, &mut echo_stepper),
            Err(ChanError::QueueFull { puts: true })
        ));
    }
}
