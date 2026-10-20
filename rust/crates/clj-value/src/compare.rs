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
        _ => Err(CompareError {
            left: type_name(a),
            right: type_name(b),
        }),
    }
}

/// `Numbers.compare` over our numeric pairs (`None` unless both are
/// numbers). `Int`/`Int` stays exact; anything touching `Double`
/// compares as `f64`, like the combined ops.
fn compare_numbers(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Int(x), Value::Double(y)) => Some(num_cmp(*x as f64, *y)),
        (Value::Double(x), Value::Int(y)) => Some(num_cmp(*x, *y as f64)),
        (Value::Double(x), Value::Double(y)) => Some(num_cmp(*x, *y)),
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
/// The items when `v` is a vector or subvector (`None` otherwise).
fn vector_items(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Vector(x) => Some(x.iter().cloned().collect()),
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
        Value::Int(_) => "Long",
        Value::Double(_) => "Double",
        Value::Str(_) => "String",
        Value::Char(_) => "Character",
        Value::Symbol(_) => "Symbol",
        Value::Keyword(_) => "Keyword",
        Value::Extension(e) => extension_type_name(e),
        _ => ref_or_collection_type_name(v),
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
        _ => collection_type_name(v),
    }
}

/// The extension's own class name. [`CompareError`] borrows its
/// names, so the owned name is leaked — compare errors are
/// exceptional, never hot-loop values.
fn extension_type_name(e: &std::sync::Arc<dyn crate::ExtensionValue>) -> &'static str {
    Box::leak(e.class_name().into_boxed_str())
}

fn collection_type_name(v: &Value) -> &'static str {
    match v {
        Value::List(_) => "List",
        Value::Vector(_) => "Vector",
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
}
