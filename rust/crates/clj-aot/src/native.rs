// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Native lowering of analyzed programs (issue #259 S1).
//!
//! Input: the build front end's top-level units ([`clj_eval::aot_front`]),
//! i.e. the interpreter's own analyzed [`Expr`] trees. Output: one
//! LLVM function per top-level unit and per fn arity, a constant
//! initializer, and a `main` that hands them to the `clj-rt` native
//! runtime (`rt_n_*`).
//!
//! Value ABI: every value lives by value in a 32-byte slot
//! (`[4 x i64]`), zero-initialized (= nil) at function entry and
//! dropped at both exits. Helpers borrow inputs and replace outputs.
//! A failing helper returns 1; generated code branches to the
//! function's fail block, which drops every slot and returns 1.
//!
//! Locals follow the analyzer's absolute slot numbering: `Local(i)`
//! indexes the frame chain, so a fn created at frame length `base`
//! captures exactly the `Local(i < base)` its arities mention.

use std::collections::{BTreeSet, HashMap};

use clj_eval::aot_front::{FrontEnd, for_each_child};
use clj_eval::expr::{Arity, CaseTest, Expr, FunExpr, NsClause, Pattern};
use clj_eval::native_abi;
use clj_value::{Keyword, Map, Symbol, Value, Vector};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{ArrayType, BasicMetadataTypeEnum, FunctionType, IntType, PointerType};
use inkwell::values::{BasicMetadataValueEnum, BasicValue, FunctionValue, IntValue, PointerValue};

/// "No index" for optional table indices (matches `clj-rt`).
const NONE: u64 = u64::MAX;

/// Why a program cannot (yet) be lowered natively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

/// A constant-pool entry.
#[derive(Clone, Debug, PartialEq)]
enum Const {
    Nil,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
    Keyword(Option<String>, String),
    Symbol(Option<String>, String),
    Regex(String),
    StaticField(String, String),
    Class(String),
    Read(String),
}

/// One compiled fn: display plus `(fixed, variadic, code)` arities.
struct FnInfo<'ctx> {
    display: String,
    arities: Vec<(usize, bool, FunctionValue<'ctx>)>,
}

/// How a local slot index is reached inside the current function.
#[derive(Clone, Copy)]
enum LocalRef<'ctx> {
    /// A slot of this function.
    Slot(PointerValue<'ctx>),
    /// A `letfn` cell held in a slot of this function.
    Cell(PointerValue<'ctx>),
    /// Captured value `i` of this closure.
    Captured(usize),
    /// Captured `letfn` cell `i` of this closure.
    CapturedCell(usize),
    /// Not visible here (never referenced by this function's body).
    Absent,
}

/// A `recur` target: its loop header and the patterns rebound from
/// the recur values (each pattern's slots in binding order).
struct RecurTarget<'ctx> {
    header: BasicBlock<'ctx>,
    patterns: Vec<(Pattern, Vec<PointerValue<'ctx>>)>,
}

/// Per-function lowering state.
struct FnCtx<'ctx> {
    func: FunctionValue<'ctx>,
    entry: BasicBlock<'ctx>,
    fail: BasicBlock<'ctx>,
    slots: Vec<PointerValue<'ctx>>,
    env: Vec<LocalRef<'ctx>>,
    recur: Vec<RecurTarget<'ctx>>,
    captured: PointerValue<'ctx>,
}

/// The program-level lowering session.
pub struct Lowerer<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    ptr: PointerType<'ctx>,
    i8t: IntType<'ctx>,
    i32t: IntType<'ctx>,
    i64t: IntType<'ctx>,
    slot_ty: ArrayType<'ctx>,
    code_ty: FunctionType<'ctx>,
    consts: Vec<Const>,
    const_index: HashMap<String, usize>,
    vars: Vec<(String, String)>,
    var_index: HashMap<(String, String), usize>,
    fns: Vec<FnInfo<'ctx>>,
    strings: HashMap<String, PointerValue<'ctx>>,
    fn_seq: usize,
}

impl<'ctx> Lowerer<'ctx> {
    /// A fresh session over a new module.
    #[must_use]
    pub fn new(ctx: &'ctx Context) -> Self {
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let i32t = ctx.i32_type();
        let code_ty = i32t.fn_type(
            &[ptr.into(), ptr.into(), i64t.into(), ptr.into()],
            false,
        );
        Self {
            ctx,
            module: ctx.create_module("clj_native"),
            builder: ctx.create_builder(),
            ptr,
            i8t: ctx.i8_type(),
            i32t,
            i64t,
            slot_ty: i64t.array_type(4),
            code_ty,
            consts: Vec::new(),
            const_index: HashMap::new(),
            vars: Vec::new(),
            var_index: HashMap::new(),
            fns: Vec::new(),
            strings: HashMap::new(),
            fn_seq: 0,
        }
    }

    /// Lower a whole program to a verified module.
    pub fn lower_program(
        mut self,
        front: &FrontEnd,
        path: &str,
    ) -> Result<Module<'ctx>, Unsupported> {
        let mut tops = Vec::with_capacity(front.units.len());
        for (i, unit) in front.units.iter().enumerate() {
            tops.push(self.lower_thunk(&format!("cljtop_{i}"), &unit.expr)?);
        }
        let init = self.lower_const_init();
        self.lower_main(path, init, &tops);
        crate::lower::verify_module(&self.module).map_err(Unsupported)?;
        Ok(self.module)
    }

    // ---- helpers: declarations, constants, strings ---------------------

    fn helper(&self, name: &str, ret: Option<IntType<'ctx>>, params: &[BasicMetadataTypeEnum<'ctx>]) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let ty = match ret {
            Some(r) => r.fn_type(params, false),
            None => self.ctx.void_type().fn_type(params, false),
        };
        self.module.add_function(name, ty, Some(Linkage::External))
    }

    fn call(&self, name: &str, ret: Option<IntType<'ctx>>, args: &[BasicMetadataValueEnum<'ctx>]) -> Option<IntValue<'ctx>> {
        let params: Vec<BasicMetadataTypeEnum> = args
            .iter()
            .map(|a| match a {
                BasicMetadataValueEnum::PointerValue(_) => self.ptr.into(),
                BasicMetadataValueEnum::IntValue(v) => v.get_type().into(),
                BasicMetadataValueEnum::FloatValue(v) => v.get_type().into(),
                _ => self.ptr.into(),
            })
            .collect();
        let f = self.helper(name, ret, &params);
        let site = self.builder.build_call(f, args, "").expect("call");
        ret.map(|_| site.try_as_basic_value().expect_basic("status").into_int_value())
    }

    fn u64c(&self, n: u64) -> BasicMetadataValueEnum<'ctx> {
        self.i64t.const_int(n, false).into()
    }

    fn str_ptr(&mut self, text: &str) -> (PointerValue<'ctx>, u64) {
        let len = text.len() as u64;
        if let Some(p) = self.strings.get(text) {
            return (*p, len);
        }
        let name = format!("cljs{}", self.strings.len());
        let value = self.ctx.const_string(text.as_bytes(), false);
        let global = self.module.add_global(value.get_type(), None, &name);
        global.set_initializer(&value);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        let p = global.as_pointer_value();
        self.strings.insert(text.to_owned(), p);
        (p, len)
    }

    fn constant(&mut self, c: Const) -> u64 {
        let key = format!("{c:?}");
        if let Some(k) = self.const_index.get(&key) {
            return *k as u64;
        }
        let k = self.consts.len();
        self.consts.push(c);
        self.const_index.insert(key, k);
        k as u64
    }

    /// A value as a constant-pool entry (scalars directly, everything
    /// else through the printed-and-read fallback, checked to round
    /// trip at build time).
    fn value_const(&mut self, v: &Value) -> Result<u64, Unsupported> {
        let c = match v {
            Value::Nil => Const::Nil,
            Value::Bool(b) => Const::Bool(*b),
            Value::Int(n) => Const::Int(*n),
            Value::Double(d) if d.is_finite() => Const::Double(*d),
            Value::Str(s) => Const::Str(s.to_string()),
            Value::Keyword(k) => Const::Keyword(k.namespace().map(str::to_owned), k.name().to_owned()),
            Value::Symbol(s) if s.meta().is_none() => {
                Const::Symbol(s.namespace().map(str::to_owned), s.name().to_owned())
            }
            Value::Regex(p) => Const::Regex(p.to_string()),
            Value::Class(_) => Const::Class(native_abi::class_name_of(v).unwrap_or_default()),
            other if native_abi::static_field_origin(other).is_some() => {
                let (class, field) = native_abi::static_field_origin(other).expect("checked");
                Const::StaticField(class.to_owned(), field.to_owned())
            }
            other => {
                let text = native_abi::print_constant(other);
                match native_abi::read_constant(&text) {
                    Ok(back) if back == *other => Const::Read(text),
                    _ => {
                        return Err(Unsupported(format!(
                            "constant without a readable form: {text}"
                        )));
                    }
                }
            }
        };
        Ok(self.constant(c))
    }

    fn var(&mut self, ns: &str, name: &str) -> u64 {
        let key = (ns.to_owned(), name.to_owned());
        if let Some(k) = self.var_index.get(&key) {
            return *k as u64;
        }
        let k = self.vars.len();
        self.vars.push(key.clone());
        self.var_index.insert(key, k);
        k as u64
    }

    fn var_of(&mut self, var: &clj_value::Var) -> u64 {
        let ns = var.sym.namespace().unwrap_or("user").to_owned();
        let name = var.sym.name().to_owned();
        self.var(&ns, &name)
    }

    // ---- function scaffolding ------------------------------------------

    fn begin_fn(&mut self, name: &str, env: Vec<LocalRef<'ctx>>) -> FnCtx<'ctx> {
        let func = self.module.add_function(name, self.code_ty, Some(Linkage::Private));
        let entry = self.ctx.append_basic_block(func, "entry");
        let start = self.ctx.append_basic_block(func, "start");
        let fail = self.ctx.append_basic_block(func, "fail");
        self.builder.position_at_end(start);
        let captured = func.get_nth_param(3).expect("captured").into_pointer_value();
        FnCtx {
            func,
            entry,
            fail,
            slots: Vec::new(),
            env,
            recur: Vec::new(),
            captured,
        }
    }

    /// Finish a function whose result is in `result`: entry inits every
    /// slot to nil; the exit moves the result out and drops every slot.
    fn end_fn(&mut self, f: FnCtx<'ctx>, result: PointerValue<'ctx>) -> FunctionValue<'ctx> {
        let ret = f.func.get_nth_param(0).expect("ret").into_pointer_value();
        self.call("rt_n_clone", None, &[ret.into(), result.into()]);
        self.drop_slots(&f.slots);
        self.builder
            .build_return(Some(&self.i32t.const_zero()))
            .expect("ret");
        self.builder.position_at_end(f.fail);
        self.drop_slots(&f.slots);
        self.builder
            .build_return(Some(&self.i32t.const_int(1, false)))
            .expect("ret");
        // Entry: zero-init the slots (nil), then into the body.
        let first = f.func.get_basic_blocks()[1];
        self.builder.position_at_end(f.entry);
        let zero = self.slot_ty.const_zero();
        for slot in &f.slots {
            self.builder.build_store(*slot, zero).expect("init");
        }
        self.builder
            .build_unconditional_branch(first)
            .expect("br");
        f.func
    }

    fn drop_slots(&self, slots: &[PointerValue<'ctx>]) {
        for slot in slots {
            self.call("rt_n_drop", None, &[(*slot).into()]);
        }
    }

    /// A fresh nil slot of the current function.
    fn slot(&self, f: &mut FnCtx<'ctx>) -> PointerValue<'ctx> {
        let here = self.builder.get_insert_block().expect("positioned");
        self.builder.position_at_end(f.entry);
        let p = self.builder.build_alloca(self.slot_ty, "v").expect("alloca");
        p.as_instruction_value()
            .expect("alloca instr")
            .set_alignment(8)
            .expect("align");
        self.builder.position_at_end(here);
        f.slots.push(p);
        p
    }

    /// `n` contiguous fresh slots: returns the base pointer and each
    /// element pointer.
    fn slot_array(&self, f: &mut FnCtx<'ctx>, n: usize) -> (PointerValue<'ctx>, Vec<PointerValue<'ctx>>) {
        if n == 0 {
            return (self.ptr.const_null(), Vec::new());
        }
        let here = self.builder.get_insert_block().expect("positioned");
        self.builder.position_at_end(f.entry);
        let arr_ty = self.slot_ty.array_type(n as u32);
        let base = self.builder.build_alloca(arr_ty, "arr").expect("alloca");
        base.as_instruction_value()
            .expect("alloca instr")
            .set_alignment(8)
            .expect("align");
        let elems: Vec<PointerValue> = (0..n).map(|i| self.elem(base, i)).collect();
        self.builder.position_at_end(here);
        f.slots.extend(elems.iter().copied());
        (base, elems)
    }

    /// Pointer to slot `i` of a slot array.
    fn elem(&self, base: PointerValue<'ctx>, i: usize) -> PointerValue<'ctx> {
        let idx = [self.i64t.const_int(i as u64, false)];
        // SAFETY: every caller indexes within the array it allocated
        // (or within a caller-provided array of known length).
        unsafe { self.builder.build_in_bounds_gep(self.slot_ty, base, &idx, "e") }.expect("gep")
    }

    /// Branch to the fail block when `status != 0`.
    fn check(&self, f: &FnCtx<'ctx>, status: IntValue<'ctx>) {
        let bad = self
            .builder
            .build_int_compare(IntPredicate::NE, status, status.get_type().const_zero(), "bad")
            .expect("cmp");
        let ok = self.ctx.append_basic_block(f.func, "ok");
        self.builder
            .build_conditional_branch(bad, f.fail, ok)
            .expect("br");
        self.builder.position_at_end(ok);
    }

    /// Continue in a fresh unreachable block after a terminator.
    fn dead_end(&self, f: &FnCtx<'ctx>) {
        let dead = self.ctx.append_basic_block(f.func, "dead");
        self.builder.position_at_end(dead);
    }

    // ---- units and fns ---------------------------------------------------

    fn lower_thunk(&mut self, name: &str, expr: &Expr) -> Result<FunctionValue<'ctx>, Unsupported> {
        let mut f = self.begin_fn(name, Vec::new());
        let result = self.slot(&mut f);
        self.lower(&mut f, expr, result)?;
        Ok(self.end_fn(f, result))
    }

    /// Compile `fun` as a closure created at frame length `base` in
    /// the current function `outer`: returns the fn table index and
    /// the captured local indices (in capture order).
    fn lower_fun(&mut self, fun: &FunExpr, base: usize, outer: &FnCtx<'ctx>) -> Result<(u64, Vec<usize>), Unsupported> {
        let mut free = BTreeSet::new();
        for arity in fun.arities.iter() {
            arity_free(arity, base, &mut free);
        }
        let captured: Vec<usize> = free.into_iter().collect();
        let mut env = vec![LocalRef::Absent; base];
        for (pos, idx) in captured.iter().enumerate() {
            env[*idx] = match outer.env.get(*idx) {
                Some(LocalRef::Cell(_) | LocalRef::CapturedCell(_)) => LocalRef::CapturedCell(pos),
                _ => LocalRef::Captured(pos),
            };
        }
        let k = self.fns.len();
        self.fns.push(FnInfo {
            display: fun.display.clone(),
            arities: Vec::new(),
        });
        let here = self.builder.get_insert_block();
        let mut arities = Vec::new();
        for (i, arity) in fun.arities.iter().enumerate() {
            self.fn_seq += 1;
            let name = format!("cljfn_{}_{i}_{}", self.fn_seq, sanitize(&fun.display));
            let code = self.lower_arity(&name, arity, env.clone())?;
            arities.push((arity.fixed.len(), arity.rest.is_some(), code));
        }
        self.fns[k].arities = arities;
        if let Some(b) = here {
            self.builder.position_at_end(b);
        }
        Ok((k as u64, captured))
    }

    fn lower_arity(&mut self, name: &str, arity: &Arity, env: Vec<LocalRef<'ctx>>) -> Result<FunctionValue<'ctx>, Unsupported> {
        let mut f = self.begin_fn(name, env);
        let args = f.func.get_nth_param(1).expect("args").into_pointer_value();
        let pats: Vec<&Pattern> = arity.fixed.iter().chain(arity.rest.iter()).collect();
        // Params: each arg cloned into its own slot, then bound.
        let mut arg_slots = Vec::with_capacity(pats.len());
        for i in 0..pats.len() {
            let s = self.slot(&mut f);
            let src = self.elem(args, i);
            self.call("rt_n_clone", None, &[s.into(), src.into()]);
            arg_slots.push(s);
        }
        let mut targets = Vec::with_capacity(pats.len());
        for (pat, s) in pats.iter().zip(&arg_slots) {
            let slots = self.bind_pattern(&mut f, pat, *s)?;
            targets.push(((*pat).clone(), slots));
        }
        let header = self.ctx.append_basic_block(f.func, "body");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget {
            header,
            patterns: targets,
        });
        let result = self.slot(&mut f);
        self.lower_body(&mut f, &arity.body, result)?;
        Ok(self.end_fn(f, result))
    }

    // ---- patterns ----------------------------------------------------------

    /// Bind `pat` to the value in `src`: allocates the pattern's local
    /// slots (in `pattern_names` order), pushes them on the frame, and
    /// returns them.
    fn bind_pattern(&mut self, f: &mut FnCtx<'ctx>, pat: &Pattern, src: PointerValue<'ctx>) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
        match pat {
            Pattern::Symbol(_) => {
                let s = self.slot(f);
                self.call("rt_n_clone", None, &[s.into(), src.into()]);
                f.env.push(LocalRef::Slot(s));
                Ok(vec![s])
            }
            Pattern::Vec(v) => {
                let mut out = Vec::new();
                if let Some(rest) = &v.rest {
                    let cur = self.slot(f);
                    let st = self.call("rt_n_gseq", Some(self.i32t), &[cur.into(), src.into()]).expect("st");
                    self.check(f, st);
                    for fixed in &v.fixed {
                        let item = self.slot(f);
                        let st = self.call("rt_n_first", Some(self.i32t), &[item.into(), cur.into()]).expect("st");
                        self.check(f, st);
                        out.extend(self.bind_pattern(f, fixed, item)?);
                        let st = self.call("rt_n_next", Some(self.i32t), &[cur.into(), cur.into()]).expect("st");
                        self.check(f, st);
                    }
                    out.extend(self.bind_pattern(f, rest, cur)?);
                } else {
                    for (i, fixed) in v.fixed.iter().enumerate() {
                        let item = self.slot(f);
                        let st = self
                            .call("rt_n_nth_nil", Some(self.i32t), &[item.into(), src.into(), self.u64c(i as u64)])
                            .expect("st");
                        self.check(f, st);
                        out.extend(self.bind_pattern(f, fixed, item)?);
                    }
                }
                if v.as_sym.is_some() {
                    let s = self.slot(f);
                    self.call("rt_n_clone", None, &[s.into(), src.into()]);
                    f.env.push(LocalRef::Slot(s));
                    out.push(s);
                }
                Ok(out)
            }
            Pattern::Map(_) => {
                // The interpreter's own destructure over the encoded
                // pattern, `:or` defaults evaluated here first (eager,
                // in the enclosing frame, like the interpreter).
                let (enc, defaults) = native_abi::encode_pattern(pat);
                let k = self.value_const(&enc)?;
                let dexprs: Vec<Expr> = defaults.into_iter().cloned().collect();
                let darr = self.lower_args(f, &dexprs)?;
                let n = native_abi::pattern_local_count(pat);
                let (oarr, outs) = self.slot_array(f, n);
                let st = self
                    .call("rt_n_destructure", Some(self.i32t), &[
                        oarr.into(),
                        self.u64c(n as u64),
                        src.into(),
                        self.u64c(k),
                        darr.into(),
                        self.u64c(dexprs.len() as u64),
                    ])
                    .expect("st");
                self.check(f, st);
                for s in &outs {
                    f.env.push(LocalRef::Slot(*s));
                }
                Ok(outs)
            }
        }
    }

    /// Rebind `pat`'s existing slots from `src` (for `recur`).
    fn rebind_pattern(&mut self, f: &mut FnCtx<'ctx>, pat: &Pattern, src: PointerValue<'ctx>, slots: &[PointerValue<'ctx>]) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let fresh = self.bind_pattern(f, pat, src)?;
        f.env.truncate(depth);
        for (dst, s) in slots.iter().zip(fresh) {
            self.call("rt_n_clone", None, &[(*dst).into(), s.into()]);
            self.call("rt_n_drop", None, &[s.into()]);
        }
        Ok(())
    }

    // ---- expressions -------------------------------------------------------

    fn lower_body(&mut self, f: &mut FnCtx<'ctx>, body: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let Some((last, init)) = body.split_last() else {
            self.call("rt_n_nil", None, &[dest.into()]);
            return Ok(());
        };
        for e in init {
            let t = self.slot(f);
            self.lower(f, e, t)?;
            self.call("rt_n_drop", None, &[t.into()]);
        }
        self.lower(f, last, dest)
    }

    /// Lower each expression into its own slot of a fresh array.
    fn lower_args(&mut self, f: &mut FnCtx<'ctx>, args: &[Expr]) -> Result<PointerValue<'ctx>, Unsupported> {
        let (base, elems) = self.slot_array(f, args.len());
        for (a, e) in args.iter().zip(elems) {
            self.lower(f, a, e)?;
        }
        Ok(base)
    }

    fn lower_local(&mut self, f: &FnCtx<'ctx>, idx: usize, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        match f.env.get(idx).copied().unwrap_or(LocalRef::Absent) {
            LocalRef::Slot(p) => {
                self.call("rt_n_clone", None, &[dest.into(), p.into()]);
            }
            LocalRef::Cell(p) => {
                self.call("rt_n_cell_get", None, &[dest.into(), p.into()]);
            }
            LocalRef::Captured(i) => {
                let p = self.elem(f.captured, i);
                self.call("rt_n_clone", None, &[dest.into(), p.into()]);
            }
            LocalRef::CapturedCell(i) => {
                let p = self.elem(f.captured, i);
                self.call("rt_n_cell_get", None, &[dest.into(), p.into()]);
            }
            LocalRef::Absent => return Err(Unsupported(format!("unresolved local {idx}"))),
        }
        Ok(())
    }

    /// The raw slot holding local `idx` (a cell local yields the cell).
    fn local_slot(&self, f: &FnCtx<'ctx>, idx: usize) -> Option<PointerValue<'ctx>> {
        match f.env.get(idx).copied()? {
            LocalRef::Slot(p) | LocalRef::Cell(p) => Some(p),
            LocalRef::Captured(i) | LocalRef::CapturedCell(i) => Some(self.elem(f.captured, i)),
            LocalRef::Absent => None,
        }
    }

    fn lower(&mut self, f: &mut FnCtx<'ctx>, expr: &Expr, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        match expr {
            Expr::Literal(v) | Expr::Quote(v) => self.lower_const(v, dest),
            Expr::Local(i) => self.lower_local(f, *i, dest),
            Expr::Global(var) => {
                let k = self.var_of(var);
                self.call("rt_n_deref", None, &[dest.into(), self.u64c(k)]);
                Ok(())
            }
            Expr::VarRef(var) => {
                let k = self.var_of(var);
                self.call("rt_n_varref", None, &[dest.into(), self.u64c(k)]);
                Ok(())
            }
            Expr::VecLit(items, meta) => self.lower_coll(f, "rt_n_vec_lit", items, meta.as_ref(), dest, false),
            Expr::SetLit(items, meta) => self.lower_coll(f, "rt_n_set_lit", items, meta.as_ref(), dest, true),
            Expr::MapLit(pairs, meta) => {
                let flat: Vec<Expr> = pairs.iter().flat_map(|(k, v)| [k.clone(), v.clone()]).collect();
                self.lower_coll(f, "rt_n_map_lit", &flat, meta.as_ref(), dest, true)
            }
            Expr::If { test, then, els } => self.lower_if(f, test, then, els, dest),
            Expr::Do(body) => self.lower_body(f, body, dest),
            Expr::Let { bindings, body } => {
                let depth = f.env.len();
                let mut bound = Vec::new();
                for (pat, init) in bindings {
                    let t = self.slot(f);
                    self.lower(f, init, t)?;
                    bound.extend(self.bind_pattern(f, pat, t)?);
                    self.call("rt_n_drop", None, &[t.into()]);
                }
                self.lower_body(f, body, dest)?;
                f.env.truncate(depth);
                // Release let-bound values at scope end (RSS parity).
                self.drop_slots(&bound);
                Ok(())
            }
            Expr::Loop { bindings, body } => self.lower_loop(f, bindings, body, dest),
            Expr::Recur(args) => self.lower_recur(f, args),
            Expr::Fn(fun) => {
                let base = f.env.len();
                self.make_closure(f, fun, base, dest)
            }
            Expr::LetFn { fns, body } => self.lower_letfn(f, fns, body, dest),
            Expr::Invoke { head, args, .. } => {
                let h = self.slot(f);
                self.lower(f, head, h)?;
                let arr = self.lower_args(f, args)?;
                let st = self
                    .call("rt_n_invoke", Some(self.i32t), &[dest.into(), h.into(), arr.into(), self.u64c(args.len() as u64)])
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::DirectNative(call) => {
                let k = self.var_of(&call.var);
                let h = self.slot(f);
                self.call("rt_n_deref", None, &[h.into(), self.u64c(k)]);
                let arr = self.lower_args(f, &call.args)?;
                let st = self
                    .call("rt_n_invoke", Some(self.i32t), &[dest.into(), h.into(), arr.into(), self.u64c(call.args.len() as u64)])
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Def { sym, doc, init, meta, span } => self.lower_def(f, sym, doc.as_deref(), init.as_deref(), meta.as_deref(), *span, dest),
            Expr::Throw(e) => {
                let t = self.slot(f);
                self.lower(f, e, t)?;
                self.call("rt_n_throw", Some(self.i32t), &[t.into()]);
                self.builder.build_unconditional_branch(f.fail).expect("br");
                self.dead_end(f);
                Ok(())
            }
            Expr::NewThrowable { class, message } => {
                let c = self.constant(Const::Str(class.jvm_name().to_owned()));
                let m = self.constant(Const::Str(message.clone()));
                self.call("rt_n_new_throwable", None, &[dest.into(), self.u64c(c), self.u64c(m)]);
                Ok(())
            }
            Expr::Try { body, catches, finally_body } => self.lower_try(f, body, catches, finally_body, dest),
            Expr::Set { var, val } => {
                let k = self.var_of(var);
                let t = self.slot(f);
                self.lower(f, val, t)?;
                let st = self.call("rt_n_set_var", Some(self.i32t), &[dest.into(), self.u64c(k), t.into()]).expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Binding { pairs, body } => self.lower_binding(f, pairs, body, dest),
            Expr::Declare { syms, span } => {
                let v = Value::Vector(Vector::from_vec(syms.iter().cloned().map(Value::Symbol).collect()));
                let k = self.value_const(&v)?;
                self.call("rt_n_declare", None, &[dest.into(), self.u64c(k), self.i32t.const_int(u64::from(span.line), false).into(), self.i32t.const_int(u64::from(span.col), false).into()]);
                Ok(())
            }
            Expr::Ns { name, doc, attr, clauses } => {
                let v = ns_desc(name, doc.as_deref(), attr.as_ref(), clauses);
                let k = self.value_const(&v)?;
                let st = self.call("rt_n_ns", Some(self.i32t), &[dest.into(), self.u64c(k)]).expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::CaseStar { expr, shift, mask, default, cases, skip, test } => {
                self.lower_case(f, expr, *shift, *mask, default, cases, skip, *test, dest)
            }
            other => Err(Unsupported(format!("form {}", expr_kind(other)))),
        }
    }

    fn lower_const(&mut self, v: &Value, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        match v {
            Value::Nil => {
                self.call("rt_n_nil", None, &[dest.into()]);
            }
            Value::Int(n) => {
                self.call("rt_n_int", None, &[dest.into(), self.i64t.const_int(*n as u64, true).into()]);
            }
            Value::Bool(b) => {
                self.call("rt_n_bool", None, &[dest.into(), self.i8t.const_int(u64::from(*b), false).into()]);
            }
            Value::Var(var) => {
                let k = self.var_of(var);
                self.call("rt_n_varref", None, &[dest.into(), self.u64c(k)]);
            }
            other => {
                let k = self.value_const(other)?;
                self.call("rt_n_const", None, &[dest.into(), self.u64c(k)]);
            }
        }
        Ok(())
    }

    fn meta_const(&mut self, meta: Option<&Map>) -> Result<u64, Unsupported> {
        match meta {
            Some(m) => self.value_const(&Value::Map(m.clone())),
            None => Ok(NONE),
        }
    }

    fn lower_coll(&mut self, f: &mut FnCtx<'ctx>, helper: &str, items: &[Expr], meta: Option<&Map>, dest: PointerValue<'ctx>, fallible: bool) -> Result<(), Unsupported> {
        let mk = self.meta_const(meta)?;
        let arr = self.lower_args(f, items)?;
        let args = [dest.into(), arr.into(), self.u64c(items.len() as u64), self.u64c(mk)];
        if fallible {
            let st = self.call(helper, Some(self.i32t), &args).expect("st");
            self.check(f, st);
        } else {
            self.call(helper, None, &args);
        }
        Ok(())
    }

    fn truthy(&self, v: PointerValue<'ctx>) -> IntValue<'ctx> {
        let t = self.call("rt_n_truthy", Some(self.i8t), &[v.into()]).expect("truthy");
        self.builder
            .build_int_compare(IntPredicate::NE, t, self.i8t.const_zero(), "truthy")
            .expect("cmp")
    }

    fn lower_if(&mut self, f: &mut FnCtx<'ctx>, test: &Expr, then: &Expr, els: &Expr, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let t = self.slot(f);
        self.lower(f, test, t)?;
        let cond = self.truthy(t);
        let then_b = self.ctx.append_basic_block(f.func, "then");
        let else_b = self.ctx.append_basic_block(f.func, "else");
        let merge = self.ctx.append_basic_block(f.func, "merge");
        self.builder.build_conditional_branch(cond, then_b, else_b).expect("br");
        self.builder.position_at_end(then_b);
        self.lower(f, then, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(else_b);
        self.lower(f, els, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(merge);
        Ok(())
    }

    fn lower_loop(&mut self, f: &mut FnCtx<'ctx>, bindings: &[(Pattern, Expr)], body: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let mut targets = Vec::new();
        for (pat, init) in bindings {
            let t = self.slot(f);
            self.lower(f, init, t)?;
            let slots = self.bind_pattern(f, pat, t)?;
            targets.push((pat.clone(), slots));
        }
        let header = self.ctx.append_basic_block(f.func, "loop");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget { header, patterns: targets });
        let result = self.lower_body(f, body, dest);
        f.recur.pop();
        f.env.truncate(depth);
        result
    }

    fn lower_recur(&mut self, f: &mut FnCtx<'ctx>, args: &[Expr]) -> Result<(), Unsupported> {
        let fresh: Vec<PointerValue> = {
            let mut out = Vec::with_capacity(args.len());
            for a in args {
                let t = self.slot(f);
                self.lower(f, a, t)?;
                out.push(t);
            }
            out
        };
        let Some(target) = f.recur.pop() else {
            return Err(Unsupported("recur without target".to_owned()));
        };
        let depth = f.env.len();
        // Rebinding pushes env entries per pattern position: rebuild
        // the env so later patterns see earlier ones like a fresh bind.
        for ((pat, slots), src) in target.patterns.iter().zip(&fresh) {
            self.rebind_pattern(f, pat, *src, slots)?;
        }
        f.env.truncate(depth);
        for t in &fresh {
            self.call("rt_n_drop", None, &[(*t).into()]);
        }
        self.builder.build_unconditional_branch(target.header).expect("br");
        f.recur.push(target);
        self.dead_end(f);
        Ok(())
    }

    /// Build the captured array for a fn created at `base` and make the
    /// closure value in `dest`.
    fn make_closure(&mut self, f: &mut FnCtx<'ctx>, fun: &FunExpr, base: usize, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let (k, captured) = self.lower_fun(fun, base, f)?;
        let (arr, elems) = self.slot_array(f, captured.len());
        for (idx, e) in captured.iter().zip(&elems) {
            let src = self
                .local_slot(f, *idx)
                .ok_or_else(|| Unsupported(format!("captured local {idx} unresolved")))?;
            self.call("rt_n_clone", None, &[(*e).into(), src.into()]);
        }
        self.call("rt_n_make_fn", None, &[dest.into(), self.u64c(k), arr.into(), self.u64c(captured.len() as u64)]);
        // The closure owns copies; release ours now.
        self.drop_slots(&elems);
        Ok(())
    }

    fn lower_letfn(&mut self, f: &mut FnCtx<'ctx>, fns: &[(String, FunExpr)], body: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let mut cells = Vec::with_capacity(fns.len());
        for _ in fns {
            let c = self.slot(f);
            self.call("rt_n_cell_new", None, &[c.into()]);
            f.env.push(LocalRef::Cell(c));
            cells.push(c);
        }
        let base = f.env.len();
        for ((_, fun), cell) in fns.iter().zip(&cells) {
            let t = self.slot(f);
            self.make_closure(f, fun, base, t)?;
            self.call("rt_n_cell_set", None, &[(*cell).into(), t.into()]);
            self.call("rt_n_drop", None, &[t.into()]);
        }
        let r = self.lower_body(f, body, dest);
        f.env.truncate(depth);
        r
    }

    /// A body run under a dynamic extent: compiled as a zero-argument
    /// closure (or a one-argument handler binding at the current
    /// frame length) whose value lands in `dest`.
    fn body_closure(&mut self, f: &mut FnCtx<'ctx>, params: usize, body: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let fun = FunExpr {
            display: "clojure.core/fn".to_owned(),
            arities: std::sync::Arc::from([Arity {
                fixed: (0..params).map(|i| Pattern::Symbol(format!("p{i}"))).collect(),
                rest: None,
                body: body.to_vec(),
            }]),
        };
        let base = f.env.len();
        self.make_closure(f, &fun, base, dest)
    }

    fn lower_try(&mut self, f: &mut FnCtx<'ctx>, body: &[Expr], catches: &[clj_eval::expr::Catch], fin: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let b = self.slot(f);
        self.body_closure(f, 0, body, b)?;
        let (harr, hs) = self.slot_array(f, catches.len());
        let mut classes = Vec::new();
        for (c, h) in catches.iter().zip(&hs) {
            if c.slot != f.env.len() {
                return Err(Unsupported("catch slot mismatch".to_owned()));
            }
            self.body_closure(f, 1, &c.body, *h)?;
            classes.push(Value::Str(std::sync::Arc::from(c.class.as_str())));
        }
        let ck = self.value_const(&Value::Vector(Vector::from_vec(classes)))?;
        let fin_ptr = if fin.is_empty() {
            self.ptr.const_null()
        } else {
            let s = self.slot(f);
            self.body_closure(f, 0, fin, s)?;
            s
        };
        let st = self
            .call("rt_n_try", Some(self.i32t), &[dest.into(), b.into(), self.u64c(ck), harr.into(), self.u64c(catches.len() as u64), fin_ptr.into()])
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    fn lower_binding(&mut self, f: &mut FnCtx<'ctx>, pairs: &[(clj_value::Var, Expr)], body: &[Expr], dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let exprs: Vec<Expr> = pairs.iter().map(|(_, e)| e.clone()).collect();
        let vals = self.lower_args(f, &exprs)?;
        let ks: Vec<u64> = pairs.iter().map(|(v, _)| self.var_of(v)).collect();
        let karr = self.u64_array(&ks);
        let b = self.slot(f);
        self.body_closure(f, 0, body, b)?;
        let st = self
            .call("rt_n_binding", Some(self.i32t), &[dest.into(), karr.into(), vals.into(), self.u64c(pairs.len() as u64), b.into()])
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    fn u64_array(&mut self, values: &[u64]) -> PointerValue<'ctx> {
        if values.is_empty() {
            return self.ptr.const_null();
        }
        let consts: Vec<IntValue> = values.iter().map(|v| self.i64t.const_int(*v, false)).collect();
        let arr = self.i64t.const_array(&consts);
        self.fn_seq += 1;
        let g = self.module.add_global(arr.get_type(), None, &format!("cljk{}", self.fn_seq));
        g.set_initializer(&arr);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.as_pointer_value()
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_def(&mut self, f: &mut FnCtx<'ctx>, sym: &Symbol, doc: Option<&str>, init: Option<&Expr>, meta: Option<&[(Expr, Expr)]>, span: clj_reader::Span, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let sk = self.value_const(&Value::Symbol(sym.clone()))?;
        let dk = match doc {
            Some(d) => self.constant(Const::Str(d.to_owned())),
            None => NONE,
        };
        let id = self.call("rt_n_def_begin", Some(self.i64t), &[self.u64c(sk)]).expect("id");
        let value = match init {
            Some(e) => {
                let t = self.slot(f);
                self.lower(f, e, t)?;
                t
            }
            None => self.ptr.const_null(),
        };
        let (marr, n_meta) = match meta {
            Some(pairs) => {
                let flat: Vec<Expr> = pairs.iter().flat_map(|(k, v)| [k.clone(), v.clone()]).collect();
                (self.lower_args(f, &flat)?, flat.len() as u64)
            }
            None => (self.ptr.const_null(), NONE),
        };
        let st = self
            .call("rt_n_def_finish", Some(self.i32t), &[
                dest.into(),
                id.into(),
                self.u64c(sk),
                self.u64c(dk),
                value.into(),
                marr.into(),
                self.u64c(n_meta),
                self.i32t.const_int(u64::from(span.line), false).into(),
                self.i32t.const_int(u64::from(span.col), false).into(),
            ])
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_case(&mut self, f: &mut FnCtx<'ctx>, expr: &Expr, shift: i32, mask: i32, default: &Expr, cases: &[(i32, Expr, Expr)], skip: &[i32], test: CaseTest, dest: PointerValue<'ctx>) -> Result<(), Unsupported> {
        let v = self.slot(f);
        self.lower(f, expr, v)?;
        let desc = Value::Vector(Vector::from_vec(vec![
            Value::Int(i64::from(shift)),
            Value::Int(i64::from(mask)),
            Value::Int(match test {
                CaseTest::Int => 0,
                CaseTest::HashEquiv => 1,
                CaseTest::HashIdentity => 2,
            }),
            Value::Vector(Vector::from_vec(cases.iter().map(|(k, _, _)| Value::Int(i64::from(*k))).collect())),
            Value::Vector(Vector::from_vec(skip.iter().map(|k| Value::Int(i64::from(*k))).collect())),
        ]));
        let ck = self.value_const(&desc)?;
        let sel = {
            let here = self.builder.get_insert_block().expect("positioned");
            self.builder.position_at_end(f.entry);
            let p = self.builder.build_alloca(self.i64t, "sel").expect("alloca");
            self.builder.position_at_end(here);
            p
        };
        let realized = self.slot(f);
        let st = self
            .call("rt_n_case", Some(self.i32t), &[sel.into(), realized.into(), v.into(), self.u64c(ck)])
            .expect("st");
        self.check(f, st);
        let selv = self.builder.build_load(self.i64t, sel, "sel").expect("load").into_int_value();
        let idx = self
            .builder
            .build_and(selv, self.i64t.const_int(0xffff_ffff, false), "idx")
            .expect("and");
        let checked = self
            .builder
            .build_right_shift(selv, self.i64t.const_int(32, false), false, "chk")
            .expect("shr");
        let default_b = self.ctx.append_basic_block(f.func, "case_default");
        let merge = self.ctx.append_basic_block(f.func, "case_end");
        let is_default = self
            .builder
            .build_int_compare(IntPredicate::EQ, selv, self.i64t.const_all_ones(), "dflt")
            .expect("cmp");
        let dispatch = self.ctx.append_basic_block(f.func, "case_dispatch");
        self.builder.build_conditional_branch(is_default, default_b, dispatch).expect("br");
        self.builder.position_at_end(dispatch);
        let mut arms = Vec::new();
        let mut blocks = Vec::new();
        for i in 0..cases.len() {
            let b = self.ctx.append_basic_block(f.func, "case_arm");
            arms.push((self.i64t.const_int(i as u64, false), b));
            blocks.push(b);
        }
        self.builder.build_switch(idx, default_b, &arms).expect("switch");
        for ((_, t, then), b) in cases.iter().zip(blocks) {
            self.builder.position_at_end(b);
            let do_check = self
                .builder
                .build_int_compare(IntPredicate::NE, checked, self.i64t.const_zero(), "c")
                .expect("cmp");
            let check_b = self.ctx.append_basic_block(f.func, "case_check");
            let then_b = self.ctx.append_basic_block(f.func, "case_then");
            self.builder.build_conditional_branch(do_check, check_b, then_b).expect("br");
            self.builder.position_at_end(check_b);
            let tv = self.slot(f);
            self.lower(f, t, tv)?;
            let eq = {
                let here = self.builder.get_insert_block().expect("positioned");
                self.builder.position_at_end(f.entry);
                let p = self.builder.build_alloca(self.i8t, "eq").expect("alloca");
                self.builder.position_at_end(here);
                p
            };
            let st = self
                .call("rt_n_equiv", Some(self.i32t), &[eq.into(), realized.into(), tv.into()])
                .expect("st");
            self.check(f, st);
            let eqv = self.builder.build_load(self.i8t, eq, "eqv").expect("load").into_int_value();
            let hit = self
                .builder
                .build_int_compare(IntPredicate::NE, eqv, self.i8t.const_zero(), "hit")
                .expect("cmp");
            self.builder.build_conditional_branch(hit, then_b, default_b).expect("br");
            self.builder.position_at_end(then_b);
            self.lower(f, then, dest)?;
            self.builder.build_unconditional_branch(merge).expect("br");
        }
        self.builder.position_at_end(default_b);
        self.lower(f, default, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(merge);
        Ok(())
    }

    // ---- program wiring ------------------------------------------------------

    fn lower_const_init(&mut self) -> FunctionValue<'ctx> {
        let mut f = self.begin_fn("cljconst_init", Vec::new());
        let t = self.slot(&mut f);
        let consts = self.consts.clone();
        for (k, c) in consts.iter().enumerate() {
            match c {
                Const::Nil => {
                    self.call("rt_n_nil", None, &[t.into()]);
                }
                Const::Bool(b) => {
                    self.call("rt_n_bool", None, &[t.into(), self.i8t.const_int(u64::from(*b), false).into()]);
                }
                Const::Int(n) => {
                    self.call("rt_n_int", None, &[t.into(), self.i64t.const_int(*n as u64, true).into()]);
                }
                Const::Double(d) => {
                    let fv = self.ctx.f64_type().const_float(*d);
                    self.call("rt_n_double", None, &[t.into(), fv.into()]);
                }
                Const::Str(s) => {
                    let (p, len) = self.str_ptr(s);
                    self.call("rt_n_str", None, &[t.into(), p.into(), self.u64c(len)]);
                }
                Const::Keyword(ns, name) | Const::Symbol(ns, name) => {
                    let (np, nl) = match ns {
                        Some(ns) => self.str_ptr(ns),
                        None => (self.ptr.const_null(), NONE),
                    };
                    let (p, len) = self.str_ptr(name);
                    let h = if matches!(c, Const::Keyword(..)) { "rt_n_kw" } else { "rt_n_sym" };
                    self.call(h, None, &[t.into(), np.into(), self.u64c(nl), p.into(), self.u64c(len)]);
                }
                Const::Regex(text) => {
                    let (p, len) = self.str_ptr(text);
                    self.call("rt_n_regex", None, &[t.into(), p.into(), self.u64c(len)]);
                }
                Const::StaticField(class, field) => {
                    let (cp, cl) = self.str_ptr(class);
                    let (fp, fl) = self.str_ptr(field);
                    self.call("rt_n_static_field", None, &[t.into(), cp.into(), self.u64c(cl), fp.into(), self.u64c(fl)]);
                }
                Const::Class(name) => {
                    let (p, len) = self.str_ptr(name);
                    self.call("rt_n_class", None, &[t.into(), p.into(), self.u64c(len)]);
                }
                Const::Read(text) => {
                    let (p, len) = self.str_ptr(text);
                    let st = self.call("rt_n_read", Some(self.i32t), &[t.into(), p.into(), self.u64c(len)]).expect("st");
                    self.check(&f, st);
                }
            }
            self.call("rt_n_set_const", None, &[self.u64c(k as u64), t.into()]);
        }
        let result = self.slot(&mut f);
        self.end_fn(f, result)
    }

    fn lower_main(&mut self, path: &str, init: FunctionValue<'ctx>, tops: &[FunctionValue<'ctx>]) {
        let ty = self.i32t.fn_type(&[self.i32t.into(), self.ptr.into()], false);
        let main = self.module.add_function("main", ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        let vars = self.vars.clone();
        for (k, (ns, name)) in vars.iter().enumerate() {
            let (np, nl) = self.str_ptr(ns);
            let (p, len) = self.str_ptr(name);
            self.call("rt_n_declare_var", None, &[self.u64c(k as u64), np.into(), self.u64c(nl), p.into(), self.u64c(len)]);
        }
        let fns: Vec<(String, Vec<(usize, bool, FunctionValue)>)> =
            self.fns.iter().map(|i| (i.display.clone(), i.arities.clone())).collect();
        for (k, (display, arities)) in fns.iter().enumerate() {
            let (dp, dl) = self.str_ptr(display);
            let fixed: Vec<u64> = arities.iter().map(|a| a.0 as u64).collect();
            let fixed_p = self.u64_array(&fixed);
            let var_bytes: Vec<IntValue> = arities.iter().map(|a| self.i8t.const_int(u64::from(a.1), false)).collect();
            let var_arr = self.i8t.const_array(&var_bytes);
            self.fn_seq += 1;
            let vg = self.module.add_global(var_arr.get_type(), None, &format!("cljv{}", self.fn_seq));
            vg.set_initializer(&var_arr);
            vg.set_constant(true);
            vg.set_linkage(Linkage::Private);
            let code_ptrs: Vec<PointerValue> = arities.iter().map(|a| a.2.as_global_value().as_pointer_value()).collect();
            let code_arr = self.ptr.const_array(&code_ptrs);
            let cg = self.module.add_global(code_arr.get_type(), None, &format!("cljc{}", self.fn_seq));
            cg.set_initializer(&code_arr);
            cg.set_constant(true);
            cg.set_linkage(Linkage::Private);
            self.call("rt_n_declare_fn", None, &[
                self.u64c(k as u64),
                dp.into(),
                self.u64c(dl),
                self.u64c(arities.len() as u64),
                fixed_p.into(),
                vg.as_pointer_value().into(),
                cg.as_pointer_value().into(),
            ]);
        }
        let top_ptrs: Vec<PointerValue> = tops.iter().map(|t| t.as_global_value().as_pointer_value()).collect();
        let tops_g = {
            let arr = self.ptr.const_array(&top_ptrs);
            let g = self.module.add_global(arr.get_type(), None, "cljtops");
            g.set_initializer(&arr);
            g.set_constant(true);
            g.set_linkage(Linkage::Private);
            g.as_pointer_value()
        };
        let (pp, pl) = self.str_ptr(path);
        let argc = main.get_nth_param(0).expect("argc");
        let argv = main.get_nth_param(1).expect("argv");
        let code = self
            .call("rt_n_main", Some(self.i32t), &[
                pp.into(),
                self.u64c(pl),
                argv.into(),
                argc.into(),
                init.as_global_value().as_pointer_value().into(),
                tops_g.into(),
                self.u64c(tops.len() as u64),
            ])
            .expect("code");
        self.builder.build_return(Some(&code)).expect("ret");
    }
}

/// The free locals (`< base`) an arity mentions, nested fns included
/// (absolute slot numbering makes nesting transparent).
fn arity_free(arity: &Arity, base: usize, out: &mut BTreeSet<usize>) {
    fn walk(e: &Expr, base: usize, out: &mut BTreeSet<usize>) {
        if let Expr::Local(i) = e {
            if *i < base {
                out.insert(*i);
            }
        }
        if let Expr::CaseStar { .. } = e {
            // children walked below like any other
        }
        for_each_child(e, &mut |c| walk(c, base, out));
    }
    let probe = Expr::Fn(FunExpr {
        display: String::new(),
        arities: std::sync::Arc::from([arity.clone()]),
    });
    for_each_child(&probe, &mut |c| walk(c, base, out));
}

/// The `ns` constant descriptor (see `clj_rt::native::run_ns`).
fn ns_desc(name: &str, doc: Option<&str>, attr: Option<&Map>, clauses: &[NsClause]) -> Value {
    let mut items = vec![
        Value::Str(std::sync::Arc::from(name)),
        doc.map_or(Value::Nil, |d| Value::Str(std::sync::Arc::from(d))),
        attr.map_or(Value::Nil, |a| Value::Map(a.clone())),
    ];
    for c in clauses {
        let (kind, vals) = match c {
            NsClause::ReferClojure(v) => (0, v),
            NsClause::Require(v) => (1, v),
            NsClause::Use(v) => (2, v),
            NsClause::Import(v) => (3, v),
        };
        let mut row = vec![Value::Int(kind)];
        row.extend(vals.iter().cloned());
        items.push(Value::Vector(Vector::from_vec(row)));
    }
    Value::Vector(Vector::from_vec(items))
}

/// A short name for an `Expr` variant (diagnostics).
fn expr_kind(e: &Expr) -> String {
    let d = format!("{e:?}");
    let end = d.find(|c: char| !c.is_alphanumeric()).unwrap_or(d.len());
    d[..end].to_owned()
}

/// A symbol-safe fragment of a display name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(40)
        .collect()
}

// Keep `Keyword` in scope for future builders (constant kinds).
#[allow(dead_code)]
fn _kw(k: &Keyword) -> &Keyword {
    k
}
