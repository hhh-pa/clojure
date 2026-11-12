// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The S8 AOT subset planner: Clojure surface forms become a closed
//! [`SubsetProgram`] (or the first failure in source order). LLVM-free
//! by design — `clj-aot` lowers the plan, never the analyzer.
//!
//! Two passes per top-level form, in load order (mirroring
//! `Compiler.eval`, minus evaluation):
//!
//! 1. Validate: `macroexpand_full`, `do`-split, analyze on a
//!    throwaway `boot()` evaluator (bare failures become uncaught,
//!    exactly like `eval_top_form`). Analyzer errors surface with
//!    interpreter-identical messages and direct file spans.
//! 2. Classify the ORIGINAL surface (pre-expansion — the `defn`
//!    macro's expansion holds `alter-meta!`/`var`, and other
//!    macros must NOT leak through expansion): single-clause
//!    fixed-arity `defn`, `if`/`do`/`let`/`loop`, tail `recur`,
//!    the `cond`/`and`/`or`/`when`/`when-not` macro-forms
//!    (desugared to `If`/`Do`/`Let` with the macros' exact
//!    short-circuit semantics), subset literals, direct sibling
//!    calls, closed-primitive calls, `print`. Anything else is a
//!    [`SubsetError`] naming the form. (The remaining macros are
//!    later slices' work — the boundary is the [`SubsetError`]
//!    table below, not an accident.)
//!
//! Validation runs before classification per form, so the first
//! failure in source order wins regardless of kind, and anything
//! reaching classification already resolved (unknown heads are
//! analyzer errors, never subset errors). Special forms pass
//! expansion untouched, so validated shapes (`if` arity, `let`
//! bindings) ARE the classified shapes — the few
//! validation-first trusts below name this invariant, and every
//! one is pinned by a direct unit test (the 100% line gate
//! admits no dead arms).

use std::collections::HashMap;
use std::iter::Peekable;
use std::str::Chars;
use std::sync::Arc;

use clj_reader::symbol::{is_terminating_macro, is_whitespace};
use clj_reader::{ReaderError, Span};
use clj_value::{Keyword, PrintOpts, Symbol, Throwable, Value, pr_to_string};

use crate::error::{CompileError, compile_diagnostic, span_of};
use crate::eval::{Evaluator, SourceError};
use crate::ns::boot;

mod mixed;
pub use mixed::{MIXED_INTERN_NS, MixedProgram, intern_call, plan_mixed};

/// A closed primitive call in the AOT subset (spec 2.4): the runtime
/// delegates each to the interpreter's own native, so semantics and
/// error texts are shared, never reimplemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Add,
    Sub,
    Mul,
    Quot,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Count,
    Nth,
    Get,
    Conj,
    Long,
    Int,
    Transient,
    ConjBang,
    AssocBang,
    PersistentBang,
}

/// The closed-prim name table in discriminant order (a table, not
/// a match: one arm per prim would trip the complexity gate —
/// the pairing is pinned by `prim_resolve_table` instead, and
/// new prims append so discriminants never shift).
const PRIM_TABLE: [(&str, Prim); 19] = [
    ("+", Prim::Add),
    ("-", Prim::Sub),
    ("*", Prim::Mul),
    ("quot", Prim::Quot),
    ("<", Prim::Lt),
    ("<=", Prim::Le),
    (">", Prim::Gt),
    (">=", Prim::Ge),
    ("=", Prim::Eq),
    ("count", Prim::Count),
    ("nth", Prim::Nth),
    ("get", Prim::Get),
    ("conj", Prim::Conj),
    ("long", Prim::Long),
    ("int", Prim::Int),
    ("transient", Prim::Transient),
    ("conj!", Prim::ConjBang),
    ("assoc!", Prim::AssocBang),
    ("persistent!", Prim::PersistentBang),
];

impl Prim {
    /// Resolve an unqualified or `clojure.core/`-qualified call head.
    /// Validation-first: only those heads resolve here (anything
    /// else failed resolution), so the qualified-other check is
    /// defense in depth, pinned by a direct test.
    fn resolve(ns: Option<&str>, name: &str) -> Option<Self> {
        if ns.is_some_and(|ns| ns != "clojure.core") {
            return None;
        }
        PRIM_TABLE
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, prim)| *prim)
    }
}

/// One planned expression: literals over the subset value flow
/// (`Int`/`Double`/`Bool`/`Str`/`Keyword`/`Vector`, plus the
/// unavoidable `nil` from `get`-miss/`if`-no-else/`print`),
/// locals by flat slot, control flow, and calls (primitives,
/// sibling `defn`s by index, `print`).
#[derive(Clone, Debug, PartialEq)]
pub enum SubsetExpr {
    Nil,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(Arc<str>),
    Keyword(Keyword),
    VecLit(Vec<SubsetExpr>),
    Local(usize),
    If {
        test: Box<SubsetExpr>,
        then: Box<SubsetExpr>,
        els: Box<SubsetExpr>,
    },
    Do(Vec<SubsetExpr>),
    Let {
        bindings: Vec<(usize, SubsetExpr)>,
        body: Vec<SubsetExpr>,
    },
    /// A `loop` over flat frame slots: the inits bind once, then
    /// the body runs; a tail [`SubsetExpr::Recur`] rebinds the
    /// slots and restarts the body, any other completion yields
    /// the body's value (empty bodies yield nil). Nested loops
    /// shadow outer targets (innermost wins, like the analyzer).
    Loop {
        bindings: Vec<(usize, SubsetExpr)>,
        body: Vec<SubsetExpr>,
    },
    /// A tail `recur` to the innermost enclosing `loop`: one arg
    /// per binding slot, all evaluated before any slot rebinds
    /// (so `(recur b a)` swaps). The planner guarantees tail
    /// position and arity — lowering stores the args and
    /// branches to the loop head, never checking.
    Recur(Vec<SubsetExpr>),
    CallPrim {
        prim: Prim,
        args: Vec<SubsetExpr>,
        span: Span,
    },
    CallFn {
        index: usize,
        args: Vec<SubsetExpr>,
        span: Span,
    },
    /// A sibling call with the wrong arg count: the interpreter
    /// fails these at runtime, so the plan carries the abort (the
    /// binary exits 1 with the arity text; the display is the
    /// stable `user/name` — the interpreter's `evalNN/fn--MM`
    /// counters are unreproducible by design).
    ArityError {
        display: String,
        expected: usize,
        got: usize,
        span: Span,
    },
    Print {
        args: Vec<SubsetExpr>,
        span: Span,
    },
}

/// One planned top-level `defn`: fixed arity, single body, flat
/// frame (`frame_slots` covers params `0..arity` plus every
/// `let`/`loop`-allocated slot; slots never reuse, so the frame
/// is a sound over-approximation by construction).
#[derive(Clone, Debug)]
pub struct SubsetFn {
    /// The `defn` name as written.
    pub name: String,
    /// Fixed parameter count.
    pub arity: usize,
    /// The body (implicit `do` over the clause body).
    pub body: SubsetExpr,
    /// Frame slot count (`>= arity`, `>=` every `Local` + 1).
    pub frame_slots: usize,
    /// The `defn` form's span.
    pub span: Span,
}

/// A planned program: sibling functions plus the `main` statement
/// sequence (non-`defn` subset tops in load order, values
/// discarded — like `run`'s no-echo rule).
#[derive(Clone, Debug)]
pub struct SubsetProgram {
    /// Sibling `defn`s in source order (`CallFn.index`).
    pub fns: Vec<SubsetFn>,
    /// `main` statements in load order.
    pub main: Vec<SubsetExpr>,
    /// `main` frame slot count.
    pub main_slots: usize,
    /// Index into [`fns`](Self::fns) of the arity-1 `-main`
    /// entry, if any (T1192 FX1 D-argv: the binary invokes it
    /// with the argv list after the tops run).
    pub main_fn: Option<usize>,
}

/// The first unsupported form: its span (nearest enclosing list),
/// the operator as written, and a hint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubsetError {
    /// The blamed span (nearest enclosing list's span).
    pub span: Span,
    /// The operator as written (`loop`, `map`, `:kw`, ...).
    pub form: String,
    /// Why it is out (category or actionable fix).
    pub hint: String,
}

/// Planning failures: interpreter failures verbatim (reader,
/// analyzer, uncaught, internal) plus subset errors.
#[derive(Clone, Debug)]
pub enum PlanError {
    Reader(ReaderError),
    Compile(CompileError),
    Uncaught(Throwable),
    Internal(String),
    Subset(SubsetError),
    Depth {
        span: Span,
        depth: usize,
        expanded: bool,
    },
}

impl PlanError {
    /// Render as `path:line:col: message` (spec 1.4/C8).
    #[must_use]
    pub fn diagnostic(&self, path: &str) -> String {
        match self {
            Self::Reader(e) => {
                let span = e.span();
                format!("{path}:{}:{}: {}", span.line, span.col, e.message())
            }
            Self::Compile(e) => compile_diagnostic(path, e),
            Self::Uncaught(t) => {
                let class = t.class.jvm_name();
                let msg = t.message.as_deref().unwrap_or("");
                format!("{path}:1:1: {class}: {msg}")
            }
            Self::Internal(msg) => format!("{path}:1:1: internal error: {msg}"),
            Self::Subset(e) => format!(
                "{path}:{}:{}: unsupported form in AOT subset: {} ({})",
                e.span.line, e.span.col, e.form, e.hint
            ),
            Self::Depth {
                span,
                depth,
                expanded,
            } => {
                let after = if *expanded { " after expansion" } else { "" };
                format!(
                    "{path}:{}:{}: nesting depth {depth}{after} exceeds the AOT build limit ({MAX_NESTING_DEPTH})",
                    span.line, span.col
                )
            }
        }
    }
}

/// Lift an interpreter failure 1:1 (every arm is reachable or
/// directly pinned: `Internal` cannot arise in planning, but the
/// mapping is total and tested).
fn lift(e: SourceError) -> PlanError {
    match e {
        SourceError::Reader(e) => PlanError::Reader(e),
        SourceError::Compile(e) => PlanError::Compile(e),
        SourceError::Uncaught(t) => PlanError::Uncaught(t),
        SourceError::Internal(msg) => PlanError::Internal(msg),
    }
}

/// Max nesting depth accepted by `build` (S9C): the reader,
/// analyzer, and lowering all recurse per nesting level on the
/// calling thread, and Rust stack overflow aborts. Probed
/// minimum overflow is 161 levels on a 1 MB debug stack; the
/// limit sits at a quarter of that (deepest corpus input: 16).
const MAX_NESTING_DEPTH: usize = 40;
/// A backtick costs 8: syntax-quote multiplies read-output depth
/// ~3x per level (measured: 121/364/3280 deep at 5/6/8 ticks, and
/// exactly 3x+1 over deep content), while every other wrapper is
/// +1. At weight 8 at most 5 ticks pass the scan, bounding read
/// output to ~648 deep (drop-safe on every stack, incl. 1 MB
/// test threads); 6 ticks reject outright. Real code nests at
/// most 1-2.
const BACKQUOTE_WEIGHT: usize = 8;

/// Lexer states for the source pre-scan.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    Code,
    Str,
    Comment,
}

/// Lexer-aware source nesting scan (S9C): measures max delimiter
/// depth before `read_forms` runs, so pathological input fails
/// with a diagnostic instead of overflowing the reader. Strings,
/// comments, regex bodies, and char literals are inert; wrapper
/// runs (`'`, `~`, `@`, `^`, `#`-dispatch) fold into the level
/// they prefix at +1 each, except backquote at
/// [`BACKQUOTE_WEIGHT`] (syntax-quote multiplies read-output
/// depth ~3x per level). Over-approximates everywhere (a `#`
/// counts even where the reader takes one frame): near-limit
/// inputs may reject, deep inputs never pass.
struct DepthScan {
    mode: ScanMode,
    escaped: bool,
    base: usize,
    run: usize,
    folds: Vec<usize>,
    line: u32,
    col: u32,
}

impl DepthScan {
    /// Scan `text`, returning the crossing position and depth when
    /// nesting exceeds [`MAX_NESTING_DEPTH`].
    fn scan(text: &str) -> Option<(Span, usize)> {
        let mut scan = Self {
            mode: ScanMode::Code,
            escaped: false,
            base: 0,
            run: 0,
            folds: Vec::new(),
            line: 1,
            col: 1,
        };
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if let Some(hit) = scan.step(ch, &mut chars) {
                return Some(hit);
            }
            scan.advance(ch);
        }
        None
    }

    fn span(&self) -> Span {
        Span::new(self.line, self.col)
    }

    fn advance(&mut self, ch: char) {
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
    }

    fn step(&mut self, ch: char, rest: &mut Peekable<Chars<'_>>) -> Option<(Span, usize)> {
        match self.mode {
            ScanMode::Str => {
                self.step_str(ch);
                None
            }
            ScanMode::Comment => {
                self.step_comment(ch);
                None
            }
            ScanMode::Code => self.step_code(ch, rest),
        }
    }

    fn step_str(&mut self, ch: char) {
        if self.escaped {
            self.escaped = false;
        } else if ch == '\\' {
            self.escaped = true;
        } else if ch == '"' {
            self.mode = ScanMode::Code;
            self.run = 0;
        }
    }

    fn step_comment(&mut self, ch: char) {
        if ch == '\n' {
            self.mode = ScanMode::Code;
        }
    }

    fn step_code(&mut self, ch: char, rest: &mut Peekable<Chars<'_>>) -> Option<(Span, usize)> {
        match ch {
            '"' => {
                self.mode = ScanMode::Str;
                None
            }
            ';' => {
                self.mode = ScanMode::Comment;
                None
            }
            '\\' => {
                self.skip_char(rest);
                self.run = 0;
                None
            }
            '#' => self.step_hash(rest),
            '`' => self.bump_run(BACKQUOTE_WEIGHT),
            '\'' | '~' | '@' | '^' => self.bump_run(1),
            '(' | '[' | '{' => self.open(),
            ')' | ']' | '}' => {
                self.close();
                None
            }
            _ if ch.is_whitespace() || ch == ',' => None,
            _ => {
                self.run = 0;
                None
            }
        }
    }

    /// A char literal: exactly one char is consumed (a letter-run
    /// like `newline` holds no delimiters, so skipping one
    /// suffices).
    fn skip_char(&mut self, rest: &mut Peekable<Chars<'_>>) {
        if let Some(next) = rest.next() {
            if next == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
    }

    /// A `#` dispatch: over-counted by up to a frame (safe
    /// direction). A lone `#` (gensym `x#`, or reader-invalid) is
    /// inert.
    fn step_hash(&mut self, rest: &mut Peekable<Chars<'_>>) -> Option<(Span, usize)> {
        match rest.peek() {
            Some('"') => {
                rest.next();
                self.col += 1;
                self.mode = ScanMode::Str;
                self.bump_run(2)
            }
            Some('(') | Some('{') => self.bump_run(2),
            Some('\'') | Some('_') | Some('?') => {
                rest.next();
                self.col += 1;
                self.bump_run(2)
            }
            Some(':') => {
                rest.next();
                self.col += 1;
                self.skip_tag(rest);
                self.bump_run(2)
            }
            Some(ch) if ch.is_alphabetic() => {
                self.skip_tag(rest);
                self.bump_run(2)
            }
            _ => None,
        }
    }

    /// Consume a tag/alias token exactly as the reader does:
    /// until EOF, whitespace, or a terminating macro (the
    /// `read_token` rule, same predicates, so the two can never
    /// drift apart). The `step_hash` bump then survives to the
    /// form for every spellable tag (`#a-1`, `#::alias`,
    /// `#:a-1`); stopping early would reset the run on the first
    /// non-alpha char and under-count stacked tags.
    fn skip_tag(&mut self, rest: &mut Peekable<Chars<'_>>) {
        while matches!(rest.peek(), Some(ch) if !is_whitespace(*ch) && !is_terminating_macro(*ch)) {
            rest.next();
            self.col += 1;
        }
    }

    /// Add wrapper levels, bailing past the limit.
    fn bump_run(&mut self, n: usize) -> Option<(Span, usize)> {
        self.run += n;
        let depth = self.base + self.run;
        if depth > MAX_NESTING_DEPTH {
            Some((self.span(), depth))
        } else {
            None
        }
    }

    /// Open a level, folding the pending wrapper run into it (the
    /// fold pops with the matching closer).
    fn open(&mut self) -> Option<(Span, usize)> {
        let depth = self.base + self.run + 1;
        if depth > MAX_NESTING_DEPTH {
            return Some((self.span(), depth));
        }
        self.folds.push(self.run);
        self.base = depth;
        self.run = 0;
        None
    }

    /// Close a level, unfolding its wrapper run (an unmatched
    /// closer is the reader's error; the scan leaves depth alone).
    fn close(&mut self) {
        self.run = 0;
        if let Some(fold) = self.folds.pop() {
            self.base = self.base.saturating_sub(1 + fold);
        }
    }
}

/// Max container nesting of an expanded form, walked iteratively
/// (atoms score 0, each list/vector/map/set/tag/conditional adds
/// a level). Metadata is never traversed: analyze only clones
/// and propagates it.
fn value_depth(value: &Value) -> usize {
    let mut max = 0;
    let mut stack = vec![(value.clone(), 0)];
    while let Some((node, depth)) = stack.pop() {
        if push_children(&mut stack, &node, depth + 1) {
            max = max.max(depth + 1);
        }
    }
    max
}

/// Push a node's children at `depth`; true when the node is a
/// container. The worklist owns its nodes (packed vector items have
/// no address to lend); children are cloned in, which is bumps for
/// heap values on this analyzer-only path.
fn push_children(stack: &mut Vec<(Value, usize)>, value: &Value, depth: usize) -> bool {
    match value {
        Value::List(l) => {
            push_each(stack, l.iter().cloned(), depth);
            true
        }
        Value::Vector(v) => {
            push_each(stack, v.iter(), depth);
            true
        }
        Value::Set(s) => {
            push_each(stack, s.iter().cloned(), depth);
            true
        }
        Value::Map(m) => {
            for (k, v) in m.iter() {
                stack.push((k.clone(), depth));
                stack.push((v.clone(), depth));
            }
            true
        }
        Value::TaggedLiteral { form, .. } => {
            stack.push((form.as_ref().clone(), depth));
            true
        }
        Value::ReaderConditional { form, .. } => {
            stack.push((form.as_ref().clone(), depth));
            true
        }
        _ => false,
    }
}

/// Build the nesting diagnostic error (shared by the source
/// pre-scan and the expanded-form rescan).
fn depth_error(span: Span, depth: usize, expanded: bool) -> PlanError {
    PlanError::Depth {
        span,
        depth,
        expanded,
    }
}

fn push_each(stack: &mut Vec<(Value, usize)>, items: impl Iterator<Item = Value>, depth: usize) {
    for item in items {
        stack.push((item, depth));
    }
}

/// Plan `text` on a throwaway booted evaluator (planning interns
/// vars as analysis runs, so the evaluator is never shared).
pub fn plan_source(text: &str) -> Result<SubsetProgram, PlanError> {
    let mut eval = boot();
    // Third guard (B1): analyze-time macroexpansion can deepen
    // forms the rescan measured shallow, so the analyzer itself
    // carries the budget on this throwaway evaluator (never unset:
    // nothing else runs on it).
    eval.analyze_depth_limit = Some(MAX_NESTING_DEPTH);
    if let Some((span, depth)) = DepthScan::scan(text) {
        return Err(depth_error(span, depth, false));
    }
    let forms = eval
        .read_forms(text, &mut clj_reader::NoHooks)
        .map_err(lift)?;
    let mut planner = Planner::new();
    for form in &forms {
        planner.plan_top(&mut eval, form)?;
    }
    Ok(planner.finish())
}

/// Lexical scope: names to flat slots, shadowing via save/restore.
#[derive(Default)]
struct Scope {
    slots: HashMap<String, usize>,
}

impl Scope {
    fn get(&self, name: &str) -> Option<usize> {
        self.slots.get(name).copied()
    }

    /// Bind `name` to `slot`, returning the shadowed slot (if any)
    /// for restore.
    fn push(&mut self, name: &str, slot: usize) -> Option<usize> {
        self.slots.insert(name.to_owned(), slot)
    }

    fn restore(&mut self, name: &str, prev: Option<usize>) {
        match prev {
            Some(slot) => {
                self.slots.insert(name.to_owned(), slot);
            }
            None => {
                self.slots.remove(name);
            }
        }
    }
}

/// The innermost enclosing `loop` for `recur`: its binding count
/// plus whether the position is a tail of the loop body.
/// Validation-first: the analyzer fences the same shapes (tail,
/// arity, `try`-crossing) on the expansion before classification
/// runs, so a mismatch here is defense in depth, pinned by
/// direct unit tests.
#[derive(Clone, Copy)]
struct RecurCtx {
    arity: usize,
    tail: bool,
}

/// The same target in a non-tail position (call heads, tests,
/// args, inits — only `loop` inits clear the target outright,
/// mirroring the analyzer).
fn nontail(recur: Option<RecurCtx>) -> Option<RecurCtx> {
    recur.map(|ctx| RecurCtx { tail: false, ..ctx })
}

struct Planner {
    fns: Vec<SubsetFn>,
    by_name: HashMap<String, usize>,
    main: Vec<SubsetExpr>,
    main_next: usize,
}

/// A [`Planner`] rollback point (mixed planning retries one
/// top-level form at a time and drops a failed form's partial
/// registration).
struct PlannerSnapshot {
    fns: usize,
    by_name: HashMap<String, usize>,
    main: usize,
    main_next: usize,
}

impl Planner {
    fn new() -> Self {
        Self {
            fns: Vec::new(),
            by_name: HashMap::new(),
            main: Vec::new(),
            main_next: 0,
        }
    }

    fn snapshot(&self) -> PlannerSnapshot {
        PlannerSnapshot {
            fns: self.fns.len(),
            by_name: self.by_name.clone(),
            main: self.main.len(),
            main_next: self.main_next,
        }
    }

    fn restore(&mut self, snapshot: PlannerSnapshot) {
        self.fns.truncate(snapshot.fns);
        self.by_name = snapshot.by_name;
        self.main.truncate(snapshot.main);
        self.main_next = snapshot.main_next;
    }

    fn finish(self) -> SubsetProgram {
        SubsetProgram {
            main_fn: self.by_name.get("-main").copied(),
            fns: self.fns,
            main: self.main,
            main_slots: self.main_next,
        }
    }

    /// Validate then classify one top-level form (single pass, so
    /// the first failure in source order wins).
    fn plan_top(&mut self, eval: &mut Evaluator, form: &Value) -> Result<(), PlanError> {
        let fallback = span_of(form, Span::new(1, 1));
        let source: Arc<str> = eval.source().into();
        // Validate: the interpreter's own pipeline (expand, split,
        // analyze in order, bare failures uncaught) minus evaluation.
        let expanded = crate::macros::macroexpand_full(eval, form, fallback, &source)
            .map_err(|e| lift(eval.compile_err(e)))?;
        // Expansion can multiply depth (`->` with N steps nests N
        // deep from a flat source), so re-budget the expanded form
        // before analyze recurses on it.
        let expanded_depth = value_depth(&expanded);
        if expanded_depth > MAX_NESTING_DEPTH {
            // B1 twin: dropping this output would recurse per level
            // (see `analyze_list`); the build fails here either way.
            std::mem::forget(expanded);
            return Err(depth_error(fallback, expanded_depth, true));
        }
        let fallback_parts = vec![&expanded];
        let parts = match crate::eval::split_top_do(&expanded) {
            Some(parts) => parts,
            None => fallback_parts,
        };
        for part in parts {
            eval.registry.bump_eval_seq();
            if let Err(e) = eval.analyze(part) {
                // Budget breach: expansion outgrew the limit mid-
                // analyze — the Depth diagnostic, not a compile
                // error (first crossing trips at limit + 1).
                if e.class == crate::expr::class::STACK_OVERFLOW {
                    return Err(depth_error(
                        span_of(part, fallback),
                        MAX_NESTING_DEPTH + 1,
                        true,
                    ));
                }
                return Err(lift(eval.compile_err(e)));
            }
        }
        // Classify the ORIGINAL surface (never the expansion).
        self.classify_top(form, fallback)
    }

    /// Classify one surface top-level form (`do`-split, `defn`,
    /// or a `main` statement).
    fn classify_top(&mut self, form: &Value, fallback: Span) -> Result<(), PlanError> {
        if let Value::List(list) = form {
            let items: Vec<&Value> = list.iter().collect();
            if let [Value::Symbol(head), rest @ ..] = items.as_slice() {
                let span = span_of(form, fallback);
                if head.namespace().is_none() && head.name() == "do" {
                    if rest.is_empty() {
                        self.main.push(SubsetExpr::Do(Vec::new()));
                        return Ok(());
                    }
                    for element in rest.iter().copied() {
                        self.classify_top(element, span)?;
                    }
                    return Ok(());
                }
                if is_defn_head(head) {
                    return self.classify_defn(form, span);
                }
            }
        }
        let mut scope = Scope::default();
        let mut next = self.main_next;
        let expr = self.classify_expr(form, &mut scope, &mut next, None, fallback)?;
        self.main_next = next;
        self.main.push(expr);
        Ok(())
    }

    /// Classify a `(defn ...)` top-level form.
    fn classify_defn(&mut self, form: &Value, span: Span) -> Result<(), PlanError> {
        let Value::List(list) = form else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "defn".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        let items: Vec<&Value> = list.iter().collect();
        let (name, params, body) = match parse_defn(&items) {
            DefnShape::Single { name, params, body } => (name, params, body),
            DefnShape::Multi => {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "defn".to_owned(),
                    hint: "multi-arity; use a single vector clause".to_owned(),
                }));
            }
            DefnShape::Variadic => {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "defn".to_owned(),
                    hint: "variadic; use fixed params".to_owned(),
                }));
            }
            DefnShape::PrePost => {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "defn".to_owned(),
                    hint: "pre/post conditions are runtime checks".to_owned(),
                }));
            }
            DefnShape::Malformed => {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "defn".to_owned(),
                    hint: "not in the AOT pure subset".to_owned(),
                }));
            }
        };
        if self.by_name.contains_key(name) {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "defn".to_owned(),
                hint: "redefinition; AOT resolves calls at compile time".to_owned(),
            }));
        }
        // The `-main` entry takes exactly `[args]` (T1192 FX1
        // D-argv): the binary passes the whole argv list as one
        // argument (a subset adaptation — the JVM applies the
        // args, which needs rest-args the subset lacks).
        if name == "-main" && params.len() != 1 {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "-main".to_owned(),
                hint: "`-main` takes exactly one parameter ([args]) in the subset".to_owned(),
            }));
        }
        // Register before the body: direct self-calls resolve
        // (arity included). A body failure aborts planning, so a
        // half-built entry is harmless.
        let index = self.fns.len();
        self.by_name.insert(name.to_owned(), index);
        self.fns.push(SubsetFn {
            name: name.to_owned(),
            arity: params.len(),
            body: SubsetExpr::Do(Vec::new()),
            frame_slots: params.len(),
            span,
        });
        let mut scope = Scope::default();
        let mut next = params.len();
        for (i, param) in params.iter().enumerate() {
            scope.push(param, i);
        }
        let mut body_exprs = Vec::with_capacity(body.len());
        for form in body {
            body_exprs.push(self.classify_expr(form, &mut scope, &mut next, None, span)?);
        }
        self.fns[index] = SubsetFn {
            name: name.to_owned(),
            arity: params.len(),
            body: SubsetExpr::Do(body_exprs),
            frame_slots: next,
            span,
        };
        Ok(())
    }

    /// Classify one surface expression (`recur` is the innermost
    /// enclosing `loop` target plus whether this position is one
    /// of its tails — children derive, never inherit blindly).
    fn classify_expr(
        &mut self,
        form: &Value,
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        fallback: Span,
    ) -> Result<SubsetExpr, PlanError> {
        match form {
            Value::Nil => Ok(SubsetExpr::Nil),
            Value::Bool(b) => Ok(SubsetExpr::Bool(*b)),
            Value::Int(i) => Ok(SubsetExpr::Int(*i)),
            Value::Double(d) => Ok(SubsetExpr::Double(*d)),
            Value::Str(s) => Ok(SubsetExpr::Str(Arc::clone(s))),
            Value::Keyword(k) => Ok(SubsetExpr::Keyword(k.clone())),
            Value::Symbol(sym) => self.classify_symbol(sym, scope, fallback),
            Value::List(list) => {
                let span = span_of(form, fallback);
                let items: Vec<&Value> = list.iter().collect();
                let Some((&head, args)) = items.split_first() else {
                    return Err(PlanError::Subset(SubsetError {
                        span,
                        form: "()".to_owned(),
                        hint: "lists are outside the value flow".to_owned(),
                    }));
                };
                let Value::Symbol(sym) = head else {
                    // The head must itself classify first (its error,
                    // if any, is first in source order); a valid head
                    // value still cannot be called.
                    self.classify_expr(head, scope, next, nontail(recur), span)?;
                    return Err(PlanError::Subset(SubsetError {
                        span,
                        form: short_pr(head),
                        hint: "higher-order calls are outside the subset".to_owned(),
                    }));
                };
                self.classify_call(sym, args, scope, next, recur, span)
            }
            Value::Vector(vec) => {
                let mut out = Vec::with_capacity(vec.len());
                for item in vec.iter() {
                    out.push(self.classify_expr(&item, scope, next, nontail(recur), fallback)?);
                }
                Ok(SubsetExpr::VecLit(out))
            }
            other => Err(PlanError::Subset(SubsetError {
                span: fallback,
                form: short_pr(other),
                hint: "only Int/Double/Bool/Str/Keyword/Vector values flow".to_owned(),
            })),
        }
    }

    /// Classify a bare symbol: a shadowing local (unqualified
    /// reads only) flows its slot; a sibling name is a
    /// function-value error; anything else names a var.
    fn classify_symbol(
        &self,
        sym: &Symbol,
        scope: &Scope,
        fallback: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let name = sym.name();
        // Locals shadow unqualified symbols only (a qualified
        // read is a var, never a local — R1 finding 3).
        // (Match guard, not a let-chain: MSRV 1.85.)
        match scope.get(name) {
            Some(slot) if sym.namespace().is_none() => {
                return Ok(SubsetExpr::Local(slot));
            }
            _ => {}
        }
        if self.by_name.contains_key(name)
            && (sym.namespace().is_none() || sym.namespace() == Some("user"))
        {
            return Err(PlanError::Subset(SubsetError {
                span: fallback,
                form: display_sym(sym),
                hint: "function values are outside the subset; call it directly".to_owned(),
            }));
        }
        Err(PlanError::Subset(SubsetError {
            span: fallback,
            form: display_sym(sym),
            hint: "vars are outside the AOT subset".to_owned(),
        }))
    }

    /// Classify a list headed by a symbol.
    fn classify_call(
        &mut self,
        sym: &Symbol,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let ns = sym.namespace();
        let name = sym.name();
        // Locals shadow everything (higher-order: out).
        if ns.is_none() && scope.get(name).is_some() {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: name.to_owned(),
                hint: "higher-order calls are outside the subset".to_owned(),
            }));
        }
        if let Some(expr) = self.classify_surface(sym, args, scope, next, recur, span)? {
            return Ok(expr);
        }
        // Sibling `defn` calls (validation-first: a same-named
        // resolved referent is the sibling — resolution order
        // guarantees it).
        let sibling = match ns {
            None | Some("user") => self.by_name.get(name).copied(),
            _ => None,
        };
        if let Some(index) = sibling {
            let mut out = Vec::with_capacity(args.len());
            for arg in args.iter().copied() {
                out.push(self.classify_expr(arg, scope, next, nontail(recur), span)?);
            }
            let arity = self.fns[index].arity;
            if out.len() != arity {
                return Ok(SubsetExpr::ArityError {
                    display: format!("user/{}", self.fns[index].name),
                    expected: arity,
                    got: out.len(),
                    span,
                });
            }
            return Ok(SubsetExpr::CallFn {
                index,
                args: out,
                span,
            });
        }
        // Closed primitives.
        if let Some(prim) = Prim::resolve(ns, name) {
            let mut out = Vec::with_capacity(args.len());
            for arg in args.iter().copied() {
                out.push(self.classify_expr(arg, scope, next, nontail(recur), span)?);
            }
            return Ok(SubsetExpr::CallPrim {
                prim,
                args: out,
                span,
            });
        }
        // Everything else names its head.
        Err(PlanError::Subset(SubsetError {
            span,
            form: display_sym(sym),
            hint: head_hint(name).to_owned(),
        }))
    }

    /// Classify dedicated surface forms by namespace (`if`/`do`/
    /// `let`/`loop`/`recur`/`print` plus the desugared
    /// `cond`/`and`/`or`/`when`/`when-not`): `Some` when the head
    /// is one, `None` for sibling/prim/hint dispatch downstream
    /// (`recur` threads like everywhere else).
    fn classify_surface(
        &mut self,
        sym: &Symbol,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<Option<SubsetExpr>, PlanError> {
        // The symbol rides whole (namespace + name split inside)
        // so the recur threading stays under the arg-count gate.
        let ns = sym.namespace();
        let name = sym.name();
        if ns.is_none() {
            return self.classify_unqualified(name, args, scope, next, recur, span);
        }
        if ns == Some("clojure.core") {
            return self.classify_core(name, args, scope, next, recur, span);
        }
        Ok(None)
    }

    /// Classify an unqualified surface head: special forms and
    /// `print` first (never shadowable, exactly like the
    /// interpreter), then the macro-forms — unless a sibling
    /// `defn` shadows one (a redefined macro var is a plain call
    /// in the interpreter, so it falls through to sibling
    /// dispatch, exactly like primitives).
    fn classify_unqualified(
        &mut self,
        name: &str,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<Option<SubsetExpr>, PlanError> {
        if self.shadowed_macro(name) {
            return Ok(None);
        }
        if let Some(expr) = self.classify_special(name, args, scope, next, recur, span)? {
            return Ok(Some(expr));
        }
        self.classify_macro(name, args, scope, next, recur, span)
    }

    /// Classify an unqualified call head that may be special:
    /// `Some` when `name` is one (`if`/`do`/`let`/`loop`/
    /// `recur`/`defn`-error/`print`), `None` to continue to
    /// sibling/prim dispatch.
    fn classify_special(
        &mut self,
        name: &str,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<Option<SubsetExpr>, PlanError> {
        match name {
            "if" => Ok(Some(self.classify_if(args, scope, next, recur, span)?)),
            "do" => Ok(Some(self.classify_do(args, scope, next, recur, span)?)),
            "let" => Ok(Some(self.classify_let(args, scope, next, recur, span)?)),
            "loop" => Ok(Some(self.classify_loop(args, scope, next, span)?)),
            "recur" => Ok(Some(self.classify_recur(args, scope, next, recur, span)?)),
            "defn" => Err(PlanError::Subset(SubsetError {
                span,
                form: "defn".to_owned(),
                hint: "defn is top-level only".to_owned(),
            })),
            "print" => Ok(Some(self.classify_print(args, scope, next, recur, span)?)),
            _ => Ok(None),
        }
    }

    /// Classify a `clojure.core/`-qualified surface head (macros
    /// are vars, so qualified invocation is the macro — a sibling
    /// never shadows a qualified head).
    fn classify_core(
        &mut self,
        name: &str,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<Option<SubsetExpr>, PlanError> {
        match name {
            "print" => Ok(Some(self.classify_print(args, scope, next, recur, span)?)),
            _ => self.classify_macro(name, args, scope, next, recur, span),
        }
    }

    /// Classify a surface macro-form by desugaring (`None` for any
    /// other head). Shared by the unqualified and qualified paths.
    fn classify_macro(
        &mut self,
        name: &str,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<Option<SubsetExpr>, PlanError> {
        match name {
            "cond" => Ok(Some(self.classify_cond(args, scope, next, recur, span)?)),
            "and" => Ok(Some(
                self.classify_and_or(true, args, scope, next, recur, span)?,
            )),
            "or" => Ok(Some(
                self.classify_and_or(false, args, scope, next, recur, span)?,
            )),
            "when" => Ok(Some(
                self.classify_when(false, args, scope, next, recur, span)?,
            )),
            "when-not" => Ok(Some(
                self.classify_when(true, args, scope, next, recur, span)?,
            )),
            _ => Ok(None),
        }
    }

    /// Whether `name` is a surface macro-form shadowed by a sibling
    /// `defn` (unqualified heads only — the qualified path never
    /// asks).
    fn shadowed_macro(&self, name: &str) -> bool {
        match name {
            "cond" | "and" | "or" | "when" | "when-not" => self.by_name.contains_key(name),
            _ => false,
        }
    }

    /// Classify `(cond test expr ...)` to nested `If` (the macro's
    /// own expansion shape): first truthy test wins, later tests
    /// never evaluate, a trailing test without an expr is the
    /// macro's arity failure. `(cond)` is nil. `:else` needs no
    /// special case — it is just a truthy keyword test.
    /// Validation-first: an odd tail dies in macroexpansion
    /// (`cond requires an even number of forms`), so the tail arm
    /// is pinned by a direct test.
    fn classify_cond(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let Some((&test, rest)) = args.split_first() else {
            return Ok(SubsetExpr::Nil);
        };
        let Some((&expr, rest)) = rest.split_first() else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "cond".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        // The test is never a tail; the winning branch and the
        // remaining clauses inherit the form's (the expansion is
        // nested `if`s, so this matches the analyzer exactly).
        let test = self.classify_expr(test, scope, next, nontail(recur), span)?;
        let then = self.classify_expr(expr, scope, next, recur, span)?;
        let els = self.classify_cond(rest, scope, next, recur, span)?;
        Ok(SubsetExpr::If {
            test: Box::new(test),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// Classify `(and x ...)` / `(or x ...)` to `Let`-bound nested
    /// `If` (the macros' own expansion shape): each operand
    /// evaluates at most once, evaluation stops at the first
    /// falsy (`and`) / truthy (`or`) value, and THAT value (not a
    /// boolean) is the result. `(and)` is true, `(or)` is nil, a
    /// single operand is itself. The `Let` slot is anonymous (no
    /// scope name), so no user binding can capture it.
    fn classify_and_or(
        &mut self,
        is_and: bool,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let Some((&first, rest)) = args.split_first() else {
            return Ok(if is_and {
                SubsetExpr::Bool(true)
            } else {
                SubsetExpr::Nil
            });
        };
        if rest.is_empty() {
            return self.classify_expr(first, scope, next, recur, span);
        }
        // The bound operand evaluates in init position (never a
        // tail); the remaining operands sit in an `if` branch of
        // the expansion, so they inherit the form's tailness.
        let init = self.classify_expr(first, scope, next, nontail(recur), span)?;
        let slot = *next;
        *next += 1;
        let tail = self.classify_and_or(is_and, rest, scope, next, recur, span)?;
        let (then, els) = if is_and {
            (tail, SubsetExpr::Local(slot))
        } else {
            (SubsetExpr::Local(slot), tail)
        };
        Ok(SubsetExpr::Let {
            bindings: vec![(slot, init)],
            body: vec![SubsetExpr::If {
                test: Box::new(SubsetExpr::Local(slot)),
                then: Box::new(then),
                els: Box::new(els),
            }],
        })
    }

    /// Classify `(when test body...)` / `(when-not test body...)`
    /// to `If` over an implicit `Do` (the macros' own expansion
    /// shape): a falsy `when` test (truthy `when-not` test) is
    /// nil, else the last body value. Validation-first: a missing
    /// test dies in macroexpansion (macro arity), so the empty
    /// arm is pinned by a direct test.
    fn classify_when(
        &mut self,
        negated: bool,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let Some((&test, body)) = args.split_first() else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: if negated { "when-not" } else { "when" }.to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        // The test is never a tail; the implicit `do` body sits in
        // an `if` branch of the expansion, so its last form
        // inherits the form's tailness like `classify_do`.
        let test = self.classify_expr(test, scope, next, nontail(recur), span)?;
        let mut out = Vec::with_capacity(body.len());
        for (i, form) in body.iter().copied().enumerate() {
            let ctx = if i + 1 == body.len() {
                recur
            } else {
                nontail(recur)
            };
            out.push(self.classify_expr(form, scope, next, ctx, span)?);
        }
        let body = SubsetExpr::Do(out);
        if negated {
            Ok(SubsetExpr::If {
                test: Box::new(test),
                then: Box::new(SubsetExpr::Nil),
                els: Box::new(body),
            })
        } else {
            Ok(SubsetExpr::If {
                test: Box::new(test),
                then: Box::new(body),
                els: Box::new(SubsetExpr::Nil),
            })
        }
    }

    /// Classify `(do body...)`: every statement but the last in
    /// non-tail position, the last inheriting the form's.
    fn classify_do(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let mut out = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().copied().enumerate() {
            let ctx = if i + 1 == args.len() {
                recur
            } else {
                nontail(recur)
            };
            out.push(self.classify_expr(arg, scope, next, ctx, span)?);
        }
        Ok(SubsetExpr::Do(out))
    }

    /// Classify `(if test then els?)` (2-3 args: validated —
    /// `if` is special, so the surface IS the validated shape).
    /// The test is never a tail; the branches inherit the form's.
    fn classify_if(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let test = self.classify_expr(args[0], scope, next, nontail(recur), span)?;
        let then = self.classify_expr(args[1], scope, next, recur, span)?;
        let els = match args.get(2) {
            Some(els) => self.classify_expr(els, scope, next, recur, span)?,
            None => SubsetExpr::Nil,
        };
        Ok(SubsetExpr::If {
            test: Box::new(test),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// Classify `(print args...)` (args are never tails).
    fn classify_print(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args.iter().copied() {
            out.push(self.classify_expr(arg, scope, next, nontail(recur), span)?);
        }
        Ok(SubsetExpr::Print { args: out, span })
    }

    /// Classify `(let [bindings...] body...)` (symbol bindings
    /// only; the vector+even shape is validated — `let` is
    /// special, so the surface IS the validated shape — and the
    /// trust arms are pinned by direct tests). Inits are never
    /// tails; the body's last form inherits the form's.
    fn classify_let(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let [bindings, body @ ..] = args else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "let".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        let Value::Vector(vec) = bindings else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "let".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        let items: Vec<Value> = vec.iter().collect();
        let mut bound = Vec::with_capacity(items.len() / 2);
        let mut shadows: Vec<(&str, Option<usize>)> = Vec::new();
        for pair in items.chunks(2) {
            // Validated first (odd bindings never analyze), so the
            // short tail is pinned by a direct test.
            let [pat, init] = pair else {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "let".to_owned(),
                    hint: "not in the AOT pure subset".to_owned(),
                }));
            };
            let Value::Symbol(sym) = pat else {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "let".to_owned(),
                    hint: "destructuring is outside the subset; bind plain symbols".to_owned(),
                }));
            };
            if sym.namespace().is_some() {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "let".to_owned(),
                    hint: "destructuring is outside the subset; bind plain symbols".to_owned(),
                }));
            }
            let init = self.classify_expr(init, scope, next, nontail(recur), span)?;
            let slot = *next;
            *next += 1;
            shadows.push((sym.name(), scope.push(sym.name(), slot)));
            bound.push((slot, init));
        }
        let mut body_out = Vec::with_capacity(body.len());
        for (i, form) in body.iter().copied().enumerate() {
            let ctx = if i + 1 == body.len() {
                recur
            } else {
                nontail(recur)
            };
            body_out.push(self.classify_expr(form, scope, next, ctx, span)?);
        }
        for (name, prev) in shadows.into_iter().rev() {
            scope.restore(name, prev);
        }
        Ok(SubsetExpr::Let {
            bindings: bound,
            body: body_out,
        })
    }

    /// Classify `(loop [bindings...] body...)` (symbol bindings
    /// only, like `let`; the vector+even shape is validated —
    /// `loop` is special, so the surface IS the validated shape).
    /// Inits classify with no `recur` target (the analyzer clears
    /// it there too); the body is self-tail (a `loop` in any
    /// position takes `recur`, like the analyzer).
    fn classify_loop(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let [bindings, body @ ..] = args else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "loop".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        let Value::Vector(vec) = bindings else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "loop".to_owned(),
                hint: "not in the AOT pure subset".to_owned(),
            }));
        };
        let items: Vec<Value> = vec.iter().collect();
        let mut bound = Vec::with_capacity(items.len() / 2);
        let mut shadows: Vec<(&str, Option<usize>)> = Vec::new();
        for pair in items.chunks(2) {
            // Validated first (odd bindings never analyze), so the
            // short tail is pinned by a direct test.
            let [pat, init] = pair else {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "loop".to_owned(),
                    hint: "not in the AOT pure subset".to_owned(),
                }));
            };
            let Value::Symbol(sym) = pat else {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "loop".to_owned(),
                    hint: "destructuring is outside the subset; bind plain symbols".to_owned(),
                }));
            };
            if sym.namespace().is_some() {
                return Err(PlanError::Subset(SubsetError {
                    span,
                    form: "loop".to_owned(),
                    hint: "destructuring is outside the subset; bind plain symbols".to_owned(),
                }));
            }
            let init = self.classify_expr(init, scope, next, None, span)?;
            let slot = *next;
            *next += 1;
            shadows.push((sym.name(), scope.push(sym.name(), slot)));
            bound.push((slot, init));
        }
        let arity = bound.len();
        let mut body_out = Vec::with_capacity(body.len());
        for (i, form) in body.iter().copied().enumerate() {
            let tail = i + 1 == body.len();
            let ctx = Some(RecurCtx { arity, tail });
            body_out.push(self.classify_expr(form, scope, next, ctx, span)?);
        }
        for (name, prev) in shadows.into_iter().rev() {
            scope.restore(name, prev);
        }
        Ok(SubsetExpr::Loop {
            bindings: bound,
            body: body_out,
        })
    }

    /// Classify `(recur args...)`: only in tail position of a
    /// `loop` body, one arg per binding. The analyzer fences the
    /// same shapes first, so every arm but the missing-target one
    /// (fn-targeted `recur`, which validates) is defense in
    /// depth, pinned by direct tests.
    fn classify_recur(
        &mut self,
        args: &[&Value],
        scope: &mut Scope,
        next: &mut usize,
        recur: Option<RecurCtx>,
        span: Span,
    ) -> Result<SubsetExpr, PlanError> {
        let Some(ctx) = recur else {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "recur".to_owned(),
                hint: "recur outside loop is outside the subset; call the function by name"
                    .to_owned(),
            }));
        };
        if !ctx.tail {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "recur".to_owned(),
                hint: "recur must be in tail position of the loop body".to_owned(),
            }));
        }
        if args.len() != ctx.arity {
            return Err(PlanError::Subset(SubsetError {
                span,
                form: "recur".to_owned(),
                hint: "wrong number of args to recur; match the loop bindings".to_owned(),
            }));
        }
        let mut out = Vec::with_capacity(args.len());
        for arg in args.iter().copied() {
            out.push(self.classify_expr(arg, scope, next, nontail(recur), span)?);
        }
        Ok(SubsetExpr::Recur(out))
    }
}

/// Whether a top-level head is the `defn` macro (unqualified or
/// `clojure.core/`-qualified; no aliases exist in-subset).
fn is_defn_head(sym: &Symbol) -> bool {
    sym.name() == "defn" && sym.namespace().is_none_or(|ns| ns == "clojure.core")
}

/// A parsed `defn` fdecl: either a single fixed-arity clause or a
/// cleanly-recognized out-of-subset shape (`Malformed` covers what
/// the interpreter's own errors already reject in validation).
enum DefnShape<'a> {
    Single {
        name: &'a str,
        params: Vec<String>,
        body: Vec<&'a Value>,
    },
    Multi,
    Variadic,
    PrePost,
    Malformed,
}

/// Parse `(defn name doc? attr? clause+/attr?)`, mirroring the
/// macro's fdecl walk (doc, attr, vector→single else multi,
/// trailing attr). Pure: directly unit-tested, including every
/// `Malformed` turn.
fn parse_defn<'a>(items: &[&'a Value]) -> DefnShape<'a> {
    let [_, name, fdecl @ ..] = items else {
        return DefnShape::Malformed;
    };
    let Value::Symbol(sym) = name else {
        return DefnShape::Malformed;
    };
    if sym.namespace().is_some() {
        return DefnShape::Malformed;
    }
    let name = sym.name();
    let mut fdecl: &[&Value] = fdecl;
    if matches!(fdecl.first(), Some(Value::Str(_))) {
        fdecl = &fdecl[1..];
    }
    if matches!(fdecl.first(), Some(Value::Map(_))) {
        fdecl = &fdecl[1..];
    }
    // Single vector clause vs multi list clauses.
    let clause: Vec<&Value> = match fdecl.first() {
        Some(Value::Vector(_)) => fdecl.to_vec(),
        Some(Value::List(first)) => {
            let mut cls = fdecl;
            if matches!(cls.last(), Some(Value::Map(_))) {
                cls = &cls[..cls.len() - 1];
            }
            // Non-empty by construction (`first` matched), so `!= 1`
            // is multi-arity.
            if cls.len() != 1 {
                return DefnShape::Multi;
            }
            first.iter().collect()
        }
        _ => return DefnShape::Malformed,
    };
    let [argv, body @ ..] = clause.as_slice() else {
        return DefnShape::Malformed;
    };
    let Value::Vector(params) = argv else {
        return DefnShape::Malformed;
    };
    let mut out = Vec::with_capacity(params.len());
    for param in params.iter() {
        let Value::Symbol(p) = param else {
            return DefnShape::Malformed;
        };
        if p.namespace().is_some() {
            return DefnShape::Malformed;
        }
        if p.name() == "&" {
            return DefnShape::Variadic;
        }
        out.push(p.name().to_owned());
    }
    // Pre/post: a map in second position with a body after (the
    // macro's `(and (map? pp) (nnext sig))`).
    if body.len() > 1 && matches!(body.first(), Some(Value::Map(_))) {
        return DefnShape::PrePost;
    }
    DefnShape::Single {
        name,
        params: out,
        body: body.to_vec(),
    }
}

/// A symbol as written (`name` or `ns/name`).
fn display_sym(sym: &Symbol) -> String {
    match sym.namespace() {
        Some(ns) => format!("{ns}/{}", sym.name()),
        None => sym.name().to_owned(),
    }
}

/// `pr-str`, truncated past 80 chars (heads/values in diagnostics).
fn short_pr(v: &Value) -> String {
    let text = pr_to_string(v, &PrintOpts::pr());
    if text.chars().count() > 80 {
        let mut out: String = text.chars().take(80).collect();
        out.push('…');
        out
    } else {
        text
    }
}

/// The hint naming a rejected call head's category.
fn head_hint(name: &str) -> &'static str {
    match name {
        "def" | "declare" | "defonce" => "only defn defines in the AOT subset",
        "fn" | "fn*" => "only defn defines functions in the AOT subset",
        "letfn" => "use top-level defn instead",
        "try" | "catch" | "finally" | "throw" => "error handling is outside the AOT subset",
        "var" | "set!" | "binding" | "with-redefs" | "with-bindings" | "bound-fn"
        | "alter-var-root" => "vars are outside the AOT subset",
        "defmacro" | "macroexpand" | "macroexpand-1" | "eval" => {
            "macros at runtime are outside the subset"
        }
        "future" | "delay" | "force" | "promise" | "deliver" | "atom" | "swap!" | "reset!"
        | "compare-and-set!" | "send" | "send-off" | "await" | "agent" | "deref" | "add-watch"
        | "locking" | "pmap" | "pcalls" => "concurrency is outside the AOT subset",
        "lazy-seq" => "laziness is outside the AOT subset",
        "println" | "pr" | "prn" | "newline" | "read-string" | "read" | "read-line"
        | "line-seq" | "slurp" | "spit" | "format" => "only print writes in the AOT subset",
        "ns" | "in-ns" | "require" | "use" | "refer" | "load" | "load-file" | "load-string" => {
            "AOT programs are single-file, without namespaces"
        }
        "comment" | "assert" | "time" | "with-out-str" | "with-in-str" | "if-let" | "when-let"
        | "if-some" | "when-some" | "condp" | "case" | "for" | "doseq" | "dotimes" | "while"
        | "defn-" => "macros are not expanded in the AOT subset",
        "quote" => "quoted values are outside the value flow",
        _ => "not in the closed call set",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clj_value::{List, Vector, new_map};

    fn sym(name: &str) -> Value {
        Value::Symbol(Symbol::new(None, name))
    }

    fn plan(text: &str) -> Result<SubsetProgram, PlanError> {
        plan_source(text)
    }

    /// The diagnostic for `text`, or `None` when it plans (the 100%
    /// line gate admits no `panic!` arms in tests, so failures pin
    /// full strings instead of destructuring).
    fn diagnostic_of(text: &str) -> Option<String> {
        match plan(text) {
            Ok(_) => None,
            Err(e) => Some(e.diagnostic("f.clj")),
        }
    }

    fn diag(text: &str) -> String {
        diagnostic_of(text).expect("want diagnostic")
    }

    #[test]
    fn empty_program_plans_empty() {
        let prog = plan(";; nothing here\n").expect("empty plans");
        assert!(prog.fns.is_empty());
        assert!(prog.main.is_empty());
        assert_eq!(prog.main_slots, 0);
        assert_eq!(diagnostic_of("(print 1)"), None);
    }

    #[test]
    fn main_fn_tracks_arity_1_dash_main() {
        // T1192 FX1 D-argv: the binary invokes `-main` with argv.
        let prog = plan("(defn f [x] x) (defn -main [args] args)").expect("plans");
        assert_eq!(prog.main_fn, Some(1));
        let prog = plan("(defn f [x] x) (print 1)").expect("plans");
        assert_eq!(prog.main_fn, None);
        assert_eq!(
            diag("(defn -main [] 1)"),
            "f.clj:1:1: unsupported form in AOT subset: -main (`-main` takes exactly one parameter ([args]) in the subset)"
        );
        assert_eq!(
            diag("(defn -main [a b] 1)"),
            "f.clj:1:1: unsupported form in AOT subset: -main (`-main` takes exactly one parameter ([args]) in the subset)"
        );
    }

    #[test]
    fn literal_statements_plan() {
        let prog = plan("1 2.5 \"s\" true nil [1 [2]]").expect("literals plan");
        assert_eq!(
            prog.main,
            vec![
                SubsetExpr::Int(1),
                SubsetExpr::Double(2.5),
                SubsetExpr::Str(Arc::from("s")),
                SubsetExpr::Bool(true),
                SubsetExpr::Nil,
                SubsetExpr::VecLit(vec![
                    SubsetExpr::Int(1),
                    SubsetExpr::VecLit(vec![SubsetExpr::Int(2)]),
                ]),
            ]
        );
    }

    #[test]
    fn defn_and_call_plan() {
        let prog = plan("(defn f [a b] (+ a b)) (print (f 1 2))").expect("plans");
        assert_eq!(prog.fns.len(), 1);
        let f = &prog.fns[0];
        assert_eq!(f.name, "f");
        assert_eq!(f.arity, 2);
        assert_eq!(f.frame_slots, 2);
        assert_eq!(
            f.body,
            SubsetExpr::Do(vec![SubsetExpr::CallPrim {
                prim: Prim::Add,
                args: vec![SubsetExpr::Local(0), SubsetExpr::Local(1)],
                span: Span::new(1, 15),
            }])
        );
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![SubsetExpr::CallFn {
                    index: 0,
                    args: vec![SubsetExpr::Int(1), SubsetExpr::Int(2)],
                    span: Span::new(1, 31),
                }],
                span: Span::new(1, 24),
            }]
        );
    }

    #[test]
    fn self_call_plans_to_index_zero() {
        let prog = plan("(defn f [n] (if (= n 0) 0 (f (+ n -1))))").expect("plans");
        assert_eq!(
            prog.fns[0].body,
            SubsetExpr::Do(vec![SubsetExpr::If {
                test: Box::new(SubsetExpr::CallPrim {
                    prim: Prim::Eq,
                    args: vec![SubsetExpr::Local(0), SubsetExpr::Int(0)],
                    span: Span::new(1, 17),
                }),
                then: Box::new(SubsetExpr::Int(0)),
                els: Box::new(SubsetExpr::CallFn {
                    index: 0,
                    args: vec![SubsetExpr::CallPrim {
                        prim: Prim::Add,
                        args: vec![SubsetExpr::Local(0), SubsetExpr::Int(-1)],
                        span: Span::new(1, 30),
                    }],
                    span: Span::new(1, 27),
                }),
            }])
        );
    }

    #[test]
    fn if_without_else_is_nil() {
        let prog = plan("(print (if true 1))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![SubsetExpr::If {
                    test: Box::new(SubsetExpr::Bool(true)),
                    then: Box::new(SubsetExpr::Int(1)),
                    els: Box::new(SubsetExpr::Nil),
                }],
                span: Span::new(1, 1),
            }]
        );
    }

    #[test]
    fn top_do_splits_and_bare_do_is_nil() {
        let prog = plan("(do (defn f [x] x) (print (f 1))) (do)").expect("plans");
        assert_eq!(prog.fns.len(), 1);
        assert_eq!(prog.main.len(), 2);
        assert_eq!(prog.main[1], SubsetExpr::Do(Vec::new()));
    }

    #[test]
    fn let_binds_flat_slots_with_shadowing() {
        let prog = plan("(let [x 1 y (+ x 1)] (* x y))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Let {
                bindings: vec![
                    (0, SubsetExpr::Int(1)),
                    (
                        1,
                        SubsetExpr::CallPrim {
                            prim: Prim::Add,
                            args: vec![SubsetExpr::Local(0), SubsetExpr::Int(1)],
                            span: Span::new(1, 13),
                        },
                    ),
                ],
                body: vec![SubsetExpr::CallPrim {
                    prim: Prim::Mul,
                    args: vec![SubsetExpr::Local(0), SubsetExpr::Local(1)],
                    span: Span::new(1, 22),
                }],
            }]
        );
        assert_eq!(prog.main_slots, 2);
        let shadow = plan("(let [x 1] (let [x 2] x))").expect("plans");
        assert_eq!(shadow.main_slots, 2);
        assert_eq!(
            shadow.main,
            vec![SubsetExpr::Let {
                bindings: vec![(0, SubsetExpr::Int(1))],
                body: vec![SubsetExpr::Let {
                    bindings: vec![(1, SubsetExpr::Int(2))],
                    body: vec![SubsetExpr::Local(1)],
                }],
            }]
        );
    }

    #[test]
    fn loop_binds_flat_slots_and_recur_plans() {
        let prog = plan("(loop [i 0] (if (= i 3) i (recur (+ i 1))))").expect("plans");
        assert_eq!(prog.main_slots, 1);
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Loop {
                bindings: vec![(0, SubsetExpr::Int(0))],
                body: vec![SubsetExpr::If {
                    test: Box::new(SubsetExpr::CallPrim {
                        prim: Prim::Eq,
                        args: vec![SubsetExpr::Local(0), SubsetExpr::Int(3)],
                        span: Span::new(1, 17),
                    }),
                    then: Box::new(SubsetExpr::Local(0)),
                    els: Box::new(SubsetExpr::Recur(vec![SubsetExpr::CallPrim {
                        prim: Prim::Add,
                        args: vec![SubsetExpr::Local(0), SubsetExpr::Int(1)],
                        span: Span::new(1, 34),
                    }])),
                }],
            }]
        );
    }

    #[test]
    fn recur_threads_through_macro_desugars() {
        // Integration (macro-forms + loop slices): a `recur` in a
        // macro branch sits in an `if` branch of the expansion, so
        // it is a tail exactly when the macro form is — while a
        // test position never is.
        let prog = plan("(loop [i 0] (cond (= i 0) 0 :else (recur (+ i -1))))")
            .expect("recur in cond branch plans");
        assert!(matches!(
            prog.main.as_slice(),
            [SubsetExpr::Loop { body, .. }] if matches!(body.as_slice(),
                [SubsetExpr::If { els, .. }] if matches!(els.as_ref(),
                    SubsetExpr::If { then, .. } if matches!(then.as_ref(), SubsetExpr::Recur(_))))
        ));
        let prog =
            plan("(loop [i 0] (when (= i 0) (recur (+ i -1))))").expect("recur in when body plans");
        assert!(matches!(
            prog.main.as_slice(),
            [SubsetExpr::Loop { body, .. }] if matches!(body.as_slice(),
                [SubsetExpr::If { then, .. }] if matches!(then.as_ref(),
                    SubsetExpr::Do(exprs) if matches!(exprs.as_slice(), [SubsetExpr::Recur(_)])))
        ));
        let prog = plan("(loop [i 0] (and (= i 0) (recur (+ i -1))))")
            .expect("recur as last and-operand plans");
        assert!(matches!(
            prog.main.as_slice(),
            [SubsetExpr::Loop { body, .. }] if matches!(body.as_slice(),
                [SubsetExpr::Let { body: arms, .. }] if matches!(arms.as_slice(),
                    [SubsetExpr::If { then, .. }] if matches!(then.as_ref(), SubsetExpr::Recur(_))))
        ));
        assert!(
            plan("(loop [i 0] (cond (recur 1) 0 :else 1))").is_err(),
            "recur in cond test rejects"
        );
        assert!(
            plan("(loop [i 0] (when (recur 1) 0))").is_err(),
            "recur in when test rejects"
        );
    }

    #[test]
    fn loop_tails_nest_and_shadow() {
        // A `loop` in non-tail position still takes `recur`
        // (self-tail, like the analyzer); multi-statement `let`
        // bodies track tails per statement.
        let prog =
            plan("(print (loop [i 0] (let [x i] (print x) (if (= x 1) x (recur (+ x 1))))))")
                .expect("self-tail plans");
        assert_eq!(prog.main_slots, 2);
        assert!(matches!(prog.main.as_slice(), [SubsetExpr::Print { .. }]));
        // Nested loops: the inner `recur` targets the inner loop;
        // slots never reuse across the nest.
        let nested =
            plan("(loop [i 0] (loop [j 1 k 2] (if (= i j) k (recur 7 8))))").expect("nested plans");
        assert_eq!(nested.main_slots, 3);
        assert!(matches!(
            nested.main.as_slice(),
            [SubsetExpr::Loop { body, .. }]
            if matches!(body.as_slice(), [SubsetExpr::Loop { .. }])
        ));
        // `recur` after a completed inner loop targets the outer
        // one (multi-statement loop bodies track tails too).
        let outer = plan("(loop [i 0] (loop [j 0] j) (if (= i 1) i (recur 1)))")
            .expect("outer recur plans");
        assert_eq!(outer.main_slots, 2);
        assert!(matches!(
            outer.main.as_slice(),
            [SubsetExpr::Loop { body, .. }] if body.len() == 2
        ));
        // Edge shapes: empty bodies yield nil, empty bindings take
        // bare `(recur)` (both plan; running the latter never
        // returns, so planning is the whole test).
        let empty = plan("(loop [i 0]) (loop [] 1) (loop [] (recur))").expect("edges plan");
        assert_eq!(empty.main_slots, 1);
        assert!(matches!(
            empty.main.as_slice(),
            [
                SubsetExpr::Loop { body, .. },
                SubsetExpr::Loop { bindings, .. },
                SubsetExpr::Loop { .. },
            ] if body.is_empty() && bindings.is_empty()
        ));
        // Loops in `defn` bodies allocate past the params.
        let defn = plan("(defn f [a] (loop [i a] (if (= i 0) a (recur (+ i -1)))))")
            .expect("defn loop plans");
        assert_eq!(defn.fns[0].frame_slots, 2);
        assert!(matches!(
            &defn.fns[0].body,
            SubsetExpr::Do(body)
            if matches!(body.as_slice(), [SubsetExpr::Loop { .. }])
        ));
    }

    #[test]
    fn wrong_argc_is_a_runtime_arity_node() {
        let prog = plan("(defn f [a] a) (print (f 1 2))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![SubsetExpr::ArityError {
                    display: "user/f".to_owned(),
                    expected: 1,
                    got: 2,
                    span: Span::new(1, 23),
                }],
                span: Span::new(1, 16),
            }]
        );
    }

    #[test]
    fn qualified_calls_plan() {
        let prog = plan("(defn f [a] a) (user/f 1) (clojure.core/+ 1 2) (clojure.core/print 3)")
            .expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::CallFn { index: 0, .. }));
        assert!(matches!(
            prog.main[1],
            SubsetExpr::CallPrim {
                prim: Prim::Add,
                ..
            }
        ));
        assert!(matches!(prog.main[2], SubsetExpr::Print { .. }));
    }

    #[test]
    fn transient_pipeline_plans_to_nested_prims() {
        let prog = plan("(print (persistent! (conj! (transient [1]) 2)))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![SubsetExpr::CallPrim {
                    prim: Prim::PersistentBang,
                    args: vec![SubsetExpr::CallPrim {
                        prim: Prim::ConjBang,
                        args: vec![
                            SubsetExpr::CallPrim {
                                prim: Prim::Transient,
                                args: vec![SubsetExpr::VecLit(vec![SubsetExpr::Int(1)])],
                                span: Span::new(1, 28),
                            },
                            SubsetExpr::Int(2),
                        ],
                        span: Span::new(1, 21),
                    }],
                    span: Span::new(1, 8),
                }],
                span: Span::new(1, 1),
            }]
        );
        let prog = plan("(print (assoc! (transient [1]) 0 9))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![SubsetExpr::CallPrim {
                    prim: Prim::AssocBang,
                    args: vec![
                        SubsetExpr::CallPrim {
                            prim: Prim::Transient,
                            args: vec![SubsetExpr::VecLit(vec![SubsetExpr::Int(1)])],
                            span: Span::new(1, 16),
                        },
                        SubsetExpr::Int(0),
                        SubsetExpr::Int(9),
                    ],
                    span: Span::new(1, 8),
                }],
                span: Span::new(1, 1),
            }]
        );
    }

    #[test]
    fn user_defn_shadows_primitive() {
        let prog = plan("(defn + [a b] a) (+ 1 2)").expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::CallFn { index: 0, .. }));
    }

    #[test]
    fn let_shadowed_prim_is_a_local() {
        let prog = plan("(let [+ 5] +)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Let {
                bindings: vec![(0, SubsetExpr::Int(5))],
                body: vec![SubsetExpr::Local(0)],
            }]
        );
    }

    #[test]
    fn forward_reference_is_an_analyzer_error() {
        assert_eq!(
            diag("(defn f [x] (g x))"),
            "f.clj:1:13: Syntax error compiling: Unable to resolve symbol: g in this context"
        );
    }

    #[test]
    fn reader_error_diagnostic() {
        assert_eq!(diag(")"), "f.clj:1:2: Unmatched delimiter: )");
    }

    #[test]
    fn analyzer_error_diagnostic() {
        assert_eq!(
            diag("(if)"),
            "f.clj:1:1: Syntax error compiling if: Too few arguments to if"
        );
    }

    #[test]
    fn bare_analyzer_failure_is_uncaught() {
        assert_eq!(
            diag("(defn)"),
            "f.clj:1:1: clojure.lang.ArityException: Wrong number of args (0) passed to: clojure.core/defn"
        );
    }

    #[test]
    fn duplicate_defn_rejected() {
        assert_eq!(
            diag("(defn f [a] a) (defn f [b] b)"),
            "f.clj:1:16: unsupported form in AOT subset: defn (redefinition; AOT resolves calls at compile time)"
        );
    }

    #[test]
    fn nested_defn_rejected() {
        assert_eq!(
            diag("(let [x 1] (defn f [a] a))"),
            "f.clj:1:12: unsupported form in AOT subset: defn (defn is top-level only)"
        );
    }

    #[test]
    fn rejection_table_names_forms() {
        // Full diagnostics: span + form + hint, pinned exactly.
        // (`@` reads as qualified `clojure.core/deref` with no span
        // of its own — the interpreter's arity texts name it
        // qualified too — so the outer span wins.)
        let cases: &[(&str, &str)] = &[
            (
                "(def x 1)",
                "f.clj:1:1: unsupported form in AOT subset: def (only defn defines in the AOT subset)",
            ),
            (
                "(print (clojure.core/loop [i 0] i))",
                "f.clj:1:8: unsupported form in AOT subset: clojure.core/loop (not in the closed call set)",
            ),
            (
                "(defn g [n] (recur n))",
                "f.clj:1:13: unsupported form in AOT subset: recur (recur outside loop is outside the subset; call the function by name)",
            ),
            (
                "(print (try 1 (catch Exception e 2)))",
                "f.clj:1:8: unsupported form in AOT subset: try (error handling is outside the AOT subset)",
            ),
            (
                "(print (var print))",
                "f.clj:1:8: unsupported form in AOT subset: var (vars are outside the AOT subset)",
            ),
            (
                "(print @(future 7))",
                "f.clj:1:1: unsupported form in AOT subset: clojure.core/deref (concurrency is outside the AOT subset)",
            ),
            (
                "(print (take 2 [1 2]))",
                "f.clj:1:8: unsupported form in AOT subset: take (not in the closed call set)",
            ),
            (
                "(println :x)",
                "f.clj:1:1: unsupported form in AOT subset: println (only print writes in the AOT subset)",
            ),
            (
                "(print (read))",
                "f.clj:1:8: unsupported form in AOT subset: read (only print writes in the AOT subset)",
            ),
            (
                "(print (read-line))",
                "f.clj:1:8: unsupported form in AOT subset: read-line (only print writes in the AOT subset)",
            ),
            (
                "(print (line-seq *in*))",
                "f.clj:1:8: unsupported form in AOT subset: line-seq (only print writes in the AOT subset)",
            ),
            (
                "(with-in-str \"x\" 1)",
                "f.clj:1:1: unsupported form in AOT subset: with-in-str (macros are not expanded in the AOT subset)",
            ),
            (
                "(ns foo)",
                "f.clj:1:1: unsupported form in AOT subset: ns (AOT programs are single-file, without namespaces)",
            ),
            (
                "(if-let [a 1] a)",
                "f.clj:1:1: unsupported form in AOT subset: if-let (macros are not expanded in the AOT subset)",
            ),
            (
                "(defmacro m [x] x)",
                "f.clj:1:1: unsupported form in AOT subset: defmacro (macros at runtime are outside the subset)",
            ),
            (
                "(print ((fn [x] x) 7))",
                "f.clj:1:9: unsupported form in AOT subset: fn (only defn defines functions in the AOT subset)",
            ),
            (
                "(print (quote 5))",
                "f.clj:1:8: unsupported form in AOT subset: quote (quoted values are outside the value flow)",
            ),
            (
                "(defn m ([a] a) ([a b] b))",
                "f.clj:1:1: unsupported form in AOT subset: defn (multi-arity; use a single vector clause)",
            ),
            (
                "(defn m ([a] a) ([a b] b) {:a 1})",
                "f.clj:1:1: unsupported form in AOT subset: defn (multi-arity; use a single vector clause)",
            ),
            (
                "(defn v [a & b] a)",
                "f.clj:1:1: unsupported form in AOT subset: defn (variadic; use fixed params)",
            ),
            (
                "(defn p [a] {:pre [true]} a)",
                "f.clj:1:1: unsupported form in AOT subset: defn (pre/post conditions are runtime checks)",
            ),
            (
                "(let [[a b] [1 2]] a)",
                "f.clj:1:1: unsupported form in AOT subset: let (destructuring is outside the subset; bind plain symbols)",
            ),
            (
                "(loop [[a b] [1 2]] a)",
                "f.clj:1:1: unsupported form in AOT subset: loop (destructuring is outside the subset; bind plain symbols)",
            ),
            (
                "(print (map inc [1]))",
                "f.clj:1:8: unsupported form in AOT subset: map (not in the closed call set)",
            ),
            (
                "(print (future 1))",
                "f.clj:1:8: unsupported form in AOT subset: future (concurrency is outside the AOT subset)",
            ),
            (
                "(print (lazy-seq [1]))",
                "f.clj:1:8: unsupported form in AOT subset: lazy-seq (laziness is outside the AOT subset)",
            ),
            (
                "(letfn [(f [x] x)] (f 1))",
                "f.clj:1:1: unsupported form in AOT subset: letfn (use top-level defn instead)",
            ),
            (
                "(print (str 1))",
                "f.clj:1:8: unsupported form in AOT subset: str (not in the closed call set)",
            ),
            (
                "(do (def x 1) (print x))",
                "f.clj:1:5: unsupported form in AOT subset: def (only defn defines in the AOT subset)",
            ),
            (
                "(print (loop [i 0] i))\n(def x 1)",
                "f.clj:2:1: unsupported form in AOT subset: def (only defn defines in the AOT subset)",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(&diag(input), want, "{input}");
        }
    }

    #[test]
    fn keyword_literals_flow() {
        // T73 item 8: keywords are subset values (plain and
        // namespaced), including nested in vectors.
        let prog = plan(":miss [:a :u/id]").expect("keywords plan");
        assert_eq!(
            prog.main,
            vec![
                SubsetExpr::Keyword(Keyword::new(None, "miss")),
                SubsetExpr::VecLit(vec![
                    SubsetExpr::Keyword(Keyword::new(None, "a")),
                    SubsetExpr::Keyword(Keyword::new(Some("u"), "id")),
                ]),
            ]
        );
    }

    #[test]
    fn value_literals_outside_the_flow() {
        assert_eq!(
            diag("{:a 1}"),
            "f.clj:1:1: unsupported form in AOT subset: {:a 1} (only Int/Double/Bool/Str/Keyword/Vector values flow)"
        );
        assert_eq!(
            diag("()"),
            "f.clj:1:1: unsupported form in AOT subset: () (lists are outside the value flow)"
        );
    }

    #[test]
    fn bare_symbols_name_vars() {
        assert_eq!(
            diag("(print map)"),
            "f.clj:1:1: unsupported form in AOT subset: map (vars are outside the AOT subset)"
        );
        assert_eq!(
            diag("(print clojure.core/map)"),
            "f.clj:1:1: unsupported form in AOT subset: clojure.core/map (vars are outside the AOT subset)"
        );
        assert_eq!(
            diag("(defn f [a] a) (print f)"),
            "f.clj:1:16: unsupported form in AOT subset: f (function values are outside the subset; call it directly)"
        );
        assert_eq!(
            diag("(defn f [a] a) (print user/f)"),
            "f.clj:1:16: unsupported form in AOT subset: user/f (function values are outside the subset; call it directly)"
        );
    }

    #[test]
    fn qualified_reads_ignore_shadowing_locals() {
        // R1 finding 3: locals shadow unqualified symbols only —
        // a qualified read under a same-named `let` is still the
        // var, never the local (else `(let [+ 1] ...)` would
        // silently miscompile `clojure.core/+` to `1`).
        assert_eq!(
            diag("(let [+ 1] (print clojure.core/+))"),
            "f.clj:1:12: unsupported form in AOT subset: clojure.core/+ (vars are outside the AOT subset)"
        );
        assert_eq!(
            diag("(defn v [a] a) (let [v 1] user/v)"),
            "f.clj:1:16: unsupported form in AOT subset: user/v (function values are outside the subset; call it directly)"
        );
    }

    #[test]
    fn higher_order_calls_rejected() {
        assert_eq!(
            diag("(let [f 1] (f 2))"),
            "f.clj:1:12: unsupported form in AOT subset: f (higher-order calls are outside the subset)"
        );
        assert_eq!(
            diag("([1 2] 0)"),
            "f.clj:1:1: unsupported form in AOT subset: [1 2] (higher-order calls are outside the subset)"
        );
    }

    #[test]
    fn defn_shapes_accepted() {
        plan("(defn f \"doc\" {:a 1} [x] x)").expect("doc+attr");
        plan("(defn f ([x] x))").expect("single list clause");
        plan("(clojure.core/defn f [x] x)").expect("qualified defn");
        // A lone map body is a body form (rejected as a map
        // literal), not pre/post — which names `defn`.
        assert_eq!(
            diag("(defn f [x] {:a 1})"),
            "f.clj:1:1: unsupported form in AOT subset: {:a 1} (only Int/Double/Bool/Str/Keyword/Vector values flow)"
        );
    }

    #[test]
    fn nested_do_sequences_in_expression_position() {
        let body_ok = |program: &SubsetProgram| {
            program.main.is_empty()
                && program.fns.len() == 1
                && program.fns[0].frame_slots == 1
                && matches!(&program.fns[0].body, SubsetExpr::Do(outer)
                    if matches!(outer.as_slice(), [SubsetExpr::Do(seq)] if seq.len() == 2))
        };
        assert!(matches!(plan("(defn f [x] (do (+ x 1) (* x 2)))"), Ok(p) if body_ok(&p)));
        assert!(matches!(plan("(if true (do 1 2) 3)"), Ok(p)
            if matches!(p.main.as_slice(), [SubsetExpr::If { then, .. }]
                if matches!(**then, SubsetExpr::Do(_)))));
    }

    #[test]
    fn cond_desugars_to_nested_if() {
        let prog = plan("(cond false 1 (= 1 2) 2 :else 3)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::If {
                test: Box::new(SubsetExpr::Bool(false)),
                then: Box::new(SubsetExpr::Int(1)),
                els: Box::new(SubsetExpr::If {
                    test: Box::new(SubsetExpr::CallPrim {
                        prim: Prim::Eq,
                        args: vec![SubsetExpr::Int(1), SubsetExpr::Int(2)],
                        span: Span::new(1, 15),
                    }),
                    then: Box::new(SubsetExpr::Int(2)),
                    els: Box::new(SubsetExpr::If {
                        test: Box::new(SubsetExpr::Keyword(Keyword::new(None, "else"))),
                        then: Box::new(SubsetExpr::Int(3)),
                        els: Box::new(SubsetExpr::Nil),
                    }),
                }),
            }]
        );
        // Empty is nil; a missing else is nil (no implicit `:else`).
        let prog = plan("(cond) (cond false 1)").expect("plans");
        assert_eq!(
            prog.main,
            vec![
                SubsetExpr::Nil,
                SubsetExpr::If {
                    test: Box::new(SubsetExpr::Bool(false)),
                    then: Box::new(SubsetExpr::Int(1)),
                    els: Box::new(SubsetExpr::Nil),
                },
            ]
        );
    }

    #[test]
    fn and_or_short_circuit_through_anonymous_slots() {
        // `(and 1 2)`: the first operand binds an anonymous slot;
        // falsy yields the slot, truthy falls to the tail.
        let prog = plan("(and 1 2)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Let {
                bindings: vec![(0, SubsetExpr::Int(1))],
                body: vec![SubsetExpr::If {
                    test: Box::new(SubsetExpr::Local(0)),
                    then: Box::new(SubsetExpr::Int(2)),
                    els: Box::new(SubsetExpr::Local(0)),
                }],
            }]
        );
        assert_eq!(prog.main_slots, 1);
        // `(or 1 2)`: mirrored — truthy yields the slot.
        let prog = plan("(or 1 2)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Let {
                bindings: vec![(0, SubsetExpr::Int(1))],
                body: vec![SubsetExpr::If {
                    test: Box::new(SubsetExpr::Local(0)),
                    then: Box::new(SubsetExpr::Local(0)),
                    els: Box::new(SubsetExpr::Int(2)),
                }],
            }]
        );
        // Three operands nest (each operand evaluates at most once).
        let prog = plan("(and 1 2 3)").expect("plans");
        assert_eq!(prog.main_slots, 2);
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Let {
                bindings: vec![(0, SubsetExpr::Int(1))],
                body: vec![SubsetExpr::If {
                    test: Box::new(SubsetExpr::Local(0)),
                    then: Box::new(SubsetExpr::Let {
                        bindings: vec![(1, SubsetExpr::Int(2))],
                        body: vec![SubsetExpr::If {
                            test: Box::new(SubsetExpr::Local(1)),
                            then: Box::new(SubsetExpr::Int(3)),
                            els: Box::new(SubsetExpr::Local(1)),
                        }],
                    }),
                    els: Box::new(SubsetExpr::Local(0)),
                }],
            }]
        );
    }

    #[test]
    fn and_or_degenerate_arities() {
        // The macros' own edge shapes: `(and)` is true, `(or)` is
        // nil, a single operand is itself.
        let prog = plan("(and) (or) (and 5) (or 6)").expect("plans");
        assert_eq!(
            prog.main,
            vec![
                SubsetExpr::Bool(true),
                SubsetExpr::Nil,
                SubsetExpr::Int(5),
                SubsetExpr::Int(6),
            ]
        );
    }

    #[test]
    fn when_branches_over_implicit_do() {
        let prog = plan("(when (> 2 1) 7 8)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::If {
                test: Box::new(SubsetExpr::CallPrim {
                    prim: Prim::Gt,
                    args: vec![SubsetExpr::Int(2), SubsetExpr::Int(1)],
                    span: Span::new(1, 7),
                }),
                then: Box::new(SubsetExpr::Do(vec![SubsetExpr::Int(7), SubsetExpr::Int(8)])),
                els: Box::new(SubsetExpr::Nil),
            }]
        );
        let prog = plan("(when-not (= 1 1) 9)").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::If {
                test: Box::new(SubsetExpr::CallPrim {
                    prim: Prim::Eq,
                    args: vec![SubsetExpr::Int(1), SubsetExpr::Int(1)],
                    span: Span::new(1, 11),
                }),
                then: Box::new(SubsetExpr::Nil),
                els: Box::new(SubsetExpr::Do(vec![SubsetExpr::Int(9)])),
            }]
        );
        // An empty body is an empty `do` (nil at runtime, like the
        // macro's `(if test (do))`).
        let prog = plan("(when true) (when-not false)").expect("plans");
        assert_eq!(
            prog.main,
            vec![
                SubsetExpr::If {
                    test: Box::new(SubsetExpr::Bool(true)),
                    then: Box::new(SubsetExpr::Do(Vec::new())),
                    els: Box::new(SubsetExpr::Nil),
                },
                SubsetExpr::If {
                    test: Box::new(SubsetExpr::Bool(false)),
                    then: Box::new(SubsetExpr::Nil),
                    els: Box::new(SubsetExpr::Do(Vec::new())),
                },
            ]
        );
    }

    #[test]
    fn qualified_macro_forms_plan() {
        // Macros are vars: qualified invocation is the macro.
        let prog = plan(
            "(clojure.core/cond false 1 :else 2) (clojure.core/and 1 2) \
             (clojure.core/or 1 2) (clojure.core/when true 1) (clojure.core/when-not false 1)",
        )
        .expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::If { .. }));
        assert!(matches!(prog.main[1], SubsetExpr::Let { .. }));
        assert!(matches!(prog.main[2], SubsetExpr::Let { .. }));
        assert!(matches!(prog.main[3], SubsetExpr::If { .. }));
        assert!(matches!(prog.main[4], SubsetExpr::If { .. }));
    }

    #[test]
    fn sibling_defn_shadows_macro_form() {
        // A redefined macro var is a plain call (like primitives);
        // the qualified spelling stays the macro.
        let prog = plan("(defn and [x] x) (and 1) (clojure.core/and 1 2)").expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::CallFn { index: 0, .. }));
        assert!(matches!(prog.main[1], SubsetExpr::Let { .. }));
        let prog = plan("(defn cond [a b] a) (cond 1 2)").expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::CallFn { index: 0, .. }));
    }

    #[test]
    fn locals_shadow_macro_form_heads() {
        assert_eq!(
            diag("(let [when 1] (when true 2))"),
            "f.clj:1:15: unsupported form in AOT subset: when (higher-order calls are outside the subset)"
        );
    }

    #[test]
    fn long_and_int_plan_as_prims() {
        let prog = plan("(print (long 5.9) (int 7) (clojure.core/long 1))").expect("plans");
        assert_eq!(
            prog.main,
            vec![SubsetExpr::Print {
                args: vec![
                    SubsetExpr::CallPrim {
                        prim: Prim::Long,
                        args: vec![SubsetExpr::Double(5.9)],
                        span: Span::new(1, 8),
                    },
                    SubsetExpr::CallPrim {
                        prim: Prim::Int,
                        args: vec![SubsetExpr::Int(7)],
                        span: Span::new(1, 19),
                    },
                    SubsetExpr::CallPrim {
                        prim: Prim::Long,
                        args: vec![SubsetExpr::Int(1)],
                        span: Span::new(1, 27),
                    },
                ],
                span: Span::new(1, 1),
            }]
        );
        // Siblings shadow cast prims, exactly like `+`.
        let prog = plan("(defn long [x] x) (long 5)").expect("plans");
        assert!(matches!(prog.main[0], SubsetExpr::CallFn { index: 0, .. }));
    }

    #[test]
    fn malformed_macro_forms_fail_before_classification() {
        // An odd `cond` tail and a missing `when` test die in
        // macroexpansion (validation runs first), never reaching
        // the planner's trust arms.
        assert_eq!(
            diag("(cond 1)"),
            "f.clj:1:1: Syntax error macroexpanding cond: cond requires an even number of forms"
        );
        assert_eq!(
            diag("(when)"),
            "f.clj:1:1: clojure.lang.ArityException: Wrong number of args (0) passed to: clojure.core/when"
        );
    }

    #[test]
    fn top_level_non_symbol_head_classifies_head_first() {
        assert_eq!(
            diag("((fn [x] x) 1)"),
            "f.clj:1:2: unsupported form in AOT subset: fn (only defn defines functions in the AOT subset)"
        );
    }

    #[test]
    fn prim_resolve_table() {
        let all = [
            ("+", Prim::Add),
            ("-", Prim::Sub),
            ("*", Prim::Mul),
            ("quot", Prim::Quot),
            ("<", Prim::Lt),
            ("<=", Prim::Le),
            (">", Prim::Gt),
            (">=", Prim::Ge),
            ("=", Prim::Eq),
            ("count", Prim::Count),
            ("nth", Prim::Nth),
            ("get", Prim::Get),
            ("conj", Prim::Conj),
            ("long", Prim::Long),
            ("int", Prim::Int),
            ("transient", Prim::Transient),
            ("conj!", Prim::ConjBang),
            ("assoc!", Prim::AssocBang),
            ("persistent!", Prim::PersistentBang),
        ];
        for (name, prim) in all {
            assert_eq!(Prim::resolve(None, name), Some(prim));
            assert_eq!(Prim::resolve(Some("clojure.core"), name), Some(prim));
        }
        assert_eq!(Prim::resolve(None, "map"), None);
        assert_eq!(Prim::resolve(Some("user"), "+"), None);
    }

    #[test]
    fn hint_table_lines() {
        let heads = [
            "def", "fn", "letfn", "try", "var", "defmacro", "future", "lazy-seq", "println", "ns",
            "when", "quote", "map",
        ];
        for head in heads {
            assert!(!head_hint(head).is_empty(), "{head}");
        }
    }

    #[test]
    fn short_pr_truncates() {
        assert_eq!(short_pr(&sym("x")), "x");
        let long = Value::Str(Arc::from("y".repeat(100).as_str()));
        let text = short_pr(&long);
        assert_eq!(text.chars().count(), 81);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn lift_and_diagnostic_cover_all_variants() {
        assert!(matches!(
            lift(SourceError::Internal("boom".to_owned())),
            PlanError::Internal(_)
        ));
        assert_eq!(
            PlanError::Internal("boom".to_owned()).diagnostic("f.clj"),
            "f.clj:1:1: internal error: boom"
        );
    }

    fn defn_items(parts: &[Value]) -> Vec<&Value> {
        parts.iter().collect()
    }

    #[test]
    fn parse_defn_malformed_table() {
        let defn = sym("defn");
        let one = Value::Int(1);
        // Too short.
        assert!(matches!(
            parse_defn(&defn_items(std::slice::from_ref(&defn))),
            DefnShape::Malformed
        ));
        // Non-symbol name.
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), one.clone()])),
            DefnShape::Malformed
        ));
        // Namespaced name.
        let qual = Value::Symbol(Symbol::new(Some("a"), "f"));
        let argv = Value::Vector(Vector::from_vec(vec![sym("x")]));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), qual, argv.clone()])),
            DefnShape::Malformed
        ));
        // Empty/scalar fdecl.
        let f = sym("f");
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone()])),
            DefnShape::Malformed
        ));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone(), one.clone()])),
            DefnShape::Malformed
        ));
        // Empty clause / non-vector argv / bad params.
        let empty = Value::List(List::from_vec(Vec::new()));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone(), empty])),
            DefnShape::Malformed
        ));
        let bad_argv = Value::List(List::from_vec(vec![sym("a")]));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone(), bad_argv])),
            DefnShape::Malformed
        ));
        let bad_param = Value::Vector(Vector::from_vec(vec![one.clone()]));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone(), bad_param])),
            DefnShape::Malformed
        ));
        let ns_param = Value::Vector(Vector::from_vec(vec![Value::Symbol(Symbol::new(
            Some("a"),
            "b",
        ))]));
        assert!(matches!(
            parse_defn(&defn_items(&[defn.clone(), f.clone(), ns_param])),
            DefnShape::Malformed
        ));
        // Attr-only fdecl.
        let attr = Value::Map(new_map());
        assert!(matches!(
            parse_defn(&defn_items(&[defn, f, attr])),
            DefnShape::Malformed
        ));
    }

    #[test]
    fn classify_defn_rejects_non_list_and_malformed_directly() {
        let mut planner = Planner::new();
        let span = Span::new(3, 4);
        let r = planner.classify_defn(&Value::Int(1), span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.span == span));
        let form = Value::List(List::from_vec(vec![sym("defn"), Value::Int(1)]));
        let r = planner.classify_defn(&form, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.form == "defn"));
    }

    #[test]
    fn classify_let_shape_trusts_directly() {
        let mut planner = Planner::new();
        let mut scope = Scope::default();
        let mut next = 0;
        let span = Span::new(1, 1);
        // No bindings element.
        let empty: &[&Value] = &[];
        assert!(
            planner
                .classify_let(empty, &mut scope, &mut next, None, span)
                .is_err()
        );
        // Non-vector bindings.
        let one = Value::Int(1);
        let args = vec![&one];
        assert!(
            planner
                .classify_let(&args, &mut scope, &mut next, None, span)
                .is_err()
        );
        // Odd bindings.
        let odd = Value::Vector(Vector::from_vec(vec![sym("x")]));
        let args = vec![&odd];
        assert!(
            planner
                .classify_let(&args, &mut scope, &mut next, None, span)
                .is_err()
        );
        // Namespaced binding.
        let qual = Value::Symbol(Symbol::new(Some("a"), "b"));
        let vec = Value::Vector(Vector::from_vec(vec![qual, Value::Int(1)]));
        let args = vec![&vec];
        let r = planner.classify_let(&args, &mut scope, &mut next, None, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.hint.contains("destructuring")));
    }

    #[test]
    fn classify_cond_and_when_shape_trusts_directly() {
        let mut planner = Planner::new();
        let mut scope = Scope::default();
        let mut next = 0;
        let span = Span::new(1, 1);
        // Odd cond tail (macroexpansion throws first).
        let one = Value::Int(1);
        let args = vec![&one];
        let r = planner.classify_cond(&args, &mut scope, &mut next, None, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.form == "cond"));
        // Missing when/when-not test (macro arity throws first).
        let empty: &[&Value] = &[];
        let r = planner.classify_when(false, empty, &mut scope, &mut next, None, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.form == "when"));
        let r = planner.classify_when(true, empty, &mut scope, &mut next, None, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.form == "when-not"));
    }

    #[test]
    fn classify_loop_shape_trusts_directly() {
        let mut planner = Planner::new();
        let mut scope = Scope::default();
        let mut next = 0;
        let span = Span::new(1, 1);
        // No bindings element.
        let empty: &[&Value] = &[];
        assert!(
            planner
                .classify_loop(empty, &mut scope, &mut next, span)
                .is_err()
        );
        // Non-vector bindings.
        let one = Value::Int(1);
        let args = vec![&one];
        assert!(
            planner
                .classify_loop(&args, &mut scope, &mut next, span)
                .is_err()
        );
        // Odd bindings.
        let odd = Value::Vector(Vector::from_vec(vec![sym("x")]));
        let args = vec![&odd];
        assert!(
            planner
                .classify_loop(&args, &mut scope, &mut next, span)
                .is_err()
        );
        // Namespaced binding.
        let qual = Value::Symbol(Symbol::new(Some("a"), "b"));
        let vec = Value::Vector(Vector::from_vec(vec![qual, Value::Int(1)]));
        let args = vec![&vec];
        let r = planner.classify_loop(&args, &mut scope, &mut next, span);
        assert!(matches!(&r, Err(PlanError::Subset(e)) if e.hint.contains("destructuring")));
    }

    #[test]
    fn classify_recur_fences_directly() {
        let mut planner = Planner::new();
        let mut scope = Scope::default();
        let mut next = 0;
        let span = Span::new(1, 1);
        let one = Value::Int(1);
        let args = vec![&one];
        // Non-tail `recur` (validation-first: the analyzer rejects
        // these before classification runs).
        let nontail = Some(RecurCtx {
            arity: 1,
            tail: false,
        });
        let r = planner.classify_recur(&args, &mut scope, &mut next, nontail, span);
        assert!(
            matches!(&r, Err(PlanError::Subset(e)) if e.form == "recur" && e.hint.contains("tail position"))
        );
        // Arity mismatch.
        let wrong = Some(RecurCtx {
            arity: 2,
            tail: true,
        });
        let r = planner.classify_recur(&args, &mut scope, &mut next, wrong, span);
        assert!(
            matches!(&r, Err(PlanError::Subset(e)) if e.form == "recur" && e.hint.contains("match the loop bindings"))
        );
        // No target (fn-targeted `recur`, which validates).
        let r = planner.classify_recur(&args, &mut scope, &mut next, None, span);
        assert!(
            matches!(&r, Err(PlanError::Subset(e)) if e.form == "recur" && e.hint.contains("outside loop"))
        );
    }

    /// `depth` nested parens around `1`.
    fn parens(depth: usize) -> String {
        format!("{}1{}", "(".repeat(depth), ")".repeat(depth))
    }

    #[test]
    fn nesting_accepts_shallow_source() {
        assert_eq!(DepthScan::scan("(defn f [x] (+ x 1))"), None);
        assert_eq!(DepthScan::scan("(do (f 1) (f 2))"), None);
        assert_eq!(DepthScan::scan(""), None);
    }

    #[test]
    fn nesting_rejects_forty_one_deep() {
        assert_eq!(DepthScan::scan(&parens(41)), Some((Span::new(1, 41), 41)));
    }

    #[test]
    fn nesting_accepts_exactly_forty() {
        assert_eq!(DepthScan::scan(&parens(40)), None);
    }

    #[test]
    fn nesting_rejects_unbalanced_deep() {
        assert_eq!(
            DepthScan::scan(&"(".repeat(50)),
            Some((Span::new(1, 41), 41))
        );
    }

    #[test]
    fn nesting_ignores_unmatched_closers() {
        assert_eq!(DepthScan::scan("()))"), None);
    }

    #[test]
    fn nesting_counts_mixed_delimiters() {
        assert_eq!(
            DepthScan::scan(&"([{".repeat(14)),
            Some((Span::new(1, 41), 41))
        );
    }

    #[test]
    fn nesting_counts_quote_runs() {
        assert_eq!(
            DepthScan::scan(&format!("{}x", "'".repeat(41))),
            Some((Span::new(1, 41), 41))
        );
        assert_eq!(DepthScan::scan(&format!("{}@x", "^".repeat(39))), None);
        assert_eq!(
            DepthScan::scan(&format!("{}@@x", "^".repeat(39))),
            Some((Span::new(1, 41), 41))
        );
    }

    #[test]
    fn nesting_folds_quotes_into_openers() {
        assert_eq!(
            DepthScan::scan(&format!("{}''1{}", "(".repeat(38), ")".repeat(38))),
            None
        );
        assert_eq!(
            DepthScan::scan(&format!("{}''1{}", "(".repeat(39), ")".repeat(39))),
            Some((Span::new(1, 41), 41))
        );
    }

    #[test]
    fn nesting_ignores_parens_in_strings() {
        assert_eq!(DepthScan::scan("(print \"(((\")"), None);
        assert_eq!(DepthScan::scan("(f \"a\\\"(b\")"), None);
    }

    #[test]
    fn nesting_ignores_parens_in_comments() {
        assert_eq!(DepthScan::scan("; ((((\n(+ 1 1)"), None);
    }

    #[test]
    fn nesting_skips_char_literals() {
        assert_eq!(DepthScan::scan("(= \\( x)"), None);
        assert_eq!(DepthScan::scan("(f 1)\\"), None);
    }

    #[test]
    fn nesting_skips_backslash_newline() {
        assert_eq!(DepthScan::scan("\\\n(+ 1 1)"), None);
    }

    #[test]
    fn nesting_skips_regex_bodies() {
        assert_eq!(DepthScan::scan("#\"(((\""), None);
    }

    #[test]
    fn nesting_counts_fn_and_set_dispatch() {
        assert_eq!(DepthScan::scan("(map #(+ % 1) xs)"), None);
        assert_eq!(DepthScan::scan("#{:a #{:b}}"), None);
    }

    #[test]
    fn nesting_counts_quote_discard_and_conditional() {
        assert_eq!(DepthScan::scan("(def x #'foo)"), None);
        assert_eq!(DepthScan::scan("(do #_ (ignored [x]) :kept)"), None);
        assert_eq!(DepthScan::scan("(do #?@(:clj [1]))"), None);
    }

    #[test]
    fn nesting_counts_discarded_forms() {
        assert_eq!(
            DepthScan::scan(&format!("#_{}", parens(41))),
            Some((Span::new(1, 41), 41))
        );
    }

    #[test]
    fn nesting_counts_tagged_literals() {
        assert_eq!(DepthScan::scan("#inst \"2020-01-01\""), None);
    }

    #[test]
    fn nesting_counts_namespaced_maps() {
        assert_eq!(DepthScan::scan("#:person{:name \"x\"}"), None);
    }

    #[test]
    fn nesting_counts_stacked_hyphen_tags() {
        // H1: the bump must survive `-` (the old letter-run skip
        // reset on it, so stacked `#a-1` tags measured ~2 while the
        // reader recursed per tag). Each tag costs 2: 20 pass at
        // the limit, the 21st rejects.
        assert_eq!(DepthScan::scan(&"#a-1 ".repeat(20)), None);
        assert_eq!(
            DepthScan::scan(&"#a-1 ".repeat(21)),
            Some((Span::new(1, 104), 42))
        );
    }

    #[test]
    fn nesting_counts_double_colon_aliases() {
        // The second `:` is tag-internal (reader token), not a reset.
        assert_eq!(DepthScan::scan(&"#::a ".repeat(20)), None);
        assert_eq!(
            DepthScan::scan(&"#::a ".repeat(21)),
            Some((Span::new(1, 104), 42))
        );
    }

    #[test]
    fn nesting_weights_backquote_eightfold() {
        // M2: each backtick costs 8 (syntax-quote multiplies
        // read-output depth ~3x per level). Five pass at the
        // limit; the sixth rejects.
        assert_eq!(DepthScan::scan("`````x"), None);
        assert_eq!(DepthScan::scan("``````x"), Some((Span::new(1, 6), 48)));
    }

    #[test]
    fn nesting_rejects_backquote_run_at_plan() {
        // Eight ticks: the pre-scan rejects (no "after expansion"
        // suffix). Pre-fix the scan passed and only the rescan
        // caught it at 3280 deep — and ten ticks crashed the
        // reader outright (see the 1 MB test in emit.rs).
        assert_eq!(
            diag("````````x"),
            "f.clj:1:6: nesting depth 48 exceeds the AOT build limit (40)"
        );
    }

    #[test]
    fn nesting_rejects_stacked_tags_at_plan() {
        // 200 stacked tags: the pre-scan rejects (no "after
        // expansion" suffix). Pre-fix the scan passed and only the
        // rescan caught it — and 20000 crashed the reader outright.
        assert_eq!(
            diag(&"#a-1 ".repeat(200)),
            "f.clj:1:104: nesting depth 42 exceeds the AOT build limit (40)"
        );
    }

    #[test]
    fn nesting_ignores_lone_hash() {
        assert_eq!(DepthScan::scan("(let [x# 1] x#)"), None);
    }

    #[test]
    fn nesting_accepts_wide_shallow() {
        assert_eq!(
            DepthScan::scan(&format!("(do {} )", "(f 1) ".repeat(100))),
            None
        );
    }

    #[test]
    fn value_depth_scores_nesting() {
        let mut eval = boot();
        let forms = eval
            .read_forms("([1] {:a 1} #{2})", &mut clj_reader::NoHooks)
            .expect("read");
        let tagged = clj_value::tagged_literal(Symbol::new(None, "foo"), Value::Int(1));
        let cond = clj_value::reader_conditional(Value::Int(2), true);
        let mixed = Value::List(List::from_vec(vec![
            forms[0].clone(),
            tagged,
            cond,
            Value::Int(3),
        ]));
        assert_eq!(value_depth(&mixed), 3);
        assert_eq!(value_depth(&Value::Int(1)), 0);
    }

    #[test]
    fn nesting_rejects_deep_source_at_plan() {
        assert_eq!(
            diag(&parens(41)),
            "f.clj:1:41: nesting depth 41 exceeds the AOT build limit (40)"
        );
    }

    #[test]
    fn nesting_rejects_nested_macro_expansion() {
        // B1: the rescan measures nested macro calls unexpanded, so
        // only the analyze-time budget catches this (pre-fix: clean
        // subset rejection at 300 steps, SIGABRT at 30000).
        let cond_src = format!("(do (cond {}))", "true 1 ".repeat(300));
        for (src, span) in [
            (format!("(do (-> 0{}))", " (+ 1)".repeat(300)), "1:5"),
            (format!("(defn f [] (-> 0{}))", " (+ 1)".repeat(300)), "1:1"),
            (cond_src, "1:5"),
            // Crash scale (SIGABRT pre-fix): the budget trips at 41
            // regardless of how deep the expansion runs.
            (format!("(do (-> 0{}))", " (+ 1)".repeat(30_000)), "1:5"),
        ] {
            assert_eq!(
                diag(&src),
                format!(
                    "f.clj:{span}: nesting depth 41 after expansion exceeds the AOT build limit (40)"
                ),
                "{src:.40}"
            );
        }
    }

    #[test]
    fn nesting_rejects_deep_expansion() {
        let threaded = format!("(-> 0{} )", " (+ 1)".repeat(300));
        assert_eq!(
            diag(&threaded),
            "f.clj:1:1: nesting depth 300 after expansion exceeds the AOT build limit (40)"
        );
    }

    #[test]
    fn nesting_reports_threading_as_unsupported() {
        assert_eq!(
            diag("(-> 0 (+ 1) (+ 2))"),
            "f.clj:1:1: unsupported form in AOT subset: -> (not in the closed call set)"
        );
    }

    #[test]
    fn nesting_reports_deep_unsupported_source() {
        assert_eq!(
            diag(&parens(40)),
            "f.clj:1:40: unsupported form in AOT subset: 1 (higher-order calls are outside the subset)"
        );
    }

    #[test]
    fn nesting_backstops_user_macros() {
        assert_eq!(
            diag("(defmacro m [x] x)"),
            "f.clj:1:1: unsupported form in AOT subset: defmacro (macros at runtime are outside the subset)"
        );
    }
}
