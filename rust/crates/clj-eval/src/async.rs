// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slices 1–3: the `clojure.core.async` namespace —
//! channels plus blocking ops (`chan`, `buffer`/`dropping-buffer`/
//! `sliding-buffer`, `>!!`, `<!!`, `close!`, `closed?`, `alts!!`),
//! with slice 2's channel transducers (`chan` xform/ex-handler
//! arities, stepping on the put path, completion on close and
//! closed takes) and slice 3's `go`/`go-loop` blocks plus the
//! parking ops (`<!`/`>!`/`alts!` rewrite to the blocking ops
//! inside `go`; `alt!`/`alt!!` are the `do-alt` macros). The
//! parking fns stay stubs outside `go`, failing exactly like the
//! JVM's (`Assert failed: … used not in (go ...) block`).
//!
//! The namespace installs lazily on `require` (see
//! [`install_async_ns`], called from `load_one`), so boot is
//! untouched and unqualified use without a require fails with the
//! stock "no such namespace", like the JVM without the require.
//! Oracle: `docs/gap-123/g123-*.clj`; pins:
//! `docs/gap-123/probes.md`, `docs/gap-123/slice-2.md`,
//! `docs/gap-123/slice-3.md`.
//!
//! Documented deviations (D1–D7 in `slice-2.md`, D9–D12 in
//! `slice-3.md`): `closed?` is public here (the JVM keeps it in
//! `impl.protocols` — the slice requires it); a non-number
//! non-buffer `chan` argument fails eagerly with the `:full?`
//! protocol error (the JVM stores it and fails on first use); the
//! default ex-handler prints one line (the JVM prints the full
//! trace); puts step at arrival, so a filtered put behind a full
//! buffer completes at once and an aborted put still ran its
//! transducer (same values, only timing differs); a close racing
//! a slow step wins (the JVM holds its mutex across the step);
//! each `go` block runs on its own thread (no state machine, no
//! fixed dispatch pool); two go-internal arities report through
//! the blocking layer; parking ops hidden behind fn-creating
//! macros execute instead of asserting; `go-call` is a public
//! port-internal var.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use num_bigint::Sign;
use num_traits::ToPrimitive;
use rand::seq::SliceRandom;

use clj_value::{
    BufferDesc, BufferKind, ChanError, Channel, Keyword, MAX_QUEUE_SIZE, OpReady, StepOut, StepReq,
    Symbol, ThrowClass, Throwable, Value, Vector, Waiter, as_buffer, as_channel, jvm_class_of,
    map_entry, new_map, new_set, seq_vec,
};

use crate::core::{as_reduced, cce_err};
use crate::error::{NativeError, Signal, internal_throwable};
use crate::eval::Evaluator;
use crate::fx4_form::{gs, gs_auto, is_amp, is_as, iseq_err, kw, lst, sym_q, vecv, with_form_meta};
use crate::ns::{NativeFnPtr, check_arity, deref_core, str_of, throwable_to_native};
use crate::seqs::call_checked;
use crate::vars::check_min_arity;

/// The namespace this module installs.
pub(crate) const ASYNC_NS: &str = "clojure.core.async";

/// The `:default` `alts!!` result port.
fn default_port() -> Value {
    Value::Keyword(Keyword::new(None, "default"))
}

/// The missing-protocol error (`take!`/`ReadPort` & co. — oracle
/// `e-take-nonchan`; nil renders `nil`, not `null`).
fn proto_err(v: &Value, method: &str, proto: &str) -> NativeError {
    let class = match v {
        Value::Nil => "nil".to_owned(),
        _ => jvm_class_of(v),
    };
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!(
            "No implementation of method: :{method} of protocol: #'clojure.core.async.impl.protocols/{proto} found for class: {class}"
        ),
    )
}

/// The `assert` failure shape (`Assert failed: <msg>\n<form>`).
fn assert_failed(msg: &str, form: &str) -> NativeError {
    NativeError::new(
        ThrowClass::AssertionError,
        format!("Assert failed: {msg}\n{form}"),
    )
}

/// The empty `IndexOutOfBoundsException` (a short vector `alts!!`
/// op — JVM `(port 1)`).
fn ioobe() -> NativeError {
    NativeError::new(ThrowClass::IndexOutOfBoundsException, String::new())
}

/// Map a channel-queue failure to its JVM error text (oracle
/// `e-take-bound`, `e-put-bound`; the sliding-zero evict is a
/// `NoSuchElementException` with a nil message; a transducer
/// throw rethrows).
fn chan_err(e: ChanError) -> NativeError {
    match e {
        ChanError::QueueFull { puts } => {
            let what = if puts { "puts" } else { "takes" };
            let extra = if puts {
                " Consider using a windowed buffer."
            } else {
                ""
            };
            assert_failed(
                &format!(
                    "No more than {MAX_QUEUE_SIZE} pending {what} are allowed on a single channel.{extra}"
                ),
                &format!("(< (.size {what}) impl/MAX-QUEUE-SIZE)"),
            )
        }
        ChanError::EmptyEvict => {
            NativeError::new(ThrowClass::NoSuchElementException, String::new())
        }
        ChanError::XformThrow(t) => throwable_to_native(&t),
    }
}

/// A channel receiver for `method`/`proto` (nil included — the
/// protocol error, never an NPE).
fn as_chan<'a>(v: &'a Value, method: &str, proto: &str) -> Result<&'a Channel, NativeError> {
    as_channel(v).ok_or_else(|| proto_err(v, method, proto))
}

/// A buffer size as `(positive?, ceiling, integral)`: the JVM
/// compares `(>= size n)` / `(= size n)` exactly, so `1.5` behaves
/// as `2`, `NaN`/`+inf` are unbounded (`(i64::MAX, false)`), and
/// only fixed buffers assert positivity.
fn buf_size(v: &Value) -> Result<(bool, i64, bool), NativeError> {
    match v {
        Value::Int(n) => Ok((*n > 0, *n, true)),
        Value::Double(f) => Ok(double_cap(*f)),
        Value::BigInt(b) | Value::BigInteger(b) => Ok(bigint_cap(b)),
        Value::Ratio(r) => Ok(ratio_cap(r)),
        Value::BigDec(d) => {
            let (ceil, integral) = d.ceil_to_i64();
            Ok((d.sign() == std::cmp::Ordering::Greater, ceil, integral))
        }
        _ => Err(cce_err(v, "java.lang.Number")),
    }
}

/// A double buffer size (`NaN` is non-positive and unbounded;
/// infinities saturate).
fn double_cap(f: f64) -> (bool, i64, bool) {
    if f.is_nan() {
        return (false, i64::MAX, false);
    }
    if f.is_infinite() {
        return if f > 0.0 {
            (true, i64::MAX, false)
        } else {
            (false, i64::MIN, false)
        };
    }
    (f > 0.0, f.ceil() as i64, f.fract() == 0.0)
}

/// A bigint buffer size (saturates past `i64`).
fn bigint_cap(b: &num_bigint::BigInt) -> (bool, i64, bool) {
    let pos = b.sign() == Sign::Plus;
    let sat = if pos { i64::MAX } else { i64::MIN };
    (pos, b.to_i64().unwrap_or(sat), true)
}

/// A ratio buffer size (exact ceiling from `numer`/`denom` — the
/// denominator is always positive).
fn ratio_cap(r: &num_rational::BigRational) -> (bool, i64, bool) {
    let (n, d) = (r.numer(), r.denom());
    let pos = n.sign() == Sign::Plus;
    let sat = if pos { i64::MAX } else { i64::MIN };
    let rem = n % d;
    if rem.sign() == Sign::NoSign {
        return (pos, (n / d).to_i64().unwrap_or(sat), true);
    }
    let trunc = n / d;
    let ceil = if pos { trunc + 1 } else { trunc };
    (pos, ceil.to_i64().unwrap_or(sat), false)
}

/// Make a buffer descriptor of `kind` (`fixed` asserts positivity,
/// like `(buffer n)`).
fn make_buffer(v: &Value, kind: BufferKind, fixed: bool) -> Result<Value, NativeError> {
    let (pos, ceil, integral) = buf_size(v)?;
    if fixed && !pos {
        return Err(assert_failed(
            "fixed buffers must have size > 0",
            "(pos? n)",
        ));
    }
    Ok(Value::Extension(Arc::new(BufferDesc::new(
        kind, ceil, integral,
    ))))
}

fn native_buffer(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/buffer", args, &[1])?;
    make_buffer(&args[0], BufferKind::Fixed, true)
}

fn native_dropping_buffer(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/dropping-buffer", args, &[1])?;
    make_buffer(&args[0], BufferKind::Dropping, false)
}

fn native_sliding_buffer(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/sliding-buffer", args, &[1])?;
    make_buffer(&args[0], BufferKind::Sliding, false)
}

/// Resolve `buf-or-n` to a buffer (buffers pass through, numbers
/// become fixed buffers, nil is unbuffered; anything else is the
/// eager `:full?` protocol error).
fn resolve_chan_buf(buf_or_n: &Value) -> Result<Option<BufferDesc>, NativeError> {
    if let Some(buf) = as_buffer(buf_or_n) {
        return Ok(Some(buf.clone()));
    }
    if !matches!(buf_or_n, Value::Nil) {
        match buf_size(buf_or_n) {
            Ok((pos, ceil, integral)) => {
                if !pos {
                    return Err(assert_failed(
                        "fixed buffers must have size > 0",
                        "(pos? n)",
                    ));
                }
                return Ok(Some(BufferDesc::new(BufferKind::Fixed, ceil, integral)));
            }
            Err(_) => return Err(proto_err(buf_or_n, "full?", "Buffer")),
        }
    }
    Ok(None)
}

fn native_chan(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/chan", args, &[0, 1, 2, 3])?;
    let buf_or_n = args.first().unwrap_or(&Value::Nil);
    let xform = args.get(1).cloned().unwrap_or(Value::Nil);
    if xform.is_truthy() {
        if matches!(buf_or_n, Value::Nil) {
            return Err(assert_failed(
                "buffer must be supplied when transducer is",
                "buf-or-n",
            ));
        }
        // Upstream order (`async.clj` buffer-before-xform): the
        // size validates before the transducer runs, so `(chan 0
        // 42)` asserts instead of CCEing and a bad size never
        // triggers xform side effects (R1).
        let buf = resolve_chan_buf(buf_or_n)?;
        let conj = deref_core(eval, "conj");
        let step = call_checked(eval, &xform, &[conj])?;
        let exh = args.get(2).cloned().unwrap_or(Value::Nil);
        let exh = exh.is_truthy().then_some(exh);
        return Ok(Value::Extension(Arc::new(Channel::new_xform(
            buf.as_ref(),
            step,
            exh,
        ))));
    }
    let buf = resolve_chan_buf(buf_or_n)?;
    Ok(Value::Extension(Arc::new(Channel::new(buf.as_ref()))))
}

/// Split a step result into its emits plus the `reduced` flag (a
/// degenerate step returning a non-vector emits nothing — the
/// JVM's `add!` likewise buffers nothing when the inner `rf`
/// never runs).
fn split_step(ret: Value) -> StepOut {
    let (val, reduced) = match as_reduced(&ret) {
        Some(inner) => (inner.clone(), true),
        None => (ret, false),
    };
    let emits = match val {
        Value::Vector(x) => x.iter().cloned().collect(),
        _ => Vec::new(),
    };
    StepOut { emits, reduced }
}

/// The default ex-handler's one-line print shape (D6; the JVM
/// prints the full stack trace — the port has none).
fn default_ex_message(t: &Throwable) -> String {
    let msg = t.message.as_deref().unwrap_or("");
    format!("{} | {}", t.class.jvm_name(), msg)
}

/// The default ex-handler: print the throwable and drop the value
/// (see [`default_ex_message`] for the shape).
fn default_ex_handler(t: &Throwable) -> Value {
    eprintln!("{}", default_ex_message(t));
    Value::Nil
}

/// The ex-handler path (the JVM's `handle`): a nil result drops
/// the value, anything else (even `false`) buffers as a single
/// untransformed replacement; an ex-handler throw escapes.
fn ex_path(eval: &mut Evaluator, exh: Option<&Value>, t: Throwable) -> Result<StepOut, Throwable> {
    let ret = match exh {
        Some(h) => {
            let arg = Value::Throwable(t);
            call_checked(eval, h, &[arg]).map_err(|e| eval.throwable(e))?
        }
        None => default_ex_handler(&t),
    };
    if matches!(ret, Value::Nil) {
        return Ok(StepOut {
            emits: Vec::new(),
            reduced: false,
        });
    }
    Ok(StepOut {
        emits: vec![ret],
        reduced: false,
    })
}

/// Run one transducer-step request (the channel holds the gate;
/// step failures route through the ex-handler path, like the
/// JVM's `add!` try/catch).
fn xform_step(
    eval: &mut Evaluator,
    step: &Value,
    exh: Option<&Value>,
    req: StepReq,
) -> Result<StepOut, Throwable> {
    let empty = Value::Vector(Vector::from_vec(Vec::new()));
    match req {
        StepReq::Input(val) => match call_checked(eval, step, &[empty, val]) {
            Ok(ret) => Ok(split_step(ret)),
            Err(e) => {
                let t = eval.throwable(e);
                ex_path(eval, exh, t)
            }
        },
        StepReq::Complete => match call_checked(eval, step, &[empty]) {
            Ok(ret) => Ok(split_step(ret)),
            Err(e) => {
                let t = eval.throwable(e);
                ex_path(eval, exh, t)
            }
        },
        StepReq::Failed(t) => ex_path(eval, exh, t),
    }
}

/// A parked-forever waiter result (the owner never waits after
/// killing — unreachable).
fn killed() -> NativeError {
    NativeError::new(
        ThrowClass::Error,
        "async waiter abandoned while parking".to_owned(),
    )
}

/// Run `f` with the channel's stepper (transducer channels only).
fn with_stepper<T>(
    eval: &mut Evaluator,
    ch: &Channel,
    f: impl FnOnce(&mut dyn FnMut(StepReq) -> Result<StepOut, Throwable>) -> T,
) -> T {
    let xform = ch
        .xform()
        .expect("stepper on a channel without a transducer");
    let (step, exh) = (xform.step.clone(), xform.exh.clone());
    let mut stepper = |req: StepReq| xform_step(eval, &step, exh.as_ref(), req);
    f(&mut stepper)
}

/// One put attempt, stepping through the transducer on xform
/// channels (shared by `>!!` and `alts!!`).
fn put_attempt(
    eval: &mut Evaluator,
    ch: &Channel,
    val: Value,
    waiter: &Arc<Waiter>,
    op: usize,
) -> Result<OpReady<bool>, NativeError> {
    if ch.xform().is_some() {
        with_stepper(eval, ch, |stepper| ch.put_xform(val, waiter, op, stepper)).map_err(chan_err)
    } else {
        ch.put_one(val, waiter, op).map_err(chan_err)
    }
}

/// One closed take's value, flushing through the completion
/// arity on xform channels (shared by `<!!` and `alts!!`).
fn closed_take_val(eval: &mut Evaluator, ch: &Channel) -> Result<Value, NativeError> {
    if ch.xform().is_none() {
        return Ok(Value::Nil);
    }
    let flushed =
        with_stepper(eval, ch, |stepper| ch.complete_closed(stepper)).map_err(chan_err)?;
    Ok(flushed.unwrap_or(Value::Nil))
}

fn native_put_block(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/>!!", args, &[2])?;
    let (port, val) = (&args[0], &args[1]);
    if matches!(val, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Can't put nil on channel".to_owned(),
        ));
    }
    let ch = as_chan(port, "put!", "WritePort")?;
    let waiter = Arc::new(Waiter::new());
    match put_attempt(eval, ch, val.clone(), &waiter, 0)? {
        OpReady::Ready(done) => Ok(Value::Bool(done)),
        OpReady::Queued => waiter.wait().map(|(_, v)| v).ok_or_else(killed),
    }
}

fn native_take_block(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/<!!", args, &[1])?;
    let ch = as_chan(&args[0], "take!", "ReadPort")?;
    let waiter = Arc::new(Waiter::new());
    match ch.take_one(&waiter, 0).map_err(chan_err)? {
        OpReady::Ready(found) => match found {
            Some(v) => Ok(v),
            None => closed_take_val(eval, ch),
        },
        OpReady::Queued => waiter.wait().map(|(_, v)| v).ok_or_else(killed),
    }
}

fn native_close(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/close!", args, &[1])?;
    let ch = as_chan(&args[0], "close!", "Channel")?;
    if ch.xform().is_some() {
        with_stepper(eval, ch, |stepper| ch.close_xform(stepper)).map_err(chan_err)?;
    } else {
        ch.close();
    }
    Ok(Value::Nil)
}

fn native_closed(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/closed?", args, &[1])?;
    Ok(Value::Bool(
        as_chan(&args[0], "closed?", "Channel")?.is_closed(),
    ))
}

/// The parking stubs (called outside `go` — the JVM's `assert
/// nil` text, oracle `e-bang-outside-go`). Slice 3 keeps them:
/// inside `go` the macro rewrites parking calls away before they
/// can run, so the stubs only ever fire outside a block.
fn native_bang_take(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/<!", args, &[1])?;
    Err(assert_failed("<! used not in (go ...) block", "nil"))
}

fn native_bang_put(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/>!", args, &[2])?;
    Err(assert_failed(">! used not in (go ...) block", "nil"))
}

fn native_alts_park(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core.async/alts!", args, 1)?;
    Err(assert_failed("alts! used not in (go ...) block", "nil"))
}

/// One parsed `alts!!` op: a take port or a `[put-port val]` pair.
enum AltOp {
    Take(Value),
    Put(Value, Value),
}

/// Parse + precheck every op before any attempt (the JVM's nil
/// precheck loop; short vectors are the `(port 1)` IOOBE — a
/// `MapEntry` is a 2-vector, like the JVM's `vector?`).
fn parse_alt_ops(ports: &[Value]) -> Result<Vec<AltOp>, NativeError> {
    let mut ops = Vec::with_capacity(ports.len());
    for p in ports {
        match p {
            Value::Vector(x) => {
                let mut it = x.iter();
                let (Some(ch), Some(val)) = (it.next(), it.next()) else {
                    return Err(ioobe());
                };
                let (ch, val) = (ch.clone(), val.clone());
                if matches!(val, Value::Nil) {
                    return Err(assert_failed(
                        "can't put nil on channel",
                        "(some? (port 1))",
                    ));
                }
                ops.push(AltOp::Put(ch, val));
            }
            Value::MapEntry { key, val, .. } => {
                if matches!(val.as_ref(), Value::Nil) {
                    return Err(assert_failed(
                        "can't put nil on channel",
                        "(some? (port 1))",
                    ));
                }
                ops.push(AltOp::Put(key.as_ref().clone(), val.as_ref().clone()));
            }
            _ => ops.push(AltOp::Take(p.clone())),
        }
    }
    Ok(ops)
}

/// The `[val port]` result pair.
fn alt_result(val: Value, port: Value) -> Value {
    Value::Vector(Vector::from_vec(vec![val, port]))
}

/// The winning op's port value.
fn op_port(op: &AltOp) -> Value {
    match op {
        AltOp::Take(ch) | AltOp::Put(ch, _) => ch.clone(),
    }
}

/// The parsed `alts!!` options (later pairs win, like
/// `hash-map`; `:default` is presence, `:priority` truth).
struct AltOpts {
    default: Option<Value>,
    priority: bool,
}

fn parse_alt_opts(opts: &[Value]) -> Result<AltOpts, NativeError> {
    if opts.len() % 2 == 1 {
        let last = opts.last().expect("odd length has a last item");
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No value supplied for key: {}", str_of(last)),
        ));
    }
    let is_key =
        |v: &Value, name: &str| matches!(v, Value::Keyword(k) if *k == Keyword::new(None, name));
    let mut out = AltOpts {
        default: None,
        priority: false,
    };
    for pair in opts.chunks_exact(2) {
        if is_key(&pair[0], "default") {
            out.default = Some(pair[1].clone());
        } else if is_key(&pair[0], "priority") {
            out.priority = pair[1].is_truthy();
        }
    }
    Ok(out)
}

fn native_alts_block(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_min_arity("clojure.core.async/alts!!", args, 1)?;
    let opts = parse_alt_opts(&args[1..])?;
    let ports = seq_vec(&args[0]).ok_or_else(|| iseq_err(&args[0]))?;
    if ports.is_empty() {
        return Err(assert_failed(
            "alts must have at least one channel operation",
            "(pos? (count ports))",
        ));
    }
    let ops = parse_alt_ops(&ports)?;
    let mut order: Vec<usize> = (0..ops.len()).collect();
    if !opts.priority {
        order.shuffle(&mut rand::rng());
    }
    let waiter = Arc::new(Waiter::new());
    if let Some(won) = alts_try_ready(eval, &ops, &order, &waiter)? {
        return Ok(won);
    }
    // The `:default` fallback (#123s1): claim-first — exactly one
    // of us or a racing completion wins the waiter (the `do-alts`
    // flag commit). The winner's op index resolves through the
    // port table without branching (default rides at `ops.len()`).
    if let Some(default) = &opts.default {
        let _ = waiter.try_win(ops.len(), default.clone());
    }
    let (op, val) = waiter.wait().ok_or_else(killed)?;
    let mut ports: Vec<Value> = ops.iter().map(op_port).collect();
    if opts.default.is_some() {
        ports.push(default_port());
    }
    Ok(alt_result(val, ports[op].clone()))
}

/// One `alts!!` immediate pass (#123s1): register every op in
/// order, returning the first ready result (like `do-alts`'
/// immediate check before parking). `None` means every op queued.
fn alts_try_ready(
    eval: &mut Evaluator,
    ops: &[AltOp],
    order: &[usize],
    waiter: &Arc<Waiter>,
) -> Result<Option<Value>, NativeError> {
    for idx in order {
        match &ops[*idx] {
            AltOp::Take(chv) => {
                let ch = as_chan(chv, "take!", "ReadPort")?;
                match ch.take_one(waiter, *idx).map_err(chan_err)? {
                    OpReady::Ready(found) => {
                        let val = match found {
                            Some(v) => v,
                            None => closed_take_val(eval, ch)?,
                        };
                        return Ok(Some(alt_result(val, chv.clone())));
                    }
                    OpReady::Queued => {}
                }
            }
            AltOp::Put(chv, val) => {
                let ch = as_chan(chv, "put!", "WritePort")?;
                match put_attempt(eval, ch, val.clone(), waiter, *idx)? {
                    OpReady::Ready(done) => {
                        return Ok(Some(alt_result(Value::Bool(done), chv.clone())));
                    }
                    OpReady::Queued => {}
                }
            }
        }
    }
    Ok(None)
}

/// Slice 3: the `go` parking-op renames (`<!` → `<!!`, `>!` →
/// `>!!`, `alts!` → `alts!!`, `alt!` → `alt!!` — `alt!` renames
/// to `alt!!` before it can expand, so the choice parks through
/// the blocking op).
fn parking_rename(name: &str) -> Option<&'static str> {
    match name {
        "<!" => Some("<!!"),
        ">!" => Some(">!!"),
        "alts!" => Some("alts!!"),
        "alt!" => Some("alt!!"),
        _ => None,
    }
}

/// Whether `sym` names one of this namespace's parking ops
/// (resolve-aware: aliases, refers, and renames count — the
/// rename follows the TARGET var, so a `:rename`d parking op
/// still parks; any other target, or an unresolvable, is left
/// alone). Returns the blocking replacement name.
fn parking_target(eval: &Evaluator, sym: &Symbol) -> Option<&'static str> {
    let cur = eval.current_ns_name();
    let target = eval.registry.resolve_in(&cur, sym)?;
    if target.namespace() != Some(ASYNC_NS) {
        return None;
    }
    parking_rename(target.name())
}

/// Whether the `clojure.core` builtin behind `name` is intact
/// (the analyzer's `core_macro_intact`: resolvable to the
/// still-a-macro core var).
fn core_macro_intact(eval: &Evaluator, name: &str) -> bool {
    let cur = eval.current_ns_name();
    let probe = Symbol::new(Some("clojure.core"), name);
    let Some(target) = eval.registry.resolve_in(&cur, &probe) else {
        return false;
    };
    target.namespace() == Some("clojure.core")
        && target.name() == name
        && eval
            .registry
            .get("clojure.core")
            .and_then(|ns| ns.lookup(&target))
            .is_some_and(|id| crate::macros::is_macro_var(eval, id))
}

/// Whether a bare `let`/`loop`/`fn` head is the builtin (the
/// analyzer's shadow rule: a lexically-visible local always wins;
/// otherwise a global shadows unless it is the intact core
/// macro).
fn builtin_head(eval: &Evaluator, shadow: &HashSet<String>, name: &str) -> bool {
    if shadow.contains(name) {
        return false;
    }
    let cur = eval.current_ns_name();
    let probe = Symbol::new(None, name);
    let Some(target) = eval.registry.resolve_in(&cur, &probe) else {
        return true;
    };
    target.namespace() == Some("clojure.core")
        && target.name() == name
        && eval
            .registry
            .get("clojure.core")
            .and_then(|ns| ns.lookup(&target))
            .is_some_and(|id| crate::macros::is_macro_var(eval, id))
}

/// The `&env` local names (nil at top level, else symbol keys —
/// `contains?` and truthiness behave).
fn env_locals(env: &Value) -> HashSet<String> {
    let Value::Map(m) = env else {
        return HashSet::new();
    };
    m.iter()
        .filter_map(|(k, _)| match k {
            Value::Symbol(s) if s.namespace().is_none() => Some(s.name().to_owned()),
            _ => None,
        })
        .collect()
}

/// Whether the head is always opaque (real specials are never
/// shadowed — `fn*` too, since `#()` reads through it).
fn is_boundary_head(ns: Option<&str>, name: &str) -> bool {
    ns.is_none() && matches!(name, "quote" | "var" | "fn*")
}

/// The `go` body rewrite (slice 3): operator-position parking
/// calls resolving to this namespace become the blocking ops;
/// `fn`/`fn*` bodies, `quote`, and `var` forms bound the walk;
/// `let`/`loop` (plus `let*`/`loop*`) bindings extend the shadow
/// set. Rebuilt forms keep their meta, so reader spans survive
/// the rewrite.
struct GoRewrite<'a> {
    eval: &'a Evaluator,
    shadow: HashSet<String>,
}

impl GoRewrite<'_> {
    fn builtin(&self, name: &str) -> bool {
        builtin_head(self.eval, &self.shadow, name)
    }

    fn walk(&mut self, form: &Value) -> Value {
        match form {
            Value::List(l) => {
                let items: Vec<Value> = l.iter().cloned().collect();
                self.walk_list(form, &items)
            }
            Value::Vector(v) => {
                let items = v.iter().map(|e| self.walk(e)).collect();
                vecv(items).with_meta(form.meta().cloned())
            }
            Value::Map(m) => {
                let mut out = new_map();
                for (k, v) in m.iter() {
                    out = out.insert(self.walk(k), self.walk(v));
                }
                Value::Map(out).with_meta(form.meta().cloned())
            }
            Value::Set(s) => {
                let mut out = new_set();
                for e in s.iter() {
                    out = out.insert(self.walk(e));
                }
                Value::Set(out).with_meta(form.meta().cloned())
            }
            Value::MapEntry { key, val, .. } => map_entry(self.walk(key), self.walk(val)),
            _ => form.clone(),
        }
    }

    /// Walk a list form by head (analyzer dispatch order: real
    /// specials first, then the shadowable `let`/`loop`/`fn`,
    /// then the parking rename, else uniformly).
    fn walk_list(&mut self, form: &Value, items: &[Value]) -> Value {
        let Some(head) = items.first() else {
            return form.clone();
        };
        let Value::Symbol(sym) = head else {
            return self.walk_uniform(form, items);
        };
        let (ns, name) = (sym.namespace(), sym.name());
        if is_boundary_head(ns, name) || self.is_fn_head(ns, name) {
            return form.clone();
        }
        if self.is_let_head(ns, name) {
            return self.walk_let(form, items);
        }
        // The parking rename (locals shadow bare heads).
        if ns.is_none() && self.shadow.contains(name) {
            return self.walk_uniform(form, items);
        }
        if let Some(blocking) = parking_target(self.eval, sym) {
            let mut out = Vec::with_capacity(items.len());
            let renamed = Symbol::new(Some(ASYNC_NS), blocking).with_meta(sym.meta().cloned());
            out.push(Value::Symbol(renamed));
            out.extend(items[1..].iter().map(|e| self.walk(e)));
            return lst(out).with_meta(form.meta().cloned());
        }
        self.walk_uniform(form, items)
    }

    /// Whether the head opens a `let`/`loop` binding form (`+` the
    /// `*` pair): builtin when unshadowed, a plain call when
    /// shadowed. A qualified real special is never the builtin (a
    /// var lookup instead).
    fn is_let_head(&self, ns: Option<&str>, name: &str) -> bool {
        if !matches!(name, "let" | "let*" | "loop" | "loop*") {
            return false;
        }
        if ns.is_none() {
            return self.builtin(name);
        }
        ns == Some("clojure.core")
            && matches!(name, "let" | "loop")
            && core_macro_intact(self.eval, name)
    }

    /// Whether the head opens a builtin `fn` (a boundary; a
    /// shadowed `fn` is a plain call).
    fn is_fn_head(&self, ns: Option<&str>, name: &str) -> bool {
        if name != "fn" {
            return false;
        }
        if ns.is_none() {
            return self.builtin(name);
        }
        ns == Some("clojure.core") && core_macro_intact(self.eval, name)
    }

    /// Walk every element (operator included — it may itself nest
    /// a parking call, as in `((fn [c] ...) ...)`).
    fn walk_uniform(&mut self, form: &Value, items: &[Value]) -> Value {
        lst(items.iter().map(|e| self.walk(e)).collect()).with_meta(form.meta().cloned())
    }

    /// Walk a `let`/`loop` form: inits left to right (each sees
    /// the previous bindings), then the body under the full set.
    /// A non-vector bindings form walks uniformly (the analyzer
    /// reports the shape).
    fn walk_let(&mut self, form: &Value, items: &[Value]) -> Value {
        let Some(Value::Vector(bv)) = items.get(1) else {
            return self.walk_uniform(form, items);
        };
        let bindings_meta = items[1].meta().cloned();
        let parts: Vec<Value> = bv.iter().cloned().collect();
        let mut new_parts: Vec<Value> = Vec::with_capacity(parts.len());
        let mut added: Vec<String> = Vec::new();
        let mut chunks = parts.chunks_exact(2);
        for pair in &mut chunks {
            let (names, pat) = self.walk_pattern(&pair[0]);
            new_parts.push(pat);
            new_parts.push(self.walk(&pair[1]));
            for name in names {
                if !self.shadow.contains(&name) {
                    self.shadow.insert(name.clone());
                    added.push(name);
                }
            }
        }
        // A trailing odd form walks as code (the analyzer reports it).
        for rest in chunks.remainder() {
            new_parts.push(self.walk(rest));
        }
        let mut out: Vec<Value> = Vec::with_capacity(items.len());
        out.push(items[0].clone());
        out.push(vecv(new_parts).with_meta(bindings_meta));
        out.extend(items.get(2..).unwrap_or(&[]).iter().map(|e| self.walk(e)));
        for name in added {
            self.shadow.remove(&name);
        }
        lst(out).with_meta(form.meta().cloned())
    }

    /// Collect a binding pattern's names, rewriting `:or`
    /// defaults as code (they evaluate in the pre-pattern scope).
    /// The analyzer validates shapes; unknown shapes bind
    /// nothing.
    fn walk_pattern(&mut self, pat: &Value) -> (Vec<String>, Value) {
        match pat {
            Value::Symbol(s) if s.namespace().is_none() => (vec![s.name().to_owned()], pat.clone()),
            Value::Vector(v) => {
                let mut names = Vec::new();
                let mut out = Vec::new();
                for e in v.iter() {
                    // `&` / `:as` markers bind nothing themselves;
                    // their operands flow through the same arms.
                    if is_amp(e) || is_as(e) {
                        out.push(e.clone());
                    } else {
                        let (mut found, walked) = self.walk_pattern(e);
                        names.append(&mut found);
                        out.push(walked);
                    }
                }
                (names, vecv(out).with_meta(pat.meta().cloned()))
            }
            Value::Map(m) => self.walk_map_pattern(pat, m),
            _ => (Vec::new(), pat.clone()),
        }
    }

    /// Collect a map pattern's names while rewriting `:or`
    /// defaults as code.
    fn walk_map_pattern(&mut self, pat: &Value, m: &clj_value::Map) -> (Vec<String>, Value) {
        let mut names = Vec::new();
        let mut out = new_map();
        for (k, v) in m.iter() {
            let (walked_k, walked_v) = self.walk_map_entry(k, v, &mut names);
            out = out.insert(walked_k, walked_v);
        }
        (names, Value::Map(out).with_meta(pat.meta().cloned()))
    }

    /// Walk one map-pattern entry (`:keys`/`:syms`/`:strs`
    /// entries bind their name part, `:as` its symbol, `:or`
    /// defaults rewrite as code; any other key recurses as a
    /// local-or-pattern while its value — match data, never code
    /// — stands).
    fn walk_map_entry(&mut self, k: &Value, v: &Value, names: &mut Vec<String>) -> (Value, Value) {
        if let Value::Keyword(key) = k {
            if key.namespace().is_none() {
                match key.name() {
                    "keys" | "syms" | "strs" => {
                        names.extend(short_names(v));
                        return (k.clone(), v.clone());
                    }
                    "as" => {
                        if let Value::Symbol(s) = v {
                            if s.namespace().is_none() {
                                names.push(s.name().to_owned());
                            }
                        }
                        return (k.clone(), v.clone());
                    }
                    "or" => return (k.clone(), self.walk_or(v)),
                    _ => {}
                }
            }
        }
        let (mut found, walked) = self.walk_pattern(k);
        names.append(&mut found);
        (walked, v.clone())
    }

    /// Rewrite an `:or` defaults map's values as code (keys are
    /// local names — data).
    fn walk_or(&mut self, v: &Value) -> Value {
        let Value::Map(m) = v else {
            return v.clone();
        };
        let mut out = new_map();
        for (k, dflt) in m.iter() {
            out = out.insert(k.clone(), self.walk(dflt));
        }
        Value::Map(out).with_meta(v.meta().cloned())
    }
}

/// The locals a `:keys`/`:syms`/`:strs` vector binds (each entry
/// binds its name part — `:keys [a/b]` binds `b`).
fn short_names(v: &Value) -> Vec<String> {
    let Value::Vector(items) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|e| match e {
            Value::Symbol(s) => Some(s.name().to_owned()),
            _ => None,
        })
        .collect()
}

/// `go`: rewrite the body's visible parking calls to the blocking
/// ops and run the rewritten body on its own thread, returning
/// the result channel (D9: one thread per block instead of the
/// state machine + dispatch pool).
///
/// ```clojure
/// (go body...) =>
/// (let [c (chan 1)] (go-call c (fn [] <rewritten body...>)) c)
/// ```
fn native_go(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 {
        return Err(NativeError::arity("clojure.core.async/go", args.len()));
    }
    let env = args[1].clone();
    let rewritten: Vec<Value> = {
        let mut rw = GoRewrite {
            eval: &*eval,
            shadow: env_locals(&env),
        };
        args[2..].iter().map(|e| rw.walk(e)).collect()
    };
    let chan = Value::Symbol(gs(eval, "G__"));
    let mut thunk = vec![sym_q("clojure.core", "fn"), vecv(Vec::new())];
    thunk.extend(rewritten);
    Ok(with_form_meta(
        lst(vec![
            sym_q("clojure.core", "let"),
            vecv(vec![
                chan.clone(),
                lst(vec![sym_q(ASYNC_NS, "chan"), Value::Int(1)]),
            ]),
            lst(vec![sym_q(ASYNC_NS, "go-call"), chan.clone(), lst(thunk)]),
            chan,
        ]),
        &args[0],
    ))
}

/// `go-loop`: `(go (loop bindings body...))`.
fn native_go_loop(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 3 {
        return Err(NativeError::arity("clojure.core.async/go-loop", args.len()));
    }
    let mut inner = vec![sym_q("clojure.core", "loop"), args[2].clone()];
    inner.extend(args[3..].iter().cloned());
    Ok(lst(vec![sym_q(ASYNC_NS, "go"), lst(inner)]))
}

/// `alt!`: the `do-alt` expansion over `alts!` (inside `go` the
/// `go` rewrite renames the operator to `alt!!` first; outside
/// `go` the `alts!` call fails with the parking assert).
fn native_alt_bang(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    alt_impl(eval, args, "clojure.core.async/alt!", "alts!")
}

/// `alt!!`: the `do-alt` expansion over `alts!!`.
fn native_alt_bang_block(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    alt_impl(eval, args, "clojure.core.async/alt!!", "alts!!")
}

fn alt_impl(
    eval: &mut Evaluator,
    args: &[Value],
    display: &str,
    alts: &str,
) -> Result<Value, NativeError> {
    if args.len() < 2 {
        return Err(NativeError::arity(display, args.len()));
    }
    do_alt(eval, alts, &args[2..])
}

/// The `alt!`/`alt!!` expansion (upstream `do-alt` over `alts!` /
/// `alts!!`): gensym every port/val operand once, choose through
/// the multi-op, dispatch on the winning port, with binding-form
/// result exprs destructuring the `[val port]` return.
fn do_alt(eval: &mut Evaluator, alts: &str, clauses: &[Value]) -> Result<Value, NativeError> {
    if clauses.len() % 2 == 1 {
        return Err(assert_failed(
            "unbalanced clauses",
            "(even? (count clauses))",
        ));
    }
    // Partition into option pairs (keyword-led — any keyword) and
    // op clauses, preserving order (upstream `filter`/`remove`
    // over `opt?`).
    let mut opts: Vec<Value> = Vec::new();
    let mut ops: Vec<(&Value, &Value)> = Vec::new();
    for pair in clauses.chunks_exact(2) {
        if matches!(&pair[0], Value::Keyword(_)) {
            opts.push(pair[0].clone());
            opts.push(pair[1].clone());
        } else {
            ops.push((&pair[0], &pair[1]));
        }
    }
    // Gensym every operand once (takes bind one port gensym; puts
    // bind port+val and pass the pair). Short put vectors
    // destructure leniently (a missing val is nil, like the JVM's
    // `[port val]` binding — the multi-op asserts it).
    let mut bindings: Vec<Value> = Vec::new();
    let mut ports: Vec<Value> = Vec::new();
    let mut conds: Vec<(Vec<Value>, Value)> = Vec::new();
    for (ports_form, expr) in &ops {
        let op_ports: Vec<&Value> = match ports_form {
            Value::Vector(v) => v.iter().collect(),
            _ => vec![*ports_form],
        };
        let mut tests: Vec<Value> = Vec::new();
        for port in op_ports {
            match port {
                Value::Vector(pv) => {
                    let items: Vec<&Value> = pv.iter().collect();
                    let gp = Value::Symbol(gs(eval, "G__"));
                    let gv = Value::Symbol(gs(eval, "G__"));
                    bindings.push(gp.clone());
                    bindings.push(items.first().map(|v| (*v).clone()).unwrap_or(Value::Nil));
                    bindings.push(gv.clone());
                    bindings.push(items.get(1).map(|v| (*v).clone()).unwrap_or(Value::Nil));
                    ports.push(vecv(vec![gp.clone(), gv]));
                    tests.push(gp);
                }
                _ => {
                    let gp = Value::Symbol(gs(eval, "G__"));
                    bindings.push(gp.clone());
                    bindings.push(port.clone());
                    ports.push(gp.clone());
                    tests.push(gp);
                }
            }
        }
        conds.push((tests, (*expr).clone()));
    }
    let gch = Value::Symbol(gs(eval, "ch"));
    let gret = Value::Symbol(gs(eval, "ret"));
    let gval = Value::Symbol(gs_auto(eval, "val"));
    let mut call = vec![sym_q(ASYNC_NS, alts), vecv(ports)];
    call.extend(opts);
    bindings.push(vecv(vec![
        gval.clone(),
        gch.clone(),
        kw("as"),
        gret.clone(),
    ]));
    bindings.push(lst(call));
    let mut arms: Vec<Value> = Vec::new();
    for (tests, expr) in &conds {
        let or: Vec<Value> = std::iter::once(sym_q("clojure.core", "or"))
            .chain(
                tests
                    .iter()
                    .map(|t| lst(vec![sym_q("clojure.core", "="), gch.clone(), t.clone()])),
            )
            .collect();
        arms.push(lst(or));
        arms.push(alt_result_expr(expr, &gret));
    }
    arms.push(lst(vec![
        sym_q("clojure.core", "="),
        gch.clone(),
        kw("default"),
    ]));
    arms.push(gval);
    let mut cond = vec![sym_q("clojure.core", "cond")];
    cond.extend(arms);
    Ok(lst(vec![
        sym_q("clojure.core", "let"),
        vecv(bindings),
        lst(cond),
    ]))
}

/// An `alt!` result expr: a list led by a vector destructures the
/// `[val port]` return, anything else stands as-is.
fn alt_result_expr(expr: &Value, gret: &Value) -> Value {
    if let Value::List(l) = expr {
        if let Some(Value::Vector(_)) = l.first() {
            let mut out = vec![
                sym_q("clojure.core", "let"),
                vecv(vec![
                    l.first().cloned().expect("vector head checked above"),
                    gret.clone(),
                ]),
            ];
            out.extend(l.iter().skip(1).cloned());
            return lst(out);
        }
    }
    expr.clone()
}

/// Go-thread sequence (names only — `clj-go-{n}`).
static GO_SEQ: AtomicU64 = AtomicU64::new(1);

/// `future-call`'s spawn failure, reused for `go` threads (the
/// JVM's `unable to create native thread`).
fn go_spawn_failed() -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        "unable to create native thread".to_owned(),
    )
}

/// The `go` spawn + completion protocol (D12, public like
/// upstream's `ioc-alts!`): run the thunk on its own thread with
/// the calling thread's bindings conveyed, deliver a non-nil
/// result to the channel (`return-chan`: nil skipped, `false`
/// delivered), and close it; on a throw, close and print the D6
/// one-liner. The thread detaches — the dispatch pool's threads
/// are daemon, so parked gos never hold the runtime alive past
/// main. No shutdown latch either (upstream never shuts the
/// dispatch pools down).
fn native_go_call(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/go-call", args, &[2])?;
    as_chan(&args[0], "put!", "WritePort")?;
    let port = args[0].clone();
    let thunk = args[1].clone();
    let child = eval.fork_for_thread(eval.clone_bindings());
    let n = GO_SEQ.fetch_add(1, Ordering::Relaxed);
    let handle = std::thread::Builder::new()
        .name(format!("clj-go-{n}"))
        .spawn(move || run_go_body(child, port, thunk))
        .map_err(|_| go_spawn_failed())?;
    drop(handle); // detached: daemon semantics (parked gos never hold the exit join)
    Ok(Value::Nil)
}

/// A go block's whole thread: call the thunk, deliver + close, or
/// close + report (every path settles the channel — waiters never
/// hang — and the thread never dies).
fn run_go_body(mut child: Evaluator, port: Value, thunk: Value) {
    let ch = as_channel(&port).expect("go-call validates its channel before spawning");
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| child.call(&thunk, &[])));
    match go_outcome(out) {
        Ok(v) => {
            if !matches!(v, Value::Nil) {
                go_deliver(ch, v);
            }
            ch.close();
        }
        Err(t) => {
            ch.close();
            eprintln!("{}", default_ex_message(&t));
        }
    }
}

/// Map a caught go-body outcome to a value-or-throwable (a Rust
/// panic or an escaping `recur` is a synthetic `Error` — both
/// unreachable in practice, but the channel must still close).
fn go_outcome(
    out: Result<Result<Value, Signal>, Box<dyn std::any::Any + Send>>,
) -> Result<Value, Throwable> {
    match out {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(Signal::Throw(t))) => Err(t),
        Ok(Err(Signal::Recur(_))) => Err(internal_throwable(
            "recur escaped its loop or fn".to_owned(),
        )),
        Err(panicked) => Err(internal_throwable(go_panic_message(&panicked))),
    }
}

/// A Rust panic's message, best-effort (`&str`/`String` payloads,
/// else a fixed tag — panics are port bugs, never user failures).
fn go_panic_message(panicked: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panicked.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panicked.downcast_ref::<String>() {
        s.clone()
    } else {
        "go block panicked".to_owned()
    }
}

/// Deliver the go result (the channel buffers one and takes
/// exactly one put, so the put always completes synchronously —
/// a queued put waits out of caution, folded into the ready
/// line).
fn go_deliver(ch: &Channel, val: Value) {
    let waiter = Arc::new(Waiter::new());
    if let Ok(ready) = ch.put_one(val, &waiter, 0) {
        let _ = matches!(ready, OpReady::Queued).then(|| waiter.wait());
    }
}

/// The `clojure.core.async` natives (installed lazily by
/// [`install_async_ns`] — never referred into `user`).
pub(crate) const ASYNC_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("chan", native_chan),
    ("buffer", native_buffer),
    ("dropping-buffer", native_dropping_buffer),
    ("sliding-buffer", native_sliding_buffer),
    (">!!", native_put_block),
    ("<!!", native_take_block),
    ("close!", native_close),
    ("closed?", native_closed),
    ("alts!!", native_alts_block),
    ("<!", native_bang_take),
    (">!", native_bang_put),
    ("alts!", native_alts_park),
    ("go", native_go),
    ("go-loop", native_go_loop),
    ("alt!", native_alt_bang),
    ("alt!!", native_alt_bang_block),
    ("go-call", native_go_call),
];

/// Install the `clojure.core.async` namespace (idempotent —
/// reinstalling over itself is a `:reload` no-op).
pub(crate) fn install_async_ns(eval: &mut Evaluator) {
    eval.registry.ensure_ns(ASYNC_NS);
    for (name, f) in ASYNC_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(ASYNC_NS, name, func);
    }
    for name in ["go", "go-loop", "alt!", "alt!!"] {
        let q = Symbol::new(Some(ASYNC_NS), name);
        let id = eval.registry.intern(&q);
        eval.registry.with_state_mut(id, |st| {
            let meta = st.meta.clone().unwrap_or_else(new_map).insert(
                Value::Keyword(Keyword::new(None, "macro")),
                Value::Bool(true),
            );
            st.meta = Some(meta);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx4_form::sym;

    /// A scratch evaluator with the async namespace installed
    /// (fully-qualified heads resolve; no aliases needed).
    fn scratch_async() -> Evaluator {
        let mut eval = Evaluator::new_bare("user");
        install_async_ns(&mut eval);
        eval
    }

    /// Expand `(go body...)` over a synthetic `(&form, &env)` pair.
    fn expand_go(eval: &mut Evaluator, env: Value, body: &[Value]) -> Value {
        let form = lst(vec![sym("go")]);
        let mut args = vec![form, env];
        args.extend(body.iter().cloned());
        native_go(eval, &args).expect("go expands")
    }

    /// Readable print (expansion shapes assert as strings — a
    /// fresh scratch evaluator mints deterministic gensyms).
    fn pr_exp(v: &Value) -> String {
        clj_value::pr_to_string(v, &clj_value::PrintOpts::pr())
    }

    /// `QueueFull` maps to the exact JVM assert texts (puts name
    /// the windowed-buffer hint; takes do not).
    #[test]
    fn chan_err_queue_full_texts() {
        let puts = chan_err(ChanError::QueueFull { puts: true });
        assert_eq!(puts.class, ThrowClass::AssertionError);
        assert!(puts.message.contains("pending puts"), "{}", puts.message);
        assert!(puts.message.contains("windowed buffer"), "{}", puts.message);
        let takes = chan_err(ChanError::QueueFull { puts: false });
        assert_eq!(takes.class, ThrowClass::AssertionError);
        assert!(takes.message.contains("pending takes"), "{}", takes.message);
        assert!(!takes.message.contains("windowed"), "{}", takes.message);
    }

    /// The default ex-handler prints one `Class | message` line
    /// (D6; the trace itself is unobservable, so the shape pins
    /// the message fn, not the stderr transport).
    #[test]
    fn default_ex_message_shape() {
        let t = Throwable {
            id: 0,
            class: ThrowClass::ExceptionInfo,
            message: Some(Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert_eq!(default_ex_message(&t), "clojure.lang.ExceptionInfo | boom");
        let mute = Throwable { message: None, ..t };
        assert_eq!(default_ex_message(&mute), "clojure.lang.ExceptionInfo | ");
    }

    /// Exotic buffer sizes: negative infinity, bigints, exact
    /// ratios (fractional doubles pin through eval).
    #[test]
    fn exotic_caps() {
        assert_eq!(double_cap(f64::NEG_INFINITY), (false, i64::MIN, false));
        assert_eq!(bigint_cap(&num_bigint::BigInt::from(5)), (true, 5, true));
        assert_eq!(bigint_cap(&num_bigint::BigInt::from(-5)), (false, -5, true));
        let four_halves = num_rational::BigRational::new(
            num_bigint::BigInt::from(4),
            num_bigint::BigInt::from(2),
        );
        assert_eq!(ratio_cap(&four_halves), (true, 2, true));
        let big5 = Arc::new(num_bigint::BigInt::from(5));
        assert_eq!(
            buf_size(&Value::BigInt(Arc::clone(&big5))).expect("bigint buf"),
            (true, 5, true)
        );
        assert_eq!(
            buf_size(&Value::BigInteger(big5)).expect("biginteger buf"),
            (true, 5, true)
        );
    }

    /// The abandoned-waiter internal error shape.
    #[test]
    fn killed_shape() {
        let err = killed();
        assert_eq!(err.class, ThrowClass::Error);
        assert_eq!(err.message, "async waiter abandoned while parking");
    }

    /// A map-entry op with a nil value fails the nil-put assert
    /// (like the vector pair shape).
    #[test]
    fn map_entry_nil_put_asserts() {
        let ch = Value::Extension(Arc::new(Channel::new(None)));
        let entry = clj_value::map_entry(ch, Value::Nil);
        let err = parse_alt_ops(std::slice::from_ref(&entry))
            .err()
            .expect("nil-put assert");
        assert_eq!(err.class, ThrowClass::AssertionError);
        let msg = &err.message;
        assert!(msg.contains("can't put nil on channel"), "{msg}");
    }

    /// The `go` rewrite renames every visible parking call (all
    /// four heads, arg positions included) to the fully-qualified
    /// blocking op.
    #[test]
    fn go_rewrite_renames_visible_parking_calls() {
        let mut eval = scratch_async();
        let take = lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")]);
        let body = lst(vec![
            sym("do"),
            take,
            lst(vec![sym_q(ASYNC_NS, ">!"), sym("c"), sym("v")]),
            lst(vec![
                sym("+"),
                lst(vec![sym_q(ASYNC_NS, "alts!"), sym("ps")]),
            ]),
            lst(vec![sym_q(ASYNC_NS, "alt!"), sym("c"), sym("x")]),
        ]);
        assert_eq!(
            pr_exp(&expand_go(&mut eval, Value::Nil, &[body])),
            "(clojure.core/let [G__1 (clojure.core.async/chan 1)] \
             (clojure.core.async/go-call G__1 (clojure.core/fn [] \
             (do (clojure.core.async/<!! c) (clojure.core.async/>!! c v) \
             (+ (clojure.core.async/alts!! ps)) (clojure.core.async/alt!! c x)))) G__1)"
        );
    }

    /// The `go` rewrite boundaries: `fn`/`fn*` bodies, `quote`,
    /// and `var` forms pass through untouched.
    #[test]
    fn go_rewrite_boundaries_hold() {
        let mut eval = scratch_async();
        let take = || lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")]);
        let body = lst(vec![
            sym("do"),
            lst(vec![sym("fn"), vecv(vec![]), take()]),
            lst(vec![sym("fn*"), vecv(vec![]), take()]),
            lst(vec![sym("quote"), take()]),
            lst(vec![sym("var"), sym_q(ASYNC_NS, "<!")]),
            take(),
        ]);
        assert_eq!(
            pr_exp(&expand_go(&mut eval, Value::Nil, &[body])),
            "(clojure.core/let [G__1 (clojure.core.async/chan 1)] \
             (clojure.core.async/go-call G__1 (clojure.core/fn [] \
             (do (fn [] (clojure.core.async/<! c)) (fn* [] (clojure.core.async/<! c)) \
             (quote (clojure.core.async/<! c)) (var clojure.core.async/<!) \
             (clojure.core.async/<!! c)))) G__1)"
        );
    }

    /// The `go` rewrite shadows: `&env` locals plus `let` bindings
    /// on the walk path (sequential — an init sees only the
    /// previous pairs).
    #[test]
    fn go_rewrite_shadows() {
        let mut eval = scratch_async();
        // An `&env` local shadows the bare head.
        let env = new_map().insert(sym("<!"), Value::Bool(true));
        let body = lst(vec![sym("<!"), sym("x")]);
        assert_eq!(
            pr_exp(&expand_go(&mut eval, Value::Map(env), &[body])),
            "(clojure.core/let [G__1 (clojure.core.async/chan 1)] \
             (clojure.core.async/go-call G__1 (clojure.core/fn [] (<! x))) G__1)"
        );
        // A `let` binding shadows the body but not its own init.
        let body = lst(vec![
            sym("let"),
            vecv(vec![sym("<!"), lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")])]),
            lst(vec![sym("<!"), sym("x")]),
        ]);
        assert_eq!(
            pr_exp(&expand_go(&mut eval, Value::Nil, &[body])),
            "(clojure.core/let [G__2 (clojure.core.async/chan 1)] \
             (clojure.core.async/go-call G__2 (clojure.core/fn [] \
             (let [<! (clojure.core.async/<!! c)] (<! x)))) G__2)"
        );
    }

    /// Heads resolving anywhere but the async var are left alone
    /// (unresolvable here — the analyzer reports them).
    #[test]
    fn go_rewrite_ignores_foreign_heads() {
        let mut eval = scratch_async();
        let body = lst(vec![sym("not-a-parking-op"), sym("x")]);
        let exp = pr_exp(&expand_go(&mut eval, Value::Nil, &[body]));
        assert!(exp.contains("(not-a-parking-op x)"), "{exp}");
    }

    /// A qualified `let` the registry cannot resolve walks
    /// uniformly (the analyzer reports it — the intact-builtin
    /// path pins at the Clojure level).
    #[test]
    fn go_rewrite_unresolved_qualified_let_walks_uniformly() {
        let mut eval = scratch_async();
        let body = lst(vec![
            sym_q("clojure.core", "let"),
            vecv(vec![sym("x"), lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")])]),
            sym("x"),
        ]);
        let exp = pr_exp(&expand_go(&mut eval, Value::Nil, &[body]));
        assert!(
            exp.contains("(clojure.core/let [x (clojure.core.async/<!! c)] x)"),
            "{exp}"
        );
    }

    /// Malformed `let` shapes walk uniformly (the analyzer reports
    /// them): a non-vector bindings form, and a trailing odd form
    /// walked as code.
    #[test]
    fn go_rewrite_malformed_lets_walk_uniformly() {
        let eval = scratch_async();
        let mut rw = GoRewrite {
            eval: &eval,
            shadow: HashSet::new(),
        };
        let take = lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")]);
        let flat = lst(vec![sym("let"), Value::Int(5), take.clone()]);
        assert_eq!(
            pr_exp(&rw.walk(&flat)),
            "(let 5 (clojure.core.async/<!! c))"
        );
        let odd = lst(vec![
            sym("let"),
            vecv(vec![sym("x"), Value::Int(1), take.clone()]),
            sym("x"),
        ]);
        assert_eq!(
            pr_exp(&rw.walk(&odd)),
            "(let [x 1 (clojure.core.async/<!! c)] x)"
        );
    }

    /// Unknown binding shapes bind nothing: namespaced symbols, a
    /// non-map `:or`, unknown map directives, and non-patterns.
    #[test]
    fn go_walk_pattern_unknown_shapes_bind_nothing() {
        let eval = scratch_async();
        let mut rw = GoRewrite {
            eval: &eval,
            shadow: HashSet::new(),
        };
        let (names, _) = rw.walk_pattern(&sym_q("n", "x"));
        assert!(names.is_empty());
        let (names, _) = rw.walk_pattern(&Value::Int(7));
        assert!(names.is_empty());
        let or = new_map()
            .insert(kw("or"), Value::Int(7))
            .insert(kw("bogus"), Value::Int(1));
        let (names, walked) = rw.walk_pattern(&Value::Map(or));
        assert!(names.is_empty());
        assert_eq!(pr_exp(&walked), "{:or 7, :bogus 1}");
        let entry = map_entry(sym("k"), lst(vec![sym_q(ASYNC_NS, "<!"), sym("c")]));
        assert_eq!(pr_exp(&rw.walk(&entry)), "[k (clojure.core.async/<!! c)]");
    }

    /// The `do-alt` shape: one gensym per operand, the multi-op
    /// call, the port dispatch with a binding-form result, and
    /// the `:default` fallthrough — all gensym occurrences
    /// consistent (exact print, like upstream's EXP1).
    #[test]
    fn do_alt_shape() {
        let mut eval = scratch_async();
        let form = lst(vec![sym("alt!!")]);
        let result = lst(vec![vecv(vec![sym("v")]), sym("body")]);
        let args = vec![
            form,
            Value::Nil,
            sym("c1"),
            result,
            kw("default"),
            Value::Int(42),
        ];
        let exp = native_alt_bang_block(&mut eval, &args).expect("alt!! expands");
        assert_eq!(
            pr_exp(&exp),
            "(clojure.core/let [G__1 c1 [val__4__auto__ ch2 :as ret3] \
             (clojure.core.async/alts!! [G__1] :default 42)] \
             (clojure.core/cond (clojure.core/or (clojure.core/= ch2 G__1)) \
             (clojure.core/let [[v] ret3] body) \
             (clojure.core/= ch2 :default) val__4__auto__))"
        );
    }

    /// Odd `alt!` clauses fail the `do-alt` assert (the expansion
    /// wrap blames the operator — pinned at the Clojure level).
    #[test]
    fn do_alt_unbalanced_asserts() {
        let mut eval = scratch_async();
        let err = do_alt(&mut eval, "alts!!", &[sym("c1")]).expect_err("unbalanced assert");
        assert_eq!(err.class, ThrowClass::AssertionError);
        assert_eq!(
            err.message,
            "Assert failed: unbalanced clauses\n(even? (count clauses))"
        );
    }

    /// The block macros take `(&form, &env)` plus user args; short
    /// calls are the macro's own arity (the wrap strips the two
    /// hidden params).
    #[test]
    fn block_macro_arities() {
        let mut eval = scratch_async();
        let err = native_go(&mut eval, &[sym("go")]).expect_err("arity");
        assert_eq!(err.class, ThrowClass::ArityException);
        assert!(
            err.message.ends_with("passed to: clojure.core.async/go"),
            "{}",
            err.message
        );
        let err = native_go_loop(&mut eval, &[sym("go-loop"), Value::Nil]).expect_err("arity");
        assert_eq!(err.class, ThrowClass::ArityException);
        assert!(
            err.message
                .ends_with("passed to: clojure.core.async/go-loop"),
            "{}",
            err.message
        );
        let err = alt_impl(
            &mut eval,
            &[sym("alt!")],
            "clojure.core.async/alt!",
            "alts!",
        )
        .expect_err("arity");
        assert_eq!(err.class, ThrowClass::ArityException);
    }

    /// Every caught go-body shape maps (values pass, throws pass
    /// through, escaping `recur`s and Rust panics become synthetic
    /// `Error`s — the channel still closes).
    #[test]
    fn go_outcome_maps_every_caught_shape() {
        assert!(matches!(
            go_outcome(Ok(Ok(Value::Int(1)))),
            Ok(Value::Int(1))
        ));
        let thrown = Throwable {
            id: 7,
            class: ThrowClass::ExceptionInfo,
            message: Some(std::sync::Arc::from("boom")),
            data: None,
            cause: None,
        };
        assert!(matches!(
            go_outcome(Ok(Err(Signal::Throw(thrown)))),
            Err(t) if t.id == 7
        ));
        let recur = go_outcome(Ok(Err(Signal::Recur(vec![])))).expect_err("recur maps");
        assert_eq!(recur.class, ThrowClass::Error);
        let panicked: Box<dyn std::any::Any + Send> = Box::new("bang");
        let panic = go_outcome(Err(panicked)).expect_err("panic maps");
        assert_eq!(panic.class, ThrowClass::Error);
        assert_eq!(panic.message.as_deref(), Some("bang"));
        let stringed: Box<dyn std::any::Any + Send> = Box::new("bang".to_owned());
        let panic = go_outcome(Err(stringed)).expect_err("string panic maps");
        assert_eq!(panic.message.as_deref(), Some("bang"));
        let opaque: Box<dyn std::any::Any + Send> = Box::new(42u32);
        let fallback = go_outcome(Err(opaque)).expect_err("fallback maps");
        assert_eq!(fallback.message.as_deref(), Some("go block panicked"));
    }

    /// `go` spawn failure reports the JVM's native-thread text.
    #[test]
    fn go_spawn_failed_shape() {
        let err = go_spawn_failed();
        assert_eq!(err.class, ThrowClass::RuntimeException);
        assert_eq!(err.message, "unable to create native thread");
    }

    /// `go_deliver` lands the result on a buffer-1 channel.
    #[test]
    fn go_deliver_puts_one() {
        let buf = BufferDesc::new(clj_value::BufferKind::Fixed, 1, true);
        let ch = Channel::new(Some(&buf));
        go_deliver(&ch, Value::Int(9));
        let waiter = Arc::new(Waiter::new());
        let taken = ch.take_one(&waiter, 0).expect("take");
        assert!(matches!(taken, OpReady::Ready(Some(ref v)) if *v == Value::Int(9)));
    }

    /// `:keys` entries bind their name part; `&env` reads bare
    /// symbols only.
    #[test]
    fn short_names_and_env_locals() {
        let keys = vecv(vec![sym("a"), sym_q("n", "b"), Value::Int(1)]);
        assert_eq!(short_names(&keys), vec!["a".to_owned(), "b".to_owned()]);
        assert!(short_names(&sym("x")).is_empty());
        let env = new_map()
            .insert(sym("x"), Value::Bool(true))
            .insert(sym_q("n", "y"), Value::Bool(true));
        let locals = env_locals(&Value::Map(env));
        assert!(locals.contains("x"));
        assert!(!locals.contains("y"));
        assert!(env_locals(&Value::Nil).is_empty());
    }
}
