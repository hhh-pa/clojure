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
use std::sync::atomic::AtomicU64;

use clj_reader::Span;
use clj_value::{
    BindCounts, Keyword, List, Map, RangeNum, Set, Symbol, ThrowClass, Throwable, Value, Var,
    Vector, new_map,
};

use crate::error::{NativeError, Signal};
use crate::eval::Evaluator;
use crate::expr::{CaseTest, NsClause};

/// The result of every step: a value or a pending throw.
pub type Step = Result<Value, Signal>;

/// Call `f` with `args` exactly like an interpreted call.
pub fn invoke(eval: &mut Evaluator, f: &Value, args: &[Value]) -> Step {
    // A native (a compiled closure included) runs straight away: the
    // general call reaches the same `func` after a VM-closure hook, a
    // struct check and a second downcast (#271: ~4x a direct call).
    if let Some(native) = crate::ext::as_native(f) {
        return native.func.call(eval, args).map_err(|e| eval.throw(e));
    }
    if let Some(found) = lookup_call(f, args) {
        return Ok(found);
    }
    eval.call(f, args)
}

/// A map, set, keyword or symbol called on one arg, or two with a
/// default (#271), through [`lookup_value_call`]. Other arities take the
/// general call (for their arity errors).
fn lookup_call(f: &Value, args: &[Value]) -> Option<Value> {
    match args {
        [a] => lookup_value_call(f, a, None),
        [a, not_found] => lookup_value_call(f, a, Some(not_found)),
        _ => None,
    }
}

/// `(f a)` / `(f a not-found)` when `f` is a map, set, keyword or symbol
/// (#271): the lookup `Evaluator::invoke` ends in (`invoke_map`,
/// `invoke_set`, `invoke_keyed_name`), without the general call. `None`
/// for any other callee.
#[must_use]
pub fn lookup_value_call(f: &Value, a: &Value, not_found: Option<&Value>) -> Option<Value> {
    let found = match f {
        Value::Map(_) => crate::eval::lookup(f, a),
        Value::Set(s) => crate::eval::member(s, a),
        Value::Keyword(_) | Value::Symbol(_) => crate::eval::lookup(a, f),
        _ => return None,
    };
    Some(found.or_else(|| not_found.cloned()).unwrap_or(Value::Nil))
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

/// Whether `def`s keep var metadata (doc, auto keys): a Lite program
/// that cannot read var metadata keeps only the name's own keys.
#[must_use]
pub fn keeps_var_meta(eval: &Evaluator) -> bool {
    !eval.skip_var_meta
}

/// `(def sym …)` first half: the target var's id, before the init
/// runs (recursive `defn`s see their own var).
pub fn def_begin(eval: &mut Evaluator, sym: &Symbol) -> u64 {
    eval.def_begin(sym)
}

/// [`def_begin`] for a target var already interned (a compiled plain
/// def's var slot: no symbol constant, no second intern).
pub fn def_begin_var(eval: &mut Evaluator, id: u64) -> u64 {
    eval.def_begin_id(id)
}

/// `(def sym …)` second half: root, metadata, dynamic flag; returns
/// the var.
#[allow(clippy::too_many_arguments)]
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
    Value::Vector(match meta {
        Some(m) => Vector::from_vec(items).with_meta(Some(m)),
        None => Vector::from_vec(items),
    })
}

fn as_int_val(v: &Value) -> Option<i64> {
    match *v {
        Value::Int(n) => Some(n),
        _ => None,
    }
}

fn as_double_val(v: &Value) -> Option<f64> {
    match *v {
        Value::Double(d) => Some(d),
        _ => None,
    }
}

/// [`vec_lit`] taking the evaluated items over (each non-POD slot left nil,
/// #271): no clone per item, and all-long / all-double items go straight to
/// packed storage (no intermediate boxed vector).
pub fn vec_lit_owned(items: &mut [Value], meta: Option<Map>) -> Value {
    // All-long and all-double literals pack in a single classification pass
    // without touching the caller's POD slots again (#271 Slice 5n);
    // everything else moves the items out. `with_meta(None)` would
    // re-`make_mut` a fresh vector and mint a second identity for nothing,
    // so it is skipped.
    let vec = match items.first() {
        Some(Value::Int(_)) if items.iter().all(|v| as_int_val(v).is_some()) => {
            Vector::from_ints(items.iter().filter_map(as_int_val).collect())
        }
        Some(Value::Double(_)) if items.iter().all(|v| as_double_val(v).is_some()) => {
            Vector::from_doubles(items.iter().filter_map(as_double_val).collect())
        }
        _ => Vector::from_vec(
            items
                .iter_mut()
                .map(|v| std::mem::replace(v, Value::Nil))
                .collect(),
        ),
    };
    Value::Vector(match meta {
        Some(m) => vec.with_meta(Some(m)),
        None => vec,
    })
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

/// A keyword map literal (#271): the `{k nil …}` template constant's
/// keys over `vals`, sharing its interned shape (a non-map template,
/// which the compiler never emits, builds nil).
#[must_use]
pub fn kw_map(template: &Value, vals: &[Value]) -> Value {
    match template {
        Value::Map(t) => Value::Map(Map::with_vals(t, vals.to_vec())),
        _ => Value::Nil,
    }
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
    let signal = match eval.call(body, &[]) {
        Ok(value) => {
            run_fin(eval)?;
            return Ok(value);
        }
        Err(signal) => signal,
    };
    // Catches match throws only; anything else unwinds through the
    // finally like an unmatched throw.
    let thrown = thrown_of(&signal);
    for (class, handler) in catches {
        if let Some(thrown) = thrown.filter(|t| t.class.matches(class)) {
            let result = eval.call(handler, &[caught(thrown.clone())]);
            run_fin(eval)?;
            return result;
        }
    }
    run_fin(eval)?;
    Err(signal)
}

/// The throwable a signal carries (`None` for a non-throw signal).
fn thrown_of(signal: &Signal) -> Option<&Throwable> {
    match signal {
        Signal::Throw(t) => Some(t),
        Signal::Recur(_) => None,
    }
}

/// `(binding [v x …] body)` over evaluated values.
pub fn binding_call(eval: &mut Evaluator, pairs: Vec<(Var, Value)>, body: &Value) -> Step {
    binding_call_with(eval, pairs, &|ev: &mut Evaluator| ev.call(body, &[]))
}

/// The `binding` core: non-dynamic vars refuse, then one frame
/// around `body`.
pub(crate) fn binding_call_with(
    eval: &mut Evaluator,
    pairs: Vec<(Var, Value)>,
    body: crate::stm::TxBody<'_>,
) -> Step {
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
    let result = body(eval);
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

/// `^{k v …} ns/name` exactly as [`read_constant`] builds it — the
/// metadata pairs (`kvs`, interleaved) inserted one by one into an empty
/// map, like the reader's merge — so pooled symbol constants with
/// metadata (`defn-`'s `{:private true}`) skip the reader (#271).
#[must_use]
pub fn meta_symbol(ns: Option<&str>, name: &str, kvs: &[Value]) -> Value {
    let mut meta = new_map();
    for pair in kvs.chunks(2) {
        meta = meta.insert(pair[0].clone(), pair[1].clone());
    }
    Value::Symbol(Symbol::new(ns, name)).with_meta(Some(meta))
}

/// A metadata-free constant collection over its items, built by the
/// reader's own constructors (#271: pooled constants skip the reader).
/// `kind`: 0 list, 1 vector, 2 map (`items` interleave keys and
/// values), anything else a set.
#[must_use]
pub fn const_coll(kind: u64, items: &[Value]) -> Value {
    match kind {
        0 if items.is_empty() => Value::List(List::new_sync()),
        0 => Value::List(List::from_vec(items.to_vec())),
        1 => Value::Vector(Vector::from_vec(items.to_vec())),
        2 => {
            let pairs = items
                .chunks(2)
                .map(|p| (p[0].clone(), p[1].clone()))
                .collect();
            Value::Map(Map::from_kvs_checked(pairs).expect("constant map keys are distinct"))
        }
        _ => Value::Set(
            Set::from_items_checked(items.to_vec()).expect("constant set items are distinct"),
        ),
    }
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

fn enc_pattern<'a>(
    pat: &'a crate::expr::Pattern,
    defaults: &mut Vec<&'a crate::expr::Expr>,
) -> Value {
    use crate::expr::Pattern;
    match pat {
        Pattern::Symbol(name) => Value::Str(Arc::from(name.as_str())),
        Pattern::Vec(v) => {
            let fixed = v.fixed.iter().map(|p| enc_pattern(p, defaults)).collect();
            let rest = v
                .rest
                .as_ref()
                .map_or(Value::Nil, |p| enc_pattern(p, defaults));
            vec_lit(
                vec![
                    kw("vec"),
                    vec_lit(fixed, None),
                    rest,
                    opt_str(v.as_sym.as_ref()),
                ],
                None,
            )
        }
        Pattern::Map(m) => {
            let entries = m
                .entries
                .iter()
                .map(|(p, key, or_key, req)| {
                    vec_lit(
                        vec![
                            enc_pattern(p, defaults),
                            key.clone(),
                            or_key.clone(),
                            Value::Bool(*req),
                        ],
                        None,
                    )
                })
                .collect();
            let keys = m
                .keys
                .iter()
                .map(|k| {
                    vec_lit(
                        vec![
                            Value::Str(Arc::from(k.local.as_str())),
                            k.key.clone(),
                            Value::Bool(k.required),
                        ],
                        None,
                    )
                })
                .collect();
            let or_keys =
                m.or.iter()
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
        let fixed = dec_items(items.get(1))
            .iter()
            .map(|p| dec_pattern(p, defaults))
            .collect();
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
        .map(|key| {
            (
                key,
                Expr::Literal(defaults.next().cloned().unwrap_or(Value::Nil)),
            )
        })
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
pub fn destructure_encoded(
    eval: &mut Evaluator,
    encoded: &Value,
    value: &Value,
    defaults: &[Value],
) -> Result<Vec<Value>, Signal> {
    let pat = dec_pattern(encoded, &mut defaults.iter());
    let mut out = Vec::new();
    eval.destructure_in(&pat, value, &crate::env::Env::new(), &mut out)?;
    Ok(out)
}

/// A map-destructure source as the bindings read it (#271): a list
/// converts to its map (`seq_to_map`), anything else is itself.
pub fn destructure_source(eval: &mut Evaluator, value: &Value) -> Result<Value, Signal> {
    eval.seq_to_map(value)
}

/// A map-destructure binding's lookup in a converted source (#271):
/// present keys only (a present `nil` stays `nil`), over the shapes the
/// interpreter's own binding reads (records, maps, map entries,
/// int-keyed vectors and strings, sets).
#[must_use]
pub fn destructure_lookup(coll: &Value, key: &Value) -> Option<Value> {
    crate::eval::lookup(coll, key)
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

/// The registry's root epoch (bumps on every var root change).
#[must_use]
pub fn root_epoch(eval: &Evaluator) -> (u64, u64) {
    (eval.registry.epoch(), eval.bindings.version())
}

/// The registry's standalone root-generation counter (even outside
/// mutations).
#[must_use]
pub fn registry_epoch(eval: &Evaluator) -> u64 {
    eval.registry.epoch()
}

/// Share the evaluator's atomic root-generation counter.
#[must_use]
pub fn registry_epoch_handle(eval: &Evaluator) -> Arc<AtomicU64> {
    eval.registry.epoch_handle()
}

/// Share the process-wide per-var binding refcount table.
#[must_use]
pub fn bind_counts() -> Arc<BindCounts> {
    crate::env::bind_counts()
}

/// The argument-count bit a caller of compiled code sets when it hands
/// its argument array over (`call_owned`): the callee may then take a
/// param it consumes (moving it out, leaving nil) instead of cloning it,
/// so e.g. `reduce`'s accumulator reaches `assoc` uniquely owned (#271).
/// The count itself is the low bits.
pub const TAKE_ARGS: u64 = 1 << 62;

/// Per-argument ownership bit shift (`TAKE_ARG_SHIFT + (i & 31)`):
/// a compiled caller sets bit `30 + i` in `n` when `argv[i]` is an owned
/// temporary or moved last-use local that the callee may move out in place.
pub const TAKE_ARG_SHIFT: u32 = 30;

/// The single-argument ownership bit in `n` for parameter `i`.
#[inline]
#[must_use]
pub const fn single_take_arg_bit(i: usize) -> u64 {
    1u64 << (TAKE_ARG_SHIFT + ((i & 31) as u32))
}

/// The bitmask in `n` allowing parameter `i` to be moved out of `argv[i]`:
/// either [`TAKE_ARGS`] (all args owned) or [`single_take_arg_bit`].
#[inline]
#[must_use]
pub const fn take_arg_mask(i: usize) -> u64 {
    TAKE_ARGS | single_take_arg_bit(i)
}

/// Parse a JVM `Long/parseLong` base-10 string (`None` on malformed or out-of-range).
#[must_use]
pub fn parse_long_str(s: &str) -> Option<i64> {
    crate::core::parse_long_str(s)
}

/// Parse a JVM `Double/parseDouble` string (`None` on malformed input).
#[must_use]
pub fn parse_double_str(s: &str) -> Option<f64> {
    crate::core::parse_double_str(s)
}

/// Call `callee` (a cached var root) with `args` by value: a native
/// runs its entry directly (an owned-args entry gets them moved in —
/// no per-arg clone/drop, and head-releasing loopers can free as they
/// walk); anything else takes the general call.
pub fn call_value_owned(
    eval: &mut Evaluator,
    callee: &Value,
    args: smallvec::SmallVec<[Value; 4]>,
) -> Step {
    if let Some(native) = crate::ext::as_native(callee) {
        return native.call_owned(eval, args).map_err(|e| eval.throw(e));
    }
    eval.call(callee, &args)
}

// ---- intrinsic steps (borrowed args, no var lookup) -------------------
// Valid only where the compiled program cannot rebind the core var;
// each mirrors the core native's own fast path and falls back to it.

/// Enter a direct native-to-native call (see
/// `Evaluator::enter_native_call`).
pub fn enter_call(eval: &mut Evaluator) -> bool {
    eval.enter_native_call()
}

/// Leave a call entered with [`enter_call`].
pub fn leave_call(eval: &mut Evaluator) {
    eval.leave_native_call();
}

/// A core native's entries, resolved once (the compiled program
/// cannot rebind core vars when it uses these).
pub struct CoreEntry {
    func: crate::ext::NativeTarget,
    owned: Option<crate::ext::OwnedTarget>,
}

impl CoreEntry {
    /// Call the native over borrowed args, as [`call_core`] does (#271):
    /// an owned-args native takes clones by value, a borrowing one gets
    /// them in a stack array (one or a few args) — one entry resolution
    /// either way, where an owned native used to be cloned for, refused,
    /// cloned again and resolved a second time.
    pub fn call_refs(&self, eval: &mut Evaluator, args: &[&Value]) -> Result<Value, NativeError> {
        if let Some(owned) = &self.owned {
            return owned.call(eval, args.iter().map(|a| (*a).clone()).collect());
        }
        match *args {
            [a] => self.func.call(eval, std::slice::from_ref(a)),
            [a, b] => self.func.call(eval, &[a.clone(), b.clone()]),
            [a, b, c] => self.func.call(eval, &[a.clone(), b.clone(), c.clone()]),
            _ => {
                let owned: smallvec::SmallVec<[Value; 4]> =
                    args.iter().map(|a| (*a).clone()).collect();
                self.func.call(eval, &owned)
            }
        }
    }
}

/// A Lite native's [`CoreEntry`] straight from the registered tables —
/// compiled code calling it directly needs no var, so its install stays
/// lazy (`None`: not a Lite native; see `aot_boot::lite_entry`).
#[must_use]
pub fn lite_core_entry(
    decl: (&str, &str, u32),
    tables: crate::aot_boot::LiteTables,
) -> Option<CoreEntry> {
    crate::aot_boot::lite_entry(decl, tables).map(|(func, owned)| CoreEntry { func, owned })
}

/// Resolve var `var`'s root as a [`CoreEntry`] (`None`: not a native).
pub fn core_entry(eval: &mut Evaluator, var: &Var) -> Option<CoreEntry> {
    let root = eval.deref_var_cached(var.id, &var.sym);
    crate::ext::as_native(&root).map(|n| CoreEntry {
        func: n.func.clone(),
        owned: n.owned.clone(),
    })
}

/// Call a resolved core entry with moved args.
pub fn call_core(
    eval: &mut Evaluator,
    entry: &CoreEntry,
    args: smallvec::SmallVec<[Value; 4]>,
) -> Step {
    match &entry.owned {
        Some(owned) => owned.call(eval, args).map_err(|e| eval.throw(e)),
        None => entry.func.call(eval, &args).map_err(|e| eval.throw(e)),
    }
}

/// The VM's inline op for core fn `name` called with `arity` args (the
/// id for [`apply_inline`]), if it has one.
#[must_use]
pub fn inline_op(name: &str, arity: usize) -> Option<u32> {
    use crate::vm_ops::{APPLY1, APPLY2, APPLY3, APPLYN, RE_MATCHES_OP};
    if arity == 2 && name == RE_MATCHES_OP.0 {
        return Some(RE_MATCHES_TAG);
    }
    let pos = match arity {
        1 => APPLY1.iter().position(|r| r.0 == name),
        2 => APPLY2.iter().position(|r| r.0 == name),
        3 => APPLY3.iter().position(|r| r.0 == name),
        _ => None,
    };
    let nary = || {
        APPLYN
            .iter()
            .position(|r| r.0 == name)
            .filter(|_| arity >= 3)
            .map(|i| i as u32 | NARY_OP)
    };
    pos.map(|i| i as u32).or_else(nary)
}

/// [`inline_op`]'s mark for a 3+-argument fold or comparison chain.
const NARY_OP: u32 = 1 << 30;
/// Dedicated inline tag for `(re-matches pat s)` so `APPLY2` stays regex-free.
const RE_MATCHES_TAG: u32 = 1 << 29;

/// Bit mask covering disjoint op-namespace tag bits so [`RE_MATCHES_TAG`],
/// [`NARY_OP`], and [`OWN_BIT`] can never alias inside shared `u32` dispatch helpers.
const TAG_MASK: u32 = RE_MATCHES_TAG | NARY_OP | OWN_BIT;

/// Whether `id` is the dedicated regex inline op (`(re-matches ...)`).
#[must_use]
pub const fn is_re_op(id: u32) -> bool {
    id == RE_MATCHES_TAG
}

/// Apply inline op `id` ([`inline_op`] at `args.len()`) to borrowed
/// args — the same inline answers the VM gives (excluding regex ops so
/// non-regex AOT binaries strip `regex_automata`).
#[must_use]
pub fn apply_inline(id: u32, args: &[&Value]) -> Option<Value> {
    use crate::vm_ops::APPLYN;
    match args {
        [a] => apply1(id, a),
        [a, b] => apply2_no_re(id, a, b),
        [a, b, c] => apply3(id, a, b, c),
        _ if id != u32::MAX && (id & TAG_MASK) == NARY_OP => {
            let (_, op, chain) = APPLYN.get((id & !NARY_OP) as usize)?;
            nary(*op, *chain, args)
        }
        _ => None,
    }
}

/// [`apply_inline`] for one arg.
#[must_use]
pub fn apply1(id: u32, a: &Value) -> Option<Value> {
    crate::vm_ops::APPLY1.get(id as usize)?.1(a)
}

/// [`apply_inline`] for two args (including `re-matches`).
#[must_use]
pub fn apply2(id: u32, a: &Value, b: &Value) -> Option<Value> {
    if id == RE_MATCHES_TAG {
        return apply_re_matches(a, b);
    }
    apply2_no_re(id, a, b)
}

/// [`apply_inline`] for two args excluding `re-matches` (keeps `regex_automata`
/// stripped from non-regex AOT binaries).
#[must_use]
pub fn apply2_no_re(id: u32, a: &Value, b: &Value) -> Option<Value> {
    crate::vm_ops::APPLY2.get(id as usize)?.1(a, b)
}

/// Inline `(re-matches pat s)` fast path for `rt_n_op2_re`.
#[must_use]
pub fn apply_re_matches(a: &Value, b: &Value) -> Option<Value> {
    (crate::vm_ops::RE_MATCHES_OP.1)(a, b)
}

/// Cached O(1) ASCII check for strings (memoizes long strings after one scan).
#[must_use]
pub fn str_is_ascii(s: &Arc<str>) -> bool {
    crate::coll::str_ascii(s)
}

/// [`apply_inline`] for three args (a ternary op, or a 3-arg fold or
/// chain).
#[must_use]
pub fn apply3(id: u32, a: &Value, b: &Value, c: &Value) -> Option<Value> {
    use crate::vm_ops::{APPLY3, APPLYN};
    if id != u32::MAX && (id & TAG_MASK) == NARY_OP {
        let (_, op, chain) = APPLYN.get((id & !NARY_OP) as usize)?;
        return nary(*op, *chain, &[a, b, c]);
    }
    APPLY3.get(id as usize)?.1(a, b, c)
}

/// A fold of binary `op` over `args`, or (`chain`) a comparison chain
/// stopping at the first false pair (like `clojure.core/<`).
fn nary(op: crate::vm_ops::Apply2, chain: bool, args: &[&Value]) -> Option<Value> {
    let (first, rest) = args.split_first()?;
    if chain {
        let mut prev = *first;
        for x in rest {
            if !op(prev, x)?.is_truthy() {
                return Some(Value::Bool(false));
            }
            prev = x;
        }
        return Some(Value::Bool(true));
    }
    rest.iter().try_fold((*first).clone(), |acc, x| op(&acc, x))
}

// ---- extent forms over compiled body fns --------------------------------

fn body_of(body: &Value) -> impl Fn(&mut Evaluator) -> Step + '_ {
    move |ev: &mut Evaluator| ev.call(body, &[])
}

/// `with-redefs`.
pub fn with_redefs_call(eval: &mut Evaluator, vals: Vec<(Var, Value)>, body: &Value) -> Step {
    eval.with_redefs_core(vals, &body_of(body))
}

/// `with-bindings*`.
pub fn with_bindings_call(eval: &mut Evaluator, map: &Value, body: &Value) -> Step {
    eval.with_bindings_core(map, &body_of(body))
}

/// `time`.
pub fn time_call(eval: &mut Evaluator, body: &Value) -> Step {
    eval.time_core(&body_of(body))
}

/// `with-out-str` (binding `out`, `*out*` as written).
pub fn with_out_str_call(eval: &mut Evaluator, out: &Symbol, body: &Value) -> Step {
    eval.with_out_str_core(out, &body_of(body))
}

/// `with-in-str`.
pub fn with_in_str_call(eval: &mut Evaluator, input: &Symbol, s: &Value, body: &Value) -> Step {
    eval.with_in_str_core(input, s, &body_of(body))
}

/// `locking`.
pub fn locking_call(eval: &mut Evaluator, target: &Value, body: &Value) -> Step {
    eval.locking_core(target, &body_of(body))
}

/// `sync` (`dosync`): the body re-runs on retry.
pub fn sync_call(eval: &mut Evaluator, body: &Value) -> Step {
    crate::stm::eval_sync(eval, &body_of(body))
}

/// `io!`.
pub fn io_call(eval: &mut Evaluator, body: &Value) -> Step {
    crate::stm::eval_io(eval, &body_of(body))
}

/// `lazy-seq` over a compiled zero-arg body fn.
#[must_use]
pub fn lazy_seq(f: Value) -> Value {
    crate::lazy::call0_value(f)
}

/// `delay` over a compiled zero-arg body fn.
#[must_use]
pub fn delay(f: Value) -> Value {
    Value::Delay(clj_value::Delay::new(f))
}

/// `bound-fn`: `f` re-pushes the creation-time thread bindings around
/// every call.
pub fn bound_fn(eval: &mut Evaluator, f: Value) -> Value {
    let conveyed = eval.bindings.clone();
    let display = crate::ext::DisplayName::shared("clojure.core/bound-fn");
    let func =
        crate::ext::NativeTarget::Dyn(Arc::new(move |ev: &mut Evaluator, args: &[Value]| {
            let depth = ev.bindings_depth();
            let frame: Vec<(Var, Value)> = conveyed
                .pairs()
                .map(|(v, x)| (v.clone(), x.clone()))
                .collect();
            ev.push_frame(frame);
            let out = ev.call(&f, args);
            ev.pop_bindings(depth);
            out.map_err(signal_to_native)
        }));
    Value::Extension(Arc::new(crate::ext::NativeFn {
        id: eval.next_id(),
        display,
        func,
        owned: None,
        meta: None,
        fast: crate::ext::FastArith::None,
        compiled: None,
    }))
}

/// `defmacro` over a compiled macro fn.
#[allow(clippy::too_many_arguments)]
pub fn defmacro(
    eval: &mut Evaluator,
    sym: &Symbol,
    doc: Option<&str>,
    attr: Option<&Map>,
    arglists: &[Value],
    line: u32,
    col: u32,
    f: Value,
) -> Step {
    eval.defmacro_with(sym, doc, attr, arglists, Span::new(line, col), f)
}

/// A failed `assert` (`message` already evaluated).
pub fn assert_failed(eval: &mut Evaluator, message: Option<Value>, form: &str) -> Signal {
    eval.assert_failed(message, form)
}

/// Fused `(assoc! t1 k (conj (nth t2 k) x))` in place when `t1` and
/// `t2` alias the same live transient vector and `k` is in bounds.
pub fn assoc_conj(t1: &Value, t2: &Value, k: &Value, x: &mut Value, take_x: bool) -> bool {
    let (Value::Transient(a), Value::Transient(b), Value::Int(n)) = (t1, t2, k) else {
        return false;
    };
    if !a.same_cell(b) {
        return false;
    }
    let Ok(i) = usize::try_from(*n) else {
        return false;
    };
    if take_x {
        crate::vm_ops::assoc_conj_in_cell(a, i, x)
    } else {
        let mut xv = x.clone();
        crate::vm_ops::assoc_conj_in_cell(a, i, &mut xv)
    }
}

/// Effect-only `(conj! coll x)` in place on a live transient vector.
pub fn conj_bang_fx(a: &Value, b: &Value) -> bool {
    crate::vm_ops::ConjBang::apply_fx(a, b)
}

type OwnFn = fn(&mut [Value]) -> Option<Value>;

const OWN_BIT: u32 = 1 << 31;

const OWN_OPS: &[(&str, usize, OwnFn)] = &[
    ("pop", 1, <crate::vm_ops::Pop as crate::vm_ops::Upd>::apply),
    (
        "conj",
        2,
        <crate::vm_ops::Conj as crate::vm_ops::Upd>::apply,
    ),
    (
        "assoc",
        3,
        <crate::vm_ops::Assoc as crate::vm_ops::Upd>::apply,
    ),
    (
        "next",
        1,
        <crate::vm_ops::Next as crate::vm_ops::Upd>::apply,
    ),
    (
        "rest",
        1,
        <crate::vm_ops::Rest as crate::vm_ops::Upd>::apply,
    ),
    ("update", 3, crate::vm_ops::apply_update3),
    ("assoc-in2", 4, crate::vm_ops::apply_assoc_in2),
    ("update-in2", 4, crate::vm_ops::apply_update_in2),
];

/// Opcode for [`apply_own`], matching core fns whose in-place `Upd`
/// fast path consumes owned operands.
#[must_use]
pub fn own_op(name: &str, arity: usize) -> Option<u32> {
    OWN_OPS
        .iter()
        .position(|(n, a, _)| *n == name && *a == arity)
        .map(|i| OWN_BIT | (i as u32))
}

/// Apply an owned-operand fast op (`None`: fall back; `args` remain
/// valid on `None`).
pub fn apply_own(id: u32, args: &mut [Value]) -> Option<Value> {
    if id == u32::MAX || (id & OWN_BIT) == 0 {
        return None;
    }
    OWN_OPS
        .get((id & !OWN_BIT) as usize)
        .and_then(|(_, _, f)| f(args))
}

/// Repack unboxed 2-key path arguments (`[m, k1, k2, vf]`) back into the
/// 3-argument `[m, [k1 k2], vf]` form when [`apply_own`] falls back to the
/// general `assoc-in` / `update-in` native.
pub fn repack_own_args(id: u32, args: &mut smallvec::SmallVec<[Value; 4]>) {
    if (id & OWN_BIT) != 0
        && matches!(
            OWN_OPS.get((id & !OWN_BIT) as usize),
            Some(&("assoc-in2" | "update-in2", 4, _))
        )
        && args.len() == 4
    {
        let k2 = args.remove(2);
        let k1 = std::mem::replace(&mut args[1], Value::Nil);
        args[1] = Value::Vector(Vector::from_vec(vec![k1, k2]));
    }
}

/// A compiled loop's direct iteration plan (#271 Slice 4c/5k): `(1, count,
/// 0, 0)` for a persistent vector, subvector, or split vector (items by
/// index, [`vec_item`]), `(2, count, start, step)` for a long range (items
/// `start`, `start + step`, …). `None`: the loop takes the general call,
/// which walks the seq exactly as `run` does (lazy forcing order included).
#[must_use]
pub fn iter_plan(coll: &Value) -> Option<(u8, i64, i64, i64)> {
    match coll {
        Value::Vector(v) => Some((1, i64::try_from(v.len()).ok()?, 0, 0)),
        Value::Extension(e) => {
            let any = e.as_any();
            if let Some(sv) = any.downcast_ref::<clj_value::SubVectorValue>() {
                return Some((1, i64::try_from(sv.len()).ok()?, 0, 0));
            }
            if let Some(sp) = any.downcast_ref::<clj_value::SplitVecValue>() {
                return Some((1, i64::try_from(sp.len()).ok()?, 0, 0));
            }
            let r = any.downcast_ref::<clj_value::RangeValue>()?;
            match (r.is_long(), r.count(), r.cur(), r.step()) {
                (true, Some(n), RangeNum::Int(start), RangeNum::Int(step)) => {
                    Some((2, n, start, step))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// A compiled loop's direct plan for `(range …)` with all-`Int` args
/// (#271 Slice 5l), mirroring `lazy::long_range`'s classification without
/// building the range value: `(2, count, start, step)` for a finite long
/// range — the empty shapes answer `(2, 0, start, step)`, which walks zero
/// items exactly as the general call walks the empty list — and `None` for
/// the infinite repeat and count-overflow shapes the direct loop cannot
/// walk (the general call builds those ranges itself).
#[must_use]
pub fn range_plan(argc: usize, a: [i64; 3]) -> Option<(u8, i64, i64, i64)> {
    let (s, e, st) = match argc {
        1 => (0, a[0], 1),
        2 => (a[0], a[1], 1),
        _ => (a[0], a[1], a[2]),
    };
    if (st > 0 && e <= s) || (st < 0 && e >= s) || (st == 0 && e == s) {
        return Some((2, 0, s, st));
    }
    if st == 0 {
        return None;
    }
    let count = crate::lazy::range_count(s, e, st).filter(|&c| c <= i64::from(i32::MAX))?;
    Some((2, count, s, st))
}

/// Item `i` of an indexed vector/subvector/split-vector (an index below
/// [`iter_plan`]'s count; anything else is nil).
#[must_use]
pub fn vec_item(v: &Value, i: usize) -> Value {
    match v {
        Value::Vector(v) => v.get(i).unwrap_or(Value::Nil),
        Value::Extension(e) => {
            let any = e.as_any();
            if let Some(sv) = any.downcast_ref::<clj_value::SubVectorValue>() {
                return sv.nth(i).unwrap_or(Value::Nil);
            }
            if let Some(sp) = any.downcast_ref::<clj_value::SplitVecValue>() {
                return sp.nth(i).unwrap_or(Value::Nil);
            }
            Value::Nil
        }
        _ => Value::Nil,
    }
}

#[derive(Debug)]
enum VecBuf {
    Ints(Vec<i64>),
    Doubles(Vec<f64>),
    Generic(Vec<Value>),
}

impl Default for VecBuf {
    fn default() -> Self {
        Self::Ints(Vec::new())
    }
}

/// A compiled `mapv` / `vec` / `into []` loop's accumulator (#271): items
/// gather in a plain buffer, packed while every item is a long or every
/// item is a double, and become one persistent vector when the loop ends.
/// Only the loop ever holds it, so pushes take no lock and no copy-on-write check.
#[derive(Debug, Default)]
pub struct VecBuilder {
    buf: VecBuf,
}

impl VecBuilder {
    fn push(&mut self, v: Value) {
        let next = match (&mut self.buf, v) {
            (VecBuf::Ints(xs), Value::Int(n)) => {
                xs.push(n);
                None
            }
            (VecBuf::Ints(xs), Value::Double(d)) if xs.is_empty() => Some(VecBuf::Doubles(vec![d])),
            (VecBuf::Ints(xs), other) => {
                let mut g: Vec<Value> = xs.drain(..).map(Value::Int).collect();
                g.push(other);
                Some(VecBuf::Generic(g))
            }
            (VecBuf::Doubles(xs), Value::Double(d)) => {
                xs.push(d);
                None
            }
            (VecBuf::Doubles(xs), other) => {
                let mut g: Vec<Value> = xs.drain(..).map(Value::Double).collect();
                g.push(other);
                Some(VecBuf::Generic(g))
            }
            (VecBuf::Generic(xs), other) => {
                xs.push(other);
                None
            }
        };
        if let Some(buf) = next {
            self.buf = buf;
        }
    }

    fn freeze(&mut self) -> Vector {
        match std::mem::take(&mut self.buf) {
            VecBuf::Ints(xs) => Vector::from_flat_ints(xs),
            VecBuf::Doubles(xs) => Vector::from_flat_doubles(xs),
            VecBuf::Generic(xs) => Vector::from_vec(xs),
        }
    }
}

impl clj_value::ExtensionValue for VecBuilder {
    fn class_name(&self) -> String {
        "clojure.lang.PersistentVector$TransientVector".to_owned()
    }

    fn print(&self) -> String {
        "#object[clojure.lang.PersistentVector$TransientVector]".to_owned()
    }

    fn dyn_eq(&self, other: &dyn clj_value::ExtensionValue) -> bool {
        std::ptr::addr_eq(self, other)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_usize(std::ptr::from_ref(self).addr());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// A fresh [`VecBuilder`] accumulator.
#[must_use]
pub fn vec_builder() -> Value {
    Value::Extension(Arc::new(VecBuilder::default()))
}

fn builder(acc: &mut Value) -> Option<&mut VecBuilder> {
    let Value::Extension(e) = acc else {
        return None;
    };
    Arc::get_mut(e)?.as_any_mut()?.downcast_mut::<VecBuilder>()
}

/// Append `v` to a loop's [`VecBuilder`] accumulator.
pub fn vec_push(acc: &mut Value, v: Value) {
    if let Some(b) = builder(acc) {
        b.push(v);
    }
}

/// The loop's result: its [`VecBuilder`] frozen into a persistent vector.
pub fn vec_done(acc: &mut Value) -> Value {
    builder(acc).map_or(Value::Nil, |b| Value::Vector(b.freeze()))
}

/// Unwrap a `reduced` accumulator in place (`true`: it was one, so the
/// compiled `reduce` loop stops).
pub fn unreduce(v: &mut Value) -> bool {
    let Some(inner) = crate::core::as_reduced(v).cloned() else {
        return false;
    };
    *v = inner;
    true
}

enum StrColl<'a> {
    Vec(&'a Vector, usize, usize),
    Split(&'a clj_value::SplitVecValue),
}

impl<'a> StrColl<'a> {
    fn from_value(coll: &'a Value) -> Option<Self> {
        if let Value::Vector(v) = coll {
            return Some(Self::Vec(v, 0, v.len()));
        }
        if let Some(sp) = crate::lazy::as_splitvec(coll) {
            return Some(Self::Split(sp));
        }
        crate::lazy::as_subvector(coll).map(|sv| Self::Vec(sv.vec(), sv.start(), sv.end()))
    }

    fn window(&self) -> (usize, usize) {
        match *self {
            Self::Vec(_, start, end) => (start, end),
            Self::Split(sp) => (0, sp.len()),
        }
    }

    #[inline]
    fn get(&self, i: usize) -> Option<&'a str> {
        match *self {
            Self::Vec(v, _, _) => vec_str_at(v, i),
            Self::Split(sp) => sp.piece_str(i),
        }
    }
}

fn vec_str_at(v: &Vector, i: usize) -> Option<&str> {
    match v.get_val_ref(i)? {
        clj_value::ValueRef::Str(s) => Some(s),
        clj_value::ValueRef::Ref(Value::Str(s)) => Some(s),
        _ => None,
    }
}

#[inline]
fn parse_ascii_long(s: &str, allow_plus: bool, max_18: bool) -> Option<i64> {
    let bytes = s.as_bytes();
    let (neg, rest) = match bytes.first()? {
        b'-' => (true, &bytes[1..]),
        b'+' if allow_plus => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    if rest.is_empty() {
        return None;
    }
    if rest.len() <= 18 {
        let mut acc: u64 = 0;
        for &b in rest {
            let d = b.wrapping_sub(b'0');
            if d > 9 {
                return None;
            }
            acc = acc * 10 + u64::from(d);
        }
        return Some(if neg { -(acc as i64) } else { acc as i64 });
    }
    if max_18 || !s.is_ascii() {
        return None;
    }
    parse_long_str(s)
}

fn try_parse_token(coll: &Value, mode: u8) -> Option<Value> {
    if mode == 5 {
        let Value::Str(s) = coll else { return None };
        return parse_ascii_long(s, true, true).map(Value::Int);
    }
    let allow_plus = mode == 4;
    let v = match coll {
        Value::Str(s) => parse_ascii_long(s, allow_plus, false).map_or(Value::Nil, Value::Int),
        _ => Value::Nil,
    };
    Some(v)
}

#[inline]
fn parse_int_item(s: &str, mode: u8) -> Option<i64> {
    if mode == 3 || mode == 6 {
        return parse_ascii_long(s, mode == 3, false);
    }
    let n = parse_long_str(s)?;
    if mode == 1 && i32::try_from(n).is_err() {
        return None;
    }
    Some(n)
}

/// Parse a string collection (`Vector`, `SplitVecValue`, `SubVectorValue`)
/// in one pass without materializing token strings (`mode`: 0 `Long/parseLong`,
/// 1 `Integer/parseInt`, 2 `Double/parseDouble`, 3 ASCII `[+-]?[0-9]+`,
/// 6 ASCII `-?[0-9]+`), or parse a single string token (`mode`: 4
/// `(when (and (string? s) (re-matches #"[+-]?[0-9]+" s)) (parse-long s))`,
/// 5 1..18-digit ASCII `(if (and (string? s) (re-matches #"[+-]?[0-9]{1,18}" s))
/// (parse-long s) els)`, 7 `(when (and (string? s) (re-matches #"-?[0-9]+" s))
/// (parse-long s))`).
#[must_use]
pub fn try_mapv_parse_num(coll: &Value, mode: u8) -> Option<Value> {
    if matches!(mode, 4 | 5 | 7) {
        return try_parse_token(coll, mode);
    }
    let src = StrColl::from_value(coll)?;
    let (start, end) = src.window();
    if mode == 2 {
        let mut out = Vec::with_capacity(end - start);
        for i in start..end {
            out.push(parse_double_str(src.get(i)?)?);
        }
        return Some(Value::Vector(Vector::from_flat_doubles(out)));
    }
    let mut ints = Vec::with_capacity(end - start);
    for i in start..end {
        ints.push(parse_int_item(src.get(i)?, mode)?);
    }
    Some(Value::Vector(Vector::from_flat_ints(ints)))
}

fn num_val_as_i64(vr: clj_value::ValueRef<'_>) -> Option<i64> {
    match vr {
        clj_value::ValueRef::Int(n) => Some(n),
        clj_value::ValueRef::Ref(Value::Int(n)) => Some(*n),
        _ => None,
    }
}

fn dot_ints(mut acc: i64, v1: &Vector, v2: &Vector) -> Option<Value> {
    if let (Some(s1), Some(s2)) = (v1.as_flat_ints(), v2.as_flat_ints()) {
        for (&x, &y) in s1.iter().zip(s2.iter()) {
            acc = acc.checked_add(x.checked_mul(y)?)?;
        }
        return Some(Value::Int(acc));
    }
    let n = v1.len().min(v2.len());
    for i in 0..n {
        let x = num_val_as_i64(v1.get_val_ref(i)?)?;
        let y = num_val_as_i64(v2.get_val_ref(i)?)?;
        acc = acc.checked_add(x.checked_mul(y)?)?;
    }
    Some(Value::Int(acc))
}

fn num_val_as_f64(vr: clj_value::ValueRef<'_>) -> Option<f64> {
    match vr {
        clj_value::ValueRef::Double(d) => Some(d),
        clj_value::ValueRef::Ref(Value::Double(d)) => Some(*d),
        _ => None,
    }
}

fn dot_doubles_flat(mut acc: f64, v1: &Vector, v2: &Vector) -> Option<Value> {
    if let (Some(s1), Some(s2)) = (v1.as_flat_doubles(), v2.as_flat_doubles()) {
        for (&x, &y) in s1.iter().zip(s2.iter()) {
            acc += x * y;
        }
        return Some(Value::Double(acc));
    }
    if let (Some(s1), Some(s2)) = (v1.as_flat_doubles(), v2.as_flat_ints()) {
        for (&x, &y) in s1.iter().zip(s2.iter()) {
            acc += x * (y as f64);
        }
        return Some(Value::Double(acc));
    }
    if let (Some(s1), Some(s2)) = (v1.as_flat_ints(), v2.as_flat_doubles()) {
        for (&x, &y) in s1.iter().zip(s2.iter()) {
            acc += (x as f64) * y;
        }
        return Some(Value::Double(acc));
    }
    None
}

fn dot_doubles(mut acc: f64, v1: &Vector, v2: &Vector) -> Option<Value> {
    if let Some(res) = dot_doubles_flat(acc, v1, v2) {
        return Some(res);
    }
    let n = v1.len().min(v2.len());
    for i in 0..n {
        let r1 = v1.get_val_ref(i)?;
        let r2 = v2.get_val_ref(i)?;
        let prod = match (num_val_as_f64(r1), num_val_as_f64(r2)) {
            (Some(x), Some(y)) => x * y,
            (Some(x), None) => x * num_val_as_i64(r2)? as f64,
            (None, Some(y)) => num_val_as_i64(r1)? as f64 * y,
            (None, None) => num_val_as_i64(r1)?.checked_mul(num_val_as_i64(r2)?)? as f64,
        };
        acc += prod;
    }
    Some(Value::Double(acc))
}

/// Fast path for `(reduce + init (map * c1 c2))` over two vectors when
/// `init` and all elements are `Int` or `Double`.
#[must_use]
pub fn try_dot_product(init: &Value, c1: &Value, c2: &Value) -> Option<Value> {
    let (Value::Vector(v1), Value::Vector(v2)) = (c1, c2) else {
        return None;
    };
    match init {
        Value::Int(acc) => dot_ints(*acc, v1, v2),
        Value::Double(acc) => dot_doubles(*acc, v1, v2),
        _ => None,
    }
}

/// Streaming fast path for `(println|print (clojure.string/join sep coll))`.
pub fn try_print_join(
    eval: &mut Evaluator,
    sep: &Value,
    coll: &Value,
    flags: u8,
    hit: &mut u8,
) -> Result<(), Signal> {
    let h = crate::io::try_print_join(eval, sep, coll, flags).map_err(|e| eval.throw(e))?;
    *hit = u8::from(h);
    Ok(())
}

/// Fold `res` into `acc` using `clojure.core/+` for a compiled
/// `(reduce + [init] (map F ...))` loop.
pub fn add_acc(eval: &mut Evaluator, acc: &mut Value, res: Value) -> Result<(), Signal> {
    match (&mut *acc, &res) {
        (Value::Double(a), Value::Double(b)) => {
            *a += *b;
            return Ok(());
        }
        (Value::Double(a), Value::Int(b)) => {
            *a += *b as f64;
            return Ok(());
        }
        (Value::Int(a), Value::Double(b)) => {
            *acc = Value::Double(*a as f64 + *b);
            return Ok(());
        }
        (Value::Int(a), Value::Int(b)) => {
            if let Some(sum) = a.checked_add(*b) {
                *a = sum;
                return Ok(());
            }
        }
        _ => {}
    }
    let lhs = std::mem::replace(acc, Value::Nil);
    *acc = crate::core::native_add(eval, &[lhs, res]).map_err(|e| eval.throw(e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_set_and_keyword_calls_look_up_directly_271() {
        let mut eval = crate::boot();
        let kw = |n: &str| Value::Keyword(Keyword::new(None, n));
        let m = Value::Map(Map::from_kvs_last_wins(vec![(kw("a"), Value::Int(1))]));
        let s = Value::Set(clj_value::new_set().insert(Value::Int(2)));
        let sym = Value::Symbol(Symbol::new(None, "a"));
        let sm = Value::Map(Map::from_kvs_last_wins(vec![(sym.clone(), Value::Int(3))]));
        let mut call = |f: &Value, args: &[Value]| invoke(&mut eval, f, args).ok();
        assert_eq!(call(&m, &[kw("a")]), Some(Value::Int(1)));
        assert_eq!(call(&m, &[kw("b"), Value::Int(9)]), Some(Value::Int(9)));
        assert_eq!(call(&s, &[Value::Int(2)]), Some(Value::Int(2)));
        assert_eq!(call(&s, &[Value::Int(5)]), Some(Value::Nil));
        assert_eq!(
            call(&kw("a"), std::slice::from_ref(&m)),
            Some(Value::Int(1))
        );
        assert_eq!(call(&sym, &[sm, Value::Nil]), Some(Value::Int(3)));
        // Other arities keep the general call (and its arity error).
        assert_eq!(call(&m, &[]), None);
        assert_eq!(call(&kw("a"), &[m.clone(), Value::Nil, Value::Nil]), None);
        // The general call answers the same (natives still reach it).
        assert_eq!(
            eval.call(&s, &[Value::Int(5), Value::Int(0)]).ok(),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn kw_map_shares_its_template_keys() {
        let kw = |n: &str| Value::Keyword(Keyword::new(None, n));
        let template = Value::Map(Map::from_kvs_last_wins(vec![(kw("a"), Value::Nil)]));
        let built = kw_map(&template, &[Value::Int(1)]);
        assert_eq!(
            built,
            Value::Map(Map::from_kvs_last_wins(vec![(kw("a"), Value::Int(1))]))
        );
        assert_eq!(kw_map(&Value::Nil, &[Value::Int(1)]), Value::Nil);
    }

    #[test]
    fn meta_symbols_match_their_read_back() {
        let kw = |n: &str| Value::Keyword(Keyword::new(None, n));
        for (ns, name, kvs, text) in [
            (
                Some("user"),
                "f",
                vec![kw("private"), Value::Bool(true)],
                "^{:private true} user/f",
            ),
            (
                None,
                "g",
                vec![
                    kw("tag"),
                    Value::Symbol(Symbol::new(None, "String")),
                    kw("dynamic"),
                    Value::Bool(true),
                    kw("doc"),
                    Value::Str(std::sync::Arc::from("d")),
                    kw("n"),
                    Value::Int(3),
                ],
                "^{:tag String :dynamic true :doc \"d\" :n 3} g",
            ),
        ] {
            let built = meta_symbol(ns, name, &kvs);
            let read = read_constant(text).expect("reads");
            assert_eq!(print_constant(&built), print_constant(&read));
            assert_eq!(built.meta(), read.meta());
        }
    }

    #[test]
    fn owned_vector_literals_match_the_cloning_ones() {
        let s = Value::Str(Arc::from("s"));
        let meta = Map::from_kvs_last_wins(vec![(
            Value::Keyword(Keyword::new(None, "tag")),
            Value::Int(1),
        )]);
        for items in [
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Double(1.5), Value::Double(2.5)],
            vec![Value::Int(1), s.clone()],
            vec![Value::Double(1.5), s.clone()],
            vec![s.clone(), Value::Int(2)],
            vec![Value::Char('a'), Value::Char('b')],
            Vec::new(),
        ] {
            let mut slots = items.clone();
            let owned = vec_lit_owned(&mut slots, None);
            assert_eq!(owned, vec_lit(items.clone(), None), "{items:?}");
            assert!(
                slots
                    .iter()
                    .all(|v| matches!(v, Value::Nil | Value::Int(_) | Value::Double(_)))
            );
            let mut meta_slots = items.clone();
            let owned_meta = vec_lit_owned(&mut meta_slots, Some(meta.clone()));
            let cloned_meta = vec_lit(items.clone(), Some(meta.clone()));
            assert_eq!(owned_meta, cloned_meta);
            assert_eq!(owned_meta.meta(), Some(&meta));
            assert_eq!(cloned_meta.meta(), Some(&meta));
        }
    }

    #[test]
    fn loop_plans_vectors_and_long_ranges() {
        use clj_value::RangeValue;
        let v = Value::Vector(Vector::from_vec(vec![Value::Int(7), Value::Int(8)]));
        assert_eq!(iter_plan(&v), Some((1, 2, 0, 0)));
        assert_eq!(vec_item(&v, 1), Value::Int(8));
        assert_eq!(vec_item(&Value::Nil, 0), Value::Nil);
        let range = |cur, end, step, long, count| {
            Value::Extension(Arc::new(RangeValue::new(cur, end, step, long, count)))
        };
        let long = range(
            RangeNum::Int(3),
            Some(RangeNum::Int(9)),
            RangeNum::Int(2),
            true,
            Some(3),
        );
        assert_eq!(iter_plan(&long), Some((2, 3, 3, 2)));
        let dbl = range(
            RangeNum::Double(0.0),
            Some(RangeNum::Double(1.0)),
            RangeNum::Double(0.5),
            false,
            None,
        );
        assert_eq!(iter_plan(&dbl), None);
        // Bounds-only range plans (#271 Slice 5l) agree with the plans
        // of the range values `lazy::long_range` builds for the same
        // all-long args, and refuse the shapes it answers with a repeat
        // or a double-path range.
        assert_eq!(range_plan(1, [5, 0, 0]), Some((2, 5, 0, 1)));
        assert_eq!(range_plan(2, [3, 9, 0]), Some((2, 6, 3, 1)));
        assert_eq!(range_plan(3, [3, 9, 2]), Some((2, 3, 3, 2)));
        assert_eq!(range_plan(3, [9, 3, -2]), Some((2, 3, 9, -2)));
        for (s, e, st) in [(0i64, 5, 1), (3, 9, 2), (9, 3, -2), (-4, 4, 3)] {
            let want = iter_plan(&crate::lazy::long_range(s, e, st)).filter(|&(k, _, _, _)| k == 2);
            assert_eq!(range_plan(3, [s, e, st]), want, "({s}, {e}, {st})");
        }
        // Empty shapes walk zero items, like the empty-list value path.
        assert_eq!(range_plan(1, [0, 0, 0]), Some((2, 0, 0, 1)));
        assert_eq!(range_plan(2, [5, 0, 0]), Some((2, 0, 5, 1)));
        assert_eq!(range_plan(3, [0, 5, -1]), Some((2, 0, 0, -1)));
        assert_eq!(range_plan(3, [4, 4, 0]), Some((2, 0, 4, 0)));
        // A zero step repeats forever; a count that overflows `i32` (or
        // the count arithmetic) takes the double-path range: both stay
        // general.
        assert_eq!(range_plan(3, [4, 5, 0]), None);
        assert_eq!(range_plan(2, [0, i64::MAX, 0]), None);
        assert_eq!(range_plan(2, [i64::MIN, i64::MAX, 0]), None);
        let mut reduced = crate::core::Reduced::wrap(Value::Int(5));
        assert_eq!(iter_plan(&reduced), None);
        assert_eq!(iter_plan(&Value::Nil), None);
        assert!(unreduce(&mut reduced));
        assert_eq!(reduced, Value::Int(5));
        assert!(!unreduce(&mut reduced));
        let ints = |xs: &[i64]| {
            Value::Vector(Vector::from_vec(
                xs.iter().map(|n| Value::Int(*n)).collect(),
            ))
        };
        let mut acc = vec_builder();
        vec_push(&mut acc, Value::Int(1));
        vec_push(&mut acc, Value::Int(2));
        assert_eq!(vec_done(&mut acc), ints(&[1, 2]));
        let mut mixed = vec_builder();
        vec_push(&mut mixed, Value::Int(1));
        vec_push(&mut mixed, Value::Nil);
        vec_push(&mut mixed, Value::Int(3));
        let want = Value::Vector(Vector::from_vec(vec![
            Value::Int(1),
            Value::Nil,
            Value::Int(3),
        ]));
        assert_eq!(vec_done(&mut mixed), want);
        let mut not_builder = Value::Int(3);
        vec_push(&mut not_builder, Value::Int(1));
        assert_eq!(vec_done(&mut not_builder), Value::Nil);
        let mut shared = vec_builder();
        let _other = shared.clone();
        vec_push(&mut shared, Value::Int(1));
        assert_eq!(vec_done(&mut shared), Value::Nil);
        use clj_value::ExtensionValue;
        let mut b = VecBuilder::default();
        assert!(b.class_name().ends_with("TransientVector"));
        assert!(b.print().starts_with("#object["));
        assert!(b.dyn_eq(&b));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.dyn_hash(&mut h);
        assert!(b.as_any().is::<VecBuilder>());
        assert!(b.as_any_mut().is_some());
    }

    #[test]
    fn const_colls_match_the_reader() {
        let kw = |n: &str| Value::Keyword(Keyword::new(None, n));
        let sym = |n: &str| Value::Symbol(Symbol::new(None, n));
        let vec_of = |xs: Vec<Value>| const_coll(1, &xs);
        for (kind, items, text) in [
            (0, vec![], "()"),
            (
                0,
                vec![
                    vec_of(vec![sym("x")]),
                    vec_of(vec![sym("x"), sym("&"), sym("y")]),
                ],
                "([x] [x & y])",
            ),
            (
                1,
                vec![Value::Str(std::sync::Arc::from("java.lang.Exception"))],
                "[\"java.lang.Exception\"]",
            ),
            (
                2,
                vec![
                    kw("added"),
                    Value::Str(std::sync::Arc::from("1.0")),
                    kw("n"),
                    Value::Int(2),
                ],
                "{:added \"1.0\", :n 2}",
            ),
            (3, vec![kw("a"), Value::Int(1)], "#{:a 1}"),
        ] {
            let built = const_coll(kind, &items);
            let read = read_constant(text).expect("reads");
            assert_eq!(built, read);
            // Read lists carry their read position; built ones carry none.
            let shown = |v: &Value| print_constant(&v.clone().with_meta(None));
            assert_eq!(shown(&built), shown(&read));
            assert!(built.meta().is_none());
        }
    }

    #[test]
    fn only_throws_carry_a_throwable() {
        assert!(thrown_of(&Signal::Recur(Vec::new())).is_none());
    }

    #[test]
    fn assoc_conj_and_own_ops_handle_hits_and_misses() {
        let tv = Value::Transient(clj_value::Transient::new(clj_value::TransientKind::Vector(
            clj_value::TransientVector::from_vector_owned(Vector::from_vec(vec![
                Value::Vector(Vector::from_vec(vec![Value::Int(1)])),
                Value::Int(42),
            ])),
        )));
        let tv2 = Value::Transient(clj_value::Transient::new(clj_value::TransientKind::Vector(
            clj_value::TransientVector::new(),
        )));
        let mut x = Value::Int(9);
        assert!(assoc_conj(&tv, &tv, &Value::Int(0), &mut x, true));
        assert!(matches!(x, Value::Nil));
        let mut x2 = Value::Int(8);
        assert!(assoc_conj(&tv, &tv, &Value::Int(0), &mut x2, false));
        assert_eq!(x2, Value::Int(8));
        // Non-conjable element misses and restores `x`.
        let mut x3 = Value::Int(7);
        assert!(!assoc_conj(&tv, &tv, &Value::Int(1), &mut x3, true));
        assert_eq!(x3, Value::Int(7));
        // Mismatched transients / bad key / non-transient miss cleanly.
        assert!(!assoc_conj(&tv, &tv2, &Value::Int(0), &mut x3, false));
        assert!(!assoc_conj(&tv, &tv, &Value::Int(-1), &mut x3, false));
        assert!(!assoc_conj(
            &Value::Nil,
            &tv,
            &Value::Int(0),
            &mut x3,
            false
        ));
        assert!(conj_bang_fx(&tv, &Value::Int(99)));
        assert!(!conj_bang_fx(&Value::Nil, &Value::Int(99)));

        let pop_id = own_op("pop", 1).expect("pop op");
        let mut pop_args = [Value::Vector(Vector::from_vec(vec![Value::Int(1)]))];
        assert!(apply_own(pop_id, &mut pop_args).is_some());
        let mut empty_pop = [Value::Vector(Vector::new_sync())];
        assert!(apply_own(pop_id, &mut empty_pop).is_none());
        assert!(own_op("unknown", 2).is_none());
        let add_id = inline_op("+", 2).expect("+");
        assert_eq!(
            apply_inline(add_id, &[&Value::Int(2), &Value::Int(3)]),
            Some(Value::Int(5))
        );
        let assoc_id = own_op("assoc", 3).expect("assoc op");
        assert_eq!(
            apply_inline(assoc_id, &[&Value::Int(2), &Value::Int(3), &Value::Int(4)]),
            None
        );
        assert_eq!(
            apply3(assoc_id, &Value::Int(2), &Value::Int(3), &Value::Int(4)),
            None
        );
        let mul3_id = inline_op("*", 3).expect("*/3");
        let mut mul_args = [
            Value::Vector(Vector::from_vec(vec![Value::Int(1)])),
            Value::Int(0),
            Value::Int(9),
        ];
        assert_eq!(apply_own(mul3_id, &mut mul_args), None);
        let mut assoc_miss = [Value::Int(2), Value::Int(3), Value::Int(4)];
        assert_eq!(apply_own(assoc_id, &mut assoc_miss), None);

        let re_id = inline_op("re-matches", 2).expect("re-matches");
        assert!(is_re_op(re_id));
        assert!(!is_re_op(add_id));
        let re_pattern = crate::re::RE_NATIVES
            .iter()
            .find(|&&(k, _)| k == "re-pattern")
            .expect("re-pattern")
            .1;
        let pat = re_pattern(
            &mut Evaluator::new_bare("user"),
            &[Value::Str(Arc::from("a+"))],
        )
        .expect("pat");
        let aa = Value::Str(Arc::from("aa"));
        assert_eq!(apply2_no_re(re_id, &pat, &aa), None);
        assert_eq!(apply2(re_id, &pat, &aa), Some(aa.clone()));
        assert_eq!(
            apply2(add_id, &Value::Int(2), &Value::Int(3)),
            Some(Value::Int(5))
        );
        assert_eq!(apply_re_matches(&pat, &aa), Some(aa));
    }

    #[test]
    fn try_mapv_parse_num_covers_vectors_splits_subvecs_and_misses() {
        let strs = |xs: &[&str]| {
            Value::Vector(Vector::from_vec(
                xs.iter().map(|s| Value::Str(Arc::from(*s))).collect(),
            ))
        };
        let ints = |xs: &[i64]| Value::Vector(Vector::from_flat_ints(xs.to_vec()));
        let dbls = |xs: &[f64]| {
            Value::Vector(Vector::from_vec(
                xs.iter().map(|d| Value::Double(*d)).collect(),
            ))
        };
        // Generic Vector of strings (modes 0, 1, 2)
        let gv = strs(&["-7", "42", "0"]);
        assert_eq!(try_mapv_parse_num(&gv, 0), Some(ints(&[-7, 42, 0])));
        assert_eq!(try_mapv_parse_num(&gv, 1), Some(ints(&[-7, 42, 0])));
        assert_eq!(
            try_mapv_parse_num(&strs(&["1.5", "-2.25"]), 2),
            Some(dbls(&[1.5, -2.25]))
        );
        // PackedStrings vector (>= 256 items through TransientVector)
        let mut tv = clj_value::TransientVector::new();
        for i in 0..260 {
            tv.conj(Value::Str(Arc::from(i.to_string()))).expect("conj");
        }
        let pv = Value::Vector(tv.persistent().expect("vec"));
        assert!(matches!(&pv, Value::Vector(v) if v.as_packed_strs().is_some()));
        let parsed = try_mapv_parse_num(&pv, 0).expect("packed long");
        assert_eq!(parsed, ints(&(0..260).collect::<Vec<_>>()));
        // SplitVecValue
        let sp = Value::Extension(Arc::new(clj_value::SplitVecValue::new(
            Arc::from("10 20 30"),
            Arc::new(vec![(0, 2), (3, 2), (6, 2)]),
        )));
        assert_eq!(try_mapv_parse_num(&sp, 0), Some(ints(&[10, 20, 30])));
        // SubVectorValue
        let base_vec = Vector::from_vec(vec![
            Value::Str(Arc::from("x")),
            Value::Str(Arc::from("3")),
            Value::Str(Arc::from("4")),
        ]);
        let sv = Value::Extension(Arc::new(clj_value::SubVectorValue::new(base_vec, 1, 3)));
        assert_eq!(try_mapv_parse_num(&sv, 1), Some(ints(&[3, 4])));
        // Misses: non-collection, non-string item, malformed token, i32 overflow on mode 1
        assert_eq!(try_mapv_parse_num(&Value::Nil, 0), None);
        assert_eq!(try_mapv_parse_num(&ints(&[1, 2]), 0), None);
        assert_eq!(try_mapv_parse_num(&strs(&["1", "nope"]), 0), None);
        assert_eq!(try_mapv_parse_num(&strs(&["1.0", "bad"]), 2), None);
        assert_eq!(try_mapv_parse_num(&strs(&["3000000000"]), 1), None);
        assert_eq!(
            try_mapv_parse_num(&strs(&["3000000000"]), 0),
            Some(ints(&[3_000_000_000]))
        );
    }

    #[test]
    fn try_dot_product_and_add_acc_cover_fast_and_fallback_paths() {
        let flat = |xs: &[i64]| Value::Vector(Vector::from_flat_ints(xs.to_vec()));
        let generic_ints = Value::Vector(Vector::from_vec(vec![
            Value::Int(2),
            Value::Int(5),
            Value::Nil,
        ]));
        let dbls = |xs: &[f64]| {
            Value::Vector(Vector::from_vec(
                xs.iter().map(|d| Value::Double(*d)).collect(),
            ))
        };
        let mixed = Value::Vector(Vector::from_vec(vec![Value::Double(1.5), Value::Int(4)]));
        let non_num = Value::Vector(Vector::from_vec(vec![Value::Nil]));
        // dot_ints: flat + generic + overflow miss + non-int miss
        assert_eq!(
            try_dot_product(&Value::Int(10), &flat(&[2, 3]), &flat(&[4, 5])),
            Some(Value::Int(33))
        );
        assert_eq!(
            try_dot_product(&Value::Int(1), &generic_ints, &flat(&[3, 4])),
            Some(Value::Int(27))
        );
        assert_eq!(
            try_dot_product(&Value::Int(0), &flat(&[i64::MAX]), &flat(&[2])),
            None
        );
        assert_eq!(
            try_dot_product(&Value::Int(0), &generic_ints, &non_num),
            None
        );
        // dot_doubles: flat (Double, Double), (Double, Int), (Int, Double),
        // generic/mixed fallback across all 4 combos, overflow/non-num miss
        assert_eq!(
            try_dot_product(&Value::Double(0.5), &dbls(&[2.0, 3.0]), &dbls(&[4.0, 5.0])),
            Some(Value::Double(23.5))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &dbls(&[2.0, 3.0]), &flat(&[4, 5])),
            Some(Value::Double(23.0))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &flat(&[2, 3]), &dbls(&[4.0, 5.0])),
            Some(Value::Double(23.0))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &mixed, &flat(&[2, 3])),
            Some(Value::Double(15.0))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &flat(&[2, 3]), &mixed),
            Some(Value::Double(15.0))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &mixed, &mixed),
            Some(Value::Double(18.25))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &dbls(&[2.0, 3.0]), &mixed),
            Some(Value::Double(15.0))
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &flat(&[i64::MAX]), &flat(&[2])),
            None
        );
        assert_eq!(
            try_dot_product(&Value::Double(0.0), &dbls(&[1.0]), &non_num),
            None
        );
        // Non-vector and non-numeric init misses
        assert_eq!(
            try_dot_product(&Value::Int(0), &Value::Nil, &flat(&[1])),
            None
        );
        assert_eq!(try_dot_product(&Value::Nil, &flat(&[1]), &flat(&[1])), None);

        // add_acc: all 4 numeric combos + i64 overflow fallback + non-int fallback
        let mut eval = crate::boot();
        let mut a = Value::Double(1.5);
        add_acc(&mut eval, &mut a, Value::Double(2.5)).expect("d+d");
        assert_eq!(a, Value::Double(4.0));
        add_acc(&mut eval, &mut a, Value::Int(3)).expect("d+i");
        assert_eq!(a, Value::Double(7.0));
        let mut b = Value::Int(2);
        add_acc(&mut eval, &mut b, Value::Double(0.5)).expect("i+d");
        assert_eq!(b, Value::Double(2.5));
        let mut c = Value::Int(10);
        add_acc(&mut eval, &mut c, Value::Int(5)).expect("i+i");
        assert_eq!(c, Value::Int(15));
        let mut ov = Value::Int(i64::MAX);
        assert!(add_acc(&mut eval, &mut ov, Value::Int(1)).is_err());
        let mut big = read_constant("10N").expect("10N");
        add_acc(&mut eval, &mut big, Value::Int(5)).expect("bigint+i");
        assert_eq!(big, read_constant("15N").expect("15N"));

        // SplitVec direct walking (`seqs::walk_direct`, `lazy::walk_all_other`, `filterv seq`)
        let sp = Value::Extension(Arc::new(clj_value::SplitVecValue::new(
            Arc::from(" ab c"),
            Arc::new(vec![(0, 0), (1, 2), (4, 1)]),
        )));
        let walker = eval
            .eval_source(
                "(fn [sp] [(vec (take 2 sp)) (some seq sp) (filterv seq sp) \
                  (vec (take 1 (subvec [\"x\" \"y\"] 0 2))) (some seq (subvec [\"x\" \"y\"] 0 2))])",
            )
            .expect("walker")
            .pop()
            .expect("fn");
        let split_walk = eval.call(&walker, &[sp]).expect("split walk");
        assert_eq!(
            print_constant(&split_walk),
            "[[\"\" \"ab\"] (\\a \\b) [\"ab\" \"c\"] [\"x\"] (\\x)]"
        );

        // native_abi::try_print_join thin wrapper
        let mut hit = 9u8;
        try_print_join(&mut eval, &Value::Int(1), &Value::Nil, 3, &mut hit).expect("miss");
        assert_eq!(hit, 0);

        // iter_plan + vec_item on SubVectorValue, SplitVecValue, and non-indexed Extension
        let sub_v = Value::Extension(Arc::new(clj_value::SubVectorValue::new(
            clj_value::Vector::from_ints(vec![10, 20, 30]),
            1,
            3,
        )));
        assert_eq!(iter_plan(&sub_v), Some((1, 2, 0, 0)));
        assert_eq!(vec_item(&sub_v, 0), Value::Int(20));
        assert_eq!(vec_item(&sub_v, 9), Value::Nil);
        let sp_v = Value::Extension(Arc::new(clj_value::SplitVecValue::new(
            Arc::from("x y"),
            Arc::new(vec![(0, 1), (2, 1)]),
        )));
        assert_eq!(iter_plan(&sp_v), Some((1, 2, 0, 0)));
        assert_eq!(vec_item(&sp_v, 1), Value::Str(Arc::from("y")));
        assert_eq!(vec_item(&sp_v, 9), Value::Nil);
        let rep_ext = Value::Extension(Arc::new(clj_value::RepeatValue::new(
            Value::Int(1),
            Some(2),
        )));
        assert_eq!(iter_plan(&rep_ext), None);
        assert_eq!(vec_item(&rep_ext, 0), Value::Nil);

        // VecBuilder: packed Doubles freeze, Doubles->Generic widening, and Debug across variants
        let mut vb_d = VecBuilder::default();
        assert!(format!("{vb_d:?}").contains("Ints"));
        vb_d.push(Value::Double(1.5));
        vb_d.push(Value::Double(2.5));
        assert!(format!("{vb_d:?}").contains("Doubles"));
        assert_eq!(vb_d.freeze().as_flat_doubles(), Some([1.5, 2.5].as_slice()));
        let mut vb_dg = VecBuilder::default();
        vb_dg.push(Value::Double(1.5));
        vb_dg.push(Value::Int(2));
        vb_dg.push(Value::Bool(true));
        assert!(format!("{vb_dg:?}").contains("Generic"));
        assert_eq!(vb_dg.freeze().len(), 3);

        // try_mapv_parse_num modes 3..=7
        let toks = Value::Vector(clj_value::Vector::from_vec(vec![
            Value::Str(Arc::from("42")),
            Value::Str(Arc::from("-7")),
        ]));
        assert_eq!(
            try_mapv_parse_num(&toks, 3),
            Some(Value::Vector(clj_value::Vector::from_ints(vec![42, -7])))
        );
        assert_eq!(
            try_mapv_parse_num(&toks, 6),
            Some(Value::Vector(clj_value::Vector::from_ints(vec![42, -7])))
        );
        let plus_tok = Value::Vector(clj_value::Vector::from_vec(vec![Value::Str(Arc::from(
            "+5",
        ))]));
        assert_eq!(try_mapv_parse_num(&plus_tok, 6), None);
        let non_ascii = Value::Vector(clj_value::Vector::from_vec(vec![Value::Str(Arc::from(
            "１",
        ))]));
        assert_eq!(try_mapv_parse_num(&non_ascii, 3), None);
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("-123")), 4),
            Some(Value::Int(-123))
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("+5")), 7),
            Some(Value::Nil)
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("-5")), 7),
            Some(Value::Int(-5))
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("abc")), 4),
            Some(Value::Nil)
        );
        assert_eq!(try_mapv_parse_num(&Value::Int(5), 4), Some(Value::Nil));
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("99")), 5),
            Some(Value::Int(99))
        );
        assert_eq!(try_mapv_parse_num(&Value::Str(Arc::from("x")), 5), None);
        assert_eq!(try_mapv_parse_num(&Value::Nil, 5), None);
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("")), 4),
            Some(Value::Nil)
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("-")), 4),
            Some(Value::Nil)
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("+7")), 4),
            Some(Value::Int(7))
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("9223372036854775807")), 4),
            Some(Value::Int(i64::MAX))
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("9223372036854775807")), 5),
            None
        );
        assert_eq!(
            try_mapv_parse_num(&Value::Str(Arc::from("١٢٣٤٥٦٧٨٩٠١٢٣٤٥٦٧٨٩")), 4),
            Some(Value::Nil)
        );
    }
}
