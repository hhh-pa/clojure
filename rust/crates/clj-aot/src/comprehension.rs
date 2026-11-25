// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Native `for` comprehensions (issue #271 Slice 6a).
//!
//! `(vec (for …))` and `(into [] (for …))` whose `for` nests bindings or
//! filters (`:when`, `:let`) analyze to lazy `mapcat` / `__for-keep*`
//! stages with a closure per item (`clj-eval/src/core_macros.rs`,
//! `comprehension`). Consumed whole by `vec`, they run here as nested
//! native loops pushing every item into one vector builder: each stage
//! over a vector or a long range indexes its items, binds its `fn`
//! param, and lowers its `:let`s, `:when`s and body inline; any other
//! collection takes the stage's own core call over it (the closure the
//! analysis made) and appends the items it yields. The items, their
//! order, and every side effect's order are those of the lazy form,
//! which `vec` realizes in full.

use clj_eval::expr::{DirectCall, Expr, FunExpr, Pattern};
use clj_value::Value;
use inkwell::IntPredicate;
use inkwell::values::PointerValue;

use super::{FnCtx, LocalRef, Lowerer, Unsupported, inline_fn, is_core_global};

/// How a stage yields items.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `(map F xs)`: one item per element, `F`'s value.
    Map,
    /// `(__for-keep* F xs skip)`: the innermost stage under `:when`s, an
    /// item per element whose chain reaches its body.
    Keep,
    /// `(mapcat F xs)`: an outer stage, `F`'s body the next stage.
    Cat,
}

/// One stage of an expanded `for`, ready to lower.
struct Stage<'a> {
    kind: Kind,
    call: &'a DirectCall,
    fun: &'a FunExpr,
    coll: &'a Expr,
    body: Step<'a>,
}

/// A stage `fn`'s body: its `:let`s and `:when`s down to the item or the
/// next stage.
enum Step<'a> {
    Let {
        bindings: &'a [(Pattern, Expr)],
        body: &'a [Expr],
        next: Box<Step<'a>>,
    },
    When {
        test: &'a Expr,
        next: Box<Step<'a>>,
    },
    Item(&'a [Expr]),
    Stage(Box<Stage<'a>>),
}

/// `e` as a comprehension stage whose whole shape lowers here.
fn stage(e: &Expr) -> Option<Stage<'_>> {
    let Expr::DirectNative(call) = e else {
        return None;
    };
    if call.var.sym.namespace() != Some("clojure.core") {
        return None;
    }
    let (kind, fe, coll) = match (call.var.sym.name(), call.args.as_slice()) {
        ("map", [fe, coll]) => (Kind::Map, fe, coll),
        ("mapcat", [fe, coll]) => (Kind::Cat, fe, coll),
        ("__for-keep*", [fe, coll, skip]) if is_core_global(skip, "__for-keep*") => {
            (Kind::Keep, fe, coll)
        }
        _ => return None,
    };
    let (fun, arity) = inline_fn(fe, 1)?;
    let body = match kind {
        Kind::Map => Step::Item(&arity.body),
        _ => step(&arity.body, kind)?,
    };
    Some(Stage {
        kind,
        call,
        fun,
        coll,
        body,
    })
}

/// A `Keep` or `Cat` stage `fn`'s body as the expansion builds it: `let`s
/// and `if`s whose else is the stage's skip (the `__for-keep*` sentinel,
/// or `nil` for an outer stage), ending in the item (which must not name
/// the sentinel) or the next stage.
fn step(body: &[Expr], kind: Kind) -> Option<Step<'_>> {
    match body {
        [Expr::Let { bindings, body }] => Some(Step::Let {
            bindings,
            body,
            next: Box::new(step(body, kind)?),
        }),
        [Expr::If { test, then, els }] if skips(els, kind) => Some(Step::When {
            test,
            next: Box::new(step(std::slice::from_ref(then), kind)?),
        }),
        [e] if kind == Kind::Keep => (!names_skip(e)).then_some(Step::Item(body)),
        [e] => stage(e).map(|s| Step::Stage(Box::new(s))),
        _ => None,
    }
}

/// Whether `els` is a `:when`'s skip in a `kind` stage.
fn skips(els: &Expr, kind: Kind) -> bool {
    match kind {
        Kind::Keep => is_core_global(els, "__for-keep*"),
        _ => matches!(els, Expr::Literal(Value::Nil)),
    }
}

/// Whether `e` mentions the `__for-keep*` sentinel anywhere.
fn names_skip(e: &Expr) -> bool {
    let mut hit = is_core_global(e, "__for-keep*");
    clj_eval::aot_front::for_each_child(e, &mut |c| hit |= names_skip(c));
    hit
}

impl<'ctx> Lowerer<'ctx> {
    /// `dest = (vec E)` / `(into [] E)` for a comprehension `E` with nested
    /// bindings or filters (a single unfiltered binding is a traversal
    /// loop already, see `hof_loop`).
    pub(super) fn try_lower_comprehension(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let e = match (call.var.sym.name(), call.args.as_slice()) {
            ("vec", [e]) => e,
            ("into", [Expr::VecLit(items, None), e]) if items.is_empty() => e,
            _ => return Ok(false),
        };
        let Some(st) = stage(e).filter(|s| s.kind != Kind::Map) else {
            return Ok(false);
        };
        let acc = self.slot(f);
        self.call(
            "rt_n_vec_new",
            None,
            &[
                acc.into(),
                self.i64t.const_zero().into(),
                self.i8t.const_zero().into(),
            ],
        );
        // The analysis saw each stage `fn` as a closure made once, so its
        // last-use moves may take a local an inline body reads again on the
        // next item: no moves here, and no tail position inside the loops.
        let moves = std::mem::take(&mut f.moves);
        let tail = std::mem::replace(&mut f.in_tail, false);
        let lowered = self.emit_stage(f, &st, acc);
        f.moves = moves;
        f.in_tail = tail;
        lowered?;
        self.call("rt_n_vec_done", None, &[dest.into(), acc.into()]);
        self.free_slots(f, &[acc]);
        Ok(true)
    }

    /// One stage's items into `acc`: indexed over a vector or long range,
    /// else through the stage's own core call.
    fn emit_stage(
        &mut self,
        f: &mut FnCtx<'ctx>,
        st: &Stage<'_>,
        acc: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let base = f.env.len();
        let coll = self.slot(f);
        let (kind, plan, rsrc) = self.emit_coll_plan(f, st.coll, coll)?;
        let direct = self.ctx.append_basic_block(f.func, "comp_direct");
        let general = self.ctx.append_basic_block(f.func, "comp_general");
        let next = self.ctx.append_basic_block(f.func, "comp_next");
        let is_direct = self
            .builder
            .build_int_compare(IntPredicate::NE, kind, self.i8t.const_zero(), "direct")
            .expect("cmp");
        self.builder
            .build_conditional_branch(is_direct, direct, general)
            .expect("br");
        self.builder.position_at_end(direct);
        let state = self.load_loop_state(acc, coll, kind, plan);
        let i_ptr = self.entry_alloca(f, self.i64t, "comp_i");
        let cur_ptr = self.entry_alloca(f, self.i64t, "comp_cur");
        self.builder
            .build_store(i_ptr, self.i64t.const_zero())
            .expect("store");
        self.builder
            .build_store(cur_ptr, state.start)
            .expect("store");
        let head = self.ctx.append_basic_block(f.func, "comp_head");
        let body = self.ctx.append_basic_block(f.func, "comp_body");
        let exit = self.ctx.append_basic_block(f.func, "comp_exit");
        self.builder.build_unconditional_branch(head).expect("br");
        self.builder.position_at_end(head);
        let i = self
            .builder
            .build_load(self.i64t, i_ptr, "i")
            .expect("load")
            .into_int_value();
        let more = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, state.n, "more")
            .expect("cmp");
        self.builder
            .build_conditional_branch(more, body, exit)
            .expect("br");
        self.builder.position_at_end(body);
        let item = self.slot(f);
        self.emit_loop_item(f, &state, item, i, cur_ptr);
        let i_next = self
            .builder
            .build_int_add(i, self.i64t.const_int(1, false), "i_next")
            .expect("add");
        self.builder.build_store(i_ptr, i_next).expect("store");
        f.env.push(Some(LocalRef::Slot(item)));
        let stepped = self.emit_step(f, &st.body, acc);
        f.env.truncate(base);
        stepped?;
        self.drop_and_nil_slots(f.func, &[item]);
        self.builder.build_unconditional_branch(head).expect("br");
        self.builder.position_at_end(exit);
        f.free_slots.push(item);
        self.builder.build_unconditional_branch(next).expect("br");
        self.builder.position_at_end(general);
        let range_args = self.materialize_range_srcs(f, rsrc.as_ref(), None, coll, None);
        let closure = self.slot(f);
        self.make_closure(f, st.fun, base, closure)?;
        let mut args = vec![closure, coll];
        for extra in &st.call.args[2..] {
            let s = self.slot(f);
            self.lower(f, extra, s)?;
            args.push(s);
        }
        let seq = self.slot(f);
        self.core_call_owned(f, st.call, &args, seq);
        let status = self
            .call(
                "rt_n_vec_push_all",
                Some(self.i32t),
                &[acc.into(), seq.into()],
            )
            .expect("status");
        self.check(f, status);
        self.free_slots(f, &args);
        self.free_slots(f, &[seq]);
        self.free_slots(f, &range_args);
        self.builder.build_unconditional_branch(next).expect("br");
        self.builder.position_at_end(next);
        Ok(())
    }

    /// A stage `fn` body's `:let`s and `:when`s, then its item or next stage.
    fn emit_step(
        &mut self,
        f: &mut FnCtx<'ctx>,
        step: &Step<'_>,
        acc: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        match step {
            Step::Let {
                bindings,
                body,
                next,
            } => self.emit_let_step(f, bindings, body, next, acc),
            Step::When { test, next } => {
                let then_b = self.ctx.append_basic_block(f.func, "comp_when");
                let join = self.ctx.append_basic_block(f.func, "comp_skip");
                self.lower_cond(f, test, then_b, join)?;
                self.builder.position_at_end(then_b);
                self.emit_step(f, next, acc)?;
                self.builder.build_unconditional_branch(join).expect("br");
                self.builder.position_at_end(join);
                Ok(())
            }
            Step::Item(body) => {
                let res = self.slot(f);
                self.lower_body(f, body, res)?;
                self.call("rt_n_vec_push", None, &[acc.into(), res.into()]);
                self.free_slots(f, &[res]);
                Ok(())
            }
            Step::Stage(st) => self.emit_stage(f, st, acc),
        }
    }

    /// A `:let` step: its bindings as a `let` binds them (destructuring,
    /// dead drops), the rest of the body, then the scope's release.
    fn emit_let_step(
        &mut self,
        f: &mut FnCtx<'ctx>,
        bindings: &[(Pattern, Expr)],
        body: &[Expr],
        next: &Step<'_>,
        acc: PointerValue<'ctx>,
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let let_depth = f.let_slots.len();
        let tail_min = std::mem::replace(&mut f.tail_min, depth);
        let mut bound = Vec::new();
        let lowered = self
            .lower_let_bindings(f, bindings, body, &mut bound)
            .and_then(|()| {
                f.let_slots.extend_from_slice(&bound);
                self.emit_step(f, next, acc)
            });
        f.tail_min = tail_min;
        f.let_slots.truncate(let_depth);
        f.env.truncate(depth);
        f.pod_locals.retain(|&i| i < depth);
        self.free_slots(f, &bound);
        lowered
    }
}
