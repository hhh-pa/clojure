// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slice 1: `core.async` channels (blocking side only).
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
//! Lock order is channel-then-waiter (completions run under the
//! channel lock); waits hold only the waiter lock — no path takes
//! them in the other order, and every op holds at most one channel
//! lock at a time (like `do-alts`' one-mutex-at-a-time scan).

use std::collections::VecDeque;
use std::hash::Hasher;
use std::sync::{Arc, Condvar, Mutex};

use crate::Value;
use crate::extension::ExtensionValue;
use crate::value::next_identity_id;

/// `impl/MAX-QUEUE-SIZE`: at most 1024 pending takes (resp. puts)
/// per channel; the 1025th errors (oracle `e-take-bound`,
/// `e-put-bound`).
pub const MAX_QUEUE_SIZE: usize = 1024;

/// A buffer kind (`impl/buffers`; slice 1 has no transducers, so no
/// promise-buffer).
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

    /// The buffer kind.
    #[must_use]
    pub fn kind(&self) -> BufferKind {
        self.kind
    }

    /// The ceiling capacity.
    #[must_use]
    pub fn cap(&self) -> i64 {
        self.cap
    }

    /// Whether the requested size was integral.
    #[must_use]
    pub fn integral(&self) -> bool {
        self.integral
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
/// (`put_val: Some`) parked on `waiter` for op `op`.
#[derive(Clone, Debug)]
struct Reg {
    waiter: Arc<Waiter>,
    op: usize,
    put_val: Option<Value>,
}

impl Reg {
    fn take(waiter: &Arc<Waiter>, op: usize) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            op,
            put_val: None,
        }
    }

    fn put(waiter: &Arc<Waiter>, op: usize, val: Value) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            op,
            put_val: Some(val),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChanError {
    /// The 1025th pending take (`puts: false`) or put (`puts:
    /// true`) — `MAX-QUEUE-SIZE`.
    QueueFull { puts: bool },
    /// A sliding buffer of size 0 evicting from empty (the JVM
    /// crashes the same way — `LinkedList.removeLast`; its default
    /// ex-handler then swallows the trace and drops the value,
    /// which the port surfaces as this error instead).
    EmptyEvict,
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

    /// Pop the next live take registration.
    fn next_taker(&mut self) -> Option<Reg> {
        while let Some(reg) = self.takes.pop_front() {
            if reg.waiter.is_pending() {
                return Some(reg);
            }
        }
        None
    }

    /// Pop the next live put registration.
    fn next_putter(&mut self) -> Option<Reg> {
        while let Some(reg) = self.puts.pop_front() {
            if reg.waiter.is_pending() {
                return Some(reg);
            }
        }
        None
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
    /// completing them (the take-path refill).
    fn refill(&mut self) {
        while self.refill_room() {
            let Some(reg) = self.next_putter() else {
                break;
            };
            let val = reg
                .put_val
                .clone()
                .expect("put registration carries a value");
            if reg.waiter.try_win(reg.op, Value::Bool(true)) {
                self.buf.push_back(val);
            }
        }
    }
}

/// A many-to-many channel (slice 1: blocking ops only — no
/// transducers, no async callbacks).
#[derive(Debug)]
pub struct Channel {
    id: u64,
    state: Mutex<ChanState>,
}

impl Channel {
    /// Mint a channel (`None` = unbuffered).
    #[must_use]
    pub fn new(buf: Option<&BufferDesc>) -> Self {
        Self {
            id: next_identity_id(),
            state: Mutex::new(ChanState {
                buf: VecDeque::new(),
                kind: buf.map(|b| (b.kind, b.cap, b.integral)),
                closed: false,
                takes: VecDeque::new(),
                puts: VecDeque::new(),
            }),
        }
    }

    /// The print/identity id.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
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
        let takes: Vec<Reg> = state.takes.drain(..).collect();
        for reg in takes {
            // Peek-then-pop under the channel lock: a lost commit
            // (a co-registered alt won elsewhere) leaves the
            // buffered value for the next taker.
            let val = state.buf.front().cloned().unwrap_or(Value::Nil);
            if reg.waiter.try_win(reg.op, val) {
                state.buf.pop_front();
            }
        }
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
        loop {
            let Some(reg) = state.next_putter() else {
                break;
            };
            let val = reg
                .put_val
                .clone()
                .expect("put registration carries a value");
            if reg.waiter.try_win(reg.op, Value::Bool(true)) {
                return Ok(OpReady::Ready(Some(val)));
            }
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
        loop {
            let Some(reg) = state.next_taker() else {
                break;
            };
            if reg.waiter.try_win(reg.op, val.clone()) {
                return Ok(OpReady::Ready(true));
            }
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

    /// A non-blocking take probe (a queued probe is killed, so it
    /// never disturbs later assertions).
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

    fn assert_int(v: &Value, want: i64) {
        assert!(
            matches!(v, Value::Int(n) if *n == want),
            "expected int {want}, got {v:?}"
        );
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
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 1),
            other => panic!("expected 1, got {}", probe_name(&other)),
        }
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 2),
            other => panic!("expected 2, got {}", probe_name(&other)),
        }
        assert!(matches!(take_now(&ch), TakeProbe::WouldBlock));
    }

    fn probe_name(p: &TakeProbe) -> &'static str {
        match p {
            TakeProbe::Got(_) => "value",
            TakeProbe::Closed => "closed",
            TakeProbe::WouldBlock => "would-block",
        }
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
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 1),
            other => panic!("expected 1, got {}", probe_name(&other)),
        }
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 2),
            other => panic!("expected 2, got {}", probe_name(&other)),
        }

        let buf = sliding(2, true);
        let ch = Channel::new(Some(&buf));
        for i in 1..=3 {
            assert!(matches!(
                put_now(&ch, Value::Int(i)),
                PutProbe::Accepted(true)
            ));
        }
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 2),
            other => panic!("expected 2, got {}", probe_name(&other)),
        }
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 3),
            other => panic!("expected 3, got {}", probe_name(&other)),
        }
    }

    #[test]
    fn unbuffered_rendezvous() {
        let ch = Arc::new(Channel::new(None));
        let taker = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                let w = Arc::new(Waiter::new());
                match ch.take_one(&w, 0).expect("take") {
                    OpReady::Ready(found) => found,
                    OpReady::Queued => w.wait().map(|(_, v)| v),
                }
            })
        };
        wait_depth(&ch, 1, true);
        assert!(matches!(
            put_now(&ch, Value::Int(42)),
            PutProbe::Accepted(true)
        ));
        match taker.join().expect("taker joined") {
            Some(v) => assert_int(&v, 42),
            None => panic!("taker killed"),
        }
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
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 9),
            other => panic!("expected 9, got {}", probe_name(&other)),
        }
        assert!(matches!(take_now(&ch), TakeProbe::Closed));
    }

    #[test]
    fn pending_put_survives_close_until_taker() {
        let ch = Arc::new(Channel::new(None));
        let putter = {
            let ch = Arc::clone(&ch);
            std::thread::spawn(move || {
                let w = Arc::new(Waiter::new());
                match ch.put_one(Value::Int(7), &w, 0).expect("put") {
                    OpReady::Ready(done) => Some(done),
                    OpReady::Queued => w.wait().map(|(_, v)| matches!(v, Value::Bool(true))),
                }
            })
        };
        wait_depth(&ch, 1, false);
        ch.close();
        // Still parked (not failed): the take releases it.
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 7),
            other => panic!("expected 7, got {}", probe_name(&other)),
        }
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
            .map(|t| match t.join().expect("taker joined") {
                Some(Value::Int(n)) => Some(n),
                Some(Value::Nil) => None,
                other => panic!("unexpected take: {other:?}"),
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
                        match ch.take_one(&w, 0).expect("take") {
                            OpReady::Ready(_) => {}
                            OpReady::Queued => {
                                w.wait();
                            }
                        }
                    })
                    .expect("spawn"),
            );
        }
        wait_depth(&ch, MAX_QUEUE_SIZE, true);
        let w = Arc::new(Waiter::new());
        let err = ch.take_one(&w, 0).unwrap_err();
        assert_eq!(err, ChanError::QueueFull { puts: false });
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
                        match ch.put_one(Value::Int(i as i64), &w, 0).expect("put") {
                            OpReady::Ready(_) => {}
                            OpReady::Queued => {
                                w.wait();
                            }
                        }
                    })
                    .expect("spawn"),
            );
        }
        wait_depth(&ch, MAX_QUEUE_SIZE, false);
        let w = Arc::new(Waiter::new());
        let err = ch.put_one(Value::Int(-1), &w, 0).unwrap_err();
        assert_eq!(err, ChanError::QueueFull { puts: true });
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
        match w.wait() {
            Some((1, v)) => assert_int(&v, 9),
            other => panic!("expected op 1 with 9, got {other:?}"),
        }
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
        match take_now(&ch) {
            TakeProbe::Got(v) => assert_int(&v, 5),
            other => panic!("expected 5, got {}", probe_name(&other)),
        }
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
}
