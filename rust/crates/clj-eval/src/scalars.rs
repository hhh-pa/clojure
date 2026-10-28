// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S9 scalar natives (issue #52): `parse-uuid` /
//! `random-uuid` / `uuid?` / `uri?` plus the hidden
//! `__inst_millis*` backing the `Inst` extension. (`Inst`,
//! `inst-ms*`, `inst-ms`, `inst?` are `core.clj` protocol
//! definitions over these kinds.) Oracle:
//! `docs/zero-oos-s9-scalarkinds/probes/*.out`.

use std::str::FromStr;

use clj_value::{ThrowClass, Uuid, Value};
use rand::Rng;

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity};

/// The S9 scalar natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const SCALAR_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("parse-uuid", native_parse_uuid),
    ("random-uuid", native_random_uuid),
    ("uuid?", native_uuid_pred),
    ("uri?", native_uri_pred),
    ("__inst_millis*", native_inst_millis),
];

/// `(parse-uuid s)`: `[^String s]` semantics — nil is the verbatim
/// NPE, non-strings fail the `String` cast, `fromString` failures
/// answer nil (probes `u-parse-*`, `e-uuid-*`).
fn native_parse_uuid(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/parse-uuid", args, &[1])?;
    match &args[0] {
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.length()\" because \"name\" is null".to_owned(),
        )),
        Value::Str(s) => Ok(Uuid::from_str(s).map_or(Value::Nil, Value::Uuid)),
        other => Err(cce_err(other, "java.lang.String")),
    }
}

/// `(random-uuid)`: two draws from the `rng.rs` source, stamped
/// version 4 / Leach-Salz (probe `e-uuid-ver4`).
fn native_random_uuid(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/random-uuid", args, &[0])?;
    let mut rng = rand::rng();
    Ok(Value::Uuid(Uuid::from_random(rng.random(), rng.random())))
}

/// Whether a value is a UUID (the `pred1!` pattern, written out —
/// that macro is `core.rs`-local).
fn is_uuid(v: &Value) -> bool {
    matches!(v, Value::Uuid(_))
}

/// Whether a value is a URI (see [`is_uuid`]).
fn is_uri(v: &Value) -> bool {
    matches!(v, Value::Uri(_))
}

/// `(uuid? x)` (probes `u-p-*`).
fn native_uuid_pred(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/uuid?", args, &[1])?;
    Ok(Value::Bool(is_uuid(&args[0])))
}

/// `(uri? x)` (probes `r-p-*`).
fn native_uri_pred(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/uri?", args, &[1])?;
    Ok(Value::Bool(is_uri(&args[0])))
}

/// The hidden millis reader behind the `java.util.Date` `Inst`
/// extension (the `__protocol*` precedent): instants answer their
/// millis; anything else fails the `Date` cast (out-of-contract
/// direct calls only — dispatch guarantees instants).
fn native_inst_millis(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__inst_millis*", args, &[1])?;
    match &args[0] {
        Value::Instant(i) => Ok(Value::Int(i.millis)),
        other => Err(cce_err(other, "java.util.Date")),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use clj_value::{Instant, Symbol, Value};

    use crate::error::Signal;

    /// Call a `clojure.core` var root with constructed values (the
    /// S6 boundary keeps instants out of Clojure source, so the
    /// `Date` extension is exercised here).
    fn call_var(
        eval: &mut crate::eval::Evaluator,
        name: &str,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let sym = Symbol::new(Some("clojure.core"), name);
        let id = eval.registry.lookup(&sym).expect("core var exists");
        let func = eval.deref_var(id, &sym);
        eval.call(&func, args)
    }

    /// `inst-ms` over real instants (probes `i-ms-*`, `e-cut-*`).
    #[test]
    fn inst_ms_over_instants() {
        let mut eval = crate::ns::boot();
        for (input, want) in [
            ("2024-01-01", 1704067200000),
            ("2024-01-01T00:59:60", 1704070800000),
            ("2024-01-01T00:00:00.1234567", 1704067200123),
            ("1960-05-05", -304819200000),
            ("1970-01-01T00:00:00.000-00:00", 0),
            ("0001-01-01", -62135769600000),
            ("1582-10-10", -12218860800000),
        ] {
            let instant = Value::Instant(Instant::from_str(input).unwrap());
            assert_eq!(
                call_var(&mut eval, "inst-ms", &[instant]).expect("inst-ms answers"),
                Value::Int(want),
                "{input}"
            );
        }
    }

    /// `inst?` over a real instant (probe `i-p-inst`).
    #[test]
    fn inst_pred_over_instant() {
        let mut eval = crate::ns::boot();
        let instant = Value::Instant(Instant::from_str("2024-01-01").unwrap());
        assert_eq!(
            call_var(&mut eval, "inst?", &[instant]).expect("inst? answers"),
            Value::Bool(true)
        );
        assert_eq!(
            call_var(&mut eval, "inst?", &[Value::Int(42)]).expect("inst? answers"),
            Value::Bool(false)
        );
    }

    /// The `Date` extension's failure text (probe `i-ms-int`).
    #[test]
    fn inst_ms_error_names_class() {
        let mut eval = crate::ns::boot();
        let result = call_var(&mut eval, "inst-ms", &[Value::Int(42)]);
        // Guarded `matches!`: the class and message asserts below
        // only pass on the `Throw` shape, so no `else`/`panic`
        // arm is needed (every line runs — the 100% line gate).
        assert!(matches!(
            &result,
            Err(Signal::Throw(t)) if t.class.jvm_name() == "java.lang.IllegalArgumentException"
        ));
        assert!(matches!(
            &result,
            Err(Signal::Throw(t))
                if t.message.as_deref().unwrap_or("")
                    == "No implementation of method: :inst-ms* of protocol: #'clojure.core/Inst found for class: java.lang.Long"
        ));
    }

    /// The hidden millis reader, direct.
    #[test]
    fn hidden_millis_direct() {
        let mut eval = crate::ns::boot();
        let instant = Value::Instant(Instant::from_str("2024-01-01").unwrap());
        assert_eq!(
            call_var(&mut eval, "__inst_millis*", &[instant]).expect("millis answers"),
            Value::Int(1704067200000)
        );
        let result = call_var(&mut eval, "__inst_millis*", &[Value::Str("x".into())]);
        assert!(matches!(
            &result,
            Err(Signal::Throw(t)) if t.class.jvm_name() == "java.lang.ClassCastException"
        ));
        assert!(matches!(
            &result,
            Err(Signal::Throw(t))
                if t.message
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("class java.lang.String cannot be cast to class java.util.Date")
        ));
    }
}
