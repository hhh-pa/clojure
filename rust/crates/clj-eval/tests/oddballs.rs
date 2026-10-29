// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S20 oddballs: guard-row absence pins, `*repl*`, and the
//! error-shape natives (see
//! `docs/zero-oos-s20-oddballs/probes/{guards,errors,errors2,errors3,errors4}.{clj,out}`).
//! Every test names the probe labels it replays. The session-true
//! leg of `*repl*` lives in `clj-repl/tests/session.rs`
//! (`s20_repl_*`) — a `Session` cannot be built here without a
//! dependency cycle.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| pr(&realized(eval, v)))
        .collect()
}

fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

/// Probes `g-clone`: `clone-thread-bindings` resolves nowhere, so
/// its matrix row was removed as inventory repair.
#[test]
fn guard_clone_thread_bindings_absent() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(resolve 'clone-thread-bindings)"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(resolve 'clojure.core/clone-thread-bindings)"),
        "nil"
    );
}

/// Probes `g-pprint-public`, `g-pprint-var`: `pprint` is neither in
/// `clojure.core` publics nor interned there.
#[test]
fn guard_pprint_absent() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(contains? (ns-publics 'clojure.core) 'pprint)"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(resolve 'pprint)"), "nil");
    assert_eq!(run_last(&mut eval, "(resolve 'clojure.core/pprint)"), "nil");
}

/// Probes `r-root`, `r-dynamic`, `r-bound`: false root, dynamic,
/// bound outside any session.
#[test]
fn repl_root_and_meta() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*repl*"), "false");
    assert_eq!(
        run_last(&mut eval, "(-> #'clojure.core/*repl* meta :dynamic)"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(bound? #'clojure.core/*repl*)"),
        "true"
    );
}

/// `binding` conveys `*repl*` like the other session dynamics; the
/// root stays false.
#[test]
fn repl_binding_conveys() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(binding [*repl* :x] *repl*)"), ":x");
    assert_eq!(run_last(&mut eval, "*repl*"), "false");
}

/// Probes `a-*`: the single ex-info shape — top keys, cause, one via
/// entry, emulated `:at`, root-first trace of one.
#[test]
fn throwable_map_single() {
    let mut eval = boot();
    let m = "(Throwable->map (ex-info \"boom\" {:a 1}))";
    assert_eq!(
        run_last(&mut eval, &format!("(sort (map str (keys {m})))")),
        "(\":cause\" \":data\" \":trace\" \":via\")"
    );
    assert_eq!(run_last(&mut eval, &format!("(:cause {m})")), "\"boom\"");
    assert_eq!(run_last(&mut eval, &format!("(count (:via {m}))")), "1");
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(sort (map str (keys (first (:via {m})))))")
        ),
        "(\":at\" \":data\" \":message\" \":type\")"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:type (first (:via {m})))")),
        "clojure.lang.ExceptionInfo"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:message (first (:via {m})))")),
        "\"boom\""
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:data (first (:via {m})))")),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, &format!("(:data {m})")), "{:a 1}");
    assert_eq!(
        run_last(&mut eval, &format!("(:at (first (:via {m})))")),
        "[clojure.lang.ExceptionInfo unknown nil -1]"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(vector? (:trace {m}))")),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(= (:at (first (:via {m}))) (first (:trace {m})))")
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(every? #(and (vector? %) (= 4 (count %))) (:trace {m}))")
        ),
        "true"
    );
}

/// Probes `b-*`: the chained shape — root cause, outer-first via,
/// root-first trace.
#[test]
fn throwable_map_chained() {
    let mut eval = boot();
    let m = "(Throwable->map (ex-info \"outer\" {:a 1} (ex-info \"inner\" {:b 2})))";
    assert_eq!(run_last(&mut eval, &format!("(:cause {m})")), "\"inner\"");
    assert_eq!(run_last(&mut eval, &format!("(count (:via {m}))")), "2");
    assert_eq!(
        run_last(&mut eval, &format!("(mapv :message (:via {m}))")),
        "[\"outer\" \"inner\"]"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(mapv :type (:via {m}))")),
        "[clojure.lang.ExceptionInfo clojure.lang.ExceptionInfo]"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(mapv :data (:via {m}))")),
        "[{:a 1} {:b 2}]"
    );
    assert_eq!(run_last(&mut eval, &format!("(:data {m})")), "{:b 2}");
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(= (:trace {m}) [(second (mapv :at (:via {m}))) (first (mapv :at (:via {m})))])"
            )
        ),
        "true"
    );
}

/// Probes `c-*`, `g-data-c`: nil message drops `:cause`/`:message`
/// keys; nil `ExceptionInfo` data normalizes to `{}`.
#[test]
fn throwable_map_nil_message_data() {
    let mut eval = boot();
    let m = "(Throwable->map (ex-info nil nil))";
    assert_eq!(
        run_last(&mut eval, &format!("(sort (map str (keys {m})))")),
        "(\":data\" \":trace\" \":via\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(sort (map str (keys (first (:via {m})))))")
        ),
        "(\":at\" \":data\" \":type\")"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:data (first (:via {m})))")),
        "{}"
    );
    assert_eq!(run_last(&mut eval, &format!("(:data {m})")), "{}");
}

/// The documented seam (spec): the S20 fns apply the alpha10 nil→`{}`
/// rule at the `:data` projection, while the port's own `ex-data`
/// still answers nil (converging `ex-info` is out of the S20 fence).
#[test]
fn throwable_map_nil_data_seam() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(ex-data (ex-info \"m\" nil))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(:data (Throwable->map (ex-info \"m\" nil)))"),
        "{}"
    );
}

/// Probes `d-*`, `l-keys-n`, `j-*`, `k-*`: plain links carry no
/// `:data`; top `:data` follows the root link only.
#[test]
fn throwable_map_plain_links() {
    let mut eval = boot();
    let arith = "(try (quot 1 0) (catch Throwable t t))";
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(sort (map str (keys (Throwable->map {arith}))))")
        ),
        "(\":cause\" \":trace\" \":via\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(sort (map str (keys (first (:via (Throwable->map {arith}))))))")
        ),
        "(\":at\" \":message\" \":type\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(:type (first (:via (Throwable->map {arith}))))")
        ),
        "java.lang.ArithmeticException"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:cause (Throwable->map {arith}))")),
        "\"/ by zero\""
    );
    // 3-chain over a reader failure (probe k-* shape): [ex-info o]
    // -> [ReaderException] -> [plain root]. A plain middle under an
    // ExceptionInfo root (probe j) is unconstructible on the port —
    // only `ex-info` attaches causes — so the reader wrap stands in
    // for the plain-middle pin.
    let reader = "(try (with-in-str \"(a\" (read)) (catch Throwable t t))";
    let k = format!("(Throwable->map (ex-info \"o\" {{:a 1}} {reader}))");
    assert_eq!(run_last(&mut eval, &format!("(count (:via {k}))")), "3");
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(mapv #(contains? % :data) (:via {k}))")
        ),
        "[true false false]"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(sort (map str (keys {k})))")),
        "(\":cause\" \":trace\" \":via\")"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(:type (second (:via {k})))")),
        "clojure.lang.LispReader$ReaderException"
    );
    // Span chain across the links: outer has no span (-1), the
    // reader wrap carries the bumped EOF line 2 (house pin
    // `stdin_retry_exhausted_throws_truncated_span`), the inner
    // root has no data (-1).
    assert_eq!(
        run_last(&mut eval, &format!("(mapv #(nth (:at %) 3) (:via {k}))")),
        "[-1 2 -1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(= (first (:trace {k})) (:at (nth (:via {k}) 2)))")
        ),
        "true"
    );
    // Plain root: no top :data key (probe k-top-keys).
    let p = format!("(Throwable->map (ex-info \"o\" {{:a 1}} {arith}))");
    assert_eq!(
        run_last(&mut eval, &format!("(sort (map str (keys {p})))")),
        "(\":cause\" \":trace\" \":via\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(mapv #(contains? % :data) (:via {p}))")
        ),
        "[true false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(= (first (:trace {p})) (:at (second (:via {p}))))")
        ),
        "true"
    );
}

/// The diagnostic span chain: a reader failure's frame carries the
/// ex-data line; a non-`Int` line falls back to `-1`.
#[test]
fn throwable_map_reader_span_line() {
    let mut eval = boot();
    // The bumped EOF line (house pin above), replayed through
    // the frame projection.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) :no-throw (catch Throwable e (:clojure.error/line (ex-data e))))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) :no-throw (catch Throwable e (nth (StackTraceElement->vec e) 3)))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(StackTraceElement->vec (ex-info \"m\" {:clojure.error/line \"x\"}))"
        ),
        "[clojure.lang.ExceptionInfo unknown nil -1]"
    );
}

/// Probe `e-ste` shape: the throwable's emulated top frame, shared
/// with `Throwable->map`'s `:at`.
#[test]
fn ste_vec_frame() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(StackTraceElement->vec (ex-info \"boom\" {:a 1}))"
        ),
        "[clojure.lang.ExceptionInfo unknown nil -1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (ex-info \"boom\" {:a 1})] (= (StackTraceElement->vec t) (:at (first (:via (Throwable->map t))))))"
        ),
        "true"
    );
}

/// Probes `f-map-*`, `g-map-nil`, `g-map-cce`: cast, nil, arities.
#[test]
fn throwable_map_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(Throwable->map 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Throwable (java.lang.Long and java.lang.Throwable are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(Throwable->map nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Throwable.getStackTrace()\" because the return value of \"clojure.core$identity.invokeStatic(Object)\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Throwable->map)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/Throwable->map"
    );
    assert_eq!(
        uncaught(&mut eval, "(Throwable->map (ex-info \"x\" {}) 1)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/Throwable->map"
    );
}

/// Probes `f-ste-*`, `g-ste-nil`, `g-ste-cce`: cast, nil, arities.
#[test]
fn ste_vec_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(StackTraceElement->vec \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.StackTraceElement (java.lang.String and java.lang.StackTraceElement are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(StackTraceElement->vec nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.StackTraceElement.getClassName()\" because \"o\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(StackTraceElement->vec)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/StackTraceElement->vec"
    );
}
