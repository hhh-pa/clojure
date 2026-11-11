// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The register VM (#226): analyzed [`Expr`] trees compile to nested
//! closures ([`Exec`]) over flat per-call register frames.
//!
//! The tree evaluator keeps every local in a linked chain of shared
//! frames (one allocation per `let`/call, a chain walk per read) so
//! closures can capture the chain. Here each fn arity runs over one
//! flat register array sized at compile time: params, `let` and
//! `loop` locals are register writes, `recur` rewrites the loop's
//! registers in place, and closures capture the values they use
//! (Clojure locals are immutable, so a value snapshot is the same
//! observable state the shared frame gave). An analyzed local is an
//! absolute lexical index; the compiler maps each to a register, a
//! captured slot ([`Loc::Up`]), or a `letfn` sibling ([`Loc::Rec`]).
//!
//! Forms the VM does not compile run on the tree evaluator
//! ([`escape`]): the visible locals are copied into a fresh [`Env`]
//! at their lexical indexes, so the tree evaluator sees exactly the
//! frame it would have built. A `recur` from such a subtree arrives
//! as the ordinary `Signal::Recur(values)`; the VM's own `recur`
//! writes the target registers first and signals with an empty
//! vector (no allocation).

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, OnceLock};

use clj_value::{Map, Symbol, ThrowClass, Throwable, Value, Var, Vector};
use smallvec::SmallVec;

use crate::env::{Env, SharedRegistry};
use crate::error::Signal;
use crate::eval::Evaluator;
use crate::expr::{Arity, CaseTest, Catch, DirectCall, Expr, FunExpr, Pattern, pattern_count};
use crate::ext::Closure;
use clj_reader::Span;

/// Compiled code producing a `T` against the running frame.
pub(crate) type Code<T> =
    Box<dyn for<'f> Fn(&mut Evaluator, &mut Frame<'f>) -> Result<T, Signal> + Send + Sync>;

/// One compiled node: evaluate against the running frame.
pub(crate) type Exec = Code<Value>;

/// A compiled branch test: a form's truthiness (comparisons and
/// boolean intrinsics answer without a boolean `Value`).
pub(crate) type Test = Code<bool>;

/// A running frame: this call's registers, the closure's captured
/// values, and its `letfn` group (empty when none).
pub(crate) struct Frame<'f> {
    pub(crate) regs: &'f mut [Value],
    pub(crate) up: &'f [Value],
    pub(crate) rec: &'f [Value],
}

impl Frame<'_> {
    pub(crate) fn get(&self, loc: Loc) -> &Value {
        match loc {
            Loc::Reg(r) => &self.regs[r],
            Loc::Up(u) => &self.up[u],
            Loc::Rec(j) => &self.rec[j],
        }
    }
}

/// Where a lexical local lives at run time.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Loc {
    Reg(usize),
    Up(usize),
    Rec(usize),
}

/// A compiled fn: the closure's code plus the analyzed source (the
/// `Closure::arities` every closure over it shares).
pub(crate) struct CompiledFun {
    pub(crate) display: Arc<str>,
    pub(crate) source: Arc<[Arity]>,
    /// The lexical index of the first param (locals below it are
    /// captured or the fn's own `letfn` names).
    pub(crate) base: usize,
    arities: Vec<CompiledArity>,
}

impl std::fmt::Debug for CompiledFun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledFun")
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

/// One compiled arity: params arrive in registers `0..fixed` (the
/// rest seq at `fixed` when variadic); `bind` destructures them into
/// their names' registers before every body run (initial and after
/// each `recur`).
pub(crate) struct CompiledArity {
    fixed: usize,
    variadic: bool,
    nregs: usize,
    bind: Option<Exec>,
    body: Exec,
}

/// A VM closure's run-time half: the code, the captured values, and
/// the `letfn` group (set once every sibling exists).
#[derive(Clone)]
pub(crate) struct VmCode {
    pub(crate) fun: Arc<CompiledFun>,
    pub(crate) captured: Arc<[Value]>,
    pub(crate) rec: Option<Arc<OnceLock<Box<[Value]>>>>,
}

impl std::fmt::Debug for VmCode {
    /// The group is cyclic (a named fn is its own sibling), so only
    /// its size shows.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmCode")
            .field("fun", &self.fun)
            .field("captured", &self.captured)
            .field(
                "rec",
                &self.rec.as_ref().and_then(|g| g.get()).map(|g| g.len()),
            )
            .finish()
    }
}

/// A compiled top-level form and its register count.
pub(crate) struct TopCode {
    nregs: usize,
    exec: Exec,
}

/// Compile one analyzed top-level form.
pub(crate) fn compile_top(expr: &Expr, roots: &SharedRegistry) -> TopCode {
    let mut c = Compiler {
        scopes: vec![Scope::new(0, Vec::new())],
        roots: roots.clone(),
    };
    c.scopes[0].last = crate::vm_live::last_uses(std::slice::from_ref(expr), 0, 0);
    let exec = c.compile(expr);
    TopCode {
        nregs: c.scopes[0].nregs,
        exec,
    }
}

/// Run a compiled top-level form (no captured values, no group).
pub(crate) fn run_top(eval: &mut Evaluator, code: &TopCode) -> Result<Value, Signal> {
    let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
    regs.resize(code.nregs, Value::Nil);
    let mut frame = Frame {
        regs: &mut regs,
        up: &[],
        rec: &[],
    };
    (code.exec)(eval, &mut frame)
}

/// The arity a call with `argc` args runs (an exact fixed arity
/// first, else the variadic one), like the tree evaluator.
fn select(code: &VmCode, argc: usize) -> Option<&CompiledArity> {
    let mut variadic: Option<&CompiledArity> = None;
    for arity in &code.fun.arities {
        if !arity.variadic && arity.fixed == argc {
            return Some(arity);
        }
        if arity.variadic {
            variadic = Some(arity);
        }
    }
    variadic.filter(|a| argc >= a.fixed)
}

fn arity_error(eval: &mut Evaluator, closure: &Closure, argc: usize) -> Signal {
    eval.throw(crate::error::NativeError::arity(&closure.display, argc))
}

/// Apply a VM closure to borrowed args (natives calling back).
pub(crate) fn apply(
    eval: &mut Evaluator,
    closure: &Closure,
    code: &VmCode,
    args: &[Value],
) -> Result<Value, Signal> {
    let Some(arity) = select(code, args.len()) else {
        return Err(arity_error(eval, closure, args.len()));
    };
    let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
    regs.extend(args[..arity.fixed].iter().cloned());
    if arity.variadic {
        regs.push(crate::eval::rest_list(&args[arity.fixed..]));
    }
    run_arity(eval, code, arity, &mut regs)
}

/// Apply a VM closure to owned args, moving them into registers
/// (the VM's own calls; depth-guarded like `apply_closure`).
pub(crate) fn apply_moved(
    eval: &mut Evaluator,
    closure: &Closure,
    code: &VmCode,
    args: &mut SmallVec<[Value; 4]>,
) -> Result<Value, Signal> {
    let Some(arity) = select(code, args.len()) else {
        return Err(arity_error(eval, closure, args.len()));
    };
    let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
    reserve_regs(&mut regs, arity.nregs);
    if arity.variadic {
        let rest: Vec<Value> = args.drain(arity.fixed..).collect();
        move_into(&mut regs, args);
        regs.push(if rest.is_empty() {
            Value::Nil
        } else {
            Value::List(clj_value::List::from_vec(rest))
        });
    } else {
        move_into(&mut regs, args);
    }
    eval.guarded_call(|ev| run_arity(ev, code, arity, &mut regs))
}

/// Make room for `nregs` in a fresh register file. Callers build
/// the file in place (`SmallVec::new()` then this): returning one
/// from a helper — or `with_capacity` — moves the inline buffer by
/// value, a 528-byte copy per call.
#[inline(always)]
fn reserve_regs(regs: &mut SmallVec<[Value; 8]>, nregs: usize) {
    if nregs > regs.inline_size() {
        regs.reserve_exact(nregs);
    }
}

/// Move every arg into the next register (the buffer keeps `nil`s).
#[inline(always)]
fn move_into(regs: &mut SmallVec<[Value; 8]>, args: &mut SmallVec<[Value; 4]>) {
    for v in args.iter_mut() {
        regs.push(std::mem::replace(v, Value::Nil));
    }
}

/// Call a fixed arity over borrowed args (natives calling back:
/// each arg clones straight into its register).
pub(crate) fn call_borrowed(
    ev: &mut Evaluator,
    code: &VmCode,
    arity: &CompiledArity,
    args: &[Value],
) -> Result<Value, Signal> {
    let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
    reserve_regs(&mut regs, arity.nregs);
    for a in args {
        regs.push(clone_value(a));
    }
    ev.guarded_call(|ev| run_arity(ev, code, arity, &mut regs))
}

/// A callee resolved once, for natives that call the same fn per
/// item (`reduce`, `mapv`, …): a VM closure's fixed arity takes its
/// args straight into registers; a native runs its entry directly
/// (an `Int` pair through a tagged arith native computes inline).
pub(crate) enum Prepared<'c> {
    Vm {
        code: &'c VmCode,
        arity: &'c CompiledArity,
    },
    Native(&'c crate::ext::NativeFn),
}

impl<'c> Prepared<'c> {
    /// `callee` prepared for `argc` args: a VM closure without
    /// conveyed bindings with a fixed arity for the count, or a
    /// native (`None`: call generally).
    pub(crate) fn of(callee: &'c Value, argc: usize) -> Option<Self> {
        if let Some((code, arity)) = fixed_target(callee, argc) {
            return Some(Prepared::Vm { code, arity });
        }
        crate::ext::as_native(callee).map(Prepared::Native)
    }

    /// Call with one arg.
    pub(crate) fn call1(&self, ev: &mut Evaluator, a: Value) -> Result<Value, Signal> {
        match self {
            Prepared::Vm { code, arity } => {
                let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
                reserve_regs(&mut regs, arity.nregs);
                regs.push(a);
                ev.guarded_call(|ev| run_arity(ev, code, arity, &mut regs))
            }
            Prepared::Native(n) => call_native(ev, n, smallvec::smallvec![a]),
        }
    }

    /// Call with two args.
    pub(crate) fn call2(&self, ev: &mut Evaluator, a: Value, b: Value) -> Result<Value, Signal> {
        match self {
            Prepared::Vm { code, arity } => {
                let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
                reserve_regs(&mut regs, arity.nregs);
                regs.push(a);
                regs.push(b);
                ev.guarded_call(|ev| run_arity(ev, code, arity, &mut regs))
            }
            Prepared::Native(n) => {
                if let (Value::Int(x), Value::Int(y)) = (&a, &b) {
                    if let Some(v) = Evaluator::fast_int2(n.fast, *x, *y) {
                        return Ok(v);
                    }
                }
                call_native(ev, n, smallvec::smallvec![a, b])
            }
        }
    }
}

/// A native over owned args: the owned entry takes the buffer, the
/// borrow entry borrows it (like `invoke_args`; the native call
/// span is the caller's already).
fn call_native(
    ev: &mut Evaluator,
    n: &crate::ext::NativeFn,
    args: SmallVec<[Value; 4]>,
) -> Result<Value, Signal> {
    match &n.owned {
        Some(owned) => owned(ev, args).map_err(|e| ev.throw(e)),
        None => (n.func)(ev, &args).map_err(|e| ev.throw(e)),
    }
}

/// Run an arity over its loaded params (`regs` grows to the
/// arity's register count), looping on `recur`.
fn run_arity(
    eval: &mut Evaluator,
    code: &VmCode,
    arity: &CompiledArity,
    regs: &mut SmallVec<[Value; 8]>,
) -> Result<Value, Signal> {
    // A push loop, not `resize` (an out-of-line call per run).
    while regs.len() < arity.nregs {
        regs.push(Value::Nil);
    }
    let rec = code
        .rec
        .as_ref()
        .and_then(|g| g.get())
        .map_or(&[][..], |g| &g[..]);
    let mut frame = Frame {
        regs,
        up: &code.captured,
        rec,
    };
    let params = arity.fixed + usize::from(arity.variadic);
    loop {
        if let Some(bind) = &arity.bind {
            bind(eval, &mut frame)?;
        }
        match (arity.body)(eval, &mut frame) {
            // The VM's own `recur` wrote the params already (empty
            // values); a tree-evaluated one carries them.
            Err(Signal::Recur(values)) if values.is_empty() => {}
            Err(Signal::Recur(values)) => {
                (0..params).zip(values).for_each(|(r, v)| frame.regs[r] = v);
            }
            other => return other,
        }
    }
}

/// A VM closure's fixed arity for `argc` args (the one [`select`]
/// picks first), when the callee is a VM closure without conveyed
/// bindings.
#[inline]
pub(crate) fn fixed_target(callee: &Value, argc: usize) -> Option<(&VmCode, &CompiledArity)> {
    let closure = crate::ext::as_closure(callee)?;
    let code = closure.vm.as_ref().filter(|_| closure.conveyed.is_none())?;
    let arity = code
        .fun
        .arities
        .iter()
        .find(|a| !a.variadic && a.fixed == argc)?;
    Some((code, arity))
}

/// A destructuring pattern the VM binds inline on its common value
/// shape (#226: the general binder cost ~3K instructions a bind),
/// leaving every other shape to the general binder.
enum Unpack {
    /// Lookups (`{a :a}`, `:keys`/`:strs`/`:syms`, then `:as`) on an
    /// unsorted map: the `get` of each scalar key, in bind order.
    Map { keys: Vec<Value>, whole: bool },
    /// Positions (`[a b]`, then `:as`) on a vector or a map entry:
    /// `nth` with a `nil` default.
    Vec { n: usize, whole: bool },
}

/// The inline binding for `pat`, when it has one: only symbol locals,
/// no `:or`/required keys/1.13 outlets/`&`, scalar lookup keys.
fn inline_unpack(pat: &Pattern) -> Option<Unpack> {
    match pat {
        Pattern::Map(m) => inline_map(m).map(|keys| Unpack::Map {
            keys,
            whole: m.as_sym.is_some(),
        }),
        Pattern::Vec(v)
            if v.rest.is_none() && v.fixed.iter().all(|p| matches!(p, Pattern::Symbol(_))) =>
        {
            Some(Unpack::Vec {
                n: v.fixed.len(),
                whole: v.as_sym.is_some(),
            })
        }
        _ => None,
    }
}

/// A plain map pattern's lookup keys in bind order (`None` for
/// anything the inline binding does not cover).
fn inline_map(m: &crate::expr::MapPattern) -> Option<Vec<Value>> {
    let outlets = [&m.select, &m.all, &m.excess, &m.missing];
    if !m.or.is_empty() || !m.extra_keys.is_empty() || outlets.iter().any(|o| o.is_some()) {
        return None;
    }
    let entries = m
        .entries
        .iter()
        .map(|(local, key, _, req)| (matches!(local, Pattern::Symbol(_)) && !req).then_some(key));
    let keys = m.keys.iter().map(|k| (!k.required).then_some(&k.key));
    let keys: Option<Vec<Value>> = entries.chain(keys).map(|k| k.cloned()).collect();
    keys.filter(|ks| ks.iter().all(crate::vm_intrinsics::is_scalar_key))
}

/// Bind `v` through `u` into `regs` (`false`, binding nothing, when
/// `v`'s shape needs the general binder).
fn unpack_inline(u: &Unpack, v: &Value, regs: &[usize], f: &mut Frame<'_>) -> bool {
    let n = match (u, v) {
        (Unpack::Map { keys, .. }, Value::Map(m)) if !m.is_sorted() => {
            for (r, k) in regs.iter().zip(keys) {
                f.regs[*r] = m.get(k).cloned().unwrap_or(Value::Nil);
            }
            keys.len()
        }
        (Unpack::Vec { n, .. }, Value::Vector(x)) => {
            for (i, r) in regs.iter().take(*n).enumerate() {
                f.regs[*r] = x.get(i).unwrap_or(Value::Nil);
            }
            *n
        }
        (Unpack::Vec { n, .. }, Value::MapEntry { key, val, .. }) => {
            let items = [&**key, &**val];
            for (i, r) in regs.iter().take(*n).enumerate() {
                f.regs[*r] = items.get(i).map_or(Value::Nil, |x| (*x).clone());
            }
            *n
        }
        _ => return false,
    };
    if let (Unpack::Map { whole: true, .. } | Unpack::Vec { whole: true, .. }, Some(r)) =
        (u, regs.get(n))
    {
        f.regs[*r] = v.clone();
    }
    true
}

/// Call a fixed arity: each arg evaluates straight into the
/// callee's next register (depth-guarded like `apply_moved`).
fn call_fixed(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    code: &VmCode,
    arity: &CompiledArity,
    args: &[Operand],
) -> Result<Value, Signal> {
    let mut regs: SmallVec<[Value; 8]> = SmallVec::new();
    reserve_regs(&mut regs, arity.nregs);
    for a in args {
        regs.push(a.eval(ev, f)?);
    }
    ev.guarded_call(|ev| run_arity(ev, code, arity, &mut regs))
}

/// Invoke an evaluated callee with owned args: VM closures (without
/// conveyed bindings) take them by move, everything else goes the
/// general road.
fn call_value(
    ev: &mut Evaluator,
    callee: &Value,
    args: &mut SmallVec<[Value; 4]>,
    span: Span,
) -> Result<Value, Signal> {
    if let Some(closure) = crate::ext::as_closure(callee) {
        if let (Some(code), None) = (&closure.vm, &closure.conveyed) {
            return apply_moved(ev, closure, code, args);
        }
    }
    if let Some(v) = invoke_lookup(callee, args) {
        return Ok(v);
    }
    ev.invoke_args(callee, std::mem::take(args), span)
}

/// `(m k)`, `(m k nf)`, `(:k m)`, `(:k m nf)`, `(v i)`, `(s k)`, and
/// `(s k nf)` inline (`None` leaves whatever the inline lookup does
/// not cover — other shapes, arities and their errors — to the
/// general invoke).
pub(crate) fn invoke_lookup(callee: &Value, args: &[Value]) -> Option<Value> {
    use crate::vm_intrinsics::get_of;
    match (callee, args) {
        (Value::Map(_) | Value::Set(_), [k]) => get_of(callee, k, None),
        (Value::Map(_) | Value::Set(_), [k, nf]) => get_of(callee, k, Some(nf)),
        (Value::Keyword(_), [m]) => get_of(m, callee, None),
        (Value::Keyword(_), [m, nf]) => get_of(m, callee, Some(nf)),
        (Value::Vector(v), [Value::Int(n)]) => usize::try_from(*n).ok().and_then(|i| v.get(i)),
        _ => None,
    }
}

/// A fresh VM closure value over `fun`.
fn make_closure(
    eval: &mut Evaluator,
    fun: &Arc<CompiledFun>,
    captured: Arc<[Value]>,
    rec: Option<Arc<OnceLock<Box<[Value]>>>>,
) -> Value {
    closure_value(eval.next_id(), fun, captured, rec)
}

/// A VM closure value with identity `id` (`lazy-seq` bodies take 0:
/// they never reach user code, so they skip the shared counter).
fn closure_value(
    id: u64,
    fun: &Arc<CompiledFun>,
    captured: Arc<[Value]>,
    rec: Option<Arc<OnceLock<Box<[Value]>>>>,
) -> Value {
    Value::Extension(Arc::new(Closure {
        id,
        display: Arc::clone(&fun.display),
        arities: Arc::clone(&fun.source),
        env: Env::new(),
        meta: None,
        conveyed: None,
        vm: Some(VmCode {
            fun: Arc::clone(fun),
            captured,
            rec,
        }),
    }))
}

/// The capture list's current values.
fn capture(frame: &Frame<'_>, locs: &[Loc]) -> Arc<[Value]> {
    locs.iter().map(|l| frame.get(*l).clone()).collect()
}

/// One fn's compile scope. Lexical indexes below `base` are outer
/// locals (the `letfn` group's names, else captures); `locals` maps
/// the rest (minus `base`) to registers.
struct Scope {
    base: usize,
    locals: Vec<usize>,
    next: usize,
    nregs: usize,
    captures: Vec<(usize, Loc)>,
    rec: Vec<usize>,
    /// The innermost `loop`/arity's raw registers (`recur` targets).
    recur: Arc<[usize]>,
    /// The body's last-use reads (see `vm_live`): those move their
    /// register's value out instead of cloning it.
    last: crate::vm_live::LastUses,
}

impl Scope {
    fn new(base: usize, rec: Vec<usize>) -> Self {
        Self {
            base,
            locals: Vec::new(),
            next: 0,
            nregs: 0,
            captures: Vec::new(),
            rec,
            recur: Arc::from([]),
            last: crate::vm_live::LastUses::new(),
        }
    }
}

/// A scope position to restore after a binding form.
#[derive(Clone, Copy)]
struct Mark {
    locals: usize,
    next: usize,
}

struct Compiler {
    scopes: Vec<Scope>,
    /// The var roots as of compilation (what `vm_scalar` may inline).
    roots: SharedRegistry,
}

impl crate::vm_scalar::Host for Compiler {
    fn resolve_local(&mut self, idx: usize) -> Loc {
        self.resolve(idx)
    }

    fn reads(&self, expr: &Expr, g: usize) -> bool {
        let base = self.scopes.last().expect("a scope is always open").base;
        crate::vm_live::may_read(expr, base, g + 1, g)
    }

    fn site(&self, call: &DirectCall) -> crate::vm_ops::Site {
        crate::vm_ops::Site::new(site_of(call))
    }

    fn callee(&self, var: &Var, argc: usize) -> Option<crate::vm_scalar::Callee> {
        inline_callee(&self.roots.root_or_unbound(var.id, &var.sym), argc)
    }
}

/// The arity a call with `argc` args runs on `root` when its body
/// may inline (see [`crate::vm_scalar::Callee`]): a VM closure
/// without conveyed bindings whose fixed arity for the count binds
/// symbols only — the arity [`fixed_target`] would run.
fn inline_callee(root: &Value, argc: usize) -> Option<crate::vm_scalar::Callee> {
    let Value::Extension(expect) = root else {
        return None;
    };
    let closure = crate::ext::as_closure(root)?;
    let code = closure.vm.as_ref().filter(|_| closure.conveyed.is_none())?;
    let fun = &code.fun;
    let index = fun
        .source
        .iter()
        .position(|a| a.rest.is_none() && a.fixed.len() == argc)?;
    fun.source[index]
        .fixed
        .iter()
        .all(|p| matches!(p, Pattern::Symbol(_)))
        .then(|| crate::vm_scalar::Callee {
            base: fun.base,
            arities: Arc::clone(&fun.source),
            index,
            expect: Arc::clone(expect),
        })
}

impl Compiler {
    fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("a scope is always open")
    }

    /// The lexical locals visible here (outer ones included).
    fn visible(&self) -> usize {
        let s = self.scopes.last().expect("a scope is always open");
        s.base + s.locals.len()
    }

    fn alloc(&mut self) -> usize {
        let s = self.scope();
        let r = s.next;
        s.next += 1;
        s.nregs = s.nregs.max(s.next);
        r
    }

    /// Allocate a register for the next lexical local.
    fn push_local(&mut self) -> usize {
        let r = self.alloc();
        self.scope().locals.push(r);
        r
    }

    fn mark(&mut self) -> Mark {
        let s = self.scope();
        Mark {
            locals: s.locals.len(),
            next: s.next,
        }
    }

    fn reset(&mut self, mark: Mark) {
        let s = self.scope();
        s.locals.truncate(mark.locals);
        s.next = mark.next;
    }

    /// Whether `expr` (a local read) is its register's last use.
    fn is_last(&self, expr: &Expr) -> bool {
        let s = self.scopes.last().expect("a scope is always open");
        s.last.contains(&(expr as *const Expr))
    }

    fn resolve(&mut self, old: usize) -> Loc {
        self.resolve_at(self.scopes.len() - 1, old)
    }

    /// A lexical index's location in scope `depth`, capturing it
    /// through every fn between its home and here.
    fn resolve_at(&mut self, depth: usize, old: usize) -> Loc {
        let s = &self.scopes[depth];
        if old >= s.base {
            return Loc::Reg(s.locals[old - s.base]);
        }
        if let Some(j) = s.rec.iter().position(|i| *i == old) {
            return Loc::Rec(j);
        }
        if let Some(u) = s.captures.iter().position(|(i, _)| *i == old) {
            return Loc::Up(u);
        }
        let parent = self.resolve_at(depth - 1, old);
        let s = &mut self.scopes[depth];
        s.captures.push((old, parent));
        Loc::Up(s.captures.len() - 1)
    }

    fn compile(&mut self, expr: &Expr) -> Exec {
        match expr {
            Expr::Literal(v) | Expr::Quote(v) => konst(v.clone()),
            Expr::VarRef(var) => konst(Value::Var(var.clone())),
            Expr::Local(idx) => match (self.resolve(*idx), self.is_last(expr)) {
                (Loc::Reg(r), true) => take_local(r),
                (loc, _) => local(loc),
            },
            Expr::Global(var) => global(var.clone()),
            Expr::If { test, then, els } => self.compile_if(test, then, els),
            Expr::Do(body) => self.compile_body(body),
            Expr::Invoke { head, args, span } => self.compile_invoke(head, args, *span),
            Expr::DirectNative(call) => self.compile_direct(call),
            _ => self.compile_binding_form(expr),
        }
    }

    /// The binding and closure forms (split from [`compile`] for the
    /// complexity budget).
    fn compile_binding_form(&mut self, expr: &Expr) -> Exec {
        match expr {
            Expr::Let { bindings, body } => self.compile_let(bindings, body),
            Expr::Loop { bindings, body } => self.compile_loop(bindings, body),
            Expr::Recur(args) => self.compile_recur(args),
            Expr::Fn(fun) => self.compile_fn(fun),
            Expr::LetFn { fns, body } => self.compile_letfn(fns, body),
            Expr::LazySeqStar { body } => self.compile_lazy_seq(body),
            Expr::DelayStar { display, body } => self.compile_delay(display, body),
            Expr::Try {
                body,
                catches,
                finally_body,
            } => self.compile_try(body, catches, finally_body),
            _ => self.compile_data_form(expr),
        }
    }

    /// Literals, `throw`, `def` (split from [`compile`] for the
    /// complexity budget).
    fn compile_data_form(&mut self, expr: &Expr) -> Exec {
        match expr {
            Expr::VecLit(elems, meta) => self.compile_vec(elems, meta.clone()),
            Expr::MapLit(pairs, meta) => self.compile_map(pairs, meta.clone()),
            Expr::SetLit(elems, meta) => self.compile_set(elems, meta.clone()),
            Expr::Throw(inner) => {
                let inner = self.compile(inner);
                Box::new(move |ev, f| {
                    let v = inner(ev, f)?;
                    Err(ev.throw_value(v))
                })
            }
            Expr::Def {
                sym,
                doc,
                init,
                meta,
                span,
            } => self.compile_def(sym, doc.clone(), init.as_deref(), meta.as_deref(), *span),
            Expr::CaseStar {
                expr,
                shift,
                mask,
                default,
                cases,
                skip,
                test,
            } => {
                let shape = CaseShape {
                    shift: *shift,
                    mask: *mask,
                    skip: skip.clone(),
                    test: *test,
                };
                self.compile_case(expr, default, cases, shape)
            }
            Expr::Assert {
                test,
                message,
                form,
            } => self.compile_assert(test, message.as_deref(), form),
            Expr::NewThrowable { class, message } => {
                let (class, message) = (*class, Arc::<str>::from(message.as_str()));
                Box::new(move |ev, _| Ok(new_throwable(ev, class, &message)))
            }
            _ => self.escape(expr),
        }
    }

    /// Run `expr` on the tree evaluator over a fresh frame holding
    /// every visible local at its lexical index.
    fn escape(&mut self, expr: &Expr) -> Exec {
        let locs = self.visible_locs();
        let expr = expr.clone();
        Box::new(move |ev, f| {
            let env = frame_env(f, &locs);
            ev.eval(&expr, &env)
        })
    }

    /// Every visible local's location, in lexical order (what a
    /// tree-evaluator frame at this point holds).
    fn visible_locs(&mut self) -> Vec<Loc> {
        (0..self.visible()).map(|i| self.resolve(i)).collect()
    }

    fn compile_all(&mut self, exprs: &[Expr]) -> Vec<Exec> {
        exprs.iter().map(|e| self.compile(e)).collect()
    }

    /// A form whose value is discarded: identical to
    /// [`compile`](Self::compile) except that a transient
    /// mutator compiles to its effect code (no transient clone
    /// built just to drop).
    fn compile_effect(&mut self, expr: &Expr) -> Exec {
        if let Expr::DirectNative(call) = expr {
            if let Some(code) = self.try_assoc_conj(call, true) {
                return code;
            }
            if let Some(ctor) = crate::vm_ops::effect_ctor(call) {
                let ops = self.in_place_operands(call);
                return ctor.code(site_of(call), ops);
            }
        }
        self.compile(expr)
    }

    /// The fused transient update `(assoc! t k (conj (nth t k) x))`
    /// (see [`crate::vm_ops::assoc_conj_code`]): `t` one local in
    /// both places, `k` the same local or `Int` literal in both, and
    /// `x` a local, literal, or metadata-free vector literal of
    /// those. The call's ordinary code compiles first (the same
    /// registers and last-use moves) and runs on every miss.
    fn try_assoc_conj(&mut self, call: &DirectCall, effect: bool) -> Option<Exec> {
        let (t, k, x, conj, nth) = assoc_conj_shape(call)?;
        let generic = match crate::vm_ops::effect_ctor(call) {
            Some(ctor) if effect => {
                let ops = self.in_place_operands(call);
                ctor.code(site_of(call), ops)
            }
            _ => self.compile_direct_plain(call),
        };
        let srcs = (self.in_place(t), self.in_place(k), self.pure_src(x));
        let (Some(t), Some(k), Some(x)) = srcs else {
            return Some(generic);
        };
        let sites = [call, conj, nth].map(|c| crate::vm_ops::Site::new(site_of(c)));
        Some(crate::vm_ops::assoc_conj_code(
            sites, t, k, x, effect, generic,
        ))
    }

    /// A local (not a `letfn` member) or literal, read in place.
    fn in_place(&mut self, e: &Expr) -> Option<crate::vm_ops::InPlace> {
        use crate::vm_ops::InPlace;
        match e {
            Expr::Literal(v) => Some(InPlace::Kst(v.clone())),
            Expr::Local(idx) => match self.resolve(*idx) {
                Loc::Reg(r) => Some(InPlace::Reg(r)),
                Loc::Up(u) => Some(InPlace::Up(u)),
                Loc::Rec(_) => None,
            },
            _ => None,
        }
    }

    /// The fused update's appended operand (see [`Self::try_assoc_conj`]).
    fn pure_src(&mut self, e: &Expr) -> Option<crate::vm_ops::PureSrc> {
        use crate::vm_ops::PureSrc;
        match e {
            Expr::VecLit(items, None) if !items.is_empty() => items
                .iter()
                .map(|i| self.in_place(i))
                .collect::<Option<Vec<_>>>()
                .map(|v| PureSrc::Vec(v.into())),
            other => self.in_place(other).map(PureSrc::One),
        }
    }

    fn compile_body(&mut self, body: &[Expr]) -> Exec {
        let mut execs: Vec<Exec> = match body.split_last() {
            None => Vec::new(),
            Some((last, rest)) => {
                let mut execs: Vec<Exec> = rest.iter().map(|e| self.compile_effect(e)).collect();
                execs.push(self.compile(last));
                execs
            }
        };
        match execs.len() {
            0 => konst(Value::Nil),
            1 => execs.pop().expect("one form"),
            _ => {
                let last = execs.pop().expect("a last form");
                Box::new(move |ev, f| {
                    for e in &execs {
                        e(ev, f)?;
                    }
                    last(ev, f)
                })
            }
        }
    }

    fn compile_if(&mut self, test: &Expr, then: &Expr, els: &Expr) -> Exec {
        let test = self.compile_test(test);
        let then = self.compile(then);
        let els = self.compile(els);
        Box::new(move |ev, f| {
            if test(ev, f)? {
                then(ev, f)
            } else {
                els(ev, f)
            }
        })
    }

    /// A branch test: comparisons and predicates compute the truth
    /// value directly.
    fn compile_test(&mut self, expr: &Expr) -> Test {
        if let Some(test) = self.short_circuit_test(expr) {
            return test;
        }
        if let Expr::DirectNative(call) = expr {
            if let Some(ctor) = crate::vm_ops::test_ctor(call) {
                let ops = self.in_place_operands(call);
                return ctor.code(site_of(call), ops);
            }
        }
        let e = self.compile(expr);
        Box::new(move |ev, f| Ok(e(ev, f)?.is_truthy()))
    }

    /// An `and`/`or` step in test position (#226): `(let* [g a] (if
    /// g b g))` is `a`'s truth and then `b`'s, `(let* [g a] (if g g
    /// b))` `a`'s or else `b`'s — when `b` never reads `g`, `g`'s
    /// value is never needed, so neither step builds one.
    fn short_circuit_test(&mut self, expr: &Expr) -> Option<Test> {
        let g = self.visible();
        let (a, b, and) = and_or_shape(expr, g)?;
        let base = self.scopes.last().expect("a scope is always open").base;
        if crate::vm_live::may_read(b, base, g + 1, g) {
            return None;
        }
        let ta = self.compile_test(a);
        let mark = self.mark();
        // `g`'s lexical slot (its register is never written: no read).
        self.push_local();
        let tb = self.compile_test(b);
        self.reset(mark);
        Some(if and {
            Box::new(move |ev, f| Ok(ta(ev, f)? && tb(ev, f)?))
        } else {
            Box::new(move |ev, f| Ok(ta(ev, f)? || tb(ev, f)?))
        })
    }

    /// A specialized or intrinsic call's operands: locals read in
    /// place (never moved out).
    fn in_place_operands(&mut self, call: &DirectCall) -> Vec<Operand> {
        call.args.iter().map(|a| self.operand(a, false)).collect()
    }

    /// `let`: a symbol's init writes its register directly (no
    /// step closure); destructuring binds through its step.
    fn compile_let(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> Exec {
        let mark = self.mark();
        let mut steps: Vec<LetStep> = Vec::with_capacity(bindings.len());
        for (pat, init) in bindings {
            let init = self.compile(init);
            steps.push(if matches!(pat, Pattern::Symbol(_)) {
                LetStep::Name(self.push_local(), init)
            } else {
                LetStep::Unpack(self.bind_step(pat, init))
            });
        }
        let body = self.compile_body(body);
        self.reset(mark);
        Box::new(move |ev, f| {
            for step in &steps {
                match step {
                    LetStep::Name(r, init) => {
                        let v = init(ev, f)?;
                        set_reg(&mut f.regs[*r], v);
                    }
                    LetStep::Unpack(step) => {
                        step(ev, f)?;
                    }
                }
            }
            body(ev, f)
        })
    }

    /// Bind one evaluated init to a destructuring pattern's registers
    /// (allocated in [`crate::expr::pattern_names`] order, like the
    /// analyzer's locals).
    fn bind_step(&mut self, pat: &Pattern, init: Exec) -> Exec {
        let raw = self.alloc();
        let step = self.destructure_step(pat, raw);
        Box::new(move |ev, f| {
            let v = init(ev, f)?;
            f.regs[raw] = v;
            step(ev, f)
        })
    }

    /// Destructure the value in register `raw` into the pattern's
    /// names (registers allocated here, in [`crate::expr::pattern_names`] order).
    /// `:or` defaults evaluate on the tree evaluator against the
    /// frame visible before the names, like `bind_pattern`.
    fn destructure_step(&mut self, pat: &Pattern, raw: usize) -> Exec {
        let env_locs = crate::vm_live::has_defaults(pat).then(|| self.visible_locs());
        let regs: Vec<usize> = (0..pattern_count(pat)).map(|_| self.push_local()).collect();
        let inline = inline_unpack(pat);
        let pat = pat.clone();
        Box::new(move |ev, f| {
            let v = f.regs[raw].clone();
            if inline
                .as_ref()
                .is_some_and(|u| unpack_inline(u, &v, &regs, f))
            {
                return Ok(Value::Nil);
            }
            let env = env_locs
                .as_ref()
                .map_or_else(Env::new, |locs| frame_env(f, locs));
            let mut out = Vec::with_capacity(regs.len());
            ev.destructure_in(&pat, &v, &env, &mut out)?;
            for (r, v) in regs.iter().zip(out) {
                f.regs[*r] = v;
            }
            Ok(Value::Nil)
        })
    }

    /// `loop`: each binding's raw value takes a register (a symbol's
    /// is its own); destructured bindings unpack after every write,
    /// initial and `recur`, in order.
    fn compile_loop(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> Exec {
        let mark = self.mark();
        let mut steps: Vec<LoopStep> = Vec::with_capacity(bindings.len());
        for (pat, init) in bindings {
            let init = self.compile(init);
            steps.push(if matches!(pat, Pattern::Symbol(_)) {
                (self.push_local(), init, None)
            } else {
                let raw = self.alloc();
                (raw, init, Some(self.destructure_step(pat, raw)))
            });
        }
        let targets: Arc<[usize]> = steps.iter().map(|(r, ..)| *r).collect();
        let unpacks = steps.iter().any(|(_, _, u)| u.is_some());
        let saved = std::mem::replace(&mut self.scope().recur, Arc::clone(&targets));
        let fused = self.try_fused_loop(body, &targets, unpacks);
        let flat = if fused.is_none() && !unpacks {
            Some(self.compile_flat_loop(body, targets.len()))
        } else {
            None
        };
        let int_fused = fused.as_ref().is_some_and(|f| f.int.is_some());
        let scalar = if unpacks || int_fused {
            None
        } else {
            let base = self.visible();
            crate::vm_scalar::compile(self, body, &targets, base)
        };
        let body = if fused.is_some() || flat.is_some() {
            konst(Value::Nil)
        } else {
            self.compile_body(body)
        };
        self.scope().recur = saved;
        self.reset(mark);
        let code = LoopCode {
            steps,
            targets,
            body,
            unpacks,
            fused,
            flat,
            scalar,
        };
        Box::new(move |ev, f| run_loop(ev, f, &code))
    }

    /// Detect the fused loop shape: a single-form body that is an
    /// `if` with a bare terminal `recur` in exactly one branch.
    /// Parts compile in `compile_if` order (test, then, els) so
    /// register allocation and last-use moves match the generic
    /// path exactly.
    fn try_fused_loop(
        &mut self,
        body: &[Expr],
        targets: &[usize],
        unpacks: bool,
    ) -> Option<FusedLoop> {
        let arity = targets.len();
        let [Expr::If { test, then, els }] = body else {
            return None;
        };
        let (recur_args, recur_when_true) =
            match (terminal_recur(then, arity), terminal_recur(els, arity)) {
                (Some(args), None) => (args, true),
                (None, Some(args)) => (args, false),
                _ => return None,
            };
        let test_code = self.compile_test(test);
        // Compile in source order: then-branch before els-branch.
        let (then_ops, exit) = if recur_when_true {
            let ops: Vec<Operand> = recur_args.iter().map(|a| self.operand(a, true)).collect();
            (ops, self.compile(els))
        } else {
            let exit = self.compile(then);
            let ops: Vec<Operand> = recur_args.iter().map(|a| self.operand(a, true)).collect();
            (ops, exit)
        };
        // Classified last (pure inspection over the same AST;
        // `resolve` is idempotent once the parts compiled).
        let int = if unpacks {
            None
        } else {
            self.try_int_fused(test, recur_args, targets, &then_ops)
        };
        Some(FusedLoop {
            test: test_code,
            exit,
            ops: then_ops,
            recur_when_true,
            int,
        })
    }

    /// An unboxed source for `e`: an `Int` literal, or a local
    /// (loop variables by state position; other registers are
    /// loop-invariant, captures immutable).
    fn int_src(&mut self, e: &Expr, targets: &[usize]) -> Option<IntSrc> {
        match e {
            Expr::Literal(Value::Int(n)) => Some(IntSrc::Kst(*n)),
            Expr::Local(idx) => match self.resolve(*idx) {
                Loc::Reg(r) => Some(match targets.iter().position(|t| *t == r) {
                    Some(p) => IntSrc::State(p),
                    None => IntSrc::Reg(r),
                }),
                Loc::Up(u) => Some(IntSrc::Up(u)),
                Loc::Rec(_) => None,
            },
            _ => None,
        }
    }

    /// An unboxed operand for a `recur` argument (its site is
    /// recorded only when the whole call classifies).
    fn int_operand(
        &mut self,
        e: &Expr,
        targets: &[usize],
        sites: &mut Vec<crate::vm_ops::Site>,
    ) -> Option<IntOperand> {
        if let Expr::DirectNative(call) = e {
            if let Some(ctor) = crate::vm_ops::int_value_op(call) {
                let op = match ctor {
                    crate::vm_ops::IntCtor::One(op) => {
                        IntOperand::One(op, self.int_src(&call.args[0], targets)?)
                    }
                    crate::vm_ops::IntCtor::Two(op) => IntOperand::Two(
                        op,
                        self.int_src(&call.args[0], targets)?,
                        self.int_src(&call.args[1], targets)?,
                    ),
                };
                sites.push(crate::vm_ops::Site::new(site_of(call)));
                return Some(op);
            }
        }
        self.int_src(e, targets).map(IntOperand::Src)
    }

    /// An unboxed test for the loop's branch (see
    /// [`int_operand`](Self::int_operand)).
    fn int_test(
        &mut self,
        e: &Expr,
        targets: &[usize],
        sites: &mut Vec<crate::vm_ops::Site>,
    ) -> Option<IntTest> {
        let Expr::DirectNative(call) = e else {
            return None;
        };
        let test = match crate::vm_ops::int_test_op(call)? {
            crate::vm_ops::IntTestCtor::One(op) => {
                IntTest::One(op, self.int_src(&call.args[0], targets)?)
            }
            crate::vm_ops::IntTestCtor::Two(op) => IntTest::Two(
                op,
                self.int_src(&call.args[0], targets)?,
                self.int_src(&call.args[1], targets)?,
            ),
        };
        sites.push(crate::vm_ops::Site::new(site_of(call)));
        Some(test)
    }

    /// The unboxed program for a fused loop, when the test and
    /// every `recur` operand mirror on `i64` and no compiled
    /// operand moves a non-target register out (a last-use take
    /// the unboxed stretch would not perform).
    fn try_int_fused(
        &mut self,
        test: &Expr,
        recur_args: &[Expr],
        targets: &[usize],
        ops: &[Operand],
    ) -> Option<IntFused> {
        // Defensive: liveness never marks a loop-carried read as
        // last-use (the boxed road would break too), but a
        // non-target take must not slip past the stretch.
        let clear = !ops.iter().any(|op| match op {
            Operand::Loc(Loc::Reg(r), true) => !targets.contains(r),
            _ => false,
        });
        clear
            .then(|| self.build_int_fused(test, recur_args, targets))
            .flatten()
    }

    fn build_int_fused(
        &mut self,
        test: &Expr,
        recur_args: &[Expr],
        targets: &[usize],
    ) -> Option<IntFused> {
        let mut sites = Vec::new();
        let test = self.int_test(test, targets, &mut sites)?;
        let mut int_ops = Vec::with_capacity(recur_args.len());
        for a in recur_args {
            int_ops.push(self.int_operand(a, targets, &mut sites)?);
        }
        Some(IntFused {
            test,
            ops: int_ops.into_boxed_slice(),
            sites: sites.into_boxed_slice(),
        })
    }

    /// Compile a loop body to flat steps (see [`FlatLoop`]).
    /// Leaf forms compile through the ordinary paths, in exactly
    /// the order `compile_body`/`compile_let`/`compile_if` compile
    /// them, so register allocation and last-use moves match the
    /// generic loop exactly.
    fn compile_flat_loop(&mut self, body: &[Expr], arity: usize) -> FlatLoop {
        let mut steps = Vec::new();
        self.flat_body_tail(&mut steps, body, arity);
        FlatLoop {
            steps: steps.into_boxed_slice(),
        }
    }

    /// A body in tail position: statements, then the tail form.
    fn flat_body_tail(&mut self, steps: &mut Vec<Step>, body: &[Expr], arity: usize) {
        match body.split_last() {
            None => steps.push(Step::Exit(konst(Value::Nil))),
            Some((last, rest)) => {
                for e in rest {
                    self.flat_stmt(steps, e, arity);
                }
                self.flat_tail(steps, last, arity);
            }
        }
    }

    /// One form in tail position: every path ends in `Recur` or
    /// `Exit`, so an `if` needs no joining jump.
    fn flat_tail(&mut self, steps: &mut Vec<Step>, e: &Expr, arity: usize) {
        match e {
            Expr::Recur(args) if args.len() == arity => {
                let ops: Vec<Operand> = args.iter().map(|a| self.operand(a, true)).collect();
                steps.push(Step::Recur(ops.into_boxed_slice()));
            }
            Expr::If { test, then, els } => {
                let test = self.compile_test(test);
                let jump = steps.len();
                steps.push(Step::JumpUnless(test, usize::MAX));
                self.flat_tail(steps, then, arity);
                patch_jump(steps, jump);
                self.flat_tail(steps, els, arity);
            }
            Expr::Let { bindings, body } => self.flat_let(steps, bindings, body, arity, true),
            Expr::Do(body) => self.flat_body_tail(steps, body, arity),
            _ => steps.push(Step::Exit(self.compile(e))),
        }
    }

    /// One form in statement position (its value is discarded).
    fn flat_stmt(&mut self, steps: &mut Vec<Step>, e: &Expr, arity: usize) {
        match e {
            Expr::If { test, then, els } => self.flat_if_stmt(steps, test, then, els, arity),
            Expr::Let { bindings, body } => self.flat_let(steps, bindings, body, arity, false),
            Expr::Do(body) => {
                for e in body {
                    self.flat_stmt(steps, e, arity);
                }
            }
            _ => steps.push(Step::Eval(self.compile_effect(e))),
        }
    }

    /// A statement `if`: a literal-nil branch needs no step (the
    /// `when`/`when-not` shapes), anything else joins after.
    /// Branches compile in source order, like `compile_if`.
    fn flat_if_stmt(
        &mut self,
        steps: &mut Vec<Step>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        arity: usize,
    ) {
        let test = self.compile_test(test);
        let jump = steps.len();
        steps.push(Step::JumpUnless(test, usize::MAX));
        if is_nil_literal(then) {
            let end = steps.len();
            steps.push(Step::Jump(usize::MAX));
            patch_jump(steps, jump);
            self.flat_stmt(steps, els, arity);
            patch_jump(steps, end);
            return;
        }
        self.flat_stmt(steps, then, arity);
        if is_nil_literal(els) {
            patch_jump(steps, jump);
            return;
        }
        let end = steps.len();
        steps.push(Step::Jump(usize::MAX));
        patch_jump(steps, jump);
        self.flat_stmt(steps, els, arity);
        patch_jump(steps, end);
    }

    /// A `let` as flat steps: bindings in order (each init
    /// compiles before its register is pushed, like
    /// `compile_let`), then the body in tail or statement flow.
    fn flat_let(
        &mut self,
        steps: &mut Vec<Step>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        arity: usize,
        tail: bool,
    ) {
        let mark = self.mark();
        for (pat, init) in bindings {
            let init = self.compile(init);
            steps.push(if matches!(pat, Pattern::Symbol(_)) {
                Step::Set(self.push_local(), init)
            } else {
                Step::Unpack(self.bind_step(pat, init))
            });
        }
        if tail {
            self.flat_body_tail(steps, body, arity);
        } else {
            for e in body {
                self.flat_stmt(steps, e, arity);
            }
        }
        self.reset(mark);
    }

    /// `recur`: every arg evaluates (in order), then the target
    /// registers take them; the empty signal says they are written.
    fn compile_recur(&mut self, args: &[Expr]) -> Exec {
        let targets = Arc::clone(&self.scope().recur);
        let ops: Vec<Operand> = args.iter().map(|a| self.operand(a, true)).collect();
        match (&targets[..], ops.len()) {
            (&[r], 1) => recur1(r, ops),
            (&[r0, r1], 2) => recur2([r0, r1], ops),
            (&[r0, r1, r2], 3) => recur3([r0, r1, r2], ops),
            (&[r0, r1, r2, r3], 4) => recur4([r0, r1, r2, r3], ops),
            _ => recur_n(targets, ops),
        }
    }

    /// Compile a fn's arities in a fresh scope whose params start at
    /// lexical index `base`; the result's capture list reads the
    /// enclosing scope.
    fn compile_fun(
        &mut self,
        fun: &FunExpr,
        base: usize,
        rec: Vec<usize>,
    ) -> (Arc<CompiledFun>, Vec<Loc>) {
        self.scopes.push(Scope::new(base, rec));
        let arities = fun.arities.iter().map(|a| self.compile_arity(a)).collect();
        let scope = self.scopes.pop().expect("the fn scope");
        let captures = scope.captures.iter().map(|(_, loc)| *loc).collect();
        let compiled = CompiledFun {
            display: Arc::from(fun.display.as_str()),
            source: Arc::clone(&fun.arities),
            base,
            arities,
        };
        (Arc::new(compiled), captures)
    }

    fn compile_arity(&mut self, arity: &Arity) -> CompiledArity {
        let pats: Vec<&Pattern> = arity.fixed.iter().chain(arity.rest.iter()).collect();
        let s = self.scope();
        s.locals.clear();
        s.next = pats.len();
        s.nregs = pats.len();
        s.recur = (0..pats.len()).collect();
        let mut binds: Vec<Exec> = Vec::new();
        for (raw, pat) in pats.iter().enumerate() {
            if matches!(pat, Pattern::Symbol(_)) {
                self.scope().locals.push(raw);
            } else {
                binds.push(self.destructure_step(pat, raw));
            }
        }
        let (base, visible) = {
            let s = self.scopes.last().expect("the fn scope");
            (s.base, s.base + s.locals.len())
        };
        self.scope().last = crate::vm_live::last_uses(&arity.body, base, visible);
        let body = self.compile_body(&arity.body);
        let bind: Option<Exec> = match binds.len() {
            0 => None,
            _ => Some(Box::new(move |ev, f| {
                for b in &binds {
                    b(ev, f)?;
                }
                Ok(Value::Nil)
            })),
        };
        CompiledArity {
            fixed: arity.fixed.len(),
            variadic: arity.rest.is_some(),
            nregs: self.scope().nregs,
            bind,
            body,
        }
    }

    fn compile_fn(&mut self, fun: &FunExpr) -> Exec {
        let base = self.visible();
        let (compiled, captures) = self.compile_fun(fun, base, Vec::new());
        Box::new(move |ev, f| {
            let captured = capture(f, &captures);
            Ok(make_closure(ev, &compiled, captured, None))
        })
    }

    /// `letfn` (and named `fn`s, the analyzer's one-fn `letfn`): the
    /// names take registers here; inside the fns they are the group
    /// slots, filled once every sibling exists.
    fn compile_letfn(&mut self, fns: &[(String, FunExpr)], body: &[Expr]) -> Exec {
        let mark = self.mark();
        let visible = self.visible();
        let names: Vec<usize> = fns.iter().map(|_| self.push_local()).collect();
        let rec: Vec<usize> = (visible..visible + fns.len()).collect();
        let funs: Vec<(Arc<CompiledFun>, Vec<Loc>)> = fns
            .iter()
            .map(|(_, fun)| self.compile_fun(fun, visible + fns.len(), rec.clone()))
            .collect();
        let body = self.compile_body(body);
        self.reset(mark);
        Box::new(move |ev, f| {
            let group = Arc::new(OnceLock::new());
            let mut made: Vec<Value> = Vec::with_capacity(funs.len());
            for (fun, captures) in &funs {
                let captured = capture(f, captures);
                made.push(make_closure(ev, fun, captured, Some(Arc::clone(&group))));
            }
            for (r, v) in names.iter().zip(&made) {
                f.regs[*r] = v.clone();
            }
            let _ = group.set(made.into_boxed_slice());
            body(ev, f)
        })
    }

    /// `lazy-seq*`: the body as a zero-arity closure (the JVM's
    /// `(fn* [] body)`, a `recur` target), forced by the seq.
    fn compile_lazy_seq(&mut self, body: &[Expr]) -> Exec {
        let (compiled, captures) = self.compile_thunk("lazy-seq", body);
        Box::new(move |_, f| {
            let captured = capture(f, &captures);
            let thunk = closure_value(0, &compiled, captured, None);
            Ok(crate::lazy::call0_value(thunk))
        })
    }

    /// `delay*`: a zero-arity closure (named like the tree
    /// evaluator's) trapped in an unrealized delay.
    fn compile_delay(&mut self, display: &str, body: &[Expr]) -> Exec {
        let (compiled, captures) = self.compile_thunk(display, body);
        Box::new(move |ev, f| {
            let captured = capture(f, &captures);
            let thunk = make_closure(ev, &compiled, captured, None);
            Ok(Value::Delay(clj_value::Delay::new(thunk)))
        })
    }

    fn compile_thunk(&mut self, display: &str, body: &[Expr]) -> (Arc<CompiledFun>, Vec<Loc>) {
        let fun = FunExpr {
            display: display.to_owned(),
            arities: Arc::from([Arity {
                fixed: Vec::new(),
                rest: None,
                body: body.to_vec(),
            }]),
        };
        let base = self.visible();
        self.compile_fun(&fun, base, Vec::new())
    }

    /// `try`: like the tree evaluator's — a panic unwinding through
    /// the body still runs `finally`; the first matching `catch`
    /// (by class hierarchy) binds the throwable in its register.
    fn compile_try(&mut self, body: &[Expr], catches: &[Catch], finally_body: &[Expr]) -> Exec {
        let body = self.compile_body(body);
        let handlers: Vec<(String, usize, Exec)> = catches
            .iter()
            .map(|c| {
                let mark = self.mark();
                let r = self.push_local();
                let handler = self.compile_body(&c.body);
                self.reset(mark);
                (c.class.clone(), r, handler)
            })
            .collect();
        let fin = self.compile_body(finally_body);
        let code = TryCode {
            body,
            handlers,
            fin,
        };
        Box::new(move |ev, f| run_try(ev, f, &code))
    }

    fn compile_case(
        &mut self,
        expr: &Expr,
        default: &Expr,
        cases: &[(i32, Expr, Expr)],
        shape: CaseShape,
    ) -> Exec {
        let target = self.compile(expr);
        let default = self.compile(default);
        let cases: Vec<(i32, Exec, Exec)> = cases
            .iter()
            .map(|(k, t, then)| (*k, self.compile(t), self.compile(then)))
            .collect();
        Box::new(move |ev, f| {
            let v = target(ev, f)?;
            let Some((probe, key)) = ev.case_probe(v, shape.shift, shape.mask, shape.test)? else {
                return default(ev, f);
            };
            match cases.iter().find(|(k, ..)| *k == key) {
                None => default(ev, f),
                Some((k, _, then)) if shape.test != CaseTest::Int && shape.skip.contains(k) => {
                    then(ev, f)
                }
                Some((_, t, then)) => {
                    let tv = t(ev, f)?;
                    if crate::lazy::equiv_vals(ev, &probe, &tv)? {
                        then(ev, f)
                    } else {
                        default(ev, f)
                    }
                }
            }
        })
    }

    fn compile_assert(&mut self, test: &Expr, message: Option<&Expr>, form: &str) -> Exec {
        let test = self.compile(test);
        let message = message.map(|m| self.compile(m));
        let form = form.to_owned();
        Box::new(move |ev, f| {
            if test(ev, f)?.is_truthy() {
                return Ok(Value::Nil);
            }
            let message = match &message {
                Some(m) => Some(m(ev, f)?),
                None => None,
            };
            Err(ev.assert_failed(message, &form))
        })
    }

    /// An invocation: the head, then the args in order. A VM closure
    /// with a fixed arity for the arg count takes them straight into
    /// its registers.
    fn compile_invoke(&mut self, head: &Expr, args: &[Expr], span: Span) -> Exec {
        if let Expr::Literal(Value::Keyword(kw)) = head {
            if let Some(exec) = self.compile_literal_keyword_invoke(kw, args, span) {
                return exec;
            }
        }
        let head = self.compile(head);
        let args: Vec<Operand> = args.iter().map(|a| self.operand(a, true)).collect();
        Box::new(move |ev, f| {
            let callee = head(ev, f)?;
            if let Some((code, arity)) = fixed_target(&callee, args.len()) {
                return call_fixed(ev, f, code, arity, &args);
            }
            let mut values: SmallVec<[Value; 4]> = SmallVec::new();
            eval_operands(ev, f, &args, &mut values)?;
            call_value(ev, &callee, &mut values, span)
        })
    }

    /// Fast-path literal keyword invocations (`(:kw target)` or `(:kw target fallback)`).
    fn compile_literal_keyword_invoke(
        &mut self,
        kw: &clj_value::Keyword,
        args: &[Expr],
        span: Span,
    ) -> Option<Exec> {
        if args.len() == 1 {
            let target = self.operand(&args[0], false);
            let kw_val = Value::Keyword(kw.clone());
            Some(Box::new(move |ev, f| {
                let comp = target.computed(ev, f)?;
                let m = target.read(f, comp.as_ref());
                if let Some(v) = crate::vm_intrinsics::get_of(m, &kw_val, None) {
                    return Ok(v);
                }
                let mut values: SmallVec<[Value; 4]> = SmallVec::new();
                values.push(m.clone());
                call_value(ev, &kw_val, &mut values, span)
            }))
        } else if args.len() == 2 {
            let target = self.operand(&args[0], false);
            let fallback = self.operand(&args[1], false);
            let kw_val = Value::Keyword(kw.clone());
            Some(Box::new(move |ev, f| {
                let comp_target = target.computed(ev, f)?;
                let comp_fb = fallback.computed(ev, f)?;
                let m = target.read(f, comp_target.as_ref());
                let fb = fallback.read(f, comp_fb.as_ref());
                if let Some(v) = crate::vm_intrinsics::get_of(m, &kw_val, Some(fb)) {
                    return Ok(v);
                }
                let mut values: SmallVec<[Value; 4]> = SmallVec::new();
                values.push(m.clone());
                values.push(fb.clone());
                call_value(ev, &kw_val, &mut values, span)
            }))
        } else {
            None
        }
    }

    /// A direct-call operand; `may_move` lets a last-use local move
    /// out (not for intrinsics, which read locals in place).
    fn operand(&mut self, expr: &Expr, may_move: bool) -> Operand {
        match expr {
            Expr::Literal(v) => Operand::Const(v.clone()),
            Expr::MapLit(pairs, meta) if pairs.is_empty() => {
                Operand::Const(Value::Map(clj_value::new_map().with_meta(meta.clone())))
            }
            Expr::VecLit(elems, meta) if elems.is_empty() => {
                Operand::Const(Value::Vector(Vector::new_sync().with_meta(meta.clone())))
            }
            Expr::SetLit(elems, meta) if elems.is_empty() => {
                Operand::Const(Value::Set(clj_value::new_set().with_meta(meta.clone())))
            }
            Expr::Local(idx) => match (self.resolve(*idx), may_move && self.is_last(expr)) {
                (loc @ Loc::Reg(_), true) => Operand::Loc(loc, true),
                (loc, _) => Operand::Loc(loc, false),
            },
            other => Operand::Exec(self.compile(other)),
        }
    }

    /// A direct-threaded native call: validated like the tree
    /// evaluator's `DirectNative`; the hot core ops compile per
    /// operand shape ([`crate::vm_ops`]), the other intrinsics read
    /// registers and literals in place.
    fn compile_direct(&mut self, call: &DirectCall) -> Exec {
        if let Some(code) = self.try_assoc_conj(call, false) {
            return code;
        }
        self.compile_direct_plain(call)
    }

    fn compile_direct_plain(&mut self, call: &DirectCall) -> Exec {
        if let Some(ctor) = crate::vm_ops::value_ctor(call) {
            let ops = self.in_place_operands(call);
            return ctor.code(site_of(call), ops);
        }
        let site = site_of(call);
        if let Some(ctor) = crate::vm_ops::update_ctor(call) {
            let ops: Vec<Operand> = call.args.iter().map(|a| self.operand(a, true)).collect();
            return crate::vm_ops::update_code(ctor, site, ops);
        }
        let ops: Vec<Operand> = call.args.iter().map(|a| self.operand(a, true)).collect();
        direct_general(site, ops)
    }

    fn compile_vec(&mut self, elems: &[Expr], meta: Option<Map>) -> Exec {
        if elems.is_empty() {
            let empty = Value::Vector(Vector::new_sync().with_meta(meta));
            return Box::new(move |_, _| Ok(empty.clone()));
        }
        let elems = self.compile_all(elems);
        Box::new(move |ev, f| {
            let mut out = Vec::with_capacity(elems.len());
            for e in &elems {
                out.push(e(ev, f)?);
            }
            let v = Vector::from_vec(out);
            // Metadata re-mints the identity: only a literal with any.
            Ok(Value::Vector(match &meta {
                Some(m) => v.with_meta(Some(m.clone())),
                None => v,
            }))
        })
    }

    fn compile_pairs(&mut self, pairs: &[(Expr, Expr)]) -> Vec<(Exec, Exec)> {
        pairs
            .iter()
            .map(|(k, v)| (self.compile(k), self.compile(v)))
            .collect()
    }

    fn compile_map(&mut self, pairs: &[(Expr, Expr)], meta: Option<Map>) -> Exec {
        if pairs.is_empty() {
            let empty = Value::Map(clj_value::new_map().with_meta(meta));
            return Box::new(move |_, _| Ok(empty.clone()));
        }
        let pairs = self.compile_pairs(pairs);
        Box::new(move |ev, f| {
            Ok(Value::Map(
                eval_pairs(ev, f, &pairs)?.with_meta(meta.clone()),
            ))
        })
    }

    fn compile_set(&mut self, elems: &[Expr], meta: Option<Map>) -> Exec {
        if elems.is_empty() {
            let empty = Value::Set(clj_value::new_set().with_meta(meta));
            return Box::new(move |_, _| Ok(empty.clone()));
        }
        let elems = self.compile_all(elems);
        Box::new(move |ev, f| {
            let mut out = clj_value::new_set();
            for e in &elems {
                let elem = e(ev, f)?;
                let elem = crate::lazy::realize_key(ev, &elem)?;
                out = out.insert(elem);
            }
            Ok(Value::Set(out.with_meta(meta.clone())))
        })
    }

    fn compile_def(
        &mut self,
        sym: &Symbol,
        doc: Option<String>,
        init: Option<&Expr>,
        meta: Option<&[(Expr, Expr)]>,
        span: Span,
    ) -> Exec {
        let meta = meta.map(|pairs| self.compile_pairs(pairs));
        let init = init.map(|e| self.compile(e));
        let sym = sym.clone();
        Box::new(move |ev, f| {
            let id = ev.def_begin(&sym);
            let value = match &init {
                Some(init) => Some(init(ev, f)?),
                None => None,
            };
            let name_meta = match &meta {
                Some(pairs) => Some(eval_pairs(ev, f, pairs)?),
                None => sym.meta().cloned(),
            };
            Ok(ev.def_finish(id, &sym, doc.clone(), value, name_meta, span))
        })
    }
}

/// A map literal's pairs, evaluated left to right (keys realized).
fn eval_pairs(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    pairs: &[(Exec, Exec)],
) -> Result<Map, Signal> {
    let mut out = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        let key = k(ev, f)?;
        let val = v(ev, f)?;
        let key = crate::lazy::realize_key(ev, &key)?;
        out.push((key, val));
    }
    Ok(Map::from_kvs_last_wins(out))
}

/// A compiled `try`: the body, `(class, register, handler)` per
/// `catch`, and the `finally` body.
struct TryCode {
    body: Exec,
    handlers: Vec<(String, usize, Exec)>,
    fin: Exec,
}

/// Run a `try` like the tree evaluator's: a panic unwinding through
/// the body still runs `finally`; a throw runs the first matching
/// handler (by class hierarchy) with the throwable in its register.
fn run_try(ev: &mut Evaluator, f: &mut Frame<'_>, code: &TryCode) -> Result<Value, Signal> {
    let result = match catch_unwind(AssertUnwindSafe(|| (code.body)(&mut *ev, &mut *f))) {
        Ok(result) => result,
        Err(payload) => {
            if let Err(signal @ Signal::Throw(_)) = (code.fin)(ev, f) {
                return Err(signal);
            }
            resume_unwind(payload);
        }
    };
    let thrown = match result {
        Ok(value) => {
            (code.fin)(ev, f)?;
            return Ok(value);
        }
        Err(signal) => crate::eval::try_thrown(signal).expect("recur cannot cross try"),
    };
    let handled = code
        .handlers
        .iter()
        .find(|(class, ..)| thrown.class.matches(class));
    let result = match handled {
        Some((_, r, handler)) => {
            f.regs[*r] = Value::Throwable(Arc::new(thrown));
            handler(ev, f)
        }
        None => Err(Signal::Throw(thrown)),
    };
    finish_try(ev, f, &code.fin, result)
}

/// `finally` after the handled (or rethrown) outcome: a throwing
/// `finally` masks the in-flight exception (probe29).
fn finish_try(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    fin: &Exec,
    result: Result<Value, Signal>,
) -> Result<Value, Signal> {
    match result {
        Ok(value) => {
            fin(ev, f)?;
            Ok(value)
        }
        Err(_) => match fin(ev, f) {
            Err(signal @ Signal::Throw(_)) => Err(signal),
            _ => result,
        },
    }
}

/// A one-arg `recur` (the empty signal says the register is
/// written; see `run_loop`).
fn recur1(r: usize, mut ops: Vec<Operand>) -> Exec {
    let a = ops.pop().expect("one arg");
    Box::new(move |ev, f| {
        let v = a.eval(ev, f)?;
        set_reg(&mut f.regs[r], v);
        Err(Signal::Recur(Vec::new()))
    })
}

/// A two-arg `recur` (every arg evaluates before any register is
/// written).
fn recur2([r0, r1]: [usize; 2], ops: Vec<Operand>) -> Exec {
    let [a, b] = <[Operand; 2]>::try_from(ops).ok().expect("two args");
    Box::new(move |ev, f| {
        let (va, vb) = (a.eval(ev, f)?, b.eval(ev, f)?);
        set_reg(&mut f.regs[r0], va);
        set_reg(&mut f.regs[r1], vb);
        Err(Signal::Recur(Vec::new()))
    })
}

fn recur3([r0, r1, r2]: [usize; 3], ops: Vec<Operand>) -> Exec {
    let [a, b, c] = <[Operand; 3]>::try_from(ops).ok().expect("three args");
    Box::new(move |ev, f| {
        let (va, vb, vc) = (a.eval(ev, f)?, b.eval(ev, f)?, c.eval(ev, f)?);
        set_reg(&mut f.regs[r0], va);
        set_reg(&mut f.regs[r1], vb);
        set_reg(&mut f.regs[r2], vc);
        Err(Signal::Recur(Vec::new()))
    })
}

fn recur4([r0, r1, r2, r3]: [usize; 4], ops: Vec<Operand>) -> Exec {
    let [a, b, c, d] = <[Operand; 4]>::try_from(ops).ok().expect("four args");
    Box::new(move |ev, f| {
        let (va, vb) = (a.eval(ev, f)?, b.eval(ev, f)?);
        let (vc, vd) = (c.eval(ev, f)?, d.eval(ev, f)?);
        set_reg(&mut f.regs[r0], va);
        set_reg(&mut f.regs[r1], vb);
        set_reg(&mut f.regs[r2], vc);
        set_reg(&mut f.regs[r3], vd);
        Err(Signal::Recur(Vec::new()))
    })
}

/// Any other `recur`: each value moves straight to its register
/// once every arg has evaluated (no buffer-to-iterator copy).
fn recur_n(targets: Arc<[usize]>, ops: Vec<Operand>) -> Exec {
    Box::new(move |ev, f| {
        let mut values: SmallVec<[Value; 8]> = SmallVec::new();
        for op in &ops {
            values.push(op.eval(ev, f)?);
        }
        for (r, v) in targets.iter().zip(values.iter_mut()) {
            set_reg(&mut f.regs[*r], std::mem::replace(v, Value::Nil));
        }
        Err(Signal::Recur(Vec::new()))
    })
}

/// An `and`/`or` step over the let-bound local `g`: `(let* [g a]
/// (if g b g))` answers `(a, b, true)`, `(let* [g a] (if g g b))`
/// `(a, b, false)`.
pub(crate) fn and_or_shape(expr: &Expr, g: usize) -> Option<(&Expr, &Expr, bool)> {
    let Expr::Let { bindings, body } = expr else {
        return None;
    };
    let ([(Pattern::Symbol(_), a)], [Expr::If { test, then, els }]) =
        (bindings.as_slice(), body.as_slice())
    else {
        return None;
    };
    let is_g = |e: &Expr| matches!(e, Expr::Local(i) if *i == g);
    match (is_g(test), is_g(then), is_g(els)) {
        (true, false, true) => Some((a, then, true)),
        (true, true, false) => Some((a, els, false)),
        _ => None,
    }
}

/// One `loop` binding: the raw value's register, its init, and the
/// destructuring unpack (none for a symbol).
type LoopStep = (usize, Exec, Option<Exec>);

/// One `let` binding: a symbol's register and init, or a
/// destructuring step.
enum LetStep {
    Name(usize, Exec),
    Unpack(Exec),
}

/// A compiled `loop`.
struct LoopCode {
    steps: Vec<LoopStep>,
    targets: Arc<[usize]>,
    body: Exec,
    /// Whether any binding destructures (unpacks after each `recur`).
    unpacks: bool,
    /// The fused shape (single `if` body with a terminal `recur`
    /// in one branch), when detected.
    fused: Option<FusedLoop>,
    /// The flat shape (see [`FlatLoop`]) when the loop takes it:
    /// no destructuring loop bindings, no fused shape.
    flat: Option<FlatLoop>,
    /// The unboxed program (see [`crate::vm_scalar`]) tried first,
    /// when no unboxed-int fused program covers the loop.
    scalar: Option<crate::vm_scalar::ScalarLoop>,
}

/// One flat-loop step: the `let`/`if`/`do` skeleton of a loop
/// body as a jump-threaded list, so an iteration pays one Rust
/// `match` dispatch per step instead of a dyn-closure call per
/// nesting level. Leaf forms stay compiled [`Exec`]s, evaluated
/// in exactly the order the closure tree evaluated them.
enum Step {
    /// Evaluate for effect, discard the value.
    Eval(Exec),
    /// A `let` name binding: evaluate into its register.
    Set(usize, Exec),
    /// A `let` destructuring step.
    Unpack(Exec),
    /// Jump to the target step when the test is falsy.
    JumpUnless(Test, usize),
    /// Jump to the target step.
    Jump(usize),
    /// A terminal `recur`: write the loop targets, restart.
    Recur(Box<[Operand]>),
    /// Evaluate and return from the loop.
    Exit(Exec),
}

/// A loop body as flat steps (see [`Step`]).
struct FlatLoop {
    steps: Box<[Step]>,
}

/// A fused loop body: test, recur operands (evaluated in order,
/// registers written after every operand), and the exit branch —
/// one Rust loop instead of a closure tree plus a signal per
/// iteration.
struct FusedLoop {
    test: Test,
    exit: Exec,
    ops: Vec<Operand>,
    recur_when_true: bool,
    /// The unboxed-int program for the same test/operands, when
    /// every piece mirrors on `i64` (see [`run_fused_int`]).
    int: Option<IntFused>,
}

/// An unboxed-int operand source.
#[derive(Clone, Copy)]
enum IntSrc {
    /// A loop variable, by position in the loop targets.
    State(usize),
    /// A loop-invariant register.
    Reg(usize),
    /// A captured local.
    Up(usize),
    /// An integer literal.
    Kst(i64),
}

/// One unboxed `recur` operand: a source, or a direct int op
/// over sources.
enum IntOperand {
    Src(IntSrc),
    One(crate::vm_ops::IntOp1, IntSrc),
    Two(crate::vm_ops::IntOp2, IntSrc, IntSrc),
}

/// An unboxed branch test over sources.
enum IntTest {
    One(crate::vm_ops::IntTest1, IntSrc),
    Two(crate::vm_ops::IntTest2, IntSrc, IntSrc),
}

/// A fused loop's unboxed-int program: while the loop state
/// stays `Int` and every site stays valid, iterations run on
/// `i64` locals with no `Value` traffic. The first miss
/// (non-`Int` state or source, an overflowing or otherwise
/// missing op, an invalid site, the exit test) writes the
/// state back boxed and hands the loop to [`run_fused`], so
/// every observable behavior comes from the ordinary road.
struct IntFused {
    test: IntTest,
    ops: Box<[IntOperand]>,
    sites: Box<[crate::vm_ops::Site]>,
}

/// Run a `loop`: bind in order (each init sees the earlier names),
/// then run the body, rewriting the raw registers (and unpacking
/// again) on every `recur`.
/// Write a tree-evaluated `recur`'s carried values into the loop
/// targets (the VM's own `recur` writes registers first and
/// carries none).
fn absorb_recur_values(code: &LoopCode, f: &mut Frame<'_>, values: Vec<Value>) {
    for (r, v) in code.targets.iter().zip(values) {
        f.regs[*r] = v;
    }
}

/// Re-unpack destructured loop bindings after a `recur` rewrote
/// their raw registers (no steps run for plain bindings).
fn run_unpacks(ev: &mut Evaluator, f: &mut Frame<'_>, code: &LoopCode) -> Result<(), Signal> {
    if code.unpacks {
        for unpack in code.steps.iter().filter_map(|(_, _, u)| u.as_ref()) {
            unpack(ev, f)?;
        }
    }
    Ok(())
}

fn run_loop(ev: &mut Evaluator, f: &mut Frame<'_>, code: &LoopCode) -> Result<Value, Signal> {
    for (r, init, unpack) in &code.steps {
        let v = init(ev, f)?;
        set_reg(&mut f.regs[*r], v);
        if let Some(unpack) = unpack {
            unpack(ev, f)?;
        }
    }
    if let Some(v) = crate::vm_scalar::try_run(ev, f, code.scalar.as_ref(), &code.targets) {
        return Ok(v);
    }
    if let Some(flat) = &code.flat {
        return run_flat(ev, f, code, flat);
    }
    if let Some(fused) = &code.fused {
        if let Some(int) = &fused.int {
            return run_fused_int(ev, f, code, fused, int);
        }
        return run_fused(ev, f, code, fused);
    }
    loop {
        match (code.body)(ev, f) {
            // The VM's own `recur` wrote the registers already.
            Err(Signal::Recur(values)) if values.is_empty() => {}
            // A tree-evaluated one carries the values.
            Err(Signal::Recur(values)) => absorb_recur_values(code, f, values),
            other => return other,
        }
        run_unpacks(ev, f, code)?;
    }
}

/// A discarded evaluation's next step (see [`run_flat`]).
fn discard_value(v: Result<Value, Signal>, pc: usize) -> Result<usize, Signal> {
    v.map(|_| pc + 1)
}

/// A branch test's next step: fall through or jump.
fn branch_target(t: Result<bool, Signal>, pc: usize, target: usize) -> Result<usize, Signal> {
    t.map(|b| if b { pc + 1 } else { target })
}

/// A `recur` write's next step: back to the first.
fn restart(r: Result<(), Signal>) -> Result<usize, Signal> {
    r.map(|()| 0)
}

/// A flat `Set` step: evaluate into its register, next step on.
fn flat_set(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    r: usize,
    e: &Exec,
    pc: usize,
) -> Result<usize, Signal> {
    let v = e(ev, f)?;
    set_reg(&mut f.regs[r], v);
    Ok(pc + 1)
}

/// Run a flat loop body: step dispatch in one Rust loop (a
/// separate step function cost more than the closure call it
/// replaced). A `Recur` signal from an opaque leaf (a VM `recur`
/// inside a compiled subform, or a tree-evaluated one carrying
/// values) restarts at the first step, like [`run_loop`].
fn run_flat(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    code: &LoopCode,
    flat: &FlatLoop,
) -> Result<Value, Signal> {
    let mut pc = 0usize;
    loop {
        let flow = match &flat.steps[pc] {
            Step::Eval(e) | Step::Unpack(e) => discard_value(e(ev, f), pc),
            Step::Set(r, e) => flat_set(ev, f, *r, e, pc),
            Step::JumpUnless(test, target) => branch_target(test(ev, f), pc, *target),
            Step::Jump(target) => Ok(*target),
            Step::Recur(ops) => restart(write_recur_ops(ev, f, &code.targets, ops)),
            Step::Exit(e) => match e(ev, f) {
                Ok(v) => return Ok(v),
                Err(s) => Err(s),
            },
        };
        match flow {
            Ok(next) => pc = next,
            Err(Signal::Recur(values)) => {
                absorb_recur_values(code, f, values);
                pc = 0;
            }
            Err(s) => return Err(s),
        }
    }
}

/// Point the not-yet-targeted jump step at `at` to the
/// current end of the step list.
fn patch_jump(steps: &mut [Step], at: usize) {
    let target = steps.len();
    if let Step::JumpUnless(_, t) | Step::Jump(t) = &mut steps[at] {
        *t = target;
    }
}

/// A literal `nil` form (its compile allocates nothing, so a
/// flat emitter may skip it without disturbing allocation).
fn is_nil_literal(e: &Expr) -> bool {
    matches!(e, Expr::Literal(Value::Nil))
}

/// The args of a bare terminal `recur` with the loop's arity.
fn terminal_recur(e: &Expr, arity: usize) -> Option<&Vec<Expr>> {
    match e {
        Expr::Recur(args) if args.len() == arity => Some(args),
        _ => None,
    }
}

/// Evaluate a `recur`'s operands (in order) and write the target
/// registers only after every operand has evaluated — the same
/// order `recur1..4`/`recur_n` keep.
fn write_recur_ops(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    ops: &[Operand],
) -> Result<(), Signal> {
    match ops {
        [a] => write_ops1(ev, f, t, a),
        [a, b] => write_ops2(ev, f, t, a, b),
        [a, b, c] => write_ops3(ev, f, t, a, b, c),
        [a, b, c, d] => write_ops4(ev, f, t, a, b, c, d),
        ops => write_ops_n(ev, f, t, ops),
    }
}

fn write_ops1(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    a: &Operand,
) -> Result<(), Signal> {
    let v = a.eval(ev, f)?;
    set_reg(&mut f.regs[t[0]], v);
    Ok(())
}

fn write_ops2(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    a: &Operand,
    b: &Operand,
) -> Result<(), Signal> {
    let (va, vb) = (a.eval(ev, f)?, b.eval(ev, f)?);
    set_reg(&mut f.regs[t[0]], va);
    set_reg(&mut f.regs[t[1]], vb);
    Ok(())
}

fn write_ops3(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    a: &Operand,
    b: &Operand,
    c: &Operand,
) -> Result<(), Signal> {
    let (va, vb, vc) = (a.eval(ev, f)?, b.eval(ev, f)?, c.eval(ev, f)?);
    set_reg(&mut f.regs[t[0]], va);
    set_reg(&mut f.regs[t[1]], vb);
    set_reg(&mut f.regs[t[2]], vc);
    Ok(())
}

fn write_ops4(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    a: &Operand,
    b: &Operand,
    c: &Operand,
    d: &Operand,
) -> Result<(), Signal> {
    let (va, vb, vc, vd) = (
        a.eval(ev, f)?,
        b.eval(ev, f)?,
        c.eval(ev, f)?,
        d.eval(ev, f)?,
    );
    set_reg(&mut f.regs[t[0]], va);
    set_reg(&mut f.regs[t[1]], vb);
    set_reg(&mut f.regs[t[2]], vc);
    set_reg(&mut f.regs[t[3]], vd);
    Ok(())
}

fn write_ops_n(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    t: &[usize],
    ops: &[Operand],
) -> Result<(), Signal> {
    let mut values: SmallVec<[Value; 8]> = SmallVec::new();
    for op in ops {
        values.push(op.eval(ev, f)?);
    }
    for (r, v) in t.iter().zip(values) {
        set_reg(&mut f.regs[*r], v);
    }
    Ok(())
}

/// Run a fused loop body in one Rust loop (no `Recur` signal
/// per iteration): `fused_once`'s `None` continues, its value
/// ends the loop, and a `Recur` signal from an opaque leaf is
/// absorbed like [`run_loop`]'s.
fn run_fused(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    code: &LoopCode,
    fused: &FusedLoop,
) -> Result<Value, Signal> {
    loop {
        match fused_once(ev, f, code, fused) {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {}
            Err(Signal::Recur(values)) => absorb_recur_values(code, f, values),
            Err(s) => return Err(s),
        }
        run_unpacks(ev, f, code)?;
    }
}

/// One fused iteration: the exit branch's value, or `None` after
/// a `recur` wrote the target registers.
fn fused_once(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    code: &LoopCode,
    fused: &FusedLoop,
) -> Result<Option<Value>, Signal> {
    if (fused.test)(ev, f)? != fused.recur_when_true {
        return (fused.exit)(ev, f).map(Some);
    }
    write_recur_ops(ev, f, &code.targets, &fused.ops)?;
    Ok(None)
}

/// An unboxed source's value (`None` for a non-`Int` read: the
/// stretch deopts).
#[inline(always)]
fn int_src_value(src: &IntSrc, f: &Frame<'_>, state: &[i64]) -> Option<i64> {
    let v = match src {
        IntSrc::State(p) => return Some(state[*p]),
        IntSrc::Reg(r) => &f.regs[*r],
        IntSrc::Up(u) => &f.up[*u],
        IntSrc::Kst(n) => return Some(*n),
    };
    match v {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

/// The unboxed test's verdict (`None` deopts).
#[inline(always)]
fn int_test_value(test: &IntTest, f: &Frame<'_>, state: &[i64]) -> Option<bool> {
    match test {
        IntTest::One(op, a) => Some(op.test(int_src_value(a, f, state)?)),
        IntTest::Two(op, a, b) => {
            Some(op.test(int_src_value(a, f, state)?, int_src_value(b, f, state)?))
        }
    }
}

/// The next unboxed state (every operand read from the current
/// state first, like the boxed write-after-all; `next` is
/// pre-sized to the state; `false` deopts).
#[inline(always)]
fn int_next_state(int: &IntFused, f: &Frame<'_>, state: &[i64], next: &mut [i64]) -> bool {
    for (slot, op) in next.iter_mut().zip(&int.ops) {
        let v = match op {
            IntOperand::Src(a) => int_src_value(a, f, state),
            IntOperand::One(op, a) => int_src_value(a, f, state).and_then(|x| op.apply(x)),
            IntOperand::Two(op, a, b) => {
                match (int_src_value(a, f, state), int_src_value(b, f, state)) {
                    (Some(x), Some(y)) => op.apply(x, y),
                    _ => None,
                }
            }
        };
        match v {
            Some(v) => *slot = v,
            None => return false,
        }
    }
    true
}

/// Whether every site of the program still validates, checking
/// under a fresh snapshot (the sentinel start forces the first
/// stretch's validation through here).
#[inline(always)]
fn int_sites_ok(ev: &mut Evaluator, int: &IntFused, snap: &mut (u64, u64)) -> bool {
    if ev.direct_snapshot() == *snap {
        return true;
    }
    let ok = int.sites.iter().all(|s| s.valid(ev));
    *snap = ev.direct_snapshot();
    ok
}

/// The unboxed stretch of a fused loop (see [`IntFused`]): runs
/// plain iterations on `i64` locals and deopts to [`run_fused`]
/// with the state written back at the first miss or exit. The
/// iteration that deopts has run nothing observable (the test
/// and operands are pure int reads), so the boxed road re-runs
/// it whole.
fn run_fused_int(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    code: &LoopCode,
    fused: &FusedLoop,
    int: &IntFused,
) -> Result<Value, Signal> {
    let mut cur_buf: SmallVec<[i64; 8]> = SmallVec::new();
    for r in code.targets.iter() {
        match f.regs[*r] {
            Value::Int(n) => cur_buf.push(n),
            _ => return run_fused(ev, f, code, fused),
        }
    }
    let mut snap = (u64::MAX, u64::MAX);
    let mut nxt_buf: SmallVec<[i64; 8]> = SmallVec::new();
    nxt_buf.resize(cur_buf.len(), 0);
    // Ping-pong the two buffers (a per-iteration copy of the
    // state would dominate the stretch it saves).
    let mut flipped = false;
    loop {
        let (state, next): (&[i64], &mut [i64]) = if flipped {
            (&nxt_buf, &mut cur_buf)
        } else {
            (&cur_buf, &mut nxt_buf)
        };
        if !int_sites_ok(ev, int, &mut snap) {
            break;
        }
        match int_test_value(&int.test, f, state) {
            Some(t) if t == fused.recur_when_true => {}
            _ => break,
        }
        if !int_next_state(int, f, state, next) {
            break;
        }
        flipped = !flipped;
    }
    let state: &[i64] = if flipped { &nxt_buf } else { &cur_buf };
    for (r, v) in code.targets.iter().zip(state) {
        set_reg(&mut f.regs[*r], Value::Int(*v));
    }
    run_fused(ev, f, code, fused)
}

/// A tree-evaluator frame holding `locs`' values at their lexical
/// indexes.
fn frame_env(f: &Frame<'_>, locs: &[Loc]) -> Env {
    if locs.is_empty() {
        Env::new()
    } else {
        Env::new().extend(locs.iter().map(|l| f.get(*l).clone()).collect())
    }
}

/// `case*`'s dispatch shape.
struct CaseShape {
    shift: i32,
    mask: i32,
    skip: Vec<i32>,
    test: CaseTest,
}

/// A constant-message throwable (a fresh identity per evaluation).
fn new_throwable(ev: &mut Evaluator, class: ThrowClass, message: &Arc<str>) -> Value {
    Value::Throwable(Arc::new(Throwable {
        id: ev.next_id(),
        class,
        message: Some(Arc::clone(message)),
        data: None,
        cause: None,
    }))
}

/// Clone with the scalar kinds inline (no call into the clone glue).
#[inline(always)]
pub(crate) fn clone_value(v: &Value) -> Value {
    match v {
        Value::Int(n) => Value::Int(*n),
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        other => other.clone(),
    }
}

/// Overwrite a register, skipping the drop glue for scalars.
#[inline(always)]
fn set_reg(slot: &mut Value, v: Value) {
    let old = std::mem::replace(slot, v);
    if matches!(
        old,
        Value::Int(_) | Value::Nil | Value::Bool(_) | Value::Char(_) | Value::Double(_)
    ) {
        std::mem::forget(old);
    }
}

fn konst(v: Value) -> Exec {
    Box::new(move |_, _| Ok(v.clone()))
}

/// A last-use read: the value moves out (the register is dead).
fn take_local(r: usize) -> Exec {
    Box::new(move |_, f| Ok(std::mem::replace(&mut f.regs[r], Value::Nil)))
}

pub(crate) fn local(loc: Loc) -> Exec {
    match loc {
        Loc::Reg(r) => Box::new(move |_, f| Ok(clone_value(&f.regs[r]))),
        Loc::Up(u) => Box::new(move |_, f| Ok(f.up[u].clone())),
        Loc::Rec(j) => Box::new(move |_, f| Ok(f.rec[j].clone())),
    }
}

/// The next VM cache site (process-wide numbering: every
/// evaluator indexes its own cache by it).
pub(crate) fn next_site() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn global(var: Var) -> Exec {
    let site = next_site();
    Box::new(move |ev, _| Ok(ev.site_global(site, &var)))
}

/// A direct-call argument: literals and locals read in place (no
/// clone for the `Int` fast paths).
pub(crate) enum Operand {
    Const(Value),
    /// A local; `true` marks a last-use register read, which moves
    /// out on evaluation.
    Loc(Loc, bool),
    Exec(Exec),
}

impl Operand {
    /// A computed operand's value (`None` for in-place operands).
    pub(crate) fn computed(
        &self,
        ev: &mut Evaluator,
        f: &mut Frame<'_>,
    ) -> Result<Option<Value>, Signal> {
        match self {
            Operand::Exec(e) => e(ev, f).map(Some),
            _ => Ok(None),
        }
    }

    /// The operand in place (`computed` holds an `Exec`'s value).
    pub(crate) fn read<'v>(&'v self, f: &'v Frame<'_>, computed: Option<&'v Value>) -> &'v Value {
        match self {
            Operand::Const(v) => v,
            Operand::Loc(l, _) => f.get(*l),
            Operand::Exec(_) => computed.expect("computed operands are evaluated first"),
        }
    }

    #[inline]
    pub(crate) fn eval(&self, ev: &mut Evaluator, f: &mut Frame<'_>) -> Result<Value, Signal> {
        match self {
            Operand::Const(v) => Ok(v.clone()),
            Operand::Loc(Loc::Reg(r), true) => Ok(std::mem::replace(&mut f.regs[*r], Value::Nil)),
            Operand::Loc(l, _) => Ok(clone_value(f.get(*l))),
            Operand::Exec(e) => e(ev, f),
        }
    }
}

fn eval_operands(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    ops: &[Operand],
    out: &mut SmallVec<[Value; 4]>,
) -> Result<(), Signal> {
    for op in ops {
        out.push(op.eval(ev, f)?);
    }
    Ok(())
}

/// The slow road (validation miss): re-resolve the head and invoke
/// generally, like the tree evaluator's `invoke_direct_slow`.
fn direct_slow(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    site: &DirectCall,
    ops: &[Operand],
) -> Result<Value, Signal> {
    let callee = ev.deref_var_cached(site.var.id, &site.var.sym);
    let mut values: SmallVec<[Value; 4]> = SmallVec::new();
    eval_operands(ev, f, ops, &mut values)?;
    call_value(ev, &callee, &mut values, site.span)
}

fn direct_general(site: Arc<DirectCall>, ops: Vec<Operand>) -> Exec {
    let sid = next_site();
    Box::new(move |ev, f| {
        if !ev.site_direct_valid(sid, &site) {
            return direct_slow(ev, f, &site, &ops);
        }
        let mut values: SmallVec<[Value; 4]> = SmallVec::new();
        eval_operands(ev, f, &ops, &mut values)?;
        ev.call_direct_values(&site, &mut values)
    })
}

/// The direct call `e` makes to `clojure.core/<name>` with `argc`
/// arguments.
fn core_call<'e>(e: &'e Expr, name: &str, argc: usize) -> Option<&'e DirectCall> {
    match e {
        Expr::DirectNative(call)
            if call.args.len() == argc && crate::vm_ops::core_name(call) == Some(name) =>
        {
            Some(call)
        }
        _ => None,
    }
}

/// `(assoc! t k (conj (nth t k) x))`'s parts: the transient local,
/// the key, the appended form, and the `conj` and `nth` calls.
type AssocConj<'e> = (&'e Expr, &'e Expr, &'e Expr, &'e DirectCall, &'e DirectCall);

/// The fused transient update's shape (see
/// `Compiler::try_assoc_conj`): one local `t` and the same key (a
/// local or `Int` literal) in both places.
fn assoc_conj_shape(call: &DirectCall) -> Option<AssocConj<'_>> {
    let ([t, k, upd], Some("assoc!")) = (&call.args[..], crate::vm_ops::core_name(call)) else {
        return None;
    };
    let conj = core_call(upd, "conj", 2)?;
    let nth = core_call(&conj.args[0], "nth", 2)?;
    let same_t = matches!((t, &nth.args[0]), (Expr::Local(a), Expr::Local(b)) if a == b);
    let same_k = match (k, &nth.args[1]) {
        (Expr::Local(a), Expr::Local(b)) => a == b,
        (Expr::Literal(Value::Int(a)), Expr::Literal(Value::Int(b))) => a == b,
        _ => false,
    };
    (same_t && same_k).then_some((t, k, &conj.args[1], conj, nth))
}

/// A direct call's site record (the analyzed call minus its
/// argument forms, which compile to operands).
fn site_of(call: &DirectCall) -> Arc<DirectCall> {
    Arc::new(DirectCall {
        var: call.var.clone(),
        expect: Arc::clone(&call.expect),
        func: Arc::clone(&call.func),
        owned: call.owned.clone(),
        fast: call.fast,
        args: Vec::new(),
        span: call.span,
    })
}

/// [`direct_slow`] over already-evaluated args.
pub(crate) fn direct_slow_values(
    ev: &mut Evaluator,
    site: &DirectCall,
    mut values: SmallVec<[Value; 4]>,
) -> Result<Value, Signal> {
    let callee = ev.deref_var_cached(site.var.id, &site.var.sym);
    call_value(ev, &callee, &mut values, site.span)
}

#[cfg(test)]
mod tests {
    use clj_value::Value;

    #[test]
    fn compiled_closures_debug_their_display() {
        // `Closure`'s derived `Debug` reaches the compiled fn, which
        // shows its display name only (its code is closures).
        let mut eval = crate::boot();
        let v = eval.eval_source("(fn named-for-debug [x] x)").unwrap();
        assert!(matches!(v.last(), Some(Value::Extension(_))));
        let shown = format!("{:?}", v.last());
        assert!(shown.contains("CompiledFun"), "{shown}");
        assert!(shown.contains("named-for-debug"), "{shown}");
        assert!(shown.contains("rec: Some(1)"), "{shown}");
    }
}
