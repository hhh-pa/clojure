// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Host pieces the FX4 expansions call without new special forms:
//! `req!`, `some-vals`, `with-redefs-fn`, `bound-fn*`, the
//! array-map constructor, and the monitor natives. `instance?` /
//! `identical?` / `(new Object)` / `EMPTY` rewrite at analyze time
//! (`instance?` and `identical?` stay unresolved vars).

use std::any::Any;
use std::cell::RefCell;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use clj_value::{ExtensionValue, List, Map, Symbol, ThrowClass, Value, new_map};

use crate::coll::lookup_present;
use crate::concurrency::{LockingGuard, locking_acquire_key, monitor_key};
use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ext::as_closure;
use crate::fx4_form::{iae, str_of};
use crate::lazy::collect_all;
use crate::ns::{NativeFnPtr, cast_err, check_arity, npe, signal_to_native};

/// Natives installed into `clojure.core` by boot. `fx4-*` are private.
pub(crate) const FX4_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("req!", native_req),
    ("some-vals", native_some_vals),
    ("with-redefs-fn", native_with_redefs_fn),
    ("bound-fn*", native_bound_fn),
    ("fx4-pam-create", native_pam),
    ("fx4-monitor-enter", native_monitor_enter),
    ("fx4-monitor-exit", native_monitor_exit),
];

/// Private names (referred nowhere).
pub(crate) const FX4_PRIVATE: &[&str] =
    &["fx4-pam-create", "fx4-monitor-enter", "fx4-monitor-exit"];

static NEXT_OBJ: AtomicU64 = AtomicU64::new(1);

/// A unique `java.lang.Object` (`(new Object)` not-found sentinel).
#[derive(Debug)]
pub(crate) struct Fx4Object {
    id: u64,
}

impl Fx4Object {
    pub(crate) fn fresh() -> Self {
        Self {
            id: NEXT_OBJ.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl ExtensionValue for Fx4Object {
    fn class_name(&self) -> String {
        "java.lang.Object".to_owned()
    }

    fn print(&self) -> String {
        format!("#object[java.lang.Object 0x{:x}]", self.id)
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Fx4Object>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// What an expansion call lowers to before ordinary analysis.
pub(crate) enum HostOp<'a> {
    None,
    SeqQ(&'a Value),
    Identical(&'a Value, &'a Value),
    NewObject,
    Pam(&'a Value),
    MonitorEnter(&'a Value),
    MonitorExit(&'a Value),
    LetFnStar,
    /// A direct qualified `instance?` call with a non-2 arity (the
    /// count rides along for the compile-time arity error).
    InstanceArity(usize),
    /// A direct qualified `identical?` call with a non-2 arity.
    IdenticalArity(usize),
}

/// Classify a call the expansions emit. Operator position never
/// defers to locals here: `new`/`letfn*` are real specials and the
/// `monitor-*` macros beat shadowing bindings too (Oracle FX4
/// `shadow-new/monitor/letfn`), matching the analyzer's
/// `is_shadowable_macro` set, which excludes all these heads.
pub(crate) fn classify<'a>(items: &[&'a Value]) -> HostOp<'a> {
    let Some(Value::Symbol(sym)) = items.first().copied() else {
        return HostOp::None;
    };
    match (sym.namespace(), sym.name()) {
        (Some("clojure.core"), "instance?") => classify_instance(items),
        (Some("clojure.core"), "identical?") => classify_identical(items),
        (None, "new") => classify_new(items),
        (Some("clojure.lang.PersistentArrayMap"), "createAsIfByAssoc") => classify_pam(items),
        (None, "monitor-enter") => one_arg(items, HostOp::MonitorEnter),
        (None, "monitor-exit") => one_arg(items, HostOp::MonitorExit),
        (None, "letfn*") => HostOp::LetFnStar,
        _ => HostOp::None,
    }
}

fn classify_instance<'a>(items: &[&'a Value]) -> HostOp<'a> {
    match items {
        [_, Value::Symbol(class), arg]
            if class.namespace().is_none() && class.name() == "clojure.lang.ISeq" =>
        {
            HostOp::SeqQ(arg)
        }
        // Two-arg calls otherwise fall through to the `instance?`
        // var; any other arity of a direct call is a compile error.
        [_, _, _] => HostOp::None,
        _ => HostOp::InstanceArity(items.len() - 1),
    }
}

fn classify_identical<'a>(items: &[&'a Value]) -> HostOp<'a> {
    match items {
        [_, a, b] => HostOp::Identical(a, b),
        _ => HostOp::IdenticalArity(items.len() - 1),
    }
}

fn classify_new<'a>(items: &[&'a Value]) -> HostOp<'a> {
    match items {
        [_, Value::Symbol(class)] if class.namespace().is_none() && class.name() == "Object" => {
            HostOp::NewObject
        }
        _ => HostOp::None,
    }
}

fn classify_pam<'a>(items: &[&'a Value]) -> HostOp<'a> {
    let [_, arg] = items else {
        return HostOp::None;
    };
    let Value::List(list) = arg else {
        return HostOp::None;
    };
    let mut it = list.iter();
    let Some(Value::Symbol(head)) = it.next() else {
        return HostOp::None;
    };
    if head.namespace() != Some("clojure.core") || head.name() != "to-array" {
        return HostOp::None;
    }
    match it.next() {
        Some(inner) if it.next().is_none() => HostOp::Pam(inner),
        _ => HostOp::None,
    }
}

fn one_arg<'a>(items: &[&'a Value], make: fn(&'a Value) -> HostOp<'a>) -> HostOp<'a> {
    match items {
        [_, arg] => make(arg),
        _ => HostOp::None,
    }
}

/// `clojure.lang.PersistentArrayMap/EMPTY` in value position.
pub(crate) fn pam_empty(sym: &Symbol) -> Option<Value> {
    if sym.namespace() == Some("clojure.lang.PersistentArrayMap") && sym.name() == "EMPTY" {
        Some(Value::Map(new_map()))
    } else {
        None
    }
}

fn native_req(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/req!", args, &[2])?;
    match lookup_present(eval, &args[0], &args[1])? {
        Some(v) => Ok(v),
        None => Err(iae(format!("Missing required key: {}", str_of(&args[1])))),
    }
}

fn native_some_vals(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/some-vals", args, &[1])?;
    match &args[0] {
        Value::Nil => Ok(Value::Nil),
        Value::Map(m) => Ok(some_vals_map(m)),
        other => Err(cce_err(other, "clojure.lang.IPersistentMap")),
    }
}

fn some_vals_map(map: &Map) -> Value {
    let mut out = new_map();
    let mut any = false;
    for (k, v) in map.iter() {
        if !matches!(v, Value::Nil) {
            out = out.insert(k.clone(), v.clone());
            any = true;
        }
    }
    if any { Value::Map(out) } else { Value::Nil }
}

fn native_with_redefs_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/with-redefs-fn", args, &[2])?;
    let pairs = redef_pairs(&args[0])?;
    let saved = save_roots(eval, &pairs);
    apply_roots(eval, &pairs);
    let result = eval.call(&args[1], &[]);
    restore_roots(eval, &saved);
    result.map_err(signal_to_native)
}

fn redef_pairs(map: &Value) -> Result<Vec<(u64, Value)>, NativeError> {
    match map {
        Value::Nil => Ok(Vec::new()),
        Value::Map(m) => map_pairs(m),
        other => Err(cce_err(other, "clojure.lang.IPersistentMap")),
    }
}

fn map_pairs(map: &Map) -> Result<Vec<(u64, Value)>, NativeError> {
    let mut out = Vec::new();
    for (k, v) in map.iter() {
        let Value::Var(var) = k else {
            return Err(cast_err(k, "clojure.lang.Var"));
        };
        out.push((var.id, v.clone()));
    }
    Ok(out)
}

fn save_roots(eval: &Evaluator, pairs: &[(u64, Value)]) -> Vec<(u64, Option<Value>)> {
    pairs
        .iter()
        .map(|(id, _)| {
            let old = eval.registry.state(*id).and_then(|s| s.root.clone());
            (*id, old)
        })
        .collect()
}

fn apply_roots(eval: &mut Evaluator, pairs: &[(u64, Value)]) {
    for (id, value) in pairs {
        eval.registry.with_state_mut(*id, |st| {
            st.root = Some(value.clone());
        });
    }
}

fn restore_roots(eval: &mut Evaluator, saved: &[(u64, Option<Value>)]) {
    for (id, old) in saved.iter().rev() {
        eval.registry.with_state_mut(*id, |st| {
            st.root = old.clone();
        });
    }
}

fn native_bound_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/bound-fn*", args, &[1])?;
    let Some(closure) = as_closure(&args[0]) else {
        return Err(cast_err(&args[0], "clojure.lang.IFn"));
    };
    let mut owned = closure.clone();
    owned.conveyed = Some(eval.clone_bindings());
    Ok(Value::Extension(Arc::new(owned)))
}

fn native_pam(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/fx4-pam-create", args, &[1])?;
    let items = collect_all(eval, &args[0]).map_err(signal_to_native)?;
    let list = Value::List(List::from_vec(items));
    eval.seq_to_map(&list).map_err(signal_to_native)
}

thread_local! {
    static MONITORS: RefCell<Vec<(u64, LockingGuard<'static>)>> =
        const { RefCell::new(Vec::new()) };
}

fn native_monitor_enter(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/fx4-monitor-enter", args, &[1, 2])?;
    if matches!(args[0], Value::Nil) {
        return Err(npe(&nil_lockee_message("enter", args.get(1))));
    }
    let key = monitor_key(&args[0]);
    let guard = locking_acquire_key(key);
    MONITORS.with(|stack| stack.borrow_mut().push((key, guard)));
    Ok(Value::Nil)
}

fn native_monitor_exit(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/fx4-monitor-exit", args, &[1, 2])?;
    // Fence 4b E1k/E1t/E1u (rule order matters): a nil lockee with
    // an empty stack is the form-naming NPE; a nil lockee over held
    // monitors is a mismatch (`nil` never matches) — live-oracle
    // matrix (kept-fixes.out, all JIT modes; pure-bytecode matrix
    // in monitor-matrix-jvms.txt), against review B3's JVMS reading.
    // Otherwise the lockee's 3b key must be held (content keys
    // for Box/symbol kinds, so clone exits match their enter;
    // minted ids elsewhere, which behave identity-like; the most
    // recent hold releases, so reentrant nests unwind in order).
    if matches!(args[0], Value::Nil) {
        let held = MONITORS.with(|stack| !stack.borrow().is_empty());
        if held {
            return Err(ims());
        }
        return Err(npe(&nil_lockee_message("exit", args.get(1))));
    }
    let key = monitor_key(&args[0]);
    let released = MONITORS.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack
            .iter()
            .rposition(|(held, _)| *held == key)
            .map(|pos| stack.remove(pos))
            .is_some()
    });
    if !released {
        return Err(ims());
    }
    Ok(Value::Nil)
}

/// Unmatched `monitor-exit` (fence 4b E1k): the JVM's
/// `IllegalMonitorStateException` with an empty message.
fn ims() -> NativeError {
    NativeError::new(ThrowClass::IllegalMonitorStateException, String::new())
}

/// How a nil lockee names itself in the `monitor-enter`/
/// `monitor-exit` NPE (fence 4b E1r/E1s/E1u/E1v/E1w/E1x/E1y — the
/// JVM's compiler-generated null messages over the EXPANDED form,
/// which the analyzer quotes as the natives' second argument):
/// nil/`(quote nil)` → `"null"`; symbols → the name; calls → the
/// `IFn.invoke` return-value template by arity (`RT.get` and other
/// inlined callees stay port-fixed as `IFn.invoke`; `set!`
/// compiles to `Var.set`, pinned exactly); `try`/`loop` → the
/// synthetic 0-arg template; `let`/`let*`/`letfn*`/`do` → the last
/// body form (`"null"` when body-less); `if`/`case*` and anything
/// else → the bare message (the JVM gives up on branches the same
/// way). A missing form (direct native calls) keeps the legacy
/// `"lock"` placeholder. Nested monitor forms stay divergent (E1w
/// `nestmon`: the JVM throws IMS there by an unknown mechanism).
enum NilLockee {
    Null,
    Local(String),
    Call(String, usize),
    Bare,
}

fn nil_lockee_message(verb: &str, form: Option<&Value>) -> String {
    let Some(form) = form else {
        // Direct native calls pass no form (legacy placeholder).
        return format!("Cannot {verb} synchronized block because \"lock\" is null");
    };
    match nil_lockee_tag(form) {
        NilLockee::Null => {
            format!("Cannot {verb} synchronized block because \"null\" is null")
        }
        NilLockee::Local(name) => {
            format!("Cannot {verb} synchronized block because \"{name}\" is null")
        }
        NilLockee::Call(method, arity) => {
            let desc = vec!["Object"; arity].join(", ");
            format!(
                "Cannot {verb} synchronized block because the return value of \"{method}({desc})\" is null"
            )
        }
        NilLockee::Bare => format!("Cannot {verb} synchronized block"),
    }
}

fn nil_lockee_tag(form: &Value) -> NilLockee {
    // Default bare; every line below runs per call, so no arm is
    // dead (each refinement is pinned by its own probe row).
    let mut tag = NilLockee::Bare;
    if matches!(form, Value::Nil) {
        tag = NilLockee::Null;
    } else if let Value::Symbol(sym) = form {
        tag = NilLockee::Local(format!("{sym}"));
    } else if let Value::List(l) = form {
        tag = nil_lockee_list(l);
    }
    tag
}

fn nil_lockee_list(l: &List) -> NilLockee {
    let head = l.iter().next();
    // `()` never evaluates nil, so this arity is unused there.
    let arity = l.len().saturating_sub(1);
    let mut tag = NilLockee::Call("clojure.lang.IFn.invoke".to_owned(), arity);
    if list_head_is(head, "quote") {
        // `(quote nil)`; other quotes never evaluate nil.
        tag = NilLockee::Null;
    } else if list_head_is(head, "let")
        || list_head_is(head, "let*")
        || list_head_is(head, "letfn")
        || list_head_is(head, "letfn*")
        || list_head_is(head, "do")
    {
        // The value form is the last body form; a body-less
        // `let`/`do` evaluates nil like a `nil` literal (E1w).
        let off = if list_head_is(head, "do") { 1 } else { 2 };
        tag = match l.iter().skip(off).last() {
            Some(last) => nil_lockee_tag(last),
            None => NilLockee::Null,
        };
    } else if list_head_is(head, "if") || list_head_is(head, "case*") {
        tag = NilLockee::Bare;
    } else if list_head_is(head, "try") || list_head_is(head, "loop") || list_head_is(head, "loop*")
    {
        tag = NilLockee::Call("clojure.lang.IFn.invoke".to_owned(), 0);
    } else if list_head_is(head, "set!") {
        // `set!` on a var compiles to `Var.set` (E1y); field
        // targets are out of scope (interop fence).
        tag = NilLockee::Call("clojure.lang.Var.set".to_owned(), 1);
    }
    tag
}

fn list_head_is(head: Option<&Value>, name: &str) -> bool {
    matches!(head, Some(Value::Symbol(s)) if s.name() == name && s.namespace().is_none_or(|ns| ns == "clojure.core"))
}
