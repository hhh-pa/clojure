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

use std::collections::{BTreeMap, BTreeSet, HashMap};

use clj_eval::aot_front::{FrontEnd, for_each_child};
use clj_eval::expr::{Arity, CaseTest, Expr, FunExpr, NsClause, Pattern};
use clj_eval::native_abi;
use clj_value::{Map, Symbol, Value, Vector};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{
    ArrayType, BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, IntType, PointerType,
};
use inkwell::values::{BasicMetadataValueEnum, BasicValue, FunctionValue, IntValue, PointerValue};

/// "No index" for optional table indices (matches `clj-rt`).
const NONE: u64 = u64::MAX;

/// `Value` tags the inline paths test (pinned by `value_layout_abi`).
const TAG_INT: u64 = 2;
const TAG_CHAR: u64 = 9;

/// Why a program cannot (yet) be lowered natively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported(pub String);

/// A constant-pool entry.
#[derive(Clone, Debug, PartialEq)]
enum Const {
    // (`nil`, booleans, and ints are written inline, never pooled.)
    Double(f64),
    Str(String),
    Keyword(Option<String>, String),
    Symbol(Option<String>, String),
    Regex(String),
    StaticField(String, String),
    Class(String),
    Read(String),
}

/// One compiled arity: `(fixed, variadic, code)`.
type ArityCode<'ctx> = (usize, bool, FunctionValue<'ctx>);

/// One compiled fn: display plus its arities.
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
}

/// A `recur` target: its loop header and the patterns rebound from
/// the recur values (each pattern's slots in binding order).
struct RecurTarget<'ctx> {
    header: BasicBlock<'ctx>,
    patterns: Vec<(Pattern, Vec<PointerValue<'ctx>>)>,
    /// A `loop`'s env depth: its locals (and the body's) are dead once
    /// `recur` rebinds, so their last uses may move. `None` for a fn
    /// arity (its params live in the caller's argument array).
    base: Option<usize>,
}

/// Per-function lowering state.
struct FnCtx<'ctx> {
    func: FunctionValue<'ctx>,
    entry: BasicBlock<'ctx>,
    fail: BasicBlock<'ctx>,
    slots: Vec<PointerValue<'ctx>>,
    env: Vec<Option<LocalRef<'ctx>>>,
    recur: Vec<RecurTarget<'ctx>>,
    captured: PointerValue<'ctx>,
    /// Locals whose single remaining use (in the `recur` args being
    /// lowered) may take the value by move.
    moves: Vec<usize>,
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
    /// Inline the analyzer's `FastArith` int ops (off when the program
    /// can rebind vars, so core arithmetic always means the native).
    inline_arith: bool,
    /// Top-level `defn`s reserved before lowering (by `FunExpr`
    /// address): fn index and pre-declared arity functions.
    reserved: HashMap<usize, (usize, Vec<FunctionValue<'ctx>>)>,
    /// Direct-call targets by var: fn index and `(fixed, variadic,
    /// code)` arities.
    direct: HashMap<(String, String), (usize, Vec<ArityCode<'ctx>>)>,
}

impl<'ctx> Lowerer<'ctx> {
    /// A fresh session over a new module.
    #[must_use]
    pub fn new(ctx: &'ctx Context) -> Self {
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let i32t = ctx.i32_type();
        let code_ty = i32t.fn_type(&[ptr.into(), ptr.into(), i64t.into(), ptr.into()], false);
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
            inline_arith: true,
            reserved: HashMap::new(),
            direct: HashMap::new(),
        }
    }

    /// Lower a whole program to a verified module.
    pub fn lower_program(
        mut self,
        front: &FrontEnd,
        path: &str,
    ) -> Result<Module<'ctx>, Unsupported> {
        self.inline_arith = !front.units.iter().any(|u| mentions_rebinding(&u.expr));
        self.reserve_defns(front);
        let mut tops = Vec::with_capacity(front.units.len());
        for (i, unit) in front.units.iter().enumerate() {
            tops.push(self.lower_thunk(&format!("cljtop_{i}"), &unit.expr)?);
        }
        let init = self.lower_const_init();
        self.lower_main(path, init, &tops);
        crate::lower::verify_module(&self.module).map_err(Unsupported)?;
        Ok(self.module)
    }

    /// Reserve every top-level `(def name (fn …))` whose var is defined
    /// exactly once: calls through such a var may go direct.
    fn reserve_defns(&mut self, front: &FrontEnd) {
        let key = |sym: &Symbol| {
            (
                sym.namespace().unwrap_or("user").to_owned(),
                sym.name().to_owned(),
            )
        };
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for u in &front.units {
            if let Expr::Def { sym, .. } | Expr::DefMacro { sym, .. } = &u.expr {
                *counts.entry(key(sym)).or_default() += 1;
            }
            if let Expr::Declare { syms, .. } = &u.expr {
                for sym in syms {
                    *counts.entry(key(sym)).or_default() += 0;
                }
            }
        }
        for u in &front.units {
            let Expr::Def {
                sym,
                init: Some(init),
                ..
            } = &u.expr
            else {
                continue;
            };
            let Expr::Fn(fun) = init.as_ref() else {
                continue;
            };
            if counts.get(&key(sym)) != Some(&1) {
                continue;
            }
            let k = self.fns.len();
            self.fns.push(FnInfo {
                display: fun.display.clone(),
                arities: Vec::new(),
            });
            let mut funcs = Vec::new();
            let mut arities = Vec::new();
            for (i, arity) in fun.arities.iter().enumerate() {
                self.fn_seq += 1;
                let name = format!("cljfn_{}_{i}_{}", self.fn_seq, sanitize(&fun.display));
                let func = self
                    .module
                    .add_function(&name, self.code_ty, Some(Linkage::Private));
                funcs.push(func);
                arities.push((arity.fixed.len(), arity.rest.is_some(), func));
            }
            self.reserved
                .insert(std::ptr::from_ref(fun) as usize, (k, funcs));
            self.direct.insert(key(sym), (k, arities));
        }
    }

    // ---- helpers: declarations, constants, strings ---------------------

    fn helper(
        &self,
        name: &str,
        ret: Option<IntType<'ctx>>,
        params: &[BasicMetadataTypeEnum<'ctx>],
    ) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let ty = match ret {
            Some(r) => r.fn_type(params, false),
            None => self.ctx.void_type().fn_type(params, false),
        };
        self.module.add_function(name, ty, Some(Linkage::External))
    }

    fn call(
        &self,
        name: &str,
        ret: Option<IntType<'ctx>>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<IntValue<'ctx>> {
        let params: Vec<BasicMetadataTypeEnum> = args
            .iter()
            .map(|a| match a {
                BasicMetadataValueEnum::IntValue(v) => v.get_type().into(),
                BasicMetadataValueEnum::FloatValue(v) => v.get_type().into(),
                // Pointers (slots, headers, code); no other kind is passed.
                _ => self.ptr.into(),
            })
            .collect();
        let f = self.helper(name, ret, &params);
        let site = self.builder.build_call(f, args, "").expect("call");
        ret.map(|_| {
            site.try_as_basic_value()
                .expect_basic("status")
                .into_int_value()
        })
    }

    fn u64c(&self, n: u64) -> BasicMetadataValueEnum<'ctx> {
        self.i64t.const_int(n, false).into()
    }

    /// A private constant global holding `value`.
    fn const_global(&mut self, value: &dyn BasicValue<'ctx>, prefix: &str) -> PointerValue<'ctx> {
        self.fn_seq += 1;
        let v = value.as_basic_value_enum();
        let g = self
            .module
            .add_global(v.get_type(), None, &format!("{prefix}{}", self.fn_seq));
        g.set_initializer(&v);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.as_pointer_value()
    }

    /// The `{ptr, len}` header type of a `clj_rt::nabi::CSlice`.
    fn slice_ty(&self) -> inkwell::types::StructType<'ctx> {
        self.ctx
            .struct_type(&[self.ptr.into(), self.i64t.into()], false)
    }

    /// A constant `CSlice` header over `len` items at `base`.
    fn const_slice(&mut self, base: PointerValue<'ctx>, len: u64) -> PointerValue<'ctx> {
        let header = self
            .slice_ty()
            .const_named_struct(&[base.into(), self.i64t.const_int(len, false).into()]);
        self.const_global(&header, "cljh")
    }

    /// A `CSlice` header over `n` slots at `base` (a slot array of
    /// `f`), built once in the entry block.
    fn slots_ref(
        &mut self,
        f: &FnCtx<'ctx>,
        base: PointerValue<'ctx>,
        n: usize,
    ) -> PointerValue<'ctx> {
        if n == 0 {
            return self.const_slice(self.ptr.const_null(), 0);
        }
        let here = self.builder.get_insert_block().expect("positioned");
        self.builder.position_at_end(f.entry);
        let ty = self.slice_ty();
        let h = self.builder.build_alloca(ty, "sl").expect("alloca");
        let p0 = self.builder.build_struct_gep(ty, h, 0, "sp").expect("gep");
        self.builder.build_store(p0, base).expect("store");
        let p1 = self.builder.build_struct_gep(ty, h, 1, "sn").expect("gep");
        self.builder
            .build_store(p1, self.i64t.const_int(n as u64, false))
            .expect("store");
        self.builder.position_at_end(here);
        h
    }

    /// A constant `CSlice<u8>` header over `text` (cached).
    fn str_ptr(&mut self, text: &str) -> PointerValue<'ctx> {
        if let Some(p) = self.strings.get(text) {
            return *p;
        }
        let value = self.ctx.const_string(text.as_bytes(), false);
        let bytes = self.const_global(&value, "cljs");
        let p = self.const_slice(bytes, text.len() as u64);
        self.strings.insert(text.to_owned(), p);
        p
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
        let c = match scalar_const(v) {
            Some(c) => c,
            None => {
                let text = native_abi::print_constant(v);
                match native_abi::read_constant(&text) {
                    Ok(back) if back == *v => Const::Read(text),
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

    fn begin_fn(&mut self, name: &str, env: Vec<Option<LocalRef<'ctx>>>) -> FnCtx<'ctx> {
        let func = self
            .module
            .add_function(name, self.code_ty, Some(Linkage::Private));
        self.begin_fn_into(func, env)
    }

    fn begin_fn_into(
        &mut self,
        func: FunctionValue<'ctx>,
        env: Vec<Option<LocalRef<'ctx>>>,
    ) -> FnCtx<'ctx> {
        let entry = self.ctx.append_basic_block(func, "entry");
        let start = self.ctx.append_basic_block(func, "start");
        let fail = self.ctx.append_basic_block(func, "fail");
        self.builder.position_at_end(start);
        let captured = func
            .get_nth_param(3)
            .expect("captured")
            .into_pointer_value();
        FnCtx {
            func,
            entry,
            fail,
            slots: Vec::new(),
            env,
            recur: Vec::new(),
            captured,
            moves: Vec::new(),
        }
    }

    /// Finish a function whose result is in `result`: entry inits every
    /// slot to nil; the exit moves the result out and drops every slot.
    fn end_fn(&mut self, f: FnCtx<'ctx>, result: PointerValue<'ctx>) -> FunctionValue<'ctx> {
        let ret = f.func.get_nth_param(0).expect("ret").into_pointer_value();
        self.clone_v(f.func, ret, result);
        self.drop_slots(f.func, &f.slots);
        self.builder
            .build_return(Some(&self.i32t.const_zero()))
            .expect("ret");
        self.builder.position_at_end(f.fail);
        self.drop_slots(f.func, &f.slots);
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
        self.builder.build_unconditional_branch(first).expect("br");
        f.func
    }

    fn drop_slots(&self, func: FunctionValue<'ctx>, slots: &[PointerValue<'ctx>]) {
        for slot in slots {
            self.drop_v(func, *slot);
        }
    }

    // ---- inline value primitives (`Value` is `repr(u8)`: tag byte at 0,
    // `Bool` payload at 1, `Int`/`Double` payload at 8; POD tags are
    // Nil 0, Bool 1, Int 2, Double 3, Char 9) ---------------------------

    fn tag(&self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_load(self.i8t, slot, "tag")
            .expect("load")
            .into_int_value()
    }

    /// A pointer to byte `off` (1: the bool payload, 8: the numeric
    /// payload) of a slot, through the slot's field view
    /// `{i8 tag, i8, i16, i32, i64, i64, i64}` (offsets 0, 1, 2, 4, 8,
    /// 16, 24 — the `Value` layout, see `value_layout_abi`).
    fn byte_at(&self, slot: PointerValue<'ctx>, off: u64) -> PointerValue<'ctx> {
        let i16t = self.ctx.i16_type();
        let view = self.ctx.struct_type(
            &[
                self.i8t.into(),
                self.i8t.into(),
                i16t.into(),
                self.i32t.into(),
                self.i64t.into(),
                self.i64t.into(),
                self.i64t.into(),
            ],
            false,
        );
        let field = match off {
            1 => 1,
            4 => 3,
            _ => 4,
        };
        self.builder
            .build_struct_gep(view, slot, field, "b")
            .expect("gep")
    }

    /// The numeric payload of an `Int` (converted) or `Double` slot as
    /// an `f64` (callers checked the tag is 2 or 3).
    fn num_as_f64(&self, slot: PointerValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        let f64t = self.ctx.f64_type();
        let is_int = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                self.tag(slot),
                self.i8t.const_int(2, false),
                "isi",
            )
            .expect("cmp");
        let as_int = self
            .builder
            .build_signed_int_to_float(self.int_payload(slot), f64t, "itof")
            .expect("itof");
        let as_dbl = self
            .builder
            .build_load(f64t, self.byte_at(slot, 8), "d")
            .expect("load")
            .into_float_value();
        self.builder
            .build_select(is_int, as_int, as_dbl, "num")
            .expect("sel")
            .into_float_value()
    }

    fn write_double(
        &self,
        func: FunctionValue<'ctx>,
        dest: PointerValue<'ctx>,
        v: inkwell::values::FloatValue<'ctx>,
    ) {
        self.drop_v(func, dest);
        self.write_tag(dest, 3);
        self.builder
            .build_store(self.byte_at(dest, 8), v)
            .expect("store");
    }

    /// `FastArith` over numeric (int/double) args when not all ints:
    /// computes in `f64` (at least one arg is a double), else `slow`.
    fn lower_double_arith(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        elems: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        use inkwell::FloatPredicate;
        let mut numeric = self.ctx.bool_type().const_int(1, false);
        for e in elems {
            let t = self.tag(*e);
            let ok = self
                .builder
                .build_int_compare(
                    IntPredicate::ULE,
                    self.builder
                        .build_int_sub(t, self.i8t.const_int(2, false), "t2")
                        .expect("sub"),
                    self.i8t.const_int(1, false),
                    "num",
                )
                .expect("cmp");
            numeric = self.builder.build_and(numeric, ok, "nums").expect("and");
        }
        let go = self.ctx.append_basic_block(f.func, "dbl_fast");
        self.builder
            .build_conditional_branch(numeric, go, slow)
            .expect("br");
        self.builder.position_at_end(go);
        let x = self.num_as_f64(elems[0]);
        let one = self.ctx.f64_type().const_float(1.0);
        let y = if elems.len() == 2 {
            self.num_as_f64(elems[1])
        } else {
            one
        };
        let b = &self.builder;
        match op {
            FastArith::Lt | FastArith::Lte | FastArith::Gt | FastArith::Gte => {
                let pred = match op {
                    FastArith::Lt => FloatPredicate::OLT,
                    FastArith::Lte => FloatPredicate::OLE,
                    FastArith::Gt => FloatPredicate::OGT,
                    _ => FloatPredicate::OGE,
                };
                let r = b.build_float_compare(pred, x, y, "fc").expect("fcmp");
                self.write_bool(f.func, dest, r);
            }
            _ => {
                let r = match op {
                    FastArith::Add | FastArith::Inc => b.build_float_add(x, y, "fa"),
                    FastArith::Sub | FastArith::Dec => b.build_float_sub(x, y, "fs"),
                    _ => b.build_float_mul(x, y, "fm"),
                }
                .expect("fop");
                self.write_double(f.func, dest, r);
            }
        }
        self.builder.build_unconditional_branch(done).expect("br");
    }

    /// A `Char` slot's code point (`u32` at offset 4), as an `i32`.
    fn char_payload(&self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        let p = self.byte_at(slot, 4);
        self.builder
            .build_load(self.i32t, p, "c")
            .expect("load")
            .into_int_value()
    }

    fn int_payload(&self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        let p = self.byte_at(slot, 8);
        self.builder
            .build_load(self.i64t, p, "i")
            .expect("load")
            .into_int_value()
    }

    fn is_pod(&self, tag: IntValue<'ctx>) -> IntValue<'ctx> {
        let small = self
            .builder
            .build_int_compare(
                IntPredicate::ULE,
                tag,
                self.i8t.const_int(3, false),
                "small",
            )
            .expect("cmp");
        let chr = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, self.i8t.const_int(9, false), "chr")
            .expect("cmp");
        self.builder.build_or(small, chr, "pod").expect("or")
    }

    /// `if !pod(slot) { rt_n_drop(slot) }` (leaves a POD in place).
    fn drop_v(&self, func: FunctionValue<'ctx>, slot: PointerValue<'ctx>) {
        let pod = self.is_pod(self.tag(slot));
        let heap = self.ctx.append_basic_block(func, "drop_heap");
        let done = self.ctx.append_basic_block(func, "drop_done");
        self.builder
            .build_conditional_branch(pod, done, heap)
            .expect("br");
        self.builder.position_at_end(heap);
        self.call("rt_n_drop", None, &[slot.into()]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    /// `dest = clone(src)`: POD values copy inline, others call out.
    fn clone_v(
        &self,
        func: FunctionValue<'ctx>,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
    ) {
        let pod = self.is_pod(self.tag(src));
        let podb = self.ctx.append_basic_block(func, "clone_pod");
        let heap = self.ctx.append_basic_block(func, "clone_heap");
        let done = self.ctx.append_basic_block(func, "clone_done");
        self.builder
            .build_conditional_branch(pod, podb, heap)
            .expect("br");
        self.builder.position_at_end(podb);
        self.drop_v(func, dest);
        let v = self
            .builder
            .build_load(self.slot_ty, src, "v")
            .expect("load");
        self.builder.build_store(dest, v).expect("store");
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(heap);
        self.call("rt_n_clone", None, &[dest.into(), src.into()]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    fn write_tag(&self, dest: PointerValue<'ctx>, tag: u64) {
        self.builder
            .build_store(dest, self.i8t.const_int(tag, false))
            .expect("store");
    }

    /// Move `src`'s value into `dest` (dropping `dest`'s), leaving `src`
    /// nil without dropping it: ownership transfers.
    fn move_v(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>, src: PointerValue<'ctx>) {
        self.drop_v(func, dest);
        let v = self
            .builder
            .build_load(self.slot_ty, src, "mv")
            .expect("load");
        self.builder.build_store(dest, v).expect("store");
        self.write_tag(src, 0);
    }

    fn write_nil(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>) {
        self.drop_v(func, dest);
        self.write_tag(dest, 0);
    }

    fn write_int(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>, v: IntValue<'ctx>) {
        self.drop_v(func, dest);
        self.write_tag(dest, 2);
        self.builder
            .build_store(self.byte_at(dest, 8), v)
            .expect("store");
    }

    fn write_bool(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>, b: IntValue<'ctx>) {
        self.drop_v(func, dest);
        self.write_tag(dest, 1);
        let byte = self
            .builder
            .build_int_z_extend(b, self.i8t, "b")
            .expect("zext");
        self.builder
            .build_store(self.byte_at(dest, 1), byte)
            .expect("store");
    }

    /// A fresh nil slot of the current function.
    fn slot(&self, f: &mut FnCtx<'ctx>) -> PointerValue<'ctx> {
        let here = self.builder.get_insert_block().expect("positioned");
        self.builder.position_at_end(f.entry);
        let p = self
            .builder
            .build_alloca(self.slot_ty, "v")
            .expect("alloca");
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
    fn slot_array(
        &self,
        f: &mut FnCtx<'ctx>,
        n: usize,
    ) -> (PointerValue<'ctx>, Vec<PointerValue<'ctx>>) {
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

    /// Pointer to slot `i` of a slot array (viewed as a struct of
    /// `i + 1` slots: same layout, and the GEP stays in bounds by
    /// construction).
    fn elem(&self, base: PointerValue<'ctx>, i: usize) -> PointerValue<'ctx> {
        let fields: Vec<BasicTypeEnum<'ctx>> = vec![self.slot_ty.into(); i + 1];
        let view = self.ctx.struct_type(&fields, false);
        self.builder
            .build_struct_gep(view, base, i as u32, "e")
            .expect("gep")
    }

    /// Branch to the fail block when `status != 0`.
    fn check(&self, f: &FnCtx<'ctx>, status: IntValue<'ctx>) {
        let bad = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                status,
                status.get_type().const_zero(),
                "bad",
            )
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
    fn lower_fun(
        &mut self,
        fun: &FunExpr,
        base: usize,
        outer: &FnCtx<'ctx>,
    ) -> Result<(u64, Vec<usize>), Unsupported> {
        let mut free = BTreeSet::new();
        for arity in fun.arities.iter() {
            arity_free(arity, base, &mut free);
        }
        let captured: Vec<usize> = free.into_iter().collect();
        // Locals not captured stay `None` (never referenced here).
        let mut env = vec![None; base];
        for (pos, idx) in captured.iter().enumerate() {
            env[*idx] = Some(match outer.env.get(*idx) {
                Some(Some(LocalRef::Cell(_) | LocalRef::CapturedCell(_))) => {
                    LocalRef::CapturedCell(pos)
                }
                _ => LocalRef::Captured(pos),
            });
        }
        let reserved = self
            .reserved
            .get(&(std::ptr::from_ref(fun) as usize))
            .cloned();
        let k = match &reserved {
            Some((k, _)) => *k,
            None => {
                self.fns.push(FnInfo {
                    display: fun.display.clone(),
                    arities: Vec::new(),
                });
                self.fns.len() - 1
            }
        };
        let here = self.builder.get_insert_block();
        let mut arities = Vec::new();
        for (i, arity) in fun.arities.iter().enumerate() {
            let func = match &reserved {
                Some((_, funcs)) => funcs[i],
                None => {
                    self.fn_seq += 1;
                    let name = format!("cljfn_{}_{i}_{}", self.fn_seq, sanitize(&fun.display));
                    self.module
                        .add_function(&name, self.code_ty, Some(Linkage::Private))
                }
            };
            let code = self.lower_arity(func, arity, env.clone())?;
            arities.push((arity.fixed.len(), arity.rest.is_some(), code));
        }
        self.fns[k].arities = arities;
        if let Some(b) = here {
            self.builder.position_at_end(b);
        }
        Ok((k as u64, captured))
    }

    fn lower_arity(
        &mut self,
        func: FunctionValue<'ctx>,
        arity: &Arity,
        env: Vec<Option<LocalRef<'ctx>>>,
    ) -> Result<FunctionValue<'ctx>, Unsupported> {
        let mut f = self.begin_fn_into(func, env);
        let args = f.func.get_nth_param(1).expect("args").into_pointer_value();
        let pats: Vec<&Pattern> = arity.fixed.iter().chain(arity.rest.iter()).collect();
        // Params read the caller's argument array in place (the caller
        // owns it for the call; compiled code never drops or moves a
        // param). An arity that `recur`s rebinds its params, so it takes
        // owned copies first.
        let owned = self_recurs(&arity.body);
        let mut targets = Vec::with_capacity(pats.len());
        for (i, pat) in pats.iter().enumerate() {
            let arg = self.elem(args, i);
            let src = if owned {
                let s = self.slot(&mut f);
                self.clone_v(f.func, s, arg);
                s
            } else {
                arg
            };
            let slots = if matches!(pat, Pattern::Symbol(_)) {
                f.env.push(Some(LocalRef::Slot(src)));
                vec![src]
            } else {
                self.bind_pattern(&mut f, pat, src)?
            };
            targets.push(((*pat).clone(), slots));
        }
        let header = self.ctx.append_basic_block(f.func, "body");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget {
            header,
            patterns: targets,
            base: None,
        });
        let result = self.slot(&mut f);
        self.lower_body(&mut f, &arity.body, result)?;
        Ok(self.end_fn(f, result))
    }

    // ---- patterns ----------------------------------------------------------

    /// Bind `pat` to the value in `src`: allocates the pattern's local
    /// slots (in `pattern_names` order), pushes them on the frame, and
    /// returns them.
    fn bind_pattern(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pat: &Pattern,
        src: PointerValue<'ctx>,
    ) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
        match pat {
            Pattern::Symbol(_) => {
                let s = self.slot(f);
                self.clone_v(f.func, s, src);
                f.env.push(Some(LocalRef::Slot(s)));
                Ok(vec![s])
            }
            Pattern::Vec(v) => {
                let mut out = Vec::new();
                if let Some(rest) = &v.rest {
                    let cur = self.slot(f);
                    let st = self
                        .call("rt_n_gseq", Some(self.i32t), &[cur.into(), src.into()])
                        .expect("st");
                    self.check(f, st);
                    for fixed in &v.fixed {
                        let item = self.slot(f);
                        let st = self
                            .call("rt_n_first", Some(self.i32t), &[item.into(), cur.into()])
                            .expect("st");
                        self.check(f, st);
                        out.extend(self.bind_pattern(f, fixed, item)?);
                        let st = self
                            .call("rt_n_next_mut", Some(self.i32t), &[cur.into()])
                            .expect("st");
                        self.check(f, st);
                    }
                    out.extend(self.bind_pattern(f, rest, cur)?);
                } else {
                    for (i, fixed) in v.fixed.iter().enumerate() {
                        let item = self.slot(f);
                        let st = self
                            .call(
                                "rt_n_nth_nil",
                                Some(self.i32t),
                                &[item.into(), src.into(), self.u64c(i as u64)],
                            )
                            .expect("st");
                        self.check(f, st);
                        out.extend(self.bind_pattern(f, fixed, item)?);
                    }
                }
                if v.as_sym.is_some() {
                    let s = self.slot(f);
                    self.clone_v(f.func, s, src);
                    f.env.push(Some(LocalRef::Slot(s)));
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
                let oref = self.slots_ref(f, oarr, n);
                let dref = self.slots_ref(f, darr, dexprs.len());
                let st = self
                    .call(
                        "rt_n_destructure",
                        Some(self.i32t),
                        &[oref.into(), src.into(), self.u64c(k), dref.into()],
                    )
                    .expect("st");
                self.check(f, st);
                for s in &outs {
                    f.env.push(Some(LocalRef::Slot(*s)));
                }
                Ok(outs)
            }
        }
    }

    /// Rebind `pat`'s existing slots from `src` (for `recur`).
    fn rebind_pattern(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pat: &Pattern,
        src: PointerValue<'ctx>,
        slots: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        // A plain symbol takes the (dead after `recur`) temp by move.
        if let (Pattern::Symbol(_), [dst]) = (pat, slots) {
            self.move_v(f.func, *dst, src);
            return Ok(());
        }
        let depth = f.env.len();
        let fresh = self.bind_pattern(f, pat, src)?;
        f.env.truncate(depth);
        for (dst, s) in slots.iter().zip(fresh) {
            self.clone_v(f.func, *dst, s);
            self.drop_v(f.func, s);
        }
        Ok(())
    }

    // ---- expressions -------------------------------------------------------

    fn lower_body(
        &mut self,
        f: &mut FnCtx<'ctx>,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let Some((last, init)) = body.split_last() else {
            self.write_nil(f.func, dest);
            return Ok(());
        };
        for e in init {
            let t = self.slot(f);
            self.lower(f, e, t)?;
            self.drop_v(f.func, t);
        }
        self.lower(f, last, dest)
    }

    /// Lower each expression into its own slot of a fresh array.
    fn lower_args(
        &mut self,
        f: &mut FnCtx<'ctx>,
        args: &[Expr],
    ) -> Result<PointerValue<'ctx>, Unsupported> {
        let (base, elems) = self.slot_array(f, args.len());
        for (a, e) in args.iter().zip(elems) {
            self.lower(f, a, e)?;
        }
        Ok(base)
    }

    fn lower_local(
        &mut self,
        f: &FnCtx<'ctx>,
        idx: usize,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        // The analyzer resolves every local it emits (captures included).
        let local = f.env[idx].expect("analyzed local is visible");
        let p = self.local_slot(f, idx).expect("analyzed local is visible");
        if matches!(local, LocalRef::Cell(_) | LocalRef::CapturedCell(_)) {
            self.call("rt_n_cell_get", None, &[dest.into(), p.into()]);
        } else {
            self.clone_v(f.func, dest, p);
        }
        Ok(())
    }

    /// The raw slot holding local `idx` (a cell local yields the cell).
    fn local_slot(&self, f: &FnCtx<'ctx>, idx: usize) -> Option<PointerValue<'ctx>> {
        match f.env.get(idx).copied().flatten()? {
            LocalRef::Slot(p) | LocalRef::Cell(p) => Some(p),
            LocalRef::Captured(i) | LocalRef::CapturedCell(i) => Some(self.elem(f.captured, i)),
        }
    }

    fn lower(
        &mut self,
        f: &mut FnCtx<'ctx>,
        expr: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match expr {
            Expr::Literal(v) | Expr::Quote(v) => self.lower_const(f.func, v, dest),
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
            Expr::VecLit(items, meta) => {
                self.lower_coll(f, "rt_n_vec_lit", items, meta.as_ref(), dest, false)
            }
            Expr::SetLit(items, meta) => {
                self.lower_coll(f, "rt_n_set_lit", items, meta.as_ref(), dest, true)
            }
            Expr::MapLit(pairs, meta) => {
                let flat: Vec<Expr> = pairs
                    .iter()
                    .flat_map(|(k, v)| [k.clone(), v.clone()])
                    .collect();
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
                    if matches!(pat, Pattern::Symbol(_)) {
                        // The init's slot becomes the local's (no copy).
                        f.env.push(Some(LocalRef::Slot(t)));
                        bound.push(t);
                    } else {
                        bound.extend(self.bind_pattern(f, pat, t)?);
                        self.drop_v(f.func, t);
                    }
                }
                self.lower_body(f, body, dest)?;
                f.env.truncate(depth);
                // Release let-bound values at scope end (RSS parity).
                self.drop_slots(f.func, &bound);
                Ok(())
            }
            Expr::Loop { bindings, body } => self.lower_loop(f, bindings, body, dest),
            Expr::Recur(args) => self.lower_recur(f, args),
            Expr::Fn(fun) => {
                let base = f.env.len();
                self.make_closure(f, fun, base, dest)
            }
            Expr::LetFn { fns, body } => self.lower_letfn(f, fns, body, dest),
            Expr::Invoke { head, args, .. } if self.direct_target(head, args.len()).is_some() => {
                self.lower_direct_invoke(f, head, args, dest)
            }
            Expr::Invoke { head, args, .. } => {
                let h = self.slot(f);
                self.lower(f, head, h)?;
                let arr = self.lower_args(f, args)?;
                let aref = self.slots_ref(f, arr, args.len());
                let st = self
                    .call(
                        "rt_n_invoke",
                        Some(self.i32t),
                        &[dest.into(), h.into(), aref.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::DirectNative(call) => self.lower_direct(f, call, dest),
            Expr::Def {
                sym,
                doc,
                init,
                meta,
                span,
            } => self.lower_def(
                f,
                sym,
                doc.as_deref(),
                init.as_deref(),
                meta.as_deref(),
                *span,
                dest,
            ),
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
                self.call(
                    "rt_n_new_throwable",
                    None,
                    &[dest.into(), self.u64c(c), self.u64c(m)],
                );
                Ok(())
            }
            Expr::Try {
                body,
                catches,
                finally_body,
            } => self.lower_try(f, body, catches, finally_body, dest),
            Expr::Set { var, val } => {
                let k = self.var_of(var);
                let t = self.slot(f);
                self.lower(f, val, t)?;
                let st = self
                    .call(
                        "rt_n_set_var",
                        Some(self.i32t),
                        &[dest.into(), self.u64c(k), t.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Binding { pairs, body } => self.lower_binding(f, pairs, body, dest),
            Expr::Declare { syms, span } => {
                let v = Value::Vector(Vector::from_vec(
                    syms.iter().cloned().map(Value::Symbol).collect(),
                ));
                let k = self.value_const(&v)?;
                self.call(
                    "rt_n_declare",
                    None,
                    &[
                        dest.into(),
                        self.u64c(k),
                        self.i32t.const_int(u64::from(span.line), false).into(),
                        self.i32t.const_int(u64::from(span.col), false).into(),
                    ],
                );
                Ok(())
            }
            Expr::Ns {
                name,
                doc,
                attr,
                clauses,
            } => {
                let v = ns_desc(name, doc.as_deref(), attr.as_ref(), clauses);
                let k = self.value_const(&v)?;
                let st = self
                    .call("rt_n_ns", Some(self.i32t), &[dest.into(), self.u64c(k)])
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::CaseStar {
                expr,
                shift,
                mask,
                default,
                cases,
                skip,
                test,
            } => self.lower_case(f, expr, *shift, *mask, default, cases, skip, *test, dest),
            Expr::WithRedefs { pairs, body } => {
                let exprs: Vec<Expr> = pairs.iter().map(|(_, e)| e.clone()).collect();
                let vals = self.lower_args(f, &exprs)?;
                let ks: Vec<u64> = pairs.iter().map(|(v, _)| self.var_of(v)).collect();
                let karr = self.u64_array(&ks);
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let vref = self.slots_ref(f, vals, pairs.len());
                let st = self
                    .call(
                        "rt_n_with_redefs",
                        Some(self.i32t),
                        &[dest.into(), karr.into(), vref.into(), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::WithBindings { map, body } => {
                let m = self.slot(f);
                self.lower(f, map, m)?;
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let st = self
                    .call(
                        "rt_n_with_bindings",
                        Some(self.i32t),
                        &[dest.into(), m.into(), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Time { expr } => {
                let b = self.slot(f);
                self.body_closure(f, 0, std::slice::from_ref(expr.as_ref()), b)?;
                let st = self
                    .call("rt_n_time", Some(self.i32t), &[dest.into(), b.into()])
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::WithOutStr { out, body } => {
                let k = self.value_const(&Value::Symbol(out.clone()))?;
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let st = self
                    .call(
                        "rt_n_with_out_str",
                        Some(self.i32t),
                        &[dest.into(), self.u64c(k), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::WithInStr { input, s, body } => {
                let k = self.value_const(&Value::Symbol(input.clone()))?;
                let sv = self.slot(f);
                self.lower(f, s, sv)?;
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let st = self
                    .call(
                        "rt_n_with_in_str",
                        Some(self.i32t),
                        &[dest.into(), self.u64c(k), sv.into(), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Locking { lockee, body } => {
                let t = self.slot(f);
                self.lower(f, lockee, t)?;
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let st = self
                    .call(
                        "rt_n_locking",
                        Some(self.i32t),
                        &[dest.into(), t.into(), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Sync { body } | Expr::Io { body } => {
                let io = u64::from(matches!(expr, Expr::Io { .. }));
                let b = self.slot(f);
                self.body_closure(f, 0, body, b)?;
                let st = self
                    .call(
                        "rt_n_sync",
                        Some(self.i32t),
                        &[dest.into(), self.i8t.const_int(io, false).into(), b.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::LazySeqStar { body } => self.wrap_body(f, 0, "clojure.core/fn", body, dest),
            Expr::DelayStar { display, body } => self.wrap_body(f, 1, display, body, dest),
            Expr::BoundFn { name, fun } => {
                let depth = f.env.len();
                let cell = name.as_ref().map(|_| {
                    let c = self.slot(f);
                    self.call("rt_n_cell_new", None, &[c.into()]);
                    f.env.push(Some(LocalRef::Cell(c)));
                    c
                });
                let t = self.slot(f);
                let base = f.env.len();
                self.make_closure(f, fun, base, t)?;
                self.call(
                    "rt_n_wrap_fn",
                    None,
                    &[dest.into(), self.i8t.const_int(2, false).into(), t.into()],
                );
                if let Some(c) = cell {
                    self.call("rt_n_cell_set", None, &[c.into(), dest.into()]);
                }
                f.env.truncate(depth);
                Ok(())
            }
            Expr::DefMacro {
                sym,
                doc,
                attr,
                arglists,
                fun,
                span,
            } => {
                let desc = Value::Vector(Vector::from_vec(vec![
                    Value::Symbol(sym.clone()),
                    doc.as_deref()
                        .map_or(Value::Nil, |d| Value::Str(std::sync::Arc::from(d))),
                    attr.clone().map_or(Value::Nil, Value::Map),
                    Value::Vector(Vector::from_vec(arglists.clone())),
                    Value::Int(i64::from(span.line)),
                    Value::Int(i64::from(span.col)),
                ]));
                let k = self.value_const(&desc)?;
                let t = self.slot(f);
                let base = f.env.len();
                self.make_closure(f, fun, base, t)?;
                let st = self
                    .call(
                        "rt_n_defmacro",
                        Some(self.i32t),
                        &[dest.into(), self.u64c(k), t.into()],
                    )
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::Assert {
                test,
                message,
                form,
            } => {
                let t = self.slot(f);
                self.lower(f, test, t)?;
                let ok = self.truthy(t);
                let fail_b = self.ctx.append_basic_block(f.func, "assert_fail");
                let pass_b = self.ctx.append_basic_block(f.func, "assert_pass");
                self.builder
                    .build_conditional_branch(ok, pass_b, fail_b)
                    .expect("br");
                self.builder.position_at_end(fail_b);
                let msg = match message {
                    Some(m) => {
                        let ms = self.slot(f);
                        self.lower(f, m, ms)?;
                        ms
                    }
                    None => self.ptr.const_null(),
                };
                let fk = self.constant(Const::Str(form.clone()));
                self.call(
                    "rt_n_assert_fail",
                    Some(self.i32t),
                    &[msg.into(), self.u64c(fk)],
                );
                self.builder.build_unconditional_branch(f.fail).expect("br");
                self.builder.position_at_end(pass_b);
                self.write_nil(f.func, dest);
                Ok(())
            }
        }
    }

    /// A body compiled as a zero-arg fn named `display`, wrapped by
    /// the runtime (`kind`: 0 `lazy-seq`, 1 `delay`).
    fn wrap_body(
        &mut self,
        f: &mut FnCtx<'ctx>,
        kind: u64,
        display: &str,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let fun = FunExpr {
            display: display.to_owned(),
            arities: std::sync::Arc::from([Arity {
                fixed: Vec::new(),
                rest: None,
                body: body.to_vec(),
            }]),
        };
        let t = self.slot(f);
        let base = f.env.len();
        self.make_closure(f, &fun, base, t)?;
        self.call(
            "rt_n_wrap_fn",
            None,
            &[
                dest.into(),
                self.i8t.const_int(kind, false).into(),
                t.into(),
            ],
        );
        Ok(())
    }

    /// The direct-call target of `(head args…)` with `n` args: var
    /// index, fn index, and the fixed arity's code.
    fn direct_target(
        &self,
        head: &Expr,
        n: usize,
    ) -> Option<(clj_value::Var, usize, FunctionValue<'ctx>)> {
        let Expr::Global(var) = head else { return None };
        let key = (
            var.sym.namespace().unwrap_or("user").to_owned(),
            var.sym.name().to_owned(),
        );
        let (k, arities) = self.direct.get(&key)?;
        let code = arities
            .iter()
            .find(|(fixed, variadic, _)| !variadic && *fixed == n)?
            .2;
        Some((var.clone(), *k, code))
    }

    /// A call of a compiled top-level `defn`: guarded direct call of
    /// its arity code (the var still holds that fn; depth/stack under
    /// the interpreter's limits), else the general invoke.
    fn lower_direct_invoke(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let (var, k, code) = self.direct_target(head, args.len()).expect("checked");
        let vk = self.var_of(&var);
        // Plain locals ride in the argument array as bitwise aliases (the
        // callee only reads its args); they are cleared, never dropped,
        // right after the call. Computed args are owned there.
        let (arr, elems) = self.slot_array(f, args.len());
        let mut aliases = Vec::new();
        for (a, e) in args.iter().zip(&elems) {
            let local = match a {
                Expr::Local(i) => self.borrowable(f, *i),
                _ => None,
            };
            if let Some(p) = local {
                self.copy_bits(*e, p);
                aliases.push(*e);
            } else {
                self.lower(f, a, *e)?;
            }
        }
        let ok = self
            .call(
                "rt_n_enter_fn",
                Some(self.i8t),
                &[self.u64c(vk), self.u64c(k as u64)],
            )
            .expect("ok");
        let go = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "go")
            .expect("cmp");
        let direct_b = self.ctx.append_basic_block(f.func, "call_direct");
        let slow_b = self.ctx.append_basic_block(f.func, "call_slow");
        let done = self.ctx.append_basic_block(f.func, "call_done");
        self.builder
            .build_conditional_branch(go, direct_b, slow_b)
            .expect("br");
        self.builder.position_at_end(direct_b);
        let st = self
            .builder
            .build_call(
                code,
                &[
                    dest.into(),
                    arr.into(),
                    self.u64c(args.len() as u64),
                    self.ptr.const_null().into(),
                ],
                "st",
            )
            .expect("call")
            .try_as_basic_value()
            .expect_basic("st")
            .into_int_value();
        self.call("rt_n_leave", None, &[]);
        self.clear_aliases(&aliases);
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(slow_b);
        let h = self.slot(f);
        self.call("rt_n_deref", None, &[h.into(), self.u64c(vk)]);
        let aref = self.slots_ref(f, arr, args.len());
        let st = self
            .call(
                "rt_n_invoke",
                Some(self.i32t),
                &[dest.into(), h.into(), aref.into()],
            )
            .expect("st");
        self.clear_aliases(&aliases);
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        Ok(())
    }

    /// `dest = src`'s bits without a refcount change (an alias the
    /// caller clears with [`clear_aliases`](Self::clear_aliases)).
    fn copy_bits(&self, dest: PointerValue<'ctx>, src: PointerValue<'ctx>) {
        let v = self
            .builder
            .build_load(self.slot_ty, src, "alias")
            .expect("load");
        self.builder.build_store(dest, v).expect("store");
    }

    /// Reset alias slots to nil without dropping (they never owned).
    fn clear_aliases(&self, aliases: &[PointerValue<'ctx>]) {
        for a in aliases {
            self.write_tag(*a, 0);
        }
    }

    /// A direct-native call: inline `FastArith` int ops (falling back
    /// to the native on non-int args or overflow), else a direct call
    /// of the var's native.
    fn lower_direct(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if !self.inline_arith {
            return self.lower_direct_owned(f, call, dest);
        }
        let k = self.var_of(&call.var);
        let n = call.args.len();
        let fast = fast_op(call.fast, n);
        let core_name = (call.var.sym.namespace() == Some("clojure.core"))
            .then(|| call.var.sym.name().to_owned());
        let moving = fast.is_none()
            && call
                .args
                .iter()
                .any(|a| matches!(a, Expr::Local(i) if f.moves.contains(i)));
        if moving {
            return self.lower_direct_moving(f, call, k, core_name.as_deref(), dest);
        }
        let (refs, temps) = self.arg_refs(f, &call.args, dest)?;
        let slow = self.ctx.append_basic_block(f.func, "native_slow");
        let done = self.ctx.append_basic_block(f.func, "native_done");
        let inline = match (fast, core_name.as_deref()) {
            (None, Some(name)) => self.lower_intrinsic(f, name, &refs, dest, slow, done),
            _ => false,
        };
        match (inline, fast) {
            (false, Some(op)) => self.lower_fast_arith(f, op, &refs, dest, slow, done),
            (false, None) => {
                self.builder.build_unconditional_branch(slow).expect("br");
            }
            (true, _) => {}
        }
        // Slow path: the VM's inline op over the borrowed args, else
        // the native (args cloned only there).
        self.builder.position_at_end(slow);
        let op = core_name
            .as_deref()
            .and_then(|name| native_abi::inline_op(name, n))
            .unwrap_or(u32::MAX);
        let aref = self.refs_slice(f, &refs);
        let st = self
            .call(
                "rt_n_core_ref",
                Some(self.i32t),
                &[
                    dest.into(),
                    self.i32t.const_int(u64::from(op), false).into(),
                    self.u64c(k),
                    aref.into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.drop_slots(f.func, &temps);
        Ok(())
    }

    /// A core call taking a local at its last use (inside `recur` args)
    /// by move: the args are owned (the moved local's slot is left nil),
    /// so a native with an owned-args entry gets a uniquely held
    /// collection and updates it in place, like the VM's last-use moves.
    fn lower_direct_moving(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        k: u64,
        core_name: Option<&str>,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let n = call.args.len();
        let (arr, elems) = self.slot_array(f, n);
        for (a, e) in call.args.iter().zip(&elems) {
            let moved = match a {
                Expr::Local(i) if f.moves.contains(i) => self.borrowable(f, *i),
                _ => None,
            };
            if let Some(src) = moved {
                self.move_v(f.func, *e, src);
            } else {
                self.lower(f, a, *e)?;
            }
        }
        let op = core_name
            .and_then(|name| native_abi::inline_op(name, n))
            .unwrap_or(u32::MAX);
        let aref = self.slots_ref(f, arr, n);
        let st = self
            .call(
                "rt_n_core_own",
                Some(self.i32t),
                &[
                    dest.into(),
                    self.i32t.const_int(u64::from(op), false).into(),
                    self.u64c(k),
                    aref.into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        self.drop_slots(f.func, &elems);
        Ok(())
    }

    /// [`lower_direct`](Self::lower_direct) for a program that may rebind
    /// core vars: owned args through the var's current root.
    fn lower_direct_owned(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let k = self.var_of(&call.var);
        let n = call.args.len();
        let arr = self.lower_args(f, &call.args)?;
        let aref = self.slots_ref(f, arr, n);
        let st = self
            .call(
                "rt_n_native_mv",
                Some(self.i32t),
                &[dest.into(), self.u64c(k), aref.into()],
            )
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    /// Each argument as a slot the call may borrow: a plain local's own
    /// slot (no clone; never `dest`, which the call writes), else a
    /// fresh slot holding the lowered argument. Returns the pointers and
    /// the fresh slots (`temps`, dropped after the call). Locals are
    /// immutable, so borrowing one reads the same value its evaluation
    /// would have copied.
    fn arg_refs(
        &mut self,
        f: &mut FnCtx<'ctx>,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(Vec<PointerValue<'ctx>>, Vec<PointerValue<'ctx>>), Unsupported> {
        let mut refs = Vec::with_capacity(args.len());
        let mut temps = Vec::new();
        for a in args {
            let local = match a {
                Expr::Local(idx) => self.borrowable(f, *idx),
                _ => None,
            };
            if let Some(p) = local.filter(|p| *p != dest) {
                refs.push(p);
            } else {
                let t = self.slot(f);
                self.lower(f, a, t)?;
                refs.push(t);
                temps.push(t);
            }
        }
        Ok((refs, temps))
    }

    /// The slot holding local `idx`'s value, when it can be borrowed
    /// (`letfn` cells hold a cell, not the value).
    fn borrowable(&self, f: &FnCtx<'ctx>, idx: usize) -> Option<PointerValue<'ctx>> {
        match f.env.get(idx).copied().flatten()? {
            LocalRef::Slot(p) => Some(p),
            LocalRef::Captured(i) => Some(self.elem(f.captured, i)),
            LocalRef::Cell(_) | LocalRef::CapturedCell(_) => None,
        }
    }

    /// A `CSlice<&Value>` header over `refs` (the pointer array lives in
    /// the entry block; the pointers are stored here, at the call).
    fn refs_slice(&mut self, f: &FnCtx<'ctx>, refs: &[PointerValue<'ctx>]) -> PointerValue<'ctx> {
        let fields: Vec<BasicTypeEnum<'ctx>> = vec![self.ptr.into(); refs.len()];
        let ty = self.ctx.struct_type(&fields, false);
        let here = self.builder.get_insert_block().expect("positioned");
        self.builder.position_at_end(f.entry);
        let base = self.builder.build_alloca(ty, "refs").expect("alloca");
        self.builder.position_at_end(here);
        for (i, r) in refs.iter().enumerate() {
            let p = self
                .builder
                .build_struct_gep(ty, base, i as u32, "ref")
                .expect("gep");
            self.builder.build_store(p, *r).expect("store");
        }
        self.slots_ref(f, base, refs.len())
    }

    /// The analyzer's `FastArith` op inline: all-int args compute in
    /// `i64` (overflow takes `slow`), numeric mixes in `f64`, anything
    /// else takes `slow`.
    fn lower_fast_arith(
        &mut self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        elems: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        let n = elems.len();
        let two = self.i8t.const_int(2, false);
        let mut ints = self
            .builder
            .build_int_compare(IntPredicate::EQ, self.tag(elems[0]), two, "a_int")
            .expect("cmp");
        if n == 2 {
            let b_int = self
                .builder
                .build_int_compare(IntPredicate::EQ, self.tag(elems[1]), two, "b_int")
                .expect("cmp");
            ints = self.builder.build_and(ints, b_int, "ints").expect("and");
        }
        let fastb = self.ctx.append_basic_block(f.func, "native_fast");
        let dbl_check = self.ctx.append_basic_block(f.func, "native_dbl_check");
        self.builder
            .build_conditional_branch(ints, fastb, dbl_check)
            .expect("br");
        // Doubles (and int/double mixes): f64 math like the native.
        self.builder.position_at_end(dbl_check);
        self.lower_double_arith(f, op, elems, dest, slow, done);
        self.builder.position_at_end(fastb);
        let a = self.int_payload(elems[0]);
        let b = if n == 2 {
            self.int_payload(elems[1])
        } else {
            self.i64t.const_int(1, false)
        };
        let cmp = |pred: IntPredicate, me: &Self| {
            me.builder
                .build_int_compare(pred, a, b, "cmp")
                .expect("cmp")
        };
        match op {
            FastArith::Lt | FastArith::Lte | FastArith::Gt | FastArith::Gte => {
                let pred = match op {
                    FastArith::Lt => IntPredicate::SLT,
                    FastArith::Lte => IntPredicate::SLE,
                    FastArith::Gt => IntPredicate::SGT,
                    _ => IntPredicate::SGE,
                };
                let r = cmp(pred, self);
                self.write_bool(f.func, dest, r);
                self.builder.build_unconditional_branch(done).expect("br");
            }
            _ => {
                let name = match op {
                    FastArith::Add | FastArith::Inc => "llvm.sadd.with.overflow",
                    FastArith::Sub | FastArith::Dec => "llvm.ssub.with.overflow",
                    _ => "llvm.smul.with.overflow",
                };
                let intr = inkwell::intrinsics::Intrinsic::find(name).expect("intrinsic");
                let decl = intr
                    .get_declaration(&self.module, &[self.i64t.into()])
                    .expect("decl");
                let pair = self
                    .builder
                    .build_call(decl, &[a.into(), b.into()], "ov")
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
                let okb = self.ctx.append_basic_block(f.func, "native_ok");
                self.builder
                    .build_conditional_branch(o, slow, okb)
                    .expect("br");
                self.builder.position_at_end(okb);
                self.write_int(f.func, dest, r);
                self.builder.build_unconditional_branch(done).expect("br");
            }
        }
    }

    /// The slow block (the var's native, args moved in) and the join.
    #[allow(clippy::too_many_arguments)]
    /// `nil?`/`some?`/`not` of any value (no fallback needed).
    fn any_pred(&self, name: &str, v: PointerValue<'ctx>) -> IntValue<'ctx> {
        if name == "not" {
            let t = self.truthy(v);
            return self.builder.build_not(t, "not").expect("not");
        }
        let pred = if name == "nil?" {
            IntPredicate::EQ
        } else {
            IntPredicate::NE
        };
        self.builder
            .build_int_compare(pred, self.tag(v), self.i8t.const_zero(), "nil")
            .expect("cmp")
    }

    /// `zero?`/`pos?`/`neg?`/`even?`/`odd?` of an `Int` payload.
    fn int_pred(&self, name: &str, x: IntValue<'ctx>) -> IntValue<'ctx> {
        let zero = self.i64t.const_zero();
        let bb = &self.builder;
        match name {
            "zero?" => bb.build_int_compare(IntPredicate::EQ, x, zero, "p"),
            "pos?" => bb.build_int_compare(IntPredicate::SGT, x, zero, "p"),
            "neg?" => bb.build_int_compare(IntPredicate::SLT, x, zero, "p"),
            _ => {
                let low = bb
                    .build_and(x, self.i64t.const_int(1, false), "low")
                    .expect("and");
                let pred = if name == "even?" {
                    IntPredicate::EQ
                } else {
                    IntPredicate::NE
                };
                bb.build_int_compare(pred, low, zero, "p")
            }
        }
        .expect("cmp")
    }

    /// An int-only binary core op over checked `Int` payloads
    /// (division by zero and `MIN/-1` branch to `slow`).
    fn int_binop(
        &self,
        f: &FnCtx<'ctx>,
        name: &str,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        slow: BasicBlock<'ctx>,
    ) -> IntValue<'ctx> {
        match name {
            "quot" | "rem" | "mod" => self.int_div(f, name, x, y, slow),
            "max" | "min" => {
                let pred = if name == "max" {
                    IntPredicate::SGE
                } else {
                    IntPredicate::SLE
                };
                let pick = self
                    .builder
                    .build_int_compare(pred, x, y, "pick")
                    .expect("cmp");
                self.builder
                    .build_select(pick, x, y, "mm")
                    .expect("sel")
                    .into_int_value()
            }
            _ => {
                let bb = &self.builder;
                let sh = bb
                    .build_and(y, self.i64t.const_int(63, false), "sh")
                    .expect("and");
                match name {
                    "bit-and" => bb.build_and(x, y, "r"),
                    "bit-or" => bb.build_or(x, y, "r"),
                    "bit-xor" => bb.build_xor(x, y, "r"),
                    "bit-shift-left" => bb.build_left_shift(x, sh, "r"),
                    "bit-shift-right" => bb.build_right_shift(x, sh, true, "r"),
                    _ => bb.build_right_shift(x, sh, false, "r"),
                }
                .expect("bit")
            }
        }
    }

    /// `quot`/`rem`/`mod` on ints; the native handles zero and overflow.
    fn int_div(
        &self,
        f: &FnCtx<'ctx>,
        name: &str,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        slow: BasicBlock<'ctx>,
    ) -> IntValue<'ctx> {
        let bb = &self.builder;
        let zero = self.i64t.const_zero();
        let y0 = bb
            .build_int_compare(IntPredicate::EQ, y, zero, "y0")
            .expect("cmp");
        let ym1 = bb
            .build_int_compare(IntPredicate::EQ, y, self.i64t.const_all_ones(), "ym1")
            .expect("cmp");
        let xmin = bb
            .build_int_compare(
                IntPredicate::EQ,
                x,
                self.i64t.const_int(i64::MIN as u64, true),
                "xmin",
            )
            .expect("cmp");
        let ovf = bb.build_and(ym1, xmin, "ovf").expect("and");
        let bad = bb.build_or(y0, ovf, "bad").expect("or");
        let ok = self.ctx.append_basic_block(f.func, "div_ok");
        bb.build_conditional_branch(bad, slow, ok).expect("br");
        self.builder.position_at_end(ok);
        let bb = &self.builder;
        match name {
            "quot" => bb.build_int_signed_div(x, y, "q").expect("div"),
            "rem" => bb.build_int_signed_rem(x, y, "r").expect("rem"),
            _ => {
                // Clojure `mod`: the remainder takes the divisor's sign.
                let m = bb.build_int_signed_rem(x, y, "m").expect("rem");
                let xor = bb.build_xor(m, y, "x").expect("xor");
                let diff = bb
                    .build_int_compare(IntPredicate::SLT, xor, zero, "diff")
                    .expect("cmp");
                let nz = bb
                    .build_int_compare(IntPredicate::NE, m, zero, "nz")
                    .expect("cmp");
                let fix = bb.build_and(diff, nz, "fix").expect("and");
                let adj = bb.build_int_add(m, y, "adj").expect("add");
                bb.build_select(fix, adj, m, "mod")
                    .expect("sel")
                    .into_int_value()
            }
        }
    }

    /// Branch to `fast` when every slot holds an `Int`, else `slow`.
    fn when_ints(&self, f: &FnCtx<'ctx>, slots: &[PointerValue<'ctx>], slow: BasicBlock<'ctx>) {
        self.when_tag(f, slots, TAG_INT, slow);
    }

    /// Continue in a fresh block when every slot has tag `tag`, else
    /// branch to `slow`.
    fn when_tag(
        &self,
        f: &FnCtx<'ctx>,
        slots: &[PointerValue<'ctx>],
        tag: u64,
        slow: BasicBlock<'ctx>,
    ) {
        let want = self.i8t.const_int(tag, false);
        let mut all = self.ctx.bool_type().const_int(1, false);
        for s in slots {
            let is = self
                .builder
                .build_int_compare(IntPredicate::EQ, self.tag(*s), want, "is_tag")
                .expect("cmp");
            all = self.builder.build_and(all, is, "ints").expect("and");
        }
        let fast = self.ctx.append_basic_block(f.func, "intr_fast");
        self.builder
            .build_conditional_branch(all, fast, slow)
            .expect("br");
        self.builder.position_at_end(fast);
    }

    /// Inline a core fn by name when it has an intrinsic: returns
    /// `false` (nothing emitted) when it does not. Int-only ops guard
    /// on `Int` args and branch to `slow` otherwise; every emitted
    /// fast path ends by branching to `done`, and the caller positions
    /// at `slow` next.
    fn lower_intrinsic(
        &mut self,
        f: &FnCtx<'ctx>,
        name: &str,
        a: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) -> bool {
        let n = a.len();
        match (name, n) {
            ("nil?" | "some?" | "not", 1) => {
                let r = self.any_pred(name, a[0]);
                self.write_bool(f.func, dest, r);
            }
            ("=" | "not=", 2) => {
                // Ints inline; anything else takes `slow` (the VM's
                // inline op, then the native).
                let other = self.ctx.append_basic_block(f.func, "eq_other");
                self.when_ints(f, a, other);
                let (x, y) = (self.int_payload(a[0]), self.int_payload(a[1]));
                let pred = if name == "not=" {
                    IntPredicate::NE
                } else {
                    IntPredicate::EQ
                };
                let r = self
                    .builder
                    .build_int_compare(pred, x, y, "eq")
                    .expect("cmp");
                self.write_bool(f.func, dest, r);
                self.builder.build_unconditional_branch(done).expect("br");
                // Chars compare by code point; anything else takes `slow`.
                self.builder.position_at_end(other);
                self.when_tag(f, a, TAG_CHAR, slow);
                let (x, y) = (self.char_payload(a[0]), self.char_payload(a[1]));
                let r = self
                    .builder
                    .build_int_compare(pred, x, y, "ceq")
                    .expect("cmp");
                self.write_bool(f.func, dest, r);
            }
            ("int" | "long", 1) => {
                // A char's code point (always in range); anything else
                // takes `slow`.
                self.when_tag(f, a, TAG_CHAR, slow);
                let c = self.char_payload(a[0]);
                let n = self
                    .builder
                    .build_int_z_extend(c, self.i64t, "cp")
                    .expect("zext");
                self.write_int(f.func, dest, n);
            }
            ("==", 2) => {
                self.when_ints(f, a, slow);
                let (x, y) = (self.int_payload(a[0]), self.int_payload(a[1]));
                let r = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, x, y, "eq")
                    .expect("cmp");
                self.write_bool(f.func, dest, r);
            }
            ("zero?" | "pos?" | "neg?" | "even?" | "odd?", 1) => {
                self.when_ints(f, a, slow);
                let r = self.int_pred(name, self.int_payload(a[0]));
                self.write_bool(f.func, dest, r);
            }
            ("quot" | "rem" | "mod" | "max" | "min", 2)
            | (
                "bit-and"
                | "bit-or"
                | "bit-xor"
                | "bit-shift-left"
                | "bit-shift-right"
                | "unsigned-bit-shift-right",
                2,
            ) => {
                self.when_ints(f, a, slow);
                let (x, y) = (self.int_payload(a[0]), self.int_payload(a[1]));
                let r = self.int_binop(f, name, x, y, slow);
                self.write_int(f.func, dest, r);
            }
            _ => return false,
        }
        self.builder.build_unconditional_branch(done).expect("br");
        true
    }

    fn lower_const(
        &mut self,
        func: FunctionValue<'ctx>,
        v: &Value,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match v {
            Value::Nil => self.write_nil(func, dest),
            Value::Int(n) => self.write_int(func, dest, self.i64t.const_int(*n as u64, true)),
            Value::Bool(b) => {
                let bit = self.ctx.bool_type().const_int(u64::from(*b), false);
                self.write_bool(func, dest, bit);
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

    fn lower_coll(
        &mut self,
        f: &mut FnCtx<'ctx>,
        helper: &str,
        items: &[Expr],
        meta: Option<&Map>,
        dest: PointerValue<'ctx>,
        fallible: bool,
    ) -> Result<(), Unsupported> {
        let mk = self.meta_const(meta)?;
        let arr = self.lower_args(f, items)?;
        let aref = self.slots_ref(f, arr, items.len());
        let args = [dest.into(), aref.into(), self.u64c(mk)];
        if fallible {
            let st = self.call(helper, Some(self.i32t), &args).expect("st");
            self.check(f, st);
        } else {
            self.call(helper, None, &args);
        }
        Ok(())
    }

    fn truthy(&self, v: PointerValue<'ctx>) -> IntValue<'ctx> {
        let t = self.tag(v);
        let non_nil = self
            .builder
            .build_int_compare(IntPredicate::NE, t, self.i8t.const_zero(), "nn")
            .expect("cmp");
        let not_bool = self
            .builder
            .build_int_compare(IntPredicate::NE, t, self.i8t.const_int(1, false), "nb")
            .expect("cmp");
        let b = self
            .builder
            .build_load(self.i8t, self.byte_at(v, 1), "b")
            .expect("load")
            .into_int_value();
        let true_bool = self
            .builder
            .build_int_compare(IntPredicate::NE, b, self.i8t.const_zero(), "tb")
            .expect("cmp");
        let ok = self
            .builder
            .build_or(not_bool, true_bool, "ok")
            .expect("or");
        self.builder.build_and(non_nil, ok, "truthy").expect("and")
    }

    fn lower_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let t = self.slot(f);
        self.lower(f, test, t)?;
        let cond = self.truthy(t);
        let then_b = self.ctx.append_basic_block(f.func, "then");
        let else_b = self.ctx.append_basic_block(f.func, "else");
        let merge = self.ctx.append_basic_block(f.func, "merge");
        self.builder
            .build_conditional_branch(cond, then_b, else_b)
            .expect("br");
        self.builder.position_at_end(then_b);
        self.lower(f, then, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(else_b);
        self.lower(f, els, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(merge);
        Ok(())
    }

    fn lower_loop(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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
        f.recur.push(RecurTarget {
            header,
            patterns: targets,
            base: Some(depth),
        });
        let result = self.lower_body(f, body, dest);
        f.recur.pop();
        f.env.truncate(depth);
        result
    }

    fn lower_recur(&mut self, f: &mut FnCtx<'ctx>, args: &[Expr]) -> Result<(), Unsupported> {
        let base = f.recur.last().and_then(|t| t.base);
        let movable = movable_locals(f, args, base);
        let saved = std::mem::replace(&mut f.moves, movable);
        let fresh: Result<Vec<PointerValue>, Unsupported> = args
            .iter()
            .map(|a| {
                let t = self.slot(f);
                self.lower(f, a, t).map(|()| t)
            })
            .collect();
        f.moves = saved;
        let fresh = fresh?;
        // The analyzer only emits `recur` in tail position of a target.
        let target = f.recur.pop().expect("recur has a target");
        let depth = f.env.len();
        // Rebinding pushes env entries per pattern position: rebuild
        // the env so later patterns see earlier ones like a fresh bind.
        for ((pat, slots), src) in target.patterns.iter().zip(&fresh) {
            self.rebind_pattern(f, pat, *src, slots)?;
        }
        f.env.truncate(depth);
        for t in &fresh {
            self.drop_v(f.func, *t);
        }
        self.builder
            .build_unconditional_branch(target.header)
            .expect("br");
        f.recur.push(target);
        self.dead_end(f);
        Ok(())
    }

    /// Build the captured array for a fn created at `base` and make the
    /// closure value in `dest`.
    fn make_closure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        fun: &FunExpr,
        base: usize,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let (k, captured) = self.lower_fun(fun, base, f)?;
        let (arr, elems) = self.slot_array(f, captured.len());
        for (idx, e) in captured.iter().zip(&elems) {
            let src = self
                .local_slot(f, *idx)
                .ok_or_else(|| Unsupported(format!("captured local {idx} unresolved")))?;
            self.clone_v(f.func, *e, src);
        }
        let cref = self.slots_ref(f, arr, captured.len());
        self.call(
            "rt_n_make_fn",
            None,
            &[dest.into(), self.u64c(k), cref.into()],
        );
        // The closure owns copies; release ours now.
        self.drop_slots(f.func, &elems);
        Ok(())
    }

    fn lower_letfn(
        &mut self,
        f: &mut FnCtx<'ctx>,
        fns: &[(String, FunExpr)],
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let mut cells = Vec::with_capacity(fns.len());
        for _ in fns {
            let c = self.slot(f);
            self.call("rt_n_cell_new", None, &[c.into()]);
            f.env.push(Some(LocalRef::Cell(c)));
            cells.push(c);
        }
        let base = f.env.len();
        for ((_, fun), cell) in fns.iter().zip(&cells) {
            let t = self.slot(f);
            self.make_closure(f, fun, base, t)?;
            self.call("rt_n_cell_set", None, &[(*cell).into(), t.into()]);
            self.drop_v(f.func, t);
        }
        let r = self.lower_body(f, body, dest);
        f.env.truncate(depth);
        r
    }

    /// A body run under a dynamic extent: compiled as a zero-argument
    /// closure (or a one-argument handler binding at the current
    /// frame length) whose value lands in `dest`.
    fn body_closure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        params: usize,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let fun = FunExpr {
            display: "clojure.core/fn".to_owned(),
            arities: std::sync::Arc::from([Arity {
                fixed: (0..params)
                    .map(|i| Pattern::Symbol(format!("p{i}")))
                    .collect(),
                rest: None,
                body: body.to_vec(),
            }]),
        };
        let base = f.env.len();
        self.make_closure(f, &fun, base, dest)
    }

    fn lower_try(
        &mut self,
        f: &mut FnCtx<'ctx>,
        body: &[Expr],
        catches: &[clj_eval::expr::Catch],
        fin: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let b = self.slot(f);
        self.body_closure(f, 0, body, b)?;
        let (harr, hs) = self.slot_array(f, catches.len());
        let mut classes = Vec::new();
        for (c, h) in catches.iter().zip(&hs) {
            assert_eq!(c.slot, f.env.len(), "a catch binds the next local slot");
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
        let href = self.slots_ref(f, harr, catches.len());
        let st = self
            .call(
                "rt_n_try",
                Some(self.i32t),
                &[
                    dest.into(),
                    b.into(),
                    self.u64c(ck),
                    href.into(),
                    fin_ptr.into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    fn lower_binding(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pairs: &[(clj_value::Var, Expr)],
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let exprs: Vec<Expr> = pairs.iter().map(|(_, e)| e.clone()).collect();
        let vals = self.lower_args(f, &exprs)?;
        let ks: Vec<u64> = pairs.iter().map(|(v, _)| self.var_of(v)).collect();
        let karr = self.u64_array(&ks);
        let b = self.slot(f);
        self.body_closure(f, 0, body, b)?;
        let vref = self.slots_ref(f, vals, pairs.len());
        let st = self
            .call(
                "rt_n_binding",
                Some(self.i32t),
                &[dest.into(), karr.into(), vref.into(), b.into()],
            )
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    /// A constant `CSlice<u64>` header over `values`.
    fn u64_array(&mut self, values: &[u64]) -> PointerValue<'ctx> {
        if values.is_empty() {
            return self.const_slice(self.ptr.const_null(), 0);
        }
        let consts: Vec<IntValue> = values
            .iter()
            .map(|v| self.i64t.const_int(*v, false))
            .collect();
        let arr = self.i64t.const_array(&consts);
        let g = self.const_global(&arr, "cljk");
        self.const_slice(g, values.len() as u64)
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_def(
        &mut self,
        f: &mut FnCtx<'ctx>,
        sym: &Symbol,
        doc: Option<&str>,
        init: Option<&Expr>,
        meta: Option<&[(Expr, Expr)]>,
        span: clj_reader::Span,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let sk = self.value_const(&Value::Symbol(sym.clone()))?;
        let dk = match doc {
            Some(d) => self.constant(Const::Str(d.to_owned())),
            None => NONE,
        };
        let id = self
            .call("rt_n_def_begin", Some(self.i64t), &[self.u64c(sk)])
            .expect("id");
        let value = match init {
            Some(e) => {
                let t = self.slot(f);
                self.lower(f, e, t)?;
                t
            }
            None => self.ptr.const_null(),
        };
        let mref = match meta {
            Some(pairs) => {
                let flat: Vec<Expr> = pairs
                    .iter()
                    .flat_map(|(k, v)| [k.clone(), v.clone()])
                    .collect();
                let marr = self.lower_args(f, &flat)?;
                self.slots_ref(f, marr, flat.len())
            }
            None => self.ptr.const_null(),
        };
        let st = self
            .call(
                "rt_n_def_finish",
                Some(self.i32t),
                &[
                    dest.into(),
                    id.into(),
                    self.u64c(sk),
                    self.u64c(dk),
                    value.into(),
                    mref.into(),
                    self.i32t.const_int(u64::from(span.line), false).into(),
                    self.i32t.const_int(u64::from(span.col), false).into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_case(
        &mut self,
        f: &mut FnCtx<'ctx>,
        expr: &Expr,
        shift: i32,
        mask: i32,
        default: &Expr,
        cases: &[(i32, Expr, Expr)],
        skip: &[i32],
        test: CaseTest,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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
            Value::Vector(Vector::from_vec(
                cases
                    .iter()
                    .map(|(k, _, _)| Value::Int(i64::from(*k)))
                    .collect(),
            )),
            Value::Vector(Vector::from_vec(
                skip.iter().map(|k| Value::Int(i64::from(*k))).collect(),
            )),
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
            .call(
                "rt_n_case",
                Some(self.i32t),
                &[sel.into(), realized.into(), v.into(), self.u64c(ck)],
            )
            .expect("st");
        self.check(f, st);
        let selv = self
            .builder
            .build_load(self.i64t, sel, "sel")
            .expect("load")
            .into_int_value();
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
        self.builder
            .build_conditional_branch(is_default, default_b, dispatch)
            .expect("br");
        self.builder.position_at_end(dispatch);
        let mut arms = Vec::new();
        let mut blocks = Vec::new();
        for i in 0..cases.len() {
            let b = self.ctx.append_basic_block(f.func, "case_arm");
            arms.push((self.i64t.const_int(i as u64, false), b));
            blocks.push(b);
        }
        self.builder
            .build_switch(idx, default_b, &arms)
            .expect("switch");
        for ((_, t, then), b) in cases.iter().zip(blocks) {
            self.builder.position_at_end(b);
            let do_check = self
                .builder
                .build_int_compare(IntPredicate::NE, checked, self.i64t.const_zero(), "c")
                .expect("cmp");
            let check_b = self.ctx.append_basic_block(f.func, "case_check");
            let then_b = self.ctx.append_basic_block(f.func, "case_then");
            self.builder
                .build_conditional_branch(do_check, check_b, then_b)
                .expect("br");
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
                .call(
                    "rt_n_equiv",
                    Some(self.i32t),
                    &[eq.into(), realized.into(), tv.into()],
                )
                .expect("st");
            self.check(f, st);
            let eqv = self
                .builder
                .build_load(self.i8t, eq, "eqv")
                .expect("load")
                .into_int_value();
            let hit = self
                .builder
                .build_int_compare(IntPredicate::NE, eqv, self.i8t.const_zero(), "hit")
                .expect("cmp");
            self.builder
                .build_conditional_branch(hit, then_b, default_b)
                .expect("br");
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
                Const::Double(d) => {
                    let fv = self.ctx.f64_type().const_float(*d);
                    self.call("rt_n_double", None, &[t.into(), fv.into()]);
                }
                Const::Str(s) => {
                    let p = self.str_ptr(s);
                    self.call("rt_n_str", None, &[t.into(), p.into()]);
                }
                Const::Keyword(ns, name) | Const::Symbol(ns, name) => {
                    let np = match ns {
                        Some(ns) => self.str_ptr(ns),
                        None => self.ptr.const_null(),
                    };
                    let p = self.str_ptr(name);
                    let h = if matches!(c, Const::Keyword(..)) {
                        "rt_n_kw"
                    } else {
                        "rt_n_sym"
                    };
                    self.call(h, None, &[t.into(), np.into(), p.into()]);
                }
                Const::Regex(text) => {
                    let p = self.str_ptr(text);
                    self.call("rt_n_regex", None, &[t.into(), p.into()]);
                }
                Const::StaticField(class, field) => {
                    let cp = self.str_ptr(class);
                    let fp = self.str_ptr(field);
                    self.call("rt_n_static_field", None, &[t.into(), cp.into(), fp.into()]);
                }
                Const::Class(name) => {
                    let p = self.str_ptr(name);
                    self.call("rt_n_class", None, &[t.into(), p.into()]);
                }
                Const::Read(text) => {
                    let p = self.str_ptr(text);
                    self.call("rt_n_read", None, &[t.into(), p.into()]);
                }
            }
            self.call("rt_n_set_const", None, &[self.u64c(k as u64), t.into()]);
        }
        let result = self.slot(&mut f);
        self.end_fn(f, result)
    }

    fn lower_main(&mut self, path: &str, init: FunctionValue<'ctx>, tops: &[FunctionValue<'ctx>]) {
        let ty = self
            .i32t
            .fn_type(&[self.i32t.into(), self.ptr.into()], false);
        let main = self.module.add_function("main", ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        let vars = self.vars.clone();
        for (k, (ns, name)) in vars.iter().enumerate() {
            let np = self.str_ptr(ns);
            let p = self.str_ptr(name);
            self.call(
                "rt_n_declare_var",
                None,
                &[self.u64c(k as u64), np.into(), p.into()],
            );
        }
        let fns: Vec<(String, Vec<ArityCode>)> = self
            .fns
            .iter()
            .map(|i| (i.display.clone(), i.arities.clone()))
            .collect();
        for (k, (display, arities)) in fns.iter().enumerate() {
            let dp = self.str_ptr(display);
            let arity_ty = self.ctx.struct_type(
                &[self.i64t.into(), self.i64t.into(), self.ptr.into()],
                false,
            );
            let descs: Vec<inkwell::values::StructValue> = arities
                .iter()
                .map(|a| {
                    arity_ty.const_named_struct(&[
                        self.i64t.const_int(a.0 as u64, false).into(),
                        self.i64t.const_int(u64::from(a.1), false).into(),
                        a.2.as_global_value().as_pointer_value().into(),
                    ])
                })
                .collect();
            let arr = arity_ty.const_array(&descs);
            let g = self.const_global(&arr, "clja");
            let aref = self.const_slice(g, arities.len() as u64);
            self.call(
                "rt_n_declare_fn",
                None,
                &[self.u64c(k as u64), dp.into(), aref.into()],
            );
        }
        let top_ptrs: Vec<PointerValue> = tops
            .iter()
            .map(|t| t.as_global_value().as_pointer_value())
            .collect();
        let tops_arr = self.ptr.const_array(&top_ptrs);
        let tops_g = self.const_global(&tops_arr, "cljtops");
        let tops_ref = self.const_slice(tops_g, tops.len() as u64);
        let pp = self.str_ptr(path);
        let argc = main.get_nth_param(0).expect("argc").into_int_value();
        let argv = main.get_nth_param(1).expect("argv");
        let ty = self.slice_ty();
        let argv_ref = self.builder.build_alloca(ty, "argv").expect("alloca");
        let p0 = self
            .builder
            .build_struct_gep(ty, argv_ref, 0, "ap")
            .expect("gep");
        self.builder.build_store(p0, argv).expect("store");
        let n = self
            .builder
            .build_int_z_extend(argc, self.i64t, "argc")
            .expect("zext");
        let p1 = self
            .builder
            .build_struct_gep(ty, argv_ref, 1, "an")
            .expect("gep");
        self.builder.build_store(p1, n).expect("store");
        let code = self
            .call(
                "rt_n_main",
                Some(self.i32t),
                &[
                    pp.into(),
                    argv_ref.into(),
                    init.as_global_value().as_pointer_value().into(),
                    tops_ref.into(),
                ],
            )
            .expect("code");
        self.builder.build_return(Some(&code)).expect("ret");
    }
}

/// The `FastArith` op a direct call inlines (`inc`/`dec` take one
/// arg, the rest two).
fn fast_op(fast: clj_eval::ext::FastArith, n: usize) -> Option<clj_eval::ext::FastArith> {
    use clj_eval::ext::FastArith;
    match (fast, n) {
        (FastArith::Inc | FastArith::Dec, 1) => Some(fast),
        (FastArith::None | FastArith::Inc | FastArith::Dec, _) => None,
        (other, 2) => Some(other),
        _ => None,
    }
}

/// A value with a direct constant builder (`None`: the reader
/// fallback).
fn scalar_const(v: &Value) -> Option<Const> {
    Some(match v {
        Value::Double(d) => Const::Double(*d),
        Value::Str(s) => Const::Str(s.to_string()),
        Value::Keyword(k) => Const::Keyword(k.namespace().map(str::to_owned), k.name().to_owned()),
        Value::Symbol(s) if s.meta().is_none() => {
            Const::Symbol(s.namespace().map(str::to_owned), s.name().to_owned())
        }
        _ => return host_const(v),
    })
}

/// Host-derived constants rebuilt by name at startup (regexes,
/// classes, folded static fields).
fn host_const(v: &Value) -> Option<Const> {
    match v {
        Value::Regex(p) => Some(Const::Regex(p.to_string())),
        Value::Class(_) => Some(Const::Class(
            native_abi::class_name_of(v).unwrap_or_default(),
        )),
        other => native_abi::static_field_origin(other)
            .map(|(class, field)| Const::StaticField(class.to_owned(), field.to_owned())),
    }
}

/// Whether an arity body `recur`s to the arity itself (a `recur` outside
/// any nested `loop` or `fn`, which have their own targets).
fn self_recurs(body: &[Expr]) -> bool {
    fn walk(e: &Expr) -> bool {
        match e {
            Expr::Recur(_) => true,
            Expr::Loop { bindings, .. } => bindings.iter().any(|(_, init)| walk(init)),
            Expr::Fn(_) => false,
            _ => {
                let mut found = false;
                for_each_child(e, &mut |c| found = found || walk(c));
                found
            }
        }
    }
    body.iter().any(walk)
}

/// The locals a `recur` to a loop at env depth `base` may take by move:
/// scoped inside the loop (`>= base`), held in a plain slot, and used
/// at most once on any path through the `recur` args (nested fns
/// included, so a capture counts as a use). They are dead once `recur`
/// rebinds. An `if` uses its test plus the busier of its branches (only
/// one runs); an occurrence inside a nested `loop` runs once per inner
/// iteration, so it counts as many.
fn movable_locals(f: &FnCtx<'_>, args: &[Expr], base: Option<usize>) -> Vec<usize> {
    fn count(e: &Expr, weight: usize, uses: &mut BTreeMap<usize, usize>) {
        if let Expr::Local(i) = e {
            *uses.entry(*i).or_default() += weight;
        }
        if let Expr::If { test, then, els } = e {
            count(test, weight, uses);
            let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
            count(then, weight, &mut a);
            count(els, weight, &mut b);
            for (i, n) in &b {
                let busier = a.entry(*i).or_default();
                *busier = (*busier).max(*n);
            }
            for (i, n) in a {
                *uses.entry(i).or_default() += n;
            }
            return;
        }
        let inner = if matches!(e, Expr::Loop { .. }) {
            2
        } else {
            weight
        };
        for_each_child(e, &mut |c| count(c, inner, uses));
    }
    let Some(base) = base else {
        return Vec::new();
    };
    let mut uses = BTreeMap::new();
    for a in args {
        count(a, 1, &mut uses);
    }
    uses.into_iter()
        .filter(|&(i, n)| n == 1 && i >= base)
        .filter(|&(i, _)| matches!(f.env.get(i), Some(Some(LocalRef::Slot(_)))))
        .map(|(i, _)| i)
        .collect()
}

/// The free locals (`< base`) an arity mentions, nested fns included
/// (absolute slot numbering makes nesting transparent).
fn arity_free(arity: &Arity, base: usize, out: &mut BTreeSet<usize>) {
    fn walk(e: &Expr, base: usize, out: &mut BTreeSet<usize>) {
        if let Expr::Local(i) = e
            && *i < base
        {
            out.insert(*i);
        }
        for_each_child(e, &mut |c| walk(c, base, out));
    }
    let probe = Expr::Fn(FunExpr {
        display: String::new(),
        arities: std::sync::Arc::from([arity.clone()]),
    });
    for_each_child(&probe, &mut |c| walk(c, base, out));
}

/// Whether a unit can rebind a core var at run time (then inline
/// arithmetic would not mean the current root).
fn mentions_rebinding(e: &Expr) -> bool {
    // (`alter-meta!` is benign here: `defn` itself uses it for
    // `:arglists`.)
    // (The var-resolving fns hand out any var — `alter-var-root`
    // included — without naming it statically.)
    const REBIND: [&str; 12] = [
        "alter-var-root",
        "with-redefs-fn",
        "intern",
        "ns-unmap",
        "eval",
        "resolve",
        "ns-resolve",
        "find-var",
        "requiring-resolve",
        "ns-interns",
        "ns-publics",
        "ns-map",
    ];
    let core_rebind = |v: &clj_value::Var| {
        v.sym.namespace() == Some("clojure.core") && REBIND.contains(&v.sym.name())
    };
    let hit = match e {
        Expr::WithRedefs { .. } => true,
        Expr::Global(v) | Expr::VarRef(v) => core_rebind(v),
        Expr::DirectNative(c) => core_rebind(&c.var),
        Expr::Def { sym, .. } => sym.namespace() == Some("clojure.core"),
        _ => false,
    };
    let mut found = hit;
    if !found {
        for_each_child(e, &mut |c| found = found || mentions_rebinding(c));
    }
    found
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

/// A symbol-safe fragment of a display name.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(40)
        .collect()
}
