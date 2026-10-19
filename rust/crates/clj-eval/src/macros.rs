// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5C macroexpansion (`macroexpand-1`, `macroexpand`, `gensym`)
//! plus the early list-building natives syntax-quote expansions
//! call (`seq`, `concat`, `list`, `apply`, `vector`, `hash-map`,
//! `hash-set` — S5D rows implemented eagerly here; S5D makes
//! `seq`/`concat` lazy per C4). Oracle: probe38; rules:
//! `docs/t1174/macros-contract.md`. The wrap partition is
//! source-read from `Compiler.macroexpand1`.

use std::sync::Arc;

use clj_reader::Span;
use clj_value::{Keyword, List, Symbol, ThrowClass, Value, Vector, new_map, new_set};

use crate::error::{CompileError, NativeError, Signal, Verb, jvm_class_of};
use crate::eval::Evaluator;
use crate::expr::is_special;
use crate::ns::{NativeFnPtr, check_arity, signal_to_native, str_of, throw_class_for};

/// Builtins whose vars resolve as macros (the analyzer
/// intercepts every form, so the roots only matter to `resolve`;
/// `boot` installs the vars from this table).
pub(crate) const BUILTIN_MACROS: &[&str] = &[
    "let",
    "loop",
    "fn",
    "binding",
    "letfn",
    "defmacro",
    "declare",
    "ns",
    "with-redefs",
    "with-bindings",
    "bound-fn",
    "assert",
    "time",
    "with-out-str",
];

/// A resolved macro's current root (the call runs it with
/// `(form, env, args...)`).
pub(crate) struct MacroRef {
    pub root: Value,
}

/// Whether a var id is a macro (`:macro` meta truthy).
pub(crate) fn is_macro_var(eval: &Evaluator, id: u64) -> bool {
    let probe = Value::Keyword(Keyword::new(None, "macro"));
    eval.registry
        .state(id)
        .and_then(|s| s.meta.as_ref()?.get(&probe))
        .is_some_and(Value::is_truthy)
}

/// Whether a var id is private (`:private` meta truthy; cleared
/// metas read public).
pub(crate) fn is_private_var(eval: &Evaluator, id: u64) -> bool {
    let probe = Value::Keyword(Keyword::new(None, "private"));
    eval.registry
        .state(id)
        .and_then(|s| s.meta.as_ref()?.get(&probe))
        .is_some_and(Value::is_truthy)
}

/// Whether a symbol names a builtin-macro var.
fn is_builtin_sym(sym: &Symbol) -> bool {
    sym.namespace() == Some("clojure.core") && BUILTIN_MACROS.contains(&sym.name())
}

/// Heads `macroexpand-1` leaves alone: specials (bare or
/// `clojure.core`-qualified) and builtin-macro vars (the
/// documented identity fence — the JVM expands the real
/// `let`/`declare`/`ns`/… macros; the port's placeholder roots
/// cannot run, so user-macro expansions headed by a builtin
/// stop there instead of failing).
fn is_identity_head(eval: &Evaluator, op: &Value) -> bool {
    match op {
        Value::Symbol(sym) => {
            if sym.namespace().is_none_or(|ns| ns == "clojure.core") && is_special(sym.name()) {
                return true;
            }
            let cur = eval.current_ns_name();
            eval.registry
                .resolve_in(&cur, sym)
                .is_some_and(|target| is_builtin_sym(&target))
        }
        Value::Var(v) => is_builtin_sym(&v.sym),
        _ => false,
    }
}

/// `isMacro` (`Compiler.java`): resolve the operator — a symbol
/// via the synced current namespace, or a direct var value — to a
/// macro var. Private cross-namespace macros are the bare
/// `IllegalStateException` (callers wrap per position); anything
/// unresolvable or non-macro is `Ok(None)`.
pub(crate) fn lookup_macro(eval: &Evaluator, op: &Value) -> Result<Option<MacroRef>, NativeError> {
    let (id, target) = match op {
        Value::Symbol(sym) => {
            let cur = eval.current_ns_name();
            let Some(target) = eval.registry.resolve_in(&cur, sym) else {
                return Ok(None);
            };
            let home = target.namespace().unwrap_or(&cur);
            let Some(id) = eval.registry.get(home).and_then(|ns| ns.lookup(&target)) else {
                return Ok(None);
            };
            (id, target)
        }
        Value::Var(v) => (v.id, v.sym.clone()),
        _ => return Ok(None),
    };
    if !is_macro_var(eval, id) {
        return Ok(None);
    }
    let home = target.namespace().unwrap_or("");
    if home != eval.current_ns_name() && is_private_var(eval, id) {
        return Err(NativeError::new(
            ThrowClass::IllegalStateException,
            format!("var: #'{} is not public", target),
        ));
    }
    let root = eval.registry.root_or_unbound(id, &target);
    Ok(Some(MacroRef { root }))
}

/// Display name of a callable root (for the arity-strip check).
fn root_display(root: &Value) -> Option<&str> {
    match root {
        Value::Extension(e) => {
            if let Some(c) = e.as_any().downcast_ref::<crate::ext::Closure>() {
                return Some(&c.display);
            }
            if let Some(n) = e.as_any().downcast_ref::<crate::ext::NativeFn>() {
                return Some(&n.display);
            }
            None
        }
        _ => None,
    }
}

/// Wrap an expansion failure per `Compiler.macroexpand1`: the
/// macro's *own* `ArityException` (name matches the root display)
/// rethrows bare with the 2 hidden `&form`/`&env` params stripped;
/// any other arity error rethrows bare as-is; `IAE` (including
/// `NumberFormatException`), `ISE`, `ExceptionInfo`, and exactly
/// `Exception` wrap as `Syntax error`; everything else wraps as
/// `Unexpected error`. The wrapper names the operator as written
/// (absent for `Var` operators).
fn wrap_expansion(
    t: &clj_value::Throwable,
    op: &Value,
    root: &Value,
    call_argc: usize,
    span: Span,
    source: &Arc<str>,
) -> CompileError {
    let sym = match op {
        Value::Symbol(s) => Some(match s.namespace() {
            Some(ns) => format!("{ns}/{}", s.name()),
            None => s.name().to_owned(),
        }),
        _ => None,
    };
    if t.class == ThrowClass::ArityException {
        // The failing name rides after `passed to: ` (displays
        // never contain the marker).
        let failing = t
            .message
            .as_deref()
            .and_then(|m| m.rsplit("passed to: ").next());
        let own = failing.is_some_and(|name| Some(name) == root_display(root));
        if !own {
            // Not ours (an inner call failed): the original error
            // escapes bare and untouched (probed Q1: `(max)` of
            // nothing inside `case` keeps its `(0)` count).
            return CompileError {
                class: "clojure.lang.ArityException",
                message: t.message.clone().unwrap_or_default().to_string(),
                span,
                source: Arc::clone(source),
                verb: Verb::Compiling,
                sym: None,
                unexpected: false,
                bare: true,
            };
        }
        // The macro received `(form env & user-args)`: strip the 2
        // hidden params from OUR count (exact, no message parse).
        let actual = call_argc.saturating_sub(2);
        let name = failing.unwrap_or("clojure.core/fn");
        return CompileError {
            class: "clojure.lang.ArityException",
            message: NativeError::arity(name, actual).into_inner().message,
            span,
            source: Arc::clone(source),
            verb: Verb::Compiling,
            sym: None,
            unexpected: false,
            bare: true,
        };
    }
    // `Exception` exactly (not subclasses) joins the `Syntax` side.
    let syntax = matches!(
        t.class,
        ThrowClass::IllegalArgumentException
            | ThrowClass::NumberFormatException
            | ThrowClass::IllegalStateException
            | ThrowClass::ExceptionInfo
            | ThrowClass::Exception
    );
    CompileError {
        class: t.class.jvm_name(),
        message: t.message.as_deref().unwrap_or("").to_owned(),
        span,
        source: Arc::clone(source),
        verb: Verb::Macroexpanding,
        sym,
        unexpected: !syntax,
        bare: false,
    }
}

/// Expand one macro step: run the root with `(form, env,
/// args...)`. `env` is the analyzer's local map (or nil at top);
/// natives pass nil. The wrap blames the evaluator's current
/// source (every caller passed that same source).
pub(crate) fn expand1(
    eval: &mut Evaluator,
    op: &Value,
    root: &Value,
    form: &Value,
    env: &Value,
    args: &[Value],
    span: Span,
) -> Result<Value, CompileError> {
    let mut call_args = Vec::with_capacity(args.len() + 2);
    call_args.push(form.clone());
    call_args.push(env.clone());
    call_args.extend(args.iter().cloned());
    let argc = call_args.len();
    let source = Arc::from(eval.source());
    match eval.call(root, &call_args) {
        Ok(v) => Ok(v),
        Err(Signal::Throw(t)) => Err(wrap_expansion(&t, op, root, argc, span, &source)),
        Err(Signal::Recur(_)) => unreachable!("recur cannot escape a macro call"),
    }
}

/// One `macroexpand-1` step over any form (natives and
/// `eval_top_form` share this): non-seqs, `()`, special/builtin
/// operators (bare or `clojure.core`-qualified), unresolvable
/// operators, and non-macro `Var` operators return unchanged;
/// macros expand with a nil `&env`.
pub(crate) fn macroexpand1_value(
    eval: &mut Evaluator,
    form: &Value,
    span: Span,
    source: &Arc<str>,
) -> Result<Value, CompileError> {
    let Value::List(l) = form else {
        return Ok(form.clone());
    };
    let Some(op) = l.first() else {
        return Ok(form.clone());
    };
    if is_identity_head(eval, op) {
        return Ok(form.clone());
    }
    let keen = match lookup_macro(eval, op) {
        Err(private) => {
            let inner = private.into_inner();
            return Err(CompileError {
                class: inner.class.jvm_name(),
                message: inner.message,
                span,
                source: Arc::clone(source),
                verb: Verb::Compiling,
                sym: None,
                unexpected: false,
                bare: true,
            });
        }
        Ok(keen) => keen,
    };
    let Some(mac) = keen else {
        return Ok(form.clone());
    };
    let args: Vec<Value> = l.iter().skip(1).cloned().collect();
    expand1(eval, op, &mac.root, form, &Value::Nil, &args, span)
}

/// `Compiler.macroexpand`: loop `macroexpand-1` while the head is
/// a macro (equivalent to the JVM's identity loop in every
/// terminating case, hanging in the same self-similar cases).
pub(crate) fn macroexpand_full(
    eval: &mut Evaluator,
    form: &Value,
    span: Span,
    source: &Arc<str>,
) -> Result<Value, CompileError> {
    let mut current = form.clone();
    loop {
        let expanded = macroexpand1_value(eval, &current, span, source)?;
        let headed = match &expanded {
            Value::List(l) => match l.first() {
                None => false,
                Some(op) => {
                    if is_identity_head(eval, op) {
                        return Ok(expanded);
                    }
                    match lookup_macro(eval, op) {
                        Err(_) => return macroexpand1_value(eval, &expanded, span, source),
                        Ok(keen) => keen.is_some(),
                    }
                }
            },
            _ => false,
        };
        if !headed {
            return Ok(expanded);
        }
        current = expanded;
    }
}

/// A `CompileError` as a native error (bare causes stay bare;
/// wrapped chains render `wrapper\ncause`, like `eval`).
fn expansion_to_native(e: CompileError) -> NativeError {
    if e.bare {
        NativeError::new(throw_class_for(e.class), e.message)
    } else {
        NativeError::new(throw_class_for(e.class), e.to_string())
    }
}

fn native_macroexpand_1(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/macroexpand-1", args, &[1])?;
    // The wrap blames the call site (probed: a nested call
    // reports the call's position), never the form's own meta.
    let span = eval.native_call_span().unwrap_or(Span::new(1, 1));
    let source = Arc::from(eval.source());
    macroexpand1_value(eval, &args[0], span, &source).map_err(expansion_to_native)
}

fn native_macroexpand(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/macroexpand", args, &[1])?;
    let span = eval.native_call_span().unwrap_or(Span::new(1, 1));
    let source = Arc::from(eval.source());
    macroexpand_full(eval, &args[0], span, &source).map_err(expansion_to_native)
}

fn native_gensym(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/gensym", args, &[0, 1])?;
    let prefix = args.first().map_or_else(|| "G__".to_owned(), str_of);
    let n = eval.next_gensym();
    Ok(Value::Symbol(Symbol::new(None, &format!("{prefix}{n}"))))
}

// ---------------------------------------------------------------------------
// Early list natives (S5D rows, eager; S5D adds laziness per C4).
// ---------------------------------------------------------------------------

/// `seq` a value into items (`None` = nil/empty): lists, vectors,
/// maps (as `[k v]` vectors — the JVM yields `MapEntry`s, fence),
/// sets, and strings (as chars); anything else is the `ISeq`
/// error.
/// A map entry as a `[k v]` vector (`seq` on maps).
fn map_entry(k: &Value, val: &Value) -> Value {
    Value::Vector(Vector::from_vec(vec![k.clone(), val.clone()]))
}

fn seq_items(v: &Value) -> Result<Option<Vec<Value>>, NativeError> {
    let items: Vec<Value> = match v {
        Value::Nil => return Ok(None),
        Value::List(l) => l.iter().cloned().collect(),
        Value::Vector(x) => x.iter().cloned().collect(),
        Value::Map(m) => m.iter().map(|(k, val)| map_entry(k, val)).collect(),
        Value::Set(s) => s.iter().cloned().collect(),
        Value::Str(s) => s.chars().map(Value::Char).collect(),
        _ => {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!("Don't know how to create ISeq from: {}", jvm_class_of(v)),
            ));
        }
    };
    Ok((!items.is_empty()).then_some(items))
}

fn native_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/seq", args, &[1])?;
    Ok(seq_items(&args[0])?.map_or(Value::Nil, |items| Value::List(List::from_vec(items))))
}

fn native_concat(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let mut out = Vec::new();
    for arg in args {
        if let Some(items) = seq_items(arg)? {
            out.extend(items);
        }
    }
    Ok(Value::List(List::from_vec(out)))
}

fn native_list(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    Ok(Value::List(List::from_vec(args.to_vec())))
}

fn native_vector(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    Ok(Value::Vector(Vector::from_vec(args.to_vec())))
}

fn native_hash_map(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() % 2 == 1 {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!(
                "No value supplied for key: {}",
                str_of(&args[args.len() - 1])
            ),
        ));
    }
    let mut out = new_map();
    for i in (0..args.len()).step_by(2) {
        out = out.insert(args[i].clone(), args[i + 1].clone());
    }
    Ok(Value::Map(out.with_hashy(true)))
}

fn native_hash_set(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let mut out = new_set();
    for arg in args {
        out = out.insert(arg.clone());
    }
    Ok(Value::Set(out))
}

fn native_apply(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 {
        return Err(NativeError::arity("clojure.core/apply", args.len()));
    }
    let mut call_args: Vec<Value> = args[1..args.len() - 1].to_vec();
    // The spread realizes (JVM `spreadToArray`); any seq value —
    // including the lazy ones syntax-quote expansions build — seqs.
    let spread = crate::lazy::collect_all(eval, &args[args.len() - 1]).map_err(signal_to_native)?;
    call_args.extend(spread);
    eval.call(&args[0], &call_args).map_err(signal_to_native)
}

/// The S5C natives installed into `clojure.core` by [`boot`](crate::ns::boot).
pub(crate) const MACRO_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("macroexpand-1", native_macroexpand_1),
    ("macroexpand", native_macroexpand),
    ("gensym", native_gensym),
    // Early (S5D rows): syntax-quote expansions call these.
    ("seq", native_seq),
    ("concat", native_concat),
    ("list", native_list),
    ("apply", native_apply),
    ("vector", native_vector),
    ("hash-map", native_hash_map),
    ("hash-set", native_hash_set),
];
