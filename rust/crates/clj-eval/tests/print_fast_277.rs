// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #277: the full boot prints scalars through the booted print defaults
//! while `print-method`/`print-dup` are still the booted multis, and
//! returns to the live multis once a `defmethod` installs a new one,
//! against JVM Clojure 1.13.0-alpha10 on JDK 26. The rows are the
//! `(t ...)` forms of `docs/spec/i277-recipes/probes/print_fast.clj` in
//! file order, answered by `print_fast.out`: the `q` rows run while the
//! multis are pristine (every scalar kind, `*print-readably*`,
//! `*print-meta*`, `*print-dup*`, a custom `*out*` writer, the `-str`
//! family), the `p5`-onward rows after its `defmethod`s. Left out:
//! `p10`, `str` of a collection under a user `print-method`: this runtime prints collection elements without consulting the multis (unchanged here); the JVM's
//! //! `toString` dispatches `print-method` on them.

use clj_eval::boot;
use clj_value::Value;

/// `(label, expression, JVM answer)`.
const ORACLE: &[(&str, &str, &str)] = &[
    (
        "p1 scalars",
        r##"(with-out-str (println "a" 1 :k \c 2.5 nil true))"##,
        r##"a 1 :k c 2.5 nil true\n"##,
    ),
    (
        "p2 print str",
        r##"(with-out-str (print "b\n"))"##,
        r##"b\n"##,
    ),
    (
        "p3 pr scalars",
        r##"(with-out-str (prn "s" 1 :k))"##,
        r##""s" 1 :k\n"##,
    ),
    (
        "p4 join",
        r##"(with-out-str (println (clojure.string/join " " [1 2 3])))"##,
        r##"1 2 3\n"##,
    ),
    (
        "q1 numbers",
        r##"(with-out-str (prn 1.0 1.5 1.0E7 12345678.0 0.001 1.0E-4 3.0 (float 1) ##NaN ##Inf (- ##Inf) -0.0 (double 7) 1e21 7.0))"##,
        r##"1.0 1.5 1.0E7 1.2345678E7 0.001 1.0E-4 3.0 1.0 ##NaN ##Inf ##-Inf -0.0 7.0 1.0E21 7.0\n"##,
    ),
    (
        "q2 big numbers",
        r##"(with-out-str (prn 12345678901234567890 1.5M 1/3 (- 1) (bigint 7) 2N 3.0M 22/7 (double 22/7)))"##,
        r##"12345678901234567890N 1.5M 1/3 -1 7N 2N 3.0M 22/7 3.142857142857143\n"##,
    ),
    (
        "q3 chars strings",
        r##"(with-out-str (prn \a \newline \tab \formfeed \backspace \return \u00e9 "s\nq\"z\\é" (str \a) (name :a/b) (namespace 'a/b)))"##,
        r##"\a \newline \tab \formfeed \backspace \return \é "s\nq\"z\\é" "a" "b" "a"\n"##,
    ),
    (
        "q4 not readable",
        r##"(with-out-str (binding [*print-readably* nil] (pr "a" :k 'sym \b 1.0 (keyword "a b")) (print "c" \d)))"##,
        r##"a :k sym b 1.0 :a bc d"##,
    ),
    (
        "q5 meta",
        r##"(with-out-str (binding [*print-meta* true] (pr (with-meta 'sym {:a 1}) (with-meta :k {:b 2}) (with-meta "s" {:c 3}))))"##,
        r##"java.lang.ClassCastException: class clojure.lang.Keyword cannot be cast to class clojure.lang.IObj (clojure.lang.Keyword and clojure.lang.IObj are in unnamed module of loader 'app')"##,
    ),
    (
        "q6 dup pristine",
        r##"(with-out-str (binding [*print-dup* true] (prn 1 "h" :k 1.0 [1 "s"])))"##,
        r##"1 "h" :k 1.0 [1 "s"]\n"##,
    ),
    (
        "q7 writer",
        r##"(let [w (java.io.StringWriter.)] (binding [*out* w] (println "x" 1) (pr "y") (newline) (.flush w)) (str w))"##,
        r##"x 1\n"y"\n"##,
    ),
    (
        "q8 pieces",
        r##"(with-out-str (print "a") (print 1) (newline) (pr) (print) (prn) (println))"##,
        r##"a1\n\n\n"##,
    ),
    (
        "q9 limits",
        r##"(with-out-str (binding [*print-length* 1 *print-level* 1] (prn 1 "s" :k)))"##,
        r##"1 "s" :k\n"##,
    ),
    (
        "q10 pr-str",
        r##"[(pr-str "tab\there" \c) (prn-str 1 "a") (print-str 1 "a") (println-str 1 "a")]"##,
        r##"["\"tab\\there\" \\c" "1 \"a\"\n" "1 a" "1 a\n"]"##,
    ),
    (
        "q11 names",
        r##"(with-out-str (prn ::k 'user/s (symbol "x") '/ '/ 'a-b 'a/b (symbol "a b") 'foo.bar/baz-qux :foo.bar/baz-qux))"##,
        r##":user/k user/s x / / a-b a/b a b foo.bar/baz-qux :foo.bar/baz-qux\n"##,
    ),
    (
        "q12 unicode",
        r##"(with-out-str (prn "é中ß€") (println "é中ß€"))"##,
        r##""é中ß€"\né中ß€\n"##,
    ),
    (
        "q13 mixed",
        r##"(with-out-str (println [1 "a"] {:a 1} #{1} '(1 2) nil) (prn [1 "a" \b]))"##,
        r##"[1 a] {:a 1} #{1} (1 2) nil\n[1 "a" \b]\n"##,
    ),
    (
        "q14 nils",
        r##"(with-out-str (println nil nil) (pr nil nil) (print nil))"##,
        r##"nil nil\nnil nilnil"##,
    ),
    (
        "q15 ns maps",
        r##"(with-out-str (binding [*print-namespace-maps* true] (prn {:a/b 1})))"##,
        r##"#:a{:b 1}\n"##,
    ),
    (
        "q16 int kinds",
        r##"(with-out-str (prn -5 0 (int 5) (short 5) (byte 5) 5 (Integer/MAX_VALUE) (Long/MAX_VALUE) 2147483648))"##,
        r##"-5 0 5 5 5 5 2147483647 9223372036854775807 2147483648\n"##,
    ),
    (
        "p5 defmethod",
        r##"(do (defmethod print-method String [s w] (.write w (str "<" s ">"))) :ok)"##,
        r##":ok"##,
    ),
    (
        "p6 after defmethod",
        r##"(with-out-str (println "c" 2) (print "d\n") (prn "e"))"##,
        r##"<c> 2\n<d\n><e>\n"##,
    ),
    (
        "p7 nested",
        r##"(with-out-str (println (with-out-str (print "f"))))"##,
        r##"<<f>>\n"##,
    ),
    (
        "p8 long method",
        r##"(do (defmethod print-method Long [n w] (.write w (str "#" n))) (with-out-str (println "g" 3)))"##,
        r##"<g> #3\n"##,
    ),
    (
        "p9 print-dup",
        r##"(with-out-str (binding [*print-dup* true] (prn 1 "h")))"##,
        r##"#1 <h>\n"##,
    ),
];

#[test]
fn print_fast_paths_follow_the_live_print_methods_277() {
    let mut eval = boot();
    eval.eval_source(
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.string]))",
    )
    .expect("clojure.string loads");
    for (label, expr, want) in ORACLE {
        let src = format!(
            "(clojure.string/escape (str (try {expr} (catch Throwable e (str (.getName (class e)) \": \" (.getMessage e))))) {{\\newline \"\\\\n\"}})"
        );
        let got = match eval.eval_source(&src).expect("the probe evaluates").last() {
            Some(Value::Str(s)) => s.to_string(),
            other => panic!("{label}: {other:?}"),
        };
        assert_eq!(got, *want, "{label}");
    }
}
