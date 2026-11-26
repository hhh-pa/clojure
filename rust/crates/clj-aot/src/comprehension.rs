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
    fun: StageFn<'a>,
    coll: &'a Expr,
    body: Step<'a>,
}

/// A stage's `fn`: a literal one, or (#271 Slice 6e) a fn by name
/// (`(map count rows)`, `(map parse-row lines)`: [`Lowerer::named_fn`]).
#[derive(Clone, Copy)]
enum StageFn<'a> {
    Literal(&'a FunExpr),
    Named(&'a Expr),
}

/// Whether a fn by name lowers as a `map` stage's one-arg eta `fn`.
type Named<'n> = dyn Fn(&Expr) -> bool + 'n;

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
    /// A `map` stage's fn by name over its param (Slice 6e).
    NamedItem(&'a Expr, clj_reader::Span),
    Stage(Box<Stage<'a>>),
}

/// Where a comprehension's items go.
#[derive(Clone, Copy)]
enum Sink<'ctx> {
    /// Pushed into a vector builder.
    Push(PointerValue<'ctx>),
    /// Folded into an accumulator by `+` (`(reduce + [init] E)`, Slice 6b);
    /// without an init, `first` holds 1 until the first item, which then
    /// becomes the accumulator as `reduce` takes it (no `+` call).
    Sum {
        acc: PointerValue<'ctx>,
        first: Option<PointerValue<'ctx>>,
    },
    /// `(into to E)` (Slice 6h).
    Chunk(Chunked<'ctx>),
}

/// An `into` sink: items pushed into `acc`, which every [`CHUNK`] items
/// `into` (var `k`, op `op`) pours into `to` (`n` counts the pending
/// ones), so the items never all live at once.
#[derive(Clone, Copy)]
struct Chunked<'ctx> {
    acc: PointerValue<'ctx>,
    to: PointerValue<'ctx>,
    n: PointerValue<'ctx>,
    k: u64,
    op: u32,
}

/// Items an `into` sink holds before pouring them into its target: a lazy
/// comprehension's chunk.
const CHUNK: u64 = 32;

/// What consumes a comprehension.
enum Consumer<'a> {
    /// `vec` / `into []`: the vector itself.
    Vector,
    /// `(into to E)`, `E` poured into `to` in chunks (Slice 6h).
    Into(&'a Expr),
    /// A consumer realizing the whole sequence in order, called over its
    /// vector: `count`, `sort`, `set`, `apply`.
    Call(Option<&'a Expr>),
    /// `(reduce + [init] E)`.
    Sum(Option<&'a Expr>),
}

/// `call` as a comprehension's consumer, with the comprehension.
fn consumer_of(call: &DirectCall) -> Option<(Consumer<'_>, &Expr)> {
    let plus = |e: &Expr| is_core_global(e, "+");
    Some(match (call.var.sym.name(), call.args.as_slice()) {
        ("vec", [e]) => (Consumer::Vector, e),
        ("into", [Expr::VecLit(items, None), e]) if items.is_empty() => (Consumer::Vector, e),
        ("into", [to, e]) => (Consumer::Into(to), e),
        ("count" | "sort" | "set", [e]) => (Consumer::Call(None), e),
        ("apply", [g, e]) if applies_whole(g) => (Consumer::Call(Some(g)), e),
        ("reduce", [p, init, e]) if plus(p) => (Consumer::Sum(Some(init)), e),
        ("reduce", [p, e]) if plus(p) => (Consumer::Sum(None), e),
        _ => return None,
    })
}

/// `clojure.core` fns whose variadic arity consumes every arg in order, so
/// `(apply g E)` realizes the whole sequence `E` (#271 Slice 6d) — not
/// `concat`, which walks its rest args lazily.
fn applies_whole(g: &Expr) -> bool {
    ["str", "max", "min", "+", "*", "merge", "vector", "hash-set"]
        .iter()
        .any(|name| is_core_global(g, name))
}

/// `e` as a comprehension stage whose whole shape lowers here.
fn stage<'a>(e: &'a Expr, named: &Named<'_>) -> Option<Stage<'a>> {
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
    if let Some((fun, arity)) = inline_fn(fe, 1) {
        let body = match kind {
            Kind::Map => Step::Item(&arity.body),
            _ => step(&arity.body, kind, named)?,
        };
        return Some(Stage {
            kind,
            call,
            fun: StageFn::Literal(fun),
            coll,
            body,
        });
    }
    (kind == Kind::Map && named(fe)).then_some(Stage {
        kind,
        call,
        fun: StageFn::Named(fe),
        coll,
        body: Step::NamedItem(fe, call.span),
    })
}

/// A `Keep` or `Cat` stage `fn`'s body as the expansion builds it: `let`s
/// and `if`s whose else is the stage's skip (the `__for-keep*` sentinel,
/// or `nil` for an outer stage), ending in the item (which must not name
/// the sentinel) or the next stage.
fn step<'a>(body: &'a [Expr], kind: Kind, named: &Named<'_>) -> Option<Step<'a>> {
    match body {
        [Expr::Let { bindings, body }] => Some(Step::Let {
            bindings,
            body,
            next: Box::new(step(body, kind, named)?),
        }),
        [Expr::If { test, then, els }] if skips(els, kind) => Some(Step::When {
            test,
            next: Box::new(step(std::slice::from_ref(then), kind, named)?),
        }),
        [e] if kind == Kind::Keep => (!names_skip(e)).then_some(Step::Item(body)),
        [e] => stage(e, named).map(|s| Step::Stage(Box::new(s))),
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
    /// loop already, see `hof_loop`); or (#271 Slice 6b) a consumer that
    /// always realizes the whole sequence in order — `(into to E)` for any
    /// `to`, `(count E)`, `(sort E)`, `(set E)` over the vector of `E`'s
    /// items, and `(reduce + [init] E)` folding them as they come (`+`
    /// never stops a `reduce`) — its other args evaluated first, as the
    /// call would.
    pub(super) fn try_lower_comprehension(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &DirectCall,
        dest: PointerValue<'ctx>,
    ) -> Result<bool, Unsupported> {
        let Some((consumer, e)) = consumer_of(call) else {
            return Ok(false);
        };
        // A single unfiltered binding under `vec`/`into []` or `reduce +` is
        // a traversal loop already (see `hof_loop`); under the other
        // consumers it runs here too (#271 Slice 6d).
        let single_ok = matches!(consumer, Consumer::Call(_) | Consumer::Into(_));
        let named = |fe: &Expr| self.named_callable(fe, 1);
        let Some(st) = stage(e, &named).filter(|s| s.kind != Kind::Map || single_ok) else {
            return Ok(false);
        };
        let (prior, sink) = match consumer {
            Consumer::Sum(init) => self.sum_sink(f, init)?,
            Consumer::Into(to) => {
                let s = self.slot(f);
                self.lower(f, to, s)?;
                (None, self.chunk_sink(f, call, s))
            }
            Consumer::Call(Some(to)) => {
                let s = self.slot(f);
                self.lower(f, to, s)?;
                (Some(s), self.push_sink(f))
            }
            _ => (None, self.push_sink(f)),
        };
        // The analysis saw each stage `fn` as a closure made once, so its
        // last-use moves may take a local an inline body reads again on the
        // next item: no moves here, and no tail position inside the loops.
        let moves = std::mem::take(&mut f.moves);
        let tail = std::mem::replace(&mut f.in_tail, false);
        let lowered = self.emit_stage(f, &st, sink);
        f.moves = moves;
        f.in_tail = tail;
        lowered?;
        self.finish_comprehension(f, call, &consumer, prior, sink, dest);
        Ok(true)
    }

    /// The comprehension's value into `dest` from its sink: the vector,
    /// the consumer's call over it, or the `+` fold.
    fn finish_comprehension(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &DirectCall,
        consumer: &Consumer<'_>,
        prior: Option<PointerValue<'ctx>>,
        sink: Sink<'ctx>,
        dest: PointerValue<'ctx>,
    ) {
        match (consumer, sink) {
            (Consumer::Vector, Sink::Push(acc)) => {
                self.call("rt_n_vec_done", None, &[dest.into(), acc.into()]);
                self.free_slots(f, &[acc]);
            }
            (_, Sink::Push(acc)) => {
                let items = self.slot(f);
                self.call("rt_n_vec_done", None, &[items.into(), acc.into()]);
                let args: Vec<PointerValue<'ctx>> = prior.into_iter().chain([items]).collect();
                self.core_call_owned(f, call, &args, dest);
                self.free_slots(f, &args);
                self.free_slots(f, &[acc]);
            }
            (_, Sink::Sum { acc, .. }) => {
                self.move_v(f.func, dest, acc);
                self.free_slots(f, &[acc]);
            }
            (_, Sink::Chunk(c)) => {
                self.flush_chunk(f, c);
                self.move_v(f.func, dest, c.to);
                self.free_slots(f, &[c.acc, c.to]);
            }
        }
    }

    /// An `into` sink over target slot `to` (its value already in it).
    fn chunk_sink(
        &mut self,
        f: &mut FnCtx<'ctx>,
        call: &DirectCall,
        to: PointerValue<'ctx>,
    ) -> Sink<'ctx> {
        let acc = self.slot(f);
        self.new_builder(acc);
        let n = self.entry_alloca(f, self.i64t, "into_n");
        self.builder
            .build_store(n, self.i64t.const_zero())
            .expect("store");
        let (k, op) = self.own_target(call, 2);
        Sink::Chunk(Chunked { acc, to, n, k, op })
    }

    /// `to = (into to pending)`, then an empty builder.
    fn flush_chunk(&mut self, f: &mut FnCtx<'ctx>, c: Chunked<'ctx>) {
        let items = self.slot(f);
        self.call("rt_n_vec_done", None, &[items.into(), c.acc.into()]);
        self.new_builder(c.acc);
        self.core_own_args(f, c.k, c.op, &[c.to, items], c.to);
        self.free_slots(f, &[items]);
        self.builder
            .build_store(c.n, self.i64t.const_zero())
            .expect("store");
    }

    /// An empty vector builder into slot `acc`.
    fn new_builder(&self, acc: PointerValue<'ctx>) {
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

    /// A fresh vector-builder sink.
    fn push_sink(&mut self, f: &mut FnCtx<'ctx>) -> Sink<'ctx> {
        let acc = self.slot(f);
        self.new_builder(acc);
        Sink::Push(acc)
    }

    /// A `+` fold's sink: the init (lowered first), else `0` and a pending
    /// first item.
    fn sum_sink(
        &mut self,
        f: &mut FnCtx<'ctx>,
        init: Option<&Expr>,
    ) -> Result<(Option<PointerValue<'ctx>>, Sink<'ctx>), Unsupported> {
        let acc = self.slot(f);
        let first = match init {
            Some(e) => {
                self.lower(f, e, acc)?;
                None
            }
            None => {
                self.write_int(f.func, acc, self.i64t.const_zero());
                let flag = self.entry_alloca(f, self.i8t, "sum_first");
                self.builder
                    .build_store(flag, self.i8t.const_int(1, false))
                    .expect("store");
                Some(flag)
            }
        };
        Ok((None, Sink::Sum { acc, first }))
    }

    /// One item into `sink` (`res` taken).
    fn sink_item(&mut self, f: &mut FnCtx<'ctx>, sink: Sink<'ctx>, res: PointerValue<'ctx>) {
        match sink {
            Sink::Push(acc) => {
                self.call("rt_n_vec_push", None, &[acc.into(), res.into()]);
            }
            Sink::Chunk(c) => {
                let (acc, n) = (c.acc, c.n);
                self.call("rt_n_vec_push", None, &[acc.into(), res.into()]);
                let pending = self
                    .builder
                    .build_load(self.i64t, n, "into_n")
                    .expect("load")
                    .into_int_value();
                let next = self
                    .builder
                    .build_int_add(pending, self.i64t.const_int(1, false), "into_n1")
                    .expect("add");
                self.builder.build_store(n, next).expect("store");
                let full = self
                    .builder
                    .build_int_compare(
                        IntPredicate::EQ,
                        next,
                        self.i64t.const_int(CHUNK, false),
                        "full",
                    )
                    .expect("cmp");
                let pour = self.ctx.append_basic_block(f.func, "into_pour");
                let join = self.ctx.append_basic_block(f.func, "into_join");
                self.builder
                    .build_conditional_branch(full, pour, join)
                    .expect("br");
                self.builder.position_at_end(pour);
                self.flush_chunk(f, c);
                self.builder.build_unconditional_branch(join).expect("br");
                self.builder.position_at_end(join);
            }
            Sink::Sum { acc, first } => {
                let join = self.ctx.append_basic_block(f.func, "sum_join");
                if let Some(flag) = first {
                    let set = self.ctx.append_basic_block(f.func, "sum_set");
                    let add = self.ctx.append_basic_block(f.func, "sum_add");
                    let pending = self
                        .builder
                        .build_load(self.i8t, flag, "first")
                        .expect("load")
                        .into_int_value();
                    let is_first = self
                        .builder
                        .build_int_compare(IntPredicate::NE, pending, self.i8t.const_zero(), "f")
                        .expect("cmp");
                    self.builder
                        .build_conditional_branch(is_first, set, add)
                        .expect("br");
                    self.builder.position_at_end(set);
                    self.move_v(f.func, acc, res);
                    self.builder
                        .build_store(flag, self.i8t.const_zero())
                        .expect("store");
                    self.builder.build_unconditional_branch(join).expect("br");
                    self.builder.position_at_end(add);
                }
                let st = self
                    .call("rt_n_add_acc", Some(self.i32t), &[acc.into(), res.into()])
                    .expect("st");
                self.check(f, st);
                self.builder.build_unconditional_branch(join).expect("br");
                self.builder.position_at_end(join);
            }
        }
    }

    /// Every item of `seq` (taken) into `sink`.
    fn sink_all(&mut self, f: &mut FnCtx<'ctx>, sink: Sink<'ctx>, seq: PointerValue<'ctx>) {
        let status = match sink {
            Sink::Chunk(c) => {
                // The pending items first, then the stage's own sequence.
                self.flush_chunk(f, c);
                self.core_own_args(f, c.k, c.op, &[c.to, seq], c.to);
                return;
            }
            Sink::Push(acc) => self.call(
                "rt_n_vec_push_all",
                Some(self.i32t),
                &[acc.into(), seq.into()],
            ),
            Sink::Sum { acc, first } => {
                let flag = first.unwrap_or_else(|| self.ptr.const_null());
                self.call(
                    "rt_n_add_all",
                    Some(self.i32t),
                    &[acc.into(), flag.into(), seq.into()],
                )
            }
        }
        .expect("status");
        self.check(f, status);
    }

    /// One stage's items into `sink`: indexed over a vector or long range,
    /// else through the stage's own core call.
    fn emit_stage(
        &mut self,
        f: &mut FnCtx<'ctx>,
        st: &Stage<'_>,
        sink: Sink<'ctx>,
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
        let state = self.load_loop_state(coll, coll, kind, plan);
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
        let stepped = self.emit_step(f, &st.body, sink);
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
        match st.fun {
            StageFn::Literal(fun) => self.make_closure(f, fun, base, closure)?,
            StageFn::Named(fe) => self.lower(f, fe, closure)?,
        }
        let mut args = vec![closure, coll];
        for extra in &st.call.args[2..] {
            let s = self.slot(f);
            self.lower(f, extra, s)?;
            args.push(s);
        }
        let seq = self.slot(f);
        self.core_call_owned(f, st.call, &args, seq);
        self.sink_all(f, sink, seq);
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
        sink: Sink<'ctx>,
    ) -> Result<(), Unsupported> {
        match step {
            Step::Let {
                bindings,
                body,
                next,
            } => self.emit_let_step(f, bindings, body, next, sink),
            Step::When { test, next } => {
                let then_b = self.ctx.append_basic_block(f.func, "comp_when");
                let join = self.ctx.append_basic_block(f.func, "comp_skip");
                self.lower_cond(f, test, then_b, join)?;
                self.builder.position_at_end(then_b);
                self.emit_step(f, next, sink)?;
                self.builder.build_unconditional_branch(join).expect("br");
                self.builder.position_at_end(join);
                Ok(())
            }
            Step::Item(body) => {
                let res = self.slot(f);
                self.lower_body(f, body, res)?;
                self.sink_item(f, sink, res);
                self.free_slots(f, &[res]);
                Ok(())
            }
            Step::NamedItem(fe, span) => {
                let param = f.env.len() - 1;
                let fun = self.named_fn(fe, 1, param, *span).expect("named_callable");
                let res = self.slot(f);
                self.lower_body(f, &fun.arities[0].body, res)?;
                self.sink_item(f, sink, res);
                self.free_slots(f, &[res]);
                Ok(())
            }
            Step::Stage(st) => self.emit_stage(f, st, sink),
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
        sink: Sink<'ctx>,
    ) -> Result<(), Unsupported> {
        let depth = f.env.len();
        let let_depth = f.let_slots.len();
        let tail_min = std::mem::replace(&mut f.tail_min, depth);
        let mut bound = Vec::new();
        let lowered = self
            .lower_let_bindings(f, bindings, body, &mut bound)
            .and_then(|()| {
                f.let_slots.extend_from_slice(&bound);
                self.emit_step(f, next, sink)
            });
        f.tail_min = tail_min;
        f.let_slots.truncate(let_depth);
        f.env.truncate(depth);
        f.pod_locals.retain(|&i| i < depth);
        self.free_slots(f, &bound);
        lowered
    }
}
