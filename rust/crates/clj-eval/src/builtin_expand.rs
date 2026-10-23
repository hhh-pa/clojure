// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! FX4 expanders for the nine analyzer builtins (`let`, `loop`,
//! `fn`, `binding`, `letfn`, `with-redefs`, `with-bindings`,
//! `bound-fn`, `locking`). Each native is the macro fn: `&form`
//! and `&env` are the first two arguments. Shapes follow alpha9
//! `clojure.core` (one `macroexpand-1` step does not expand
//! nested `let`/`fn`).

use clj_value::Value;

use crate::destructure_expand::destructure;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::fx4_form::{
    form_head, gs, gs_auto, iae, is_symbol_val, is_vector_val, lst, qcall, seq_data, str_of, sym,
    sym_q, symv, ucall, vector_items, vecv, with_form_meta,
};
use crate::lazy::is_seq_value;

type Fx4Expand = fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError>;

/// The macro native for one of the nine heads, if `name` is one.
pub(crate) fn fx4_macro_fn(name: &str) -> Option<Fx4Expand> {
    Some(match name {
        "let" => expand_let,
        "loop" => expand_loop,
        "fn" => expand_fn,
        "binding" => expand_binding,
        "letfn" => expand_letfn,
        "with-redefs" => expand_with_redefs,
        "with-bindings" => expand_with_bindings,
        "bound-fn" => expand_bound_fn,
        "locking" => expand_locking,
        _ => return None,
    })
}

/// `&form`, then the user args. `min_user` counts args after the
/// two hidden macro params. A short call is the macro's own arity.
fn parts<'a>(
    display: &str,
    args: &'a [Value],
    min_user: usize,
) -> Result<(&'a Value, &'a [Value]), NativeError> {
    if args.len() < min_user + 2 {
        return Err(NativeError::arity(display, args.len()));
    }
    Ok((&args[0], &args[2..]))
}

fn assert_vec(form: &Value, bindings: &Value) -> Result<(), NativeError> {
    let op = str_of(&form_head(form));
    if !is_vector_val(bindings) {
        return Err(iae(format!("{op} requires a vector for its binding")));
    }
    let n = vector_items(bindings).map_or(0, |v| v.len());
    if n % 2 != 0 {
        return Err(iae(format!(
            "{op} requires an even number of forms in binding vector"
        )));
    }
    Ok(())
}

fn star_form(op: &str, bindings: &Value, body: &[Value]) -> Value {
    let mut items = vec![sym(op), bindings.clone()];
    items.extend(body.iter().cloned());
    lst(items)
}

fn expand_let(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (form, rest) = parts("clojure.core/let", args, 1)?;
    let bindings = &rest[0];
    assert_vec(form, bindings)?;
    let db = destructure(eval, bindings)?;
    Ok(star_form("let*", &db, &rest[1..]))
}

fn expand_loop(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (form, rest) = parts("clojure.core/loop", args, 1)?;
    let bindings = &rest[0];
    assert_vec(form, bindings)?;
    // Consume `destructure`'s gensyms even when the result is dropped.
    let db = destructure(eval, bindings)?;
    if &db == bindings {
        return Ok(star_form("loop*", bindings, &rest[1..]));
    }
    Ok(loop_dance(eval, bindings, &rest[1..]))
}

fn loop_dance(eval: &mut Evaluator, bindings: &Value, body: &[Value]) -> Value {
    let items = vector_items(bindings).unwrap_or_default();
    let (bfs, gs_vec, inner) = loop_pairs(eval, &items);
    let inner_let = qualified_let(&inner, body);
    let mut loop_items = vec![sym("loop*"), vecv(gs_vec)];
    loop_items.push(inner_let);
    qualified_let(&bfs, &[lst(loop_items)])
}

fn loop_pairs(eval: &mut Evaluator, items: &[Value]) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut bfs = Vec::new();
    let mut gs_vec = Vec::new();
    let mut inner = Vec::new();
    let mut i = 0;
    while i + 1 < items.len() {
        let b = &items[i];
        let v = &items[i + 1];
        let g = if is_symbol_val(b) {
            b.clone()
        } else {
            symv(&gs(eval, "G__"))
        };
        bfs.push(g.clone());
        bfs.push(v.clone());
        if !is_symbol_val(b) {
            bfs.push(b.clone());
            bfs.push(g.clone());
        }
        gs_vec.push(g.clone());
        gs_vec.push(g.clone());
        inner.push(b.clone());
        inner.push(g);
        i += 2;
    }
    (bfs, gs_vec, inner)
}

fn qualified_let(binds: &[Value], body: &[Value]) -> Value {
    let mut items = vec![sym_q("clojure.core", "let"), vecv(binds.to_vec())];
    items.extend(body.iter().cloned());
    lst(items)
}

fn expand_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (form, sigs) = parts("clojure.core/fn", args, 0)?;
    let expanded = assemble_fn(eval, sigs)?;
    Ok(with_form_meta(expanded, form))
}

fn assemble_fn(eval: &mut Evaluator, sigs: &[Value]) -> Result<Value, NativeError> {
    let (name, sigs) = split_fn_name(sigs);
    let sigs = normalize_sigs(sigs)?;
    let mut new_sigs = Vec::with_capacity(sigs.len());
    for sig in &sigs {
        new_sigs.push(one_sig(eval, sig)?);
    }
    Ok(fn_star_form(name.as_ref(), &new_sigs))
}

fn split_fn_name(sigs: &[Value]) -> (Option<Value>, &[Value]) {
    match sigs.first() {
        Some(Value::Symbol(s)) => (Some(Value::Symbol(s.clone())), &sigs[1..]),
        _ => (None, sigs),
    }
}

fn normalize_sigs(sigs: &[Value]) -> Result<Vec<Value>, NativeError> {
    match sigs.first() {
        Some(v) if is_vector_val(v) => Ok(vec![lst(sigs.to_vec())]),
        Some(v) if is_seq_value(v) => Ok(sigs.to_vec()),
        _ if sigs.is_empty() => Err(iae("Parameter declaration missing".to_owned())),
        _ => Err(iae(format!(
            "Parameter declaration {} should be a vector",
            str_of(&sigs[0])
        ))),
    }
}

fn fn_star_form(name: Option<&Value>, sigs: &[Value]) -> Value {
    let mut items = vec![sym("fn*")];
    if let Some(n) = name {
        items.push(n.clone());
    }
    items.extend(sigs.iter().cloned());
    lst(items)
}

fn one_sig(eval: &mut Evaluator, sig: &Value) -> Result<Value, NativeError> {
    if !is_seq_value(sig) {
        return Err(iae(format!(
            "Invalid signature {} should be a list",
            str_of(sig)
        )));
    }
    let items = list_or_empty(sig);
    let params = items.first().cloned().unwrap_or(Value::Nil);
    if !is_vector_val(&params) {
        return Err(bad_params(&params));
    }
    let (pre, post, body) = split_pre_post(&params, &items[1..]);
    let body = apply_post(post.as_ref(), &body)?;
    let body = apply_pre(pre.as_ref(), &body)?;
    maybe_destructured(eval, &params, &body)
}

fn list_or_empty(v: &Value) -> Vec<Value> {
    match v {
        Value::List(l) => l.iter().cloned().collect(),
        _ => Vec::new(),
    }
}

fn bad_params(params: &Value) -> NativeError {
    iae(format!(
        "Parameter declaration {} should be a vector",
        str_of(params)
    ))
}

fn split_pre_post(params: &Value, body: &[Value]) -> (Option<Value>, Option<Value>, Vec<Value>) {
    let (conds, rest) = body_conds(params, body);
    let pre = conds.as_ref().and_then(|c| map_key(c, "pre"));
    let post = conds.as_ref().and_then(|c| map_key(c, "post"));
    (pre, post, rest)
}

fn body_conds(params: &Value, body: &[Value]) -> (Option<Value>, Vec<Value>) {
    if body.len() > 1 {
        if let Value::Map(_) = &body[0] {
            return (Some(body[0].clone()), body[1..].to_vec());
        }
    }
    let meta = params.meta().cloned().map(Value::Map);
    (meta, body.to_vec())
}

fn map_key(v: &Value, name: &str) -> Option<Value> {
    match v {
        Value::Map(m) => m.get(&crate::fx4_form::kw(name)).cloned(),
        _ => None,
    }
}

fn apply_post(post: Option<&Value>, body: &[Value]) -> Result<Vec<Value>, NativeError> {
    let Some(post) = post.filter(|v| v.is_truthy()) else {
        return Ok(body.to_vec());
    };
    let conds = seq_data(post)?;
    let init = post_init(body);
    let mut items = vec![sym_q("clojure.core", "let"), vecv(vec![sym("%"), init])];
    for c in conds {
        items.push(qcall("assert", vec![c]));
    }
    items.push(sym("%"));
    Ok(vec![lst(items)])
}

fn post_init(body: &[Value]) -> Value {
    if body.len() > 1 {
        ucall("do", body.to_vec())
    } else {
        body.first().cloned().unwrap_or(Value::Nil)
    }
}

fn apply_pre(pre: Option<&Value>, body: &[Value]) -> Result<Vec<Value>, NativeError> {
    let Some(pre) = pre.filter(|v| v.is_truthy()) else {
        return Ok(body.to_vec());
    };
    let mut out = Vec::new();
    for c in seq_data(pre)? {
        out.push(qcall("assert", vec![c]));
    }
    out.extend(body.iter().cloned());
    Ok(out)
}

fn maybe_destructured(
    eval: &mut Evaluator,
    params: &Value,
    body: &[Value],
) -> Result<Value, NativeError> {
    let items = vector_items(params).unwrap_or_default();
    if items.iter().all(is_symbol_val) {
        let mut out = vec![params.clone()];
        out.extend(body.iter().cloned());
        return Ok(lst(out));
    }
    let (new_params, lets) = destructure_params(eval, &items);
    let params_val = vecv(new_params).with_meta(params.meta().cloned());
    let mut inner = vec![sym_q("clojure.core", "let"), vecv(lets)];
    inner.extend(body.iter().cloned());
    Ok(lst(vec![params_val, lst(inner)]))
}

fn destructure_params(eval: &mut Evaluator, items: &[Value]) -> (Vec<Value>, Vec<Value>) {
    let mut new_params = Vec::new();
    let mut lets = Vec::new();
    for p in items {
        if is_symbol_val(p) {
            new_params.push(p.clone());
        } else {
            let g = symv(&gs(eval, "p__"));
            new_params.push(g.clone());
            lets.push(p.clone());
            lets.push(g);
        }
    }
    (new_params, lets)
}

fn expand_binding(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (form, rest) = parts("clojure.core/binding", args, 1)?;
    assert_vec(form, &rest[0])?;
    let hash_args = var_ize(&rest[0]);
    let push = qcall("push-thread-bindings", vec![qcall("hash-map", hash_args)]);
    let body_try = binding_try(&rest[1..]);
    Ok(qualified_let(&[], &[push, body_try]))
}

fn var_ize(bindings: &Value) -> Vec<Value> {
    let items = vector_items(bindings).unwrap_or_default();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < items.len() {
        out.push(ucall("var", vec![items[i].clone()]));
        out.push(items[i + 1].clone());
        i += 2;
    }
    out
}

fn binding_try(body: &[Value]) -> Value {
    let mut items = vec![sym("try")];
    items.extend(body.iter().cloned());
    items.push(lst(vec![
        sym("finally"),
        qcall("pop-thread-bindings", vec![]),
    ]));
    lst(items)
}

fn expand_letfn(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (_form, rest) = parts("clojure.core/letfn", args, 1)?;
    let specs = seq_data(&rest[0])?;
    let mut binds = Vec::new();
    for spec in &specs {
        let parts = seq_data(spec)?;
        let name = parts.first().cloned().unwrap_or(Value::Nil);
        binds.push(name);
        binds.push(fn_over_spec(&parts));
    }
    let mut items = vec![sym("letfn*"), vecv(binds)];
    items.extend(rest[1..].iter().cloned());
    Ok(lst(items))
}

fn fn_over_spec(parts: &[Value]) -> Value {
    let mut items = vec![sym_q("clojure.core", "fn")];
    items.extend(parts.iter().cloned());
    lst(items)
}

fn expand_with_redefs(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (_form, rest) = parts("clojure.core/with-redefs", args, 1)?;
    let items = seq_data(&rest[0])?;
    let map = zip_vars(&items);
    Ok(qcall("with-redefs-fn", vec![map, fn_body(&rest[1..])]))
}

fn zip_vars(items: &[Value]) -> Value {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < items.len() {
        pairs.push((ucall("var", vec![items[i].clone()]), items[i + 1].clone()));
        i += 2;
    }
    crate::fx4_form::map_of(pairs)
}

fn fn_body(body: &[Value]) -> Value {
    let mut items = vec![sym_q("clojure.core", "fn"), vecv(vec![])];
    items.extend(body.iter().cloned());
    lst(items)
}

fn expand_with_bindings(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (_form, rest) = parts("clojure.core/with-bindings", args, 1)?;
    Ok(qcall(
        "with-bindings*",
        vec![rest[0].clone(), fn_body(&rest[1..])],
    ))
}

fn expand_bound_fn(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (_form, sigs) = parts("clojure.core/bound-fn", args, 0)?;
    let mut items = vec![sym_q("clojure.core", "fn")];
    items.extend(sigs.iter().cloned());
    Ok(qcall("bound-fn*", vec![lst(items)]))
}

fn expand_locking(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (_form, rest) = parts("clojure.core/locking", args, 1)?;
    let lockee = symv(&gs_auto(eval, "lockee"));
    let locklocal = symv(&gs_auto(eval, "locklocal"));
    let inner = locking_inner(&lockee, &locklocal, &rest[1..]);
    let outer_try = lst(vec![sym("try"), inner]);
    Ok(qualified_let(&[lockee, rest[0].clone()], &[outer_try]))
}

fn locking_inner(lockee: &Value, locklocal: &Value, body: &[Value]) -> Value {
    let mut tried = vec![sym("try")];
    tried.extend(body.iter().cloned());
    tried.push(lst(vec![
        sym("finally"),
        ucall("monitor-exit", vec![locklocal.clone()]),
    ]));
    qualified_let(
        &[locklocal.clone(), lockee.clone()],
        &[ucall("monitor-enter", vec![locklocal.clone()]), lst(tried)],
    )
}
