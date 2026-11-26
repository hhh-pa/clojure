// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by the
// terms of this license.

//! Build-time folding of closed integer tables (#271 Slice 6g).
//!
//! A `def` whose init is `(mapv F (range …))` (literal bounds) or
//! `(mapv F table)` over an earlier folded table, with `F` a literal
//! one-arg `fn` or a core fn by name, computes its value when the program
//! starts: the same longs every run. Here a bounded evaluator runs `F` per
//! item at build time, and the program builds the result from the longs
//! in one runtime call. The evaluator knows `let`, `if`, `do`,
//! `loop`/`recur`, an empty `[]`, `conj` of a long onto a vector, `count`
//! of one, `not`, and the core long natives (called through their own
//! entries, so values and overflow errors are the runtime's). Anything
//! else, any error, a step budget run out, or a result other than longs or
//! rows of longs leaves the init to run as compiled.

use std::collections::HashMap;

use clj_eval::Evaluator;
use clj_eval::expr::{Arity, DirectCall, Expr, Pattern};
use clj_eval::ext::NativeTarget;
use clj_value::Value;

/// Longs a folded table may hold.
const MAX_ITEMS: usize = 1 << 16;
/// Expressions one fold may evaluate.
const MAX_STEPS: usize = 1 << 22;

/// A folded init: longs as one vector (`ends` empty), or rows of longs cut
/// at the cumulative `ends`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Table {
    pub(super) items: Vec<i64>,
    pub(super) ends: Vec<u64>,
}

impl Table {
    /// The table's values, one per item (each row as a fresh vector of
    /// longs: only results that are longs come of folding over them).
    fn values(&self) -> Vec<Folded> {
        if self.ends.is_empty() {
            return self.items.iter().map(|&n| Folded::Int(n)).collect();
        }
        let mut start = 0;
        self.ends
            .iter()
            .map(|&end| {
                let row = self.items[start..end as usize].to_vec();
                start = end as usize;
                Folded::Row(row)
            })
            .collect()
    }
}

/// A value while folding. Vectors are only ever an empty `[]` literal (by
/// site: the runtime's `[]` at one site is one object, so the rows that
/// are it stay one) or a vector `conj` made (each a new one). `Recur`
/// carries a `recur`'s operands to its loop (analysis puts `recur` in
/// tail position only, so no other form ever receives one).
#[derive(Clone, Debug, PartialEq)]
enum Folded {
    Nil,
    Bool(bool),
    Int(i64),
    Empty(usize),
    Row(Vec<i64>),
    Recur(Vec<Folded>),
}

impl Folded {
    fn truthy(&self) -> bool {
        !matches!(self, Self::Nil | Self::Bool(false))
    }
}

/// The items a fold runs `F` over.
pub(super) enum Source<'a> {
    /// A literal `range`'s longs.
    Range(&'a [Expr]),
    /// An earlier folded table: `F` must return longs over its rows.
    Table(&'a Table),
}

/// Fold `(mapv F source)` with `F` as the one-arity `fun` whose param
/// takes env index `base`, or `None`.
pub(super) fn fold(fun: &Arity, base: usize, source: Source<'_>) -> Option<Table> {
    let (items, from_table) = match source {
        Source::Range(args) => (range_items(args)?, false),
        Source::Table(t) => (t.values(), true),
    };
    let mut folder = Folder {
        runtime: Evaluator::new_bare("<folded table>"),
        base,
        steps: MAX_STEPS,
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let mut env = vec![item];
        out.push(folder.forms(&fun.body, &mut env)?);
    }
    // Over a table's rows a row result could be one of them; over a range
    // the rows' empties must all be the one `[]` (one object at run time).
    (!from_table || out.iter().all(|v| matches!(v, Folded::Int(_)))).then_some(())?;
    let mut sites = out.iter().filter_map(|v| match v {
        Folded::Empty(site) => Some(*site),
        _ => None,
    });
    let first = sites.next();
    sites.all(|site| Some(site) == first).then_some(())?;
    table(out)
}

/// The longs of `(range args…)` with literal long bounds.
fn range_items(args: &[Expr]) -> Option<Vec<Folded>> {
    let mut bounds = [0i64; 3];
    for (arg, bound) in args.iter().zip(&mut bounds) {
        let Expr::Literal(Value::Int(n)) = arg else {
            return None;
        };
        *bound = *n;
    }
    let (_, count, start, step) = clj_eval::native_abi::range_plan(args.len(), bounds)?;
    let count = usize::try_from(count)
        .ok()
        .filter(|&c| c > 0 && c <= MAX_ITEMS)?;
    Some(
        (0..count as i64)
            .map(|i| Folded::Int(start + i * step))
            .collect(),
    )
}

/// The results as a [`Table`]: all longs, or all rows.
fn table(values: Vec<Folded>) -> Option<Table> {
    let mut t = Table {
        items: Vec::new(),
        ends: Vec::new(),
    };
    let rows = !matches!(values.first()?, Folded::Int(_));
    for v in values {
        match (v, rows) {
            (Folded::Int(n), false) => t.items.push(n),
            (Folded::Row(r), true) => t.items.extend(r),
            (Folded::Empty(_), true) => {}
            _ => return None,
        }
        if rows {
            t.ends.push(t.items.len() as u64);
        }
        (t.items.len() <= MAX_ITEMS).then_some(())?;
    }
    Some(t)
}

struct Folder {
    runtime: Evaluator,
    /// The env index of `F`'s param (locals below it are the enclosing
    /// code's: an `F` reading one is not closed).
    base: usize,
    steps: usize,
}

impl Folder {
    /// Forms in order; the last one's value.
    fn forms(&mut self, body: &[Expr], env: &mut Vec<Folded>) -> Option<Folded> {
        let (last, init) = body.split_last()?;
        for e in init {
            self.eval(e, env)?;
        }
        self.eval(last, env)
    }

    /// Control forms here, values in [`leaf`](Self::leaf).
    fn eval(&mut self, e: &Expr, env: &mut Vec<Folded>) -> Option<Folded> {
        self.steps = self.steps.checked_sub(1)?;
        match e {
            Expr::If { test, then, els } => {
                let branch = if self.eval(test, env)?.truthy() {
                    then
                } else {
                    els
                };
                self.eval(branch, env)
            }
            Expr::Do(forms) => self.forms(forms, env),
            Expr::Let { bindings, body } => {
                let mark = env.len();
                self.bind(bindings, env)?;
                let out = self.forms(body, env);
                env.truncate(mark);
                out
            }
            Expr::Loop { bindings, body } => self.run_loop(bindings, body, env),
            Expr::Recur(args) => args
                .iter()
                .map(|a| self.eval(a, env))
                .collect::<Option<Vec<_>>>()
                .map(Folded::Recur),
            _ => self.leaf(e, env),
        }
    }

    fn leaf(&mut self, e: &Expr, env: &mut Vec<Folded>) -> Option<Folded> {
        match e {
            Expr::Literal(v) | Expr::Quote(v) => literal(v),
            Expr::VecLit(items, None) if items.is_empty() => Some(Folded::Empty(expr_site(e))),
            Expr::Local(i) => env.get(i.checked_sub(self.base)?).cloned(),
            Expr::DirectNative(call) => self.call(call, env),
            _ => None,
        }
    }

    /// Push `bindings`' values (plain names only).
    fn bind(&mut self, bindings: &[(Pattern, Expr)], env: &mut Vec<Folded>) -> Option<()> {
        for (p, init) in bindings {
            matches!(p, Pattern::Symbol(_)).then_some(())?;
            let v = self.eval(init, env)?;
            env.push(v);
        }
        Some(())
    }

    fn run_loop(
        &mut self,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        env: &mut Vec<Folded>,
    ) -> Option<Folded> {
        let mark = env.len();
        self.bind(bindings, env)?;
        loop {
            let out = self.forms(body, env)?;
            env.truncate(mark);
            match out {
                Folded::Recur(vals) => env.extend(vals),
                v => return Some(v),
            }
        }
    }

    /// A `clojure.core` call the fold knows: the long ops by their own
    /// native entries, the vector ops here.
    fn call(&mut self, call: &DirectCall, env: &mut Vec<Folded>) -> Option<Folded> {
        let (Some("clojure.core"), NativeTarget::Ptr(native)) =
            (call.var.sym.namespace(), &call.func)
        else {
            return None;
        };
        let args = call
            .args
            .iter()
            .map(|a| self.eval(a, env))
            .collect::<Option<Vec<_>>>()?;
        let name = call.var.sym.name();
        if !LONG_OPS.contains(&name) {
            return coll_op(name, &args);
        }
        let longs = args
            .iter()
            .map(|x| match x {
                Folded::Int(n) => Some(Value::Int(*n)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        literal(&native(&mut self.runtime, &longs).ok()?)
    }
}

/// `conj` of a long onto a vector, `count` of one, and `not`.
fn coll_op(name: &str, args: &[Folded]) -> Option<Folded> {
    match (name, args) {
        ("conj", [Folded::Empty(_), Folded::Int(n)]) => Some(Folded::Row(vec![*n])),
        ("conj", [Folded::Row(r), Folded::Int(n)]) => Some(Folded::Row([&r[..], &[*n]].concat())),
        ("count", [Folded::Empty(_)]) => Some(Folded::Int(0)),
        ("count", [Folded::Row(r)]) => Some(Folded::Int(r.len() as i64)),
        ("not", [x]) => Some(Folded::Bool(!x.truthy())),
        _ => None,
    }
}

/// The core natives the fold calls over longs.
const LONG_OPS: [&str; 33] = [
    "+",
    "-",
    "*",
    "inc",
    "dec",
    "quot",
    "rem",
    "mod",
    "max",
    "min",
    "abs",
    "<",
    "<=",
    ">",
    ">=",
    "=",
    "==",
    "not=",
    "zero?",
    "pos?",
    "neg?",
    "even?",
    "odd?",
    "bit-and",
    "bit-or",
    "bit-xor",
    "bit-not",
    "bit-and-not",
    "bit-shift-left",
    "bit-shift-right",
    "unsigned-bit-shift-right",
    "bit-test",
    "bit-set",
];

/// A literal's value: `nil`, a boolean or a long (a literal vector is a
/// pooled constant the fold could not keep shared, so it declines).
fn literal(v: &Value) -> Option<Folded> {
    match v {
        Value::Nil => Some(Folded::Nil),
        Value::Bool(b) => Some(Folded::Bool(*b)),
        Value::Int(n) => Some(Folded::Int(*n)),
        _ => None,
    }
}

/// An expression node's address (one `[]` site).
fn expr_site(e: &Expr) -> usize {
    std::ptr::from_ref(e) as usize
}

/// The folded tables of earlier `def`s, by var (`ns`, `name`).
pub(super) type Known = HashMap<(String, String), Table>;
