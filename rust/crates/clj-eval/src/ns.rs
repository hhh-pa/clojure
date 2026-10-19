// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5A2 namespace vars, the loader, and `boot` (normative:
//! `docs/t1174/namespaces-contract.md`, oracles probe35-36).
//!
//! Natives receive the live evaluator (§N), so `in-ns` switches the
//! registry current, `require` evaluates files, and `eval` re-enters
//! [`Evaluator::eval_top_form`]. Stale handles read the registry's
//! tombstones, like the JVM's detached namespace objects.

use std::sync::Arc;

use clj_value::{
    Keyword, List, Map, PrintOpts, Set, Symbol, ThrowClass, Throwable, Value, Var, Vector, new_map,
    new_set, pr_to_string, seq_vec,
};

use crate::env::{Namespace, SharedRegistry};
use crate::error::{NativeError, Signal, jvm_class_of, site_of};
use crate::eval::{Evaluator, SourceError};
use crate::expr::NsClause;
use crate::ext::{NsHandle, as_ns};

/// `clojure.lang.Symbol`, the cast target for namespace arguments.
pub(crate) const SYMBOL_CLASS: &str = "clojure.lang.Symbol";

/// `clojure.lang.Named`, the cast target for lib names.
const NAMED_CLASS: &str = "clojure.lang.Named";

/// The `find`/`findOrCreate` nil shape (a null key into the
/// namespace table, probe35 `find-ns-nil`).
const NIL_KEY_NPE: &str = "Cannot invoke \"Object.hashCode()\" because \"key\" is null";

/// Reading `.ns` off a nil symbol (probe35 `resolve-nil`).
const NIL_SYM_NPE: &str = "Cannot read field \"ns\" because \"sym\" is null";

/// Reading `.getName` off a nil nameable (probe36 `req-nil-lib`).
const NIL_NAME_NPE: &str = "Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null";

/// The embedded `clojure.core` composites: `boot` loads this with
/// the current namespace already `clojure.core` (so the file carries
/// intentionally no `ns` form), then refers the publics into `user`.
const CORE_CLJ: &str = include_str!("core.clj");

pub(crate) fn npe(message: &str) -> NativeError {
    NativeError::new(ThrowClass::NullPointerException, message.to_owned())
}

pub(crate) fn ex(message: String) -> NativeError {
    NativeError::new(ThrowClass::Exception, message)
}

pub(crate) fn cast_err(v: &Value, to: &str) -> NativeError {
    NativeError::cast(&jvm_class_of(v), to)
}

/// `str` a value (`nil` prints empty, unlike `print`).
pub(crate) fn str_of(v: &Value) -> String {
    match v {
        Value::Nil => String::new(),
        _ => pr_to_string(v, &PrintOpts::print()),
    }
}

/// A symbol as written (`a/b` or `a`).
pub(crate) fn sym_str(s: &Symbol) -> String {
    match s.namespace() {
        Some(ns) => format!("{ns}/{}", s.name()),
        None => s.name().to_owned(),
    }
}

/// A strict namespace-name symbol: nil is the null-key NPE, anything
/// else non-symbol is the `Symbol` cast (the `find`/`findOrCreate`
/// path, probe35 `find-ns-ns-arg`).
fn strict_sym(v: &Value) -> Result<&Symbol, NativeError> {
    match v {
        Value::Symbol(s) => Ok(s),
        Value::Nil => Err(npe(NIL_KEY_NPE)),
        _ => Err(cast_err(v, SYMBOL_CLASS)),
    }
}

/// `clojure.core/the-ns`: a handle resolves to its name (handles
/// never miss, like the JVM's identity return — stale ones read
/// tombstones downstream); a symbol must name a live namespace.
fn the_ns_name(reg: &SharedRegistry, v: &Value) -> Result<String, NativeError> {
    if let Some(h) = as_ns(v) {
        return Ok(h.name.to_string());
    }
    let key = sym_str(strict_sym(v)?);
    if reg.get(&key).is_some() {
        Ok(key)
    } else {
        Err(ex(format!("No namespace: {key} found")))
    }
}

/// A namespace handle value for a live-or-removed namespace.
fn ns_value(name: &str, id: u64) -> Value {
    Value::Extension(Arc::new(NsHandle {
        name: Arc::from(name),
        id,
    }))
}

/// A var value for an id and qualified symbol.
pub(crate) fn var_value(id: u64, sym: &Symbol) -> Value {
    Value::Var(Var {
        id,
        sym: sym.clone(),
    })
}

/// Stamp the `:ns`/`:name` keys onto a var meta map (JVM
/// `Var.intern`: always present, winning over adopted meta).
pub(crate) fn stamped_meta(base: Map, ns_name: &str, ns_id: u64, name: &str) -> Map {
    let ns_key = Value::Keyword(Keyword::new(None, "ns"));
    let name_key = Value::Keyword(Keyword::new(None, "name"));
    base.insert(ns_key, ns_value(ns_name, ns_id))
        .insert(name_key, Value::Symbol(Symbol::new(None, name)))
}

/// Whether a var id is public (`^:private` meta is truthiness-tested,
/// `Var.isPublic`).
pub(crate) fn is_public(reg: &SharedRegistry, id: u64) -> bool {
    let probe = Value::Keyword(Keyword::new(None, "private"));
    !reg.state(id).is_some_and(|s| {
        s.meta
            .as_ref()
            .and_then(|m| m.get(&probe))
            .is_some_and(Value::is_truthy)
    })
}

/// Resolve a symbol against an explicit namespace home, returning the
/// qualified target and the var id. Refers yield their install-pinned
/// id (the referred object survives re-interns, like the JVM); alias
/// targets may dangle into tombstones, but real-namespace qualifiers
/// must be live.
fn resolve_var_in(home: &Namespace, reg: &SharedRegistry, sym: &Symbol) -> Option<(Symbol, u64)> {
    match sym.namespace() {
        Some(part) if part == home.name() => home.lookup(sym).map(|id| (sym.clone(), id)),
        Some(part) => resolve_qualified(home, reg, part, sym),
        None => {
            let q = Symbol::new(Some(home.name()), sym.name());
            if let Some(id) = home.lookup(&q) {
                return Some((q, id));
            }
            home.refers()
                .find(|(n, _, _)| *n == sym.name())
                .map(|(_, t, i)| (t.clone(), i))
        }
    }
}

/// The qualified arm of [`resolve_var_in`]: aliases (which may
/// dangle into tombstones) win over live namespaces.
fn resolve_qualified(
    home: &Namespace,
    reg: &SharedRegistry,
    part: &str,
    sym: &Symbol,
) -> Option<(Symbol, u64)> {
    let target = home
        .aliases()
        .find(|(a, _, _)| *a == part)
        .map(|(_, t, _)| t.to_owned())
        .or_else(|| reg.get(part).map(|_| part.to_owned()))?;
    let target_ns = reg.ns_live_or_removed(&target)?;
    let q = Symbol::new(Some(target.as_str()), sym.name());
    target_ns.lookup(&q).map(|id| (q, id))
}

/// Build a symbol-keyed map from `(simple-name, value)` pairs, sorted
/// by name (deterministic; JVM hash order is unspecified).
fn sorted_map(pairs: Vec<(String, Value)>) -> Map {
    let mut pairs = pairs;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = new_map();
    for (name, v) in pairs {
        out = out.insert(Value::Symbol(Symbol::new(None, &name)), v);
    }
    out
}

/// Fixed arities (`display` is the qualified `clojure.core/name`,
/// probe35 `arity-intern-1`).
pub(crate) fn check_arity(
    display: &str,
    args: &[Value],
    want: &[usize],
) -> Result<(), NativeError> {
    if want.contains(&args.len()) {
        Ok(())
    } else {
        Err(NativeError::arity(display, args.len()))
    }
}

/// A qualified symbol's var id, without the alias step (the target
/// is already resolved).
fn id_of(reg: &SharedRegistry, q: &Symbol) -> Option<u64> {
    reg.get(q.namespace()?).and_then(|ns| ns.lookup(q))
}

/// The ISeq coercion error (probe36 `req-string`).
fn iseq_err(v: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Don't know how to create ISeq from: {}", jvm_class_of(v)),
    )
}

/// A `throw` signal as a native error (the class, message, data, and
/// cause survive; the evaluator stamps a fresh identity).
fn throwable_to_native(t: &Throwable) -> NativeError {
    let mut err = NativeError::new(t.class, t.message.as_deref().unwrap_or("").to_owned());
    err.data = t.data.clone().map(|b| *b);
    err.cause = t.cause.clone().map(|b| *b);
    err
}

/// An invoked value's failure as a native error (an escaping `recur`
/// is unreachable — the analyzer cages every one).
pub(crate) fn signal_to_native(s: Signal) -> NativeError {
    match s {
        Signal::Throw(t) => throwable_to_native(&t),
        Signal::Recur(_) => ex("recur escaped its loop or fn".to_owned()),
    }
}

fn native_create_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/create-ns", args, &[1])?;
    let key = sym_str(strict_sym(&args[0])?);
    let id = eval.registry.ensure_ns(&key).id();
    Ok(ns_value(&key, id))
}

fn native_in_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/in-ns", args, &[1])?;
    let key = sym_str(strict_sym(&args[0])?);
    let id = eval.registry.ensure_ns(&key).id();
    eval.registry.set_current(&key);
    // `RT.inNamespace` is a thread-local `set`: the top frame when
    // bound (boot pushes a base frame, so the root never moves),
    // else the root.
    let star = Symbol::new(Some("clojure.core"), "*ns*");
    if let Some(var_id) = id_of(&eval.registry, &star) {
        set_ns_current(eval, var_id, ns_value(&key, id));
    }
    Ok(ns_value(&key, id))
}

/// Overwrite one var's root.
fn set_root(reg: &SharedRegistry, id: u64, v: Value) {
    reg.with_state_mut(id, |st| {
        st.root = Some(v);
    });
}

/// `in-ns` assignment: the top `*ns*` frame when bound, else the
/// root (`RT.inNamespace` is a thread-local `set`).
fn set_ns_current(eval: &mut Evaluator, var_id: u64, handle: Value) {
    if !eval.set_binding_top(var_id, handle.clone()) {
        set_root(&eval.registry, var_id, handle);
    }
}

fn native_ns_name(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ns-name", args, &[1])?;
    let name = the_ns_name(&eval.registry, &args[0])?;
    Ok(Value::Symbol(Symbol::new(None, &name)))
}

fn native_the_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/the-ns", args, &[1])?;
    if as_ns(&args[0]).is_some() {
        return Ok(args[0].clone());
    }
    let key = the_ns_name(&eval.registry, &args[0])?;
    match eval.registry.ns_id(&key) {
        Some(id) => Ok(ns_value(&key, id)),
        None => Err(ex(format!("No namespace: {key} found"))),
    }
}

fn native_find_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/find-ns", args, &[1])?;
    let key = sym_str(strict_sym(&args[0])?);
    match eval.registry.ns_id(&key) {
        Some(id) => Ok(ns_value(&key, id)),
        None => Ok(Value::Nil),
    }
}

fn native_remove_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/remove-ns", args, &[1])?;
    let sym = match &args[0] {
        Value::Nil => {
            return Err(npe(
                "Cannot invoke \"clojure.lang.Symbol.equals(Object)\" because \"name\" is null",
            ));
        }
        Value::Symbol(s) => s,
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    let key = sym_str(sym);
    if key == "clojure.core" {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Cannot remove clojure namespace".to_owned(),
        ));
    }
    match eval.registry.ns_id(&key) {
        Some(id) => {
            eval.registry.remove_ns(&key);
            Ok(ns_value(&key, id))
        }
        None => Ok(Value::Nil),
    }
}

fn native_all_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/all-ns", args, &[0])?;
    let mut out = Vec::new();
    for (name, id) in eval.registry.all_ns_sorted() {
        out.push(ns_value(&name, id));
    }
    Ok(Value::List(List::from_vec(out)))
}

/// The home namespace for the `ns-*` family (handles read
/// tombstones; the `None` arm is unreachable — `the-ns` checked).
fn family_home(eval: &Evaluator, args: &[Value], display: &str) -> Result<Namespace, NativeError> {
    check_arity(display, args, &[1])?;
    let name = the_ns_name(&eval.registry, &args[0])?;
    // Handles pin their generation (stale ones read the tombstone);
    // symbols resolve by name.
    let found = match as_ns(&args[0]).map(|h| h.id) {
        Some(id) => eval.registry.ns_for_handle(&name, id),
        None => eval.registry.ns_live_or_removed(&name),
    };
    match found {
        Some(ns) => Ok(ns),
        None => Err(ex(format!("No namespace: {name} found"))),
    }
}

fn native_ns_map(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let home = family_home(eval, args, "clojure.core/ns-map")?;
    // Refers first: interns overwrite on collision (stable sort
    // keeps that order for equal names).
    let mut pairs = Vec::new();
    for (n, t, i) in home.refers() {
        pairs.push((n.to_owned(), var_value(i, t)));
    }
    for (s, i) in home.interns() {
        pairs.push((s.name().to_owned(), var_value(i, s)));
    }
    Ok(Value::Map(sorted_map(pairs)))
}

fn native_ns_interns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let home = family_home(eval, args, "clojure.core/ns-interns")?;
    let pairs: Vec<(String, Value)> = home
        .interns()
        .map(|(s, i)| (s.name().to_owned(), var_value(i, s)))
        .collect();
    Ok(Value::Map(sorted_map(pairs)))
}

fn native_ns_publics(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let home = family_home(eval, args, "clojure.core/ns-publics")?;
    let pairs: Vec<(String, Value)> = home
        .interns()
        .filter(|(_, i)| is_public(&eval.registry, *i))
        .map(|(s, i)| (s.name().to_owned(), var_value(i, s)))
        .collect();
    Ok(Value::Map(sorted_map(pairs)))
}

fn native_ns_refers(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let home = family_home(eval, args, "clojure.core/ns-refers")?;
    let pairs: Vec<(String, Value)> = home
        .refers()
        .map(|(n, t, i)| (n.to_owned(), var_value(i, t)))
        .collect();
    Ok(Value::Map(sorted_map(pairs)))
}

fn native_ns_imports(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = family_home(eval, args, "clojure.core/ns-imports")?;
    Ok(Value::Map(new_map()))
}

fn native_ns_aliases(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let home = family_home(eval, args, "clojure.core/ns-aliases")?;
    let pairs: Vec<(String, Value)> = home
        .aliases()
        .map(|(a, t, i)| (a.to_owned(), ns_value(t, i)))
        .collect();
    // Alias keys keep the installed spelling (qualified aliases
    // re-split on the first `/`, like the reader).
    let mut pairs = pairs;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = new_map();
    for (name, v) in pairs {
        out = out.insert(Value::Symbol(Symbol::intern(&name)), v);
    }
    Ok(Value::Map(out))
}

fn native_alias(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/alias", args, &[2])?;
    // The target resolves first: call-argument order beats the name
    // cast (core.clj `alias` evaluates `(the-ns ...)` before the
    // `addAlias` checkcast runs).
    let target = the_ns_name(&eval.registry, &args[1])?;
    let target_id = match eval.registry.ns_live_or_removed(&target) {
        Some(ns) => ns.id(),
        None => return Err(ex(format!("No namespace: {target} found"))),
    };
    let alias_sym = match &args[0] {
        Value::Nil => return Err(npe("Expecting Symbol + Namespace")),
        Value::Symbol(s) => s,
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    let a = sym_str(alias_sym);
    let cur = eval.current_ns_name();
    eval.registry.ensure_ns(&cur);
    match eval
        .registry
        .with_ns_mut(&cur, |home| home.add_alias(&a, &target, target_id))
    {
        Some(Ok(())) | None => Ok(Value::Nil),
        Some(Err(old)) => Err(NativeError::new(
            ThrowClass::IllegalStateException,
            format!("Alias {a} already exists in namespace {cur}, aliasing {old}"),
        )),
    }
}

/// Option pairs (`(apply hash-map filters)` fails odd counts naming
/// the last item, probe36 `refer-odd-filters`).
fn opt_pairs(filters: &[Value]) -> Result<Vec<(&Value, &Value)>, NativeError> {
    if filters.len() % 2 == 1 {
        let last = &filters[filters.len() - 1];
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No value supplied for key: {}", str_of(last)),
        ));
    }
    Ok((0..filters.len())
        .step_by(2)
        .map(|i| (&filters[i], &filters[i + 1]))
        .collect())
}

/// Look up an exact (unqualified) keyword option (later pairs win).
fn opt_get<'a>(pairs: &[(&'a Value, &'a Value)], name: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .rev()
        .find(|(k, _)| {
            matches!(k, Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == name)
        })
        .map(|(_, v)| *v)
}

/// The `:only`/`:refer` sequential check (lists and vectors only —
/// strings, maps, and sets fail like the JVM's `Sequential` test).
fn check_sequential(v: &Value) -> Result<Vec<Value>, NativeError> {
    match v {
        Value::List(l) => Ok(l.iter().cloned().collect()),
        Value::Vector(x) => Ok(x.iter().cloned().collect()),
        _ => Err(ex(
            ":only/:refer value must be a sequential collection of symbols".to_owned(),
        )),
    }
}

/// `refer` (and `refer-clojure`, which fixes the lib): refer the
/// source's publics into the current namespace, subject to filters.
/// A namespace's public vars, sorted by name: qualified symbol
/// plus the id pinned for each refer.
fn refer_publics(eval: &Evaluator, lib_key: &str) -> Vec<(Symbol, u64)> {
    let mut publics = Vec::new();
    if let Some(home) = eval.registry.get(lib_key) {
        for (s, i) in home.interns() {
            if is_public(&eval.registry, i) {
                publics.push((s.clone(), i));
            }
        }
    }
    publics.sort_by(|a, b| a.0.name().cmp(b.0.name()));
    publics
}

/// The `refer` work list: `(or (:refer fs) (:only fs) (keys
/// nspublics))`, with nils falling through and `:all` meaning every
/// public (core.clj `refer`).
fn refer_todo(
    public_names: Vec<Value>,
    pairs: &[(&Value, &Value)],
) -> Result<Vec<Value>, NativeError> {
    let refer_v = opt_get(pairs, "refer").filter(|v| !matches!(*v, Value::Nil));
    let only_v = opt_get(pairs, "only").filter(|v| !matches!(*v, Value::Nil));
    match refer_v {
        Some(Value::Keyword(kw)) if kw.namespace().is_none() && kw.name() == "all" => {
            Ok(public_names)
        }
        Some(v) => check_sequential(v),
        None => match only_v {
            Some(v) => check_sequential(v),
            None => Ok(public_names),
        },
    }
}

/// The `refer` exclusion set (`(set (:exclude fs))`).
fn refer_exclude(pairs: &[(&Value, &Value)]) -> Result<Vec<Value>, NativeError> {
    match opt_get(pairs, "exclude") {
        None | Some(Value::Nil) => Ok(Vec::new()),
        Some(v) => seq_vec(v).ok_or_else(|| iseq_err(v)),
    }
}

/// Resolve one `refer` item to its simple name plus the target var
/// (private and absent vars are the two `IllegalAccessError`s).
fn refer_target(
    eval: &Evaluator,
    lib_key: &str,
    publics: &[(Symbol, u64)],
    item: &Value,
) -> Result<(Symbol, Symbol, u64), NativeError> {
    let item_sym = match item {
        Value::Symbol(s) if s.namespace().is_none() => s,
        _ => {
            return Err(NativeError::new(
                ThrowClass::IllegalAccessError,
                format!("{} does not exist", str_of(item)),
            ));
        }
    };
    match publics.iter().find(|(s, _)| s.name() == item_sym.name()) {
        Some((s, i)) => Ok(((*item_sym).clone(), s.clone(), *i)),
        None => {
            let interned = eval
                .registry
                .get(lib_key)
                .and_then(|home| home.lookup(&Symbol::new(Some(lib_key), item_sym.name())))
                .is_some();
            let why = if interned {
                "is not public"
            } else {
                "does not exist"
            };
            Err(NativeError::new(
                ThrowClass::IllegalAccessError,
                format!("{} {why}", item_sym.name()),
            ))
        }
    }
}

/// Apply the `:rename` option to one item (a map looks up, anything
/// else invokes with nil falling back to the plain name).
fn refer_renamed(
    eval: &mut Evaluator,
    rename: Option<&Value>,
    item_sym: &Symbol,
    item: &Value,
) -> Result<Symbol, NativeError> {
    let renamed = match rename {
        None => item_sym.clone(),
        Some(Value::Map(m)) => match m.get(item) {
            None => item_sym.clone(),
            Some(Value::Symbol(s)) => s.clone(),
            Some(v) => return Err(cast_err(v, SYMBOL_CLASS)),
        },
        Some(f) => match eval.call(f, std::slice::from_ref(item)) {
            Ok(Value::Nil) => item_sym.clone(),
            Ok(Value::Symbol(s)) => s,
            Ok(v) => return Err(cast_err(&v, SYMBOL_CLASS)),
            Err(s) => return Err(signal_to_native(s)),
        },
    };
    if renamed.namespace().is_some() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Can't intern namespace-qualified symbol".to_owned(),
        ));
    }
    Ok(renamed)
}

fn refer_into(eval: &mut Evaluator, lib: &Value, filters: &[Value]) -> Result<Value, NativeError> {
    let lib_key = sym_str(strict_sym(lib)?);
    if eval.registry.get(&lib_key).is_none() {
        return Err(ex(format!("No namespace: {lib_key}")));
    }
    let pairs = opt_pairs(filters)?;
    let publics = refer_publics(eval, &lib_key);
    let public_names: Vec<Value> = publics
        .iter()
        .map(|(s, _)| Value::Symbol(Symbol::new(None, s.name())))
        .collect();
    let to_do = refer_todo(public_names, &pairs)?;
    let exclude = refer_exclude(&pairs)?;
    let rename = match opt_get(&pairs, "rename") {
        None | Some(Value::Nil) | Some(Value::Bool(false)) => None,
        Some(v) => Some(v),
    };
    for item in &to_do {
        if exclude.contains(item) {
            continue;
        }
        let (item_sym, target_q, var_id) = refer_target(eval, &lib_key, &publics, item)?;
        let renamed = refer_renamed(eval, rename, &item_sym, item)?;
        // Interns win over refers (the JVM's REJECTED warning waits
        // for `*err*` in S5D).
        let cur = eval.current_ns_name();
        eval.registry.ensure_ns(&cur);
        let q = Symbol::new(Some(cur.as_str()), renamed.name());
        eval.registry.with_ns_mut(&cur, |home| {
            if home.lookup(&q).is_none() {
                home.add_refer(renamed.name(), target_q, var_id);
            }
        });
    }
    Ok(Value::Nil)
}

fn native_refer(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/refer", 0));
    }
    refer_into(eval, &args[0], &args[1..])
}

fn native_refer_clojure(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let lib = Value::Symbol(Symbol::new(None, "clojure.core"));
    refer_into(eval, &lib, args)
}

/// Route an `intern` through a handle pin (stale handles land in
/// the tombstone, which becomes the var's home) or by name (live
/// first). Returns the var id and the home namespace id.
fn intern_routed(
    eval: &mut Evaluator,
    ns_name: &str,
    pinned: Option<u64>,
    q: &Symbol,
) -> Result<(u64, u64), NativeError> {
    match pinned {
        Some(expected) => eval
            .registry
            .intern_in_pinned(ns_name, expected, q)
            .ok_or_else(|| ex(format!("No namespace: {ns_name} found"))),
        None => {
            let id = eval
                .registry
                .intern_in(ns_name, q)
                .ok_or_else(|| ex(format!("No namespace: {ns_name} found")))?;
            Ok((id, eval.registry.get(ns_name).map_or(0, |ns| ns.id())))
        }
    }
}

fn native_intern(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/intern", args, &[2, 3])?;
    let ns_name = the_ns_name(&eval.registry, &args[0])?;
    let name = match &args[1] {
        Value::Nil => return Err(npe(NIL_SYM_NPE)),
        Value::Symbol(s) if s.namespace().is_some() => {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "Can't intern namespace-qualified symbol".to_owned(),
            ));
        }
        Value::Symbol(s) => s,
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    let meta = name.meta().cloned();
    let q = Symbol::new(Some(ns_name.as_str()), name.name());
    let pinned = as_ns(&args[0]).map(|h| h.id);
    let (id, home) = intern_routed(eval, &ns_name, pinned, &q)?;
    // The var adopts the name symbol's meta, if any (core.clj
    // `intern`); 3-arity resets the root on the same var. The
    // `:ns`/`:name` keys are always (re)stamped (JVM
    // `Var.intern`; a cleared meta stays cleared).
    let reset = args.get(2).cloned();
    eval.registry.with_state_mut(id, |st| {
        if let Some(m) = meta {
            st.meta = Some(m);
        }
        if let Some(m) = st.meta.clone() {
            st.meta = Some(stamped_meta(m, &ns_name, home, name.name()));
        }
        if let Some(v) = reset {
            st.root = Some(v);
        }
    });
    Ok(var_value(id, &q))
}

/// `ns-resolve`/`resolve` through an explicit home (the env check
/// runs before symbol validation, like the `when-not` in
/// `ns-resolve`).
fn do_resolve(
    eval: &Evaluator,
    ns_name: &str,
    pinned: Option<u64>,
    env: Option<&Value>,
    sym_v: &Value,
) -> Result<Value, NativeError> {
    if let Some(e) = env {
        let hit = match e {
            Value::Map(m) => m.contains_key(sym_v),
            Value::Set(s) => s.contains(sym_v),
            _ => false,
        };
        if hit {
            return Ok(Value::Nil);
        }
    }
    let sym = match sym_v {
        Value::Nil => return Err(npe(NIL_SYM_NPE)),
        Value::Symbol(s) => s,
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    // Handles pin their generation (stale ones resolve the tombstone).
    let home = match pinned {
        Some(id) => eval.registry.ns_for_handle(ns_name, id),
        None => eval.registry.ns_live_or_removed(ns_name),
    };
    match home.and_then(|h| resolve_var_in(&h, &eval.registry, sym)) {
        Some((q, id)) => Ok(var_value(id, &q)),
        None => Ok(Value::Nil),
    }
}

fn native_ns_resolve(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ns-resolve", args, &[2, 3])?;
    let ns_name = the_ns_name(&eval.registry, &args[0])?;
    let (env, sym_v) = if args.len() == 3 {
        (Some(&args[1]), &args[2])
    } else {
        (None, &args[1])
    };
    let pinned = as_ns(&args[0]).map(|h| h.id);
    do_resolve(eval, &ns_name, pinned, env, sym_v)
}

fn native_resolve(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/resolve", args, &[1, 2])?;
    let cur = eval.current_ns_name();
    let (env, sym_v) = if args.len() == 2 {
        (Some(&args[0]), &args[1])
    } else {
        (None, &args[0])
    };
    do_resolve(eval, &cur, None, env, sym_v)
}

fn native_ns_unmap(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ns-unmap", args, &[2])?;
    let ns_name = the_ns_name(&eval.registry, &args[0])?;
    let simple = match &args[1] {
        Value::Nil => return Err(npe(NIL_SYM_NPE)),
        Value::Symbol(s) if s.namespace().is_none() => s.name().to_owned(),
        Value::Symbol(_) => return Ok(Value::Nil),
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    // Handles pin their generation (stale ones unmap the tombstone).
    match as_ns(&args[0]).map(|h| h.id) {
        Some(id) => eval
            .registry
            .with_handle_mut(&ns_name, id, |home| home.unmap(&simple)),
        None => eval
            .registry
            .with_live_or_removed_mut(&ns_name, |home| home.unmap(&simple)),
    };
    Ok(Value::Nil)
}

fn native_ns_unalias(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/ns-unalias", args, &[2])?;
    let ns_name = the_ns_name(&eval.registry, &args[0])?;
    let simple = match &args[1] {
        Value::Nil => return Ok(Value::Nil),
        Value::Symbol(s) => sym_str(s),
        v => return Err(cast_err(v, SYMBOL_CLASS)),
    };
    // Handles pin their generation (stale ones unalias the tombstone).
    match as_ns(&args[0]).map(|h| h.id) {
        Some(id) => eval
            .registry
            .with_handle_mut(&ns_name, id, |home| home.remove_alias(&simple)),
        None => eval
            .registry
            .with_live_or_removed_mut(&ns_name, |home| home.remove_alias(&simple)),
    };
    Ok(Value::Nil)
}

/// A JVM class name (simple or fully qualified) to its
/// [`ThrowClass`]. Unknown names — including the port-only
/// `ReaderError` deferrals — fall back to `Exception`.
pub(crate) fn throw_class_for(name: &str) -> ThrowClass {
    match name.rsplit('.').next().unwrap_or(name) {
        "Exception" => ThrowClass::Exception,
        "Error" => ThrowClass::Error,
        "RuntimeException" => ThrowClass::RuntimeException,
        "IllegalArgumentException" => ThrowClass::IllegalArgumentException,
        "IllegalStateException" => ThrowClass::IllegalStateException,
        "UnsupportedOperationException" => ThrowClass::UnsupportedOperationException,
        "AssertionError" => ThrowClass::AssertionError,
        "ArityException" => ThrowClass::ArityException,
        "NullPointerException" => ThrowClass::NullPointerException,
        "ClassCastException" => ThrowClass::ClassCastException,
        "IndexOutOfBoundsException" => ThrowClass::IndexOutOfBoundsException,
        "ArithmeticException" => ThrowClass::ArithmeticException,
        "ExceptionInfo" => ThrowClass::ExceptionInfo,
        "FileNotFoundException" => ThrowClass::FileNotFoundException,
        "ExecutionException" => ThrowClass::ExecutionException,
        "CancellationException" => ThrowClass::CancellationException,
        "RejectedExecutionException" => ThrowClass::RejectedExecutionException,
        "IllegalAccessError" => ThrowClass::IllegalAccessError,
        "NumberFormatException" => ThrowClass::NumberFormatException,
        "StringIndexOutOfBoundsException" => ThrowClass::StringIndexOutOfBoundsException,
        "IllegalFormatConversionException" => ThrowClass::IllegalFormatConversionException,
        "UnknownFormatConversionException" => ThrowClass::UnknownFormatConversionException,
        "MissingFormatConversionException" => ThrowClass::MissingFormatConversionException,
        "MissingFormatArgumentException" => ThrowClass::MissingFormatArgumentException,
        "IllegalFormatPrecisionException" => ThrowClass::IllegalFormatPrecisionException,
        "IllegalFormatCodePointException" => ThrowClass::IllegalFormatCodePointException,
        "IOException" => ThrowClass::IOException,
        "Compiler$CompilerException" => ThrowClass::CompilerException,
        _ => ThrowClass::Exception,
    }
}

/// A load/eval failure as a native error: a real
/// `CompilerException` wrapper with the structured cause (so `catch`
/// and `ex-cause` walk the JVM chain); bare failures (macro arity)
/// escape unwrapped, cause class and cause message only.
fn source_to_native(eval: &mut Evaluator, source: &str, e: SourceError) -> NativeError {
    match e {
        SourceError::Reader(e) => {
            let span = e.span();
            let mut outer = NativeError::new(
                ThrowClass::CompilerException,
                format!(
                    "Syntax error reading source at ({}).",
                    site_of(source, span)
                ),
            );
            outer.cause = Some(Throwable {
                id: eval.next_id(),
                class: throw_class_for(e.jvm_class()),
                message: Some(Arc::from(e.message().as_str())),
                data: None,
                cause: None,
            });
            outer
        }
        SourceError::Compile(e) => {
            if e.bare {
                return NativeError::new(throw_class_for(e.class), e.message.clone());
            }
            let mut outer = NativeError::new(ThrowClass::CompilerException, e.wrapper());
            outer.cause = Some(Throwable {
                id: eval.next_id(),
                class: throw_class_for(e.class),
                message: Some(Arc::from(e.message.as_str())),
                data: None,
                cause: None,
            });
            outer
        }
        SourceError::Uncaught(t) => throwable_to_native(&t),
        SourceError::Internal(m) => ex(format!("internal error: {m}")),
    }
}

/// A `clojure.core` var id, if installed.
/// A `Var` handle for a `clojure.core` var id (binding frames
/// carry the handle so snapshots print without re-lookup).
fn core_var_handle(id: u64, name: &str) -> Var {
    Var {
        id,
        sym: Symbol::new(Some("clojure.core"), name),
    }
}

fn core_var(eval: &Evaluator, name: &str) -> Option<u64> {
    id_of(&eval.registry, &Symbol::new(Some("clojure.core"), name))
}

/// The visible value of a `clojure.core` var (total fallback nil —
/// boot installs every var the loader touches).
pub(crate) fn deref_core(eval: &Evaluator, name: &str) -> Value {
    match core_var(eval, name) {
        Some(id) => eval.deref_var(id, &Symbol::new(Some("clojure.core"), name)),
        None => Value::Nil,
    }
}

/// The visible `*loaded-libs*` set.
fn libs_set(eval: &Evaluator) -> Result<Set, NativeError> {
    match deref_core(eval, "*loaded-libs*") {
        Value::Set(s) => Ok(s),
        v => Err(cast_err(&v, "clojure.lang.IPersistentSet")),
    }
}

/// `*loaded-libs*` assignment: the top frame when thread-bound
/// (`load-all` scopes a blank set per load), else the root
/// (mirrors [`set_ns_current`]).
fn set_loaded_libs(eval: &mut Evaluator, v: Value) {
    // (Match guard, not a let-chain: MSRV 1.85.)
    match core_var(eval, "*loaded-libs*") {
        Some(id) if !eval.set_binding_top(id, v.clone()) => {
            set_root(&eval.registry, id, v);
        }
        _ => {}
    }
}

/// Mark a lib loaded (conjs the visible set — the port holds the
/// set directly, where the JVM commutes a `Ref` — and writes it
/// back through [`set_loaded_libs`], so marks under `load-all`
/// accumulate in its frame instead of leaking to the root).
fn mark_loaded(eval: &mut Evaluator, lib: &Symbol) -> Result<(), NativeError> {
    let set = libs_set(eval)?;
    set_loaded_libs(eval, Value::Set(set.insert(Value::Symbol(lib.clone()))));
    Ok(())
}

/// `(name x)`: strings pass through, symbols and keywords give their
/// name, nil is the `getName` NPE, anything else the `Named` cast
/// (core.clj `name`, probe35 `name-num`).
fn name_of(v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Str(s) => Ok(s.to_string()),
        Value::Symbol(s) => Ok(s.name().to_owned()),
        Value::Keyword(k) => Ok(k.name().to_owned()),
        Value::Nil => Err(npe(NIL_NAME_NPE)),
        _ => Err(cast_err(v, NAMED_CLASS)),
    }
}

/// The root resource for a lib name: dashes to underscores, dots to
/// slashes, with a leading slash (core.clj `root-resource`).
fn root_resource(lib_name: &str) -> String {
    let mapped: String = lib_name
        .chars()
        .map(|c| match c {
            '-' => '_',
            '.' => '/',
            _ => c,
        })
        .collect();
    format!("/{mapped}")
}

/// The root directory for a namespace name (core.clj
/// `root-directory`): the resource minus its last segment.
fn root_directory(ns_name: &str) -> String {
    let r = root_resource(ns_name);
    match r.rfind('/') {
        Some(i) => r[..i].to_owned(),
        None => String::new(),
    }
}

/// The missing-file error, verbatim (`RT.load`: the three tried
/// names plus the underscore hint, probe36 `req-missing`).
fn could_not_locate(root: &str) -> NativeError {
    let mut msg =
        format!("Could not locate {root}__init.class, {root}.clj or {root}.cljc on classpath.");
    if root.contains('_') {
        msg.push_str(
            " Please check that namespaces with dashes use underscores in the Clojure file name.",
        );
    }
    NativeError::new(ThrowClass::FileNotFoundException, msg)
}

/// A filesystem failure as `FileNotFoundException` with the path as
/// given (Rust's ` (os error N)` suffix is stripped — the JVM shape
/// is the bare OS message, probe36 `loadfile-missing`).
fn fs_err(path: &str, e: &std::io::Error) -> NativeError {
    let mut msg = e.to_string();
    if let Some(i) = msg.rfind(" (os error ").filter(|_| msg.ends_with(')')) {
        msg.truncate(i);
    }
    NativeError::new(ThrowClass::FileNotFoundException, format!("{path} ({msg})"))
}

/// The `*load-path*` directories (seqable, lenient nil; entries must
/// be strings).
fn load_dirs(eval: &Evaluator) -> Result<Vec<String>, NativeError> {
    let v = deref_core(eval, "*load-path*");
    let items = seq_vec(&v).ok_or_else(|| cast_err(&v, "clojure.lang.IPersistentVector"))?;
    let mut out = Vec::new();
    for item in &items {
        match item {
            Value::Str(s) => out.push(s.to_string()),
            _ => return Err(cast_err(item, "java.lang.String")),
        }
    }
    Ok(out)
}

/// Resolve a load root (no leading slash) to the `*file*` path and
/// text: embedded libs first (none yet — the flat set lives here
/// when S5D lands), then `.clj` across every `*load-path*` dir, then
/// `.cljc` (`RT.load` lookup order).
fn resolve_source(eval: &Evaluator, root: &str) -> Result<(String, String), NativeError> {
    let dirs = load_dirs(eval)?;
    for ext in ["clj", "cljc"] {
        for dir in &dirs {
            // `*file*` pins `/` separators on every platform (S5A2
            // file_star_during_load); Win32 file access accepts them.
            let candidate = format!("{dir}/{root}.{ext}").replace('\\', "/");
            if std::path::Path::new(&candidate).is_file() {
                match std::fs::read_to_string(&candidate) {
                    Ok(text) => return Ok((candidate, text)),
                    Err(e) => return Err(fs_err(&candidate, &e)),
                }
            }
        }
    }
    Err(could_not_locate(root))
}

/// Evaluate loaded text: `*file*` bound, the current namespace saved
/// and restored, and (for `load`, like `RT.load`) an isolating `*ns*`
/// frame so the file's `in-ns` never escapes.
fn run_loaded(
    eval: &mut Evaluator,
    file_star: Value,
    spans: &str,
    text: &str,
    isolate_ns: bool,
) -> Result<Vec<Value>, SourceError> {
    let cur = eval.registry.current().to_owned();
    let mut depth = None;
    if let Some(id) = core_var(eval, "*file*") {
        depth = Some(eval.push_binding(core_var_handle(id, "*file*"), file_star));
    }
    if isolate_ns {
        push_ns_frame(eval, &mut depth);
    }
    let out = eval.load_source(spans, text);
    eval.registry.set_current(&cur);
    if let Some(d) = depth {
        eval.pop_bindings(d);
    }
    out
}

/// Push the isolating `*ns*` frame for a load (recording the
/// depth when it is the first push).
fn push_ns_frame(eval: &mut Evaluator, depth: &mut Option<usize>) {
    if let Some(id) = core_var(eval, "*ns*") {
        let v = deref_core(eval, "*ns*");
        let d = eval.push_binding(core_var_handle(id, "*ns*"), v);
        if depth.is_none() {
            *depth = Some(d);
        }
    }
}

/// `load-one`: load a lib's root resource, check the namespace when
/// `need_ns`, and mark when `require_` (core.clj `load-one` — the
/// mark runs after the check, probe36 `req-mismatch-as`).
fn load_one(
    eval: &mut Evaluator,
    lib: &Symbol,
    need_ns: bool,
    require_: bool,
) -> Result<(), NativeError> {
    let root = root_resource(lib.name());
    load_paths(
        eval,
        std::slice::from_ref(&Value::Str(Arc::from(root.as_str()))),
    )?;
    if need_ns && eval.registry.get(&sym_str(lib)).is_none() {
        return Err(ex(format!(
            "namespace '{}' not found after loading '{}'",
            sym_str(lib),
            root
        )));
    }
    if require_ {
        mark_loaded(eval, lib)?;
    }
    Ok(())
}

/// `load-all`: `load-one` against a blank `*loaded-libs*`, unioning
/// whatever the load pulled in (core.clj `load-all` — the port's
/// embedded set is flat, so this behaves as `:reload`).
fn load_all(
    eval: &mut Evaluator,
    lib: &Symbol,
    need_ns: bool,
    require_: bool,
) -> Result<(), NativeError> {
    let save = libs_set(eval)?;
    let Some(id) = core_var(eval, "*loaded-libs*") else {
        return load_one(eval, lib, need_ns, require_);
    };
    let depth = eval.push_binding(core_var_handle(id, "*loaded-libs*"), Value::Set(new_set()));
    let out = load_one(eval, lib, need_ns, require_);
    let new = libs_set(eval);
    eval.pop_bindings(depth);
    out?;
    let mut acc = save;
    if let Ok(new) = new {
        for m in new.iter() {
            acc = acc.insert(m.clone());
        }
    }
    // The union lands in the caller's frame when a load-all
    // nests (an outer frame is still pushed), else the root.
    set_loaded_libs(eval, Value::Set(acc));
    Ok(())
}

/// How `load-lib` loads (or skips) a lib.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadKind {
    One,
    All,
    Stub,
}

/// `load-lib`: flags, the load-or-skip, and `:as`/`:refer`
/// application (core.clj `load-lib`, mirrored branch for branch).
/// A libspec's parsed options (core.clj `load-lib` destructuring).
struct LibOpts<'a> {
    as_: Option<&'a Value>,
    use_: bool,
    require_: bool,
    reload: bool,
    reload_all: bool,
    verbose: bool,
    as_alias: Option<&'a Value>,
    need_ns: bool,
}

fn lib_opts<'a>(pairs: &'a [(&'a Value, &'a Value)]) -> LibOpts<'a> {
    let truthy = |name: &str| opt_get(pairs, name).is_some_and(Value::is_truthy);
    let as_ = opt_get(pairs, "as");
    let use_ = truthy("use");
    LibOpts {
        as_,
        use_,
        require_: truthy("require"),
        reload: truthy("reload"),
        reload_all: truthy("reload-all"),
        verbose: truthy("verbose"),
        as_alias: opt_get(pairs, "as-alias"),
        need_ns: as_.is_some_and(Value::is_truthy) || use_,
    }
}

/// Which load (if any) a libspec selects (core.clj `load-lib` cond).
fn select_load(opts: &LibOpts, loaded: bool) -> Option<LoadKind> {
    if opts.reload_all {
        Some(LoadKind::All)
    } else if opts.reload {
        Some(LoadKind::One)
    } else if !loaded {
        if opts.need_ns {
            Some(LoadKind::One)
        } else if opts.as_alias.is_some_and(Value::is_truthy) {
            Some(LoadKind::Stub)
        } else {
            Some(LoadKind::One)
        }
    } else {
        None
    }
}

/// The `load-lib` load-or-skip phase: the verbose binding, the load
/// with its `Exception` cleanup (errors pass through — a failed load
/// unmakes a namespace it introduced), or the skip's `need-ns` check.
fn load_lib_phase(
    eval: &mut Evaluator,
    joined_sym: &Symbol,
    opts: &LibOpts,
    load: Option<LoadKind>,
    undefined_on_entry: bool,
) -> Result<(), NativeError> {
    // The verbose binding is observable; the echo itself waits for
    // `*out*` printing in S5D.
    let outer_verbose = deref_core(eval, "*loading-verbosely*").is_truthy() || opts.verbose;
    let depth = core_var(eval, "*loading-verbosely*").map(|id| {
        eval.push_binding(
            core_var_handle(id, "*loading-verbosely*"),
            Value::Bool(outer_verbose),
        )
    });
    let out = match load {
        Some(kind) => {
            let r = match kind {
                LoadKind::One => load_one(eval, joined_sym, opts.need_ns, opts.require_),
                LoadKind::All => load_all(eval, joined_sym, opts.need_ns, opts.require_),
                LoadKind::Stub => {
                    eval.registry.ensure_ns(&sym_str(joined_sym));
                    Ok(())
                }
            };
            match r {
                Err(e) if e.class.matches("java.lang.Exception") => {
                    if undefined_on_entry {
                        eval.registry.remove_ns(&sym_str(joined_sym));
                    }
                    Err(e)
                }
                other => other,
            }
        }
        None => {
            if opts.need_ns && eval.registry.get(&sym_str(joined_sym)).is_none() {
                Err(ex(format!("namespace '{}' not found", sym_str(joined_sym))))
            } else {
                Ok(())
            }
        }
    };
    if let Some(d) = depth {
        eval.pop_bindings(d);
    }
    out
}

/// The `load-lib` finish: `:as`/`:as-alias` aliases, then the
/// `:refer` application for `use` and explicit `:refer`.
fn load_lib_finish(
    eval: &mut Evaluator,
    joined_sym: &Symbol,
    pairs: &[(&Value, &Value)],
    opts: &LibOpts,
) -> Result<(), NativeError> {
    if let Some(a) = opts.as_.filter(|a| a.is_truthy()) {
        native_alias(eval, &[(*a).clone(), Value::Symbol(joined_sym.clone())])?;
    }
    if let Some(a) = opts.as_alias.filter(|a| a.is_truthy()) {
        native_alias(eval, &[(*a).clone(), Value::Symbol(joined_sym.clone())])?;
    }
    if opts.use_ || opt_get(pairs, "refer").is_some() {
        let mut filter = Vec::new();
        for name in ["exclude", "only", "rename", "refer"] {
            if let Some(v) = opt_get(pairs, name) {
                filter.push(Value::Keyword(Keyword::new(None, name)));
                filter.push(v.clone());
            }
        }
        refer_into(eval, &Value::Symbol(joined_sym.clone()), &filter)?;
    }
    Ok(())
}

fn load_lib(
    eval: &mut Evaluator,
    prefix: Option<String>,
    lib: &Value,
    options: &[Value],
) -> Result<(), NativeError> {
    // Eager `(name lib)`: `throw-if` is a function, so the format
    // args evaluate before the predicate (probe36 `req-num-lib`).
    let lib_name = name_of(lib)?;
    if let Some(p) = prefix.as_deref().filter(|_| lib_name.contains('.')) {
        return Err(ex(format!(
            "Found lib name '{lib_name}' containing period with prefix '{p}'.  lib names inside prefix lists must not contain periods"
        )));
    }
    let joined: Value = match &prefix {
        Some(p) => Value::Symbol(Symbol::intern(&format!("{p}.{}", str_of(lib)))),
        None => lib.clone(),
    };
    let pairs = opt_pairs(options)?;
    let opts = lib_opts(&pairs);
    let loaded = libs_set(eval)?.contains(&joined);
    let joined_sym = strict_sym(&joined)?.clone();
    let undefined_on_entry = eval.registry.get(&sym_str(&joined_sym)).is_none();
    let load = select_load(&opts, loaded);
    load_lib_phase(eval, &joined_sym, &opts, load, undefined_on_entry)?;
    load_lib_finish(eval, &joined_sym, &pairs, &opts)
}

/// A libspec: a symbol, or a vector whose second item is nil or a
/// keyword (core.clj `libspec?` — lists and strings take the prefix
/// path, probe36 `req-string`).
fn is_libspec(v: &Value) -> bool {
    match v {
        Value::Symbol(_) => true,
        Value::Vector(x) => matches!(
            x.iter().nth(1),
            None | Some(Value::Nil) | Some(Value::Keyword(_))
        ),
        _ => false,
    }
}

/// `prependss`: a symbol conses, anything else concats (seqing it —
/// the ISeq error fires here for scalars, probe36 `req-string`).
fn expand_libspec(v: &Value, flag_opts: &[Value]) -> Result<Vec<Value>, NativeError> {
    let mut out = match v {
        Value::Symbol(_) => vec![v.clone()],
        _ => seq_vec(v).ok_or_else(|| iseq_err(v))?,
    };
    out.extend(flag_opts.iter().cloned());
    Ok(out)
}

/// `load-libs`: flags, libspecs, and prefix lists (core.clj
/// `load-libs`; `require` injects `:require`, `use` `:require :use`).
/// Split `require`/`use` args into top-level flags and libspecs,
/// rejecting unknown flags and empty arg lists (core.clj
/// `load-libs`).
fn split_flags(args: &[Value]) -> Result<(Vec<&Value>, Vec<&Value>), NativeError> {
    let mut flags: Vec<&Value> = Vec::new();
    let mut rest: Vec<&Value> = Vec::new();
    for a in args {
        if matches!(a, Value::Keyword(_)) {
            flags.push(a);
        } else {
            rest.push(a);
        }
    }
    const SUPPORTED: &[&str] = &[
        "as",
        "reload",
        "reload-all",
        "require",
        "use",
        "verbose",
        "refer",
        "as-alias",
    ];
    let mut unsupported = Vec::new();
    for f in &flags {
        if let Value::Keyword(kw) = f {
            let ok = kw.namespace().is_none() && SUPPORTED.contains(&kw.name());
            if !ok {
                unsupported.push(str_of(f));
            }
        }
    }
    if !unsupported.is_empty() {
        return Err(ex(format!(
            "Unsupported option(s) supplied: {}",
            unsupported.join(",")
        )));
    }
    if rest.is_empty() {
        return Err(ex("Nothing specified to load".to_owned()));
    }
    Ok((flags, rest))
}

/// Load one expanded libspec (`(apply load-lib prefix ...)`).
fn load_expanded(
    eval: &mut Evaluator,
    prefix: Option<String>,
    expanded: &[Value],
) -> Result<(), NativeError> {
    let mut expanded = expanded.to_vec();
    if expanded.is_empty() {
        expanded.push(Value::Nil);
    }
    let lib = expanded[0].clone();
    load_lib(eval, prefix, &lib, &expanded[1..])
}

/// Load one prefix-list arg: the first item is the prefix, the rest
/// are libspecs joined under it.
fn load_prefix_arg(
    eval: &mut Evaluator,
    arg: &Value,
    flag_opts: &[Value],
) -> Result<(), NativeError> {
    let items = seq_vec(arg).ok_or_else(|| iseq_err(arg))?;
    let mut it = items.into_iter();
    let prefix = match it.next() {
        None | Some(Value::Nil) => {
            return Err(ex("prefix cannot be nil".to_owned()));
        }
        Some(p) => str_of(&p),
    };
    for sub in it {
        let expanded = expand_libspec(&sub, flag_opts)?;
        load_expanded(eval, Some(prefix.clone()), &expanded)?;
    }
    Ok(())
}

fn load_libs(
    eval: &mut Evaluator,
    injected: &[&str],
    args: &[Value],
) -> Result<Value, NativeError> {
    let (flags, rest) = split_flags(args)?;
    let mut flag_opts: Vec<Value> = Vec::new();
    for f in &flags {
        flag_opts.push((*f).clone());
        flag_opts.push(Value::Bool(true));
    }
    for name in injected {
        flag_opts.push(Value::Keyword(Keyword::new(None, name)));
        flag_opts.push(Value::Bool(true));
    }
    for arg in rest {
        if is_libspec(arg) {
            let expanded = expand_libspec(arg, &flag_opts)?;
            load_expanded(eval, None, &expanded)?;
        } else {
            load_prefix_arg(eval, arg, &flag_opts)?;
        }
    }
    Ok(Value::Nil)
}

fn native_require(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    load_libs(eval, &["require"], args)
}

/// Evaluate an `(ns …)` form: `in-ns`, unconditional `resetMeta`
/// (the attrmap wins over the docstring for `:doc`; no attrmap
/// clears), default-or-clause `refer-clojure`, each
/// `:require`/`:use` clause in order through the loader (failures
/// surface raw), then mark the lib loaded (probe37 `ns-*`).
/// `with-loading-context` needs no analog: the loader already
/// scopes `*loaded-libs*` per load.
pub(crate) fn eval_ns_form(
    eval: &mut Evaluator,
    name: &str,
    doc: Option<&str>,
    attr: Option<&Map>,
    clauses: &[NsClause],
) -> Result<Value, Signal> {
    let name_arg = Value::Symbol(Symbol::new(None, name));
    native_in_ns(eval, &[name_arg]).map_err(|e| eval.throw(e))?;
    let meta = ns_form_meta(doc, attr);
    eval.registry.with_ns_mut(name, |ns| {
        ns.set_meta(meta);
    });
    run_ns_clauses(eval, clauses)?;
    let lib = Symbol::new(None, name);
    mark_loaded(eval, &lib).map_err(|e| eval.throw(e))?;
    Ok(Value::Nil)
}

/// An `(ns …)` form's meta: the attrmap (winning over the
/// docstring for `:doc`), the docstring alone, or nothing (which
/// clears, like the unconditional `resetMeta`).
fn ns_form_meta(doc: Option<&str>, attr: Option<&Map>) -> Option<Map> {
    let doc_key = Value::Keyword(Keyword::new(None, "doc"));
    match attr {
        None => doc.map(|d| new_map().insert(doc_key, Value::Str(Arc::from(d)))),
        Some(a) => {
            let mut m = a.clone();
            if let Some(d) = doc.filter(|_| m.get(&doc_key).is_none()) {
                m = m.insert(doc_key, Value::Str(Arc::from(d)));
            }
            Some(m)
        }
    }
}

/// An `(ns …)` form's clauses: the default `refer-clojure` unless
/// a clause overrides it, then each `:require`/`:use` clause in
/// order through the loader (failures surface raw).
fn run_ns_clauses(eval: &mut Evaluator, clauses: &[NsClause]) -> Result<(), Signal> {
    let has_refer_clojure = clauses
        .iter()
        .any(|c| matches!(c, NsClause::ReferClojure(_)));
    if !has_refer_clojure {
        native_refer_clojure(eval, &[]).map_err(|e| eval.throw(e))?;
    }
    for clause in clauses {
        match clause {
            NsClause::ReferClojure(filters) => {
                native_refer_clojure(eval, filters).map_err(|e| eval.throw(e))?;
            }
            NsClause::Require(libspecs) => {
                native_require(eval, libspecs).map_err(|e| eval.throw(e))?;
            }
            NsClause::Use(libspecs) => {
                native_use(eval, libspecs).map_err(|e| eval.throw(e))?;
            }
        }
    }
    Ok(())
}

fn native_use(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    load_libs(eval, &["require", "use"], args)
}

/// `load` over paths (shared by the `load` native and `load-one`):
/// root resolution, the cycle guard, and the isolated file eval.
/// Resolve one `load` path: a leading slash is absolute, otherwise
/// relative to the current namespace's root directory (core.clj
/// `load`). Returns the full path and the visible pending stack.
fn resolve_load_path(eval: &Evaluator, p: &Value) -> Result<(String, Vec<Value>), NativeError> {
    let path = match p {
        Value::Str(s) => s.to_string(),
        _ => return Err(cast_err(p, "java.lang.String")),
    };
    let full = if path.starts_with('/') {
        path
    } else {
        let starred = deref_core(eval, "*ns*");
        let ns_name = as_ns(&starred).map_or_else(
            || eval.registry.current().to_owned(),
            |h| h.name.to_string(),
        );
        format!("{}/{}", root_directory(&ns_name), path)
    };
    match deref_core(eval, "*pending-paths*") {
        Value::List(l) => Ok((full, l.iter().cloned().collect())),
        v => Err(cast_err(&v, "clojure.lang.IPersistentList")),
    }
}

/// The `*pending-paths*` cycle guard: a repeat below the top errors
/// (`check-cyclic-dependency`), the top itself is exempt (self-load),
/// anything else proceeds.
fn cycle_action(pending: &[Value], full: &str) -> Result<bool, NativeError> {
    let is_full = |x: &Value| matches!(x, Value::Str(s) if s.as_ref() == full);
    if pending.iter().skip(1).any(is_full) {
        let mut chain = vec![full.to_owned()];
        chain.extend(pending.iter().map(str_of));
        let marked: Vec<String> = chain
            .iter()
            .map(|x| {
                if x == full {
                    format!("[ {x} ]")
                } else {
                    x.clone()
                }
            })
            .collect();
        return Err(ex(format!("Cyclic load dependency: {}", marked.join("->"))));
    }
    Ok(!pending.first().is_some_and(is_full))
}

/// Load, bind, and evaluate one resolved path.
fn load_one_path(eval: &mut Evaluator, full: &str) -> Result<(), NativeError> {
    let root = full.strip_prefix('/').unwrap_or(full).to_owned();
    let (file_star, text) = resolve_source(eval, &root)?;
    let pending_depth = match core_var(eval, "*pending-paths*") {
        Some(id) => match deref_core(eval, "*pending-paths*") {
            Value::List(l) => Some(eval.push_binding(
                core_var_handle(id, "*pending-paths*"),
                Value::List(l.push_front(Value::Str(Arc::from(full)))),
            )),
            v => return Err(cast_err(&v, "clojure.lang.IPersistentList")),
        },
        None => None,
    };
    let out = run_loaded(
        eval,
        Value::Str(Arc::from(file_star.as_str())),
        &file_star,
        &text,
        true,
    );
    if let Some(d) = pending_depth {
        eval.pop_bindings(d);
    }
    match out {
        Ok(_) => Ok(()),
        Err(e) => Err(source_to_native(eval, &file_star, e)),
    }
}

fn load_paths(eval: &mut Evaluator, paths: &[Value]) -> Result<(), NativeError> {
    for p in paths {
        let (full, pending) = resolve_load_path(eval, p)?;
        if cycle_action(&pending, &full)? {
            load_one_path(eval, &full)?;
        }
    }
    Ok(())
}

fn native_load(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    load_paths(eval, args)?;
    Ok(Value::Nil)
}

/// The `load-file` `*file*`: the path absolutized, `..` preserved
/// (`java.io.File.getAbsolutePath`, probe36 `file-dotdot`).
fn absolutize(path: &str) -> String {
    if std::path::Path::new(path).is_absolute() {
        return path.to_owned();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path).to_string_lossy().into_owned(),
        Err(_) => path.to_owned(),
    }
}

fn native_load_file(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/load-file", args, &[1])?;
    let path = match &args[0] {
        Value::Str(s) => s.to_string(),
        v => return Err(cast_err(v, "java.lang.String")),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return Err(fs_err(&path, &e)),
    };
    let abs = absolutize(&path);
    // `Compiler.loadFile`: no `*ns*` isolation, no cycle guard —
    // only `*file*`.
    match run_loaded(
        eval,
        Value::Str(Arc::from(abs.as_str())),
        &abs,
        &text,
        false,
    ) {
        Ok(_) => Ok(Value::Nil),
        Err(e) => Err(source_to_native(eval, &abs, e)),
    }
}

fn native_load_string(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/load-string", args, &[1])?;
    let text = match &args[0] {
        Value::Str(s) => s.to_string(),
        v => return Err(cast_err(v, "java.lang.String")),
    };
    // `*file*` is nil and spans blame the empty source (probe36
    // `file-loadstring`, `loadstring-errspan`).
    match run_loaded(eval, Value::Nil, "", &text, false) {
        Ok(_) => Ok(Value::Nil),
        Err(e) => Err(source_to_native(eval, "", e)),
    }
}

fn native_loaded_libs(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/loaded-libs", args, &[0])?;
    Ok(Value::Set(libs_set(eval)?))
}

fn native_eval(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/eval", args, &[1])?;
    let source = eval.source().to_owned();
    match eval.eval_top_form(&args[0]) {
        Ok(v) => Ok(v),
        Err(e) => Err(source_to_native(eval, &source, e)),
    }
}

/// A native fn pointer (all §N natives share the signature).
pub(crate) type NativeFnPtr = fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError>;

/// The §N natives installed into `clojure.core` by [`boot`].
const NATIVES: &[(&str, NativeFnPtr)] = &[
    ("create-ns", native_create_ns),
    ("in-ns", native_in_ns),
    ("ns-name", native_ns_name),
    ("the-ns", native_the_ns),
    ("find-ns", native_find_ns),
    ("remove-ns", native_remove_ns),
    ("all-ns", native_all_ns),
    ("ns-aliases", native_ns_aliases),
    ("ns-map", native_ns_map),
    ("ns-interns", native_ns_interns),
    ("ns-publics", native_ns_publics),
    ("ns-refers", native_ns_refers),
    ("ns-imports", native_ns_imports),
    ("alias", native_alias),
    ("refer", native_refer),
    ("refer-clojure", native_refer_clojure),
    ("intern", native_intern),
    ("ns-resolve", native_ns_resolve),
    ("resolve", native_resolve),
    ("ns-unmap", native_ns_unmap),
    ("ns-unalias", native_ns_unalias),
    ("require", native_require),
    ("use", native_use),
    ("load", native_load),
    ("load-string", native_load_string),
    ("load-file", native_load_file),
    ("loaded-libs", native_loaded_libs),
    ("eval", native_eval),
];

/// The `clojure.core` vars installed by [`boot`]: name, root,
/// dynamic, private.
fn core_vars(user_ns: Value) -> Vec<(String, Value, bool, bool)> {
    vec![
        ("*ns*".to_owned(), user_ns, true, false),
        (
            "*file*".to_owned(),
            Value::Str(Arc::from("NO_SOURCE_PATH")),
            true,
            false,
        ),
        (
            "*loaded-libs*".to_owned(),
            Value::Set(new_set()),
            true,
            true,
        ),
        (
            "*pending-paths*".to_owned(),
            Value::List(List::from_vec(Vec::new())),
            true,
            true,
        ),
        (
            "*loading-verbosely*".to_owned(),
            Value::Bool(false),
            true,
            true,
        ),
        (
            "*load-path*".to_owned(),
            Value::Vector(Vector::from_vec(Vec::new())),
            true,
            false,
        ),
        // `core.clj` `^:dynamic` vars: slot AND `:dynamic` meta
        // (corpus 11 `:print-vars-dynamic` pins all three `true`).
        ("*print-length*".to_owned(), Value::Nil, true, false),
        ("*print-level*".to_owned(), Value::Nil, true, false),
        (
            "*print-namespace-maps*".to_owned(),
            Value::Bool(true),
            true,
            false,
        ),
    ]
}

/// Install one `clojure.core` var with the dynamic SLOT set but no
/// `:dynamic` meta — the JVM `RT.java` interns (`Var.intern(...).
/// setDynamic()`): `*assert*`, the `*unchecked-math*` /
/// `*warn-on-reflection*` / `*allow-unresolved-vars*` flags, `*out*` /
/// `*err*`, and the `*print-meta*` / `*print-readably*` /
/// `*print-dup*` triad (corpus 11/15 pin the meta `false` while
/// `binding` still works).
fn def_dynamic_root(eval: &mut Evaluator, name: &str, root: Value) {
    let q = Symbol::new(Some("clojure.core"), name);
    let id = eval.registry.intern(&q);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(root);
        st.dynamic = true;
    });
}

/// Install one `clojure.core` var (private vars carry
/// `^:private` meta, core.clj `defonce`).
fn def_var(eval: &mut Evaluator, name: &str, root: Value, dynamic: bool, private: bool) {
    let q = Symbol::new(Some("clojure.core"), name);
    let id = eval.registry.intern(&q);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(root);
        st.dynamic = dynamic;
        // `intern` stamped `:ns`/`:name`; boot adds `:dynamic`
        // (JVM `def^:dynamic` keeps it in the meta) and `:private`.
        let mut meta = st.meta.clone().unwrap_or_else(new_map);
        if dynamic {
            meta = meta.insert(
                Value::Keyword(Keyword::new(None, "dynamic")),
                Value::Bool(true),
            );
        }
        if private {
            meta = meta.insert(
                Value::Keyword(Keyword::new(None, "private")),
                Value::Bool(true),
            );
        }
        st.meta = Some(meta);
    });
}

/// Evaluate one boot form: the embedded boot sequence is fixed, so
/// any failure is a port bug (pinned by `core_loads`), never user
/// input.
fn boot_eval(eval: &mut Evaluator, src: &str) {
    if let Err(e) = eval.eval_source(src) {
        unreachable!("boot form failed ({src}): {e:?}");
    }
}

/// Boot a fresh evaluator: `clojure.core` plus `user` (current),
/// the §N natives and vars installed, every public install referred
/// into `user` (JVM boot refers `clojure.core`), a permanent `*ns*`
/// base frame (so `in-ns` writes the top frame and the root never
/// moves, like `RT.inNamespace` under `clojure.main`), and the
/// embedded `core.clj` composites loaded into `clojure.core` with
/// their publics referred into `user`.
#[must_use]
pub fn boot() -> Evaluator {
    let mut eval = Evaluator::new_bare("user");
    eval.registry.ensure_ns("clojure.core");
    let user_ns = eval.registry.ensure_ns("user").id();
    eval.registry.set_current("user");
    boot_install_natives(&mut eval);
    boot_install_builtin_macros(&mut eval);
    boot_mark_private(&mut eval);
    let user_handle = ns_value("user", user_ns);
    boot_core_vars(&mut eval, user_handle.clone());
    boot_refer_user(&mut eval);
    if let Some(id) = core_var(&eval, "*ns*") {
        eval.push_binding(core_var_handle(id, "*ns*"), user_handle);
    }
    // Embedded `core.clj` composites (their privates resolve in
    // `clojure.core`), then `user` refers the publics — the file
    // carries no `ns` form by design.
    boot_eval(&mut eval, "(in-ns 'clojure.core)");
    boot_eval(&mut eval, CORE_CLJ);
    boot_eval(&mut eval, "(in-ns 'user)");
    boot_eval(&mut eval, "(refer 'clojure.core)");
    // The load's macroexpansions consume `gensym` numbers; the
    // user-visible counter restarts from 1 (S5C pins `G__1` first).
    eval.reset_gensym();
    eval
}

/// Install the §N natives into `clojure.core`.
fn boot_install_natives(eval: &mut Evaluator) {
    for (name, f) in NATIVES
        .iter()
        .chain(crate::vars::VAR_NATIVES.iter())
        .chain(crate::macros::MACRO_NATIVES.iter())
        .chain(crate::lazy::LAZY_NATIVES.iter())
        .chain(crate::core::CORE_NATIVES.iter())
        .chain(crate::seqs::SEQS_NATIVES.iter())
        .chain(crate::coll::COLL_NATIVES.iter())
        .chain(crate::io::IO_NATIVES.iter())
        .chain(crate::concurrency::CONC_NATIVES.iter())
    {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native("clojure.core", name, func);
    }
}

/// Builtin-macro vars (`let`, `defmacro`, `declare`, `ns`, …):
/// the analyzer intercepts the forms, so the roots are
/// placeholders that fail LOUDLY on direct invocation.
fn boot_install_builtin_macros(eval: &mut Evaluator) {
    for name in crate::macros::BUILTIN_MACROS {
        let display = format!("clojure.core/{name}");
        eval.install_native(
            "clojure.core",
            name,
            Arc::new(move |_, _| {
                Err(NativeError::new(
                    ThrowClass::IllegalStateException,
                    format!("Can't invoke builtin macro: {display}"),
                ))
            }),
        );
        let q = Symbol::new(Some("clojure.core"), name);
        let id = eval.registry.intern(&q);
        eval.registry.with_state_mut(id, |st| {
            let meta = st.meta.clone().unwrap_or_else(new_map).insert(
                Value::Keyword(Keyword::new(None, "macro")),
                Value::Bool(true),
            );
            st.meta = Some(meta);
        });
    }
}

/// Port-internal natives (macro-expansion errors, `case` dispatch
/// hashing): usable but never referred into `user` (the refers
/// filter on `is_public`).
fn boot_mark_private(eval: &mut Evaluator) {
    for name in ["throw-illegal-arg!", "throw-npe!", "hash-code"] {
        let q = Symbol::new(Some("clojure.core"), name);
        let id = eval.registry.intern(&q);
        eval.registry.with_state_mut(id, |st| {
            let meta = st.meta.clone().unwrap_or_else(new_map).insert(
                Value::Keyword(Keyword::new(None, "private")),
                Value::Bool(true),
            );
            st.meta = Some(meta);
        });
    }
}

/// Install the `clojure.core` vars and the `RT.java` dynamics (slot
/// set, no `:dynamic` meta).
fn boot_core_vars(eval: &mut Evaluator, user_handle: Value) {
    for (name, root, dynamic, private) in core_vars(user_handle) {
        def_var(eval, &name, root, dynamic, private);
    }
    let stdout = crate::io::stdout_writer(eval);
    let stderr = crate::io::stderr_writer(eval);
    def_dynamic_root(eval, "*out*", stdout);
    def_dynamic_root(eval, "*err*", stderr);
    def_dynamic_root(eval, "*assert*", Value::Bool(true));
    def_dynamic_root(eval, "*unchecked-math*", Value::Bool(false));
    def_dynamic_root(eval, "*warn-on-reflection*", Value::Bool(false));
    def_dynamic_root(eval, "*allow-unresolved-vars*", Value::Bool(false));
    def_dynamic_root(eval, "*print-meta*", Value::Bool(false));
    def_dynamic_root(eval, "*print-readably*", Value::Bool(true));
    def_dynamic_root(eval, "*print-dup*", Value::Bool(false));
    // `RT.java`: `*agent*` is a dynamic slot with a nil root (bound
    // to the agent inside each action by `send`).
    def_dynamic_root(eval, "*agent*", Value::Nil);
}

/// Refer every public install into `user` (JVM boot refers
/// `clojure.core`).
fn boot_refer_user(eval: &mut Evaluator) {
    let mut names: Vec<String> = NATIVES
        .iter()
        .chain(crate::vars::VAR_NATIVES.iter())
        .chain(crate::macros::MACRO_NATIVES.iter())
        .chain(crate::lazy::LAZY_NATIVES.iter())
        .chain(crate::core::CORE_NATIVES.iter())
        .chain(crate::seqs::SEQS_NATIVES.iter())
        .chain(crate::coll::COLL_NATIVES.iter())
        .chain(crate::io::IO_NATIVES.iter())
        .chain(crate::concurrency::CONC_NATIVES.iter())
        .map(|(n, _)| (*n).to_owned())
        .collect();
    names.extend(
        crate::macros::BUILTIN_MACROS
            .iter()
            .map(|s| (*s).to_owned()),
    );
    names.extend(
        [
            "*ns*",
            "*file*",
            "*loaded-libs*",
            "*pending-paths*",
            "*loading-verbosely*",
            "*load-path*",
            "*out*",
            "*err*",
            "*assert*",
            "*unchecked-math*",
            "*warn-on-reflection*",
            "*allow-unresolved-vars*",
            "*print-meta*",
            "*print-readably*",
            "*print-dup*",
            "*agent*",
            "*print-length*",
            "*print-level*",
            "*print-namespace-maps*",
        ]
        .iter()
        .map(|s| (*s).to_owned()),
    );
    let mut refers: Vec<(String, Symbol, u64)> = Vec::new();
    for name in &names {
        let q = Symbol::new(Some("clojure.core"), name);
        if let Some(id) = id_of(&eval.registry, &q).filter(|id| is_public(&eval.registry, *id)) {
            refers.push((name.clone(), q, id));
        }
    }
    eval.registry.with_ns_mut("user", |user| {
        for (name, q, id) in refers {
            user.add_refer(&name, q, id);
        }
    });
}
