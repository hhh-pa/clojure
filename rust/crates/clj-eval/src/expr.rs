// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! [`Expr`] plus the [`Analyzer`]: reader [`Value`]s become typed
//! expressions, enforcing every §E2–E6/E11 shape rule with
//! oracle-verbatim classes, messages, spans, and wrappers.
//!
//! There are no macros in S4: `let`/`loop`/`letfn`/`fn` analyze
//! directly, replicating the macro layers' observable routing
//! (spec-check then body, `loop`'s emit-then-revalidate, `fn`
//! params through the `clojure.core/let` wrapper) per probes 20–25.

use std::collections::HashMap;
use std::sync::Arc;

use clj_reader::Span;
use clj_value::{
    Keyword, Map, PrintOpts, Symbol, Value, Vector, new_map, pr_to_string, print_to_string,
};

use crate::error::{CompileError, NativeError, Signal, Verb, jvm_class_of, span_of};
use crate::eval::Evaluator;

/// A `catch` clause: resolved class name, local slot, body.
#[derive(Clone, Debug)]
pub struct Catch {
    pub class: String,
    pub slot: usize,
    pub body: Vec<Expr>,
}

/// One `:keys`/`:strs`/`:syms` entry (`!` = required).
#[derive(Clone, Debug)]
pub struct KeyBinding {
    pub local: String,
    pub key: Value,
    pub required: bool,
}

/// A map-destructure pattern (`destmap*`, incl. the 1.13 directives).
#[derive(Clone, Debug)]
pub struct MapPattern {
    /// `(local-pattern, lookup-key)` entries.
    pub entries: Vec<(Pattern, Value)>,
    /// `:keys`/`:strs`/`:syms` (and `!`) bindings.
    pub keys: Vec<KeyBinding>,
    /// Post-`&` key-list keys (selection only, never bound).
    pub extra_keys: Vec<Value>,
    /// `:as` (whole post-conversion map).
    pub as_sym: Option<String>,
    /// `:or` (key as written → default expression, eager).
    pub or: Vec<(Value, Expr)>,
    /// 1.13 outlets (`:defaults` is silently ignored).
    pub select: Option<String>,
    pub all: Option<String>,
    pub excess: Option<String>,
    pub missing: Option<String>,
}

/// A sequential-destructure pattern (`destvec*`).
#[derive(Clone, Debug)]
pub struct VecPattern {
    pub fixed: Vec<Pattern>,
    pub rest: Option<Box<Pattern>>,
    pub as_sym: Option<String>,
}

/// A binding pattern: plain symbol, sequential, or map.
#[derive(Clone, Debug)]
pub enum Pattern {
    Symbol(String),
    Vec(VecPattern),
    Map(MapPattern),
}

/// One `fn` arity: parameters plus the desugared body (leading
/// pre-asserts, then the body — or the post `%` wrapper).
#[derive(Clone, Debug)]
pub struct Arity {
    pub fixed: Vec<Pattern>,
    pub rest: Option<Pattern>,
    pub body: Vec<Expr>,
}

/// An analyzed `fn`: display name plus arities.
#[derive(Clone, Debug)]
pub struct FunExpr {
    /// `user/eval7/fn--8` or `user/eval7/name--9`.
    pub display: String,
    pub arities: Vec<Arity>,
}

/// One `ns` clause (raw unevaluated items, like the macro's
/// quoted args).
#[derive(Clone, Debug)]
pub enum NsClause {
    ReferClojure(Vec<Value>),
    Require(Vec<Value>),
    Use(Vec<Value>),
}

/// A `case*` dispatch mode: `:int` funnels numbers through
/// `intValue` (non-numbers miss, then an `equiv` post-check);
/// `:hash-equiv`/`:hash-identity` hash and post-check (`identity`
/// is `equiv` here — keywords are not interned, and equal keywords
/// must match either way).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseTest {
    Int,
    HashEquiv,
    HashIdentity,
}

/// An analyzed expression.
#[derive(Clone, Debug)]
pub enum Expr {
    /// Self-evaluating (atoms, quotes' values, `()`).
    Literal(Value),
    /// Collection literals evaluate their elements.
    VecLit(Vec<Expr>),
    MapLit(Vec<(Expr, Expr)>),
    SetLit(Vec<Expr>),
    /// A resolved global (qualified `user/x`).
    Global(Symbol),
    /// A local slot index.
    Local(usize),
    /// `(def sym doc? init?)` (created in the ns at analyze time).
    Def {
        sym: Symbol,
        doc: Option<String>,
        init: Option<Box<Expr>>,
        span: Span,
    },
    If {
        test: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    Do(Vec<Expr>),
    Let {
        bindings: Vec<(Pattern, Expr)>,
        body: Vec<Expr>,
    },
    Loop {
        bindings: Vec<(Pattern, Expr)>,
        body: Vec<Expr>,
    },
    LetFn {
        fns: Vec<(String, FunExpr)>,
        body: Vec<Expr>,
    },
    Fn(FunExpr),
    Recur(Vec<Expr>),
    Quote(Value),
    /// `(var sym)` (resolved at analyze time).
    VarRef(Symbol),
    Throw(Box<Expr>),
    /// A freshly constructed throwable (pre/post `assert` failures;
    /// the evaluator stamps the identity).
    NewThrowable {
        class: clj_value::ThrowClass,
        message: String,
    },
    Try {
        body: Vec<Expr>,
        catches: Vec<Catch>,
        finally_body: Vec<Expr>,
    },
    /// `(set! global init)` (locals rejected at analyze time).
    Set {
        sym: Symbol,
        val: Box<Expr>,
    },
    /// `(binding [var init …] body…)` (S4 minimal surface form).
    Binding {
        pairs: Vec<(Symbol, Expr)>,
        body: Vec<Expr>,
    },
    /// `(with-redefs [var init …] body…)` (S5B parallel root
    /// swap; symbols are resolved, like `var`).
    WithRedefs {
        pairs: Vec<(Symbol, Expr)>,
        body: Vec<Expr>,
    },
    /// `(with-bindings map-expr body…)` (S5B).
    WithBindings {
        map: Box<Expr>,
        body: Vec<Expr>,
    },
    /// `(bound-fn name? argv body…)` (S5B: like `fn`, snapshots
    /// the thread bindings at creation).
    BoundFn {
        name: Option<String>,
        fun: FunExpr,
    },
    /// `(defmacro name doc? attr? fdecl)` (S5C: like `def` of a
    /// `fn` with `&form`/`&env` prepended, plus `:macro` and
    /// `:arglists` meta).
    DefMacro {
        sym: Symbol,
        doc: Option<String>,
        attr: Option<Map>,
        arglists: Vec<Value>,
        fun: FunExpr,
        span: Span,
    },
    /// `(declare name…)` (S5C: per-name `def`-with-no-init plus
    /// `:declared`, returning the last var).
    Declare {
        syms: Vec<Symbol>,
        span: Span,
    },
    /// `(ns name doc? attr? clause…)` (S5C: `in-ns`, `resetMeta`,
    /// `refer-clojure`, `:require`/`:use` clauses, mark loaded).
    Ns {
        name: String,
        doc: Option<String>,
        attr: Option<Map>,
        clauses: Vec<NsClause>,
    },
    /// `(assert x)` / `(assert x message)` (S5D analyzer builtin):
    /// `*assert*` was truthy at analysis (else analysis emits
    /// `Literal(Nil)` without touching `test`). `form` is the
    /// `pr-str` of the test form.
    Assert {
        test: Box<Expr>,
        message: Option<Box<Expr>>,
        form: String,
    },
    /// `(time expr)` (S5D analyzer builtin).
    Time {
        expr: Box<Expr>,
    },
    /// `(with-out-str body…)` (S5D analyzer builtin): `out` is the
    /// `*out*` binding target (resolved like `binding`, so user
    /// shadows apply).
    WithOutStr {
        out: Symbol,
        body: Vec<Expr>,
    },
    /// `(lazy-seq* body…)` (S5D analyzer form — the JVM has no such
    /// var; `lazy-seq` expands to this).
    LazySeqStar {
        body: Vec<Expr>,
    },
    /// `(delay* body…)` (S6A analyzer form — the JVM has no such
    /// var; `delay` expands to this). `display` names the trapped
    /// zero-arg closure, as `fn` displays do.
    DelayStar {
        display: String,
        body: Vec<Expr>,
    },
    /// `(case* ge shift mask default imap switch-type test-type
    /// skip?)` (S5D analyzer form, emitted by the `case` macro):
    /// `cases` maps dispatch keys to `(test, then)` (`test` is an
    /// analyzed expr, evaluated on a hash hit like the JVM
    /// `test.emit`); `skip` holds case ints whose `then` is already
    /// a collision-`condp` (no post-check).
    CaseStar {
        expr: Box<Expr>,
        shift: i32,
        mask: i32,
        default: Box<Expr>,
        cases: Vec<(i32, Expr, Expr)>,
        skip: Vec<i32>,
        test: CaseTest,
    },
    Invoke {
        head: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
}

/// JVM classes used by analyzer causes.
mod class {
    pub const RUNTIME: &str = "java.lang.RuntimeException";
    pub const ILLEGAL_ARG: &str = "java.lang.IllegalArgumentException";
    pub const ILLEGAL_STATE: &str = "java.lang.IllegalStateException";
    pub const UNSUPPORTED: &str = "java.lang.UnsupportedOperationException";
    pub const NULL_POINTER: &str = "java.lang.NullPointerException";
    pub const CLASS_CAST: &str = "java.lang.ClassCastException";
    pub const EX_INFO: &str = "clojure.lang.ExceptionInfo";
    pub const EXCEPTION: &str = "java.lang.Exception";
    pub const CLASS_NOT_FOUND: &str = "java.lang.ClassNotFoundException";
}

/// `java.lang` short names that resolve as `catch` classes: the
/// builtin throwable shorts, every other `java.lang` throwable
/// (bulk-verified on the oracle, probe32 — dead handlers in S4 since
/// nothing throws them), and the real-but-unthrowable shorts
/// (accepted, never match — the JVM `VerifyError`s those instead,
/// §E6 documents the divergence).
const JAVA_LANG_CLASSES: &[(&str, &str)] = &[
    ("Throwable", "java.lang.Throwable"),
    ("Exception", "java.lang.Exception"),
    ("Error", "java.lang.Error"),
    ("RuntimeException", "java.lang.RuntimeException"),
    (
        "IllegalArgumentException",
        "java.lang.IllegalArgumentException",
    ),
    ("IllegalStateException", "java.lang.IllegalStateException"),
    (
        "UnsupportedOperationException",
        "java.lang.UnsupportedOperationException",
    ),
    ("AssertionError", "java.lang.AssertionError"),
    ("NullPointerException", "java.lang.NullPointerException"),
    ("ClassCastException", "java.lang.ClassCastException"),
    (
        "IndexOutOfBoundsException",
        "java.lang.IndexOutOfBoundsException",
    ),
    ("ArithmeticException", "java.lang.ArithmeticException"),
    ("InterruptedException", "java.lang.InterruptedException"),
    ("NumberFormatException", "java.lang.NumberFormatException"),
    (
        "IllegalMonitorStateException",
        "java.lang.IllegalMonitorStateException",
    ),
    (
        "IllegalThreadStateException",
        "java.lang.IllegalThreadStateException",
    ),
    (
        "ArrayIndexOutOfBoundsException",
        "java.lang.ArrayIndexOutOfBoundsException",
    ),
    (
        "StringIndexOutOfBoundsException",
        "java.lang.StringIndexOutOfBoundsException",
    ),
    (
        "NegativeArraySizeException",
        "java.lang.NegativeArraySizeException",
    ),
    ("ArrayStoreException", "java.lang.ArrayStoreException"),
    ("NoSuchMethodError", "java.lang.NoSuchMethodError"),
    ("NoSuchFieldError", "java.lang.NoSuchFieldError"),
    ("AbstractMethodError", "java.lang.AbstractMethodError"),
    ("UnsatisfiedLinkError", "java.lang.UnsatisfiedLinkError"),
    (
        "ExceptionInInitializerError",
        "java.lang.ExceptionInInitializerError",
    ),
    ("StackOverflowError", "java.lang.StackOverflowError"),
    ("OutOfMemoryError", "java.lang.OutOfMemoryError"),
    ("ClassFormatError", "java.lang.ClassFormatError"),
    ("NoClassDefFoundError", "java.lang.NoClassDefFoundError"),
    ("VerifyError", "java.lang.VerifyError"),
    ("LinkageError", "java.lang.LinkageError"),
    ("ClassCircularityError", "java.lang.ClassCircularityError"),
    (
        "IncompatibleClassChangeError",
        "java.lang.IncompatibleClassChangeError",
    ),
    ("InstantiationError", "java.lang.InstantiationError"),
    ("InternalError", "java.lang.InternalError"),
    ("UnknownError", "java.lang.UnknownError"),
    ("ThreadDeath", "java.lang.ThreadDeath"),
    (
        "ReflectiveOperationException",
        "java.lang.ReflectiveOperationException",
    ),
    ("ClassNotFoundException", "java.lang.ClassNotFoundException"),
    ("IllegalAccessException", "java.lang.IllegalAccessException"),
    ("InstantiationException", "java.lang.InstantiationException"),
    ("NoSuchMethodException", "java.lang.NoSuchMethodException"),
    ("NoSuchFieldException", "java.lang.NoSuchFieldException"),
    (
        "CloneNotSupportedException",
        "java.lang.CloneNotSupportedException",
    ),
    ("SecurityException", "java.lang.SecurityException"),
    (
        "TypeNotPresentException",
        "java.lang.TypeNotPresentException",
    ),
    (
        "EnumConstantNotPresentException",
        "java.lang.EnumConstantNotPresentException",
    ),
    ("Object", "java.lang.Object"),
    ("String", "java.lang.String"),
    ("Number", "java.lang.Number"),
    ("Boolean", "java.lang.Boolean"),
    ("Character", "java.lang.Character"),
    ("Byte", "java.lang.Byte"),
    ("Short", "java.lang.Short"),
    ("Integer", "java.lang.Integer"),
    ("Long", "java.lang.Long"),
    ("Float", "java.lang.Float"),
    ("Double", "java.lang.Double"),
    ("Void", "java.lang.Void"),
    ("Class", "java.lang.Class"),
    ("Thread", "java.lang.Thread"),
];

/// The span a form binds: only lists rebind it — vectors, maps,
/// sets, and atoms inherit the fallback, like the JVM (which binds
/// `LINE`/`COLUMN` for seq forms only, probe31 `span-*`).
fn analyze_span(form: &Value, fallback: Span) -> Span {
    match form {
        Value::List(_) => span_of(form, fallback),
        _ => fallback,
    }
}

/// A `java.lang` short name's JVM binary name, if it resolves.
fn resolve_java_lang(name: &str) -> Option<&'static str> {
    JAVA_LANG_CLASSES
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, jvm)| *jvm)
}

/// Curated throwable FQNs outside `java.lang` (each oracle-verified,
/// probe32 `catch-fqn-*`): the port has no class loader, so only
/// these resolve; anything else dotted is `ClassNotFoundException`
/// (§E6 fences the exotic remainder).
fn is_curated_fqn(fqn: &str) -> bool {
    matches!(
        fqn,
        "java.io.IOException"
            | "java.io.FileNotFoundException"
            | "java.io.EOFException"
            | "java.io.UncheckedIOException"
            | "java.util.NoSuchElementException"
            | "java.util.concurrent.ExecutionException"
            | "java.util.concurrent.CancellationException"
            | "java.util.concurrent.RejectedExecutionException"
            | "java.lang.reflect.InvocationTargetException"
            | "clojure.lang.Compiler$CompilerException"
    )
}

/// Clojure `str` of a value (`nil` → `""`): the `Unsupported binding
/// form` formatter.
fn str_of(v: &Value) -> String {
    if matches!(v, Value::Nil) {
        String::new()
    } else {
        print_to_string(v)
    }
}

/// Java `String.valueOf`: `nil` → `"null"`, else `str`. The `got:`
/// and `classname:` formatter.
fn value_of(v: &Value) -> String {
    if matches!(v, Value::Nil) {
        "null".to_owned()
    } else {
        str_of(v)
    }
}

/// `pr` of a value (forms, assert conditions, `:or` key sets print
/// readably).
fn pr_of(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Whether a symbol names a special form (unqualified only).
pub(crate) fn is_special(name: &str) -> bool {
    matches!(
        name,
        "def"
            | "if"
            | "do"
            | "let"
            | "let*"
            | "loop"
            | "loop*"
            | "letfn"
            | "fn"
            | "fn*"
            | "recur"
            | "quote"
            | "var"
            | "throw"
            | "try"
            | "set!"
            | "binding"
            | "with-redefs"
            | "with-bindings"
            | "bound-fn"
            | "defmacro"
            | "declare"
            | "ns"
            | "assert"
            | "time"
            | "with-out-str"
            | "lazy-seq*"
            | "delay*"
            | "case*"
    )
}

/// The macro-implemented specials: a local or global binding
/// with one of these names shadows the macro (real specials never
/// shadow, probe31 `shadow-*`). `case*` is not shadowable (the JVM
/// hardcodes it); `lazy-seq*` and `delay*` are (the JVM has no such
/// forms, so a user definition wins there).
fn is_shadowable_macro(name: &str) -> bool {
    matches!(
        name,
        "let"
            | "loop"
            | "letfn"
            | "fn"
            | "binding"
            | "with-redefs"
            | "with-bindings"
            | "bound-fn"
            | "defmacro"
            | "declare"
            | "ns"
            | "assert"
            | "time"
            | "with-out-str"
            | "lazy-seq*"
            | "delay*"
    )
}

/// The analyzer: reader values become [`Expr`]s. Holds the local
/// scope, the `recur` target, the `try` fence, and the live
/// evaluator (namespace resolve checks plus `def` interning like
/// the JVM, macro calls, and the `*ns*`-synced current name).
pub struct Analyzer<'a> {
    source: Arc<str>,
    eval: &'a mut Evaluator,
    locals: Vec<String>,
    recur: Option<usize>,
    no_recur: bool,
    eval_seq: u64,
}

impl<'a> Analyzer<'a> {
    /// Analyze top-level forms from `source` against the evaluator
    /// (`eval_seq` numbers the form for anonymous-`fn` display
    /// names).
    pub fn new(source: &Arc<str>, eval: &'a mut Evaluator, eval_seq: u64) -> Self {
        Self {
            source: Arc::clone(source),
            eval,
            locals: Vec::new(),
            recur: None,
            no_recur: false,
            eval_seq,
        }
    }

    /// The `*ns*`-synced current namespace name (every resolve
    /// check and installation reads this, never the raw registry
    /// current).
    fn current_ns(&self) -> String {
        self.eval.current_ns_name()
    }

    fn err(
        &self,
        class: &'static str,
        message: String,
        span: Span,
        verb: Verb,
        sym: Option<String>,
    ) -> CompileError {
        CompileError::new(class, message, span, &self.source, verb, sym)
    }

    /// Analyze one form (`fallback` = enclosing span; `tail` = tail
    /// position for `recur`).
    pub fn analyze(
        &mut self,
        form: &Value,
        fallback: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let span = analyze_span(form, fallback);
        match form {
            Value::Symbol(sym) => self.analyze_symbol(sym, span),
            Value::List(list) if !list.is_empty() => {
                let items: Vec<&Value> = list.iter().collect();
                self.analyze_list(form, &items, span, tail)
            }
            form if crate::lazy::is_seq_value(form) => self.analyze_seq_form(form, span, tail),
            Value::Vector(v) => self.analyze_vec_lit(v, span),
            Value::Map(m) => self.analyze_map_lit(m, span),
            Value::Set(s) => self.analyze_set_lit(s, span),
            _ => Ok(Expr::Literal(form.clone())),
        }
    }

    /// Any other seq value (macro expansions built at runtime by
    /// `seq`/`concat` arrive as realized or pending seq extensions;
    /// the JVM analyzes every `ISeq` as a form).
    fn analyze_seq_form(
        &mut self,
        form: &Value,
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let items = match crate::lazy::collect_all(self.eval, form) {
            Ok(items) => items,
            Err(Signal::Throw(t)) => {
                return Err(CompileError::bare(
                    t.class.jvm_name(),
                    t.message.as_deref().unwrap_or("").to_owned(),
                    span,
                    &self.source,
                ));
            }
            Err(Signal::Recur(_)) => {
                unreachable!("recur cannot escape forcing a form")
            }
        };
        if items.is_empty() {
            // `EmptyExpr`: an empty form evaluates to itself.
            return Ok(Expr::Literal(form.clone()));
        }
        let refs: Vec<&Value> = items.iter().collect();
        self.analyze_list(form, &refs, span, tail)
    }

    /// A vector literal (elements analyze left to right).
    fn analyze_vec_lit(&mut self, v: &Vector, span: Span) -> Result<Expr, CompileError> {
        let mut out = Vec::with_capacity(v.len());
        for elem in v.iter() {
            out.push(self.analyze(elem, span, false)?);
        }
        Ok(Expr::VecLit(out))
    }

    /// A map literal (keys and values analyze left to right).
    fn analyze_map_lit(&mut self, m: &Map, span: Span) -> Result<Expr, CompileError> {
        let mut out = Vec::with_capacity(m.size());
        for (k, val) in m.iter() {
            out.push((
                self.analyze(k, span, false)?,
                self.analyze(val, span, false)?,
            ));
        }
        Ok(Expr::MapLit(out))
    }

    /// A set literal (elements analyze left to right).
    fn analyze_set_lit(&mut self, s: &clj_value::Set, span: Span) -> Result<Expr, CompileError> {
        let mut out = Vec::with_capacity(s.size());
        for elem in s.iter() {
            out.push(self.analyze(elem, span, false)?);
        }
        Ok(Expr::SetLit(out))
    }

    /// A body: every form non-tail except the last.
    fn analyze_body(
        &mut self,
        forms: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Vec<Expr>, CompileError> {
        let mut out = Vec::with_capacity(forms.len());
        for (i, form) in forms.iter().enumerate() {
            let last = i + 1 == forms.len();
            out.push(self.analyze(form, span, tail && last)?);
        }
        Ok(out)
    }

    /// A local slot by name (innermost wins).
    fn local_idx(&self, name: &str) -> Option<usize> {
        self.locals.iter().rposition(|l| l == name)
    }

    /// Resolve a symbol in value position.
    fn analyze_symbol(&mut self, sym: &Symbol, span: Span) -> Result<Expr, CompileError> {
        if let Some(idx) = self
            .local_idx(sym.name())
            .filter(|_| sym.namespace().is_none())
        {
            return Ok(Expr::Local(idx));
        }
        let cur = self.current_ns();
        if let Some(target) = self.eval.registry.resolve_in(&cur, sym) {
            // Value-position checks (`analyzeSymbol` order: private
            // first, then macro). Neither wrapper names an operator
            // (probed); the private message spells the symbol as
            // written without `#'`, the macro one the `#'` target.
            // (Invoke position has its own check in `analyze_list`.)
            let home = target.namespace().unwrap_or("");
            let id = self
                .eval
                .registry
                .get(home)
                .and_then(|ns| ns.lookup(&target));
            if let Some(id) = id {
                if home != cur && crate::macros::is_private_var(self.eval, id) {
                    return Err(self.err(
                        class::ILLEGAL_STATE,
                        format!("var: {sym} is not public"),
                        span,
                        Verb::Compiling,
                        None,
                    ));
                }
                if crate::macros::is_macro_var(self.eval, id) {
                    return Err(self.err(
                        class::RUNTIME,
                        format!("Can't take value of a macro: #'{}", target),
                        span,
                        Verb::Compiling,
                        None,
                    ));
                }
            }
            return Ok(Expr::Global(target));
        }
        match sym.namespace() {
            // Aliases count as namespaces even when dangling
            // (probe34 `ns-alias-removed`); both miss shapes blame
            // the symbol as written (probe34 `ns-alias-novar`).
            Some(ns) if self.eval.registry.namespace_target_in(&cur, ns).is_none() => Err(self
                .err(
                    class::RUNTIME,
                    format!("No such namespace: {ns}"),
                    span,
                    Verb::Compiling,
                    None,
                )),
            Some(_) => Err(self.err(
                class::RUNTIME,
                format!("No such var: {sym}"),
                span,
                Verb::Compiling,
                None,
            )),
            None => Err(self.err(
                class::RUNTIME,
                format!("Unable to resolve symbol: {sym} in this context"),
                span,
                Verb::Compiling,
                None,
            )),
        }
    }

    /// Analyze a non-empty list: special form, static-`nil` check,
    /// or invocation.
    fn analyze_list(
        &mut self,
        form: &Value,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let head = items[0];
        if let Some(expr) = self.intercept_head(head, items, span, tail)? {
            return Ok(expr);
        }
        // Macro expansion (`analyzeSeq` order: specials, then
        // macros): an expanded form re-analyzes from the top.
        if let Some(expanded) = self.try_expand_macro(form, head, &items[1..], span)? {
            return self.analyze(&expanded, span, tail);
        }
        // Private cross-namespace invocation (the `isInline`
        // analog): the wrapper names the operator as written, the
        // message the `#'`-qualified target. Macros took the
        // expansion path above and never reach this.
        if let Value::Symbol(sym) = head {
            self.check_private_invoke(sym, span)?;
        }
        if matches!(head, Value::Nil) {
            return Err(self.err(
                class::ILLEGAL_ARG,
                format!("Can't call nil, form: {}", pr_of(form)),
                span,
                Verb::Compiling,
                None,
            ));
        }
        let head_expr = self.analyze(head, span, false)?;
        let mut args = Vec::with_capacity(items.len() - 1);
        for arg in &items[1..] {
            args.push(self.analyze(arg, span, false)?);
        }
        Ok(Expr::Invoke {
            head: Box::new(head_expr),
            args,
            span,
        })
    }

    /// Intercept special/builtin heads: bare specials (shadowable
    /// macros defer to any binding; real specials never do), and
    /// `clojure.core`-qualified builtins while the core var is
    /// intact (syntax-quote emits `(clojure.core/let …)`).
    /// Qualified real specials stay var lookups that miss. `Some`
    /// when a form parsed, `None` to continue to expansion.
    fn intercept_head(
        &mut self,
        head: &Value,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Option<Expr>, CompileError> {
        let Value::Symbol(sym) = head else {
            return Ok(None);
        };
        // Shadowable macros defer to any binding (a local, a
        // current-ns global, or a referred one); real specials
        // never do (probe31 `shadow-*`). The builtin's own
        // `clojure.core` macro var is the builtin, not a shadow
        // (only a non-macro re-`def` there shadows).
        if sym.namespace().is_none() && is_special(sym.name()) {
            let shadowed = is_shadowable_macro(sym.name())
                && (self.local_idx(sym.name()).is_some() || self.global_shadows(sym));
            if shadowed {
                return Ok(None);
            }
            let expr = self.analyze_special(sym.name(), items, span, tail)?;
            return Ok(Some(expr));
        }
        if sym.namespace() == Some("clojure.core")
            && is_shadowable_macro(sym.name())
            && self.core_macro_intact(sym)
        {
            let expr = self.analyze_special(sym.name(), items, span, tail)?;
            return Ok(Some(expr));
        }
        Ok(None)
    }

    /// The `&env` for macro calls: nil at top level, else the
    /// in-scope local names mapped to `true` (the JVM's
    /// `LocalBinding` values are unobservable here — keys,
    /// `contains?`, and truthiness behave — fence).
    fn macro_env(&self) -> Value {
        if self.locals.is_empty() {
            Value::Nil
        } else {
            let mut out = new_map();
            for name in &self.locals {
                out = out.insert(Value::Symbol(Symbol::new(None, name)), Value::Bool(true));
            }
            Value::Map(out)
        }
    }

    /// Whether `clojure.core/{name}` still resolves to its
    /// builtin macro var (a re-`def` there falls through to
    /// ordinary invocation).
    fn core_macro_intact(&self, sym: &Symbol) -> bool {
        let cur = self.current_ns();
        let Some(target) = self.eval.registry.resolve_in(&cur, sym) else {
            return false;
        };
        if target.namespace() != Some("clojure.core") || target.name() != sym.name() {
            return false;
        }
        self.eval
            .registry
            .get("clojure.core")
            .and_then(|ns| ns.lookup(&target))
            .is_some_and(|id| crate::macros::is_macro_var(self.eval, id))
    }

    /// Invoke-position private check (the `isInline` analog):
    /// cross-namespace private non-macro vars fail with the
    /// operator as written and the `#'` target (probed).
    fn check_private_invoke(&self, sym: &Symbol, span: Span) -> Result<(), CompileError> {
        let cur = self.current_ns();
        let Some(target) = self.eval.registry.resolve_in(&cur, sym) else {
            return Ok(());
        };
        let home = target.namespace().unwrap_or("");
        if home == cur {
            return Ok(());
        }
        let id = self
            .eval
            .registry
            .get(home)
            .and_then(|ns| ns.lookup(&target));
        let Some(id) = id else {
            return Ok(());
        };
        if crate::macros::is_macro_var(self.eval, id) {
            return Ok(());
        }
        if crate::macros::is_private_var(self.eval, id) {
            return Err(self.err(
                class::ILLEGAL_STATE,
                format!("var: #'{} is not public", target),
                span,
                Verb::Compiling,
                Some(sym.to_string()),
            ));
        }
        Ok(())
    }

    /// Expand a macro-headed list (`isMacro` + call): locals shadow
    /// macros; private cross-namespace macros escape bare (probed:
    /// direct and nested invocations alike surface the raw
    /// `IllegalStateException`).
    fn try_expand_macro(
        &mut self,
        form: &Value,
        head: &Value,
        args: &[&Value],
        span: Span,
    ) -> Result<Option<Value>, CompileError> {
        if matches!(head, Value::Symbol(sym) if sym.namespace().is_none() && self.local_idx(sym.name()).is_some())
        {
            return Ok(None);
        }
        let mac = match crate::macros::lookup_macro(self.eval, head) {
            Err(private) => {
                return Err(CompileError::bare(
                    class::ILLEGAL_STATE,
                    private.into_inner().message,
                    span,
                    &self.source,
                ));
            }
            Ok(mac) => mac,
        };
        let Some(mac) = mac else {
            return Ok(None);
        };
        let env = self.macro_env();
        let arg_vals: Vec<Value> = args.iter().map(|a| (*a).clone()).collect();
        let expanded =
            crate::macros::expand1(self.eval, head, &mac.root, form, &env, &arg_vals, span)?;
        Ok(Some(expanded))
    }

    /// Whether a global binding shadows a shadowable-macro
    /// builtin (the builtin's own `clojure.core` macro var does
    /// not — only locals, other-namespace targets, and non-macro
    /// re-`def`s do).
    fn global_shadows(&self, sym: &Symbol) -> bool {
        let cur = self.current_ns();
        let Some(target) = self.eval.registry.resolve_in(&cur, sym) else {
            return false;
        };
        if target.namespace() == Some("clojure.core") && target.name() == sym.name() {
            let id = self
                .eval
                .registry
                .get("clojure.core")
                .and_then(|ns| ns.lookup(&target));
            return id.is_none_or(|id| !crate::macros::is_macro_var(self.eval, id));
        }
        true
    }

    /// Dispatch one special form (operator already matched).
    fn analyze_special(
        &mut self,
        op: &str,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        // Unspec'd macros with no arguments fail arity, unwrapped
        // (spec'd ones reach their spec/assert checks instead).
        if items.len() == 1 {
            self.check_bare_arity(op, span)?;
        }
        match op {
            "def" => self.parse_def(items, span),
            "if" => self.parse_if(items, span, tail),
            "do" => Ok(Expr::Do(self.analyze_body(&items[1..], span, tail)?)),
            "quote" => self.parse_quote(items, span),
            "var" => self.parse_var(items, span),
            "throw" => self.parse_throw(items, span),
            "recur" => self.parse_recur(items, span, tail),
            "set!" => self.parse_set(items, span),
            "try" => self.parse_try(items, span, tail),
            "let" | "let*" => self.parse_let(op, items, span, tail),
            "loop" | "loop*" => self.parse_let(op, items, span, tail),
            "letfn" => self.parse_letfn(op, items, span, tail),
            "fn" => self.parse_fn(items, span),
            "fn*" => self.parse_fn_star(items, span),
            "binding" => self.parse_binding(items, span, tail),
            "with-redefs" => self.parse_with_redefs(items, span, tail),
            "with-bindings" => self.parse_with_bindings(items, span, tail),
            "bound-fn" => self.parse_bound_fn(items, span),
            "defmacro" => self.parse_defmacro(items, span),
            "declare" => self.parse_declare(items, span),
            "ns" => self.parse_ns(items, span),
            "assert" => self.parse_assert(items, span),
            "time" => self.parse_time(items, span),
            "with-out-str" => self.parse_with_out_str(items, span),
            "lazy-seq*" => self.parse_lazy_seq_star(items, span),
            "delay*" => self.parse_delay_star(items, span),
            "case*" => self.parse_case_star(items, span, tail),
            _ => unreachable!("dispatch covers every special form"),
        }
    }

    /// Bare `(loop)`/`(binding)`/`(letfn)`: the macro fn itself
    /// rejects 0 args before any body runs (probe25).
    fn check_bare_arity(&self, op: &str, span: Span) -> Result<(), CompileError> {
        let name = match op {
            "loop" => "clojure.core/loop",
            "binding" => "clojure.core/binding",
            "letfn" => "clojure.core/letfn",
            "with-redefs" => "clojure.core/with-redefs",
            "with-bindings" => "clojure.core/with-bindings",
            "defmacro" => "clojure.core/defmacro",
            _ => return Ok(()),
        };
        Err(CompileError::macro_arity(name, span, &self.source))
    }

    /// `(def sym doc? init?)`: count routing first (a 3-count with a
    /// non-string second is "Too many", probe25), then the symbol
    /// check. Interns before analyzing the init (self-`def` stays
    /// unbound, probe25).
    /// `def` target rules shared by `def` and `declare`: same-ns
    /// symbols create, other-ns symbols fail — "Can't create defs"
    /// when the var exists, "Can't refer" when it doesn't (probe34
    /// `ns-intern-def`, `ns-crossdef-missing`, probe37
    /// `declare-qualified`).
    fn check_def_target(&self, sym: &Symbol, span: Span) -> Result<(), CompileError> {
        let cur = self.current_ns();
        if sym.namespace().is_some_and(|ns| ns != cur) {
            let message = if self.eval.registry.resolve_in(&cur, sym).is_some() {
                "Can't create defs outside of current ns"
            } else {
                "Can't refer to qualified var that doesn't exist"
            };
            return Err(self.err(
                class::RUNTIME,
                message.to_owned(),
                span,
                Verb::Compiling,
                Some("def".to_owned()),
            ));
        }
        Ok(())
    }

    fn parse_def(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let (sym, doc, init) = self.def_parts(items, span)?;
        // `def` always defines in the current namespace.
        self.check_def_target(sym, span)?;
        let cur = self.current_ns();
        let qualified = self
            .eval
            .registry
            .qualify_in(&cur, sym)
            .with_meta(sym.meta().cloned());
        self.eval.registry.intern(&qualified);
        let init = init
            .map(|form| self.analyze(form, span, false).map(Box::new))
            .transpose()?;
        Ok(Expr::Def {
            sym: qualified,
            doc,
            init,
            span,
        })
    }

    /// `def` arity shapes: bare, init, or docstring + init (a
    /// 3-arg form without a string second arg is `Too many`).
    fn def_parts<'v>(
        &self,
        items: &[&'v Value],
        span: Span,
    ) -> Result<(&'v Symbol, Option<String>, Option<&'v Value>), CompileError> {
        match items.len() - 1 {
            0 => Err(self.def_error("Too few arguments to def", span)),
            1 => Ok((self.def_target(items[1], span)?, None, None)),
            2 => Ok((self.def_target(items[1], span)?, None, Some(items[2]))),
            3 if matches!(items[2], Value::Str(_)) => {
                let Value::Str(doc) = items[2] else {
                    unreachable!("guard matched Str")
                };
                Ok((
                    self.def_target(items[1], span)?,
                    Some(doc.to_string()),
                    Some(items[3]),
                ))
            }
            _ => Err(self.def_error("Too many arguments to def", span)),
        }
    }

    /// The `def` target symbol (anything else is the
    /// `First argument` error).
    fn def_target<'v>(&self, form: &'v Value, span: Span) -> Result<&'v Symbol, CompileError> {
        as_symbol(form)
            .ok_or_else(|| self.def_error("First argument to def must be a Symbol", span))
    }

    /// A `def` (`Compiling`) shape error.
    fn def_error(&self, message: &str, span: Span) -> CompileError {
        self.err(
            class::RUNTIME,
            message.to_owned(),
            span,
            Verb::Compiling,
            Some("def".to_owned()),
        )
    }

    /// `(if test then else?)`.
    fn parse_if(&mut self, items: &[&Value], span: Span, tail: bool) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if argc < 2 {
            return Err(self.err(
                class::RUNTIME,
                "Too few arguments to if".to_owned(),
                span,
                Verb::Compiling,
                Some("if".to_owned()),
            ));
        }
        if argc > 3 {
            return Err(self.err(
                class::RUNTIME,
                "Too many arguments to if".to_owned(),
                span,
                Verb::Compiling,
                Some("if".to_owned()),
            ));
        }
        let test = self.analyze(items[1], span, false)?;
        let then = self.analyze(items[2], span, tail)?;
        let els = match items.get(3) {
            Some(form) => self.analyze(form, span, tail)?,
            None => Expr::Literal(Value::Nil),
        };
        Ok(Expr::If {
            test: Box::new(test),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// `(quote form)`: exactly one argument (`ExceptionInfo`).
    fn parse_quote(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if argc != 1 {
            return Err(self.err(
                class::EX_INFO,
                format!("Wrong number of args ({argc}) passed to quote"),
                span,
                Verb::Compiling,
                Some("quote".to_owned()),
            ));
        }
        Ok(Expr::Quote(items[1].clone()))
    }

    /// Resolve a symbol as a var (shared by `var` and `binding`
    /// var-forms): locals are invisible, refers and other
    /// namespaces are visible (probe34 `ns-var-refer`,
    /// `ns-var-cross`), and every miss is the single `Unable to
    /// resolve var` shape (probe25).
    fn resolve_var(&self, sym: &Symbol, span: Span, op: &str) -> Result<Symbol, CompileError> {
        // Locals are invisible: only namespace membership matters.
        let cur = self.current_ns();
        if let Some(target) = self.eval.registry.resolve_in(&cur, sym) {
            Ok(target)
        } else {
            Err(self.err(
                class::RUNTIME,
                format!("Unable to resolve var: {sym} in this context"),
                span,
                Verb::Compiling,
                Some(op.to_owned()),
            ))
        }
    }

    /// `(var sym)`: missing is the modern-JDK `NullPointerException`,
    /// non-symbols are `ClassCastException`s, extras ignored.
    fn parse_var(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let Some(second) = items.get(1) else {
            return Err(self.err(
                class::NULL_POINTER,
                "Cannot read field \"ns\" because \"sym\" is null".to_owned(),
                span,
                Verb::Compiling,
                Some("var".to_owned()),
            ));
        };
        let Value::Symbol(sym) = second else {
            return Err(self.err(
                class::CLASS_CAST,
                NativeError::cast(&jvm_class_of(second), "clojure.lang.Symbol")
                    .into_inner()
                    .message,
                span,
                Verb::Compiling,
                Some("var".to_owned()),
            ));
        };
        Ok(Expr::VarRef(self.resolve_var(sym, span, "var")?))
    }

    /// `(throw expr)`: exactly one argument.
    fn parse_throw(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if argc != 1 {
            let few = argc < 1;
            return Err(self.err(
                class::RUNTIME,
                format!(
                    "Too {} arguments to throw, throw expects a single Throwable instance",
                    if few { "few" } else { "many" }
                ),
                span,
                Verb::Compiling,
                Some("throw".to_owned()),
            ));
        }
        Ok(Expr::Throw(Box::new(self.analyze(items[1], span, false)?)))
    }

    /// `(recur expr*)`: across-`try` first (beats tail, target,
    /// and arity alike), then tail (targeted only — targetless never
    /// consults it), then arg analysis (failures keep their own
    /// wrappers), then arity (§E2). A targetless `(recur)` is the
    /// constant tail-position error (the JVM hangs; §E2 documents the
    /// divergence).
    fn parse_recur(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if self.no_recur {
            return Err(self.err(
                class::UNSUPPORTED,
                "Cannot recur across try".to_owned(),
                span,
                Verb::Compiling,
                Some("recur".to_owned()),
            ));
        }
        if self.recur.is_some() && !tail {
            return Err(self.tail_err(span));
        }
        let mut args = Vec::with_capacity(argc);
        for arg in &items[1..] {
            args.push(self.analyze(arg, span, false)?);
        }
        let count = self.recur.unwrap_or(0);
        if argc == 0 && self.recur.is_none() {
            return Err(self.tail_err(span));
        }
        if argc != count {
            return Err(self.err(
                class::ILLEGAL_ARG,
                format!("Mismatched argument count to recur, expected: {count} args, got: {argc}"),
                span,
                Verb::Compiling,
                Some("recur".to_owned()),
            ));
        }
        Ok(Expr::Recur(args))
    }

    /// The constant tail-position error.
    fn tail_err(&self, span: Span) -> CompileError {
        self.err(
            class::UNSUPPORTED,
            "Can only recur from tail position".to_owned(),
            span,
            Verb::Compiling,
            Some("recur".to_owned()),
        )
    }

    /// `(set! target val)`: the target analyzes first (failures
    /// propagate with their own wrappers); locals are the constant
    /// `fn*` error; anything analyzed-but-unassignable is `Invalid
    /// assignment target`.
    fn parse_set(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        if items.len() != 3 {
            return Err(self.err(
                class::ILLEGAL_ARG,
                "Malformed assignment, expecting (set! target val)".to_owned(),
                span,
                Verb::Compiling,
                Some("set!".to_owned()),
            ));
        }
        let target = self.analyze(items[1], span, false)?;
        match target {
            Expr::Local(_) => Err(self.err(
                class::ILLEGAL_ARG,
                format!(
                    "Cannot assign to non-mutable: {}",
                    local_name(items[1]).unwrap_or("?")
                ),
                span,
                Verb::Compiling,
                Some("fn*".to_owned()),
            )),
            Expr::Global(sym) => Ok(Expr::Set {
                sym,
                val: Box::new(self.analyze(items[2], span, false)?),
            }),
            _ => Err(self.err(
                class::ILLEGAL_ARG,
                "Invalid assignment target".to_owned(),
                span,
                Verb::Compiling,
                Some("set!".to_owned()),
            )),
        }
    }
}

/// A form as a symbol, if it is one.
fn as_symbol(form: &Value) -> Option<&Symbol> {
    match form {
        Value::Symbol(sym) => Some(sym),
        _ => None,
    }
}

/// The name of a local-target form (always a symbol here).
fn local_name(form: &Value) -> Option<&str> {
    as_symbol(form).map(Symbol::name)
}

/// A plain binding symbol: unqualified and not `&` (`::local-name`).
fn is_local_name(sym: &Symbol) -> bool {
    sym.namespace().is_none() && sym.name() != "&"
}

/// `fn`/`letfn` names: any unqualified symbol — including `&`,
/// which the JVM mangles (`_AMPERSAND_`) but otherwise treats
/// as a name (probe33 `fn-amp-name`, `letfn-amp`).
fn is_fn_name(sym: &Symbol) -> bool {
    sym.namespace().is_none()
}

/// Characters allowed after `&` in key lists are unrestricted; the
/// bindings before it must be idents (symbols or keywords).
fn is_ident(v: &Value) -> bool {
    matches!(v, Value::Symbol(_) | Value::Keyword(_))
}

/// Bound names of a pattern in canonical slot order (entries, then
/// keys, then `:as`/outlets; fixed, then rest, then `:as`). The
/// builder and the evaluator share this order.
pub(crate) fn pattern_names(pat: &Pattern) -> Vec<String> {
    match pat {
        Pattern::Symbol(name) => vec![name.clone()],
        Pattern::Vec(v) => {
            let mut out = Vec::new();
            for fixed in &v.fixed {
                out.extend(pattern_names(fixed));
            }
            if let Some(rest) = &v.rest {
                out.extend(pattern_names(rest));
            }
            out.extend(v.as_sym.clone());
            out
        }
        Pattern::Map(m) => {
            let mut out = Vec::new();
            for (local, _) in &m.entries {
                out.extend(pattern_names(local));
            }
            for key in &m.keys {
                out.push(key.local.clone());
            }
            out.extend(m.as_sym.clone());
            out.extend(m.select.clone());
            out.extend(m.all.clone());
            out.extend(m.excess.clone());
            out.extend(m.missing.clone());
            out
        }
    }
}

impl<'a> Analyzer<'a> {
    /// `(try body* catch* finally?)`: leading non-clauses are the
    /// body; everything after must be clauses. The whole `try` sets
    /// the `recur` fence; catches and `finally` analyze non-tail
    /// (probe25).
    fn parse_try(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let mut splits = items.len();
        for (i, form) in items.iter().enumerate().skip(1) {
            if is_clause(form) {
                splits = i;
                break;
            }
        }
        let saved_recur = self.no_recur;
        self.no_recur = true;
        let body = self.analyze_body(&items[1..splits], span, tail)?;
        // Catch/finally bodies reset the fence and hide the recur
        // target (empty, never `nil`): a `recur` there is always the
        // tail-position error, whatever the outer fence or target
        // (probe30 `fence-catch-*`, `fence-finally-*`). Nested `try`
        // bodies re-arm the fence; nested `fn`/`loop` re-establish a
        // target.
        self.no_recur = false;
        let saved_target = self.recur;
        self.recur = Some(0);
        let mut catches = Vec::new();
        let mut finally_body = Vec::new();
        let mut seen_finally = false;
        for form in &items[splits..] {
            let clause = as_clause(form);
            match clause {
                Some("catch") => {
                    if seen_finally {
                        return Err(self.err(
                            class::RUNTIME,
                            "finally clause must be last in try expression".to_owned(),
                            span,
                            Verb::Compiling,
                            Some("try".to_owned()),
                        ));
                    }
                    catches.push(self.parse_catch(form, span)?);
                }
                Some("finally") => {
                    if seen_finally {
                        return Err(self.err(
                            class::RUNTIME,
                            "finally clause must be last in try expression".to_owned(),
                            span,
                            Verb::Compiling,
                            Some("try".to_owned()),
                        ));
                    }
                    seen_finally = true;
                    let Value::List(list) = form else {
                        unreachable!("clauses are lists")
                    };
                    let parts: Vec<&Value> = list.iter().collect();
                    finally_body = self.analyze_body(&parts[1..], span, false)?;
                }
                _ => {
                    // Past a `finally`, anything is the must-be-last
                    // error; otherwise the only-catch-or-finally one
                    // (probe32 `try-after-*`).
                    let message = if seen_finally {
                        "finally clause must be last in try expression"
                    } else {
                        "Only catch or finally clause can follow catch in try expression"
                    };
                    return Err(self.err(
                        class::RUNTIME,
                        message.to_owned(),
                        span,
                        Verb::Compiling,
                        Some("try".to_owned()),
                    ));
                }
            }
        }
        self.no_recur = saved_recur;
        self.recur = saved_target;
        Ok(Expr::Try {
            body,
            catches,
            finally_body,
        })
    }

    /// One `(catch class binding body*)`. Shape errors blame the
    /// `try` (the clause is parsed inline, without rebinding —
    /// probe25 `catch-nonsym`, probe32 `catch-*-nope`); only the
    /// clause body descends (via [`Analyzer::analyze`]).
    fn parse_catch(&mut self, form: &Value, span: Span) -> Result<Catch, CompileError> {
        let Value::List(list) = form else {
            unreachable!("clauses are lists")
        };
        let parts: Vec<&Value> = list.iter().collect();
        let class = match parts.get(1) {
            None => {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    "Unable to resolve classname: null".to_owned(),
                    span,
                    Verb::Compiling,
                    Some("try".to_owned()),
                ));
            }
            Some(Value::Symbol(sym)) => self.resolve_classname(sym, span)?,
            Some(other) => {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    format!("Unable to resolve classname: {}", value_of(other)),
                    span,
                    Verb::Compiling,
                    Some("try".to_owned()),
                ));
            }
        };
        let binding = match parts.get(2) {
            None => {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    "Bad binding form, expected symbol, got: null".to_owned(),
                    span,
                    Verb::Compiling,
                    Some("try".to_owned()),
                ));
            }
            Some(Value::Symbol(sym)) if sym.namespace().is_some() => {
                return Err(self.err(
                    class::RUNTIME,
                    format!("Can't bind qualified name:{sym}"),
                    span,
                    Verb::Compiling,
                    Some("try".to_owned()),
                ));
            }
            Some(Value::Symbol(sym)) => sym.name().to_owned(),
            Some(other) => {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    format!(
                        "Bad binding form, expected symbol, got: {}",
                        value_of(other)
                    ),
                    span,
                    Verb::Compiling,
                    Some("try".to_owned()),
                ));
            }
        };
        self.locals.push(binding);
        let slot = self.locals.len() - 1;
        let body = self.analyze_body(&parts[3..], span, false)?;
        self.locals.pop();
        Ok(Catch { class, slot, body })
    }

    /// Resolve a `catch` class: default-import shorts (`java.lang`
    /// throwables, the real-but-unthrowable names — which resolve
    /// and never match, the JVM `VerifyError`s those instead — plus
    /// clojure's `ExceptionInfo` short), and dotted FQNs (curated
    /// throwables, `java.lang.*` shorts, the two `clojure.lang.*`
    /// throwables). Unknown shorts are `Unable to resolve
    /// classname`; unknown dotted names are `ClassNotFoundException`
    /// (probe32; §E6).
    fn resolve_classname(&self, sym: &Symbol, span: Span) -> Result<String, CompileError> {
        match sym.namespace() {
            // Default imports (`ArityException` short does NOT
            // resolve — oracle-verified, probe32). Dotted bare names
            // are FQNs: the curated set resolves, `java.lang.*` and
            // `clojure.lang.*` prefixes strip to their short rules,
            // anything else dotted is `ClassNotFoundException`.
            None => {
                if sym.name().contains('.') {
                    return self.resolve_dotted(sym.name(), span);
                }
                match resolve_java_lang(sym.name()) {
                    Some(jvm) => Ok(jvm.to_owned()),
                    None if sym.name() == "ExceptionInfo" => {
                        Ok("clojure.lang.ExceptionInfo".to_owned())
                    }
                    None => Err(self.err(
                        class::ILLEGAL_ARG,
                        format!("Unable to resolve classname: {sym}"),
                        span,
                        Verb::Compiling,
                        Some("try".to_owned()),
                    )),
                }
            }
            // Slash-qualified: dotless namespaces (`q/Nope`) never
            // resolve; dotted ones (`java.lang`, `clojure.lang`,
            // `java.io`, …) join with a dot and run the FQN rule.
            Some(ns) if !ns.contains('.') => Err(self.err(
                class::ILLEGAL_ARG,
                format!("Unable to resolve classname: {sym}"),
                span,
                Verb::Compiling,
                Some("try".to_owned()),
            )),
            Some(ns) => self.resolve_dotted(&format!("{ns}.{}", sym.name()), span),
        }
    }

    /// A dotted fully-qualified classname: the curated set resolves,
    /// `java.lang.*` strips to the short rule, `clojure.lang.*`
    /// takes the two throwables, anything else is
    /// `ClassNotFoundException` (probe32).
    fn resolve_dotted(&self, fqn: &str, span: Span) -> Result<String, CompileError> {
        if is_curated_fqn(fqn) {
            return Ok(fqn.to_owned());
        }
        if let Some(jvm) = fqn.strip_prefix("java.lang.").and_then(resolve_java_lang) {
            return Ok(jvm.to_owned());
        }
        if fqn
            .strip_prefix("clojure.lang.")
            .is_some_and(|short| matches!(short, "ExceptionInfo" | "ArityException"))
        {
            return Ok(fqn.to_owned());
        }
        Err(self.err(
            class::CLASS_NOT_FOUND,
            fqn.to_owned(),
            span,
            Verb::Compiling,
            Some("try".to_owned()),
        ))
    }

    /// `(binding [var init …] body…)`: assert-args shapes, then each
    /// var-form validated exactly like `var` (the macro emits `(var
    /// v)`, probe25).
    fn parse_binding(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let op = match items[0] {
            Value::Symbol(sym) => sym.name().to_owned(),
            _ => unreachable!("dispatched on the operator symbol"),
        };
        let Some(bindings) = items.get(1) else {
            return Err(self.err(
                class::ILLEGAL_ARG,
                "binding requires a vector for its binding".to_owned(),
                span,
                Verb::Macroexpanding,
                Some(op),
            ));
        };
        let Value::Vector(vec) = bindings else {
            return Err(self.err(
                class::ILLEGAL_ARG,
                "binding requires a vector for its binding".to_owned(),
                span,
                Verb::Macroexpanding,
                Some(op),
            ));
        };
        let forms: Vec<&Value> = vec.iter().collect();
        // MSRV 1.85: `usize::is_multiple_of` is 1.87+.
        #[allow(clippy::manual_is_multiple_of)]
        if forms.len() % 2 != 0 {
            return Err(self.err(
                class::ILLEGAL_ARG,
                "binding requires an even number of forms in binding vector".to_owned(),
                span,
                Verb::Macroexpanding,
                Some(op),
            ));
        }
        let mut pairs = Vec::with_capacity(forms.len() / 2);
        // Step indexing, not `chunks_exact` (new clippy wants
        // `as_chunks`, which needs Rust 1.88 > MSRV 1.85).
        for i in (0..forms.len()).step_by(2) {
            let var_sym = match forms[i] {
                Value::Symbol(sym) => self.resolve_var(sym, span, "var")?,
                other => {
                    return Err(self.err(
                        class::CLASS_CAST,
                        NativeError::cast(&jvm_class_of(other), "clojure.lang.Symbol")
                            .into_inner()
                            .message,
                        span,
                        Verb::Compiling,
                        Some("var".to_owned()),
                    ));
                }
            };
            pairs.push((var_sym, self.analyze(forms[i + 1], span, false)?));
        }
        Ok(Expr::Binding {
            pairs,
            body: self.analyze_body(&items[2..], span, tail)?,
        })
    }

    /// `(with-redefs bindings body…)`: bindings seq like the
    /// macro's `zipmap` — vectors, lists, sets, and nil (empty);
    /// maps seq entries, so a non-empty map fails the entry's
    /// `Symbol` cast under `compiling var`; anything else is the
    /// `ISeq` error under `macroexpanding with-redefs`. An odd
    /// trailing binding silently drops (probe37 `wrd-odd`).
    fn parse_with_redefs(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let op = match items[0] {
            Value::Symbol(sym) => sym.name().to_owned(),
            _ => unreachable!("dispatched on the operator symbol"),
        };
        let bindings = items.get(1).copied().unwrap_or(&Value::Nil);
        let forms = self.with_redefs_forms(bindings, span, &op)?;
        let mut pairs = Vec::with_capacity(forms.len() / 2);
        // Step indexing, not `chunks_exact` (new clippy wants
        // `as_chunks`, which needs Rust 1.88 > MSRV 1.85); the
        // `len() - len() % 2` drops an odd tail like `zipmap`.
        for i in (0..forms.len() - forms.len() % 2).step_by(2) {
            let var_sym = match forms[i] {
                Value::Symbol(sym) => self.resolve_var(sym, span, "var")?,
                other => {
                    return Err(self.err(
                        class::CLASS_CAST,
                        NativeError::cast(&jvm_class_of(other), "clojure.lang.Symbol")
                            .into_inner()
                            .message,
                        span,
                        Verb::Compiling,
                        Some("var".to_owned()),
                    ));
                }
            };
            pairs.push((var_sym, self.analyze(forms[i + 1], span, false)?));
        }
        Ok(Expr::WithRedefs {
            pairs,
            body: self.analyze_body(&items[2..], span, tail)?,
        })
    }

    /// The `with-redefs` binding items: vectors, lists, sets, and
    /// nil (empty) seq; a non-empty map fails its first entry's
    /// `Symbol` cast under `compiling var` (entries aren't
    /// symbols); anything else is the `ISeq` error under
    /// `macroexpanding with-redefs`.
    fn with_redefs_forms<'v>(
        &self,
        bindings: &'v Value,
        span: Span,
        op: &str,
    ) -> Result<Vec<&'v Value>, CompileError> {
        match bindings {
            Value::Vector(v) => Ok(v.iter().collect()),
            Value::List(l) => Ok(l.iter().collect()),
            Value::Nil => Ok(Vec::new()),
            Value::Set(s) => Ok(s.iter().collect()),
            Value::Map(m) if m.is_empty() => Ok(Vec::new()),
            Value::Map(_) => Err(self.err(
                class::CLASS_CAST,
                NativeError::cast("clojure.lang.MapEntry", "clojure.lang.Symbol")
                    .into_inner()
                    .message,
                span,
                Verb::Compiling,
                Some("var".to_owned()),
            )),
            other => Err(self.err(
                class::ILLEGAL_ARG,
                format!(
                    "Don't know how to create ISeq from: {}",
                    jvm_class_of(other)
                ),
                span,
                Verb::Macroexpanding,
                Some(op.to_owned()),
            )),
        }
    }

    /// `(with-bindings map-expr body…)`: the map is an evaluated
    /// expression (computed maps work, probe37 `wb-computed-map`).
    /// Bare `(with-bindings)` is intercepted by `check_bare_arity`.
    fn parse_with_bindings(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let Some(map) = items.get(1) else {
            unreachable!("bare with-bindings fails arity before dispatch");
        };
        Ok(Expr::WithBindings {
            map: Box::new(self.analyze(map, span, false)?),
            body: self.analyze_body(&items[2..], span, tail)?,
        })
    }

    /// `(bound-fn name? argv body…)`: parsed exactly like `fn` (the
    /// macro expands to one, probe37 `bf-expansion`) — including
    /// the named single-`letfn` shape, whose slot layout the
    /// evaluator replays with a conveyed closure.
    fn parse_bound_fn(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        match self.parse_fn(items, span)? {
            Expr::Fn(fun) => Ok(Expr::BoundFn { name: None, fun }),
            Expr::LetFn { mut fns, .. } if fns.len() == 1 => {
                let (name, fun) = fns.remove(0);
                Ok(Expr::BoundFn {
                    name: Some(name),
                    fun,
                })
            }
            _ => unreachable!("parse_fn returns Fn or a single LetFn"),
        }
    }

    /// `(defmacro name doc? attr? fdecl)`: the name must be an
    /// unqualified symbol and the fdecl a vector-headed single
    /// arity or list-of-clauses (else the `defmacro` spec shape —
    /// the JVM routes through `defn`, the port names its own
    /// validator); clause *contents* parse through `parse_fn`
    /// with `fn`-flavored errors. `&form`/`&env` prepend
    /// positionally (an explicit leading `&form` shadows, probe38
    /// `83-explicit-ampersand`); the display is the qualified var
    /// name (arity errors name `user/m`, probe38 `06`).
    /// A `defmacro` fdecl's user arglists (without `&form`/`&env`,
    /// for `:arglists`): one vector, or one vector per list clause
    /// (anything else is the `defmacro` spec error).
    fn defmacro_arglists(&self, rest: &[&Value], span: Span) -> Result<Vec<Value>, CompileError> {
        let spec = || self.spec_error("clojure.core/defmacro", span);
        match rest[0] {
            Value::Vector(v) => Ok(vec![Value::Vector(v.clone())]),
            Value::List(_) => {
                let mut out = Vec::with_capacity(rest.len());
                for clause in rest.iter().copied() {
                    match clause {
                        Value::List(l) => match l.first() {
                            Some(Value::Vector(argv)) => {
                                out.push(Value::Vector(argv.clone()));
                            }
                            _ => return Err(spec()),
                        },
                        _ => return Err(spec()),
                    }
                }
                Ok(out)
            }
            _ => Err(spec()),
        }
    }

    fn parse_defmacro(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let spec = || self.spec_error("clojure.core/defmacro", span);
        let Some(name_value) = items.get(1) else {
            return Err(spec());
        };
        let Value::Symbol(name_sym) = name_value else {
            return Err(spec());
        };
        if name_sym.namespace().is_some() {
            return Err(spec());
        }
        let mut rest = &items[2..];
        let mut doc = None;
        if let Some(Value::Str(s)) = rest.first() {
            doc = Some(s.to_string());
            rest = &rest[1..];
        }
        let mut attr = None;
        if let Some(Value::Map(m)) = rest.first() {
            attr = Some((*m).clone());
            rest = &rest[1..];
        }
        if rest.is_empty() {
            return Err(spec());
        }
        // The `:arglists` (user arglists, without `&form`/`&env`).
        let arglists = self.defmacro_arglists(rest, span)?;
        // Clause contents through the `fn` parser stages, with
        // `&form`/`&env` consed onto every arglist *before*
        // analysis (like the JVM `defmacro`), so bodies resolve
        // user params at their shifted slots. The shape above
        // guarantees no leading name symbol, so this always
        // yields a plain `Fn`.
        let owned = self.collect_fn_clauses(rest, span)?;
        let slices: Vec<&[&Value]> = owned.iter().map(Vec::as_slice).collect();
        let mut raw = self.parse_clause_slices(&slices, span)?;
        let ampform = Value::Symbol(Symbol::new(None, "&form"));
        let ampenv = Value::Symbol(Symbol::new(None, "&env"));
        for clause in &mut raw {
            clause.fixed.insert(0, &ampenv);
            clause.fixed.insert(0, &ampform);
        }
        let mut fun = match self.finish_fn(None, &raw, span)? {
            Expr::Fn(fun) => fun,
            _ => unreachable!("defmacro fdecl has no leading name"),
        };
        let cur = self.current_ns();
        fun.display = format!("{cur}/{}", name_sym.name());
        let qualified =
            Symbol::new(Some(&cur), name_sym.name()).with_meta(name_sym.meta().cloned());
        self.eval.registry.intern(&qualified);
        Ok(Expr::DefMacro {
            sym: qualified,
            doc,
            attr,
            arglists,
            fun,
            span,
        })
    }

    /// `(declare name…)`: every name must be a symbol (else the
    /// bare `IObj` cast — the JVM reaches it through `vary-meta`
    /// during expansion, probe37 `declare-num`) and pass the
    /// `def` target rules (probe37 `declare-qualified`).
    fn parse_declare(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let mut syms = Vec::with_capacity(items.len().saturating_sub(1));
        for form in items[1..].iter().copied() {
            let Value::Symbol(sym) = form else {
                return Err(CompileError::bare(
                    class::CLASS_CAST,
                    NativeError::cast(&jvm_class_of(form), "clojure.lang.IObj")
                        .into_inner()
                        .message,
                    span,
                    &self.source,
                ));
            };
            self.check_def_target(sym, span)?;
            // `declare` is `(def name)` per name: qualify and
            // intern exactly like `parse_def`.
            let cur = self.current_ns();
            let qualified = self
                .eval
                .registry
                .qualify_in(&cur, sym)
                .with_meta(sym.meta().cloned());
            self.eval.registry.intern(&qualified);
            syms.push(qualified);
        }
        Ok(Expr::Declare { syms, span })
    }

    /// `(ns name doc? attr? clause…)`: the name must be an
    /// unqualified symbol and every clause a list headed by
    /// `:refer-clojure`/`:require`/`:use` (else the `ns` spec
    /// shape, probe37 `ns-bad-clause`, `ns-num-name`).
    /// `:import`/`:gen-class`/`:load` are spec errors too (N1:
    /// no host classes — loud, never skipped).
    fn parse_ns(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let spec = || self.spec_error("clojure.core/ns", span);
        let Some(name_value) = items.get(1) else {
            return Err(spec());
        };
        let Value::Symbol(name_sym) = name_value else {
            return Err(spec());
        };
        if name_sym.namespace().is_some() {
            return Err(spec());
        }
        let mut rest = &items[2..];
        let mut doc = None;
        if let Some(Value::Str(s)) = rest.first() {
            doc = Some(s.to_string());
            rest = &rest[1..];
        }
        let mut attr = None;
        if let Some(Value::Map(m)) = rest.first() {
            attr = Some((*m).clone());
            rest = &rest[1..];
        }
        let mut clauses = Vec::with_capacity(rest.len());
        for form in rest.iter().copied() {
            let Value::List(l) = form else {
                return Err(spec());
            };
            let mut elems = l.iter();
            match elems.next() {
                Some(Value::Keyword(kw)) if kw.namespace().is_none() => match kw.name() {
                    "refer-clojure" => {
                        clauses.push(NsClause::ReferClojure(elems.cloned().collect()));
                    }
                    "require" => clauses.push(NsClause::Require(elems.cloned().collect())),
                    "use" => clauses.push(NsClause::Use(elems.cloned().collect())),
                    _ => return Err(spec()),
                },
                _ => return Err(spec()),
            }
        }
        Ok(Expr::Ns {
            name: name_sym.name().to_owned(),
            doc,
            attr,
            clauses,
        })
    }
}

/// A `try` clause opener: a list headed by unqualified
/// `catch`/`finally`.
fn is_clause(form: &Value) -> bool {
    as_clause(form).is_some()
}

fn as_clause(form: &Value) -> Option<&str> {
    match form {
        Value::List(list) => match list.first() {
            Some(Value::Symbol(sym)) if sym.namespace().is_none() => match sym.name() {
                "catch" | "finally" => Some(sym.name()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Map directives the spec accepts (`::map-special-binding` plus
/// `::ns-keys`, spec lines 44-50): anything else in keyword position
/// fails `let`/`fn` spec, or is `Unsupported map directive` on the
/// `loop` path. Qualified names are keys/syms only (`:foo/strs` and
/// `:foo/strs!` are not `::ns-keys`).
fn is_known_directive(kw: &Keyword) -> bool {
    if kw.namespace().is_some() {
        return matches!(kw.name(), "keys" | "syms" | "keys!" | "syms!");
    }
    matches!(
        kw.name(),
        "as" | "or"
            | "keys"
            | "syms"
            | "strs"
            | "keys!"
            | "syms!"
            | "strs!"
            | "select"
            | "excess"
            | "missing"
            | "defaults"
            | "all"
    )
}

/// The deep `::binding-form` spec mirror: plain symbol, sequential
/// (`forms*` + optional `& rest` + optional `:as sym`), or map
/// (binding entries + known directives with shaped values).
fn spec_valid_binding(form: &Value) -> bool {
    match form {
        Value::Symbol(sym) => is_local_name(sym),
        Value::Vector(v) => spec_valid_seq(&v.iter().collect::<Vec<_>>()),
        Value::Map(m) => spec_valid_map(m),
        _ => false,
    }
}

fn spec_valid_seq(forms: &[&Value]) -> bool {
    let Some(rest) = strip_spec_as(forms) else {
        return false;
    };
    let Some(rest) = strip_spec_rest(rest) else {
        return false;
    };
    // Fixed forms, none `&`/`:as`.
    rest.iter().all(|f| match f {
        Value::Symbol(sym) if sym.namespace().is_none() && sym.name() == "&" => false,
        Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == "as" => false,
        other => spec_valid_binding(other),
    })
}

/// Strip an optional trailing `:as sym` (`None` = spec failure).
fn strip_spec_as<'a, 'v>(forms: &'a [&'v Value]) -> Option<&'a [&'v Value]> {
    let as_at_end = forms.len() >= 2
        && matches!(forms[forms.len() - 2], Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == "as");
    if !as_at_end {
        return Some(forms);
    }
    if !matches!(forms[forms.len() - 1], Value::Symbol(sym) if is_local_name(sym)) {
        return None;
    }
    Some(&forms[..forms.len() - 2])
}

/// Strip an optional `& rest` (`None` = dangling/invalid `&`).
fn strip_spec_rest<'a, 'v>(forms: &'a [&'v Value]) -> Option<&'a [&'v Value]> {
    let Some(pos) = forms.iter().position(
        |f| matches!(f, Value::Symbol(sym) if sym.namespace().is_none() && sym.name() == "&"),
    ) else {
        return Some(forms);
    };
    if pos + 1 != forms.len() - 1 {
        return None;
    }
    if !spec_valid_binding(forms[pos + 1]) {
        return None;
    }
    Some(&forms[..pos])
}

fn spec_valid_map(m: &Map) -> bool {
    for (k, v) in m.iter() {
        if let Value::Keyword(kw) = k {
            if !spec_valid_directive(kw, v) {
                return false;
            }
        } else if !spec_valid_binding(k) {
            return false;
        }
    }
    true
}

/// One directive's value shape (`::as`/`::or`/`::keys-form`/…).
/// Qualified `::ns-keys` take `::simple-syms-form` regardless of
/// kind (simple symbols only — stricter than unqualified `:keys`
/// idents or `:syms` symbols).
fn spec_valid_directive(kw: &Keyword, v: &Value) -> bool {
    if !is_known_directive(kw) {
        return false;
    }
    if kw.namespace().is_some() {
        return spec_valid_key_list(v, KeyKind::Strs);
    }
    match kw.name() {
        "as" | "select" | "excess" | "missing" | "defaults" | "all" => {
            matches!(v, Value::Symbol(sym) if is_local_name(sym))
        }
        "or" => matches!(v, Value::Map(_)),
        "keys" | "keys!" => spec_valid_key_list(v, KeyKind::Keys),
        "syms" | "syms!" => spec_valid_key_list(v, KeyKind::Syms),
        "strs" | "strs!" => spec_valid_key_list(v, KeyKind::Strs),
        _ => unreachable!("directive set checked above"),
    }
}

#[derive(Clone, Copy)]
enum KeyKind {
    Keys,
    Syms,
    Strs,
}

/// `::keys-form`: idents, then an optional `&` tail of anything
/// (dangling `&` is fine, probe25).
fn spec_valid_key_list(v: &Value, kind: KeyKind) -> bool {
    let Value::Vector(list) = v else {
        return false;
    };
    let forms: Vec<&Value> = list.iter().collect();
    let tail = match forms.iter().position(
        |f| matches!(f, Value::Symbol(sym) if sym.namespace().is_none() && sym.name() == "&"),
    ) {
        None => &forms[..],
        Some(pos) => &forms[..pos],
    };
    tail.iter().all(|f| match kind {
        KeyKind::Keys => is_ident(f),
        KeyKind::Syms => matches!(f, Value::Symbol(_)),
        KeyKind::Strs => matches!(f, Value::Symbol(sym) if is_local_name(sym)),
    })
}

/// A destructure-body failure before wrapper attribution: the
/// `destructure`/`destvec*`/`destmap*` layer runs at macroexpansion,
/// so `Exception`/`IllegalArgumentException` render `Syntax error`
/// while anything else (our `ClassCastException`s) renders
/// `Unexpected error` (probe25).
struct WalkError {
    class: &'static str,
    message: String,
}

impl WalkError {
    fn unsupported(form: &Value) -> Self {
        Self {
            class: class::EXCEPTION,
            message: format!("Unsupported binding form: {}", str_of(form)),
        }
    }

    fn iseq(class_name: &str) -> Self {
        Self {
            class: class::ILLEGAL_ARG,
            message: format!("Don't know how to create ISeq from: {class_name}"),
        }
    }
}

/// Whether a value seqs (`keys`/key-lists accept these; anything
/// else is the `ISeq` error).
fn is_seqable(v: &Value) -> bool {
    matches!(
        v,
        Value::Nil
            | Value::List(_)
            | Value::Vector(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::Str(_)
    )
}

/// The directive kind of a `:keys`-family keyword: `(tr bb)`
/// mapping plus requiredness. Qualified `:ns/keys` carry their ns
/// (probe25); anything else is `Unsupported map directive`.
struct KeyDirective {
    kind: KeyKind,
    ns: Option<String>,
    required: bool,
}

fn as_key_directive(kw: &Keyword) -> Option<KeyDirective> {
    let name = kw.name();
    let (base, required) = match name.strip_suffix('!') {
        Some(base) => (base, true),
        None => (name, false),
    };
    let kind = if base.starts_with("keys") {
        KeyKind::Keys
    } else if base.starts_with("syms") {
        KeyKind::Syms
    } else if base.starts_with("strs") {
        KeyKind::Strs
    } else {
        return None;
    };
    Some(KeyDirective {
        kind,
        ns: kw.namespace().map(str::to_owned),
        required,
    })
}

/// Validate one binding form the way `destructure` does (the `loop`
/// path and the shared semantic layer): symbols pass through, nested
/// shapes recurse, anything else is `Unsupported binding form`.
fn walk_binding(form: &Value) -> Result<(), WalkError> {
    match form {
        Value::Symbol(_) => Ok(()),
        Value::Vector(v) => walk_seq(&v.iter().collect::<Vec<_>>()),
        Value::Map(m) => walk_map(m),
        _ => Err(WalkError::unsupported(form)),
    }
}

/// `destvec*`: fixed shapes recurse; `&` takes the next form as the
/// rest pattern (dangling `&` walks `nil`); after `&` only `:as`
/// may follow; `:as` validates its value (dangling walks `nil`)
/// immediately — before emission — then ends the scan.
fn walk_seq(forms: &[&Value]) -> Result<(), WalkError> {
    let mut rest = forms;
    let mut seen_rest = false;
    while let Some((first, tail)) = rest.split_first() {
        if is_amp(first) {
            let second = tail.first().copied().unwrap_or(&Value::Nil);
            walk_binding(second)?;
            rest = tail.get(1..).unwrap_or(&[]);
            seen_rest = true;
        } else if is_as(first) {
            // The `:as` value walks as a binding form right away
            // (probe31 `loop-as-*`): scalars fail here, while shapes
            // pass and die at the emitted `let`'s spec instead.
            let value = tail.first().copied().unwrap_or(&Value::Nil);
            walk_binding(value)?;
            return Ok(());
        } else {
            if seen_rest {
                return Err(WalkError {
                    class: class::EXCEPTION,
                    message: "Unsupported binding form, only :as can follow & parameter".to_owned(),
                });
            }
            walk_binding(first)?;
            rest = tail;
        }
    }
    Ok(())
}

fn is_amp(v: &Value) -> bool {
    matches!(v, Value::Symbol(sym) if sym.namespace().is_none() && sym.name() == "&")
}

fn is_as(v: &Value) -> bool {
    matches!(v, Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == "as")
}

/// `destmap*` validation: directives known, `:or` seqable,
/// key-lists well-formed, `:or` semantics (`Multiple`, strict).
fn walk_map(m: &Map) -> Result<(), WalkError> {
    // `(keys defaults)`: maps are defaults, `nil` is none, empty
    // seqables proceed default-less, nonempty seqables throw on
    // their first element (`key` assumes entries), anything else is
    // the `ISeq` error (probe25).
    let or_map = match map_get(m, "or") {
        None | Some(Value::Nil) => None,
        Some(Value::Map(or)) => Some(or),
        Some(other) if !is_seqable(other) => {
            return Err(WalkError::iseq(&jvm_class_of(other)));
        }
        Some(other) => match seq_values(other).first() {
            None => None,
            Some(first) => {
                return Err(WalkError {
                    class: class::CLASS_CAST,
                    message: NativeError::cast(&jvm_class_of(first), "java.util.Map$Entry")
                        .into_inner()
                        .message,
                });
            }
        },
    };
    let mut sel: Vec<Value> = Vec::new();
    let mut bound_keys: HashMap<String, Value> = HashMap::new();
    for (k, v) in m.iter() {
        walk_map_pair(k, v, or_map, &mut sel, &mut bound_keys)?;
    }
    check_strict_or(m, or_map, &sel, &bound_keys)?;
    Ok(())
}

/// One map-pattern pair: outlet directives pass, key-list
/// directives validate, binding entries recurse.
fn walk_map_pair(
    k: &Value,
    v: &Value,
    or_map: Option<&Map>,
    sel: &mut Vec<Value>,
    bound_keys: &mut HashMap<String, Value>,
) -> Result<(), WalkError> {
    if let Value::Keyword(kw) = k {
        if is_outlet_directive(kw) {
            return Ok(());
        }
        let Some(dir) = as_key_directive(kw) else {
            return Err(WalkError {
                class: class::EXCEPTION,
                message: format!("Unsupported map directive: {kw}"),
            });
        };
        return walk_key_list(v, kw, &dir, or_map, sel, bound_keys);
    }
    walk_binding(k)?;
    sel.push(v.clone());
    if let Value::Symbol(local) = k {
        bound_keys.insert(local.name().to_owned(), v.clone());
        check_or_pair(or_map, local.name(), v, false)?;
    }
    Ok(())
}

/// One `:keys`-family list: pre-`&` idents bind (non-`Named`s are
/// the `ClassCastException`, probe25), a second `&` or a post-`&`
/// symbol is the `IllegalArgumentException`, post-`&` forms join
/// the selection.
fn walk_key_list(
    v: &Value,
    dir_kw: &Keyword,
    dir: &KeyDirective,
    or_map: Option<&Map>,
    sel: &mut Vec<Value>,
    bound_keys: &mut HashMap<String, Value>,
) -> Result<(), WalkError> {
    if !is_seqable(v) {
        return Err(WalkError::iseq(&jvm_class_of(v)));
    }
    let forms: Vec<Value> = seq_values(v);
    let mut preamp = true;
    for form in &forms {
        if is_amp(form) {
            if !preamp {
                return Err(WalkError {
                    class: class::ILLEGAL_ARG,
                    message: format!("& can only appear once in {dir_kw}"),
                });
            }
            preamp = false;
            continue;
        }
        if !preamp {
            if let Value::Symbol(sym) = form {
                return Err(WalkError {
                    class: class::ILLEGAL_ARG,
                    message: format!(
                        "'{}' - binding symbols can only appear before '&', use keys after",
                        sym.name()
                    ),
                });
            }
            sel.push(form.clone());
            continue;
        }
        // `:strs` never casts (`core.clj:4556` bare `str`):
        // vectors and maps take the `push1`/`pb` recurse —
        // validated as nested patterns here, rejected by the spec
        // downstream (probe33 `strs-shape-*`). The `sel` push is
        // unobservable (the spec always fails after), so only
        // validation runs.
        if matches!(dir.kind, KeyKind::Strs) && matches!(form, Value::Vector(_) | Value::Map(_)) {
            walk_binding(form)?;
            continue;
        }
        let (key, local) = key_for(dir, form)?;
        sel.push(key.clone());
        bound_keys.insert(local.clone(), key.clone());
        check_or_pair(or_map, &local, &key, dir.required)?;
    }
    Ok(())
}

/// `(tr bb)`: the lookup key plus the bound local for one
/// pre-`&` element. Only idents pass (probe32): keywords ride
/// the fast path untouched, and the local is the plain name.
/// Anything else takes the `(namespace x)` cast — except under
/// `:strs`, which never casts (see `key_cast_err`).
fn key_for(dir: &KeyDirective, form: &Value) -> Result<(Value, String), WalkError> {
    let (elem_ns, name) = match form {
        Value::Symbol(s) => (s.namespace().map(str::to_owned), s.name().to_owned()),
        Value::Keyword(k) => (k.namespace().map(str::to_owned), k.name().to_owned()),
        _ => return Err(key_cast_err(dir, form)),
    };
    let key = match dir.kind {
        KeyKind::Keys => {
            let ns = dir.ns.clone().or(elem_ns);
            Value::Keyword(Keyword::new(ns.as_deref(), &name))
        }
        KeyKind::Syms => {
            let ns = dir.ns.clone().or(elem_ns);
            Value::Symbol(Symbol::new(ns.as_deref(), &name))
        }
        KeyKind::Strs => Value::Str(Arc::from(name.as_str())),
    };
    Ok((key, name))
}

/// The `(tr bb)` cast for a non-ident key-list element
/// (probe32): `ClassCastException … to clojure.lang.Named`,
/// the helpful-NPE text for `nil` — but `:strs` never casts
/// and reports `Unsupported binding form` (str-rendered)
/// instead, and qualified directives short-circuit the
/// namespace call (`core.clj:4554` `(or mkns (namespace %))`):
/// only `(name %)` runs, so strings fall into `push1`/`pb`
/// (`Unsupported`) and `nil` takes the `getName` NPE text
/// (probe33 `qual-cast-*`).
fn key_cast_err(dir: &KeyDirective, form: &Value) -> WalkError {
    if matches!(dir.kind, KeyKind::Strs) {
        return WalkError::unsupported(form);
    }
    if dir.ns.is_some() && matches!(form, Value::Str(_)) {
        return WalkError::unsupported(form);
    }
    if matches!(form, Value::Nil) {
        let method = if dir.ns.is_some() {
            "getName"
        } else {
            "getNamespace"
        };
        return WalkError {
            class: class::NULL_POINTER,
            message: format!(
                "Cannot invoke \"clojure.lang.Named.{method}()\" because \"x\" is null"
            ),
        };
    }
    WalkError {
        class: class::CLASS_CAST,
        message: NativeError::cast(&jvm_class_of(form), "clojure.lang.Named")
            .into_inner()
            .message,
    }
}

/// `:or` semantics for one binding (compile time): `Multiple :or
/// defaults` when both the local and the key have defaults, `Can't
/// supply default value for required key` when a required binding
/// has either.
fn check_or_pair(
    or_map: Option<&Map>,
    local: &str,
    key: &Value,
    required: bool,
) -> Result<(), WalkError> {
    let Some(or) = or_map else {
        return Ok(());
    };
    let local_key = Value::Symbol(Symbol::new(None, local));
    let has_local = or.get(&local_key).is_some();
    let has_key = or.get(key).is_some();
    if has_local && has_key {
        return Err(WalkError {
            class: class::EXCEPTION,
            message: format!(
                "Multiple :or defaults for same key: {} '{local}'",
                str_of(key)
            ),
        });
    }
    if required && (has_local || has_key) {
        return Err(WalkError {
            class: class::EXCEPTION,
            message: format!(
                "Can't supply default value for required key: {}",
                pr_of(key)
            ),
        });
    }
    Ok(())
}

/// Strict `:or` with `:select`/`:all` (compile time).
fn check_strict_or(
    m: &Map,
    or_map: Option<&Map>,
    sel: &[Value],
    bound_keys: &HashMap<String, Value>,
) -> Result<(), WalkError> {
    let has_select = map_get(m, "select").is_some();
    let has_all = map_get(m, "all").is_some();
    if !has_select && !has_all {
        return Ok(());
    }
    let Some(or) = or_map else {
        return Ok(());
    };
    let mut mapped: Vec<Value> = Vec::new();
    for (k, _) in or.iter() {
        if let Value::Symbol(sym) = k {
            match bound_keys.get(sym.name()) {
                Some(key) => mapped.push(key.clone()),
                None => {
                    return Err(WalkError {
                        class: class::ILLEGAL_ARG,
                        message: format!("symbol {sym} in :or does not refer to a binding"),
                    });
                }
            }
        } else {
            mapped.push((*k).clone());
        }
    }
    let extras: Vec<&Value> = mapped.iter().filter(|k| !sel.contains(k)).collect();
    if !extras.is_empty() {
        let mut set = clj_value::new_set();
        for k in extras {
            set = set.insert((*k).clone());
        }
        return Err(WalkError {
            class: class::ILLEGAL_ARG,
            message: format!("keys {} appear only in :or", pr_of(&Value::Set(set))),
        });
    }
    Ok(())
}

fn map_get<'m>(m: &'m Map, key: &str) -> Option<&'m Value> {
    let probe = Value::Keyword(Keyword::new(None, key));
    m.get(&probe)
}

/// One raw `fn` clause (or `letfn` fnspec tail clause): parameter
/// forms plus pre/post/body forms.
struct RawClause<'v> {
    fixed: Vec<&'v Value>,
    rest: Option<&'v Value>,
    pre: Vec<Value>,
    post: Vec<Value>,
    body: Vec<&'v Value>,
}

/// One validated `letfn` fnspec: the name plus its tail forms.
struct FnSpec {
    name: String,
    tail: Vec<Value>,
}

impl<'a> Analyzer<'a> {
    /// The `CheckSpecs` failure for `let`/`fn`.
    fn spec_error(&self, macro_name: &str, span: Span) -> CompileError {
        self.err(
            class::EX_INFO,
            format!("Call to {macro_name} did not conform to spec."),
            span,
            Verb::Macroexpanding,
            Some(macro_name.to_owned()),
        )
    }

    /// A bare analyzer-builtin arity failure (`assert`/`time`
    /// reject 0 and 3+ args like the JVM macros — the `ArityException`
    /// escapes `eval` unwrapped).
    fn builtin_arity(&self, name: &'static str, argc: usize, span: Span) -> CompileError {
        CompileError::bare(
            "clojure.lang.ArityException",
            NativeError::arity(name, argc).into_inner().message,
            span,
            &self.source,
        )
    }

    /// `(assert x)` / `(assert x message)`: the expansion-time
    /// `*assert*` check reads the DYNAMIC value (a `binding` around
    /// `eval` disables, corpus 15 `:assert-bound-false-eval`); when
    /// off, the test is neither analyzed nor evaluated (the
    /// expansion is `nil`). A missing/unbound `*assert*` reads
    /// truthy (assert on).
    fn parse_assert(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if argc != 1 && argc != 2 {
            return Err(self.builtin_arity("clojure.core/assert", argc, span));
        }
        let sym = Symbol::new(Some("clojure.core"), "*assert*");
        let on = self
            .eval
            .registry
            .get("clojure.core")
            .and_then(|ns| ns.lookup(&sym))
            .is_none_or(|id| self.eval.deref_var(id, &sym).is_truthy());
        if !on {
            return Ok(Expr::Literal(Value::Nil));
        }
        let test = self.analyze(items[1], span, false)?;
        let message = match items.get(2) {
            Some(m) => Some(Box::new(self.analyze(m, span, false)?)),
            None => None,
        };
        Ok(Expr::Assert {
            test: Box::new(test),
            message,
            form: pr_to_string(items[1], &PrintOpts::pr()),
        })
    }

    /// `(time expr)`: exactly one arg (else the bare macro arity).
    /// The expr is not in tail position (it binds `ret#` on the
    /// JVM), so an inner `recur` is the tail-position error.
    fn parse_time(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let argc = items.len() - 1;
        if argc != 1 {
            return Err(self.builtin_arity("clojure.core/time", argc, span));
        }
        Ok(Expr::Time {
            expr: Box::new(self.analyze(items[1], span, false)?),
        })
    }

    /// `(with-out-str body…)`: the body never runs in tail position
    /// (the JVM expansion ends in `(str s#)`). `*out*` resolves like
    /// a `binding` target, so user shadows apply.
    fn parse_with_out_str(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let body = self.analyze_body(&items[1..], span, false)?;
        Ok(Expr::WithOutStr {
            out: Symbol::new(None, "*out*"),
            body,
        })
    }

    /// `(lazy-seq* body…)`: a zero-arg `fn` boundary for `recur`
    /// (an empty `recur` re-invokes the body at force time, like the
    /// JVM closure; anything else is the mismatch error).
    fn parse_lazy_seq_star(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let saved_recur = self.recur;
        let saved_no_recur = self.no_recur;
        self.recur = Some(0);
        self.no_recur = false;
        let body = self.analyze_body(&items[1..], span, true);
        self.recur = saved_recur;
        self.no_recur = saved_no_recur;
        Ok(Expr::LazySeqStar { body: body? })
    }

    /// `(delay* body…)`: a zero-arg `fn` boundary for `recur` (like
    /// `lazy-seq*` — the body runs at force time, as the JVM's
    /// `(fn* [] …)` does). The display names the trapped closure.
    fn parse_delay_star(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let seq = self.eval.next_fn_seq();
        let display = format!("user/eval{}/delay--{seq}", self.eval_seq);
        let saved_recur = self.recur;
        let saved_no_recur = self.no_recur;
        self.recur = Some(0);
        self.no_recur = false;
        let body = self.analyze_body(&items[1..], span, true);
        self.recur = saved_recur;
        self.no_recur = saved_no_recur;
        Ok(Expr::DelayStar {
            display,
            body: body?,
        })
    }

    /// `(case* ge shift mask default imap switch-type test-type
    /// skip?)`: the `case` macro's expansion shape (anything else is
    /// the spec error — hand-written `case*` beyond the macro shape
    /// is a port-level error). `default`/`then`s inherit tail
    /// position; `test`s analyze as exprs (evaluated on a hash hit,
    /// like the JVM `test.emit`); `skip` defaults to empty.
    fn parse_case_star(
        &mut self,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        if items.len() != 8 && items.len() != 9 {
            return Err(self.spec_error("clojure.core/case*", span));
        }
        let (shift, mask, imap, test) = self.parse_case_star_head(items, span)?;
        let skip = self.parse_case_star_skip(items, span)?;
        let expr = self.analyze(items[1], span, false)?;
        let default = self.analyze(items[4], span, tail)?;
        let cases = self.parse_case_star_cases(imap, span, tail)?;
        Ok(Expr::CaseStar {
            expr: Box::new(expr),
            shift,
            mask,
            default: Box::new(default),
            cases,
            skip,
            test,
        })
    }

    /// A `case*` integer slot (`i32`-ranged `Int`, else `None`).
    fn case_star_int(v: &Value) -> Option<i32> {
        match v {
            Value::Int(i) => i32::try_from(*i).ok(),
            _ => None,
        }
    }

    /// The `case*` header: shift, mask, the case map, the test kind,
    /// and the switch-kind check (all spec errors).
    fn parse_case_star_head<'v>(
        &self,
        items: &[&'v Value],
        span: Span,
    ) -> Result<(i32, i32, &'v Map, CaseTest), CompileError> {
        let (Some(shift), Some(mask)) =
            (Self::case_star_int(items[2]), Self::case_star_int(items[3]))
        else {
            return Err(self.spec_error("clojure.core/case*", span));
        };
        let Value::Map(imap) = items[5] else {
            return Err(self.spec_error("clojure.core/case*", span));
        };
        let test = self.parse_case_star_test(items[7], span)?;
        if !matches!(items[6], Value::Keyword(kw) if kw.namespace().is_none() && matches!(kw.name(), "sparse" | "compact"))
        {
            return Err(self.spec_error("clojure.core/case*", span));
        }
        Ok((shift, mask, imap, test))
    }

    /// The `case*` test kind (`:int` / `:hash-equiv` /
    /// `:hash-identity`, unqualified).
    fn parse_case_star_test(&self, v: &Value, span: Span) -> Result<CaseTest, CompileError> {
        match v {
            Value::Keyword(kw) if kw.namespace().is_none() => match kw.name() {
                "int" => Ok(CaseTest::Int),
                "hash-equiv" => Ok(CaseTest::HashEquiv),
                "hash-identity" => Ok(CaseTest::HashIdentity),
                _ => Err(self.spec_error("clojure.core/case*", span)),
            },
            _ => Err(self.spec_error("clojure.core/case*", span)),
        }
    }

    /// The optional `case*` skip set (absent or `nil` means empty).
    fn parse_case_star_skip(&self, items: &[&Value], span: Span) -> Result<Vec<i32>, CompileError> {
        let mut skip = Vec::new();
        if let Some(s) = items.get(8).copied()
            && !matches!(s, Value::Nil)
        {
            let Value::Set(set) = s else {
                return Err(self.spec_error("clojure.core/case*", span));
            };
            for v in set.iter() {
                let Some(k) = Self::case_star_int(v) else {
                    return Err(self.spec_error("clojure.core/case*", span));
                };
                skip.push(k);
            }
        }
        Ok(skip)
    }

    /// The `case*` case map: dispatch keys to `(test, then)` (test
    /// constants are literal data — probed: symbols, lists, and
    /// defined-var names all match as data, never evaluated; only
    /// the `then` is an expression).
    fn parse_case_star_cases(
        &mut self,
        imap: &Map,
        span: Span,
        tail: bool,
    ) -> Result<Vec<(i32, Expr, Expr)>, CompileError> {
        let mut cases = Vec::new();
        for (k, v) in imap.iter() {
            let Some(key) = Self::case_star_int(k) else {
                return Err(self.spec_error("clojure.core/case*", span));
            };
            let Value::Vector(pair) = v else {
                return Err(self.spec_error("clojure.core/case*", span));
            };
            let mut pair_forms = pair.iter();
            let (Some(test_form), Some(then_form), None) =
                (pair_forms.next(), pair_forms.next(), pair_forms.next())
            else {
                return Err(self.spec_error("clojure.core/case*", span));
            };
            let test_expr = Expr::Literal(test_form.clone());
            let then_expr = self.analyze(then_form, span, tail)?;
            cases.push((key, test_expr, then_expr));
        }
        Ok(cases)
    }

    /// Attribute a destructure-body failure: `Syntax error` for
    /// `Exception`/`IllegalArgumentException` causes, `Unexpected
    /// error` otherwise (mirroring the macroexpand `catch` ladder).
    fn walk_error(&self, err: WalkError, span: Span, op: &str) -> CompileError {
        let mut e = self.err(
            err.class,
            err.message,
            span,
            Verb::Macroexpanding,
            Some(op.to_owned()),
        );
        e.unexpected = !matches!(
            err.class,
            class::EXCEPTION | class::ILLEGAL_ARG | class::ILLEGAL_STATE | class::EX_INFO
        );
        e
    }

    /// `(let [pat init …] body…)` / `(loop …)`: `let` runs the deep
    /// spec first, then the shared walk (as-written wrapper);
    /// `loop` runs asserts, the walk (`loop` wrapper), the emission
    /// re-validation (inner-`let` spec), or the `loop*` star checks.
    fn parse_let(
        &mut self,
        op: &str,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let is_loop = op == "loop" || op == "loop*";
        let forms = self.let_binding_forms(op, items, span)?;
        let pats: Vec<&Value> = forms.iter().step_by(2).copied().collect();
        self.validate_let_pats(op, &pats, span)?;
        let saved_recur = self.recur;
        if is_loop {
            self.recur = None;
        }
        let base = self.locals.len();
        let mut bindings_out = Vec::with_capacity(pats.len());
        for i in (0..forms.len()).step_by(2) {
            let init = self.analyze(forms[i + 1], span, false)?;
            let (pat, names) = self.build_pattern(forms[i], span)?;
            self.locals.extend(names);
            bindings_out.push((pat, init));
        }
        // A `loop` body resets the try/recur fence; the inits keep
        // the outer fence (probe30 `fence-loop-*`). The body is
        // self-tail (like `fn`): a `loop` in non-tail position still
        // takes `recur` (probe30 `fence-loop-in-catch`).
        let saved_no_recur = self.no_recur;
        if is_loop {
            self.recur = Some(pats.len());
            self.no_recur = false;
        }
        let body_tail = if is_loop { true } else { tail };
        let body = self.analyze_body(&items[2..], span, body_tail)?;
        self.locals.truncate(base);
        self.recur = saved_recur;
        self.no_recur = saved_no_recur;
        if is_loop {
            Ok(Expr::Loop {
                bindings: bindings_out,
                body,
            })
        } else {
            Ok(Expr::Let {
                bindings: bindings_out,
                body,
            })
        }
    }

    /// The `let`/`loop`/`let*`/`loop*` bindings vector,
    /// shape-checked: star shapes fail `Bad binding form`, macro
    /// shapes route to the spec/assert checks.
    fn let_binding_forms<'v>(
        &self,
        op: &str,
        items: &[&'v Value],
        span: Span,
    ) -> Result<Vec<&'v Value>, CompileError> {
        let is_loop = op == "loop" || op == "loop*";
        let is_star = matches!(op, "let*" | "loop*");
        let bindings = items.get(1).copied().unwrap_or(&Value::Nil);
        let Value::Vector(vec) = bindings else {
            if is_star {
                return Err(self.star_shape_error(op, "expected vector", span));
            }
            return Err(self.shape_error(is_loop, "a vector for its binding", span, op));
        };
        let forms: Vec<&Value> = vec.iter().collect();
        // MSRV 1.85: `usize::is_multiple_of` is 1.87+.
        #[allow(clippy::manual_is_multiple_of)]
        if forms.len() % 2 != 0 {
            if is_star {
                return Err(self.star_shape_error(
                    op,
                    "expected matched symbol expression pairs",
                    span,
                ));
            }
            return Err(self.shape_error(
                is_loop,
                "an even number of forms in binding vector",
                span,
                op,
            ));
        }
        Ok(forms)
    }

    /// Binding-pattern validation for `parse_let`: the star
    /// checks, the `loop` checks, or the `let` spec+walk.
    fn validate_let_pats(&self, op: &str, pats: &[&Value], span: Span) -> Result<(), CompileError> {
        if matches!(op, "let*" | "loop*") {
            self.validate_star_bindings(pats, span, op)
        } else if op == "loop" {
            self.validate_loop_bindings(pats, span, op)
        } else {
            self.validate_let_bindings(pats, span, op)
        }
    }

    /// `let` validation: deep spec first, then the shared walk
    /// (as-written wrapper).
    fn validate_let_bindings(
        &self,
        pats: &[&Value],
        span: Span,
        op: &str,
    ) -> Result<(), CompileError> {
        for pat in pats {
            if !spec_valid_binding(pat) {
                return Err(self.spec_error("clojure.core/let", span));
            }
        }
        for pat in pats {
            if let Err(e) = walk_binding(pat) {
                return Err(self.walk_error(e, span, op));
            }
        }
        Ok(())
    }

    /// `loop` validation: asserts, the walk (`loop` wrapper), then
    /// the emission re-validation (inner-`let` spec) for
    /// destructuring patterns, or the `loop*` star checks for
    /// all-symbol bindings.
    fn validate_loop_bindings(
        &self,
        pats: &[&Value],
        span: Span,
        op: &str,
    ) -> Result<(), CompileError> {
        for pat in pats {
            if let Err(e) = walk_binding(pat) {
                return Err(self.walk_error(e, span, op));
            }
        }
        if pats.iter().any(|p| !matches!(p, Value::Symbol(_))) {
            for pat in pats {
                if !spec_valid_binding(pat) {
                    return Err(self.spec_error("clojure.core/let", span));
                }
            }
            return Ok(());
        }
        self.validate_star_bindings(pats, span, "loop*")
    }

    /// Raw `let*`/`loop*` binding checks (probe33): symbols only
    /// (`Bad binding form, expected symbol, got: <pr>`), no `&`,
    /// no qualified names.
    fn validate_star_bindings(
        &self,
        pats: &[&Value],
        span: Span,
        op: &str,
    ) -> Result<(), CompileError> {
        for pat in pats {
            let Value::Symbol(sym) = pat else {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    format!("Bad binding form, expected symbol, got: {}", pr_of(pat)),
                    span,
                    Verb::Compiling,
                    Some(op.to_owned()),
                ));
            };
            if is_amp(pat) {
                return Err(self.err(
                    class::RUNTIME,
                    "Can't use & as a local binding".to_owned(),
                    span,
                    Verb::Compiling,
                    Some(op.to_owned()),
                ));
            }
            if sym.namespace().is_some() {
                return Err(self.err(
                    class::RUNTIME,
                    format!("Can't let qualified name: {sym}"),
                    span,
                    Verb::Compiling,
                    Some(op.to_owned()),
                ));
            }
        }
        Ok(())
    }

    /// A raw `let*`/`loop*` bindings-shape error (`Compiling`).
    fn star_shape_error(&self, op: &str, what: &str, span: Span) -> CompileError {
        self.err(
            class::ILLEGAL_ARG,
            format!("Bad binding form, {what}"),
            span,
            Verb::Compiling,
            Some(op.to_owned()),
        )
    }

    /// The bindings-shape failure: spec for `let`, assert-args for
    /// `loop` (volatile `in NS:LINE` suffix dropped, §E4).
    fn shape_error(&self, is_loop: bool, what: &str, span: Span, op: &str) -> CompileError {
        if is_loop {
            self.err(
                class::ILLEGAL_ARG,
                format!("loop requires {what}"),
                span,
                Verb::Macroexpanding,
                Some(op.to_owned()),
            )
        } else {
            self.spec_error("clojure.core/let", span)
        }
    }

    /// Build a validated pattern, analyzing `:or` defaults in the
    /// current scope. Names come from the shared
    /// [`pattern_names`] order.
    fn build_pattern(
        &mut self,
        form: &Value,
        span: Span,
    ) -> Result<(Pattern, Vec<String>), CompileError> {
        let pat = match form {
            Value::Symbol(sym) => Pattern::Symbol(sym.name().to_owned()),
            Value::Vector(v) => {
                let forms: Vec<&Value> = v.iter().collect();
                self.build_seq(&forms, span)?
            }
            Value::Map(m) => self.build_map(m, span)?,
            _ => unreachable!("patterns validated before building"),
        };
        let names = pattern_names(&pat);
        Ok((pat, names))
    }

    /// Split a validated sequential pattern (`&` rest, trailing
    /// `:as`; junk after `:as` ignored like the walk).
    fn build_seq(&mut self, forms: &[&Value], span: Span) -> Result<Pattern, CompileError> {
        let mut fixed = Vec::new();
        let mut rest_pat = None;
        let mut as_sym = None;
        let mut rest = forms;
        while let Some((first, tail)) = rest.split_first() {
            if is_amp(first) {
                let second = tail.first().copied().unwrap_or(&Value::Nil);
                let (pat, _) = self.build_pattern(second, span)?;
                rest_pat = Some(Box::new(pat));
                rest = tail.get(1..).unwrap_or(&[]);
            } else if is_as(first) {
                if let Some(Value::Symbol(sym)) = tail.first() {
                    as_sym = Some(sym.name().to_owned());
                }
                break;
            } else {
                let (pat, _) = self.build_pattern(first, span)?;
                fixed.push(pat);
                rest = tail;
            }
        }
        Ok(Pattern::Vec(VecPattern {
            fixed,
            rest: rest_pat,
            as_sym,
        }))
    }

    /// Build a validated map pattern, analyzing `:or` defaults.
    fn build_map(&mut self, m: &Map, span: Span) -> Result<Pattern, CompileError> {
        let mut acc = MapAcc::default();
        for (k, v) in m.iter() {
            self.build_map_pair(k, v, span, &mut acc)?;
        }
        let or = self.build_or(m, span)?;
        Ok(Pattern::Map(MapPattern {
            entries: acc.entries,
            keys: acc.keys,
            extra_keys: acc.extra_keys,
            as_sym: acc.as_sym,
            or,
            select: acc.select,
            all: acc.all,
            excess: acc.excess,
            missing: acc.missing,
        }))
    }

    /// One map-pattern pair: outlet directive, key-list
    /// directive, or a binding entry.
    fn build_map_pair(
        &mut self,
        k: &Value,
        v: &Value,
        span: Span,
        acc: &mut MapAcc,
    ) -> Result<(), CompileError> {
        if let Value::Keyword(kw) = k {
            if is_outlet_directive(kw) {
                set_outlet(kw, v, acc);
            } else if let Some(dir) = as_key_directive(kw) {
                build_keys(v, &dir, &mut acc.keys, &mut acc.extra_keys);
            }
            return Ok(());
        }
        let (pat, _) = self.build_pattern(k, span)?;
        acc.entries.push((pat, v.clone()));
        Ok(())
    }

    /// The analyzed `:or` defaults (eager at runtime).
    fn build_or(&mut self, m: &Map, span: Span) -> Result<Vec<(Value, Expr)>, CompileError> {
        let mut or = Vec::new();
        if let Some(Value::Map(or_map)) = map_get(m, "or") {
            for (k, default) in or_map.iter() {
                or.push(((*k).clone(), self.analyze(default, span, false)?));
            }
        }
        Ok(or)
    }
}

/// A map pattern under construction.
#[derive(Default)]
struct MapAcc {
    entries: Vec<(Pattern, Value)>,
    keys: Vec<KeyBinding>,
    extra_keys: Vec<Value>,
    as_sym: Option<String>,
    select: Option<String>,
    all: Option<String>,
    excess: Option<String>,
    missing: Option<String>,
}

/// Unqualified `:as`/`:or`/outlet directives (validation ran
/// already; unknown directives never reach the builder).
fn is_outlet_directive(kw: &Keyword) -> bool {
    kw.namespace().is_none()
        && matches!(
            kw.name(),
            "as" | "or" | "select" | "excess" | "missing" | "defaults" | "all"
        )
}

/// Record one outlet binding (`:or`/`:defaults` carry no local).
fn set_outlet(kw: &Keyword, v: &Value, acc: &mut MapAcc) {
    let slot = match kw.name() {
        "as" => &mut acc.as_sym,
        "select" => &mut acc.select,
        "all" => &mut acc.all,
        "excess" => &mut acc.excess,
        "missing" => &mut acc.missing,
        _ => return,
    };
    *slot = as_symbol(v).map(|s| s.name().to_owned());
}

/// One validated key-list: pre-`&` elements bind, post-`&`
/// elements join the selection.
fn build_keys(
    v: &Value,
    dir: &KeyDirective,
    keys: &mut Vec<KeyBinding>,
    extra_keys: &mut Vec<Value>,
) {
    let mut preamp = true;
    for form in seq_values(v) {
        if is_amp(&form) {
            preamp = false;
            continue;
        }
        if !preamp {
            extra_keys.push(form);
            continue;
        }
        if let Ok((key, local)) = key_for(dir, &form) {
            keys.push(KeyBinding {
                local,
                key,
                required: dir.required,
            });
        }
    }
}

/// The unrealistically generous `seq` for key-lists (only the
/// seqable check matters; elements drive validation).
fn seq_values(v: &Value) -> Vec<Value> {
    match v {
        Value::Nil => Vec::new(),
        Value::List(l) => l.iter().cloned().collect(),
        Value::Vector(v) => v.iter().cloned().collect(),
        Value::Map(m) => m
            .iter()
            .map(|(k, v)| Value::Vector(clj_value::Vector::from_vec(vec![k.clone(), v.clone()])))
            .collect(),
        Value::Set(s) => s.iter().cloned().collect(),
        Value::Str(s) => s.chars().map(Value::Char).collect(),
        _ => Vec::new(),
    }
}

/// Split parameter forms into fixed + optional rest (`&` must be
/// second-to-last; `fn` params take no `:as`). `None` = spec failure.
fn split_params<'v>(forms: &[&'v Value]) -> Option<(Vec<&'v Value>, Option<&'v Value>)> {
    match forms.iter().position(|f| is_amp(f)) {
        None => {
            if forms.iter().any(|f| matches!(f, Value::Keyword(_))) {
                return None;
            }
            Some((forms.to_vec(), None))
        }
        Some(pos) => {
            if pos + 1 != forms.len() - 1 {
                return None;
            }
            if forms[..pos].iter().any(|f| matches!(f, Value::Keyword(_))) {
                return None;
            }
            Some((forms[..pos].to_vec(), Some(forms[pos + 1])))
        }
    }
}

impl<'a> Analyzer<'a> {
    /// `(fn name? params body* | name? (params body*)+)`: the deep
    /// spec first, then per-clause validation in source order
    /// (20-count, parameter walk under the emitted `let`'s wrapper,
    /// body, dup checks; fixed-vs-variadic last).
    fn parse_fn(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let mut rest = &items[1..];
        let mut name: Option<String> = None;
        match rest.split_first() {
            Some((Value::Symbol(sym), tail))
                if tail
                    .first()
                    .is_some_and(|f| matches!(f, Value::Vector(_) | Value::List(_))) =>
            {
                if !is_fn_name(sym) {
                    return Err(self.spec_error("clojure.core/fn", span));
                }
                name = Some(sym.name().to_owned());
                rest = tail;
            }
            _ => {}
        }
        let owned = self.collect_fn_clauses(rest, span)?;
        let slices: Vec<&[&Value]> = owned.iter().map(Vec::as_slice).collect();
        let raw = self.parse_clause_slices(&slices, span)?;
        self.finish_fn(name, &raw, span)
    }

    /// `(fn* name? params body* | name? (params body*)+)`: raw
    /// `FnExpr` semantics — no spec, no destructuring, no
    /// pre/post. A leading symbol is always the name (any symbol;
    /// the simple name binds for self-recursion, probe33
    /// `fn*-selfsimple`); a leading vector is the single clause,
    /// otherwise every item is a list clause. `nil` passes the
    /// seq cast and dies at the arglist-NPE like `()` (probe33
    /// `fn*-nil`); anything else is the `ISeq` CCE.
    fn parse_fn_star(&mut self, items: &[&Value], span: Span) -> Result<Expr, CompileError> {
        let mut rest = &items[1..];
        let mut name: Option<String> = None;
        if let Some((Value::Symbol(sym), tail)) = rest.split_first() {
            name = Some(sym.name().to_owned());
            rest = tail;
        }
        let owned: Vec<Vec<&Value>> = if rest.is_empty() {
            Vec::new()
        } else if matches!(rest.first(), Some(Value::Vector(_))) {
            vec![rest.to_vec()]
        } else {
            let mut out = Vec::with_capacity(rest.len());
            for clause in rest {
                match clause {
                    Value::List(list) => out.push(list.iter().collect()),
                    Value::Nil => out.push(Vec::new()),
                    other => {
                        return Err(self.fn_star_error(
                            class::CLASS_CAST,
                            &NativeError::cast(&jvm_class_of(other), "clojure.lang.ISeq")
                                .into_inner()
                                .message,
                            span,
                        ));
                    }
                }
            }
            out
        };
        let slices: Vec<&[&Value]> = owned.iter().map(Vec::as_slice).collect();
        let raw = self.parse_star_clauses(&slices, span)?;
        self.finish_fn(name, &raw, span)
    }

    /// Shared `fn`/`fn*` tail: display name, the self-recursion
    /// slot, per-clause analysis, the `LetFn` wrap for names.
    fn finish_fn(
        &mut self,
        name: Option<String>,
        raw: &[RawClause],
        span: Span,
    ) -> Result<Expr, CompileError> {
        let seq = self.eval.next_fn_seq();
        let display = match &name {
            Some(n) => format!("user/eval{}/{}--{seq}", self.eval_seq, n),
            None => format!("user/eval{}/fn--{seq}", self.eval_seq),
        };
        let base = self.locals.len();
        let name_slot = if let Some(n) = &name {
            self.locals.push(n.clone());
            Some(self.locals.len() - 1)
        } else {
            None
        };
        let arities = self.analyze_arities(raw, span)?;
        self.locals.truncate(base);
        let fun = FunExpr { display, arities };
        // A named `fn` is a one-function `letfn`: the frame extends
        // first, the closure captures it, then the slot fills
        // (self-recursion). A `let` wrap would capture the
        // pre-extension frame instead.
        match (name, name_slot) {
            (Some(n), Some(slot)) => Ok(Expr::LetFn {
                fns: vec![(n, fun)],
                body: vec![Expr::Local(slot)],
            }),
            _ => Ok(Expr::Fn(fun)),
        }
    }

    /// One raw `fn*` clause per slice: the params vector plus the
    /// whole body (no pre/post extraction — a leading map is a
    /// body form, probe33 `fn*-pre`). Missing params are the
    /// arglist-NPE; non-vectors are the `IPersistentVector` CCE.
    fn parse_star_clauses<'v>(
        &self,
        clauses: &[&[&'v Value]],
        span: Span,
    ) -> Result<Vec<RawClause<'v>>, CompileError> {
        let mut raw = Vec::with_capacity(clauses.len());
        for clause in clauses {
            let Some((params, body)) = clause.split_first() else {
                return Err(self.fn_star_error(
                    class::NULL_POINTER,
                    "Cannot invoke \"clojure.lang.IPersistentVector.count()\" because \"arglist\" is null",
                    span,
                ));
            };
            let Value::Vector(vec) = params else {
                return Err(self.fn_star_error(
                    class::CLASS_CAST,
                    &NativeError::cast(&jvm_class_of(params), "clojure.lang.IPersistentVector")
                        .message,
                    span,
                ));
            };
            let param_forms: Vec<&Value> = vec.iter().collect();
            let (fixed, rest) = self.star_params(&param_forms, span)?;
            raw.push(RawClause {
                fixed,
                rest,
                pre: Vec::new(),
                post: Vec::new(),
                body: body.to_vec(),
            });
        }
        Ok(raw)
    }

    /// Raw `fn*` parameter splitting (probe33): a trailing `&`
    /// drops (fixed arity); `&` as the rest name, or after the
    /// rest, is `Invalid parameter list`; a plain parameter after
    /// the rest is `Unexpected parameter`.
    fn star_params<'v>(
        &self,
        forms: &[&'v Value],
        span: Span,
    ) -> Result<(Vec<&'v Value>, Option<&'v Value>), CompileError> {
        let mut fixed = Vec::new();
        let mut rest = None;
        let mut i = 0;
        while i < forms.len() {
            let form = forms[i];
            if is_amp(form) {
                if rest.is_some() {
                    return Err(self.fn_star_error(class::RUNTIME, "Invalid parameter list", span));
                }
                i += 1;
                match forms.get(i) {
                    None => {}
                    Some(f) if is_amp(f) => {
                        return Err(self.fn_star_error(
                            class::RUNTIME,
                            "Invalid parameter list",
                            span,
                        ));
                    }
                    Some(f) => {
                        self.check_star_param(f, span)?;
                        rest = Some(*f);
                    }
                }
            } else if rest.is_some() {
                return Err(self.fn_star_error(class::RUNTIME, "Unexpected parameter", span));
            } else {
                self.check_star_param(form, span)?;
                fixed.push(form);
            }
            i += 1;
        }
        Ok((fixed, rest))
    }

    /// One raw `fn*` parameter: symbols only, unqualified.
    fn check_star_param(&self, form: &Value, span: Span) -> Result<(), CompileError> {
        let Value::Symbol(sym) = form else {
            return Err(self.fn_star_error(class::ILLEGAL_ARG, "fn params must be Symbols", span));
        };
        if sym.namespace().is_some() {
            return Err(self.fn_star_error(
                class::RUNTIME,
                &format!("Can't use qualified name as parameter: {sym}"),
                span,
            ));
        }
        Ok(())
    }

    /// A `fn*` (`Compiling`) shape error.
    fn fn_star_error(&self, class: &'static str, message: &str, span: Span) -> CompileError {
        self.err(
            class,
            message.to_owned(),
            span,
            Verb::Compiling,
            Some("fn*".to_owned()),
        )
    }

    /// Collect one clause per arity: a leading vector is the
    /// single clause, otherwise every item is a list clause
    /// (non-lists and empty arities fail spec).
    fn collect_fn_clauses<'v>(
        &self,
        rest: &[&'v Value],
        span: Span,
    ) -> Result<Vec<Vec<&'v Value>>, CompileError> {
        if matches!(rest.first(), Some(Value::Vector(_))) {
            return Ok(vec![rest.to_vec()]);
        }
        let mut out = Vec::with_capacity(rest.len());
        for clause in rest {
            let Value::List(list) = clause else {
                return Err(self.spec_error("clojure.core/fn", span));
            };
            out.push(list.iter().collect());
        }
        if out.is_empty() {
            return Err(self.spec_error("clojure.core/fn", span));
        }
        Ok(out)
    }

    /// One clause's parameter walk (the fn macro's
    /// `maybe-destructured` runs before `FnExpr`): carries the
    /// emitted `let`'s wrapper.
    fn walk_clause(&self, clause: &RawClause, span: Span) -> Result<(), CompileError> {
        for pat in clause.fixed.iter().chain(clause.rest.iter()) {
            if let Err(e) = walk_binding(pat) {
                return Err(self.walk_error(e, span, "clojure.core/let"));
            }
        }
        Ok(())
    }

    /// Per-clause `fn*` validation in source order: 20-count, the
    /// parameter walk, the body, then the dup/variadic checks;
    /// fixed-vs-variadic last (probe32 `fn-*`).
    fn analyze_arities(
        &mut self,
        raw: &[RawClause],
        span: Span,
    ) -> Result<Vec<Arity>, CompileError> {
        let mut arities = Vec::with_capacity(raw.len());
        let mut fixed_counts = std::collections::HashSet::new();
        let mut variadic_fixed: Option<usize> = None;
        for clause in raw {
            if clause.fixed.len() > 20 {
                return Err(self.err(
                    class::RUNTIME,
                    "Can't specify more than 20 params".to_owned(),
                    span,
                    Verb::Compiling,
                    Some("fn*".to_owned()),
                ));
            }
            self.walk_clause(clause, span)?;
            let arity = self.analyze_clause(clause, span)?;
            if clause.rest.is_some() {
                if variadic_fixed.is_some() {
                    return Err(self.err(
                        class::RUNTIME,
                        "Can't have more than 1 variadic overload".to_owned(),
                        span,
                        Verb::Compiling,
                        Some("fn*".to_owned()),
                    ));
                }
                variadic_fixed = Some(clause.fixed.len());
            } else if !fixed_counts.insert(clause.fixed.len()) {
                return Err(self.err(
                    class::RUNTIME,
                    "Can't have 2 overloads with same arity".to_owned(),
                    span,
                    Verb::Compiling,
                    Some("fn*".to_owned()),
                ));
            }
            arities.push(arity);
        }
        if variadic_fixed.is_some_and(|vfixed| fixed_counts.iter().any(|c| *c > vfixed)) {
            return Err(self.err(
                class::RUNTIME,
                "Can't have fixed arity function with more params than variadic function"
                    .to_owned(),
                span,
                Verb::Compiling,
                Some("fn*".to_owned()),
            ));
        }
        Ok(arities)
    }

    /// Analyze one clause: parameters, pre-asserts, body (non-tail
    /// under `:post`, which wraps it in the `%` `let`), post-asserts.
    fn analyze_clause(&mut self, clause: &RawClause, span: Span) -> Result<Arity, CompileError> {
        let base = self.locals.len();
        let mut fixed = Vec::with_capacity(clause.fixed.len());
        for pat in &clause.fixed {
            let (pattern, names) = self.build_pattern(pat, span)?;
            self.locals.extend(names);
            fixed.push(pattern);
        }
        let mut rest_pat = None;
        if let Some(rest) = clause.rest {
            let (pattern, names) = self.build_pattern(rest, span)?;
            self.locals.extend(names);
            rest_pat = Some(pattern);
        }
        let saved_recur = self.recur;
        let count = clause.fixed.len() + usize::from(clause.rest.is_some());
        self.recur = Some(count);
        // A `fn` boundary resets the try/recur fence (`Compiler.java`
        // binds `NO_RECUR` null per `FnExpr`), for the whole clause
        // (pre/post included, probe30 `fence-fn-*`).
        let saved_no_recur = self.no_recur;
        self.no_recur = false;
        let mut body = Vec::with_capacity(clause.pre.len() + clause.body.len() + 1);
        for cond in &clause.pre {
            body.push(self.analyze_assert(cond, span)?);
        }
        if clause.post.is_empty() {
            body.extend(self.analyze_body(&clause.body, span, true)?);
        } else {
            let inner = self.analyze_body(&clause.body, span, false)?;
            let pct = self.locals.len();
            self.locals.push("%".to_owned());
            let mut post = Vec::with_capacity(clause.post.len() + 1);
            for cond in &clause.post {
                post.push(self.analyze_assert(cond, span)?);
            }
            post.push(Expr::Local(pct));
            self.locals.pop();
            body.push(Expr::Let {
                bindings: vec![(Pattern::Symbol("%".to_owned()), Expr::Do(inner))],
                body: post,
            });
        }
        self.locals.truncate(base);
        self.recur = saved_recur;
        self.no_recur = saved_no_recur;
        Ok(Arity {
            fixed,
            rest: rest_pat,
            body,
        })
    }

    /// `(assert cond)`: falsy throws
    /// `AssertionError | Assert failed: COND`.
    fn analyze_assert(&mut self, cond: &Value, span: Span) -> Result<Expr, CompileError> {
        let message = format!("Assert failed: {}", pr_of(cond));
        Ok(Expr::If {
            test: Box::new(self.analyze(cond, span, false)?),
            then: Box::new(Expr::Literal(Value::Nil)),
            els: Box::new(Expr::Throw(Box::new(Expr::NewThrowable {
                class: clj_value::ThrowClass::AssertionError,
                message,
            }))),
        })
    }

    /// `(letfn [name tail …] body…)`: fnspecs seq (the `ISeq` error
    /// for non-seqables), name (the `letfn*` error), tails as `fn`
    /// clauses (the `fn`/`fn*` wrappers).
    fn parse_letfn(
        &mut self,
        op: &str,
        items: &[&Value],
        span: Span,
        tail: bool,
    ) -> Result<Expr, CompileError> {
        let bindings = items.get(1).copied().unwrap_or(&Value::Nil);
        let specs: Vec<Value> = match bindings {
            Value::Vector(v) => v.iter().cloned().collect(),
            Value::Nil => Vec::new(),
            other => {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    format!(
                        "Don't know how to create ISeq from: {}",
                        jvm_class_of(other)
                    ),
                    span,
                    Verb::Macroexpanding,
                    Some(op.to_owned()),
                ));
            }
        };
        // Validate every fnspec before analyzing any tail.
        let validated = self.validate_fnspecs(&specs, span, op)?;
        let base = self.locals.len();
        for spec in &validated {
            self.locals.push(spec.name.clone());
        }
        let mut fns = Vec::with_capacity(validated.len());
        for spec in &validated {
            let seq = self.eval.next_fn_seq();
            let display = format!("user/eval{}/{}--{seq}", self.eval_seq, spec.name);
            let tail_refs: Vec<&Value> = spec.tail.iter().collect();
            let owned = self.collect_fn_clauses(&tail_refs, span)?;
            let slices: Vec<&[&Value]> = owned.iter().map(Vec::as_slice).collect();
            let raw = self.parse_clause_slices(&slices, span)?;
            let arities = self.analyze_arities(&raw, span)?;
            fns.push((spec.name.clone(), FunExpr { display, arities }));
        }
        let body = self.analyze_body(&items[2..], span, tail)?;
        self.locals.truncate(base);
        Ok(Expr::LetFn { fns, body })
    }

    /// Validate every fnspec (`(map first fnspecs)`: strict
    /// `seq`, then a plain-symbol head) before any tail analysis.
    fn validate_fnspecs(
        &self,
        specs: &[Value],
        span: Span,
        op: &str,
    ) -> Result<Vec<FnSpec>, CompileError> {
        let mut validated = Vec::with_capacity(specs.len());
        for spec in specs {
            if !is_seqable(spec) {
                return Err(self.err(
                    class::ILLEGAL_ARG,
                    format!("Don't know how to create ISeq from: {}", jvm_class_of(spec)),
                    span,
                    Verb::Macroexpanding,
                    Some(op.to_owned()),
                ));
            }
            let parts = seq_values(spec);
            validated.push(self.fnspec_head(&parts, span)?);
        }
        Ok(validated)
    }

    /// One fnspec head: a plain symbol (anything else is the
    /// `letfn*` error).
    fn fnspec_head(&self, parts: &[Value], span: Span) -> Result<FnSpec, CompileError> {
        let head = parts.first();
        let Some(Value::Symbol(name)) = head else {
            return Err(self.err(
                class::ILLEGAL_ARG,
                format!(
                    "Bad binding form, expected symbol, got: {}",
                    head.map_or("null".to_owned(), value_of)
                ),
                span,
                Verb::Compiling,
                Some("letfn*".to_owned()),
            ));
        };
        // Qualified names hit the `let` pre-seed rule, not the
        // binding-form check (probe31 `letfn-qual-name`).
        if name.namespace().is_some() {
            return Err(self.err(
                class::RUNTIME,
                format!("Can't let qualified name: {name}"),
                span,
                Verb::Compiling,
                Some("letfn*".to_owned()),
            ));
        }
        // Non-symbols and qualified names are rejected above;
        // `&` is a valid function name (mangled, probe33
        // `letfn-amp`).
        Ok(FnSpec {
            name: name.name().to_owned(),
            tail: parts.get(1..).unwrap_or(&[]).to_vec(),
        })
    }

    /// Parse `fn` clause slices (shared by `fn` and `letfn`):
    /// spec-checked into raw clauses.
    fn parse_clause_slices<'v>(
        &mut self,
        clauses: &[&[&'v Value]],
        span: Span,
    ) -> Result<Vec<RawClause<'v>>, CompileError> {
        let mut raw = Vec::with_capacity(clauses.len());
        for clause in clauses {
            let Some((Value::Vector(params), body)) = clause.split_first() else {
                return Err(self.spec_error("clojure.core/fn", span));
            };
            let param_forms: Vec<&Value> = params.iter().collect();
            let Some((fixed, rest_pat)) = split_params(&param_forms) else {
                return Err(self.spec_error("clojure.core/fn", span));
            };
            for pat in fixed.iter().chain(rest_pat.iter()) {
                if !spec_valid_binding(pat) {
                    return Err(self.spec_error("clojure.core/fn", span));
                }
            }
            // A leading map is pre/post only when a body form follows
            // it; a lone map is the body (probe31 `fn-lone-*`).
            let (pre, post, body) = match body.split_first() {
                Some((Value::Map(prepost), rest)) if !rest.is_empty() => {
                    let pre = self.prepost_conds(prepost, "pre", span)?;
                    let post = self.prepost_conds(prepost, "post", span)?;
                    (pre, post, rest.to_vec())
                }
                _ => (Vec::new(), Vec::new(), body.to_vec()),
            };
            raw.push(RawClause {
                fixed,
                rest: rest_pat,
                pre,
                post,
                body,
            });
        }
        Ok(raw)
    }

    /// One pre/post condition list: `nil` is empty, seqables are
    /// conditions, anything else is the `fn*` `ISeq` error. Map
    /// values seq to whole entry vectors (evaluated in map order),
    /// never flattened to k/v conds (probe33 `pre-map-*`).
    fn prepost_conds(
        &self,
        prepost: &Map,
        which: &str,
        span: Span,
    ) -> Result<Vec<Value>, CompileError> {
        match map_get(prepost, which) {
            None | Some(Value::Nil) => Ok(Vec::new()),
            Some(Value::Map(m)) => Ok(m
                .iter()
                .map(|(k, v)| Value::Vector(Vector::from_vec(vec![k.clone(), v.clone()])))
                .collect()),
            Some(v) if is_seqable(v) => Ok(seq_refs(v).into_iter().cloned().collect()),
            Some(other) => Err(self.err(
                class::ILLEGAL_ARG,
                format!(
                    "Don't know how to create ISeq from: {}",
                    jvm_class_of(other)
                ),
                span,
                Verb::Compiling,
                Some("fn*".to_owned()),
            )),
        }
    }
}

/// Borrowed `seq` for pre/post conditions (maps yield entries).
fn seq_refs(v: &Value) -> Vec<&Value> {
    match v {
        Value::Nil => Vec::new(),
        Value::List(l) => l.iter().collect(),
        Value::Vector(v) => v.iter().collect(),
        Value::Map(m) => m.iter().flat_map(|(k, val)| [k, val]).collect(),
        Value::Set(s) => s.iter().collect(),
        _ => Vec::new(),
    }
}
