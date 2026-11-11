// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Mixed AOT planning (issue #256, spec D1/D2): every program
//! builds. Top-level `defn`s the subset planner accepts become
//! native [`SubsetFn`]s. Everything else stays in the embedded
//! source, which the binary loads at startup through `run`'s own
//! pipeline.
//!
//! The embedded source is the `\r\n`-normalized program text with
//! each native `defn` masked in place. The form's first chars
//! become `(clj.aot/n K)` (the runtime interns native `K` under
//! its name at exactly that load position), and every other
//! non-newline char of the form becomes a space. Lines and
//! columns of every embedded form are therefore unchanged, so
//! spans and line-based diagnostics match `run`.
//!
//! Planning is conservative: anything that could make a native
//! `defn` observe something different from the interpreted one
//! demotes it (redefinition, var rebinding, `print` bindings, or
//! a call into a demoted sibling). The fallback is always the
//! exact `run` path, so demotion never changes behavior.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use clj_reader::Span;
use clj_value::Value;

use super::{DepthScan, Planner, SubsetExpr, SubsetFn, is_defn_head};
use crate::error::span_of;
use crate::ns::boot;

/// The runtime namespace holding the native-intern hook (`n`).
pub const MIXED_INTERN_NS: &str = "clj.aot";

/// Symbols whose presence anywhere makes var identity observable
/// in ways a native direct-call graph cannot follow: every
/// `defn` stays embedded when one appears.
const REBINDING_SYMBOLS: [&str; 7] = [
    "alter-var-root",
    "with-redefs",
    "with-redefs-fn",
    "intern",
    "ns-unmap",
    "refer-clojure",
    "alter-meta!",
];

/// Top-level heads that (re)define the var their second element
/// names (a native `defn` sharing that name is demoted).
const DEF_HEADS: [&str; 7] = [
    "def", "defn", "defn-", "defonce", "defmacro", "defmulti", "deftype",
];

/// A mixed build plan.
#[derive(Clone, Debug)]
pub struct MixedProgram {
    /// Native `defn`s in load order; [`SubsetExpr::CallFn`]
    /// indices address this vector.
    pub fns: Vec<SubsetFn>,
    /// The embedded source (normalized, native `defn`s masked).
    pub source: String,
    /// Top-level `defn` forms seen (native or not).
    pub defns: usize,
    /// Top-level forms left in the embedded source.
    pub embedded: usize,
}

impl MixedProgram {
    /// The one-line `build --report` summary (spec 1.3 #5).
    #[must_use]
    pub fn report(&self) -> String {
        format!(
            "aot: native {}/{} defns, embedded {} top-level forms",
            self.fns.len(),
            self.defns,
            self.embedded
        )
    }
}

/// The in-place replacement for native `index`.
#[must_use]
pub fn intern_call(index: usize) -> String {
    format!("({MIXED_INTERN_NS}/n {index})")
}

/// `\r\n` and lone `\r` become `\n`, matching the reader's own
/// normalization, so char ranges index this text directly.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// One top-level form as read.
struct TopForm {
    form: Value,
    range: Range<usize>,
}

/// A native candidate: its plan index plus where to mask it.
struct Candidate {
    plan_index: usize,
    name: String,
    /// Char index of the form's first char (its `(`).
    start: usize,
    range: Range<usize>,
}

/// Plan `text` for a mixed build. Infallible: when nothing can go
/// native (unreadable source, pathological nesting, rebinding),
/// the whole program is embedded unmasked and behaves exactly
/// like `run`, including any read-time failure.
#[must_use]
pub fn plan_mixed(text: &str) -> MixedProgram {
    let source = normalize(text);
    let all_embedded = |defns: usize, embedded: usize| MixedProgram {
        fns: Vec::new(),
        source: source.clone(),
        defns,
        embedded,
    };
    if DepthScan::scan(&source).is_some() {
        return all_embedded(0, 0);
    }
    let mut eval = boot();
    eval.analyze_depth_limit = Some(super::MAX_NESTING_DEPTH);
    let Ok((forms, ranges)) = eval.read_forms_ranged(&source, &mut clj_reader::NoHooks) else {
        return all_embedded(0, 0);
    };
    let tops: Vec<TopForm> = forms
        .into_iter()
        .zip(ranges)
        .map(|(form, range)| TopForm { form, range })
        .collect();
    let defns = tops.iter().filter(|t| defn_name(&t.form).is_some()).count();
    if tops.iter().any(|t| mentions_rebinding(&t.form)) {
        return all_embedded(defns, tops.len());
    }
    let line_starts = line_starts(&source);
    let mut planner = Planner::new();
    let mut candidates = Vec::new();
    let mut redefined: HashSet<String> = HashSet::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for top in &tops {
        if let Some(name) = def_target(&top.form) {
            *seen.entry(name.to_owned()).or_default() += 1;
        }
        let Some(name) = defn_name(&top.form) else {
            continue;
        };
        let snapshot = planner.snapshot();
        let planned = planner.plan_top(&mut eval, &top.form).is_ok();
        if !planned || planner.fns.len() != snapshot.fns + 1 {
            planner.restore(snapshot);
            redefined.insert(name.to_owned());
            continue;
        }
        let span = span_of(&top.form, Span::new(1, 1));
        candidates.push(Candidate {
            plan_index: snapshot.fns,
            name: name.to_owned(),
            start: char_index(&line_starts, span),
            range: top.range.clone(),
        });
    }
    redefined.extend(seen.into_iter().filter(|(_, n)| *n > 1).map(|(k, _)| k));
    let chars: Vec<char> = source.chars().collect();
    let keep = select_natives(&planner.fns, &candidates, &redefined, &chars);
    build_program(planner.fns, &candidates, &keep, chars, defns, tops.len())
}

/// Which candidates stay native: drop redefined names, `print`
/// users, and forms too short to hold their replacement, then
/// close over sibling calls (a caller of a dropped fn drops).
fn select_natives(
    fns: &[SubsetFn],
    candidates: &[Candidate],
    redefined: &HashSet<String>,
    chars: &[char],
) -> Vec<bool> {
    let widest = intern_call(candidates.len()).chars().count();
    let mut keep: Vec<bool> = candidates
        .iter()
        .map(|c| {
            !redefined.contains(&c.name)
                && !uses_print(&fns[c.plan_index].body)
                && fits(chars, c.start, c.range.end, widest)
        })
        .collect();
    let by_plan: HashMap<usize, usize> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| (c.plan_index, i))
        .collect();
    loop {
        let mut changed = false;
        for (i, c) in candidates.iter().enumerate() {
            if keep[i]
                && calls(&fns[c.plan_index].body)
                    .iter()
                    .any(|p| !keep[by_plan[p]])
            {
                keep[i] = false;
                changed = true;
            }
        }
        if !changed {
            return keep;
        }
    }
}

/// Renumber the kept natives, rewrite their sibling calls, and
/// mask them into the embedded source.
fn build_program(
    planned: Vec<SubsetFn>,
    candidates: &[Candidate],
    keep: &[bool],
    mut chars: Vec<char>,
    defns: usize,
    tops: usize,
) -> MixedProgram {
    let mut renumber = HashMap::new();
    for (c, _) in candidates.iter().zip(keep).filter(|(_, k)| **k) {
        renumber.insert(c.plan_index, renumber.len());
    }
    let mut fns = Vec::with_capacity(renumber.len());
    for (c, _) in candidates.iter().zip(keep).filter(|(_, k)| **k) {
        let mut def = planned[c.plan_index].clone();
        renumber_calls(&mut def.body, &renumber);
        let index = fns.len();
        for slot in &mut chars[c.start..c.range.end] {
            if *slot != '\n' {
                *slot = ' ';
            }
        }
        for (slot, ch) in chars[c.start..].iter_mut().zip(intern_call(index).chars()) {
            *slot = ch;
        }
        fns.push(def);
    }
    MixedProgram {
        embedded: tops - fns.len(),
        fns,
        source: chars.into_iter().collect(),
        defns,
    }
}

/// Char index of each line's first char (index 0 is line 1).
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, ch) in source.chars().enumerate() {
        if ch == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The char index of a 1-based `(line, col)` span.
fn char_index(line_starts: &[usize], span: Span) -> usize {
    let line = (span.line as usize).saturating_sub(1);
    let col = (span.col as usize).saturating_sub(1);
    line_starts.get(line).copied().unwrap_or(0) + col
}

/// Whether `width` chars from `start` stay on one line and inside
/// the form (`start..end`).
fn fits(chars: &[char], start: usize, end: usize, width: usize) -> bool {
    start + width <= end && !chars[start..start + width].contains(&'\n')
}

/// The name a surface `(defn name ...)` defines.
fn defn_name(form: &Value) -> Option<&str> {
    let Value::List(list) = form else {
        return None;
    };
    let mut items = list.iter();
    match (items.next(), items.next()) {
        (Some(Value::Symbol(head)), Some(Value::Symbol(name))) if is_defn_head(head) => {
            Some(name.name())
        }
        _ => None,
    }
}

/// The var a surface def-like top form (re)defines.
fn def_target(form: &Value) -> Option<&str> {
    let Value::List(list) = form else {
        return None;
    };
    let mut items = list.iter();
    match (items.next(), items.next()) {
        (Some(Value::Symbol(head)), Some(Value::Symbol(name)))
            if DEF_HEADS.contains(&head.name()) =>
        {
            Some(name.name())
        }
        _ => None,
    }
}

/// Whether a form mentions any [`REBINDING_SYMBOLS`] entry (as a
/// symbol or keyword, at any depth).
fn mentions_rebinding(form: &Value) -> bool {
    let mut stack = vec![form.clone()];
    while let Some(value) = stack.pop() {
        match value {
            Value::Symbol(s) if REBINDING_SYMBOLS.contains(&s.name()) => return true,
            Value::Keyword(k) if REBINDING_SYMBOLS.contains(&k.name()) => return true,
            Value::List(l) => stack.extend(l.iter().cloned()),
            Value::Vector(v) => stack.extend(v.iter()),
            Value::Map(m) => {
                for (k, v) in m.iter() {
                    stack.push(k.clone());
                    stack.push(v.clone());
                }
            }
            Value::Set(s) => stack.extend(s.iter().cloned()),
            _ => {}
        }
    }
    false
}

/// Every sub-expression of `expr` (pre-order).
fn children(expr: &SubsetExpr) -> Vec<&SubsetExpr> {
    match expr {
        SubsetExpr::VecLit(items)
        | SubsetExpr::Do(items)
        | SubsetExpr::Recur(items)
        | SubsetExpr::CallPrim { args: items, .. }
        | SubsetExpr::CallFn { args: items, .. }
        | SubsetExpr::Print { args: items, .. } => items.iter().collect(),
        SubsetExpr::If { test, then, els } => vec![test, then, els],
        SubsetExpr::Let { bindings, body } | SubsetExpr::Loop { bindings, body } => bindings
            .iter()
            .map(|(_, init)| init)
            .chain(body.iter())
            .collect(),
        _ => Vec::new(),
    }
}

/// The mutable twin of [`children`].
fn children_mut(expr: &mut SubsetExpr) -> Vec<&mut SubsetExpr> {
    match expr {
        SubsetExpr::VecLit(items)
        | SubsetExpr::Do(items)
        | SubsetExpr::Recur(items)
        | SubsetExpr::CallPrim { args: items, .. }
        | SubsetExpr::CallFn { args: items, .. }
        | SubsetExpr::Print { args: items, .. } => items.iter_mut().collect(),
        SubsetExpr::If { test, then, els } => vec![test, then, els],
        SubsetExpr::Let { bindings, body } | SubsetExpr::Loop { bindings, body } => bindings
            .iter_mut()
            .map(|(_, init)| init)
            .chain(body.iter_mut())
            .collect(),
        _ => Vec::new(),
    }
}

/// Plan indices of every sibling `expr` calls.
fn calls(expr: &SubsetExpr) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        if let SubsetExpr::CallFn { index, .. } = e {
            out.push(*index);
        }
        stack.extend(children(e));
    }
    out
}

/// Whether `expr` prints (native `print` would bypass the
/// interpreter's `*out*` binding).
fn uses_print(expr: &SubsetExpr) -> bool {
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        if matches!(e, SubsetExpr::Print { .. }) {
            return true;
        }
        stack.extend(children(e));
    }
    false
}

/// Rewrite sibling call indices through `renumber` (every callee
/// of a kept fn is kept, by the closure in [`select_natives`]).
fn renumber_calls(expr: &mut SubsetExpr, renumber: &HashMap<usize, usize>) {
    if let SubsetExpr::CallFn { index, .. } = expr {
        *index = renumber[index];
    }
    for child in children_mut(expr) {
        renumber_calls(child, renumber);
    }
}

#[cfg(test)]
mod tests {
    use super::{intern_call, normalize, plan_mixed};
    use crate::aot_subset::SubsetExpr;

    /// Every embedded line keeps its position: masking only
    /// replaces non-newline chars.
    fn same_shape(a: &str, b: &str) -> bool {
        a.chars().count() == b.chars().count()
            && a.chars()
                .zip(b.chars())
                .all(|(x, y)| (x == '\n') == (y == '\n'))
    }

    #[test]
    fn native_defn_is_masked_in_place() {
        let src = "(defn sq [x] (* x x))\n(println (map sq [1 2 3]))\n";
        let plan = plan_mixed(src);
        assert_eq!(plan.fns.len(), 1);
        assert_eq!(plan.fns[0].name, "sq");
        assert_eq!(plan.defns, 1);
        assert_eq!(plan.embedded, 1);
        assert!(
            plan.source.starts_with("(clj.aot/n 0)        \n"),
            "{:?}",
            plan.source
        );
        assert!(plan.source.contains("(println (map sq [1 2 3]))"));
        assert!(same_shape(src, &plan.source));
        assert_eq!(
            plan.report(),
            "aot: native 1/1 defns, embedded 1 top-level forms"
        );
    }

    #[test]
    fn sibling_calls_are_renumbered() {
        // `bad` uses `str` (outside the subset), so it stays
        // embedded and the natives renumber around it.
        let src = "(defn bad [x] (str x))\n(defn a [x] (+ x 1))\n(defn b [x] (a (a x)))\n";
        let plan = plan_mixed(src);
        let names: Vec<_> = plan.fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let SubsetExpr::Do(body) = &plan.fns[1].body else {
            panic!("do body");
        };
        let SubsetExpr::CallFn { index, .. } = &body[0] else {
            panic!("call");
        };
        assert_eq!(*index, 0);
        assert!(plan.source.contains("(defn bad [x] (str x))"));
        assert!(plan.source.contains(&intern_call(1)));
        assert!(same_shape(src, &plan.source));
    }

    #[test]
    fn demotion_closes_over_callers() {
        // `a` is redefined later, so `a` and its caller `b` stay
        // embedded; `c` is independent.
        let src = "(defn a [x] (+ x 1))\n(defn b [x] (a x))\n(defn c [x] (* x 2))\n(def a 3)\n";
        let plan = plan_mixed(src);
        let names: Vec<_> = plan.fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["c"]);
        assert_eq!(plan.defns, 3);
        assert_eq!(plan.embedded, 3);
    }

    #[test]
    fn duplicate_defns_both_stay_embedded() {
        let src = "(defn a [x] (+ x 1))\n(defn a [x] (+ x 2))\n";
        assert!(plan_mixed(src).fns.is_empty());
    }

    #[test]
    fn printing_and_short_defns_stay_embedded() {
        let src = "(defn p [x] (print x))\n(defn f[]1)\n(defn g\n  [x]\n  (+ x 1))\n";
        let plan = plan_mixed(src);
        assert!(plan.fns.is_empty(), "{:?}", plan.fns);
        assert_eq!(plan.source, src);
    }

    #[test]
    fn rebinding_keeps_everything_embedded() {
        for src in [
            "(defn a [x] (+ x 1))\n(with-redefs [a inc] (a 1))\n",
            "(ns foo (:refer-clojure :exclude [nth]))\n(defn a [x] (+ x 1))\n",
            "(defn a [x] (+ x 1))\n(alter-var-root #'a (constantly inc))\n",
            "(defn a [x] (+ x 1))\n{:k #{[(intern *ns* 'z 1)]}}\n",
        ] {
            let plan = plan_mixed(src);
            assert!(plan.fns.is_empty(), "{src}");
            assert_eq!(plan.source, src);
            assert_eq!(plan.defns, 1);
        }
    }

    #[test]
    fn unreadable_or_deep_sources_embed_unchanged() {
        let plan = plan_mixed("(defn a [x] (+ x 1))\n(oops");
        assert!(plan.fns.is_empty());
        assert_eq!(plan.source, "(defn a [x] (+ x 1))\n(oops");
        let deep = format!("{}1{}", "(".repeat(60), ")".repeat(60));
        let plan = plan_mixed(&deep);
        assert!(plan.fns.is_empty());
        assert_eq!(plan.source, deep);
    }

    #[test]
    fn crlf_is_normalized_and_leading_trivia_kept() {
        assert_eq!(normalize("a\r\nb\rc"), "a\nb\nc");
        let src = ";; header\r\n\r\n(defn sq [x] (* x x))\r\n(sq 2)\r\n";
        let plan = plan_mixed(src);
        assert_eq!(plan.fns.len(), 1);
        assert!(
            plan.source.starts_with(";; header\n\n(clj.aot/n 0)"),
            "{:?}",
            plan.source
        );
        assert!(same_shape(&normalize(src), &plan.source));
    }

    #[test]
    fn non_defn_tops_and_unplannable_defns_embed() {
        // A multi-arity defn fails planning; a non-list top and a
        // def-like head with a non-symbol name are plain embeds.
        let src = "(defn m ([] 1) ([x] x))\n42\n(def)\n(defn ok [x] (- x))\n";
        let plan = plan_mixed(src);
        let names: Vec<_> = plan.fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["ok"]);
        assert_eq!(plan.defns, 2);
        assert_eq!(plan.embedded, 3);
    }

    #[test]
    fn loop_and_let_bodies_are_walked() {
        let src = "(defn a [x] (+ x 1))\n\
                   (defn b [n] (loop [i 0 acc [1]] (if (< i n) (recur (+ i 1) (let [y (a i)] [y])) acc)))\n";
        let plan = plan_mixed(src);
        assert_eq!(plan.fns.len(), 2, "{:?}", plan.source);
    }
}
