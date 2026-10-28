// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S8 (issue #51): the print system — the evaluator-level
//! `pr-on` driver, the booted `print-method`/`print-dup`
//! multimethods, `print-simple`/`print-ctor`, and the char-map vars.
//!
//! Every pr-family call (`pr`/`prn`/`print`/`println`/`pr-str`/
//! `print-str`) renders each arg through [`pr_on`], which invokes
//! the live multis (user `defmethod`s fire, recursively).
//! Collection shapes recurse per child through [`pr_on`];
//! scalar shapes delegate to the [`pr_to_string`](clj_value::pr_to_string)
//! engine. Native `str`, `format`, and error interpolation never
//! touch the multis.

use std::sync::Arc;

use clj_value::{
    CHAR_ESCAPES, CHAR_NAMES, ConsValue, CycleValue, ExtensionValue, HostKind, HostVal,
    IterateValue, KeySeqValue, Keyword, Map, MultiFn, PrintOpts, RangeValue, RealizedSeq,
    RepeatValue, SubVectorValue, Symbol, ThrowClass, ValSeqValue, Value, Var, lift_ns, new_map,
    pr_to_string,
};

use crate::core::{Reduced, cce_err, is_lazy_seq_str, print_opts, str_of_one};
use crate::error::{NativeError, Signal, jvm_class_of, map_class_of, set_class_of};
use crate::eval::Evaluator;
use crate::ext::{Closure, NativeFn, NsHandle, munge};
use crate::io::{EmissionGuard, Writer, check_writer, write_to_writer};
use crate::lazy::{PendingSeq, realize_deep};
use crate::multi::invoke_multifn;
use crate::ns::{NativeFnPtr, check_arity, deref_core, signal_to_native};
use crate::reader::StreamReader;
use crate::xform::Eduction;

/// Dispatch tokens (§1b): stable per-kind strings echoing the JVM
/// table keys, plus the value `nil` for nil (handled separately)
/// and user keywords via meta `:type`.
pub(crate) const TOK_BOOLEAN: &str = "java.lang.Boolean";
pub(crate) const TOK_LONG: &str = "java.lang.Long";
pub(crate) const TOK_DOUBLE: &str = "java.lang.Double";
pub(crate) const TOK_BIGINT: &str = "clojure.lang.BigInt";
pub(crate) const TOK_BIGDEC: &str = "java.math.BigDecimal";
pub(crate) const TOK_RATIO: &str = "clojure.lang.Ratio";
pub(crate) const TOK_STRING: &str = "java.lang.String";
pub(crate) const TOK_CHARACTER: &str = "java.lang.Character";
pub(crate) const TOK_SYMBOL: &str = "clojure.lang.Symbol";
pub(crate) const TOK_KEYWORD: &str = "clojure.lang.Keyword";
pub(crate) const TOK_PLIST: &str = "clojure.lang.PersistentList";
pub(crate) const TOK_PVEC: &str = "clojure.lang.PersistentVector";
pub(crate) const TOK_MAPENTRY: &str = "clojure.lang.MapEntry";
pub(crate) const TOK_ARRAYMAP: &str = "clojure.lang.PersistentArrayMap";
pub(crate) const TOK_HASHMAP: &str = "clojure.lang.PersistentHashMap";
pub(crate) const TOK_TREEMAP: &str = "clojure.lang.PersistentTreeMap";
pub(crate) const TOK_HASHSET: &str = "clojure.lang.PersistentHashSet";
pub(crate) const TOK_TREESET: &str = "clojure.lang.PersistentTreeSet";
pub(crate) const TOK_PATTERN: &str = "java.util.regex.Pattern";
pub(crate) const TOK_TAGGED: &str = "clojure.lang.TaggedLiteral";
pub(crate) const TOK_READERCOND: &str = "clojure.lang.ReaderConditional";
pub(crate) const TOK_VAR: &str = "clojure.lang.Var";
pub(crate) const TOK_UNBOUND: &str = "clojure.lang.Var$Unbound";
pub(crate) const TOK_THROWABLE: &str = "java.lang.Throwable";
pub(crate) const TOK_ITRANSIENT: &str = "clojure.lang.ITransientCollection";
pub(crate) const TOK_ATOM: &str = "clojure.lang.Atom";
pub(crate) const TOK_DELAY: &str = "clojure.lang.Delay";
pub(crate) const TOK_AGENT: &str = "clojure.lang.Agent";
pub(crate) const TOK_FUTURE: &str = "clojure.core$future_call$reify";
pub(crate) const TOK_PROMISE: &str = "clojure.core$promise$reify";
pub(crate) const TOK_ISEQ: &str = "clojure.lang.ISeq";
pub(crate) const TOK_SUBVEC: &str = "clojure.lang.APersistentVector$SubVector";
pub(crate) const TOK_FN: &str = "clojure.lang.Fn";
pub(crate) const TOK_NS: &str = "clojure.lang.Namespace";
pub(crate) const TOK_WRITER: &str = "java.io.Writer";
pub(crate) const TOK_READER: &str = "clojure.lang.LineNumberingPushbackReader";
pub(crate) const TOK_REDUCED: &str = "clojure.lang.Reduced";
pub(crate) const TOK_EDUCTION: &str = "clojure.core.Eduction";
pub(crate) const TOK_CLASS: &str = "java.lang.Class";
pub(crate) const TOK_IRECORD: &str = "clojure.lang.IRecord";
pub(crate) const TOK_OBJECT: &str = "java.lang.Object";
pub(crate) const TOK_MULTIFN: &str = "clojure.lang.MultiFn";

/// The dispatch token for a value (`None` = actual nil only;
/// unknown kinds fall through to `Object` per §1b's total
/// fallthrough row). Exact per-kind strings (§1b) — coarse where
/// JVM shapes unify (all lists, all throwables, all transients,
/// every seq kind).
pub(crate) fn token_of(v: &Value) -> Option<&'static str> {
    match v {
        Value::Nil => None,
        Value::Extension(e) => Some(ext_token(e)),
        Value::HostType(h) => Some(host_token(h)),
        Value::Map(m) => Some(map_class_of(m)),
        Value::Set(s) => Some(set_class_of(s)),
        // Total: future `Value` variants (S10's `Volatile`, S15's
        // `Array`, ...) dispatch `Object`, whose default renders
        // today's engine shape — never the nil method.
        _ => Some(
            num_str_token(v)
                .or_else(|| sym_coll_token(v))
                .or_else(|| ref_token(v))
                .unwrap_or(TOK_OBJECT),
        ),
    }
}

/// Number/string tokens (split for the complexity budget).
fn num_str_token(v: &Value) -> Option<&'static str> {
    match v {
        Value::Bool(_) => Some(TOK_BOOLEAN),
        Value::Int(_) => Some(TOK_LONG),
        Value::Double(_) => Some(TOK_DOUBLE),
        Value::BigInt(_) => Some(TOK_BIGINT),
        Value::BigDec(_) => Some(TOK_BIGDEC),
        Value::Ratio(_) => Some(TOK_RATIO),
        Value::Str(_) => Some(TOK_STRING),
        Value::Char(_) => Some(TOK_CHARACTER),
        _ => None,
    }
}

/// Symbol/collection/reader tokens (split for the complexity budget).
fn sym_coll_token(v: &Value) -> Option<&'static str> {
    match v {
        Value::Symbol(_) => Some(TOK_SYMBOL),
        Value::Keyword(_) => Some(TOK_KEYWORD),
        Value::List(_) => Some(TOK_PLIST),
        Value::Vector(_) => Some(TOK_PVEC),
        Value::MapEntry { .. } => Some(TOK_MAPENTRY),
        Value::Regex(_) => Some(TOK_PATTERN),
        Value::TaggedLiteral { .. } => Some(TOK_TAGGED),
        Value::ReaderConditional { .. } => Some(TOK_READERCOND),
        _ => None,
    }
}

/// Reference/var/multifn tokens (split for the complexity budget).
fn ref_token(v: &Value) -> Option<&'static str> {
    match v {
        Value::Var(_) => Some(TOK_VAR),
        Value::Unbound(_) => Some(TOK_UNBOUND),
        Value::Throwable(_) => Some(TOK_THROWABLE),
        Value::Transient(_) => Some(TOK_ITRANSIENT),
        Value::Atom(_) => Some(TOK_ATOM),
        Value::Delay(_) => Some(TOK_DELAY),
        Value::Future(_) => Some(TOK_FUTURE),
        Value::Promise(_) => Some(TOK_PROMISE),
        Value::Agent(_) => Some(TOK_AGENT),
        Value::MultiFn(_) => Some(TOK_MULTIFN),
        _ => None,
    }
}

/// Extension tokens: evaluator-local types first, then the
/// sequence types, else the total `Object` fallthrough
/// (deftype-adjacent values, `Fx4Object`, test doubles).
fn ext_token(e: &Arc<dyn ExtensionValue>) -> &'static str {
    if let Some(t) = eval_ext_token(e) {
        return t;
    }
    seq_ext_token(e).unwrap_or(TOK_OBJECT)
}

/// Evaluator-local extension tokens (split for the complexity budget).
fn eval_ext_token(e: &Arc<dyn ExtensionValue>) -> Option<&'static str> {
    let any = e.as_any();
    if any.downcast_ref::<Closure>().is_some() {
        return Some(TOK_FN);
    }
    if any.downcast_ref::<NativeFn>().is_some() {
        return Some(TOK_FN);
    }
    if any.downcast_ref::<NsHandle>().is_some() {
        return Some(TOK_NS);
    }
    if any.downcast_ref::<Writer>().is_some() {
        return Some(TOK_WRITER);
    }
    if any.downcast_ref::<PendingSeq>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<Reduced>().is_some() {
        return Some(TOK_REDUCED);
    }
    if any.downcast_ref::<Eduction>().is_some() {
        return Some(TOK_EDUCTION);
    }
    if any.downcast_ref::<StreamReader>().is_some() {
        return Some(TOK_READER);
    }
    None
}

/// Sequence extension tokens (split for the complexity budget):
/// every seq kind unifies to `ISeq` except subvectors.
fn seq_ext_token(e: &Arc<dyn ExtensionValue>) -> Option<&'static str> {
    let any = e.as_any();
    if any.downcast_ref::<RealizedSeq>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<RangeValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<RepeatValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<CycleValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<IterateValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<ConsValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<KeySeqValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<ValSeqValue>().is_some() {
        return Some(TOK_ISEQ);
    }
    if any.downcast_ref::<SubVectorValue>().is_some() {
        return Some(TOK_SUBVEC);
    }
    None
}

/// Host tokens: descriptors are `Class`, records are `IRecord`,
/// every other instance is `Object`.
fn host_token(h: &HostVal) -> &'static str {
    match h {
        HostVal::Def(_) => TOK_CLASS,
        HostVal::Inst(i) if i.def.kind == HostKind::Record => TOK_IRECORD,
        HostVal::Inst(_) => TOK_OBJECT,
    }
}

/// The meta map the printer sees (mirrors the engine's
/// `print_meta_prefix` sources exactly: `Value::meta` plus the
/// behind-lock cell metas).
fn value_meta(v: &Value) -> Option<Map> {
    match v {
        Value::Atom(a) => a.meta(),
        Value::Agent(a) => a.meta(),
        Value::Future(f) => f.meta(),
        Value::Promise(p) => p.meta(),
        _ => v.meta().cloned(),
    }
}

/// The JVM-`IObj` set for `:default` chaining (a static kind table,
/// NOT the port's `with-meta` ability): collections (entries are
/// never `IObj`), `Symbol`, fns, seqs, records. `PendingSeq` is
/// excluded: its meta is invisible port-wide (`Value::meta`
/// cannot see it, so `:type` can never dispatch it here) and it
/// cannot rebuild — the `print-simple` arm terminates where
/// strip-and-reinvoke would loop.
fn is_jvm_iobj(v: &Value) -> bool {
    match v {
        Value::Symbol(_) | Value::List(_) | Value::Vector(_) | Value::Map(_) | Value::Set(_) => {
            true
        }
        Value::HostType(h) => {
            matches!(h.as_ref(), HostVal::Inst(i) if i.def.kind == HostKind::Record)
        }
        Value::Extension(e) => ext_is_iobj(e),
        _ => false,
    }
}

/// `IObj` extensions: fns plus the nine sequence types (subvectors
/// included; pending seqs excluded — see [`is_jvm_iobj`]). The fn
/// arms only matter past a removed `Fn` default or once fn meta
/// turns visible (§2d F7) — retained, not dead code.
fn ext_is_iobj(e: &Arc<dyn ExtensionValue>) -> bool {
    let any = e.as_any();
    any.downcast_ref::<Closure>().is_some()
        || any.downcast_ref::<NativeFn>().is_some()
        || seq_ext_token(e).is_some()
}

/// The `:type` key for dispatch and stripping.
fn type_key() -> Value {
    Value::Keyword(Keyword::new(None, "type"))
}

/// Strip `:type` from a value's meta for `:default` chaining
/// (empty-after-strip clears the meta; fns rebuild directly —
/// their meta lives outside `Value::meta`). The fn arms are
/// currently unreachable in non-degenerate flows (fn meta is
/// invisible to dispatch — §2d F7) and retained for when it
/// turns visible.
fn strip_type_meta(v: &Value) -> Value {
    if let Value::Extension(e) = v {
        let any = e.as_any();
        if let Some(c) = any.downcast_ref::<Closure>() {
            let mut next = c.clone();
            next.meta = strip_type_map(&next.meta);
            return Value::Extension(Arc::new(next));
        }
        if let Some(n) = any.downcast_ref::<NativeFn>() {
            let mut next = n.clone();
            next.meta = strip_type_map(&next.meta);
            return Value::Extension(Arc::new(next));
        }
    }
    match value_meta(v) {
        Some(m) if m.get(&type_key()).is_some() => v.clone().with_meta(strip_type_map(&Some(m))),
        _ => v.clone(),
    }
}

/// Strip `:type` from an option map (`None` when nothing meaningful
/// remains).
fn strip_type_map(meta: &Option<Map>) -> Option<Map> {
    let m = meta.as_ref()?;
    if m.get(&type_key()).is_none() {
        return meta.clone();
    }
    let stripped = m.clone().without(&type_key());
    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

/// A token as a dispatch value (`nil` for nil).
fn token_value(v: &Value) -> Value {
    match token_of(v) {
        None => Value::Nil,
        Some(t) => Value::Str(Arc::from(t)),
    }
}

/// The `print-method` dispatch value: the meta `:type` when it is
/// a keyword, else the token. (`print-dup` dispatches the token
/// only — [`token_value`].)
fn method_dispatch_value(v: &Value) -> Value {
    if let Some(m) = value_meta(v) {
        if let Some(Value::Keyword(kw)) = m.get(&type_key()) {
            return Value::Keyword(kw.clone());
        }
    }
    token_value(v)
}

/// The `pr-on` driver: `print-dup` under `*print-dup*`, else
/// `print-method` — through the live multis, or the booted
/// defaults when `print_defaults_only` is set (no `print-simple`
/// path sets it: the non-collection arm bypasses via engine-direct
/// `str`, and the meta prefix renders live like the JVM's).
/// Answers nil (like the JVM's `pr-on`).
pub(crate) fn pr_on(eval: &mut Evaluator, v: &Value, w: &Value) -> Result<Value, Signal> {
    if deref_core(eval, "*print-dup*").is_truthy() {
        pr_on_dup(eval, v, w)
    } else {
        pr_on_method(eval, v, w)
    }
}

/// One `print-method` step (live multi, or the static default).
fn pr_on_method(eval: &mut Evaluator, v: &Value, w: &Value) -> Result<Value, Signal> {
    if eval.print_defaults_only {
        let dv = method_dispatch_value(v);
        let f = static_method_fn(&dv);
        return f(eval, &[v.clone(), w.clone()]).map_err(|e| eval.throw(e));
    }
    match deref_core(eval, "print-method") {
        Value::MultiFn(mf) => invoke_multifn(eval, &mf, &[v.clone(), w.clone()]),
        // Unbooted (no core vars): the engine renders directly.
        Value::Nil | Value::Unbound(_) => engine_fallback(eval, v, w),
        // A redefined root breaks `pr` on both sides (implementer
        // latitude — degenerate, unpinned).
        other => Err(eval.throw(cce_err(&other, "clojure.lang.MultiFn"))),
    }
}

/// One `print-dup` step (live multi, or the static default — a
/// static miss throws the stock `No method` error).
fn pr_on_dup(eval: &mut Evaluator, v: &Value, w: &Value) -> Result<Value, Signal> {
    if eval.print_defaults_only {
        let dv = token_value(v);
        match static_dup_fn(&dv) {
            Some(f) => return f(eval, &[v.clone(), w.clone()]).map_err(|e| eval.throw(e)),
            None => return Err(eval.throw(no_method_dup(&dv))),
        }
    }
    match deref_core(eval, "print-dup") {
        Value::MultiFn(mf) => invoke_multifn(eval, &mf, &[v.clone(), w.clone()]),
        Value::Nil | Value::Unbound(_) => engine_fallback(eval, v, w),
        other => Err(eval.throw(cce_err(&other, "clojure.lang.MultiFn"))),
    }
}

/// The unbooted fallback: the engine renders directly (bare
/// evaluators have no multis to consult).
fn engine_fallback(eval: &mut Evaluator, v: &Value, w: &Value) -> Result<Value, Signal> {
    let opts = print_opts(eval, None).map_err(|e| eval.throw(e))?;
    let text = pr_to_string(v, &opts);
    write_to_writer(w, &text).map_err(|e| eval.throw(e))?;
    Ok(Value::Nil)
}

/// The stock missing-method error for static `print-dup` misses
/// (message-verbatim with the #37 `No method` error — the token
/// prints quoted).
fn no_method_dup(dispatch_val: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!(
            "No method in multimethod 'print-dup' for dispatch value: {}",
            pr_to_string(dispatch_val, &clj_value::PrintOpts::pr())
        ),
    )
}

/// Bind a `clojure.core` var to `value` (one frame; silently skips
/// missing vars — bare evaluators).
pub(crate) fn bind_core_var(eval: &mut Evaluator, name: &str, value: Value) {
    let sym = Symbol::new(Some("clojure.core"), name);
    if let Some(id) = eval.registry.lookup_in("clojure.core", &sym) {
        eval.push_binding(Var { id, sym }, value);
    }
}

/// Bind the `readably` override (`print` binds false, `pr` honors
/// the ambient binding). Answers the depth to pop to.
pub(crate) fn maybe_bind_print_readably(eval: &mut Evaluator, readably: Option<bool>) -> usize {
    let depth = eval.bindings_depth();
    if let Some(r) = readably {
        bind_core_var(eval, "*print-readably*", Value::Bool(r));
    }
    depth
}

/// `print-meta` through the driver: `^tag-or-map ` when the value
/// carries a non-empty meta and the policy holds (forced under
/// dup, else `meta && readably`). The tag/map renders through
/// [`pr_on`], so dup-aware meta values (ctors) and user methods
/// apply — exactly like the JVM's `print-meta`.
fn write_meta_prefix(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    let Some(m) = value_meta(o) else {
        return Ok(());
    };
    if m.is_empty() {
        return Ok(());
    }
    if !(opts.dup || (opts.meta && opts.readably)) {
        return Ok(());
    }
    let tag_key = Value::Keyword(Keyword::new(None, "tag"));
    write_to_writer(w, "^")?;
    match m.get(&tag_key) {
        Some(tag) if m.size() == 1 && tag.is_truthy() => {
            pr_on(eval, tag, w).map_err(signal_to_native)?;
        }
        _ => {
            pr_on(eval, &Value::Map(m), w).map_err(signal_to_native)?;
        }
    }
    write_to_writer(w, " ")?;
    Ok(())
}

/// Enter a collection level (the engine's dec-first `#`): spent
/// budgets write `#` and answer `None`; otherwise the decremented
/// level binds (skipped when unlimited or dup — dup ignores level)
/// and the answer is the depth to pop to.
fn level_child(
    eval: &mut Evaluator,
    opts: &PrintOpts,
    w: &Value,
) -> Result<Option<usize>, NativeError> {
    if opts.dup {
        return Ok(Some(eval.bindings_depth()));
    }
    let Some(limit) = opts.level else {
        return Ok(Some(eval.bindings_depth()));
    };
    if limit == 0 {
        write_to_writer(w, "#")?;
        return Ok(None);
    }
    let depth = eval.bindings_depth();
    let next = i64::try_from(limit - 1).unwrap_or(i64::MAX);
    bind_core_var(eval, "*print-level*", Value::Int(next));
    Ok(Some(depth))
}

/// The engine body without any meta prefix (production paths
/// never pass `dup: true` — §1d). Used for cells (which have no
/// rendered children, so nested policy is moot) and degenerate
/// direct calls.
fn write_engine_body(o: &Value, w: &Value, opts: &PrintOpts) -> Result<(), NativeError> {
    let body = PrintOpts {
        meta: false,
        dup: false,
        length: if opts.dup { None } else { opts.length },
        level: if opts.dup { None } else { opts.level },
        ..opts.clone()
    };
    write_to_writer(w, &pr_to_string(o, &body))
}

/// One sequential body through the driver (the engine's
/// `print-sequential` policy verbatim: `...` truncation, separator
/// placement). Children go through [`pr_on`], so user methods fire
/// nested.
fn write_seq_items(
    eval: &mut Evaluator,
    items: &[Value],
    begin: &str,
    end: &str,
    sep: &str,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    // Dup ignores length (like the JVM's `print-sequential`).
    let len = if opts.dup { None } else { opts.length };
    write_to_writer(w, begin)?;
    let mut first = true;
    for (i, item) in items.iter().enumerate() {
        if len.is_some_and(|max| i >= max) {
            if !first {
                write_to_writer(w, sep)?;
            }
            write_to_writer(w, "...")?;
            break;
        }
        if !first {
            write_to_writer(w, sep)?;
        }
        first = false;
        pr_on(eval, item, w).map_err(signal_to_native)?;
    }
    write_to_writer(w, end)?;
    Ok(())
}

/// The effective length cap (dup never truncates — the JVM's
/// `print-sequential` policy).
fn capped_len(opts: &PrintOpts) -> Option<usize> {
    if opts.dup { None } else { opts.length }
}

/// Whether index `i` trips the length cap.
fn is_truncated(len: Option<usize>, i: usize) -> bool {
    len.is_some_and(|max| i >= max)
}

/// The `", "` separator between map pairs (a no-op first).
fn write_pair_sep(w: &Value, first: bool) -> Result<(), NativeError> {
    if !first {
        write_to_writer(w, ", ")?;
    }
    Ok(())
}

/// The length-truncation tail (`", ..."` mid-map, `"..."` empty).
fn write_truncated_tail(w: &Value, first: bool) -> Result<(), NativeError> {
    write_pair_sep(w, first)?;
    write_to_writer(w, "...")
}

/// One `k v` pair through the driver (children see user methods).
fn write_map_pair(
    eval: &mut Evaluator,
    k: &Value,
    v: &Value,
    w: &Value,
) -> Result<(), NativeError> {
    pr_on(eval, k, w).map_err(signal_to_native)?;
    write_to_writer(w, " ")?;
    pr_on(eval, v, w).map_err(signal_to_native)?;
    Ok(())
}

/// One map body through the driver (`prefix{` + `k v` pairs, `", "`
/// separators — the engine's `print_map_entries` policy verbatim).
fn write_map_items(
    eval: &mut Evaluator,
    pairs: &[(Value, Value)],
    prefix: &str,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    let len = capped_len(opts);
    write_to_writer(w, prefix)?;
    write_to_writer(w, "{")?;
    let mut first = true;
    for (i, (k, v)) in pairs.iter().enumerate() {
        if is_truncated(len, i) {
            write_truncated_tail(w, first)?;
            break;
        }
        write_pair_sep(w, first)?;
        first = false;
        write_map_pair(eval, k, v, w)?;
    }
    write_to_writer(w, "}")?;
    Ok(())
}

/// Scalar shapes: the driver meta prefix plus the engine body.
/// (Nested tagged/throwable children render engine-direct — user
/// methods inside them do not fire. Unpinned JVM divergence,
/// documented: only collection shapes recurse through `pr_on`.)
fn scalar_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    match o {
        // Cells have no rendered children: the plain body (the
        // prefix above is their only meta).
        Value::Atom(_) | Value::Agent(_) | Value::Future(_) | Value::Promise(_) => {
            write_engine_body(o, w, &opts)?;
        }
        // Anything else renders top-meta-stripped (already
        // printed) with ambient nested policy; dup forces nested
        // meta, readable top strings/chars, and ignores
        // length/level.
        _ => {
            let bare = o.clone().with_meta(None);
            let body = PrintOpts {
                dup: false,
                meta: opts.meta || opts.dup,
                length: if opts.dup { None } else { opts.length },
                level: if opts.dup { None } else { opts.level },
                readably: opts.readably
                    || (opts.dup && matches!(bare, Value::Str(_) | Value::Char(_))),
                ..opts.clone()
            };
            write_to_writer(w, &pr_to_string(&bare, &body))?;
        }
    }
    Ok(Value::Nil)
}

/// Sequential shapes (lists, vectors, entries-as-2-vectors,
/// realized seqs): the driver meta prefix, the level gate, then
/// per-child [`pr_on`]. Unrealized extensions (unbounded seqs)
/// take the engine's fixed rendering with no level gate — exactly
/// like the engine.
fn seq_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    // Unrealized extensions bypass the level gate (the engine never
    // gates them — unbounded seqs print `(...)` at any level).
    if is_unrealized_seq(o) {
        write_engine_body(o, w, &opts)?;
        return Ok(Value::Nil);
    }
    let Some(depth) = level_child(eval, &opts, w)? else {
        return Ok(Value::Nil);
    };
    let result = write_seq_body(eval, o, w, &opts);
    eval.pop_bindings(depth);
    result?;
    Ok(Value::Nil)
}

/// Whether the value prints through the engine's fixed extension
/// rendering (unrealized seqs — the driver realizes everything
/// else before dispatching).
fn is_unrealized_seq(o: &Value) -> bool {
    match o {
        Value::List(_) | Value::Vector(_) | Value::MapEntry { .. } => false,
        Value::Extension(e) => e.as_any().downcast_ref::<RealizedSeq>().is_none(),
        _ => false,
    }
}

/// The sequential body for the value's own shape (split for the
/// complexity budget).
fn write_seq_body(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    match o {
        Value::List(l) => {
            let items: Vec<Value> = l.iter().cloned().collect();
            write_seq_items(eval, &items, "(", ")", " ", w, opts)
        }
        Value::Vector(v) => {
            let items: Vec<Value> = v.iter().cloned().collect();
            write_seq_items(eval, &items, "[", "]", " ", w, opts)
        }
        Value::MapEntry { key, val } => write_seq_items(
            eval,
            &[(**key).clone(), (**val).clone()],
            "[",
            "]",
            " ",
            w,
            opts,
        ),
        Value::Extension(e) => write_seq_ext_body(eval, o, e, w, opts),
        // Degenerate direct calls: the engine renders the value.
        _ => write_engine_body(o, w, opts),
    }
}

/// The sequential body for extension values (split for the
/// complexity budget): realized seqs print their items, every
/// other extension takes the engine's fixed rendering.
fn write_seq_ext_body(
    eval: &mut Evaluator,
    o: &Value,
    e: &Arc<dyn ExtensionValue>,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    if let Some(s) = e.as_any().downcast_ref::<RealizedSeq>() {
        return write_seq_items(eval, &s.items(), "(", ")", " ", w, opts);
    }
    write_engine_body(o, w, opts)
}

/// Map shapes: the driver meta prefix, the level gate, the shared
/// namespace lift, then per-entry [`pr_on`].
fn map_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    let Some(depth) = level_child(eval, &opts, w)? else {
        return Ok(Value::Nil);
    };
    let result = write_map_body(eval, o, w, &opts);
    eval.pop_bindings(depth);
    result?;
    Ok(Value::Nil)
}

/// The map body (split for the complexity budget).
fn write_map_body(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    let Value::Map(m) = o else {
        return write_engine_body(o, w, opts);
    };
    let entries: Vec<(&Value, &Value)> = m.iter().collect();
    let lifted = opts.namespace_maps.then(|| lift_ns(&entries)).flatten();
    match lifted {
        Some((ns, lifted)) => write_map_items(eval, &lifted, &format!("#:{ns}"), w, opts),
        None => {
            let pairs: Vec<(Value, Value)> = entries
                .iter()
                .map(|(k, v)| ((*k).clone(), (*v).clone()))
                .collect();
            write_map_items(eval, &pairs, "", w, opts)
        }
    }
}

/// Set shapes: the driver meta prefix, the level gate, then
/// per-item [`pr_on`].
fn set_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    let Some(depth) = level_child(eval, &opts, w)? else {
        return Ok(Value::Nil);
    };
    let result = write_set_body(eval, o, w, &opts);
    eval.pop_bindings(depth);
    result?;
    Ok(Value::Nil)
}

/// The set body (split for the complexity budget).
fn write_set_body(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    let Value::Set(s) = o else {
        return write_engine_body(o, w, opts);
    };
    let items: Vec<Value> = s.iter().cloned().collect();
    write_seq_items(eval, &items, "#{", "}", " ", w, opts)
}

/// Subvector shapes: the window items through [`pr_on`].
fn subvec_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    let Some(depth) = level_child(eval, &opts, w)? else {
        return Ok(Value::Nil);
    };
    let result = write_subvec_body(eval, o, w, &opts);
    eval.pop_bindings(depth);
    result?;
    Ok(Value::Nil)
}

/// The subvector body (split for the complexity budget).
fn write_subvec_body(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    if let Value::Extension(e) = o {
        if let Some(sv) = e.as_any().downcast_ref::<SubVectorValue>() {
            return write_seq_items(eval, &sv.items(), "[", "]", " ", w, opts);
        }
    }
    write_engine_body(o, w, opts)
}

/// Record shapes: `#Name{` + content-iteration entries (never
/// namespace-lifted — the engine path) through [`pr_on`].
fn record_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, o, w, &opts)?;
    let Some(depth) = level_child(eval, &opts, w)? else {
        return Ok(Value::Nil);
    };
    let result = write_record_body(eval, o, w, &opts);
    eval.pop_bindings(depth);
    result?;
    Ok(Value::Nil)
}

/// The record body (split for the complexity budget).
fn write_record_body(
    eval: &mut Evaluator,
    o: &Value,
    w: &Value,
    opts: &PrintOpts,
) -> Result<(), NativeError> {
    if let Value::HostType(h) = o {
        if let HostVal::Inst(i) = h.as_ref() {
            if i.def.kind == HostKind::Record {
                let pairs: Vec<(Value, Value)> = i
                    .content
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                return write_map_items(eval, &pairs, &format!("#{}", i.def.name), w, opts);
            }
        }
    }
    write_engine_body(o, w, opts)
}

/// The chaining `:default`: `IObj` values strip `:type` and
/// re-invoke `print-method`; anything else prints via
/// `print-simple` (the live root, so redefinitions apply).
fn method_default(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    if is_jvm_iobj(o) {
        let stripped = strip_type_meta(o);
        pr_on_method(eval, &stripped, w).map_err(signal_to_native)?;
    } else {
        invoke_print_simple(eval, o, w)?;
    }
    Ok(Value::Nil)
}

/// The `:default` print-simple arm: the native directly in
/// defaults-only mode, the live root otherwise.
fn invoke_print_simple(eval: &mut Evaluator, o: &Value, w: &Value) -> Result<(), NativeError> {
    if eval.print_defaults_only {
        native_print_simple(eval, &[o.clone(), w.clone()])?;
        return Ok(());
    }
    eval.call(&deref_core(eval, "print-simple"), &[o.clone(), w.clone()])
        .map_err(signal_to_native)?;
    Ok(())
}

/// Dup delegation (`→method`): the live `print-method` multi (or
/// the static default) renders the value, so user methods apply
/// under dup. The delegated side prints the meta prefix.
fn dup_delegate(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    check_writer(&args[1])?;
    pr_on_method(eval, &args[0], &args[1]).map_err(signal_to_native)?;
    Ok(Value::Nil)
}

/// The validated head of a dup ctor default (arity, writer,
/// ambient options, dup-forced meta prefix).
struct DupHead {
    o: Value,
    w: Value,
    opts: PrintOpts,
}

/// Read a dup ctor head (split so each ctor stays small).
fn dup_head(eval: &mut Evaluator, args: &[Value]) -> Result<DupHead, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    check_writer(&args[1])?;
    let opts = print_opts(eval, None)?;
    write_meta_prefix(eval, &args[0], &args[1], &opts)?;
    Ok(DupHead {
        o: args[0].clone(),
        w: args[1].clone(),
        opts,
    })
}

/// `#=(<class>/create {entries})` for array/tree maps (entries
/// dup-aware through [`pr_on`]; never truncated, never lifted).
fn dup_map_shape(head: DupHead, eval: &mut Evaluator, class: &str) -> Result<Value, NativeError> {
    let DupHead { o, w, opts } = head;
    let Value::Map(m) = &o else {
        write_engine_body(&o, &w, &opts)?;
        return Ok(Value::Nil);
    };
    write_to_writer(&w, "#=(")?;
    write_to_writer(&w, class)?;
    write_to_writer(&w, "/create {")?;
    let mut first = true;
    for (k, v) in m.iter() {
        if !first {
            write_to_writer(&w, ", ")?;
        }
        first = false;
        pr_on(eval, k, &w).map_err(signal_to_native)?;
        write_to_writer(&w, " ")?;
        pr_on(eval, v, &w).map_err(signal_to_native)?;
    }
    write_to_writer(&w, "})")?;
    Ok(Value::Nil)
}

/// Dup array-map ctors.
fn dup_arraymap(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    dup_map_shape(head, eval, TOK_ARRAYMAP)
}

/// Dup tree-map ctors.
fn dup_treemap(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    dup_map_shape(head, eval, TOK_TREEMAP)
}

/// `#=(<class>/create [items])` for tree sets, map entries, and
/// subvectors (items dup-aware through [`pr_on`]).
fn dup_seq_shape(
    eval: &mut Evaluator,
    items: &[Value],
    class: &str,
    w: &Value,
) -> Result<(), NativeError> {
    write_to_writer(w, "#=(")?;
    write_to_writer(w, class)?;
    write_to_writer(w, "/create [")?;
    let mut first = true;
    for item in items {
        if !first {
            write_to_writer(w, " ")?;
        }
        first = false;
        pr_on(eval, item, w).map_err(signal_to_native)?;
    }
    write_to_writer(w, "])")?;
    Ok(())
}

/// Dup tree-set ctors.
fn dup_treeset(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    let Value::Set(s) = &head.o else {
        write_engine_body(&head.o, &head.w, &head.opts)?;
        return Ok(Value::Nil);
    };
    let items: Vec<Value> = s.iter().cloned().collect();
    dup_seq_shape(eval, &items, TOK_TREESET, &head.w)?;
    Ok(Value::Nil)
}

/// Dup map-entry ctors.
fn dup_mapentry(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    let Value::MapEntry { key, val } = &head.o else {
        write_engine_body(&head.o, &head.w, &head.opts)?;
        return Ok(Value::Nil);
    };
    let pair = [key.as_ref().clone(), val.as_ref().clone()];
    dup_seq_shape(eval, &pair, TOK_MAPENTRY, &head.w)?;
    Ok(Value::Nil)
}

/// Dup subvector ctors.
fn dup_subvec(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    if let Value::Extension(e) = &head.o {
        if let Some(sv) = e.as_any().downcast_ref::<SubVectorValue>() {
            dup_seq_shape(eval, &sv.items(), TOK_SUBVEC, &head.w)?;
            return Ok(Value::Nil);
        }
    }
    write_engine_body(&head.o, &head.w, &head.opts)?;
    Ok(Value::Nil)
}

/// Dup vars (`#=(var ns/name)`, never a meta prefix — the method
/// skips `print-meta`, like the JVM's).
fn dup_var(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let Value::Var(v) = o else {
        let opts = print_opts(eval, None)?;
        write_engine_body(o, w, &opts)?;
        return Ok(Value::Nil);
    };
    write_to_writer(w, &format!("#=(var {})", v.sym))?;
    Ok(Value::Nil)
}

/// Dup namespaces (`#=(find-ns name)`, the name through `print-dup`
/// like the JVM's).
fn dup_ns(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let name = match o {
        Value::Extension(e) => match e.as_any().downcast_ref::<NsHandle>() {
            Some(h) => h.name.to_string(),
            None => {
                let opts = print_opts(eval, None)?;
                write_engine_body(o, w, &opts)?;
                return Ok(Value::Nil);
            }
        },
        _ => {
            let opts = print_opts(eval, None)?;
            write_engine_body(o, w, &opts)?;
            return Ok(Value::Nil);
        }
    };
    write_to_writer(w, "#=(find-ns ")?;
    let sym = Value::Symbol(Symbol::new(None, &name));
    pr_on_dup(eval, &sym, w).map_err(signal_to_native)?;
    write_to_writer(w, ")")?;
    Ok(Value::Nil)
}

/// Dup fns (`#=(<munged-display>. )`, via the empty-args
/// `print-ctor` shape).
fn dup_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let display = match o {
        Value::Extension(e) => {
            if let Some(c) = e.as_any().downcast_ref::<Closure>() {
                c.display.to_string()
            } else if let Some(n) = e.as_any().downcast_ref::<NativeFn>() {
                n.display.to_string()
            } else {
                let opts = print_opts(eval, None)?;
                write_engine_body(o, w, &opts)?;
                return Ok(Value::Nil);
            }
        }
        _ => {
            let opts = print_opts(eval, None)?;
            write_engine_body(o, w, &opts)?;
            return Ok(Value::Nil);
        }
    };
    write_to_writer(w, &format!("#=({}. )", munge(&display)))?;
    Ok(Value::Nil)
}

/// The dup-record payload (name, declared fields, content map),
/// or `None` for degenerate direct calls (non-records render
/// engine-direct).
fn dup_record_parts(o: &Value) -> Option<(String, Vec<Arc<str>>, Map)> {
    let Value::HostType(h) = o else {
        return None;
    };
    let HostVal::Inst(i) = h.as_ref() else {
        return None;
    };
    if i.def.kind != HostKind::Record {
        return None;
    }
    Some((
        i.def.name.to_string(),
        i.def.fields.clone(),
        i.content.clone(),
    ))
}

/// The `#Name[` record header.
fn write_record_header(w: &Value, name: &str) -> Result<(), NativeError> {
    write_to_writer(w, "#")?;
    write_to_writer(w, name)?;
    write_to_writer(w, "[")
}

/// Declared fields as lookup keys (`:field` keywords).
fn record_field_keys(fields: &[Arc<str>]) -> Vec<Value> {
    fields
        .iter()
        .map(|f| Value::Keyword(Keyword::new(None, f)))
        .collect()
}

/// One dup-record value (`, ` separator past the first).
fn write_dup_record_value(
    eval: &mut Evaluator,
    v: &Value,
    w: &Value,
    first: &mut bool,
) -> Result<(), NativeError> {
    if !*first {
        write_to_writer(w, ", ")?;
    }
    *first = false;
    pr_on(eval, v, w).map_err(signal_to_native)?;
    Ok(())
}

/// Declared fields in definition order (missing fields skipped —
/// the first half of `(vals r)`).
fn write_dup_record_fields(
    eval: &mut Evaluator,
    content: &Map,
    keys: &[Value],
    w: &Value,
    first: &mut bool,
) -> Result<(), NativeError> {
    for key in keys {
        let Some(val) = content.get(key) else {
            continue;
        };
        write_dup_record_value(eval, val, w, first)?;
    }
    Ok(())
}

/// The extension map: content entries past the field keys, in
/// content-iteration order (the second half of `(vals r)`).
fn write_dup_record_extmap(
    eval: &mut Evaluator,
    content: &Map,
    keys: &[Value],
    w: &Value,
    first: &mut bool,
) -> Result<(), NativeError> {
    for (k, v) in content.iter() {
        if keys.contains(k) {
            continue;
        }
        write_dup_record_value(eval, v, w, first)?;
    }
    Ok(())
}

/// Dup records (`#Name[v1, v2, ...]` — `(vals r)`: fields in
/// definition order, then extmap entries in content-iteration
/// order; always non-verbose).
fn dup_record(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let head = dup_head(eval, args)?;
    let Some((name, fields, content)) = dup_record_parts(&head.o) else {
        write_engine_body(&head.o, &head.w, &head.opts)?;
        return Ok(Value::Nil);
    };
    write_record_header(&head.w, &name)?;
    let keys = record_field_keys(&fields);
    let mut first = true;
    write_dup_record_fields(eval, &content, &keys, &head.w, &mut first)?;
    write_dup_record_extmap(eval, &content, &keys, &head.w, &mut first)?;
    write_to_writer(&head.w, "]")?;
    Ok(Value::Nil)
}

/// Dup type descriptors (`#=<name>`).
fn dup_class(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let name = match o {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => d.name.to_string(),
            _ => {
                let opts = print_opts(eval, None)?;
                write_engine_body(o, w, &opts)?;
                return Ok(Value::Nil);
            }
        },
        _ => {
            let opts = print_opts(eval, None)?;
            write_engine_body(o, w, &opts)?;
            return Ok(Value::Nil);
        }
    };
    write_to_writer(w, "#=")?;
    write_to_writer(w, &name)?;
    Ok(Value::Nil)
}

/// The `print-method` dispatch fn (`:type`-or-token).
fn dispatch_method(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-method", args, &[2])?;
    Ok(method_dispatch_value(&args[0]))
}

/// The `print-dup` dispatch fn (token only).
fn dispatch_dup(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-dup", args, &[2])?;
    Ok(token_value(&args[0]))
}

/// The booted `print-method` table (single source for boot and
/// static dispatch): every §1b token (`nil` rides separately).
const METHOD_TABLE: &[(&str, NativeFnPtr)] = &[
    (TOK_BOOLEAN, scalar_default),
    (TOK_LONG, scalar_default),
    (TOK_DOUBLE, scalar_default),
    (TOK_BIGINT, scalar_default),
    (TOK_BIGDEC, scalar_default),
    (TOK_RATIO, scalar_default),
    (TOK_STRING, scalar_default),
    (TOK_CHARACTER, scalar_default),
    (TOK_SYMBOL, scalar_default),
    (TOK_KEYWORD, scalar_default),
    (TOK_PATTERN, scalar_default),
    (TOK_TAGGED, scalar_default),
    (TOK_READERCOND, scalar_default),
    (TOK_VAR, scalar_default),
    (TOK_UNBOUND, scalar_default),
    (TOK_THROWABLE, scalar_default),
    (TOK_ITRANSIENT, scalar_default),
    (TOK_ATOM, scalar_default),
    (TOK_DELAY, scalar_default),
    (TOK_AGENT, scalar_default),
    (TOK_FUTURE, scalar_default),
    (TOK_PROMISE, scalar_default),
    (TOK_WRITER, scalar_default),
    (TOK_READER, scalar_default),
    (TOK_REDUCED, scalar_default),
    (TOK_EDUCTION, scalar_default),
    (TOK_CLASS, scalar_default),
    (TOK_OBJECT, scalar_default),
    (TOK_MULTIFN, scalar_default),
    (TOK_FN, scalar_default),
    (TOK_NS, scalar_default),
    (TOK_PLIST, seq_default),
    (TOK_PVEC, seq_default),
    (TOK_MAPENTRY, seq_default),
    (TOK_ISEQ, seq_default),
    (TOK_ARRAYMAP, map_default),
    (TOK_HASHMAP, map_default),
    (TOK_TREEMAP, map_default),
    (TOK_HASHSET, set_default),
    (TOK_TREESET, set_default),
    (TOK_SUBVEC, subvec_default),
    (TOK_IRECORD, record_default),
];

/// The booted `print-dup` table (single source for boot and
/// static dispatch): exactly the kinds with a JVM dup shape
/// (`nil` rides separately; no `:default`).
const DUP_TABLE: &[(&str, NativeFnPtr)] = &[
    (TOK_BOOLEAN, dup_delegate),
    (TOK_LONG, dup_delegate),
    (TOK_DOUBLE, dup_delegate),
    (TOK_BIGINT, dup_delegate),
    (TOK_BIGDEC, dup_delegate),
    (TOK_RATIO, dup_delegate),
    (TOK_STRING, dup_delegate),
    (TOK_CHARACTER, dup_delegate),
    (TOK_SYMBOL, dup_delegate),
    (TOK_KEYWORD, dup_delegate),
    (TOK_PATTERN, dup_delegate),
    (TOK_TAGGED, dup_delegate),
    (TOK_PLIST, dup_delegate),
    (TOK_PVEC, dup_delegate),
    (TOK_ISEQ, dup_delegate),
    (TOK_HASHMAP, dup_delegate),
    (TOK_HASHSET, dup_delegate),
    (TOK_ARRAYMAP, dup_arraymap),
    (TOK_TREEMAP, dup_treemap),
    (TOK_TREESET, dup_treeset),
    (TOK_MAPENTRY, dup_mapentry),
    (TOK_SUBVEC, dup_subvec),
    (TOK_VAR, dup_var),
    (TOK_NS, dup_ns),
    (TOK_FN, dup_fn),
    (TOK_IRECORD, dup_record),
    (TOK_CLASS, dup_class),
];

/// The static `print-method` default for a dispatch value (exact
/// entry else `:default` — the `method_for` semantics over the
/// booted table, user entries bypassed).
fn static_method_fn(dv: &Value) -> NativeFnPtr {
    match dv {
        Value::Nil => scalar_default,
        Value::Str(s) => METHOD_TABLE
            .iter()
            .find(|(t, _)| *t == s.as_ref())
            .map(|(_, f)| *f)
            .unwrap_or(method_default),
        _ => method_default,
    }
}

/// The static `print-dup` default for a dispatch value (`None`
/// misses to the stock `No method` throw).
fn static_dup_fn(dv: &Value) -> Option<NativeFnPtr> {
    match dv {
        Value::Nil => Some(dup_delegate),
        Value::Str(s) => DUP_TABLE
            .iter()
            .find(|(t, _)| *t == s.as_ref())
            .map(|(_, f)| *f),
        _ => None,
    }
}

/// `(print-simple o w)`: the `print-meta` prefix plus the §1f-`str`
/// rendering, both written to `w`. The prefix and the collection
/// arm render through the live driver, like the JVM's `RT.print`;
/// only non-collection object halves bypass (engine-direct `str`,
/// which never consults the multis).
fn native_print_simple(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-simple", args, &[2])?;
    let (o, w) = (&args[0], &args[1]);
    check_writer(w)?;
    let _emission = EmissionGuard::acquire();
    let opts = print_opts(eval, None)?;
    // The explicit prefix (live, like the JVM's `print-meta` — the
    // `str` half adds its own, the JVM's double prefix, since `str`
    // honors ambient bindings).
    write_meta_prefix(eval, o, w, &opts)?;
    print_simple_str(eval, o, w)?;
    Ok(Value::Nil)
}

/// The §1f-`str` rendering: collection-likes through the LIVE
/// driver (JVM collection `toString` routes through `RT.print` —
/// user methods fire, nested and top-level); everything else
/// through the existing native `str` arms (frozen behavior —
/// raw strings, lazy content-hashes, `toString` fallbacks —
/// which never consult the multis).
fn print_simple_str(eval: &mut Evaluator, o: &Value, w: &Value) -> Result<(), NativeError> {
    if is_simple_collection(o) {
        let r = realize_deep(eval, o).map_err(signal_to_native)?;
        // Liveness guard (a no-op now that no caller sets
        // defaults-only — kept so the arm stays live by construction).
        let saved = eval.print_defaults_only;
        eval.print_defaults_only = false;
        let result = pr_on(eval, &r, w).map_err(signal_to_native);
        eval.print_defaults_only = saved;
        result?;
        return Ok(());
    }
    let s = str_of_one(eval, o)?;
    write_to_writer(w, &s)
}

/// The §1f collection-likes (lazy seqs excluded — they take the
/// content-hash arm).
fn is_simple_collection(v: &Value) -> bool {
    if is_lazy_seq_str(v) {
        return false;
    }
    match v {
        Value::List(_)
        | Value::Vector(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::MapEntry { .. } => true,
        Value::Extension(e) => {
            seq_ext_token(e).is_some() || e.as_any().downcast_ref::<PendingSeq>().is_some()
        }
        _ => false,
    }
}

/// `(print-ctor o print-args w)`: `#=(<class>. <print-args o w>)`
/// (`print-args` runs as a normal call, so user closures work).
fn native_print_ctor(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/print-ctor", args, &[3])?;
    let (o, print_args_fn, w) = (&args[0], &args[1], &args[2]);
    // The JVM touches `w` first (`#=(` writes before `class`).
    check_writer(w)?;
    if matches!(o, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.Class.getName()\" because \"o\" is null".to_owned(),
        ));
    }
    // Serialized like `print_impl` (multi-write emission; the
    // user call in the middle re-enters safely).
    let _emission = EmissionGuard::acquire();
    write_to_writer(w, "#=(")?;
    write_to_writer(w, &jvm_class_of(o))?;
    write_to_writer(w, ". ")?;
    eval.call(print_args_fn, &[o.clone(), w.clone()])
        .map_err(signal_to_native)?;
    write_to_writer(w, ")")?;
    Ok(Value::Nil)
}

/// The print-system natives installed into `clojure.core`.
pub(crate) const PRINTSYS_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("print-simple", native_print_simple),
    ("print-ctor", native_print_ctor),
];

/// Boot the print system: the two multis (booted defaults), the
/// char-map vars, and the writer-fn arglists. Called from `boot`
/// after the natives install.
pub(crate) fn boot_print_system(eval: &mut Evaluator) {
    let mut methods = new_map();
    methods = methods.insert(
        Value::Nil,
        method_fn_value(eval, "print-method", scalar_default),
    );
    for (token, f) in METHOD_TABLE {
        methods = methods.insert(
            Value::Str(Arc::from(*token)),
            method_fn_value(eval, "print-method", *f),
        );
    }
    let default_key = Value::Keyword(Keyword::new(None, "default"));
    methods = methods.insert(
        default_key.clone(),
        method_fn_value(eval, "print-method", method_default),
    );
    let method_dispatch = method_fn_value(eval, "print-method", dispatch_method);
    install_plain_root(
        eval,
        "print-method",
        Value::MultiFn(Arc::new(MultiFn {
            name: Arc::from("print-method"),
            dispatch: method_dispatch,
            default: default_key,
            methods,
            prefers: new_map(),
            hierarchy: None,
        })),
    );
    let mut dup_methods = new_map();
    dup_methods = dup_methods.insert(Value::Nil, method_fn_value(eval, "print-dup", dup_delegate));
    for (token, f) in DUP_TABLE {
        dup_methods = dup_methods.insert(
            Value::Str(Arc::from(*token)),
            method_fn_value(eval, "print-dup", *f),
        );
    }
    let dup_dispatch = method_fn_value(eval, "print-dup", dispatch_dup);
    install_plain_root(
        eval,
        "print-dup",
        Value::MultiFn(Arc::new(MultiFn {
            name: Arc::from("print-dup"),
            dispatch: dup_dispatch,
            default: Value::Keyword(Keyword::new(None, "default")),
            methods: dup_methods,
            prefers: new_map(),
            hierarchy: None,
        })),
    );
    install_plain_root(eval, "char-escape-string", char_map_value(CHAR_ESCAPES));
    install_plain_root(eval, "char-name-string", char_map_value(CHAR_NAMES));
    set_arglists(eval, "print-simple", &["o", "w"]);
    set_arglists(eval, "print-ctor", &["o", "print-args", "w"]);
}

/// One booted method fn value (a `NativeFn` — invokable through
/// `eval.call` like any method fn).
fn method_fn_value(eval: &mut Evaluator, display: &str, f: NativeFnPtr) -> Value {
    let func: Arc<crate::ext::NativeImpl> = Arc::new(f);
    Value::Extension(Arc::new(NativeFn {
        id: eval.next_id(),
        display: Arc::from(display),
        func,
        meta: None,
    }))
}

/// Install a plain (non-dynamic) root var.
fn install_plain_root(eval: &mut Evaluator, name: &str, root: Value) {
    let q = Symbol::new(Some("clojure.core"), name);
    let id = eval.registry.intern(&q);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(root);
    });
}

/// Stamp `:arglists` meta on an installed var (single arity).
fn set_arglists(eval: &mut Evaluator, name: &str, params: &[&str]) {
    crate::ns::set_arglists_multi(eval, name, &[params]);
}

/// A char-map var root from a single-source table.
fn char_map_value(table: &[(char, &str)]) -> Value {
    let mut map = new_map();
    for (c, s) in table {
        map = map.insert(Value::Char(*c), Value::Str(Arc::from(*s)));
    }
    Value::Map(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;
    use crate::ext::{NativeImpl, as_closure};
    use crate::io::{string_writer, writer_contents};
    use clj_value::{Atom, HostDef, HostInst, Unbound, Volatile};

    fn booted_writer(eval: &mut Evaluator) -> Value {
        string_writer(eval)
    }

    fn last_value(eval: &mut Evaluator, src: &str) -> Value {
        eval.eval_source(src).unwrap().pop().unwrap()
    }

    fn test_def(kind: HostKind, name: &str, fields: &[&str]) -> Arc<HostDef> {
        Arc::new(HostDef {
            name: Arc::from(name),
            kind,
            fields: fields.iter().map(|f| Arc::from(*f)).collect(),
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        })
    }

    #[test]
    fn static_tables_mirror_boot() {
        // The static dispatch tables agree with the booted tables
        // entry for entry (user entries bypassed); unknown
        // dispatches miss to `:default` / `None`.
        for (token, f) in METHOD_TABLE {
            let dv = Value::Str(Arc::from(*token));
            assert!(std::ptr::fn_addr_eq(static_method_fn(&dv), *f), "{token}");
        }
        assert!(std::ptr::fn_addr_eq(
            static_method_fn(&Value::Nil),
            scalar_default as NativeFnPtr
        ));
        assert!(std::ptr::fn_addr_eq(
            static_method_fn(&Value::Str(Arc::from("bogus"))),
            method_default as NativeFnPtr
        ));
        assert!(std::ptr::fn_addr_eq(
            static_method_fn(&Value::Int(1)),
            method_default as NativeFnPtr
        ));
        for (token, f) in DUP_TABLE {
            let dv = Value::Str(Arc::from(*token));
            assert!(
                static_dup_fn(&dv).is_some_and(|g| std::ptr::fn_addr_eq(g, *f)),
                "{token}"
            );
        }
        assert!(
            static_dup_fn(&Value::Nil)
                .is_some_and(|g| std::ptr::fn_addr_eq(g, dup_delegate as NativeFnPtr))
        );
        assert!(static_dup_fn(&Value::Str(Arc::from("bogus"))).is_none());
        assert!(static_dup_fn(&Value::Int(1)).is_none());
    }

    #[test]
    fn defaults_only_bypasses_user_methods() {
        // `print_defaults_only` renders through the booted defaults
        // (user table entries bypassed). No production path sets
        // the flag (only `print_simple_str` touches it, and only to
        // save/set-false/restore) — the mode pins here directly.
        let mut eval = crate::boot();
        eval.eval_source(
            "(defmethod print-method \"java.lang.Long\" [o w] (print-simple \"HACKED\" w))",
        )
        .unwrap();
        let w = booted_writer(&mut eval);
        pr_on(&mut eval, &Value::Int(5), &w).unwrap();
        assert_eq!(writer_contents(&w).as_deref(), Some("HACKED"));
        let w2 = booted_writer(&mut eval);
        eval.print_defaults_only = true;
        pr_on(&mut eval, &Value::Int(5), &w2).unwrap();
        eval.print_defaults_only = false;
        assert_eq!(writer_contents(&w2).as_deref(), Some("5"));
    }

    #[test]
    fn defaults_only_dup_paths() {
        // Static `print-dup`: token hits delegate through the
        // static method side; misses throw the stock `No method`
        // error.
        let mut eval = crate::boot();
        eval.print_defaults_only = true;
        bind_core_var(&mut eval, "*print-dup*", Value::Bool(true));
        let w = booted_writer(&mut eval);
        pr_on(&mut eval, &Value::Int(5), &w).unwrap();
        assert_eq!(writer_contents(&w).as_deref(), Some("5"));
        let atom = Value::Atom(Atom::new(Value::Int(1)));
        let w2 = booted_writer(&mut eval);
        let err = pr_on(&mut eval, &atom, &w2).unwrap_err();
        assert!(
            matches!(err, Signal::Throw(_)),
            "expected a throw, got {err:?}"
        );
        let dbg = format!("{err:?}");
        assert!(dbg.contains("IllegalArgumentException"), "{dbg}");
        assert!(
            dbg.contains("No method in multimethod 'print-dup' for dispatch value:")
                && dbg.contains("clojure.lang.Atom"),
            "{dbg}"
        );
        eval.print_defaults_only = false;
    }

    #[test]
    fn defaults_only_default_takes_native_simple() {
        // The `:default` print-simple arm runs the native directly
        // in defaults-only mode (the live root otherwise).
        let mut eval = crate::boot();
        eval.print_defaults_only = true;
        let w = booted_writer(&mut eval);
        method_default(&mut eval, &[Value::Int(5), w.clone()]).unwrap();
        eval.print_defaults_only = false;
        assert_eq!(writer_contents(&w).as_deref(), Some("5"));
    }

    #[test]
    fn degenerate_direct_calls_render_engine_direct() {
        // Dispatch never routes a mismatched value to a default
        // (each table entry is keyed by the token its kind
        // produces, and dispatch computes the token from the
        // value) — these arms only run for direct method-fn calls,
        // where they render engine-direct.
        let mut eval = crate::boot();
        let cases: &[(&str, NativeFnPtr, Value)] = &[
            ("seq", seq_default, Value::Int(5)),
            ("map", map_default, Value::Int(5)),
            ("set", set_default, Value::Int(5)),
            ("subvec", subvec_default, Value::Int(5)),
            ("record", record_default, Value::Int(5)),
            ("arraymap", dup_arraymap, Value::Int(5)),
            ("treeset", dup_treeset, Value::Int(5)),
            ("mapentry", dup_mapentry, Value::Int(5)),
            ("subvec-ctor", dup_subvec, Value::Int(5)),
            ("var", dup_var, Value::Int(5)),
            ("ns", dup_ns, Value::Int(5)),
            ("fn", dup_fn, Value::Int(5)),
            ("record-ctor", dup_record, Value::Int(5)),
            ("class", dup_class, Value::Int(5)),
        ];
        for (name, f, v) in cases {
            let w = booted_writer(&mut eval);
            let out = f(&mut eval, &[v.clone(), w.clone()]).unwrap();
            assert_eq!(out, Value::Nil, "{name}");
            assert_eq!(writer_contents(&w).as_deref(), Some("5"), "{name}");
        }
        // Extension-typed mismatches (each `else` shape in turn).
        let closure = last_value(&mut eval, "(fn [x] x)");
        let w = booted_writer(&mut eval);
        dup_ns(&mut eval, &[closure.clone(), w.clone()]).unwrap();
        assert!(writer_contents(&w).unwrap().starts_with("#object["));
        let ns = last_value(&mut eval, "(find-ns 'user)");
        let w = booted_writer(&mut eval);
        dup_fn(&mut eval, &[ns, w.clone()]).unwrap();
        assert!(writer_contents(&w).unwrap().starts_with("#object["));
        let w = booted_writer(&mut eval);
        subvec_default(&mut eval, &[closure.clone(), w.clone()]).unwrap();
        assert!(writer_contents(&w).unwrap().starts_with("#object["));
        let w = booted_writer(&mut eval);
        dup_subvec(&mut eval, &[closure.clone(), w.clone()]).unwrap();
        assert!(writer_contents(&w).unwrap().starts_with("#object["));
        // Host-typed mismatches (descriptor vs instance vs record).
        let def = test_def(HostKind::Type, "user.DT", &["x"]);
        let deftype_inst = Value::HostType(Arc::new(HostVal::Inst(HostInst {
            def: def.clone(),
            content: new_map(),
            meta: None,
        })));
        let descriptor = Value::HostType(Arc::new(HostVal::Def(def)));
        for (name, f, v) in [
            (
                "record-ctor-def",
                dup_record as NativeFnPtr,
                descriptor.clone(),
            ),
            (
                "record-ctor-deftype",
                dup_record as NativeFnPtr,
                deftype_inst.clone(),
            ),
            ("class-inst", dup_class as NativeFnPtr, deftype_inst.clone()),
        ] {
            let w = booted_writer(&mut eval);
            f(&mut eval, &[v, w.clone()]).unwrap();
            assert!(!writer_contents(&w).unwrap().is_empty(), "{name}");
        }
        for v in [descriptor, deftype_inst] {
            let w = booted_writer(&mut eval);
            record_default(&mut eval, &[v, w.clone()]).unwrap();
            assert!(!writer_contents(&w).unwrap().is_empty());
        }
        // A non-sequence extension through the sequential body (the
        // `seq_default` unrealized guard returns before this — only
        // a direct call reaches it).
        let w = booted_writer(&mut eval);
        let opts = print_opts(&eval, None).unwrap();
        write_seq_body(&mut eval, &closure, &w, &opts).unwrap();
        assert!(writer_contents(&w).unwrap().starts_with("#object["));
    }

    #[test]
    fn strip_type_handles_fn_and_plain_metas() {
        // Fn metas live outside `Value::meta`, so dispatch never
        // sees fn `:type` (§2d F7) — these arms only run for direct
        // calls (removing the `Fn` method would loop
        // strip-and-reinvoke, so no surface test can reach them).
        // Plain values route through `value_meta`
        // (surface-pinned); the no-type arm pins here.
        let key = type_key();
        let with_type = new_map().insert(key.clone(), Value::Keyword(Keyword::new(None, "mine")));
        let closure = Closure {
            id: 1,
            display: Arc::from("user/f"),
            arities: Vec::new(),
            env: Env::new(),
            meta: Some(with_type.clone()),
            conveyed: None,
        };
        // `:type`-only fn meta strips to nothing ...
        let stripped = strip_type_meta(&Value::Extension(Arc::new(closure.clone())));
        assert!(as_closure(&stripped).unwrap().meta.is_none());
        // ... extra keys survive minus `:type` ...
        let rich = new_map()
            .insert(key.clone(), Value::Keyword(Keyword::new(None, "mine")))
            .insert(Value::Keyword(Keyword::new(None, "k")), Value::Int(1));
        let rich_closure = Closure {
            meta: Some(rich),
            ..closure.clone()
        };
        let stripped = strip_type_meta(&Value::Extension(Arc::new(rich_closure)));
        let meta = as_closure(&stripped).unwrap().meta.clone().unwrap();
        assert_eq!(meta.size(), 1);
        assert!(meta.get(&key).is_none());
        // ... present-but-typeless fn meta clones back ...
        let kmeta = new_map().insert(Value::Keyword(Keyword::new(None, "k")), Value::Int(1));
        let k_closure = Closure {
            meta: Some(kmeta.clone()),
            ..closure.clone()
        };
        let stripped = strip_type_meta(&Value::Extension(Arc::new(k_closure)));
        let kept = as_closure(&stripped).unwrap().meta.clone().unwrap();
        assert_eq!(kept.size(), 1);
        assert!(kept.get(&key).is_none());
        // ... missing fn meta stays missing ...
        let plain_closure = Closure {
            meta: None,
            ..closure.clone()
        };
        let stripped = strip_type_meta(&Value::Extension(Arc::new(plain_closure)));
        assert!(as_closure(&stripped).unwrap().meta.is_none());
        // ... and native fns strip the same way.
        let func: Arc<NativeImpl> = Arc::new(|_, _| Ok(Value::Nil));
        let native = NativeFn {
            id: 2,
            display: Arc::from("user/g"),
            func,
            meta: Some(with_type),
        };
        let stripped = strip_type_meta(&Value::Extension(Arc::new(native)));
        assert!(matches!(stripped, Value::Extension(_)));
        // Plain values without `:type` pass through.
        let bare = Value::Vector(clj_value::Vector::from_vec(vec![Value::Int(1)]));
        assert_eq!(strip_type_meta(&bare), bare);
    }

    #[test]
    fn token_fallthroughs() {
        // Total-dispatch arms: tagged literals and unbounds are
        // unconstructible from Clojure surface (dup-table comment);
        // pending seqs DO reach dispatch on direct `print-method` /
        // `print-dup` surface calls (the chain is
        // realization-free — only the pr-family driver path
        // pre-realizes via `collect_all`); volatiles take the
        // `Object` fallthrough (no suite prints one through the
        // driver).
        let tagged = Value::TaggedLiteral {
            tag: Symbol::new(None, "x"),
            form: Box::new(Value::Int(1)),
        };
        assert_eq!(token_of(&tagged), Some(TOK_TAGGED));
        let unbound = Value::Unbound(Unbound {
            sym: Symbol::new(Some("user"), "u"),
            var_id: 1,
        });
        assert_eq!(token_of(&unbound), Some(TOK_UNBOUND));
        let mut eval = crate::boot();
        let pending = last_value(&mut eval, "(map inc [1 2])");
        assert_eq!(token_of(&pending), Some(TOK_ISEQ));
        let vol = Value::Volatile(Volatile::new(Value::Int(1)));
        assert_eq!(token_of(&vol), Some(TOK_OBJECT));
    }

    #[test]
    fn dup_record_skips_missing_fields() {
        // A record missing a declared field skips it (defensive:
        // the ctors always fill every field and `dissoc` of a field
        // loses recordness, so surface values never trigger this).
        let mut eval = crate::boot();
        let def = test_def(HostKind::Record, "user.RP", &["x", "y"]);
        let content = new_map().insert(Value::Keyword(Keyword::new(None, "x")), Value::Int(1));
        let w = booted_writer(&mut eval);
        let keys = record_field_keys(&def.fields);
        let mut first = true;
        write_dup_record_fields(&mut eval, &content, &keys, &w, &mut first).unwrap();
        assert_eq!(writer_contents(&w).as_deref(), Some("1"));
    }
}
