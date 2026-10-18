// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The tree-walking [`Evaluator`]: [`Expr`]s become values, throwing
//! [`Signal`]s. Covers invocation (§E7), `try`, `loop`/`recur`,
//! `def`/`set!`/`binding`, and the full destructure engine (§E12).
//!
//! S4 ships no prelude: tests inject [`NativeFn`](crate::ext::NativeFn)
//! doubles; `ex-info` et al. arrive with the S5 core subset.

use std::sync::Arc;

use clj_reader::{ReaderOpts, Span, read_all};
use clj_value::{Keyword, List, Map, Symbol, ThrowClass, Throwable, Value, Var, Vector, new_map};

use crate::env::{Bindings, Env, Namespace};
use crate::error::{CompileError, NativeError, Signal, jvm_class_of, simple_class_of};
use crate::expr::{Analyzer, Catch, Expr, MapPattern, Pattern, pattern_names};
use crate::ext::{Closure, NativeFn, as_closure, as_native};

/// `eval_source` failures: reader errors, analyzer errors, uncaught
/// throwables, and (unreachable by construction) escaping `recur`s.
#[derive(Debug)]
pub enum SourceError {
    Reader(clj_reader::ReaderError),
    Compile(CompileError),
    Uncaught(Throwable),
    Internal(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(e) => write!(f, "{e}"),
            Self::Compile(e) => write!(f, "{e}"),
            Self::Uncaught(t) => write!(f, "{} | {}", t.class.jvm_name(), t.message),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// The evaluator: namespace, bindings, load path, and the
/// deterministic counters (form/fn numbering for display names, one
/// identity sequence for throwables/closures/natives).
pub struct Evaluator {
    pub ns: Namespace,
    bindings: Bindings,
    source: Arc<str>,
    eval_seq: u64,
    fn_seq: u64,
    next_id: u64,
}

impl Evaluator {
    /// A fresh evaluator loading from `source` (`:file`, used in
    /// spans and `def` meta).
    #[must_use]
    pub fn new(source: &str) -> Self {
        Self {
            ns: Namespace::new("user"),
            bindings: Bindings::new(),
            source: Arc::from(source),
            eval_seq: 0,
            fn_seq: 0,
            next_id: 1,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Analyze one form against the current namespace (tests use
    /// this for contract errors without evaluating).
    pub fn analyze(&mut self, form: &Value) -> Result<Expr, CompileError> {
        Analyzer::new(&self.source, &mut self.ns, self.eval_seq, &mut self.fn_seq).analyze(
            form,
            Span::new(1, 1),
            true,
        )
    }

    /// Read every form in `text` (default `:read-cond :allow`
    /// options), analyze, and evaluate each in turn, collecting the
    /// values. Uncaught throwables (including bare macro-arity
    /// failures) abort the load.
    pub fn eval_source(&mut self, text: &str) -> Result<Vec<Value>, SourceError> {
        let forms = read_all(text, &ReaderOpts::default()).map_err(SourceError::Reader)?;
        let mut out = Vec::with_capacity(forms.len());
        for form in &forms {
            self.eval_seq += 1;
            let expr = self.analyze(form).map_err(|e| {
                if e.bare {
                    let id = self.next_id();
                    SourceError::Uncaught(Throwable {
                        id,
                        class: ThrowClass::ArityException,
                        message: Arc::from(e.message.as_str()),
                        data: None,
                        cause: None,
                    })
                } else {
                    SourceError::Compile(e)
                }
            })?;
            match self.eval(&expr, &Env::new()) {
                Ok(v) => out.push(v),
                Err(Signal::Throw(t)) => return Err(SourceError::Uncaught(t)),
                Err(Signal::Recur(_)) => {
                    return Err(SourceError::Internal(
                        "recur escaped its loop or fn".to_owned(),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Install a native fn under `name` (test doubles; S5 grows
    /// this into the core subset).
    pub fn install_native(&mut self, name: &str, func: Arc<crate::ext::NativeImpl>) -> Value {
        let id = self.next_id();
        let sym = Symbol::new(Some("user"), name);
        let var_id = self.ns.intern(&sym);
        let native = Value::Extension(Arc::new(NativeFn {
            id,
            display: Arc::from(format!("user/{name}").as_str()),
            func,
        }));
        if let Some(state) = self.ns.state_mut(var_id) {
            state.root = Some(native.clone());
        }
        native
    }

    /// Stamp a runtime failure with an identity and throw it.
    fn throw(&mut self, err: NativeError) -> Signal {
        Signal::Throw(Throwable {
            id: self.next_id(),
            class: err.class,
            message: Arc::from(err.message.as_str()),
            data: err.data.map(Box::new),
            cause: err.cause.map(Box::new),
        })
    }

    /// The visible value of a var: thread binding, else root (or
    /// the var's own `Unbound` sentinel, never a throw).
    fn deref_var(&self, id: u64, sym: &Symbol) -> Value {
        if let Some(v) = self.bindings.get(id) {
            return v.clone();
        }
        self.ns.root_or_unbound(id, sym)
    }

    /// Evaluate an expression to a value (or a control signal).
    pub fn eval(&mut self, expr: &Expr, env: &Env) -> Result<Value, Signal> {
        match expr {
            Expr::Literal(v) | Expr::Quote(v) => Ok(v.clone()),
            Expr::VecLit(elems) => self.eval_vec(elems, env),
            Expr::MapLit(pairs) => self.eval_map_lit(pairs, env),
            Expr::SetLit(elems) => self.eval_set_lit(elems, env),
            Expr::Global(sym) => {
                let id = self.var_id(sym);
                Ok(self.deref_var(id, sym))
            }
            Expr::Local(idx) => Ok(env.get(*idx)),
            Expr::VarRef(sym) => {
                let id = self.var_id(sym);
                Ok(Value::Var(Var {
                    id,
                    sym: sym.clone(),
                }))
            }
            Expr::Def {
                sym,
                doc,
                init,
                span,
            } => self.eval_def(sym, doc.clone(), init.as_deref(), *span, env),
            Expr::If { test, then, els } => {
                if self.eval(test, env)?.is_truthy() {
                    self.eval(then, env)
                } else {
                    self.eval(els, env)
                }
            }
            Expr::Do(exprs) => self.eval_body(exprs, env),
            Expr::Let { bindings, body } => {
                let env = self.bind_all(bindings, env)?;
                self.eval_body(body, &env)
            }
            Expr::Loop { bindings, body } => self.eval_loop(bindings, body, env),
            Expr::LetFn { fns, body } => self.eval_letfn(fns, body, env),
            Expr::Fn(fun) => Ok(Value::Extension(Arc::new(self.make_closure(fun, env)))),
            Expr::Recur(args) => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                Err(Signal::Recur(values))
            }
            Expr::Throw(inner) => self.eval_throw(inner, env),
            Expr::NewThrowable { class, message } => Ok(Value::Throwable(Throwable {
                id: self.next_id(),
                class: *class,
                message: Arc::from(message.as_str()),
                data: None,
                cause: None,
            })),
            Expr::Try {
                body,
                catches,
                finally_body,
            } => self.eval_try(body, catches, finally_body, env),
            Expr::Set { sym, val } => self.eval_set(sym, val, env),
            Expr::Binding { pairs, body } => self.eval_binding(pairs, body, env),
            Expr::Invoke { head, args } => {
                let callee = self.eval(head, env)?;
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, env)?);
                }
                self.invoke(&callee, &values)
            }
        }
    }

    /// A var id, interning on first reference.
    fn var_id(&mut self, sym: &Symbol) -> u64 {
        self.ns.lookup(sym).unwrap_or_else(|| self.ns.intern(sym))
    }

    /// A vector literal (elements evaluate left to right).
    fn eval_vec(&mut self, elems: &[Expr], env: &Env) -> Result<Value, Signal> {
        let mut out = Vec::with_capacity(elems.len());
        for elem in elems {
            out.push(self.eval(elem, env)?);
        }
        Ok(Value::Vector(Vector::from_vec(out)))
    }

    /// A map literal (keys and values evaluate left to right).
    fn eval_map_lit(&mut self, pairs: &[(Expr, Expr)], env: &Env) -> Result<Value, Signal> {
        let mut out = new_map();
        for (k, v) in pairs {
            let key = self.eval(k, env)?;
            let val = self.eval(v, env)?;
            out = out.insert(key, val);
        }
        Ok(Value::Map(out))
    }

    /// A set literal (elements evaluate left to right).
    fn eval_set_lit(&mut self, elems: &[Expr], env: &Env) -> Result<Value, Signal> {
        let mut out = clj_value::new_set();
        for elem in elems {
            out = out.insert(self.eval(elem, env)?);
        }
        Ok(Value::Set(out))
    }

    /// `letfn`: extend the frame, fill each slot with its closure
    /// (self/mutual recursion), run the body.
    fn eval_letfn(
        &mut self,
        fns: &[(String, crate::expr::FunExpr)],
        body: &[Expr],
        env: &Env,
    ) -> Result<Value, Signal> {
        let env = env.extend(vec![Value::Nil; fns.len()]);
        let base = env.len() - fns.len();
        for (i, (_, fun)) in fns.iter().enumerate() {
            let closure = self.make_closure(fun, &env);
            env.set(base + i, Value::Extension(Arc::new(closure)));
        }
        self.eval_body(body, &env)
    }

    /// `throw`: throwables signal, anything else is the
    /// `Throwable` cast error.
    fn eval_throw(&mut self, inner: &Expr, env: &Env) -> Result<Value, Signal> {
        let value = self.eval(inner, env)?;
        match value {
            Value::Throwable(t) => Err(Signal::Throw(t)),
            other => Err(self.throw(NativeError::cast(
                &jvm_class_of(&other),
                "java.lang.Throwable",
            ))),
        }
    }

    /// `set!`: rebind the thread-local top, else the root-binding
    /// error.
    fn eval_set(&mut self, sym: &Symbol, val: &Expr, env: &Env) -> Result<Value, Signal> {
        let value = self.eval(val, env)?;
        let id = self.var_id(sym);
        if self.bindings.set_top(id, value.clone()) {
            Ok(value)
        } else {
            Err(self.throw(NativeError::new(
                ThrowClass::IllegalStateException,
                format!(
                    "Can't change/establish root binding of: {} with set",
                    sym.name()
                ),
            )))
        }
    }

    /// `binding`: push dynamic bindings (validating dynamic-ness
    /// first), run the body, always pop.
    fn eval_binding(
        &mut self,
        pairs: &[(Symbol, Expr)],
        body: &[Expr],
        env: &Env,
    ) -> Result<Value, Signal> {
        let depth = self.bindings.depth();
        for (sym, init) in pairs {
            let id = self.var_id(sym);
            let dynamic = self.ns.state(id).is_some_and(|s| s.dynamic);
            if !dynamic {
                self.bindings.truncate(depth);
                return Err(self.throw(NativeError::new(
                    ThrowClass::IllegalStateException,
                    format!("Can't dynamically bind non-dynamic var: {sym}"),
                )));
            }
            let value = match self.eval(init, env) {
                Ok(v) => v,
                Err(signal) => {
                    self.bindings.truncate(depth);
                    return Err(signal);
                }
            };
            self.bindings.push(id, value);
        }
        let result = self.eval_body(body, env);
        self.bindings.truncate(depth);
        result
    }

    /// A body: the last value, or `nil` when empty.
    fn eval_body(&mut self, body: &[Expr], env: &Env) -> Result<Value, Signal> {
        let mut value = Value::Nil;
        for expr in body {
            value = self.eval(expr, env)?;
        }
        Ok(value)
    }

    /// Bind `let`/`loop` pairs left to right (each init sees the
    /// previous pairs), returning the extended frame.
    fn bind_all(&mut self, bindings: &[(Pattern, Expr)], env: &Env) -> Result<Env, Signal> {
        let mut env = env.clone();
        for (pat, init) in bindings {
            let value = self.eval(init, &env)?;
            env = self.bind_pattern(pat, value, &env)?;
        }
        Ok(env)
    }

    /// Bind one pattern: extend the frame, evaluate `:or` defaults
    /// against the pre-extension frame, destructure into the slots.
    fn bind_pattern(&mut self, pat: &Pattern, value: Value, env: &Env) -> Result<Env, Signal> {
        let count = pattern_names(pat).len();
        let env = env.extend(vec![Value::Nil; count]);
        let base = env.len() - count;
        let mut out = Vec::with_capacity(count);
        self.destructure(pat, &value, &env, &mut out)?;
        debug_assert_eq!(out.len(), count, "destructure fills every slot");
        for (i, v) in out.into_iter().enumerate() {
            env.set(base + i, v);
        }
        Ok(env)
    }

    /// `(def …)`: evaluate the init (or leave rootless), refresh
    /// root/meta/dynamic, return the var. Dynamic is the truthiness
    /// of the sym's `:dynamic` meta, reset by every `def`
    /// (probe25).
    fn eval_def(
        &mut self,
        sym: &Symbol,
        doc: Option<String>,
        init: Option<&Expr>,
        span: Span,
        env: &Env,
    ) -> Result<Value, Signal> {
        let id = self.ns.intern(sym);
        let value = match init {
            Some(init) => Some(self.eval(init, env)?),
            None => None,
        };
        let dynamic = sym
            .meta()
            .and_then(|meta| {
                let probe = Value::Keyword(Keyword::new(None, "dynamic"));
                meta.get(&probe)
            })
            .is_some_and(Value::is_truthy);
        if let Some(state) = self.ns.state_mut(id) {
            if let Some(v) = value {
                state.root = Some(v);
            }
            let mut meta = sym.meta().cloned().unwrap_or_else(new_map);
            if let Some(doc) = doc {
                let key = Value::Keyword(Keyword::new(None, "doc"));
                meta = meta.insert(key, Value::Str(Arc::from(doc.as_str())));
            }
            let auto = [
                ("line", Value::Int(i64::from(span.line))),
                ("column", Value::Int(i64::from(span.col))),
                ("file", Value::Str(Arc::clone(&self.source))),
                ("name", Value::Symbol(Symbol::new(None, sym.name()))),
            ];
            for (key, val) in auto {
                let key = Value::Keyword(Keyword::new(None, key));
                meta = meta.insert(key, val);
            }
            state.meta = meta;
            state.dynamic = dynamic;
        }
        Ok(Value::Var(Var {
            id,
            sym: sym.clone(),
        }))
    }

    /// `(loop …)`: bind once, then run the body, rebinding through
    /// the patterns on every `recur`.
    fn eval_loop(
        &mut self,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        env: &Env,
    ) -> Result<Value, Signal> {
        let base = env.len();
        // Bind sequentially (each init sees the previous pairs),
        // then run the body against the frame.
        let mut env = env.clone();
        for (pat, init) in bindings {
            let value = self.eval(init, &env)?;
            env = self.bind_pattern(pat, value, &env)?;
        }
        loop {
            match self.eval_body(body, &env) {
                Ok(v) => return Ok(v),
                Err(Signal::Recur(values)) => {
                    let mut slot = base;
                    for ((pat, _), value) in bindings.iter().zip(values) {
                        let count = pattern_names(pat).len();
                        let mut out = Vec::with_capacity(count);
                        self.destructure(pat, &value, &env, &mut out)?;
                        for (i, v) in out.into_iter().enumerate() {
                            env.set(slot + i, v);
                        }
                        slot += count;
                    }
                }
                Err(signal) => return Err(signal),
            }
        }
    }

    /// `(try …)`: the first matching `catch` wins (by hierarchy);
    /// `finally` runs on every path and its value is discarded.
    fn eval_try(
        &mut self,
        body: &[Expr],
        catches: &[Catch],
        finally_body: &[Expr],
        env: &Env,
    ) -> Result<Value, Signal> {
        let result = self.eval_body(body, env);
        match result {
            Ok(value) => {
                self.eval_body(finally_body, env)?;
                Ok(value)
            }
            Err(Signal::Throw(thrown)) => {
                for catch in catches {
                    if thrown.class.matches(&catch.class) {
                        let env = env.extend(vec![Value::Throwable(thrown.clone())]);
                        debug_assert_eq!(env.len() - 1, catch.slot, "catch binds one slot");
                        let result = self.eval_body(&catch.body, &env);
                        // `finally` still runs when the handler throws.
                        match result {
                            Ok(value) => {
                                self.eval_body(finally_body, &env)?;
                                return Ok(value);
                            }
                            Err(Signal::Throw(_)) => {
                                let _ = self.eval_body(finally_body, &env);
                                return result;
                            }
                            Err(signal) => return Err(signal),
                        }
                    }
                }
                let _ = self.eval_body(finally_body, env);
                Err(Signal::Throw(thrown))
            }
            Err(signal) => {
                let _ = self.eval_body(finally_body, env);
                Err(signal)
            }
        }
    }

    /// Build a closure over the current frame.
    fn make_closure(&mut self, fun: &crate::expr::FunExpr, env: &Env) -> Closure {
        Closure {
            id: self.next_id(),
            display: Arc::from(fun.display.as_str()),
            arities: fun.arities.clone(),
            env: env.clone(),
        }
    }

    /// The §E7 `IFn` table: closures/natives, keywords, symbols,
    /// vectors, maps, sets, vars (deref-and-delegate), unbounds
    /// (the `IllegalStateException`), and the `ClassCastException`
    /// for everything else.
    fn invoke(&mut self, callee: &Value, args: &[Value]) -> Result<Value, Signal> {
        match callee {
            Value::Nil => Err(self.throw(NativeError::new(
                ThrowClass::NullPointerException,
                "Cannot invoke nil as a function".to_owned(),
            ))),
            Value::Extension(_) => self.invoke_extension(callee, args),
            Value::Keyword(kw) => {
                let name = kw.to_string();
                match args {
                    [coll] => Ok(lookup(coll, callee).unwrap_or(Value::Nil)),
                    [coll, not_found] => {
                        Ok(lookup(coll, callee).unwrap_or_else(|| not_found.clone()))
                    }
                    _ => Err(self.throw(NativeError::arity(&name, args.len()))),
                }
            }
            Value::Symbol(_) => match args {
                [coll] => Ok(lookup(coll, callee).unwrap_or(Value::Nil)),
                [coll, not_found] => Ok(lookup(coll, callee).unwrap_or_else(|| not_found.clone())),
                _ => Err(self.throw(NativeError::arity("clojure.lang.Symbol", args.len()))),
            },
            Value::Vector(v) => self.invoke_vector(v, args),
            Value::Map(_) => {
                let name = jvm_class_of(callee);
                match args {
                    [key] => Ok(lookup(callee, key).unwrap_or(Value::Nil)),
                    [key, not_found] => {
                        Ok(lookup(callee, key).unwrap_or_else(|| not_found.clone()))
                    }
                    _ => Err(self.throw(NativeError::arity(&name, args.len()))),
                }
            }
            Value::Set(s) => match args {
                [key] => Ok(member(s, key).unwrap_or(Value::Nil)),
                [key, not_found] => Ok(member(s, key).unwrap_or_else(|| not_found.clone())),
                _ => Err(self.throw(NativeError::arity(
                    "clojure.lang.PersistentHashSet",
                    args.len(),
                ))),
            },
            Value::Var(v) => self.invoke_var(v, args),
            Value::Unbound(u) => Err(self.throw(NativeError::new(
                ThrowClass::IllegalStateException,
                format!("Attempting to call unbound fn: #'{}", u.sym),
            ))),
            _ => Err(self.throw(NativeError::cast(&jvm_class_of(callee), "clojure.lang.IFn"))),
        }
    }

    /// Invoke an extension value: closures apply, natives run,
    /// anything else is the `IFn` cast error.
    fn invoke_extension(&mut self, callee: &Value, args: &[Value]) -> Result<Value, Signal> {
        if let Some(closure) = as_closure(callee) {
            return self.apply_closure(closure, args);
        }
        if let Some(native) = as_native(callee) {
            let func = Arc::clone(&native.func);
            return func(args).map_err(|e| self.throw(e));
        }
        Err(self.throw(NativeError::cast(&jvm_class_of(callee), "clojure.lang.IFn")))
    }

    /// Invoke a vector: exactly one integer key (arity, key-type,
    /// then bounds, §E7).
    fn invoke_vector(&mut self, v: &clj_value::Vector, args: &[Value]) -> Result<Value, Signal> {
        if args.len() != 1 {
            return Err(self.throw(NativeError::arity(
                "clojure.lang.PersistentVector",
                args.len(),
            )));
        }
        let Value::Int(n) = &args[0] else {
            return Err(self.throw(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "Key must be integer".to_owned(),
            )));
        };
        match usize::try_from(*n).ok().and_then(|i| v.iter().nth(i)) {
            Some(elem) => Ok(elem.clone()),
            None => Err(self.throw(NativeError::new(
                ThrowClass::IndexOutOfBoundsException,
                String::new(),
            ))),
        }
    }

    /// Invoke a var: deref and delegate (unbounds are the
    /// `IllegalStateException`).
    fn invoke_var(&mut self, v: &Var, args: &[Value]) -> Result<Value, Signal> {
        let root = self.deref_var(v.id, &v.sym);
        if matches!(root, Value::Unbound(_)) {
            return Err(self.throw(NativeError::new(
                ThrowClass::IllegalStateException,
                format!("Attempting to call unbound fn: #'{}", v.sym),
            )));
        }
        self.invoke(&root, args)
    }

    /// Apply a closure: fixed arities by count, else the variadic
    /// arity (`rest` binds `nil` when empty, §E5).
    fn apply_closure(&mut self, closure: &Closure, args: &[Value]) -> Result<Value, Signal> {
        let mut variadic: Option<&crate::expr::Arity> = None;
        for arity in &closure.arities {
            if arity.rest.is_none() && arity.fixed.len() == args.len() {
                return self.apply_arity(closure, arity, args, None);
            }
            if arity.rest.is_some() {
                variadic = Some(arity);
            }
        }
        if let Some(arity) = variadic.filter(|a| args.len() >= a.fixed.len()) {
            let rest = rest_list(&args[arity.fixed.len()..]);
            return self.apply_arity(closure, arity, args, Some(rest));
        }
        Err(self.throw(NativeError::arity(&closure.display, args.len())))
    }

    /// Bind one arity's parameters (sequentially, like `let`) and
    /// run the body, looping on `recur`.
    fn apply_arity(
        &mut self,
        closure: &Closure,
        arity: &crate::expr::Arity,
        args: &[Value],
        rest: Option<Value>,
    ) -> Result<Value, Signal> {
        let mut values: Vec<Value> = args[..arity.fixed.len()].to_vec();
        if let Some(rest) = rest {
            values.push(rest);
        }
        let pats: Vec<&Pattern> = arity.fixed.iter().chain(arity.rest.iter()).collect();
        let base = closure.env.len();
        let mut env = closure.env.clone();
        for pat in &pats {
            let count = pattern_names(pat).len();
            env = env.extend(vec![Value::Nil; count]);
        }
        let frame = env.clone();
        let mut rebind = values;
        loop {
            let mut slot = base;
            for (pat, value) in pats.iter().zip(rebind) {
                let count = pattern_names(pat).len();
                let mut out = Vec::with_capacity(count);
                self.destructure(pat, &value, &frame, &mut out)?;
                for (i, v) in out.into_iter().enumerate() {
                    frame.set(slot + i, v);
                }
                slot += count;
            }
            match self.eval_body(&arity.body, &frame) {
                Ok(v) => return Ok(v),
                Err(Signal::Recur(values)) => rebind = values,
                Err(signal) => return Err(signal),
            }
        }
    }

    /// Destructure a value against a pattern, pushing bound values
    /// in [`pattern_names`] order.
    fn destructure(
        &mut self,
        pat: &Pattern,
        value: &Value,
        env: &Env,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        match pat {
            Pattern::Symbol(_) => {
                out.push(value.clone());
                Ok(())
            }
            Pattern::Vec(v) => self.destructure_seq(v, value, env, out),
            Pattern::Map(m) => self.bind_map(m, value, env, out),
        }
    }

    /// `destvec*`: without `&`, positional `nth`-with-`nil` (wrong
    /// types are `nth not supported`); with `&`, `first`/`next`
    /// chaining; `:as` takes the whole.
    fn destructure_seq(
        &mut self,
        pat: &crate::expr::VecPattern,
        value: &Value,
        env: &Env,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        if pat.rest.is_none() {
            for (i, fixed) in pat.fixed.iter().enumerate() {
                let nth = nth_or_nil(value, i, self)?;
                self.destructure(fixed, &nth, env, out)?;
            }
        } else {
            // `destvec*` binds `gseq` to `(seq gvec)`: the `&`
            // pattern gets a seq (`nil` when empty) and
            // non-seqables raise the `ISeq` error (§E12).
            let mut current = self.seq_val(value)?;
            for fixed in &pat.fixed {
                let first = first_of(&current);
                self.destructure(fixed, &first, env, out)?;
                current = next_of(&current);
            }
            if let Some(rest) = &pat.rest {
                self.destructure(rest, &current, env, out)?;
            }
        }
        if pat.as_sym.is_some() {
            out.push(value.clone());
        }
        Ok(())
    }

    /// `destmap*`: seq-convert, eager `:or`, entries, keys,
    /// outlets. Pushes values in [`pattern_names`] order (entries,
    /// keys, `:as`, `:select`, `:all`, `:excess`, `:missing`).
    fn bind_map(
        &mut self,
        pat: &MapPattern,
        value: &Value,
        env: &Env,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        let converted = self.seq_to_map(value)?;
        let defaults = self.eval_defaults(pat, env)?;
        let ctx = MapBind {
            pat,
            value,
            env,
            converted: &converted,
            defaults: &defaults,
        };
        // Entries and keys (missing keys take the local-then-key
        // default; required keys `req!`).
        let mut missing: Option<Map> = pat.missing.is_some().then(new_map);
        self.bind_entries(&ctx, &mut missing, out)?;
        self.bind_key_list(&ctx, &mut missing, out)?;
        if pat.as_sym.is_some() {
            out.push(converted.clone());
        }
        self.bind_outlets(&ctx, &mut missing, out)?;
        if pat.missing.is_some() {
            out.push(missing.map(Value::Map).unwrap_or(Value::Nil));
        }
        Ok(())
    }

    /// The eager `:or` defaults (evaluated once, before any lookup).
    fn eval_defaults(&mut self, pat: &MapPattern, env: &Env) -> Result<Map, Signal> {
        let mut defaults: Map = new_map();
        for (key, default) in &pat.or {
            let val = self.eval(default, env)?;
            defaults = defaults.insert((*key).clone(), val);
        }
        Ok(defaults)
    }

    /// The `symbol keyword` entries.
    fn bind_entries(
        &mut self,
        ctx: &MapBind,
        missing: &mut Option<Map>,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        for (local, key) in &ctx.pat.entries {
            // `:or` is keyed by the binding local first, then the
            // lookup key (`destmap*` `push1`); nested locals have no
            // `Value` form, so only symbols get the local lookup.
            let local_val = match local {
                Pattern::Symbol(name) => Some(Value::Symbol(Symbol::new(None, name))),
                _ => None,
            };
            let val = self.map_binding(
                ctx.converted,
                ctx.defaults,
                local_val.as_ref(),
                key,
                false,
                missing,
            )?;
            self.destructure(local, &val, ctx.env, out)?;
        }
        Ok(())
    }

    /// The `:keys`/`:strs`/`:syms` bindings.
    fn bind_key_list(
        &mut self,
        ctx: &MapBind,
        missing: &mut Option<Map>,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        for key in &ctx.pat.keys {
            let local_key = Value::Symbol(Symbol::new(None, &key.local));
            let val = self.map_binding(
                ctx.converted,
                ctx.defaults,
                Some(&local_key),
                &key.key,
                key.required,
                missing,
            )?;
            out.push(val);
        }
        Ok(())
    }

    /// The 1.13 outlets (`:select`/`:all`/`:excess`, folding the
    /// nested-outlet `:missing` merge in when requested).
    fn bind_outlets(
        &mut self,
        ctx: &MapBind,
        missing: &mut Option<Map>,
        out: &mut Vec<Value>,
    ) -> Result<(), Signal> {
        let pat = ctx.pat;
        if pat.select.is_none() && pat.all.is_none() && pat.excess.is_none() {
            if pat.missing.is_some() {
                let subs_missing = self.sub_outlets(pat, ctx.value, ctx.env, Outlet::Missing)?;
                *missing = merge_missing(missing.take(), subs_missing);
            }
            return Ok(());
        }
        let sel = selection(pat);
        let subs_select = self.sub_outlets(pat, ctx.value, ctx.env, Outlet::Select)?;
        let subs_all = self.sub_outlets(pat, ctx.value, ctx.env, Outlet::All)?;
        let subs_excess = self.sub_outlets(pat, ctx.value, ctx.env, Outlet::Excess)?;
        let subs_missing = self.sub_outlets(pat, ctx.value, ctx.env, Outlet::Missing)?;
        *missing = merge_missing(missing.take(), subs_missing);
        if pat.select.is_some() {
            let dm = or_mapped(pat, ctx.defaults);
            let merged = merge3(&dm, ctx.converted, &subs_select);
            out.push(select_keys(&merged, &sel));
        }
        if pat.all.is_some() {
            let dm = or_mapped(pat, ctx.defaults);
            out.push(Value::Map(merge3(&dm, ctx.converted, &subs_all)));
        }
        if pat.excess.is_some() {
            let leftover = dissoc_all(ctx.converted, &sel);
            let base = if leftover.is_empty() {
                None
            } else {
                Some(Value::Map(leftover))
            };
            out.push(merge_opt(base.as_ref(), subs_excess.as_ref()));
        }
        Ok(())
    }

    /// One map binding: present keys read, missing keys take the
    /// local-then-key default, missing required keys `req!` (or
    /// record into `:missing` and bind `nil`).
    fn map_binding(
        &mut self,
        converted: &Value,
        defaults: &Map,
        local: Option<&Value>,
        key: &Value,
        required: bool,
        missing: &mut Option<Map>,
    ) -> Result<Value, Signal> {
        if let Some(val) = lookup(converted, key) {
            return Ok(val);
        }
        let default = local
            .and_then(|local| defaults.get(local).cloned())
            .or_else(|| defaults.get(key).cloned());
        if required && default.is_none() {
            if missing.is_some() {
                if let Some(m) = missing.take() {
                    *missing = Some(m.insert(key.clone(), Value::Nil));
                }
                return Ok(Value::Nil);
            }
            return Err(self.throw(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!("Missing required key: {}", pr_of(key)),
            )));
        }
        Ok(default.unwrap_or(Value::Nil))
    }

    /// `seq-to-map-for-destructuring`: only lists convert (S4 has no
    /// other `ISeq`s). Empty is `{}`, one element is the element,
    /// longer reads pairs plus the trailing rule (§E12).
    fn seq_to_map(&mut self, value: &Value) -> Result<Value, Signal> {
        let Value::List(list) = value else {
            return Ok(value.clone());
        };
        let items: Vec<&Value> = list.iter().collect();
        match items.as_slice() {
            [] => Ok(Value::Map(new_map())),
            [single] => Ok((*single).clone()),
            _ => {
                let mut map = new_map();
                let mut pairs = &items[..];
                if pairs.len() % 2 == 1 {
                    let trailing = pairs[pairs.len() - 1];
                    pairs = &pairs[..pairs.len() - 1];
                    self.cons_trailing(&mut map, trailing)?;
                }
                for pair in pairs.chunks_exact(2) {
                    map = map.insert(pair[0].clone(), pair[1].clone());
                }
                Ok(Value::Map(map))
            }
        }
    }

    /// The odd-tail rule: maps merge, 2-vectors assoc, `nil`/empty
    /// seqables vanish, seqable elements are entries-or-`CCE`,
    /// anything else is the `ISeq` error (§E12).
    fn cons_trailing(&mut self, map: &mut Map, trailing: &Value) -> Result<(), Signal> {
        match trailing {
            Value::Map(m) => {
                for (k, v) in m.iter() {
                    *map = std::mem::replace(map, new_map()).insert(k.clone(), v.clone());
                }
                Ok(())
            }
            Value::Vector(v) => {
                let items: Vec<&Value> = v.iter().collect();
                if items.len() != 2 {
                    return Err(self.throw(NativeError::new(
                        ThrowClass::IllegalArgumentException,
                        "Vector arg to map conj must be a pair".to_owned(),
                    )));
                }
                *map = std::mem::replace(map, new_map()).insert(items[0].clone(), items[1].clone());
                Ok(())
            }
            Value::Nil => Ok(()),
            Value::List(_) | Value::Set(_) | Value::Str(_) => match seq_first(trailing) {
                None => Ok(()),
                Some(first) => Err(self.throw(NativeError::cast(
                    &jvm_class_of(&first),
                    "java.util.Map$Entry",
                ))),
            },
            other => Err(self.throw(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!(
                    "Don't know how to create ISeq from: {}",
                    jvm_class_of(other)
                ),
            ))),
        }
    }

    /// Nested outlet values (`:subs`/`:suba`/…): for each entry
    /// with a nested map pattern carrying the outlet, the
    /// entry-key → outlet-value pair, `nil`s dropped (`some-vals`).
    fn sub_outlets(
        &mut self,
        pat: &MapPattern,
        value: &Value,
        env: &Env,
        outlet: Outlet,
    ) -> Result<Option<Map>, Signal> {
        let converted = self.seq_to_map(value)?;
        let mut out: Option<Map> = None;
        for (local, key) in &pat.entries {
            let Pattern::Map(nested) = local else {
                continue;
            };
            if outlet.name(nested).is_none() {
                continue;
            }
            let Some(entry_val) = lookup(&converted, key) else {
                continue;
            };
            self.sub_outlet_entry(nested, key, &entry_val, env, outlet, &mut out)?;
        }
        Ok(out)
    }

    /// One nested entry's outlet contribution (entry-key →
    /// outlet-value, `nil`s dropped).
    fn sub_outlet_entry(
        &mut self,
        nested: &MapPattern,
        key: &Value,
        entry_val: &Value,
        env: &Env,
        outlet: Outlet,
        out: &mut Option<Map>,
    ) -> Result<(), Signal> {
        let mut vals = Vec::new();
        self.bind_map(nested, entry_val, env, &mut vals)?;
        let names = pattern_names(&Pattern::Map(nested.clone()));
        let Some(want) = outlet.name(nested) else {
            return Ok(());
        };
        for (name, val) in names.iter().zip(vals) {
            if name == want && !matches!(val, Value::Nil) {
                let map = out.take().unwrap_or_else(new_map);
                *out = Some(map.insert(key.clone(), val));
            }
        }
        Ok(())
    }
}

/// The shared inputs to one map-pattern bind.
struct MapBind<'a> {
    pat: &'a MapPattern,
    value: &'a Value,
    env: &'a Env,
    converted: &'a Value,
    defaults: &'a Map,
}

/// Merge the nested-outlet `:missing` map into the local one.
fn merge_missing(local: Option<Map>, sub: Option<Map>) -> Option<Map> {
    match (local, sub) {
        (Some(m), Some(s)) => Some(merge_maps(&m, &s)),
        (m, s) => m.or(s),
    }
}

/// The four nested outlets (`:subs`/`:suba`/`:subexcess`/`:submissing`).
#[derive(Clone, Copy)]
enum Outlet {
    Select,
    All,
    Excess,
    Missing,
}

impl Outlet {
    /// The outlet's local name in a nested pattern, if bound.
    fn name(self, nested: &MapPattern) -> Option<&String> {
        match self {
            Outlet::Select => nested.select.as_ref(),
            Outlet::All => nested.all.as_ref(),
            Outlet::Excess => nested.excess.as_ref(),
            Outlet::Missing => nested.missing.as_ref(),
        }
    }
}

/// `get` without the not-found: maps, int-keyed vectors/strings
/// (in range), sets (membership), else `None` (§E12). Never throws.
fn lookup(coll: &Value, key: &Value) -> Option<Value> {
    match coll {
        Value::Map(m) => m.get(key).cloned(),
        Value::Vector(v) => match key {
            Value::Int(n) => usize::try_from(*n)
                .ok()
                .and_then(|i| v.iter().nth(i).cloned()),
            _ => None,
        },
        Value::Str(s) => match key {
            Value::Int(n) => usize::try_from(*n)
                .ok()
                .and_then(|i| s.chars().nth(i))
                .map(Value::Char),
            _ => None,
        },
        Value::Set(s) => member(s, key),
        _ => None,
    }
}

/// Set membership: the key when present (`get` returns the equal
/// key; ours is identical by construction).
fn member(s: &clj_value::Set, key: &Value) -> Option<Value> {
    s.contains(key).then(|| key.clone())
}

/// `pr` of a value (assert conditions, `:or` key sets).
fn pr_of(v: &Value) -> String {
    clj_value::pr_to_string(v, &clj_value::PrintOpts::pr())
}

/// Variadic rest: `nil` when empty, else the list (§E5).
fn rest_list(args: &[Value]) -> Value {
    if args.is_empty() {
        Value::Nil
    } else {
        Value::List(List::from_vec(args.to_vec()))
    }
}

/// `(nth coll i nil)`: vectors/lists/strings (OOB is `nil`),
/// `nil` is `nil`, anything else is `nth not supported` (§E12).
fn nth_or_nil(coll: &Value, i: usize, eval: &mut Evaluator) -> Result<Value, Signal> {
    match coll {
        Value::Nil => Ok(Value::Nil),
        Value::Vector(v) => Ok(v.iter().nth(i).cloned().unwrap_or(Value::Nil)),
        Value::List(l) => Ok(l.iter().nth(i).cloned().unwrap_or(Value::Nil)),
        Value::Str(s) => Ok(s.chars().nth(i).map(Value::Char).unwrap_or(Value::Nil)),
        other => Err(eval.throw(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            format!("nth not supported on this type: {}", simple_class_of(other)),
        ))),
    }
}

/// `first`: head element (map entries as 2-vectors), `nil` when
/// empty or unsupported.
fn first_of(coll: &Value) -> Value {
    match coll {
        Value::Nil => Value::Nil,
        Value::Vector(v) => v.iter().next().cloned().unwrap_or(Value::Nil),
        Value::List(l) => l.iter().next().cloned().unwrap_or(Value::Nil),
        Value::Str(s) => s.chars().next().map(Value::Char).unwrap_or(Value::Nil),
        Value::Map(m) => m
            .iter()
            .next()
            .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
            .unwrap_or(Value::Nil),
        Value::Set(s) => s.iter().next().cloned().unwrap_or(Value::Nil),
        _ => Value::Nil,
    }
}

impl Evaluator {
    /// `clojure.lang.RT.seq`: `nil` in, `nil` out; empties to
    /// `nil`; vectors/strings/sets to element lists; maps to
    /// entry lists; anything else is the `ISeq` error.
    fn seq_val(&mut self, value: &Value) -> Result<Value, Signal> {
        match value {
            Value::Nil => Ok(Value::Nil),
            Value::List(l) => {
                if l.is_empty() {
                    Ok(Value::Nil)
                } else {
                    Ok(value.clone())
                }
            }
            Value::Vector(v) => {
                let elems: Vec<Value> = v.iter().cloned().collect();
                Ok(Self::seq_list(elems))
            }
            Value::Set(s) => {
                let elems: Vec<Value> = s.iter().cloned().collect();
                Ok(Self::seq_list(elems))
            }
            Value::Str(s) => {
                let elems: Vec<Value> = s.chars().map(Value::Char).collect();
                Ok(Self::seq_list(elems))
            }
            Value::Map(m) => {
                let elems: Vec<Value> = m
                    .iter()
                    .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
                    .collect();
                Ok(Self::seq_list(elems))
            }
            other => Err(self.throw(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!(
                    "Don't know how to create ISeq from: {}",
                    jvm_class_of(other)
                ),
            ))),
        }
    }

    fn seq_list(elems: Vec<Value>) -> Value {
        if elems.is_empty() {
            Value::Nil
        } else {
            Value::List(List::from_vec(elems))
        }
    }
}

/// `next`: the tail as a list, `nil` when empty or unsupported.
fn next_of(coll: &Value) -> Value {
    let tail: Vec<Value> = match coll {
        Value::Vector(v) => v.iter().skip(1).cloned().collect(),
        Value::List(l) => l.iter().skip(1).cloned().collect(),
        Value::Str(s) => s.chars().skip(1).map(Value::Char).collect(),
        Value::Map(m) => m
            .iter()
            .skip(1)
            .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
            .collect(),
        Value::Set(s) => s.iter().skip(1).cloned().collect(),
        _ => Vec::new(),
    };
    if tail.is_empty() {
        Value::Nil
    } else {
        Value::List(List::from_vec(tail))
    }
}

/// The selection: entry keys, key-list keys, and `&`-tail keys in
/// first-seen order (source order; the JVM iterates a hash-ordered
/// set, §E12 documents the divergence).
fn selection(pat: &MapPattern) -> Vec<Value> {
    let mut sel = Vec::new();
    for (_, key) in &pat.entries {
        if !sel.contains(key) {
            sel.push(key.clone());
        }
    }
    for key in &pat.keys {
        if !sel.contains(&key.key) {
            sel.push(key.key.clone());
        }
    }
    for key in &pat.extra_keys {
        if !sel.contains(key) {
            sel.push(key.clone());
        }
    }
    sel
}

/// `:or` defaults keyed by lookup key (symbols resolve through the
/// local→key map; strict-validated under `:select`/`:all`).
fn or_mapped(pat: &MapPattern, defaults: &Map) -> Map {
    let mut b_to_k: std::collections::HashMap<&str, &Value> = std::collections::HashMap::new();
    for (local, key) in &pat.entries {
        if let Pattern::Symbol(name) = local {
            b_to_k.insert(name.as_str(), key);
        }
    }
    for key in &pat.keys {
        b_to_k.insert(key.local.as_str(), &key.key);
    }
    let mut dm = new_map();
    for (key, _) in defaults.iter() {
        let mapped = match key {
            Value::Symbol(sym) => b_to_k.get(sym.name()).copied().unwrap_or(key),
            _ => key,
        };
        if let Some(val) = defaults.get(key) {
            dm = dm.insert(mapped.clone(), val.clone());
        }
    }
    dm
}

/// Left-biased merge of two maps.
fn merge_maps(a: &Map, b: &Map) -> Map {
    let mut out = a.clone();
    for (k, v) in b.iter() {
        out = out.insert(k.clone(), v.clone());
    }
    out
}

/// `(merge dm m subs?)`.
fn merge3(dm: &Map, converted: &Value, subs: &Option<Map>) -> Map {
    let mut out = dm.clone();
    if let Value::Map(m) = converted {
        out = merge_maps(&out, m);
    }
    if let Some(sub) = subs {
        out = merge_maps(&out, sub);
    }
    out
}

/// `(merge base? subs?)`: `nil` when both are absent.
fn merge_opt(base: Option<&Value>, subs: Option<&Map>) -> Value {
    match (base, subs) {
        (None, None) => Value::Nil,
        (Some(v), None) => v.clone(),
        (None, Some(s)) => Value::Map(s.clone()),
        (Some(Value::Map(m)), Some(s)) => Value::Map(merge_maps(m, s)),
        (Some(v), Some(_)) => v.clone(),
    }
}

/// `(select-keys m sel)` in selection order.
fn select_keys(converted: &Map, sel: &[Value]) -> Value {
    let mut out = new_map();
    for key in sel {
        if let Some(val) = converted.get(key) {
            out = out.insert(key.clone(), val.clone());
        }
    }
    Value::Map(out)
}

/// `(apply dissoc m sel…)`: our maps have no removal, so rebuild
/// (insertion order preserved).
fn dissoc_all(converted: &Value, sel: &[Value]) -> Map {
    let mut out = new_map();
    if let Value::Map(m) = converted {
        for (k, v) in m.iter() {
            if !sel.contains(k) {
                out = out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// The first element of a seqable (`nil` when empty).
fn seq_first(v: &Value) -> Option<Value> {
    match v {
        Value::List(l) => l.iter().next().cloned(),
        Value::Set(s) => s.iter().next().cloned(),
        Value::Str(s) => s.chars().next().map(Value::Char),
        _ => None,
    }
}
