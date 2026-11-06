// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Native expanders for the `core.clj` macros nearly every script
//! uses (#226). Each returns exactly what its `core.clj` definition
//! returns — the same (unqualified or `clojure.core/`) symbols,
//! gensym shapes, and errors — so expansion skips the interpreted
//! macro and its definition is never forced. The `core.clj` form
//! stays the var's metadata source (`core_lazy` forces it on a
//! metadata read only). Each native is the macro fn: `&form` and
//! `&env` are the first two arguments.

use clj_value::{List, Map, Symbol, Value};

use crate::error::NativeError;
use crate::eval::Evaluator;
use clj_value::ThrowClass;

use crate::fx4_form::{
    form_head, gs, iae, is_vector_val, kw, lst, qcall, str_of, sym, ucall, vector_items, vecv,
};
use crate::lazy::{collect_all, is_seq_value};
use crate::ns::signal_to_native;

type Expand = fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError>;

/// The native-backed `core.clj` macros, by name.
pub(crate) const CORE_MACROS: &[(&str, Expand)] = &[
    ("when", expand_when),
    ("when-not", expand_when_not),
    ("if-not", expand_if_not),
    ("and", expand_and),
    ("or", expand_or),
    ("cond", expand_cond),
    ("->", expand_thread_first),
    ("->>", expand_thread_last),
    ("comment", expand_comment),
    ("while", expand_while),
    ("defn", expand_defn),
    ("defn-", expand_defn_private),
    ("for", expand_for),
    ("doseq", expand_doseq),
    ("..", expand_dotdot),
];

/// The user args after `&form`/`&env`, with at least `min` of them
/// (a short call is the macro's own arity error, counted by the
/// expansion wrapper without the hidden params).
fn user_args<'a>(display: &str, args: &'a [Value], min: usize) -> Result<&'a [Value], NativeError> {
    if args.len() < min + 2 {
        return Err(NativeError::arity(display, args.len()));
    }
    Ok(&args[2..])
}

/// `(apply list 'do body)`.
fn do_body(body: &[Value]) -> Value {
    ucall("do", body.to_vec())
}

/// `[test & body]` ⇒ `(if test (do body…))`.
fn expand_when(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/when", args, 1)?;
    Ok(ucall("if", vec![rest[0].clone(), do_body(&rest[1..])]))
}

/// `[test & body]` ⇒ `(if test nil (do body…))`.
fn expand_when_not(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/when-not", args, 1)?;
    Ok(ucall(
        "if",
        vec![rest[0].clone(), Value::Nil, do_body(&rest[1..])],
    ))
}

/// `[test then]` ⇒ `(clojure.core/if-not test then nil)`;
/// `[test then else]` ⇒ `(if test else then)`.
fn expand_if_not(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/if-not", args, 2)?;
    match rest {
        [test, then] => Ok(qcall(
            "if-not",
            vec![test.clone(), then.clone(), Value::Nil],
        )),
        [test, then, els] => Ok(ucall("if", vec![test.clone(), els.clone(), then.clone()])),
        _ => Err(NativeError::arity("clojure.core/if-not", args.len())),
    }
}

/// `and`/`or` share their shape: one gensym'd `let*` around an `if`
/// that recurs on the rest (`and` keeps the falsey value, `or` the
/// truthy one).
fn expand_and_or(
    eval: &mut Evaluator,
    args: &[Value],
    name: &str,
    empty: Value,
) -> Result<Value, NativeError> {
    let rest = user_args(&format!("clojure.core/{name}"), args, 0)?;
    match rest {
        [] => Ok(empty),
        [x] => Ok(x.clone()),
        [x, next @ ..] => {
            let g = Value::Symbol(gs(eval, name));
            let recur = qcall(name, next.to_vec());
            let branches = if name == "and" {
                vec![g.clone(), recur, g.clone()]
            } else {
                vec![g.clone(), g.clone(), recur]
            };
            Ok(ucall(
                "let*",
                vec![vecv(vec![g.clone(), x.clone()]), ucall("if", branches)],
            ))
        }
    }
}

fn expand_and(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    expand_and_or(eval, args, "and", Value::Bool(true))
}

fn expand_or(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    expand_and_or(eval, args, "or", Value::Nil)
}

/// `[& clauses]` ⇒ `(if t e (clojure.core/cond rest…))`, nil when
/// empty; a lone trailing test is the IAE.
fn expand_cond(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let clauses = user_args("clojure.core/cond", args, 0)?;
    match clauses {
        [] => Ok(Value::Nil),
        [_] => Err(iae("cond requires an even number of forms".to_owned())),
        [test, then, rest @ ..] => Ok(ucall(
            "if",
            vec![test.clone(), then.clone(), qcall("cond", rest.to_vec())],
        )),
    }
}

/// The items of a threaded form that is a seq (`seq?`), else `None`.
fn seq_form_items(eval: &mut Evaluator, form: &Value) -> Result<Option<Vec<Value>>, NativeError> {
    match form {
        Value::List(l) => Ok(Some(l.iter().cloned().collect())),
        v if is_seq_value(v) => collect_all(eval, v.clone())
            .map(Some)
            .map_err(signal_to_native),
        _ => Ok(None),
    }
}

/// `->`/`->>`: thread `x` into each form (seq forms keep their
/// metadata; anything else becomes `(form x)`).
fn thread(
    eval: &mut Evaluator,
    args: &[Value],
    display: &str,
    last: bool,
) -> Result<Value, NativeError> {
    let rest = user_args(display, args, 1)?;
    let mut x = rest[0].clone();
    for form in &rest[1..] {
        x = match seq_form_items(eval, form)? {
            Some(items) => {
                let mut iter = items.into_iter();
                let mut threaded = vec![iter.next().unwrap_or(Value::Nil)];
                if last {
                    threaded.extend(iter);
                    threaded.push(x);
                } else {
                    threaded.push(x);
                    threaded.extend(iter);
                }
                Value::List(List::from_vec(threaded)).with_meta(form.meta().cloned())
            }
            None => lst(vec![form.clone(), x]),
        };
    }
    Ok(x)
}

fn expand_thread_first(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    thread(eval, args, "clojure.core/->", false)
}

fn expand_thread_last(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    thread(eval, args, "clojure.core/->>", true)
}

/// `[& body]` ⇒ nil.
fn expand_comment(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    user_args("clojure.core/comment", args, 0)?;
    Ok(Value::Nil)
}

/// `[test & body]` ⇒ `(clojure.core/loop [] (clojure.core/when test
/// body… (recur)))`.
fn expand_while(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/while", args, 1)?;
    let mut when = vec![rest[0].clone()];
    when.extend(rest[1..].iter().cloned());
    when.push(ucall("recur", Vec::new()));
    Ok(qcall("loop", vec![vecv(Vec::new()), qcall("when", when)]))
}

/// `defn`: `(do (def name doc? (fn sigs…)) (alter-meta! (var name)
/// assoc :arglists '(…)) [(alter-meta! (var name) merge 'attr)]
/// (var name))` — doc, attr map, single-arity vector, and trailing
/// attr map handled like `core.clj`'s `defn`.
fn expand_defn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/defn", args, 1)?;
    defn_form(eval, &rest[0], &rest[1..])
}

fn defn_form(eval: &mut Evaluator, name: &Value, fdecl: &[Value]) -> Result<Value, NativeError> {
    if !matches!(name, Value::Symbol(_)) {
        return Err(iae("First argument to defn must be a symbol".to_owned()));
    }
    let (doc, attr, fdecl) = defn_parts(fdecl);
    let mut arglists = Vec::with_capacity(fdecl.len());
    for sig in &fdecl {
        arglists.push(sig_arglist(eval, sig)?);
    }
    let mut fnform = vec![sym("fn")];
    fnform.extend(fdecl);
    let mut defform = vec![sym("def"), name.clone()];
    defform.extend(doc);
    defform.push(lst(fnform));
    let var = ucall("var", vec![name.clone()]);
    let metaform = ucall(
        "alter-meta!",
        vec![
            var.clone(),
            sym("assoc"),
            kw("arglists"),
            ucall("quote", vec![lst(arglists)]),
        ],
    );
    let mut out = vec![sym("do"), lst(defform), metaform];
    if let Some(attr) = attr.filter(|m| !m.is_empty()) {
        out.push(ucall(
            "alter-meta!",
            vec![
                var.clone(),
                sym("merge"),
                ucall("quote", vec![Value::Map(attr)]),
            ],
        ));
    }
    out.push(var);
    Ok(lst(out))
}

/// `defn`'s declaration peeled like `core.clj`: the docstring, the
/// leading and trailing attr maps (merged), and the arity forms (a
/// single-arity body wrapped as one).
fn defn_parts(fdecl: &[Value]) -> (Option<Value>, Option<Map>, Vec<Value>) {
    let mut fdecl = fdecl.to_vec();
    let doc = match fdecl.first() {
        Some(d @ Value::Str(_)) => Some(d.clone()),
        _ => None,
    };
    if doc.is_some() {
        fdecl.remove(0);
    }
    let attr = match fdecl.first() {
        Some(Value::Map(m)) => Some(m.clone()),
        _ => None,
    };
    if attr.is_some() {
        fdecl.remove(0);
    }
    if fdecl.first().is_some_and(is_vector_val) {
        fdecl = vec![lst(fdecl)];
    }
    let attr2 = match fdecl.last() {
        Some(Value::Map(m)) => Some(m.clone()),
        _ => None,
    };
    if attr2.is_some() {
        fdecl.pop();
    }
    (doc, merge_attrs(attr, attr2), fdecl)
}

/// `(merge attr attr2)` over optional maps (nil when both are).
fn merge_attrs(attr: Option<Map>, attr2: Option<Map>) -> Option<Map> {
    match (attr, attr2) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => Some(b.iter().fold(a, |m, (k, v)| m.insert(k.clone(), v.clone()))),
    }
}

/// One signature's `:arglists` entry: its params (minus a leading
/// `&form &env` pair), carrying a prepost map's keys as metadata
/// when a body follows the map.
fn sig_arglist(eval: &mut Evaluator, sig: &Value) -> Result<Value, NativeError> {
    let items = form_items(eval, sig)?;
    let args = items.first().cloned().unwrap_or(Value::Nil);
    // `(first args)`, erroring on a non-seqable like `core.clj`'s.
    let params = match vector_items(&args) {
        Some(params) => params,
        None => form_items(eval, &args)?,
    };
    let args = if params.first().is_some_and(is_form_sym) {
        vecv(params.into_iter().skip(2).collect())
    } else {
        args
    };
    Ok(match items.get(1) {
        Some(Value::Map(pp)) if items.len() > 2 => {
            let merged = pp.iter().fold(
                args.meta().cloned().unwrap_or_else(clj_value::new_map),
                |m, (k, v)| m.insert(k.clone(), v.clone()),
            );
            args.with_meta(Some(merged))
        }
        _ => args,
    })
}

/// A form's items: a seq form's elements, else the data seq (the
/// ISeq error for a non-seqable).
fn form_items(eval: &mut Evaluator, v: &Value) -> Result<Vec<Value>, NativeError> {
    match seq_form_items(eval, v)? {
        Some(items) => Ok(items),
        None => crate::fx4_form::seq_data(v),
    }
}

fn is_form_sym(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.namespace().is_none() && s.name() == "&form")
}

/// `defn-`: `(defn name' decls…)` with `name'` marked `:private`.
fn expand_defn_private(_: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/defn-", args, 1)?;
    let Value::Symbol(name) = &rest[0] else {
        // `with-meta` on a non-symbol: the JVM cast, as `core.clj`
        // would have raised it.
        return Err(crate::core::cce_err(&rest[0], "clojure.lang.IObj"));
    };
    let meta = name
        .meta()
        .cloned()
        .unwrap_or_else(clj_value::new_map)
        .insert(kw("private"), Value::Bool(true));
    let mut out = vec![
        sym("defn"),
        Value::Symbol(Symbol::clone(name).with_meta(Some(meta))),
    ];
    out.extend(rest[1..].iter().cloned());
    Ok(lst(out))
}

/// `core.clj`'s private `assert-args`: the first failing check
/// throws `"<op> requires <what> in <ns>:<line>"` (the operator as
/// written; the line from `&form`, empty when absent).
fn assert_args(eval: &Evaluator, form: &Value, checks: &[(bool, &str)]) -> Result<(), NativeError> {
    let Some(&(_, what)) = checks.iter().find(|(ok, _)| !ok) else {
        return Ok(());
    };
    let line = form
        .meta()
        .and_then(|m| m.get(&kw("line")).cloned())
        .map_or_else(String::new, |l| str_of(&l));
    Err(iae(format!(
        "{} requires {what} in {}:{line}",
        str_of(&form_head(form)),
        eval.current_ns_name()
    )))
}

/// Which comprehension: `for` (lazy, `:let`/`:when`/`:while`) or
/// `doseq` (`dorun` over the same shape).
#[derive(Clone, Copy, PartialEq)]
enum Comp {
    For,
    Doseq,
}

/// A binding group: the binding pair plus its modifier pairs.
type Group = Vec<(Value, Value)>;

/// Split the binding vector into groups: each binding pair starts
/// a group, each keyword pair joins the current one (a leading
/// modifier pops the empty group vector, like `core.clj`).
fn binding_groups(pairs: &[Value]) -> Result<Vec<Group>, NativeError> {
    let mut groups: Vec<Group> = Vec::new();
    for pair in pairs.chunks(2) {
        let (k, v) = (pair[0].clone(), pair[1].clone());
        if matches!(k, Value::Keyword(_)) {
            let Some(last) = groups.last_mut() else {
                return Err(NativeError::new(
                    ThrowClass::IllegalStateException,
                    "Can't pop empty vector".to_owned(),
                ));
            };
            last.push((k, v));
        } else {
            groups.push(vec![(k, v)]);
        }
    }
    Ok(groups)
}

fn is_kw(v: &Value, name: &str) -> bool {
    matches!(v, Value::Keyword(k) if k.namespace().is_none() && k.name() == name)
}

/// An unknown modifier: `for` names it (IAE); `doseq` throws the
/// JVM's `steppair` NPE verbatim (it never validated modifiers).
fn bad_modifier(comp: Comp, k: &Value) -> NativeError {
    match comp {
        Comp::For => iae(format!("Invalid 'for' keyword {}", str_of(k))),
        Comp::Doseq => NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"steppair\" is null"
                .to_owned(),
        ),
    }
}

/// `(into ls v)` for a `:let` vector (any seqable; anything else is
/// the ISeq error).
fn into_lets(eval: &mut Evaluator, ls: &mut Vec<Value>, v: &Value) -> Result<(), NativeError> {
    let items = match v {
        Value::Nil => Vec::new(),
        _ => match vector_items(v) {
            Some(items) => items,
            None => form_items(eval, v)?,
        },
    };
    ls.extend(items);
    Ok(())
}

/// `(let [pat tmp] inner)`.
fn bind_tmp(pat: &Value, tmp: &Value, inner: Value) -> Value {
    ucall("let", vec![vecv(vec![pat.clone(), tmp.clone()]), inner])
}

/// `(fn [tmp] body)`.
fn tmp_fn(tmp: &Value, body: Value) -> Value {
    ucall("fn", vec![vecv(vec![tmp.clone()]), body])
}

/// `for`/`doseq` (`core.clj`'s shared expansion): the first group
/// maps (or mapcats) over its collection; the remaining groups nest
/// as an inner comprehension. A `:when` without `:while` fuses the
/// `:let`/`:when` chain into one mapcat stage; otherwise `:when`
/// filters and `:while` takes-while outside the map.
fn comprehension(
    eval: &mut Evaluator,
    form: &Value,
    seq_exprs: &Value,
    body: &[Value],
    comp: Comp,
) -> Result<Value, NativeError> {
    let items = vector_items(seq_exprs);
    assert_args(
        eval,
        form,
        &[
            (items.is_some(), "a vector for its binding"),
            (
                items.as_ref().is_none_or(|v| v.len() % 2 == 0),
                "an even number of forms in binding vector",
            ),
        ],
    )?;
    let groups = binding_groups(&items.unwrap_or_default())?;
    let Some((first, rest)) = groups.split_first() else {
        return Ok(match comp {
            Comp::For => ucall("list", vec![body[0].clone()]),
            Comp::Doseq => do_body(body),
        });
    };
    let stage = Stage {
        comp,
        pat: &first[0].0,
        xs: &first[0].1,
        mods: &first[1..],
        tmp: Value::Symbol(gs(eval, "G__")),
        nested: !rest.is_empty(),
    };
    let core = comprehension_core(comp, rest, body);
    let has_when = stage.mods.iter().any(|(k, _)| is_kw(k, "when"));
    let has_while = stage.mods.iter().any(|(k, _)| is_kw(k, "while"));
    let expansion = if has_when && !has_while {
        stage.fused(core)?
    } else {
        stage.chained(eval, core)?
    };
    Ok(match comp {
        Comp::For => expansion,
        Comp::Doseq => ucall("dorun", vec![expansion]),
    })
}

/// The innermost expression: the body, or the remaining groups as a
/// nested comprehension.
fn comprehension_core(comp: Comp, rest: &[Group], body: &[Value]) -> Value {
    if rest.is_empty() {
        return match comp {
            Comp::For => body[0].clone(),
            Comp::Doseq => do_body(body),
        };
    }
    let flat: Vec<Value> = rest
        .iter()
        .flat_map(|g| g.iter().flat_map(|(k, v)| [k.clone(), v.clone()]))
        .collect();
    let op = if comp == Comp::For { "for" } else { "doseq" };
    let mut nested = vec![sym(op), vecv(flat)];
    nested.extend(body.iter().cloned());
    lst(nested)
}

/// The first binding group of a comprehension: its pattern,
/// collection, and modifiers, over a fresh temp symbol.
struct Stage<'a> {
    comp: Comp,
    pat: &'a Value,
    xs: &'a Value,
    mods: &'a [(Value, Value)],
    tmp: Value,
    nested: bool,
}

impl Stage<'_> {
    /// `(fn [tmp] (let [pat tmp] inner))` — or `(fn [pat] inner)`
    /// when the pattern is a plain symbol (#226: no rebinding per
    /// item; the same arity for a `recur` in the body).
    fn mapper(&self, inner: Value) -> Value {
        if matches!(self.pat, Value::Symbol(s) if s.namespace().is_none() && s.name() != "&") {
            return tmp_fn(self.pat, inner);
        }
        tmp_fn(&self.tmp, bind_tmp(self.pat, &self.tmp, inner))
    }

    /// `:when` without `:while`: one mapcat stage over the
    /// `:let`/`:when` chain (a nested `for` core is already a seq;
    /// `doseq` always lists).
    ///
    /// An innermost `for` stage keeps instead (#226): the chain
    /// answers the item, or the `__for-keep*` native itself — a
    /// sentinel no item can be — when a `:when` rejects it, so no
    /// one-item list is built and spliced per item.
    fn fused(&self, core: Value) -> Result<Value, NativeError> {
        let keep = self.comp == Comp::For && !self.nested;
        let skip = if keep {
            Value::Symbol(Symbol::new(Some("clojure.core"), "__for-keep*"))
        } else {
            Value::Nil
        };
        let mut inner = if keep || self.nested && self.comp == Comp::For {
            core
        } else {
            ucall("list", vec![core])
        };
        for (k, v) in self.mods.iter().rev() {
            inner = if is_kw(k, "let") {
                ucall("let", vec![v.clone(), inner])
            } else if is_kw(k, "when") {
                ucall("if", vec![v.clone(), inner, skip.clone()])
            } else {
                return Err(bad_modifier(self.comp, k));
            };
        }
        if keep {
            return Ok(qcall(
                "__for-keep*",
                vec![self.mapper(inner), self.xs.clone(), skip],
            ));
        }
        Ok(ucall("mapcat", vec![self.mapper(inner), self.xs.clone()]))
    }

    /// Otherwise: `:when` filters and `:while` takes-while over the
    /// collection (each test seeing the `:let`s so far), then a map
    /// (mapcat for a nested `for`).
    fn chained(&self, eval: &mut Evaluator, core: Value) -> Result<Value, NativeError> {
        let mut se = self.xs.clone();
        let mut ls = Vec::new();
        let wrap = |ls: &[Value], inner: Value| {
            if ls.is_empty() {
                inner
            } else {
                ucall("let", vec![vecv(ls.to_vec()), inner])
            }
        };
        for (k, v) in self.mods {
            if is_kw(k, "let") {
                into_lets(eval, &mut ls, v)?;
                continue;
            }
            let op = if is_kw(k, "when") {
                "filter"
            } else if is_kw(k, "while") {
                "take-while"
            } else {
                return Err(bad_modifier(self.comp, k));
            };
            se = ucall(op, vec![self.mapper(wrap(&ls, v.clone())), se]);
        }
        let op = if self.comp == Comp::For && self.nested {
            "mapcat"
        } else {
            "map"
        };
        Ok(ucall(op, vec![self.mapper(wrap(&ls, core)), se]))
    }
}

/// `[seq-exprs body-expr]`.
fn expand_for(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/for", args, 2)?;
    if rest.len() > 2 {
        return Err(NativeError::arity("clojure.core/for", args.len()));
    }
    comprehension(eval, &args[0], &rest[0], &rest[1..], Comp::For)
}

/// `[seq-exprs & body]`.
fn expand_doseq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/doseq", args, 1)?;
    comprehension(eval, &args[0], &rest[0], &rest[1..], Comp::Doseq)
}

/// `[x form & more]`: each step is a `clojure.core/__meth*` call
/// (a seq form calls its head with its args; a symbol is a 0-arg
/// member), chaining through `clojure.core/..` while forms remain.
fn expand_dotdot(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let rest = user_args("clojure.core/..", args, 2)?;
    let (x, form, more) = (&rest[0], &rest[1], &rest[2..]);
    let one = match seq_form_items(eval, form)? {
        Some(items) => {
            let mut items = items.into_iter();
            let member = items.next().unwrap_or(Value::Nil);
            let mut call = vec![ucall("quote", vec![member]), x.clone()];
            call.extend(items);
            qcall("__meth*", call)
        }
        None => qcall(
            "__meth*",
            vec![ucall("quote", vec![form.clone()]), x.clone()],
        ),
    };
    if more.is_empty() {
        return Ok(one);
    }
    let mut chain = vec![one];
    chain.extend(more.iter().cloned());
    Ok(qcall("..", chain))
}
