// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S8 (issue #51) print-system rows: every oracle probe row
//! transcribed (`m-*`/`d-*`/`p-*`/`c-*`/`s-*`/`t-*`/`s2-*`/`f-*`),
//! the driver truncation mirror, the booted table key sets, and the
//! converged `*print-dup*` atom rows.
//!
//! Port adaptations (spec-directed, never silent): the JVM's class
//! fixtures become meta-`:type` fixtures (the port dispatches
//! tokens, never classes); writer fixtures become `with-out-str` +
//! `*out*` (no `StringWriter` surface); JVM-only kinds (queue,
//! primitives, volatile, array seqs) are skipped with a comment;
//! `m-table-*` is pinned as `print_tables_keys`, `m-iface` as the
//! specified non-replication.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

/// Run for effect (fixture installs), discarding values.
fn run(eval: &mut Evaluator, src: &str) {
    eval.eval_source(src).unwrap();
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

/// Dispatch rows (`m-*`): user methods fire on `pr-str`,
/// recursively, with separators; `str` never consults them; meta
/// `:type` keywords dispatch with `:default` chaining; removal
/// restores the default. Probe-file order (the `String` override
/// stays last — it rewrites string printing in this evaluator).
#[test]
fn dispatch_rows() {
    let mut eval = boot();
    // `m-shape`: both multis are `MultiFn`-classed values.
    assert_eq!(
        run_last(
            &mut eval,
            "[(map? (methods print-method)) (map? (methods print-dup))]"
        ),
        "[true true]"
    );
    assert!(
        run_last(&mut eval, "(pr-str print-method)")
            .starts_with("\"#object[clojure.lang.MultiFn 0x")
    );
    // `m-has-default`: method has `:default`, dup does not.
    assert_eq!(
        run_last(
            &mut eval,
            "[(contains? (methods print-method) :default) (contains? (methods print-dup) :default)]"
        ),
        "[true false]"
    );
    // (`m-table-method`/`m-table-dup` are JVM-class tables — the
    // port equivalent is `print_tables_keys`.)
    // `m-getmethod`: table queries interoperate.
    assert_eq!(
        run_last(
            &mut eval,
            "[(fn? (get-method print-method \"java.lang.String\")) (nil? (get-method print-dup ::unknown))]"
        ),
        "[true true]"
    );
    // User fixtures (meta-`:type` — the port has no class values
    // to hang methods on; user methods write via `print-simple`).
    run(
        &mut eval,
        "(defmethod print-method ::dt [o w] (print-simple (str \"DT\" (first o)) w))",
    );
    // (`identical?` is compiler magic, not a var — `=` on writers
    // compares the same identity.)
    run(
        &mut eval,
        "(defmethod print-method ::dw [o w] (print-simple (if (= w *out*) \"SAME\" \"DIFF\") w))",
    );
    run(
        &mut eval,
        "(defmethod print-method ::ntype [o w] (print-simple \"NTYPE\" w))",
    );
    run(
        &mut eval,
        "(defmethod print-method ::xatom [o w] (print-simple \"X!\" w))",
    );
    // A `:type`-carrying host instance (records take `with-meta`;
    // port deftypes reject it with a CCE).
    run(&mut eval, "(defrecord DT [x])");
    // User `print-dup` hangs on the token (dup ignores `:type`).
    run(
        &mut eval,
        "(defmethod print-dup \"clojure.lang.PersistentVector\" [o w] (print-simple \"DUP!\" w))",
    );
    // `m-user`, `m-user-nested`: user methods fire, recursively.
    assert_eq!(
        run_last(&mut eval, "(pr-str (with-meta [1] {:type ::dt}))"),
        "\"DT1\""
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str [(with-meta [1] {:type ::dt}) 2])"),
        "\"[DT1 2]\""
    );
    // `m-user-spaces`, `m-user-newline`: separators/newline interleave.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pr (with-meta [1] {:type ::dt}) (with-meta [2] {:type ::dt})))"
        ),
        "\"DT1 DT2\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (prn (with-meta [1] {:type ::dt})))"
        ),
        "\"DT1\\n\""
    );
    // `m-writer-id`: nested values receive the live `*out*`.
    assert_eq!(
        run_last(&mut eval, "(pr-str (with-meta [1] {:type ::dw}))"),
        "\"SAME\""
    );
    // `print` binds `*print-readably*` false (observed through a
    // user method); `pr` honors the ambient binding.
    run(
        &mut eval,
        "(defmethod print-method ::rd [o w] (print-simple (str *print-readably*) w))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(pr-str (with-meta [1] {:type ::rd})) (print-str (with-meta [1] {:type ::rd})) (binding [*print-readably* false] (pr-str \"hi\"))]"
        ),
        "[\"true\" \"false\" \"hi\"]"
    );
    // `m-str-unaffected`: top-level `str` never consults
    // `print-method` (oracle-faithful: the oracle row is a
    // top-level deftype; transcribed with a `:type`-tagged
    // record — also a non-collection leaf taking the `toString`
    // arm).
    assert!(
        run_last(&mut eval, "(str (with-meta (->DT 1) {:type ::dt}))").starts_with("\"user.DT@")
    );
    // `m-type-kw`, `m-type-nonstr`: keyword `:type` dispatches,
    // anything else falls to the token.
    assert_eq!(
        run_last(&mut eval, "(pr-str (with-meta [1] {:type ::ntype}))"),
        "\"NTYPE\""
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str (with-meta [1] {:type \"nope\"}))"),
        "\"[1]\""
    );
    // `m-type-unknown-kw`: unknown keywords chain through
    // `:default` (strip, re-invoke) — never a throw.
    assert_eq!(
        run_last(&mut eval, "(pr-str (with-meta [1] {:type ::unknown}))"),
        "\"[1]\""
    );
    // `m-type-atom-ctor`: atoms are not `IObj`, so `:default`
    // takes the `print-simple` arm (`toString`).
    assert!(
        run_last(&mut eval, "(pr-str (atom 1 :meta {:type ::unknown}))")
            .starts_with("\"clojure.lang.Atom@")
    );
    // `m-dup-delegates`: dup delegation goes through the LIVE
    // `print-method` (user methods apply under dup).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta (into {} (map (fn [i] [i i]) (range 20))) {:type ::xatom})))"
        ),
        "\"X!\""
    );
    // `m-dup-nodelegates`: array maps take the ctor (with the
    // dup-aware meta prefix), never the delegation.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta {:a 1} {:type ::xatom})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:type :user/xatom}) #=(clojure.lang.PersistentArrayMap/create {:a 1})\""
    );
    // `m-remove`: removal restores the default.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (remove-method print-method ::dt) (pr-str (with-meta [1] {:type ::dt})))"
        ),
        "\"[1]\""
    );
    // `m-dup-user`: user `print-dup` is gated by `*print-dup*`.
    assert_eq!(
        run_last(
            &mut eval,
            "[(binding [*print-dup* true] (pr-str [1])) (binding [*print-dup* false] (pr-str [1]))]"
        ),
        "[\"DUP!\" \"[1]\"]"
    );
    // `m-dup-remove`: removing a dup token restores the throw (the
    // port dispatches tokens, so removal targets the token).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (remove-method print-dup \"clojure.lang.PersistentVector\") (binding [*print-dup* true] (pr-str [1])))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.lang.PersistentVector\""
    );
    // `m-type-strip-visible`: the `:default` strip empties the meta,
    // so no prefix prints even with `*print-meta*`.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta [1] {:type ::unknown})))"
        ),
        "\"[1]\""
    );
    // `m-type-nonstr-meta`: non-keyword `:type` prints in the meta.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta [1] {:type \"nope\"})))"
        ),
        "\"^{:type \\\"nope\\\"} [1]\""
    );
    // `m-iface`: interface-level user methods NEVER fire (exact
    // match, no hierarchy — the specified non-replication).
    run(
        &mut eval,
        "(defmethod print-method \"java.lang.Number\" [o w] (print-simple \"NUM\" w))",
    );
    assert_eq!(run_last(&mut eval, "(pr-str 5)"), "\"5\"");
    // `m-str-override` LAST: redefining a core default beats it;
    // `str` still unaffected. (`run_last` renders through the
    // engine, so observing stays readable.)
    run(
        &mut eval,
        "(defmethod print-method \"java.lang.String\" [s w] (print-simple \"STR!\" w))",
    );
    assert_eq!(
        run_last(&mut eval, "[(pr-str \"hi\") (str \"hi\")]"),
        "[\"STR!\" \"hi\"]"
    );
}

/// Dup shapes (`d-*`): `→method` kinds print the readable shape,
/// ctor kinds print `#=` shapes, undupable kinds throw the stock
/// `No method` error naming the §1b token (quoted).
#[test]
fn dup_rows() {
    let mut eval = boot();
    // Fixtures (stable class names).
    run(&mut eval, "(def dupshape-x 1)");
    run(&mut eval, "(defn dupshape-f [x] x)");
    run(&mut eval, "(defrecord RP [x y])");
    run(&mut eval, "(deftype DT [x])");
    // `d-nil`, `d-bool`, `d-long`, `d-double`, `d-nan`,
    // `d-bigint`, `d-bigdec`, `d-ratio`.
    for (form, want) in [
        ("nil", "\"nil\""),
        ("true", "\"true\""),
        ("5", "\"5\""),
        ("1.5", "\"1.5\""),
        ("##NaN", "\"##NaN\""),
        ("5N", "\"5N\""),
        ("5.0M", "\"5.0M\""),
        ("1/2", "\"1/2\""),
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(binding [*print-dup* true] (pr-str {form}))")
            ),
            want,
            "{form}"
        );
    }
    // `d-string`, `d-char`, `d-char-named`, `d-keyword`,
    // `d-symbol`, `d-regex`.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str \"hi\"))"),
        "\"\\\"hi\\\"\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str \\a))"),
        "\"\\\\a\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str \\newline))"),
        "\"\\\\newline\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str :u/id))"),
        "\":u/id\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str 'a/b))"),
        "\"a/b\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str #\"a+\"))"),
        "\"#\\\"a+\\\"\""
    );
    // (`d-tagged`: `tagged-literal` is out of scope and the reader
    // only builds `TaggedLiteral` under `:preserve` inside `#?()`,
    // so no bare tagged value is surface-reachable. The token and
    // the `→method` default are pinned by `print_tables_keys`;
    // scalar delegation is covered structurally by `d-regex`.)
    // (`d-int`, `d-short`, `d-byte`, `d-float`, `d-biginteger`:
    // non-Long/Double `Number` ctor forms — no port kinds.)
    // Seqs/collections: `d-list`, `d-vec`, `d-lazy`, `d-range`,
    // `d-repeat`, `d-cons`, `d-stringseq`, `d-keyseq`, `d-valseq`,
    // `d-chunked`.
    for (form, want) in [
        ("(list 1 2)", "\"(1 2)\""),
        ("[1 2]", "\"[1 2]\""),
        ("(map inc [1 2])", "\"(2 3)\""),
        ("(range 3)", "\"(0 1 2)\""),
        ("(take 2 (repeat 7))", "\"(7 7)\""),
        ("(cons 1 [2])", "\"(1 2)\""),
        ("(seq \"ab\")", "\"(\\\\a \\\\b)\""),
        ("(keys {:a 1})", "\"(:a)\""),
        ("(vals {:a 1})", "\"(1)\""),
        ("(seq [1 2])", "\"(1 2)\""),
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(binding [*print-dup* true] (pr-str {form}))")
            ),
            want,
            "{form}"
        );
    }
    // (`d-arrayseq`: no array seq kind. `d-queue`: the port has no
    // queue kind.)
    // `d-hashset`.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str #{1 2}))"),
        "\"#{1 2}\""
    );
    // Collection ctors: `d-arraymap`, `d-sortedmap`,
    // `d-sortedset`, `d-mapentry`, `d-subvec`.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str {:a 1}))"),
        "\"#=(clojure.lang.PersistentArrayMap/create {:a 1})\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (sorted-map :b 2 :a 1)))"
        ),
        "\"#=(clojure.lang.PersistentTreeMap/create {:a 1, :b 2})\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (sorted-set 2 1)))"
        ),
        "\"#=(clojure.lang.PersistentTreeSet/create [1 2])\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (first {:a 1})))"
        ),
        "\"#=(clojure.lang.MapEntry/create [:a 1])\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (subvec [1 2 3] 1)))"
        ),
        "\"#=(clojure.lang.APersistentVector$SubVector/create [2 3])\""
    );
    // `d-eduction`: no dup shape — the stock throw.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (eduction (map inc) [1 2])))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.core.Eduction\""
    );
    // `d-hashmap-shape`: readable braces (never `#=(`), all keys.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (let [m (into {} (map (fn [i] [(str \"k\" i) i]) (range 20))) s (pr-str m)] [(= (subs s 0 1) \"{\") (not= (subs s 0 3) \"#=(\") (some? (re-find #\"k0\" s)) (some? (re-find #\"k19\" s))]))"
        ),
        "[true true true true]"
    );
    // Vars, namespaces, fns: `d-var-user`, `d-var-core`, `d-ns`.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str #'dupshape-x))"
        ),
        "\"#=(var user/dupshape-x)\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str #'inc))"),
        "\"#=(var clojure.core/inc)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (find-ns 'user)))"
        ),
        "\"#=(find-ns user)\""
    );
    // `d-anonfn`, `d-namedfn` (numbers normalized — display ids).
    let anon = run_last(
        &mut eval,
        "(binding [*print-dup* true] (pr-str (fn [x] x)))",
    );
    assert!(
        anon.starts_with("\"#=(user$eval") && anon.contains("$fn__") && anon.ends_with(". )\""),
        "{anon}"
    );
    // (Port `defn` builds an anonymous `fn`, so the named shape
    // uses a named `fn` form directly.)
    let named = run_last(
        &mut eval,
        "(binding [*print-dup* true] (pr-str (fn dupshape-f [x] x)))",
    );
    assert!(
        named.starts_with("\"#=(user$eval")
            && named.contains("dupshape_f__")
            && named.ends_with(". )\""),
        "{named}"
    );
    // Records, classes: `d-record`, `d-record-str`,
    // `d-record-ext`, `d-record-ext2`, `d-class`, `d-class-iface`.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (->RP 1 2)))"
        ),
        "\"#user.RP[1, 2]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (->RP 1 \"a\")))"
        ),
        "\"#user.RP[1, \\\"a\\\"]\""
    );
    // `d-record-ext`, `d-record-ext2`: dup prints `(vals r)` —
    // fields, then extmap entries in insertion order.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (assoc (->RP 1 2) :z 9)))"
        ),
        "\"#user.RP[1, 2, 9]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (assoc (->RP 1 2) :z 8 :w 9)))"
        ),
        "\"#user.RP[1, 2, 8, 9]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str DT))"),
        "\"#=user.DT\""
    );
    run(&mut eval, "(definterface II (m [x]))");
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str II))"),
        "\"#=user.II\""
    );
    // (`d-class-prim`, `d-class-array`: no primitive/array classes.)
    // (`d-object`: no bare `Object` values — `d-deftype` pins the
    // same `java.lang.Object` token throw.)
    // Undupable kinds throw (`d-deftype`, `d-reify`, `d-atom`,
    // `d-delay`, `d-future`, `d-promise`, `d-agent`, `d-reduced`,
    // `d-transient`, `d-throwable`, `d-readercond`, `d-multifn`,
    // `d-writer`, `d-reader`).
    for (form, token) in [
        ("(DT. 1)", "java.lang.Object"),
        ("(reify Object (toString [this] \"R\"))", "java.lang.Object"),
        ("(atom 1)", "clojure.lang.Atom"),
        ("(delay 1)", "clojure.lang.Delay"),
        ("(future 1)", "clojure.core$future_call$reify"),
        ("(promise)", "clojure.core$promise$reify"),
        ("(agent 1)", "clojure.lang.Agent"),
        ("(reduced 1)", "clojure.lang.Reduced"),
        ("(transient [1])", "clojure.lang.ITransientCollection"),
        ("(ex-info \"x\" {})", "java.lang.Throwable"),
        (
            "(read-string {:read-cond :preserve} \"#?@(:x)\")",
            "clojure.lang.ReaderConditional",
        ),
        ("print-method", "clojure.lang.MultiFn"),
        ("*out*", "java.io.Writer"),
        ("*in*", "clojure.lang.LineNumberingPushbackReader"),
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(binding [*print-dup* true] (pr-str {form}))")
            ),
            format!(
                "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"{token}\""
            ),
            "{form}"
        );
    }
    // (`d-volatile`: no volatile kind.)
}

/// Dup policy (`p-*`): length/level ignored, meta forced
/// (dup-aware values), readable strings/chars beat `readably`
/// false, `:type` ignored.
#[test]
fn dup_policy_rows() {
    let mut eval = boot();
    // A `print-dup` method on a keyword: never consulted
    // (`p-type-ignored` — dup dispatches the token only).
    run(
        &mut eval,
        "(defmethod print-dup ::pdup-mine [o w] (print-simple \"SHOULD-NOT-APPEAR\" w))",
    );
    run(&mut eval, "(defrecord PP [x y])");
    run(&mut eval, "(deftype DT [x])");
    run(&mut eval, "(def pdup-x 1)");
    // `p-root`.
    assert_eq!(run_last(&mut eval, "(pr-str *print-dup*)"), "\"false\"");
    // `p-length`, `p-level`, `p-length-ctor`: length/level ignored
    // under dup (even `0`, even inside ctor args).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *print-length* 1] (pr-str [1 2 3]))"
        ),
        "\"[1 2 3]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *print-level* 1] (pr-str [[1] [2]]))"
        ),
        "\"[[1] [2]]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *print-length* 0] (pr-str {:a 1 :b 2}))"
        ),
        "\"#=(clojure.lang.PersistentArrayMap/create {:a 1, :b 2})\""
    );
    // `p-meta`, `p-record-meta`: meta always prints (dup-aware
    // values — maps take the ctor form).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta [1] {:a 1})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) [1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta (->PP 1 2) {:a 1})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) #user.PP[1, 2]\""
    );
    // `p-meta-tag`: a single `:tag` prints bare, dup-aware (a
    // class tag takes `#=`; a map tag takes the ctor form).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta [1] {:tag DT})))"
        ),
        "\"^#=user.DT [1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta [1] {:tag {:a 1}})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) [1]\""
    );
    // `p-var-meta`: dup `Var` prints no meta.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str #'pdup-x))"),
        "\"#=(var user/pdup-x)\""
    );
    // `p-nested`, `p-nested-map`, `p-nested-vec-in-set`: ctor and
    // collection args recurse dup-aware.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str [{:a 1}]))"),
        "\"[#=(clojure.lang.PersistentArrayMap/create {:a 1})]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str {:a [1]}))"),
        "\"#=(clojure.lang.PersistentArrayMap/create {:a [1]})\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (sorted-set [1])))"
        ),
        "\"#=(clojure.lang.PersistentTreeSet/create [[1]])\""
    );
    // `p-str-unreadable`, `p-char-unreadable`, `p-print-str`,
    // `p-print-str-meta`: dup beats `*print-readably*` false.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *print-readably* false] (pr-str \"hi\"))"
        ),
        "\"\\\"hi\\\"\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *print-readably* false] (pr-str \\a))"
        ),
        "\"\\\\a\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (print-str [1 2]))"),
        "\"[1 2]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (print-str (with-meta [1] {:a 1})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) [1]\""
    );
    // `p-type-ignored`: dup ignores meta `:type` (the `::pdup-mine`
    // method never fires; the delegation strips the meta, so the
    // output is bare).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta [1] {:type ::pdup-mine})))"
        ),
        "\"[1]\""
    );
    // Scalar dups print ONE meta prefix (the driver owns it; the
    // scalar body renders top-meta-stripped).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (pr-str (with-meta 'sym {:a 1})))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) sym\""
    );
    // Cells print `#object` through scalar delegation (dup-off).
    let cells = run_last(
        &mut eval,
        "(pr-str [(agent 1) (promise) (future 1) (delay 1)])",
    );
    assert!(
        cells.starts_with("\"[#object[clojure.lang.Agent 0x")
            && cells.contains("#object[clojure.core$promise$reify")
            && cells.contains("#object[clojure.core$future_call$reify")
            && cells.contains("#object[clojure.lang.Delay 0x"),
        "{cells}"
    );
    // Unbounded seqs print `(...)` (never forced — driver and
    // `print-simple` alike).
    assert_eq!(run_last(&mut eval, "(pr-str (range))"), "\"(...)\"");
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple (range) *out*))"),
        "\"(...)\""
    );
    // `print-simple` under dup throws the stock `No method` error
    // for undupable nested values (static dup miss — the JVM's
    // `RT.printString` honors dup the same way).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*print-dup* true] (with-out-str (print-simple [(atom 1)] *out*)))"
        ),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.lang.Atom\""
    );
    // `p-false-map`, `p-false-meta`: dup-off controls.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* false] (pr-str {:a 1}))"),
        "\"{:a 1}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* false] (pr-str (with-meta [1] {:a 1})))"
        ),
        "\"[1]\""
    );
}

/// Char maps (`c-*`): full contents, counts, types, lookups,
/// non-dynamic roots, behavioral anchors, and the documented
/// redefinition-invisibility.
#[test]
fn charmaps_rows() {
    let mut eval = boot();
    // `c-escape`, `c-name`: full sorted contents (entry for entry
    // with the oracle literals).
    assert_eq!(
        run_last(&mut eval, "(into (sorted-map) char-escape-string)"),
        "{\\backspace \"\\\\b\", \\tab \"\\\\t\", \\newline \"\\\\n\", \\formfeed \"\\\\f\", \\return \"\\\\r\", \\\" \"\\\\\\\"\", \\\\ \"\\\\\\\\\"}"
    );
    assert_eq!(
        run_last(&mut eval, "(into (sorted-map) char-name-string)"),
        "{\\backspace \"backspace\", \\tab \"tab\", \\newline \"newline\", \\formfeed \"formfeed\", \\return \"return\", \\space \"space\"}"
    );
    // `c-counts`, `c-types`.
    assert_eq!(
        run_last(
            &mut eval,
            "[(count char-escape-string) (count char-name-string)]"
        ),
        "[7 6]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(every? char? (keys char-escape-string)) (every? string? (vals char-escape-string)) (every? char? (keys char-name-string)) (every? string? (vals char-name-string))]"
        ),
        "[true true true true]"
    );
    // `c-escape-lookup`, `c-name-lookup`.
    assert_eq!(
        run_last(
            &mut eval,
            "[(char-escape-string \\newline) (char-escape-string \\a)]"
        ),
        "[\"\\\\n\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(char-name-string \\space) (char-name-string \\a)]"
        ),
        "[\"space\" nil]"
    );
    // `c-dynamic`, `c-bind-err`: plain (non-dynamic) roots.
    assert_eq!(
        run_last(
            &mut eval,
            "[(:dynamic (meta #'char-escape-string)) (:dynamic (meta #'char-name-string))]"
        ),
        "[nil nil]"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [char-escape-string {}] 1)"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: clojure.core/char-escape-string"
    );
    // `c-uses-escape`, `c-uses-name`: behavioral anchors tying the
    // maps to the printer.
    assert_eq!(
        run_last(&mut eval, "(pr-str \"a\\nb\\t\\\"\\\\\")"),
        "\"\\\"a\\\\nb\\\\t\\\\\\\"\\\\\\\\\\\"\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str [\\newline \\tab \\space \\backspace \\formfeed \\return])"
        ),
        "\"[\\\\newline \\\\tab \\\\space \\\\backspace \\\\formfeed \\\\return]\""
    );
    // `c-redef-escape`, `c-redef-name`: the port asserts table
    // equality instead — redefinition does NOT reach the printer.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [char-escape-string {}] (pr-str \"a\\nb\"))"
        ),
        "\"\\\"a\\\\nb\\\"\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-redefs [char-name-string {}] (pr-str \\newline))"
        ),
        "\"\\\\newline\""
    );
}

/// Writer fns (`s-*`/`t-*`) and the `str`-half semantics (`s2-*`):
/// `print-simple` writes the prefix plus `(str o)`, `print-ctor`
/// writes the `#=` shape around `print-args`, and the `s2-*` rows
/// pin the ambient-binding-aware object half. Writers come from
/// `with-out-str` (no `StringWriter` surface).
#[test]
fn simple_ctor_rows() {
    let mut eval = boot();
    // `s-arglists`, `s-fn`.
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta #'print-simple))"),
        "([o w])"
    );
    assert_eq!(
        run_last(&mut eval, "[(fn? print-simple) (var? #'print-simple)]"),
        "[true true]"
    );
    // `s-str`, `s-kw`, `s-vec`, `s-onto-out`: `(str o)` to `w`.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple \"hi\" *out*))"),
        "\"hi\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple :a *out*))"),
        "\":a\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple [1 2] *out*))"),
        "\"[1 2]\""
    );
    // `s-meta` (the double prefix), `s-meta-off`, `s-meta-dup`,
    // `s-meta-unreadable`.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (with-meta [1] {:a 1}) *out*)))"
        ),
        "\"^{:a 1} ^{:a 1} [1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (with-meta [1] {:a 1}) *out*))"
        ),
        "\"[1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (with-out-str (print-simple (with-meta [1] {:a 1}) *out*)))"
        ),
        "\"^#=(clojure.lang.PersistentArrayMap/create {:a 1}) ^#=(clojure.lang.PersistentArrayMap/create {:a 1}) [1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true *print-readably* false] (with-out-str (print-simple (with-meta [1] {:a 1}) *out*)))"
        ),
        "\"[1]\""
    );
    // `s-ret`: nil (captured through an atom — the writer must come
    // from `with-out-str`, which answers the string).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom nil)] (with-out-str (reset! a (print-simple 1 *out*))) @a)"
        ),
        "nil"
    );
    // `s-arity0`, `s-arity1`, `s-arity3`.
    assert_eq!(
        uncaught(&mut eval, "(print-simple)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/print-simple"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-out-str (print-simple 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/print-simple"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-out-str (print-simple 1 *out* :x))"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/print-simple"
    );
    // `s-cce`, `s-nilw`: the `write_out` validations, verbatim.
    assert_eq!(
        uncaught(&mut eval, "(print-simple 1 \"nope\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.Writer (java.lang.String and java.io.Writer are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(print-simple 1 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null"
    );
    // `t-arglists`, `t-fn`.
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta #'print-ctor))"),
        "([o print-args w])"
    );
    assert_eq!(
        run_last(&mut eval, "[(fn? print-ctor) (var? #'print-ctor)]"),
        "[true true]"
    );
    // `t-basic`, `t-passes-o`, `t-writer-is-w` (`=` for writer
    // identity — `identical?` is not a var).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-ctor 5 (fn [o w] (print-simple \"ARGS\" w)) *out*))"
        ),
        "\"#=(java.lang.Long. ARGS)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-ctor 5 (fn [o w] (print-simple (str o) w)) *out*))"
        ),
        "\"#=(java.lang.Long. 5)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-ctor 5 (fn [o w] (print-simple (if (= w *out*) \"SAME\" \"DIFF\") w)) *out*))"
        ),
        "\"#=(java.lang.Long. SAME)\""
    );
    // `t-ret` (atom-captured, like `s-ret`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom nil)] (with-out-str (reset! a (print-ctor 5 (fn [o w]) *out*))) @a)"
        ),
        "nil"
    );
    // `t-arity`, `t-cce`, `t-nilw`.
    assert_eq!(
        uncaught(&mut eval, "(print-ctor 5)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/print-ctor"
    );
    assert_eq!(
        uncaught(&mut eval, "(print-ctor 5 (fn [o w]) \"nope\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.Writer (java.lang.String and java.io.Writer are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(print-ctor 5 (fn [o w]) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null"
    );
    // `t-nilo`: nil `o` throws the port's nil-receiver NPE (the JVM
    // text names `core$class` internals — unreproducible here).
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-out-str (print-ctor nil (fn [o w]) *out*))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.getName()\" because \"o\" is null"
    );
    // `s-pm-nondynamic`, `s-pd-nondynamic`: the multis are plain
    // (non-dynamic) roots.
    assert_eq!(
        uncaught(&mut eval, "(binding [print-method 5] 1)"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: clojure.core/print-method"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [print-dup 5] 1)"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: clojure.core/print-dup"
    );
    // `s2-meta`: the `str` half honors `*print-meta*` (single
    // prefix — the double arrives via the explicit prefix, `s-meta`).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (with-meta [1] {:a 1}) *out*)))"
        ),
        "\"^{:a 1} ^{:a 1} [1]\""
    );
    // `s2-dup-map`, `s2-dup-vec`: the `str` half honors
    // `*print-dup*` (no meta — the str half shows through alone).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (with-out-str (print-simple {:a 1} *out*)))"
        ),
        "\"#=(clojure.lang.PersistentArrayMap/create {:a 1})\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (with-out-str (print-simple [1 2] *out*)))"
        ),
        "\"[1 2]\""
    );
    // `s2-length`, `s2-level`: the `str` half honors
    // length/level.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-length* 1] (with-out-str (print-simple [1 2] *out*)))"
        ),
        "\"[1 ...]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-level* 1] (with-out-str (print-simple [[1] [2]] *out*)))"
        ),
        "\"[# #]\""
    );
    // `s2-str-raw`, `s2-dup-str`: strings stay raw.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple \"hi\" *out*)))"
        ),
        "\"hi\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true] (with-out-str (print-simple \"hi\" *out*)))"
        ),
        "\"hi\""
    );
    // `s2-atom`: references take the `toString` arm (plus the
    // explicit prefix — the meta is non-empty and bound).
    assert!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (atom 1 :meta {:a 1}) *out*)))"
        )
        .starts_with("\"^{:a 1} clojure.lang.Atom@")
    );
    // `s2-readably`: the `str` half honors ambient readably (not
    // forced).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-readably* false] (with-out-str (print-simple [1 \"a\"] *out*)))"
        ),
        "\"[1 a]\""
    );
    // `s2-cons`, `s2-range`, `s2-lazyseq`: strict seqs print
    // contents, lazy seqs print the content hash.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (cons 1 [2]) *out*))"
        ),
        "\"(1 2)\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple (range 3) *out*))"),
        "\"(0 1 2)\""
    );
    assert!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (map inc [1 2]) *out*))"
        )
        .starts_with("\"clojure.lang.LazySeq@")
    );
    // `s2-seqlength`, `s2-mapentry-str` (ahead of the user-method
    // installs: under the live collection arm a `String` method
    // would fire nested in the entry).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-length* 1] (with-out-str (print-simple (cons 1 [2 3]) *out*)))"
        ),
        "\"(1 ...)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (first {\"a\" 1}) *out*))"
        ),
        "\"[\\\"a\\\" 1]\""
    );
    // `s2-simple-nested-usermethod`: collections route through the
    // LIVE multi, so nested user methods fire. (Port deftypes
    // can't carry `:type` — `with-meta` is a CCE — so the
    // oracle's class-hung fixture is transcribed with a
    // `:type`-tagged record, also a non-collection leaf.)
    run(&mut eval, "(defrecord DT [x])");
    run(
        &mut eval,
        "(defmethod print-method ::dt [o w] (print-simple \"DT!\" w))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple [(with-meta (->DT 1) {:type ::dt})] *out*))"
        ),
        "\"[DT!]\""
    );
    // `s2-str-nested-usermethod` (DIVERGENT, frozen per §2d F6):
    // JVM `str` consults nested methods (`"[DT!]"`); native
    // `str` is behavior-frozen and renders the default shape.
    assert_eq!(
        run_last(&mut eval, "(str [(with-meta (->DT 1) {:type ::dt})])"),
        "\"[#user.DT{:x 1}]\""
    );
    // `s2-simple-usermethod`: top-level NON-collections still
    // bypass (the oracle row is a top-level deftype; same record
    // transcription — the `toString` arm, never the multi).
    assert!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (with-meta (->DT 1) {:type ::dt}) *out*))"
        )
        .starts_with("\"user.DT@")
    );
    // `s2-simple-type-vec`: top-level `:type` on a collection
    // dispatches live too (same arm — the JVM consults the multi
    // for the top-level collection, not just nested values).
    run(
        &mut eval,
        "(defmethod print-method ::st [o w] (print-simple \"ST!\" w))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (with-meta [1] {:type ::st}) *out*))"
        ),
        "\"ST!\""
    );
    // `s2-str-type-vec` (DIVERGENT, frozen per §2d F6): JVM `str`
    // consults the top-level `:type` method (`"ST!"`); native
    // `str` is behavior-frozen and renders the default shape.
    assert_eq!(
        run_last(&mut eval, "(str (with-meta [1] {:type ::st}))"),
        "\"[1]\""
    );
    // `s2-simple-type-map` / `s2-str-type-map`: the same split on
    // maps — `print-simple` converges, frozen `str` diverges.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print-simple (with-meta {:a 1} {:type ::st}) *out*))"
        ),
        "\"ST!\""
    );
    assert_eq!(
        run_last(&mut eval, "(str (with-meta {:a 1} {:type ::st}))"),
        "\"{:a 1}\""
    );
    // `s2-simple-meta-tag-live`, `s2-simple-meta-map-live`: the
    // explicit prefix renders through the LIVE multi (like the
    // JVM's `print-meta`), so user methods fire in both prefixes
    // (the `^tag` and the `^{...}` shapes).
    run(&mut eval, "(defrecord DA [x])");
    run(
        &mut eval,
        "(defmethod print-method ::da [o w] (print-simple \"A!\" w))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (with-meta [1] {:tag (with-meta (->DA 1) {:type ::da})}) *out*)))"
        ),
        "\"^A! ^A! [1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (with-out-str (print-simple (with-meta [1] {:a (with-meta (->DA 1) {:type ::da})}) *out*)))"
        ),
        "\"^{:a A!} ^{:a A!} [1]\""
    );
    // The `String` override stays LAST (it rewrites string
    // printing in this evaluator — and under the live arm it
    // would fire nested in every later collection row).
    run(
        &mut eval,
        "(defmethod print-method \"java.lang.String\" [s w] (print-simple \"SHOULD-NOT-APPEAR\" w))",
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print-simple \"hi\" *out*))"),
        "\"hi\""
    );
}

/// Flush rows (`f-*`): the root, the slot-dynamic proof, the
/// effective separator, and every timing row through the
/// per-evaluator flush counter.
#[test]
fn flush_rows() {
    let mut eval = boot();
    // `f-root`: true.
    assert_eq!(
        run_last(&mut eval, "(pr-str *flush-on-newline*)"),
        "\"true\""
    );
    // `f-dynamic`: slot-dynamic (binding works).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*flush-on-newline* false] *flush-on-newline*)"
        ),
        "false"
    );
    // `f-sep`: the effective separator is `"\n"`.
    assert_eq!(run_last(&mut eval, "(with-out-str (newline))"), "\"\\n\"");
    // Timings: (contents, flushes) per row.
    for (form, want, flushes) in [
        ("(newline)", "\"\\n\"", 0), // `f-newline`
        (
            "(binding [*flush-on-newline* false] (newline))",
            "\"\\n\"",
            0,
        ), // `f-newline-false`
        ("(prn 1)", "\"1\\n\"", 1),  // `f-prn`
        (
            "(binding [*flush-on-newline* false] (prn 1))",
            "\"1\\n\"",
            0,
        ), // `f-prn-false`
        ("(println 1)", "\"1\\n\"", 1), // `f-println`
        (
            "(binding [*flush-on-newline* false] (println 1))",
            "\"1\\n\"",
            0,
        ), // `f-println-false`
        ("(pr 1)", "\"1\"", 0),      // `f-pr`
        ("(print 1)", "\"1\"", 0),   // `f-print`
        ("(prn)", "\"\\n\"", 1),     // `f-prn-empty`
        ("(prn 1 2)", "\"1 2\\n\"", 1), // `f-multi`
        ("(do (newline) (prn 1))", "\"\\n1\\n\"", 1), // `f-newline-then-prn`
    ] {
        let before = eval.flush_count();
        assert_eq!(
            run_last(&mut eval, &format!("(with-out-str {form})")),
            want,
            "{form}"
        );
        assert_eq!(eval.flush_count() - before, flushes, "{form}");
    }
}

/// Driver truncation mirror (§1h): length/level/meta/
/// `namespace-maps` through `pr-str` (the driver), matching the
/// engine-direct expectations exactly.
#[test]
fn driver_truncation_rows() {
    let mut eval = boot();
    // Length (mirrors `print_opts_rows` + the engine tests).
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 2] (pr-str [1 2 3]))"),
        "\"[1 2 ...]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 0] (pr-str [1]))"),
        "\"[...]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-length* 1] (pr-str {:a 1 :b 2}))"
        ),
        "\"{:a 1, ...}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-length* 0] (pr-str {:a 1 :b 2}))"
        ),
        "\"{...}\""
    );
    // Level (dec-first: non-positive prints `#`).
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 1] (pr-str [[1]]))"),
        "\"[#]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 0] (pr-str [[1]]))"),
        "\"#\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 2] (pr-str [[1]]))"),
        "\"[[1]]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 0] (pr-str {:a 1}))"),
        "\"#\""
    );
    // Meta (`^tag` short form, full maps, falsy tags, readably
    // gating).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta ['x] {:tag 'String})))"
        ),
        "\"^String [x]\""
    );
    assert!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta ['x] {:tag 'String :k true})))"
        )
        .starts_with("\"^{")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta ['x] {:tag nil})))"
        ),
        "\"^{:tag nil} [x]\""
    );
    assert_eq!(
        run_last(&mut eval, "(print-str (with-meta ['x] {:tag 'String}))"),
        "\"[x]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta 'x {:k true})))"
        ),
        "\"^{:k true} x\""
    );
    // Namespace maps (lifted, flat, mixed).
    let lifted = run_last(&mut eval, "(pr-str {:user/a 1 :user/b 2})");
    assert!(
        lifted.starts_with("\"#:user{") && lifted.contains(":a 1") && lifted.contains(":b 2"),
        "{lifted}"
    );
    let flat = run_last(
        &mut eval,
        "(binding [*print-namespace-maps* false] (pr-str {:user/a 1 :user/b 2}))",
    );
    assert!(flat.contains(":user/a"), "{flat}");
    assert!(!run_last(&mut eval, "(pr-str {:u/a 1 :b 2})").starts_with("\"#:"));
    // Scalar-nested meta prints (throwable data keeps ambient
    // policy through the scalar body).
    let err = run_last(
        &mut eval,
        "(binding [*print-meta* true] (pr-str (ex-info \"x\" (with-meta {:a 1} {:m 1}))))",
    );
    assert!(err.contains("^{:m 1} {:a 1}"), "{err}");
    // `print-str` readability through the driver.
    assert_eq!(run_last(&mut eval, "(print-str [1 \"a\"])"), "\"[1 a]\"");
    assert_eq!(
        run_last(&mut eval, "(print-str {:a 1} [2])"),
        "\"{:a 1} [2]\""
    );
}

/// Booted table key sets (§1c): exactly the §1b token sets (+
/// `:default` for `print-method` only).
#[test]
fn print_tables_keys() {
    let mut eval = boot();
    // 42 string tokens + `nil` + `:default`.
    let mut method_keys = vec![
        "",
        ":default",
        "clojure.core.Eduction",
        "clojure.core$future_call$reify",
        "clojure.core$promise$reify",
        "clojure.lang.APersistentVector$SubVector",
        "clojure.lang.Agent",
        "clojure.lang.Atom",
        "clojure.lang.BigInt",
        "clojure.lang.Delay",
        "clojure.lang.Fn",
        "clojure.lang.IRecord",
        "clojure.lang.ISeq",
        "clojure.lang.ITransientCollection",
        "clojure.lang.Keyword",
        "clojure.lang.LineNumberingPushbackReader",
        "clojure.lang.MapEntry",
        "clojure.lang.MultiFn",
        "clojure.lang.Namespace",
        "clojure.lang.PersistentArrayMap",
        "clojure.lang.PersistentHashMap",
        "clojure.lang.PersistentHashSet",
        "clojure.lang.PersistentList",
        "clojure.lang.PersistentTreeMap",
        "clojure.lang.PersistentTreeSet",
        "clojure.lang.PersistentVector",
        "clojure.lang.Ratio",
        "clojure.lang.ReaderConditional",
        "clojure.lang.Reduced",
        "clojure.lang.Symbol",
        "clojure.lang.TaggedLiteral",
        "clojure.lang.Var",
        "clojure.lang.Var$Unbound",
        "java.io.Writer",
        "java.lang.Boolean",
        "java.lang.Character",
        "java.lang.Class",
        "java.lang.Double",
        "java.lang.Long",
        "java.lang.Object",
        "java.lang.String",
        "java.lang.Throwable",
        "java.math.BigDecimal",
        "java.util.regex.Pattern",
    ];
    method_keys.sort();
    let mut want = String::from("[");
    for (i, k) in method_keys.iter().enumerate() {
        if i > 0 {
            want.push(' ');
        }
        want.push('"');
        want.push_str(k);
        want.push('"');
    }
    want.push(']');
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (sort (map str (keys (methods print-method)))))"
        ),
        want
    );
    assert_eq!(run_last(&mut eval, "(count (methods print-method))"), "44");
    // 27 string tokens + `nil`, no `:default`.
    let mut dup_keys = vec![
        "",
        "clojure.lang.BigInt",
        "clojure.lang.Fn",
        "clojure.lang.IRecord",
        "clojure.lang.ISeq",
        "clojure.lang.Keyword",
        "clojure.lang.MapEntry",
        "clojure.lang.Namespace",
        "clojure.lang.PersistentArrayMap",
        "clojure.lang.PersistentHashMap",
        "clojure.lang.PersistentHashSet",
        "clojure.lang.PersistentList",
        "clojure.lang.PersistentTreeMap",
        "clojure.lang.PersistentTreeSet",
        "clojure.lang.PersistentVector",
        "clojure.lang.Ratio",
        "clojure.lang.Symbol",
        "clojure.lang.TaggedLiteral",
        "clojure.lang.Var",
        "clojure.lang.APersistentVector$SubVector",
        "java.lang.Boolean",
        "java.lang.Character",
        "java.lang.Class",
        "java.lang.Double",
        "java.lang.Long",
        "java.lang.String",
        "java.math.BigDecimal",
        "java.util.regex.Pattern",
    ];
    dup_keys.sort();
    let mut want = String::from("[");
    for (i, k) in dup_keys.iter().enumerate() {
        if i > 0 {
            want.push(' ');
        }
        want.push('"');
        want.push_str(k);
        want.push('"');
    }
    want.push(']');
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (sort (map str (keys (methods print-dup)))))"
        ),
        want
    );
    assert_eq!(run_last(&mut eval, "(count (methods print-dup))"), "28");
}

/// The converged `*print-dup*` atom rows (§1i): `false` prints the
/// `#object` shape, `true` throws like the JVM (corpus
/// `20-print-dup.clj` `:dup-atom-true` converges to the golden).
#[test]
fn print_dup_atom_throws() {
    let mut eval = boot();
    assert!(
        run_last(&mut eval, "(binding [*print-dup* false] (pr-str (atom 1)))")
            .starts_with("\"#object[clojure.lang.Atom 0x")
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*print-dup* true] (pr-str (atom 1)))"),
        "java.lang.IllegalArgumentException | No method in multimethod 'print-dup' for dispatch value: \"clojure.lang.Atom\""
    );
}
