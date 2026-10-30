// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! JVM class names for values ([`jvm_class_of`]). Shared home
//! (moved from `clj-eval` for #102): `compare` formats its
//! `ClassCastException` through the same table the evaluator's
//! cast failures use, so the two can never drift.

use crate::{HostKind, HostVal, Map, Set, Value};

/// The JVM class name of a value: cast sources, arity names, and
/// `ISeq` errors (§E8). Collection names are size-dependent like
/// the JVM's (`AFn.name = getClass().getName()`; maps mirror
/// `canBePAM` exactly over insertion order).
#[must_use]
pub fn jvm_class_of(v: &Value) -> String {
    match v {
        Value::Nil => "null".to_owned(),
        Value::Bool(_) => "java.lang.Boolean".to_owned(),
        Value::Int(_) => "java.lang.Long".to_owned(),
        Value::Double(_) => "java.lang.Double".to_owned(),
        Value::BigInt(_) | Value::BigInteger(_) | Value::BigDec(_) | Value::Ratio(_) => {
            tower_class_of(v)
        }
        Value::Str(_) => "java.lang.String".to_owned(),
        Value::Char(_) => "java.lang.Character".to_owned(),
        Value::Symbol(_) | Value::Keyword(_) => named_class_of(v),
        Value::Uuid(_) | Value::Instant(_) | Value::Uri(_) => scalar_class_of(v),
        Value::Vector(_) => "clojure.lang.PersistentVector".to_owned(),
        Value::MapEntry { .. } => "clojure.lang.MapEntry".to_owned(),
        Value::Map(m) => map_class_of(m).to_owned(),
        Value::Set(s) => set_class_of(s).to_owned(),
        other => jvm_class_of_ref(other),
    }
}

/// `Symbol`/`Keyword` class names. Chained so each match stays
/// under the complexity gate.
fn named_class_of(v: &Value) -> String {
    match v {
        Value::Symbol(_) => "clojure.lang.Symbol".to_owned(),
        _ => "clojure.lang.Keyword".to_owned(),
    }
}

/// S9 scalar class names (split from `jvm_class_of` for the
/// complexity budget).
fn scalar_class_of(v: &Value) -> String {
    match v {
        Value::Uuid(_) => "java.util.UUID".to_owned(),
        Value::Instant(_) => "java.util.Date".to_owned(),
        _ => "java.net.URI".to_owned(),
    }
}

/// Tower class names. Chained so each match stays under the
/// complexity gate.
fn tower_class_of(v: &Value) -> String {
    match v {
        Value::BigInt(_) => "clojure.lang.BigInt".to_owned(),
        Value::BigInteger(_) => "java.math.BigInteger".to_owned(),
        Value::BigDec(_) => "java.math.BigDecimal".to_owned(),
        _ => "clojure.lang.Ratio".to_owned(),
    }
}

/// Sorted sets print as tree sets, the rest as hash sets.
/// Shared with the print driver's dispatch tokens (single source).
#[must_use]
pub fn set_class_of(s: &Set) -> &'static str {
    if s.is_sorted() {
        "clojure.lang.PersistentTreeSet"
    } else {
        "clojure.lang.PersistentHashSet"
    }
}

/// Lists, reader forms, and vars. Chained so each match stays under
/// the complexity gate.
fn jvm_class_of_ref(v: &Value) -> String {
    match v {
        Value::List(l) if l.is_empty() => "clojure.lang.PersistentList$EmptyList".to_owned(),
        Value::List(_) => "clojure.lang.PersistentList".to_owned(),
        Value::Regex(_) => "java.util.regex.Pattern".to_owned(),
        Value::TaggedLiteral { .. } => "clojure.lang.TaggedLiteral".to_owned(),
        Value::ReaderConditional { .. } => "clojure.lang.ReaderConditional".to_owned(),
        Value::Var(_) => "clojure.lang.Var".to_owned(),
        Value::Unbound(_) => "clojure.lang.Var$Unbound".to_owned(),
        Value::Throwable(t) => t.class.jvm_name().to_owned(),
        other => jvm_class_of_host(other),
    }
}

/// Host and reference objects.
/// The JVM class of a cell value (split from [`jvm_class_of_host`]
/// for the complexity budget).
fn cell_jvm_class(v: &Value) -> &'static str {
    match v {
        Value::Transient(t) => t.jvm_class(),
        Value::Atom(a) => a.jvm_class(),
        Value::Delay(d) => d.jvm_class(),
        Value::Future(f) => f.jvm_class(),
        Value::Promise(p) => p.jvm_class(),
        Value::Agent(a) => a.jvm_class(),
        Value::Volatile(x) => x.jvm_class(),
        Value::Ref(r) => r.jvm_class(),
        Value::Executor(x) => x.jvm_class(),
        _ => unreachable!("cell partition"),
    }
}

fn jvm_class_of_host(v: &Value) -> String {
    match v {
        Value::Transient(_)
        | Value::Atom(_)
        | Value::Delay(_)
        | Value::Future(_)
        | Value::Promise(_)
        | Value::Agent(_)
        | Value::Volatile(_)
        | Value::Ref(_)
        | Value::Executor(_) => cell_jvm_class(v).to_owned(),
        Value::Extension(e) => e.class_name(),
        Value::HostType(h) => jvm_class_of_hosttype(h),
        // Multimethods report the JVM class (S-typechain stage 3).
        Value::MultiFn(_) => "clojure.lang.MultiFn".to_owned(),
        // Classes report `java.lang.Class` (S16).
        Value::Class(_) => "java.lang.Class".to_owned(),
        // Arrays report their descriptor (S15: `[I`, `[[I`,
        // `[Ljava.lang.Object;`, …).
        Value::Array(a) => a.descriptor(),
        // Primitive vectors report the JVM class (S15).
        Value::PrimitiveVector(_) => "clojure.core.Vec".to_owned(),
        // Handled upstream; re-enter so the chain stays exhaustive.
        Value::Nil
        | Value::Bool(_)
        | Value::Int(_)
        | Value::Double(_)
        | Value::BigInt(_)
        | Value::BigInteger(_)
        | Value::BigDec(_)
        | Value::Ratio(_)
        | Value::Str(_)
        | Value::Char(_)
        | Value::Symbol(_)
        | Value::Keyword(_)
        | Value::Vector(_)
        | Value::MapEntry { .. }
        | Value::Map(_)
        | Value::Set(_)
        | Value::List(_)
        | Value::Uuid(_)
        | Value::Instant(_)
        | Value::Uri(_)
        | Value::Regex(_)
        | Value::TaggedLiteral { .. }
        | Value::ReaderConditional { .. }
        | Value::Var(_)
        | Value::Unbound(_)
        | Value::Throwable(_) => jvm_class_of(v),
    }
}

/// The host-type branch of [`jvm_class_of_host`] (split for the
/// complexity budget): port-defined types report their class name
/// (S-typechain); struct bases report the `$Def` member class (S12).
fn jvm_class_of_hosttype(h: &HostVal) -> String {
    match h {
        HostVal::Def(d) if d.kind == HostKind::Struct => {
            format!("{}$Def", d.name)
        }
        _ => h.type_name().to_owned(),
    }
}

/// The reported map class: stored, never derived (promotion is
/// one-way and history-dependent — `dissoc` never demotes and
/// `array-map` forces array at any size). Shared with the print
/// driver's dispatch tokens (single source).
#[must_use]
pub fn map_class_of(m: &Map) -> &'static str {
    if m.is_sorted() {
        "clojure.lang.PersistentTreeMap"
    } else if m.is_hashy() {
        "clojure.lang.PersistentHashMap"
    } else {
        "clojure.lang.PersistentArrayMap"
    }
}

#[cfg(test)]
mod tests {
    use super::{cell_jvm_class, jvm_class_of, jvm_class_of_host};
    use crate::{Atom, Instant, Ref, Symbol, Uri, Uuid, Value, reader_conditional, tagged_literal};
    use std::sync::Arc;

    #[test]
    fn jvm_class_of_host_reenters_scalars() {
        // The host match lists scalars only so the split stays
        // exhaustive. That arm re-enters the public function.
        assert_eq!(jvm_class_of_host(&Value::Nil), "null");
    }

    #[test]
    fn jvm_class_of_scalars() {
        // S9: the host classes (oracles `u-class`, `i-class`,
        // `r-class`; the `Inst` extension dispatches on Date).
        assert_eq!(
            jvm_class_of(&Value::Uuid(Uuid::from_bits(0, 0))),
            "java.util.UUID"
        );
        assert_eq!(
            jvm_class_of(&Value::Instant(Instant::from_millis(0))),
            "java.util.Date"
        );
        assert_eq!(
            jvm_class_of(&Value::Uri(Box::new(Uri::parse("http://h/x").unwrap()))),
            "java.net.URI"
        );
        assert_eq!(
            jvm_class_of_host(&Value::Uuid(Uuid::from_bits(0, 0))),
            "java.util.UUID"
        );
    }

    #[test]
    fn cell_jvm_class_names_every_cell() {
        // The split preserves each cell's class (refs join the
        // partition in S19).
        assert_eq!(
            cell_jvm_class(&Value::Ref(Ref::new(Value::Nil))),
            "clojure.lang.Ref"
        );
        assert_eq!(
            cell_jvm_class(&Value::Atom(Atom::new(Value::Nil))),
            "clojure.lang.Atom"
        );
    }

    #[test]
    #[should_panic(expected = "cell partition")]
    fn cell_jvm_class_rejects_non_cells() {
        // Defensive fallback: the host arm only ever passes cells.
        cell_jvm_class(&Value::Int(1));
    }

    #[test]
    fn jvm_class_of_reader_values() {
        // Oracle O1.
        assert_eq!(
            jvm_class_of(&Value::Regex(Arc::from("x"))),
            "java.util.regex.Pattern"
        );
        assert_eq!(
            jvm_class_of(&tagged_literal(Symbol::new(None, "foo"), Value::Int(1))),
            "clojure.lang.TaggedLiteral"
        );
        assert_eq!(
            jvm_class_of(&reader_conditional(Value::Int(1), false)),
            "clojure.lang.ReaderConditional"
        );
    }
}
