// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #226 startup work: lazily loaded `core.clj`/`clojure.string`, the
//! native `clojure.string` fast paths (and their interpreted upstream
//! fallbacks), the native `core.clj` macro expanders, shared refer
//! tables, and the compiled-regex cache. Expected values are the
//! JVM's (or, where the port already diverged before #226, the
//! port's prior output, noted per case).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

/// A booted evaluator with `clojure.string` aliased as `str` (inside
/// the `*warn-on-reflection*` frame `clojure.main` provides).
fn booted() -> Evaluator {
    let mut eval = boot();
    eval.eval_source(
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.string :as str]))",
    )
    .unwrap();
    eval
}

/// `pr-str` of the form's value, or `Class: message` when it throws.
fn show(eval: &mut Evaluator, form: &str) -> String {
    let src = format!(
        "(try (pr-str {form}) (catch Throwable t (str (.getName (class t)) \": \" (.getMessage t))))"
    );
    let vs = eval
        .eval_source(&src)
        .unwrap_or_else(|e| panic!("{form}: {e}"));
    let s = pr_to_string(vs.last().unwrap(), &PrintOpts::print());
    s.to_owned()
}

fn check(eval: &mut Evaluator, cases: &[(&str, &str)]) {
    for (form, want) in cases {
        assert_eq!(&show(eval, form), want, "{form}");
    }
}

#[test]
fn string_natives_answer_strings_and_regexes() {
    let mut eval = booted();
    check(
        &mut eval,
        &[
            (r#"(str/split "a,b,,c,," #",")"#, r#"["a" "b" "" "c"]"#),
            (
                r#"(str/split "10 010 00 007 999" #" ")"#,
                r#"["10" "010" "00" "007" "999"]"#,
            ),
            (r#"(str/split "a,b,,c,," #"," 2)"#, r#"["a" "b,,c,,"]"#),
            (
                r#"(str/split "a,b,,c,," #"," -1)"#,
                r#"["a" "b" "" "c" "" ""]"#,
            ),
            (r#"(str/split-lines "a\nb\r\nc\n\n")"#, r#"["a" "b" "c"]"#),
            (r#"(str/join ", " [1 nil :k "s" \c])"#, r#""1, , :k, s, c""#),
            (r#"(str/join \, [1 2])"#, r#""1,2""#),
            (r#"(str/join nil [1 2])"#, r#""12""#),
            (r#"(str/join ", " "abc")"#, r#""a, b, c""#),
            (r#"(str/join ", " nil)"#, r#""""#),
            (
                r#"(str/join "" (range 40))"#,
                r#""0123456789101112131415161718192021222324252627282930313233343536373839""#,
            ),
            (r#"(str/join ", " (map inc (range 3)))"#, r#""1, 2, 3""#),
            (r#"(str/trim "  a b \t\n")"#, r#""a b""#),
            (r#"(str/trim " x ")"#, r#""x""#),
            ("(str/blank? nil)", "true"),
            ("(str/blank? false)", "true"),
            (r#"(str/blank? "")"#, "true"),
            (r#"(str/blank? " \t\n\u000b\u001c")"#, "true"),
            (r#"(str/blank? " ")"#, "false"),
            (r#"(str/starts-with? "abc" "ab")"#, "true"),
            (r#"(str/starts-with? "abc" "ac")"#, "false"),
            (r#"(str/ends-with? "abc" "bc")"#, "true"),
            (r#"(str/ends-with? "abc" "ab")"#, "false"),
            (r#"(str/includes? "abc" "b")"#, "true"),
            (r#"(str/includes? "abc" "x")"#, "false"),
        ],
    );
}

#[test]
fn string_natives_hand_other_arguments_to_upstream() {
    // `split` checks its arguments like the compiled `string.clj`
    // (JVM messages); outside the other fast paths the interpreted
    // definition answers (the results the port gave before #226).
    let mut eval = booted();
    check(
        &mut eval,
        &[
            (
                r#"(str/split "a,b" #"," 9999999999)"#,
                "java.lang.ArithmeticException: integer overflow",
            ),
            (
                r#"(str/split nil #" ")"#,
                "java.lang.NullPointerException: Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null",
            ),
            (
                r#"(str/split "a b" " ")"#,
                "java.lang.ClassCastException: class java.lang.String cannot be cast to class java.util.regex.Pattern (java.lang.String and java.util.regex.Pattern are in module java.base of loader 'bootstrap')",
            ),
            (
                r#"(str/split "a b" nil 2)"#,
                "java.lang.NullPointerException: Cannot invoke \"java.util.regex.Pattern.split(java.lang.CharSequence, int)\" because \"re\" is null",
            ),
            (
                r#"(str/split :k #" ")"#,
                "java.lang.ClassCastException: class clojure.lang.Keyword cannot be cast to class java.lang.CharSequence (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.CharSequence is in module java.base of loader 'bootstrap')",
            ),
            (r#"(str/split "a b c" #" " 2.7)"#, r#"["a" "b c"]"#),
            (r#"(str/split (StringBuilder. "x y") #" ")"#, r#"["x" "y"]"#),
            (
                "(str/split-lines nil)",
                "java.lang.NullPointerException: Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null",
            ),
            ("(str/join [1 2 3])", r#""123""#),
            (r#"(str/join :sep [1 2])"#, r#""1:sep2""#),
            (
                r#"(str/join ", " 5)"#,
                "java.lang.IllegalArgumentException: Don't know how to create ISeq from: java.lang.Long",
            ),
            (
                "(str/trim nil)",
                "java.lang.NullPointerException: Cannot invoke \"Object.getClass()\" because \"target\" is null",
            ),
            (
                r#"(str/starts-with? nil "a")"#,
                "java.lang.NullPointerException: Cannot invoke \"Object.getClass()\" because \"target\" is null",
            ),
            (
                r#"(str/ends-with? nil "a")"#,
                "java.lang.NullPointerException: Cannot invoke \"Object.getClass()\" because \"target\" is null",
            ),
            (
                r#"(str/includes? nil "a")"#,
                "java.lang.NullPointerException: Cannot invoke \"Object.getClass()\" because \"target\" is null",
            ),
            (r#"(.startsWith "abc" "ab")"#, "true"),
            (r#"(.startsWith "abc" "b" 1)"#, "true"),
            (r#"(.startsWith "abc" "b" 5)"#, "false"),
            (r#"(.startsWith "abc" "a" -1)"#, "false"),
            (r#"(.startsWith "héllo" "é" 1)"#, "true"),
            (r#"(.startsWith "héllo" "x" 1)"#, "false"),
            (r#"(.startsWith "hé" "ééé" 1)"#, "false"),
            (
                "(str/blank? 5)",
                "java.lang.IllegalArgumentException: No matching field found: length for class java.lang.Long",
            ),
        ],
    );
    // Large splits answer through the lazy span vector with the
    // same observable surface as a plain vector of pieces.
    check(
        &mut eval,
        &[
            (
                "(def big-toks (str/split (str/join \" \" (range 40)) #\" \"))",
                "#'user/big-toks",
            ),
            ("(class big-toks)", "clojure.lang.PersistentVector"),
            ("(vector? big-toks)", "true"),
            ("(count big-toks)", "40"),
            ("(nth big-toks 39)", "\"39\""),
            ("(get big-toks 3)", "\"3\""),
            ("(big-toks 7)", "\"7\""),
            ("(= big-toks (mapv str (range 40)))", "true"),
            ("(= (hash big-toks) (hash (mapv str (range 40))))", "true"),
            ("(subvec big-toks 2 5)", "[\"2\" \"3\" \"4\"]"),
            (
                "(class (subvec big-toks 2 5))",
                "clojure.lang.APersistentVector$SubVector",
            ),
            ("(= (mapv parse-long big-toks) (vec (range 40)))", "true"),
            ("(meta (with-meta big-toks {:a 1}))", "{:a 1}"),
            ("(compare big-toks (mapv str (range 40)))", "0"),
            ("(peek (conj big-toks \"x\"))", "\"x\""),
            ("(peek big-toks)", "\"39\""),
            ("(find big-toks 3)", "[3 \"3\"]"),
            ("(find big-toks 99)", "nil"),
            ("(contains? big-toks 39)", "true"),
            ("(contains? big-toks 40)", "false"),
            ("(get big-toks 99)", "nil"),
            ("(apply get [big-toks 3])", "\"3\""),
            ("(apply count [big-toks])", "40"),
            ("(empty big-toks)", "[]"),
            ("(first big-toks)", "\"0\""),
            ("(last big-toks)", "\"39\""),
            ("(+ 1 (count (nth big-toks 5)))", "2"),
            ("(count (filterv seq big-toks))", "40"),
            ("(filterv (fn [s] (= s \"7\")) big-toks)", "[\"7\"]"),
            ("(pop (subvec big-toks 0 1))", "[]"),
            ("(conj {} (subvec big-toks 0 2))", "{\"0\" \"1\"}"),
            (
                "(try (conj {} big-toks) (catch Exception e (class e)))",
                "java.lang.IllegalArgumentException",
            ),
            ("(nth (assoc big-toks 0 \"z\") 0)", "\"z\""),
            ("(count (pop big-toks))", "39"),
            (
                "(try (subvec big-toks 3 2) (catch Exception e (class e)))",
                "java.lang.IndexOutOfBoundsException",
            ),
            (
                "(try (subvec big-toks -1 2) (catch Exception e (class e)))",
                "java.lang.IndexOutOfBoundsException",
            ),
            ("(subvec big-toks 40 40)", "[]"),
            (
                "(mapv parse-long (str/split (str/join \" \" (concat (range 20) [\"x\"] (range 20))) #\" \"))",
                "[0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 nil 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19]",
            ),
        ],
    );
    // Arity errors come from the upstream fn, named like the JVM's.
    for (form, want) in [
        (r#"(str/split "a")"#, "(1) passed to: clojure.string/split"),
        (
            r#"(str/split "a" #"," 0 1)"#,
            "(4) passed to: clojure.string/split",
        ),
        ("(str/join)", "(0) passed to: clojure.string/join"),
    ] {
        assert_eq!(
            show(&mut eval, form),
            format!("clojure.lang.ArityException: Wrong number of args {want}"),
            "{form}"
        );
    }
    // The fallback is cached: a second slow call reuses it.
    assert_eq!(show(&mut eval, "(str/join [4 5])"), r#""45""#);
}

#[test]
fn comprehension_modifiers_expand_like_core_clj() {
    let mut eval = booted();
    check(
        &mut eval,
        &[
            ("(for [] 1)", "(1)"),
            (
                "(for [x (range 10) :let [y (* x x)] :when (odd? x) :while (< y 50)] [x y])",
                "([1 1] [3 9] [5 25] [7 49])",
            ),
            (
                "(for [x (range 10) :while (< x 6) :let [y (inc x)] :when (even? y)] y)",
                "(2 4 6)",
            ),
            ("(for [x (range 6) :when (even? x)] x)", "(0 2 4)"),
            (
                "(for [x (range 3) :let [y (* 2 x) z (inc y)]] [y z])",
                "([0 1] [2 3] [4 5])",
            ),
            ("(for [x (range 3) :let nil] x)", "(0 1 2)"),
            (
                "(for [x (range 4) :when (odd? x) y (range x) :let [z (+ x y)] :when (even? z)] [x y z])",
                "([3 1 4])",
            ),
            (
                "(for [x (range 4) y (range 4) :while (< y x)] [x y])",
                "([1 0] [2 0] [2 1] [3 0] [3 1] [3 2])",
            ),
            (
                "(for [x (range 3) :let [a x] :when true y [a]] y)",
                "(0 1 2)",
            ),
            (
                "(with-out-str (doseq [x (range 6) :let [y (* x 10)] :when (odd? x) :while (< x 5)] (print y \\space)))",
                r#""10  30  ""#,
            ),
            (
                "(with-out-str (doseq [x (range 6) :when (odd? x)] (print x)))",
                r#""135""#,
            ),
            (
                "(with-out-str (doseq [x (range 3) y (range x)] (print [x y])))",
                r#""[1 0][2 0][2 1]""#,
            ),
            ("(with-out-str (doseq [] (print 1) (print 2)))", r#""12""#),
        ],
    );
}

#[test]
fn comprehension_errors_match_core_clj() {
    let mut eval = booted();
    for (form, want) in [
        ("(for [:when true] 1)", "Can't pop empty vector"),
        ("(for [x [1] :bogus 1] x)", "Invalid 'for' keyword :bogus"),
        ("(doseq [x [1] :bogus 1] x)", "\"steppair\" is null"),
        (
            "(for [x [1] :let [y 1] :bogus 2] x)",
            "Invalid 'for' keyword :bogus",
        ),
        (
            "(for [x [1]] 1 2)",
            "Wrong number of args (3) passed to: clojure.core/for",
        ),
        ("(for [x [1] :let (list y 1)] y)", "did not conform to spec"),
    ] {
        let err = eval.eval_source(form).expect_err(form).to_string();
        assert!(err.contains(want), "{form}: {err}");
    }
}

#[test]
fn defn_and_simple_macros_expand_like_core_clj() {
    let mut eval = booted();
    eval.eval_source(
        "(defn f1 \"doc\" {:a 1} ([x] x) {:b 2})
         (defn f2 [x] {:pre [(pos? x)] :post [(pos? %)]} x)
         (defn f3 ([] 0) ([x] {:pre [x]} x) ([x & more] (count more)))
         (defn- f4 [] 4)",
    )
    .unwrap();
    check(
        &mut eval,
        &[
            (
                "(select-keys (meta #'f1) [:a :b :doc :arglists])",
                r#"{:a 1, :b 2, :doc "doc", :arglists ([x])}"#,
            ),
            ("(:arglists (meta #'f2))", "([x])"),
            ("(f2 3)", "3"),
            (
                "[(f3) (f3 1) (f3 1 2 3) (:arglists (meta #'f3))]",
                "[0 1 2 ([] [x] [x & more])]",
            ),
            ("[(f4) (:private (meta #'f4))]", "[4 true]"),
            ("(if-not false 1 2)", "1"),
            ("(if-not true 1)", "nil"),
            ("[(and) (or) (cond)]", "[true nil nil]"),
            ("(-> 1 inc (* 2) (- 1))", "3"),
            ("(->> (range 5) (map inc) (reduce +))", "15"),
            (r#"(.. "abc" (toUpperCase) (substring 1))"#, r#""BC""#),
            ("(comment 1 2 3)", "nil"),
            ("(let [a (atom 0)] (while (< @a 5) (swap! a inc)) @a)", "5"),
            ("(when-not false 1 2)", "2"),
        ],
    );
    for (form, want) in [
        (
            "(if-not)",
            "Wrong number of args (0) passed to: clojure.core/if-not",
        ),
        (
            "(defn- 5 [] 1)",
            "cannot be cast to class clojure.lang.IObj",
        ),
    ] {
        let err = eval.eval_source(form).expect_err(form).to_string();
        assert!(err.contains(want), "{form}: {err}");
    }
}

#[test]
fn lazy_core_vars_force_on_first_use() {
    let mut eval = booted();
    // A var invoked through `#'` resolves at run time, not analysis.
    check(
        &mut eval,
        &[
            ("(#'clojure.core/println-str 1 2)", "\"1 2\\n\""),
            ("(:arglists (meta #'clojure.core/rand-nth))", "([coll])"),
            ("(bound? #'clojure.core/prn-str)", "true"),
        ],
    );
}

#[test]
fn reloading_clojure_string_past_the_lazy_segments_loads_eagerly() {
    // Each lazy library load claims an index segment; once they run
    // out, reloads evaluate the file eagerly — same vars either way.
    let mut eval = booted();
    for i in 0..10 {
        let got = show(
            &mut eval,
            "(do (binding [*warn-on-reflection* *warn-on-reflection*]
                   (require 'clojure.string :reload))
                 [(clojure.string/join \"-\" [1 2]) (clojure.string/upper-case \"x\")])",
        );
        assert_eq!(got, r#"["1-2" "X"]"#, "reload {i}");
    }
}

#[test]
fn refer_shares_the_publics_table_and_copies_on_change() {
    let mut eval = booted();
    check(
        &mut eval,
        &[
            (
                "(do (ns r1) (clojure.core/refer 'clojure.core :exclude '[a/b 5 first])
                     (clojure.core/in-ns 'user)
                     [(contains? (ns-refers 'r1) 'first) (contains? (ns-refers 'r1) 'map)])",
                "[true true]",
            ),
            (
                "(do (create-ns 'r2) (binding [*ns* (the-ns 'r2)] (clojure.core/refer 'clojure.core :exclude '[map]))
                     [(contains? (ns-refers 'r2) 'map) (contains? (ns-refers 'r2) 'filter)])",
                "[false true]",
            ),
            (
                "(do (create-ns 'r3) (intern 'r3 'filter 1)
                     (binding [*ns* (the-ns 'r3)] (clojure.core/refer 'clojure.core))
                     [(contains? (ns-refers 'r3) 'filter) (contains? (ns-refers 'r3) 'map)])",
                "[false true]",
            ),
        ],
    );
}

#[test]
fn regex_cache_stays_bounded() {
    // Past 256 distinct patterns the per-thread cache clears and keeps
    // compiling correctly.
    let mut eval = booted();
    let got = show(
        &mut eval,
        "(count (filter identity (for [i (range 300)] (re-matches (re-pattern (str \"a\" i)) (str \"a\" i)))))",
    );
    assert_eq!(got, "300");
}

#[test]
fn captured_string_natives_survive_redefinition_and_removal() {
    // A captured native's slow path still runs the original upstream
    // definition after the var is redefined (its pending metadata
    // source was dropped by the redefinition) or its namespace removed
    // (the tombstone keeps the var); an unmapped name has none left.
    let mut eval = booted();
    eval.eval_source(
        "(def sp clojure.string/split) (def jn clojure.string/join) (def bl clojure.string/blank?)
         (in-ns 'clojure.string)
         (clojure.core/defn split [s re] :redefined)
         (clojure.core/in-ns 'user)",
    )
    .unwrap();
    check(
        &mut eval,
        &[
            (r#"(clojure.string/split "a b" #" ")"#, ":redefined"),
            (r#"(sp "a b" #" ")"#, r#"["a" "b"]"#),
            (r#"(sp (StringBuilder. "a b") #" ")"#, r#"["a" "b"]"#),
            // A shape outside the native (an arity miss) runs the
            // file's original definition, not the redefinition.
            (
                r#"(sp "a")"#,
                "clojure.lang.ArityException: Wrong number of args (1) passed to: clojure.string/split",
            ),
        ],
    );
    eval.eval_source("(remove-ns 'clojure.string)").unwrap();
    check(&mut eval, &[(r#"(jn :sep [1 2])"#, r#""1:sep2""#)]);
    let mut eval = booted();
    eval.eval_source("(def bl clojure.string/blank?) (ns-unmap 'clojure.string 'blank?)")
        .unwrap();
    check(
        &mut eval,
        &[(
            "(bl 5)",
            "java.lang.IllegalStateException: No definition behind clojure.string/blank?",
        )],
    );
}

#[test]
fn macro_expanders_take_seq_forms_and_odd_signatures() {
    let mut eval = booted();
    eval.eval_source(
        "(defmacro thr [] `(-> 1 ~(cons 'inc ())))
         (defmacro thr2 [] `(->> 1 ~(cons 'list (cons 0 ()))))
         (defn ff [&form &env x] x)",
    )
    .unwrap();
    check(
        &mut eval,
        &[
            ("(thr)", "2"),
            ("(thr2)", "(0 1)"),
            ("[(:arglists (meta #'ff)) (ff 1 2 3)]", "[([x]) 3]"),
            (r#"(str/join "," '(1 2 3))"#, r#""1,2,3""#),
        ],
    );
    for (form, want) in [
        (
            "(if-not 1 2 3 4)",
            "Wrong number of args (4) passed to: clojure.core/if-not",
        ),
        (
            "(for [x [1] :when true :bogus 1] x)",
            "Invalid 'for' keyword :bogus",
        ),
        // The JVM's spec rejects this signature first; without spec,
        // `core.clj`'s `defn` fails taking the params' seq.
        (
            "(defn fg ([x] x) [y])",
            "Don't know how to create ISeq from: clojure.lang.Symbol",
        ),
    ] {
        let err = eval.eval_source(form).expect_err(form).to_string();
        assert!(err.contains(want), "{form}: {err}");
    }
}

#[test]
fn an_old_var_forced_after_a_reload_keeps_its_native() {
    // The old var's metadata source now defines into the recreated
    // namespace, so the old root stays the native — which must never
    // become its own slow path.
    let mut eval = booted();
    check(
        &mut eval,
        &[(
            "(do (def v #'clojure.string/split)
                 (remove-ns 'clojure.string)
                 (binding [*warn-on-reflection* *warn-on-reflection*]
                   (require 'clojure.string :reload))
                 [(:name (meta v)) (v \"a b\" #\" \") (clojure.string/split \"c d\" #\" \")])",
            r#"[split ["a" "b"] ["c" "d"]]"#,
        )],
    );
}

#[test]
fn natives_boot_no_longer_reaches_still_answer() {
    // Paths only eager `core.clj` evaluation used to exercise: the
    // empty-arg arities, short non-ASCII `nth`, and the port-internal
    // NPE helper behind `core.clj`'s (now metadata-only) `doseq`.
    let mut eval = booted();
    check(
        &mut eval,
        &[
            ("(*)", "1"),
            (
                "(=)",
                "clojure.lang.ArityException: Wrong number of args (0) passed to: clojure.core/=",
            ),
            (r#"(nth "héllo" 1)"#, r"\é"),
            (
                r#"(#'clojure.core/throw-npe! "steppair")"#,
                "java.lang.NullPointerException: steppair",
            ),
        ],
    );
}

#[test]
fn string_join_and_contains_cases() {
    let mut eval = booted();
    eval.eval_source("(require '[clojure.string :as str])")
        .unwrap();
    check(
        &mut eval,
        &[
            (r#"(str/join "," ["a" "b" "c"])"#, r#""a,b,c""#),
            (r#"(str/join \- [1 2 3])"#, r#""1-2-3""#),
            (r#"(str/join nil ["x" "y"])"#, r#""xy""#),
            (r#"(str/join "," [])"#, r#""""#),
            (r#"(str/join "," [42])"#, r#""42""#),
            (r#"(str/join "," (seq [10 20 30]))"#, r#""10,20,30""#),
            (r#"(contains? [1 2 3] 0)"#, "true"),
            (r#"(contains? [1 2 3] 2)"#, "true"),
            (r#"(contains? [1 2 3] 3)"#, "false"),
            (r#"(contains? [1 2 3] -1)"#, "false"),
            (r#"(contains? [1 2 3] :foo)"#, "false"),
            (r#"(contains? nil 0)"#, "false"),
            (r#"(apply contains? [nil 0])"#, "false"),
        ],
    );
}
