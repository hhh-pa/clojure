// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Build-time front end of the native compiler (issue #259 §2.1).
//!
//! Reads a program one top-level form at a time with the live
//! reader (data readers, `::kw`, `*ns*` exactly as `run` sees them),
//! macroexpands and splits top-level `do` like `eval_top_form`, and
//! analyzes each unit into an [`Expr`] with the interpreter's own
//! analyzer. Between units it applies the **environment effects**
//! later reads and expansions depend on — namespaces, requires,
//! macros, and plain fn definitions (so macros calling helper fns
//! expand) — by evaluating exactly those units on the build
//! evaluator. Every other unit is only analyzed: nothing like
//! `(slurp *in*)` ever runs during `build`.

use std::sync::Arc;

use clj_reader::Span;
use clj_value::Value;

use crate::error::span_of;
use crate::eval::{Evaluator, SourceError, split_top_do};
use crate::expr::{Arity, Expr, FunExpr, Pattern};

/// Core fns whose top-level calls change what later forms read or
/// resolve (evaluated at build time as well as compiled).
const ENV_FNS: [&str; 9] = [
    "require",
    "use",
    "import",
    "refer",
    "refer-clojure",
    "in-ns",
    "alias",
    "load",
    "create-ns",
];

/// One analyzed top-level unit.
#[derive(Debug)]
pub struct TopUnit {
    /// The analyzed unit.
    pub expr: Expr,
    /// Whether the build evaluator also evaluated it (environment
    /// effect).
    pub env_effect: bool,
    /// The unit's source span.
    pub span: Span,
}

/// The front end's output: units in load order.
#[derive(Debug, Default)]
pub struct FrontEnd {
    /// Every top-level unit, `do`-split, in load order.
    pub units: Vec<TopUnit>,
}

/// Run the front end over `text` (named `path` in spans) on a
/// booted build evaluator, inside `run`'s own binding frame
/// ([`run_script`](crate::script::run_script): `*ns*`,
/// `*warn-on-reflection*`, … plus the `clojure.string` preload), so
/// every unit expands and analyzes exactly as under `run`. The first
/// read, expansion, or analysis failure stops the front end (the
/// caller decides the fallback).
pub fn front_end(eval: &mut Evaluator, text: &str, path: &str) -> Result<FrontEnd, SourceError> {
    type Slot = Arc<std::sync::Mutex<Option<Result<FrontEnd, SourceError>>>>;
    let slot: Slot = Arc::default();
    let inner = Arc::clone(&slot);
    let text: Arc<str> = Arc::from(text);
    let path: Arc<str> = Arc::from(path);
    eval.install_native(
        FRONT_NS,
        "front-end",
        Arc::new(move |eval: &mut Evaluator, _: &[Value]| {
            let prev = std::mem::replace(&mut eval.source, Arc::clone(&path));
            let out = front_end_inner(eval, &text);
            eval.source = prev;
            *inner.lock().expect("front-end slot") = Some(out);
            Ok(Value::Nil)
        }),
    );
    let script = crate::script::run_script(&[], &format!("({FRONT_NS}/front-end)"));
    eval.eval_source(&script)?;
    let out = slot.lock().expect("front-end slot").take();
    out.unwrap_or_else(|| Err(SourceError::Internal("front end did not run".to_owned())))
}

/// The build-time namespace holding the front-end entry.
const FRONT_NS: &str = "clj.aot.build";

fn front_end_inner(eval: &mut Evaluator, text: &str) -> Result<FrontEnd, SourceError> {
    let mut front = FrontEnd::default();
    let mut forms = eval.live_forms(text);
    while let Some(form) = eval.next_live_form(&mut forms)? {
        unit(eval, &form, &mut front)?;
    }
    Ok(front)
}

/// Expand, split, analyze (and maybe evaluate) one top-level form.
fn unit(eval: &mut Evaluator, form: &Value, front: &mut FrontEnd) -> Result<(), SourceError> {
    let source = Arc::clone(&eval.source);
    let span = span_of(form, Span::new(1, 1));
    let expanded = crate::macros::macroexpand_full(eval, form, span, &source)
        .map_err(|e| eval.compile_err(e))?;
    if let Some(elements) = split_top_do(&expanded) {
        for element in elements {
            unit(eval, element, front)?;
        }
        return Ok(());
    }
    eval.registry.bump_eval_seq();
    let expr = eval.analyze(&expanded).map_err(|e| eval.compile_err(e))?;
    let env_effect = is_env_effect(&expr);
    if env_effect {
        let code = crate::vm::compile_top(&expr);
        crate::vm::run_top(eval, &code).map_err(crate::eval::uncaught)?;
    }
    front.units.push(TopUnit {
        expr,
        env_effect,
        span,
    });
    Ok(())
}

/// Whether a unit changes the environment later units read or
/// expand against (and is safe to run at build time).
#[must_use]
pub fn is_env_effect(expr: &Expr) -> bool {
    match expr {
        Expr::Ns { .. } | Expr::DefMacro { .. } | Expr::Declare { .. } => true,
        // A plain fn definition whose name metadata is side-effect
        // free: macros may call it during later expansions.
        Expr::Def { init, meta, .. } => {
            init.as_deref().is_none_or(|e| matches!(e, Expr::Fn(_)))
                && meta.iter().flatten().all(|(k, v)| is_pure(k) && is_pure(v))
        }
        Expr::Invoke { head, .. } => {
            matches!(head.as_ref(), Expr::Global(var) if is_env_var(var))
        }
        Expr::DirectNative(call) => is_env_var(&call.var),
        _ => false,
    }
}

/// Whether evaluating `expr` cannot have side effects (constants and
/// collection literals of constants).
fn is_pure(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(_) | Expr::Quote(_) => true,
        Expr::VecLit(items, _) | Expr::SetLit(items, _) => items.iter().all(is_pure),
        Expr::MapLit(pairs, _) => pairs.iter().all(|(k, v)| is_pure(k) && is_pure(v)),
        _ => false,
    }
}

fn is_env_var(var: &clj_value::Var) -> bool {
    var.sym.namespace() == Some("clojure.core") && ENV_FNS.contains(&var.sym.name())
}

/// Call `f` on every direct child expression of `expr` (patterns'
/// `:or` defaults and fn arity bodies included).
pub fn for_each_child<'a>(expr: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    match expr {
        Expr::Literal(_)
        | Expr::Global(_)
        | Expr::Local(_)
        | Expr::Quote(_)
        | Expr::VarRef(_)
        | Expr::NewThrowable { .. }
        | Expr::Declare { .. }
        | Expr::Ns { .. } => {}
        Expr::VecLit(items, _) | Expr::SetLit(items, _) | Expr::Do(items) | Expr::Recur(items) => {
            items.iter().for_each(f);
        }
        Expr::MapLit(pairs, _) => pairs.iter().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
        Expr::Def { init, meta, .. } => {
            init.iter().for_each(|e| f(e));
            meta.iter().flatten().for_each(|(k, v)| {
                f(k);
                f(v);
            });
        }
        Expr::If { test, then, els } => {
            f(test);
            f(then);
            f(els);
        }
        Expr::Let { bindings, body } => {
            for (pat, init) in bindings {
                pattern_children(pat, f);
                f(init);
            }
            body.iter().for_each(f);
        }
        Expr::Loop { bindings, body } => {
            for (pat, init) in bindings {
                pattern_children(pat, f);
                f(init);
            }
            body.iter().for_each(f);
        }
        Expr::LetFn { fns, body } => {
            fns.iter().for_each(|(_, fun)| fun_children(fun, f));
            body.iter().for_each(f);
        }
        Expr::Fn(fun) | Expr::BoundFn { fun, .. } | Expr::DefMacro { fun, .. } => {
            fun_children(fun, f);
        }
        Expr::Throw(e) | Expr::Time { expr: e } => f(e),
        Expr::Try {
            body,
            catches,
            finally_body,
        } => {
            body.iter().for_each(&mut *f);
            catches.iter().for_each(|c| c.body.iter().for_each(&mut *f));
            finally_body.iter().for_each(f);
        }
        Expr::Set { val, .. } => f(val),
        Expr::Binding { pairs, body } | Expr::WithRedefs { pairs, body } => {
            pairs.iter().for_each(|(_, e)| f(e));
            body.iter().for_each(f);
        }
        Expr::WithBindings { map, body } => {
            f(map);
            body.iter().for_each(f);
        }
        Expr::Assert { test, message, .. } => {
            f(test);
            message.iter().for_each(|m| f(m));
        }
        Expr::WithOutStr { body, .. }
        | Expr::DelayStar { body, .. }
        | Expr::Sync { body }
        | Expr::Io { body } => body.iter().for_each(f),
        Expr::LazySeqStar { body } => body.iter().for_each(f),
        Expr::WithInStr { s, body, .. } => {
            f(s);
            body.iter().for_each(f);
        }
        Expr::Locking { lockee, body } => {
            f(lockee);
            body.iter().for_each(f);
        }
        Expr::CaseStar {
            expr,
            default,
            cases,
            ..
        } => {
            f(expr);
            f(default);
            cases.iter().for_each(|(_, test, then)| {
                f(test);
                f(then);
            });
        }
        Expr::Invoke { head, args, .. } => {
            f(head);
            args.iter().for_each(f);
        }
        Expr::DirectNative(call) => call.args.iter().for_each(f),
    }
}

fn fun_children<'a>(fun: &'a FunExpr, f: &mut dyn FnMut(&'a Expr)) {
    for Arity { fixed, rest, body } in fun.arities.iter() {
        fixed.iter().for_each(|p| pattern_children(p, f));
        rest.iter().for_each(|p| pattern_children(p, f));
        body.iter().for_each(&mut *f);
    }
}

fn pattern_children<'a>(pat: &'a Pattern, f: &mut dyn FnMut(&'a Expr)) {
    match pat {
        Pattern::Symbol(_) => {}
        Pattern::Vec(v) => {
            v.fixed.iter().for_each(|p| pattern_children(p, f));
            v.rest.iter().for_each(|p| pattern_children(p, f));
        }
        Pattern::Map(m) => {
            m.entries
                .iter()
                .for_each(|(p, _, _, _)| pattern_children(p, f));
            m.or.iter().for_each(|(_, e)| f(e));
        }
    }
}
