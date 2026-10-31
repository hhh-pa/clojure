// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Ordering (S3, C2): a port of `Util.compare` + `Numbers.compare`
//! (`docs/t1174/probes/probe17.clj`).
//!
//! Numbers compare numerically across `Int`/`Double` (the JVM lt-pair,
//! so NaN equals everything and `-0.0` equals `0.0`); chars, bools,
//! strings, symbols, keywords, and vectors compare within their own
//! type; anything else fails with the JDK `ClassCastException`
//! text — the `Util.compare` checkcast (`Number` for numeric `k1`,
//! `Comparable` for non-`Comparable` `k1`) or the `compareTo`
//! checkcast of `k2` to `k1`'s class (#102).

use std::cmp::Ordering;
use std::fmt;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::ToPrimitive;

use crate::{SubVectorValue, Value};

/// A failed comparison: the JVM throws `ClassCastException` here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompareError {
    from: &'static str,
    to: &'static str,
}

impl CompareError {
    /// The JDK `ClassCastException` text for the failed checkcast
    /// (with module verbiage, via [`cce_message`](crate::cce_message)).
    #[must_use]
    pub fn message(&self) -> String {
        crate::cce_message(self.from, self.to)
    }

    /// The JVM exception class.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "java.lang.ClassCastException"
    }
}

impl fmt::Display for CompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for CompareError {}

/// `Util.compare`: total within comparable pairs, `Err` across them.
pub fn compare(a: &Value, b: &Value) -> Result<Ordering, CompareError> {
    if let Some(ord) = compare_numbers(a, b) {
        return Ok(ord);
    }
    if let (Some(x), Some(y)) = (vector_items(a), vector_items(b)) {
        return compare_item_slices(&x, &y);
    }
    match (a, b) {
        (Value::Nil, Value::Nil) => Ok(Ordering::Equal),
        (Value::Nil, _) => Ok(Ordering::Less),
        (_, Value::Nil) => Ok(Ordering::Greater),
        (Value::Char(x), Value::Char(y)) => Ok(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Ok(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Ok(utf16_cmp(x, y)),
        (Value::Symbol(x), Value::Symbol(y)) => Ok(compare_namespaced(
            x.namespace(),
            x.name(),
            y.namespace(),
            y.name(),
        )),
        (Value::Keyword(x), Value::Keyword(y)) => Ok(compare_namespaced(
            x.namespace(),
            x.name(),
            y.namespace(),
            y.name(),
        )),
        // S9 scalars order within their kind (signed UUID words,
        // instant millis, the JDK URI order's sign).
        (Value::Uuid(x), Value::Uuid(y)) => Ok(x.cmp_uuid(y)),
        (Value::Instant(x), Value::Instant(y)) => Ok(x.cmp_instant(y)),
        (Value::Uri(x), Value::Uri(y)) => Ok(x.compare_uri(y).cmp(&0)),
        _ => Err(compare_cast_err(a, b)),
    }
}

/// `Util.compare` on an incomparable pair: a numeric `k1`
/// checkcasts `k2` to `Number`; otherwise `((Comparable) k1)`
/// fails a non-`Comparable` `k1` against `Comparable`, and a
/// `Comparable` `k1` runs `compareTo`, whose first act is the
/// checkcast of `k2` to `k1`'s class (mirrors `clj-eval`'s
/// `compare_cast_err`, which pins the same texts through
/// `cce_err`).
fn compare_cast_err(a: &Value, b: &Value) -> CompareError {
    if is_number(a) {
        return CompareError {
            from: jvm_name(b),
            to: "java.lang.Number",
        };
    }
    match compare_to_target(a) {
        Some(target) => CompareError {
            from: jvm_name(b),
            to: target,
        },
        None => CompareError {
            from: jvm_name(a),
            to: "java.lang.Comparable",
        },
    }
}

/// A tower number (the `Util.compare` `instanceof Number` gate).
fn is_number(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(_)
            | Value::Double(_)
            | Value::BigInt(_)
            | Value::BigInteger(_)
            | Value::BigDec(_)
            | Value::Ratio(_)
    )
}

/// The `compareTo` checkcast target for a `Comparable` `k1`
/// (`None` for non-`Comparable`s, which fail against
/// `Comparable` instead): `String`/`Character`/`Boolean`/
/// `Symbol`/`Keyword` cast `k2` to their own class, the S9
/// scalars to theirs, and vectors (entries and primitive
/// vectors compare as vectors) to `IPersistentVector`, like
/// `APersistentVector.compareTo` — subvectors ride along.
fn compare_to_target(a: &Value) -> Option<&'static str> {
    match a {
        Value::Str(_) => Some("java.lang.String"),
        Value::Char(_) => Some("java.lang.Character"),
        Value::Bool(_) => Some("java.lang.Boolean"),
        Value::Keyword(_) => Some("clojure.lang.Keyword"),
        Value::Symbol(_) => Some("clojure.lang.Symbol"),
        Value::Uuid(_) => Some("java.util.UUID"),
        Value::Instant(_) => Some("java.util.Date"),
        Value::Uri(_) => Some("java.net.URI"),
        Value::Vector(_) | Value::MapEntry { .. } | Value::PrimitiveVector(_) => {
            Some("clojure.lang.IPersistentVector")
        }
        Value::Extension(e) => e
            .as_any()
            .downcast_ref::<SubVectorValue>()
            .map(|_| "clojure.lang.IPersistentVector"),
        _ => None,
    }
}

/// [`jvm_class_of`](crate::jvm_class_of), borrowed:
/// [`CompareError`] keeps `Copy`, so the owned name is leaked —
/// compare errors are exceptional, never hot-loop values.
fn jvm_name(v: &Value) -> &'static str {
    Box::leak(crate::jvm_class_of(v).into_boxed_str())
}

/// `Numbers.compare` over our numeric pairs (`None` unless both are
/// numbers). `Int`/`Int` stays exact; anything touching `Double`
/// compares as `f64`, like the combined ops; every other tower
/// pairing compares exactly over `BigRational`.
fn compare_numbers(a: &Value, b: &Value) -> Option<Ordering> {
    if matches!(a, Value::Double(_)) || matches!(b, Value::Double(_)) {
        let (x, y) = (to_f64_value(a)?, to_f64_value(b)?);
        return Some(num_cmp(x, y));
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        // `BigDec` against `BigDec`/`Int`/`BigInt` decides by sign
        // and magnitude first — astronomical scales never expand
        // (`compareTo` parity; `compare_exact` mirrors this for
        // `Num`).
        (Value::BigDec(d), Value::BigDec(e)) => Some(d.compare(e)),
        (Value::BigDec(d), Value::Int(i)) => {
            Some(d.compare(&crate::BigDec::from_bigint((*i).into())))
        }
        (Value::Int(i), Value::BigDec(e)) => {
            Some(crate::BigDec::from_bigint((*i).into()).compare(e))
        }
        (Value::BigDec(d), Value::BigInt(n) | Value::BigInteger(n)) => {
            Some(d.compare(&crate::BigDec::from_bigint(n.as_ref().clone())))
        }
        (Value::BigInt(n) | Value::BigInteger(n), Value::BigDec(e)) => {
            Some(crate::BigDec::from_bigint(n.as_ref().clone()).compare(e))
        }
        _ => {
            let (x, y) = (to_rational_value(a)?, to_rational_value(b)?);
            Some(x.cmp(&y))
        }
    }
}

/// An exact rational for a non-`Double` number (`None` for
/// non-numbers).
fn to_rational_value(v: &Value) -> Option<BigRational> {
    match v {
        Value::Int(i) => Some(BigRational::from_integer(BigInt::from(*i))),
        Value::BigInt(n) | Value::BigInteger(n) => {
            Some(BigRational::from_integer(n.as_ref().clone()))
        }
        Value::BigDec(d) => Some(d.to_rational()),
        Value::Ratio(r) => Some(r.as_ref().clone()),
        _ => None,
    }
}

/// An `f64` for any number (`None` for non-numbers).
fn to_f64_value(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Double(d) => Some(*d),
        Value::BigInt(n) | Value::BigInteger(n) => n.to_f64(),
        Value::BigDec(d) => Some(d.to_f64()),
        Value::Ratio(r) => r.to_f64(),
        _ => None,
    }
}

/// The JVM lt-pair: less, else greater, else equal — so NaN equals
/// everything numeric and `-0.0` equals `0.0` (probe17).
fn num_cmp(a: f64, b: f64) -> Ordering {
    if a < b {
        Ordering::Less
    } else if b < a {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// `Symbol.compareTo` (which `Keyword.compareTo` delegates to):
/// unqualified sorts before qualified, then namespace, then name.
fn compare_namespaced(
    a_ns: Option<&str>,
    a_name: &str,
    b_ns: Option<&str>,
    b_name: &str,
) -> Ordering {
    match (a_ns, b_ns) {
        (None, None) => utf16_cmp(a_name, b_name),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => utf16_cmp(x, y).then_with(|| utf16_cmp(a_name, b_name)),
    }
}

/// `String.compareTo`: lexicographic over UTF-16 units. Byte order
/// flips against it when a BMP char `>= U+E000` meets an astral
/// char (`\uFFFF` sorts after astral — reviewer probe).
fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Lexicographic element compare (element errors propagate, like the
/// JVM's recursive `compareTo`).
/// The items when `v` is a vector, subvector, map entry, or
/// primitive vector (an entry is its 2 items — `None`
/// otherwise).
fn vector_items(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Vector(x) => Some(x.iter().cloned().collect()),
        Value::MapEntry { key, val, .. } => Some(vec![(**key).clone(), (**val).clone()]),
        Value::PrimitiveVector(p) => Some(p.items()),
        Value::Extension(e) => e
            .as_any()
            .downcast_ref::<SubVectorValue>()
            .map(SubVectorValue::items),
        _ => None,
    }
}

fn compare_item_slices(a: &[Value], b: &[Value]) -> Result<Ordering, CompareError> {
    for (x, y) in a.iter().zip(b.iter()) {
        let ord = compare(x, y)?;
        if ord != Ordering::Equal {
            return Ok(ord);
        }
    }
    Ok(a.len().cmp(&b.len()))
}

#[cfg(test)]
mod tests {
    use super::compare;
    use crate::{
        Agent, Atom, Delay, Elem8, Future, Instant, Keyword, List, PrimitiveVector, Promise,
        RepeatValue, SubVectorValue, Symbol, Transient, TransientKind, TransientVector, Uri, Uuid,
        Value, Vector,
    };
    use std::cmp::Ordering;
    use std::sync::Arc;

    fn transient_value() -> Value {
        Value::Transient(Transient::new(
            TransientKind::Vector(TransientVector::new()),
        ))
    }

    fn message_of(a: &Value, b: &Value) -> String {
        compare(a, b).expect_err("expected incomparable").message()
    }

    /// A subvector extension over `[1]`.
    fn subvector_value() -> Value {
        Value::Extension(Arc::new(SubVectorValue::new(
            Vector::from_vec(vec![Value::Int(1)]),
            0,
            1,
        )))
    }

    /// A numeric `k1` against a non-number fails `k2` to `Number`
    /// (`Util.compare` checkcasts before `Numbers.compare`) — every
    /// tower kind, incl the S2 `BigInteger` kind.
    #[test]
    fn numeric_k1_fails_k2_to_number() {
        use num_bigint::BigInt;
        let s = Value::Str(Arc::from("s"));
        let want = "class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')";
        for k1 in [
            Value::Int(1),
            Value::Double(1.0),
            Value::BigInt(Arc::new(BigInt::from(1))),
            Value::BigInteger(Arc::new(BigInt::from(1))),
            Value::BigDec(Arc::new(crate::BigDec::from_bigint(BigInt::from(1)))),
            Value::Ratio(Arc::new(num_rational::BigRational::new(
                BigInt::from(1),
                BigInt::from(2),
            ))),
        ] {
            assert_eq!(message_of(&k1, &s), want);
        }
        let err = compare(&Value::Int(1), &s).expect_err("expected incomparable");
        assert_eq!(err.jvm_class(), "java.lang.ClassCastException");
    }

    #[test]
    fn display_matches_message() {
        let err = compare(&Value::Int(1), &Value::Str(Arc::from("s")))
            .expect_err("expected incomparable");
        let want = "class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')";
        assert_eq!(err.message(), want);
        assert_eq!(err.to_string(), want);
    }

    #[test]
    fn classes_are_never_comparable() {
        // The JVM's `Class` is not `Comparable` either: it fails
        // itself against `Comparable`.
        let class = || {
            Value::Class(Arc::new(crate::ClassVal::new(
                Arc::from("java.lang.String"),
                false,
                Arc::from([]),
            )))
        };
        assert_eq!(
            message_of(&class(), &class()),
            "class java.lang.Class cannot be cast to class java.lang.Comparable (java.lang.Class and java.lang.Comparable are in module java.base of loader 'bootstrap')"
        );
    }

    #[test]
    fn vectors_order_by_first_differing_element() {
        let one = Value::Vector(Vector::from_vec(vec![Value::Int(1)]));
        let two = Value::Vector(Vector::from_vec(vec![Value::Int(2)]));
        assert_eq!(compare(&one, &two), Ok(Ordering::Less));
        assert_eq!(compare(&two, &one), Ok(Ordering::Greater));
    }

    /// Tower pairs compare exactly (no `Double` involved) or as
    /// `f64` (a `Double` present) — every `to_rational_value` /
    /// `to_f64_value` tower arm walks here.
    #[test]
    fn tower_pairs_compare() {
        let big = Value::BigInt(Arc::new(num_bigint::BigInt::from(5)));
        let dec = Value::BigDec(Arc::new(crate::BigDec::parse("5.5").unwrap()));
        let half = Value::Ratio(Arc::new(num_rational::BigRational::new(
            num_bigint::BigInt::from(1),
            num_bigint::BigInt::from(2),
        )));
        assert_eq!(compare(&big, &Value::Int(3)), Ok(Ordering::Greater));
        assert_eq!(compare(&dec, &big), Ok(Ordering::Greater));
        assert_eq!(compare(&half, &Value::Int(1)), Ok(Ordering::Less));
        assert_eq!(compare(&Value::Double(0.5), &big), Ok(Ordering::Less));
        assert_eq!(compare(&dec, &Value::Double(5.0)), Ok(Ordering::Greater));
        assert_eq!(compare(&half, &Value::Double(1.0)), Ok(Ordering::Less));
    }

    /// `BigDec` pairings decide by sign and magnitude first:
    /// astronomical scales never expand (the `1E+2147483648`
    /// literal parses — scale `i32` — but comparing it must not
    /// build a two-billion-digit `pow10`).
    #[test]
    fn bigdec_pairs_skip_expansion() {
        let huge = Value::BigDec(Arc::new(crate::BigDec::parse("1E+2147483648").unwrap()));
        let zero = Value::BigDec(Arc::new(crate::BigDec::parse("0").unwrap()));
        let one = Value::BigDec(Arc::new(crate::BigDec::parse("1.5").unwrap()));
        assert_eq!(compare(&huge, &zero), Ok(Ordering::Greater));
        assert_eq!(compare(&zero, &huge), Ok(Ordering::Less));
        assert_eq!(compare(&one, &Value::Int(1)), Ok(Ordering::Greater));
        assert_eq!(compare(&Value::Int(2), &one), Ok(Ordering::Greater));
        assert_eq!(
            compare(&one, &Value::BigInt(Arc::new(num_bigint::BigInt::from(2)))),
            Ok(Ordering::Less)
        );
        assert_eq!(
            compare(&Value::BigInt(Arc::new(num_bigint::BigInt::from(1))), &one),
            Ok(Ordering::Less)
        );
    }

    /// `Comparable` scalar `k1` against an alien `k2` fails `k2`
    /// to `k1`'s class (the `compareTo` checkcast — the JDK
    /// `String`/`Character`/`Boolean`/`UUID`/`Date`/`URI`
    /// internals cast to their own class).
    #[test]
    fn comparable_scalars_fail_k2_to_own_class() {
        let n = Value::Int(1);
        for (k1, target) in [
            (Value::Str(Arc::from("s")), "java.lang.String"),
            (Value::Char('a'), "java.lang.Character"),
            (Value::Bool(true), "java.lang.Boolean"),
            (Value::Uuid(Uuid::from_bits(0, 1)), "java.util.UUID"),
            (Value::Instant(Instant::from_millis(0)), "java.util.Date"),
            (
                Value::Uri(Box::new(Uri::parse("http://h/x").unwrap())),
                "java.net.URI",
            ),
        ] {
            assert_eq!(
                message_of(&k1, &n),
                format!(
                    "class java.lang.Long cannot be cast to class {target} (java.lang.Long and {target} are in module java.base of loader 'bootstrap')"
                ),
                "{k1:?}"
            );
        }
        // A `Double` alien fails by its own class too.
        assert_eq!(
            message_of(&Value::Str(Arc::from("s")), &Value::Double(1.0)),
            "class java.lang.Double cannot be cast to class java.lang.String (java.lang.Double and java.lang.String are in module java.base of loader 'bootstrap')"
        );
    }

    /// `Keyword.compareTo` casts to `Keyword` and
    /// `Symbol.compareTo` to `Symbol` (upstream sources).
    #[test]
    fn comparable_nameds_fail_k2_to_own_class() {
        let kw = Value::Keyword(Keyword::new(None, "a"));
        let sym = Value::Symbol(Symbol::new(None, "a"));
        assert_eq!(
            message_of(&kw, &sym),
            "class clojure.lang.Symbol cannot be cast to class clojure.lang.Keyword (clojure.lang.Symbol and clojure.lang.Keyword are in unnamed module of loader 'app')"
        );
        assert_eq!(
            message_of(&sym, &kw),
            "class clojure.lang.Keyword cannot be cast to class clojure.lang.Symbol (clojure.lang.Keyword and clojure.lang.Symbol are in unnamed module of loader 'app')"
        );
    }

    /// Non-`Comparable` cells fail themselves against
    /// `Comparable` (`((Comparable) k1)`).
    #[test]
    fn cells_fail_against_comparable() {
        let n = Value::Int(1);
        for (k1, from) in [
            (
                transient_value(),
                "clojure.lang.PersistentVector$TransientVector",
            ),
            (Value::Atom(Atom::new(Value::Nil)), "clojure.lang.Atom"),
            (Value::Delay(Delay::new(Value::Nil)), "clojure.lang.Delay"),
            (
                Value::Future(Future::new()),
                "clojure.core$future_call$reify",
            ),
            (Value::Promise(Promise::new()), "clojure.core$promise$reify"),
            (Value::Agent(Agent::new(Value::Nil)), "clojure.lang.Agent"),
            (Value::Ref(crate::Ref::new(Value::Nil)), "clojure.lang.Ref"),
        ] {
            assert_eq!(
                message_of(&k1, &n),
                format!(
                    "class {from} cannot be cast to class java.lang.Comparable ({from} is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
                ),
                "{k1:?}"
            );
        }
    }

    #[test]
    fn reader_forms_fail_against_comparable() {
        let n = Value::Int(1);
        let tagged = crate::tagged_literal(Symbol::new(None, "t"), Value::Nil);
        assert_eq!(
            message_of(&tagged, &n),
            "class clojure.lang.TaggedLiteral cannot be cast to class java.lang.Comparable (clojure.lang.TaggedLiteral is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
        let cond = crate::reader_conditional(Value::Nil, false);
        assert_eq!(
            message_of(&cond, &n),
            "class clojure.lang.ReaderConditional cannot be cast to class java.lang.Comparable (clojure.lang.ReaderConditional is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
    }

    #[test]
    fn multifn_fails_against_comparable() {
        // Multimethods fail their JVM class (unreachable from
        // Clojure — `sort` uses its own comparator — so pinned
        // directly).
        let mf = Value::MultiFn(Arc::new(crate::MultiFn {
            name: Arc::from("m"),
            dispatch: Value::Nil,
            default: Value::Nil,
            methods: crate::new_map(),
            prefers: crate::new_map(),
            hierarchy: None,
        }));
        assert_eq!(
            message_of(&mf, &Value::Int(1)),
            "class clojure.lang.MultiFn cannot be cast to class java.lang.Comparable (clojure.lang.MultiFn is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
    }

    #[test]
    fn host_values_fail_against_comparable() {
        // Port-defined types fail their class name (like the
        // JVM's `ClassCastException` operands).
        let def = Arc::new(crate::HostDef {
            name: Arc::from("user.T"),
            kind: crate::HostKind::Type,
            fields: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
            struct_keys: Vec::new(),
        });
        let v = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def,
            content: crate::new_map(),
            meta: None,
            proxy_map: None,
        })));
        assert_eq!(
            message_of(&v, &Value::Int(1)),
            "class user.T cannot be cast to class java.lang.Comparable (user.T is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
    }

    /// Vectors, entries, primitive vectors, and subvectors fail an
    /// alien `k2` to `IPersistentVector` (like
    /// `APersistentVector.compareTo`).
    #[test]
    fn vector_family_fails_k2_to_ipersistentvector() {
        let n = Value::Int(1);
        let want = "class java.lang.Long cannot be cast to class clojure.lang.IPersistentVector (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')";
        for k1 in [
            Value::Vector(Vector::from_vec(vec![Value::Int(1)])),
            crate::map_entry(Value::Int(1), Value::Int(2)),
            Value::PrimitiveVector(PrimitiveVector::new(Elem8::Long, vec![Value::Int(1)], None)),
            subvector_value(),
        ] {
            assert_eq!(message_of(&k1, &n), want);
        }
    }

    /// Non-subvector extensions are not `Comparable`: they fail
    /// themselves (here a `Repeat` against `Comparable`).
    #[test]
    fn plain_extensions_fail_against_comparable() {
        let rep = Value::Extension(Arc::new(RepeatValue::new(Value::Int(1), Some(2))));
        assert_eq!(
            message_of(&rep, &Value::Int(1)),
            "class clojure.lang.Repeat cannot be cast to class java.lang.Comparable (clojure.lang.Repeat is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
    }

    /// Lists fail themselves both ways (a list `k2` against a
    /// vector `k1` fails to `IPersistentVector` instead).
    #[test]
    fn lists_fail_against_comparable() {
        let list = Value::List(List::from_vec(vec![Value::Int(1)]));
        let vec = Value::Vector(Vector::from_vec(vec![Value::Int(1)]));
        assert_eq!(
            message_of(&list, &vec),
            "class clojure.lang.PersistentList cannot be cast to class java.lang.Comparable (clojure.lang.PersistentList is in unnamed module of loader 'app'; java.lang.Comparable is in module java.base of loader 'bootstrap')"
        );
        assert_eq!(
            message_of(&vec, &list),
            "class clojure.lang.PersistentList cannot be cast to class clojure.lang.IPersistentVector (clojure.lang.PersistentList and clojure.lang.IPersistentVector are in unnamed module of loader 'app')"
        );
    }
}
