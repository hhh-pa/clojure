// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `destructure` / `destvec*` / `destmap*` as value builders
//! (alpha9 `clojure.core`). Star specials only accept symbols, so
//! a destructuring `let` must emit symbol bindings before analyze.

use clj_value::{Map, Symbol, Value, new_set};

use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::fx4_form::{
    ex, gs, gs_auto, iae, is_amp, is_as, is_ident, is_seqable_data, is_symbol_val, iseq_err, kw,
    kw_get, lst, map_of, npe, pr_of, qcall, seq_data, seq_items, str_of, sym, sym_q, symv,
    tag_iseq, ucall, vector_items, vecv,
};

/// Rewrite a binding vector. All-symbol vectors pass through (no gensyms).
pub(crate) fn destructure(eval: &mut Evaluator, bindings: &Value) -> Result<Value, NativeError> {
    let items = vector_items(bindings).unwrap_or_default();
    if items.iter().step_by(2).all(is_symbol_val) && items.len() % 2 == 0 {
        return Ok(bindings.clone());
    }
    let mut ret = Vec::new();
    let mut i = 0;
    while i + 1 < items.len() {
        bind_one(eval, &mut ret, &items[i], &items[i + 1])?;
        i += 2;
    }
    Ok(vecv(ret))
}

fn bind_one(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    pat: &Value,
    val: &Value,
) -> Result<(), NativeError> {
    if is_symbol_val(pat) {
        ret.push(pat.clone());
        ret.push(val.clone());
        return Ok(());
    }
    if let Some(items) = vector_items(pat) {
        return dest_vec(eval, ret, &items, val);
    }
    if let Value::Map(map) = pat {
        return dest_map(eval, ret, map, val);
    }
    Err(ex(format!("Unsupported binding form: {}", str_of(pat))))
}

fn dest_vec(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    items: &[Value],
    val: &Value,
) -> Result<(), NativeError> {
    let gvec = gs(eval, "vec__");
    let gseq = gs(eval, "seq__");
    let gfirst = gs(eval, "first__");
    let has_rest = items.iter().any(is_amp);
    ret.push(symv(&gvec));
    ret.push(val.clone());
    if has_rest {
        ret.push(symv(&gseq));
        ret.push(qcall("seq", vec![symv(&gvec)]));
    }
    dest_vec_walk(eval, ret, items, &gvec, &gseq, &gfirst, has_rest)
}

fn dest_vec_walk(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    items: &[Value],
    gvec: &Symbol,
    gseq: &Symbol,
    gfirst: &Symbol,
    has_rest: bool,
) -> Result<(), NativeError> {
    let mut i = 0;
    let mut n = 0i64;
    let mut seen = false;
    while i < items.len() {
        let item = &items[i];
        if is_amp(item) {
            let next = items.get(i + 1).cloned().unwrap_or(Value::Nil);
            bind_one(eval, ret, &next, &symv(gseq))?;
            i += 2;
            seen = true;
            continue;
        }
        if is_as(item) {
            let next = items.get(i + 1).cloned().unwrap_or(Value::Nil);
            return bind_one(eval, ret, &next, &symv(gvec));
        }
        if seen {
            return Err(ex(
                "Unsupported binding form, only :as can follow & parameter".to_owned(),
            ));
        }
        dest_vec_elem(
            eval,
            ret,
            item,
            &VecNames { gvec, gseq, gfirst },
            has_rest,
            n,
        )?;
        n += 1;
        i += 1;
    }
    Ok(())
}

struct VecNames<'a> {
    gvec: &'a Symbol,
    gseq: &'a Symbol,
    gfirst: &'a Symbol,
}

fn dest_vec_elem(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    pat: &Value,
    names: &VecNames<'_>,
    has_rest: bool,
    n: i64,
) -> Result<(), NativeError> {
    if has_rest {
        ret.push(symv(names.gfirst));
        ret.push(qcall("first", vec![symv(names.gseq)]));
        ret.push(symv(names.gseq));
        ret.push(qcall("next", vec![symv(names.gseq)]));
        return bind_one(eval, ret, pat, &symv(names.gfirst));
    }
    let nth = qcall("nth", vec![symv(names.gvec), Value::Int(n), Value::Nil]);
    bind_one(eval, ret, pat, &nth)
}

struct MapCx {
    gmap: Symbol,
    gseq: Symbol,
    gtemp: Symbol,
    defaults: Option<Map>,
    gdefaults: Vec<(Value, Symbol)>,
    select: Option<Value>,
    all: Option<Value>,
    excess: Option<Value>,
    missing: Option<Value>,
    gnotfound: Option<Symbol>,
    gnotfoundq: Option<Symbol>,
}

struct Walk {
    sel: Vec<Value>,
    btok: Vec<(Value, Value)>,
    subs: Vec<(Value, Value)>,
    suba: Vec<(Value, Value)>,
    subx: Vec<(Value, Value)>,
    subm: Vec<(Value, Value)>,
}

impl Walk {
    fn new() -> Self {
        Self {
            sel: Vec::new(),
            btok: Vec::new(),
            subs: Vec::new(),
            suba: Vec::new(),
            subx: Vec::new(),
            subm: Vec::new(),
        }
    }
}

fn dest_map(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    map: &Map,
    val: &Value,
) -> Result<(), NativeError> {
    let cx = map_cx(eval, map)?;
    emit_defaults(ret, &cx);
    emit_gmap(ret, &cx, val);
    emit_as(ret, map, &cx);
    emit_missing_slot(ret, &cx);
    let bes = bes_map(map);
    let walk = walk_bes(eval, ret, &cx, &bes)?;
    emit_outlets(eval, ret, &cx, &walk)
}

fn map_cx(eval: &mut Evaluator, map: &Map) -> Result<MapCx, NativeError> {
    let gmap = gs(eval, "map__");
    let gseq = tag_iseq(gmap.clone());
    let gtemp = gs(eval, "temp__");
    let defaults = or_defaults(map)?;
    let gdefaults = alloc_defaults(eval, defaults.as_ref());
    let missing = opt_copy(map, "missing");
    let notfound = alloc_notfound(eval, truthy(missing.as_ref()));
    Ok(MapCx {
        gmap,
        gseq,
        gtemp,
        defaults,
        gdefaults,
        select: opt_copy(map, "select"),
        all: opt_copy(map, "all"),
        excess: opt_copy(map, "excess"),
        missing,
        gnotfound: notfound.0,
        gnotfoundq: notfound.1,
    })
}

fn or_defaults(map: &Map) -> Result<Option<Map>, NativeError> {
    match kw_get(map, "or") {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Map(m)) => Ok(Some(m.clone())),
        Some(other) => non_map_or(other),
    }
}

fn non_map_or(v: &Value) -> Result<Option<Map>, NativeError> {
    if !is_seqable_data(v) {
        return Err(iseq_err(v));
    }
    match seq_items(v).first() {
        None => Ok(None),
        Some(first) => Err(NativeError::cast(
            &jvm_class_of(first),
            "java.util.Map$Entry",
        )),
    }
}

fn alloc_defaults(eval: &mut Evaluator, defaults: Option<&Map>) -> Vec<(Value, Symbol)> {
    let Some(map) = defaults else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, _) in map.iter() {
        out.push((k.clone(), gs(eval, "default__")));
    }
    out
}

fn alloc_notfound(eval: &mut Evaluator, missing: bool) -> (Option<Symbol>, Option<Symbol>) {
    if !missing {
        return (None, None);
    }
    (Some(gs(eval, "notfound__")), Some(gs(eval, "notfound?__")))
}

fn opt_copy(map: &Map, name: &str) -> Option<Value> {
    kw_get(map, name).cloned()
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(Value::is_truthy)
}

fn emit_defaults(ret: &mut Vec<Value>, cx: &MapCx) {
    let Some(defaults) = &cx.defaults else {
        return;
    };
    for (k, g) in &cx.gdefaults {
        ret.push(symv(g));
        ret.push(defaults.get(k).cloned().unwrap_or(Value::Nil));
    }
}

fn emit_gmap(ret: &mut Vec<Value>, cx: &MapCx, val: &Value) {
    ret.push(symv(&cx.gmap));
    ret.push(val.clone());
    ret.push(symv(&cx.gmap));
    ret.push(convert_form(cx));
}

fn emit_as(ret: &mut Vec<Value>, map: &Map, cx: &MapCx) {
    if let Some(as_sym) = kw_get(map, "as") {
        ret.push(as_sym.clone());
        ret.push(symv(&cx.gmap));
    }
}

fn emit_missing_slot(ret: &mut Vec<Value>, cx: &MapCx) {
    let Some(missing) = cx.missing.as_ref().filter(|v| v.is_truthy()) else {
        return;
    };
    let gnot = cx.gnotfound.as_ref().expect("missing allocates notfound");
    ret.push(missing.clone());
    ret.push(Value::Nil);
    ret.push(symv(gnot));
    ret.push(ucall("new", vec![sym("Object")]));
}

fn convert_form(cx: &MapCx) -> Value {
    let g = symv(&cx.gmap);
    let gs = symv(&cx.gseq);
    let class_sym = sym("clojure.lang.ISeq");
    ucall(
        "if",
        vec![
            qcall("instance?", vec![class_sym, g.clone()]),
            ucall(
                "if",
                vec![
                    qcall("next", vec![gs.clone()]),
                    pam_call(gs.clone()),
                    ucall(
                        "if",
                        vec![
                            qcall("seq", vec![gs.clone()]),
                            qcall("first", vec![gs]),
                            sym_q("clojure.lang.PersistentArrayMap", "EMPTY"),
                        ],
                    ),
                ],
            ),
            g,
        ],
    )
}

fn pam_call(gseq: Value) -> Value {
    lst(vec![
        sym_q("clojure.lang.PersistentArrayMap", "createAsIfByAssoc"),
        qcall("to-array", vec![gseq]),
    ])
}

fn bes_map(map: &Map) -> Map {
    let drop = ["defaults", "as", "or", "select", "all", "excess", "missing"];
    let mut out = map.clone();
    for name in drop {
        out = out.without(&kw(name));
    }
    out
}

fn walk_bes(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    bes: &Map,
) -> Result<Walk, NativeError> {
    let mut walk = Walk::new();
    for (bb, bk) in bes.iter() {
        walk_entry(eval, ret, cx, &mut walk, bb, bk)?;
    }
    Ok(walk)
}

fn walk_entry(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    bb: &Value,
    bk: &Value,
) -> Result<(), NativeError> {
    if let Value::Keyword(kw) = bb {
        walk_directive(eval, ret, cx, walk, kw, bk)
    } else {
        walk_plain(eval, ret, cx, walk, bb, bk)
    }
}

enum DirKind {
    Keys,
    Syms,
    Strs,
}

struct Dir {
    kind: DirKind,
    ns: Option<String>,
    required: bool,
}

fn walk_directive(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    dir: &clj_value::Keyword,
    bk: &Value,
) -> Result<(), NativeError> {
    let kind = directive_kind(dir)?;
    let items = seq_data(bk)?;
    let mut preamp = true;
    for item in &items {
        let dir_val = Value::Keyword(dir.clone());
        preamp = directive_item(
            eval,
            ret,
            cx,
            walk,
            &DirStep {
                dir: &dir_val,
                kind: &kind,
                item,
                preamp,
            },
        )?;
    }
    Ok(())
}

fn directive_kind(dir: &clj_value::Keyword) -> Result<Dir, NativeError> {
    let name = dir.name();
    let kind = if name.starts_with("keys") {
        DirKind::Keys
    } else if name.starts_with("syms") {
        DirKind::Syms
    } else if name.starts_with("strs") {
        DirKind::Strs
    } else {
        return Err(ex(format!("Unsupported map directive: {dir}")));
    };
    Ok(Dir {
        kind,
        ns: dir.namespace().map(str::to_owned),
        required: name.ends_with('!'),
    })
}

struct DirStep<'a> {
    dir: &'a Value,
    kind: &'a Dir,
    item: &'a Value,
    preamp: bool,
}

fn directive_item(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    step: &DirStep<'_>,
) -> Result<bool, NativeError> {
    if is_amp(step.item) {
        return amp_step(step.dir, step.preamp);
    }
    if !step.preamp {
        post_amp(eval, ret, cx, walk, step.kind, step.item)?;
        return Ok(false);
    }
    let key = translate(step.kind, step.item)?;
    push1(eval, ret, cx, walk, step.item, &key, step.kind.required)?;
    sel_add(&mut walk.sel, key.clone());
    btok_put(&mut walk.btok, localize(step.item), key);
    Ok(true)
}

fn amp_step(dir: &Value, preamp: bool) -> Result<bool, NativeError> {
    if preamp {
        return Ok(false);
    }
    Err(iae(format!("& can only appear once in {}", str_of(dir))))
}

fn post_amp(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    kind: &Dir,
    item: &Value,
) -> Result<(), NativeError> {
    if is_symbol_val(item) {
        return Err(iae(format!(
            "'{}' - binding symbols can only appear before '&', use keys after",
            str_of(item)
        )));
    }
    sel_add(&mut walk.sel, item.clone());
    if kind.required {
        push1(eval, ret, cx, walk, &symv(&cx.gtemp), item, true)?;
    }
    Ok(())
}

fn translate(kind: &Dir, item: &Value) -> Result<Value, NativeError> {
    if matches!(kind.kind, DirKind::Strs) {
        return Ok(Value::Str(str_of(item).into()));
    }
    let (elem_ns, name) = named_parts(item, kind.ns.is_some())?;
    let ns = kind.ns.clone().or(elem_ns);
    Ok(match kind.kind {
        DirKind::Keys => Value::Keyword(clj_value::Keyword::new(ns.as_deref(), &name)),
        DirKind::Syms => lst(vec![
            sym("quote"),
            Value::Symbol(clj_value::Symbol::new(ns.as_deref(), &name)),
        ]),
        DirKind::Strs => Value::Str(name.into()),
    })
}

fn named_parts(v: &Value, have_ns: bool) -> Result<(Option<String>, String), NativeError> {
    if matches!(v, Value::Nil) {
        let method = if have_ns { "getName" } else { "getNamespace" };
        return Err(npe(format!(
            "Cannot invoke \"clojure.lang.Named.{method}()\" because \"x\" is null"
        )));
    }
    match v {
        Value::Symbol(s) => Ok((s.namespace().map(str::to_owned), s.name().to_owned())),
        Value::Keyword(k) => Ok((k.namespace().map(str::to_owned), k.name().to_owned())),
        other => Err(NativeError::cast(
            &jvm_class_of(other),
            "clojure.lang.Named",
        )),
    }
}

fn walk_plain(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    bb: &Value,
    bk: &Value,
) -> Result<(), NativeError> {
    let bb = inject_all(eval, cx, walk, bb, bk)?;
    if is_symbol_val(&bb) {
        btok_put(&mut walk.btok, bb.clone(), bk.clone());
    }
    sel_add(&mut walk.sel, bk.clone());
    push1(eval, ret, cx, walk, &bb, bk, false)
}

fn inject_all(
    eval: &mut Evaluator,
    cx: &MapCx,
    walk: &mut Walk,
    bb: &Value,
    bk: &Value,
) -> Result<Value, NativeError> {
    let bb = inject_one(eval, bb, cx.select.as_ref(), "select", &mut walk.subs, bk)?;
    let bb = inject_one(eval, &bb, cx.all.as_ref(), "all", &mut walk.suba, bk)?;
    let bb = inject_one(eval, &bb, cx.excess.as_ref(), "excess", &mut walk.subx, bk)?;
    inject_one(
        eval,
        &bb,
        cx.missing.as_ref(),
        "missing",
        &mut walk.subm,
        bk,
    )
}

fn inject_one(
    eval: &mut Evaluator,
    bb: &Value,
    parent: Option<&Value>,
    which: &str,
    slot: &mut Vec<(Value, Value)>,
    bk: &Value,
) -> Result<Value, NativeError> {
    if !truthy(parent) {
        return Ok(bb.clone());
    }
    let Value::Map(map) = bb else {
        return Ok(bb.clone());
    };
    let key = kw(which);
    let (value, fresh) = outlet_sym(eval, map, &key, which)?;
    btok_put(slot, bk.clone(), value.clone());
    if fresh {
        return Ok(Value::Map(map.clone().insert(key, value)));
    }
    Ok(bb.clone())
}

fn outlet_sym(
    eval: &mut Evaluator,
    map: &Map,
    key: &Value,
    which: &str,
) -> Result<(Value, bool), NativeError> {
    if let Some(existing) = map.get(key) {
        return Ok((existing.clone(), false));
    }
    let generated = gs(eval, &format!("{which}__"));
    Ok((symv(&generated), true))
}

fn push1(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &mut Walk,
    bb: &Value,
    bk: &Value,
    req: bool,
) -> Result<(), NativeError> {
    let local = localize(bb);
    let bv = getter(cx, &local, bk, req)?;
    if is_ident(bb) {
        push_ident(ret, cx, &local, &bv, bk, req);
        Ok(())
    } else {
        let _ = walk;
        bind_one(eval, ret, bb, &bv)
    }
}

fn push_ident(ret: &mut Vec<Value>, cx: &MapCx, local: &Value, bv: &Value, bk: &Value, req: bool) {
    if req && truthy(cx.missing.as_ref()) {
        push_required_missing(ret, cx, local, bv, bk);
        return;
    }
    ret.push(local.clone());
    ret.push(bv.clone());
}

fn push_required_missing(ret: &mut Vec<Value>, cx: &MapCx, local: &Value, bv: &Value, bk: &Value) {
    let gnot = cx.gnotfound.as_ref().expect("missing allocates notfound");
    let gnotq = cx.gnotfoundq.as_ref().expect("missing allocates notfound?");
    let missing = cx.missing.as_ref().expect("missing");
    ret.push(symv(&cx.gtemp));
    ret.push(bv.clone());
    ret.push(symv(gnotq));
    ret.push(qcall("identical?", vec![symv(&cx.gtemp), symv(gnot)]));
    ret.push(missing.clone());
    ret.push(ucall(
        "if",
        vec![
            symv(gnotq),
            qcall("assoc", vec![missing.clone(), bk.clone(), Value::Nil]),
            missing.clone(),
        ],
    ));
    ret.push(local.clone());
    ret.push(qcall("when-not", vec![symv(gnotq), symv(&cx.gtemp)]));
}

fn getter(cx: &MapCx, local: &Value, bk: &Value, req: bool) -> Result<Value, NativeError> {
    let local_def = has_default(cx, local);
    let key_def = has_default(cx, bk);
    if local_def || key_def {
        return defaulted_get(cx, local, bk, req, local_def, key_def);
    }
    Ok(plain_get(cx, bk, req))
}

fn has_default(cx: &MapCx, key: &Value) -> bool {
    cx.defaults.as_ref().is_some_and(|m| m.contains_key(key))
}

fn defaulted_get(
    cx: &MapCx,
    local: &Value,
    bk: &Value,
    req: bool,
    local_def: bool,
    key_def: bool,
) -> Result<Value, NativeError> {
    if local_def && key_def {
        return Err(ex(format!(
            "Multiple :or defaults for same key: {} '{}'",
            str_of(bk),
            str_of(local)
        )));
    }
    if req {
        return Err(ex(format!(
            "Can't supply default value for required key: {}",
            str_of(bk)
        )));
    }
    let key = if local_def { local } else { bk };
    let g = cx
        .gdefaults
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, g)| symv(g))
        .unwrap_or(Value::Nil);
    Ok(qcall("get", vec![symv(&cx.gmap), bk.clone(), g]))
}

fn plain_get(cx: &MapCx, bk: &Value, req: bool) -> Value {
    let g = symv(&cx.gmap);
    if req && truthy(cx.missing.as_ref()) {
        let notfound = cx.gnotfound.as_ref().map(symv).unwrap_or(Value::Nil);
        return qcall("get", vec![g, bk.clone(), notfound]);
    }
    if req {
        return qcall("req!", vec![g, bk.clone()]);
    }
    qcall("get", vec![g, bk.clone()])
}

fn localize(bb: &Value) -> Value {
    match bb {
        Value::Symbol(s) => {
            Value::Symbol(clj_value::Symbol::new(None, s.name()).with_meta(s.meta().cloned()))
        }
        Value::Keyword(k) => Value::Symbol(clj_value::Symbol::new(None, k.name())),
        other => other.clone(),
    }
}

fn sel_add(sel: &mut Vec<Value>, key: Value) {
    if !sel.iter().any(|e| e == &key) {
        sel.push(key);
    }
}

fn btok_put(slot: &mut Vec<(Value, Value)>, key: Value, val: Value) {
    if let Some(found) = slot.iter_mut().find(|(k, _)| k == &key) {
        found.1 = val;
    } else {
        slot.push((key, val));
    }
}

fn btok_get(slot: &[(Value, Value)], key: &Value) -> Option<Value> {
    slot.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn emit_outlets(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &Walk,
) -> Result<(), NativeError> {
    let dm = or_dm(cx, walk)?;
    emit_select(eval, ret, cx, walk, dm.as_ref());
    emit_all(ret, cx, walk, dm.as_ref());
    emit_excess(ret, cx, walk);
    emit_missing_final(ret, cx, walk);
    Ok(())
}

fn or_dm(cx: &MapCx, walk: &Walk) -> Result<Option<Value>, NativeError> {
    if cx.gdefaults.is_empty() {
        return Ok(None);
    }
    let strict = truthy(cx.select.as_ref()) || truthy(cx.all.as_ref());
    let mut pairs = Vec::new();
    for (k, g) in &cx.gdefaults {
        pairs.push((map_or_key(k, &walk.btok, strict)?, symv(g)));
    }
    let built = map_of(pairs);
    if strict {
        if let Value::Map(map) = &built {
            keys_only_in_or(map, &walk.sel, cx.gdefaults.len())?;
        }
    }
    Ok(Some(built))
}

fn map_or_key(key: &Value, btok: &[(Value, Value)], strict: bool) -> Result<Value, NativeError> {
    let Value::Symbol(sym) = key else {
        return Ok(key.clone());
    };
    match btok_get(btok, key) {
        Some(bk) => Ok(bk),
        None if strict => Err(iae(format!(
            "symbol {sym} in :or does not refer to a binding"
        ))),
        None => Ok(Value::Nil),
    }
}

fn keys_only_in_or(map: &Map, sel: &[Value], n_defaults: usize) -> Result<(), NativeError> {
    let hit = sel.iter().filter(|s| map.contains_key(s)).count();
    if hit == n_defaults {
        return Ok(());
    }
    let mut set = new_set();
    for (k, _) in map.iter() {
        if !sel.iter().any(|s| s == k) {
            set = set.insert(k.clone());
        }
    }
    Err(iae(format!(
        "keys {} appear only in :or",
        pr_of(&Value::Set(set))
    )))
}

fn emit_select(
    eval: &mut Evaluator,
    ret: &mut Vec<Value>,
    cx: &MapCx,
    walk: &Walk,
    dm: Option<&Value>,
) {
    let Some(name) = cx.select.as_ref().filter(|v| v.is_truthy()) else {
        return;
    };
    let mm = gs_auto(eval, "mm");
    let init = qcall(
        "merge",
        vec![
            dm.cloned().unwrap_or(Value::Nil),
            symv(&cx.gmap),
            qcall("some-vals", vec![pair_map(&walk.subs)]),
        ],
    );
    let body = qcall("select-keys", vec![symv(&mm), vecv(walk.sel.clone())]);
    let form = lst(vec![
        sym_q("clojure.core", "when-let"),
        vecv(vec![symv(&mm), init]),
        body,
    ]);
    ret.push(name.clone());
    ret.push(form);
}

fn emit_all(ret: &mut Vec<Value>, cx: &MapCx, walk: &Walk, dm: Option<&Value>) {
    let Some(name) = cx.all.as_ref().filter(|v| v.is_truthy()) else {
        return;
    };
    ret.push(name.clone());
    ret.push(qcall(
        "merge",
        vec![
            dm.cloned().unwrap_or(Value::Nil),
            symv(&cx.gmap),
            qcall("some-vals", vec![pair_map(&walk.suba)]),
        ],
    ));
}

fn emit_excess(ret: &mut Vec<Value>, cx: &MapCx, walk: &Walk) {
    let Some(name) = cx.excess.as_ref().filter(|v| v.is_truthy()) else {
        return;
    };
    let dissoc = qcall(
        "apply",
        vec![
            sym_q("clojure.core", "dissoc"),
            symv(&cx.gmap),
            vecv(walk.sel.clone()),
        ],
    );
    ret.push(name.clone());
    ret.push(qcall(
        "merge",
        vec![
            qcall("not-empty", vec![dissoc]),
            qcall("some-vals", vec![pair_map(&walk.subx)]),
        ],
    ));
}

fn emit_missing_final(ret: &mut Vec<Value>, cx: &MapCx, walk: &Walk) {
    let Some(name) = cx.missing.as_ref().filter(|v| v.is_truthy()) else {
        return;
    };
    ret.push(name.clone());
    ret.push(qcall(
        "merge",
        vec![name.clone(), qcall("some-vals", vec![pair_map(&walk.subm)])],
    ));
}

fn pair_map(pairs: &[(Value, Value)]) -> Value {
    if pairs.is_empty() {
        Value::Nil
    } else {
        map_of(pairs.to_vec())
    }
}
