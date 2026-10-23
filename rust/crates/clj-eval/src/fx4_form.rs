// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Value builders for the FX4 macro expanders (`let`/`fn`/…).
//! Syntax-quote qualification is explicit: `clojure.core/…` for
//! resolved vars, unqualified for specials.

use clj_value::{Keyword, List, Map, Symbol, ThrowClass, Value, Vector, new_map, print_to_string};

use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;

/// `str` (`nil` prints empty).
pub(crate) fn str_of(v: &Value) -> String {
    match v {
        Value::Nil => String::new(),
        _ => print_to_string(v),
    }
}

/// Readable print (sets in `:or` errors).
pub(crate) fn pr_of(v: &Value) -> String {
    clj_value::pr_to_string(v, &clj_value::PrintOpts::pr())
}

pub(crate) fn iae(message: String) -> NativeError {
    NativeError::new(ThrowClass::IllegalArgumentException, message)
}

pub(crate) fn ex(message: String) -> NativeError {
    NativeError::new(ThrowClass::Exception, message)
}

pub(crate) fn npe(message: String) -> NativeError {
    NativeError::new(ThrowClass::NullPointerException, message)
}

pub(crate) fn iseq_err(v: &Value) -> NativeError {
    iae(format!(
        "Don't know how to create ISeq from: {}",
        jvm_class_of(v)
    ))
}

pub(crate) fn sym(name: &str) -> Value {
    Value::Symbol(Symbol::intern(name))
}

pub(crate) fn sym_q(ns: &str, name: &str) -> Value {
    Value::Symbol(Symbol::new(Some(ns), name))
}

pub(crate) fn symv(s: &Symbol) -> Value {
    Value::Symbol(s.clone())
}

pub(crate) fn kw(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

pub(crate) fn lst(items: Vec<Value>) -> Value {
    Value::List(List::from_vec(items))
}

pub(crate) fn vecv(items: Vec<Value>) -> Value {
    Value::Vector(Vector::from_vec(items))
}

/// `(clojure.core/name …)`.
pub(crate) fn qcall(name: &str, args: Vec<Value>) -> Value {
    let mut items = vec![sym_q("clojure.core", name)];
    items.extend(args);
    lst(items)
}

/// Unqualified call (`if`, `try`, `quote`, `new`, …).
pub(crate) fn ucall(name: &str, args: Vec<Value>) -> Value {
    let mut items = vec![sym(name)];
    items.extend(args);
    lst(items)
}

/// `(gensym prefix)` — `prefix{n}`, no `__auto__`.
pub(crate) fn gs(eval: &mut Evaluator, prefix: &str) -> Symbol {
    let n = eval.next_gensym();
    Symbol::intern(&format!("{prefix}{n}"))
}

/// Syntax-quote auto-gensym `prefix__{n}__auto__`.
pub(crate) fn gs_auto(eval: &mut Evaluator, prefix: &str) -> Symbol {
    let n = eval.next_gensym();
    Symbol::intern(&format!("{prefix}__{n}__auto__"))
}

/// `{:tag clojure.lang.ISeq}` on a copy (the binding occurrence stays plain).
pub(crate) fn tag_iseq(sym: Symbol) -> Symbol {
    let meta = new_map().insert(
        kw("tag"),
        Value::Symbol(Symbol::new(None, "clojure.lang.ISeq")),
    );
    sym.with_meta(Some(meta))
}

pub(crate) fn is_symbol_val(v: &Value) -> bool {
    matches!(v, Value::Symbol(_))
}

pub(crate) fn is_ident(v: &Value) -> bool {
    matches!(v, Value::Symbol(_) | Value::Keyword(_))
}

pub(crate) fn is_amp(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.namespace().is_none() && s.name() == "&")
}

pub(crate) fn is_as(v: &Value) -> bool {
    matches!(v, Value::Keyword(k) if k.namespace().is_none() && k.name() == "as")
}

/// `vector?` — vectors and map entries.
pub(crate) fn is_vector_val(v: &Value) -> bool {
    vector_items(v).is_some()
}

pub(crate) fn vector_items(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Vector(vec) => Some(vec.iter().cloned().collect()),
        Value::MapEntry { key, val } => Some(vec![(**key).clone(), (**val).clone()]),
        _ => None,
    }
}

pub(crate) fn is_seqable_data(v: &Value) -> bool {
    matches!(
        v,
        Value::Nil
            | Value::List(_)
            | Value::Vector(_)
            | Value::MapEntry { .. }
            | Value::Map(_)
            | Value::Set(_)
            | Value::Str(_)
    )
}

/// Seq a binding/key-list datum. Maps yield pair vectors (the
/// analyzer's seq, so a non-`Named` cast names `PersistentVector`).
pub(crate) fn seq_items(v: &Value) -> Vec<Value> {
    match v {
        Value::Nil => Vec::new(),
        Value::List(l) => l.iter().cloned().collect(),
        Value::Vector(vec) => vec.iter().cloned().collect(),
        Value::MapEntry { key, val } => vec![(**key).clone(), (**val).clone()],
        Value::Map(m) => m
            .iter()
            .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
            .collect(),
        Value::Set(s) => s.iter().cloned().collect(),
        Value::Str(s) => s.chars().map(Value::Char).collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn seq_data(v: &Value) -> Result<Vec<Value>, NativeError> {
    if !is_seqable_data(v) {
        return Err(iseq_err(v));
    }
    Ok(seq_items(v))
}

pub(crate) fn form_head(form: &Value) -> Value {
    match form {
        Value::List(l) => l.first().cloned().unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

/// Copy `&form` meta onto an expansion (`fn`'s `with-meta`).
pub(crate) fn with_form_meta(expanded: Value, form: &Value) -> Value {
    expanded.with_meta(form.meta().cloned())
}

pub(crate) fn map_of(pairs: Vec<(Value, Value)>) -> Value {
    let mut map = new_map();
    for (k, v) in pairs {
        map = map.insert(k, v);
    }
    Value::Map(map)
}

pub(crate) fn kw_get<'a>(m: &'a Map, name: &str) -> Option<&'a Value> {
    m.get(&kw(name))
}
