// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slices 1–2: the `clojure.core.async` namespace —
//! channels plus blocking ops (`chan`, `buffer`/`dropping-buffer`/
//! `sliding-buffer`, `>!!`, `<!!`, `close!`, `closed?`, `alts!!`),
//! with slice 2's channel transducers (`chan` xform/ex-handler
//! arities, stepping on the put path, completion on close and
//! closed takes). Go blocks and parking ops (`go`, `>!`, `<!`,
//! `alt!`, `alts!`) are a later slice; the parking fns exist as
//! stubs that fail exactly like the JVM's (`Assert failed: … used
//! not in (go ...) block`).
//!
//! The namespace installs lazily on `require` (see
//! [`install_async_ns`], called from `load_one`), so boot is
//! untouched and unqualified use without a require fails with the
//! stock "no such namespace", like the JVM without the require.
//! Oracle: `docs/gap-123/g123-*.clj`; pins:
//! `docs/gap-123/probes.md`, `docs/gap-123/slice-2.md`.
//!
//! Documented deviations (D1–D7 in `slice-2.md`): `closed?` is
//! public here (the JVM keeps it in `impl.protocols` — the slice
//! requires it); a non-number non-buffer `chan` argument fails
//! eagerly with the `:full?` protocol error (the JVM stores it
//! and fails on first use); the default ex-handler prints one
//! line (the JVM prints the full trace); puts step at arrival,
//! so a filtered put behind a full buffer completes at once and
//! an aborted put still ran its transducer (same values, only
//! timing differs); a close racing a slow step wins (the JVM
//! holds its mutex across the step).

use std::sync::Arc;

use num_bigint::Sign;
use num_traits::ToPrimitive;
use rand::seq::SliceRandom;

use clj_value::{
    BufferDesc, BufferKind, ChanError, Channel, Keyword, MAX_QUEUE_SIZE, OpReady, StepOut, StepReq,
    ThrowClass, Throwable, Value, Vector, Waiter, as_buffer, as_channel, jvm_class_of, seq_vec,
};

use crate::core::{as_reduced, cce_err};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::fx4_form::iseq_err;
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

/// The slice-2 parking stubs (called outside `go` — the JVM's
/// `assert nil` text, oracle `e-bang-outside-go`).
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
];

/// Install the `clojure.core.async` namespace (idempotent —
/// reinstalling over itself is a `:reload` no-op).
pub(crate) fn install_async_ns(eval: &mut Evaluator) {
    eval.registry.ensure_ns(ASYNC_NS);
    for (name, f) in ASYNC_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(ASYNC_NS, name, func);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
