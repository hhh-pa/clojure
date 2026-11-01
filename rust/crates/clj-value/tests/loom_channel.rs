// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Channel mailbox core (#188) — loom model over a faithful mirror of
//! `src/channel.rs` (`take_one` / `put_one` / `close`,
//! `ChanState::{refill, serve_takes_all}`) plus the attempt-then-wait
//! drivers of `clj-eval/src/async.rs` (`native_put_block`,
//! `native_take_block`).
//!
//! Properties: rendezvous both directions with no lost wakeup on any
//! schedule, fixed-full park + take-refill wakeup + FIFO, dropping
//! overflow dropped but true, sliding evict-oldest, close wakes
//! parked takers (buffered-then-nil) while parked puts survive until
//! a taker releases them, put-after-close false, take-after-close nil.
//!
//! Mirror notes (faithful to the synchronization structure):
//!   - values are u64 ids; the waiter payload is a Value stand-in
//!     (`Num` / `Yes` / `Nil`) — payloads never steer the mailbox.
//!   - plain ops only: each waiter serves one op (the `op` index is
//!     dropped), so no waiter ever goes stale while parked and every
//!     claim-first scan wins on the head reg. The sweep/skip loops
//!     stay for structural fidelity (they still take the waiter lock
//!     under the channel lock, pinning the lock order).
//!   - `MAX_QUEUE_SIZE` mirrors the real 1024 (unreachable in these
//!     models — a qfull would panic the model loudly; the TLA spec
//!     `channel_mailbox` pins the check placement with MAX=2).
//!   - OUT (like the TLA spec): transducers, alts!! shared-waiter
//!     first-wins, go-block settle paths.
//!
//! Run with the rest of the suite (`cargo test -p clj-value
//! --offline`); loom explores every schedule of each model.

use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;
use std::collections::VecDeque;

/// Mirror of `MAX-QUEUE-SIZE` (oracle `e-take-bound`, `e-put-bound`).
const MAX_QUEUE_SIZE: usize = 1024;

/// Mirror of `BufferKind` (no promise-buffer — the real mailbox has
/// none either).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferKind {
    Fixed,
    Dropping,
    Sliding,
}

/// Mirror of the waiter payload (`Value` stand-in): a taken number,
/// a completed put (`true`), or a closed-empty take (`nil`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum MVal {
    Num(u64),
    Yes,
    Nil,
}

/// Mirror of `WaitStatus` (single-op waiters: no `op` index).
#[derive(Clone, Debug)]
enum WaitStatus {
    Pending,
    Won(MVal),
    Dead,
}

/// Mirror of `Waiter`: one thread parks per waiter.
#[derive(Debug)]
struct Waiter {
    inner: Mutex<WaitStatus>,
    cv: Condvar,
}

impl Waiter {
    fn new() -> Self {
        Self {
            inner: Mutex::new(WaitStatus::Pending),
            cv: Condvar::new(),
        }
    }

    /// Mirror of `Waiter::try_win` (first wins; notify under the
    /// waiter lock, called with the channel lock held — the
    /// channel-then-waiter order).
    fn try_win(&self, val: MVal) -> bool {
        let mut status = self.inner.lock().unwrap();
        if matches!(*status, WaitStatus::Pending) {
            *status = WaitStatus::Won(val);
            self.cv.notify_one();
            true
        } else {
            false
        }
    }

    /// Mirror of `Waiter::kill` (abandon a registration — the
    /// `:default` path; the owner never waits after killing).
    fn kill(&self) {
        let mut status = self.inner.lock().unwrap();
        if matches!(*status, WaitStatus::Pending) {
            *status = WaitStatus::Dead;
        }
    }

    /// Mirror of `Waiter::is_pending` (the `cleanup` sweep probe).
    fn is_pending(&self) -> bool {
        matches!(*self.inner.lock().unwrap(), WaitStatus::Pending)
    }

    /// Mirror of `Waiter::wait` (the `wait_while(Pending)` loop —
    /// a missed wakeup would deadlock a schedule, failing the model).
    fn wait(&self) -> Option<MVal> {
        let mut status = self.inner.lock().unwrap();
        while matches!(*status, WaitStatus::Pending) {
            status = self.cv.wait(status).unwrap();
        }
        match &*status {
            WaitStatus::Won(val) => Some(val.clone()),
            WaitStatus::Dead => None,
            WaitStatus::Pending => unreachable!("wait loop exits only when settled"),
        }
    }
}

/// Mirror of `Reg` (plain put/take registrations; no `rest`/`reduced`
/// — transducer puts are OUT).
#[derive(Clone, Debug)]
struct Reg {
    waiter: Arc<Waiter>,
    put_val: Option<u64>,
}

impl Reg {
    fn take(waiter: &Arc<Waiter>) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            put_val: None,
        }
    }

    fn put(waiter: &Arc<Waiter>, val: u64) -> Self {
        Self {
            waiter: Arc::clone(waiter),
            put_val: Some(val),
        }
    }
}

/// Mirror of `OpReady`.
#[derive(Debug, PartialEq, Eq)]
enum OpReady<T> {
    Ready(T),
    Queued,
}

/// Mirror of the reachable `ChanError` arms (`XformThrow` is OUT).
#[derive(Clone, Debug, PartialEq, Eq)]
enum ChanError {
    QueueFull { puts: bool },
    EmptyEvict,
}

/// Mirror of `ChanState`: the buffer, its kind, the close latch, and
/// the queued regs behind the one channel mutex.
#[derive(Debug)]
struct ChanState {
    buf: VecDeque<u64>,
    kind: Option<(BufferKind, usize, bool)>,
    closed: bool,
    takes: VecDeque<Reg>,
    puts: VecDeque<Reg>,
}

impl ChanState {
    /// Mirror of `ChanState::sweep` (no-op on plain waiters, which
    /// never go stale — kept for lock-order fidelity).
    fn sweep(&mut self) {
        self.takes.retain(|r| r.waiter.is_pending());
        self.puts.retain(|r| r.waiter.is_pending());
    }

    /// Mirror of `ChanState::next_taker`.
    fn next_taker(&mut self) -> Option<Reg> {
        std::iter::from_fn(|| self.takes.pop_front()).find(|reg| reg.waiter.is_pending())
    }

    /// Mirror of `ChanState::next_putter`.
    fn next_putter(&mut self) -> Option<Reg> {
        std::iter::from_fn(|| self.puts.pop_front()).find(|reg| reg.waiter.is_pending())
    }

    /// Mirror of `ChanState::refill_room`.
    fn refill_room(&self) -> bool {
        match self.kind {
            Some((BufferKind::Fixed, cap, _)) | Some((BufferKind::Dropping, cap, _)) => {
                self.buf.len() < cap
            }
            Some((BufferKind::Sliding, _, _)) | None => false,
        }
    }

    /// Mirror of `ChanState::refill` (plain putters carry one value;
    /// the `rest`/`reduced` arms are OUT).
    fn refill(&mut self) {
        while self.refill_room() {
            let Some(reg) = self.next_putter() else {
                break;
            };
            let val = reg.put_val.expect("put registration carries a value");
            if reg.waiter.try_win(MVal::Yes) {
                self.buf.push_back(val);
            }
        }
    }

    /// Mirror of `ChanState::serve_takes_all` (peek-then-pop: a lost
    /// commit leaves the buffered value for the next taker —
    /// unreachable on plain waiters, kept for fidelity).
    fn serve_takes_all(&mut self) {
        let takes: Vec<Reg> = self.takes.drain(..).collect();
        for reg in takes {
            let val = self.buf.front().cloned().map_or(MVal::Nil, MVal::Num);
            if reg.waiter.try_win(val) {
                self.buf.pop_front();
            }
        }
    }
}

/// Mirror of `Channel` (blocking ops only, no transducer).
#[derive(Debug)]
struct Channel {
    state: Mutex<ChanState>,
}

impl Channel {
    /// Mirror of `Channel::new` (`None` = unbuffered).
    fn new(buf: Option<(BufferKind, usize, bool)>) -> Self {
        Self {
            state: Mutex::new(ChanState {
                buf: VecDeque::new(),
                kind: buf,
                closed: false,
                takes: VecDeque::new(),
                puts: VecDeque::new(),
            }),
        }
    }

    /// Mirror of `Channel::close`.
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        state.sweep();
        state.serve_takes_all();
    }

    /// Mirror of `Channel::take_one`.
    fn take_one(&self, waiter: &Arc<Waiter>) -> Result<OpReady<Option<u64>>, ChanError> {
        let mut state = self.state.lock().unwrap();
        state.sweep();
        if let Some(val) = state.buf.pop_front() {
            state.refill();
            return Ok(OpReady::Ready(Some(val)));
        }
        let claimed = std::iter::from_fn(|| state.next_putter()).find_map(|reg| {
            let val = reg.put_val.expect("put registration carries a value");
            reg.waiter.try_win(MVal::Yes).then_some(val)
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
        state.takes.push_back(Reg::take(waiter));
        Ok(OpReady::Queued)
    }

    /// Mirror of `Channel::put_one`.
    fn put_one(&self, val: u64, waiter: &Arc<Waiter>) -> Result<OpReady<bool>, ChanError> {
        let mut state = self.state.lock().unwrap();
        state.sweep();
        let claimed =
            std::iter::from_fn(|| state.next_taker()).any(|reg| reg.waiter.try_win(MVal::Num(val)));
        if claimed {
            return Ok(OpReady::Ready(true));
        }
        if state.closed {
            return Ok(OpReady::Ready(false));
        }
        match state.kind {
            Some((BufferKind::Fixed, cap, _)) => {
                if state.buf.len() < cap {
                    state.buf.push_back(val);
                    return Ok(OpReady::Ready(true));
                }
            }
            Some((BufferKind::Dropping, cap, _)) => {
                if state.buf.len() < cap {
                    state.buf.push_back(val);
                }
                return Ok(OpReady::Ready(true));
            }
            Some((BufferKind::Sliding, cap, integral)) => {
                if integral && state.buf.len() == cap {
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
        state.puts.push_back(Reg::put(waiter, val));
        Ok(OpReady::Queued)
    }
}

/// Mirror of `native_put_block`'s attempt-then-wait driver.
fn put_blocking(ch: &Channel, val: u64) -> bool {
    let waiter = Arc::new(Waiter::new());
    match ch.put_one(val, &waiter).expect("no qfull in these models") {
        OpReady::Ready(done) => done,
        OpReady::Queued => matches!(waiter.wait(), Some(MVal::Yes)),
    }
}

/// Mirror of `native_take_block`'s attempt-then-wait driver.
fn take_blocking(ch: &Channel) -> Option<u64> {
    let waiter = Arc::new(Waiter::new());
    match ch.take_one(&waiter).expect("no qfull in these models") {
        OpReady::Ready(found) => found,
        OpReady::Queued => match waiter.wait() {
            Some(MVal::Num(v)) => Some(v),
            Some(MVal::Nil) => None,
            _ => unreachable!("plain waiter never killed or put-completed"),
        },
    }
}

/// A parked take is released by the next put (no lost wakeup on any
/// schedule — a missed notify would deadlock the model).
#[test]
fn rendezvous_parked_take_released_by_put() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(None));
        let waiter = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&waiter), Ok(OpReady::Queued)));
        let putter = Arc::clone(&ch);
        let h = thread::spawn(move || put_blocking(&putter, 7));
        assert_eq!(waiter.wait(), Some(MVal::Num(7)));
        assert!(h.join().unwrap());
    });
}

/// A parked put is released by the next take (the mirror image).
#[test]
fn rendezvous_parked_put_released_by_take() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(None));
        let waiter = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(7, &waiter), Ok(OpReady::Queued)));
        let taker = Arc::clone(&ch);
        let h = thread::spawn(move || take_blocking(&taker));
        assert_eq!(waiter.wait(), Some(MVal::Yes));
        assert_eq!(h.join().unwrap(), Some(7));
    });
}

/// A racing taker and putter rendezvous whichever parks first (both
/// orders arise across schedules; at most one ever parks).
#[test]
fn racing_rendezvous_both_orders() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(None));
        let taker = Arc::clone(&ch);
        let taker = thread::spawn(move || take_blocking(&taker));
        let putter = Arc::clone(&ch);
        let putter = thread::spawn(move || put_blocking(&putter, 9));
        assert_eq!(taker.join().unwrap(), Some(9));
        assert!(putter.join().unwrap());
    });
}

/// A fixed-full put parks; the take refills the freed room and wakes
/// the putter (FIFO across the buffer and the puts queue).
#[test]
fn fixed_full_parks_and_take_refills() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(Some((BufferKind::Fixed, 1, true))));
        let fill = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &fill), Ok(OpReady::Ready(true))));
        let waiter = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(2, &waiter), Ok(OpReady::Queued)));
        let taker = Arc::clone(&ch);
        let h = thread::spawn(move || take_blocking(&taker));
        assert_eq!(h.join().unwrap(), Some(1));
        assert_eq!(waiter.wait(), Some(MVal::Yes));
        assert_eq!(take_blocking(&ch), Some(2));
    });
}

/// Two putters parked behind a fixed-1 buffer drain FIFO through the
/// refill chain (take, refill, take, refill).
#[test]
fn fixed_two_parked_putters_refill_fifo() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(Some((BufferKind::Fixed, 1, true))));
        let fill = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &fill), Ok(OpReady::Ready(true))));
        let wb = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(2, &wb), Ok(OpReady::Queued)));
        let wc = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(3, &wc), Ok(OpReady::Queued)));
        let taker = Arc::clone(&ch);
        let h = thread::spawn(move || (take_blocking(&taker), take_blocking(&taker)));
        assert_eq!(wb.wait(), Some(MVal::Yes));
        assert_eq!(wc.wait(), Some(MVal::Yes));
        assert_eq!(h.join().unwrap(), (Some(1), Some(2)));
        assert_eq!(take_blocking(&ch), Some(3));
    });
}

/// Dropping buffers never block: the overflow completes true but is
/// dropped (the next take parks past the kept value).
#[test]
fn dropping_never_blocks_and_drops_overflow() {
    loom::model(|| {
        let ch = Channel::new(Some((BufferKind::Dropping, 1, true)));
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &w), Ok(OpReady::Ready(true))));
        assert!(matches!(ch.put_one(2, &w), Ok(OpReady::Ready(true))));
        assert_eq!(take_blocking(&ch), Some(1));
        let parked = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&parked), Ok(OpReady::Queued)));
        ch.close();
        assert_eq!(parked.wait(), Some(MVal::Nil));
    });
}

/// Two racing dropping puts both complete true; exactly the first is
/// kept (one buffered value, either one, across schedules).
#[test]
fn dropping_racing_puts_both_true_one_kept() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(Some((BufferKind::Dropping, 1, true))));
        let a = Arc::clone(&ch);
        let ha = thread::spawn(move || put_blocking(&a, 10));
        let b = Arc::clone(&ch);
        let hb = thread::spawn(move || put_blocking(&b, 20));
        assert!(ha.join().unwrap());
        assert!(hb.join().unwrap());
        let got = take_blocking(&ch).unwrap();
        assert!(got == 10 || got == 20, "kept one put: {got}");
        let parked = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&parked), Ok(OpReady::Queued)));
        ch.close();
        assert_eq!(parked.wait(), Some(MVal::Nil));
    });
}

/// Sliding buffers never block: the overflow evicts the oldest (the
/// next take sees the newest).
#[test]
fn sliding_evicts_oldest_never_blocks() {
    loom::model(|| {
        let ch = Channel::new(Some((BufferKind::Sliding, 1, true)));
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &w), Ok(OpReady::Ready(true))));
        assert!(matches!(ch.put_one(2, &w), Ok(OpReady::Ready(true))));
        assert_eq!(take_blocking(&ch), Some(2));
        let parked = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&parked), Ok(OpReady::Queued)));
        ch.close();
        assert_eq!(parked.wait(), Some(MVal::Nil));
    });
}

/// Two racing sliding puts both complete true; exactly one (the
/// last) is kept across schedules.
#[test]
fn sliding_racing_puts_both_true_newest_kept() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(Some((BufferKind::Sliding, 1, true))));
        let a = Arc::clone(&ch);
        let ha = thread::spawn(move || put_blocking(&a, 10));
        let b = Arc::clone(&ch);
        let hb = thread::spawn(move || put_blocking(&b, 20));
        assert!(ha.join().unwrap());
        assert!(hb.join().unwrap());
        let got = take_blocking(&ch).unwrap();
        assert!(got == 10 || got == 20, "kept one put: {got}");
        let parked = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&parked), Ok(OpReady::Queued)));
        ch.close();
        assert_eq!(parked.wait(), Some(MVal::Nil));
    });
}

/// Sliding-zero evicting from empty errors (the JVM's
/// `LinkedList.removeLast` crash, surfaced as `EmptyEvict`); a
/// non-integral sliding size never evicts (the JVM `(= size n)`
/// quirk — the buffer grows past the nominal cap).
#[test]
fn sliding_zero_errors_and_nonintegral_never_evicts() {
    loom::model(|| {
        let ch = Channel::new(Some((BufferKind::Sliding, 0, true)));
        let w = Arc::new(Waiter::new());
        assert_eq!(ch.put_one(1, &w), Err(ChanError::EmptyEvict));

        let ch = Channel::new(Some((BufferKind::Sliding, 2, false)));
        for v in 1..=3 {
            assert!(matches!(ch.put_one(v, &w), Ok(OpReady::Ready(true))));
        }
        assert_eq!(take_blocking(&ch), Some(1));
        assert_eq!(take_blocking(&ch), Some(2));
        assert_eq!(take_blocking(&ch), Some(3));
    });
}

/// Close wakes every parked taker with nil past the buffer (a taker
/// racing the close either parks-then-nil or reads nil directly).
#[test]
fn close_wakes_parked_takers_with_nil() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(None));
        let first = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&first), Ok(OpReady::Queued)));
        let taker = Arc::clone(&ch);
        let h = thread::spawn(move || take_blocking(&taker));
        ch.close();
        assert_eq!(first.wait(), Some(MVal::Nil));
        assert_eq!(h.join().unwrap(), None);
    });
}

/// Close past a buffer: buffered values still drain first, then nil;
/// close is idempotent.
#[test]
fn close_drains_buffer_then_nil() {
    loom::model(|| {
        let ch = Channel::new(Some((BufferKind::Fixed, 2, true)));
        assert!(put_blocking(&ch, 9));
        ch.close();
        ch.close();
        assert_eq!(take_blocking(&ch), Some(9));
        assert_eq!(take_blocking(&ch), None);
    });
}

/// A put parked at close stays parked (not failed) until a taker
/// releases it — whether the taker wins before or after the latch.
#[test]
fn parked_puts_survive_close_until_taker() {
    loom::model(|| {
        let ch = Arc::new(Channel::new(None));
        let waiter = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(7, &waiter), Ok(OpReady::Queued)));
        let taker = Arc::clone(&ch);
        let h = thread::spawn(move || take_blocking(&taker));
        ch.close();
        assert_eq!(waiter.wait(), Some(MVal::Yes));
        assert_eq!(h.join().unwrap(), Some(7));
        assert_eq!(take_blocking(&ch), None);
    });
}

/// Post-close puts fail false and post-close takes read nil (the
/// closed-empty edges).
#[test]
fn put_after_close_false_take_after_close_nil() {
    loom::model(|| {
        let ch = Channel::new(None);
        ch.close();
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &w), Ok(OpReady::Ready(false))));
        assert!(matches!(ch.take_one(&w), Ok(OpReady::Ready(None))));
    });
}

/// Stale regs skip: a take whose waiter won elsewhere never
/// satisfies a put (and vice versa); a killed waiter resolves None.
#[test]
fn stale_regs_skip_and_killed_wait_resolves_none() {
    loom::model(|| {
        let ch = Channel::new(None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&w), Ok(OpReady::Queued)));
        assert!(w.try_win(MVal::Num(99)));
        let p = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &p), Ok(OpReady::Queued)));

        let ch = Channel::new(None);
        let w = Arc::new(Waiter::new());
        assert!(matches!(ch.put_one(1, &w), Ok(OpReady::Queued)));
        assert!(w.try_win(MVal::Yes));
        let t = Arc::new(Waiter::new());
        assert!(matches!(ch.take_one(&t), Ok(OpReady::Queued)));

        let k = Waiter::new();
        k.kill();
        assert_eq!(k.wait(), None);
    });
}
