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

use std::collections::{BTreeSet, HashMap, HashSet};

use clj_eval::aot_front::{FrontEnd, TopUnit, for_each_child, names_lib, pattern_children};
use clj_eval::expr::{Arity, CaseTest, Expr, FunExpr, NsClause, Pattern};
use clj_eval::native_abi;
use clj_value::{Map, Symbol, Value, Vector};
use inkwell::AddressSpace;
use inkwell::AtomicOrdering;
use inkwell::IntPredicate;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{
    ArrayType, BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, IntType, PointerType,
};
use inkwell::values::{BasicMetadataValueEnum, BasicValue, FunctionValue, IntValue, PointerValue};

#[path = "scalar.rs"]
mod scalar;

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
    // (`nil`, booleans, ints, doubles, and chars are written inline,
    // never pooled.)
    Str(String),
    Keyword(Option<String>, String),
    Symbol(Option<String>, String),
    Regex(String),
    StaticField(String, String),
    Class(String),
    Read(String),
    /// A symbol whose metadata maps keywords to simple values (`defn-`'s
    /// `^{:private true}` names), built without the reader (#271). Its
    /// metadata is self-contained: the init cannot read other pool
    /// constants before the pool freezes.
    MetaSymbol {
        ns: Option<String>,
        name: String,
        meta: Vec<(MetaVal, MetaVal)>,
    },
    /// A metadata-free collection of simple forms (quoted data, catch
    /// class lists, `:arglists`), built in place by the reader's own
    /// constructors (#271); `rt_n_coll`'s kind code.
    Coll(u64, Vec<ConstForm>),
    /// An empty collection literal without metadata (`[]`, `{}`, `#{}`):
    /// one value per form, as under `run` — built once by its literal
    /// shim, `site` keeping each form's value its own.
    EmptyColl {
        helper: &'static str,
        site: usize,
    },
}

/// A [`Const::MetaSymbol`] metadata key or value, built in place: a
/// scalar written inline, or a keyword, symbol, or string made by name.
#[derive(Clone, Debug, PartialEq)]
enum MetaVal {
    Inline(Value),
    Keyword(Option<String>, String),
    Symbol(Option<String>, String),
    Str(String),
}

/// One item of a [`Const::Coll`], built in place.
#[derive(Clone, Debug, PartialEq)]
enum ConstForm {
    Simple(MetaVal),
    MetaSym(Option<String>, String, Vec<(MetaVal, MetaVal)>),
    Coll(u64, Vec<ConstForm>),
}

/// How many forms a [`Const::Coll`] may hold (larger ones read back:
/// their text is the compact encoding).
const COLL_FORMS_MAX: usize = 256;

/// The [`MetaVal`] for `v` (`None`: not a simple metadata value).
fn meta_val(v: &Value) -> Option<MetaVal> {
    Some(match v {
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Double(_) | Value::Char(_) => {
            MetaVal::Inline(v.clone())
        }
        Value::Keyword(k) => {
            MetaVal::Keyword(k.namespace().map(str::to_owned), k.name().to_owned())
        }
        Value::Symbol(s) if s.meta().is_none() => {
            MetaVal::Symbol(s.namespace().map(str::to_owned), s.name().to_owned())
        }
        Value::Str(s) => MetaVal::Str(s.to_string()),
        _ => return None,
    })
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

/// A `recur` target: its loop header, `let_slots` depth, and the
/// patterns rebound from the recur values (each pattern's slots in
/// binding order).
struct RecurTarget<'ctx> {
    header: BasicBlock<'ctx>,
    let_depth: usize,
    patterns: Vec<(Pattern, Vec<PointerValue<'ctx>>)>,
}

/// How a lowered boolean/comparison operation delivers its `i1` result.
enum BoolSink<'ctx> {
    Slot {
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    },
    Jump {
        join: BasicBlock<'ctx>,
        phi: inkwell::values::PhiValue<'ctx>,
    },
}

type SlotArray<'ctx> = (PointerValue<'ctx>, Vec<PointerValue<'ctx>>);

/// Per-function lowering state.
struct FnCtx<'ctx> {
    func: FunctionValue<'ctx>,
    entry: BasicBlock<'ctx>,
    fail: BasicBlock<'ctx>,
    slots: Vec<PointerValue<'ctx>>,
    free_slots: Vec<PointerValue<'ctx>>,
    free_arrays: HashMap<usize, Vec<SlotArray<'ctx>>>,
    let_slots: Vec<PointerValue<'ctx>>,
    env: Vec<Option<LocalRef<'ctx>>>,
    recur: Vec<RecurTarget<'ctx>>,
    captured: PointerValue<'ctx>,
    /// `Expr::Local` occurrences `(node address, local)` in the `recur`
    /// args being lowered that are their local's last use, so may move.
    /// Such a local is never borrowed or aliased at an earlier
    /// occurrence (the move would invalidate the reference).
    moves: Vec<(usize, usize)>,
    /// Current inline nesting depth in this function (caps nested inlining).
    inline_depth: usize,
    /// Total inlined calls in this function (caps code-size growth).
    inlined_count: usize,
    /// Lowest env index eligible for mid-block dead-local drops in the
    /// active tail scope (`captures_end` for non-recurring arities, else
    /// the innermost non-tail `let` depth).
    tail_min: usize,
    /// Whether the expression currently being lowered is in tail position
    /// of the scope starting at `tail_min`.
    in_tail: bool,
    /// Un-taken symbol parameters (`env_idx -> args_idx`) that point into
    /// the caller's argument array and may be dropped early only when the
    /// caller passed ownership (`param_take_flag`).
    borrowed_params: HashMap<usize, usize>,
    /// Env indices of `let`-bound locals known to hold POD values (skip
    /// mid-block early drop checks).
    pod_locals: HashSet<usize>,
}

const INLINE_MAX_DEPTH: usize = 3;
const INLINE_MAX_PER_FN: usize = 24;
const INLINE_GEN_MAX_NODES: usize = 32;
const INLINE_DESTR_MAX_NODES: usize = 80;

enum InlineDest<'a, 'ctx> {
    Single(PointerValue<'ctx>),
    Destructure(&'a [PointerValue<'ctx>]),
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
    /// Whether `def`s keep their whole name metadata: always under the
    /// full boot; in a Lite program only when it references a reader of
    /// var metadata ([`program_reads_var_meta`]) — otherwise the
    /// runtime keeps no `def` metadata it could see (#271).
    var_meta: bool,
    /// Boot the compiled binary via `rt_n_main_lite` / `rt_n_ns_lite` (no
    /// embedded stdlib `core_lazy` evaluation) when the program uses only
    /// Lite-safe core/string natives and user-defined vars.
    lite: bool,
    /// Emit and link LLVM bitcode runtime helpers (`clj-bc`) instead
    /// of going straight to out-of-line `rt_n_*` shims.
    use_bitcode: bool,
    /// Global slot initialized on entry to `cljtop_0` holding `&FastVars`
    /// (`None` when `use_bitcode` is false).
    fast_vars: Option<PointerValue<'ctx>>,
    /// Top-level `defn`s reserved before lowering (by `FunExpr`
    /// address): fn index and pre-declared arity functions.
    reserved: HashMap<usize, (usize, Vec<FunctionValue<'ctx>>)>,
    /// Direct-call targets by var: fn index and `(fixed, variadic,
    /// code)` arities.
    direct: HashMap<(String, String), (usize, Vec<ArityCode<'ctx>>)>,
    /// Single-def top-level fn arities eligible for scalar-loop callee inlining.
    callees: HashMap<(String, String), std::sync::Arc<[Arity]>>,
    /// Global holding the frozen constant table's base
    /// (`rt_n_consts_base`), stored on entry to `cljtop_0`: call
    /// operands borrow pooled constants in place (#271).
    consts_base: PointerValue<'ctx>,
    /// A Lite program's stable vars ([`stable_vars`]) and their root
    /// caches: call operands borrow the cached root, other reads clone
    /// it, never going through the var (#271).
    stable: HashMap<(String, String), StableRoot<'ctx>>,
    /// Top-level units [`dead_defs`] drops (a Lite program's unread,
    /// pure `def`s).
    dead: HashSet<usize>,
    /// The index of the unit being lowered: code in a unit after a
    /// stable var's `def` runs only once that `def` has run, so it reads
    /// the cache without checking it is set.
    unit: usize,
}

/// A stable var's root cache: the unit of its one `def`, a module
/// global the `def` fills with the root right after it runs, and that
/// global's set flag (#271).
#[derive(Clone, Copy)]
struct StableRoot<'ctx> {
    unit: usize,
    root: PointerValue<'ctx>,
    set: PointerValue<'ctx>,
    kind: StableKind,
}

#[derive(Clone, Copy)]
enum StableKind {
    Unknown,
    Scalar(clj_eval::aot_scalar::Sv),
    Lookup,
}

impl<'ctx> Lowerer<'ctx> {
    /// A fresh session selecting bitcode helpers (`true`) vs out-of-line
    /// `rt_n_*` fallback (`false`).
    #[must_use]
    pub fn with_bitcode(ctx: &'ctx Context, use_bitcode: bool) -> Self {
        let ptr = ctx.ptr_type(AddressSpace::default());
        let i64t = ctx.i64_type();
        let i32t = ctx.i32_type();
        let code_ty = i32t.fn_type(&[ptr.into(), ptr.into(), i64t.into(), ptr.into()], false);
        let module = ctx.create_module("clj_native");
        let fast_vars = use_bitcode.then(|| {
            let g = module.add_global(ptr, None, "clj_fast_vars");
            g.set_initializer(&ptr.const_null());
            g.set_linkage(Linkage::Internal);
            g.as_pointer_value()
        });
        let consts_base = module.add_global(ptr, None, "clj_consts");
        consts_base.set_initializer(&ptr.const_null());
        consts_base.set_linkage(Linkage::Internal);
        Self {
            ctx,
            module,
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
            var_meta: true,
            lite: false,
            use_bitcode,
            fast_vars,
            reserved: HashMap::new(),
            direct: HashMap::new(),
            callees: HashMap::new(),
            consts_base: consts_base.as_pointer_value(),
            stable: HashMap::new(),
            dead: HashSet::new(),
            unit: 0,
        }
    }

    /// Lower a whole program to a verified module.
    pub fn lower_program(
        mut self,
        front: &FrontEnd,
        path: &str,
    ) -> Result<Module<'ctx>, Unsupported> {
        self.lite = can_lite_program(front);
        // A full boot loads the spliced libraries through the program's
        // own `require` (#271): their compiled units drop.
        let units = if self.lite {
            &front.units[..]
        } else {
            &front.units[front.lib_units..]
        };
        self.inline_arith = !units.iter().any(|u| mentions_rebinding(&u.expr));
        self.var_meta = !self.lite || program_reads_var_meta(units);
        let stable = self.lite.then(|| stable_vars(units));
        if let Some(candidates) = &stable {
            self.dead = dead_defs(units, candidates, self.var_meta);
        }
        self.reserve_defns(units);
        if let Some(candidates) = stable {
            self.reserve_stable(candidates);
        }
        let mut tops = Vec::with_capacity(units.len());
        for (ui, unit) in units.iter().enumerate() {
            if self.drops(&unit.expr) || self.dead.contains(&ui) {
                continue;
            }
            self.unit = ui;
            let i = tops.len();
            tops.push(self.lower_thunk(&format!("cljtop_{i}"), &unit.expr, i == 0)?);
        }
        let init = self.lower_const_init();
        self.lower_main(path, init, &tops);
        self.lower_frame_registry_wraps();
        crate::lower::verify_module(&self.module).map_err(Unsupported)?;
        Ok(self.module)
    }

    /// A root cache for each of the program's stable vars
    /// ([`stable_vars`]): an internal global holding `nil` until the var's
    /// `def` copies the root in, and its set flag. Globals are numbered in
    /// var order so the module text is reproducible.
    fn reserve_stable(&mut self, vars: HashMap<(String, String), (usize, StableKind)>) {
        let mut found: Vec<_> = vars
            .into_iter()
            .filter(|(_, (unit, _))| !self.dead.contains(unit))
            .collect();
        found.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (i, (key, (unit, kind))) in found.into_iter().enumerate() {
            let root = self
                .module
                .add_global(self.slot_ty, None, &format!("clj_root{i}"));
            root.set_initializer(&self.slot_ty.const_zero());
            root.set_linkage(Linkage::Internal);
            root.set_alignment(8);
            let set = self
                .module
                .add_global(self.i8t, None, &format!("clj_root_set{i}"));
            set.set_initializer(&self.i8t.const_zero());
            set.set_linkage(Linkage::Internal);
            self.stable.insert(
                key,
                StableRoot {
                    unit,
                    root: root.as_pointer_value(),
                    set: set.as_pointer_value(),
                    kind,
                },
            );
        }
    }

    /// Reserve every top-level `(def name (fn …))` whose var is defined
    /// exactly once: calls through such a var may go direct.
    fn reserve_defns(&mut self, units: &[TopUnit]) {
        let key = |sym: &Symbol| {
            (
                sym.namespace().unwrap_or("user").to_owned(),
                sym.name().to_owned(),
            )
        };
        let mut counts: HashMap<(String, String), usize> = HashMap::new();
        for u in units {
            if let Expr::Def { sym, .. } | Expr::DefMacro { sym, .. } = &u.expr {
                *counts.entry(key(sym)).or_default() += 1;
            }
            if let Expr::Declare { syms, .. } = &u.expr {
                for sym in syms {
                    *counts.entry(key(sym)).or_default() += 0;
                }
            }
        }
        for (ui, u) in units.iter().enumerate() {
            if self.dead.contains(&ui) {
                continue;
            }
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
            if self.inline_arith {
                self.callees
                    .insert(key(sym), std::sync::Arc::clone(&fun.arities));
            }
        }
    }

    // ---- helpers: declarations, constants, strings ---------------------

    fn helper(
        &self,
        name: &str,
        ret: Option<BasicTypeEnum<'ctx>>,
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

    fn call_raw(
        &self,
        name: &str,
        ret: Option<BasicTypeEnum<'ctx>>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> inkwell::values::CallSiteValue<'ctx> {
        let params: Vec<BasicMetadataTypeEnum> = args
            .iter()
            .map(|a| match a {
                BasicMetadataValueEnum::IntValue(v) => v.get_type().into(),
                BasicMetadataValueEnum::FloatValue(v) => v.get_type().into(),
                _ => self.ptr.into(),
            })
            .collect();
        let f = self.helper(name, ret, &params);
        self.builder.build_call(f, args, "").expect("call")
    }

    fn call_f64(
        &self,
        name: &str,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> inkwell::values::FloatValue<'ctx> {
        self.call_raw(name, Some(self.ctx.f64_type().into()), args)
            .try_as_basic_value()
            .expect_basic("f64")
            .into_float_value()
    }

    fn call(
        &self,
        name: &str,
        ret: Option<IntType<'ctx>>,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> Option<IntValue<'ctx>> {
        let site = self.call_raw(name, ret.map(Into::into), args);
        ret.map(|_| {
            site.try_as_basic_value()
                .expect_basic("status")
                .into_int_value()
        })
    }

    fn call_ptr(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) -> PointerValue<'ctx> {
        let site = self.call_raw(name, Some(self.ptr.into()), args);
        site.try_as_basic_value()
            .expect_basic("ptr")
            .into_pointer_value()
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
        let bytes = self.str_bytes(text);
        let p = self.const_slice(bytes, text.len() as u64);
        self.strings.insert(text.to_owned(), p);
        p
    }

    /// A private constant global holding `text`'s bytes (callers
    /// dedupe: [`str_lit`](Self::str_lit) caches its headers).
    fn str_bytes(&mut self, text: &str) -> PointerValue<'ctx> {
        let value = self.ctx.const_string(text.as_bytes(), false);
        self.const_global(&value, "cljs")
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
        let c = match scalar_const(v)
            .or_else(|| meta_symbol_const(v))
            .or_else(|| coll_const(v))
        {
            Some(c) => c,
            None => Const::Read(readable_text(v)?),
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
            free_slots: Vec::new(),
            free_arrays: HashMap::new(),
            let_slots: Vec::new(),
            env,
            recur: Vec::new(),
            captured,
            moves: Vec::new(),
            inline_depth: 0,
            inlined_count: 0,
            tail_min: usize::MAX,
            in_tail: false,
            borrowed_params: HashMap::new(),
            pod_locals: HashSet::new(),
        }
    }

    /// Finish a function whose result is in `result`: entry inits every
    /// slot to nil; the exit moves the result out and drops only live slots
    /// (skipping slots already nilled in `free_slots`/`free_arrays`/`result`),
    /// while `fail` calls `rt_n_drop` directly with `cold` to keep exception
    /// cleanup out of hot `.text` pages (#271 Slice 5r).
    fn end_fn(&mut self, f: FnCtx<'ctx>, result: PointerValue<'ctx>) -> FunctionValue<'ctx> {
        let ret = f.func.get_nth_param(0).expect("ret").into_pointer_value();
        self.move_v(f.func, ret, result);
        let mut nilled: HashSet<PointerValue<'ctx>> = f.free_slots.iter().copied().collect();
        nilled.insert(result);
        for pairs in f.free_arrays.values() {
            for (_, elems) in pairs {
                nilled.extend(elems.iter().copied());
            }
        }
        for &slot in &f.slots {
            if !nilled.contains(&slot) {
                self.drop_v(f.func, slot);
            }
        }
        self.builder
            .build_return(Some(&self.i32t.const_zero()))
            .expect("ret");
        self.builder.position_at_end(f.fail);
        let cold_attr = self.ctx.create_enum_attribute(
            inkwell::attributes::Attribute::get_named_enum_kind_id("cold"),
            0,
        );
        for &slot in &f.slots {
            self.call_raw("rt_n_drop", None, &[slot.into()])
                .add_attribute(inkwell::attributes::AttributeLoc::Function, cold_attr);
        }
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

    fn drop_and_nil_slots(&self, func: FunctionValue<'ctx>, slots: &[PointerValue<'ctx>]) {
        for slot in slots {
            self.drop_v(func, *slot);
            self.write_tag(*slot, 0);
        }
    }

    fn free_slots(&self, f: &mut FnCtx<'ctx>, slots: &[PointerValue<'ctx>]) {
        self.drop_and_nil_slots(f.func, slots);
        f.free_slots.extend_from_slice(slots);
    }

    fn free_slot_array(
        &self,
        f: &mut FnCtx<'ctx>,
        arr: PointerValue<'ctx>,
        elems: Vec<PointerValue<'ctx>>,
    ) {
        self.drop_and_nil_slots(f.func, &elems);
        f.free_arrays
            .entry(elems.len())
            .or_default()
            .push((arr, elems));
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
        let as_dbl = self.double_payload(slot);
        self.builder
            .build_select(is_int, as_int, as_dbl, "num")
            .expect("sel")
            .into_float_value()
    }

    fn double_payload(&self, slot: PointerValue<'ctx>) -> inkwell::values::FloatValue<'ctx> {
        self.builder
            .build_load(self.ctx.f64_type(), self.byte_at(slot, 8), "d")
            .expect("load")
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

    fn write_char(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>, cp: IntValue<'ctx>) {
        self.drop_v(func, dest);
        self.write_tag(dest, TAG_CHAR);
        self.builder
            .build_store(self.byte_at(dest, 4), cp)
            .expect("store");
    }

    fn is_numeric(&self, slot: PointerValue<'ctx>) -> IntValue<'ctx> {
        let t = self.tag(slot);
        let shifted = self
            .builder
            .build_int_sub(t, self.i8t.const_int(2, false), "t2")
            .expect("sub");
        self.builder
            .build_int_compare(
                IntPredicate::ULE,
                shifted,
                self.i8t.const_int(1, false),
                "num",
            )
            .expect("cmp")
    }

    /// Continue in a fresh block when every slot is `Int` (`2`) or `Double` (`3`),
    /// else branch to `slow`.
    fn when_numeric(&self, f: &FnCtx<'ctx>, elems: &[PointerValue<'ctx>], slow: BasicBlock<'ctx>) {
        let mut numeric = self.ctx.bool_type().const_int(1, false);
        for e in elems {
            let ok = self.is_numeric(*e);
            numeric = self.builder.build_and(numeric, ok, "nums").expect("and");
        }
        let go = self.ctx.append_basic_block(f.func, "num_fast");
        self.builder
            .build_conditional_branch(numeric, go, slow)
            .expect("br");
        self.builder.position_at_end(go);
    }

    /// `true` iff `a` (and `b` when present) hold `TAG_INT`.
    fn both_ints(&self, a: PointerValue<'ctx>, b: Option<PointerValue<'ctx>>) -> IntValue<'ctx> {
        let two = self.i8t.const_int(2, false);
        let a_int = self
            .builder
            .build_int_compare(IntPredicate::EQ, self.tag(a), two, "a_int")
            .expect("cmp");
        match b {
            Some(rhs) => {
                let b_int = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, self.tag(rhs), two, "b_int")
                    .expect("cmp");
                self.builder.build_and(a_int, b_int, "ints").expect("and")
            }
            None => a_int,
        }
    }

    /// `FastArith` over numeric (int/double) args when not all ints:
    /// computes in `f64` (at least one arg is a double), else `slow`.
    fn lower_float_operands(
        &self,
        f: &FnCtx<'ctx>,
        lhs: PointerValue<'ctx>,
        rhs: Option<PointerValue<'ctx>>,
        slow: BasicBlock<'ctx>,
    ) -> (
        inkwell::values::FloatValue<'ctx>,
        inkwell::values::FloatValue<'ctx>,
    ) {
        if let Some(r) = rhs {
            self.when_numeric(f, &[lhs, r], slow);
        } else {
            self.when_numeric(f, &[lhs], slow);
        }
        let x = self.num_as_f64(lhs);
        let one = self.ctx.f64_type().const_float(1.0);
        let y = match rhs {
            Some(r) => self.num_as_f64(r),
            None => one,
        };
        (x, y)
    }

    fn lower_double_cmp(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        sink: &BoolSink<'ctx>,
        slow: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        use inkwell::FloatPredicate;
        let (x, y) = self.lower_float_operands(f, lhs, Some(rhs), slow);
        let pred = match op {
            FastArith::Lt => FloatPredicate::OLT,
            FastArith::Lte => FloatPredicate::OLE,
            FastArith::Gt => FloatPredicate::OGT,
            _ => FloatPredicate::OGE,
        };
        let r = self
            .builder
            .build_float_compare(pred, x, y, "fc")
            .expect("fcmp");
        self.finish_bool(f.func, sink, r);
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_double_arith(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        lhs: PointerValue<'ctx>,
        rhs: Option<PointerValue<'ctx>>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        let (x, y) = self.lower_float_operands(f, lhs, rhs, slow);
        let b = &self.builder;
        let r = match op {
            FastArith::Add | FastArith::Inc => b.build_float_add(x, y, "fa"),
            FastArith::Sub | FastArith::Dec => b.build_float_sub(x, y, "fs"),
            FastArith::Mul => b.build_float_mul(x, y, "fm"),
            _ => b.build_float_div(x, y, "fd"),
        }
        .expect("fop");
        self.write_double(f.func, dest, r);
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

    /// `dest = clone(src)`: POD values copy inline, others try the
    /// bitcode helper (`rt_bc_clone`) when enabled, else call out.
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
        if self.use_bitcode {
            self.drop_v(func, dest);
            let ok = self
                .call("rt_bc_clone", Some(self.i8t), &[dest.into(), src.into()])
                .expect("ok");
            let slow = self.ctx.append_basic_block(func, "clone_slow");
            let hit = self
                .builder
                .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "ok")
                .expect("cmp");
            self.builder
                .build_conditional_branch(hit, done, slow)
                .expect("br");
            self.builder.position_at_end(slow);
        }
        self.call("rt_n_clone", None, &[dest.into(), src.into()]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    fn emit_deref(&self, func: FunctionValue<'ctx>, dest: PointerValue<'ctx>, k: u64) {
        let Some(gv) = self.fast_vars else {
            self.call("rt_n_deref", None, &[dest.into(), self.u64c(k)]);
            return;
        };
        let fv = self
            .builder
            .build_load(self.ptr, gv, "fv")
            .expect("load")
            .into_pointer_value();
        let non_null = self.builder.build_is_not_null(fv, "has_fv").expect("nn");
        let fast = self.ctx.append_basic_block(func, "deref_fast");
        let slow = self.ctx.append_basic_block(func, "deref_slow");
        let done = self.ctx.append_basic_block(func, "deref_done");
        self.builder
            .build_conditional_branch(non_null, fast, slow)
            .expect("br");
        self.builder.position_at_end(fast);
        self.drop_v(func, dest);
        let ok = self
            .call(
                "rt_bc_deref",
                Some(self.i8t),
                &[dest.into(), fv.into(), self.u64c(k)],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done, slow)
            .expect("br");
        self.builder.position_at_end(slow);
        self.call("rt_n_deref", None, &[dest.into(), self.u64c(k)]);
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
        if let Some(p) = f.free_slots.pop() {
            return p;
        }
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
        if let Some(pair) = f.free_arrays.get_mut(&n).and_then(Vec::pop) {
            return pair;
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

    fn lower_thunk(
        &mut self,
        name: &str,
        expr: &Expr,
        init_fast_vars: bool,
    ) -> Result<FunctionValue<'ctx>, Unsupported> {
        let mut f = self.begin_fn(name, Vec::new());
        if init_fast_vars {
            if let Some(gv) = self.fast_vars {
                let fv = self.call_ptr("rt_n_fast_vars", &[]);
                self.builder.build_store(gv, fv).expect("store");
            }
            let base = self.call_ptr("rt_n_consts_base", &[]);
            self.builder
                .build_store(self.consts_base, base)
                .expect("store");
        }
        f.moves = collect_body_moves(std::slice::from_ref(expr), 0, &f.env, &[]);
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
        let here = self.builder.get_insert_block().expect("positioned");
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
        self.builder.position_at_end(here);
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
        let captures_end = f.env.len();
        let owned = self_recurs(&arity.body);
        let n_param = f.func.get_nth_param(2).expect("n").into_int_value();
        let mut targets = Vec::with_capacity(pats.len());
        let mut sym_params = Vec::new();
        for (i, pat) in pats.iter().enumerate() {
            let arg = self.elem(args, i);
            let env_idx = f.env.len();
            let slots = self.bind_param(&mut f, pat, arg, n_param, i, owned)?;
            if !owned && matches!(pat, Pattern::Symbol(_)) {
                sym_params.push((env_idx, i));
            }
            targets.push(((*pat).clone(), slots));
        }
        let min_slot = if owned { captures_end } else { f.env.len() };
        let taken = if owned {
            (captures_end..f.env.len()).collect()
        } else {
            self.own_consumed_params(&mut f, &arity.body, captures_end, &sym_params, &mut targets)
        };
        let min_slot = if taken.is_empty() {
            min_slot
        } else {
            captures_end
        };
        f.moves = collect_body_moves(&arity.body, min_slot, &f.env, &taken);
        if !owned {
            for (env_idx, param_i) in sym_params {
                if !taken.contains(&env_idx) {
                    f.borrowed_params.insert(env_idx, param_i);
                }
            }
            f.tail_min = captures_end;
            f.in_tail = true;
        }
        let header = self.ctx.append_basic_block(f.func, "body");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget {
            header,
            let_depth: f.let_slots.len(),
            patterns: targets,
        });
        let result = self.slot(&mut f);
        self.lower_body(&mut f, &arity.body, result)?;
        Ok(self.end_fn(f, result))
    }

    /// Give each symbol param the body consumes (a last use it would move)
    /// an owned slot: taken from the caller's array when the call handed
    /// it over (`native_abi::TAKE_ARGS` or per-arg take bit in the count),
    /// cloned otherwise. Params the body never consumes keep reading the
    /// caller's array in place. Returns the owned params' env indices.
    fn own_consumed_params(
        &mut self,
        f: &mut FnCtx<'ctx>,
        body: &[Expr],
        captures_end: usize,
        sym_params: &[(usize, usize)],
        targets: &mut [(Pattern, Vec<PointerValue<'ctx>>)],
    ) -> Vec<usize> {
        let symbols: Vec<usize> = sym_params.iter().map(|&(local, _)| local).collect();
        let probe = collect_body_moves(body, captures_end, &f.env, &symbols);
        let n = f.func.get_nth_param(2).expect("n").into_int_value();
        let mut taken = Vec::new();
        for &(local, param) in sym_params {
            if !probe.iter().any(|m| m.1 == local) || borrow_only(body, local) {
                continue;
            }
            let take = self.param_take_flag(n, param);
            let arg = self.local_slot(f, local).expect("param slot");
            let s = self.slot(f);
            self.take_or_clone(f, s, arg, take);
            f.env[local] = Some(LocalRef::Slot(s));
            targets[param].1 = vec![s];
            taken.push(local);
        }
        taken
    }

    fn param_take_flag(&self, n: IntValue<'ctx>, param_idx: usize) -> IntValue<'ctx> {
        let mask = clj_eval::native_abi::take_arg_mask(param_idx);
        let bit = self
            .builder
            .build_and(n, self.i64t.const_int(mask, false), "take_bit")
            .expect("and");
        self.builder
            .build_int_compare(IntPredicate::NE, bit, self.i64t.const_zero(), "take")
            .expect("cmp")
    }

    /// `dest = take ? move(src) : clone(src)`.
    fn take_or_clone(
        &self,
        f: &FnCtx<'ctx>,
        dest: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        take: IntValue<'ctx>,
    ) {
        let take_bb = self.ctx.append_basic_block(f.func, "param_take");
        let clone_bb = self.ctx.append_basic_block(f.func, "param_clone");
        let join = self.ctx.append_basic_block(f.func, "param_ready");
        self.builder
            .build_conditional_branch(take, take_bb, clone_bb)
            .expect("br");
        self.builder.position_at_end(take_bb);
        self.move_v(f.func, dest, src);
        self.builder.build_unconditional_branch(join).expect("br");
        self.builder.position_at_end(clone_bb);
        self.clone_v(f.func, dest, src);
        self.builder.build_unconditional_branch(join).expect("br");
        self.builder.position_at_end(join);
    }

    // ---- patterns ----------------------------------------------------------

    /// Bind an arity parameter `pat` from `arg` (taking/cloning owned symbol
    /// params for `recur` arities). Destructured values have their own
    /// slots, so a caller-owned input can be released after binding.
    fn bind_param(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pat: &Pattern,
        arg: PointerValue<'ctx>,
        n_param: IntValue<'ctx>,
        i: usize,
        owned: bool,
    ) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
        let slots = match pat {
            Pattern::Symbol(_) => {
                let src = if owned {
                    let s = self.slot(f);
                    let take = self.param_take_flag(n_param, i);
                    self.take_or_clone(f, s, arg, take);
                    s
                } else {
                    arg
                };
                f.env.push(Some(LocalRef::Slot(src)));
                return Ok(vec![src]);
            }
            Pattern::Vec(v) => self.bind_vec(f, v, arg),
            Pattern::Map(m) => self.bind_map(f, m, pat, arg),
        };
        slots.inspect(|_| self.drop_taken_arg(f, arg, n_param, i))
    }

    /// Bind `pat` taking ownership of `slot`: a symbol adopts `slot`
    /// directly; a destructuring pattern binds out of `slot` and drops it.
    fn bind_owned_slot(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pat: &Pattern,
        slot: PointerValue<'ctx>,
        out: &mut Vec<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        let bound = match pat {
            Pattern::Symbol(_) => {
                f.env.push(Some(LocalRef::Slot(slot)));
                out.push(slot);
                return Ok(());
            }
            Pattern::Vec(v) => self.bind_vec(f, v, slot)?,
            Pattern::Map(m) => self.bind_map(f, m, pat, slot)?,
        };
        out.extend(bound);
        self.free_slots(f, &[slot]);
        Ok(())
    }

    /// Bind a vector destructuring pattern `v` to `src`.
    fn bind_vec(
        &mut self,
        f: &mut FnCtx<'ctx>,
        v: &clj_eval::expr::VecPattern,
        src: PointerValue<'ctx>,
    ) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
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
                self.bind_owned_slot(f, fixed, item, &mut out)?;
                let st = self
                    .call("rt_n_next_mut", Some(self.i32t), &[cur.into()])
                    .expect("st");
                self.check(f, st);
            }
            self.bind_owned_slot(f, rest, cur, &mut out)?;
        } else {
            for (i, fixed) in v.fixed.iter().enumerate() {
                let item = self.slot(f);
                self.emit_nth_nil(f, item, src, i);
                self.bind_owned_slot(f, fixed, item, &mut out)?;
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

    /// A map pattern: a [`plain_map`] binds by direct lookups, any other
    /// through the interpreter's own destructure.
    fn bind_map(
        &mut self,
        f: &mut FnCtx<'ctx>,
        m: &clj_eval::expr::MapPattern,
        pat: &Pattern,
        src: PointerValue<'ctx>,
    ) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
        if plain_map(m) {
            return self.bind_map_plain(f, m, src);
        }
        // The interpreter's own destructure over the encoded pattern,
        // `:or` defaults evaluated here first (eager, in the enclosing
        // frame, like the interpreter).
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

    /// A [`plain_map`] pattern bound by direct lookups (#271), as the
    /// interpreter binds it: the source converted (a list becomes its
    /// map), every `:or` default evaluated once in the enclosing frame,
    /// then each entry and `:keys`/`:strs`/`:syms` local read from the
    /// converted source (present keys only), else its default, else nil;
    /// `:as` binds the converted source. Locals follow `pattern_names`
    /// order. The generic path decoded the pattern and ran the whole
    /// interpreter destructure on every bind (~7.8K macOS instructions
    /// for two keys).
    fn bind_map_plain(
        &mut self,
        f: &mut FnCtx<'ctx>,
        m: &clj_eval::expr::MapPattern,
        src: PointerValue<'ctx>,
    ) -> Result<Vec<PointerValue<'ctx>>, Unsupported> {
        let conv = self.slot(f);
        let st = self
            .call(
                "rt_n_destr_map",
                Some(self.i32t),
                &[conv.into(), src.into()],
            )
            .expect("st");
        self.check(f, st);
        let mut defaults = Vec::with_capacity(m.or.len());
        for (_, default) in &m.or {
            let d = self.slot(f);
            self.lower(f, default, d)?;
            defaults.push(d);
        }
        let mut out = Vec::new();
        for (local, key, or_key, _) in &m.entries {
            let name = match local {
                Pattern::Symbol(name) => Some(Value::Symbol(Symbol::new(None, name))),
                _ => None,
            };
            let default = or_default(m, name.as_ref(), or_key).map(|i| defaults[i]);
            let item = self.slot(f);
            self.emit_destr_get(item, conv, key, default)?;
            self.bind_owned_slot(f, local, item, &mut out)?;
        }
        for binding in &m.keys {
            let name = Value::Symbol(Symbol::new(None, &binding.local));
            let default = or_default(m, Some(&name), &binding.key).map(|i| defaults[i]);
            let item = self.slot(f);
            self.emit_destr_get(item, conv, &binding.key, default)?;
            f.env.push(Some(LocalRef::Slot(item)));
            out.push(item);
        }
        self.free_slots(f, &defaults);
        if m.as_sym.is_some() {
            f.env.push(Some(LocalRef::Slot(conv)));
            out.push(conv);
        } else {
            self.free_slots(f, &[conv]);
        }
        Ok(out)
    }

    /// `item =` the binding of `key` in the converted source `conv`
    /// (`rt_n_destr_get`; `default` is the evaluated `:or` slot, if any).
    fn emit_destr_get(
        &mut self,
        item: PointerValue<'ctx>,
        conv: PointerValue<'ctx>,
        key: &Value,
        default: Option<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        let k = self.value_const(key)?;
        let key_ref = self.const_ref(k);
        let default = default.unwrap_or_else(|| self.ptr.const_null());
        self.call(
            "rt_n_destr_get",
            None,
            &[item.into(), conv.into(), key_ref.into(), default.into()],
        );
        Ok(())
    }

    /// `item = (nth src i nil)` for sequential destructuring (#271): the
    /// inlined bitcode `nth` (vectors, strings, nil), falling back to
    /// `rt_n_nth_nil` for every shape it leaves to the runtime.
    fn emit_nth_nil(
        &mut self,
        f: &mut FnCtx<'ctx>,
        item: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        i: usize,
    ) {
        let done = self.ctx.append_basic_block(f.func, "nth_done");
        if self.use_bitcode {
            let idx = self.slot(f);
            self.write_int(f.func, idx, self.i64t.const_int(i as u64, false));
            let not_found = self.slot(f);
            let bc = self.ctx.append_basic_block(f.func, "nth_bc");
            let slow = self.ctx.append_basic_block(f.func, "nth_slow");
            self.builder.build_unconditional_branch(bc).expect("br");
            self.emit_bc_op(
                f.func,
                bc,
                "rt_bc_3_nth",
                &[src, idx, not_found],
                item,
                slow,
                done,
            );
            self.builder.position_at_end(slow);
        }
        let st = self
            .call(
                "rt_n_nth_nil",
                Some(self.i32t),
                &[item.into(), src.into(), self.u64c(i as u64)],
            )
            .expect("st");
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    /// Rebind `pat`'s existing slots from `src` (for `recur`).
    fn rebind_pattern(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pat: &Pattern,
        src: PointerValue<'ctx>,
        slots: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let fresh = match pat {
            Pattern::Symbol(_) => {
                self.move_v(f.func, slots[0], src);
                return Ok(());
            }
            Pattern::Vec(v) => self.bind_vec(f, v, src)?,
            Pattern::Map(m) => self.bind_map(f, m, pat, src)?,
        };
        f.env.truncate(depth);
        for (dst, s) in slots.iter().zip(fresh) {
            self.move_v(f.func, *dst, s);
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
        let in_tail = std::mem::replace(&mut f.in_tail, false);
        let Some((last, init)) = body.split_last() else {
            self.write_nil(f.func, dest);
            return Ok(());
        };
        for (i, e) in init.iter().enumerate() {
            if self.drops(e) {
                continue;
            }
            self.lower_discard(f, e)?;
            if in_tail {
                self.drop_dead_after_stmt(f, e, &body[i + 1..]);
            }
        }
        f.in_tail = in_tail;
        self.lower(f, last, dest)
    }

    fn drop_dead_local(&self, f: &FnCtx<'ctx>, idx: usize) {
        if f.pod_locals.contains(&idx) {
            return;
        }
        let slot = self.local_slot(f, idx).expect("tail-scope local slot");
        if let Some(&param_i) = f.borrowed_params.get(&idx) {
            let n_param = f.func.get_nth_param(2).expect("n").into_int_value();
            self.drop_taken_arg(f, slot, n_param, param_i);
        } else {
            self.drop_and_nil_slots(f.func, &[slot]);
        }
    }

    fn drop_taken_arg(
        &self,
        f: &FnCtx<'ctx>,
        slot: PointerValue<'ctx>,
        n_param: IntValue<'ctx>,
        param: usize,
    ) {
        let take = self.param_take_flag(n_param, param);
        let drop_b = self.ctx.append_basic_block(f.func, "param_drop");
        let cont_b = self.ctx.append_basic_block(f.func, "param_cont");
        self.builder
            .build_conditional_branch(take, drop_b, cont_b)
            .expect("br");
        self.builder.position_at_end(drop_b);
        self.drop_and_nil_slots(f.func, &[slot]);
        self.builder.build_unconditional_branch(cont_b).expect("br");
        self.builder.position_at_end(cont_b);
    }

    fn drop_dead_after_stmt(&self, f: &FnCtx<'ctx>, stmt: &Expr, rest: &[Expr]) {
        if !expr_has_loop(stmt) {
            return;
        }
        for idx in f.tail_min..f.env.len() {
            if expr_reads_local(stmt, idx)
                && !expr_moves_local(stmt, idx, &f.moves)
                && !rest.iter().any(|r| expr_reads_local(r, idx))
            {
                self.drop_dead_local(f, idx);
            }
        }
    }

    fn drop_dead_after_binding(
        &self,
        f: &FnCtx<'ctx>,
        env_before: usize,
        pat: &Pattern,
        init: &Expr,
        rest_bindings: &[(Pattern, Expr)],
        body: &[Expr],
    ) {
        if !expr_has_loop(init) {
            return;
        }
        for idx in f.tail_min..env_before {
            let read_here = expr_reads_local(init, idx) || pat_reads_local(pat, idx);
            if read_here
                && !expr_moves_local(init, idx, &f.moves)
                && !rest_bindings
                    .iter()
                    .any(|(p, e)| expr_reads_local(e, idx) || pat_reads_local(p, idx))
                && !body.iter().any(|e| expr_reads_local(e, idx))
            {
                self.drop_dead_local(f, idx);
            }
        }
    }

    /// Whether a discarded statement can go unlowered: a value with no
    /// effect (`defn`'s trailing `(var name)`, a literal, a quote), or an
    /// inert metadata write ([`is_inert_meta_statement`]) in a program
    /// that never reads var metadata.
    fn drops(&self, e: &Expr) -> bool {
        drops_unit(e, self.var_meta)
    }

    fn bc_fx_call<'a>(
        &self,
        expr: &'a Expr,
    ) -> Option<(&'a clj_eval::expr::DirectCall, &'static str)> {
        let Expr::DirectNative(call) = expr else {
            return None;
        };
        (self.inline_arith && self.use_bitcode && call.var.sym.namespace() == Some("clojure.core"))
            .then(|| {
                clj_bc::bc_fx_symbol(call.var.sym.name(), call.args.len()).map(|s| (&**call, s))
            })
            .flatten()
    }

    fn lower_discard(&mut self, f: &mut FnCtx<'ctx>, expr: &Expr) -> Result<(), Unsupported> {
        if let Expr::DirectNative(call) = expr {
            if self.try_lower_assoc_conj(f, call, None)? {
                return Ok(());
            }
            if self.try_lower_conj_bang_fx(f, call)? {
                return Ok(());
            }
            if self.try_lower_swap_fx(f, call)? {
                return Ok(());
            }
        }
        let t = self.slot(f);
        if let Some((call, sym)) = self.bc_fx_call(expr) {
            self.lower_direct_with(f, call, Some(sym), t)?;
        } else {
            self.lower(f, expr, t)?;
        }
        self.free_slots(f, &[t]);
        Ok(())
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
        expr: &Expr,
        idx: usize,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        // The analyzer resolves every local it emits (captures included).
        let local = f.env[idx].expect("analyzed local is visible");
        let p = self.local_slot(f, idx).expect("analyzed local is visible");
        if matches!(local, LocalRef::Cell(_) | LocalRef::CapturedCell(_)) {
            self.call("rt_n_cell_get", None, &[dest.into(), p.into()]);
        } else if is_move(f, expr) && matches!(local, LocalRef::Slot(_)) {
            self.move_v(f.func, dest, p);
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
        let in_tail = std::mem::replace(&mut f.in_tail, false);
        match expr {
            Expr::Literal(v) | Expr::Quote(v) => self.lower_const(f.func, v, dest),
            Expr::Local(i) => self.lower_local(f, expr, *i, dest),
            Expr::Global(var) => {
                let k = self.var_of(var);
                match self.stable_of(var) {
                    Some(sr) => self.clone_stable(f, sr, k, dest),
                    None => self.emit_deref(f.func, dest, k),
                }
                Ok(())
            }
            Expr::VarRef(var) => {
                let k = self.var_of(var);
                self.call("rt_n_varref", None, &[dest.into(), self.u64c(k)]);
                Ok(())
            }
            Expr::If { test, then, els } => self.lower_if(f, test, then, els, in_tail, dest),
            Expr::Do(body) => {
                f.in_tail = in_tail;
                self.lower_body(f, body, dest)
            }
            Expr::Let { bindings, body } => self.lower_let(f, bindings, body, in_tail, dest),
            Expr::Loop { bindings, body } => self.lower_loop(f, bindings, body, dest),
            Expr::Recur(args) => self.lower_recur(f, args),
            Expr::Invoke { head, args, .. } => self.lower_invoke(f, head, args, dest),
            Expr::DirectNative(call) => self.lower_direct(f, call, dest),
            other => self.lower_form(f, other, dest),
        }
    }

    /// Lower literals/collections, fns, defs, and control-flow forms
    /// off the hot scalar/call path.
    fn lower_form(
        &mut self,
        f: &mut FnCtx<'ctx>,
        expr: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match expr {
            Expr::VecLit(items, meta) => {
                self.lower_coll(f, "rt_n_vec_lit", items, meta.as_ref(), dest, false)
            }
            Expr::SetLit(items, meta) => {
                self.lower_coll(f, "rt_n_set_lit", items, meta.as_ref(), dest, true)
            }
            Expr::MapLit(pairs, meta) => self.lower_map_lit(f, pairs, meta.as_ref(), dest),
            Expr::Fn(fun) => {
                let base = f.env.len();
                self.make_closure(f, fun, base, dest)
            }
            Expr::LetFn { fns, body } => self.lower_letfn(f, fns, body, dest),
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
            Expr::Throw(e) => self.lower_throw(f, e, dest),
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
            Expr::Set { var, val } => self.lower_set(f, var, val, dest),
            Expr::CaseStar {
                expr,
                shift,
                mask,
                default,
                cases,
                skip,
                test,
            } => self.lower_case(f, expr, *shift, *mask, default, cases, skip, *test, dest),
            other => self.lower_scope_form(f, other, dest),
        }
    }

    fn lower_throw(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let _ = dest;
        let t = self.slot(f);
        self.lower(f, e, t)?;
        self.call("rt_n_throw", Some(self.i32t), &[t.into()]);
        self.builder.build_unconditional_branch(f.fail).expect("br");
        self.dead_end(f);
        Ok(())
    }

    fn lower_set(
        &mut self,
        f: &mut FnCtx<'ctx>,
        var: &clj_value::Var,
        val: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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

    /// Lower dynamic-binding, namespace, and runtime-wrapper forms.
    fn lower_scope_form(
        &mut self,
        f: &mut FnCtx<'ctx>,
        expr: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match expr {
            Expr::Binding { pairs, body } => self.lower_binding(f, pairs, body, dest),
            Expr::Declare { syms, span } => self.lower_declare(syms, *span, dest),
            Expr::Ns {
                name,
                doc,
                attr,
                clauses,
            } => self.lower_ns(f, name, doc.as_deref(), attr.as_ref(), clauses, dest),
            Expr::WithRedefs { pairs, body } => self.lower_with_redefs(f, pairs, body, dest),
            Expr::WithBindings { map, body } => self.lower_with_bindings(f, map, body, dest),
            Expr::Time { expr } => {
                let b = self.slot(f);
                self.body_closure(f, 0, std::slice::from_ref(expr.as_ref()), b)?;
                let st = self
                    .call("rt_n_time", Some(self.i32t), &[dest.into(), b.into()])
                    .expect("st");
                self.check(f, st);
                Ok(())
            }
            Expr::WithOutStr { out, body } => self.lower_with_out_str(f, out, body, dest),
            Expr::WithInStr { input, s, body } => self.lower_with_in_str(f, input, s, body, dest),
            Expr::Locking { lockee, body } => self.lower_locking(f, lockee, body, dest),
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
            other => self.lower_tail_form(f, other, dest),
        }
    }

    fn lower_declare(
        &mut self,
        syms: &[Symbol],
        span: clj_reader::Span,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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

    fn lower_ns(
        &mut self,
        f: &mut FnCtx<'ctx>,
        name: &str,
        doc: Option<&str>,
        attr: Option<&Map>,
        clauses: &[NsClause],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if self.lite {
            let np = self.str_ptr(name);
            self.call("rt_n_ns_lite", None, &[dest.into(), np.into()]);
            return Ok(());
        }
        let v = ns_desc(name, doc, attr, clauses);
        let k = self.value_const(&v)?;
        let st = self
            .call("rt_n_ns", Some(self.i32t), &[dest.into(), self.u64c(k)])
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    fn lower_with_redefs(
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
                "rt_n_with_redefs",
                Some(self.i32t),
                &[dest.into(), karr.into(), vref.into(), b.into()],
            )
            .expect("st");
        self.check(f, st);
        Ok(())
    }

    fn lower_with_bindings(
        &mut self,
        f: &mut FnCtx<'ctx>,
        map: &Expr,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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

    fn lower_with_out_str(
        &mut self,
        f: &mut FnCtx<'ctx>,
        out: &Symbol,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        // The Lite boot installs the stream var a program declares.
        self.var("clojure.core", "*out*");
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

    fn lower_with_in_str(
        &mut self,
        f: &mut FnCtx<'ctx>,
        input: &Symbol,
        s: &Expr,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        self.var("clojure.core", "*in*");
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

    fn lower_locking(
        &mut self,
        f: &mut FnCtx<'ctx>,
        lockee: &Expr,
        body: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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

    /// Lower lazy-seq, delay, bound-fn, defmacro, and assert forms.
    fn lower_tail_form(
        &mut self,
        f: &mut FnCtx<'ctx>,
        expr: &Expr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match expr {
            Expr::LazySeqStar { body } => self.wrap_body(f, 0, "clojure.core/fn", body, dest),
            Expr::DelayStar { display, body } => self.wrap_body(f, 1, display, body, dest),
            Expr::BoundFn { name, fun } => self.lower_bound_fn(f, name.as_deref(), fun, dest),
            Expr::DefMacro {
                sym,
                doc,
                attr,
                arglists,
                fun,
                span,
            } => self.lower_defmacro(
                f,
                sym,
                doc.as_deref(),
                attr.as_ref(),
                arglists,
                fun,
                *span,
                dest,
            ),
            Expr::Assert {
                test,
                message,
                form,
            } => self.lower_assert(f, test, message.as_deref(), form, dest),
            _ => unreachable!("lower_tail_form partition"),
        }
    }

    fn lower_bound_fn(
        &mut self,
        f: &mut FnCtx<'ctx>,
        name: Option<&str>,
        fun: &FunExpr,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let cell = name.map(|_| {
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

    #[allow(clippy::too_many_arguments)]
    fn lower_defmacro(
        &mut self,
        f: &mut FnCtx<'ctx>,
        sym: &Symbol,
        doc: Option<&str>,
        attr: Option<&Map>,
        arglists: &[Value],
        fun: &FunExpr,
        span: clj_reader::Span,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let desc = Value::Vector(Vector::from_vec(vec![
            Value::Symbol(sym.clone()),
            doc.map_or(Value::Nil, |d| Value::Str(std::sync::Arc::from(d))),
            attr.cloned().map_or(Value::Nil, Value::Map),
            Value::Vector(Vector::from_vec(arglists.to_vec())),
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

    fn lower_assert(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        message: Option<&Expr>,
        form: &str,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
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
        let fk = self.constant(Const::Str(form.to_owned()));
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

    /// A `let`: each init lowered into its slot; a symbol binding
    /// takes over the init's slot (no copy), while a destructuring
    /// pattern binds out of it. Bound values release at scope end.
    fn lower_let(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        in_tail: bool,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let let_depth = f.let_slots.len();
        let saved_tail_min = f.tail_min;
        if !in_tail {
            f.tail_min = depth;
        }
        let mut bound = Vec::new();
        self.lower_let_bindings(f, bindings, body, &mut bound)?;
        f.let_slots.extend_from_slice(&bound);
        f.in_tail = true;
        self.lower_body(f, body, dest)?;
        f.tail_min = saved_tail_min;
        f.let_slots.truncate(let_depth);
        f.env.truncate(depth);
        f.pod_locals.retain(|&i| i < depth);
        // Release let-bound values at scope end (RSS parity).
        self.free_slots(f, &bound);
        Ok(())
    }

    fn lower_let_bindings(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        bound: &mut Vec<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        for (i, (pat, init)) in bindings.iter().enumerate() {
            let env_before = f.env.len();
            if let Pattern::Vec(vp) = pat
                && vp.rest.is_none()
                && vp.as_sym.is_none()
                && !vp.fixed.is_empty()
                && self.has_destr_vec(f, init, vp.fixed.len(), 0)
            {
                let dests: Vec<PointerValue<'ctx>> =
                    (0..vp.fixed.len()).map(|_| self.slot(f)).collect();
                self.lower_vec_destructure(f, init, &dests)?;
                for (sub, d) in vp.fixed.iter().zip(dests) {
                    self.bind_owned_slot(f, sub, d, bound)?;
                }
            } else {
                let t = self.slot(f);
                self.lower(f, init, t)?;
                self.bind_owned_slot(f, pat, t, bound)?;
                if matches!(pat, Pattern::Symbol(_)) && is_pod_expr(init) {
                    f.pod_locals.insert(env_before);
                }
            }
            self.drop_dead_after_binding(f, env_before, pat, init, &bindings[i + 1..], body);
        }
        Ok(())
    }

    fn has_destr_vec(&self, f: &FnCtx<'ctx>, e: &Expr, n: usize, depth: usize) -> bool {
        match e {
            Expr::VecLit(elems, None) => elems.len() == n,
            Expr::If { then, els, .. } => {
                self.has_destr_vec(f, then, n, depth) || self.has_destr_vec(f, els, n, depth)
            }
            Expr::Let { body, .. } | Expr::Do(body) => body
                .last()
                .is_some_and(|last| self.has_destr_vec(f, last, n, depth)),
            Expr::Loop { body, .. } => body
                .last()
                .is_some_and(|last| self.has_destr_vec(f, last, n, depth)),
            Expr::Invoke { head, args, .. } if f.inline_depth + depth < INLINE_MAX_DEPTH => self
                .inline_callee(f, head, args.len(), INLINE_DESTR_MAX_NODES)
                .is_some_and(|arity| {
                    arity
                        .body
                        .last()
                        .is_some_and(|last| self.has_destr_vec(f, last, n, depth + 1))
                }),
            _ => false,
        }
    }

    fn lower_and_unpack(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let t = self.slot(f);
        self.lower(f, e, t)?;
        for (i, &d) in dests.iter().enumerate() {
            self.emit_nth_nil(f, d, t, i);
        }
        self.free_slots(f, &[t]);
        Ok(())
    }

    fn lower_body_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        body: &[Expr],
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let in_tail = std::mem::replace(&mut f.in_tail, false);
        let (last, rest) = body.split_last().expect("non-empty body");
        for (i, e) in rest.iter().enumerate() {
            let t = self.slot(f);
            self.lower(f, e, t)?;
            self.free_slots(f, &[t]);
            if in_tail {
                self.drop_dead_after_stmt(f, e, &body[i + 1..]);
            }
        }
        f.in_tail = in_tail;
        self.lower_vec_destructure(f, last, dests)
    }

    fn lower_vec_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let in_tail = std::mem::replace(&mut f.in_tail, false);
        match e {
            Expr::VecLit(elems, None) if elems.len() == dests.len() => {
                for (elem, &d) in elems.iter().zip(dests) {
                    self.lower(f, elem, d)?;
                }
                Ok(())
            }
            Expr::If { test, then, els } => {
                self.lower_if_destructure(f, test, then, els, in_tail, dests)
            }
            Expr::Let { bindings, body } if !body.is_empty() => {
                self.lower_let_destructure(f, bindings, body, in_tail, dests)
            }
            Expr::Do(forms) if !forms.is_empty() => {
                f.in_tail = in_tail;
                self.lower_body_destructure(f, forms, dests)
            }
            Expr::Loop { bindings, body } if !body.is_empty() => {
                self.lower_loop_destructure(f, bindings, body, dests)
            }
            Expr::Recur(args) => self.lower_recur(f, args),
            Expr::Invoke { head, args, .. } => {
                self.lower_invoke_destructure(f, e, head, args, dests)
            }
            _ => self.lower_and_unpack(f, e, dests),
        }
    }

    fn lower_invoke_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
        head: &Expr,
        args: &[Expr],
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        if let Some(arity) = self.inline_callee(f, head, args.len(), INLINE_DESTR_MAX_NODES)
            && arity
                .body
                .last()
                .is_some_and(|last| self.has_destr_vec(f, last, dests.len(), 1))
        {
            return self.inline_callee_into(f, &arity, args, InlineDest::Destructure(dests));
        }
        self.lower_and_unpack(f, e, dests)
    }

    fn lower_if_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        in_tail: bool,
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let then_b = self.ctx.append_basic_block(f.func, "then");
        let else_b = self.ctx.append_basic_block(f.func, "else");
        let merge = self.ctx.append_basic_block(f.func, "merge");
        self.lower_cond(f, test, then_b, else_b)?;
        self.builder.position_at_end(then_b);
        f.in_tail = in_tail;
        self.lower_vec_destructure(f, then, dests)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(else_b);
        f.in_tail = in_tail;
        self.lower_vec_destructure(f, els, dests)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(merge);
        Ok(())
    }

    fn lower_let_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        in_tail: bool,
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let let_depth = f.let_slots.len();
        let saved_tail_min = f.tail_min;
        if !in_tail {
            f.tail_min = depth;
        }
        let mut bound = Vec::new();
        self.lower_let_bindings(f, bindings, body, &mut bound)?;
        f.let_slots.extend_from_slice(&bound);
        f.in_tail = true;
        self.lower_body_destructure(f, body, dests)?;
        f.tail_min = saved_tail_min;
        f.let_slots.truncate(let_depth);
        f.env.truncate(depth);
        f.pod_locals.retain(|&i| i < depth);
        self.free_slots(f, &bound);
        Ok(())
    }

    fn lower_loop_destructure(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        dests: &[PointerValue<'ctx>],
    ) -> Result<(), Unsupported> {
        let t = self.slot(f);
        if self.try_lower_scalar_loop(f, bindings, body, t)? {
            for (i, &d) in dests.iter().enumerate() {
                self.emit_nth_nil(f, d, t, i);
            }
            self.free_slots(f, &[t]);
            return Ok(());
        }
        self.free_slots(f, &[t]);
        let depth = f.env.len();
        let mut bound = Vec::new();
        let mut targets = Vec::new();
        for (pat, init) in bindings {
            let s = self.slot(f);
            self.lower(f, init, s)?;
            let before = bound.len();
            self.bind_owned_slot(f, pat, s, &mut bound)?;
            targets.push((pat.clone(), bound[before..].to_vec()));
        }
        let header = self.ctx.append_basic_block(f.func, "loop");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget {
            header,
            let_depth: f.let_slots.len(),
            patterns: targets,
        });
        let result = self.lower_body_destructure(f, body, dests);
        f.recur.pop();
        f.env.truncate(depth);
        result?;
        self.free_slots(f, &bound);
        Ok(())
    }

    /// Look up a small, non-recursive stable `defn` arity eligible to inline
    /// into the current function (#271 Slice 5p).
    fn inline_callee(
        &self,
        f: &FnCtx<'ctx>,
        head: &Expr,
        argc: usize,
        max_nodes: usize,
    ) -> Option<Arity> {
        if !self.inline_arith
            || f.inline_depth >= INLINE_MAX_DEPTH
            || f.inlined_count >= INLINE_MAX_PER_FN
        {
            return None;
        }
        let Expr::Global(var) = head else { return None };
        let sr = self.stable_of(var)?;
        if self.unit < sr.unit {
            return None;
        }
        let key = (
            var.sym.namespace().unwrap_or("user").to_owned(),
            var.sym.name().to_owned(),
        );
        let arities = self.callees.get(&key)?;
        let arity = arities
            .iter()
            .find(|a| a.rest.is_none() && a.fixed.len() == argc)?;
        if self_recurs(&arity.body) || !self.inline_pats_ok(&arity.fixed) {
            return None;
        }
        let mut budget = max_nodes;
        if !self.inline_exprs_ok(&arity.body, sr.unit, &mut budget) {
            return None;
        }
        Some(arity.clone())
    }

    fn inline_pats_ok(&self, pats: &[Pattern]) -> bool {
        pats.iter().all(|p| match p {
            Pattern::Symbol(_) => true,
            Pattern::Vec(vp) => self.inline_pats_ok(&vp.fixed),
            Pattern::Map(mp) => mp.or.is_empty(),
        })
    }

    fn inline_exprs_ok(&self, exprs: &[Expr], callee_unit: usize, budget: &mut usize) -> bool {
        exprs
            .iter()
            .all(|e| self.inline_expr_ok(e, callee_unit, budget))
    }

    fn inline_binds_ok(
        &self,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        callee_unit: usize,
        budget: &mut usize,
    ) -> bool {
        bindings.iter().all(|(pat, init)| {
            self.inline_pats_ok(std::slice::from_ref(pat))
                && self.inline_expr_ok(init, callee_unit, budget)
        }) && self.inline_exprs_ok(body, callee_unit, budget)
    }

    fn inline_expr_ok(&self, e: &Expr, callee_unit: usize, budget: &mut usize) -> bool {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        match e {
            Expr::Literal(_) | Expr::Local(_) | Expr::Quote(_) => true,
            Expr::Global(var) => {
                let key = (
                    var.sym.namespace().unwrap_or("user").to_owned(),
                    var.sym.name().to_owned(),
                );
                !self.direct.contains_key(&key)
                    || self.stable_of(var).is_some_and(|sr| sr.unit < callee_unit)
            }
            Expr::Do(xs) | Expr::Recur(xs) => self.inline_exprs_ok(xs, callee_unit, budget),
            Expr::DirectNative(call) => self.inline_exprs_ok(&call.args, callee_unit, budget),
            Expr::Invoke { head, args, .. } => {
                self.inline_expr_ok(head, callee_unit, budget)
                    && self.inline_exprs_ok(args, callee_unit, budget)
            }
            _ => self.inline_compound_ok(e, callee_unit, budget),
        }
    }

    fn inline_compound_ok(&self, e: &Expr, callee_unit: usize, budget: &mut usize) -> bool {
        match e {
            Expr::VecLit(xs, m) | Expr::SetLit(xs, m) => {
                !xs.is_empty() && m.is_none() && self.inline_exprs_ok(xs, callee_unit, budget)
            }
            Expr::MapLit(ps, m) => {
                !ps.is_empty()
                    && m.is_none()
                    && ps.iter().all(|(k, v)| {
                        self.inline_expr_ok(k, callee_unit, budget)
                            && self.inline_expr_ok(v, callee_unit, budget)
                    })
            }
            Expr::If { test, then, els } => {
                self.inline_expr_ok(test, callee_unit, budget)
                    && self.inline_expr_ok(then, callee_unit, budget)
                    && self.inline_expr_ok(els, callee_unit, budget)
            }
            Expr::Let { bindings, body } => {
                self.inline_binds_ok(bindings, body, callee_unit, budget)
            }
            Expr::Loop { bindings, body } => {
                self.inline_binds_ok(bindings, body, callee_unit, budget)
            }
            _ => false,
        }
    }

    fn inline_callee_into(
        &mut self,
        f: &mut FnCtx<'ctx>,
        arity: &Arity,
        args: &[Expr],
        target: InlineDest<'_, 'ctx>,
    ) -> Result<(), Unsupported> {
        let mut eval_args: Vec<(bool, PointerValue<'ctx>)> = Vec::with_capacity(args.len());
        for (i, (pat, a)) in arity.fixed.iter().zip(args).enumerate() {
            let borrowed = match (pat, a) {
                (Pattern::Symbol(_), Expr::Local(idx)) if !is_move(f, a) => {
                    self.borrowable_before(f, *idx, &args[i + 1..])
                }
                _ => None,
            };
            if let Some(p) = borrowed {
                eval_args.push((false, p));
            } else {
                let s = self.slot(f);
                self.lower(f, a, s)?;
                eval_args.push((true, s));
            }
        }
        let saved_env = std::mem::take(&mut f.env);
        let saved_recur = std::mem::take(&mut f.recur);
        let saved_moves = std::mem::take(&mut f.moves);
        let saved_borrowed = std::mem::take(&mut f.borrowed_params);
        let saved_pods = std::mem::take(&mut f.pod_locals);
        let saved_tail_min = std::mem::replace(&mut f.tail_min, usize::MAX);
        let saved_in_tail = std::mem::replace(&mut f.in_tail, false);
        let saved_captured = f.captured;
        let let_depth = f.let_slots.len();
        f.captured = self.ptr.const_null();
        f.inline_depth += 1;
        f.inlined_count += 1;

        let mut owned_slots = Vec::new();
        let res = self.inline_callee_body(f, arity, eval_args, target, &mut owned_slots);

        f.inline_depth -= 1;
        f.let_slots.truncate(let_depth);
        f.env = saved_env;
        f.recur = saved_recur;
        f.moves = saved_moves;
        f.borrowed_params = saved_borrowed;
        f.pod_locals = saved_pods;
        f.tail_min = saved_tail_min;
        f.in_tail = saved_in_tail;
        f.captured = saved_captured;
        res?;
        self.free_slots(f, &owned_slots);
        Ok(())
    }

    fn inline_callee_body(
        &mut self,
        f: &mut FnCtx<'ctx>,
        arity: &Arity,
        eval_args: Vec<(bool, PointerValue<'ctx>)>,
        target: InlineDest<'_, 'ctx>,
        owned_slots: &mut Vec<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        let mut owned_indices = Vec::new();
        for (pat, (owned, slot)) in arity.fixed.iter().zip(eval_args) {
            if owned {
                let start = f.env.len();
                self.bind_owned_slot(f, pat, slot, owned_slots)?;
                owned_indices.extend(start..f.env.len());
            } else {
                f.env.push(Some(LocalRef::Slot(slot)));
            }
        }
        f.let_slots.extend_from_slice(owned_slots);
        f.moves = collect_body_moves(&arity.body, 0, &f.env, &owned_indices);
        match target {
            InlineDest::Single(dest) => self.lower_body(f, &arity.body, dest),
            InlineDest::Destructure(dests) => self.lower_body_destructure(f, &arity.body, dests),
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

    /// Invoke lowering: handles pure `clojure.string` `require` forms in
    /// `BootMode::Lite`, guarded direct calls of compiled `defn`s, and general
    /// dynamic invocations.
    fn lower_invoke(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if self.direct_target(head, args.len()).is_some() {
            return self.lower_direct_invoke(f, head, args, dest);
        }
        if self.try_lower_static(f, head, args, dest)?
            || self.try_lower_kw_invoke(f, head, args, dest)?
            || self.try_lower_stable_lookup(f, head, args, dest)?
        {
            return Ok(());
        }
        self.lower_value_invoke(f, head, args, dest)
    }

    /// `dest = (head args…)` through [`emit_value_call`](Self::emit_value_call).
    /// As a `defn`'s direct call does, plain locals ride in place: a local
    /// head is called where it lives, and local args are bitwise aliases in
    /// the argument array (the callee only reads its args), cleared after
    /// the call; a local an arg's last use moves drops after it (#271).
    fn lower_value_invoke(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let mut temps = Vec::new();
        let mut moved = Vec::new();
        let local_head = match head {
            Expr::Local(idx) => self.borrowable_before(f, *idx, args),
            _ => None,
        };
        let h = match local_head {
            Some(p) => {
                if is_move(f, head) {
                    moved.push(p);
                }
                p
            }
            None => {
                let h = self.slot(f);
                self.lower(f, head, h)?;
                temps.push(h);
                h
            }
        };
        let (arr, elems) = self.slot_array(f, args.len());
        let mut aliases = Vec::new();
        let mut take_bits = 0u64;
        for (i, (a, e)) in args.iter().zip(&elems).enumerate() {
            let local = match a {
                Expr::Local(idx) if !is_move(f, a) => {
                    self.borrowable_before(f, *idx, &args[i + 1..])
                }
                _ => None,
            };
            if let Some(p) = local {
                self.copy_bits(*e, p);
                aliases.push(*e);
            } else {
                self.lower(f, a, *e)?;
                temps.push(*e);
                take_bits |= clj_eval::native_abi::single_take_arg_bit(i);
            }
        }
        let st = self.emit_value_call(f, h, arr, args.len(), take_bits, dest);
        self.clear_aliases(&aliases);
        self.check(f, st);
        self.drop_and_nil_slots(f.func, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        if !elems.is_empty() {
            f.free_arrays
                .entry(elems.len())
                .or_default()
                .push((arr, elems));
        }
        Ok(())
    }

    /// `dest = (h args…)` for a callee known only at run time (#271): a
    /// compiled closure of exactly this arity runs its code directly, as
    /// a `defn`'s direct call does (`rt_n_enter_closure` checks the callee
    /// and the call depth; `rt_n_leave` closes the call), and any other
    /// callee goes through `rt_n_invoke`. Answers the call's status.
    fn emit_value_call(
        &mut self,
        f: &mut FnCtx<'ctx>,
        h: PointerValue<'ctx>,
        arr: PointerValue<'ctx>,
        argc: usize,
        take_bits: u64,
        dest: PointerValue<'ctx>,
    ) -> IntValue<'ctx> {
        let (code_p, cap_p) = {
            let here = self.builder.get_insert_block().expect("positioned");
            self.builder.position_at_end(f.entry);
            let code_p = self
                .builder
                .build_alloca(self.i64t, "ccode")
                .expect("alloca");
            let cap_p = self.builder.build_alloca(self.ptr, "ccap").expect("alloca");
            self.builder.position_at_end(here);
            (code_p, cap_p)
        };
        let n = self.u64c(argc as u64);
        let n_take = self.u64c((argc as u64) | take_bits);
        let entered = self
            .call(
                "rt_n_enter_closure",
                Some(self.i8t),
                &[h.into(), n, code_p.into(), cap_p.into()],
            )
            .expect("entered");
        let go = self
            .builder
            .build_int_compare(IntPredicate::NE, entered, self.i8t.const_zero(), "go")
            .expect("cmp");
        let direct_b = self.ctx.append_basic_block(f.func, "closure_direct");
        let slow_b = self.ctx.append_basic_block(f.func, "closure_slow");
        let done = self.ctx.append_basic_block(f.func, "closure_done");
        self.builder
            .build_conditional_branch(go, direct_b, slow_b)
            .expect("br");
        self.builder.position_at_end(direct_b);
        let code = self
            .builder
            .build_load(self.i64t, code_p, "code")
            .expect("load")
            .into_int_value();
        let code = self
            .builder
            .build_int_to_ptr(code, self.ptr, "codefn")
            .expect("i2p");
        let cap = self
            .builder
            .build_load(self.ptr, cap_p, "cap")
            .expect("load");
        let st_direct = self
            .builder
            .build_indirect_call(
                self.code_ty,
                code,
                &[dest.into(), arr.into(), n_take, cap.into()],
                "st",
            )
            .expect("call")
            .try_as_basic_value()
            .expect_basic("st")
            .into_int_value();
        self.call("rt_n_leave", None, &[]);
        let direct_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(slow_b);
        let looked = self.emit_lookup_call(f, h, arr, argc, dest, done);
        let aref = self.slots_ref(f, arr, argc);
        let st_slow = self
            .call(
                "rt_n_invoke",
                Some(self.i32t),
                &[dest.into(), h.into(), aref.into()],
            )
            .expect("st");
        let slow_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        let phi = self.builder.build_phi(self.i32t, "st").expect("phi");
        phi.add_incoming(&[(&st_direct, direct_end), (&st_slow, slow_end)]);
        if let Some(b) = looked {
            phi.add_incoming(&[(&self.i32t.const_zero(), b)]);
        }
        phi.as_basic_value().into_int_value()
    }

    /// A one- or two-arg value call that missed the compiled-closure
    /// entry (#271): a map, set, keyword or symbol callee answers through
    /// `rt_n_lookup_call` straight to `done` (the block returned, status
    /// 0); anything else continues in a fresh block, positioned for
    /// `rt_n_invoke`. Other arities emit nothing.
    fn emit_lookup_call(
        &mut self,
        f: &mut FnCtx<'ctx>,
        h: PointerValue<'ctx>,
        arr: PointerValue<'ctx>,
        argc: usize,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    ) -> Option<BasicBlock<'ctx>> {
        if !(1..=2).contains(&argc) {
            return None;
        }
        let a = self.elem(arr, 0);
        let nf = if argc == 2 {
            self.elem(arr, 1)
        } else {
            self.ptr.const_null()
        };
        let hit = self
            .call(
                "rt_n_lookup_call",
                Some(self.i8t),
                &[dest.into(), h.into(), a.into(), nf.into()],
            )
            .expect("hit");
        let is_hit = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "looked")
            .expect("cmp");
        let looked = self.builder.get_insert_block().expect("positioned");
        let invoke_b = self.ctx.append_basic_block(f.func, "value_invoke");
        self.builder
            .build_conditional_branch(is_hit, done, invoke_b)
            .expect("br");
        self.builder.position_at_end(invoke_b);
        Some(looked)
    }

    /// `(:k coll)` / `(:k coll not-found)` with a literal keyword head
    /// (#271): the bitcode `get` over borrowed operands (the keyword in
    /// place in the constant pool), falling back for any shape the
    /// helper leaves to the runtime to `rt_n_destr_get`: the keyword's
    /// own invoke is the interpreter's `lookup` with its default
    /// (`invoke_keyed_name`), so it reads through that directly, with no
    /// general call (hash maps, records, sets, strings …).
    fn try_lower_kw_invoke(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let (Expr::Literal(kw @ Value::Keyword(_)) | Expr::Quote(kw @ Value::Keyword(_))) = head
        else {
            return Ok(false);
        };
        let shim = match (args.len(), self.use_bitcode) {
            (1, true) => "rt_bc_2_get",
            (2, true) => "rt_bc_3_get",
            _ => return Ok(false),
        };
        let kref = {
            let k = self.value_const(kw)?;
            self.const_ref(k)
        };
        let (args_refs, temps, moved) = self.arg_refs(f, args, dest)?;
        let mut refs = vec![args_refs[0], kref];
        refs.extend(args_refs.iter().skip(1));
        let bc = self.ctx.append_basic_block(f.func, "kw_get");
        let slow = self.ctx.append_basic_block(f.func, "kw_slow");
        let done = self.ctx.append_basic_block(f.func, "kw_done");
        self.builder.build_unconditional_branch(bc).expect("br");
        self.emit_bc_op(f.func, bc, shim, &refs, dest, slow, done);
        self.builder.position_at_end(slow);
        let nf = args_refs
            .get(1)
            .copied()
            .unwrap_or_else(|| self.ptr.const_null());
        self.call(
            "rt_n_destr_get",
            None,
            &[dest.into(), args_refs[0].into(), kref.into(), nf.into()],
        );
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        Ok(true)
    }

    /// Invoke a stable map, set, symbol, or keyword through its borrowed
    /// root. The defining top-level unit established both the value and
    /// its lookup semantics before this code can run.
    fn try_lower_stable_lookup(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let Expr::Global(var) = head else {
            return Ok(false);
        };
        let Some(root) = self.stable_of(var) else {
            return Ok(false);
        };
        if !self.stable_unchecked(root)
            || !matches!(root.kind, StableKind::Lookup)
            || !(1..=2).contains(&args.len())
        {
            return Ok(false);
        }
        let (refs, temps, moved) = self.arg_refs(f, args, dest)?;
        let default = refs
            .get(1)
            .copied()
            .unwrap_or_else(|| self.ptr.const_null());
        self.call(
            "rt_n_lookup_call",
            Some(self.i8t),
            &[
                dest.into(),
                root.root.into(),
                refs[0].into(),
                default.into(),
            ],
        );
        self.free_slots(f, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        Ok(true)
    }

    fn try_lower_static(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Expr::Global(v) = head else {
            return Ok(false);
        };
        if v.sym.namespace() != Some("clojure.core") || v.sym.name() != "__static*" {
            return Ok(false);
        }
        let Some((op, rest)) = static_call_op(args) else {
            return Ok(false);
        };
        let (refs, temps, moved) = self.arg_refs(f, rest, dest)?;
        let slow = self.ctx.append_basic_block(f.func, "static_slow");
        let done = self.ctx.append_basic_block(f.func, "static_done");
        self.emit_static_fast(f, op, &refs, dest, slow, done);
        self.emit_static_invoke_slow(f, head, args, &refs, dest, slow, done)?;
        self.builder.position_at_end(done);
        self.free_slots(f, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        Ok(true)
    }

    fn emit_static_fast(
        &self,
        f: &FnCtx<'ctx>,
        op: StaticOp,
        refs: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        match op {
            StaticOp::Parse(shim) => self.lower_static_parse(shim, refs[0], dest, slow, done),
            StaticOp::RemUnsigned => self.lower_static_urem(f, refs[0], refs[1], dest, slow, done),
            StaticOp::CharDigit => {
                self.lower_static_char_digit(f, refs[0], refs[1], dest, slow, done);
            }
            StaticOp::MathAbs => self.lower_static_math_abs(f, refs[0], dest, slow, done),
            StaticOp::MathSqrt => self.lower_static_math_sqrt(f, refs[0], dest, slow, done),
            StaticOp::MathUnary(shim) => {
                self.when_numeric(f, &[refs[0]], slow);
                let x = self.num_as_f64(refs[0]);
                let r = self.call_f64(shim, &[x.into()]);
                self.write_double(f.func, dest, r);
                self.builder.build_unconditional_branch(done).expect("br");
            }
            StaticOp::MathBinary(shim) => {
                self.when_numeric(f, &[refs[0], refs[1]], slow);
                let (x, y) = (self.num_as_f64(refs[0]), self.num_as_f64(refs[1]));
                let r = self.call_f64(shim, &[x.into(), y.into()]);
                self.write_double(f.func, dest, r);
                self.builder.build_unconditional_branch(done).expect("br");
            }
        }
    }

    fn lower_static_parse(
        &self,
        shim: &str,
        arg: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let hit = self
            .call(shim, Some(self.i8t), &[dest.into(), arg.into()])
            .expect("hit");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "parse_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, done, slow)
            .expect("br");
    }

    fn lower_static_urem(
        &self,
        f: &FnCtx<'ctx>,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let ints = self.both_ints(lhs, Some(rhs));
        let int_bb = self.ctx.append_basic_block(f.func, "urem_int");
        self.builder
            .build_conditional_branch(ints, int_bb, slow)
            .expect("br");
        self.builder.position_at_end(int_bb);
        let (x, y) = (self.int_payload(lhs), self.int_payload(rhs));
        let nz = self
            .builder
            .build_int_compare(IntPredicate::NE, y, self.i64t.const_zero(), "nz")
            .expect("cmp");
        let go_bb = self.ctx.append_basic_block(f.func, "urem_go");
        self.builder
            .build_conditional_branch(nz, go_bb, slow)
            .expect("br");
        self.builder.position_at_end(go_bb);
        let r = self
            .builder
            .build_int_unsigned_rem(x, y, "urem")
            .expect("urem");
        self.write_int(f.func, dest, r);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn lower_static_char_digit(
        &self,
        f: &FnCtx<'ctx>,
        ch: PointerValue<'ctx>,
        rad: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let bb = &self.builder;
        let is_ch = bb
            .build_int_compare(
                IntPredicate::EQ,
                self.tag(ch),
                self.i8t.const_int(TAG_CHAR, false),
                "is_ch",
            )
            .expect("cmp");
        let is_rad = bb
            .build_int_compare(
                IntPredicate::EQ,
                self.tag(rad),
                self.i8t.const_int(TAG_INT, false),
                "is_rad",
            )
            .expect("cmp");
        let both = bb.build_and(is_ch, is_rad, "ch_rad").expect("and");
        let chk_bb = self.ctx.append_basic_block(f.func, "cdig_chk");
        bb.build_conditional_branch(both, chk_bb, slow).expect("br");
        self.builder.position_at_end(chk_bb);
        let bb = &self.builder;
        let r = self.int_payload(rad);
        let r_off = bb
            .build_int_sub(r, self.i64t.const_int(2, false), "roff")
            .expect("sub");
        let r_ok = bb
            .build_int_compare(
                IntPredicate::ULE,
                r_off,
                self.i64t.const_int(34, false),
                "rok",
            )
            .expect("cmp");
        let go_bb = self.ctx.append_basic_block(f.func, "cdig_go");
        bb.build_conditional_branch(r_ok, go_bb, slow).expect("br");
        self.builder.position_at_end(go_bb);
        let bb = &self.builder;
        let cu = bb
            .build_int_z_extend(self.char_payload(ch), self.i64t, "cu")
            .expect("zext");
        let d_dig = bb
            .build_int_sub(cu, self.i64t.const_int(u64::from(b'0'), false), "dd")
            .expect("sub");
        let is_dig = bb
            .build_int_compare(
                IntPredicate::ULE,
                d_dig,
                self.i64t.const_int(9, false),
                "id",
            )
            .expect("cmp");
        let clo = bb
            .build_or(cu, self.i64t.const_int(32, false), "clo")
            .expect("or");
        let a_off = bb
            .build_int_sub(clo, self.i64t.const_int(u64::from(b'a'), false), "ao")
            .expect("sub");
        let is_alpha = bb
            .build_int_compare(
                IntPredicate::ULE,
                a_off,
                self.i64t.const_int(25, false),
                "ia",
            )
            .expect("cmp");
        let d_alpha = bb
            .build_int_add(a_off, self.i64t.const_int(10, false), "da")
            .expect("add");
        let neg1 = self.i64t.const_int((-1i64) as u64, true);
        let alpha_or = bb
            .build_select(is_alpha, d_alpha, neg1, "ao1")
            .expect("sel")
            .into_int_value();
        let raw = bb
            .build_select(is_dig, d_dig, alpha_or, "raw")
            .expect("sel")
            .into_int_value();
        let in_rad = bb
            .build_int_compare(IntPredicate::ULT, raw, r, "inrad")
            .expect("cmp");
        let out = bb
            .build_select(in_rad, raw, neg1, "dig")
            .expect("sel")
            .into_int_value();
        self.write_int(f.func, dest, out);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn lower_static_math_abs(
        &self,
        f: &FnCtx<'ctx>,
        arg: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let tag = self.tag(arg);
        let is_int = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, self.i8t.const_int(2, false), "abs_i")
            .expect("cmp");
        let int_bb = self.ctx.append_basic_block(f.func, "abs_int");
        let chk_dbl = self.ctx.append_basic_block(f.func, "abs_chk_dbl");
        self.builder
            .build_conditional_branch(is_int, int_bb, chk_dbl)
            .expect("br");

        self.builder.position_at_end(int_bb);
        let x = self.int_payload(arg);
        let intr = inkwell::intrinsics::Intrinsic::find("llvm.abs").expect("llvm.abs");
        let decl = intr
            .get_declaration(&self.module, &[self.i64t.into()])
            .expect("decl");
        let false_bit = self.ctx.bool_type().const_zero();
        let abs_i = self
            .builder
            .build_call(decl, &[x.into(), false_bit.into()], "iabs")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("iabs")
            .into_int_value();
        self.write_int(f.func, dest, abs_i);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(chk_dbl);
        let is_dbl = self
            .builder
            .build_int_compare(IntPredicate::EQ, tag, self.i8t.const_int(3, false), "abs_d")
            .expect("cmp");
        let dbl_bb = self.ctx.append_basic_block(f.func, "abs_dbl");
        self.builder
            .build_conditional_branch(is_dbl, dbl_bb, slow)
            .expect("br");

        self.builder.position_at_end(dbl_bb);
        let f64t = self.ctx.f64_type();
        let d = self
            .builder
            .build_load(f64t, self.byte_at(arg, 8), "d")
            .expect("load")
            .into_float_value();
        let fintr = inkwell::intrinsics::Intrinsic::find("llvm.fabs").expect("llvm.fabs");
        let fdecl = fintr
            .get_declaration(&self.module, &[f64t.into()])
            .expect("decl");
        let abs_d = self
            .builder
            .build_call(fdecl, &[d.into()], "fabs")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("fabs")
            .into_float_value();
        self.write_double(f.func, dest, abs_d);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn lower_static_math_sqrt(
        &self,
        f: &FnCtx<'ctx>,
        arg: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        self.when_numeric(f, &[arg], slow);
        let x = self.num_as_f64(arg);
        let f64t = self.ctx.f64_type();
        let intr = inkwell::intrinsics::Intrinsic::find("llvm.sqrt").expect("llvm.sqrt");
        let decl = intr
            .get_declaration(&self.module, &[f64t.into()])
            .expect("decl");
        let r = self
            .builder
            .build_call(decl, &[x.into()], "sqrt")
            .expect("call")
            .try_as_basic_value()
            .expect_basic("sqrt")
            .into_float_value();
        self.write_double(f.func, dest, r);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_static_invoke_slow(
        &mut self,
        f: &mut FnCtx<'ctx>,
        head: &Expr,
        args: &[Expr],
        refs: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        self.builder.position_at_end(slow);
        let h = self.slot(f);
        self.lower(f, head, h)?;
        let (arr, elems) = self.slot_array(f, args.len());
        self.lower(f, &args[0], elems[0])?;
        self.lower(f, &args[1], elems[1])?;
        for (i, r) in refs.iter().enumerate() {
            self.copy_bits(elems[2 + i], *r);
        }
        let aref = self.slots_ref(f, arr, args.len());
        let st = self
            .call(
                "rt_n_invoke",
                Some(self.i32t),
                &[dest.into(), h.into(), aref.into()],
            )
            .expect("st");
        self.clear_aliases(&elems[2..]);
        self.check(f, st);
        self.drop_v(f.func, h);
        self.write_tag(h, 0);
        self.drop_and_nil_slots(f.func, &elems[..2]);
        self.builder.build_unconditional_branch(done).expect("br");
        Ok(())
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
        if let Some(arity) = self.inline_callee(f, head, args.len(), INLINE_GEN_MAX_NODES) {
            return self.inline_callee_into(f, &arity, args, InlineDest::Single(dest));
        }
        let (var, k, code) = self.direct_target(head, args.len()).expect("checked");
        let vk = self.var_of(&var);
        // Plain locals ride in the argument array as bitwise aliases (the
        // callee only reads its args); they are cleared, never dropped,
        // right after the call. Computed args are owned there.
        let (arr, elems) = self.slot_array(f, args.len());
        let mut aliases = Vec::new();
        let mut temps = Vec::new();
        let mut take_bits = 0u64;
        for (i, (a, e)) in args.iter().zip(&elems).enumerate() {
            let local = match a {
                Expr::Local(idx) if !is_move(f, a) => {
                    self.borrowable_before(f, *idx, &args[i + 1..])
                }
                _ => None,
            };
            if let Some(p) = local {
                self.copy_bits(*e, p);
                aliases.push(*e);
            } else if let Some((p, fallback)) = self.borrowed(f, a)? {
                self.copy_bits(*e, p);
                aliases.push(*e);
                temps.extend(fallback);
            } else {
                self.lower(f, a, *e)?;
                temps.push(*e);
                take_bits |= clj_eval::native_abi::single_take_arg_bit(i);
            }
        }
        // A stable `defn`'s root is its compiled fn once the `def` has
        // run, and a direct call can run no earlier from its own unit (the
        // init is the `fn` itself) or any later one: only the call depth
        // needs checking there (#271).
        let entered = if self.stable_of(&var).is_some_and(|sr| self.unit >= sr.unit) {
            self.call("rt_n_enter", Some(self.i8t), &[])
        } else {
            self.call(
                "rt_n_enter_fn",
                Some(self.i8t),
                &[self.u64c(vk), self.u64c(k as u64)],
            )
        };
        let ok = entered.expect("ok");
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
                    self.u64c((args.len() as u64) | take_bits),
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
        self.emit_deref(f.func, h, vk);
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
        self.drop_and_nil_slots(f.func, &temps);
        if !elems.is_empty() {
            f.free_arrays
                .entry(elems.len())
                .or_default()
                .push((arr, elems));
        }
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
        if self.lite
            && let Some(aliases) = lite_direct_require_aliases(call)
        {
            for alias in aliases {
                let ap = self.str_ptr(alias.as_deref().unwrap_or(""));
                self.call("rt_n_require_string_lite", None, &[dest.into(), ap.into()]);
            }
            return Ok(());
        }
        if self.try_lower_assoc_conj(f, call, Some(dest))?
            || self.try_lower_scaled_long(f, call, dest)?
            || self.try_lower_char_selection(f, call, dest)?
            || self.try_lower_mapped_str_join(f, call, dest)?
            || (self.inline_arith && self.try_lower_hof_loop(f, call, dest)?)
        {
            return Ok(());
        }
        self.lower_direct_with(f, call, None, dest)
    }

    fn try_lower_char_selection(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Some((inner, predicate, input, target, mode)) = char_selection(call, f.env.len())
        else {
            return Ok(false);
        };
        let coll = self.slot(f);
        self.lower(f, input, coll)?;
        let done = self.ctx.append_basic_block(f.func, "char_done");
        self.emit_char_selection(f, dest, coll, target, mode, done);
        let pred = self.slot(f);
        let selected = self.slot(f);
        self.lower(f, predicate, pred)?;
        self.core_call_owned(f, inner, &[pred, coll], selected);
        self.core_call_owned(f, call, &[selected], dest);
        self.free_slots(f, &[pred, selected]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &[coll]);
        Ok(true)
    }

    fn emit_char_selection(
        &self,
        f: &FnCtx<'ctx>,
        dest: PointerValue<'ctx>,
        coll: PointerValue<'ctx>,
        target: u32,
        mode: u8,
        done: BasicBlock<'ctx>,
    ) {
        let hit = self
            .call(
                "rt_n_select_chars",
                Some(self.i8t),
                &[
                    dest.into(),
                    coll.into(),
                    self.i32t.const_int(u64::from(target), false).into(),
                    self.i8t.const_int(u64::from(mode), false).into(),
                ],
            )
            .expect("selection status");
        let fallback = self.ctx.append_basic_block(f.func, "char_fallback");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "char_hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, done, fallback)
            .expect("br");
        self.builder.position_at_end(fallback);
    }

    fn emit_char_filter_probe(
        &self,
        f: &FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
        coll: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let mode = match hl.kind {
            HofKind::CountFilter => 0,
            HofKind::FilterCollect => 1,
            _ => return,
        };
        if let Some(ch) = char_predicate(hl.arity, base) {
            self.emit_char_selection(f, dest, coll, ch, mode, done);
        }
    }

    fn try_lower_mapped_str_join(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Some((separator, input)) = mapped_str_join(call) else {
            return Ok(false);
        };
        let coll = self.slot(f);
        self.lower(f, input, coll)?;
        let sep = self.str_ptr(separator);
        let status = self
            .call(
                "rt_n_join_mapped_str",
                Some(self.i32t),
                &[dest.into(), sep.into(), coll.into()],
            )
            .expect("join status");
        self.check(f, status);
        self.free_slots(f, &[coll]);
        Ok(true)
    }

    fn try_lower_scaled_long(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Some(args) = scaled_long_args(call) else {
            return Ok(false);
        };
        // Only inert operands may be read before the product is checked.
        // Cloning the forms keeps their original last-use moves in the
        // fallback. The probe borrows local operands.
        let borrowed = args.map(Expr::clone);
        let (refs, temps, _) = self.arg_refs(f, &borrowed, dest)?;
        let hit = self
            .call(
                "rt_n_scaled_long",
                Some(self.i8t),
                &[dest.into(), refs[0].into(), refs[1].into(), refs[2].into()],
            )
            .expect("hit");
        self.free_slots(f, &temps);
        let boxed = self.ctx.append_basic_block(f.func, "scale_boxed");
        let scaled = self.ctx.append_basic_block(f.func, "scale_fast");
        let done = self.ctx.append_basic_block(f.func, "scale_done");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "scale_hit")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, scaled, boxed)
            .expect("br");
        self.builder.position_at_end(scaled);
        for arg in args {
            if let Expr::Local(idx) = arg
                && is_move(f, arg)
                && let Some(slot) = self.local_slot(f, *idx).filter(|&slot| slot != dest)
            {
                self.drop_and_nil_slots(f.func, &[slot]);
            }
        }
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(boxed);
        self.lower_direct_with(f, call, None, dest)?;
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        Ok(true)
    }

    /// Run a [`HofLoop`] inline (#271 Slice 4c). The collection (and a
    /// `reduce` init) evaluate in argument order; then a persistent vector
    /// or long range runs the `fn`'s body per item right here, with its
    /// params bound like a `let` (no closure, no lazy seq, no call). Any
    /// other collection takes the general call, the `fn` made into a
    /// closure, over the already-evaluated collection: exactly what `run`
    /// does, lazy forcing order included.
    fn try_lower_hof_loop(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if self.try_lower_dot_product(f, call, dest)?
            || self.try_lower_print_join(f, call, dest)?
            || self.try_lower_mapv_global_parse(f, call, dest)?
            || self.try_lower_atom_nth(f, call, dest)?
        {
            return Ok(true);
        }
        let Some(hl) = hof_loop(call) else {
            return Ok(false);
        };
        let base = f.env.len();
        // The analysis saw the `fn` capture its free locals before the
        // collection (and init) evaluated, so it may let those move a local
        // the body still reads. Here the body runs after them, and the
        // general path's closure is made after them: keep those locals.
        let mut free = BTreeSet::new();
        arity_free(hl.arity, base, &mut free);
        let enclosing = f.moves.clone();
        f.moves.retain(|m| !free.contains(&m.1));
        let lowered = self.lower_hof_loop(f, &hl, base, dest);
        f.moves = enclosing;
        lowered?;
        Ok(true)
    }

    fn global_parse_num_mode(&self, var: &clj_value::Var) -> Option<u8> {
        if var.sym.namespace() == Some("clojure.core") {
            return match var.sym.name() {
                "parse-long" => Some(0),
                "parse-double" => Some(2),
                _ => None,
            };
        }
        let sr = self.stable_of(var)?;
        if !self.stable_unchecked(sr) {
            return None;
        }
        let arities = self.callees.get(&var_key(&var.sym))?;
        let arity = arities
            .iter()
            .find(|a| a.rest.is_none() && a.fixed.len() == 1)?;
        matches!(arity.fixed[0], Pattern::Symbol(_))
            .then(|| parse_num_arity(arity, 0))
            .flatten()
    }

    fn try_lower_mapv_global_parse(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "mapv" {
            return Ok(false);
        }
        let [Expr::Global(var), coll] = call.args.as_slice() else {
            return Ok(false);
        };
        let Some(mode) = self.global_parse_num_mode(var) else {
            return Ok(false);
        };
        let coll_s = self.slot(f);
        self.lower(f, coll, coll_s)?;
        let done = self.ctx.append_basic_block(f.func, "mp_done");
        let slow = self.ctx.append_basic_block(f.func, "mp_slow");
        let hit = self
            .call(
                "rt_n_mapv_parse_num",
                Some(self.i8t),
                &[
                    dest.into(),
                    coll_s.into(),
                    self.i8t.const_int(u64::from(mode), false).into(),
                ],
            )
            .expect("hit");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "mp_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, done, slow)
            .expect("br");
        self.builder.position_at_end(slow);
        let fn_s = self.slot(f);
        self.lower(f, &call.args[0], fn_s)?;
        self.core_call_owned(f, call, &[fn_s, coll_s], dest);
        self.free_slots(f, &[fn_s]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &[coll_s]);
        Ok(true)
    }

    fn try_lower_atom_nth(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let Some((deref, atom, indices)) = atom_nth_args(call) else {
            return Ok(false);
        };
        let (helper, op) = if indices.len() == 1 {
            ("rt_n_op2", native_abi::OP_ATOM_NTH)
        } else {
            ("rt_n_op3", native_abi::OP_ATOM_NTH2)
        };
        let k = (self.var_of(deref) << 32) | self.var_of(&call.var);
        let mut args = vec![
            dest.into(),
            self.i32t.const_int(u64::from(op), false).into(),
            self.u64c(k),
        ];
        let mut slots = Vec::new();
        for expr in std::iter::once(atom).chain(indices) {
            let slot = self.slot(f);
            self.lower(f, expr, slot)?;
            slots.push(slot);
            args.push(slot.into());
        }
        let st = self.call(helper, Some(self.i32t), &args).expect("status");
        self.check(f, st);
        self.free_slots(f, &slots);
        Ok(true)
    }

    fn try_lower_swap_fx(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith
            || call.var.sym.namespace() != Some("clojure.core")
            || call.var.sym.name() != "swap!"
        {
            return Ok(false);
        }
        let (op_name, arg_ptrs): (&str, Vec<&Expr>) = match call.args.as_slice() {
            [atom, fe, Expr::VecLit(ks, None), v]
                if ks.len() == 2 && is_core_global(fe, "assoc-in") =>
            {
                ("swap-assoc-in2-fx!", vec![atom, fe, &ks[0], &ks[1], v])
            }
            args if matches!(args.len(), 3 | 4) => ("swap-fx!", args.iter().collect()),
            _ => return Ok(false),
        };
        let k = self.var_of(&call.var);
        let dest = self.slot(f);
        let res = self.lower_own_call(f, k, Some(op_name), &arg_ptrs, dest);
        self.free_slots(f, &[dest]);
        res?;
        Ok(true)
    }

    fn try_lower_dot_product(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let Some((plus, init, m, star, c1, c2)) = dot_product_call(call) else {
            return Ok(false);
        };
        let init_s = self.slot(f);
        self.lower(f, init, init_s)?;
        let c1_s = self.slot(f);
        self.lower(f, c1, c1_s)?;
        let c2_s = self.slot(f);
        self.lower(f, c2, c2_s)?;
        let ok = self
            .call(
                "rt_n_dot",
                Some(self.i8t),
                &[dest.into(), init_s.into(), c1_s.into(), c2_s.into()],
            )
            .expect("ok");
        let slow = self.ctx.append_basic_block(f.func, "dot_slow");
        let done = self.ctx.append_basic_block(f.func, "dot_done");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "dot_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done, slow)
            .expect("br");
        self.builder.position_at_end(slow);
        let plus_s = self.slot(f);
        self.lower(f, plus, plus_s)?;
        let star_s = self.slot(f);
        self.lower(f, star, star_s)?;
        let seq = self.slot(f);
        self.core_call_owned(f, m, &[star_s, c1_s, c2_s], seq);
        self.core_call_owned(f, call, &[plus_s, init_s, seq], dest);
        self.free_slots(f, &[plus_s, star_s, seq]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &[init_s, c1_s, c2_s]);
        Ok(true)
    }

    fn try_lower_print_join(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let Some(pj) = print_join_call(call) else {
            return Ok(false);
        };
        let sep_s = self.slot(f);
        self.lower(f, pj.sep, sep_s)?;
        let coll_s = self.slot(f);
        self.lower(f, pj.coll, coll_s)?;
        let hit_slot = self.entry_alloca(f, self.i8t, "pjh");
        let st = self
            .call(
                "rt_n_print_join",
                Some(self.i32t),
                &[
                    hit_slot.into(),
                    sep_s.into(),
                    coll_s.into(),
                    self.i8t.const_int(u64::from(pj.flags), false).into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        let fast = self.ctx.append_basic_block(f.func, "pj_fast");
        let slow = self.ctx.append_basic_block(f.func, "pj_slow");
        let done = self.ctx.append_basic_block(f.func, "pj_done");
        let hit = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                self.load_i8(hit_slot),
                self.i8t.const_zero(),
                "pj_ok",
            )
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, fast, slow)
            .expect("br");
        self.builder.position_at_end(fast);
        self.drop_v(f.func, dest);
        self.write_tag(dest, 0);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(slow);
        let joined = self.slot(f);
        self.core_call_owned(f, pj.join, &[sep_s, coll_s], joined);
        if let Some((str_call, nl_expr)) = pj.str_call {
            let nl_s = self.slot(f);
            self.lower(f, nl_expr, nl_s)?;
            let out_s = self.slot(f);
            self.core_call_owned(f, str_call, &[joined, nl_s], out_s);
            self.core_call_owned(f, call, &[out_s], dest);
            self.free_slots(f, &[nl_s, out_s]);
        } else {
            self.core_call_owned(f, call, &[joined], dest);
        }
        self.free_slots(f, &[joined]);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &[sep_s, coll_s]);
        Ok(true)
    }

    fn emit_iter_plan(
        &self,
        f: &FnCtx<'ctx>,
        coll: PointerValue<'ctx>,
    ) -> (IntValue<'ctx>, PointerValue<'ctx>) {
        let plan_ty = self.ctx.struct_type(&[self.i64t.into(); 3], false);
        let plan = self.entry_alloca(f, plan_ty, "plan");
        let kind = self
            .call(
                "rt_n_iter_plan",
                Some(self.i8t),
                &[coll.into(), plan.into()],
            )
            .expect("kind");
        (kind, plan)
    }

    fn load_loop_state(
        &self,
        acc: PointerValue<'ctx>,
        coll: PointerValue<'ctx>,
        kind: IntValue<'ctx>,
        plan: PointerValue<'ctx>,
    ) -> LoopState<'ctx> {
        let plan_ty = self.ctx.struct_type(&[self.i64t.into(); 3], false);
        let fields = (0..3)
            .map(|i| {
                let p = self
                    .builder
                    .build_struct_gep(plan_ty, plan, i, "plan_field")
                    .expect("gep");
                self.builder
                    .build_load(self.i64t, p, "plan_v")
                    .expect("load")
                    .into_int_value()
            })
            .collect::<Vec<_>>();
        LoopState {
            acc,
            coll,
            kind,
            n: fields[0],
            start: fields[1],
            step: fields[2],
        }
    }

    fn emit_parse_num_probe(
        &self,
        f: &FnCtx<'ctx>,
        coll: PointerValue<'ctx>,
        mode: u8,
        dest: PointerValue<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let hit = self
            .call(
                "rt_n_mapv_parse_num",
                Some(self.i8t),
                &[
                    dest.into(),
                    coll.into(),
                    self.i8t.const_int(u64::from(mode), false).into(),
                ],
            )
            .expect("hit");
        let plan_b = self.ctx.append_basic_block(f.func, "loop_plan");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "parsed")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, done, plan_b)
            .expect("br");
        self.builder.position_at_end(plan_b);
    }

    fn load_hof_states(
        &self,
        acc: PointerValue<'ctx>,
        coll: PointerValue<'ctx>,
        plan1: (IntValue<'ctx>, PointerValue<'ctx>),
        coll2: Option<PointerValue<'ctx>>,
        plan2: Option<(IntValue<'ctx>, PointerValue<'ctx>)>,
    ) -> (LoopState<'ctx>, Option<LoopState<'ctx>>) {
        let mut state = self.load_loop_state(acc, coll, plan1.0, plan1.1);
        let state2 = coll2
            .zip(plan2)
            .map(|(c2, (k2, p2))| self.load_loop_state(acc, c2, k2, p2));
        if let Some(st2) = &state2 {
            let lt = self
                .builder
                .build_int_compare(IntPredicate::SLT, state.n, st2.n, "n_lt")
                .expect("cmp");
            state.n = self
                .builder
                .build_select(lt, state.n, st2.n, "n_min")
                .expect("sel")
                .into_int_value();
        }
        (state, state2)
    }

    /// One fused-loop collection's `(plan kind, plan slots, range source)`:
    /// a [`range_plan_call`] lowers its argument exprs into slots and
    /// classifies them in place (`rt_n_range_plan`) without building the
    /// range value (#271 Slice 5l) — the returned source materializes the
    /// range into `slot` on the general path. Every other collection
    /// lowers into `slot` and classifies through `rt_n_iter_plan`.
    fn emit_coll_plan<'a>(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &'a Expr,
        slot: PointerValue<'ctx>,
    ) -> Result<
        (
            IntValue<'ctx>,
            PointerValue<'ctx>,
            Option<RangeSrc<'a, 'ctx>>,
        ),
        Unsupported,
    > {
        let Some(call) = range_plan_call(e) else {
            self.lower(f, e, slot)?;
            let (kind, plan) = self.emit_iter_plan(f, slot);
            return Ok((kind, plan, None));
        };
        let mut args = Vec::with_capacity(call.args.len());
        for a in &call.args {
            let s = self.slot(f);
            self.lower(f, a, s)?;
            args.push(s);
        }
        let plan_ty = self.ctx.struct_type(&[self.i64t.into(); 3], false);
        let plan = self.entry_alloca(f, plan_ty, "plan");
        let mut all_int = self.ctx.bool_type().const_int(1, false);
        for a in &args {
            let is_int = self
                .builder
                .build_int_compare(
                    IntPredicate::EQ,
                    self.tag(*a),
                    self.i8t.const_int(2, false),
                    "rng_i",
                )
                .expect("cmp");
            all_int = self
                .builder
                .build_and(all_int, is_int, "rng_ints")
                .expect("and");
        }
        let fast = self.ctx.append_basic_block(f.func, "range_fast");
        let slow = self.ctx.append_basic_block(f.func, "range_slow");
        let join = self.ctx.append_basic_block(f.func, "range_join");
        self.builder
            .build_conditional_branch(all_int, fast, slow)
            .expect("br");
        self.builder.position_at_end(fast);
        let mut bound = [self.i64t.const_zero(); 3];
        for (b, a) in bound.iter_mut().zip(&args) {
            *b = self.int_payload(*a);
        }
        let argc = self.i8t.const_int(args.len() as u64, false);
        let fast_kind = self
            .call(
                "rt_n_range_plan",
                Some(self.i8t),
                &[
                    argc.into(),
                    bound[0].into(),
                    bound[1].into(),
                    bound[2].into(),
                    plan.into(),
                ],
            )
            .expect("kind");
        self.builder.build_unconditional_branch(join).expect("br");
        self.builder.position_at_end(slow);
        let slow_kind = self.i8t.const_zero();
        self.builder.build_unconditional_branch(join).expect("br");
        self.builder.position_at_end(join);
        let kind = self.builder.build_phi(self.i8t, "rng_kind").expect("phi");
        kind.add_incoming(&[(&fast_kind, fast), (&slow_kind, slow)]);
        let kind = kind.as_basic_value().into_int_value();
        Ok((kind, plan, Some(RangeSrc { call, args })))
    }

    /// [`try_lower_hof_loop`] once the moves are safe.
    /// A fused loop's first collection, planned for its shape: the
    /// parse-num probe reads the collection value, so a probed `mapv`
    /// keeps the materialized path (`rt_n_iter_plan`); anything else
    /// goes through [`Self::emit_coll_plan`].
    fn plan_first_coll<'a>(
        &mut self,
        f: &mut FnCtx<'ctx>,
        hl: &HofLoop<'a>,
        base: usize,
        coll: PointerValue<'ctx>,
    ) -> Result<
        (
            IntValue<'ctx>,
            PointerValue<'ctx>,
            Option<RangeSrc<'a, 'ctx>>,
            Option<u8>,
        ),
        Unsupported,
    > {
        let probe = (hl.kind == HofKind::Collect && hl.coll2.is_none())
            .then(|| parse_num_arity(hl.arity, base))
            .flatten();
        if probe.is_none() {
            let (kind, plan, rsrc) = self.emit_coll_plan(f, hl.coll, coll)?;
            return Ok((kind, plan, rsrc, None));
        }
        self.lower(f, hl.coll, coll)?;
        let (kind, plan) = self.emit_iter_plan(f, coll);
        Ok((kind, plan, None, probe))
    }

    /// The general block's range materializations: every range source
    /// that skipped its value builds it here, once, over the already
    /// lowered argument slots (moved into the call). Returns the
    /// argument slots for the caller to free.
    fn materialize_range_srcs(
        &mut self,
        f: &mut FnCtx<'ctx>,
        rsrc1: Option<&RangeSrc<'_, 'ctx>>,
        rsrc2: Option<&RangeSrc<'_, 'ctx>>,
        coll: PointerValue<'ctx>,
        coll2: Option<PointerValue<'ctx>>,
    ) -> Vec<PointerValue<'ctx>> {
        let mut range_args: Vec<PointerValue<'ctx>> = Vec::new();
        let pairs = [(rsrc1, Some(coll)), (rsrc2, coll2)];
        for (rs, dest) in pairs
            .into_iter()
            .filter_map(|(rs, dest)| Some((rs?, dest?)))
        {
            self.core_call_owned(f, rs.call, &rs.args, dest);
            range_args.extend_from_slice(&rs.args);
        }
        range_args
    }

    fn lower_hof_loop(
        &mut self,
        f: &mut FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let acc = self.slot(f);
        if let Some(init) = hl.init {
            self.lower(f, init, acc)?;
        }
        let coll = self.slot(f);
        let (kind1, plan1, rsrc1, probe) = self.plan_first_coll(f, hl, base, coll)?;
        let mut coll2 = None;
        let mut plan2 = None;
        let mut rsrc2 = None;
        if let Some(c2) = hl.coll2 {
            let s = self.slot(f);
            let (k, p, r) = self.emit_coll_plan(f, c2, s)?;
            coll2 = Some(s);
            plan2 = Some((k, p));
            rsrc2 = r;
        }
        let done = self.ctx.append_basic_block(f.func, "loop_done");
        self.emit_char_filter_probe(f, hl, base, coll, dest, done);
        if let Some(mode) = probe {
            self.emit_parse_num_probe(f, coll, mode, dest, done);
        }
        let direct = self.ctx.append_basic_block(f.func, "loop_direct");
        let general = self.ctx.append_basic_block(f.func, "loop_general");
        let mut is_direct = self
            .builder
            .build_int_compare(IntPredicate::NE, kind1, self.i8t.const_zero(), "direct")
            .expect("cmp");
        if let Some((k2, _)) = plan2 {
            let d2 = self
                .builder
                .build_int_compare(IntPredicate::NE, k2, self.i8t.const_zero(), "direct2")
                .expect("cmp");
            is_direct = self
                .builder
                .build_and(is_direct, d2, "both_direct")
                .expect("and");
        }
        self.builder
            .build_conditional_branch(is_direct, direct, general)
            .expect("br");
        self.builder.position_at_end(direct);
        let (state, state2) = self.load_hof_states(acc, coll, (kind1, plan1), coll2, plan2);
        self.try_lower_scalar_hof(f, hl, base, acc, &state, state2.as_ref(), dest, done)?;
        self.init_hof_acc(f.func, hl.kind, hl.init.is_some(), acc, state.n);
        self.emit_hof_direct(f, hl, base, &state, state2.as_ref(), dest)?;
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(general);
        let range_args =
            self.materialize_range_srcs(f, rsrc1.as_ref(), rsrc2.as_ref(), coll, coll2);
        self.emit_hof_general(f, hl, base, acc, coll, coll2, dest)?;
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        self.free_slots(f, &[coll, acc]);
        if let Some(c2) = coll2 {
            self.free_slots(f, &[c2]);
        }
        self.free_slots(f, &range_args);
        Ok(())
    }

    fn init_hof_acc(
        &self,
        func: FunctionValue<'ctx>,
        kind: HofKind,
        has_init: bool,
        acc: PointerValue<'ctx>,
        cap: IntValue<'ctx>,
    ) {
        match kind {
            HofKind::Collect => {
                self.call(
                    "rt_n_vec_new",
                    None,
                    &[acc.into(), cap.into(), self.i8t.const_zero().into()],
                );
            }
            HofKind::FilterCollect => {
                self.call(
                    "rt_n_vec_new",
                    None,
                    &[
                        acc.into(),
                        self.i64t.const_zero().into(),
                        self.i8t.const_zero().into(),
                    ],
                );
            }
            HofKind::CountFilter => {
                self.write_int(func, acc, self.i64t.const_zero());
            }
            HofKind::Sum if !has_init => {
                self.write_int(func, acc, self.i64t.const_zero());
            }
            HofKind::Some => self.write_tag(acc, 0),
            HofKind::Every => {
                self.write_bool(func, acc, self.ctx.bool_type().const_int(1, false));
            }
            HofKind::Each | HofKind::Sum | HofKind::Reduce => {}
        }
    }

    /// The inline loop of [`try_lower_hof_loop`] over a vector (items by
    /// index) or a long range (items by arithmetic): binds the params,
    /// lowers the body with its own moves (a param or a body local may be
    /// moved; an enclosing local is read afresh every item), and folds the
    /// result into the accumulator. Leaves the loop's value in `dest`.
    fn emit_hof_direct(
        &mut self,
        f: &mut FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
        st: &LoopState<'ctx>,
        st2: Option<&LoopState<'ctx>>,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let i_ptr = self.entry_alloca(f, self.i64t, "loop_i");
        let cur_ptr = self.entry_alloca(f, self.i64t, "loop_cur");
        self.builder
            .build_store(i_ptr, self.i64t.const_zero())
            .expect("store");
        self.builder.build_store(cur_ptr, st.start).expect("store");
        let cur2_ptr = st2.map(|s2| {
            let p = self.entry_alloca(f, self.i64t, "loop_cur2");
            self.builder.build_store(p, s2.start).expect("store");
            p
        });
        let head = self.ctx.append_basic_block(f.func, "loop_head");
        let body = self.ctx.append_basic_block(f.func, "loop_body");
        let exit = self.ctx.append_basic_block(f.func, "loop_exit");
        self.builder.build_unconditional_branch(head).expect("br");
        self.builder.position_at_end(head);
        let i = self
            .builder
            .build_load(self.i64t, i_ptr, "i")
            .expect("load")
            .into_int_value();
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, st.n, "more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body, exit)
            .expect("br");
        self.builder.position_at_end(body);
        let item = self.slot(f);
        self.emit_loop_item(f, st, item, i, cur_ptr);
        let mut items = vec![item];
        if let (Some(s2), Some(c2p)) = (st2, cur2_ptr) {
            let item2 = self.slot(f);
            self.emit_loop_item(f, s2, item2, i, c2p);
            items.push(item2);
        }
        let next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "i_next")
            .expect("add");
        self.builder.build_store(i_ptr, next).expect("store");
        let params = match hl.kind {
            HofKind::Reduce => vec![st.acc, item],
            _ => items.clone(),
        };
        let res = self.slot(f);
        self.lower_inline_body(
            f,
            hl.arity,
            base,
            &params,
            hl.kind != HofKind::FilterCollect,
            res,
        )?;
        let first_sum = (hl.kind == HofKind::Sum && hl.init.is_none()).then_some(i);
        self.emit_hof_fold(f, hl.kind, st.acc, &items, res, first_sum, head, exit);
        self.builder.position_at_end(exit);
        f.free_slots.extend_from_slice(&items);
        f.free_slots.push(res);
        match hl.kind {
            HofKind::Each => self.write_nil(f.func, dest),
            HofKind::Collect | HofKind::FilterCollect => {
                self.call("rt_n_vec_done", None, &[dest.into(), st.acc.into()]);
            }
            HofKind::CountFilter
            | HofKind::Some
            | HofKind::Every
            | HofKind::Sum
            | HofKind::Reduce => {
                self.move_v(f.func, dest, st.acc);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_hof_fold(
        &self,
        f: &FnCtx<'ctx>,
        kind: HofKind,
        acc: PointerValue<'ctx>,
        items: &[PointerValue<'ctx>],
        res: PointerValue<'ctx>,
        first_sum: Option<IntValue<'ctx>>,
        head: BasicBlock<'ctx>,
        exit: BasicBlock<'ctx>,
    ) {
        let func = f.func;
        let item = items[0];
        match kind {
            HofKind::Each => {
                self.drop_and_nil_slots(func, &[res]);
                self.drop_and_nil_slots(func, items);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::Collect => {
                self.call("rt_n_vec_push", None, &[acc.into(), res.into()]);
                self.drop_and_nil_slots(func, items);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::FilterCollect => {
                let ok = self.truthy(res);
                self.drop_and_nil_slots(func, &[res]);
                let keep_b = self.ctx.append_basic_block(func, "filter_keep");
                let next_b = self.ctx.append_basic_block(func, "filter_next");
                self.builder
                    .build_conditional_branch(ok, keep_b, next_b)
                    .expect("br");
                self.builder.position_at_end(keep_b);
                self.call("rt_n_vec_push", None, &[acc.into(), item.into()]);
                self.builder.build_unconditional_branch(next_b).expect("br");
                self.builder.position_at_end(next_b);
                self.drop_and_nil_slots(func, &[item]);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::CountFilter => {
                let ok = self.truthy(res);
                let inc = self
                    .builder
                    .build_int_z_extend(ok, self.i64t, "inc")
                    .expect("zext");
                let cur = self.int_payload(acc);
                let next_c = self.builder.build_int_add(cur, inc, "cnt").expect("add");
                self.builder
                    .build_store(self.byte_at(acc, 8), next_c)
                    .expect("store");
                self.drop_and_nil_slots(func, &[res, item]);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::Some => {
                let ok = self.truthy(res);
                self.drop_and_nil_slots(func, &[item]);
                let hit_b = self.ctx.append_basic_block(func, "some_hit");
                let miss_b = self.ctx.append_basic_block(func, "some_miss");
                self.builder
                    .build_conditional_branch(ok, hit_b, miss_b)
                    .expect("br");
                self.builder.position_at_end(hit_b);
                self.move_v(func, acc, res);
                self.builder.build_unconditional_branch(exit).expect("br");
                self.builder.position_at_end(miss_b);
                self.drop_and_nil_slots(func, &[res]);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::Every => {
                let ok = self.truthy(res);
                self.drop_and_nil_slots(func, &[res, item]);
                let fail_b = self.ctx.append_basic_block(func, "every_fail");
                self.builder
                    .build_conditional_branch(ok, head, fail_b)
                    .expect("br");
                self.builder.position_at_end(fail_b);
                self.write_bool(func, acc, self.ctx.bool_type().const_zero());
                self.builder.build_unconditional_branch(exit).expect("br");
            }
            HofKind::Sum => {
                self.drop_and_nil_slots(func, items);
                if let Some(i) = first_sum {
                    let is_first = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, i, self.i64t.const_zero(), "sum_first")
                        .expect("cmp");
                    let set_b = self.ctx.append_basic_block(func, "sum_set");
                    let add_b = self.ctx.append_basic_block(func, "sum_add");
                    self.builder
                        .build_conditional_branch(is_first, set_b, add_b)
                        .expect("br");
                    self.builder.position_at_end(set_b);
                    self.move_v(func, acc, res);
                    self.builder.build_unconditional_branch(head).expect("br");
                    self.builder.position_at_end(add_b);
                }
                let st = self
                    .call("rt_n_add_acc", Some(self.i32t), &[acc.into(), res.into()])
                    .expect("st");
                self.check(f, st);
                self.builder.build_unconditional_branch(head).expect("br");
            }
            HofKind::Reduce => {
                self.move_v(func, acc, res);
                self.drop_and_nil_slots(func, &[item]);
                let stop = self
                    .call("rt_n_unreduce", Some(self.i8t), &[acc.into()])
                    .expect("stop");
                let stopped = self
                    .builder
                    .build_int_compare(IntPredicate::NE, stop, self.i8t.const_zero(), "stopped")
                    .expect("cmp");
                self.builder
                    .build_conditional_branch(stopped, exit, head)
                    .expect("br");
            }
        }
    }

    /// `item =` the loop's `i`th item: a vector's element (by index), or a
    /// long range's current value (then advanced by its step; wrapping, so
    /// the never-used value after the last cannot trap).
    fn emit_loop_item(
        &mut self,
        f: &FnCtx<'ctx>,
        st: &LoopState<'ctx>,
        item: PointerValue<'ctx>,
        i: IntValue<'ctx>,
        cur_ptr: PointerValue<'ctx>,
    ) {
        let vec_b = self.ctx.append_basic_block(f.func, "loop_vec_item");
        let range_b = self.ctx.append_basic_block(f.func, "loop_range_item");
        let got = self.ctx.append_basic_block(f.func, "loop_item");
        let is_vec = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                st.kind,
                self.i8t.const_int(1, false),
                "is_vec",
            )
            .expect("cmp");
        self.builder
            .build_conditional_branch(is_vec, vec_b, range_b)
            .expect("br");
        self.builder.position_at_end(vec_b);
        self.call(
            "rt_n_vec_item",
            None,
            &[item.into(), st.coll.into(), i.into()],
        );
        self.builder.build_unconditional_branch(got).expect("br");
        self.builder.position_at_end(range_b);
        let cur = self
            .builder
            .build_load(self.i64t, cur_ptr, "cur")
            .expect("load")
            .into_int_value();
        self.write_int(f.func, item, cur);
        let after = self
            .builder
            .build_int_add(cur, st.step, "cur_next")
            .expect("add");
        self.builder.build_store(cur_ptr, after).expect("store");
        self.builder.build_unconditional_branch(got).expect("br");
        self.builder.position_at_end(got);
    }

    /// Lower a literal `fn` arity's body in place, its params bound to
    /// `params` like a `let` (at env index `base` onward, where the
    /// analyzer placed them) and the body's own last uses as its moves.
    fn lower_inline_body(
        &mut self,
        f: &mut FnCtx<'ctx>,
        arity: &Arity,
        base: usize,
        params: &[PointerValue<'ctx>],
        own_params: bool,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        for p in params {
            f.env.push(Some(LocalRef::Slot(*p)));
        }
        let owned: Vec<usize> = if own_params {
            (base..base + params.len()).collect()
        } else {
            Vec::new()
        };
        let moves = collect_body_moves(&arity.body, base, &f.env, &owned);
        let saved = std::mem::replace(&mut f.moves, moves);
        let lowered = self.lower_body(f, &arity.body, dest);
        f.moves = saved;
        f.env.truncate(base);
        lowered
    }

    /// The general path of [`try_lower_hof_loop`]: the `fn` as a closure,
    /// then the original core calls (`map` then `dorun` / `vec` / `into`,
    /// or `mapv` / `reduce`) over the evaluated collection.
    #[allow(clippy::too_many_arguments)]
    fn emit_hof_general(
        &mut self,
        f: &mut FnCtx<'ctx>,
        hl: &HofLoop<'_>,
        base: usize,
        acc: PointerValue<'ctx>,
        coll: PointerValue<'ctx>,
        coll2: Option<PointerValue<'ctx>>,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let closure = self.slot(f);
        self.make_closure(f, hl.fun, base, closure)?;
        let mut args = vec![closure];
        if hl.kind == HofKind::Reduce {
            args.push(acc);
        }
        args.push(coll);
        if let Some(c2) = coll2 {
            args.push(c2);
        }
        let Some((outer, prefix)) = hl.outer else {
            self.core_call_owned(f, hl.inner, &args, dest);
            return Ok(());
        };
        let seq = self.slot(f);
        self.core_call_owned(f, hl.inner, &args, seq);
        let mut outer_args = Vec::new();
        if let Some(p) = prefix {
            let s = self.slot(f);
            self.lower(f, p, s)?;
            outer_args.push(s);
        }
        if hl.kind == HofKind::Sum && hl.init.is_some() {
            outer_args.push(acc);
        }
        outer_args.push(seq);
        self.core_call_owned(f, outer, &outer_args, dest);
        Ok(())
    }

    /// `dest = (var args…)` through the core var's native, the `args`
    /// slots moved over (left nil), as the original call took its
    /// temporaries: a `map` seq reaches `dorun` / `vec` uniquely held, so
    /// the walk can fuse it (`walk_sole_map`).
    fn core_call_owned(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        args: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
    ) {
        let k = self.var_of(&call.var);
        let n = args.len();
        let (arr, elems) = self.slot_array(f, n);
        for (e, a) in elems.iter().zip(args) {
            self.move_v(f.func, *e, *a);
        }
        let name = call.var.sym.name();
        let op = native_abi::own_op(name, n)
            .or_else(|| native_abi::inline_op(name, n))
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
        self.free_slot_array(f, arr, elems);
    }

    fn lower_direct_with(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        bc_override: Option<&'static str>,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if !self.inline_arith {
            return self.lower_direct_owned(f, call, dest);
        }
        let k = self.var_of(&call.var);
        let n = call.args.len();
        let fast = fast_op(call.fast, n);
        let ns = call.var.sym.namespace();
        let name = call.var.sym.name();
        let core_name = (ns == Some("clojure.core")).then(|| name.to_owned());
        let bc_sym = bc_override.or_else(|| {
            (fast.is_none() && self.use_bitcode)
                .then(|| clj_bc::bc_ns_op_symbol(ns, name, n))
                .flatten()
        });
        if bc_sym.is_none() && fast.is_none() && is_direct_moving(f, call, core_name.as_deref()) {
            return self.lower_direct_moving(f, call, k, core_name.as_deref(), dest);
        }
        let (refs, temps, moved) = self.arg_refs(f, &call.args, dest)?;
        let slow = self.ctx.append_basic_block(f.func, "native_slow");
        let done = self.ctx.append_basic_block(f.func, "native_done");
        self.emit_direct_inline(
            f,
            fast,
            core_name.as_deref(),
            bc_sym,
            &refs,
            dest,
            slow,
            done,
        );
        if core_name.as_deref() == Some("count") && n == 1 && (temps.len() + moved.len() == 1) {
            self.emit_owned_slow1(f, refs[0], k, dest, slow, done);
        } else {
            self.emit_direct_slow(f, &refs, k, n, core_name.as_deref(), dest, slow, done);
        }
        self.free_slots(f, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        Ok(())
    }

    fn emit_owned_slow1(
        &mut self,
        f: &FnCtx<'ctx>,
        arg: PointerValue<'ctx>,
        k: u64,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(slow);
        let aref = self.slots_ref(f, arg, 1);
        let st = self
            .call(
                "rt_n_core_own",
                Some(self.i32t),
                &[
                    dest.into(),
                    self.i32t.const_int(u64::from(u32::MAX), false).into(),
                    self.u64c(k),
                    aref.into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    /// The fast path of a direct-native call: an intrinsic, `FastArith`,
    /// or bitcode helper op that falls through to `done`, else a branch to `slow`.
    #[allow(clippy::too_many_arguments)]
    fn emit_direct_inline(
        &mut self,
        f: &mut FnCtx<'ctx>,
        fast: Option<clj_eval::ext::FastArith>,
        core_name: Option<&str>,
        bc_override: Option<&'static str>,
        refs: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let bc_sym = if bc_override.is_some() {
            bc_override
        } else if fast.is_none() && self.use_bitcode {
            core_name.and_then(|name| clj_bc::bc_op_symbol(name, refs.len()))
        } else {
            None
        };
        let bc_block = bc_sym.map(|_| self.ctx.append_basic_block(f.func, "bc_try"));
        let miss = bc_block.unwrap_or(slow);
        let inline = match (fast, core_name) {
            (None, Some(name)) => self.lower_intrinsic(f, name, refs, dest, miss, done),
            _ => false,
        };
        if !inline {
            match fast {
                Some(op) => self.lower_fast_arith(f, op, refs, dest, slow, done),
                None => {
                    self.builder.build_unconditional_branch(miss).expect("br");
                }
            }
        }
        if let (Some(sym), Some(bb)) = (bc_sym, bc_block) {
            self.emit_bc_op(f.func, bb, sym, refs, dest, slow, done);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_bc_op(
        &self,
        func: FunctionValue<'ctx>,
        bb: BasicBlock<'ctx>,
        shim: &str,
        refs: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(bb);
        self.drop_v(func, dest);
        let mut call_args: Vec<BasicMetadataValueEnum> = vec![dest.into()];
        call_args.extend(refs.iter().map(|r| BasicMetadataValueEnum::from(*r)));
        if matches!(
            shim,
            "rt_bc_1_count" | "rt_bc_2_nth" | "rt_bc_2_get" | "rt_bc_3_nth" | "rt_bc_3_get"
        ) {
            let probe = self.helper("rt_n_str_ascii", Some(self.i8t.into()), &[self.ptr.into()]);
            call_args.push(probe.as_global_value().as_pointer_value().into());
        }
        let ok = self.call(shim, Some(self.i8t), &call_args).expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done, slow)
            .expect("br");
    }

    /// The slow path of a direct-native call: the VM's inline op over the
    /// borrowed args, else the native (args cloned only there). 1-3 args
    /// go as direct operands; more through a slice.
    #[allow(clippy::too_many_arguments)]
    fn emit_direct_slow(
        &mut self,
        f: &mut FnCtx<'ctx>,
        refs: &[PointerValue<'ctx>],
        k: u64,
        n: usize,
        core_name: Option<&str>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        self.builder.position_at_end(slow);
        let op = core_name
            .and_then(|name| native_abi::inline_op(name, n))
            .unwrap_or(u32::MAX);
        let st = if op == u32::MAX {
            let s = self.refs_slice(f, refs);
            self.call(
                "rt_n_core_call",
                Some(self.i32t),
                &[dest.into(), self.u64c(k), s.into()],
            )
            .expect("st")
        } else if native_abi::is_re_op(op) && refs.len() == 2 {
            self.call(
                "rt_n_op2_re",
                Some(self.i32t),
                &[dest.into(), self.u64c(k), refs[0].into(), refs[1].into()],
            )
            .expect("st")
        } else {
            let mut call_args: Vec<BasicMetadataValueEnum> = vec![
                dest.into(),
                self.i32t.const_int(u64::from(op), false).into(),
                self.u64c(k),
            ];
            let shim = match refs.len() {
                1 => "rt_n_op1",
                2 => "rt_n_op2",
                3 => "rt_n_op3",
                _ => "rt_n_core_ref",
            };
            if (1..=3).contains(&refs.len()) {
                call_args.extend(refs.iter().map(|r| BasicMetadataValueEnum::from(*r)));
            } else {
                call_args.push(self.refs_slice(f, refs).into());
            }
            self.call(shim, Some(self.i32t), &call_args).expect("st")
        };
        self.check(f, st);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
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
        let (op_name, arg_ptrs): (Option<&str>, Vec<&Expr>) =
            match (core_name, call.args.as_slice()) {
                (Some("assoc-in"), [m, Expr::VecLit(ks, None), vf]) if ks.len() == 2 => {
                    (Some("assoc-in2"), vec![m, &ks[0], &ks[1], vf])
                }
                (Some("update-in"), [m, Expr::VecLit(ks, None), vf]) if ks.len() == 2 => {
                    (Some("update-in2"), vec![m, &ks[0], &ks[1], vf])
                }
                (Some("swap!"), [atom, fe, Expr::VecLit(ks, None), v])
                    if ks.len() == 2 && is_core_global(fe, "assoc-in") =>
                {
                    (Some("swap-assoc-in2!"), vec![atom, fe, &ks[0], &ks[1], v])
                }
                _ => (core_name, call.args.iter().collect()),
            };
        self.lower_own_call(f, k, op_name, &arg_ptrs, dest)
    }

    fn lower_own_call(
        &mut self,
        f: &mut FnCtx<'ctx>,
        k: u64,
        op_name: Option<&str>,
        arg_ptrs: &[&Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let n = arg_ptrs.len();
        let (arr, elems) = self.slot_array(f, n);
        for (&a, e) in arg_ptrs.iter().zip(&elems) {
            let moved = match a {
                Expr::Local(i) if is_move(f, a) => self.slot_of(f, *i),
                _ => None,
            };
            if let Some(src) = moved {
                self.move_v(f.func, *e, src);
            } else {
                self.lower(f, a, *e)?;
            }
        }
        let op = op_name
            .and_then(|name| native_abi::own_op(name, n).or_else(|| native_abi::inline_op(name, n)))
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
        self.free_slot_array(f, arr, elems);
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
    /// fresh slot holding the lowered argument. Returns the pointers,
    /// the fresh slots (`temps`), and any borrowed local slots at their
    /// last use (`moved`) to release right after the call completes.
    #[allow(clippy::type_complexity)]
    fn arg_refs(
        &mut self,
        f: &mut FnCtx<'ctx>,
        args: &[Expr],
        dest: PointerValue<'ctx>,
    ) -> Result<
        (
            Vec<PointerValue<'ctx>>,
            Vec<PointerValue<'ctx>>,
            Vec<PointerValue<'ctx>>,
        ),
        Unsupported,
    > {
        let mut refs = Vec::with_capacity(args.len());
        let mut temps = Vec::new();
        let mut moved = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let local = match a {
                Expr::Local(idx) => self.borrowable_before(f, *idx, &args[i + 1..]),
                _ => None,
            };
            if let Some(p) = local.filter(|p| *p != dest) {
                refs.push(p);
                if is_move(f, a) {
                    moved.push(p);
                }
            } else if let Some((p, fallback)) = self.borrowed(f, a)? {
                refs.push(p);
                temps.extend(fallback);
            } else {
                let t = self.slot(f);
                self.lower(f, a, t)?;
                refs.push(t);
                temps.push(t);
            }
        }
        Ok((refs, temps, moved))
    }

    /// A clone-free reference to `e`'s value for a call that only reads
    /// it (#271): a pooled constant in place in the frozen table, or a
    /// stable var's cached root (with the temporary a not-yet-filled
    /// cache derefs into, for the caller to drop after the call). `None`
    /// for anything else.
    #[allow(clippy::type_complexity)]
    fn borrowed(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
    ) -> Result<Option<(PointerValue<'ctx>, Option<PointerValue<'ctx>>)>, Unsupported> {
        match e {
            Expr::Literal(v) | Expr::Quote(v) if is_pooled(v) => {
                let k = self.value_const(v)?;
                Ok(Some((self.const_ref(k), None)))
            }
            Expr::Global(var) => Ok(self.stable_of(var).map(|sr| {
                let k = self.var_of(var);
                self.stable_ref(f, sr, k)
            })),
            _ => Ok(None),
        }
    }

    /// Pooled constant `k` in place: the frozen table's base plus `k`
    /// values.
    fn const_ref(&self, k: u64) -> PointerValue<'ctx> {
        let base = self
            .builder
            .build_load(self.ptr, self.consts_base, "consts")
            .expect("load")
            .into_pointer_value();
        let addr = self
            .builder
            .build_ptr_to_int(base, self.i64t, "cbase")
            .expect("p2i");
        let off = self
            .builder
            .build_int_mul(
                self.slot_ty.size_of().expect("sized"),
                self.i64t.const_int(k, false),
                "coff",
            )
            .expect("mul");
        let at = self.builder.build_int_add(addr, off, "caddr").expect("add");
        self.builder
            .build_int_to_ptr(at, self.ptr, "cref")
            .expect("i2p")
    }

    /// The stable root cache entry of `var`, if it has one.
    fn stable_of(&self, var: &clj_value::Var) -> Option<StableRoot<'ctx>> {
        let key = (
            var.sym.namespace().unwrap_or("user").to_owned(),
            var.sym.name().to_owned(),
        );
        self.stable.get(&key).copied()
    }

    /// Whether `sr`'s cache must be checked here: only code in the
    /// `def`'s own unit or an earlier one can run before the `def`.
    fn stable_unchecked(&self, sr: StableRoot<'ctx>) -> bool {
        self.unit > sr.unit
    }

    /// `sr`'s set flag (acquire: the root's bytes are visible once set).
    fn stable_is_set(&self, sr: StableRoot<'ctx>) -> IntValue<'ctx> {
        let flag = self
            .builder
            .build_load(self.i8t, sr.set, "rset")
            .expect("load");
        flag.as_instruction_value()
            .expect("load instr")
            .set_atomic_ordering(AtomicOrdering::Acquire)
            .expect("acquire");
        flag.as_instruction_value()
            .expect("load instr")
            .set_alignment(1)
            .expect("align");
        self.builder
            .build_int_compare(
                IntPredicate::NE,
                flag.into_int_value(),
                self.i8t.const_zero(),
                "is_set",
            )
            .expect("cmp")
    }

    /// A reference to stable var `k`'s root: the cache itself where it is
    /// surely filled, else the cache once set and a fresh deref
    /// (returned for dropping) before.
    fn stable_ref(
        &mut self,
        f: &mut FnCtx<'ctx>,
        sr: StableRoot<'ctx>,
        k: u64,
    ) -> (PointerValue<'ctx>, Option<PointerValue<'ctx>>) {
        if self.stable_unchecked(sr) {
            return (sr.root, None);
        }
        let t = self.slot(f);
        let set = self.stable_is_set(sr);
        let here = self.builder.get_insert_block().expect("positioned");
        let slow = self.ctx.append_basic_block(f.func, "stable_slow");
        let done = self.ctx.append_basic_block(f.func, "stable_done");
        self.builder
            .build_conditional_branch(set, done, slow)
            .expect("br");
        self.builder.position_at_end(slow);
        self.emit_deref(f.func, t, k);
        let slow_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
        let phi = self.builder.build_phi(self.ptr, "sref").expect("phi");
        phi.add_incoming(&[(&sr.root, here), (&t, slow_end)]);
        (phi.as_basic_value().into_pointer_value(), Some(t))
    }

    /// `dest =` a clone of stable var `k`'s root (the var itself only
    /// while the cache may still be unfilled).
    fn clone_stable(
        &mut self,
        f: &mut FnCtx<'ctx>,
        sr: StableRoot<'ctx>,
        k: u64,
        dest: PointerValue<'ctx>,
    ) {
        if self.stable_unchecked(sr) {
            self.clone_v(f.func, dest, sr.root);
            return;
        }
        let set = self.stable_is_set(sr);
        let hit = self.ctx.append_basic_block(f.func, "stable_hit");
        let miss = self.ctx.append_basic_block(f.func, "stable_miss");
        let done = self.ctx.append_basic_block(f.func, "stable_done");
        self.builder
            .build_conditional_branch(set, hit, miss)
            .expect("br");
        self.builder.position_at_end(hit);
        self.clone_v(f.func, dest, sr.root);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(miss);
        self.emit_deref(f.func, dest, k);
        self.builder.build_unconditional_branch(done).expect("br");
        self.builder.position_at_end(done);
    }

    /// The slot holding local `idx`'s value, when it can be borrowed
    /// safely across subsequent call arguments `rest`.
    fn borrowable_before(
        &self,
        f: &FnCtx<'ctx>,
        idx: usize,
        rest: &[Expr],
    ) -> Option<PointerValue<'ctx>> {
        // A local some occurrence in the remaining arguments of the same
        // call moves is copied instead: moving later in arg evaluation
        // would overwrite or invalidate this borrowed slot.
        if rest.iter().any(|e| expr_moves_local(e, idx, &f.moves)) {
            return None;
        }
        self.slot_of(f, idx)
    }

    /// The slot holding local `idx`'s value (not a `letfn` cell).
    fn slot_of(&self, f: &FnCtx<'ctx>, idx: usize) -> Option<PointerValue<'ctx>> {
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

    /// The analyzer's `FastArith` op inline: variadic folds over 2..=8 args
    /// step pairwise through a scratch POD slot so mixed int/double or late
    /// overflow falls back cleanly to `slow` with original `elems` intact.
    fn lower_fast_arith(
        &mut self,
        f: &mut FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        elems: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        if matches!(
            op,
            FastArith::Lt | FastArith::Lte | FastArith::Gt | FastArith::Gte
        ) {
            self.lower_cmp_arith(f, op, elems, &BoolSink::Slot { dest, done }, slow);
            return;
        }
        if elems.len() <= 2 {
            self.lower_fast_arith_step(f, op, elems[0], elems.get(1).copied(), dest, slow, done);
            return;
        }
        let acc = self.slot(f);
        for i in 1..elems.len() {
            let lhs = if i == 1 { elems[0] } else { acc };
            let last = i + 1 == elems.len();
            let out = if last { dest } else { acc };
            let step_done = if last {
                done
            } else {
                self.ctx.append_basic_block(f.func, "arith_step")
            };
            self.lower_fast_arith_step(f, op, lhs, Some(elems[i]), out, slow, step_done);
            if !last {
                self.builder.position_at_end(step_done);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_fast_arith_step(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        lhs: PointerValue<'ctx>,
        rhs: Option<PointerValue<'ctx>>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        let ints = self.both_ints(lhs, rhs);
        let fastb = self.ctx.append_basic_block(f.func, "native_fast");
        let dbl_check = self.ctx.append_basic_block(f.func, "native_dbl_check");
        self.builder
            .build_conditional_branch(ints, fastb, dbl_check)
            .expect("br");
        // Doubles (and int/double mixes): f64 math like the native.
        self.builder.position_at_end(dbl_check);
        self.lower_double_arith(f, op, lhs, rhs, dest, slow, done);
        self.builder.position_at_end(fastb);
        let a = self.int_payload(lhs);
        let b = match rhs {
            Some(r) => self.int_payload(r),
            None => self.i64t.const_int(1, false),
        };
        if op == FastArith::Div {
            self.lower_int_div_exact(f, a, b, dest, slow, done);
            return;
        }
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

    fn lower_int_div_exact(
        &self,
        f: &FnCtx<'ctx>,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let bb = &self.builder;
        let y0 = bb
            .build_int_compare(IntPredicate::EQ, y, self.i64t.const_zero(), "y0")
            .expect("cmp");
        let min_v = self.i64t.const_int(i64::MIN as u64, true);
        let xmin = bb
            .build_int_compare(IntPredicate::EQ, x, min_v, "xmin")
            .expect("cmp");
        let ymin = bb
            .build_int_compare(IntPredicate::EQ, y, min_v, "ymin")
            .expect("cmp");
        let edge = bb
            .build_or(
                y0,
                bb.build_or(xmin, ymin, "anymin").expect("or"),
                "div_edge",
            )
            .expect("or");
        let safe_bb = self.ctx.append_basic_block(f.func, "div_safe");
        bb.build_conditional_branch(edge, slow, safe_bb)
            .expect("br");
        self.builder.position_at_end(safe_bb);
        let bb = &self.builder;
        let rem = bb.build_int_signed_rem(x, y, "rem").expect("rem");
        let exact = bb
            .build_int_compare(IntPredicate::EQ, rem, self.i64t.const_zero(), "exact")
            .expect("cmp");
        let exact_bb = self.ctx.append_basic_block(f.func, "div_exact");
        bb.build_conditional_branch(exact, exact_bb, slow)
            .expect("br");
        self.builder.position_at_end(exact_bb);
        let q = self.builder.build_int_signed_div(x, y, "q").expect("div");
        self.write_int(f.func, dest, q);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn emit_int_cmp(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        a: IntValue<'ctx>,
        b: IntValue<'ctx>,
        sink: &BoolSink<'ctx>,
    ) {
        use clj_eval::ext::FastArith;
        let pred = match op {
            FastArith::Lt => IntPredicate::SLT,
            FastArith::Lte => IntPredicate::SLE,
            FastArith::Gt => IntPredicate::SGT,
            _ => IntPredicate::SGE,
        };
        let r = self
            .builder
            .build_int_compare(pred, a, b, "cmp")
            .expect("cmp");
        self.finish_bool(f.func, sink, r);
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
            "urem" => {
                let nz = self
                    .builder
                    .build_int_compare(IntPredicate::NE, y, self.i64t.const_zero(), "nz")
                    .expect("cmp");
                let go = self.ctx.append_basic_block(f.func, "urem_ok");
                self.builder
                    .build_conditional_branch(nz, go, slow)
                    .expect("br");
                self.builder.position_at_end(go);
                self.builder
                    .build_int_unsigned_rem(x, y, "urem")
                    .expect("urem")
            }
            "unchecked-add" => self.builder.build_int_add(x, y, "uadd").expect("add"),
            "unchecked-subtract" => self.builder.build_int_sub(x, y, "usub").expect("sub"),
            "unchecked-multiply" => self.builder.build_int_mul(x, y, "umul").expect("mul"),
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
    fn finish_bool(&self, func: FunctionValue<'ctx>, sink: &BoolSink<'ctx>, bit: IntValue<'ctx>) {
        let cur = self.builder.get_insert_block().expect("bb");
        match sink {
            BoolSink::Slot { dest, done } => {
                self.write_bool(func, *dest, bit);
                self.builder.build_unconditional_branch(*done).expect("br");
            }
            BoolSink::Jump { join, phi } => {
                phi.add_incoming(&[(&bit, cur)]);
                self.builder.build_unconditional_branch(*join).expect("br");
            }
        }
    }

    /// Inline predicate intrinsics (`nil?`, `some?`, `not`, `=`, `not=`, `==`,
    /// `zero?`, `pos?`, `neg?`, `even?`, `odd?`), emitting directly into `sink`.
    fn lower_pred_intrinsic(
        &self,
        f: &FnCtx<'ctx>,
        name: &str,
        a: &[PointerValue<'ctx>],
        sink: &BoolSink<'ctx>,
        slow: BasicBlock<'ctx>,
    ) -> bool {
        match (name, a.len()) {
            ("nil?" | "some?" | "not", 1) => {
                let r = self.any_pred(name, a[0]);
                self.finish_bool(f.func, sink, r);
                true
            }
            ("=" | "not=", 2) => {
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
                self.finish_bool(f.func, sink, r);
                self.builder.position_at_end(other);
                self.when_tag(f, a, TAG_CHAR, slow);
                let (x, y) = (self.char_payload(a[0]), self.char_payload(a[1]));
                let r = self
                    .builder
                    .build_int_compare(pred, x, y, "ceq")
                    .expect("cmp");
                self.finish_bool(f.func, sink, r);
                true
            }
            ("==", 2) => {
                self.when_ints(f, a, slow);
                let (x, y) = (self.int_payload(a[0]), self.int_payload(a[1]));
                let r = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, x, y, "eq")
                    .expect("cmp");
                self.finish_bool(f.func, sink, r);
                true
            }
            ("zero?" | "pos?" | "neg?" | "even?" | "odd?", 1) => {
                self.when_ints(f, a, slow);
                let r = self.int_pred(name, self.int_payload(a[0]));
                self.finish_bool(f.func, sink, r);
                true
            }
            _ => false,
        }
    }

    /// Lower a scalar-result comparison (`<`, `<=`, `>`, `>=`) into `sink` without
    /// boxing intermediate booleans when `sink` is a branch jump.
    fn lower_cmp_arith(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        elems: &[PointerValue<'ctx>],
        sink: &BoolSink<'ctx>,
        slow: BasicBlock<'ctx>,
    ) {
        if elems.len() == 2 {
            self.lower_cmp_pair(f, op, elems[0], elems[1], sink, slow);
            return;
        }
        let false_bb = self.ctx.append_basic_block(f.func, "cmp_false");
        for i in 0..elems.len() - 1 {
            if i + 2 == elems.len() {
                self.lower_cmp_pair(f, op, elems[i], elems[i + 1], sink, slow);
                break;
            }
            let here = self.builder.get_insert_block().expect("bb");
            let step_join = self.ctx.append_basic_block(f.func, "cmp_step");
            self.builder.position_at_end(step_join);
            let phi = self
                .builder
                .build_phi(self.ctx.bool_type(), "step_cmp")
                .expect("phi");
            self.builder.position_at_end(here);
            let next_bb = self.ctx.append_basic_block(f.func, "cmp_next");
            let step_sink = BoolSink::Jump {
                join: step_join,
                phi,
            };
            self.lower_cmp_pair(f, op, elems[i], elems[i + 1], &step_sink, slow);
            self.builder.position_at_end(step_join);
            self.builder
                .build_conditional_branch(phi.as_basic_value().into_int_value(), next_bb, false_bb)
                .expect("br");
            self.builder.position_at_end(next_bb);
        }
        let saved = self.builder.get_insert_block().expect("chain block");
        self.builder.position_at_end(false_bb);
        self.finish_bool(f.func, sink, self.ctx.bool_type().const_zero());
        self.builder.position_at_end(saved);
    }

    fn lower_cmp_pair(
        &self,
        f: &FnCtx<'ctx>,
        op: clj_eval::ext::FastArith,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        sink: &BoolSink<'ctx>,
        slow: BasicBlock<'ctx>,
    ) {
        let ints = self.both_ints(lhs, Some(rhs));
        let fastb = self.ctx.append_basic_block(f.func, "cmp_fast");
        let dbl_check = self.ctx.append_basic_block(f.func, "cmp_dbl_check");
        self.builder
            .build_conditional_branch(ints, fastb, dbl_check)
            .expect("br");
        // Doubles (and int/double mixes): f64 math like the native.
        self.builder.position_at_end(dbl_check);
        self.lower_double_cmp(f, op, lhs, rhs, sink, slow);
        self.builder.position_at_end(fastb);
        self.emit_int_cmp(f, op, self.int_payload(lhs), self.int_payload(rhs), sink);
    }

    fn lower_intrinsic(
        &mut self,
        f: &mut FnCtx<'ctx>,
        name: &str,
        a: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) -> bool {
        let sink = BoolSink::Slot { dest, done };
        if self.lower_pred_intrinsic(f, name, a, &sink, slow) {
            return true;
        }
        let n = a.len();
        match (name, n) {
            ("double", 1) => {
                self.when_numeric(f, a, slow);
                let d = self.num_as_f64(a[0]);
                self.write_double(f.func, dest, d);
            }
            ("int" | "long", 1) => {
                self.lower_int_long_cast(f, name == "long", a[0], dest, slow, done);
                return true;
            }
            ("char", 1) => {
                self.lower_char_cast(f, a[0], dest, slow, done);
                return true;
            }
            ("bit-not" | "unchecked-inc" | "unchecked-dec" | "unchecked-negate", 1) => {
                self.when_ints(f, a, slow);
                let x = self.int_payload(a[0]);
                let r = self.int_unop(name, x);
                self.write_int(f.func, dest, r);
            }
            ("max" | "min", 2..=8) => {
                self.lower_minmax(f, name == "max", a, dest, slow, done);
                return true;
            }
            (
                "quot" | "rem" | "mod" | "unchecked-add" | "unchecked-subtract"
                | "unchecked-multiply",
                2,
            )
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

    fn int_unop(&self, name: &str, x: IntValue<'ctx>) -> IntValue<'ctx> {
        let one = self.i64t.const_int(1, false);
        match name {
            "bit-not" => self.builder.build_not(x, "bnot").expect("not"),
            "unchecked-inc" => self.builder.build_int_add(x, one, "uinc").expect("add"),
            "unchecked-dec" => self.builder.build_int_sub(x, one, "udec").expect("sub"),
            _ => self
                .builder
                .build_int_sub(self.i64t.const_zero(), x, "uneg")
                .expect("sub"),
        }
    }

    fn lower_int_long_cast(
        &self,
        f: &FnCtx<'ctx>,
        long: bool,
        arg: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let tag = self.tag(arg);
        let int_bb = self.ctx.append_basic_block(f.func, "cast_i");
        let dbl_bb = self.ctx.append_basic_block(f.func, "cast_d");
        let chr_bb = self.ctx.append_basic_block(f.func, "cast_c");
        self.builder
            .build_switch(
                tag,
                slow,
                &[
                    (self.i8t.const_int(TAG_INT, false), int_bb),
                    (self.i8t.const_int(3, false), dbl_bb),
                    (self.i8t.const_int(TAG_CHAR, false), chr_bb),
                ],
            )
            .expect("switch");
        self.builder.position_at_end(int_bb);
        let iv = self.int_payload(arg);
        if !long {
            let t32 = self
                .builder
                .build_int_truncate(iv, self.i32t, "t32")
                .expect("trunc");
            let s64 = self
                .builder
                .build_int_s_extend(t32, self.i64t, "s64")
                .expect("sext");
            let fits = self
                .builder
                .build_int_compare(IntPredicate::EQ, s64, iv, "fits")
                .expect("cmp");
            let ok_bb = self.ctx.append_basic_block(f.func, "cast_i_ok");
            self.builder
                .build_conditional_branch(fits, ok_bb, slow)
                .expect("br");
            self.builder.position_at_end(ok_bb);
        }
        self.write_int(f.func, dest, iv);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(dbl_bb);
        let d = self.double_payload(arg);
        let f64t = self.ctx.f64_type();
        let (lo, hi, hi_pred) = if long {
            (
                f64t.const_float(-9223372036854775808.0),
                f64t.const_float(9223372036854775808.0),
                inkwell::FloatPredicate::OLT,
            )
        } else {
            (
                f64t.const_float(f64::from(i32::MIN)),
                f64t.const_float(f64::from(i32::MAX)),
                inkwell::FloatPredicate::OLE,
            )
        };
        let ge = self
            .builder
            .build_float_compare(inkwell::FloatPredicate::OGE, d, lo, "dlo")
            .expect("fcmp");
        let lt = self
            .builder
            .build_float_compare(hi_pred, d, hi, "dhi")
            .expect("fcmp");
        let ok = self.builder.build_and(ge, lt, "dok").expect("and");
        let d_ok = self.ctx.append_basic_block(f.func, "cast_d_ok");
        self.builder
            .build_conditional_branch(ok, d_ok, slow)
            .expect("br");
        self.builder.position_at_end(d_ok);
        let cv = self
            .builder
            .build_float_to_signed_int(d, self.i64t, "f2i")
            .expect("fptosi");
        self.write_int(f.func, dest, cv);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(chr_bb);
        let c = self.char_payload(arg);
        let cp = self
            .builder
            .build_int_z_extend(c, self.i64t, "cp")
            .expect("zext");
        self.write_int(f.func, dest, cp);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn lower_char_cast(
        &self,
        f: &FnCtx<'ctx>,
        arg: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        let tag = self.tag(arg);
        let int_bb = self.ctx.append_basic_block(f.func, "ch_i");
        let chr_bb = self.ctx.append_basic_block(f.func, "ch_c");
        self.builder
            .build_switch(
                tag,
                slow,
                &[
                    (self.i8t.const_int(TAG_INT, false), int_bb),
                    (self.i8t.const_int(TAG_CHAR, false), chr_bb),
                ],
            )
            .expect("switch");
        self.builder.position_at_end(int_bb);
        let iv = self.int_payload(arg);
        let ok = self
            .builder
            .build_int_compare(
                IntPredicate::ULT,
                iv,
                self.i64t.const_int(0xD800, false),
                "bmp",
            )
            .expect("cmp");
        let ok_bb = self.ctx.append_basic_block(f.func, "ch_ok");
        self.builder
            .build_conditional_branch(ok, ok_bb, slow)
            .expect("br");
        self.builder.position_at_end(ok_bb);
        let cp = self
            .builder
            .build_int_truncate(iv, self.i32t, "cp32")
            .expect("trunc");
        self.write_char(f.func, dest, cp);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(chr_bb);
        let c = self.char_payload(arg);
        self.write_char(f.func, dest, c);
        self.builder.build_unconditional_branch(done).expect("br");
    }

    fn lower_minmax(
        &mut self,
        f: &mut FnCtx<'ctx>,
        is_max: bool,
        elems: &[PointerValue<'ctx>],
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        if elems.len() == 2 {
            self.lower_minmax_step(f, is_max, elems[0], elems[1], dest, slow, done);
            return;
        }
        let acc = self.slot(f);
        for i in 1..elems.len() {
            let lhs = if i == 1 { elems[0] } else { acc };
            let last = i + 1 == elems.len();
            let out = if last { dest } else { acc };
            let step_done = if last {
                done
            } else {
                self.ctx.append_basic_block(f.func, "mm_step")
            };
            self.lower_minmax_step(f, is_max, lhs, elems[i], out, slow, step_done);
            if !last {
                self.builder.position_at_end(step_done);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_minmax_step(
        &self,
        f: &FnCtx<'ctx>,
        is_max: bool,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        dest: PointerValue<'ctx>,
        slow: BasicBlock<'ctx>,
        done: BasicBlock<'ctx>,
    ) {
        use inkwell::FloatPredicate;
        self.when_numeric(f, &[lhs, rhs], slow);
        let ints = self.both_ints(lhs, Some(rhs));
        let int_bb = self.ctx.append_basic_block(f.func, "mm_int");
        let dbl_bb = self.ctx.append_basic_block(f.func, "mm_dbl");
        self.builder
            .build_conditional_branch(ints, int_bb, dbl_bb)
            .expect("br");

        self.builder.position_at_end(int_bb);
        let (ix, iy) = (self.int_payload(lhs), self.int_payload(rhs));
        let pred = if is_max {
            IntPredicate::SGT
        } else {
            IntPredicate::SLT
        };
        let ipick = self
            .builder
            .build_int_compare(pred, ix, iy, "ipick")
            .expect("cmp");
        let iwin = self
            .builder
            .build_select(ipick, ix, iy, "iwin")
            .expect("sel")
            .into_int_value();
        self.write_int(f.func, dest, iwin);
        self.builder.build_unconditional_branch(done).expect("br");

        self.builder.position_at_end(dbl_bb);
        let (fx, fy) = (self.num_as_f64(lhs), self.num_as_f64(rhs));
        // Mirror core::minmax: strict winner keeps incumbent, NaN incumbent wins.
        let x_nan = self
            .builder
            .build_float_compare(FloatPredicate::UNO, fx, fx, "xnan")
            .expect("fcmp");
        let fpred = if is_max {
            FloatPredicate::OGT
        } else {
            FloatPredicate::OLT
        };
        let fcmp = self
            .builder
            .build_float_compare(fpred, fx, fy, "fcmp")
            .expect("fcmp");
        let fpick = self.builder.build_or(x_nan, fcmp, "fpick").expect("or");
        let win_ptr = self
            .builder
            .build_select(fpick, lhs, rhs, "winp")
            .expect("sel")
            .into_pointer_value();
        let win_bits = self
            .builder
            .build_load(self.slot_ty, win_ptr, "winv")
            .expect("load");
        self.drop_v(f.func, dest);
        self.builder.build_store(dest, win_bits).expect("store");
        self.builder.build_unconditional_branch(done).expect("br");
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
            Value::Double(d) => {
                let fv = self.ctx.f64_type().const_float(*d);
                self.write_double(func, dest, fv);
            }
            Value::Char(c) => {
                self.drop_v(func, dest);
                self.write_tag(dest, TAG_CHAR);
                let cp = self.i32t.const_int(u64::from(u32::from(*c)), false);
                self.builder
                    .build_store(self.byte_at(dest, 4), cp)
                    .expect("store");
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
        helper: &'static str,
        items: &[Expr],
        meta: Option<&Map>,
        dest: PointerValue<'ctx>,
        fallible: bool,
    ) -> Result<(), Unsupported> {
        if items.is_empty() && meta.is_none() {
            // One value per form (no allocation per evaluation). (A
            // literal with reader metadata stays per evaluation: its
            // metadata is a pool constant the init cannot read yet.)
            let site = self.consts.len();
            let k = self.constant(Const::EmptyColl { helper, site });
            self.call("rt_n_const", None, &[dest.into(), self.u64c(k)]);
            return Ok(());
        }
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

    /// A map literal. Distinct constant keyword keys (at most 8) build
    /// through a shaped `{k nil …}` template constant (#271:
    /// `rt_n_kw_map`; only the values evaluate, in order, as before);
    /// any other literal pairs up its keys and values (`rt_n_map_lit`).
    fn lower_map_lit(
        &mut self,
        f: &mut FnCtx<'ctx>,
        pairs: &[(Expr, Expr)],
        meta: Option<&Map>,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if let Some(template) = keyword_template(pairs, meta) {
            let k = self.value_const(&template)?;
            let tref = self.const_ref(k);
            let vals: Vec<Expr> = pairs.iter().map(|(_, v)| v.clone()).collect();
            let arr = self.lower_args(f, &vals)?;
            let vref = self.slots_ref(f, arr, vals.len());
            self.call(
                "rt_n_kw_map",
                None,
                &[dest.into(), tref.into(), vref.into()],
            );
            return Ok(());
        }
        let flat: Vec<Expr> = pairs
            .iter()
            .flat_map(|(k, v)| [k.clone(), v.clone()])
            .collect();
        self.lower_coll(f, "rt_n_map_lit", &flat, meta, dest, true)
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

    fn lower_cond_short_circuit(
        &mut self,
        f: &mut FnCtx<'ctx>,
        a: &Expr,
        b: &Expr,
        is_and: bool,
        then_b: BasicBlock<'ctx>,
        else_b: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let next_b = self.ctx.append_basic_block(f.func, "cond.next");
        if is_and {
            self.lower_cond(f, a, next_b, else_b)?;
        } else {
            self.lower_cond(f, a, then_b, next_b)?;
        }
        self.builder.position_at_end(next_b);
        let mark = f.env.len();
        f.env.push(None);
        let r = self.lower_cond(f, b, then_b, else_b);
        f.env.truncate(mark);
        r
    }

    fn lower_cond_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        then_b: BasicBlock<'ctx>,
        else_b: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        let tb = self.ctx.append_basic_block(f.func, "cond.then");
        let eb = self.ctx.append_basic_block(f.func, "cond.else");
        self.lower_cond(f, test, tb, eb)?;
        self.builder.position_at_end(tb);
        self.lower_cond(f, then, then_b, else_b)?;
        self.builder.position_at_end(eb);
        self.lower_cond(f, els, then_b, else_b)
    }

    /// Branch to `then_b` when `test` is truthy, else `else_b`, lowering
    /// short-circuit `and`/`or`, `not`, `if`, and inline predicates without
    /// boxing intermediate booleans.
    fn lower_cond(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then_b: BasicBlock<'ctx>,
        else_b: BasicBlock<'ctx>,
    ) -> Result<(), Unsupported> {
        if let Some((a, b, is_and)) = clj_eval::and_or_test(test, f.env.len()) {
            return self.lower_cond_short_circuit(f, a, b, is_and, then_b, else_b);
        }
        match test {
            Expr::Literal(v) => {
                let target = if matches!(v, Value::Nil | Value::Bool(false)) {
                    else_b
                } else {
                    then_b
                };
                self.builder.build_unconditional_branch(target).expect("br");
                return Ok(());
            }
            Expr::If {
                test: itest,
                then,
                els,
            } => {
                return self.lower_cond_if(f, itest, then, els, then_b, else_b);
            }
            Expr::DirectNative(call)
                if self.inline_arith && self.try_lower_cond_call(f, call, then_b, else_b)? =>
            {
                return Ok(());
            }
            _ => {}
        }
        let t = self.slot(f);
        self.lower(f, test, t)?;
        let cond = self.truthy(t);
        self.free_slots(f, &[t]);
        self.builder
            .build_conditional_branch(cond, then_b, else_b)
            .expect("br");
        Ok(())
    }

    fn try_lower_cond_call(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        then_b: BasicBlock<'ctx>,
        else_b: BasicBlock<'ctx>,
    ) -> Result<bool, Unsupported> {
        use clj_eval::ext::FastArith;
        if call.var.sym.namespace() != Some("clojure.core") {
            return Ok(false);
        }
        let name = call.var.sym.name();
        let n = call.args.len();
        if name == "not" && n == 1 {
            self.lower_cond(f, &call.args[0], else_b, then_b)?;
            return Ok(true);
        }
        let fast = fast_op(call.fast, n);
        let is_cmp = matches!(
            fast,
            Some(FastArith::Lt | FastArith::Lte | FastArith::Gt | FastArith::Gte)
        );
        let is_pred = matches!(
            (name, n),
            (
                "nil?" | "some?" | "zero?" | "pos?" | "neg?" | "even?" | "odd?",
                1
            ) | ("=" | "not=" | "==", 2)
        );
        if !is_cmp && !is_pred {
            return Ok(false);
        }
        let slow_t = self.slot(f);
        let (refs, temps, moved) = self.arg_refs(f, &call.args, slow_t)?;
        let join = self.ctx.append_basic_block(f.func, "cond.join");
        let here = self.builder.get_insert_block().expect("bb");
        self.builder.position_at_end(join);
        let phi = self
            .builder
            .build_phi(self.ctx.bool_type(), "cond")
            .expect("phi");
        self.builder.position_at_end(here);
        let sink = BoolSink::Jump { join, phi };
        let slow = self.ctx.append_basic_block(f.func, "cond.slow");
        let slow_done = self.ctx.append_basic_block(f.func, "cond.slow_done");
        let bc_sym = (fast.is_none() && self.use_bitcode)
            .then(|| clj_bc::bc_op_symbol(name, n))
            .flatten();
        let bc_block = bc_sym.map(|_| self.ctx.append_basic_block(f.func, "cond.bc"));
        let miss = bc_block.unwrap_or(slow);
        if is_cmp {
            self.lower_cmp_arith(f, fast.expect("cmp"), &refs, &sink, miss);
        } else {
            self.lower_pred_intrinsic(f, name, &refs, &sink, miss);
        }
        if let (Some(sym), Some(bb)) = (bc_sym, bc_block) {
            self.emit_bc_op(f.func, bb, sym, &refs, slow_t, slow, slow_done);
        }
        let k = self.var_of(&call.var);
        self.emit_direct_slow(f, &refs, k, n, Some(name), slow_t, slow, slow_done);
        self.builder.position_at_end(slow_done);
        let sc = self.truthy(slow_t);
        self.drop_v(f.func, slow_t);
        self.write_tag(slow_t, 0);
        self.finish_bool(f.func, &sink, sc);
        self.builder.position_at_end(join);
        self.free_slots(f, &temps);
        f.free_slots.push(slow_t);
        self.drop_and_nil_slots(f.func, &moved);
        let bit = phi.as_basic_value().into_int_value();
        self.builder
            .build_conditional_branch(bit, then_b, else_b)
            .expect("br");
        Ok(true)
    }

    fn try_lower_parse_ascii_long_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        in_tail: bool,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Some((tok, mode)) = parse_ascii_long_if(test, then, els) else {
            return Ok(false);
        };
        if !matches!(
            f.env.get(tok),
            Some(Some(LocalRef::Slot(_) | LocalRef::Captured(_)))
        ) {
            return Ok(false);
        }
        let slot = self.local_slot(f, tok).expect("checked local slot");
        let hit = self
            .call(
                "rt_n_mapv_parse_num",
                Some(self.i8t),
                &[
                    dest.into(),
                    slot.into(),
                    self.i8t.const_int(u64::from(mode), false).into(),
                ],
            )
            .expect("hit");
        if mode != 5 {
            return Ok(true);
        }
        let else_b = self.ctx.append_basic_block(f.func, "pl_else");
        let merge = self.ctx.append_basic_block(f.func, "pl_done");
        let ok = self
            .builder
            .build_int_compare(IntPredicate::NE, hit, self.i8t.const_zero(), "pl_ok")
            .expect("cmp");
        self.builder
            .build_conditional_branch(ok, merge, else_b)
            .expect("br");
        self.builder.position_at_end(else_b);
        f.in_tail = in_tail;
        self.lower(f, els, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(merge);
        Ok(true)
    }

    fn lower_if(
        &mut self,
        f: &mut FnCtx<'ctx>,
        test: &Expr,
        then: &Expr,
        els: &Expr,
        in_tail: bool,
        dest: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        if self.try_lower_parse_ascii_long_if(f, test, then, els, in_tail, dest)? {
            return Ok(());
        }
        let then_b = self.ctx.append_basic_block(f.func, "then");
        let else_b = self.ctx.append_basic_block(f.func, "else");
        let merge = self.ctx.append_basic_block(f.func, "merge");
        self.lower_cond(f, test, then_b, else_b)?;
        self.builder.position_at_end(then_b);
        f.in_tail = in_tail;
        self.lower(f, then, dest)?;
        self.builder.build_unconditional_branch(merge).expect("br");
        self.builder.position_at_end(else_b);
        f.in_tail = in_tail;
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
        if self.try_lower_scalar_loop(f, bindings, body, dest)? {
            return Ok(());
        }
        let depth = f.env.len();
        let mut bound = Vec::new();
        let mut targets = Vec::new();
        for (pat, init) in bindings {
            let t = self.slot(f);
            self.lower(f, init, t)?;
            let before = bound.len();
            self.bind_owned_slot(f, pat, t, &mut bound)?;
            targets.push((pat.clone(), bound[before..].to_vec()));
        }
        let header = self.ctx.append_basic_block(f.func, "loop");
        self.builder.build_unconditional_branch(header).expect("br");
        self.builder.position_at_end(header);
        f.recur.push(RecurTarget {
            header,
            let_depth: f.let_slots.len(),
            patterns: targets,
        });
        let result = self.lower_body(f, body, dest);
        f.recur.pop();
        f.env.truncate(depth);
        if result.is_ok() {
            self.drop_slots(f.func, &bound);
        }
        result
    }

    fn lower_recur(&mut self, f: &mut FnCtx<'ctx>, args: &[Expr]) -> Result<(), Unsupported> {
        let fresh: Result<Vec<PointerValue>, Unsupported> = args
            .iter()
            .map(|a| {
                let t = self.slot(f);
                self.lower(f, a, t).map(|()| t)
            })
            .collect();
        let fresh = fresh?;
        // The analyzer only emits `recur` in tail position of a target.
        let target = f.recur.pop().expect("recur has a target");
        let depth = f.env.len();
        // Rebinding pushes env entries per pattern position: rebuild
        // the env so later patterns see earlier ones like a fresh bind.
        for &s in &f.let_slots[target.let_depth..] {
            self.drop_v(f.func, s);
        }
        for ((pat, slots), src) in target.patterns.iter().zip(&fresh) {
            self.rebind_pattern(f, pat, *src, slots)?;
        }
        f.env.truncate(depth);
        self.free_slots(f, &fresh);
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
            let src = self.local_slot(f, *idx).expect("analyzed local is visible");
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
        let inert_free;
        let (sym, meta) = if self.var_meta {
            (sym, meta)
        } else {
            inert_free = without_inert_def_meta(sym, meta);
            (&inert_free.0, inert_free.1.as_deref())
        };
        // A plain qualified name targets its declared var slot (no
        // symbol constant, no second intern); a name with metadata keeps
        // its symbol (`:dynamic`, `:private`, … live on it).
        let (begin, finish, key) = match (sym.namespace(), sym.meta()) {
            (Some(ns), None) => (
                "rt_n_def_begin_var",
                "rt_n_def_finish_var",
                self.var(ns, sym.name()),
            ),
            _ => (
                "rt_n_def_begin",
                "rt_n_def_finish",
                self.value_const(&Value::Symbol(sym.clone()))?,
            ),
        };
        let dk = match doc {
            Some(d) => self.constant(Const::Str(d.to_owned())),
            None => NONE,
        };
        let id = self
            .call(begin, Some(self.i64t), &[self.u64c(key)])
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
                finish,
                Some(self.i32t),
                &[
                    dest.into(),
                    id.into(),
                    self.u64c(key),
                    self.u64c(dk),
                    value.into(),
                    mref.into(),
                    self.i32t.const_int(u64::from(span.line), false).into(),
                    self.i32t.const_int(u64::from(span.col), false).into(),
                ],
            )
            .expect("st");
        self.check(f, st);
        let ns = sym.namespace().unwrap_or("user").to_owned();
        if let Some(sr) = self.stable.get(&(ns, sym.name().to_owned())).copied() {
            // The root is the init value itself (`def_finish` stores a
            // clone of it): fill the cache from the slot, no var lookup.
            self.clone_v(f.func, sr.root, value);
            self.builder
                .build_store(sr.set, self.i8t.const_int(1, false))
                .expect("store")
                .set_atomic_ordering(AtomicOrdering::Release)
                .expect("release");
        }
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
                Const::MetaSymbol { ns, name, meta } => {
                    self.init_meta_symbol(&mut f, t, ns.as_deref(), name, meta);
                }
                Const::Coll(kind, items) => self.init_coll(&mut f, t, *kind, items),
                Const::EmptyColl { helper, .. } => {
                    let empty = self.const_slice(self.ptr.const_null(), 0);
                    let args = [t.into(), empty.into(), self.u64c(NONE)];
                    if *helper == "rt_n_vec_lit" {
                        self.call(helper, None, &args);
                    } else {
                        let st = self.call(helper, Some(self.i32t), &args).expect("st");
                        self.check(&f, st);
                    }
                }
            }
            self.call("rt_n_set_const", None, &[self.u64c(k as u64), t.into()]);
        }
        let result = self.slot(&mut f);
        self.end_fn(f, result)
    }

    /// Build a [`Const::MetaSymbol`] into `t`: its metadata pairs into a
    /// slot array, then `rt_n_meta_sym`.
    fn init_meta_symbol(
        &mut self,
        f: &mut FnCtx<'ctx>,
        t: PointerValue<'ctx>,
        ns: Option<&str>,
        name: &str,
        meta: &[(MetaVal, MetaVal)],
    ) {
        let n = meta.len() * 2;
        let (arr, elems) = self.slot_array(f, n);
        for ((key, val), pair) in meta.iter().zip(elems.chunks(2)) {
            self.init_meta_val(f.func, key, pair[0]);
            self.init_meta_val(f.func, val, pair[1]);
        }
        let kvs = self.slots_ref(f, arr, n);
        let np = self.opt_str_ptr(ns);
        let p = self.str_ptr(name);
        self.call(
            "rt_n_meta_sym",
            None,
            &[t.into(), np.into(), p.into(), kvs.into()],
        );
        self.drop_slots(f.func, &elems);
    }

    /// Build a [`Const::Coll`] (or a nested collection) into `t`: its
    /// items into a slot array, then `rt_n_coll`.
    fn init_coll(
        &mut self,
        f: &mut FnCtx<'ctx>,
        t: PointerValue<'ctx>,
        kind: u64,
        items: &[ConstForm],
    ) {
        let (arr, elems) = self.slot_array(f, items.len());
        for (item, slot) in items.iter().zip(&elems) {
            match item {
                ConstForm::Simple(v) => self.init_meta_val(f.func, v, *slot),
                ConstForm::MetaSym(ns, name, meta) => {
                    self.init_meta_symbol(f, *slot, ns.as_deref(), name, meta);
                }
                ConstForm::Coll(k, nested) => self.init_coll(f, *slot, *k, nested),
            }
        }
        let r = self.slots_ref(f, arr, items.len());
        self.call("rt_n_coll", None, &[t.into(), self.u64c(kind), r.into()]);
        self.drop_slots(f.func, &elems);
    }

    /// Write one [`MetaVal`] into `slot`.
    fn init_meta_val(&mut self, func: FunctionValue<'ctx>, v: &MetaVal, slot: PointerValue<'ctx>) {
        match v {
            MetaVal::Inline(v) => self
                .lower_const(func, v, slot)
                .expect("inline scalars lower"),
            MetaVal::Keyword(ns, name) | MetaVal::Symbol(ns, name) => {
                let np = self.opt_str_ptr(ns.as_deref());
                let p = self.str_ptr(name);
                let h = if matches!(v, MetaVal::Keyword(..)) {
                    "rt_n_kw"
                } else {
                    "rt_n_sym"
                };
                self.call(h, None, &[slot.into(), np.into(), p.into()]);
            }
            MetaVal::Str(s) => {
                let p = self.str_ptr(s);
                self.call("rt_n_str", None, &[slot.into(), p.into()]);
            }
        }
    }

    /// A string constant pointer, or null for `None`.
    fn opt_str_ptr(&mut self, s: Option<&str>) -> PointerValue<'ctx> {
        match s {
            Some(s) => self.str_ptr(s),
            None => self.ptr.const_null(),
        }
    }

    /// No-op targets for the Linux static link's `--wrap` of crtbeginT's
    /// `.eh_frame` registration (#271): GCC 13+'s `__register_frame_info`
    /// classifies every FDE eagerly (hundreds of thousands of instructions
    /// and the whole `.eh_frame` faulted in at startup), while unwinding
    /// finds FDEs through `--eh-frame-hdr` + `dl_iterate_phdr` without it.
    /// Unreferenced (and dead-stripped) everywhere else.
    fn lower_frame_registry_wraps(&self) {
        let ptr = self.ptr;
        let register = self.module.add_function(
            "__wrap___register_frame_info",
            self.ctx
                .void_type()
                .fn_type(&[ptr.into(), ptr.into()], false),
            None,
        );
        self.builder
            .position_at_end(self.ctx.append_basic_block(register, "entry"));
        self.builder.build_return(None).expect("ret");
        let deregister = self.module.add_function(
            "__wrap___deregister_frame_info",
            ptr.fn_type(&[ptr.into()], false),
            None,
        );
        self.builder
            .position_at_end(self.ctx.append_basic_block(deregister, "entry"));
        self.builder
            .build_return(Some(&ptr.const_null()))
            .expect("ret");
    }

    /// One names text (each namespace once, then every var name) and
    /// one `clj_rt::nabi::VarRow` per var, in index order: its spans in
    /// the text and its Lite native hint (#271: the runtime validates and
    /// copies the text once, no allocation per var).
    fn declare_vars(&mut self) {
        let row_ty = self.ctx.struct_type(&[self.i32t.into(); 5], false);
        let vars = self.vars.clone();
        let mut text = String::new();
        let mut homes: Vec<(&str, u64)> = Vec::new();
        let mut rows = Vec::with_capacity(vars.len());
        for (ns, name) in &vars {
            let ns_at = match homes.iter().find(|(home, _)| *home == ns.as_str()) {
                Some(&(_, at)) => at,
                None => {
                    let at = text.len() as u64;
                    text.push_str(ns);
                    homes.push((ns, at));
                    at
                }
            };
            let at = text.len() as u64;
            text.push_str(name);
            let hint = u64::from(clj_eval::aot_boot::lite_native_hint(ns, name));
            let fields = [ns_at, ns.len() as u64, at, name.len() as u64, hint]
                .map(|v| self.i32t.const_int(v, false).into());
            rows.push(row_ty.const_named_struct(&fields));
        }
        let names = self.str_bytes(&text);
        let names_ref = self.const_slice(names, text.len() as u64);
        let table = self.const_global(&row_ty.const_array(&rows), "cljv");
        let rows_ref = self.const_slice(table, vars.len() as u64);
        self.call(
            "rt_n_declare_vars",
            None,
            &[names_ref.into(), rows_ref.into()],
        );
    }

    /// One names text, one `clj_rt::nabi::FnRow` per compiled fn, and one
    /// flat arities table, plus the constant count so `b.consts` sizes once
    /// (#271: replaces per-fn `rt_n_declare_fn` calls and allocations).
    fn declare_fns(&mut self) {
        let row_ty = self.ctx.struct_type(&[self.i32t.into(); 4], false);
        let arity_ty = self.ctx.struct_type(
            &[self.i64t.into(), self.i64t.into(), self.ptr.into()],
            false,
        );
        let fns: Vec<(String, Vec<ArityCode>)> = self
            .fns
            .iter()
            .map(|i| (i.display.clone(), i.arities.clone()))
            .collect();
        let mut text = String::new();
        let mut rows = Vec::with_capacity(fns.len());
        let mut arities = Vec::new();
        for (display, fn_arities) in &fns {
            let name_at = text.len() as u64;
            text.push_str(display);
            let arity_at = arities.len() as u64;
            for a in fn_arities {
                arities.push(arity_ty.const_named_struct(&[
                    self.i64t.const_int(a.0 as u64, false).into(),
                    self.i64t.const_int(u64::from(a.1), false).into(),
                    a.2.as_global_value().as_pointer_value().into(),
                ]));
            }
            let fields = [
                name_at,
                display.len() as u64,
                arity_at,
                fn_arities.len() as u64,
            ]
            .map(|v| self.i32t.const_int(v, false).into());
            rows.push(row_ty.const_named_struct(&fields));
        }
        let names = self.str_bytes(&text);
        let names_ref = self.const_slice(names, text.len() as u64);
        let rows_g = self.const_global(&row_ty.const_array(&rows), "cljfnr");
        let rows_ref = self.const_slice(rows_g, rows.len() as u64);
        let arities_g = self.const_global(&arity_ty.const_array(&arities), "cljfna");
        let arities_ref = self.const_slice(arities_g, arities.len() as u64);
        let n_consts = self.u64c(self.consts.len() as u64);
        self.call(
            "rt_n_declare_fns",
            None,
            &[
                names_ref.into(),
                rows_ref.into(),
                arities_ref.into(),
                n_consts,
            ],
        );
    }

    fn lower_main(&mut self, path: &str, init: FunctionValue<'ctx>, tops: &[FunctionValue<'ctx>]) {
        let ty = self
            .i32t
            .fn_type(&[self.i32t.into(), self.ptr.into()], false);
        let main = self.module.add_function("main", ty, None);
        let entry = self.ctx.append_basic_block(main, "entry");
        self.builder.position_at_end(entry);
        self.declare_vars();
        self.declare_fns();
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
        let code = if self.lite {
            let var_refs: Vec<(&str, &str)> = self
                .vars
                .iter()
                .map(|(ns, n)| (ns.as_str(), n.as_str()))
                .collect();
            let caps = clj_eval::aot_boot::lite_caps_for(&var_refs);
            if self.var_meta {
                self.call("rt_n_cap_var_meta", None, &[]);
            }
            for (cap, sym) in [
                (clj_eval::aot_boot::LiteCaps::IO, "rt_n_cap_io"),
                (clj_eval::aot_boot::LiteCaps::CORE, "rt_n_cap_core"),
                (clj_eval::aot_boot::LiteCaps::SEQ, "rt_n_cap_seq"),
                (clj_eval::aot_boot::LiteCaps::RE, "rt_n_cap_re"),
                (clj_eval::aot_boot::LiteCaps::HOST, "rt_n_cap_host"),
                (clj_eval::aot_boot::LiteCaps::STRING, "rt_n_cap_string"),
            ] {
                if caps.contains(cap) {
                    self.call(sym, None, &[]);
                }
            }
            let need_stdin = var_refs
                .iter()
                .any(|&(ns, n)| clj_eval::aot_boot::is_lite_stdin_var(ns, n));
            let stdin_flag = self.i8t.const_int(u64::from(need_stdin), false);
            self.call(
                "rt_n_main_lite",
                Some(self.i32t),
                &[
                    pp.into(),
                    argv_ref.into(),
                    init.as_global_value().as_pointer_value().into(),
                    tops_ref.into(),
                    stdin_flag.into(),
                ],
            )
            .expect("code")
        } else {
            self.call(
                "rt_n_main",
                Some(self.i32t),
                &[
                    pp.into(),
                    argv_ref.into(),
                    init.as_global_value().as_pointer_value().into(),
                    tops_ref.into(),
                ],
            )
            .expect("code")
        };
        self.builder.build_return(Some(&code)).expect("ret");
    }
}

#[derive(Clone, Copy)]
enum StaticOp {
    Parse(&'static str),
    RemUnsigned,
    CharDigit,
    MathAbs,
    MathSqrt,
    MathUnary(&'static str),
    MathBinary(&'static str),
}

/// The inline fast path of a literal `(__static* "Cls" "member" args…)`
/// call plus its tail args (`None`: the generic `__static*` invoke).
fn static_call_op(args: &[Expr]) -> Option<(StaticOp, &[Expr])> {
    let ([fqn, mem], rest) = args.split_first_chunk::<2>()?;
    let op = static_op_spec(expr_str_lit(fqn)?, expr_str_lit(mem)?, rest.len())?;
    Some((op, rest))
}

fn expr_str_lit(e: &Expr) -> Option<&str> {
    match e {
        Expr::Quote(Value::Str(s)) | Expr::Literal(Value::Str(s)) => Some(s.as_ref()),
        _ => None,
    }
}

fn static_op_spec(fqn: &str, member: &str, n_args: usize) -> Option<StaticOp> {
    const TABLE: &[(&str, &str, usize, StaticOp)] = &[
        (
            "java.lang.Long",
            "parseLong",
            1,
            StaticOp::Parse("rt_n_parse_long"),
        ),
        (
            "java.lang.Integer",
            "parseInt",
            1,
            StaticOp::Parse("rt_n_parse_int"),
        ),
        (
            "java.lang.Double",
            "parseDouble",
            1,
            StaticOp::Parse("rt_n_parse_double"),
        ),
        (
            "java.lang.Long",
            "remainderUnsigned",
            2,
            StaticOp::RemUnsigned,
        ),
        ("java.lang.Character", "digit", 2, StaticOp::CharDigit),
        ("java.lang.Math", "abs", 1, StaticOp::MathAbs),
        ("java.lang.Math", "sqrt", 1, StaticOp::MathSqrt),
        (
            "java.lang.Math",
            "exp",
            1,
            StaticOp::MathUnary("rt_n_math_exp"),
        ),
        (
            "java.lang.Math",
            "log",
            1,
            StaticOp::MathUnary("rt_n_math_log"),
        ),
        (
            "java.lang.Math",
            "log10",
            1,
            StaticOp::MathUnary("rt_n_math_log10"),
        ),
        (
            "java.lang.Math",
            "ceil",
            1,
            StaticOp::MathUnary("rt_n_math_ceil"),
        ),
        (
            "java.lang.Math",
            "floor",
            1,
            StaticOp::MathUnary("rt_n_math_floor"),
        ),
        (
            "java.lang.Math",
            "sin",
            1,
            StaticOp::MathUnary("rt_n_math_sin"),
        ),
        (
            "java.lang.Math",
            "cos",
            1,
            StaticOp::MathUnary("rt_n_math_cos"),
        ),
        (
            "java.lang.Math",
            "tan",
            1,
            StaticOp::MathUnary("rt_n_math_tan"),
        ),
        (
            "java.lang.Math",
            "asin",
            1,
            StaticOp::MathUnary("rt_n_math_asin"),
        ),
        (
            "java.lang.Math",
            "acos",
            1,
            StaticOp::MathUnary("rt_n_math_acos"),
        ),
        (
            "java.lang.Math",
            "atan",
            1,
            StaticOp::MathUnary("rt_n_math_atan"),
        ),
        (
            "java.lang.Math",
            "sinh",
            1,
            StaticOp::MathUnary("rt_n_math_sinh"),
        ),
        (
            "java.lang.Math",
            "cosh",
            1,
            StaticOp::MathUnary("rt_n_math_cosh"),
        ),
        (
            "java.lang.Math",
            "tanh",
            1,
            StaticOp::MathUnary("rt_n_math_tanh"),
        ),
        (
            "java.lang.Math",
            "cbrt",
            1,
            StaticOp::MathUnary("rt_n_math_cbrt"),
        ),
        (
            "java.lang.Math",
            "pow",
            2,
            StaticOp::MathBinary("rt_n_math_pow"),
        ),
        (
            "java.lang.Math",
            "atan2",
            2,
            StaticOp::MathBinary("rt_n_math_atan2"),
        ),
        (
            "java.lang.Math",
            "hypot",
            2,
            StaticOp::MathBinary("rt_n_math_hypot"),
        ),
    ];
    TABLE
        .iter()
        .find(|&&(c, m, n, _)| c == fqn && m == member && n == n_args)
        .map(|r| r.3)
}

/// The `FastArith` op a direct call inlines (`inc`/`dec` take one
/// arg, binary/variadic ops fold up to 8 args).
fn fast_op(fast: clj_eval::ext::FastArith, n: usize) -> Option<clj_eval::ext::FastArith> {
    use clj_eval::ext::FastArith;
    match (fast, n) {
        (FastArith::Inc | FastArith::Dec, 1) => Some(fast),
        (FastArith::None | FastArith::Inc | FastArith::Dec, _) => None,
        (other, 2..=8) => Some(other),
        _ => None,
    }
}

/// A value with a direct constant builder (`None`: the reader
/// fallback).
/// `v`'s printed form, checked to read back equal (the
/// `rt_n_read` constant path).
fn readable_text(v: &Value) -> Result<String, Unsupported> {
    let text = native_abi::print_constant(v);
    match native_abi::read_constant(&text) {
        Ok(back) if back == *v => Ok(text),
        _ => Err(Unsupported(format!(
            "constant without a readable form: {text}"
        ))),
    }
}

fn scalar_const(v: &Value) -> Option<Const> {
    Some(match v {
        Value::Str(s) => Const::Str(s.to_string()),
        Value::Keyword(k) => Const::Keyword(k.namespace().map(str::to_owned), k.name().to_owned()),
        Value::Symbol(s) if s.meta().is_none() => {
            Const::Symbol(s.namespace().map(str::to_owned), s.name().to_owned())
        }
        _ => return host_const(v),
    })
}

/// A symbol whose metadata maps keywords to simple values, built
/// without the reader (#271; `None`: anything else).
fn meta_symbol_const(v: &Value) -> Option<Const> {
    let Value::Symbol(s) = v else {
        return None;
    };
    let mut meta = Vec::new();
    for (k, val) in s.meta()?.iter() {
        let key = matches!(k, Value::Keyword(_)).then(|| meta_val(k))??;
        meta.push((key, meta_val(val)?));
    }
    Some(Const::MetaSymbol {
        ns: s.namespace().map(str::to_owned),
        name: s.name().to_owned(),
        meta,
    })
}

/// A metadata-free collection of simple forms as a [`Const::Coll`],
/// kept only when building it prints exactly as `v` does (#271).
fn coll_const(v: &Value) -> Option<Const> {
    let mut budget = COLL_FORMS_MAX;
    let (kind, items) = coll_parts(v, &mut budget)?;
    let built =
        clj_eval::native_abi::const_coll(kind, &items.iter().map(form_value).collect::<Vec<_>>());
    let same = native_abi::print_constant(&built) == native_abi::print_constant(v);
    same.then_some(Const::Coll(kind, items))
}

/// `v`'s [`ConstForm`] collection kind and items (within `budget` forms).
fn coll_parts(v: &Value, budget: &mut usize) -> Option<(u64, Vec<ConstForm>)> {
    let (kind, items) = coll_items(v)?;
    *budget = budget.checked_sub(items.len() + 1)?;
    let forms = items
        .iter()
        .map(|item| item_form(item, budget))
        .collect::<Option<_>>()?;
    Some((kind, forms))
}

/// A metadata-free unsorted collection's kind (`rt_n_coll`'s code) and
/// items (a map's keys and values interleaved).
fn coll_items(v: &Value) -> Option<(u64, Vec<Value>)> {
    Some(match v {
        Value::List(l) if l.meta().is_none() => (0, l.iter().cloned().collect()),
        Value::Vector(x) if x.meta().is_none() => (1, x.iter().collect()),
        Value::Map(m) if m.meta().is_none() && !m.is_sorted() => (
            2,
            m.iter()
                .flat_map(|(k, val)| [k.clone(), val.clone()])
                .collect(),
        ),
        Value::Set(s) if s.meta().is_none() && !s.is_sorted() => (3, s.iter().cloned().collect()),
        _ => return None,
    })
}

/// One collection item as a [`ConstForm`].
fn item_form(v: &Value, budget: &mut usize) -> Option<ConstForm> {
    if let Some(item) = meta_val(v) {
        return Some(ConstForm::Simple(item));
    }
    if let Some(Const::MetaSymbol { ns, name, meta }) = meta_symbol_const(v) {
        return Some(ConstForm::MetaSym(ns, name, meta));
    }
    coll_parts(v, budget).map(|(kind, items)| ConstForm::Coll(kind, items))
}

/// The value a [`ConstForm`] builds (the compiler-side twin of the
/// constant init, for [`coll_const`]'s check).
fn form_value(form: &ConstForm) -> Value {
    match form {
        ConstForm::Simple(v) => meta_val_value(v),
        ConstForm::MetaSym(ns, name, meta) => {
            let kvs: Vec<Value> = meta
                .iter()
                .flat_map(|(k, v)| [meta_val_value(k), meta_val_value(v)])
                .collect();
            native_abi::meta_symbol(ns.as_deref(), name, &kvs)
        }
        ConstForm::Coll(kind, items) => {
            native_abi::const_coll(*kind, &items.iter().map(form_value).collect::<Vec<_>>())
        }
    }
}

/// The value a [`MetaVal`] builds.
fn meta_val_value(v: &MetaVal) -> Value {
    match v {
        MetaVal::Inline(v) => v.clone(),
        MetaVal::Keyword(ns, name) => Value::Keyword(clj_value::Keyword::new(ns.as_deref(), name)),
        MetaVal::Symbol(ns, name) => Value::Symbol(Symbol::new(ns.as_deref(), name)),
        MetaVal::Str(s) => Value::Str(std::sync::Arc::from(s.as_str())),
    }
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

/// Whether a direct call has an owned-args entry that can reuse moved
/// (`Arc::strong_count == 1`) collections or release consumed inputs early.
/// Which eager traversal a [`HofLoop`] runs: a `doseq` (`dorun` of a
/// `map`), a vector build (`mapv`, `vec` / `into []` of a `map`), or a
/// `reduce`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HofKind {
    Each,
    Collect,
    FilterCollect,
    CountFilter,
    Some,
    Every,
    Sum,
    Reduce,
}

/// A core traversal over a literal `fn` that a compiled loop can run
/// inline (#271 Slice 4c/5g/5h), with the calls the general path makes: the
/// inner one (`map`, `mapv`, `filter`, `filterv`, `some`, `every?`, or `reduce`)
/// and, for `dorun` / `vec` / `into []` / `count` / `reduce +`, the outer one.
pub(super) struct HofLoop<'a> {
    pub(super) kind: HofKind,
    fun: &'a FunExpr,
    pub(super) arity: &'a Arity,
    pub(super) init: Option<&'a Expr>,
    coll: &'a Expr,
    pub(super) coll2: Option<&'a Expr>,
    inner: &'a clj_eval::expr::DirectCall,
    outer: Option<(&'a clj_eval::expr::DirectCall, Option<&'a Expr>)>,
}

/// A compiled loop's runtime state: the accumulator and collection
/// slots, the [`rt_n_iter_plan`] kind, and the plan's count, start, step.
pub(super) struct LoopState<'ctx> {
    acc: PointerValue<'ctx>,
    pub(super) coll: PointerValue<'ctx>,
    pub(super) kind: IntValue<'ctx>,
    pub(super) n: IntValue<'ctx>,
    pub(super) start: IntValue<'ctx>,
    pub(super) step: IntValue<'ctx>,
}

/// `(range a0 [a1 [a2]])` as a direct `clojure.core` call a fused loop
/// classifies by its long bounds without building the range value
/// (#271 Slice 5l): 1-3 arguments (arity 0 is the infinite range; the
/// general path materializes the call over the lowered arg slots).
fn range_plan_call(e: &Expr) -> Option<&clj_eval::expr::DirectCall> {
    let Expr::DirectNative(c) = e else {
        return None;
    };
    (c.var.sym.namespace() == Some("clojure.core")
        && c.var.sym.name() == "range"
        && matches!(c.args.len(), 1..=3))
    .then_some(c)
}

/// A fused-loop collection that came from a [`range_plan_call`]: the
/// lowered argument slots (classification reads them in place; the
/// general path moves them into the range call).
struct RangeSrc<'a, 'ctx> {
    call: &'a clj_eval::expr::DirectCall,
    args: Vec<PointerValue<'ctx>>,
}

fn is_direct_moving(
    f: &FnCtx<'_>,
    call: &clj_eval::expr::DirectCall,
    core_name: Option<&str>,
) -> bool {
    let n = call.args.len();
    core_name
        .and_then(|cname| native_abi::own_op(cname, n))
        .is_some()
        || matches!(
            (core_name, call.args.as_slice()),
            (Some("swap!"), [_, fe, Expr::VecLit(ks, None), _])
                if ks.len() == 2 && is_core_global(fe, "assoc-in")
        )
        || (is_owned_move_op(call.var.sym.namespace(), call.var.sym.name(), n)
            && call.args.iter().any(|a| is_move(f, a) || is_temporary(a)))
}

fn is_core_global(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Global(v) if v.sym.namespace() == Some("clojure.core") && v.sym.name() == name)
}

#[allow(clippy::type_complexity)]
fn dot_product_call(
    call: &clj_eval::expr::DirectCall,
) -> Option<(
    &Expr,
    &Expr,
    &clj_eval::expr::DirectCall,
    &Expr,
    &Expr,
    &Expr,
)> {
    if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "reduce" {
        return None;
    }
    let [plus, init, Expr::DirectNative(m)] = call.args.as_slice() else {
        return None;
    };
    if !is_core_global(plus, "+")
        || m.var.sym.namespace() != Some("clojure.core")
        || m.var.sym.name() != "map"
    {
        return None;
    }
    let [star, c1, c2] = m.args.as_slice() else {
        return None;
    };
    is_core_global(star, "*").then_some((plus, init, m, star, c1, c2))
}

struct PrintJoin<'a> {
    join: &'a clj_eval::expr::DirectCall,
    str_call: Option<(&'a clj_eval::expr::DirectCall, &'a Expr)>,
    sep: &'a Expr,
    coll: &'a Expr,
    flags: u8,
}

fn is_str_join(e: &Expr) -> Option<(&clj_eval::expr::DirectCall, &Expr, &Expr)> {
    let Expr::DirectNative(j) = e else {
        return None;
    };
    if j.var.sym.namespace() != Some("clojure.string") || j.var.sym.name() != "join" {
        return None;
    }
    let [sep, coll] = j.args.as_slice() else {
        return None;
    };
    Some((j, sep, coll))
}

fn print_str_join_nl(arg: &Expr) -> Option<PrintJoin<'_>> {
    let Expr::DirectNative(s) = arg else {
        return None;
    };
    if s.var.sym.namespace() != Some("clojure.core") || s.var.sym.name() != "str" {
        return None;
    }
    let [inner, nl @ Expr::Literal(Value::Str(nl_s))] = s.args.as_slice() else {
        return None;
    };
    if nl_s.as_ref() != "\n" {
        return None;
    }
    let (join, sep, coll) = is_str_join(inner)?;
    Some(PrintJoin {
        join,
        str_call: Some((s, nl)),
        sep,
        coll,
        flags: 1,
    })
}

fn print_join_call(call: &clj_eval::expr::DirectCall) -> Option<PrintJoin<'_>> {
    if call.var.sym.namespace() != Some("clojure.core") {
        return None;
    }
    let [arg] = call.args.as_slice() else {
        return None;
    };
    let name = call.var.sym.name();
    if (name == "println" || name == "print")
        && let Some((join, sep, coll)) = is_str_join(arg)
    {
        return Some(PrintJoin {
            join,
            str_call: None,
            sep,
            coll,
            flags: if name == "println" { 3 } else { 0 },
        });
    }
    (name == "print").then(|| print_str_join_nl(arg)).flatten()
}

fn unwrap_single_do(e: &Expr) -> &Expr {
    match e {
        Expr::Do(xs) if xs.len() == 1 => &xs[0],
        _ => e,
    }
}

/// Match `(and (string? tok) (re-matches #"<pat>" tok))` returning `(tok, pat)`.
fn match_str_re_matches_and(test: &Expr) -> Option<(usize, &str)> {
    let Expr::Let { bindings, body } = test else {
        return None;
    };
    let [(Pattern::Symbol(_), a)] = bindings.as_slice() else {
        return None;
    };
    let [Expr::If { test, then: b, els }] = body.as_slice() else {
        return None;
    };
    let (Expr::Local(g1), Expr::Local(g2)) = (test.as_ref(), els.as_ref()) else {
        return None;
    };
    if g1 != g2 {
        return None;
    }
    let ("string?", [Expr::Local(tok)]) = core_call(a)? else {
        return None;
    };
    let ("re-matches", [Expr::Literal(Value::Regex(pat)), Expr::Local(tok2)]) = core_call(b)?
    else {
        return None;
    };
    (tok == tok2 && tok != g1).then_some((*tok, pat.as_ref()))
}

/// Match `(if (and (string? tok) (re-matches #"<pat>" tok)) (parse-long tok) els)`
/// returning `(tok, mode)` (`4` `[+-]?[0-9]+` with nil `els`, `7` `-?[0-9]+` with
/// nil `els`, `5` `[+-]?[0-9]{1,18}` with any `els`).
fn parse_ascii_long_if(test: &Expr, then: &Expr, els: &Expr) -> Option<(usize, u8)> {
    let (tok, pat) = match_str_re_matches_and(test)?;
    let ("parse-long", [Expr::Local(tok3)]) = core_call(unwrap_single_do(then))? else {
        return None;
    };
    if *tok3 != tok {
        return None;
    }
    let nil_els = matches!(els, Expr::Literal(Value::Nil));
    match pat {
        "[+-]?[0-9]+" if nil_els => Some((tok, 4)),
        "-?[0-9]+" if nil_els => Some((tok, 7)),
        "[+-]?[0-9]{1,18}" => Some((tok, 5)),
        _ => None,
    }
}

fn parse_static_num_arity(body: &Expr, base: usize) -> Option<u8> {
    let Expr::Invoke { head, args, .. } = body else {
        return None;
    };
    if !is_core_global(head, "__static*") {
        return None;
    }
    match static_call_op(args)? {
        (StaticOp::Parse("rt_n_parse_long"), [Expr::Local(i)]) if *i == base => Some(0),
        (StaticOp::Parse("rt_n_parse_int"), [Expr::Local(i)]) if *i == base => Some(1),
        (StaticOp::Parse("rt_n_parse_double"), [Expr::Local(i)]) if *i == base => Some(2),
        _ => None,
    }
}

fn parse_num_arity(arity: &Arity, base: usize) -> Option<u8> {
    let [body] = arity.body.as_slice() else {
        return None;
    };
    if let Expr::If { test, then, els } = body
        && let Some((tok, mode)) = parse_ascii_long_if(test, then, els)
        && tok == base
    {
        return match mode {
            4 => Some(3),
            7 => Some(6),
            _ => None,
        };
    }
    parse_static_num_arity(body, base)
}

/// `call` as a [`HofLoop`]: `(mapv F xs [ys])`, `(filterv F xs)`, `(some F xs)`,
/// `(every? F xs)`, `(reduce F init xs)`, `(reduce + [init] (map F xs [ys]))`,
/// `(dorun (map F xs [ys]))`, `(vec (map|filter F xs [ys]))`,
/// `(into [] (map|filter F xs [ys]))`, or `(count (filter|filterv F xs))`,
/// all `clojure.core`.
fn hof_loop<'a>(call: &'a clj_eval::expr::DirectCall) -> Option<HofLoop<'a>> {
    if call.var.sym.namespace() != Some("clojure.core") {
        return None;
    }
    let one =
        |kind, fe: &'a Expr, init: Option<&'a Expr>, coll: &'a Expr, coll2: Option<&'a Expr>| {
            let n_params = if kind == HofKind::Reduce || coll2.is_some() {
                2
            } else {
                1
            };
            let (fun, arity) = inline_fn(fe, n_params)?;
            Some(HofLoop {
                kind,
                fun,
                arity,
                init,
                coll,
                coll2,
                inner: call,
                outer: None,
            })
        };
    match (call.var.sym.name(), call.args.as_slice()) {
        ("mapv", [fe, coll]) => one(HofKind::Collect, fe, None, coll, None),
        ("mapv", [fe, c1, c2]) => one(HofKind::Collect, fe, None, c1, Some(c2)),
        ("filterv", [fe, coll]) => one(HofKind::FilterCollect, fe, None, coll, None),
        ("some", [fe, coll]) => one(HofKind::Some, fe, None, coll, None),
        ("every?", [fe, coll]) => one(HofKind::Every, fe, None, coll, None),
        ("reduce", [fe, init, coll]) => one(HofKind::Reduce, fe, Some(init), coll, None)
            .or_else(|| sum_hof_loop(fe, Some(init), coll, call)),
        ("reduce", [fe, coll]) => sum_hof_loop(fe, None, coll, call),
        _ => outer_hof_loop(call),
    }
}

fn sum_hof_loop<'a>(
    plus: &'a Expr,
    init: Option<&'a Expr>,
    inner: &'a Expr,
    outer: &'a clj_eval::expr::DirectCall,
) -> Option<HofLoop<'a>> {
    if !is_core_global(plus, "+") {
        return None;
    }
    let mut hl = inner_hof_loop(inner, &["map"], HofKind::Sum, outer, Some(plus))?;
    hl.init = init;
    Some(hl)
}

fn outer_hof_loop(call: &clj_eval::expr::DirectCall) -> Option<HofLoop<'_>> {
    match (call.var.sym.name(), call.args.as_slice()) {
        ("dorun", [inner]) => inner_hof_loop(inner, &["map"], HofKind::Each, call, None),
        ("vec", [inner]) => inner_hof_loop(inner, &["map"], HofKind::Collect, call, None)
            .or_else(|| inner_hof_loop(inner, &["filter"], HofKind::FilterCollect, call, None)),
        ("into", [to @ Expr::VecLit(items, None), inner]) if items.is_empty() => {
            inner_hof_loop(inner, &["map"], HofKind::Collect, call, Some(to)).or_else(|| {
                inner_hof_loop(inner, &["filter"], HofKind::FilterCollect, call, Some(to))
            })
        }
        ("count", [inner]) => inner_hof_loop(
            inner,
            &["filter", "filterv"],
            HofKind::CountFilter,
            call,
            None,
        ),
        _ => None,
    }
}

/// `(map|filter|filterv F xs [ys])` (a `clojure.core` traversal of a literal
/// `fn`) under the outer call `outer`.
fn inner_hof_loop<'a>(
    inner: &'a Expr,
    names: &[&str],
    kind: HofKind,
    outer: &'a clj_eval::expr::DirectCall,
    prefix: Option<&'a Expr>,
) -> Option<HofLoop<'a>> {
    let Expr::DirectNative(m) = inner else {
        return None;
    };
    if m.var.sym.namespace() != Some("clojure.core") || !names.contains(&m.var.sym.name()) {
        return None;
    }
    let (fe, coll, coll2, n_params) = match m.args.as_slice() {
        [fe, coll] => (fe, coll, None, 1),
        [fe, c1, c2] if m.var.sym.name() == "map" => (fe, c1, Some(c2), 2),
        _ => return None,
    };
    let (fun, arity) = inline_fn(fe, n_params)?;
    Some(HofLoop {
        kind,
        fun,
        arity,
        init: None,
        coll,
        coll2,
        inner: m,
        outer: Some((outer, prefix)),
    })
}

/// A literal `fn` whose one arity takes exactly `params` symbol params
/// and never `recur`s to itself: its body can run inline per item (a
/// named `fn` arrives as a `letfn`, so it never refers to itself here).
fn inline_fn(e: &Expr, params: usize) -> Option<(&FunExpr, &Arity)> {
    let Expr::Fn(fun) = e else {
        return None;
    };
    let [arity] = &fun.arities[..] else {
        return None;
    };
    (arity.rest.is_none()
        && arity.fixed.len() == params
        && arity.fixed.iter().all(|p| matches!(p, Pattern::Symbol(_)))
        && !self_recurs(&arity.body))
    .then_some((fun, arity))
}

/// Whether every use of local `i` in `body` is a read-only call operand
/// (#271): an argument of a core call outside the owned-move ops, or of
/// an invoke. A param used only so keeps reading the caller's argument
/// array: an owned copy at entry buys no in-place update.
fn borrow_only(body: &[Expr], i: usize) -> bool {
    fn walk(e: &Expr, i: usize) -> bool {
        let operand = |a: &Expr| matches!(a, Expr::Local(j) if *j == i) || walk(a, i);
        match e {
            Expr::Local(j) => *j != i,
            Expr::DirectNative(c)
                if !is_owned_move_op(c.var.sym.namespace(), c.var.sym.name(), c.args.len()) =>
            {
                c.args.iter().all(operand)
            }
            Expr::Invoke { head, args, .. } => walk(head, i) && args.iter().all(operand),
            _ => {
                let mut ok = true;
                for_each_child(e, &mut |c| ok = ok && walk(c, i));
                ok
            }
        }
    }
    body.iter().all(|e| walk(e, i))
}

/// A map literal's `{k nil …}` template (#271) when it carries no
/// metadata and its keys are 1 to 8 distinct constant keywords: an
/// array map either way, whose interned shape every evaluation shares.
fn keyword_template(pairs: &[(Expr, Expr)], meta: Option<&Map>) -> Option<Value> {
    if meta.is_some() || pairs.is_empty() || pairs.len() > 8 {
        return None;
    }
    let mut keys: Vec<Value> = Vec::with_capacity(pairs.len());
    for (k, _) in pairs {
        let Expr::Literal(key @ Value::Keyword(_)) = k else {
            return None;
        };
        if keys.contains(key) {
            return None;
        }
        keys.push(key.clone());
    }
    let kvs = keys.into_iter().map(|k| (k, Value::Nil)).collect();
    Some(Value::Map(Map::from_kvs_last_wins(kvs)))
}

/// Whether a map pattern binds by direct lookups (#271): no required
/// (`!`) keys, no `&` key list and no 1.13 outlets, which need the
/// interpreter's whole destructure. Nested patterns decide for
/// themselves (a plain map forces no outlets on them).
fn plain_map(m: &clj_eval::expr::MapPattern) -> bool {
    m.extra_keys.is_empty()
        && m.select.is_none()
        && m.all.is_none()
        && m.excess.is_none()
        && m.missing.is_none()
        && m.keys.iter().all(|k| !k.required)
        && m.entries.iter().all(|(_, _, _, required)| !required)
}

/// The `:or` default a map binding takes when its key is absent: the
/// one keyed by the binding's local symbol, else by its key form as
/// written (the interpreter's `map_binding` order; #158).
fn or_default(
    m: &clj_eval::expr::MapPattern,
    local: Option<&Value>,
    or_key: &Value,
) -> Option<usize> {
    local
        .and_then(|l| m.or.iter().position(|(k, _)| k == l))
        .or_else(|| m.or.iter().position(|(k, _)| k == or_key))
}

fn is_owned_move_op(ns: Option<&str>, name: &str, argc: usize) -> bool {
    match (ns, name) {
        (Some("clojure.string"), "join") => argc == 2,
        (Some("clojure.core"), "assoc") => argc >= 3 && argc % 2 == 1,
        (Some("clojure.core"), "conj" | "into" | "merge" | "dissoc" | "disj") => argc >= 2,
        (Some("clojure.core"), "pop" | "next" | "rest" | "transient" | "persistent!") => argc == 1,
        (Some("clojure.core"), "assoc-in") => argc == 3,
        (Some("clojure.core"), "update" | "update-in") => argc >= 3,
        (
            Some("clojure.core"),
            "str" | "vec" | "set" | "butlast" | "doall" | "dorun" | "drop-last" | "take-last"
            | "frequencies" | "reverse" | "sort" | "sort-by" | "group-by" | "some" | "every?"
            | "not-every?" | "not-any?" | "mapv" | "filterv" | "reduce" | "reduce-kv" | "run!"
            | "transduce",
        ) => argc >= 1,
        _ => false,
    }
}

/// Whether `e` evaluates into a slot of its own (a call, a constructed
/// collection — not a local, constant, or var): an owned call takes that
/// slot over instead of cloning it (#271), so e.g. `reduce` sees a fresh
/// `(map …)` seq as its only holder.
fn is_temporary(e: &Expr) -> bool {
    !matches!(
        e,
        Expr::Local(_) | Expr::Literal(_) | Expr::Quote(_) | Expr::Global(_) | Expr::VarRef(_)
    )
}

/// Whether `e` is an occurrence the current `recur` args move.
fn is_move(f: &FnCtx<'_>, e: &Expr) -> bool {
    f.moves.iter().any(|m| m.0 == expr_addr(e))
}

/// A node's address (identifies one occurrence).
fn expr_addr(e: &Expr) -> usize {
    std::ptr::from_ref(e) as usize
}

/// Collects `reads(e)` into `later` without emitting moves.
fn reads(e: &Expr, later: &mut BTreeSet<usize>) {
    if let Expr::Local(i) = e {
        later.insert(*i);
    }
    for_each_child(e, &mut |c| reads(c, later));
}

fn is_pure_expr(e: &Expr) -> bool {
    match e {
        Expr::Literal(_) | Expr::Quote(_) | Expr::Local(_) => true,
        Expr::VecLit(items, None) => items.iter().all(is_pure_expr),
        _ => false,
    }
}

fn same_pure_key<'a>(a: &'a Expr, b: &'a Expr) -> Option<&'a Expr> {
    match (a, b) {
        (Expr::Local(i), Expr::Local(j)) if i == j => Some(a),
        (Expr::Literal(Value::Int(x)), Expr::Literal(Value::Int(y))) if x == y => Some(a),
        _ => None,
    }
}

type AssocConjParts<'a> = (usize, usize, &'a Expr, &'a Expr);

fn match_assoc_conj(call: &clj_eval::expr::DirectCall) -> Option<AssocConjParts<'_>> {
    if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "assoc!" {
        return None;
    }
    let [Expr::Local(ti1), k1, Expr::DirectNative(conj_c)] = call.args.as_slice() else {
        return None;
    };
    if conj_c.var.sym.namespace() != Some("clojure.core") || conj_c.var.sym.name() != "conj" {
        return None;
    }
    let [Expr::DirectNative(nth_c), x] = conj_c.args.as_slice() else {
        return None;
    };
    if nth_c.var.sym.namespace() != Some("clojure.core") || nth_c.var.sym.name() != "nth" {
        return None;
    }
    let [Expr::Local(ti2), k2] = nth_c.args.as_slice() else {
        return None;
    };
    let k_expr = same_pure_key(k1, k2)?;
    is_pure_expr(x).then_some((*ti1, *ti2, k_expr, x))
}

impl<'ctx> Lowerer<'ctx> {
    fn resolve_assoc_conj_operand(
        &mut self,
        f: &mut FnCtx<'ctx>,
        e: &Expr,
        allow_move: Option<&dyn Fn(usize) -> bool>,
    ) -> Result<(PointerValue<'ctx>, u8, Option<PointerValue<'ctx>>), Unsupported> {
        if let Expr::Local(idx) = e
            && let Some(p) = self.slot_of(f, *idx)
        {
            let can_take = allow_move.is_some_and(|pred| {
                is_move(f, e)
                    && matches!(f.env.get(*idx), Some(Some(LocalRef::Slot(_))))
                    && pred(*idx)
            });
            return Ok((p, u8::from(can_take), None));
        }
        let s = self.slot(f);
        let saved = std::mem::take(&mut f.moves);
        let res = self.lower(f, e, s);
        f.moves = saved;
        res?;
        Ok((s, 1, Some(s)))
    }

    fn assoc_conj_slots_ok(&self, f: &FnCtx<'ctx>, e: &Expr) -> bool {
        !matches!(e, Expr::Local(i) if self.slot_of(f, *i).is_none())
    }

    /// Fused `(assoc! t1 k (conj (nth t2 k) x))` in place: returns
    /// `Ok(true)` when matched and lowered, delegating misses to
    /// ordinary lowering.
    fn try_lower_assoc_conj(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: Option<PointerValue<'ctx>>,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith {
            return Ok(false);
        }
        let Some((ti1, ti2, k_expr, x)) = match_assoc_conj(call) else {
            return Ok(false);
        };
        let (Some(t1p), Some(t2p)) = (self.slot_of(f, ti1), self.slot_of(f, ti2)) else {
            return Ok(false);
        };
        if !self.assoc_conj_slots_ok(f, k_expr) || !self.assoc_conj_slots_ok(f, x) {
            return Ok(false);
        }
        let (kp, _, temp_k) = self.resolve_assoc_conj_operand(f, k_expr, None)?;
        let same_k = |xi: usize| matches!(k_expr, Expr::Local(ki) if *ki == xi);
        let pred = |xi: usize| xi != ti1 && xi != ti2 && !same_k(xi);
        let (xp, take_x, temp_x) = self.resolve_assoc_conj_operand(f, x, Some(&pred))?;
        let ok = self
            .call(
                "rt_n_assoc_conj",
                Some(self.i8t),
                &[
                    t1p.into(),
                    t2p.into(),
                    kp.into(),
                    xp.into(),
                    self.i8t.const_int(u64::from(take_x), false).into(),
                ],
            )
            .expect("ok");
        if let Some(sk) = temp_k {
            self.drop_v(f.func, sk);
        }
        self.emit_assoc_conj_branches(f, call, ok, t1p, temp_x, dest)?;
        Ok(true)
    }

    fn emit_assoc_conj_branches(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        ok: IntValue<'ctx>,
        t1p: PointerValue<'ctx>,
        temp_x: Option<PointerValue<'ctx>>,
        dest: Option<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "ac_hit")
            .expect("cmp");
        let fast_b = self.ctx.append_basic_block(f.func, "ac_hit");
        let slow_b = self.ctx.append_basic_block(f.func, "ac_miss");
        let done_b = self.ctx.append_basic_block(f.func, "ac_done");
        self.builder
            .build_conditional_branch(hit, fast_b, slow_b)
            .expect("br");
        self.builder.position_at_end(fast_b);
        if let Some(d) = dest {
            self.clone_v(f.func, d, t1p);
        }
        self.builder.build_unconditional_branch(done_b).expect("br");
        self.builder.position_at_end(slow_b);
        if let Some(s) = temp_x {
            self.drop_v(f.func, s);
        }
        self.lower_assoc_conj_miss(f, call, dest)?;
        self.builder.build_unconditional_branch(done_b).expect("br");
        self.builder.position_at_end(done_b);
        Ok(())
    }

    fn lower_assoc_conj_miss(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
        dest: Option<PointerValue<'ctx>>,
    ) -> Result<(), Unsupported> {
        let out = match dest {
            Some(d) => d,
            None => self.slot(f),
        };
        self.lower_direct_with(f, call, None, out)?;
        if dest.is_none() {
            self.drop_v(f.func, out);
        }
        Ok(())
    }

    /// Effect-only `(conj! coll x)` via inlined `rt_bc_2_conj_bang_fx` (when
    /// bitcode enabled) + `rt_n_conj_bang_fx` (no transient clone/drop), falling
    /// back to ordinary lowering on `0`.
    fn try_lower_conj_bang_fx(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &clj_eval::expr::DirectCall,
    ) -> Result<bool, Unsupported> {
        if !self.inline_arith
            || !self.use_bitcode
            || call.var.sym.namespace() != Some("clojure.core")
            || call.var.sym.name() != "conj!"
            || call.args.len() != 2
        {
            return Ok(false);
        }
        let tmp = self.slot(f);
        let (refs, temps, moved) = self.arg_refs(f, &call.args, tmp)?;
        let done = self.ctx.append_basic_block(f.func, "cb_done");
        let nfx = self.ctx.append_basic_block(f.func, "cb_nfx");
        let ok = self
            .call(
                "rt_bc_2_conj_bang_fx",
                Some(self.i8t),
                &[tmp.into(), refs[0].into(), refs[1].into()],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "cb_bc")
            .expect("cmp");
        self.builder
            .build_conditional_branch(hit, done, nfx)
            .expect("br");
        self.builder.position_at_end(nfx);
        let ok = self
            .call(
                "rt_n_conj_bang_fx",
                Some(self.i8t),
                &[refs[0].into(), refs[1].into()],
            )
            .expect("ok");
        let hit = self
            .builder
            .build_int_compare(IntPredicate::NE, ok, self.i8t.const_zero(), "cb_hit")
            .expect("cmp");
        let slow = self.ctx.append_basic_block(f.func, "cb_miss");
        self.builder
            .build_conditional_branch(hit, done, slow)
            .expect("br");
        self.builder.position_at_end(slow);
        let k = self.var_of(&call.var);
        let fast = fast_op(call.fast, 2);
        let inner = self.ctx.append_basic_block(f.func, "cb_slow");
        self.emit_direct_inline(f, fast, Some("conj!"), None, &refs, tmp, inner, done);
        self.emit_direct_slow(f, &refs, k, 2, Some("conj!"), tmp, inner, done);
        self.drop_v(f.func, tmp);
        self.drop_and_nil_slots(f.func, &temps);
        self.drop_and_nil_slots(f.func, &moved);
        Ok(true)
    }
}

fn expr_moves_local(e: &Expr, idx: usize, moves: &[(usize, usize)]) -> bool {
    if let Expr::Local(_) = e
        && moves.iter().any(|&(a, l)| l == idx && a == expr_addr(e))
    {
        return true;
    }
    let mut hit = false;
    for_each_child(e, &mut |c| {
        if !hit && expr_moves_local(c, idx, moves) {
            hit = true;
        }
    });
    hit
}

fn collect_body_moves(
    body: &[Expr],
    min_slot: usize,
    env: &[Option<LocalRef<'_>>],
    owned_params: &[usize],
) -> Vec<(usize, usize)> {
    let mut slots = vec![false; env.len().max(min_slot)];
    for &i in owned_params {
        slots[i] = true;
    }
    let mut later = BTreeSet::new();
    let mut out = Vec::new();
    for e in body.iter().rev() {
        walk_moves(e, &mut slots, min_slot, &mut later, &mut out);
    }
    out
}

fn walk_moves(
    e: &Expr,
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    match e {
        Expr::Local(i) => {
            if !later.contains(i) && *i >= min_slot && slots.get(*i).copied().unwrap_or(false) {
                out.push((expr_addr(e), *i));
            }
            later.insert(*i);
        }
        Expr::If { test, then, els } => walk_if_moves(test, then, els, slots, min_slot, later, out),
        Expr::CaseStar {
            expr,
            default,
            cases,
            ..
        } => walk_case_moves(expr, default, cases, slots, min_slot, later, out),
        Expr::Let { bindings, body } => walk_let_moves(bindings, body, slots, min_slot, later, out),
        Expr::Loop { bindings, body } => {
            walk_loop_moves(bindings, body, slots, min_slot, later, out);
        }
        Expr::LetFn { fns, body } => walk_letfn_moves(fns, body, slots, min_slot, later, out),
        Expr::Recur(args) => walk_recur_moves(args, slots, min_slot, later, out),
        _ if walk_extent_moves(e, slots, min_slot, later, out) => {}
        _ => walk_children_moves(e, slots, min_slot, later, out),
    }
}

fn walk_if_moves(
    test: &Expr,
    then: &Expr,
    els: &Expr,
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let mut other = later.clone();
    walk_moves(els, slots, min_slot, &mut other, out);
    walk_moves(then, slots, min_slot, later, out);
    later.extend(other);
    walk_moves(test, slots, min_slot, later, out);
}

fn walk_recur_moves(
    args: &[Expr],
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    later.retain(|&i| i < min_slot);
    for a in args.iter().rev() {
        walk_moves(a, slots, min_slot, later, out);
    }
}

fn walk_letfn_moves(
    fns: &[(String, FunExpr)],
    body: &[Expr],
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let depth = slots.len();
    slots.resize(depth + fns.len(), false);
    for b in body.iter().rev() {
        walk_moves(b, slots, min_slot, later, out);
    }
    slots.truncate(depth);
    for (_, fun) in fns {
        for a in fun.arities.iter() {
            for b in &a.body {
                reads(b, later);
            }
        }
    }
    later.retain(|&i| i < depth);
}

fn walk_children_moves(
    e: &Expr,
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let mut kids = Vec::new();
    for_each_child(e, &mut |c| kids.push(c));
    for c in kids.into_iter().rev() {
        walk_moves(c, slots, min_slot, later, out);
    }
}

fn walk_case_moves(
    expr: &Expr,
    default: &Expr,
    cases: &[(i32, Expr, Expr)],
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let base_later = later.clone();
    let mut merged = later.clone();
    walk_moves(default, slots, min_slot, &mut merged, out);
    for (_, test, then) in cases {
        let mut br = base_later.clone();
        walk_moves(then, slots, min_slot, &mut br, out);
        reads(test, &mut br);
        merged.extend(br);
    }
    *later = merged;
    walk_moves(expr, slots, min_slot, later, out);
}

fn walk_bindings_rev(
    bindings: &[(Pattern, Expr)],
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    for (pat, init) in bindings.iter().rev() {
        pattern_children(pat, &mut |def_e| {
            reads(def_e, later);
        });
        let n = native_abi::pattern_local_count(pat);
        let new_len = slots.len() - n;
        slots.truncate(new_len);
        // Shadowed/inner locals at indices >= new_len are out of scope in `init`.
        later.retain(|&i| i < new_len);
        walk_moves(init, slots, min_slot, later, out);
    }
}

fn push_binding_slots(bindings: &[(Pattern, Expr)], slots: &mut Vec<bool>) {
    for (pat, _) in bindings {
        let n = native_abi::pattern_local_count(pat);
        slots.resize(slots.len() + n, true);
    }
}

fn walk_let_moves(
    bindings: &[(Pattern, Expr)],
    body: &[Expr],
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let base = slots.len();
    push_binding_slots(bindings, slots);
    for b in body.iter().rev() {
        walk_moves(b, slots, min_slot, later, out);
    }
    walk_bindings_rev(bindings, slots, min_slot, later, out);
    later.retain(|&i| i < base);
}

fn walk_loop_moves(
    bindings: &[(Pattern, Expr)],
    body: &[Expr],
    slots: &mut Vec<bool>,
    outer_min: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    let loop_base = slots.len();
    push_binding_slots(bindings, slots);
    let mut body_later = later.clone();
    body_later.retain(|&i| i < loop_base);
    for b in body.iter().rev() {
        walk_moves(b, slots, loop_base, &mut body_later, out);
    }
    later.extend(body_later);
    walk_bindings_rev(bindings, slots, outer_min, later, out);
    later.retain(|&i| i < loop_base);
}

fn walk_extent_moves(
    e: &Expr,
    slots: &mut Vec<bool>,
    min_slot: usize,
    later: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) -> bool {
    let depth = slots.len();
    match e {
        Expr::Fn(_)
        | Expr::BoundFn { .. }
        | Expr::DefMacro { .. }
        | Expr::LazySeqStar { .. }
        | Expr::DelayStar { .. }
        | Expr::Try { .. }
        | Expr::Sync { .. }
        | Expr::Io { .. }
        | Expr::Time { .. }
        | Expr::WithOutStr { .. } => {
            reads(e, later);
            later.retain(|&i| i < depth);
            true
        }
        Expr::Binding { pairs, body } | Expr::WithRedefs { pairs, body } => {
            body.iter().for_each(|b| reads(b, later));
            later.retain(|&i| i < depth);
            pairs
                .iter()
                .rev()
                .for_each(|(_, v)| walk_moves(v, slots, min_slot, later, out));
            true
        }
        Expr::WithBindings { map: arg, body }
        | Expr::Locking { lockee: arg, body }
        | Expr::WithInStr { s: arg, body, .. } => {
            body.iter().for_each(|b| reads(b, later));
            later.retain(|&i| i < depth);
            walk_moves(arg, slots, min_slot, later, out);
            true
        }
        _ => false,
    }
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

/// Data and fn vars whose root never changes after their one top-level
/// `def`, mapped to that `def`'s unit index (#271). Only a Lite program
/// has them: no unit of one can re-root an arbitrary var
/// ([`can_lite_program`] rejects [`mentions_rebinding`]'s
/// `alter-var-root`, `with-redefs`, var-resolving fns, …, and `eval` is
/// not Lite). A var qualifies when exactly one `def` with an init names
/// it anywhere in the program, that `def` is a whole unit (it runs once,
/// at its place in program order), it is not `^:dynamic`, and the
/// program never `set!`s it or takes it as a var object (`#'v`) outside
/// the inert units `defn` itself emits.
fn stable_vars(units: &[TopUnit]) -> HashMap<(String, String), (usize, StableKind)> {
    debug_assert!(!units.iter().any(|u| mentions_rebinding(&u.expr)));
    let key = |sym: &Symbol| {
        (
            sym.namespace().unwrap_or("user").to_owned(),
            sym.name().to_owned(),
        )
    };
    let mut defs: HashMap<(String, String), usize> = HashMap::new();
    let mut tainted: HashSet<(String, String)> = HashSet::new();
    for u in units {
        let inert = matches!(u.expr, Expr::VarRef(_)) || is_inert_meta_statement(&u.expr);
        visit_var_uses(&u.expr, inert, &mut |sym, defines| {
            if defines {
                *defs.entry(key(sym)).or_default() += 1;
            } else {
                tainted.insert(key(sym));
            }
        });
    }
    units
        .iter()
        .enumerate()
        .filter_map(|(i, u)| match &u.expr {
            Expr::Def {
                sym,
                init: Some(init),
                meta,
                ..
            } if !is_dynamic_def(sym, meta.as_deref())
                && defs.get(&key(sym)) == Some(&1)
                && !tainted.contains(&key(sym)) =>
            {
                Some((key(sym), (i, stable_kind(init))))
            }
            _ => None,
        })
        .collect()
}

fn expr_has_loop(e: &Expr) -> bool {
    if matches!(e, Expr::Loop { .. }) {
        return true;
    }
    let mut hit = false;
    for_each_child(e, &mut |c| {
        if !hit && expr_has_loop(c) {
            hit = true;
        }
    });
    hit
}

fn expr_reads_local(e: &Expr, idx: usize) -> bool {
    if let Expr::Local(i) = e
        && *i == idx
    {
        return true;
    }
    let mut hit = false;
    for_each_child(e, &mut |c| {
        if !hit && expr_reads_local(c, idx) {
            hit = true;
        }
    });
    hit
}

fn pat_reads_local(pat: &Pattern, idx: usize) -> bool {
    let mut hit = false;
    pattern_children(pat, &mut |e| {
        if !hit && expr_reads_local(e, idx) {
            hit = true;
        }
    });
    hit
}

fn is_pod_expr(e: &Expr) -> bool {
    match e {
        Expr::Literal(
            Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Double(_) | Value::Char(_),
        ) => true,
        Expr::DirectNative(call) => {
            call.var.sym.namespace() == Some("clojure.core")
                && matches!(
                    call.var.sym.name(),
                    "count"
                        | "+"
                        | "-"
                        | "*"
                        | "inc"
                        | "dec"
                        | "quot"
                        | "rem"
                        | "mod"
                        | "max"
                        | "min"
                        | "abs"
                        | "bit-and"
                        | "bit-or"
                        | "bit-xor"
                        | "bit-not"
                        | "bit-shift-left"
                        | "bit-shift-right"
                        | "unsigned-bit-shift-right"
                        | "="
                        | "=="
                        | "not="
                        | "<"
                        | "<="
                        | ">"
                        | ">="
                        | "not"
                        | "zero?"
                        | "pos?"
                        | "neg?"
                        | "nil?"
                        | "some?"
                        | "even?"
                        | "odd?"
                        | "int"
                        | "long"
                        | "double"
                        | "char"
                        | "boolean"
                        | "unchecked-add"
                        | "unchecked-subtract"
                        | "unchecked-multiply"
                        | "unchecked-inc"
                        | "unchecked-dec"
                        | "unchecked-negate"
                )
        }
        _ => false,
    }
}

fn drops_unit(e: &Expr, var_meta: bool) -> bool {
    matches!(e, Expr::VarRef(_) | Expr::Literal(_) | Expr::Quote(_))
        || (!var_meta && is_inert_meta_statement(e))
}

fn record_unit_var_reads(u: &TopUnit, reads: &mut HashSet<(String, String)>) {
    let own = match &u.expr {
        Expr::Def {
            sym,
            init: Some(init),
            ..
        } if matches!(init.as_ref(), Expr::Fn(_)) => Some(var_key(sym)),
        _ => None,
    };
    visit_var_reads(&u.expr, &mut |sym| {
        let k = var_key(sym);
        if own.as_ref() != Some(&k) {
            reads.insert(k);
        }
    });
}

/// The top-level `def`s a Lite program can drop (#271): a stable var
/// nothing reads, whose init is pure and total ([`is_pure_total`]), in a
/// program that cannot find vars by name. The snippets' boilerplate
/// tables are mostly unused (an unread `digit-values` `zipmap` in 177 of
/// 220 scripts, `separators` sets, `max-long`, unused `defn`s), and each
/// cost startup work. A dropped def's own reads no longer count, so the
/// set grows to a fixpoint (`max-long` frees `min-long`).
fn dead_defs<V>(
    units: &[TopUnit],
    stable: &HashMap<(String, String), V>,
    var_meta: bool,
) -> HashSet<usize> {
    let mut dead = HashSet::new();
    if units.iter().any(|u| finds_vars_by_name(&u.expr)) {
        return dead;
    }
    let longs = long_vars(units);
    loop {
        let mut reads: HashSet<(String, String)> = HashSet::new();
        for (i, u) in units.iter().enumerate() {
            if !dead.contains(&i) && !drops_unit(&u.expr, var_meta) {
                record_unit_var_reads(u, &mut reads);
            }
        }
        let grown: Vec<usize> = (units.iter().enumerate())
            .filter(|&(i, u)| !dead.contains(&i) && is_dead_def(&u.expr, stable, &reads, &longs))
            .map(|(i, _)| i)
            .collect();
        if grown.is_empty() {
            return dead;
        }
        dead.extend(grown);
    }
}

/// `(ns, name)` of a var symbol (`user` when unqualified).
fn var_key(sym: &Symbol) -> (String, String) {
    (
        sym.namespace().unwrap_or("user").to_owned(),
        sym.name().to_owned(),
    )
}

/// Whether unit `e` is a droppable `def` (see [`dead_defs`]).
fn is_dead_def<V>(
    e: &Expr,
    stable: &HashMap<(String, String), V>,
    reads: &HashSet<(String, String)>,
    longs: &HashSet<(String, String)>,
) -> bool {
    matches!(e, Expr::Def { sym, init: Some(init), meta, .. }
    if stable.contains_key(&var_key(sym))
        && !reads.contains(&var_key(sym))
        && is_pure_total(init, longs)
        && meta.as_deref().is_none_or(|ps| {
            ps.iter().all(|(k, v)| is_pure_total(k, longs) && is_pure_total(v, longs))
        }))
}

/// Report every var `e` reads, binds, sets, declares or takes as an
/// object, recursively (a `def`'s own name is not a read).
fn visit_var_reads(e: &Expr, report: &mut dyn FnMut(&Symbol)) {
    match e {
        Expr::Global(v) | Expr::VarRef(v) | Expr::Set { var: v, .. } => report(&v.sym),
        Expr::DirectNative(call) => report(&call.var.sym),
        Expr::Binding { pairs, .. } | Expr::WithRedefs { pairs, .. } => {
            pairs.iter().for_each(|(v, _)| report(&v.sym));
        }
        Expr::Declare { syms, .. } => syms.iter().for_each(&mut *report),
        _ => {}
    }
    for_each_child(e, &mut |c| visit_var_reads(c, report));
}

/// Whether `e` can reach a var by name at run time (`resolve`,
/// `ns-publics`, `eval`, …): a program that can keeps every `def`.
fn finds_vars_by_name(e: &Expr) -> bool {
    let finder = |sym: &Symbol| {
        sym.namespace() == Some("clojure.core")
            && matches!(
                sym.name(),
                "resolve"
                    | "ns-resolve"
                    | "find-var"
                    | "intern"
                    | "ns-publics"
                    | "ns-interns"
                    | "ns-map"
                    | "ns-refers"
                    | "ns-unmap"
                    | "requiring-resolve"
                    | "refer"
                    | "eval"
                    | "load"
                    | "load-file"
                    | "load-reader"
                    | "load-string"
                    | "var-get"
            )
    };
    let mut found = false;
    visit_var_reads(e, &mut |sym| found |= finder(sym));
    found
}

/// The stable-candidate vars whose `def` init is a long expression
/// ([`is_long_expr`]), in unit order.
fn long_vars(units: &[TopUnit]) -> HashSet<(String, String)> {
    let mut longs = HashSet::new();
    for u in units {
        if let Expr::Def {
            sym,
            init: Some(init),
            ..
        } = &u.expr
            && is_long_expr(init, &longs)
        {
            longs.insert(var_key(sym));
        }
    }
    longs
}

/// A direct `clojure.core` native call's name and argument forms (a Lite
/// program's core calls all analyze to direct natives).
fn core_call(e: &Expr) -> Option<(&str, &[Expr])> {
    let Expr::DirectNative(call) = e else {
        return None;
    };
    let sym = &call.var.sym;
    (sym.namespace() == Some("clojure.core")).then(|| (sym.name(), &call.args[..]))
}

#[allow(clippy::type_complexity)]
fn char_selection(
    call: &clj_eval::expr::DirectCall,
    local: usize,
) -> Option<(&clj_eval::expr::DirectCall, &Expr, &Expr, u32, u8)> {
    if call.var.sym.namespace() != Some("clojure.core") {
        return None;
    }
    let [Expr::DirectNative(inner)] = call.args.as_slice() else {
        return None;
    };
    if inner.var.sym.namespace() != Some("clojure.core") {
        return None;
    }
    let mode = match (call.var.sym.name(), inner.var.sym.name()) {
        ("vec", "remove") => 2,
        ("vec", "drop-while") => 3,
        _ => return None,
    };
    let [predicate, coll] = inner.args.as_slice() else {
        return None;
    };
    let (_, arity) = inline_fn(predicate, 1)?;
    Some((inner, predicate, coll, char_predicate(arity, local)?, mode))
}

fn char_predicate(arity: &Arity, local: usize) -> Option<u32> {
    let [body] = arity.body.as_slice() else {
        return None;
    };
    let ("=", [a, b]) = core_call(body)? else {
        return None;
    };
    match (a, b) {
        (Expr::Local(i), Expr::Literal(Value::Char(c)))
        | (Expr::Literal(Value::Char(c)), Expr::Local(i))
            if *i == local =>
        {
            Some(u32::from(*c))
        }
        _ => None,
    }
}

fn mapped_str_join(call: &clj_eval::expr::DirectCall) -> Option<(&str, &Expr)> {
    if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "apply" {
        return None;
    }
    let [str_fn, interposed] = call.args.as_slice() else {
        return None;
    };
    let Some(("interpose", [Expr::Literal(Value::Str(sep)), mapped])) = core_call(interposed)
    else {
        return None;
    };
    let Some(("map", [map_fn, input])) = core_call(mapped) else {
        return None;
    };
    (is_core_global(str_fn, "str") && is_core_global(map_fn, "str"))
        .then_some((sep.as_ref(), input))
}

fn scaled_long_args(call: &clj_eval::expr::DirectCall) -> Option<[&Expr; 3]> {
    if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "long" {
        return None;
    }
    let [quotient] = call.args.as_slice() else {
        return None;
    };
    let Some(("quot", [product, divisor])) = core_call(quotient) else {
        return None;
    };
    let Some(("*", [lhs, rhs])) = core_call(product) else {
        return None;
    };
    let args = [lhs, rhs, divisor];
    args.iter()
        .all(|e| {
            matches!(
                e,
                Expr::Local(_) | Expr::Literal(Value::Int(_) | Value::BigInt(_))
            )
        })
        .then_some(args)
}

fn stable_kind(init: &Expr) -> StableKind {
    match init {
        Expr::Literal(Value::Map(_) | Value::Set(_) | Value::Keyword(_) | Value::Symbol(_))
        | Expr::Quote(Value::Map(_) | Value::Set(_) | Value::Keyword(_) | Value::Symbol(_))
        | Expr::MapLit(..)
        | Expr::SetLit(..) => StableKind::Lookup,
        Expr::Literal(v) | Expr::Quote(v) => {
            clj_eval::aot_scalar::Sv::of(v).map_or(StableKind::Unknown, StableKind::Scalar)
        }
        other if matches!(core_call(other), Some(("zipmap", [_, _]))) => StableKind::Lookup,
        _ => StableKind::Unknown,
    }
}

/// Fuse only inert indices. An index expression may mutate the atom or
/// throw after the dereference, and a nested lookup can itself throw.
fn atom_nth_args(
    call: &clj_eval::expr::DirectCall,
) -> Option<(&clj_value::Var, &Expr, Vec<&Expr>)> {
    if call.var.sym.namespace() != Some("clojure.core") || call.var.sym.name() != "nth" {
        return None;
    }
    let [coll, idx] = call.args.as_slice() else {
        return None;
    };
    let (source, indices) = match core_call(coll) {
        Some(("nth", [source, row])) => (source, vec![row, idx]),
        _ => (coll, vec![idx]),
    };
    if !indices
        .iter()
        .all(|e| matches!(e, Expr::Local(_) | Expr::Literal(Value::Int(_))))
    {
        return None;
    }
    let Expr::DirectNative(deref) = source else {
        return None;
    };
    let Some(("deref", [atom])) = core_call(source) else {
        return None;
    };
    Some((&deref.var, atom, indices))
}

/// Whether evaluating `e` can neither fail nor be observed: a constant,
/// a collection literal of constants, a long expression, or `(zipmap
/// "text" (range n?))`.
fn is_pure_total(e: &Expr, longs: &HashSet<(String, String)>) -> bool {
    let constant = |x: &Expr| matches!(x, Expr::Literal(_) | Expr::Quote(_));
    match e {
        Expr::Literal(_) | Expr::Quote(_) | Expr::Fn(_) => true,
        Expr::VecLit(items, None) | Expr::SetLit(items, None) => items.iter().all(constant),
        Expr::MapLit(pairs, None) => pairs.iter().all(|(k, v)| constant(k) && constant(v)),
        _ => {
            is_long_expr(e, longs)
                || matches!(core_call(e), Some(("zipmap", [Expr::Literal(Value::Str(_)), r]))
                    if matches!(core_call(r), Some(("range", []))
                        | Some(("range", [Expr::Literal(Value::Int(_))]))))
        }
    }
}

/// Whether `e` is a long built from long literals, known-long vars and
/// the (total) bit operations.
fn is_long_expr(e: &Expr, longs: &HashSet<(String, String)>) -> bool {
    match e {
        Expr::Literal(Value::Int(_)) => true,
        Expr::Global(v) => longs.contains(&var_key(&v.sym)),
        _ => match core_call(e) {
            Some(("bit-not", [a])) => is_long_expr(a, longs),
            Some((
                "bit-and"
                | "bit-or"
                | "bit-xor"
                | "bit-shift-left"
                | "bit-shift-right"
                | "unsigned-bit-shift-right",
                [a, b],
            )) => is_long_expr(a, longs) && is_long_expr(b, longs),
            _ => false,
        },
    }
}

/// Report every var `e` defines with an init (`true`) or exposes to a
/// root change (`false`: a `set!` target, or `#'v` outside an `inert`
/// unit), recursively.
fn visit_var_uses(e: &Expr, inert: bool, report: &mut dyn FnMut(&Symbol, bool)) {
    match e {
        Expr::Def {
            sym, init: Some(_), ..
        } => report(sym, true),
        Expr::Set { var, .. } => report(&var.sym, false),
        Expr::VarRef(v) if !inert => report(&v.sym, false),
        _ => {}
    }
    for_each_child(e, &mut |c| visit_var_uses(c, inert, report));
}

/// Whether a `def`'s name metadata (constant, on the symbol, or as
/// forms) carries `:dynamic`.
fn is_dynamic_def(sym: &Symbol, meta: Option<&[(Expr, Expr)]>) -> bool {
    let dynamic = Value::Keyword(clj_value::Keyword::new(None, "dynamic"));
    sym.meta().is_some_and(|m| m.get(&dynamic).is_some())
        || meta.is_some_and(|pairs| {
            pairs
                .iter()
                .any(|(k, _)| matches!(k, Expr::Literal(v) | Expr::Quote(v) if *v == dynamic))
        })
}

/// Whether constant `v` lives in the pool (everything `lower_const`
/// does not write inline).
fn is_pooled(v: &Value) -> bool {
    !matches!(
        v,
        Value::Nil
            | Value::Int(_)
            | Value::Bool(_)
            | Value::Double(_)
            | Value::Char(_)
            | Value::Var(_)
    )
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
        // Redefining a `clojure.string` var only conflicts with a Lite
        // native that serves it (a spliced definition has none, #271).
        Expr::Def { sym, .. } => match sym.namespace() {
            Some("clojure.core") => true,
            Some("clojure.string") => clj_eval::aot_boot::is_lite_string_var(sym.name()),
            _ => false,
        },
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

/// Whether `front` can boot cleanly under `BootMode::Lite` (no embedded
/// stdlib / compiler / runtime eval / non-Lite namespaces or rebinds).
fn can_lite_program(front: &FrontEnd) -> bool {
    let mut defined = BTreeSet::new();
    for u in &front.units {
        if mentions_rebinding(&u.expr) {
            return false;
        }
        collect_lite_defs(&u.expr, &mut defined);
    }
    let scope = LiteScope {
        defined,
        libs: &front.libs,
    };
    front.units.iter().all(|u| expr_lite_ok(&u.expr, &scope))
}

/// What the Lite check resolves against: the program's own definitions
/// (spliced library units included) and the spliced libraries its
/// leading `ns` form may require (#271).
struct LiteScope<'a> {
    defined: BTreeSet<(String, String)>,
    libs: &'a [&'static str],
}

fn sym_key(sym: &Symbol) -> (String, String) {
    (
        sym.namespace().unwrap_or("user").to_owned(),
        sym.name().to_owned(),
    )
}

/// Whether a program references a reader of var metadata
/// ([`clj_eval::aot_boot::needs_var_meta`] over every var it names).
fn program_reads_var_meta(units: &[TopUnit]) -> bool {
    let mut refs = Vec::new();
    let mut var_objects = false;
    for u in units.iter().filter(|u| !is_inert_meta_statement(&u.expr)) {
        collect_var_refs(&u.expr, &mut refs);
        // A unit that is just `(var name)` closes a split `defn`.
        var_objects |= !matches!(u.expr, Expr::VarRef(_)) && holds_var_object(&u.expr);
    }
    let refs: Vec<(&str, &str)> = refs
        .iter()
        .map(|(ns, name)| (ns.as_str(), name.as_str()))
        .collect();
    clj_eval::aot_boot::needs_var_meta(&refs, var_objects)
}

/// Whether `e` can hand the program a var object: a `#'v` anywhere but
/// `defn`'s own forms (its inert metadata writes, and the `(var name)`
/// closing a `do` that defines `name`).
fn holds_var_object(e: &Expr) -> bool {
    match e {
        Expr::VarRef(_) => true,
        Expr::Do(body) => body.iter().enumerate().any(|(i, s)| match s {
            _ if is_inert_meta_statement(s) => false,
            Expr::VarRef(v) if i + 1 == body.len() && defines_name(&body[..i], v.sym.name()) => {
                false
            }
            _ => holds_var_object(s),
        }),
        _ => {
            let mut found = false;
            for_each_child(e, &mut |c| found = found || holds_var_object(c));
            found
        }
    }
}

/// Whether one of `forms` is a `def` of `name`.
fn defines_name(forms: &[Expr], name: &str) -> bool {
    forms
        .iter()
        .any(|f| matches!(f, Expr::Def { sym, .. } if sym.name() == name))
}

fn collect_var_refs(e: &Expr, out: &mut Vec<(String, String)>) {
    match e {
        // Discarded inert metadata writes are what this decides on
        // dropping (they are no reader).
        Expr::Do(body) => {
            let last = body.len().saturating_sub(1);
            let kept = body
                .iter()
                .enumerate()
                .filter(|&(i, s)| i == last || !is_inert_meta_statement(s));
            for (_, s) in kept {
                collect_var_refs(s, out);
            }
            return;
        }
        // (`binding` / `set!` name only dynamic vars, never a reader.)
        Expr::Global(v) | Expr::VarRef(v) => out.push(sym_key(&v.sym)),
        Expr::DirectNative(call) => out.push(sym_key(&call.var.sym)),
        _ => {}
    }
    for_each_child(e, &mut |c| collect_var_refs(c, out));
}

/// Whether `sym` is `clojure.core/name`.
fn is_core_sym(sym: &Symbol, name: &str) -> bool {
    sym.namespace() == Some("clojure.core") && sym.name() == name
}

/// `defn`'s discarded `(alter-meta! (var name) assoc :arglists '…)`
/// (and a `merge` of a constant map): a write of
/// [`clj_eval::aot_boot::LITE_INERT_DEF_META`] keys only, with constant
/// values — nothing but a var-metadata reader can tell it ran.
fn is_inert_meta_statement(e: &Expr) -> bool {
    let Some((head, args)) = global_call(e) else {
        return false;
    };
    let [Expr::VarRef(_), Expr::Global(f), rest @ ..] = args else {
        return false;
    };
    if !is_core_sym(head, "alter-meta!") {
        return false;
    }
    if is_core_sym(&f.sym, "assoc") {
        return is_inert_assoc(rest);
    }
    is_core_sym(&f.sym, "merge") && is_inert_merge(rest)
}

/// The head var's symbol and the arguments of a call to a global.
fn global_call(e: &Expr) -> Option<(&Symbol, &[Expr])> {
    match e {
        Expr::DirectNative(call) => Some((&call.var.sym, call.args.as_slice())),
        Expr::Invoke { head, args, .. } => match &**head {
            Expr::Global(v) => Some((&v.sym, args.as_slice())),
            _ => None,
        },
        _ => None,
    }
}

/// `assoc`'s `k v …` arguments: inert keys with constant values only.
fn is_inert_assoc(kvs: &[Expr]) -> bool {
    !kvs.is_empty()
        && kvs.len().is_multiple_of(2)
        && kvs.chunks(2).all(|kv| is_inert_meta_pair(&kv[0], &kv[1]))
}

/// `merge`'s argument: one constant map of inert keys.
fn is_inert_merge(args: &[Expr]) -> bool {
    match args {
        [Expr::Literal(Value::Map(m)) | Expr::Quote(Value::Map(m))] => {
            m.iter().all(|(k, _)| is_inert_write_key(k))
        }
        _ => false,
    }
}

/// A metadata `key value` pair with an inert key and a constant value.
fn is_inert_meta_pair(k: &Expr, v: &Expr) -> bool {
    matches!(k, Expr::Literal(key) if is_inert_write_key(key))
        && matches!(v, Expr::Literal(_) | Expr::Quote(_))
}

/// Whether a discarded constant metadata write of `key` (`defn`'s
/// `:arglists`, an attr map's `:added`, …) is unobservable without a
/// var-metadata reader (#271): any unqualified keyword except the keys
/// the runtime acts on (`:dynamic`, `:private` for `refer`, `:macro`,
/// `:redef`). A type hint (`:tag`) only steers analysis, which ran at
/// build time.
fn is_inert_write_key(key: &Value) -> bool {
    matches!(key, Value::Keyword(k) if k.namespace().is_none()
        && !matches!(k.name(), "dynamic" | "private" | "macro" | "redef"))
}

/// Whether `key` names a [`clj_eval::aot_boot::LITE_INERT_DEF_META`] key.
fn is_inert_def_meta_key(key: &Value) -> bool {
    matches!(key, Value::Keyword(k) if k.namespace().is_none()
        && clj_eval::aot_boot::LITE_INERT_DEF_META.contains(&k.name()))
}

/// A `def` name and its evaluated metadata pairs without the keys no
/// runtime path of a program that never reads var metadata can see
/// (only side-effect-free pairs drop; an emptied map goes away).
fn without_inert_def_meta(
    sym: &Symbol,
    pairs: Option<&[(Expr, Expr)]>,
) -> (Symbol, Option<Vec<(Expr, Expr)>>) {
    let meta = sym.meta().map(|m| {
        m.iter()
            .map(|(k, _)| k.clone())
            .filter(is_inert_def_meta_key)
            .fold(m.clone(), |m, k| m.without(&k))
    });
    let sym = sym.clone().with_meta(meta.filter(|m| !m.is_empty()));
    let pairs = pairs.map(|pairs| {
        pairs
            .iter()
            .filter(|(k, v)| !is_inert_meta_pair(k, v))
            .cloned()
            .collect::<Vec<_>>()
    });
    (sym, pairs.filter(|p| !p.is_empty()))
}

fn collect_lite_defs(e: &Expr, defined: &mut BTreeSet<(String, String)>) {
    match e {
        Expr::Def { sym, .. } => {
            defined.insert(sym_key(sym));
        }
        Expr::Declare { syms, .. } => {
            for sym in syms {
                defined.insert(sym_key(sym));
            }
        }
        _ => {}
    }
    for_each_child(e, &mut |c| collect_lite_defs(c, defined));
}

fn var_lite_ok(sym: &Symbol, defined: &BTreeSet<(String, String)>) -> bool {
    let ns = sym.namespace().unwrap_or("user");
    let name = sym.name();
    match ns {
        "clojure.core" => clj_eval::aot_boot::is_lite_core_var(name),
        // A `clojure.string` var without a Lite native may be spliced
        // in (#271): defined by the program's own units.
        "clojure.string" => {
            clj_eval::aot_boot::is_lite_string_var(name)
                || defined.contains(&(ns.to_owned(), name.to_owned()))
        }
        _ => defined.contains(&(ns.to_owned(), name.to_owned())),
    }
}

fn is_clojure_string_sym(v: &Value) -> bool {
    matches!(v, Value::Symbol(s) if s.namespace().is_none() && s.name() == "clojure.string")
}

fn lite_require_spec_alias(v: &Value) -> Option<Option<String>> {
    if is_clojure_string_sym(v) {
        return Some(None);
    }
    let Value::Vector(items) = v else {
        return None;
    };
    let mut it = items.iter();
    if !it.next().is_some_and(|first| is_clojure_string_sym(&first)) {
        return None;
    }
    while let Some(k) = it.next() {
        if matches!(&k, Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == "as")
            && let Some(Value::Symbol(sym)) = it.next()
        {
            return Some(Some(sym.name().to_owned()));
        }
    }
    Some(None)
}

fn lite_direct_require_aliases(call: &clj_eval::expr::DirectCall) -> Option<Vec<Option<String>>> {
    if call.var.sym.namespace() != Some("clojure.core")
        || call.var.sym.name() != "require"
        || call.args.is_empty()
    {
        return None;
    }
    call.args
        .iter()
        .map(|arg| match arg {
            Expr::Quote(val) => lite_require_spec_alias(val),
            _ => None,
        })
        .collect()
}

fn ns_clause_lite_ok(clause: &NsClause, libs: &[&str]) -> bool {
    match clause {
        NsClause::ReferClojure(_) => true,
        NsClause::Require(specs) => specs
            .iter()
            .all(|v| lite_require_spec_alias(v).is_some() || libs.iter().any(|l| names_lib(v, l))),
        NsClause::Use(_) | NsClause::Import(_) => false,
    }
}

fn expr_lite_ok(e: &Expr, scope: &LiteScope<'_>) -> bool {
    if let Expr::DirectNative(call) = e
        && lite_direct_require_aliases(call).is_some()
    {
        return true;
    }
    let here = match e {
        Expr::DefMacro { .. } => false,
        Expr::Ns { clauses, .. } => clauses.iter().all(|c| ns_clause_lite_ok(c, scope.libs)),
        Expr::Global(v) | Expr::VarRef(v) | Expr::Set { var: v, .. } => {
            var_lite_ok(&v.sym, &scope.defined)
        }
        Expr::DirectNative(call) => var_lite_ok(&call.var.sym, &scope.defined),
        Expr::Binding { pairs, .. } | Expr::WithRedefs { pairs, .. } => pairs
            .iter()
            .all(|(v, _)| var_lite_ok(&v.sym, &scope.defined)),
        _ => true,
    };
    if !here {
        return false;
    }
    let mut ok = true;
    for_each_child(e, &mut |c| {
        if ok && !expr_lite_ok(c, scope) {
            ok = false;
        }
    });
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_templates_need_distinct_constant_keywords() {
        let kw = |n: &str| Expr::Literal(Value::Keyword(clj_value::Keyword::new(None, n)));
        let one = Expr::Literal(Value::Int(1));
        let template = keyword_template(&[(kw("a"), one.clone()), (kw("b"), one.clone())], None);
        assert!(matches!(template, Some(Value::Map(m)) if m.size() == 2));
        // A repeated key (the reader rejects one; a template would
        // mis-pair the values) and a non-keyword key pair up instead.
        assert!(
            keyword_template(&[(kw("a"), one.clone()), (kw("a"), one.clone())], None).is_none()
        );
        assert!(keyword_template(&[(one.clone(), one.clone())], None).is_none());
    }

    #[test]
    fn simple_collections_pool_without_the_reader() {
        use clj_value::{Keyword, List, SortSpec, new_map};
        let reads = |text: &str| native_abi::read_constant(text).expect("reads");
        // Nested data, catch class lists, and arglists with a tagged param.
        for text in [
            "[\"java.lang.Exception\"]",
            "{:a [1 2.5 \\c] \"b\" #{x/y nil}}",
            "([^CharSequence s re f] [])",
        ] {
            let v = reads(text).with_meta(None);
            assert!(matches!(coll_const(&v), Some(Const::Coll(..))), "{text}");
        }
        // Anything else reads back: metadata, a sorted map, an item with
        // no simple form, more forms than the cap, a non-collection.
        let tagged = Value::List(List::from_vec(vec![Value::Int(1)])).with_meta(Some(
            new_map().insert(Value::Keyword(Keyword::new(None, "line")), Value::Int(1)),
        ));
        let sorted = Value::Map(new_map().with_sort(Some(SortSpec::Natural)));
        let big = reads("[1N]");
        let wide = Value::Vector(clj_value::Vector::from_vec(vec![
            Value::Int(0);
            COLL_FORMS_MAX
        ]));
        for v in [tagged, sorted, big, wide, Value::Int(1)] {
            assert!(coll_const(&v).is_none(), "{v:?}");
        }
    }

    #[test]
    fn meta_symbols_pool_without_the_reader() {
        use clj_value::{Keyword, List, new_map};
        let kw = |n: &str| Value::Keyword(Keyword::new(None, n));
        let named = |pairs: Vec<(Value, Value)>| {
            let meta = pairs
                .into_iter()
                .fold(new_map(), |m, (k, v)| m.insert(k, v));
            Value::Symbol(Symbol::new(Some("user"), "f")).with_meta(Some(meta))
        };
        let simple = named(vec![
            (kw("private"), Value::Bool(true)),
            (kw("k"), Value::Keyword(Keyword::new(Some("a"), "b"))),
            (kw("tag"), Value::Symbol(Symbol::new(None, "String"))),
            (kw("doc"), Value::Str(std::sync::Arc::from("d"))),
        ]);
        assert!(matches!(
            meta_symbol_const(&simple),
            Some(Const::MetaSymbol { ns, name, meta })
                if ns.as_deref() == Some("user") && name == "f" && meta.len() == 4
        ));
        // Anything else reads back: a non-keyword key, a nested value, a
        // symbol carrying its own metadata, a plain symbol, a non-symbol.
        let list = Value::List(List::from_vec(vec![Value::Int(1)]));
        assert!(meta_symbol_const(&named(vec![(Value::Int(1), Value::Nil)])).is_none());
        assert!(meta_symbol_const(&named(vec![(kw("arglists"), list)])).is_none());
        assert!(meta_symbol_const(&named(vec![(kw("tag"), simple.clone())])).is_none());
        assert!(meta_symbol_const(&Value::Symbol(Symbol::new(None, "plain"))).is_none());
        assert!(meta_symbol_const(&Value::Int(1)).is_none());
    }

    #[test]
    fn static_call_op_needs_literal_class_and_member() {
        // The analyzer always synthesizes `__static*` with literal strings;
        // anything else (or a short arg list) takes the generic invoke.
        let lit = |s: &str| Expr::Quote(Value::Str(std::sync::Arc::from(s)));
        let abs = [
            lit("java.lang.Math"),
            lit("abs"),
            Expr::Literal(Value::Int(-5)),
        ];
        assert!(matches!(
            static_call_op(&abs),
            Some((StaticOp::MathAbs, [_]))
        ));
        assert!(static_call_op(&abs[..1]).is_none());
        let computed = [
            Expr::Literal(Value::Nil),
            lit("abs"),
            Expr::Literal(Value::Int(-5)),
        ];
        assert!(static_call_op(&computed).is_none());
        assert!(static_call_op(&[lit("java.lang.Math"), lit("signum"), lit("x")]).is_none());
    }

    #[test]
    #[should_panic(expected = "lower_tail_form partition")]
    fn lower_tail_form_rejects_upstream_expr() {
        let ctx = Context::create();
        let mut lowerer = Lowerer::with_bitcode(&ctx, false);
        let func = lowerer
            .module
            .add_function("test_fn", lowerer.code_ty, None);
        let entry = ctx.append_basic_block(func, "entry");
        let dest = lowerer.ptr.const_null();
        let mut f = FnCtx {
            func,
            entry,
            fail: entry,
            slots: Vec::new(),
            free_slots: Vec::new(),
            free_arrays: HashMap::new(),
            let_slots: Vec::new(),
            env: Vec::new(),
            recur: Vec::new(),
            captured: dest,
            moves: Vec::new(),
            inline_depth: 0,
            inlined_count: 0,
            tail_min: usize::MAX,
            in_tail: false,
            borrowed_params: HashMap::new(),
            pod_locals: HashSet::new(),
        };
        let _ = lowerer.lower_tail_form(&mut f, &Expr::Literal(Value::Nil), dest);
    }
}
