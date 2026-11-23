// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Unboxed SSA-promotable scalar `loop`/`recur` lowering for `clj-aot`
//! (issue #271 Slice 3b).
//!
//! Lowers [`ScalarAst`] from `clj_eval::aot_scalar::compile_aot_loop`
//! into unboxed `(k: i8, v: i64)` slots allocated in the function entry
//! block (promoted to SSA registers and `phi` nodes by LLVM's `sroa` /
//! `mem2reg` pass). Invariant strings are probed once in the loop
//! preheader (`rt_bc_scalar_str`) so in-bounds ASCII `(nth s i)` compiles
//! to a direct byte load, and transient vector mutations (`assoc!`,
//! `conj!`, `pop!`) run through zero-alloc bitcode helpers with cold
//! in-place fallback for capacity growth.

use clj_eval::aot_scalar::{
    Calc, Cast, Cmp, Cond, Ctl, Effect, ExitVal, FpCalc, IExpr, Inner, IntOp1, IntOp2, IntTest1,
    IntTest2, K, Leaf, Loc, MATH_BINARY, MATH_UNARY, MathOp1, MathOp2, Node, ScalarAst, Stmt,
    compile_aot_expr, compile_aot_loop,
};
use clj_eval::expr::{Expr, Pattern};
use clj_value::{Value, Var};
use inkwell::basic_block::BasicBlock;
use inkwell::types::BasicType;
use inkwell::values::{FloatValue, FunctionValue, IntValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};

use super::{FnCtx, HofKind, HofLoop, LocalRef, LoopState, Lowerer, RecurTarget, Unsupported};

const _: () = assert!(std::mem::size_of::<Value>() == 32);

/// Cached preheader metadata for one invariant collection.
///
/// `str_len`, `ivec_len`, `dvec_len`, and `gvec_len` are `0` when the
/// corresponding fast path is inactive, so a single unsigned `idx < len` check
/// validates both availability and bounds without a separate ok flag.
struct CollCache<'ctx> {
    slot: PointerValue<'ctx>,
    mutated: bool,
    str_ptr: PointerValue<'ctx>,
    str_len: PointerValue<'ctx>,
    ivec_ptr: PointerValue<'ctx>,
    ivec_len: PointerValue<'ctx>,
    dvec: Option<(PointerValue<'ctx>, PointerValue<'ctx>)>,
    gvec_ptr: PointerValue<'ctx>,
    gvec_len: PointerValue<'ctx>,
    tvec_locked: Option<PointerValue<'ctx>>,
    cnt_ok: PointerValue<'ctx>,
    cnt_val: PointerValue<'ctx>,
}

/// Active loop target (outer loop vs nested `Inner` loop).
enum LoopTarget<'a, 'ctx> {
    Outer {
        scalar: &'a [bool],
        header: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
        dest: PointerValue<'ctx>,
        bound: &'a [PointerValue<'ctx>],
    },
    Inner {
        slots: &'a [usize],
        header: BasicBlock<'ctx>,
        exit: BasicBlock<'ctx>,
        res_k: PointerValue<'ctx>,
        res_v: PointerValue<'ctx>,
    },
}

/// Pre-scanned AST properties and invariant locals/collections.
#[derive(Default)]
struct AstScan {
    loads: Vec<Loc>,
    globals: Vec<Var>,
    colls: Vec<Loc>,
    mutated: Vec<Loc>,
    mutated_grow: Vec<Loc>,
    read_nth: Vec<Loc>,
    read_count: Vec<Loc>,
    all_locs: Vec<Loc>,
    has_effects: bool,
    has_deopt: bool,
    has_fp: bool,
}

impl AstScan {
    fn push_unique(vec: &mut Vec<Loc>, loc: Loc) {
        if !vec.contains(&loc) {
            vec.push(loc);
        }
    }

    fn record_load(&mut self, loc: Loc) {
        Self::push_unique(&mut self.loads, loc);
        Self::push_unique(&mut self.all_locs, loc);
    }

    fn record_global(&mut self, var: &Var) {
        if !self.globals.iter().any(|v| v.id == var.id) {
            self.globals.push(var.clone());
        }
    }

    fn record_coll(&mut self, loc: Loc, mutated: bool, grows: bool) {
        Self::push_unique(&mut self.colls, loc);
        Self::push_unique(&mut self.all_locs, loc);
        if mutated {
            Self::push_unique(&mut self.mutated, loc);
        }
        if grows {
            Self::push_unique(&mut self.mutated_grow, loc);
        }
    }

    fn scan_node(&mut self, node: &Node) {
        match node {
            Node::If(c, a, b) => {
                self.scan_cond(c);
                self.scan_node(a);
                self.scan_node(b);
            }
            Node::Let(binds, b) => {
                self.scan_binds(binds);
                self.scan_node(b);
            }
            Node::Seq(stmts, b) => {
                for s in stmts.iter() {
                    self.scan_stmt(s);
                }
                self.scan_node(b);
            }
            Node::Leaf(leaf) => self.scan_leaf(leaf),
            Node::Deopt => self.has_deopt = true,
        }
    }

    fn scan_leaf(&mut self, leaf: &Leaf) {
        match leaf {
            Leaf::Recur(ops) => {
                for e in ops.iter().flatten() {
                    self.scan_expr(e);
                }
            }
            Leaf::Exit(ExitVal::Expr(e)) => self.scan_expr(e),
            Leaf::Exit(ExitVal::Box(loc)) => Self::push_unique(&mut self.all_locs, *loc),
            // A boxed exit lowers as ordinary code over outer locals.
            Leaf::Exit(ExitVal::Const(_) | ExitVal::Var(_) | ExitVal::Boxed(_)) => {}
        }
    }

    fn scan_binds(&mut self, binds: &[(usize, IExpr)]) {
        for (_, e) in binds {
            self.scan_expr(e);
        }
    }

    fn scan_stmt(&mut self, st: &Stmt) {
        match st {
            Stmt::Effect(eff) => self.scan_effect(eff),
            Stmt::If(c, a, b) => {
                self.scan_cond(c);
                for x in a.iter().chain(b.iter()) {
                    self.scan_stmt(x);
                }
            }
            Stmt::Let(binds, b) => {
                self.scan_binds(binds);
                for x in b.iter() {
                    self.scan_stmt(x);
                }
            }
            Stmt::Loop(inner) => self.scan_inner(inner),
            Stmt::Expr(e) => self.scan_expr(e),
        }
    }

    fn scan_effect(&mut self, eff: &Effect) {
        self.has_effects = true;
        match eff {
            Effect::Assoc(loc, k, v) => {
                self.record_coll(*loc, true, false);
                self.scan_expr(k);
                self.scan_expr(v);
            }
            Effect::Conj(loc, v) => {
                self.record_coll(*loc, true, true);
                self.scan_expr(v);
            }
            Effect::Pop(loc) => self.record_coll(*loc, true, true),
        }
    }

    fn scan_inner(&mut self, inner: &Inner) {
        for e in inner.inits.iter() {
            self.scan_expr(e);
        }
        self.scan_node(&inner.body);
    }

    fn scan_cond(&mut self, c: &Cond) {
        match c {
            Cond::Leaf(Cmp::Truthy(e) | Cmp::T1(_, e)) => self.scan_expr(e),
            Cond::Leaf(Cmp::T2(_, a, b) | Cmp::Eq(a, b)) => {
                self.scan_expr(a);
                self.scan_expr(b);
            }
            Cond::Not(inner) => self.scan_cond(inner),
            Cond::And(a, b) | Cond::Or(a, b) => {
                self.scan_cond(a);
                self.scan_cond(b);
            }
        }
    }

    fn scan_expr(&mut self, e: &IExpr) {
        match e {
            IExpr::Slot(_) => {}
            IExpr::Kst(sv) => self.has_fp |= sv.k == K::Double,
            IExpr::Loc(loc) => self.record_load(*loc),
            IExpr::Global(var) => self.record_global(var),
            IExpr::Calc(c) => self.scan_calc(c),
            IExpr::Ctl(c) => self.scan_ctl(c),
            IExpr::Deopt => self.has_deopt = true,
        }
    }

    fn scan_calc(&mut self, c: &Calc) {
        match c {
            Calc::Op1(_, a) => self.scan_expr(a),
            Calc::Op2(_, a, b) => {
                self.scan_expr(a);
                self.scan_expr(b);
            }
            Calc::Fp(fp) => self.scan_fp(fp),
            Calc::Count(loc) => {
                self.record_coll(*loc, false, false);
                Self::push_unique(&mut self.read_count, *loc);
            }
            Calc::Nth(loc, a) => {
                self.record_coll(*loc, false, false);
                Self::push_unique(&mut self.read_nth, *loc);
                self.scan_expr(a);
            }
            Calc::Nth2(loc, a, b) => {
                self.record_coll(*loc, false, false);
                self.scan_expr(a);
                self.scan_expr(b);
            }
        }
    }

    fn scan_fp(&mut self, fp: &FpCalc) {
        match fp {
            FpCalc::Div(a, b) | FpCalc::Math2(_, a, b) => {
                self.has_fp = true;
                self.scan_expr(a);
                self.scan_expr(b);
            }
            FpCalc::Math1(op, a) => {
                self.has_fp |= *op != MathOp1::Abs;
                self.scan_expr(a);
            }
            FpCalc::Cast(c, a) => {
                self.has_fp |= *c == Cast::Double;
                self.scan_expr(a);
            }
        }
    }

    fn scan_ctl(&mut self, c: &Ctl) {
        match c {
            Ctl::If(cond, a, b) => {
                self.scan_cond(cond);
                self.scan_expr(a);
                self.scan_expr(b);
            }
            Ctl::Let(binds, b) => {
                self.scan_binds(binds);
                self.scan_expr(b);
            }
            Ctl::Seq(stmts, b) => {
                for s in stmts.iter() {
                    self.scan_stmt(s);
                }
                self.scan_expr(b);
            }
            Ctl::Loop(inner) => self.scan_inner(inner),
            Ctl::Test(cond) => self.scan_cond(cond),
        }
    }
}

/// Per-loop scalar lowering state.
struct ScalarSession<'ctx> {
    allow_fp: bool,
    k_slots: Vec<PointerValue<'ctx>>,
    v_slots: Vec<PointerValue<'ctx>>,
    load_locs: Vec<Loc>,
    load_vals: Vec<(IntValue<'ctx>, IntValue<'ctx>)>,
    global_ids: Vec<u64>,
    global_vals: Vec<(IntValue<'ctx>, IntValue<'ctx>)>,
    coll_locs: Vec<Loc>,
    colls: Vec<CollCache<'ctx>>,
    scratch_k: PointerValue<'ctx>,
    scratch_v: PointerValue<'ctx>,
    deopt_bb: BasicBlock<'ctx>,
}

impl<'ctx> ScalarSession<'ctx> {
    fn load_val(&self, loc: Loc) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let idx = self
            .load_locs
            .iter()
            .position(|l| *l == loc)
            .expect("registered load loc");
        self.load_vals[idx]
    }

    fn global_val(&self, id: u64) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let idx = self
            .global_ids
            .iter()
            .position(|g| *g == id)
            .expect("registered global id");
        self.global_vals[idx]
    }

    fn coll_idx(&self, loc: Loc) -> usize {
        self.coll_locs
            .iter()
            .position(|l| *l == loc)
            .expect("registered coll loc")
    }
}

impl<'ctx> Lowerer<'ctx> {
    /// Attempt to lower `(loop bindings body...)` via unboxed scalar IR,
    /// returning `Ok(false)` when the loop is not eligible.
    pub(super) fn try_lower_scalar_loop(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith || !self.use_bitcode || bindings.is_empty() {
            return Ok(false);
        }
        if !bindings
            .iter()
            .all(|(p, _)| matches!(p, Pattern::Symbol(_)))
        {
            return Ok(false);
        }
        let depth = f.env.len();
        let body_base = depth + bindings.len();
        let slots: Vec<usize> = (depth..body_base).collect();
        let Some(ast) = compile_aot_loop(body, &slots, body_base, &self.callees, |v| {
            self.is_stable_global_coll(v)
        }) else {
            return Ok(false);
        };
        let mut scan = AstScan::default();
        scan.scan_node(&ast.node);
        if !self.can_lower_scalar(f, &ast, &scan) {
            return Ok(false);
        }
        let (bound, targets) = self.bind_scalar_loop_inits(f, bindings)?;
        let read_only_nth = !scan.has_effects && !scan.read_nth.is_empty();
        let allow_fp = scan.has_fp
            || bindings
                .iter()
                .zip(&ast.scalar)
                .any(|((_, init), &is_sc)| is_sc && init_needs_fp(init, read_only_nth));
        let sc_hdr = self.ctx.append_basic_block(f.func, "sc_hdr");
        let deopt_bb = self.ctx.append_basic_block(f.func, "sc_deopt");
        let boxed_hdr = self.ctx.append_basic_block(f.func, "sc_boxed");
        let loop_done = self.ctx.append_basic_block(f.func, "sc_done");
        let session = self.init_scalar_session(
            f, &ast, &scan, allow_fp, &bound, sc_hdr, deopt_bb, boxed_hdr,
        );
        self.builder.position_at_end(sc_hdr);
        let outer = LoopTarget::Outer {
            scalar: &ast.scalar,
            header: sc_hdr,
            done: loop_done,
            dest,
            bound: &bound,
        };
        self.lower_sc_node(f, &session, &ast.node, &outer)?;
        self.emit_scalar_deopt(f, &session, &ast.scalar, &bound, deopt_bb, boxed_hdr);
        self.emit_scalar_boxed_fallback(f, body, dest, depth, targets, boxed_hdr, loop_done)?;
        self.builder.position_at_end(loop_done);
        self.drop_slots(f.func, &bound);
        Ok(true)
    }

    #[allow(clippy::type_complexity)]
    fn bind_scalar_loop_inits(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
    ) -> Result<
        (
            Vec<PointerValue<'ctx>>,
            Vec<(Pattern, Vec<PointerValue<'ctx>>)>,
        ),
        Unsupported,
    > {
        let mut bound = Vec::with_capacity(bindings.len());
        let mut targets = Vec::with_capacity(bindings.len());
        for (pat, init) in bindings {
            let t = self.slot(f);
            self.lower(f, init, t)?;
            f.env.push(Some(LocalRef::Slot(t)));
            bound.push(t);
            targets.push((pat.clone(), vec![t]));
        }
        Ok((bound, targets))
    }

    fn emit_scalar_deopt(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        scalar: &[bool],
        bound: &[PointerValue<'ctx>],
        deopt_bb: BasicBlock<'ctx>,
        boxed_hdr: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(deopt_bb);
        self.unlock_all_tvecs(f, s);
        for (p, &is_sc) in scalar.iter().enumerate() {
            if is_sc {
                let k = self.load_i8(s.k_slots[p]);
                let v = self.load_i64(s.v_slots[p]);
                self.box_scalar(f.func, bound[p], k, v);
            }
        }
        self.builder
            .build_unconditional_branch(boxed_hdr)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_scalar_boxed_fallback(
        &mut self,
        f: &mut FnCtx<'ctx>,
        body: &[Expr],
        dest: PointerValue<'ctx>,
        depth: usize,
        targets: Vec<(Pattern, Vec<PointerValue<'ctx>>)>,
        boxed_hdr: BasicBlock<'ctx>,
        loop_done: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        self.builder.position_at_end(boxed_hdr);
        f.recur.push(RecurTarget {
            header: boxed_hdr,
            let_depth: f.let_slots.len(),
            patterns: targets,
        });
        let res = self.lower_body(f, body, dest);
        f.recur.pop();
        f.env.truncate(depth);
        res?;
        self.builder
            .build_unconditional_branch(loop_done)
            .expect("br");
        Ok(())
    }

    fn can_lower_scalar(&self, f: &FnCtx<'ctx>, ast: &ScalarAst, scan: &AstScan) -> bool {
        if !scan
            .all_locs
            .iter()
            .all(|loc| self.is_direct_loc(f, &ast.global_colls, *loc))
        {
            return false;
        }
        if scan.has_effects && (ast.primed_after_effect || scan.has_deopt || ast.deferred_row_read)
        {
            return false;
        }
        true
    }

    fn is_stable_global_coll(&self, v: &Var) -> bool {
        self.stable_of(v)
            .is_some_and(|sr| self.stable_unchecked(sr))
    }

    fn is_direct_loc(&self, f: &FnCtx<'ctx>, global_colls: &[Var], loc: Loc) -> bool {
        match loc {
            Loc::Up(u) => global_colls
                .get(u)
                .is_some_and(|v| self.is_stable_global_coll(v)),
            _ => matches!(
                f.env.get(loc_reg_idx(loc)).copied().flatten(),
                Some(LocalRef::Slot(_) | LocalRef::Captured(_))
            ),
        }
    }

    fn loc_slot(&self, f: &FnCtx<'ctx>, global_colls: &[Var], loc: Loc) -> PointerValue<'ctx> {
        match loc {
            Loc::Up(u) => {
                self.stable_of(&global_colls[u])
                    .expect("checked stable global coll")
                    .root
            }
            _ => self
                .local_slot(f, loc_reg_idx(loc))
                .expect("direct local slot"),
        }
    }

    pub(super) fn entry_alloca(
        &self,
        f: &FnCtx<'ctx>,
        ty: impl BasicType<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        let here = self.builder.get_insert_block().expect("bb");
        self.builder.position_at_end(f.entry);
        let p = self.builder.build_alloca(ty, name).expect("alloca");
        self.builder.position_at_end(here);
        p
    }

    pub(super) fn load_i8(&self, ptr: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.i8t, ptr, "k")
            .expect("load")
            .into_int_value()
    }

    fn load_i64(&self, ptr: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.i64t, ptr, "v")
            .expect("load")
            .into_int_value()
    }

    /// Unbox `slot` into `(ok: i1, k: i8, v: i64)` with zero branches
    /// (`0 = Int, 1 = Char, 2 = Bool`, plus `3 = Double` when `allow_fp`).
    fn unbox_scalar(
        &self,
        slot: PointerValue<'ctx>,
        allow_fp: bool,
    ) -> (IntValue<'ctx>, IntValue<'ctx>, IntValue<'ctx>) {
        let b = &self.builder;
        let t = self.tag(slot);
        let is_int = b
            .build_int_compare(IntPredicate::EQ, t, self.i8t.const_int(2, false), "is_i")
            .expect("cmp");
        let is_chr = b
            .build_int_compare(IntPredicate::EQ, t, self.i8t.const_int(9, false), "is_c")
            .expect("cmp");
        let is_bl = b
            .build_int_compare(IntPredicate::EQ, t, self.i8t.const_int(1, false), "is_b")
            .expect("cmp");
        let (is_dbl, is_64) = if allow_fp {
            let d = b
                .build_int_compare(IntPredicate::EQ, t, self.i8t.const_int(3, false), "is_d")
                .expect("cmp");
            (Some(d), b.build_or(is_int, d, "is_64").expect("or"))
        } else {
            (None, is_int)
        };
        let ok1 = b.build_or(is_64, is_chr, "ok1").expect("or");
        let ok = b.build_or(ok1, is_bl, "sc_ok").expect("or");
        let iv = self.int_payload(slot);
        let cv = b
            .build_int_z_extend(self.char_payload(slot), self.i64t, "cv")
            .expect("zext");
        let raw_b = self.load_i8(self.byte_at(slot, 1));
        let bv = b.build_int_z_extend(raw_b, self.i64t, "bv").expect("zext");
        let v1 = b
            .build_select(is_chr, cv, bv, "v1")
            .expect("sel")
            .into_int_value();
        let v = b
            .build_select(is_64, iv, v1, "sc_v")
            .expect("sel")
            .into_int_value();
        let k1 = b
            .build_select(
                is_chr,
                self.i8t.const_int(1, false),
                self.i8t.const_int(2, false),
                "k1",
            )
            .expect("sel")
            .into_int_value();
        let k2 = match is_dbl {
            Some(d) => b
                .build_select(d, self.i8t.const_int(3, false), k1, "k2")
                .expect("sel")
                .into_int_value(),
            None => k1,
        };
        let k = b
            .build_select(is_int, self.i8t.const_zero(), k2, "sc_k")
            .expect("sel")
            .into_int_value();
        (ok, k, v)
    }

    /// Box `(k: i8, v: i64)` into `dest` (`0 = Int, 1 = Char, 2 = Bool, 3 = Double`).
    fn box_scalar(
        &self,
        func: FunctionValue<'ctx>,
        dest: PointerValue<'ctx>,
        k: IntValue<'ctx>,
        v: IntValue<'ctx>,
    ) {
        self.drop_v(func, dest);
        let b = &self.builder;
        let is_int = b
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_zero(), "bi")
            .expect("cmp");
        let is_chr = b
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_int(1, false), "bc")
            .expect("cmp");
        let is_dbl = b
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_int(3, false), "bd")
            .expect("cmp");
        let t1 = b
            .build_select(
                is_chr,
                self.i8t.const_int(9, false),
                self.i8t.const_int(1, false),
                "t1",
            )
            .expect("sel")
            .into_int_value();
        let t2 = b
            .build_select(is_dbl, self.i8t.const_int(3, false), t1, "t2")
            .expect("sel")
            .into_int_value();
        let tag = b
            .build_select(is_int, self.i8t.const_int(2, false), t2, "btag")
            .expect("sel")
            .into_int_value();
        b.build_store(dest, tag).expect("store");
        let nz = b
            .build_int_compare(IntPredicate::NE, v, self.i64t.const_zero(), "bnz")
            .expect("cmp");
        let b8 = b.build_int_z_extend(nz, self.i8t, "b8").expect("zext");
        b.build_store(self.byte_at(dest, 1), b8).expect("store");
        let c32 = b.build_int_truncate(v, self.i32t, "c32").expect("trunc");
        b.build_store(self.byte_at(dest, 4), c32).expect("store");
        b.build_store(self.byte_at(dest, 8), v).expect("store");
    }

    #[allow(clippy::too_many_arguments)]
    fn init_scalar_session(
        &mut self,
        f: &mut FnCtx<'ctx>,
        ast: &ScalarAst,
        scan: &AstScan,
        allow_fp: bool,
        bound: &[PointerValue<'ctx>],
        sc_hdr: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_hdr: BasicBlock<'ctx>,
    ) -> ScalarSession<'ctx> {
        let mut k_slots = Vec::with_capacity(ast.nslots);
        let mut v_slots = Vec::with_capacity(ast.nslots);
        for _ in 0..ast.nslots {
            k_slots.push(self.entry_alloca(f, self.i8t, "sck"));
            v_slots.push(self.entry_alloca(f, self.i64t, "scv"));
        }
        let scratch_k = self.entry_alloca(f, self.i8t, "scrk");
        let scratch_v = self.entry_alloca(f, self.i64t, "scrv");
        for (p, &is_sc) in ast.scalar.iter().enumerate() {
            if is_sc {
                let (ok, k, v) = self.unbox_scalar(bound[p], allow_fp);
                let next = self.ctx.append_basic_block(f.func, "sc_init");
                self.builder
                    .build_conditional_branch(ok, next, boxed_hdr)
                    .expect("br");
                self.builder.position_at_end(next);
                self.builder.build_store(k_slots[p], k).expect("store");
                self.builder.build_store(v_slots[p], v).expect("store");
            }
        }
        let mut load_vals = Vec::with_capacity(scan.loads.len());
        for &loc in &scan.loads {
            let slot = self.loc_slot(f, &ast.global_colls, loc);
            let (ok, k, v) = self.unbox_scalar(slot, allow_fp);
            let next = self.ctx.append_basic_block(f.func, "sc_box_ok");
            self.builder
                .build_conditional_branch(ok, next, boxed_hdr)
                .expect("br");
            self.builder.position_at_end(next);
            load_vals.push((k, v));
        }
        let mut global_ids = Vec::with_capacity(scan.globals.len());
        let mut global_vals = Vec::with_capacity(scan.globals.len());
        for var in &scan.globals {
            let tmp = self.slot(f);
            let kv = self.var_of(var);
            match self.stable_of(var) {
                Some(sr) => self.clone_stable(f, sr, kv, tmp),
                None => self.emit_deref(f.func, tmp, kv),
            }
            let (ok, k, v) = self.unbox_scalar(tmp, allow_fp);
            self.free_slots(f, &[tmp]);
            let next = self.ctx.append_basic_block(f.func, "sc_glb_ok");
            self.builder
                .build_conditional_branch(ok, next, boxed_hdr)
                .expect("br");
            self.builder.position_at_end(next);
            global_ids.push(var.id);
            global_vals.push((k, v));
        }
        let mut colls = Vec::with_capacity(scan.colls.len());
        for &loc in &scan.colls {
            colls.push(self.probe_coll(f, &ast.global_colls, loc, scan, allow_fp));
        }
        for i in 0..colls.len() {
            self.try_lock_tvec(f, &colls, i, allow_fp);
        }
        self.builder.build_unconditional_branch(sc_hdr).expect("br");
        ScalarSession {
            allow_fp,
            k_slots,
            v_slots,
            load_locs: scan.loads.clone(),
            load_vals,
            global_ids,
            global_vals,
            coll_locs: scan.colls.clone(),
            colls,
            scratch_k,
            scratch_v,
            deopt_bb,
        }
    }

    fn probe_coll(
        &self,
        f: &FnCtx<'ctx>,
        global_colls: &[Var],
        loc: Loc,
        scan: &AstScan,
        allow_fp: bool,
    ) -> CollCache<'ctx> {
        let mutated = scan.mutated.contains(&loc);
        let grows = scan.mutated_grow.contains(&loc);
        let needs_nth = scan.read_nth.contains(&loc);
        let needs_cnt = scan.read_count.contains(&loc);
        let slot = self.loc_slot(f, global_colls, loc);
        let str_ptr = self.entry_alloca(f, self.ptr, "strp");
        let str_len = self.entry_alloca(f, self.i64t, "strn");
        let ivec_ptr = self.entry_alloca(f, self.ptr, "ivp");
        let ivec_len = self.entry_alloca(f, self.i64t, "ivn");
        let dvec = allow_fp.then(|| {
            (
                self.entry_alloca(f, self.ptr, "dvp"),
                self.entry_alloca(f, self.i64t, "dvn"),
            )
        });
        let gvec_ptr = self.entry_alloca(f, self.ptr, "gvp");
        let gvec_len = self.entry_alloca(f, self.i64t, "gvn");
        let lockable = !grows && (mutated || needs_nth);
        let tvec_locked = lockable.then(|| self.entry_alloca(f, self.i8t, "tvlk"));
        let cnt_ok = self.entry_alloca(f, self.i8t, "cntok");
        let cnt_val = self.entry_alloca(f, self.i64t, "cntv");
        let z8 = self.i8t.const_zero();
        let z64 = self.i64t.const_zero();
        self.builder.build_store(str_len, z64).expect("store");
        self.builder.build_store(ivec_len, z64).expect("store");
        if let Some((_, dvec_len)) = dvec {
            self.builder.build_store(dvec_len, z64).expect("store");
        }
        self.builder.build_store(gvec_len, z64).expect("store");
        self.builder.build_store(cnt_ok, z8).expect("store");
        if let Some(tl) = tvec_locked {
            self.builder.build_store(tl, z8).expect("store");
        }
        if !mutated && (needs_nth || needs_cnt) {
            let probe = self
                .helper("rt_n_str_ascii", Some(self.i8t.into()), &[self.ptr.into()])
                .as_global_value()
                .as_pointer_value();
            if needs_nth {
                self.call(
                    "rt_bc_scalar_str",
                    Some(self.i8t),
                    &[slot.into(), str_ptr.into(), str_len.into(), probe.into()],
                );
                self.call(
                    "rt_bc_scalar_ivec",
                    Some(self.i8t),
                    &[slot.into(), ivec_ptr.into(), ivec_len.into()],
                );
                if let Some((dvec_ptr, dvec_len)) = dvec {
                    self.call(
                        "rt_bc_scalar_dvec",
                        Some(self.i8t),
                        &[slot.into(), dvec_ptr.into(), dvec_len.into()],
                    );
                    self.call(
                        "rt_bc_scalar_gvec",
                        Some(self.i8t),
                        &[slot.into(), gvec_ptr.into(), gvec_len.into()],
                    );
                }
            }
            if needs_cnt {
                let cok = self
                    .call(
                        "rt_bc_scalar_count",
                        Some(self.i8t),
                        &[slot.into(), cnt_val.into(), probe.into()],
                    )
                    .expect("cok");
                self.builder.build_store(cnt_ok, cok).expect("store");
            }
        }
        CollCache {
            slot,
            mutated,
            str_ptr,
            str_len,
            ivec_ptr,
            ivec_len,
            dvec,
            gvec_ptr,
            gvec_len,
            tvec_locked,
            cnt_ok,
            cnt_val,
        }
    }

    fn try_lock_tvec(
        &self,
        f: &FnCtx<'ctx>,
        colls: &[CollCache<'ctx>],
        idx: usize,
        allow_fp: bool,
    ) {
        let c = &colls[idx];
        let Some(tl) = c.tvec_locked else {
            return;
        };
        let done_bb = self.ctx.append_basic_block(f.func, "sc_tv_done");
        if !c.mutated {
            let next_bb = self.ctx.append_basic_block(f.func, "sc_tv_chk");
            let mut already = self
                .builder
                .build_or(self.load_i64(c.str_len), self.load_i64(c.ivec_len), "alr1")
                .expect("or");
            if let Some((_, dvec_len)) = c.dvec {
                already = self
                    .builder
                    .build_or(already, self.load_i64(dvec_len), "alr2")
                    .expect("or");
            }
            already = self
                .builder
                .build_or(already, self.load_i64(c.gvec_len), "alr")
                .expect("or");
            let skip = self
                .builder
                .build_int_compare(IntPredicate::NE, already, self.i64t.const_zero(), "skp")
                .expect("cmp");
            self.builder
                .build_conditional_branch(skip, done_bb, next_bb)
                .expect("br");
            self.builder.position_at_end(next_bb);
        }
        if colls.len() > 1 {
            let lock_bb = self.ctx.append_basic_block(f.func, "sc_tv_lock");
            let is_aliased = self.check_coll_aliased(colls, idx);
            self.builder
                .build_conditional_branch(is_aliased, done_bb, lock_bb)
                .expect("br");
            self.builder.position_at_end(lock_bb);
        }
        let dvec_try = if allow_fp {
            c.dvec
                .map(|d| (d, self.ctx.append_basic_block(f.func, "sc_dtv_try")))
        } else {
            None
        };
        let miss_bb = dvec_try.map_or(done_bb, |(_, bb)| bb);
        self.emit_lock_tvec_kind(
            f,
            c,
            tl,
            "rt_bc_scalar_tvec_lock",
            c.ivec_ptr,
            c.ivec_len,
            miss_bb,
            done_bb,
        );
        if let Some(((dvec_ptr, dvec_len), try_bb)) = dvec_try {
            self.builder.position_at_end(try_bb);
            self.emit_lock_tvec_kind(
                f,
                c,
                tl,
                "rt_bc_scalar_dtvec_lock",
                dvec_ptr,
                dvec_len,
                done_bb,
                done_bb,
            );
        }
        self.builder.position_at_end(done_bb);
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_lock_tvec_kind(
        &self,
        f: &FnCtx<'ctx>,
        c: &CollCache<'ctx>,
        tl: PointerValue<'ctx>,
        helper: &str,
        ptr_slot: PointerValue<'ctx>,
        len_slot: PointerValue<'ctx>,
        miss_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        let ok = self
            .call(
                helper,
                Some(self.i8t),
                &[c.slot.into(), ptr_slot.into(), len_slot.into()],
            )
            .expect("tlk");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_tv_ok");
        self.builder
            .build_conditional_branch(hit, ok_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        let one = self.i8t.const_int(1, false);
        self.builder.build_store(tl, one).expect("st");
        self.builder.build_store(c.cnt_ok, one).expect("st");
        let len = self.load_i64(len_slot);
        self.builder.build_store(c.cnt_val, len).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
    }

    fn check_coll_aliased(&self, colls: &[CollCache<'ctx>], idx: usize) -> IntValue<'ctx> {
        let slot = colls[idx].slot;
        let mut aliased = self.i8t.const_zero();
        for (j, other) in colls.iter().enumerate() {
            if j != idx {
                let same = self
                    .call(
                        "rt_bc_scalar_same_transient",
                        Some(self.i8t),
                        &[slot.into(), other.slot.into()],
                    )
                    .expect("same");
                aliased = self.builder.build_or(aliased, same, "al").expect("or");
            }
        }
        self.builder
            .build_int_compare(IntPredicate::NE, aliased, self.i8t.const_zero(), "isa")
            .expect("cmp")
    }

    fn unlock_coll_tvec(&self, f: &FnCtx<'ctx>, c: &CollCache<'ctx>) {
        let Some(tl) = c.tvec_locked else {
            return;
        };
        let locked = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                self.load_i8(tl),
                self.i8t.const_zero(),
                "tvlk",
            )
            .expect("cmp");
        let rel_bb = self.ctx.append_basic_block(f.func, "sc_tv_unlk");
        let cont_bb = self.ctx.append_basic_block(f.func, "sc_tv_cont");
        self.builder
            .build_conditional_branch(locked, rel_bb, cont_bb)
            .expect("br");
        self.builder.position_at_end(rel_bb);
        self.call("rt_bc_scalar_tvec_unlock", None, &[c.slot.into()]);
        let zero = self.i8t.const_zero();
        let z64 = self.i64t.const_zero();
        self.builder.build_store(tl, zero).expect("st");
        self.builder.build_store(c.ivec_len, z64).expect("st");
        if let Some((_, dvec_len)) = c.dvec {
            self.builder.build_store(dvec_len, z64).expect("st");
        }
        self.builder.build_store(c.cnt_ok, zero).expect("st");
        self.builder
            .build_unconditional_branch(cont_bb)
            .expect("br");
        self.builder.position_at_end(cont_bb);
    }

    fn unlock_all_tvecs(&self, f: &FnCtx<'ctx>, s: &ScalarSession<'ctx>) {
        for c in &s.colls {
            self.unlock_coll_tvec(f, c);
        }
    }

    fn scaled_elem_ptr(
        &self,
        ptr_slot: PointerValue<'ctx>,
        iv: IntValue<'ctx>,
        shift: u64,
    ) -> PointerValue<'ctx> {
        let base = self
            .builder
            .build_load(self.ptr, ptr_slot, "ip")
            .expect("ld")
            .into_pointer_value();
        let off = self
            .builder
            .build_left_shift(iv, self.i64t.const_int(shift, false), "ioff")
            .expect("shl");
        let addr = self
            .builder
            .build_int_add(
                self.builder
                    .build_ptr_to_int(base, self.i64t, "ip2i")
                    .expect("p2i"),
                off,
                "iaddr",
            )
            .expect("add");
        self.builder
            .build_int_to_ptr(addr, self.ptr, "ii2p")
            .expect("i2p")
    }

    fn ivec_elem_ptr(
        &self,
        ptr_slot: PointerValue<'ctx>,
        iv: IntValue<'ctx>,
    ) -> PointerValue<'ctx> {
        self.scaled_elem_ptr(ptr_slot, iv, 3)
    }

    fn lower_sc_node(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        node: &Node,
        target: &LoopTarget<'_, 'ctx>,
    ) -> Result<(), Unsupported> {
        match node {
            Node::If(c, then_n, else_n) => {
                let then_bb = self.ctx.append_basic_block(f.func, "sc_then");
                let else_bb = self.ctx.append_basic_block(f.func, "sc_else");
                self.lower_sc_cond(f, s, c, then_bb, else_bb)?;
                self.builder.position_at_end(then_bb);
                self.lower_sc_node(f, s, then_n, target)?;
                self.builder.position_at_end(else_bb);
                self.lower_sc_node(f, s, else_n, target)
            }
            Node::Let(binds, body) => {
                self.lower_sc_binds(f, s, binds)?;
                self.lower_sc_node(f, s, body, target)
            }
            Node::Seq(stmts, body) => {
                for st in stmts.iter() {
                    self.lower_sc_stmt(f, s, st)?;
                }
                self.lower_sc_node(f, s, body, target)
            }
            Node::Leaf(leaf) => self.lower_sc_leaf(f, s, leaf, target),
            Node::Deopt => {
                self.builder
                    .build_unconditional_branch(s.deopt_bb)
                    .expect("br");
                Ok(())
            }
        }
    }

    fn lower_sc_binds(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        binds: &[(usize, IExpr)],
    ) -> Result<(), Unsupported> {
        for (slot, expr) in binds {
            let (k, v) = self.lower_sc_expr(f, s, expr)?;
            self.builder.build_store(s.k_slots[*slot], k).expect("st");
            self.builder.build_store(s.v_slots[*slot], v).expect("st");
        }
        Ok(())
    }

    fn lower_sc_leaf(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        leaf: &Leaf,
        target: &LoopTarget<'_, 'ctx>,
    ) -> Result<(), Unsupported> {
        match leaf {
            Leaf::Recur(ops) => self.lower_sc_recur(f, s, ops, target),
            Leaf::Exit(val) => self.lower_sc_exit(f, s, val, target),
        }
    }

    fn lower_sc_recur(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        ops: &[Option<IExpr>],
        target: &LoopTarget<'_, 'ctx>,
    ) -> Result<(), Unsupported> {
        let (slots, header) = match target {
            LoopTarget::Outer { header, .. } => (None, *header),
            LoopTarget::Inner { slots, header, .. } => (Some(*slots), *header),
        };
        let mut next = Vec::with_capacity(ops.len());
        for (j, op) in ops.iter().enumerate() {
            if let Some(e) = op {
                let (nk, nv) = self.lower_sc_expr(f, s, e)?;
                let sl = slots.map_or(j, |ss| ss[j]);
                next.push((sl, nk, nv));
            }
        }
        for (sl, nk, nv) in next {
            self.builder.build_store(s.k_slots[sl], nk).expect("st");
            self.builder.build_store(s.v_slots[sl], nv).expect("st");
        }
        self.builder.build_unconditional_branch(header).expect("br");
        Ok(())
    }

    /// The outer loop's exit value into `dest` (split from
    /// [`Self::lower_sc_exit`] for the complexity budget).
    fn lower_outer_exit(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        val: &ExitVal,
        scalar: &[bool],
        dest: PointerValue<'ctx>,
        bound: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let expr_res = match val {
            ExitVal::Expr(e) => Some(self.lower_sc_expr(f, s, e)?),
            _ => None,
        };
        self.unlock_all_tvecs(f, s);
        match val {
            ExitVal::Const(v) => self.lower_const(f.func, v, dest)?,
            ExitVal::Box(loc) => {
                let src = self.loc_slot(f, &[], *loc);
                self.clone_v(f.func, dest, src);
            }
            ExitVal::Var(p) if !scalar[*p] => {
                self.clone_v(f.func, dest, bound[*p]);
            }
            ExitVal::Var(p) => {
                let k = self.load_i8(s.k_slots[*p]);
                let v = self.load_i64(s.v_slots[*p]);
                self.box_scalar(f.func, dest, k, v);
            }
            ExitVal::Expr(_) => {
                let (k, v) = expr_res.expect("expr");
                self.box_scalar(f.func, dest, k, v);
            }
            // Reads only locals bound before the loop (#271).
            ExitVal::Boxed(e) => self.lower(f, e, dest)?,
        }
        Ok(())
    }

    fn lower_sc_exit(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        val: &ExitVal,
        target: &LoopTarget<'_, 'ctx>,
    ) -> Result<(), Unsupported> {
        match target {
            LoopTarget::Outer {
                scalar,
                done,
                dest,
                bound,
                ..
            } => {
                self.lower_outer_exit(f, s, val, scalar, *dest, bound)?;
                self.builder.build_unconditional_branch(*done).expect("br");
            }
            LoopTarget::Inner {
                exit, res_k, res_v, ..
            } => {
                let (k, v) = match val {
                    ExitVal::Expr(e) => self.lower_sc_expr(f, s, e)?,
                    _ => (self.i8t.const_zero(), self.i64t.const_zero()),
                };
                self.builder.build_store(*res_k, k).expect("st");
                self.builder.build_store(*res_v, v).expect("st");
                self.builder.build_unconditional_branch(*exit).expect("br");
            }
        }
        Ok(())
    }

    fn lower_sc_stmt(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        st: &Stmt,
    ) -> Result<(), Unsupported> {
        match st {
            Stmt::Effect(eff) => self.lower_sc_effect(f, s, eff),
            Stmt::If(c, then_s, else_s) => self.lower_sc_stmt_if(f, s, c, then_s, else_s),
            Stmt::Let(binds, body) => {
                self.lower_sc_binds(f, s, binds)?;
                for x in body.iter() {
                    self.lower_sc_stmt(f, s, x)?;
                }
                Ok(())
            }
            Stmt::Loop(inner) => {
                self.lower_sc_inner(f, s, inner)?;
                Ok(())
            }
            Stmt::Expr(e) => {
                self.lower_sc_expr(f, s, e)?;
                Ok(())
            }
        }
    }

    fn lower_sc_stmt_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c: &Cond,
        then_s: &[Stmt],
        else_s: &[Stmt],
    ) -> Result<(), Unsupported> {
        let then_bb = self.ctx.append_basic_block(f.func, "sc_st_then");
        let else_bb = self.ctx.append_basic_block(f.func, "sc_st_else");
        let join_bb = self.ctx.append_basic_block(f.func, "sc_st_join");
        self.lower_sc_cond(f, s, c, then_bb, else_bb)?;
        self.builder.position_at_end(then_bb);
        for x in then_s {
            self.lower_sc_stmt(f, s, x)?;
        }
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(else_bb);
        for x in else_s {
            self.lower_sc_stmt(f, s, x)?;
        }
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(join_bb);
        Ok(())
    }

    fn lower_sc_effect(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        eff: &Effect,
    ) -> Result<(), Unsupported> {
        match eff {
            Effect::Conj(loc, val_e) => self.lower_sc_conj(f, s, *loc, val_e),
            Effect::Assoc(loc, idx_e, val_e) => self.lower_sc_assoc(f, s, *loc, idx_e, val_e),
            Effect::Pop(loc) => self.lower_sc_pop(f, s, *loc),
        }
    }

    fn lower_sc_conj(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
        val_e: &IExpr,
    ) -> Result<(), Unsupported> {
        let (vk, vv) = self.lower_sc_expr(f, s, val_e)?;
        let c_slot = s.colls[s.coll_idx(loc)].slot;
        let try_bb = self.ctx.append_basic_block(f.func, "sc_conj_try");
        let slow_bb = self.ctx.append_basic_block(f.func, "sc_conj_slow");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_conj_done");
        let non_i_bb = if s.allow_fp {
            self.ctx.append_basic_block(f.func, "sc_conj_cd")
        } else {
            slow_bb
        };
        self.builder
            .build_conditional_branch(self.is_k_int(vk), try_bb, non_i_bb)
            .expect("br");
        self.emit_sc_conj_call(try_bb, "rt_bc_scalar_conj", c_slot, vv, done_bb, slow_bb);
        if s.allow_fp {
            self.builder.position_at_end(non_i_bb);
            let try_d = self.ctx.append_basic_block(f.func, "sc_conj_tryd");
            self.builder
                .build_conditional_branch(self.is_k_dbl(vk), try_d, slow_bb)
                .expect("br");
            self.emit_sc_conj_call(try_d, "rt_bc_scalar_conj_d", c_slot, vv, done_bb, slow_bb);
        }
        self.builder.position_at_end(slow_bb);
        self.unlock_all_tvecs(f, s);
        let tv = self.slot(f);
        let out = self.slot(f);
        self.box_scalar(f.func, tv, vk, vv);
        let k = self.var("clojure.core", "conj!");
        let call_bb = self.ctx.append_basic_block(f.func, "sc_conj_call");
        let slow_ok = self.ctx.append_basic_block(f.func, "sc_conj_ok");
        let fx_ok = self
            .call(
                "rt_n_conj_bang_fx",
                Some(self.i8t),
                &[c_slot.into(), tv.into()],
            )
            .expect("ok");
        let fx_hit = self
            .builder
            .build_int_compare(IntPredicate::NE, fx_ok, self.i8t.const_zero(), "fx")
            .expect("cmp");
        self.builder
            .build_conditional_branch(fx_hit, slow_ok, call_bb)
            .expect("br");
        self.emit_direct_slow(f, &[c_slot, tv], k, 2, Some("conj!"), out, call_bb, slow_ok);
        self.drop_and_nil_slots(f.func, &[out, tv]);
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        Ok(())
    }

    fn emit_sc_conj_call(
        &self,
        bb: BasicBlock<'ctx>,
        helper: &str,
        c_slot: PointerValue<'ctx>,
        vv: IntValue<'ctx>,
        done_bb: BasicBlock<'ctx>,
        slow_bb: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(bb);
        let ok = self
            .call(helper, Some(self.i8t), &[c_slot.into(), vv.into()])
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done_bb, slow_bb)
            .expect("br");
    }

    fn lower_sc_assoc(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
        idx_e: &IExpr,
        val_e: &IExpr,
    ) -> Result<(), Unsupported> {
        let (ik, iv) = self.lower_sc_expr(f, s, idx_e)?;
        let (vk, vv) = self.lower_sc_expr(f, s, val_e)?;
        let c = &s.colls[s.coll_idx(loc)];
        let c_slot = c.slot;
        let try_bb = self.ctx.append_basic_block(f.func, "sc_assoc_try");
        let try_d_bb = s
            .allow_fp
            .then(|| self.ctx.append_basic_block(f.func, "sc_assoc_tryd"));
        let slow_bb = self.ctx.append_basic_block(f.func, "sc_assoc_slow");
        let slow_ok = self.ctx.append_basic_block(f.func, "sc_assoc_ok");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_assoc_done");
        let both_i = self.is_k_int(self.builder.build_or(ik, vk, "ikvk").expect("or"));
        let idx_i_val_d = s.allow_fp.then(|| {
            self.builder
                .build_and(self.is_k_int(ik), self.is_k_dbl(vk), "iid")
                .expect("and")
        });
        self.emit_sc_assoc_fast(
            f,
            c,
            both_i,
            idx_i_val_d,
            iv,
            vv,
            try_bb,
            try_d_bb,
            slow_bb,
            done_bb,
        );
        self.emit_sc_assoc_call(
            try_bb,
            "rt_bc_scalar_assoc",
            c_slot,
            iv,
            vv,
            done_bb,
            slow_bb,
        );
        if let Some(tdb) = try_d_bb {
            self.emit_sc_assoc_call(
                tdb,
                "rt_bc_scalar_assoc_d",
                c_slot,
                iv,
                vv,
                done_bb,
                slow_bb,
            );
        }
        self.builder.position_at_end(slow_bb);
        self.unlock_all_tvecs(f, s);
        let ti = self.slot(f);
        let tv = self.slot(f);
        let out = self.slot(f);
        self.box_scalar(f.func, ti, ik, iv);
        self.box_scalar(f.func, tv, vk, vv);
        let k = self.var("clojure.core", "assoc!");
        let call_bb = self.ctx.append_basic_block(f.func, "sc_assoc_call");
        self.builder
            .build_unconditional_branch(call_bb)
            .expect("br");
        self.emit_direct_slow(
            f,
            &[c_slot, ti, tv],
            k,
            3,
            Some("assoc!"),
            out,
            call_bb,
            slow_ok,
        );
        self.drop_and_nil_slots(f.func, &[out, ti, tv]);
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_assoc_call(
        &self,
        bb: BasicBlock<'ctx>,
        helper: &str,
        c_slot: PointerValue<'ctx>,
        iv: IntValue<'ctx>,
        vv: IntValue<'ctx>,
        done_bb: BasicBlock<'ctx>,
        slow_bb: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(bb);
        let ok = self
            .call(
                helper,
                Some(self.i8t),
                &[c_slot.into(), iv.into(), vv.into()],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done_bb, slow_bb)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_assoc_dispatch(
        &self,
        f: &FnCtx<'ctx>,
        both_i: IntValue<'ctx>,
        idx_i_val_d: Option<IntValue<'ctx>>,
        try_bb: BasicBlock<'ctx>,
        try_d_bb: Option<BasicBlock<'ctx>>,
        slow_bb: BasicBlock<'ctx>,
    ) {
        if let (Some(iid), Some(tdb)) = (idx_i_val_d, try_d_bb) {
            let chk_d = self.ctx.append_basic_block(f.func, "sc_assoc_cd");
            self.builder
                .build_conditional_branch(both_i, try_bb, chk_d)
                .expect("br");
            self.builder.position_at_end(chk_d);
            self.builder
                .build_conditional_branch(iid, tdb, slow_bb)
                .expect("br");
        } else {
            self.builder
                .build_conditional_branch(both_i, try_bb, slow_bb)
                .expect("br");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_assoc_fast_store(
        &self,
        kind_ok: IntValue<'ctx>,
        ptr_slot: PointerValue<'ctx>,
        len_slot: PointerValue<'ctx>,
        iv: IntValue<'ctx>,
        vv: IntValue<'ctx>,
        fast_bb: BasicBlock<'ctx>,
        miss_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        let in_b = self
            .builder
            .build_int_compare(IntPredicate::ULT, iv, self.load_i64(len_slot), "ainb")
            .expect("cmp");
        let fast = self.builder.build_and(kind_ok, in_b, "afast").expect("and");
        self.builder
            .build_conditional_branch(fast, fast_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(fast_bb);
        let ep = self.ivec_elem_ptr(ptr_slot, iv);
        self.builder.build_store(ep, vv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_assoc_fast(
        &self,
        f: &FnCtx<'ctx>,
        c: &CollCache<'ctx>,
        both_i: IntValue<'ctx>,
        idx_i_val_d: Option<IntValue<'ctx>>,
        iv: IntValue<'ctx>,
        vv: IntValue<'ctx>,
        try_bb: BasicBlock<'ctx>,
        try_d_bb: Option<BasicBlock<'ctx>>,
        slow_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        if c.tvec_locked.is_none() {
            self.emit_sc_assoc_dispatch(f, both_i, idx_i_val_d, try_bb, try_d_bb, slow_bb);
            return;
        }
        // `c` is mutated (`assoc!`), so `ivec_len`/`dvec_len` is non-zero iff locked.
        let fast_bb = self.ctx.append_basic_block(f.func, "sc_assoc_fast");
        let miss_bb = self.ctx.append_basic_block(f.func, "sc_assoc_miss");
        let dvec_branch = idx_i_val_d
            .zip(c.dvec)
            .map(|(iid, d)| (iid, d, self.ctx.append_basic_block(f.func, "sc_assoc_dchk")));
        let after_i = dvec_branch.map_or(miss_bb, |(_, _, bb)| bb);
        self.emit_sc_assoc_fast_store(
            both_i, c.ivec_ptr, c.ivec_len, iv, vv, fast_bb, after_i, done_bb,
        );
        if let Some((iid, (dvec_ptr, dvec_len), dchk_bb)) = dvec_branch {
            let dfast_bb = self.ctx.append_basic_block(f.func, "sc_assoc_dfast");
            self.builder.position_at_end(dchk_bb);
            self.emit_sc_assoc_fast_store(
                iid, dvec_ptr, dvec_len, iv, vv, dfast_bb, miss_bb, done_bb,
            );
        }
        self.builder.position_at_end(miss_bb);
        self.unlock_coll_tvec(f, c);
        self.emit_sc_assoc_dispatch(f, both_i, idx_i_val_d, try_bb, try_d_bb, slow_bb);
    }

    fn lower_sc_pop(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
    ) -> Result<(), Unsupported> {
        let c_slot = s.colls[s.coll_idx(loc)].slot;
        let slow_bb = self.ctx.append_basic_block(f.func, "sc_pop_slow");
        let call_bb = self.ctx.append_basic_block(f.func, "sc_pop_call");
        let slow_ok = self.ctx.append_basic_block(f.func, "sc_pop_ok");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_pop_done");
        let ok = self
            .call("rt_bc_scalar_pop", Some(self.i8t), &[c_slot.into()])
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done_bb, slow_bb)
            .expect("br");
        self.builder.position_at_end(slow_bb);
        self.unlock_all_tvecs(f, s);
        self.builder
            .build_unconditional_branch(call_bb)
            .expect("br");
        let out = self.slot(f);
        let k = self.var("clojure.core", "pop!");
        self.emit_direct_slow(f, &[c_slot], k, 1, Some("pop!"), out, call_bb, slow_ok);
        self.drop_and_nil_slots(f.func, &[out]);
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        Ok(())
    }

    fn lower_sc_inner(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        inner: &Inner,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        for (j, init_e) in inner.inits.iter().enumerate() {
            let (ik, iv) = self.lower_sc_expr(f, s, init_e)?;
            let sl = inner.slots[j];
            self.builder.build_store(s.k_slots[sl], ik).expect("st");
            self.builder.build_store(s.v_slots[sl], iv).expect("st");
        }
        let res_k = self.entry_alloca(f, self.i8t, "ink");
        let res_v = self.entry_alloca(f, self.i64t, "inv");
        let hdr = self.ctx.append_basic_block(f.func, "sc_in_hdr");
        let exit = self.ctx.append_basic_block(f.func, "sc_in_exit");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let t = LoopTarget::Inner {
            slots: &inner.slots,
            header: hdr,
            exit,
            res_k,
            res_v,
        };
        self.lower_sc_node(f, s, &inner.body, &t)?;
        self.builder.position_at_end(exit);
        Ok((self.load_i8(res_k), self.load_i64(res_v)))
    }

    fn lower_sc_cond(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c: &Cond,
        yes: BasicBlock<'ctx>,
        no: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        match c {
            Cond::Leaf(cmp) => {
                let bit = self.lower_sc_cmp(f, s, cmp)?;
                self.builder
                    .build_conditional_branch(bit, yes, no)
                    .expect("br");
                Ok(())
            }
            Cond::Not(inner) => self.lower_sc_cond(f, s, inner, no, yes),
            Cond::And(a, b) => {
                let mid = self.ctx.append_basic_block(f.func, "sc_and");
                self.lower_sc_cond(f, s, a, mid, no)?;
                self.builder.position_at_end(mid);
                self.lower_sc_cond(f, s, b, yes, no)
            }
            Cond::Or(a, b) => {
                let mid = self.ctx.append_basic_block(f.func, "sc_or");
                self.lower_sc_cond(f, s, a, yes, mid)?;
                self.builder.position_at_end(mid);
                self.lower_sc_cond(f, s, b, yes, no)
            }
        }
    }

    fn lower_sc_cmp(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        cmp: &Cmp,
    ) -> Result<IntValue<'ctx>, Unsupported> {
        match cmp {
            Cmp::Truthy(e) => {
                let (k, v) = self.lower_sc_expr(f, s, e)?;
                Ok(self.scalar_truthy(k, v))
            }
            Cmp::T1(op, e) => {
                let (k, v) = self.lower_sc_expr(f, s, e)?;
                Ok(self.lower_sc_t1(f, s, *op, k, v))
            }
            Cmp::T2(op, a, b) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                let (bk, bv) = self.lower_sc_expr(f, s, b)?;
                Ok(self.lower_sc_t2(f, s, *op, ak, av, bk, bv))
            }
            Cmp::Eq(a, b) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                let (bk, bv) = self.lower_sc_expr(f, s, b)?;
                Ok(self.scalar_eq(ak, av, bk, bv, s.allow_fp))
            }
        }
    }

    fn lower_sc_t1(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntTest1,
        k: IntValue<'ctx>,
        v: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let name = match op {
            IntTest1::Zero => "zero?",
            IntTest1::Pos => "pos?",
            IntTest1::Neg => "neg?",
        };
        if !s.allow_fp {
            self.require_int(f, k, s.deopt_bb);
            return self.int_pred(name, v);
        }
        let is_i = self.is_k_int(k);
        let is_d = self.is_k_dbl(k);
        let res = self.entry_alloca(f, self.ctx.bool_type(), "t1r");
        let int_bb = self.ctx.append_basic_block(f.func, "sc_t1_i");
        let chk_d = self.ctx.append_basic_block(f.func, "sc_t1_cd");
        let dbl_bb = self.ctx.append_basic_block(f.func, "sc_t1_d");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_t1_ok");
        self.builder
            .build_conditional_branch(is_i, int_bb, chk_d)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let ir = self.int_pred(name, v);
        self.builder.build_store(res, ir).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_d);
        self.builder
            .build_conditional_branch(is_d, dbl_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(dbl_bb);
        let fp = match op {
            IntTest1::Zero => FloatPredicate::OEQ,
            IntTest1::Pos => FloatPredicate::OGT,
            IntTest1::Neg => FloatPredicate::OLT,
        };
        let dr = self
            .builder
            .build_float_compare(
                fp,
                self.bits_to_f64(v),
                self.ctx.f64_type().const_zero(),
                "dt1",
            )
            .expect("fcmp");
        self.builder.build_store(res, dr).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        self.builder
            .build_load(self.ctx.bool_type(), res, "t1v")
            .expect("ld")
            .into_int_value()
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_sc_t2(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntTest2,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        if !s.allow_fp {
            self.require_ints(f, ak, bk, s.deopt_bb);
            return self.lower_sc_int_test2(op, av, bv);
        }
        let both_i = self.is_k_int(self.builder.build_or(ak, bk, "t2o").expect("or"));
        let res = self.entry_alloca(f, self.ctx.bool_type(), "t2r");
        let int_bb = self.ctx.append_basic_block(f.func, "sc_t2_i");
        let chk_d = self.ctx.append_basic_block(f.func, "sc_t2_cd");
        let dbl_bb = self.ctx.append_basic_block(f.func, "sc_t2_d");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_t2_ok");
        self.builder
            .build_conditional_branch(both_i, int_bb, chk_d)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let ir = self.lower_sc_int_test2(op, av, bv);
        self.builder.build_store(res, ir).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_d);
        let both_num = self
            .builder
            .build_and(self.is_k_num(ak), self.is_k_num(bk), "bn")
            .expect("and");
        self.builder
            .build_conditional_branch(both_num, dbl_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(dbl_bb);
        let fa = self.scalar_as_f64(ak, av);
        let fb = self.scalar_as_f64(bk, bv);
        let fp = match op {
            IntTest2::Lt => FloatPredicate::OLT,
            IntTest2::Lte => FloatPredicate::OLE,
            IntTest2::Gt => FloatPredicate::OGT,
            IntTest2::Gte => FloatPredicate::OGE,
            _ => FloatPredicate::OEQ,
        };
        let dr = self
            .builder
            .build_float_compare(fp, fa, fb, "dt2")
            .expect("fcmp");
        self.builder.build_store(res, dr).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        self.builder
            .build_load(self.ctx.bool_type(), res, "t2v")
            .expect("ld")
            .into_int_value()
    }

    fn scalar_truthy(&self, k: IntValue<'ctx>, v: IntValue<'ctx>) -> IntValue<'ctx> {
        let is_b = self
            .builder
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_int(2, false), "ib")
            .expect("cmp");
        let is_z = self
            .builder
            .build_int_compare(IntPredicate::EQ, v, self.i64t.const_zero(), "iz")
            .expect("cmp");
        let falsy = self.builder.build_and(is_b, is_z, "falsy").expect("and");
        self.builder.build_not(falsy, "truthy").expect("not")
    }

    fn scalar_eq(
        &self,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
        allow_fp: bool,
    ) -> IntValue<'ctx> {
        let keq = self
            .builder
            .build_int_compare(IntPredicate::EQ, ak, bk, "keq")
            .expect("cmp");
        let veq_i = self
            .builder
            .build_int_compare(IntPredicate::EQ, av, bv, "veqi")
            .expect("cmp");
        let veq = if allow_fp {
            let is_d = self.is_k_dbl(ak);
            let veq_d = self
                .builder
                .build_float_compare(
                    FloatPredicate::OEQ,
                    self.bits_to_f64(av),
                    self.bits_to_f64(bv),
                    "veqd",
                )
                .expect("fcmp");
            self.builder
                .build_select(is_d, veq_d, veq_i, "veq")
                .expect("sel")
                .into_int_value()
        } else {
            veq_i
        };
        self.builder.build_and(keq, veq, "eq").expect("and")
    }

    fn lower_sc_int_test2(
        &self,
        op: IntTest2,
        av: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let pred = match op {
            IntTest2::Lt => IntPredicate::SLT,
            IntTest2::Lte => IntPredicate::SLE,
            IntTest2::Gt => IntPredicate::SGT,
            IntTest2::Gte => IntPredicate::SGE,
            _ => IntPredicate::EQ,
        };
        self.builder
            .build_int_compare(pred, av, bv, "t2")
            .expect("cmp")
    }

    fn lower_sc_expr(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        e: &IExpr,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        match e {
            IExpr::Slot(sl) => Ok((self.load_i8(s.k_slots[*sl]), self.load_i64(s.v_slots[*sl]))),
            IExpr::Loc(loc) => Ok(s.load_val(*loc)),
            IExpr::Global(var) => Ok(s.global_val(var.id)),
            IExpr::Kst(sv) => {
                let k = match sv.k {
                    K::Int => 0,
                    K::Char => 1,
                    K::Bool => 2,
                    K::Double => 3,
                };
                Ok((
                    self.i8t.const_int(k, false),
                    self.i64t.const_int(sv.v as u64, true),
                ))
            }
            IExpr::Calc(c) => self.lower_sc_calc(f, s, c),
            IExpr::Ctl(c) => self.lower_sc_ctl(f, s, c),
            IExpr::Deopt => {
                let dead = self.ctx.append_basic_block(f.func, "sc_dead");
                self.builder
                    .build_unconditional_branch(s.deopt_bb)
                    .expect("br");
                self.builder.position_at_end(dead);
                Ok((self.i8t.const_zero(), self.i64t.const_zero()))
            }
        }
    }

    fn lower_sc_ctl(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        ctl: &Ctl,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        match ctl {
            Ctl::If(c, then_e, else_e) => self.lower_sc_ctl_if(f, s, c, then_e, else_e),
            Ctl::Let(binds, body) => {
                self.lower_sc_binds(f, s, binds)?;
                self.lower_sc_expr(f, s, body)
            }
            Ctl::Seq(stmts, body) => {
                for st in stmts.iter() {
                    self.lower_sc_stmt(f, s, st)?;
                }
                self.lower_sc_expr(f, s, body)
            }
            Ctl::Loop(inner) => self.lower_sc_inner(f, s, inner),
            Ctl::Test(c) => self.lower_sc_ctl_test(f, s, c),
        }
    }

    fn lower_sc_ctl_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c: &Cond,
        then_e: &IExpr,
        else_e: &IExpr,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        let rk = self.entry_alloca(f, self.i8t, "ifk");
        let rv = self.entry_alloca(f, self.i64t, "ifv");
        let then_bb = self.ctx.append_basic_block(f.func, "sc_if_then");
        let else_bb = self.ctx.append_basic_block(f.func, "sc_if_else");
        let join_bb = self.ctx.append_basic_block(f.func, "sc_if_join");
        self.lower_sc_cond(f, s, c, then_bb, else_bb)?;
        self.builder.position_at_end(then_bb);
        let (tk, tv) = self.lower_sc_expr(f, s, then_e)?;
        self.builder.build_store(rk, tk).expect("st");
        self.builder.build_store(rv, tv).expect("st");
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(else_bb);
        let (ek, ev) = self.lower_sc_expr(f, s, else_e)?;
        self.builder.build_store(rk, ek).expect("st");
        self.builder.build_store(rv, ev).expect("st");
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(join_bb);
        Ok((self.load_i8(rk), self.load_i64(rv)))
    }

    fn lower_sc_ctl_test(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c: &Cond,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        let rv = self.entry_alloca(f, self.i64t, "tstv");
        let yes_bb = self.ctx.append_basic_block(f.func, "sc_tst_y");
        let no_bb = self.ctx.append_basic_block(f.func, "sc_tst_n");
        let join_bb = self.ctx.append_basic_block(f.func, "sc_tst_j");
        self.lower_sc_cond(f, s, c, yes_bb, no_bb)?;
        self.builder.position_at_end(yes_bb);
        self.builder
            .build_store(rv, self.i64t.const_int(1, false))
            .expect("st");
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(no_bb);
        self.builder
            .build_store(rv, self.i64t.const_zero())
            .expect("st");
        self.builder
            .build_unconditional_branch(join_bb)
            .expect("br");
        self.builder.position_at_end(join_bb);
        Ok((self.i8t.const_int(2, false), self.load_i64(rv)))
    }

    fn lower_sc_calc(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        calc: &Calc,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        match calc {
            Calc::Op1(op, a) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                Ok(self.lower_sc_op1_any(f, s, *op, ak, av))
            }
            Calc::Op2(op, a, b) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                let (bk, bv) = self.lower_sc_expr(f, s, b)?;
                Ok(self.lower_sc_op2_any(f, s, *op, ak, av, bk, bv))
            }
            Calc::Fp(fp) => self.lower_sc_calc_fp(f, s, fp),
            Calc::Count(loc) => Ok(self.lower_sc_count(f, s, *loc)),
            Calc::Nth(loc, idx) => self.lower_sc_nth(f, s, *loc, idx),
            Calc::Nth2(loc, idx, item) => self.lower_sc_nth2(f, s, *loc, idx, item),
        }
    }

    fn lower_sc_calc_fp(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        fp: &FpCalc,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        match fp {
            FpCalc::Div(a, b) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                let (bk, bv) = self.lower_sc_expr(f, s, b)?;
                Ok(self.lower_sc_div(f, s, ak, av, bk, bv))
            }
            FpCalc::Math1(op, a) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                Ok(self.lower_sc_math1(f, s, *op, ak, av))
            }
            FpCalc::Math2(op, a, b) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                let (bk, bv) = self.lower_sc_expr(f, s, b)?;
                Ok(self.lower_sc_math2(f, s, *op, ak, av, bk, bv))
            }
            FpCalc::Cast(c, a) => {
                let (ak, av) = self.lower_sc_expr(f, s, a)?;
                Ok(self.lower_sc_cast(f, s, *c, ak, av))
            }
        }
    }

    fn lower_sc_op1_any(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntOp1,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        if !s.allow_fp || op == IntOp1::BitNot {
            self.require_int(f, ak, s.deopt_bb);
            return (self.i8t.const_zero(), self.lower_sc_op1(f, s, op, av));
        }
        let rk = self.entry_alloca(f, self.i8t, "o1k");
        let rv = self.entry_alloca(f, self.i64t, "o1v");
        let int_bb = self.ctx.append_basic_block(f.func, "sc_o1_i");
        let chk_d = self.ctx.append_basic_block(f.func, "sc_o1_cd");
        let dbl_bb = self.ctx.append_basic_block(f.func, "sc_o1_d");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_o1_ok");
        self.builder
            .build_conditional_branch(self.is_k_int(ak), int_bb, chk_d)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let ir = self.lower_sc_op1(f, s, op, av);
        self.builder
            .build_store(rk, self.i8t.const_zero())
            .expect("st");
        self.builder.build_store(rv, ir).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_d);
        self.builder
            .build_conditional_branch(self.is_k_dbl(ak), dbl_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(dbl_bb);
        let d = self.bits_to_f64(av);
        let one = self.ctx.f64_type().const_float(1.0);
        let dr = match op {
            IntOp1::Inc | IntOp1::UInc => {
                self.builder.build_float_add(d, one, "dinc").expect("fadd")
            }
            IntOp1::Dec | IntOp1::UDec => {
                self.builder.build_float_sub(d, one, "ddec").expect("fsub")
            }
            _ => self.builder.build_float_neg(d, "dneg").expect("fneg"),
        };
        self.builder
            .build_store(rk, self.i8t.const_int(3, false))
            .expect("st");
        self.builder.build_store(rv, self.f64_bits(dr)).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        (self.load_i8(rk), self.load_i64(rv))
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_sc_op2_any(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntOp2,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        if !s.allow_fp
            || !matches!(
                op,
                IntOp2::Add
                    | IntOp2::Sub
                    | IntOp2::Mul
                    | IntOp2::UAdd
                    | IntOp2::USub
                    | IntOp2::UMul
                    | IntOp2::Max
                    | IntOp2::Min
            )
        {
            self.require_ints(f, ak, bk, s.deopt_bb);
            return (self.i8t.const_zero(), self.lower_sc_op2(f, s, op, av, bv));
        }
        let both_i = self.is_k_int(self.builder.build_or(ak, bk, "o2o").expect("or"));
        let rk = self.entry_alloca(f, self.i8t, "o2k");
        let rv = self.entry_alloca(f, self.i64t, "o2v");
        let int_bb = self.ctx.append_basic_block(f.func, "sc_o2_i");
        let chk_d = self.ctx.append_basic_block(f.func, "sc_o2_cd");
        let dbl_bb = self.ctx.append_basic_block(f.func, "sc_o2_d");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_o2_ok");
        self.builder
            .build_conditional_branch(both_i, int_bb, chk_d)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let ir = self.lower_sc_op2(f, s, op, av, bv);
        self.builder
            .build_store(rk, self.i8t.const_zero())
            .expect("st");
        self.builder.build_store(rv, ir).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_d);
        let both_num = self
            .builder
            .build_and(self.is_k_num(ak), self.is_k_num(bk), "bn")
            .expect("and");
        self.builder
            .build_conditional_branch(both_num, dbl_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(dbl_bb);
        let (dk, dv) = self.lower_sc_float_op2(op, ak, av, bk, bv);
        self.builder.build_store(rk, dk).expect("st");
        self.builder.build_store(rv, dv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        (self.load_i8(rk), self.load_i64(rv))
    }

    fn lower_sc_float_op2(
        &self,
        op: IntOp2,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let fa = self.scalar_as_f64(ak, av);
        let fb = self.scalar_as_f64(bk, bv);
        let d3 = self.i8t.const_int(3, false);
        match op {
            IntOp2::Add | IntOp2::UAdd => (
                d3,
                self.f64_bits(self.builder.build_float_add(fa, fb, "fadd").expect("fadd")),
            ),
            IntOp2::Sub | IntOp2::USub => (
                d3,
                self.f64_bits(self.builder.build_float_sub(fa, fb, "fsub").expect("fsub")),
            ),
            IntOp2::Mul | IntOp2::UMul => (
                d3,
                self.f64_bits(self.builder.build_float_mul(fa, fb, "fmul").expect("fmul")),
            ),
            _ => {
                let pred = if op == IntOp2::Max {
                    FloatPredicate::OGT
                } else {
                    FloatPredicate::OLT
                };
                let a_nan = self
                    .builder
                    .build_float_compare(FloatPredicate::UNO, fa, fa, "anan")
                    .expect("fcmp");
                let ord = self
                    .builder
                    .build_float_compare(pred, fa, fb, "mord")
                    .expect("fcmp");
                let keep = self.builder.build_or(a_nan, ord, "mkeep").expect("or");
                let k = self
                    .builder
                    .build_select(keep, ak, bk, "mk")
                    .expect("sel")
                    .into_int_value();
                let v = self
                    .builder
                    .build_select(keep, av, bv, "mv")
                    .expect("sel")
                    .into_int_value();
                (k, v)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_sc_div(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let both_num = self
            .builder
            .build_and(self.is_k_num(ak), self.is_k_num(bk), "dn")
            .expect("and");
        let any_dbl = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                self.builder.build_or(ak, bk, "dor").expect("or"),
                self.i8t.const_zero(),
                "dad",
            )
            .expect("cmp");
        let ok = self
            .builder
            .build_and(both_num, any_dbl, "dok")
            .expect("and");
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_div_ok");
        self.builder
            .build_conditional_branch(ok, ok_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        let fa = self.scalar_as_f64(ak, av);
        let fb = self.scalar_as_f64(bk, bv);
        let r = self.builder.build_float_div(fa, fb, "fdiv").expect("fdiv");
        (self.i8t.const_int(3, false), self.f64_bits(r))
    }

    fn lower_sc_math1(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: MathOp1,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        match op {
            MathOp1::Abs => self.lower_sc_abs(f, s, ak, av),
            MathOp1::Sqrt => {
                self.require_num(f, ak, s.deopt_bb);
                let x = self.scalar_as_f64(ak, av);
                let intr = inkwell::intrinsics::Intrinsic::find("llvm.sqrt").expect("llvm.sqrt");
                let decl = intr
                    .get_declaration(&self.module, &[self.ctx.f64_type().into()])
                    .expect("decl");
                let r = self
                    .builder
                    .build_call(decl, &[x.into()], "sc_sqrt")
                    .expect("call")
                    .try_as_basic_value()
                    .expect_basic("sqrt")
                    .into_float_value();
                (self.i8t.const_int(3, false), self.f64_bits(r))
            }
            MathOp1::Unary(i) => {
                self.require_num(f, ak, s.deopt_bb);
                let x = self.scalar_as_f64(ak, av);
                let r = self.call_f64(MATH_UNARY[usize::from(i)].1, &[x.into()]);
                (self.i8t.const_int(3, false), self.f64_bits(r))
            }
        }
    }

    fn emit_iabs(&self, av: IntValue<'ctx>) -> IntValue<'ctx> {
        let intr = inkwell::intrinsics::Intrinsic::find("llvm.abs").expect("llvm.abs");
        let decl = intr
            .get_declaration(&self.module, &[self.i64t.into()])
            .expect("decl");
        self.builder
            .build_call(
                decl,
                &[av.into(), self.ctx.bool_type().const_zero().into()],
                "sc_iabs",
            )
            .expect("call")
            .try_as_basic_value()
            .expect_basic("iabs")
            .into_int_value()
    }

    fn lower_sc_abs(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        if !s.allow_fp {
            self.require_int(f, ak, s.deopt_bb);
            return (self.i8t.const_zero(), self.emit_iabs(av));
        }
        let rk = self.entry_alloca(f, self.i8t, "absk");
        let rv = self.entry_alloca(f, self.i64t, "absv");
        let int_bb = self.ctx.append_basic_block(f.func, "sc_abs_i");
        let chk_d = self.ctx.append_basic_block(f.func, "sc_abs_cd");
        let dbl_bb = self.ctx.append_basic_block(f.func, "sc_abs_d");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_abs_ok");
        self.builder
            .build_conditional_branch(self.is_k_int(ak), int_bb, chk_d)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let ir = self.emit_iabs(av);
        self.builder
            .build_store(rk, self.i8t.const_zero())
            .expect("st");
        self.builder.build_store(rv, ir).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_d);
        self.builder
            .build_conditional_branch(self.is_k_dbl(ak), dbl_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(dbl_bb);
        let fintr = inkwell::intrinsics::Intrinsic::find("llvm.fabs").expect("llvm.fabs");
        let fdecl = fintr
            .get_declaration(&self.module, &[self.ctx.f64_type().into()])
            .expect("decl");
        let dr = self
            .builder
            .build_call(fdecl, &[self.bits_to_f64(av).into()], "sc_fabs")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("fabs")
            .into_float_value();
        self.builder
            .build_store(rk, self.i8t.const_int(3, false))
            .expect("st");
        self.builder.build_store(rv, self.f64_bits(dr)).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        (self.load_i8(rk), self.load_i64(rv))
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_sc_math2(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: MathOp2,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
        bk: IntValue<'ctx>,
        bv: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        self.require_num(f, ak, s.deopt_bb);
        self.require_num(f, bk, s.deopt_bb);
        let x = self.scalar_as_f64(ak, av);
        let y = self.scalar_as_f64(bk, bv);
        let r = self.call_f64(MATH_BINARY[usize::from(op.0)].1, &[x.into(), y.into()]);
        (self.i8t.const_int(3, false), self.f64_bits(r))
    }

    fn lower_sc_op1(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntOp1,
        x: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let one = self.i64t.const_int(1, false);
        let zero = self.i64t.const_zero();
        match op {
            IntOp1::Inc => self.checked_i64_op(f, "llvm.sadd.with.overflow", x, one, s.deopt_bb),
            IntOp1::Dec => self.checked_i64_op(f, "llvm.ssub.with.overflow", x, one, s.deopt_bb),
            IntOp1::Negate => {
                self.checked_i64_op(f, "llvm.ssub.with.overflow", zero, x, s.deopt_bb)
            }
            IntOp1::BitNot => self.builder.build_not(x, "bnot").expect("not"),
            IntOp1::UInc => self.builder.build_int_add(x, one, "uinc").expect("add"),
            IntOp1::UDec => self.builder.build_int_sub(x, one, "udec").expect("sub"),
            IntOp1::UNegate => self.builder.build_int_sub(zero, x, "uneg").expect("sub"),
        }
    }

    fn lower_sc_op2(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        op: IntOp2,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        if let Some(intr) = checked_op2_intrinsic(op) {
            return self.checked_i64_op(f, intr, x, y, s.deopt_bb);
        }
        self.int_binop(f, op2_binop_name(op), x, y, s.deopt_bb)
    }

    fn checked_i64_op(
        &self,
        f: &FnCtx<'ctx>,
        intr_name: &str,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        fail: BasicBlock<'ctx>,
    ) -> IntValue<'ctx> {
        let intr = inkwell::intrinsics::Intrinsic::find(intr_name).expect("intrinsic");
        let decl = intr
            .get_declaration(&self.module, &[self.i64t.into()])
            .expect("decl");
        let pair = self
            .builder
            .build_call(decl, &[x.into(), y.into()], "ov")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("pair")
            .into_struct_value();
        let r = self
            .builder
            .build_extract_value(pair, 0, "r")
            .expect("ext")
            .into_int_value();
        let o = self
            .builder
            .build_extract_value(pair, 1, "o")
            .expect("ext")
            .into_int_value();
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_ov_ok");
        self.builder
            .build_conditional_branch(o, fail, ok_bb)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        r
    }

    fn lower_sc_cast(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c: Cast,
        ak: IntValue<'ctx>,
        av: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        if c == Cast::Double {
            self.require_num(f, ak, s.deopt_bb);
            return (
                self.i8t.const_int(3, false),
                self.f64_bits(self.scalar_as_f64(ak, av)),
            );
        }
        let rv = self.entry_alloca(f, self.i64t, "castv");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_cast_ok");
        if s.allow_fp {
            let dbl_bb = self.ctx.append_basic_block(f.func, "sc_cast_d");
            let non_d = self.ctx.append_basic_block(f.func, "sc_cast_nd");
            self.builder
                .build_conditional_branch(self.is_k_dbl(ak), dbl_bb, non_d)
                .expect("br");
            self.builder.position_at_end(dbl_bb);
            let dv = self.lower_sc_double_to_int(f, s, c == Cast::Long, av);
            self.builder.build_store(rv, dv).expect("st");
            self.builder
                .build_unconditional_branch(done_bb)
                .expect("br");
            self.builder.position_at_end(non_d);
        }
        let b = &self.builder;
        let is_chr = b
            .build_int_compare(IntPredicate::EQ, ak, self.i8t.const_int(1, false), "cc")
            .expect("cmp");
        let is_int = self.is_k_int(ak);
        let ok_kind = match c {
            Cast::Long => b.build_or(is_int, is_chr, "cl_ok").expect("or"),
            _ => {
                let t32 = b.build_int_truncate(av, self.i32t, "t32").expect("trunc");
                let s64 = b.build_int_s_extend(t32, self.i64t, "s64").expect("sext");
                let fits = b
                    .build_int_compare(IntPredicate::EQ, s64, av, "fits")
                    .expect("cmp");
                let ok_i = b.build_and(is_int, fits, "oki").expect("and");
                b.build_or(is_chr, ok_i, "ci_ok").expect("or")
            }
        };
        let keep_bb = self.ctx.append_basic_block(f.func, "sc_cast_k");
        b.build_conditional_branch(ok_kind, keep_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(keep_bb);
        self.builder.build_store(rv, av).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        (self.i8t.const_zero(), self.load_i64(rv))
    }

    fn lower_sc_double_to_int(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        long: bool,
        av: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let d = self.bits_to_f64(av);
        let f64t = self.ctx.f64_type();
        let (lo, hi, hi_pred) = if long {
            (
                f64t.const_float(-9223372036854775808.0),
                f64t.const_float(9223372036854775808.0),
                FloatPredicate::OLT,
            )
        } else {
            (
                f64t.const_float(f64::from(i32::MIN)),
                f64t.const_float(f64::from(i32::MAX)),
                FloatPredicate::OLE,
            )
        };
        let ge_lo = self
            .builder
            .build_float_compare(FloatPredicate::OGE, d, lo, "dlo")
            .expect("fcmp");
        let lt_hi = self
            .builder
            .build_float_compare(hi_pred, d, hi, "dhi")
            .expect("fcmp");
        let in_rng = self.builder.build_and(ge_lo, lt_hi, "drng").expect("and");
        let res = self.entry_alloca(f, self.i64t, "d2ir");
        let conv_bb = self.ctx.append_basic_block(f.func, "sc_d2i_ok");
        let chk_nan = self.ctx.append_basic_block(f.func, "sc_d2i_nan");
        let nan_bb = self.ctx.append_basic_block(f.func, "sc_d2i_z");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_d2i_done");
        self.builder
            .build_conditional_branch(in_rng, conv_bb, chk_nan)
            .expect("br");
        self.builder.position_at_end(conv_bb);
        let iv = self
            .builder
            .build_float_to_signed_int(d, self.i64t, "f2i")
            .expect("fptosi");
        self.builder.build_store(res, iv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(chk_nan);
        let is_nan = self
            .builder
            .build_float_compare(FloatPredicate::UNO, d, d, "isnan")
            .expect("fcmp");
        self.builder
            .build_conditional_branch(is_nan, nan_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(nan_bb);
        self.builder
            .build_store(res, self.i64t.const_zero())
            .expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        self.load_i64(res)
    }

    fn lower_sc_count(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let c = &s.colls[s.coll_idx(loc)];
        let c_slot = c.slot;
        let cnt_ok = self.load_i8(c.cnt_ok);
        let cnt_val = c.cnt_val;
        let res_v = self.entry_alloca(f, self.i64t, "cntres");
        let hit_bb = self.ctx.append_basic_block(f.func, "sc_cnt_hit");
        let bc_bb = self.ctx.append_basic_block(f.func, "sc_cnt_bc");
        let bc_ok_bb = self.ctx.append_basic_block(f.func, "sc_cnt_bcok");
        let slow_bb = self.ctx.append_basic_block(f.func, "sc_cnt_slow");
        let slow_ok = self.ctx.append_basic_block(f.func, "sc_cnt_slok");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_cnt_done");
        let is_cached = self
            .builder
            .build_int_compare(IntPredicate::NE, cnt_ok, self.i8t.const_zero(), "cc")
            .expect("cmp");
        self.builder
            .build_conditional_branch(is_cached, hit_bb, bc_bb)
            .expect("br");
        self.builder.position_at_end(hit_bb);
        let cv = self.load_i64(cnt_val);
        self.builder.build_store(res_v, cv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(bc_bb);
        self.unlock_coll_tvec(f, c);
        let probe = self
            .helper("rt_n_str_ascii", Some(self.i8t.into()), &[self.ptr.into()])
            .as_global_value()
            .as_pointer_value();
        let ok = self
            .call(
                "rt_bc_scalar_count",
                Some(self.i8t),
                &[c_slot.into(), s.scratch_v.into(), probe.into()],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, bc_ok_bb, slow_bb)
            .expect("br");
        self.builder.position_at_end(bc_ok_bb);
        let bv = self.load_i64(s.scratch_v);
        self.builder.build_store(res_v, bv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(slow_bb);
        self.unlock_all_tvecs(f, s);
        let call_bb = self.ctx.append_basic_block(f.func, "sc_cnt_call");
        self.builder
            .build_unconditional_branch(call_bb)
            .expect("br");
        let tmp_out = self.slot(f);
        let k_cnt = self.var("clojure.core", "count");
        self.emit_direct_slow(
            f,
            &[c_slot],
            k_cnt,
            1,
            Some("count"),
            tmp_out,
            call_bb,
            slow_ok,
        );
        let sv = self.int_payload(tmp_out);
        self.builder.build_store(res_v, sv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        (self.i8t.const_zero(), self.load_i64(res_v))
    }

    fn lower_sc_nth(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
        idx_e: &IExpr,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        let (ik, iv) = self.lower_sc_expr(f, s, idx_e)?;
        self.require_int(f, ik, s.deopt_bb);
        let c = &s.colls[s.coll_idx(loc)];
        let c_slot = c.slot;
        let res_k = self.entry_alloca(f, self.i8t, "nthk");
        let res_v = self.entry_alloca(f, self.i64t, "nthv");
        let ivec_chk_bb = self.ctx.append_basic_block(f.func, "sc_nth_ichk");
        let bc_bb = self.ctx.append_basic_block(f.func, "sc_nth_bc");
        let bc_ok_bb = self.ctx.append_basic_block(f.func, "sc_nth_bcok");
        let slow_bb = self.ctx.append_basic_block(f.func, "sc_nth_slow");
        let done_bb = self.ctx.append_basic_block(f.func, "sc_nth_done");
        if c.mutated {
            self.builder
                .build_unconditional_branch(ivec_chk_bb)
                .expect("br");
        } else {
            self.emit_sc_nth_str(f, c, iv, res_k, res_v, ivec_chk_bb, done_bb);
        }
        let after_ivec = if c.dvec.is_some() {
            self.ctx.append_basic_block(f.func, "sc_nth_dchk")
        } else {
            bc_bb
        };
        self.emit_sc_nth_packed64(
            f,
            c.ivec_ptr,
            c.ivec_len,
            0,
            "sc_nth_ivec",
            iv,
            res_k,
            res_v,
            ivec_chk_bb,
            after_ivec,
            done_bb,
        );
        if let Some((dvec_ptr, dvec_len)) = c.dvec {
            let after_dvec = if !c.mutated {
                self.ctx.append_basic_block(f.func, "sc_nth_gchk")
            } else {
                bc_bb
            };
            self.emit_sc_nth_packed64(
                f,
                dvec_ptr,
                dvec_len,
                3,
                "sc_nth_dvec",
                iv,
                res_k,
                res_v,
                after_ivec,
                after_dvec,
                done_bb,
            );
            if !c.mutated {
                self.emit_sc_nth_gvec(f, c, iv, res_k, res_v, after_dvec, bc_bb, done_bb);
            }
        }
        self.builder.position_at_end(bc_bb);
        self.unlock_coll_tvec(f, c);
        let probe = self
            .helper("rt_n_str_ascii", Some(self.i8t.into()), &[self.ptr.into()])
            .as_global_value()
            .as_pointer_value();
        let ok = self
            .call(
                "rt_bc_scalar_nth",
                Some(self.i8t),
                &[
                    c_slot.into(),
                    iv.into(),
                    s.scratch_k.into(),
                    s.scratch_v.into(),
                    probe.into(),
                    self.i8t.const_int(u64::from(s.allow_fp), false).into(),
                ],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, bc_ok_bb, slow_bb)
            .expect("br");
        self.builder.position_at_end(bc_ok_bb);
        self.builder
            .build_store(res_k, self.load_i8(s.scratch_k))
            .expect("st");
        self.builder
            .build_store(res_v, self.load_i64(s.scratch_v))
            .expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.emit_sc_nth_slow(f, s, c_slot, iv, res_k, res_v, slow_bb, done_bb);
        self.builder.position_at_end(done_bb);
        Ok((self.load_i8(res_k), self.load_i64(res_v)))
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_nth_str(
        &self,
        f: &FnCtx<'ctx>,
        c: &CollCache<'ctx>,
        iv: IntValue<'ctx>,
        res_k: PointerValue<'ctx>,
        res_v: PointerValue<'ctx>,
        miss_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        let fast_str = self
            .builder
            .build_int_compare(IntPredicate::ULT, iv, self.load_i64(c.str_len), "fstr")
            .expect("cmp");
        let str_bb = self.ctx.append_basic_block(f.func, "sc_nth_str");
        self.builder
            .build_conditional_branch(fast_str, str_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(str_bb);
        let base = self
            .builder
            .build_load(self.ptr, c.str_ptr, "sp")
            .expect("ld")
            .into_pointer_value();
        let addr = self
            .builder
            .build_int_add(
                self.builder
                    .build_ptr_to_int(base, self.i64t, "p2i")
                    .expect("p2i"),
                iv,
                "addr",
            )
            .expect("add");
        let bp = self
            .builder
            .build_int_to_ptr(addr, self.ptr, "i2p")
            .expect("i2p");
        let ch = self
            .builder
            .build_int_z_extend(self.load_i8(bp), self.i64t, "ch64")
            .expect("zext");
        self.builder
            .build_store(res_k, self.i8t.const_int(1, false))
            .expect("st");
        self.builder.build_store(res_v, ch).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_nth_packed64(
        &self,
        f: &FnCtx<'ctx>,
        ptr_slot: PointerValue<'ctx>,
        len_slot: PointerValue<'ctx>,
        kind: u64,
        bb_name: &str,
        iv: IntValue<'ctx>,
        res_k: PointerValue<'ctx>,
        res_v: PointerValue<'ctx>,
        chk_bb: BasicBlock<'ctx>,
        miss_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(chk_bb);
        let fast = self
            .builder
            .build_int_compare(IntPredicate::ULT, iv, self.load_i64(len_slot), "fp64")
            .expect("cmp");
        let hit_bb = self.ctx.append_basic_block(f.func, bb_name);
        self.builder
            .build_conditional_branch(fast, hit_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(hit_bb);
        let ep = self.ivec_elem_ptr(ptr_slot, iv);
        let val = self.load_i64(ep);
        self.builder
            .build_store(res_k, self.i8t.const_int(kind, false))
            .expect("st");
        self.builder.build_store(res_v, val).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_nth_gvec(
        &self,
        f: &FnCtx<'ctx>,
        c: &CollCache<'ctx>,
        iv: IntValue<'ctx>,
        res_k: PointerValue<'ctx>,
        res_v: PointerValue<'ctx>,
        chk_bb: BasicBlock<'ctx>,
        miss_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(chk_bb);
        let fast = self
            .builder
            .build_int_compare(IntPredicate::ULT, iv, self.load_i64(c.gvec_len), "fgv")
            .expect("cmp");
        let gvec_bb = self.ctx.append_basic_block(f.func, "sc_nth_gvec");
        let hit_bb = self.ctx.append_basic_block(f.func, "sc_nth_ghit");
        self.builder
            .build_conditional_branch(fast, gvec_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(gvec_bb);
        let ep = self.scaled_elem_ptr(c.gvec_ptr, iv, 5);
        let (ok, uk, uv) = self.unbox_scalar(ep, true);
        self.builder
            .build_conditional_branch(ok, hit_bb, miss_bb)
            .expect("br");
        self.builder.position_at_end(hit_bb);
        self.builder.build_store(res_k, uk).expect("st");
        self.builder.build_store(res_v, uv).expect("st");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_sc_nth_slow(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        c_slot: PointerValue<'ctx>,
        iv: IntValue<'ctx>,
        res_k: PointerValue<'ctx>,
        res_v: PointerValue<'ctx>,
        slow_bb: BasicBlock<'ctx>,
        done_bb: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(slow_bb);
        self.unlock_all_tvecs(f, s);
        let tmp_idx = self.slot(f);
        let tmp_out = self.slot(f);
        self.write_int(f.func, tmp_idx, iv);
        let k_nth = self.var("clojure.core", "nth");
        let call_bb = self.ctx.append_basic_block(f.func, "sc_nth_call");
        let slow_ok = self.ctx.append_basic_block(f.func, "sc_nth_slok");
        self.builder
            .build_unconditional_branch(call_bb)
            .expect("br");
        self.emit_direct_slow(
            f,
            &[c_slot, tmp_idx],
            k_nth,
            2,
            Some("nth"),
            tmp_out,
            call_bb,
            slow_ok,
        );
        let (u_ok, uk, uv) = self.unbox_scalar(tmp_out, s.allow_fp);
        self.drop_and_nil_slots(f.func, &[tmp_out]);
        self.builder.build_store(res_k, uk).expect("st");
        self.builder.build_store(res_v, uv).expect("st");
        self.builder
            .build_conditional_branch(u_ok, done_bb, s.deopt_bb)
            .expect("br");
    }

    fn lower_sc_nth2(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        loc: Loc,
        idx_e: &IExpr,
        item_e: &IExpr,
    ) -> Result<(IntValue<'ctx>, IntValue<'ctx>), Unsupported> {
        let (ik, iv) = self.lower_sc_expr(f, s, idx_e)?;
        self.require_int(f, ik, s.deopt_bb);
        let (jk, jv) = self.lower_sc_expr(f, s, item_e)?;
        self.require_int(f, jk, s.deopt_bb);
        let c_slot = s.colls[s.coll_idx(loc)].slot;
        let ok = self
            .call(
                "rt_bc_scalar_nth2",
                Some(self.i8t),
                &[
                    c_slot.into(),
                    iv.into(),
                    jv.into(),
                    s.scratch_k.into(),
                    s.scratch_v.into(),
                    self.i8t.const_int(u64::from(s.allow_fp), false).into(),
                ],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "hit")
            .expect("cmp");
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_nth2_ok");
        self.builder
            .build_conditional_branch(hit, ok_bb, s.deopt_bb)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        Ok((self.load_i8(s.scratch_k), self.load_i64(s.scratch_v)))
    }

    fn is_k_int(&self, k: IntValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_zero(), "ki")
            .expect("cmp")
    }

    fn is_k_dbl(&self, k: IntValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_int(3, false), "kd")
            .expect("cmp")
    }

    fn is_k_num(&self, k: IntValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_or(self.is_k_int(k), self.is_k_dbl(k), "kn")
            .expect("or")
    }

    fn bits_to_f64(&self, v: IntValue<'ctx>) -> FloatValue<'ctx> {
        self.builder
            .build_bit_cast(v, self.ctx.f64_type(), "b2d")
            .expect("bc")
            .into_float_value()
    }

    fn f64_bits(&self, d: FloatValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_bit_cast(d, self.i64t, "d2b")
            .expect("bc")
            .into_int_value()
    }

    fn scalar_as_f64(&self, k: IntValue<'ctx>, v: IntValue<'ctx>) -> FloatValue<'ctx> {
        let as_i = self
            .builder
            .build_signed_int_to_float(v, self.ctx.f64_type(), "i2d")
            .expect("sitofp");
        let as_d = self.bits_to_f64(v);
        self.builder
            .build_select(self.is_k_int(k), as_i, as_d, "fval")
            .expect("sel")
            .into_float_value()
    }

    fn require_num(&self, f: &FnCtx<'ctx>, k: IntValue<'ctx>, fail: BasicBlock<'ctx>) {
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_num_ok");
        self.builder
            .build_conditional_branch(self.is_k_num(k), ok_bb, fail)
            .expect("br");
        self.builder.position_at_end(ok_bb);
    }

    fn require_int(&self, f: &FnCtx<'ctx>, k: IntValue<'ctx>, fail: BasicBlock<'ctx>) {
        let is_i = self
            .builder
            .build_int_compare(IntPredicate::EQ, k, self.i8t.const_zero(), "req_i")
            .expect("cmp");
        let ok_bb = self.ctx.append_basic_block(f.func, "sc_int_ok");
        self.builder
            .build_conditional_branch(is_i, ok_bb, fail)
            .expect("br");
        self.builder.position_at_end(ok_bb);
    }

    fn require_ints(
        &self,
        f: &FnCtx<'ctx>,
        k1: IntValue<'ctx>,
        k2: IntValue<'ctx>,
        fail: BasicBlock<'ctx>,
    ) {
        let both = self.builder.build_or(k1, k2, "req_or").expect("or");
        self.require_int(f, both, fail);
    }

    /// Lower a pure-scalar [`HofLoop`] (`mapv`, `reduce`, `sum`, `every?`,
    /// `count-filter`) to an unboxed SSA loop before the boxed direct loop.
    /// On success, branches to `done`; on any type/overflow deopt or unsupported
    /// input vector layout, falls through to a fresh `shof_boxed` block where
    /// the caller emits `init_hof_acc` + `emit_hof_direct`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_lower_scalar_hof(
        &mut self,
        f: &mut FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
        acc: PointerValue<'ctx>,
        st1: &LoopState<'ctx>,
        st2: Option<&LoopState<'ctx>>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let Some(kind) = ScalarHofKind::of(hl) else {
            return Ok(());
        };
        let Some((ast, scan)) = self.scalar_hof_ast(f, hl, base) else {
            return Ok(());
        };
        let boxed_bb = self.ctx.append_basic_block(f.func, "shof_boxed");
        let deopt_bb = self.ctx.append_basic_block(f.func, "shof_deopt");
        let empty_bb = self.ctx.append_basic_block(f.func, "shof_empty");
        let non_empty_bb = self.ctx.append_basic_block(f.func, "shof_go");
        let is_empty = self
            .builder
            .build_int_compare(IntPredicate::SLE, st1.n, self.i64t.const_zero(), "shof_z")
            .expect("cmp");
        self.builder
            .build_conditional_branch(is_empty, empty_bb, non_empty_bb)
            .expect("br");
        self.builder.position_at_end(empty_bb);
        self.emit_shof_empty(f.func, kind, acc, dest, done);

        self.builder.position_at_end(non_empty_bb);
        let session =
            self.init_hof_session(f, ast.nslots, &ast.global_colls, &scan, boxed_bb, deopt_bb);
        let src1 = self.probe_hof_src(f, st1, st1.n, boxed_bb);
        let src2 = st2.map(|s2| self.probe_hof_src(f, s2, st1.n, boxed_bb));
        match kind {
            ScalarHofKind::Collect => self.emit_shof_collect(
                f,
                &session,
                &ast.expr,
                acc,
                st1.n,
                &src1,
                src2.as_ref(),
                dest,
                done,
                deopt_bb,
                boxed_bb,
            )?,
            ScalarHofKind::Reduce => self.emit_shof_reduce(
                f, &session, &ast.expr, acc, st1.n, &src1, dest, done, deopt_bb, boxed_bb,
            )?,
            ScalarHofKind::Sum { has_init } => self.emit_shof_sum(
                f,
                &session,
                &ast.expr,
                has_init,
                acc,
                st1.n,
                &src1,
                src2.as_ref(),
                dest,
                done,
                deopt_bb,
                boxed_bb,
            )?,
            ScalarHofKind::Every => self.emit_shof_every(
                f, &session, &ast.expr, st1.n, &src1, dest, done, deopt_bb, boxed_bb,
            )?,
            ScalarHofKind::CountFilter => self.emit_shof_count_filter(
                f, &session, &ast.expr, st1.n, &src1, dest, done, deopt_bb, boxed_bb,
            )?,
        }
        self.builder.position_at_end(boxed_bb);
        Ok(())
    }

    fn scalar_hof_ast(
        &self,
        f: &FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
    ) -> Option<(clj_eval::aot_scalar::ScalarFnAst, AstScan)> {
        if !self.inline_arith || !self.use_bitcode {
            return None;
        }
        let n_params = if hl.kind == HofKind::Reduce || hl.coll2.is_some() {
            2
        } else {
            1
        };
        let ast = compile_aot_expr(&hl.arity.body, n_params, base, &self.callees, |v| {
            self.is_stable_global_coll(v)
        })?;
        let mut scan = AstScan::default();
        scan.scan_expr(&ast.expr);
        if scan.has_effects
            || !scan
                .all_locs
                .iter()
                .all(|loc| self.is_direct_loc(f, &ast.global_colls, *loc))
        {
            return None;
        }
        Some((ast, scan))
    }

    fn emit_shof_empty(
        &self,
        func: FunctionValue<'ctx>,
        kind: ScalarHofKind,
        acc: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        match kind {
            ScalarHofKind::Collect => {
                self.call(
                    "rt_n_vec_new",
                    None,
                    &[
                        acc.into(),
                        self.i64t.const_zero().into(),
                        self.i8t.const_zero().into(),
                    ],
                );
                self.call("rt_n_vec_done", None, &[dest.into(), acc.into()]);
                self.drop_and_nil_slots(func, &[acc]);
            }
            ScalarHofKind::Reduce | ScalarHofKind::Sum { has_init: true } => {
                self.move_v(func, dest, acc);
            }
            ScalarHofKind::Sum { has_init: false } | ScalarHofKind::CountFilter => {
                self.write_int(func, dest, self.i64t.const_zero());
            }
            ScalarHofKind::Every => {
                self.write_bool(func, dest, self.ctx.bool_type().const_int(1, false));
            }
        }
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn init_hof_session(
        &mut self,
        f: &mut FnCtx<'ctx>,
        nslots: usize,
        global_colls: &[Var],
        scan: &AstScan,
        boxed_bb: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
    ) -> ScalarSession<'ctx> {
        let mut k_slots = Vec::with_capacity(nslots);
        let mut v_slots = Vec::with_capacity(nslots);
        for _ in 0..nslots {
            k_slots.push(self.entry_alloca(f, self.i8t, "hsck"));
            v_slots.push(self.entry_alloca(f, self.i64t, "hscv"));
        }
        let scratch_k = self.entry_alloca(f, self.i8t, "hscrk");
        let scratch_v = self.entry_alloca(f, self.i64t, "hscrv");
        let mut load_vals = Vec::with_capacity(scan.loads.len());
        for &loc in &scan.loads {
            let slot = self.loc_slot(f, global_colls, loc);
            let (ok, k, v) = self.unbox_scalar(slot, true);
            let next = self.ctx.append_basic_block(f.func, "hsc_loc_ok");
            self.builder
                .build_conditional_branch(ok, next, boxed_bb)
                .expect("br");
            self.builder.position_at_end(next);
            load_vals.push((k, v));
        }
        let mut global_ids = Vec::with_capacity(scan.globals.len());
        let mut global_vals = Vec::with_capacity(scan.globals.len());
        for var in &scan.globals {
            let tmp = self.slot(f);
            let kv = self.var_of(var);
            match self.stable_of(var) {
                Some(sr) => self.clone_stable(f, sr, kv, tmp),
                None => self.emit_deref(f.func, tmp, kv),
            }
            let (ok, k, v) = self.unbox_scalar(tmp, true);
            self.free_slots(f, &[tmp]);
            let next = self.ctx.append_basic_block(f.func, "hsc_glb_ok");
            self.builder
                .build_conditional_branch(ok, next, boxed_bb)
                .expect("br");
            self.builder.position_at_end(next);
            global_ids.push(var.id);
            global_vals.push((k, v));
        }
        let colls = scan
            .colls
            .iter()
            .map(|&loc| self.probe_coll(f, global_colls, loc, scan, true))
            .collect();
        ScalarSession {
            allow_fp: true,
            k_slots,
            v_slots,
            load_locs: scan.loads.clone(),
            load_vals,
            global_ids,
            global_vals,
            coll_locs: scan.colls.clone(),
            colls,
            scratch_k,
            scratch_v,
            deopt_bb,
        }
    }

    fn probe_hof_src(
        &self,
        f: &FnCtx<'ctx>,
        st: &LoopState<'ctx>,
        n: IntValue<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> HofSrc<'ctx> {
        let cur_ptr = self.entry_alloca(f, self.i64t, "hcur");
        let base = self.entry_alloca(f, self.ptr, "hbase");
        let ivn = self.entry_alloca(f, self.i64t, "hivn");
        let dvn = self.entry_alloca(f, self.i64t, "hdvn");
        let z64 = self.i64t.const_zero();
        self.builder.build_store(cur_ptr, st.start).expect("store");
        self.builder.build_store(ivn, z64).expect("store");
        self.builder.build_store(dvn, z64).expect("store");
        let is_rng = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                st.kind,
                self.i8t.const_int(2, false),
                "h_rng",
            )
            .expect("cmp");
        let vec_bb = self.ctx.append_basic_block(f.func, "hsrc_vec");
        let dv_bb = self.ctx.append_basic_block(f.func, "hsrc_dv");
        let ok_bb = self.ctx.append_basic_block(f.func, "hsrc_ok");
        let range_bb = self.builder.get_insert_block().expect("range probe");
        self.builder
            .build_conditional_branch(is_rng, ok_bb, vec_bb)
            .expect("br");
        self.builder.position_at_end(vec_bb);
        self.call(
            "rt_bc_scalar_ivec",
            Some(self.i8t),
            &[st.coll.into(), base.into(), ivn.into()],
        );
        let iv_ok = self
            .builder
            .build_int_compare(IntPredicate::SGE, self.load_i64(ivn), n, "hiv_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(iv_ok, ok_bb, dv_bb)
            .expect("br");
        self.builder.position_at_end(dv_bb);
        self.call(
            "rt_bc_scalar_dvec",
            Some(self.i8t),
            &[st.coll.into(), base.into(), dvn.into()],
        );
        let dv_ok = self
            .builder
            .build_int_compare(IntPredicate::SGE, self.load_i64(dvn), n, "hdv_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(dv_ok, ok_bb, boxed_bb)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        let element_kind = self.builder.build_phi(self.i8t, "hkind").expect("phi");
        let int_kind = self.i8t.const_zero();
        let double_kind = self.i8t.const_int(3, false);
        element_kind.add_incoming(&[
            (&int_kind, range_bb),
            (&int_kind, vec_bb),
            (&double_kind, dv_bb),
        ]);
        HofSrc {
            kind: st.kind,
            step: st.step,
            cur_ptr,
            base,
            element_kind: element_kind.as_basic_value().into_int_value(),
        }
    }

    fn load_hof_src_item(
        &self,
        f: &FnCtx<'ctx>,
        src: &HofSrc<'ctx>,
        i: IntValue<'ctx>,
    ) -> (IntValue<'ctx>, IntValue<'ctx>) {
        let is_rng = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                src.kind,
                self.i8t.const_int(2, false),
                "is_r",
            )
            .expect("cmp");
        let rng_bb = self.ctx.append_basic_block(f.func, "hld_rng");
        let vec_bb = self.ctx.append_basic_block(f.func, "hld_vec");
        let done_bb = self.ctx.append_basic_block(f.func, "hld_done");
        self.builder
            .build_conditional_branch(is_rng, rng_bb, vec_bb)
            .expect("br");
        self.builder.position_at_end(rng_bb);
        let cur = self.load_i64(src.cur_ptr);
        let next = self
            .builder
            .build_int_add(cur, src.step, "hcur_n")
            .expect("add");
        self.builder.build_store(src.cur_ptr, next).expect("store");
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(vec_bb);
        let bits = self.load_i64(self.ivec_elem_ptr(src.base, i));
        self.builder
            .build_unconditional_branch(done_bb)
            .expect("br");
        self.builder.position_at_end(done_bb);
        let v_phi = self.builder.build_phi(self.i64t, "hv").expect("phi");
        v_phi.add_incoming(&[(&cur, rng_bb), (&bits, vec_bb)]);
        (src.element_kind, v_phi.as_basic_value().into_int_value())
    }

    fn store_hof_params(
        &self,
        f: &FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        src1: &HofSrc<'ctx>,
        src2: Option<&HofSrc<'ctx>>,
        i: IntValue<'ctx>,
    ) {
        let (k1, v1) = self.load_hof_src_item(f, src1, i);
        self.builder.build_store(s.k_slots[0], k1).expect("store");
        self.builder.build_store(s.v_slots[0], v1).expect("store");
        if let Some(s2) = src2 {
            let (k2, v2) = self.load_hof_src_item(f, s2, i);
            self.builder.build_store(s.k_slots[1], k2).expect("store");
            self.builder.build_store(s.v_slots[1], v2).expect("store");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_shof_collect(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        expr: &IExpr,
        acc: PointerValue<'ctx>,
        n: IntValue<'ctx>,
        src1: &HofSrc<'ctx>,
        src2: Option<&HofSrc<'ctx>>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let out_ptr = self.entry_alloca(f, self.ptr, "ho_p");
        let out_len = self.entry_alloca(f, self.i64t, "ho_n");
        let out_kind = self.entry_alloca(f, self.i8t, "ho_k");
        let i_ptr = self.entry_alloca(f, self.i64t, "ho_i");
        let unlocked = self.i8t.const_int(255, false);
        self.builder.build_store(out_kind, unlocked).expect("store");

        self.store_hof_params(f, s, src1, src2, self.i64t.const_zero());
        let (k0, v0) = self.lower_sc_expr(f, s, expr)?;
        let is_i = self
            .builder
            .build_int_compare(IntPredicate::EQ, k0, self.i8t.const_zero(), "ho_is_i")
            .expect("cmp");
        let is_d = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                k0,
                self.i8t.const_int(3, false),
                "ho_is_d",
            )
            .expect("cmp");
        let alloc_i = self.ctx.append_basic_block(f.func, "ho_ai");
        let chk_d = self.ctx.append_basic_block(f.func, "ho_chkd");
        let alloc_d = self.ctx.append_basic_block(f.func, "ho_ad");
        let locked = self.ctx.append_basic_block(f.func, "ho_lk");
        self.builder
            .build_conditional_branch(is_i, alloc_i, chk_d)
            .expect("br");
        self.builder.position_at_end(chk_d);
        self.builder
            .build_conditional_branch(is_d, alloc_d, deopt_bb)
            .expect("br");
        self.builder.position_at_end(alloc_i);
        self.call(
            "rt_n_vec_new",
            None,
            &[acc.into(), n.into(), self.i8t.const_int(1, false).into()],
        );
        self.call(
            "rt_bc_scalar_tvec_lock",
            Some(self.i8t),
            &[acc.into(), out_ptr.into(), out_len.into()],
        );
        self.builder.build_unconditional_branch(locked).expect("br");
        self.builder.position_at_end(alloc_d);
        self.call(
            "rt_n_vec_new",
            None,
            &[acc.into(), n.into(), self.i8t.const_int(2, false).into()],
        );
        self.call(
            "rt_bc_scalar_dtvec_lock",
            Some(self.i8t),
            &[acc.into(), out_ptr.into(), out_len.into()],
        );
        self.builder.build_unconditional_branch(locked).expect("br");

        self.builder.position_at_end(locked);
        self.builder.build_store(out_kind, k0).expect("store");
        let base_p = self
            .builder
            .build_load(self.ptr, out_ptr, "ho_bp")
            .expect("load")
            .into_pointer_value();
        self.builder.build_store(base_p, v0).expect("store");
        self.builder
            .build_store(i_ptr, self.i64t.const_int(1, false))
            .expect("store");

        let hdr = self.ctx.append_basic_block(f.func, "ho_hdr");
        let body_bb = self.ctx.append_basic_block(f.func, "ho_body");
        let store_bb = self.ctx.append_basic_block(f.func, "ho_st");
        let fin = self.ctx.append_basic_block(f.func, "ho_fin");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let i = self.load_i64(i_ptr);
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "ho_more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body_bb, fin)
            .expect("br");
        self.builder.position_at_end(body_bb);
        self.store_hof_params(f, s, src1, src2, i);
        let (rk, rv) = self.lower_sc_expr(f, s, expr)?;
        let same_k = self
            .builder
            .build_int_compare(IntPredicate::EQ, rk, self.load_i8(out_kind), "ho_sk")
            .expect("cmp");
        self.builder
            .build_conditional_branch(same_k, store_bb, deopt_bb)
            .expect("br");
        self.builder.position_at_end(store_bb);
        let ep = self.ivec_elem_ptr(out_ptr, i);
        self.builder.build_store(ep, rv).expect("store");
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "ho_in")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");

        self.builder.position_at_end(fin);
        self.call("rt_bc_scalar_tvec_unlock", None, &[acc.into()]);
        self.call("rt_n_vec_done", None, &[dest.into(), acc.into()]);
        self.drop_and_nil_slots(f.func, &[acc]);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(deopt_bb);
        let was_lk = self
            .builder
            .build_int_compare(IntPredicate::NE, self.load_i8(out_kind), unlocked, "ho_wlk")
            .expect("cmp");
        let ulk = self.ctx.append_basic_block(f.func, "ho_ulk");
        self.builder
            .build_conditional_branch(was_lk, ulk, boxed_bb)
            .expect("br");
        self.builder.position_at_end(ulk);
        self.call("rt_bc_scalar_tvec_unlock", None, &[acc.into()]);
        self.drop_and_nil_slots(f.func, &[acc]);
        self.builder
            .build_unconditional_branch(boxed_bb)
            .expect("br");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_shof_reduce(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        expr: &IExpr,
        acc: PointerValue<'ctx>,
        n: IntValue<'ctx>,
        src1: &HofSrc<'ctx>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let (ok, k0, v0) = self.unbox_scalar(acc, true);
        let hdr = self.ctx.append_basic_block(f.func, "hr_hdr");
        let go = self.ctx.append_basic_block(f.func, "hr_go");
        let body_bb = self.ctx.append_basic_block(f.func, "hr_body");
        let fin = self.ctx.append_basic_block(f.func, "hr_fin");
        self.builder
            .build_conditional_branch(ok, go, boxed_bb)
            .expect("br");
        self.builder.position_at_end(go);
        self.builder.build_store(s.k_slots[0], k0).expect("store");
        self.builder.build_store(s.v_slots[0], v0).expect("store");
        let i_ptr = self.entry_alloca(f, self.i64t, "hr_i");
        self.builder
            .build_store(i_ptr, self.i64t.const_zero())
            .expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let i = self.load_i64(i_ptr);
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "hr_more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body_bb, fin)
            .expect("br");
        self.builder.position_at_end(body_bb);
        let (xk, xv) = self.load_hof_src_item(f, src1, i);
        self.builder.build_store(s.k_slots[1], xk).expect("store");
        self.builder.build_store(s.v_slots[1], xv).expect("store");
        let (rk, rv) = self.lower_sc_expr(f, s, expr)?;
        self.builder.build_store(s.k_slots[0], rk).expect("store");
        self.builder.build_store(s.v_slots[0], rv).expect("store");
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "hr_in")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(fin);
        self.box_scalar(
            f.func,
            dest,
            self.load_i8(s.k_slots[0]),
            self.load_i64(s.v_slots[0]),
        );
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(deopt_bb);
        self.builder
            .build_unconditional_branch(boxed_bb)
            .expect("br");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_shof_sum(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        expr: &IExpr,
        has_init: bool,
        acc: PointerValue<'ctx>,
        n: IntValue<'ctx>,
        src1: &HofSrc<'ctx>,
        src2: Option<&HofSrc<'ctx>>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let sum_k = self.entry_alloca(f, self.i8t, "hs_k");
        let sum_v = self.entry_alloca(f, self.i64t, "hs_v");
        let i_ptr = self.entry_alloca(f, self.i64t, "hs_i");
        if has_init {
            let (ok, k0, v0) = self.unbox_scalar(acc, true);
            let num_ok = self
                .builder
                .build_and(ok, self.is_k_num(k0), "hs_nok")
                .expect("and");
            let go = self.ctx.append_basic_block(f.func, "hs_go");
            self.builder
                .build_conditional_branch(num_ok, go, boxed_bb)
                .expect("br");
            self.builder.position_at_end(go);
            self.builder.build_store(sum_k, k0).expect("store");
            self.builder.build_store(sum_v, v0).expect("store");
            self.builder
                .build_store(i_ptr, self.i64t.const_zero())
                .expect("store");
        } else {
            self.store_hof_params(f, s, src1, src2, self.i64t.const_zero());
            let (k0, v0) = self.lower_sc_expr(f, s, expr)?;
            self.require_num(f, k0, deopt_bb);
            self.builder.build_store(sum_k, k0).expect("store");
            self.builder.build_store(sum_v, v0).expect("store");
            self.builder
                .build_store(i_ptr, self.i64t.const_int(1, false))
                .expect("store");
        }
        let hdr = self.ctx.append_basic_block(f.func, "hs_hdr");
        let body_bb = self.ctx.append_basic_block(f.func, "hs_body");
        let fin = self.ctx.append_basic_block(f.func, "hs_fin");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let i = self.load_i64(i_ptr);
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "hs_more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body_bb, fin)
            .expect("br");
        self.builder.position_at_end(body_bb);
        self.store_hof_params(f, s, src1, src2, i);
        let (rk, rv) = self.lower_sc_expr(f, s, expr)?;
        let (nk, nv) = self.lower_sc_op2_any(
            f,
            s,
            IntOp2::Add,
            self.load_i8(sum_k),
            self.load_i64(sum_v),
            rk,
            rv,
        );
        self.builder.build_store(sum_k, nk).expect("store");
        self.builder.build_store(sum_v, nv).expect("store");
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "hs_in")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(fin);
        self.box_scalar(f.func, dest, self.load_i8(sum_k), self.load_i64(sum_v));
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(deopt_bb);
        self.builder
            .build_unconditional_branch(boxed_bb)
            .expect("br");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_shof_every(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        expr: &IExpr,
        n: IntValue<'ctx>,
        src1: &HofSrc<'ctx>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let i_ptr = self.entry_alloca(f, self.i64t, "he_i");
        self.builder
            .build_store(i_ptr, self.i64t.const_zero())
            .expect("store");
        let hdr = self.ctx.append_basic_block(f.func, "he_hdr");
        let body_bb = self.ctx.append_basic_block(f.func, "he_body");
        let step_bb = self.ctx.append_basic_block(f.func, "he_step");
        let stop_bb = self.ctx.append_basic_block(f.func, "he_stop");
        let fin = self.ctx.append_basic_block(f.func, "he_fin");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let i = self.load_i64(i_ptr);
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "he_more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body_bb, fin)
            .expect("br");
        self.builder.position_at_end(body_bb);
        self.store_hof_params(f, s, src1, None, i);
        let (rk, rv) = self.lower_sc_expr(f, s, expr)?;
        let truthy = self.scalar_truthy(rk, rv);
        self.builder
            .build_conditional_branch(truthy, step_bb, stop_bb)
            .expect("br");
        self.builder.position_at_end(step_bb);
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "he_in")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(stop_bb);
        self.write_bool(f.func, dest, self.ctx.bool_type().const_zero());
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(fin);
        self.write_bool(f.func, dest, self.ctx.bool_type().const_int(1, false));
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(deopt_bb);
        self.builder
            .build_unconditional_branch(boxed_bb)
            .expect("br");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_shof_count_filter(
        &mut self,
        f: &mut FnCtx<'ctx>,
        s: &ScalarSession<'ctx>,
        expr: &IExpr,
        n: IntValue<'ctx>,
        src1: &HofSrc<'ctx>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
        deopt_bb: BasicBlock<'ctx>,
        boxed_bb: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let i_ptr = self.entry_alloca(f, self.i64t, "hcf_i");
        let cnt_ptr = self.entry_alloca(f, self.i64t, "hcf_c");
        self.builder
            .build_store(i_ptr, self.i64t.const_zero())
            .expect("store");
        self.builder
            .build_store(cnt_ptr, self.i64t.const_zero())
            .expect("store");
        let hdr = self.ctx.append_basic_block(f.func, "hcf_hdr");
        let body_bb = self.ctx.append_basic_block(f.func, "hcf_body");
        let fin = self.ctx.append_basic_block(f.func, "hcf_fin");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(hdr);
        let i = self.load_i64(i_ptr);
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, n, "hcf_more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body_bb, fin)
            .expect("br");
        self.builder.position_at_end(body_bb);
        self.store_hof_params(f, s, src1, None, i);
        let (rk, rv) = self.lower_sc_expr(f, s, expr)?;
        let truthy = self.scalar_truthy(rk, rv);
        let inc = self
            .builder
            .build_int_z_extend(truthy, self.i64t, "hcf_inc")
            .expect("zext");
        let cnt_next = self
            .builder
            .build_int_add(self.load_i64(cnt_ptr), inc, "hcf_cn")
            .expect("add");
        self.builder.build_store(cnt_ptr, cnt_next).expect("store");
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "hcf_in")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        self.builder.build_unconditional_branch(hdr).expect("br");
        self.builder.position_at_end(fin);
        self.write_int(f.func, dest, self.load_i64(cnt_ptr));
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(deopt_bb);
        self.builder
            .build_unconditional_branch(boxed_bb)
            .expect("br");
        Ok(())
    }
}

/// Probed input collection source for a scalar HOF loop (either a long range
/// or a flat packed `Int`/`Double` vector).
#[derive(Clone, Copy)]
enum ScalarHofKind {
    Collect,
    Reduce,
    Sum { has_init: bool },
    Every,
    CountFilter,
}

impl ScalarHofKind {
    fn of(hl: &HofLoop<'_>) -> Option<Self> {
        Some(match hl.kind {
            HofKind::Collect => Self::Collect,
            HofKind::Reduce => Self::Reduce,
            HofKind::Sum => Self::Sum {
                has_init: hl.init.is_some(),
            },
            HofKind::Every => Self::Every,
            HofKind::CountFilter => Self::CountFilter,
            HofKind::Each | HofKind::FilterCollect | HofKind::Some => return None,
        })
    }
}

struct HofSrc<'ctx> {
    kind: IntValue<'ctx>,
    step: IntValue<'ctx>,
    cur_ptr: PointerValue<'ctx>,
    base: PointerValue<'ctx>,
    element_kind: IntValue<'ctx>,
}

fn init_needs_fp(e: &Expr, read_only_nth: bool) -> bool {
    match e {
        Expr::Literal(Value::Double(_)) => true,
        Expr::DirectNative(dc) => {
            let name = dc.var.sym.name();
            name == "double" || (read_only_nth && matches!(name, "nth" | "first" | "get"))
        }
        _ => false,
    }
}

fn loc_reg_idx(loc: Loc) -> usize {
    match loc {
        Loc::Reg(idx) => idx,
        Loc::Up(_) | Loc::Rec(_) => usize::MAX,
    }
}

fn checked_op2_intrinsic(op: IntOp2) -> Option<&'static str> {
    match op {
        IntOp2::Add => Some("llvm.sadd.with.overflow"),
        IntOp2::Sub => Some("llvm.ssub.with.overflow"),
        IntOp2::Mul => Some("llvm.smul.with.overflow"),
        _ => None,
    }
}

fn op2_binop_name(op: IntOp2) -> &'static str {
    const NAMES: [(&IntOp2, &str); 15] = [
        (&IntOp2::UAdd, "unchecked-add"),
        (&IntOp2::USub, "unchecked-subtract"),
        (&IntOp2::UMul, "unchecked-multiply"),
        (&IntOp2::Quot, "quot"),
        (&IntOp2::Rem, "rem"),
        (&IntOp2::RemUnsigned, "urem"),
        (&IntOp2::Mod, "mod"),
        (&IntOp2::Max, "max"),
        (&IntOp2::Min, "min"),
        (&IntOp2::BitAnd, "bit-and"),
        (&IntOp2::BitOr, "bit-or"),
        (&IntOp2::BitXor, "bit-xor"),
        (&IntOp2::Shl, "bit-shift-left"),
        (&IntOp2::Shr, "bit-shift-right"),
        (&IntOp2::Ushr, "unsigned-bit-shift-right"),
    ];
    NAMES
        .iter()
        .find(|(k, _)| **k == op)
        .map(|(_, v)| *v)
        .expect("non-checked IntOp2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loc_reg_idx_maps_non_reg_to_max() {
        assert_eq!(loc_reg_idx(Loc::Reg(2)), 2);
        assert_eq!(loc_reg_idx(Loc::Up(0)), usize::MAX);
        assert_eq!(loc_reg_idx(Loc::Rec(0)), usize::MAX);
    }
}
