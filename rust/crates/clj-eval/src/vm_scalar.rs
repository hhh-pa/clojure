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
//! place, and performs `assoc!`/`conj!`/`pop!` on transient vectors
//! where the source puts them: as statements anywhere in a body,
//! under `if`/`when`, and inside nested loops. Calls through vars
//! holding closures inline (the program enters only while each var
//! still holds its closure, see [`RootSite`]).
//!
//! Every operation answers exactly what its boxed op answers, or
//! nothing: a non-scalar read, an overflow, an out-of-range
//! index, a non-ASCII-safe miss, an invalid call site, or an
//! unsupported form. Everything runs in source order, and every
//! effect logs how to undo itself ([`Undo`]); the log clears at
//! each iteration's head. A miss undoes the iteration's effects,
//! newest first, writes the iteration's starting state back to the
//! registers, and hands the loop to the boxed tiers, which re-run
//! that iteration whole. Nothing else observes a transient while an
//! iteration runs (the program calls no code of its own), so results,
//! errors, evaluation order and redefinition behavior all come from
//! the ordinary road.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use std::sync::Arc;

use clj_value::{ExtensionValue, Transient, TransientKind, TransientVector, Value, Var};
use smallvec::SmallVec;

use crate::eval::Evaluator;
use crate::expr::{Arity, DirectCall, Expr, Pattern, VecPattern};
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
#[derive(Clone)]
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
#[derive(Clone)]
pub(crate) enum Calc {
    Op1(IntOp1, IExpr),
    Op2(IntOp2, IExpr, IExpr),
    Cast(Cast, IExpr),
    /// `(nth coll i)` over an invariant collection.
    Nth(Loc, IExpr),
    /// `(count coll)` over an invariant collection.
    Count(Loc),
    /// Item `k` of the vector at `coll[i]`: a name of a
    /// destructured `(let [[a b …] (nth coll i)] …)`.
    Nth2(Loc, IExpr, u16),
}

/// A control form in value position.
#[derive(Clone)]
pub(crate) enum Ctl {
    If(Cond, IExpr, IExpr),
    Let(Box<[(usize, IExpr)]>, IExpr),
    /// Statements, then the value (`(do stmt… x)`).
    Seq(Box<[Stmt]>, IExpr),
    /// A nested loop's value.
    Loop(Inner),
    /// A test as a boolean value.
    Test(Cond),
}

/// A branch test.
#[derive(Clone)]
pub(crate) enum Cond {
    Leaf(Cmp),
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

/// A comparison or truth test.
#[derive(Clone)]
pub(crate) enum Cmp {
    Truthy(IExpr),
    T1(IntTest1, IExpr),
    /// `<`/`<=`/`>`/`>=`/`==` over `Int`s.
    T2(IntTest2, IExpr, IExpr),
    /// `=` over two scalars of any kinds.
    Eq(IExpr, IExpr),
}

/// A transient vector mutation.
#[derive(Clone)]
pub(crate) enum Effect {
    /// `(assoc! t k v)` (`k` at most the count, which appends).
    Assoc(Loc, IExpr, IExpr),
    /// `(conj! t v)`.
    Conj(Loc, IExpr),
    /// `(pop! t)`.
    Pop(Loc),
}

/// A form run for its effect, in source order.
#[derive(Clone)]
pub(crate) enum Stmt {
    Effect(Effect),
    /// `(if test then… else…)` over statements (`when`, `when-not`).
    If(Cond, Box<[Stmt]>, Box<[Stmt]>),
    Let(Box<[(usize, IExpr)]>, Box<[Stmt]>),
    /// A nested loop run for effect (its value dropped).
    Loop(Inner),
    /// A scalar expression computed for its misses (value dropped).
    Expr(IExpr),
}

/// A loop's exit value.
#[derive(Clone)]
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
#[derive(Clone)]
pub(crate) enum Leaf {
    /// `recur`: one operand per target (`None` passes the
    /// variable through unchanged).
    Recur(Box<[Option<IExpr>]>),
    Exit(ExitVal),
}

/// A loop body: a tree of tests, temps and statements ending in
/// leaves.
#[derive(Clone)]
pub(crate) enum Node {
    If(Cond, Box<Node>, Box<Node>),
    Let(Box<[(usize, IExpr)]>, Box<Node>),
    /// Statements, then the rest of the body.
    Seq(Box<[Stmt]>, Box<Node>),
    Leaf(Leaf),
    Deopt,
}

/// A nested loop used as a value.
#[derive(Clone)]
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
    /// Result, collection index, index slot, item.
    Nth2(Sl, u16, Sl, u16),
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
    /// The outer loop's head: re-validate the call sites (and clear
    /// the undo log: the last iteration's effects stand).
    Top,
    Do(Step),
    Co(Co),
    /// Run an effect (the effect table index), logging its undo
    /// unless nothing after it can miss before the iteration ends
    /// (see [`mark_unlogged`]).
    Eff(u16, bool),
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
            | Ins::Co(Co::Nth(d, ..) | Co::Count(d, _) | Co::Nth2(d, ..)) => d,
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
    Pop(u16),
}

/// How to undo one applied effect (plain data: a replaced or popped
/// item that is not an `Int` waits in [`Log::boxed`]).
#[derive(Clone, Copy)]
enum UndoOp {
    /// Drop this many appended items.
    Pops(usize),
    /// Put the replaced item back.
    Put(usize, Old),
    /// Append the popped item back.
    Push(Old),
}

/// An item an undo puts back: an `Int`, or a boxed item's index in
/// [`Log::boxed`].
#[derive(Clone, Copy)]
enum Old {
    Int(i64),
    Boxed(usize),
}

impl Old {
    /// `v` as an undo's item (one that is not an `Int` waits in
    /// `boxed`; rare: the tier's effects store scalars).
    fn of(v: Value, boxed: &mut Vec<Value>) -> Self {
        match v {
            Value::Int(n) => Old::Int(n),
            v => {
                boxed.push(v);
                Old::Boxed(boxed.len() - 1)
            }
        }
    }

    /// The item back (a boxed one taken from `boxed`).
    fn value(self, boxed: &mut [Value]) -> Value {
        match self {
            Old::Int(n) => Value::Int(n),
            Old::Boxed(i) => std::mem::replace(&mut boxed[i], Value::Nil),
        }
    }
}

/// One applied effect's undo, on its transient.
#[derive(Clone, Copy)]
struct Undo<'v> {
    t: &'v Transient,
    op: UndoOp,
}

/// An iteration's undo log: its undos (plain data, so clearing them
/// is free) and the items they put back that are not `Int`s.
#[derive(Default)]
struct Log<'v> {
    undos: Vec<Undo<'v>>,
    boxed: Vec<Value>,
}

/// The undo entries an iteration may log before it misses instead
/// (bounds the log's memory; runs of appends share one entry). A
/// program that fills it leaves the tier.
const MAX_UNDO: usize = 1024;

/// A program whose iterations average more undo entries than this,
/// over all its runs once [`HEAVY_AFTER`] iterations have run (judged
/// as a run ends), leaves the tier: its nested loops of effects cost
/// more logged than on their own programs, which the boxed tiers then
/// run. (One long run is not judged midway: a tree walk logs heavily
/// near its root and lightly after.)
const HEAVY_LOG: usize = 32;
const HEAVY_AFTER: usize = 1024;

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
    /// The inlined callees' vars, each still holding its closure.
    roots: Box<[RootSite]>,
    /// The deepest inlined call nesting (the calls it stands for
    /// must fit under the call-depth limit).
    depth: usize,
    /// Whether some effect logs its undo (see [`mark_unlogged`]).
    logs: bool,
    /// The site-group id the sites validate under together.
    gid: usize,
    /// Iterations and undo entries over all runs, for [`weigh`].
    log_iters: AtomicUsize,
    logged: AtomicUsize,
    /// Entries and iterations (the boxed ones first), and the first
    /// runs and early misses, for [`judge`].
    entries: AtomicU32,
    iters: AtomicU32,
    runs: AtomicU32,
    misses: AtomicU32,
    off: AtomicBool,
}

/// Entries before a loop's average trip count is judged.
pub(crate) const JUDGE_AFTER: u32 = 32;

/// The average iterations per entry a loop needs to keep the tier
/// (measured: a one- or two-iteration loop costs more to enter than
/// its boxed iterations).
const MIN_TRIPS: u32 = 2;

/// A run that misses after finishing fewer iterations than this
/// wasted its entry and its undo: a program whose first
/// [`JUDGE_AFTER`] runs all end so leaves the tier (one long run
/// among them can pay for the rest).
const MISS_TRIPS: u32 = 8;

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
        Coll::Vec(v) => v.read_at(idx, Sv::of),
        Coll::Trans(t) => match &*t.lock() {
            TransientKind::Vector(tv) => tv.read_at(idx, Sv::of),
            _ => None,
        },
        Coll::Other => None,
    }
}

/// The scalar item `k` of the vector at `coll[i]` (a destructured
/// `(nth coll i)`): anything but a vector there, or a non-scalar or
/// missing item, misses.
fn nth2(coll: &Coll<'_>, i: Sv, k: usize) -> Option<Sv> {
    let idx = usize::try_from(i.int_of()?).ok()?;
    let item = |v: &Value| match v {
        Value::Vector(inner) => inner.read_at(k, Sv::of),
        _ => None,
    };
    match coll {
        Coll::Vec(v) => v.read_at(idx, item),
        Coll::Trans(t) => vector(&mut t.lock())?.read_at(idx, item),
        _ => None,
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

/// `assoc!` of a scalar (an `Int` keeps a packed buffer packed).
#[inline(always)]
fn store(tv: &mut TransientVector, i: usize, v: Sv) -> bool {
    match v {
        Sv { k: K::Int, v: n } => tv.assoc_int_opt(i, n),
        v => tv.assoc(i, v.value()).is_ok(),
    }
}

/// `conj!` of a scalar (an `Int` keeps a packed buffer packed).
#[inline(always)]
fn push(tv: &mut TransientVector, v: Sv) -> bool {
    match v {
        Sv { k: K::Int, v: n } => tv.conj_int_opt(n),
        v => tv.conj(v.value()).is_ok(),
    }
}

/// An `assoc!` index: an `int` (the native's `intValue` would
/// truncate a wider one, which the boxed road answers).
fn index(i: i64) -> Option<usize> {
    usize::try_from(i).ok().filter(|i| *i <= i32::MAX as usize)
}

/// An effect nothing after which can miss (so it needs no undo):
/// the natives' fast paths, which check the vector and the index and
/// change nothing when they fail. `false`: it cannot run.
#[inline(always)]
fn effect_bare(e: &EffI, s: &[Sv], colls: &[Coll<'_>]) -> bool {
    let (EffI::Assoc(c, ..) | EffI::Conj(c, _) | EffI::Pop(c)) = e;
    let Coll::Trans(t) = &colls[usize::from(*c)] else {
        return false;
    };
    let mut guard = t.lock();
    let Some(tv) = vector(&mut guard) else {
        return false;
    };
    match e {
        EffI::Conj(_, v) => push(tv, s[usize::from(*v)]),
        EffI::Assoc(_, k, v) => s[usize::from(*k)]
            .int_of()
            .and_then(index)
            .is_some_and(|i| store(tv, i, s[usize::from(*v)])),
        EffI::Pop(_) => tv.pop().is_ok(),
    }
}

/// Run one effect now and log its undo: `false` (nothing changed)
/// when it cannot run — it needs a live transient vector, an `int`
/// `assoc!` index of at most its count (the count appends), a
/// non-empty vector to `pop!`, and room in the log (checked before
/// the effect runs).
#[inline(never)]
fn effect_logged<'v>(e: &EffI, s: &[Sv], colls: &[Coll<'v>], log: &mut Log<'v>) -> bool {
    let (EffI::Assoc(c, ..) | EffI::Conj(c, _) | EffI::Pop(c)) = e;
    let (Coll::Trans(t), true) = (&colls[usize::from(*c)], log.undos.len() < MAX_UNDO) else {
        return false;
    };
    let mut guard = t.lock();
    let Some(tv) = vector(&mut guard) else {
        return false;
    };
    let op = logged(tv, e, s, &mut log.boxed);
    drop(guard);
    op.map(|op| log_undo(&mut log.undos, t, op)).is_some()
}

/// [`effect_logged`]'s effect: its undo, or `None` when it cannot
/// run (nothing changed).
fn logged(tv: &mut TransientVector, e: &EffI, s: &[Sv], boxed: &mut Vec<Value>) -> Option<UndoOp> {
    match e {
        EffI::Conj(_, v) => push(tv, s[usize::from(*v)]).then_some(UndoOp::Pops(1)),
        EffI::Assoc(_, k, v) => {
            let i = s[usize::from(*k)].int_of().and_then(index)?;
            let v = s[usize::from(*v)];
            // A packed-int buffer swaps in place.
            let swapped = match v {
                Sv { k: K::Int, v: n } => tv.swap_int(i, n),
                _ => None,
            };
            if let Some(old) = swapped {
                return Some(old.map_or(UndoOp::Pops(1), |o| UndoOp::Put(i, Old::Int(o))));
            }
            // `None` exactly when the index is the count (an append);
            // past it the store fails and nothing changes.
            let old = tv.nth_opt(i);
            store(tv, i, v)
                .then(|| old.map_or(UndoOp::Pops(1), |old| UndoOp::Put(i, Old::of(old, boxed))))
        }
        EffI::Pop(_) => {
            let old = tv.nth_opt(tv.len().ok()?.checked_sub(1)?)?;
            tv.pop().is_ok().then(|| UndoOp::Push(Old::of(old, boxed)))
        }
    }
}

/// Log an undo, folding an append into the run of appends to the
/// same transient just before it.
fn log_undo<'v>(log: &mut Vec<Undo<'v>>, t: &'v Transient, op: UndoOp) {
    if matches!(op, UndoOp::Pops(1)) {
        if let Some(Undo {
            t: last,
            op: UndoOp::Pops(k),
        }) = log.last_mut()
        {
            if std::ptr::eq(*last, t) {
                *k += 1;
                return;
            }
        }
    }
    log.push(Undo { t, op });
}

/// Undo an iteration's effects, newest first (each inverse runs on
/// the state its effect left, so it cannot fail).
fn rollback(log: &mut Log<'_>) {
    while let Some(Undo { t, op }) = log.undos.pop() {
        let _ = undo(t, op, &mut log.boxed);
    }
    log.boxed.clear();
}

/// One effect's inverse (on the vector the effect ran on).
fn undo(t: &Transient, op: UndoOp, boxed: &mut [Value]) -> Option<()> {
    let mut guard = t.lock();
    let tv = vector(&mut guard)?;
    match op {
        UndoOp::Pops(k) => (0..k).try_for_each(|_| tv.pop()).ok(),
        UndoOp::Put(i, v) => tv.assoc(i, v.value(boxed)).ok(),
        UndoOp::Push(v) => tv.conj(v.value(boxed)).ok(),
    }
}

/// A transient's vector (`None` for a map or a set).
fn vector(k: &mut TransientKind) -> Option<&mut TransientVector> {
    match k {
        TransientKind::Vector(tv) => Some(tv),
        _ => None,
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
        Co::Nth2(d, c, i, k) => {
            let v = nth2(&colls[usize::from(c)], s[usize::from(i)], usize::from(k))?;
            (d, v)
        }
        Co::Count(d, c) => (d, count(&colls[usize::from(c)])?),
    };
    s[usize::from(d)] = v;
    Some(())
}

fn exit_value(x: &ExitI, s: &[Sv], f: &Frame<'_>) -> Value {
    match x {
        ExitI::Slot(v) => s[usize::from(*v)].value(),
        ExitI::Const(v) => v.clone(),
        ExitI::Loc(l) => f.get(*l).clone(),
    }
}

/// An effect, logged or not (see [`mark_unlogged`]).
#[inline(never)]
fn effect<'v>(e: &EffI, logged: bool, s: &[Sv], colls: &[Coll<'v>], log: &mut Log<'v>) -> bool {
    if logged {
        effect_logged(e, s, colls, log)
    } else {
        effect_bare(e, s, colls)
    }
}

/// The loop head: in a program that logs (`LOGS`), the last
/// iteration's effects stand (the log clears, its entries counted for
/// [`weigh`]); then [`head`].
#[inline(always)]
fn top<const COUNT: bool, const LOGS: bool>(
    ev: &Evaluator,
    w: &mut Watch,
    log: &mut Log<'_>,
    pc: usize,
) -> Option<usize> {
    if LOGS {
        w.iters += 1;
        if !log.undos.is_empty() {
            w.logged += log.undos.len();
            log.undos.clear();
            log.boxed.clear();
        }
    }
    head::<COUNT>(ev, w, pc)
}

/// After a run of a program that logs: count its iterations and
/// undo entries, and send a program that logs heavily (see
/// [`HEAVY_LOG`]) to the boxed tiers for good.
fn weigh(sc: &ScalarLoop, w: &Watch) {
    let iters = sc.log_iters.fetch_add(w.iters, Ordering::Relaxed) + w.iters;
    let logged = sc.logged.fetch_add(w.logged, Ordering::Relaxed) + w.logged;
    if iters >= HEAVY_AFTER && logged > HEAVY_LOG * iters {
        sc.off.store(true, Ordering::Relaxed);
    }
}

/// The loop head: count the trip while judging (the trips before it
/// belong to finished iterations), and go on unless the direct
/// snapshot moved (another thread redefined a var, or this one's
/// bindings changed), which hands the loop back to the boxed road,
/// where every call validates itself.
#[inline(always)]
fn head<const COUNT: bool>(ev: &Evaluator, w: &mut Watch, pc: usize) -> Option<usize> {
    if COUNT {
        w.done = w.trips;
        w.trips += 1;
    }
    (ev.direct_snapshot() == w.snap).then_some(pc + 1)
}

/// A run's bookkeeping: the direct snapshot the sites validated
/// under at entry; while judging, the trips begun and those
/// finished; and (in a program that logs) the iterations begun and
/// the undo entries they logged.
struct Watch {
    snap: (u64, u64),
    trips: u32,
    done: u32,
    iters: usize,
    logged: usize,
}

/// Run the program from its head: the loop's value, or `None` at
/// the first miss (the state slots then hold the iteration's start,
/// and `log` its effects). `COUNT` also counts the iterations begun
/// (for [`judge`]); `LOGS`: the program has logged effects.
#[inline(never)]
fn exec<'v, const COUNT: bool, const LOGS: bool>(
    sc: &ScalarLoop,
    ev: &mut Evaluator,
    f: &Frame<'_>,
    s: &mut [Sv],
    colls: &[Coll<'v>],
    log: &mut Log<'v>,
    w: &mut Watch,
) -> Option<Value> {
    let mut pc = 0usize;
    loop {
        let next = match sc.code[pc] {
            Ins::Do(st) => step(st, s).map(|()| pc + 1),
            Ins::Co(co) => co_step(co, s, colls).map(|()| pc + 1),
            Ins::Eff(e, logged) => {
                effect(&sc.effs[usize::from(e)], logged, s, colls, log).then_some(pc + 1)
            }
            Ins::Br(b, t) => branch(b, s).map(|holds| if holds { pc + 1 } else { t as usize }),
            Ins::Jmp(t) => Some(t as usize),
            Ins::Top => top::<COUNT, LOGS>(ev, w, log, pc),
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
    if !may_enter(ev, sc) {
        return None;
    }
    let out = run_program(ev, f, sc, s);
    if out.is_none() {
        for (p, r) in targets.iter().enumerate() {
            if sc.scalar[p] {
                f.regs[*r] = s[p].value();
            }
        }
    }
    out
}

/// Run the program over a loaded slot file; a miss undoes the
/// missed iteration's effects.
fn run_program(ev: &mut Evaluator, f: &Frame<'_>, sc: &ScalarLoop, s: &mut [Sv]) -> Option<Value> {
    let mut w = Watch {
        snap: ev.direct_snapshot(),
        trips: 0,
        done: 0,
        iters: 0,
        logged: 0,
    };
    let colls: SmallVec<[Coll<'_>; 8]> = sc.colls.iter().map(|l| coll_of(f.get(*l))).collect();
    let mut log = Log::default();
    // Trips are counted only while the loop is being judged.
    let judging = sc.runs.load(Ordering::Relaxed) < JUDGE_AFTER;
    let (c, l) = (&colls[..], &mut log);
    let out = match (judging, sc.logs) {
        (true, true) => exec::<true, true>(sc, ev, f, s, c, l, &mut w),
        (true, false) => exec::<true, false>(sc, ev, f, s, c, l, &mut w),
        (false, true) => exec::<false, true>(sc, ev, f, s, c, l, &mut w),
        (false, false) => exec::<false, false>(sc, ev, f, s, c, l, &mut w),
    };
    if judging {
        judge(sc, out.is_some(), &w);
    }
    if sc.logs {
        weigh(sc, &w);
    }
    if out.is_none() {
        // A full log: this program logs too much to stay.
        if log.undos.len() >= MAX_UNDO {
            sc.off.store(true, Ordering::Relaxed);
        }
        rollback(&mut log);
    }
    out
}

/// Whether a run may start: the sites validate (one cached verdict
/// per evaluator for the whole group) and the calls the program
/// inlines fit under the call-depth limit.
fn may_enter(ev: &mut Evaluator, sc: &ScalarLoop) -> bool {
    let valid = |ev: &mut Evaluator| {
        sc.sites.iter().all(|s| s.valid(ev)) && sc.roots.iter().all(|r| r.valid(ev))
    };
    ev.calls_fit(sc.depth) && ev.group_valid(sc.gid, valid)
}

/// Count one of the program's first [`JUDGE_AFTER`] runs. After as
/// many entries (the loop's boxed ones first, see [`compile`]), a
/// loop averaging under [`MIN_TRIPS`] iterations per entry stops
/// entering the tier (its setup costs more than the boxed iterations
/// it replaces); so does a program whose first runs all miss early
/// (see [`MISS_TRIPS`]).
fn judge(sc: &ScalarLoop, finished: bool, w: &Watch) {
    let entries = sc.entries.fetch_add(1, Ordering::Relaxed) + 1;
    let iters = sc
        .iters
        .fetch_add(w.trips, Ordering::Relaxed)
        .saturating_add(w.trips);
    let early = u32::from(!finished && w.done < MISS_TRIPS);
    let misses = sc.misses.fetch_add(early, Ordering::Relaxed) + early;
    let runs = sc.runs.fetch_add(1, Ordering::Relaxed) + 1;
    let short = entries == JUDGE_AFTER && iters < MIN_TRIPS * JUDGE_AFTER;
    if short || (runs == JUDGE_AFTER && misses == JUDGE_AFTER) {
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
        /// The loop value's slot (`None`: a statement loop).
        dst: Option<Sl>,
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
            Calc::Nth2(l, i, k) => {
                let (c, i) = (self.coll(*l), self.val(i));
                Ins::Co(Co::Nth2(self.sl(), c, i, *k))
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
            Ctl::Seq(stmts, v) => {
                self.stmts(stmts);
                self.val(v)
            }
            Ctl::Loop(inner) => {
                let d = self.sl();
                self.inner(inner, Some(d));
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
            Node::Seq(stmts, rest) => {
                // Effects that end a body over a tail reading no
                // collection run at each of its leaves, after the leaf's
                // values: no step that can miss follows the last of them.
                let k = if sinks(rest) {
                    let effect = |st: &&Stmt| matches!(st, Stmt::Effect(_));
                    stmts.iter().rev().take_while(effect).count()
                } else {
                    0
                };
                let (now, last) = stmts.split_at(stmts.len() - k);
                self.stmts(now);
                self.node_then(rest, last, ctx);
            }
            Node::Deopt => {
                self.miss();
            }
            n => self.node_then(n, &[], ctx),
        }
    }

    /// `n` with the `pending` effects run at each of its leaves, after
    /// the leaf's values (see [`sinks`]).
    fn node_then(&mut self, n: &Node, pending: &[Stmt], ctx: &Ctx<'_>) {
        match n {
            Node::If(c, a, b) => {
                let els = self.label();
                self.cond(c, els);
                self.node_then(a, pending, ctx);
                self.place(els);
                self.node_then(b, pending, ctx);
            }
            Node::Let(binds, body) => {
                self.binds(binds);
                self.node_then(body, pending, ctx);
            }
            Node::Leaf(Leaf::Recur(ops)) => self.recur(ops, pending, ctx),
            Node::Leaf(Leaf::Exit(val)) => self.exit(val, pending, ctx),
            Node::Seq(..) | Node::Deopt => {
                self.stmts(pending);
                self.node(n, ctx);
            }
        }
    }

    /// Statements in source order.
    fn stmts(&mut self, stmts: &[Stmt]) {
        for st in stmts {
            match st {
                Stmt::Effect(e) => self.effect(e),
                Stmt::If(c, a, b) => {
                    let (els, end) = (self.label(), self.label());
                    self.cond(c, els);
                    self.stmts(a);
                    self.code.push(Ins::Jmp(end));
                    self.place(els);
                    self.stmts(b);
                    self.place(end);
                }
                Stmt::Let(binds, body) => {
                    self.binds(binds);
                    self.stmts(body);
                }
                Stmt::Loop(inner) => self.inner(inner, None),
                Stmt::Expr(e) => {
                    self.val(e);
                }
            }
        }
    }

    /// An effect: its operands in order, then the effect itself.
    fn effect(&mut self, e: &Effect) {
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
            Effect::Pop(t) => EffI::Pop(self.coll(*t)),
        };
        self.effs.push(eff);
        self.code.push(Ins::Eff((self.effs.len() - 1) as u16, true));
    }

    /// A `recur`: its operands, then `effects` (the iteration's last
    /// statements, when the operands read no collection), then the
    /// writes to the targets.
    fn recur(&mut self, ops: &[Option<IExpr>], effects: &[Stmt], ctx: &Ctx<'_>) {
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
        self.stmts(effects);
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

    /// An exit: its value, then `effects` (sunk to it, see [`sinks`]).
    fn exit(&mut self, val: &ExitVal, effects: &[Stmt], ctx: &Ctx<'_>) {
        if let Ctx::Inner { dst, after, .. } = ctx {
            match (dst, val) {
                (Some(d), val) => {
                    let e = exit_expr(val).next().unwrap_or(&IExpr::Deopt);
                    self.val_to(e, *d);
                }
                // A statement loop's value is dropped (computed for
                // its misses when it is a scalar expression).
                (None, ExitVal::Expr(e)) => {
                    self.val(e);
                }
                (None, _) => {}
            }
            self.stmts(effects);
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
        self.stmts(effects);
        self.exits.push(x);
        self.code.push(Ins::Exit((self.exits.len() - 1) as u16));
    }

    fn inner(&mut self, inner: &Inner, dst: Option<Sl>) {
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

/// Mark the effects after which no step can miss before the
/// iteration ends (the next `Top`, or an `Exit`): nothing could ask
/// for their undo, so they run unlogged. `true`: some effect logs.
fn mark_unlogged(code: &mut [Ins]) -> bool {
    // `may[pc]`: some path from `pc` misses before the iteration ends
    // (the least fixpoint, so a loop that cannot miss stays clear).
    let mut may = vec![false; code.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for pc in (0..code.len()).rev() {
            let at = |i: usize| may.get(i).copied().unwrap_or(true);
            let m = match code[pc] {
                Ins::Top | Ins::Exit(_) => false,
                Ins::Jmp(t) => at(t as usize),
                Ins::Br(Br::Eq(..) | Br::Truthy(_), t) => at(pc + 1) || at(t as usize),
                Ins::Do(Step::Mov(..) | Step::Set(..)) => at(pc + 1),
                _ => true,
            };
            changed |= m != may[pc];
            may[pc] = m;
        }
    }
    let mut logs = false;
    for (pc, ins) in code.iter_mut().enumerate() {
        if let Ins::Eff(_, log) = ins {
            *log = may.get(pc + 1).copied().unwrap_or(true);
            logs |= *log;
        }
    }
    logs
}

/// Whether `n` is a tree of tests and temps over leaves reading no
/// collection (effects before it may then run at its leaves instead:
/// nothing in it can observe them).
fn sinks(n: &Node) -> bool {
    match n {
        Node::If(c, a, b) => !cond_reads(c) && sinks(a) && sinks(b),
        Node::Let(binds, body) => !binds.iter().any(|(_, e)| reads_coll(e)) && sinks(body),
        Node::Leaf(Leaf::Recur(ops)) => !ops.iter().flatten().any(reads_coll),
        Node::Leaf(Leaf::Exit(val)) => !exit_expr(val).any(reads_coll),
        Node::Seq(..) | Node::Deopt => false,
    }
}

/// Whether `e` may read a collection or run effects (an operand that
/// must not move across an effect).
fn reads_coll(e: &IExpr) -> bool {
    match e {
        IExpr::Calc(c) => match &**c {
            Calc::Nth(..) | Calc::Count(_) | Calc::Nth2(..) => true,
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
        Ctl::Seq(..) | Ctl::Loop(_) => true,
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

/// `Loc` equality (the enum derives no `PartialEq`).
fn loc_eq(a: Loc, b: Loc) -> bool {
    matches!((a, b), (Loc::Reg(x), Loc::Reg(y)) | (Loc::Up(x), Loc::Up(y)) | (Loc::Rec(x), Loc::Rec(y)) if x == y)
}

/// What the compiler shares with the builder: local resolution,
/// the call-site records, and the closures calls may inline.
pub(crate) trait Host {
    fn resolve_local(&mut self, idx: usize) -> Loc;
    /// Whether `expr` may read local `g` (`vm_live::may_read` over
    /// the compiling scope).
    fn reads(&self, expr: &Expr, g: usize) -> bool;
    fn site(&self, call: &DirectCall) -> Site;
    /// The closure `var` holds now, when a call with `argc` args
    /// may inline it.
    fn callee(&self, var: &Var, argc: usize) -> Option<Callee>;
}

/// A closure whose call a program inlines: the arity a call with
/// the site's arg count runs (fixed, symbol params), over lexical
/// indexes from `base` (lower ones are captured or the fn's own
/// name, which the inlined body may not read).
pub(crate) struct Callee {
    pub(crate) base: usize,
    pub(crate) arities: Arc<[Arity]>,
    pub(crate) index: usize,
    /// The var's root at compilation (the identity witness).
    pub(crate) expect: Arc<dyn ExtensionValue>,
}

/// An inlined call's site: valid while its var has no thread
/// binding and its root is still the inlined closure (the check a
/// direct native site makes).
pub(crate) struct RootSite {
    sid: usize,
    var: Var,
    expect: Arc<dyn ExtensionValue>,
}

impl RootSite {
    fn valid(&self, ev: &mut Evaluator) -> bool {
        ev.site_root_valid(self.sid, &self.var, &self.expect)
    }
}

/// The deepest call nesting a program inlines.
const MAX_INLINE: usize = 4;

/// The calls one loop's program inlines at most (each inlined body
/// is walked whole while the loop compiles).
const MAX_INLINED: usize = 16;

/// A body-bound local: its slot, or (an inlined callee's param
/// passed one of the loop body's outer locals) that local's index.
#[derive(Clone, Copy)]
enum Lex {
    Slot(usize),
    Outer(usize),
}

/// The program builder over one `loop`'s body.
struct Builder<'h> {
    host: &'h mut dyn Host,
    /// The first lexical index bound inside the body (inside an
    /// inlined callee, its first param's).
    body_base: usize,
    /// Body-bound lexical indexes (minus `body_base`).
    lex: Vec<Lex>,
    nslots: usize,
    targets: Vec<usize>,
    scalar: Vec<bool>,
    /// Loop variables read as collections or boxed values.
    boxed: Vec<bool>,
    sites: Vec<Site>,
    roots: Vec<RootSite>,
    /// The vars of the calls being inlined here (innermost last),
    /// the deepest nesting reached, and the calls inlined so far.
    stack: Vec<u64>,
    max_depth: usize,
    inlined: usize,
    /// The innermost loop's target slots (`None` for the outer
    /// loop, whose targets are slots `0..n`).
    inner: Option<Vec<usize>>,
}

/// Compile `body` (one `loop`'s forms) over `targets`, whose
/// bindings are the lexical indexes just below `body_base`: `None`
/// when no iteration could run on scalars, or when the loop's boxed
/// `history` (entries, trips begun) already fails [`judge`]'s trip
/// rule.
pub(crate) fn compile(
    host: &mut dyn Host,
    body: &[Expr],
    targets: &[usize],
    body_base: usize,
    (entries, trips): (u32, u32),
) -> Option<ScalarLoop> {
    (entries < JUDGE_AFTER || trips >= MIN_TRIPS.saturating_mul(entries)).then_some(())?;
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
        roots: Vec::new(),
        stack: Vec::new(),
        max_depth: 0,
        inlined: 0,
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
    // Slot, effect, exit and collection ids are `u16`s.
    let ids = lw
        .nslots
        .max(lw.effs.len())
        .max(lw.exits.len())
        .max(lw.colls.len());
    (ids < usize::from(Sl::MAX)).then_some(())?;
    let mut init = vec![Sv::ZERO; lw.nslots];
    for (slot, v) in &lw.consts {
        init[usize::from(*slot)] = *v;
    }
    let loads = std::mem::take(&mut lw.loads);
    let (colls, effs) = (std::mem::take(&mut lw.colls), std::mem::take(&mut lw.effs));
    let exits = std::mem::take(&mut lw.exits);
    let mut code = lw.finish();
    let logs = mark_unlogged(&mut code);
    Some(ScalarLoop {
        scalar: b.scalar.into(),
        init: init.into(),
        code: code.into(),
        loads: loads.into(),
        colls: colls.into(),
        effs: effs.into(),
        exits: exits.into(),
        sites: b.sites.into(),
        roots: b.roots.into(),
        depth: b.max_depth,
        logs,
        gid: crate::vm::next_site(),
        log_iters: AtomicUsize::new(0),
        logged: AtomicUsize::new(0),
        entries: AtomicU32::new(entries),
        iters: AtomicU32::new(trips),
        runs: AtomicU32::new(0),
        misses: AtomicU32::new(0),
        off: AtomicBool::new(false),
    })
}

/// Whether some `recur` leaf can run on scalars (a body that
/// would miss on its first iteration is not worth entering).
fn productive(n: &Node) -> bool {
    match n {
        Node::If(c, a, b) => !cond_misses(c) && (productive(a) || productive(b)),
        Node::Let(binds, body) => !binds.iter().any(|(_, e)| misses(e)) && productive(body),
        Node::Seq(stmts, rest) => !stmts.iter().any(stmt_misses) && productive(rest),
        Node::Leaf(Leaf::Recur(ops)) => !ops.iter().flatten().any(misses),
        Node::Leaf(Leaf::Exit(_)) | Node::Deopt => false,
    }
}

/// Whether running a statement always misses.
fn stmt_misses(s: &Stmt) -> bool {
    match s {
        Stmt::Effect(Effect::Assoc(_, k, v)) => misses(k) || misses(v),
        Stmt::Effect(Effect::Conj(_, v)) | Stmt::Expr(v) => misses(v),
        Stmt::Effect(Effect::Pop(_)) | Stmt::Loop(_) => false,
        // Each branch runs only on its side of the test.
        Stmt::If(c, ..) => cond_misses(c),
        Stmt::Let(binds, body) => {
            binds.iter().any(|(_, e)| misses(e)) || body.iter().any(stmt_misses)
        }
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
        Calc::Op1(_, a) | Calc::Cast(_, a) | Calc::Nth(_, a) | Calc::Nth2(_, a, _) => misses(a),
        Calc::Op2(_, a, b) => misses(a) || misses(b),
        Calc::Count(_) => false,
    }
}

fn ctl_misses(c: &Ctl) -> bool {
    match c {
        Ctl::If(c, a, b) => cond_misses(c) || (misses(a) && misses(b)),
        Ctl::Let(binds, body) => binds.iter().any(|(_, e)| misses(e)) || misses(body),
        Ctl::Seq(stmts, v) => stmts.iter().any(stmt_misses) || misses(v),
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

    /// A local's binding (`None`: inside an inlined callee, a
    /// captured local or the fn's own name).
    fn lookup(&self, idx: usize) -> Option<Lex> {
        match idx.checked_sub(self.body_base) {
            Some(i) => self.lex.get(i).copied(),
            None => self.stack.is_empty().then_some(Lex::Outer(idx)),
        }
    }

    /// A local read: a body-bound slot, an outer loop variable (now
    /// scalar), or an invariant in place.
    fn local(&mut self, idx: usize) -> IExpr {
        match self.lookup(idx) {
            Some(Lex::Slot(s)) => IExpr::Slot(s),
            Some(Lex::Outer(i)) => match self.var(i) {
                Ok(p) => {
                    self.scalar[p] = true;
                    IExpr::Slot(p)
                }
                Err(loc) => IExpr::Loc(loc),
            },
            None => IExpr::Deopt,
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
        let Some(Lex::Outer(idx)) = self.lookup(*idx) else {
            return None;
        };
        match self.var(idx) {
            Ok(p) => {
                self.boxed[p] = true;
                Some(Loc::Reg(self.targets[p]))
            }
            Err(loc) => Some(loc),
        }
    }

    /// Whether `e` may read local `g` (inside an inlined callee,
    /// over the callee's frame).
    fn reads(&self, e: &Expr, g: usize) -> bool {
        if self.stack.is_empty() {
            self.host.reads(e, g)
        } else {
            crate::vm_live::may_read(e, self.body_base, g + 1, g)
        }
    }

    fn expr(&mut self, e: &Expr) -> IExpr {
        match e {
            Expr::Literal(v) => Sv::of(v).map_or(IExpr::Deopt, IExpr::Kst),
            Expr::Local(idx) => self.local(*idx),
            Expr::DirectNative(call) => self.call(call),
            Expr::Invoke { head, args, .. } => self.inline(head, args),
            Expr::If { test, then, els } => {
                let c = self.cond(test);
                let a = self.expr(then);
                IExpr::Ctl(Box::new(Ctl::If(c, a, self.expr(els))))
            }
            Expr::Let { bindings, body } => self.let_expr(bindings, body),
            Expr::Loop { bindings, body } => self.inner_loop(bindings, body),
            Expr::Do(forms) => self.seq_expr(forms),
            _ => IExpr::Deopt,
        }
    }

    /// The value of forms run in order (a `do`, a `let` body): the
    /// statements, then the last form's value (none is `nil`).
    fn seq_expr(&mut self, forms: &[Expr]) -> IExpr {
        match forms.split_last() {
            Some((last, [])) => self.expr(last),
            Some((last, init)) => {
                let stmts = self.stmt_seq(init);
                IExpr::Ctl(Box::new(Ctl::Seq(stmts.into(), self.expr(last))))
            }
            None => IExpr::Deopt,
        }
    }

    /// `let` bindings (symbols only) into fresh slots, visible to
    /// the later inits and `then` (the caller truncates `lex`).
    fn bindings(&mut self, bindings: &[(Pattern, Expr)]) -> Option<Vec<(usize, IExpr)>> {
        self.bind_all(bindings, false)
    }

    /// `let` bindings: symbols, and vectors of symbols destructuring
    /// an `nth` (see [`Self::vec_binding`]). (A `loop`'s destructured
    /// binding re-destructures on every `recur`: [`Self::bindings`].)
    fn let_bindings(&mut self, bindings: &[(Pattern, Expr)]) -> Option<Vec<(usize, IExpr)>> {
        self.bind_all(bindings, true)
    }

    fn bind_all(
        &mut self,
        bindings: &[(Pattern, Expr)],
        destructure: bool,
    ) -> Option<Vec<(usize, IExpr)>> {
        let mut out = Vec::with_capacity(bindings.len());
        for (pat, init) in bindings {
            match pat {
                Pattern::Symbol(_) => {
                    let e = self.expr(init);
                    let s = self.slot();
                    self.lex.push(Lex::Slot(s));
                    out.push((s, e));
                }
                Pattern::Vec(vp) if destructure => self.vec_binding(vp, init, &mut out)?,
                _ => return None,
            }
        }
        Some(out)
    }

    /// `[a b …]` (symbols only) bound to `(nth coll i)` over a boxed
    /// local: the index into a temp slot, then each name to item `k`
    /// of the vector at that index (anything else there misses, and
    /// the boxed road answers `nth`'s `nil` defaults).
    fn vec_binding(
        &mut self,
        vp: &VecPattern,
        init: &Expr,
        out: &mut Vec<(usize, IExpr)>,
    ) -> Option<()> {
        let names = vp.rest.is_none()
            && vp.as_sym.is_none()
            && vp.fixed.iter().all(|p| matches!(p, Pattern::Symbol(_)));
        let Expr::DirectNative(call) = init else {
            return None;
        };
        let (true, Some("nth"), [coll, i]) =
            (names, crate::vm_ops::core_name(call), call.args.as_slice())
        else {
            return None;
        };
        let loc = self.boxed_local(coll)?;
        let idx = self.expr(i);
        let t = self.slot();
        out.push((t, idx));
        let site = self.host.site(call);
        self.sites.push(site);
        for k in 0..vp.fixed.len() {
            let s = self.slot();
            self.lex.push(Lex::Slot(s));
            out.push((s, calc(Calc::Nth2(loc, IExpr::Slot(t), k as u16))));
        }
        Some(())
    }

    fn let_expr(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> IExpr {
        let mark = self.lex.len();
        let out = match self.let_bindings(bindings) {
            Some(binds) => IExpr::Ctl(Box::new(Ctl::Let(binds.into(), self.seq_expr(body)))),
            None => IExpr::Deopt,
        };
        self.lex.truncate(mark);
        out
    }

    fn inner_loop(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> IExpr {
        self.loop_inner(bindings, body)
            .map_or(IExpr::Deopt, |inner| IExpr::Ctl(Box::new(Ctl::Loop(inner))))
    }

    /// A nested loop (symbol bindings only).
    fn loop_inner(&mut self, bindings: &[(Pattern, Expr)], body: &[Expr]) -> Option<Inner> {
        let mark = self.lex.len();
        let out = self.bindings(bindings).map(|binds| {
            let (slots, inits): (Vec<usize>, Vec<IExpr>) = binds.into_iter().unzip();
            let saved = self.inner.replace(slots.clone());
            let body = self.body(body);
            self.inner = saved;
            Inner {
                slots: slots.into(),
                inits: inits.into(),
                body,
            }
        });
        self.lex.truncate(mark);
        out
    }

    /// A call through a var holding a closure, inlined: the params
    /// bound (one of the body's outer locals passes as itself, so a
    /// collection param reads it in place; anything else through a
    /// slot), then the arity's body in the callee's own frame. The
    /// program enters only while the var holds that closure
    /// ([`RootSite`]) and the call depth leaves room for the calls.
    fn inline(&mut self, head: &Expr, args: &[Expr]) -> IExpr {
        match self.inline_with(head, args, Self::seq_expr) {
            Some((binds, value)) => IExpr::Ctl(Box::new(Ctl::Let(binds.into(), value))),
            None => IExpr::Deopt,
        }
    }

    /// A call run for effect, inlined as statements (its value
    /// dropped).
    fn inline_stmt(&mut self, head: &Expr, args: &[Expr]) -> Stmt {
        match self.inline_with(head, args, Self::stmt_seq) {
            Some((binds, body)) => Stmt::Let(binds.into(), body.into()),
            None => Stmt::Expr(IExpr::Deopt),
        }
    }

    /// [`Self::inline`]'s frame: the param bindings, and the body as
    /// `lower` builds it in the callee's frame.
    fn inline_with<R>(
        &mut self,
        head: &Expr,
        args: &[Expr],
        lower: impl FnOnce(&mut Self, &[Expr]) -> R,
    ) -> Option<(Vec<(usize, IExpr)>, R)> {
        let found = match head {
            Expr::Global(var) if self.may_inline(var) => {
                self.host.callee(var, args.len()).map(|c| (var, c))
            }
            _ => None,
        };
        let (var, callee) = found?;
        let body = callee.arities[callee.index].body.as_slice();
        let mut binds = Vec::new();
        let mut lex = Vec::with_capacity(args.len());
        for a in args {
            let bound = match a {
                Expr::Local(idx) => self.lookup(*idx),
                other => {
                    let e = self.expr(other);
                    let s = self.slot();
                    binds.push((s, e));
                    Some(Lex::Slot(s))
                }
            };
            lex.push(bound?);
        }
        let base = std::mem::replace(&mut self.body_base, callee.base);
        let outer = std::mem::replace(&mut self.lex, lex);
        self.inlined += 1;
        self.stack.push(var.id);
        self.max_depth = self.max_depth.max(self.stack.len());
        let value = lower(self, body);
        self.stack.pop();
        self.lex = outer;
        self.body_base = base;
        self.roots.push(RootSite {
            sid: crate::vm::next_site(),
            var: var.clone(),
            expect: Arc::clone(&callee.expect),
        });
        Some((binds, value))
    }

    /// Whether a call through `var` may inline here: not inside its
    /// own inlining (recursion stays a real call), and within the
    /// nesting and per-loop budgets.
    fn may_inline(&self, var: &Var) -> bool {
        self.stack.len() < MAX_INLINE && self.inlined < MAX_INLINED && !self.stack.contains(&var.id)
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
            if !self.reads(b, self.next_index()) {
                let a = Box::new(self.cond(a));
                // `b` is analyzed with the shape's local bound.
                let mark = self.lex.len();
                let g = self.slot();
                self.lex.push(Lex::Slot(g));
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

    /// A body (forms of a `loop`, `let`, or `do`): statements, then
    /// a tail node.
    fn body(&mut self, forms: &[Expr]) -> Node {
        let Some((last, init)) = forms.split_last() else {
            return Node::Deopt;
        };
        let stmts = self.stmt_seq(init);
        let rest = self.node(last);
        if stmts.is_empty() {
            rest
        } else {
            Node::Seq(stmts.into(), Box::new(rest))
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
                let out = match self.let_bindings(bindings) {
                    Some(binds) => Node::Let(binds.into(), Box::new(self.body(body))),
                    None => Node::Deopt,
                };
                self.lex.truncate(mark);
                out
            }
            Expr::Do(forms) => self.body(forms),
            other => self.leaf(other),
        }
    }

    /// Forms run for effect, in order.
    fn stmt_seq(&mut self, forms: &[Expr]) -> Vec<Stmt> {
        let mut out = Vec::new();
        for f in forms {
            out.extend(self.stmts_of(f));
        }
        out
    }

    /// The statements a form run for effect lowers to (a literal or a
    /// local does nothing; any other value is computed for its
    /// misses and dropped).
    fn stmts_of(&mut self, e: &Expr) -> Vec<Stmt> {
        let st = match e {
            Expr::Literal(_) | Expr::Local(_) => return Vec::new(),
            Expr::Do(forms) => return self.stmt_seq(forms),
            Expr::If { test, then, els } => {
                let c = self.cond(test);
                let a = self.stmts_of(then).into();
                Stmt::If(c, a, self.stmts_of(els).into())
            }
            Expr::Let { bindings, body } => {
                let mark = self.lex.len();
                let out = match self.let_bindings(bindings) {
                    Some(binds) => Stmt::Let(binds.into(), self.stmt_seq(body).into()),
                    None => Stmt::Expr(IExpr::Deopt),
                };
                self.lex.truncate(mark);
                out
            }
            Expr::Loop { bindings, body } => match self.loop_inner(bindings, body) {
                Some(inner) => Stmt::Loop(inner),
                None => Stmt::Expr(IExpr::Deopt),
            },
            Expr::DirectNative(call) => match self.effect_call(call) {
                Some((_, eff)) => Stmt::Effect(eff),
                None => Stmt::Expr(self.call(call)),
            },
            Expr::Invoke { head, args, .. } => self.inline_stmt(head, args),
            other => Stmt::Expr(self.expr(other)),
        };
        vec![st]
    }

    /// `(assoc! t k v)`, `(conj! t v)` or `(pop! t)` over a boxed
    /// local: its target and the effect.
    fn effect_call(&mut self, call: &DirectCall) -> Option<(Loc, Effect)> {
        let (t, effect) = match (crate::vm_ops::core_name(call)?, call.args.as_slice()) {
            ("assoc!", [t, k, v]) => {
                let t = self.boxed_local(t)?;
                (t, Effect::Assoc(t, self.expr(k), self.expr(v)))
            }
            ("conj!", [t, v]) => {
                let t = self.boxed_local(t)?;
                (t, Effect::Conj(t, self.expr(v)))
            }
            ("pop!", [t]) => {
                let t = self.boxed_local(t)?;
                (t, Effect::Pop(t))
            }
            _ => return None,
        };
        let site = self.host.site(call);
        self.sites.push(site);
        Some((t, effect))
    }

    /// The iteration's end: a `recur`, or an exit value.
    fn leaf(&mut self, e: &Expr) -> Node {
        if let Expr::Recur(args) = e {
            return self
                .recur(args)
                .map_or(Node::Deopt, |ops| Node::Leaf(Leaf::Recur(ops.into())));
        }
        // `(assoc! t …)`/`(conj! t …)`/`(pop! t)` answers the transient.
        if let Expr::DirectNative(call) = e {
            if let Some((t, eff)) = self.effect_call(call) {
                let exit = Node::Leaf(Leaf::Exit(ExitVal::Box(t)));
                return Node::Seq(Box::new([Stmt::Effect(eff)]), Box::new(exit));
            }
        }
        Node::Leaf(Leaf::Exit(self.exit(e)))
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
    /// local, or a scalar expression (an inner loop's value must be
    /// one; a statement loop drops it).
    fn exit(&mut self, e: &Expr) -> ExitVal {
        if self.inner.is_some() {
            return match e {
                Expr::Literal(v) if Sv::of(v).is_none() => ExitVal::Const(v.clone()),
                other => ExitVal::Expr(self.expr(other)),
            };
        }
        match e {
            Expr::Literal(v) => ExitVal::Const(v.clone()),
            Expr::Local(idx) if *idx < self.body_base => match self.var(*idx) {
                Ok(p) => ExitVal::Var(p),
                Err(loc) => ExitVal::Box(loc),
            },
            other => ExitVal::Expr(self.expr(other)),
        }
    }
}

/// An exit value's scalar expression, if any.
fn exit_expr(v: &ExitVal) -> std::option::IntoIter<&IExpr> {
    match v {
        ExitVal::Expr(e) => Some(e),
        _ => None,
    }
    .into_iter()
}
