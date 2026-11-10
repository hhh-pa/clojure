// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The unboxed loop tier (#254, spec §1.8): a whole `loop` body
//! over scalars.
//!
//! The fused and flat tiers pay a boxed `Value` and a closure call
//! per operation; string and vector scans (Boyer-Moore and KMP
//! tables, their search loops) are nothing but such operations. A
//! loop whose body is a tree of `if`/`let`/nested `loop` forms over
//! `Int`/`Char`/`Bool` values compiles here into a program that
//! keeps the loop state, every `let` temp, and every nested loop's
//! state as tagged scalars ([`Sv`]), reads invariant locals in
//! place, and performs `assoc!`/`conj!` on transient vectors at the
//! `recur` or exit that ends an iteration.
//!
//! Every operation answers exactly what its boxed op answers, or
//! nothing: a non-scalar read, an overflow, an out-of-range
//! index, a non-ASCII-safe miss, an invalid call site, or an
//! unsupported form. Nothing observable happens before an
//! iteration commits — effects run only after every value of the
//! iteration (its `recur` operands, its effect arguments) has
//! computed and every effect's target has checked out — so a miss
//! writes the iteration's starting state back to the registers and
//! hands the loop to the boxed tiers, which re-run that iteration
//! whole. Results, errors, evaluation order and redefinition
//! behavior therefore all come from the ordinary road.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clj_value::{TransientKind, Value};
use smallvec::SmallVec;

use crate::eval::Evaluator;
use crate::expr::{DirectCall, Expr, Pattern};
use crate::vm::{Frame, Loc};
use crate::vm_ops::{IntCtor, IntOp1, IntOp2, IntTest1, IntTest2, IntTestCtor, Site};

/// A scalar's kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum K {
    Int,
    Char,
    Bool,
}

/// An unboxed scalar: its kind and payload (a char's code, a
/// boolean's 0/1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sv {
    k: K,
    v: i64,
}

impl Sv {
    const ZERO: Sv = Sv { k: K::Int, v: 0 };

    #[inline(always)]
    fn int(v: i64) -> Self {
        Sv { k: K::Int, v }
    }

    #[inline(always)]
    fn bool(b: bool) -> Self {
        Sv {
            k: K::Bool,
            v: i64::from(b),
        }
    }

    #[inline(always)]
    fn of(v: &Value) -> Option<Self> {
        match v {
            Value::Int(n) => Some(Sv::int(*n)),
            Value::Char(c) => Some(Sv {
                k: K::Char,
                v: i64::from(u32::from(*c)),
            }),
            Value::Bool(b) => Some(Sv::bool(*b)),
            _ => None,
        }
    }

    fn value(self) -> Value {
        match self.k {
            K::Int => Value::Int(self.v),
            K::Char => char::from_u32(self.v as u32).map_or(Value::Nil, Value::Char),
            K::Bool => Value::Bool(self.v != 0),
        }
    }

    #[inline(always)]
    fn int_of(self) -> Option<i64> {
        (self.k == K::Int).then_some(self.v)
    }

    /// Clojure truth: only `false` is falsy among scalars.
    #[inline(always)]
    fn truthy(self) -> bool {
        self.k != K::Bool || self.v != 0
    }
}

/// A numeric cast (`int`, `long`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cast {
    Int,
    Long,
}

/// A scalar expression.
pub(crate) enum IExpr {
    /// A loop variable, `let` temp, or nested loop variable.
    Slot(usize),
    /// A local bound outside the loop, read in place.
    Loc(Loc),
    Kst(Sv),
    Calc(Box<Calc>),
    Ctl(Box<Ctl>),
    /// An unsupported form: always misses.
    Deopt,
}

/// A computing operation.
pub(crate) enum Calc {
    Op1(IntOp1, IExpr),
    Op2(IntOp2, IExpr, IExpr),
    Cast(Cast, IExpr),
    /// `(nth coll i)` over an invariant collection.
    Nth(Loc, IExpr),
    /// `(count coll)` over an invariant collection.
    Count(Loc),
}

/// A control form in value position.
pub(crate) enum Ctl {
    If(Cond, IExpr, IExpr),
    Let(Box<[(usize, IExpr)]>, IExpr),
    /// A nested loop's value.
    Loop(Inner),
    /// A test as a boolean value.
    Test(Cond),
}

/// A branch test.
pub(crate) enum Cond {
    Leaf(Cmp),
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

/// A comparison or truth test.
pub(crate) enum Cmp {
    Truthy(IExpr),
    T1(IntTest1, IExpr),
    /// `<`/`<=`/`>`/`>=`/`==` over `Int`s.
    T2(IntTest2, IExpr, IExpr),
    /// `=` over two scalars of any kinds.
    Eq(IExpr, IExpr),
}

/// A transient mutation at the end of an iteration.
pub(crate) enum Effect {
    /// `(assoc! t k v)` with `k` inside the vector (no append).
    Assoc(Loc, IExpr, IExpr),
    /// `(conj! t v)`.
    Conj(Loc, IExpr),
}

/// A loop's exit value.
pub(crate) enum ExitVal {
    Expr(IExpr),
    /// A literal (`nil`, a keyword …).
    Const(Value),
    /// A boxed local: a passthrough loop variable or an invariant
    /// (an outer loop's own variables are read from their slots,
    /// see [`Leaf::Exit`]).
    Box(Loc),
    /// An outer loop variable (its slot when scalar, else its
    /// register).
    Var(usize),
}

/// The end of one iteration.
pub(crate) enum Leaf {
    /// `recur`: one operand per target (`None` passes the
    /// variable through unchanged).
    Recur(Box<[Effect]>, Box<[Option<IExpr>]>),
    Exit(Box<[Effect]>, ExitVal),
}

/// A loop body: a tree of tests and temps ending in leaves.
pub(crate) enum Node {
    If(Cond, Box<Node>, Box<Node>),
    Let(Box<[(usize, IExpr)]>, Box<Node>),
    Leaf(Leaf),
    Deopt,
}

/// A nested loop used as a value.
pub(crate) struct Inner {
    slots: Box<[usize]>,
    inits: Box<[IExpr]>,
    body: Node,
}

/// A slot of the program's scalar file.
type Sl = u16;

/// One straight-line step over slots.
#[derive(Clone, Copy)]
enum Step {
    Op1(IntOp1, Sl, Sl),
    Op2(IntOp2, Sl, Sl, Sl),
    Cast(Cast, Sl, Sl),
    Mov(Sl, Sl),
    Set(Sl, bool),
}

/// A collection read: result, collection index (and index slot).
#[derive(Clone, Copy)]
enum Co {
    Nth(Sl, u16, Sl),
    Count(Sl, u16),
}

/// An effect step (operand: the effect table index).
#[derive(Clone, Copy)]
enum EffStep {
    /// Miss unless the effect can run (see [`effect_step`]).
    Check(u16),
    /// Run a checked effect.
    Apply(u16),
    /// Run a lone effect (it checks and applies under one lock).
    Once(u16),
}

/// A branch test over slots.
#[derive(Clone, Copy)]
enum Br {
    T1(IntTest1, Sl),
    T2(IntTest2, Sl, Sl),
    Eq(Sl, Sl),
    Truthy(Sl),
}

/// One instruction. Jump targets are label ids until the program
/// is laid out, code indexes after.
#[derive(Clone, Copy)]
enum Ins {
    /// The outer loop's head: re-validate the call sites.
    Top,
    Do(Step),
    Co(Co),
    Eff(EffStep),
    /// Jump unless the test holds.
    Br(Br, u32),
    Jmp(u32),
    Exit(u16),
    Miss,
}

impl Ins {
    /// Point a computing instruction that writes slot `from` at slot
    /// `to` (moves and sets may be one of an `if`'s several writers,
    /// so only single-writer steps retarget).
    fn retarget(&mut self, from: Sl, to: Sl) -> bool {
        let d = match self {
            Ins::Do(Step::Op1(_, d, _) | Step::Op2(_, d, ..) | Step::Cast(_, d, _))
            | Ins::Co(Co::Nth(d, ..) | Co::Count(d, _)) => d,
            _ => return false,
        };
        let hit = *d == from;
        if hit {
            *d = to;
        }
        hit
    }
}

/// An effect over slots and a collection index.
enum EffI {
    Assoc(u16, Sl, Sl),
    Conj(u16, Sl),
}

/// An exit's value.
enum ExitI {
    Slot(Sl),
    Const(Value),
    Loc(Loc),
}

/// A compiled scalar loop (see the module docs).
pub(crate) struct ScalarLoop {
    /// Per loop target: whether its value lives in a slot.
    scalar: Box<[bool]>,
    /// The slot file at entry: constants in place, zeros elsewhere.
    init: Box<[Sv]>,
    code: Box<[Ins]>,
    /// Invariant scalars, loaded at entry.
    loads: Box<[(Sl, Loc)]>,
    colls: Box<[Loc]>,
    effs: Box<[EffI]>,
    exits: Box<[ExitI]>,
    sites: Box<[Site]>,
    /// The site-group id the sites validate under together.
    gid: usize,
    /// Entries and iterations, for [`try_run`]'s give-up rule.
    entries: AtomicU32,
    iters: AtomicU32,
    off: AtomicBool,
}

/// Entries before a loop's average trip count is judged.
const JUDGE_AFTER: u32 = 32;

/// The average iterations per entry a loop needs to keep the tier
/// (measured: a one- or two-iteration loop costs more to enter than
/// its boxed iterations).
const MIN_TRIPS: u32 = 2;

/// A collection resolved once per run (its local cannot change
/// while the loop runs).
enum Coll<'v> {
    Ascii(&'v [u8]),
    Str(&'v std::sync::Arc<str>),
    Vec(&'v clj_value::Vector),
    Trans(&'v clj_value::Transient),
    Other,
}

fn coll_of(v: &Value) -> Coll<'_> {
    match v {
        Value::Str(s) if crate::coll::str_ascii(s) => Coll::Ascii(s.as_bytes()),
        Value::Str(s) => Coll::Str(s),
        Value::Vector(v) => Coll::Vec(v),
        Value::Transient(t) => Coll::Trans(t),
        _ => Coll::Other,
    }
}

/// The scalar at `coll[i]` (`nth`'s in-range answer, else `None`).
#[inline(always)]
fn nth(coll: &Coll<'_>, i: Sv) -> Option<Sv> {
    let idx = usize::try_from(i.int_of()?).ok()?;
    match coll {
        Coll::Ascii(b) => b.get(idx).map(|c| Sv {
            k: K::Char,
            v: i64::from(*c),
        }),
        Coll::Str(s) => crate::coll::str_char_at(s, idx).map(|c| Sv {
            k: K::Char,
            v: i64::from(u32::from(c)),
        }),
        Coll::Vec(v) => Sv::of(&v.get(idx)?),
        Coll::Trans(t) => match &*t.lock() {
            TransientKind::Vector(tv) => Sv::of(&tv.nth_opt(idx)?),
            _ => None,
        },
        Coll::Other => None,
    }
}

/// `count` of a string or (transient) vector.
fn count(coll: &Coll<'_>) -> Option<Sv> {
    let n = match coll {
        Coll::Ascii(b) => b.len(),
        Coll::Str(s) => crate::coll::str_utf16_len(s),
        Coll::Vec(v) => v.len(),
        Coll::Trans(t) => match &*t.lock() {
            TransientKind::Vector(tv) => tv.len().ok()?,
            _ => return None,
        },
        Coll::Other => return None,
    };
    i64::try_from(n).ok().map(Sv::int)
}

/// `int`/`long` of a scalar (a char's code; `int` range-checked).
#[inline(always)]
fn cast(c: Cast, x: Sv) -> Option<Sv> {
    match (c, x.k) {
        (_, K::Char) | (Cast::Long, K::Int) => Some(Sv::int(x.v)),
        (Cast::Int, K::Int) => i32::try_from(x.v).ok().map(|_| Sv::int(x.v)),
        (_, K::Bool) => None,
    }
}

/// Check (`apply` false) or perform (`apply` true) one effect:
/// `false` when it cannot run — a live transient vector is needed,
/// and an `assoc!` index inside it (an earlier `conj!` only grows
/// it, so a checked batch cannot fail midway).
fn effect_step(e: &EffI, s: &[Sv], colls: &[Coll<'_>], apply: bool) -> bool {
    let (EffI::Assoc(c, ..) | EffI::Conj(c, _)) = e;
    let Coll::Trans(t) = &colls[usize::from(*c)] else {
        return false;
    };
    let mut guard = t.lock();
    let TransientKind::Vector(tv) = &mut *guard else {
        return false;
    };
    let len = tv.len();
    match (e, len) {
        (EffI::Assoc(_, k, v), Ok(n)) => {
            match usize::try_from(s[usize::from(*k)].int_of().unwrap_or(-1)) {
                Ok(i) if i < n => !apply || tv.assoc(i, s[usize::from(*v)].value()).is_ok(),
                _ => false,
            }
        }
        (EffI::Conj(_, v), Ok(_)) => !apply || tv.conj(s[usize::from(*v)].value()).is_ok(),
        (_, Err(_)) => false,
    }
}

/// A lone effect, all or nothing: the boxed `assoc!`/`conj!` fast
/// paths (an `assoc!` at the end appends, as the native does).
#[inline(always)]
fn effect_once(e: &EffI, s: &[Sv], colls: &[Coll<'_>]) -> bool {
    let (EffI::Assoc(c, ..) | EffI::Conj(c, _)) = e;
    let Coll::Trans(t) = &colls[usize::from(*c)] else {
        return false;
    };
    let mut guard = t.lock();
    let TransientKind::Vector(tv) = &mut *guard else {
        return false;
    };
    match e {
        EffI::Assoc(_, k, v) => match (s[usize::from(*k)].int_of(), s[usize::from(*v)]) {
            (Some(i), _) if !(0..=i64::from(i32::MAX)).contains(&i) => false,
            (Some(i), Sv { k: K::Int, v: n }) => tv.assoc_int_opt(i as usize, n),
            (Some(i), v) => tv.assoc(i as usize, v.value()).is_ok(),
            (None, _) => false,
        },
        EffI::Conj(_, v) => match s[usize::from(*v)] {
            Sv { k: K::Int, v: n } => tv.conj_int_opt(n),
            v => tv.conj(v.value()).is_ok(),
        },
    }
}

#[inline(always)]
fn branch(b: Br, s: &[Sv]) -> Option<bool> {
    let at = |x: Sl| s[usize::from(x)];
    Some(match b {
        Br::T1(op, a) => op.test(at(a).int_of()?),
        Br::T2(op, a, b) => op.test(at(a).int_of()?, at(b).int_of()?),
        Br::Eq(a, b) => at(a) == at(b),
        Br::Truthy(a) => at(a).truthy(),
    })
}

#[inline(always)]
fn step(st: Step, s: &mut [Sv]) -> Option<()> {
    let at = |s: &[Sv], x: Sl| s[usize::from(x)];
    let (d, v) = match st {
        Step::Op1(op, d, a) => (d, Sv::int(op.apply(at(s, a).int_of()?)?)),
        Step::Op2(op, d, a, b) => {
            let (x, y) = (at(s, a).int_of()?, at(s, b).int_of()?);
            (d, Sv::int(op.apply(x, y)?))
        }
        Step::Cast(c, d, a) => (d, cast(c, at(s, a))?),
        Step::Mov(d, a) => (d, at(s, a)),
        Step::Set(d, b) => (d, Sv::bool(b)),
    };
    s[usize::from(d)] = v;
    Some(())
}

#[inline(always)]
fn co_step(co: Co, s: &mut [Sv], colls: &[Coll<'_>]) -> Option<()> {
    let (d, v) = match co {
        Co::Nth(d, c, i) => (d, nth(&colls[usize::from(c)], s[usize::from(i)])?),
        Co::Count(d, c) => (d, count(&colls[usize::from(c)])?),
    };
    s[usize::from(d)] = v;
    Some(())
}

/// An effect step: `false` misses (nothing has run).
#[inline(always)]
fn eff_step(st: EffStep, sc: &ScalarLoop, s: &[Sv], colls: &[Coll<'_>]) -> bool {
    match st {
        EffStep::Check(e) => effect_step(&sc.effs[usize::from(e)], s, colls, false),
        EffStep::Apply(e) => effect_step(&sc.effs[usize::from(e)], s, colls, true) || true,
        EffStep::Once(e) => effect_once(&sc.effs[usize::from(e)], s, colls),
    }
}

fn exit_value(x: &ExitI, s: &[Sv], f: &Frame<'_>) -> Value {
    match x {
        ExitI::Slot(v) => s[usize::from(*v)].value(),
        ExitI::Const(v) => v.clone(),
        ExitI::Loc(l) => f.get(*l).clone(),
    }
}

/// The loop head: count the trip while judging, and go on unless the
/// direct snapshot moved (another thread redefined a var, or this
/// one's bindings changed), which hands the loop back to the boxed
/// road, where every call validates itself.
#[inline(always)]
fn head<const COUNT: bool>(
    ev: &Evaluator,
    snap: (u64, u64),
    trips: &mut u32,
    pc: usize,
) -> Option<usize> {
    if COUNT {
        *trips += 1;
    }
    (ev.direct_snapshot() == snap).then_some(pc + 1)
}

/// Run the program from its head: the loop's value, or `None` at
/// the first miss (the state slots then hold the iteration's start).
/// `snap` is the direct snapshot the sites validated under at entry;
/// `COUNT` also counts the iterations begun (for [`judge`]).
#[inline(never)]
fn exec<const COUNT: bool>(
    sc: &ScalarLoop,
    ev: &mut Evaluator,
    f: &Frame<'_>,
    s: &mut [Sv],
    colls: &[Coll<'_>],
    snap: (u64, u64),
    trips: &mut u32,
) -> Option<Value> {
    let mut pc = 0usize;
    loop {
        let next = match sc.code[pc] {
            Ins::Do(st) => step(st, s).map(|()| pc + 1),
            Ins::Co(co) => co_step(co, s, colls).map(|()| pc + 1),
            Ins::Eff(st) => eff_step(st, sc, s, colls).then_some(pc + 1),
            Ins::Br(b, t) => branch(b, s).map(|holds| if holds { pc + 1 } else { t as usize }),
            Ins::Jmp(t) => Some(t as usize),
            Ins::Top => head::<COUNT>(ev, snap, trips, pc),
            Ins::Exit(x) => return Some(exit_value(&sc.exits[usize::from(x)], s, f)),
            Ins::Miss => None,
        };
        pc = next?;
    }
}

/// Run the loop on scalars from its bound registers: `Some` is the
/// loop's value; `None` leaves the registers at the start of the
/// iteration that missed, for the boxed tiers to resume.
#[inline(never)]
fn run(ev: &mut Evaluator, f: &mut Frame<'_>, sc: &ScalarLoop, targets: &[usize]) -> Option<Value> {
    let mut buf: SmallVec<[Sv; 16]> = SmallVec::from_slice(&sc.init);
    let s: &mut [Sv] = &mut buf;
    for (p, r) in targets.iter().enumerate() {
        if sc.scalar[p] {
            s[p] = Sv::of(&f.regs[*r])?;
        }
    }
    for (slot, l) in sc.loads.iter() {
        s[usize::from(*slot)] = Sv::of(f.get(*l))?;
    }
    // One cached verdict per evaluator for the whole site group.
    if !ev.group_valid(sc.gid, |ev| sc.sites.iter().all(|s| s.valid(ev))) {
        return None;
    }
    let snap = ev.direct_snapshot();
    let mut trips = 0u32;
    let out = {
        let fr: &Frame<'_> = f;
        let mut colls: SmallVec<[Coll<'_>; 2]> = SmallVec::new();
        for l in sc.colls.iter() {
            colls.push(coll_of(fr.get(*l)));
        }
        // Trips are counted only while the loop is being judged.
        if sc.entries.load(Ordering::Relaxed) < JUDGE_AFTER {
            let out = exec::<true>(sc, ev, fr, s, &colls, snap, &mut trips);
            judge(sc, trips);
            out
        } else {
            exec::<false>(sc, ev, fr, s, &colls, snap, &mut trips)
        }
    };
    if out.is_none() {
        for (p, r) in targets.iter().enumerate() {
            if sc.scalar[p] {
                f.regs[*r] = s[p].value();
            }
        }
    }
    out
}

/// Count a run; after [`JUDGE_AFTER`] entries a loop averaging under
/// [`MIN_TRIPS`] iterations per entry stops entering the tier (its
/// setup costs more than the boxed iterations it would replace).
fn judge(sc: &ScalarLoop, trips: u32) {
    let entries = sc.entries.fetch_add(1, Ordering::Relaxed) + 1;
    let iters = sc
        .iters
        .fetch_add(trips, Ordering::Relaxed)
        .saturating_add(trips);
    if entries == JUDGE_AFTER && iters < MIN_TRIPS * JUDGE_AFTER {
        sc.off.store(true, Ordering::Relaxed);
    }
}

/// [`run`] when the loop has a scalar program still in use.
pub(crate) fn try_run(
    ev: &mut Evaluator,
    f: &mut Frame<'_>,
    sc: Option<&ScalarLoop>,
    targets: &[usize],
) -> Option<Value> {
    let sc = sc.filter(|sc| !sc.off.load(Ordering::Relaxed))?;
    run(ev, f, sc, targets)
}

/// Where a lowered leaf jumps back to.
enum Ctx<'a> {
    Outer,
    Inner {
        slots: &'a [usize],
        head: u32,
        dst: Sl,
        after: u32,
    },
}

/// Lowers a body tree to straight-line code (see [`Ins`]). An
/// unsupported form lowers to [`Ins::Miss`] where it would run
/// (control never passes a miss, so the code after it is dead).
struct Lower<'b> {
    code: Vec<Ins>,
    nslots: usize,
    consts: Vec<(Sl, Sv)>,
    loads: Vec<(Sl, Loc)>,
    colls: Vec<Loc>,
    effs: Vec<EffI>,
    exits: Vec<ExitI>,
    labels: Vec<u32>,
    scalar: &'b [bool],
    targets: &'b [usize],
}

impl Lower<'_> {
    /// A fresh slot (indexes wrap past `Sl::MAX`; [`compile`]
    /// rejects such programs).
    fn sl(&mut self) -> Sl {
        let s = self.nslots;
        self.nslots += 1;
        s as Sl
    }

    fn label(&mut self) -> u32 {
        self.labels.push(u32::MAX);
        (self.labels.len() - 1) as u32
    }

    fn place(&mut self, l: u32) {
        self.labels[l as usize] = self.code.len() as u32;
    }

    fn emit(&mut self, st: Step) {
        self.code.push(Ins::Do(st));
    }

    fn coll(&mut self, l: Loc) -> u16 {
        let i = self
            .colls
            .iter()
            .position(|c| loc_eq(*c, l))
            .unwrap_or_else(|| {
                self.colls.push(l);
                self.colls.len() - 1
            });
        i as u16
    }

    fn load(&mut self, l: Loc) -> Sl {
        if let Some((s, _)) = self.loads.iter().find(|(_, c)| loc_eq(*c, l)) {
            return *s;
        }
        let s = self.sl();
        self.loads.push((s, l));
        s
    }

    /// A constant's slot (one per distinct constant).
    fn konst(&mut self, v: Sv) -> Sl {
        if let Some((s, _)) = self.consts.iter().find(|(_, c)| *c == v) {
            return *s;
        }
        let s = self.sl();
        self.consts.push((s, v));
        s
    }

    /// A miss here; the slot it answers is never read.
    fn miss(&mut self) -> Sl {
        self.code.push(Ins::Miss);
        0
    }

    /// The slot holding `e`'s value (its code emitted).
    fn val(&mut self, e: &IExpr) -> Sl {
        match e {
            IExpr::Slot(i) => *i as Sl,
            IExpr::Kst(v) => self.konst(*v),
            IExpr::Loc(l) => self.load(*l),
            IExpr::Calc(c) => self.calc_val(c),
            IExpr::Ctl(c) => self.ctl_val(c),
            IExpr::Deopt => self.miss(),
        }
    }

    /// A computing instruction: operands first, then its result
    /// slot (the last allocated, which [`Self::val_to`] relies on).
    fn calc_val(&mut self, c: &Calc) -> Sl {
        let ins = match c {
            Calc::Op1(op, a) => {
                let a = self.val(a);
                Ins::Do(Step::Op1(*op, self.sl(), a))
            }
            Calc::Op2(op, a, b) => {
                let (a, b) = (self.val(a), self.val(b));
                Ins::Do(Step::Op2(*op, self.sl(), a, b))
            }
            Calc::Cast(k, a) => {
                let a = self.val(a);
                Ins::Do(Step::Cast(*k, self.sl(), a))
            }
            Calc::Nth(l, i) => {
                let (c, i) = (self.coll(*l), self.val(i));
                Ins::Co(Co::Nth(self.sl(), c, i))
            }
            Calc::Count(l) => {
                let c = self.coll(*l);
                Ins::Co(Co::Count(self.sl(), c))
            }
        };
        self.code.push(ins);
        (self.nslots - 1) as Sl
    }

    fn ctl_val(&mut self, c: &Ctl) -> Sl {
        match c {
            Ctl::If(c, a, b) => self.if_val(c, a, b),
            Ctl::Let(binds, body) => {
                self.binds(binds);
                self.val(body)
            }
            Ctl::Loop(inner) => {
                let d = self.sl();
                self.inner(inner, d);
                d
            }
            Ctl::Test(c) => self.test_val(c),
        }
    }

    fn if_val(&mut self, c: &Cond, a: &IExpr, b: &IExpr) -> Sl {
        let d = self.sl();
        let (els, end) = (self.label(), self.label());
        self.cond(c, els);
        self.val_to(a, d);
        self.code.push(Ins::Jmp(end));
        self.place(els);
        self.val_to(b, d);
        self.place(end);
        d
    }

    fn test_val(&mut self, c: &Cond) -> Sl {
        let d = self.sl();
        let (no, end) = (self.label(), self.label());
        self.cond(c, no);
        self.emit(Step::Set(d, true));
        self.code.push(Ins::Jmp(end));
        self.place(no);
        self.emit(Step::Set(d, false));
        self.place(end);
        d
    }

    /// Lower `e` writing its value into slot `d` (a computing step
    /// targets `d` itself instead of a temp plus a move).
    fn val_to(&mut self, e: &IExpr, d: Sl) {
        let mark = self.code.len();
        let v = self.val(e);
        let fresh = self.code.len() > mark && usize::from(v) + 1 == self.nslots;
        let moved = fresh && self.code.last_mut().is_some_and(|i| i.retarget(v, d));
        if moved {
            self.nslots -= 1;
        } else {
            self.emit(Step::Mov(d, v));
        }
    }

    fn binds(&mut self, binds: &[(usize, IExpr)]) {
        for (slot, e) in binds {
            self.val_to(e, *slot as Sl);
        }
    }

    /// Fall through when `c` holds, else jump to label `no`.
    fn cond(&mut self, c: &Cond, no: u32) {
        match c {
            Cond::Leaf(cmp) => {
                let br = self.cmp(cmp);
                self.code.push(Ins::Br(br, no));
            }
            Cond::Not(c) => {
                let yes = self.label();
                self.cond(c, yes);
                self.code.push(Ins::Jmp(no));
                self.place(yes);
            }
            Cond::And(a, b) => {
                self.cond(a, no);
                self.cond(b, no);
            }
            Cond::Or(a, b) => {
                let (second, yes) = (self.label(), self.label());
                self.cond(a, second);
                self.code.push(Ins::Jmp(yes));
                self.place(second);
                self.cond(b, no);
                self.place(yes);
            }
        }
    }

    fn cmp(&mut self, c: &Cmp) -> Br {
        match c {
            Cmp::Truthy(e) => Br::Truthy(self.val(e)),
            Cmp::T1(op, e) => Br::T1(*op, self.val(e)),
            Cmp::T2(op, a, b) => {
                let a = self.val(a);
                Br::T2(*op, a, self.val(b))
            }
            Cmp::Eq(a, b) => {
                let a = self.val(a);
                Br::Eq(a, self.val(b))
            }
        }
    }

    fn node(&mut self, n: &Node, ctx: &Ctx<'_>) {
        match n {
            Node::If(c, a, b) => {
                let els = self.label();
                self.cond(c, els);
                self.node(a, ctx);
                self.place(els);
                self.node(b, ctx);
            }
            Node::Let(binds, body) => {
                self.binds(binds);
                self.node(body, ctx);
            }
            Node::Leaf(Leaf::Recur(effects, ops)) => self.recur(effects, ops, ctx),
            Node::Leaf(Leaf::Exit(effects, val)) => self.exit(effects, val, ctx),
            Node::Deopt => {
                self.miss();
            }
        }
    }

    /// An effect's operand slots into the effect table.
    fn effects(&mut self, effects: &[Effect]) -> Vec<u16> {
        let mut ids = Vec::with_capacity(effects.len());
        for e in effects {
            let eff = match e {
                Effect::Assoc(t, k, v) => {
                    let c = self.coll(*t);
                    let k = self.val(k);
                    EffI::Assoc(c, k, self.val(v))
                }
                Effect::Conj(t, v) => {
                    let c = self.coll(*t);
                    EffI::Conj(c, self.val(v))
                }
            };
            self.effs.push(eff);
            ids.push((self.effs.len() - 1) as u16);
        }
        ids
    }

    fn run_effects(&mut self, ids: &[u16]) {
        if let [id] = ids {
            return self.code.push(Ins::Eff(EffStep::Once(*id)));
        }
        for id in ids {
            self.code.push(Ins::Eff(EffStep::Check(*id)));
        }
        for id in ids {
            self.code.push(Ins::Eff(EffStep::Apply(*id)));
        }
    }

    fn recur(&mut self, effects: &[Effect], ops: &[Option<IExpr>], ctx: &Ctx<'_>) {
        let targets: Vec<usize> = match ctx {
            Ctx::Outer => (0..ops.len()).collect(),
            Ctx::Inner { slots, .. } => slots.to_vec(),
        };
        let mut vals = Vec::with_capacity(ops.len());
        for op in ops {
            vals.push(op.as_ref().map(|e| {
                let v = self.val(e);
                // A target read must survive the earlier writes.
                if targets.contains(&usize::from(v)) {
                    let t = self.sl();
                    self.emit(Step::Mov(t, v));
                    t
                } else {
                    v
                }
            }));
        }
        let ids = self.effects(effects);
        self.run_effects(&ids);
        for (t, v) in targets.iter().zip(vals) {
            if let Some(v) = v {
                self.emit(Step::Mov(*t as Sl, v));
            }
        }
        self.code.push(Ins::Jmp(match ctx {
            Ctx::Outer => 0,
            Ctx::Inner { head, .. } => *head,
        }));
    }

    fn exit(&mut self, effects: &[Effect], val: &ExitVal, ctx: &Ctx<'_>) {
        if let Ctx::Inner { dst, after, .. } = ctx {
            let e = exit_expr(val).next().unwrap_or(&IExpr::Deopt);
            self.val_to(e, *dst);
            self.code.push(Ins::Jmp(*after));
            return;
        }
        let x = match val {
            ExitVal::Expr(e) => ExitI::Slot(self.val(e)),
            ExitVal::Const(v) => ExitI::Const(v.clone()),
            ExitVal::Box(l) => ExitI::Loc(*l),
            ExitVal::Var(p) if self.scalar[*p] => ExitI::Slot(*p as Sl),
            ExitVal::Var(p) => ExitI::Loc(Loc::Reg(self.targets[*p])),
        };
        let ids = self.effects(effects);
        self.run_effects(&ids);
        self.exits.push(x);
        self.code.push(Ins::Exit((self.exits.len() - 1) as u16));
    }

    fn inner(&mut self, inner: &Inner, dst: Sl) {
        for (slot, e) in inner.slots.iter().zip(inner.inits.iter()) {
            self.val_to(e, *slot as Sl);
        }
        let (head, after) = (self.label(), self.label());
        self.place(head);
        let ctx = Ctx::Inner {
            slots: &inner.slots,
            head,
            dst,
            after,
        };
        self.node(&inner.body, &ctx);
        self.place(after);
    }

    /// Resolve label ids to code indexes (label 0 is the head).
    fn finish(mut self) -> Vec<Ins> {
        for ins in &mut self.code {
            if let Ins::Br(_, t) | Ins::Jmp(t) = ins {
                *t = self.labels[*t as usize];
            }
        }
        self.code
    }
}

/// `Loc` equality (the enum derives no `PartialEq`).
fn loc_eq(a: Loc, b: Loc) -> bool {
    matches!((a, b), (Loc::Reg(x), Loc::Reg(y)) | (Loc::Up(x), Loc::Up(y)) | (Loc::Rec(x), Loc::Rec(y)) if x == y)
}

/// What the compiler shares with the builder: local resolution and
/// the call-site records.
pub(crate) trait Host {
    fn resolve_local(&mut self, idx: usize) -> Loc;
    /// Whether `expr` may read local `g` (`vm_live::may_read` over
    /// the compiling scope).
    fn reads(&self, expr: &Expr, g: usize) -> bool;
    fn site(&self, call: &DirectCall) -> Site;
}

/// The program builder over one `loop`'s body.
struct Builder<'h> {
    host: &'h mut dyn Host,
    /// The first lexical index bound inside the body.
    body_base: usize,
    /// Body-bound lexical indexes (minus `body_base`) to slots.
    lex: Vec<usize>,
    nslots: usize,
    targets: Vec<usize>,
    scalar: Vec<bool>,
    /// Loop variables read as collections or boxed values.
    boxed: Vec<bool>,
    sites: Vec<Site>,
    /// The innermost loop's target slots (`None` for the outer
    /// loop, whose targets are slots `0..n`).
    inner: Option<Vec<usize>>,
}

/// Compile `body` (one `loop`'s forms) over `targets`, whose
/// bindings are the lexical indexes just below `body_base`: `None`
/// when no iteration could run on scalars.
pub(crate) fn compile(
    host: &mut dyn Host,
    body: &[Expr],
    targets: &[usize],
    body_base: usize,
) -> Option<ScalarLoop> {
    let n = targets.len();
    let mut b = Builder {
        host,
        body_base,
        lex: Vec::new(),
        nslots: n,
        targets: targets.to_vec(),
        scalar: vec![false; n],
        boxed: vec![false; n],
        sites: Vec::new(),
        inner: None,
    };
    let node = b.body(body);
    let clash = b.scalar.iter().zip(&b.boxed).any(|(s, x)| *s && *x);
    if clash || !productive(&node) {
        return None;
    }
    let mut lw = Lower {
        code: vec![Ins::Top],
        nslots: b.nslots,
        consts: Vec::new(),
        loads: Vec::new(),
        colls: Vec::new(),
        effs: Vec::new(),
        exits: Vec::new(),
        labels: vec![0],
        scalar: &b.scalar,
        targets: &b.targets,
    };
    lw.node(&node, &Ctx::Outer);
    (lw.nslots < usize::from(Sl::MAX)).then_some(())?;
    let mut init = vec![Sv::ZERO; lw.nslots];
    for (slot, v) in &lw.consts {
        init[usize::from(*slot)] = *v;
    }
    let loads = std::mem::take(&mut lw.loads);
    let (colls, effs) = (std::mem::take(&mut lw.colls), std::mem::take(&mut lw.effs));
    let exits = std::mem::take(&mut lw.exits);
    let code = lw.finish();
    Some(ScalarLoop {
        scalar: b.scalar.into(),
        init: init.into(),
        code: code.into(),
        loads: loads.into(),
        colls: colls.into(),
        effs: effs.into(),
        exits: exits.into(),
        sites: b.sites.into(),
        gid: crate::vm::next_site(),
        entries: AtomicU32::new(0),
        iters: AtomicU32::new(0),
        off: AtomicBool::new(false),
    })
}

/// Whether some `recur` leaf can run on scalars (a body that
/// would miss on its first iteration is not worth entering).
fn productive(n: &Node) -> bool {
    match n {
        Node::If(c, a, b) => !cond_misses(c) && (productive(a) || productive(b)),
        Node::Let(binds, body) => !binds.iter().any(|(_, e)| misses(e)) && productive(body),
        Node::Leaf(Leaf::Recur(effects, ops)) => {
            !ops.iter().flatten().any(misses) && !effects.iter().any(effect_misses)
        }
        Node::Leaf(Leaf::Exit(..)) | Node::Deopt => false,
    }
}

fn effect_misses(e: &Effect) -> bool {
    match e {
        Effect::Assoc(_, k, v) => misses(k) || misses(v),
        Effect::Conj(_, v) => misses(v),
    }
}

/// A computing expression.
fn calc(c: Calc) -> IExpr {
    IExpr::Calc(Box::new(c))
}

/// Whether evaluating `e` always misses.
fn misses(e: &IExpr) -> bool {
    match e {
        IExpr::Deopt => true,
        IExpr::Calc(c) => calc_misses(c),
        IExpr::Ctl(c) => ctl_misses(c),
        _ => false,
    }
}

fn calc_misses(c: &Calc) -> bool {
    match c {
        Calc::Op1(_, a) | Calc::Cast(_, a) | Calc::Nth(_, a) => misses(a),
        Calc::Op2(_, a, b) => misses(a) || misses(b),
        Calc::Count(_) => false,
    }
}

fn ctl_misses(c: &Ctl) -> bool {
    match c {
        Ctl::If(c, a, b) => cond_misses(c) || (misses(a) && misses(b)),
        Ctl::Let(binds, body) => binds.iter().any(|(_, e)| misses(e)) || misses(body),
        Ctl::Test(c) => cond_misses(c),
        Ctl::Loop(_) => false,
    }
}

fn cond_misses(c: &Cond) -> bool {
    match c {
        Cond::Leaf(Cmp::Truthy(e) | Cmp::T1(_, e)) => misses(e),
        Cond::Leaf(Cmp::T2(_, a, b) | Cmp::Eq(a, b)) => misses(a) || misses(b),
        Cond::Not(c) => cond_misses(c),
        Cond::And(a, b) | Cond::Or(a, b) => cond_misses(a) || cond_misses(b),
    }
}

/// Whether `e` reads a collection (an effect's operands and the
/// `recur` beside it run before the effects, which a collection
/// read could observe out of order).
fn reads_coll(e: &IExpr) -> bool {
    match e {
        IExpr::Calc(c) => match &**c {
            Calc::Nth(..) | Calc::Count(_) => true,
            Calc::Op1(_, a) | Calc::Cast(_, a) => reads_coll(a),
            Calc::Op2(_, a, b) => reads_coll(a) || reads_coll(b),
        },
        IExpr::Ctl(c) => ctl_reads(c),
        _ => false,
    }
}

fn ctl_reads(c: &Ctl) -> bool {
    match c {
        Ctl::If(c, a, b) => cond_reads(c) || reads_coll(a) || reads_coll(b),
        Ctl::Let(binds, body) => binds.iter().any(|(_, e)| reads_coll(e)) || reads_coll(body),
        Ctl::Test(c) => cond_reads(c),
        Ctl::Loop(_) => true,
    }
}

fn cond_reads(c: &Cond) -> bool {
    match c {
        Cond::Leaf(Cmp::Truthy(e) | Cmp::T1(_, e)) => reads_coll(e),
        Cond::Leaf(Cmp::T2(_, a, b) | Cmp::Eq(a, b)) => reads_coll(a) || reads_coll(b),
        Cond::Not(c) => cond_reads(c),
        Cond::And(a, b) | Cond::Or(a, b) => cond_reads(a) || cond_reads(b),
    }
}

impl Builder<'_> {
    fn slot(&mut self) -> usize {
        let s = self.nslots;
        self.nslots += 1;
        s
    }

    /// The next body-bound lexical index.
    fn next_index(&self) -> usize {
        self.body_base + self.lex.len()
    }

    /// A local read: a body-bound slot, an outer loop variable (now
    /// scalar), or an invariant in place.
    fn local(&mut self, idx: usize) -> IExpr {
        if idx >= self.body_base {
            return IExpr::Slot(self.lex[idx - self.body_base]);
        }
        match self.var(idx) {
            Ok(p) => {
                self.scalar[p] = true;
                IExpr::Slot(p)
            }
            Err(loc) => IExpr::Loc(loc),
        }
    }

    /// An outer loop variable's position, else the local's place.
    fn var(&mut self, idx: usize) -> Result<usize, Loc> {
        let loc = self.host.resolve_local(idx);
        match loc {
            Loc::Reg(r) => self.targets.iter().position(|t| *t == r).ok_or(loc),
            other => Err(other),
        }
    }

    /// An invariant or passthrough local read boxed (a collection or
    /// effect target): `None` for a body-bound local.
    fn boxed_local(&mut self, e: &Expr) -> Option<Loc> {
        let Expr::Local(idx) = e else {
            return None;
        };
        if *idx >= self.body_base {
            return None;
        }
        match self.var(*idx) {
            Ok(p) => {
                self.boxed[p] = true;
                Some(Loc::Reg(self.targets[p]))
            }
            Err(loc) => Some(loc),
        }
    }

    fn expr(&mut self, e: &Expr) -> IExpr {
        match e {
            Expr::Literal(v) => Sv::of(v).map_or(IExpr::Deopt, IExpr::Kst),
            Expr::Local(idx) => self.local(*idx),
            Expr::DirectNative(call) => self.call(call),
            Expr::If { test, then, els } => {
                let c = self.cond(test);
                let a = self.expr(then);
                IExpr::Ctl(Box::new(Ctl::If(c, a, self.expr(els))))
            }
            Expr::Let { bindings, body } => self.let_expr(bindings, body),
            Expr::Loop { bindings, body } => self.inner_loop(bindings, body),
            Expr::Do(forms) if forms.len() == 1 => self.expr(&forms[0]),
            _ => IExpr::Deopt,
        }
    }

    /// `let` bindings (symbols only) into fresh slots, visible to
    /// the later inits and `then` (the caller truncates `lex`).
    fn bindings(&mut self, bindings: &[(Pattern, Expr)]) -> Option<Vec<(usize, IExpr)>> {
        let mut out = Vec::with_capacity(bindings.len());
        for (pat, init) in bindings {
            if !matches!(pat, Pattern::Symbol(_)) {
                return None;
            }
            let e = self.expr(init);
            let s = self.slot();
            self.lex.push(s);
            out.push((s, e));
        }
        Some(out)
    }

    fn let_expr(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> IExpr {
        let mark = self.lex.len();
        let out = match (self.bindings(bindings), body) {
            (Some(binds), [last]) => IExpr::Ctl(Box::new(Ctl::Let(binds.into(), self.expr(last)))),
            _ => IExpr::Deopt,
        };
        self.lex.truncate(mark);
        out
    }

    fn inner_loop(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> IExpr {
        let mark = self.lex.len();
        let out = match self.bindings(bindings) {
            Some(binds) => {
                let (slots, inits): (Vec<usize>, Vec<IExpr>) = binds.into_iter().unzip();
                let saved = self.inner.replace(slots.clone());
                let body = self.body(body);
                self.inner = saved;
                IExpr::Ctl(Box::new(Ctl::Loop(Inner {
                    slots: slots.into(),
                    inits: inits.into(),
                    body,
                })))
            }
            None => IExpr::Deopt,
        };
        self.lex.truncate(mark);
        out
    }

    /// A direct core call in value position.
    fn call(&mut self, call: &DirectCall) -> IExpr {
        let e = self.call_expr(call);
        if !matches!(e, IExpr::Deopt) {
            let site = self.host.site(call);
            self.sites.push(site);
        }
        e
    }

    fn call_expr(&mut self, call: &DirectCall) -> IExpr {
        let args = &call.args;
        if let Some(op) = crate::vm_ops::int_value_op(call) {
            return match op {
                IntCtor::One(op) => calc(Calc::Op1(op, self.expr(&args[0]))),
                IntCtor::Two(op) => {
                    let a = self.expr(&args[0]);
                    calc(Calc::Op2(op, a, self.expr(&args[1])))
                }
            };
        }
        if let Some(op) = crate::vm_ops::int_fold_op(call) {
            return self.fold(op, args);
        }
        match (crate::vm_ops::core_name(call), args.as_slice()) {
            (Some("nth"), [coll, i]) => match self.boxed_local(coll) {
                Some(loc) => calc(Calc::Nth(loc, self.expr(i))),
                None => IExpr::Deopt,
            },
            (Some("count"), [coll]) => self
                .boxed_local(coll)
                .map_or(IExpr::Deopt, |l| calc(Calc::Count(l))),
            (Some("int"), [x]) => calc(Calc::Cast(Cast::Int, self.expr(x))),
            (Some("long"), [x]) => calc(Calc::Cast(Cast::Long, self.expr(x))),
            _ => match self.test_call(call) {
                Some(c) => IExpr::Ctl(Box::new(Ctl::Test(c))),
                None => IExpr::Deopt,
            },
        }
    }

    /// An n-ary op as its left fold (see `vm_ops::int_fold_op`).
    fn fold(&mut self, op: IntOp2, args: &[Expr]) -> IExpr {
        let mut acc = self.expr(&args[0]);
        for a in &args[1..] {
            acc = calc(Calc::Op2(op, acc, self.expr(a)));
        }
        acc
    }

    /// A test: `and`/`or` shapes, comparison calls, else truthiness.
    fn cond(&mut self, e: &Expr) -> Cond {
        if let Some((a, b, and)) = crate::vm::and_or_shape(e, self.next_index()) {
            if !self.host.reads(b, self.next_index()) {
                let a = Box::new(self.cond(a));
                // `b` is analyzed with the shape's local bound.
                let mark = self.lex.len();
                let g = self.slot();
                self.lex.push(g);
                let b = Box::new(self.cond(b));
                self.lex.truncate(mark);
                return if and { Cond::And(a, b) } else { Cond::Or(a, b) };
            }
        }
        if let Expr::DirectNative(call) = e {
            if let Some(c) = self.test_call(call) {
                let site = self.host.site(call);
                self.sites.push(site);
                return c;
            }
        }
        Cond::Leaf(Cmp::Truthy(self.expr(e)))
    }

    /// A comparison or predicate call as a test (its site is the
    /// caller's to record).
    fn test_call(&mut self, call: &DirectCall) -> Option<Cond> {
        let args = &call.args;
        match (crate::vm_ops::core_name(call)?, args.as_slice()) {
            ("=", [a, b]) => return Some(Cond::Leaf(Cmp::Eq(self.expr(a), self.expr(b)))),
            ("not=", [a, b]) => {
                let eq = Cond::Leaf(Cmp::Eq(self.expr(a), self.expr(b)));
                return Some(Cond::Not(Box::new(eq)));
            }
            ("not", [a]) => return Some(Cond::Not(Box::new(self.cond(a)))),
            _ => {}
        }
        Some(match crate::vm_ops::int_test_op(call)? {
            IntTestCtor::One(op) => Cond::Leaf(Cmp::T1(op, self.expr(&args[0]))),
            IntTestCtor::Two(op) => {
                let a = self.expr(&args[0]);
                Cond::Leaf(Cmp::T2(op, a, self.expr(&args[1])))
            }
        })
    }

    /// A body (forms of a `loop`, `let`, or `do`): effects, then a
    /// tail node.
    fn body(&mut self, forms: &[Expr]) -> Node {
        let Some((last, init)) = forms.split_last() else {
            return Node::Deopt;
        };
        if init.is_empty() {
            return self.node(last);
        }
        let effects: Option<Vec<Effect>> = init.iter().map(|e| self.effect(e)).collect();
        match effects {
            Some(effects) => self.leaf(effects, last),
            None => Node::Deopt,
        }
    }

    fn node(&mut self, e: &Expr) -> Node {
        match e {
            Expr::If { test, then, els } => {
                let c = self.cond(test);
                let a = Box::new(self.node(then));
                Node::If(c, a, Box::new(self.node(els)))
            }
            Expr::Let { bindings, body } => {
                let mark = self.lex.len();
                let out = match self.bindings(bindings) {
                    Some(binds) => Node::Let(binds.into(), Box::new(self.body(body))),
                    None => Node::Deopt,
                };
                self.lex.truncate(mark);
                out
            }
            Expr::Do(forms) => self.body(forms),
            other => self.leaf(Vec::new(), other),
        }
    }

    /// An effect form: `(assoc! t k v)` or `(conj! t v)` over a boxed
    /// local (outer loop only).
    fn effect(&mut self, e: &Expr) -> Option<Effect> {
        let Expr::DirectNative(call) = e else {
            return None;
        };
        if self.inner.is_some() {
            return None;
        }
        let effect = match (crate::vm_ops::core_name(call)?, call.args.as_slice()) {
            ("assoc!", [t, k, v]) => {
                let t = self.boxed_local(t)?;
                Effect::Assoc(t, self.expr(k), self.expr(v))
            }
            ("conj!", [t, v]) => Effect::Conj(self.boxed_local(t)?, self.expr(v)),
            _ => return None,
        };
        let site = self.host.site(call);
        self.sites.push(site);
        Some(effect)
    }

    /// The iteration's end: a `recur`, or an exit value.
    fn leaf(&mut self, effects: Vec<Effect>, e: &Expr) -> Node {
        match e {
            Expr::Recur(args) => match self.recur(args) {
                Some(ops) if ordered(&effects, ops.iter().flatten()) => {
                    Node::Leaf(Leaf::Recur(effects.into(), ops.into()))
                }
                _ => Node::Deopt,
            },
            other => {
                let mut effects = effects;
                // `(assoc! t …)`/`(conj! t …)` answers the transient.
                let val = match self.effect(other) {
                    Some(eff) => {
                        let (Effect::Assoc(t, ..) | Effect::Conj(t, _)) = eff;
                        effects.push(eff);
                        Some(ExitVal::Box(t))
                    }
                    None => self.exit(other),
                };
                match val {
                    Some(val) if ordered(&effects, exit_expr(&val)) => {
                        Node::Leaf(Leaf::Exit(effects.into(), val))
                    }
                    _ => Node::Deopt,
                }
            }
        }
    }

    /// A `recur`'s operands (`None` for a passthrough variable).
    fn recur(&mut self, args: &[Expr]) -> Option<Vec<Option<IExpr>>> {
        let slots = self.inner.clone();
        let mut ops = Vec::with_capacity(args.len());
        for (p, a) in args.iter().enumerate() {
            ops.push(match (&slots, a) {
                (None, Expr::Local(idx))
                    if *idx < self.body_base && matches!(self.var(*idx), Ok(q) if q == p) =>
                {
                    None
                }
                (None, a) => match self.expr(a) {
                    IExpr::Deopt => return None,
                    e => {
                        self.scalar[p] = true;
                        Some(e)
                    }
                },
                (Some(_), a) => Some(self.expr(a)),
            });
        }
        Some(ops)
    }

    /// An exit value: a literal, an outer loop variable, a boxed
    /// local, or a scalar expression (an inner loop's must be one).
    fn exit(&mut self, e: &Expr) -> Option<ExitVal> {
        if self.inner.is_some() {
            return Some(ExitVal::Expr(self.expr(e)));
        }
        Some(match e {
            Expr::Literal(v) => ExitVal::Const(v.clone()),
            Expr::Local(idx) if *idx < self.body_base => match self.var(*idx) {
                Ok(p) => ExitVal::Var(p),
                Err(loc) => ExitVal::Box(loc),
            },
            other => ExitVal::Expr(self.expr(other)),
        })
    }
}

/// Whether a leaf's effects may run after its other values: no
/// `recur`/exit value, and no operand of a second effect, reads a
/// collection (an earlier effect could change it, and those values
/// compute before any effect runs).
fn ordered<'e>(effects: &[Effect], mut values: impl Iterator<Item = &'e IExpr>) -> bool {
    // A lone effect's operands run before it on either road.
    let operand_reads = effects.len() > 1
        && effects.iter().any(|x| match x {
            Effect::Assoc(_, k, v) => reads_coll(k) || reads_coll(v),
            Effect::Conj(_, v) => reads_coll(v),
        });
    effects.is_empty() || !(operand_reads || values.any(reads_coll))
}

/// An exit value's scalar expression, if any.
fn exit_expr(v: &ExitVal) -> std::option::IntoIter<&IExpr> {
    match v {
        ExitVal::Expr(e) => Some(e),
        _ => None,
    }
    .into_iter()
}
