// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S18 compiler surface (normative:
//! `docs/zero-oos-s18-compiler/specification.md`).
//!
//! This module owns the knob roots, `add-classpath`, `native_compile`,
//! the leading-`ns` strip, the §1.4 object path, and the injected AOT
//! driver hook (`DriveError` + `CompileDriver` + the guarded
//! registry). `clj-eval` cannot depend on `clj-aot` (which plans
//! through `plan_source`), so the real driver installs here from the
//! linking binary; the unset hook reports the CLI's exact unlinked
//! line (T11).

use std::sync::Arc;

use clj_value::{Value, Vector, new_map, seq_vec};

use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::io::write_out;
use crate::ns::{cast_err, check_arity, core_var, deref_core, signal_to_native};

/// The six slot-only knob roots (JVM `Var.intern(...).setDynamic()`:
/// dynamic slot, no `:dynamic` meta — `boot` installs these;
/// `*verbose-defrecords*` rides `core_vars` instead, like the other
/// `^:dynamic` vars).
pub(crate) fn knob_roots() -> [(&'static str, Value); 6] {
    [
        ("*compile-path*", Value::Str(Arc::from("classes"))),
        ("*compile-files*", Value::Bool(false)),
        ("*compiler-options*", Value::Nil),
        ("*source-path*", Value::Str(Arc::from("NO_SOURCE_FILE"))),
        ("*fn-loader*", Value::Nil),
        ("*use-context-classloader*", Value::Bool(true)),
    ]
}

/// `primitives-classnames`: the 8 spec pairs over class-name-string
/// keys (JVM keys are `Class` objects — N1-unrepresentable — so the
/// 8 `.getName` strings stand in, per the #33 convention).
pub(crate) fn prims_map() -> Value {
    let mut map = new_map();
    for (key, val) in [
        ("boolean", "Boolean/TYPE"),
        ("byte", "Byte/TYPE"),
        ("char", "Character/TYPE"),
        ("double", "Double/TYPE"),
        ("float", "Float/TYPE"),
        ("int", "Integer/TYPE"),
        ("long", "Long/TYPE"),
        ("short", "Short/TYPE"),
    ] {
        map = map.insert(Value::Str(Arc::from(key)), Value::Str(Arc::from(val)));
    }
    Value::Map(map)
}

/// `(add-classpath dir)`: the historical append contract over
/// `*load-path*` — arity (T1, no WARNING), then the verbatim WARNING
/// (printed even when the add then fails), then the String check
/// (CCE `String` — the port takes path strings only), then a
/// value-membership guarded `conj` onto the root (nil normalizes to
/// `[]` first, preserving the append invariant; non-vector colls get
/// `conj` semantics). Answers nil.
pub(crate) fn native_add_classpath(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity("clojure.core/add-classpath", args, &[1])?;
    write_out(eval, "WARNING: add-classpath is deprecated\n")?;
    let Value::Str(dir) = &args[0] else {
        return Err(cast_err(&args[0], "java.lang.String"));
    };
    let dir_val = Value::Str(dir.clone());
    let id = core_var(eval, "*load-path*").expect("boot installs *load-path*");
    let root = eval
        .registry
        .state(id)
        .and_then(|s| s.root)
        .unwrap_or(Value::Nil);
    let coll = match root {
        Value::Nil => Value::Vector(Vector::from_vec(Vec::new())),
        other => other,
    };
    let items = seq_vec(&coll).ok_or_else(|| cast_err(&coll, "clojure.lang.IPersistentVector"))?;
    if items.iter().any(|entry| entry == &dir_val) {
        return Ok(Value::Nil);
    }
    let conj = deref_core(eval, "conj");
    let next = eval
        .call(&conj, &[coll, dir_val])
        .map_err(signal_to_native)?;
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(next);
    });
    Ok(Value::Nil)
}
