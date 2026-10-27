// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Stage-2 protocols (S-typechain, issue #33): `defprotocol`
//! descriptors plus `extend` / `extend-type` / `extend-protocol`
//! registries, `satisfies?` / `extends?`, protocol-fn dispatch,
//! and the `with-open` close contract (the macro itself lives in
//! `core.clj`; closing is plain `.close` interop).
//!
//! A protocol is an ordinary map value with JVM-shaped keys:
//! `:on` (the interface class-name string), `:sigs` (method
//! keyword → `{:arglists [...]}` as written, `this` included),
//! `:impls` (class-name string → method-name string → argc → fn),
//! and `:interface` (the `definterface` descriptor the protocol
//! was built over — a port extension; no oracle pin reads it).
//! Extensions merge into `:impls` under the port's var-alter lock
//! (read-modify-install serializes concurrent extends the way
//! `alter-var-root` does — no lost update for disjoint types).
//! Dispatch keys types by [`jvm_class_of`], so port types (`user.PD`)
//! and host classes (`java.lang.String`, `null`) share one space.
//! There is no hierarchy walk (fenced with `derive`/`isa?`): the
//! only interface fallback is `clojure.lang.IPersistentMap` for
//! maps and records (oracle `c-map`).

use std::sync::Arc;

use clj_value::{Keyword, Map, Symbol, ThrowClass, Value, new_map};

use crate::core::cce_err;
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

pub const PROTOCOL_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("__protocol*", native_protocol),
    ("__extend*", native_extend),
    ("__protocol_invoke*", native_protocol_invoke),
    ("satisfies?", native_satisfies),
    ("extends?", native_extends),
];

/// Map keys of a protocol value.
fn on_key() -> Value {
    Value::Keyword(Keyword::new(None, "on"))
}

fn sigs_key() -> Value {
    Value::Keyword(Keyword::new(None, "sigs"))
}

fn impls_key() -> Value {
    Value::Keyword(Keyword::new(None, "impls"))
}

fn iface_key() -> Value {
    Value::Keyword(Keyword::new(None, "interface"))
}

/// Whether a value is a protocol map (an `:on` string, a `:sigs`
/// map, an `:impls` map, and the `:interface` descriptor — the
/// full constructor shape, so dispatch never unwraps a missing
/// key; anything else is `Not a protocol`).
pub(crate) fn as_protocol_map(v: &Value) -> Option<&Map> {
    match v {
        Value::Map(m) => match (
            m.get(&on_key()),
            m.get(&sigs_key()),
            m.get(&impls_key()),
            m.get(&iface_key()),
        ) {
            (
                Some(Value::Str(_)),
                Some(Value::Map(_)),
                Some(Value::Map(_)),
                Some(Value::HostType(h)),
            ) if matches!(h.as_ref(), clj_value::HostVal::Def(_)) => Some(m),
            _ => None,
        },
        _ => None,
    }
}

fn not_a_protocol(v: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("{} is not a protocol", pr_of(v)),
    )
}

/// Human-readable form for `is not a protocol` errors (names what
/// the caller wrote, oracle `e-ext-nonproto`).
fn pr_of(v: &Value) -> String {
    clj_value::pr_to_string(v, &clj_value::PrintOpts::pr())
}

/// A protocol reference: the var id (for locked extension), the
/// symbol naming it in errors, and the current map. Macros emit a
/// quoted symbol (or a var); anything else — including a resolved
/// value that is not a protocol — is the oracle's `is not a
/// protocol` error.
fn proto_ref(eval: &mut Evaluator, v: &Value) -> Result<(u64, Symbol, Map), NativeError> {
    match v {
        Value::Var(var) => {
            let root = eval.deref_var(var.id, &var.sym);
            match as_protocol_map(&root) {
                Some(m) => Ok((var.id, var.sym.clone(), m.clone())),
                None => Err(not_a_protocol(&root)),
            }
        }
        Value::Symbol(sym) => {
            let (id, root) = resolve_root(eval, sym)?;
            match as_protocol_map(&root) {
                Some(m) => Ok((id, sym.clone(), m.clone())),
                None => Err(NativeError::new(
                    ThrowClass::IllegalArgumentException,
                    format!("{} is not a protocol", pr_of(v)),
                )),
            }
        }
        _ => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("{} is not a protocol", pr_of(v)),
        )),
    }
}

/// Resolve a symbol to its var id plus current root (missing is the
/// stock unresolvable error). A single `lookup_in` (not
/// resolve-then-lookup) so a concurrent unmap cannot strand the
/// second step.
fn resolve_root(eval: &mut Evaluator, sym: &Symbol) -> Result<(u64, Value), NativeError> {
    let cur = eval.current_ns_name();
    match eval.registry.lookup_in(&cur, sym) {
        Some(id) => Ok((id, eval.deref_var(id, sym))),
        None => Err(unresolvable(sym)),
    }
}

fn unresolvable(sym: &Symbol) -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        format!("Unable to resolve symbol: {sym} in this context"),
    )
}

/// Build a protocol map over an interface descriptor (split for
/// budget): `:on` names the interface, `:sigs` rides as given.
fn native_protocol(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__protocol*", args, &[2])?;
    let sigs = match &args[0] {
        Value::Map(m) => m.clone(),
        _ => return Err(cce_err(&args[0], "clojure.lang.IPersistentMap")),
    };
    let def = match &args[1] {
        Value::Var(var) => match eval.deref_var(var.id, &var.sym) {
            Value::HostType(h) => match h.as_ref() {
                clj_value::HostVal::Def(d) => d.clone(),
                _ => return Err(not_an_interface(&var.sym)),
            },
            _ => return Err(not_an_interface(&var.sym)),
        },
        _ => {
            return Err(cce_err(&args[1], "clojure.lang.Var"));
        }
    };
    if def.kind != clj_value::HostKind::Interface {
        return Err(not_an_interface(&def.name));
    }
    let mut out = new_map();
    out = out.insert(on_key(), Value::Str(def.name.clone()));
    out = out.insert(sigs_key(), Value::Map(sigs));
    out = out.insert(impls_key(), Value::Map(new_map()));
    out = out.insert(
        iface_key(),
        Value::HostType(Arc::new(clj_value::HostVal::Def(def))),
    );
    Ok(Value::Map(out))
}

fn not_an_interface(sym: &dyn std::fmt::Display) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Not an interface: {sym}"),
    )
}

/// The class key an extension targets (split for budget): `nil`
/// extends null; symbols resolve via [`extend_sym`]; a descriptor
/// value names itself; anything else is not a type.
fn extend_target(eval: &mut Evaluator, v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Nil => Ok("null".to_owned()),
        Value::Symbol(sym) => match sym.namespace() {
            Some(ns) => Ok(format!("{ns}.{}", sym.name())),
            None => extend_sym(eval, sym, v),
        },
        Value::HostType(h) => match h.as_ref() {
            clj_value::HostVal::Def(d) => Ok(d.name.to_string()),
            _ => Err(not_a_type(v)),
        },
        _ => Err(not_a_type(v)),
    }
}

/// Resolve an unqualified extension symbol (split from
/// [`extend_target`] for the complexity budget): a port descriptor
/// names itself; a string names its class (the core `String` /
/// `Long` / … vars); any other bound non-type is not a class; a
/// dotted name rides as-is (like `clojure.lang.IPersistentMap` or
/// `user.PD`); any other unresolvable name stays unresolvable (a
/// typo must fail, not extend a phantom `java.lang` class — the
/// bound class-name vars cover the host values that exist).
fn extend_sym(eval: &mut Evaluator, sym: &Symbol, v: &Value) -> Result<String, NativeError> {
    match resolve_root(eval, sym) {
        Ok((_, Value::HostType(h))) => match h.as_ref() {
            clj_value::HostVal::Def(d) => Ok(d.name.to_string()),
            _ => Err(not_a_type(v)),
        },
        Ok((_, Value::Str(s))) => Ok(s.to_string()),
        Ok(_) => Err(not_a_type(v)),
        Err(e) => {
            if sym.name().contains('.') {
                Ok(sym.name().to_owned())
            } else {
                Err(e)
            }
        }
    }
}

fn not_a_type(v: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Not a type: {}", pr_of(v)),
    )
}

/// Merge one method entry into a type entry (split for budget):
/// an argc→fn map merges per arity (the macros emit exactly that);
/// a bare fn covers every declared arity (`extend` map form — the
/// fn is an ifn and dispatches itself at call time).
fn merge_one(sigs: &Map, entry: Map, name: &str, val: &Value) -> Map {
    let key = Value::Str(Arc::from(name));
    let mut merged = match entry.get(&key) {
        Some(Value::Map(m)) => m.clone(),
        _ => new_map(),
    };
    match val {
        Value::Map(am) => {
            for (argc, func) in am.iter() {
                if matches!(argc, Value::Int(_)) {
                    merged = merged.insert((*argc).clone(), (*func).clone());
                }
            }
        }
        _ => {
            for argc in method_argcnts(sigs, name) {
                merged = merged.insert(Value::Int(argc as i64), val.clone());
            }
        }
    }
    entry.insert(key, Value::Map(merged))
}

/// Merge extension methods into a protocol map (split for budget):
/// per method, per argc, the new fn wins; the type entry is created
/// on demand. Method keys are strings (macros) or keywords
/// (`extend` map form); anything else is skipped, never trusted.
fn merge_impls(proto: &Map, type_name: &str, methods: &Map) -> Map {
    let mut impls = match proto.get(&impls_key()) {
        Some(Value::Map(m)) => m.clone(),
        _ => new_map(),
    };
    let mut type_entry = match impls.get(&Value::Str(Arc::from(type_name))) {
        Some(Value::Map(m)) => m.clone(),
        _ => new_map(),
    };
    let sigs = match proto.get(&sigs_key()) {
        Some(Value::Map(m)) => m.clone(),
        _ => new_map(),
    };
    for (method, val) in methods.iter() {
        let name = match method {
            Value::Str(s) => s.as_ref().to_owned(),
            Value::Keyword(k) => k.name().to_owned(),
            _ => continue,
        };
        type_entry = merge_one(&sigs, type_entry, &name, val);
    }
    impls = impls.insert(Value::Str(Arc::from(type_name)), Value::Map(type_entry));
    let mut out = proto.clone();
    out = out.insert(impls_key(), Value::Map(impls));
    out
}

/// Extend a protocol under the var-alter lock (split for budget):
/// the root is re-read after acquiring, merged, and installed only
/// on success — concurrent disjoint extends serialize with no lost
/// update, like `alter-var-root`.
fn native_extend(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__extend*", args, &[3])?;
    let methods = match &args[2] {
        Value::Map(m) => m.clone(),
        _ => return Err(cce_err(&args[2], "clojure.lang.IPersistentMap")),
    };
    let (id, sym, _) = proto_ref(eval, &args[0])?;
    let type_name = extend_target(eval, &args[1])?;
    let next = install_impls(eval, id, &sym, &type_name, &methods)?;
    Ok(Value::Map(next))
}

/// Read one impl cell of a type entry (split for budget): total
/// matches, so a hand-built map degrades to a miss, never a panic.
fn impl_at<'a>(entry: &'a Map, method: &str, argc: usize) -> Option<&'a Value> {
    match entry.get(&Value::Str(Arc::from(method))) {
        Some(Value::Map(am)) => match am.get(&Value::Int(argc as i64)) {
            Some(f) => Some(f),
            _ => None,
        },
        _ => None,
    }
}

/// The impl entry for a value's type (split for budget): exact
/// class key, else the `IPersistentMap` entry for maps and records
/// (oracle `c-map`; no general hierarchy walk).
fn type_entry<'a>(impls: &'a Map, v: &Value) -> Option<&'a Value> {
    let key = Value::Str(Arc::from(jvm_class_of(v).as_str()));
    match impls.get(&key) {
        Some(e) => Some(e),
        None => {
            if is_map_like(v) {
                impls.get(&Value::Str(Arc::from("clojure.lang.IPersistentMap")))
            } else {
                None
            }
        }
    }
}

/// Whether a value counts as map-like for the `IPersistentMap`
/// fallback: plain maps and record instances (records are maps).
fn is_map_like(v: &Value) -> bool {
    matches!(v, Value::Map(_)) || crate::types::is_host_record(v)
}

/// Find the impl fn for a call (split for budget): descriptors
/// never dispatch by defined name (like the JVM's classes — only
/// their instances do), so they always miss to the caller's error.
fn find_impl<'a>(impls: &'a Map, method: &str, argc: usize, v: &Value) -> Option<&'a Value> {
    match v {
        Value::HostType(h) if matches!(h.as_ref(), clj_value::HostVal::Def(_)) => None,
        _ => match type_entry(impls, v) {
            Some(Value::Map(t)) => impl_at(t, method, argc),
            _ => None,
        },
    }
}

/// Declared arities (past `this`) of a protocol method (split for
/// budget): read off `:arglists`; anything unexpected contributes
/// nothing, so a hand-built map degrades to the arity error.
fn method_argcnts(sigs: &Map, method: &str) -> Vec<usize> {
    let key = Value::Keyword(Keyword::new(None, method));
    let lists: Vec<Value> = match sigs.get(&key) {
        Some(Value::Map(info)) => match info.get(&Value::Keyword(Keyword::new(None, "arglists"))) {
            Some(Value::List(lists)) => lists.iter().cloned().collect(),
            Some(Value::Vector(lists)) => lists.iter().cloned().collect(),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    lists
        .iter()
        .filter_map(|a| match a {
            Value::Vector(argv) => argv.len().checked_sub(1),
            _ => None,
        })
        .collect()
}

/// Split a protocol map into its dispatch parts (split for budget):
/// validated maps always carry all three, so anything else is
/// unreachable through the constructors.
fn proto_parts(proto: &Map) -> (&Map, &Map, &str) {
    let sigs = match proto.get(&sigs_key()) {
        Some(Value::Map(m)) => m,
        _ => unreachable!("protocols always carry sigs"),
    };
    let impls = match proto.get(&impls_key()) {
        Some(Value::Map(m)) => m,
        _ => unreachable!("protocols always carry impls"),
    };
    let on = match proto.get(&on_key()) {
        Some(Value::Str(s)) => s.as_ref(),
        _ => unreachable!("protocols always carry on"),
    };
    (sigs, impls, on)
}

/// Call a protocol method (split for budget): arity first (oracle
/// `e-arity`), then the type impl (oracle `e-noimpl`).
fn native_protocol_invoke(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__protocol_invoke*", args, &[3])?;
    let method = match &args[1] {
        Value::Str(s) => s.to_string(),
        _ => return Err(cce_err(&args[1], "java.lang.String")),
    };
    let callargs = match &args[2] {
        Value::Vector(items) => items.iter().cloned().collect::<Vec<_>>(),
        _ => return Err(cce_err(&args[2], "clojure.lang.IPersistentVector")),
    };
    if callargs.is_empty() {
        return Err(NativeError::arity("clojure.core/__protocol_invoke*", 0));
    }
    let (_, varsym, proto) = proto_ref(eval, &args[0])?;
    let (sigs, impls, on) = proto_parts(&proto);
    let argc = callargs.len() - 1;
    if !method_argcnts(sigs, &method).contains(&argc) {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!(
                "No single method: {method} of interface: {on} found for function: {method} of protocol: {}",
                varsym.name()
            ),
        ));
    }
    match find_impl(impls, &method, argc, &callargs[0]) {
        Some(func) => eval.call(func, &callargs).map_err(signal_to_native),
        None => invoke_unregistered(eval, &proto, &varsym, &method, &callargs[0], &callargs[1..]),
    }
}

/// No registry impl: inline fallback, else the oracle error (split
/// for budget).
fn invoke_unregistered(
    eval: &mut Evaluator,
    proto: &Map,
    varsym: &Symbol,
    method: &str,
    this: &Value,
    rest: &[Value],
) -> Result<Value, NativeError> {
    if let Some(r) = invoke_inline(eval, proto, this, method, rest.len(), rest) {
        return r;
    }
    Err(NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!(
            "No implementation of method: :{method} of protocol: #'{} found for class: {}",
            varsym,
            jvm_class_of(this)
        ),
    ))
}

/// Inline fallback (split for budget): registry misses consult the
/// instance's own definition — `None` for non-instances or types
/// that do not claim the protocol there.
fn invoke_inline(
    eval: &mut Evaluator,
    proto: &Map,
    this: &Value,
    method: &str,
    argc: usize,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    let inst = match this {
        Value::HostType(h) => match h.as_ref() {
            clj_value::HostVal::Inst(i) => i,
            _ => return None,
        },
        _ => return None,
    };
    let func = inline_impl(&inst.def, proto_on(proto), method, argc)?;
    Some(if inst.def.takes_this {
        crate::types::call_method(eval, &func, this.clone(), rest)
    } else {
        eval.call(&func, rest).map_err(signal_to_native)
    })
}

/// Whether a value's type implements a protocol (split for
/// budget): entry presence (any method) counts, like the JVM.
fn native_satisfies(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/satisfies?", args, &[2])?;
    let proto = match as_protocol_map(&args[0]) {
        Some(m) => m.clone(),
        None => return Err(not_a_protocol(&args[0])),
    };
    let (_, impls, on) = proto_parts(&proto);
    let hit = match &args[1] {
        Value::HostType(h) => match h.as_ref() {
            // Descriptors never satisfy (like the JVM's classes).
            clj_value::HostVal::Def(_) => false,
            clj_value::HostVal::Inst(i) => {
                type_entry(impls, &args[1]).is_some() || def_claims_proto(&i.def, on)
            }
        },
        _ => type_entry(impls, &args[1]).is_some(),
    };
    Ok(Value::Bool(hit))
}

/// Whether a class implements a protocol (split for budget):
/// descriptors by name (records also answer through the
/// `IPersistentMap` entry); class-name strings work too, since host
/// classes have no values to pass.
fn native_extends(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/extends?", args, &[2])?;
    let proto = match as_protocol_map(&args[0]) {
        Some(m) => m.clone(),
        None => return Err(not_a_protocol(&args[0])),
    };
    let (_, impls, on) = proto_parts(&proto);
    let (name, map_like, claimed) = match &args[1] {
        Value::HostType(h) => match h.as_ref() {
            clj_value::HostVal::Def(d) => (
                d.name.to_string(),
                d.kind == clj_value::HostKind::Record,
                def_claims_proto(d, on),
            ),
            _ => return Err(cce_err(&args[1], "java.lang.Class")),
        },
        Value::Str(s) => (s.to_string(), false, false),
        _ => return Err(cce_err(&args[1], "java.lang.Class")),
    };
    let key = Value::Str(Arc::from(name.as_str()));
    Ok(Value::Bool(match impls.get(&key) {
        Some(_) => true,
        None => {
            claimed
                || (map_like
                    && impls.contains_key(&Value::Str(Arc::from("clojure.lang.IPersistentMap"))))
        }
    }))
}

/// A listed interface that is really a protocol (for inline
/// `deftype` / `defrecord` / `reify` / `proxy` bodies): the current
/// map (its `:on` name is claimed on the new type; its fns stay in
/// the type's own method table — no registry round-trip).
pub(crate) struct ProtoIface {
    pub map: Map,
}

/// Resolve an interface-position symbol to its protocol (split for
/// budget): a var whose root is a protocol map. Anything else
/// (interfaces, missing) is not one.
pub(crate) fn proto_of(eval: &mut Evaluator, sym: &Symbol) -> Option<ProtoIface> {
    let cur = eval.current_ns_name();
    let q = eval.registry.resolve_in(&cur, sym)?;
    let id = eval.registry.lookup(&q)?;
    let root = eval.deref_var(id, sym);
    as_protocol_map(&root).map(|m| ProtoIface { map: m.clone() })
}

/// The `:on` interface name a protocol claims (split for budget).
pub(crate) fn proto_on(map: &Map) -> &str {
    match map.get(&on_key()) {
        Some(Value::Str(s)) => s.as_ref(),
        _ => unreachable!("protocols always carry on"),
    }
}

/// Whether a type definition claims a protocol (split for budget):
/// inline bodies list the protocol's `:on` name.
pub(crate) fn def_claims_proto(def: &clj_value::HostDef, on: &str) -> bool {
    def.interfaces.iter().any(|i| i.as_ref() == on)
}

/// An inline impl fn from an instance definition (split for
/// budget): `None` unless the def claims the protocol and tables
/// the method at the arity.
pub(crate) fn inline_impl(
    def: &clj_value::HostDef,
    on: &str,
    method: &str,
    argc: usize,
) -> Option<Value> {
    if !def_claims_proto(def, on) {
        return None;
    }
    clj_value::find_method(def, method, argc).map(|m| m.func.clone())
}

/// The interface descriptor a protocol was built over (split for
/// budget): inline bodies resolve, check, and claim through it —
/// its name is the `:on` string and its sigs are the past-`this`
/// arities, so the shared interface path just works.
pub(crate) fn proto_interface(map: &Map) -> Arc<clj_value::HostDef> {
    match map.get(&iface_key()) {
        Some(Value::HostType(h)) => match h.as_ref() {
            clj_value::HostVal::Def(d) => d.clone(),
            _ => unreachable!("protocols always carry interface"),
        },
        _ => unreachable!("protocols always carry interface"),
    }
}

/// Install extension methods under the var-alter lock (split for
/// budget): the `__extend*` write path.
pub(crate) fn install_impls(
    eval: &mut Evaluator,
    id: u64,
    sym: &Symbol,
    type_name: &str,
    methods: &Map,
) -> Result<Map, NativeError> {
    let _guard = eval.registry.lock_var_for_alter(id);
    let current = eval.registry.root_or_unbound(id, sym);
    let proto = match as_protocol_map(&current) {
        Some(m) => m.clone(),
        None => return Err(not_a_protocol(&current)),
    };
    let next = merge_impls(&proto, type_name, methods);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(Value::Map(next.clone()));
    });
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with(pairs: Vec<(Value, Value)>) -> Map {
        let mut m = new_map();
        for (k, v) in pairs {
            m = m.insert(k, v);
        }
        m
    }

    #[test]
    #[should_panic(expected = "protocols always carry sigs")]
    fn proto_parts_without_sigs_panics() {
        let m = map_with(vec![
            (on_key(), Value::Str(Arc::from("X"))),
            (impls_key(), Value::Map(new_map())),
        ]);
        let _ = proto_parts(&m);
    }

    #[test]
    #[should_panic(expected = "protocols always carry impls")]
    fn proto_parts_without_impls_panics() {
        let m = map_with(vec![
            (on_key(), Value::Str(Arc::from("X"))),
            (sigs_key(), Value::Map(new_map())),
        ]);
        let _ = proto_parts(&m);
    }

    #[test]
    #[should_panic(expected = "protocols always carry on")]
    fn proto_parts_without_on_panics() {
        let m = map_with(vec![
            (sigs_key(), Value::Map(new_map())),
            (impls_key(), Value::Map(new_map())),
        ]);
        let _ = proto_parts(&m);
    }

    #[test]
    #[should_panic(expected = "protocols always carry on")]
    fn proto_on_without_on_panics() {
        let _ = proto_on(&new_map());
    }

    #[test]
    #[should_panic(expected = "protocols always carry interface")]
    fn proto_interface_without_key_panics() {
        let _ = proto_interface(&new_map());
    }

    #[test]
    #[should_panic(expected = "protocols always carry interface")]
    fn proto_interface_with_instance_panics() {
        let def = Arc::new(clj_value::HostDef {
            name: Arc::from("user.T"),
            kind: clj_value::HostKind::Type,
            fields: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
            struct_keys: Vec::new(),
        });
        let inst = Value::HostType(Arc::new(clj_value::HostVal::Inst(clj_value::HostInst {
            def,
            content: new_map(),
            meta: None,
        })));
        let m = map_with(vec![(iface_key(), inst)]);
        let _ = proto_interface(&m);
    }

    #[test]
    fn merge_impls_defaults_missing_tables() {
        // A shape-violating map still merges (defensive totals —
        // `install_impls` only passes validated shapes, so
        // integration tests never reach these arms).
        let merged = merge_impls(&new_map(), "C", &new_map());
        let s = pr_of(&Value::Map(merged));
        assert!(s.contains("\"C\""), "merged entry present: {s}");
    }

    #[test]
    fn install_impls_rejects_non_protocol_root() {
        // The under-lock re-validation: a var whose root is not a
        // protocol errors instead of installing (covers the TOCTOU
        // arm without threads — no interleaving can be forced
        // single-threaded through `__extend*`).
        let mut eval = crate::boot();
        eval.eval_source("(def P 5)").unwrap();
        let sym = Symbol::new(None, "P");
        let cur = eval.current_ns_name();
        let q = eval.registry.resolve_in(&cur, &sym).unwrap();
        let id = eval.registry.lookup(&q).unwrap();
        let err = install_impls(&mut eval, id, &sym, "C", &new_map()).unwrap_err();
        assert_eq!(err.class, ThrowClass::IllegalArgumentException);
        assert_eq!(err.message, "5 is not a protocol");
    }
}
