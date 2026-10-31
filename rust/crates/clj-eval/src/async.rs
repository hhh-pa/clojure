// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-123 slice 1: the `clojure.core.async` namespace — channels
//! plus blocking ops only (`chan`, `buffer`/`dropping-buffer`/
//! `sliding-buffer`, `>!!`, `<!!`, `close!`, `closed?`, `alts!!`).
//! Go blocks and parking ops (`go`, `>!`, `<!`, `alt!`, `alts!`)
//! are slice 2; the parking fns exist as stubs that fail exactly
//! like the JVM's (`Assert failed: … used not in (go ...) block`).
//!
//! The namespace installs lazily on `require` (see
//! [`install_async_ns`], called from `load_one`), so boot is
//! untouched and unqualified use without a require fails with the
//! stock "no such namespace", like the JVM without the require.
//! Oracle: `docs/gap-123/g123-*.clj`; pins:
//! `docs/gap-123/probes.md`.
//!
//! Two documented deviations: `closed?` is public here (the JVM
//! keeps it in `impl.protocols` — the slice requires it), and a
//! non-number non-buffer `chan` argument fails eagerly with the
//! `:full?` protocol error (the JVM stores it and fails on first
//! use). Transducers are refused with an explicit unsupported
//! error (slice 2).

use std::sync::Arc;

use num_bigint::Sign;
use num_traits::ToPrimitive;
use rand::seq::SliceRandom;

use clj_value::{
    BufferDesc, BufferKind, ChanError, Channel, Keyword, MAX_QUEUE_SIZE, OpReady, ThrowClass,
    Value, Vector, Waiter, as_buffer, as_channel, jvm_class_of, seq_vec,
};

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::fx4_form::iseq_err;
use crate::ns::{NativeFnPtr, check_arity, str_of};
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
/// `NoSuchElementException` with a nil message).
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

/// The slice-2 refusal (transducers on channels).
fn xform_unsupported() -> NativeError {
    NativeError::new(
        ThrowClass::UnsupportedOperationException,
        "unsupported: channels with transducers (gap-123 slice 2)".to_owned(),
    )
}

fn native_chan(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/chan", args, &[0, 1, 2, 3])?;
    let buf_or_n = args.first().unwrap_or(&Value::Nil);
    if args.get(1).is_some_and(Value::is_truthy) {
        if matches!(buf_or_n, Value::Nil) {
            return Err(assert_failed(
                "buffer must be supplied when transducer is",
                "buf-or-n",
            ));
        }
        return Err(xform_unsupported());
    }
    if let Some(buf) = as_buffer(buf_or_n) {
        return Ok(Value::Extension(Arc::new(Channel::new(Some(buf)))));
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
                let buf = BufferDesc::new(BufferKind::Fixed, ceil, integral);
                return Ok(Value::Extension(Arc::new(Channel::new(Some(&buf)))));
            }
            Err(_) => return Err(proto_err(buf_or_n, "full?", "Buffer")),
        }
    }
    Ok(Value::Extension(Arc::new(Channel::new(None))))
}

/// A parked-forever waiter result (the owner never waits after
/// killing — unreachable).
fn killed() -> NativeError {
    NativeError::new(
        ThrowClass::Error,
        "async waiter abandoned while parking".to_owned(),
    )
}

fn native_put_block(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
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
    match ch.put_one(val.clone(), &waiter, 0).map_err(chan_err)? {
        OpReady::Ready(done) => Ok(Value::Bool(done)),
        OpReady::Queued => waiter.wait().map(|(_, v)| v).ok_or_else(killed),
    }
}

fn native_take_block(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/<!!", args, &[1])?;
    let ch = as_chan(&args[0], "take!", "ReadPort")?;
    let waiter = Arc::new(Waiter::new());
    match ch.take_one(&waiter, 0).map_err(chan_err)? {
        OpReady::Ready(found) => Ok(found.unwrap_or(Value::Nil)),
        OpReady::Queued => waiter.wait().map(|(_, v)| v).ok_or_else(killed),
    }
}

fn native_close(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.async/close!", args, &[1])?;
    as_chan(&args[0], "close!", "Channel")?.close();
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

fn native_alts_block(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
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
    if let Some(won) = alts_try_ready(&ops, &order, &waiter)? {
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
                        return Ok(Some(alt_result(found.unwrap_or(Value::Nil), chv.clone())));
                    }
                    OpReady::Queued => {}
                }
            }
            AltOp::Put(chv, val) => {
                let ch = as_chan(chv, "put!", "WritePort")?;
                match ch.put_one(val.clone(), waiter, *idx).map_err(chan_err)? {
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
        assert!(
            err.message.contains("can't put nil on channel"),
            "{}",
            err.message
        );
    }
}
