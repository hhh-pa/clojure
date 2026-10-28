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
//! type; anything else is [`CompareError::Incomparable`]. The error
//! message is ours (stable): the JVM's `ClassCastException` text
//! embeds JDK module verbiage that varies by release.

use std::cmp::Ordering;
use std::fmt;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::ToPrimitive;

use crate::{SubVectorValue, Value};

/// A failed comparison: the JVM throws `ClassCastException` here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompareError {
    left: &'static str,
    right: &'static str,
}

impl CompareError {
    /// The stable message (`Cannot compare <A> to <B>`).
    #[must_use]
    pub fn message(&self) -> String {
        format!("Cannot compare {} to {}", self.left, self.right)
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
        _ => Err(CompareError {
            left: type_name(a),
            right: type_name(b),
        }),
    }
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
            Some(d.compare(&crate::BigDec::from_bigint(n.clone())))
        }
        (Value::BigInt(n) | Value::BigInteger(n), Value::BigDec(e)) => {
            Some(crate::BigDec::from_bigint(n.clone()).compare(e))
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
        Value::BigInt(n) | Value::BigInteger(n) => Some(BigRational::from_integer(n.clone())),
        Value::BigDec(d) => Some(d.to_rational()),
        Value::Ratio(r) => Some(r.clone()),
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
        Value::MapEntry { key, val } => Some(vec![(**key).clone(), (**val).clone()]),
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

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Bool(_) => "Boolean",
        Value::Int(_) | Value::Double(_) => scalar_type_name(v),
        Value::BigInt(_) | Value::BigInteger(_) | Value::BigDec(_) | Value::Ratio(_) => {
            tower_type_name(v)
        }
        Value::Str(_) => "String",
        Value::Char(_) => "Character",
        Value::Symbol(_) => "Symbol",
        Value::Keyword(_) => "Keyword",
        Value::Uuid(_) | Value::Instant(_) | Value::Uri(_) => scalar_type_name(v),
        Value::Extension(e) => extension_type_name(e),
        _ => port_type_name(v).unwrap_or_else(|| ref_or_collection_type_name(v)),
    }
}

/// Scalar number and S9 type names (split from `type_name` for
/// the complexity budget).
fn scalar_type_name(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "Long",
        Value::Uuid(_) => "UUID",
        Value::Instant(_) => "Instant",
        Value::Uri(_) => "URI",
        _ => "Double",
    }
}

/// Tower number type names (split from `type_name` for the
/// complexity budget).
fn tower_type_name(v: &Value) -> &'static str {
    match v {
        Value::BigInt(_) => "BigInt",
        Value::BigInteger(_) => "BigInteger",
        Value::BigDec(_) => "BigDecimal",
        _ => "Ratio",
    }
}

/// Port-defined type names (split from `type_name` for the
/// complexity budget): host classes leak their per-value name;
/// multifns report the JVM class (static — no per-value name
/// leaks here); anything else is `None` for the caller.
fn port_type_name(v: &Value) -> Option<&'static str> {
    match v {
        Value::HostType(h) => Some(host_type_name(h)),
        Value::MultiFn(_) => Some("clojure.lang.MultiFn"),
        _ => None,
    }
}

/// Identity and collection type names (split from [`type_name`] for
/// the complexity budget).
fn ref_or_collection_type_name(v: &Value) -> &'static str {
    match v {
        Value::Var(_) => "Var",
        Value::Unbound(_) => "Unbound",
        Value::Throwable(_) => "Throwable",
        Value::Transient(_) => "Transient",
        Value::Atom(_) => "Atom",
        Value::Delay(_) => "Delay",
        Value::Future(_) => "Future",
        Value::Promise(_) => "Promise",
        Value::Agent(_) => "Agent",
        Value::Volatile(_) => "Volatile",
        _ => collection_type_name(v),
    }
}

/// The extension's own class name. [`CompareError`] borrows its
/// names, so the owned name is leaked — compare errors are
/// exceptional, never hot-loop values.
fn extension_type_name(e: &std::sync::Arc<dyn crate::ExtensionValue>) -> &'static str {
    Box::leak(e.class_name().into_boxed_str())
}

/// The host type's class name. [`CompareError`] borrows its names,
/// so the owned name is leaked — compare errors are exceptional,
/// never hot-loop values (same contract as [`extension_type_name`]).
fn host_type_name(h: &std::sync::Arc<crate::HostVal>) -> &'static str {
    Box::leak(h.type_name().to_owned().into_boxed_str())
}

fn collection_type_name(v: &Value) -> &'static str {
    match v {
        Value::List(_) => "List",
        // Entries and primitive vectors compare (and name) as vectors.
        Value::Vector(_) | Value::MapEntry { .. } | Value::PrimitiveVector(_) => "Vector",
        // Arrays name by descriptor (leaked, like extensions — compare
        // errors are exceptional, never hot-loop values).
        Value::Array(a) => Box::leak(a.descriptor().into_boxed_str()),
        Value::Map(_) => "Map",
        Value::Set(_) => "Set",
        Value::Regex(_) => "Regex",
        Value::TaggedLiteral { .. } => "TaggedLiteral",
        Value::ReaderConditional { .. } => "ReaderConditional",
        _ => unreachable!("atoms handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::{collection_type_name, compare, type_name};
    use crate::{
        Agent, Atom, Delay, Future, Promise, Symbol, Transient, TransientKind, TransientVector,
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

    /// Numeric type names, incl the S2 `BigInteger` kind.
    #[test]
    fn numeric_type_names() {
        use num_bigint::BigInt;
        assert_eq!(type_name(&Value::Int(1)), "Long");
        assert_eq!(type_name(&Value::Double(1.0)), "Double");
        assert_eq!(type_name(&Value::BigInt(BigInt::from(1))), "BigInt");
        assert_eq!(type_name(&Value::BigInteger(BigInt::from(1))), "BigInteger");
        assert_eq!(
            type_name(&Value::BigDec(crate::BigDec::from_bigint(BigInt::from(1)))),
            "BigDecimal"
        );
        assert_eq!(
            type_name(&Value::Ratio(num_rational::BigRational::new(
                BigInt::from(1),
                BigInt::from(2)
            ))),
            "Ratio"
        );
        assert_eq!(
            message_of(
                &Value::BigInteger(BigInt::from(1)),
                &Value::Str(Arc::from("s"))
            ),
            "Cannot compare BigInteger to String"
        );
    }

    #[test]
    fn display_matches_message() {
        let err = compare(&Value::Int(1), &Value::Str(Arc::from("s")))
            .expect_err("expected incomparable");
        assert_eq!(err.message(), "Cannot compare Long to String");
        assert_eq!(err.to_string(), "Cannot compare Long to String");
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
        let big = Value::BigInt(num_bigint::BigInt::from(5));
        let dec = Value::BigDec(crate::BigDec::parse("5.5").unwrap());
        let half = Value::Ratio(num_rational::BigRational::new(
            num_bigint::BigInt::from(1),
            num_bigint::BigInt::from(2),
        ));
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
        let huge = Value::BigDec(crate::BigDec::parse("1E+2147483648").unwrap());
        let zero = Value::BigDec(crate::BigDec::parse("0").unwrap());
        let one = Value::BigDec(crate::BigDec::parse("1.5").unwrap());
        assert_eq!(compare(&huge, &zero), Ok(Ordering::Greater));
        assert_eq!(compare(&zero, &huge), Ok(Ordering::Less));
        assert_eq!(compare(&one, &Value::Int(1)), Ok(Ordering::Greater));
        assert_eq!(compare(&Value::Int(2), &one), Ok(Ordering::Greater));
        assert_eq!(
            compare(&one, &Value::BigInt(num_bigint::BigInt::from(2))),
            Ok(Ordering::Less)
        );
        assert_eq!(
            compare(&Value::BigInt(num_bigint::BigInt::from(1)), &one),
            Ok(Ordering::Less)
        );
    }

    /// Tower type names surface in the incomparable error.
    #[test]
    fn tower_type_names() {
        let s = Value::Str(Arc::from("s"));
        let big = Value::BigInt(num_bigint::BigInt::from(5));
        let dec = Value::BigDec(crate::BigDec::parse("5.5").unwrap());
        let half = Value::Ratio(num_rational::BigRational::new(
            num_bigint::BigInt::from(1),
            num_bigint::BigInt::from(2),
        ));
        assert_eq!(message_of(&big, &s), "Cannot compare BigInt to String");
        assert_eq!(message_of(&dec, &s), "Cannot compare BigDecimal to String");
        assert_eq!(message_of(&half, &s), "Cannot compare Ratio to String");
    }

    #[test]
    fn scalar_type_names() {
        let s = Value::Str(Arc::from("s"));
        assert_eq!(
            message_of(&Value::Double(1.0), &s),
            "Cannot compare Double to String"
        );
        assert_eq!(
            message_of(&s, &Value::Double(1.0)),
            "Cannot compare String to Double"
        );
        assert_eq!(
            message_of(&Value::Char('a'), &Value::Int(1)),
            "Cannot compare Character to Long"
        );
    }

    #[test]
    fn ref_type_names() {
        let n = Value::Int(1);
        assert_eq!(
            message_of(&transient_value(), &n),
            "Cannot compare Transient to Long"
        );
        assert_eq!(
            message_of(&Value::Atom(Atom::new(Value::Nil)), &n),
            "Cannot compare Atom to Long"
        );
        assert_eq!(
            message_of(&Value::Delay(Delay::new(Value::Nil)), &n),
            "Cannot compare Delay to Long"
        );
        assert_eq!(
            message_of(&Value::Future(Future::new()), &n),
            "Cannot compare Future to Long"
        );
        assert_eq!(
            message_of(&Value::Promise(Promise::new()), &n),
            "Cannot compare Promise to Long"
        );
        assert_eq!(
            message_of(&Value::Agent(Agent::new(Value::Nil)), &n),
            "Cannot compare Agent to Long"
        );
    }

    #[test]
    fn reader_type_names() {
        let n = Value::Int(1);
        let tagged = Value::TaggedLiteral {
            tag: Symbol::new(None, "t"),
            form: Box::new(Value::Nil),
        };
        assert_eq!(
            message_of(&tagged, &n),
            "Cannot compare TaggedLiteral to Long"
        );
        let cond = Value::ReaderConditional {
            form: Box::new(Value::Nil),
            splicing: false,
        };
        assert_eq!(
            message_of(&cond, &n),
            "Cannot compare ReaderConditional to Long"
        );
    }

    #[test]
    fn defensive_names_coverable_directly() {
        // `type_name(Nil)` never fires through `compare` (nil
        // pairs decide early), and the collection wildcard is
        // exhaustive-by-construction — both pinned directly.
        assert_eq!(type_name(&Value::Nil), "nil");
    }

    #[test]
    #[should_panic(expected = "atoms handled above")]
    fn collection_wildcard_panics() {
        let _ = collection_type_name(&Value::Int(1));
    }

    #[test]
    fn multifn_names_its_class() {
        // Multimethods report the JVM class in compare errors
        // (unreachable from Clojure — `sort` uses its own
        // comparator — so pinned directly).
        let mf = Value::MultiFn(Arc::new(crate::MultiFn {
            name: Arc::from("m"),
            dispatch: Value::Nil,
            default: Value::Nil,
            methods: crate::new_map(),
            prefers: crate::new_map(),
            hierarchy: None,
        }));
        assert_eq!(type_name(&mf), "clojure.lang.MultiFn");
        assert_eq!(
            message_of(&mf, &Value::Int(1)),
            "Cannot compare clojure.lang.MultiFn to Long"
        );
    }

    #[test]
    fn host_values_name_their_class() {
        // Port-defined types report their class name in compare
        // errors (like the JVM's `ClassCastException` operands).
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
        assert_eq!(type_name(&v), "user.T");
        assert_eq!(
            message_of(&v, &Value::Int(1)),
            "Cannot compare user.T to Long"
        );
    }
}
