// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The safe evaluation steps natively compiled code calls (issue
//! #259 §2.4). Each function is one step of [`Evaluator::eval`]
//! with its sub-expressions already evaluated by native code (values
//! in, value out), so compiled programs and `run` share every
//! semantic — errors, laziness, var resolution — by construction.
//!
//! Bodies the interpreter evaluates under a dynamic extent (`try`,
//! `binding`, …) arrive as zero-argument fn values compiled
//! natively (`body` below): the step sets up the extent and calls
//! the body through [`Evaluator::call`].

use std::sync::Arc;

use clj_reader::Span;
use clj_value::{Keyword, Map, Symbol, ThrowClass, Throwable, Value, Var, Vector, new_map};

use crate::error::{NativeError, Signal};
use crate::eval::Evaluator;
use crate::expr::{CaseTest, NsClause};

/// The result of every step: a value or a pending throw.
pub type Step = Result<Value, Signal>;

/// Call `f` with `args` exactly like an interpreted call.
pub fn invoke(eval: &mut Evaluator, f: &Value, args: &[Value]) -> Step {
    eval.call(f, args)
}

/// The runtime var for a fully qualified symbol (interned like the
/// analyzer does when it resolves a def target or a global).
pub fn intern_var(eval: &mut Evaluator, ns: &str, name: &str) -> Var {
    let sym = Symbol::new(Some(ns), name);
    let id = eval.registry.intern(&sym);
    Var { id, sym }
}

/// A var's current value (thread binding, else root; unbound vars
/// deref like the interpreter does).
pub fn deref(eval: &mut Evaluator, var: &Var) -> Value {
    eval.deref_var_cached(var.id, &var.sym)
}

/// `(def sym …)` first half: the target var's id, before the init
/// runs (recursive `defn`s see their own var).
pub fn def_begin(eval: &mut Evaluator, sym: &Symbol) -> u64 {
    eval.def_begin(sym)
}

/// `(def sym …)` second half: root, metadata, dynamic flag; returns
/// the var.
pub fn def_finish(
    eval: &mut Evaluator,
    id: u64,
    sym: &Symbol,
    doc: Option<String>,
    value: Option<Value>,
    name_meta: Option<Map>,
    line: u32,
    col: u32,
) -> Value {
    eval.def_finish(id, sym, doc, value, name_meta, Span::new(line, col))
}

/// A vector literal.
#[must_use]
pub fn vec_lit(items: Vec<Value>, meta: Option<Map>) -> Value {
    Value::Vector(Vector::from_vec(items).with_meta(meta))
}

/// A map literal over evaluated `[k0 v0 k1 v1 …]` (keys realized,
/// last wins, like the interpreter).
pub fn map_lit(eval: &mut Evaluator, kvs: &[Value], meta: Option<Map>) -> Step {
    Ok(Value::Map(map_pairs(eval, kvs)?.with_meta(meta)))
}

/// `[k0 v0 …]` as a map (shared by map literals and `def` metadata).
pub fn map_pairs(eval: &mut Evaluator, kvs: &[Value]) -> Result<Map, Signal> {
    let mut pairs = Vec::with_capacity(kvs.len() / 2);
    for kv in kvs.chunks(2) {
        let key = crate::lazy::realize_key(eval, &kv[0])?;
        pairs.push((key, kv.get(1).cloned().unwrap_or(Value::Nil)));
    }
    Ok(Map::from_kvs_last_wins(pairs))
}

/// A set literal (elements realized, like the interpreter).
pub fn set_lit(eval: &mut Evaluator, items: &[Value], meta: Option<Map>) -> Step {
    let mut out = clj_value::new_set();
    for item in items {
        out = out.insert(crate::lazy::realize_key(eval, item)?);
    }
    Ok(Value::Set(out.with_meta(meta)))
}

/// `(throw v)`.
pub fn throw(eval: &mut Evaluator, value: Value) -> Signal {
    eval.throw_value(value)
}

/// A throwable as the value a `catch` binds.
#[must_use]
pub fn caught(thrown: Throwable) -> Value {
    Value::Throwable(Arc::new(thrown))
}

/// Whether a throw matches a `catch` class name.
#[must_use]
pub fn catch_matches(thrown: &Throwable, class: &str) -> bool {
    thrown.class.matches(class)
}

/// A fresh throwable of `class` (the analyzer's `NewThrowable`).
pub fn new_throwable(eval: &mut Evaluator, class: ThrowClass, message: &str) -> Value {
    Value::Throwable(Arc::new(Throwable {
        id: eval.next_id(),
        class,
        message: Some(Arc::from(message)),
        data: None,
        cause: None,
    }))
}

/// `(try body (catch C e handler)… (finally fin))` over natively
/// compiled zero-arg `body`/`fin` fns and one-arg handlers, with the
/// interpreter's ordering: finally after the body or the handler,
/// a throwing finally wins.
pub fn try_call(
    eval: &mut Evaluator,
    body: &Value,
    catches: &[(String, Value)],
    fin: Option<&Value>,
) -> Step {
    let run_fin = |eval: &mut Evaluator| -> Result<(), Signal> {
        match fin {
            Some(f) => eval.call(f, &[]).map(|_| ()),
            None => Ok(()),
        }
    };
    let thrown = match eval.call(body, &[]) {
        Ok(value) => {
            run_fin(eval)?;
            return Ok(value);
        }
        Err(Signal::Throw(t)) => t,
        Err(other) => return Err(other),
    };
    for (class, handler) in catches {
        if thrown.class.matches(class) {
            let result = eval.call(handler, &[caught(thrown.clone())]);
            run_fin(eval)?;
            return result;
        }
    }
    run_fin(eval)?;
    Err(Signal::Throw(thrown))
}

/// `(binding [v x …] body)` over evaluated values.
pub fn binding_call(eval: &mut Evaluator, pairs: Vec<(Var, Value)>, body: &Value) -> Step {
    for (var, _) in &pairs {
        if !eval.registry.is_dynamic(var.id) {
            return Err(eval.throw(NativeError::new(
                ThrowClass::IllegalStateException,
                format!("Can't dynamically bind non-dynamic var: {}", var.sym),
            )));
        }
    }
    let depth = eval.bindings_depth();
    eval.push_frame(pairs);
    let result = eval.call(body, &[]);
    eval.pop_bindings(depth);
    result
}

/// `(set! var v)`.
pub fn set_var(eval: &mut Evaluator, var: &Var, value: Value) -> Step {
    if eval.set_binding_top(var.id, value.clone()) {
        Ok(value)
    } else {
        Err(eval.throw(NativeError::new(
            ThrowClass::IllegalStateException,
            format!(
                "Can't change/establish root binding of: {} with set",
                var.sym.name()
            ),
        )))
    }
}

/// `(declare a b …)`: each name defined rootless with `:declared`.
pub fn declare(eval: &mut Evaluator, syms: &[Symbol], line: u32, col: u32) -> Value {
    let mut last = Value::Nil;
    for sym in syms {
        let meta = sym.meta().cloned().unwrap_or_else(new_map).insert(
            Value::Keyword(Keyword::new(None, "declared")),
            Value::Bool(true),
        );
        let sym = sym.clone().with_meta(Some(meta));
        let id = eval.def_begin(&sym);
        last = eval.def_finish(id, &sym, None, None, None, Span::new(line, col));
    }
    last
}

/// `(ns name …)` with its data clauses.
pub fn ns(
    eval: &mut Evaluator,
    name: &str,
    doc: Option<&str>,
    attr: Option<&Map>,
    clauses: &[NsClause],
) -> Step {
    crate::ns::eval_ns_form(eval, name, doc, attr, clauses)
}

/// The `case*` dispatch: `Ok(None)` takes the default; `Ok(Some(i))`
/// takes case `i`, whose test (when `checked`) the caller must still
/// confirm with [`equiv`] against the returned realized key.
pub fn case_select(
    eval: &mut Evaluator,
    value: &Value,
    shift: i32,
    mask: i32,
    keys: &[i32],
    skip: &[i32],
    test: CaseTest,
) -> Result<CaseHit, Signal> {
    if test == CaseTest::Int {
        let Some(key) = crate::eval::case_int_value(value) else {
            return Ok(CaseHit::Default);
        };
        let key = crate::eval::case_shift_mask(key, shift, mask);
        return Ok(match keys.iter().position(|k| *k == key) {
            None => CaseHit::Default,
            Some(i) => CaseHit::Check(i, value.clone()),
        });
    }
    if clj_value::seq_hash_unsupported(value) {
        return Err(eval.unsupported_hash());
    }
    let realized = crate::lazy::realize_deep(eval, value)?;
    if clj_value::seq_hash_unsupported(&realized) {
        return Err(eval.unsupported_hash());
    }
    let key = crate::eval::case_shift_mask(clj_value::hash_code(&realized), shift, mask);
    Ok(match keys.iter().position(|k| *k == key) {
        None => CaseHit::Default,
        Some(i) if skip.contains(&keys[i]) => CaseHit::Direct(i),
        Some(i) => CaseHit::Check(i, realized),
    })
}

/// A [`case_select`] outcome.
#[derive(Debug)]
pub enum CaseHit {
    /// Take the default branch.
    Default,
    /// Take case `i` without a test check.
    Direct(usize),
    /// Take case `i` only if its test `equiv`s the realized key.
    Check(usize, Value),
}

/// Clojure `=` (`equiv`) as used by `case*`.
pub fn equiv(eval: &mut Evaluator, a: &Value, b: &Value) -> Result<bool, Signal> {
    crate::lazy::equiv_vals(eval, a, b)
}

/// Positional destructuring: `(nth coll i nil)`.
pub fn nth_or_nil(eval: &mut Evaluator, coll: &Value, i: usize) -> Step {
    crate::eval::nth_or_nil(coll, i, eval)
}

/// `&`-destructuring's `gseq`.
pub fn gseq(eval: &mut Evaluator, value: &Value) -> Step {
    eval.destructure_gseq(value)
}

/// `(first s)`.
pub fn first(eval: &mut Evaluator, s: &Value) -> Step {
    crate::lazy::first_value(eval, s)
}

/// `(next s)`.
pub fn next(eval: &mut Evaluator, s: &Value) -> Step {
    crate::lazy::next_value(eval, s)
}

/// Rest args of a variadic call as the interpreter passes them.
#[must_use]
pub fn rest_args(args: &[Value]) -> Value {
    crate::eval::rest_list(args)
}

/// The arity error text the interpreter raises for `display`.
pub fn arity_error(eval: &mut Evaluator, display: &str, got: usize) -> Signal {
    eval.throw(NativeError::arity(display, got))
}

/// A `NativeError` as a pending throw.
pub fn native_throw(eval: &mut Evaluator, err: NativeError) -> Signal {
    eval.throw(err)
}

/// A pending throw as the `NativeError` a native fn returns to the
/// interpreter (the throwable rides along unchanged).
#[must_use]
pub fn signal_to_native(signal: Signal) -> NativeError {
    crate::ns::signal_to_native(signal)
}

/// Run one native fn invocation under the interpreter's call guard
/// (call-depth limit raising `StackOverflowError`, segmented stack
/// growth), so deep native recursion behaves exactly like `run`.
pub fn guarded(eval: &mut Evaluator, f: impl FnOnce(&mut Evaluator) -> Step) -> Step {
    eval.guarded_call(f)
}

/// The throwable class a catch/`NewThrowable` names (JVM spelling,
/// simple or qualified; unknown names fall back like the
/// interpreter).
#[must_use]
pub fn throw_class(name: &str) -> ThrowClass {
    crate::ns::throw_class_for(name)
}

/// Read one printed constant (the reader fallback for constants with
/// no direct builder: collections, metadata, big numbers…).
pub fn read_constant(text: &str) -> Result<Value, String> {
    clj_reader::read_str(
        text,
        &clj_reader::ReaderOpts::default(),
        &mut clj_reader::NoHooks,
    )
    .map_err(|e| e.message())
}

/// Print a constant for [`read_constant`], with metadata.
#[must_use]
pub fn print_constant(value: &Value) -> String {
    let opts = clj_value::PrintOpts {
        meta: true,
        ..clj_value::PrintOpts::pr()
    };
    clj_value::pr_to_string(value, &opts)
}

/// The `(class, field)` a static-field constant came from, when the
/// analyzer folded one into a literal (so native code can re-resolve
/// it at startup instead of printing it).
#[must_use]
pub fn static_field_origin(value: &Value) -> Option<(&'static str, &'static str)> {
    crate::classemu::static_field_origin(value)
}

/// Re-resolve a static field (`java.util.Locale/US`, …).
#[must_use]
pub fn static_field(class: &str, field: &str) -> Option<Value> {
    crate::classemu::static_field_value(class, field)
}

// ---- generic destructuring ----------------------------------------------

/// Encode a binding pattern as constant data for
/// [`destructure_encoded`], returning the `:or` default expressions in
/// the order they must be evaluated (pre-order) and passed back.
#[must_use]
pub fn encode_pattern(pat: &crate::expr::Pattern) -> (Value, Vec<&crate::expr::Expr>) {
    let mut defaults = Vec::new();
    let v = enc_pattern(pat, &mut defaults);
    (v, defaults)
}

fn kw(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

fn opt_str(s: Option<&String>) -> Value {
    s.map_or(Value::Nil, |s| Value::Str(Arc::from(s.as_str())))
}

fn enc_pattern<'a>(pat: &'a crate::expr::Pattern, defaults: &mut Vec<&'a crate::expr::Expr>) -> Value {
    use crate::expr::Pattern;
    match pat {
        Pattern::Symbol(name) => Value::Str(Arc::from(name.as_str())),
        Pattern::Vec(v) => {
            let fixed = v.fixed.iter().map(|p| enc_pattern(p, defaults)).collect();
            let rest = v.rest.as_ref().map_or(Value::Nil, |p| enc_pattern(p, defaults));
            vec_lit(vec![kw("vec"), vec_lit(fixed, None), rest, opt_str(v.as_sym.as_ref())], None)
        }
        Pattern::Map(m) => {
            let entries = m
                .entries
                .iter()
                .map(|(p, key, or_key, req)| {
                    vec_lit(vec![enc_pattern(p, defaults), key.clone(), or_key.clone(), Value::Bool(*req)], None)
                })
                .collect();
            let keys = m
                .keys
                .iter()
                .map(|k| vec_lit(vec![Value::Str(Arc::from(k.local.as_str())), k.key.clone(), Value::Bool(k.required)], None))
                .collect();
            let or_keys = m
                .or
                .iter()
                .map(|(key, e)| {
                    defaults.push(e);
                    key.clone()
                })
                .collect();
            vec_lit(
                vec![
                    kw("map"),
                    vec_lit(entries, None),
                    vec_lit(keys, None),
                    vec_lit(m.extra_keys.clone(), None),
                    opt_str(m.as_sym.as_ref()),
                    vec_lit(or_keys, None),
                    opt_str(m.select.as_ref()),
                    opt_str(m.all.as_ref()),
                    opt_str(m.excess.as_ref()),
                    opt_str(m.missing.as_ref()),
                ],
                None,
            )
        }
    }
}

fn dec_str(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn dec_items(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Vector(v)) => v.iter().collect(),
        _ => Vec::new(),
    }
}

fn dec_pattern(v: &Value, defaults: &mut std::slice::Iter<'_, Value>) -> crate::expr::Pattern {
    use crate::expr::{Expr, KeyBinding, MapPattern, Pattern, VecPattern};
    let items = match v {
        Value::Str(s) => return Pattern::Symbol(s.to_string()),
        Value::Vector(v) => v.iter().collect::<Vec<_>>(),
        _ => return Pattern::Symbol(String::new()),
    };
    let is_vec = matches!(items.first(), Some(Value::Keyword(k)) if k.name() == "vec");
    if is_vec {
        let fixed = dec_items(items.get(1)).iter().map(|p| dec_pattern(p, defaults)).collect();
        let rest = match items.get(2) {
            Some(Value::Nil) | None => None,
            Some(p) => Some(Box::new(dec_pattern(p, defaults))),
        };
        return Pattern::Vec(VecPattern {
            fixed,
            rest,
            as_sym: dec_str(items.get(3)),
        });
    }
    let entries = dec_items(items.get(1))
        .iter()
        .map(|e| {
            let parts = dec_items(Some(e));
            let p = dec_pattern(&parts[0], defaults);
            (p, parts[1].clone(), parts[2].clone(), parts[3].is_truthy())
        })
        .collect();
    let keys = dec_items(items.get(2))
        .iter()
        .map(|k| {
            let parts = dec_items(Some(k));
            KeyBinding {
                local: dec_str(parts.first()).unwrap_or_default(),
                key: parts[1].clone(),
                required: parts[2].is_truthy(),
            }
        })
        .collect();
    let or = dec_items(items.get(5))
        .into_iter()
        .map(|key| (key, Expr::Literal(defaults.next().cloned().unwrap_or(Value::Nil))))
        .collect();
    Pattern::Map(MapPattern {
        entries,
        keys,
        extra_keys: dec_items(items.get(3)),
        as_sym: dec_str(items.get(4)),
        or,
        select: dec_str(items.get(6)),
        all: dec_str(items.get(7)),
        excess: dec_str(items.get(8)),
        missing: dec_str(items.get(9)),
    })
}

/// Destructure `value` against an [`encode_pattern`]-encoded pattern
/// with its `:or` defaults already evaluated, returning the bound
/// values in binding order — the interpreter's own `destructure`.
pub fn destructure_encoded(eval: &mut Evaluator, encoded: &Value, value: &Value, defaults: &[Value]) -> Result<Vec<Value>, Signal> {
    let pat = dec_pattern(encoded, &mut defaults.iter());
    let mut out = Vec::new();
    eval.destructure_in(&pat, value, &crate::env::Env::new(), &mut out)?;
    Ok(out)
}

/// How many locals a pattern binds.
#[must_use]
pub fn pattern_local_count(pat: &crate::expr::Pattern) -> usize {
    crate::expr::pattern_names(pat).len()
}

/// A class value by fully qualified name (folded class constants).
pub fn class_named(eval: &mut Evaluator, name: &str) -> Value {
    crate::classemu::resolve_class_name(eval, name).map_or(Value::Nil, Value::Class)
}

/// The name of a class constant.
#[must_use]
pub fn class_name_of(value: &Value) -> Option<String> {
    match value {
        Value::Class(c) => Some(c.name().to_owned()),
        _ => None,
    }
}
