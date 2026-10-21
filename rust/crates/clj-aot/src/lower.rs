// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Plan → LLVM module lowering (S8C; D1/D2/D7/D8).
//!
//! Every value is an opaque `ptr` to an owned box. One LLVM
//! function per `defn` (`extern` runtime helpers per primitive
//! call); `@main` drives the tops and returns 0. Locals clone on
//! every use; all frame slots drop at scope end; fallible calls
//! null-check into an abort block that calls libc `exit(1)`.
//!
//! Builder calls use `.expect()`: the IR is correct by
//! construction (verified by every snapshot test), so a builder
//! failure is a compiler bug and panics loudly at build time.

use std::collections::HashMap;

use clj_eval::{Prim, SubsetExpr, SubsetFn, SubsetProgram};
use cljrt::prim_symbol;
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::types::{FloatType, IntType, PointerType};
use inkwell::values::{BasicMetadataValueEnum, FunctionValue, IntValue, PointerValue};

/// Prims in discriminant order (index `== prim as usize`).
const PRIM_ORDER: [Prim; 13] = [
    Prim::Add,
    Prim::Sub,
    Prim::Mul,
    Prim::Quot,
    Prim::Lt,
    Prim::Le,
    Prim::Gt,
    Prim::Ge,
    Prim::Eq,
    Prim::Count,
    Prim::Nth,
    Prim::Get,
    Prim::Conj,
];

/// The LLVM symbol per planned `defn`: sanitized name plus source
/// index (the index disambiguates; the prefix keeps clear of the
/// `rt_*` runtime and `main`).
#[must_use]
pub fn fn_symbol(index: usize, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("cljfn_{clean}_{index}")
}

/// Declared runtime helpers (resolved once per module).
struct Decls<'ctx> {
    prims: [FunctionValue<'ctx>; 13],
    nil: FunctionValue<'ctx>,
    boolean: FunctionValue<'ctx>,
    int: FunctionValue<'ctx>,
    double: FunctionValue<'ctx>,
    str_: FunctionValue<'ctx>,
    vec_new: FunctionValue<'ctx>,
    vec_push: FunctionValue<'ctx>,
    clone: FunctionValue<'ctx>,
    drop_: FunctionValue<'ctx>,
    print: FunctionValue<'ctx>,
    arity_error: FunctionValue<'ctx>,
    is_truthy: FunctionValue<'ctx>,
    exit: FunctionValue<'ctx>,
}

/// The lowering session: one module, one builder, interned string
/// globals, the baked source path, and sibling declarations.
struct Lower<'ctx> {
    ctx: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    ptr: PointerType<'ctx>,
    i8t: IntType<'ctx>,
    i32t: IntType<'ctx>,
    i64t: IntType<'ctx>,
    f64t: FloatType<'ctx>,
    decls: Decls<'ctx>,
    strings: HashMap<String, PointerValue<'ctx>>,
    path: String,
    fns: Vec<FunctionValue<'ctx>>,
    /// Whether an arity abort diverged the current straight-line
    /// span (once set, everything downstream is unreachable until
    /// the next `if` join resets it per branch).
    diverged: bool,
}

impl<'ctx> Lower<'ctx> {
    /// New session over a fresh module (declares every helper,
    /// interns the baked source path).
    fn new(ctx: &'ctx Context, source_path: &str) -> Self {
        let module = ctx.create_module("clj");
        let builder = ctx.create_builder();
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i8t = ctx.i8_type();
        let i32t = ctx.i32_type();
        let i64t = ctx.i64_type();
        let f64t = ctx.f64_type();
        let voidt = ctx.void_type();
        let prim_ty = ptr.fn_type(
            &[
                ptr.into(),
                i64t.into(),
                ptr.into(),
                i64t.into(),
                i32t.into(),
                i32t.into(),
            ],
            false,
        );
        let decls = Decls {
            prims: PRIM_ORDER.map(|prim| module.add_function(prim_symbol(prim), prim_ty, None)),
            nil: module.add_function("rt_nil", ptr.fn_type(&[], false), None),
            boolean: module.add_function("rt_bool", ptr.fn_type(&[i8t.into()], false), None),
            int: module.add_function("rt_int", ptr.fn_type(&[i64t.into()], false), None),
            double: module.add_function("rt_double", ptr.fn_type(&[f64t.into()], false), None),
            str_: module.add_function(
                "rt_str",
                ptr.fn_type(&[ptr.into(), i64t.into()], false),
                None,
            ),
            vec_new: module.add_function("rt_vec_new", ptr.fn_type(&[], false), None),
            vec_push: module.add_function(
                "rt_vec_push",
                ptr.fn_type(&[ptr.into(), ptr.into()], false),
                None,
            ),
            clone: module.add_function("rt_clone", ptr.fn_type(&[ptr.into()], false), None),
            drop_: module.add_function("rt_drop", voidt.fn_type(&[ptr.into()], false), None),
            print: module.add_function(
                "rt_print",
                ptr.fn_type(&[ptr.into(), i64t.into()], false),
                None,
            ),
            arity_error: module.add_function(
                "rt_arity_error",
                ptr.fn_type(
                    &[
                        ptr.into(),
                        i64t.into(),
                        i64t.into(),
                        ptr.into(),
                        i64t.into(),
                        i32t.into(),
                        i32t.into(),
                    ],
                    false,
                ),
                None,
            ),
            is_truthy: module.add_function("rt_is_truthy", i8t.fn_type(&[ptr.into()], false), None),
            exit: module.add_function("exit", voidt.fn_type(&[i32t.into()], false), None),
        };
        Self {
            ctx,
            module,
            builder,
            ptr,
            i8t,
            i32t,
            i64t,
            f64t,
            decls,
            strings: HashMap::new(),
            path: source_path.to_owned(),
            fns: Vec::new(),
            diverged: false,
        }
    }

    /// The baked source path (interned on first use — the builder
    /// must be positioned, which holds at every call site).
    fn path_const(&mut self) -> (PointerValue<'ctx>, u64) {
        let path = self.path.clone();
        self.const_str(&path)
    }

    /// Intern a string global (deduplicated across call sites).
    fn const_str(&mut self, text: &str) -> (PointerValue<'ctx>, u64) {
        let len = text.len() as u64;
        if let Some(hit) = self.strings.get(text) {
            return (*hit, len);
        }
        let name = format!("cljstr{}", self.strings.len());
        let global = self
            .builder
            .build_global_string_ptr(text, &name)
            .expect("string global");
        let ptr = global.as_pointer_value();
        self.strings.insert(text.to_owned(), ptr);
        (ptr, len)
    }

    /// Call a helper returning an owned box.
    fn call_ptr(
        &self,
        func: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> PointerValue<'ctx> {
        self.builder
            .build_call(func, args, "call")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("box call")
            .into_pointer_value()
    }

    /// Call a helper returning nothing.
    fn call_void(&self, func: FunctionValue<'ctx>, args: &[BasicMetadataValueEnum<'ctx>]) {
        let _ignored = self.builder.build_call(func, args, "").expect("call");
    }

    /// Call a helper returning a byte.
    fn call_byte(
        &self,
        func: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> IntValue<'ctx> {
        self.builder
            .build_call(func, args, "call")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("byte call")
            .into_int_value()
    }

    /// Drop one owned box.
    fn call_drop(&self, value: PointerValue<'ctx>) {
        self.call_void(self.decls.drop_, &[value.into()]);
    }

    /// Drop every frame slot (all hold valid owned boxes: params
    /// plus one store per `let` binding, never reused).
    fn drop_slots(&self, slots: &[PointerValue<'ctx>]) {
        for slot in slots {
            let value = self
                .builder
                .build_load(self.ptr, *slot, "slot")
                .expect("load")
                .into_pointer_value();
            self.call_drop(value);
        }
    }

    /// Abort the binary: `exit(1)` plus `unreachable`.
    fn emit_abort(&self) {
        self.call_void(self.decls.exit, &[self.i32t.const_int(1, false).into()]);
        self.builder.build_unreachable().expect("unreachable");
    }

    /// Null-check a fallible result: null exits 1, else the value
    /// flows on (single-predecessor join, no phi needed).
    fn null_check(&mut self, value: PointerValue<'ctx>) -> PointerValue<'ctx> {
        let func = self
            .builder
            .get_insert_block()
            .expect("block")
            .get_parent()
            .expect("parent");
        let is_null = self.builder.build_is_null(value, "isnull").expect("isnull");
        let abort_bb = self.ctx.append_basic_block(func, "abort");
        let cont_bb = self.ctx.append_basic_block(func, "cont");
        self.builder
            .build_conditional_branch(is_null, abort_bb, cont_bb)
            .expect("condbr");
        self.builder.position_at_end(abort_bb);
        self.emit_abort();
        self.builder.position_at_end(cont_bb);
        value
    }

    /// Pack lowered arguments into an array for a helper call
    /// (always at least one slot — zero-length allocas are
    /// avoided even for empty calls).
    fn pack_args(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        args: &[SubsetExpr],
    ) -> (PointerValue<'ctx>, IntValue<'ctx>) {
        let array_ty = self.ptr.array_type(args.len().max(1) as u32);
        let array = self
            .builder
            .build_alloca(array_ty, "argv")
            .expect("alloca argv");
        let zero = self.i32t.const_zero();
        for (i, arg) in args.iter().enumerate() {
            let value = self.lower_expr(func, slots, arg);
            // Safety: index `i` is in bounds (`i < args.len()`).
            let slot = unsafe {
                self.builder.build_in_bounds_gep(
                    array_ty,
                    array,
                    &[zero, self.i32t.const_int(i as u64, false)],
                    "arg",
                )
            }
            .expect("gep");
            self.builder.build_store(slot, value).expect("store arg");
        }
        // Safety: the base element always exists (`max(1)` slot).
        let base = unsafe {
            self.builder
                .build_in_bounds_gep(array_ty, array, &[zero, zero], "argv0")
        }
        .expect("gep");
        (base, self.i64t.const_int(args.len() as u64, false))
    }

    /// Lower an expression sequence: intermediates dropped, the
    /// last value returned (empty is nil).
    fn lower_seq(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        exprs: &[SubsetExpr],
    ) -> PointerValue<'ctx> {
        if exprs.is_empty() {
            return self.call_ptr(self.decls.nil, &[]);
        }
        let mut last = self.ptr.const_null();
        for (i, expr) in exprs.iter().enumerate() {
            let value = self.lower_expr(func, slots, expr);
            if i + 1 < exprs.len() {
                self.call_drop(value);
            } else {
                last = value;
            }
        }
        last
    }

    /// Lower one planned expression to an owned box (diverging
    /// forms — arity aborts — terminate the block and continue in
    /// a fresh one with a null value, so callers never branch).
    /// Literals lower here; compound forms and calls chain on so
    /// each match stays under the complexity gate.
    fn lower_expr(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        expr: &SubsetExpr,
    ) -> PointerValue<'ctx> {
        match expr {
            SubsetExpr::Nil => self.call_ptr(self.decls.nil, &[]),
            SubsetExpr::Bool(b) => self.call_ptr(
                self.decls.boolean,
                &[self.i8t.const_int(u64::from(*b), false).into()],
            ),
            SubsetExpr::Int(n) => self.call_ptr(
                self.decls.int,
                &[self.i64t.const_int(*n as u64, false).into()],
            ),
            SubsetExpr::Double(x) => {
                self.call_ptr(self.decls.double, &[self.f64t.const_float(*x).into()])
            }
            SubsetExpr::Str(s) => {
                let (ptr, len) = self.const_str(s);
                self.call_ptr(
                    self.decls.str_,
                    &[ptr.into(), self.i64t.const_int(len, false).into()],
                )
            }
            other => self.lower_compound(func, slots, other),
        }
    }

    /// Lower compound forms (vectors, locals, control flow).
    fn lower_compound(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        expr: &SubsetExpr,
    ) -> PointerValue<'ctx> {
        match expr {
            SubsetExpr::VecLit(elems) => {
                let mut acc = self.call_ptr(self.decls.vec_new, &[]);
                for elem in elems {
                    let value = self.lower_expr(func, slots, elem);
                    acc = self.call_ptr(self.decls.vec_push, &[acc.into(), value.into()]);
                }
                acc
            }
            SubsetExpr::Local(i) => {
                let slot = self
                    .builder
                    .build_load(self.ptr, slots[*i], "local")
                    .expect("load")
                    .into_pointer_value();
                self.call_ptr(self.decls.clone, &[slot.into()])
            }
            SubsetExpr::If { test, then, els } => self.lower_if(func, slots, test, then, els),
            SubsetExpr::Do(exprs) => self.lower_seq(func, slots, exprs),
            SubsetExpr::Let { bindings, body } => {
                for (slot, init) in bindings {
                    let value = self.lower_expr(func, slots, init);
                    self.builder
                        .build_store(slots[*slot], value)
                        .expect("store let");
                }
                self.lower_seq(func, slots, body)
            }
            other => self.lower_call(func, slots, other),
        }
    }

    /// Lower calls (primitives, siblings, arity aborts, print).
    /// Anything else re-dispatches from the top (defense in depth
    /// — the chain only ever routes calls here — terminating after
    /// exactly one hop since `lower_expr` routes correctly).
    fn lower_call(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        expr: &SubsetExpr,
    ) -> PointerValue<'ctx> {
        match expr {
            SubsetExpr::CallPrim { prim, args, span } => {
                let (argv, argc) = self.pack_args(func, slots, args);
                let (path_ptr, path_len) = self.path_const();
                let called = self.call_ptr(
                    self.decls.prims[*prim as usize],
                    &[
                        argv.into(),
                        argc.into(),
                        path_ptr.into(),
                        self.i64t.const_int(path_len, false).into(),
                        self.i32t.const_int(u64::from(span.line), false).into(),
                        self.i32t.const_int(u64::from(span.col), false).into(),
                    ],
                );
                self.null_check(called)
            }
            SubsetExpr::CallFn { index, args, .. } => {
                let mut params: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
                for arg in args {
                    params.push(self.lower_expr(func, slots, arg).into());
                }
                let called = self.call_ptr(self.fns[*index], &params);
                self.null_check(called)
            }
            SubsetExpr::ArityError {
                display, got, span, ..
            } => {
                let (display_ptr, display_len) = self.const_str(display);
                let (path_ptr, path_len) = self.path_const();
                let _reported = self
                    .builder
                    .build_call(
                        self.decls.arity_error,
                        &[
                            display_ptr.into(),
                            self.i64t.const_int(display_len, false).into(),
                            self.i64t.const_int(*got as u64, false).into(),
                            path_ptr.into(),
                            self.i64t.const_int(path_len, false).into(),
                            self.i32t.const_int(u64::from(span.line), false).into(),
                            self.i32t.const_int(u64::from(span.col), false).into(),
                        ],
                        "arity",
                    )
                    .expect("arity call");
                self.emit_abort();
                let cont = self.ctx.append_basic_block(func, "cont");
                self.builder.position_at_end(cont);
                self.diverged = true;
                self.ptr.const_null()
            }
            SubsetExpr::Print { args, .. } => {
                let (argv, argc) = self.pack_args(func, slots, args);
                self.call_ptr(self.decls.print, &[argv.into(), argc.into()])
            }
            other => self.lower_expr(func, slots, other),
        }
    }

    /// Lower `if`: the test consumes through `rt_is_truthy`; each
    /// branch joins only when it reaches (an arity abort diverges
    /// through, never joining) — two joiners share a phi, one
    /// passes its value straight through, none yields null (and the
    /// `if` itself diverges only when both branches do).
    fn lower_if(
        &mut self,
        func: FunctionValue<'ctx>,
        slots: &[PointerValue<'ctx>],
        test: &SubsetExpr,
        then: &SubsetExpr,
        els: &SubsetExpr,
    ) -> PointerValue<'ctx> {
        let test_val = self.lower_expr(func, slots, test);
        let test_div = std::mem::replace(&mut self.diverged, false);
        let flag = self.call_byte(self.decls.is_truthy, &[test_val.into()]);
        let cond = self
            .builder
            .build_int_compare(IntPredicate::NE, flag, self.i8t.const_zero(), "truthy")
            .expect("compare");
        let then_bb = self.ctx.append_basic_block(func, "then");
        let else_bb = self.ctx.append_basic_block(func, "else");
        let merge_bb = self.ctx.append_basic_block(func, "merge");
        self.builder
            .build_conditional_branch(cond, then_bb, else_bb)
            .expect("condbr");
        let mut incoming: Vec<(PointerValue<'ctx>, inkwell::basic_block::BasicBlock)> =
            Vec::with_capacity(2);
        self.builder.position_at_end(then_bb);
        self.diverged = false;
        let then_val = self.lower_expr(func, slots, then);
        let then_div = std::mem::replace(&mut self.diverged, false);
        if then_div {
            self.builder.build_unreachable().expect("seal");
        } else {
            let pred = self.builder.get_insert_block().expect("then block");
            self.builder
                .build_unconditional_branch(merge_bb)
                .expect("br");
            incoming.push((then_val, pred));
        }
        self.builder.position_at_end(else_bb);
        let else_val = self.lower_expr(func, slots, els);
        let else_div = std::mem::replace(&mut self.diverged, false);
        if else_div {
            self.builder.build_unreachable().expect("seal");
        } else {
            let pred = self.builder.get_insert_block().expect("else block");
            self.builder
                .build_unconditional_branch(merge_bb)
                .expect("br");
            incoming.push((else_val, pred));
        }
        self.builder.position_at_end(merge_bb);
        self.diverged = test_div || (then_div && else_div);
        if incoming.len() > 1 {
            let phi = self.builder.build_phi(self.ptr, "if").expect("phi");
            for (value, pred) in &incoming {
                phi.add_incoming(&[(value, *pred)]);
            }
            phi.as_basic_value().into_pointer_value()
        } else if let Some((single, _)) = incoming.first() {
            *single
        } else {
            self.ptr.const_null()
        }
    }

    /// Lower one sibling `defn` (declared before its body so
    /// direct self-calls resolve; siblings only call backwards).
    fn lower_defn(&mut self, index: usize, def: &SubsetFn) {
        let params: Vec<BasicMetadataTypeEnum> = vec![self.ptr.into(); def.arity];
        let ty = self.ptr.fn_type(params.as_slice(), false);
        let func = self
            .module
            .add_function(&fn_symbol(index, &def.name), ty, None);
        self.fns.push(func);
        let entry = self.ctx.append_basic_block(func, "entry");
        self.builder.position_at_end(entry);
        self.diverged = false;
        let mut slots = Vec::with_capacity(def.frame_slots);
        for i in 0..def.frame_slots {
            slots.push(
                self.builder
                    .build_alloca(self.ptr, &format!("slot{i}"))
                    .expect("alloca slot"),
            );
        }
        for (i, slot) in slots.iter().enumerate().take(def.arity) {
            let param = func
                .get_nth_param(i as u32)
                .expect("param")
                .into_pointer_value();
            self.builder.build_store(*slot, param).expect("store param");
        }
        let retval = self.lower_expr(func, &slots, &def.body);
        self.drop_slots(&slots);
        self.builder.build_return(Some(&retval)).expect("return");
    }

    /// Lower the `main` statements (values discarded, slots
    /// dropped, exit 0 — abnormal exits happen in abort blocks).
    fn lower_main(&mut self, main: &[SubsetExpr], main_slots: usize) {
        let ty = self.i32t.fn_type(&[], false);
        let func = self.module.add_function("main", ty, None);
        let entry = self.ctx.append_basic_block(func, "entry");
        self.builder.position_at_end(entry);
        self.diverged = false;
        let mut slots = Vec::with_capacity(main_slots);
        for i in 0..main_slots {
            slots.push(
                self.builder
                    .build_alloca(self.ptr, &format!("slot{i}"))
                    .expect("alloca slot"),
            );
        }
        for stmt in main {
            let value = self.lower_expr(func, &slots, stmt);
            self.call_drop(value);
        }
        self.drop_slots(&slots);
        self.builder
            .build_return(Some(&self.i32t.const_zero()))
            .expect("return");
    }
}

/// Verify a lowered module, mapping LLVM's free-text rejection
/// to an `Emit`-bound message. Uses the inherent lossy
/// `to_string`: `LLVMString`'s `Display` impl debug-quotes the
/// text.
pub(crate) fn verify_module(module: &Module) -> Result<(), String> {
    module
        .verify()
        .map_err(|e| format!("LLVM verification failed: {}", e.to_string().trim()))
}

/// Lower a planned program to an LLVM module (the source path is
/// baked into every error span). The module is verified before
/// return: a rejection means a lowering bug, reported as `Err`
/// instead of reaching emission.
pub fn to_module<'ctx>(
    ctx: &'ctx Context,
    program: &SubsetProgram,
    source_path: &str,
) -> Result<Module<'ctx>, String> {
    let mut lower = Lower::new(ctx, source_path);
    for (index, def) in program.fns.iter().enumerate() {
        lower.lower_defn(index, def);
    }
    lower.lower_main(&program.main, program.main_slots);
    verify_module(&lower.module)?;
    Ok(lower.module)
}

#[cfg(test)]
mod tests {
    use super::{PRIM_ORDER, fn_symbol, to_module, verify_module};
    use clj_eval::{SubsetExpr, plan_source};
    use cljrt::prim_symbol;
    use inkwell::context::Context;

    /// Lower `source` and return verified IR (every snapshot pins
    /// `contains`, and verification proves the SSA well-formed —
    /// phis, dominance, and the arity-divergence joins).
    fn compile_ir(source: &str) -> String {
        let program = plan_source(source).expect("plan");
        let ctx = Context::create();
        let module = to_module(&ctx, &program, "snap.clj").expect("verify");
        module.print_to_string().to_string()
    }

    #[test]
    fn verify_module_maps_verifier_text() {
        let ctx = Context::create();
        let module = ctx.create_module("broken");
        let ty = ctx.i32_type().fn_type(&[], false);
        let func = module.add_function("no_terminator", ty, None);
        let _entry = ctx.append_basic_block(func, "entry");
        let err = verify_module(&module).unwrap_err();
        assert!(err.starts_with("LLVM verification failed: "), "{err}");
        assert!(err.contains("terminator"), "{err}");
    }

    #[test]
    fn fn_symbol_sanitizes_and_indexes() {
        assert_eq!(fn_symbol(0, "f"), "cljfn_f_0");
        assert_eq!(fn_symbol(1, "my-fn?"), "cljfn_my_fn__1");
        assert_eq!(fn_symbol(12, "a/b.c"), "cljfn_a_b_c_12");
    }

    #[test]
    fn prim_order_matches_discriminants() {
        let names = [
            "rt_add", "rt_sub", "rt_mul", "rt_quot", "rt_lt", "rt_le", "rt_gt", "rt_ge", "rt_eq",
            "rt_count", "rt_nth", "rt_get", "rt_conj",
        ];
        for (index, prim) in PRIM_ORDER.iter().enumerate() {
            assert_eq!(*prim as usize, index);
            assert_eq!(prim_symbol(*prim), names[index]);
        }
        assert_eq!(format!("{PRIM_ORDER:?}").split(',').count(), 13);
    }

    #[test]
    fn every_prim_lowers_to_its_symbol() {
        let ir = compile_ir(
            "(+ 1 2) (- 3 1) (* 2 3) (quot 7 2) (< 1 2) (<= 1 1) (> 2 1) \
             (>= 2 2) (= 1 1) (count [1]) (nth [1] 0) (get [1] 0) (conj [1] 2)",
        );
        for symbol in [
            "rt_add", "rt_sub", "rt_mul", "rt_quot", "rt_lt", "rt_le", "rt_gt", "rt_ge", "rt_eq",
            "rt_count", "rt_nth", "rt_get", "rt_conj",
        ] {
            assert!(
                ir.contains(&format!("call ptr @{symbol}")),
                "{symbol}\n{ir}"
            );
        }
        assert!(ir.contains("call void @exit(i32 1)"), "{ir}");
        assert!(ir.contains("define i32 @main()"), "{ir}");
    }

    #[test]
    fn defn_and_call_lower() {
        let ir = compile_ir("(defn f [x] (+ x 1)) (print (f 41))");
        assert!(ir.contains("define ptr @cljfn_f_0"), "{ir}");
        assert!(ir.contains("call ptr @cljfn_f_0"), "{ir}");
        assert!(ir.contains("call ptr @rt_print"), "{ir}");
        assert!(ir.contains("call ptr @rt_clone"), "{ir}");
    }

    #[test]
    fn control_flow_and_literals_lower() {
        let ir = compile_ir("(let [x 1] (if (= x 1) [1 \"s\" 2.5 true nil] (do (print 0) 2)))");
        assert!(ir.contains("call i8 @rt_is_truthy"), "{ir}");
        assert!(ir.contains("phi ptr"), "{ir}");
        assert!(ir.contains("call ptr @rt_vec_new"), "{ir}");
        assert!(ir.contains("call ptr @rt_vec_push"), "{ir}");
        assert!(ir.contains("call ptr @rt_str"), "{ir}");
        assert!(ir.contains("call ptr @rt_double"), "{ir}");
        assert!(ir.contains("call ptr @rt_bool"), "{ir}");
        assert!(ir.contains("call ptr @rt_nil"), "{ir}");
    }

    #[test]
    fn arity_aborts_verify_in_every_branch_shape() {
        for source in [
            "(defn f [x] x) (if true (f 1 2) 3)",
            "(defn f [x] x) (if true 3 (f 1 2))",
            "(defn f [x] x) (if true (f 1 2) (f 1 2))",
            "(defn f [x] x) (if (f 1 2) 1 2)",
            "(defn f [x] x) (+ (f 1 2) 4)",
        ] {
            let ir = compile_ir(source);
            assert!(ir.contains("call ptr @rt_arity_error"), "{source}\n{ir}");
            assert!(ir.contains("unreachable"), "{source}\n{ir}");
        }
    }

    #[test]
    fn empty_forms_lower_to_nil() {
        let ir = compile_ir("(do)");
        assert!(ir.contains("call ptr @rt_nil"), "{ir}");
        let ir = compile_ir("(defn f [x] x)");
        assert!(ir.contains("define i32 @main()"), "{ir}");
        assert!(ir.contains("ret i32 0"), "{ir}");
    }

    #[test]
    fn lower_call_redispatches_defensively() {
        let ctx = Context::create();
        let mut lower = super::Lower::new(&ctx, "snap.clj");
        let ty = lower.ptr.fn_type(&[], false);
        let func = lower.module.add_function("scratch", ty, None);
        let entry = ctx.append_basic_block(func, "entry");
        lower.builder.position_at_end(entry);
        let out = lower.lower_call(func, &[], &SubsetExpr::Nil);
        lower.builder.build_return(Some(&out)).expect("return");
        lower.module.verify().expect("verify");
        let ir = lower.module.print_to_string().to_string();
        assert!(ir.contains("call ptr @rt_nil"), "{ir}");
    }

    #[test]
    fn self_calls_are_plain_calls() {
        let ir = compile_ir("(defn f [n] (if (= n 0) 0 (f 1))) (f 3)");
        assert!(ir.contains("call ptr @cljfn_f_0"), "{ir}");
        assert!(!ir.contains("tail call"), "{ir}");
    }
}
