// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Last-use analysis for the register VM (#226).
//!
//! A local read that no later evaluation of the same frame can
//! observe may move the value out of its register instead of
//! cloning it. Persistent collections then arrive uniquely owned at
//! the natives that consume their argument (`conj`, `assoc`, …),
//! which update in place instead of path-copying — the loop
//! accumulator `(recur (conj acc x))` stops copying per step.
//!
//! The analysis walks one fn body backward in the VM's evaluation
//! order, tracking which of the fn's own lexical locals (indexes at
//! or past its `base`) are still read later. `recur` jumps to its
//! target's head, whose live set comes from a fixed point; every
//! form the analysis does not model precisely — `try` (a handler may
//! still read anything), closure creation (captures copy), escapes
//! to the tree evaluator, `:or` defaults — reads every visible local,
//! so no read before it is a last use.

use std::collections::HashSet;

use crate::expr::{DirectCall, Expr, Pattern, pattern_count};

/// The last-use reads of one body, by node address (the analyzed
/// tree outlives its compilation).
pub(crate) type LastUses = HashSet<*const Expr>;

/// Lexical locals at or past `base`, as a bit set.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct Live(Vec<u64>);

impl Live {
    fn contains(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    fn insert(&mut self, i: usize) {
        if self.0.len() <= i / 64 {
            self.0.resize(i / 64 + 1, 0);
        }
        self.0[i / 64] |= 1 << (i % 64);
    }

    fn remove(&mut self, i: usize) {
        if let Some(w) = self.0.get_mut(i / 64) {
            *w &= !(1 << (i % 64));
        }
    }

    fn union(&mut self, other: &Live) {
        for (i, w) in other.0.iter().enumerate() {
            if self.0.len() <= i {
                self.0.push(0);
            }
            self.0[i] |= w;
        }
    }

    fn normalized(mut self) -> Self {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }
}

/// The last uses in a body whose locals start at lexical index
/// `base`, with `visible` locals in scope at its start.
pub(crate) fn last_uses(body: &[Expr], base: usize, visible: usize) -> LastUses {
    let mut w = Walk {
        base,
        visible,
        recur_after: Live::default(),
        record: true,
        last: HashSet::new(),
    };
    let mut live = Live::default();
    w.seq(body, &mut live);
    w.last
}

/// Whether evaluating `expr` may read lexical local `local` (at or
/// past `base`, with `visible` locals in scope): conservative —
/// every form the analysis does not model reads every visible local.
pub(crate) fn may_read(expr: &Expr, base: usize, visible: usize, local: usize) -> bool {
    let mut w = Walk {
        base,
        visible,
        recur_after: Live::default(),
        record: false,
        last: HashSet::new(),
    };
    let mut live = Live::default();
    w.walk(expr, &mut live);
    local >= base && live.contains(local - base)
}

struct Walk {
    base: usize,
    visible: usize,
    /// What a `recur` here leaves live (the target head's set minus
    /// the rebound locals).
    recur_after: Live,
    record: bool,
    last: LastUses,
}

impl Walk {
    fn read(&mut self, e: &Expr, i: usize, live: &mut Live) {
        if i < self.base {
            return;
        }
        let rel = i - self.base;
        if self.record && !live.contains(rel) {
            self.last.insert(e as *const Expr);
        }
        live.insert(rel);
    }

    /// Every visible local is read here.
    fn read_all(&self, live: &mut Live) {
        for i in self.base..self.visible.max(self.base) {
            live.insert(i - self.base);
        }
    }

    fn kill(&self, start: usize, count: usize, live: &mut Live) {
        for i in start..start + count {
            if i >= self.base {
                live.remove(i - self.base);
            }
        }
    }

    /// Forms in order (walked last to first).
    fn seq(&mut self, forms: &[Expr], live: &mut Live) {
        for f in forms.iter().rev() {
            self.walk(f, live);
        }
    }

    fn walk(&mut self, e: &Expr, live: &mut Live) {
        match e {
            Expr::Literal(_)
            | Expr::Quote(_)
            | Expr::VarRef(_)
            | Expr::Global(_)
            | Expr::NewThrowable { .. } => {}
            Expr::Local(i) => self.read(e, *i, live),
            Expr::If { test, then, els } => {
                let mut other = live.clone();
                self.walk(els, &mut other);
                self.walk(then, live);
                live.union(&other);
                self.walk(test, live);
            }
            Expr::Do(body) => self.seq(body, live),
            Expr::Invoke { head, args, .. } => {
                self.seq(args, live);
                self.walk(head, live);
            }
            Expr::DirectNative(call) => self.direct(call, live),
            _ => self.walk_form(e, live),
        }
    }

    /// The binding, data, and control forms (split from [`walk`]
    /// for the complexity budget).
    fn walk_form(&mut self, e: &Expr, live: &mut Live) {
        match e {
            Expr::Let { bindings, body } => self.bindings(bindings, body, live, false),
            Expr::Loop { bindings, body } => self.bindings(bindings, body, live, true),
            Expr::Recur(args) => {
                *live = self.recur_after.clone();
                self.seq(args, live);
            }
            Expr::VecLit(elems, _) | Expr::SetLit(elems, _) => self.seq(elems, live),
            Expr::MapLit(pairs, _) => self.pairs(pairs, live),
            Expr::Throw(inner) => {
                *live = Live::default();
                self.walk(inner, live);
            }
            Expr::Def { init, meta, .. } => {
                if let Some(pairs) = meta {
                    self.pairs(pairs, live);
                }
                if let Some(init) = init {
                    self.walk(init, live);
                }
            }
            _ => self.walk_control(e, live),
        }
    }

    /// `case*`, `assert`, and the conservative rest (split from
    /// [`walk_form`] for the complexity budget).
    fn walk_control(&mut self, e: &Expr, live: &mut Live) {
        match e {
            Expr::CaseStar {
                expr,
                default,
                cases,
                ..
            } => {
                let mut out = live.clone();
                self.walk(default, &mut out);
                for (_, test, then) in cases {
                    let mut branch = live.clone();
                    self.walk(then, &mut branch);
                    self.walk(test, &mut branch);
                    out.union(&branch);
                }
                *live = out;
                self.walk(expr, live);
            }
            Expr::Assert { test, message, .. } => {
                // A failing assert throws right after its message.
                let mut failing = Live::default();
                if let Some(m) = message {
                    self.walk(m, &mut failing);
                }
                live.union(&failing);
                self.walk(test, live);
            }
            _ => self.read_all(live),
        }
    }

    /// Map pairs in order (key, then value).
    fn pairs(&mut self, pairs: &[(Expr, Expr)], live: &mut Live) {
        for (k, v) in pairs.iter().rev() {
            self.walk(v, live);
            self.walk(k, live);
        }
    }

    /// A direct call in the VM's order: specialized ops and
    /// intrinsics evaluate their computed operands first and read
    /// locals in place after (so those reads come last, and never
    /// move); every other direct call evaluates its operands in
    /// order.
    fn direct(&mut self, call: &DirectCall, live: &mut Live) {
        if crate::vm_ops::update_order(call) {
            return self.update(call, live);
        }
        let in_place =
            crate::vm_ops::in_place(call) || crate::vm_intrinsics::intrinsic_of(call).is_some();
        if !in_place {
            self.seq(&call.args, live);
            return;
        }
        for a in &call.args {
            if let Expr::Local(i) = a {
                if *i >= self.base {
                    live.insert(*i - self.base);
                }
            }
        }
        for a in call.args.iter().rev() {
            if !matches!(a, Expr::Local(_) | Expr::Literal(_)) {
                self.walk(a, live);
            }
        }
    }

    /// An update op in the VM's order (see `vm_ops::upd`): a
    /// computed collection first, then the other operands in order,
    /// then a local collection's read — last, so it can be the last
    /// use even when the other operands read the same local.
    fn update(&mut self, call: &DirectCall, live: &mut Live) {
        let (coll, rest) = call
            .args
            .split_first()
            .expect("update ops take a collection");
        let local = matches!(coll, Expr::Local(_));
        if local {
            self.walk(coll, live);
        }
        self.seq(rest, live);
        if !local {
            self.walk(coll, live);
        }
    }

    /// `let`/`loop` bindings: each init runs with the earlier names
    /// in scope, then its own names bind (destructuring `:or`
    /// defaults read every local visible before them).
    fn bindings(
        &mut self,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        live: &mut Live,
        is_loop: bool,
    ) {
        let v0 = self.visible;
        let counts: Vec<usize> = bindings.iter().map(|(p, _)| pattern_count(p)).collect();
        let total: usize = counts.iter().sum();
        self.visible = v0 + total;
        if is_loop {
            self.loop_body(body, v0, total, live);
        } else {
            self.seq(body, live);
        }
        let mut start = v0 + total;
        for ((pat, init), count) in bindings.iter().zip(&counts).rev() {
            start -= count;
            self.kill(start, *count, live);
            self.visible = start;
            if has_defaults(pat) {
                self.read_all(live);
            }
            self.walk(init, live);
        }
        self.visible = v0;
    }

    /// A `loop` body: the head's live set is the fixed point of
    /// walking the body with `recur` leaving the head's set minus
    /// the loop locals (`[v0, v0 + total)`).
    fn loop_body(&mut self, body: &[Expr], v0: usize, total: usize, live: &mut Live) {
        let record = std::mem::replace(&mut self.record, false);
        let saved = std::mem::take(&mut self.recur_after);
        let mut head = Live::default();
        let out = live.clone();
        loop {
            let mut after = head.clone();
            self.kill(v0, total, &mut after);
            self.recur_after = after;
            let mut l = out.clone();
            self.seq(body, &mut l);
            let l = l.normalized();
            if l == head {
                break;
            }
            head = l;
        }
        self.record = record;
        let mut l = out;
        self.seq(body, &mut l);
        self.recur_after = saved;
        *live = l;
    }
}

/// Whether a pattern (or any nested one) has `:or` defaults.
pub(crate) fn has_defaults(pat: &Pattern) -> bool {
    match pat {
        Pattern::Symbol(_) => false,
        Pattern::Vec(v) => v.fixed.iter().chain(v.rest.as_deref()).any(has_defaults),
        Pattern::Map(m) => !m.or.is_empty() || m.entries.iter().any(|(p, ..)| has_defaults(p)),
    }
}
