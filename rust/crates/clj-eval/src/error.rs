// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Errors: the analyzer's [`CompileError`] (wrapper + cause, per
//! `docs/t1174/evaluator-contract.md` §E1) and the evaluator's
//! [`Signal`] control flow (thrown values + `recur`).
//!
//! JVM class names for values ([`jvm_class_of`]) and `nth` simple
//! names ([`simple_class_of`]) pin the §E8 rules, including the
//! size-dependent collection names (`AFn.name = getClass().getName()`
//! and `canBePAM`).

use std::sync::Arc;

use clj_reader::Span;
use clj_value::{ThrowClass, Throwable, Value};

/// The wrapper verb: `compiling` for special forms and resolve
/// errors, `macroexpanding` for `let`/`fn` spec failures and
/// `loop`/`letfn` body failures (§E1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Compiling,
    Macroexpanding,
}

impl Verb {
    fn text(self) -> &'static str {
        match self {
            Self::Compiling => "compiling",
            Self::Macroexpanding => "macroexpanding",
        }
    }
}

/// An analyzer failure: JVM class + message + span + wrapper
/// attribution. Renders as the two-line chain `wrapper <= cause`.
#[derive(Clone, Debug)]
pub struct CompileError {
    /// JVM class, e.g. `java.lang.RuntimeException`.
    pub class: &'static str,
    /// The informative cause message, oracle-verbatim.
    pub message: String,
    /// The blamed form's span (reader meta or inherited fallback).
    pub span: Span,
    /// The load path (`:file`), e.g. `NO_SOURCE_PATH`.
    pub source: Arc<str>,
    /// The wrapper verb.
    pub verb: Verb,
    /// Wrapper attribution: the operator as written (`if`), the
    /// resolved macro (`clojure.core/let`), the constant `fn*` for
    /// `set!`-on-local, or `None` for subform errors.
    pub sym: Option<String>,
    /// `Unexpected error` (macro-body `ClassCastException`s) instead
    /// of `Syntax error` (probe25).
    pub unexpected: bool,
    /// Bare failure (no wrapper, just the cause, which escapes
    /// `eval` like any uncaught throwable): macro arities
    /// (probe25), `declare`'s `IObj` cast, stripped macro-arity
    /// errors (probe37/38).
    pub bare: bool,
}

impl CompileError {
    /// Build an error (message is the cause layer).
    #[must_use]
    pub fn new(
        class: &'static str,
        message: String,
        span: Span,
        source: &Arc<str>,
        verb: Verb,
        sym: Option<String>,
    ) -> Self {
        Self {
            class,
            message,
            span,
            source: Arc::clone(source),
            verb,
            sym,
            unexpected: false,
            bare: false,
        }
    }

    /// A bare macro-arity failure (see [`CompileError::bare`]).
    #[must_use]
    pub fn macro_arity(name: &'static str, span: Span, source: &Arc<str>) -> Self {
        let message = NativeError::arity(name, 0).into_inner().message;
        Self {
            class: "clojure.lang.ArityException",
            message,
            span,
            source: Arc::clone(source),
            verb: Verb::Compiling,
            sym: None,
            unexpected: false,
            bare: true,
        }
    }

    /// A bare failure of any class (single-line chain, escaping
    /// `eval` like an uncaught throwable): `declare`'s `IObj` cast
    /// and stripped macro-arity errors.
    #[must_use]
    pub fn bare(class: &'static str, message: String, span: Span, source: &Arc<str>) -> Self {
        Self {
            class,
            message,
            span,
            source: Arc::clone(source),
            verb: Verb::Compiling,
            sym: None,
            unexpected: false,
            bare: true,
        }
    }

    /// The wrapper line, e.g.
    /// `Syntax error compiling if at (src:1:2).` (trailing dot
    /// included, mirroring `Compiler.makeMsg`). An empty source
    /// (string loads) omits the `source:` prefix, like the JVM's
    /// null-source branch.
    #[must_use]
    pub fn wrapper(&self) -> String {
        format!(
            "{} {} {}at ({}).",
            if self.unexpected {
                "Unexpected error"
            } else {
                "Syntax error"
            },
            self.verb.text(),
            self.sym
                .as_deref()
                .map_or(String::new(), |s| format!("{s} ")),
            site_of(&self.source, self.span),
        )
    }

    /// The cause line: `Class | message`.
    #[must_use]
    pub fn cause(&self) -> String {
        format!("{} | {}", self.class, self.message)
    }

    /// The full chain for tests: `[wrapper, cause]`, or `[cause]`
    /// for bare macro-arity failures.
    #[must_use]
    pub fn chain(&self) -> Vec<String> {
        if self.bare {
            vec![self.cause()]
        } else {
            vec![self.wrapper(), self.cause()]
        }
    }
}

/// The `at (...)` site: `source:line:col`, or `line:col` when
/// the source is empty (string loads mirror the JVM's
/// null-source branch, probe36 `loadstring-errspan`).
#[must_use]
pub fn site_of(source: &str, span: Span) -> String {
    if source.is_empty() {
        format!("{}:{}", span.line, span.col)
    } else {
        format!("{source}:{}:{}", span.line, span.col)
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n{}", self.wrapper(), self.cause())
    }
}

impl std::error::Error for CompileError {}

/// Evaluator control signals. `Throw` carries a throwable value up
/// to the nearest matching `catch` (or the top level); `Recur`
/// carries rebind values up to the nearest `loop`/`fn` frame. The
/// analyzer guarantees every `recur` has a frame, so an escaping
/// `Recur` is an internal bug.
#[derive(Clone, Debug)]
pub enum Signal {
    Throw(Throwable),
    Recur(Vec<Value>),
}

/// A runtime failure before identity assignment: native fns and
/// helpers build these, and the evaluator stamps the `id` when
/// converting to [`Signal::Throw`]. Boxed to stay small in
/// `Result` positions (the payload is way past clippy's
/// large-`Err` threshold); field access derefs through.
#[derive(Clone, Debug)]
pub struct NativeError(Box<NativeErrorInner>);

/// The boxed [`NativeError`] payload (fields stay `pub` so
/// deref'd access looks identical). Public only because
/// [`Deref`](std::ops::Deref) names it.
#[derive(Clone, Debug)]
#[doc(hidden)]
pub struct NativeErrorInner {
    pub class: ThrowClass,
    pub message: String,
    pub data: Option<Value>,
    pub cause: Option<Throwable>,
}

impl std::ops::Deref for NativeError {
    type Target = NativeErrorInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for NativeError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

const _: () = assert!(std::mem::size_of::<NativeError>() <= 16);

impl NativeError {
    #[must_use]
    pub fn new(class: ThrowClass, message: String) -> Self {
        Self(Box::new(NativeErrorInner {
            class,
            message,
            data: None,
            cause: None,
        }))
    }

    /// Unbox the payload (for moves the borrow checker cannot see
    /// through the [`Deref`](std::ops::Deref)).
    pub(crate) fn into_inner(self) -> NativeErrorInner {
        *self.0
    }

    /// `Wrong number of args (N) passed to: NAME`, with the JVM's
    /// `> 20` rule past 20 (`ArityException.java`).
    #[must_use]
    pub fn arity(name: &str, actual: usize) -> Self {
        let count = if actual <= 20 {
            actual.to_string()
        } else {
            "> 20".to_owned()
        };
        Self::new(
            ThrowClass::ArityException,
            format!("Wrong number of args ({count}) passed to: {name}"),
        )
    }

    /// The stable cast-failure text (§E8).
    #[must_use]
    pub fn cast(from_jvm: &str, to_jvm: &str) -> Self {
        Self::new(
            ThrowClass::ClassCastException,
            format!("Cannot cast {from_jvm} to {to_jvm}"),
        )
    }
}

/// The JVM class name of a value: cast sources, arity names, and
/// `ISeq` errors (§E8). Collection names are size-dependent like
/// the JVM's (`AFn.name = getClass().getName()`; maps mirror
/// `canBePAM` exactly over insertion order).
#[must_use]
pub fn jvm_class_of(v: &Value) -> String {
    match v {
        Value::Nil => "null".to_owned(),
        Value::Bool(_) => "java.lang.Boolean".to_owned(),
        Value::Int(_) => "java.lang.Long".to_owned(),
        Value::Double(_) => "java.lang.Double".to_owned(),
        Value::Str(_) => "java.lang.String".to_owned(),
        Value::Char(_) => "java.lang.Character".to_owned(),
        Value::Symbol(_) => "clojure.lang.Symbol".to_owned(),
        Value::Keyword(_) => "clojure.lang.Keyword".to_owned(),
        Value::List(l) if l.is_empty() => "clojure.lang.PersistentList$EmptyList".to_owned(),
        Value::List(_) => "clojure.lang.PersistentList".to_owned(),
        Value::Vector(_) => "clojure.lang.PersistentVector".to_owned(),
        Value::Map(m) => map_class_of(m).to_owned(),
        Value::Set(_) => "clojure.lang.PersistentHashSet".to_owned(),
        Value::Regex(_) => "java.util.regex.Pattern".to_owned(),
        Value::TaggedLiteral { .. } => "clojure.lang.TaggedLiteral".to_owned(),
        Value::ReaderConditional { .. } => "clojure.lang.ReaderConditional".to_owned(),
        Value::Var(_) => "clojure.lang.Var".to_owned(),
        Value::Unbound(_) => "clojure.lang.Var$Unbound".to_owned(),
        Value::Throwable(t) => t.class.jvm_name().to_owned(),
        Value::Transient(t) => t.jvm_class().to_owned(),
        Value::Atom(a) => a.jvm_class().to_owned(),
        Value::Delay(d) => d.jvm_class().to_owned(),
        Value::Future(f) => f.jvm_class().to_owned(),
        Value::Promise(p) => p.jvm_class().to_owned(),
        Value::Extension(e) => e.class_name(),
    }
}

/// The reported map class: stored, never derived (promotion is
/// one-way and history-dependent — `dissoc` never demotes and
/// `array-map` forces array at any size).
fn map_class_of(m: &clj_value::Map) -> &'static str {
    if m.is_hashy() {
        "clojure.lang.PersistentHashMap"
    } else {
        "clojure.lang.PersistentArrayMap"
    }
}

/// The `nth not supported on this type: X` name: the JVM's
/// `getSimpleName` rule — strip the package only (`$`-inners stay,
/// so `PersistentList$EmptyList` and `core$map` survive; default
/// packages like `user$eval$fn` have no dots to strip).
#[must_use]
pub fn simple_class_of(v: &Value) -> String {
    let full = jvm_class_of(v);
    full.rsplit('.').next().unwrap_or(&full).to_owned()
}

/// A form's span: `:line`/`:column` reader meta when present, else
/// the inherited fallback (§E1).
#[must_use]
pub fn span_of(form: &Value, fallback: Span) -> Span {
    let line = meta_int(form, "line");
    let col = meta_int(form, "column");
    match (line, col) {
        (Some(line), Some(col)) => Span::new(line, col),
        _ => fallback,
    }
}

fn meta_int(form: &Value, key: &str) -> Option<u32> {
    let meta = form.meta()?;
    let probe = Value::Keyword(clj_value::Keyword::new(None, key));
    match meta.get(&probe) {
        Some(Value::Int(n)) if *n > 0 => u32::try_from(*n).ok(),
        _ => None,
    }
}
