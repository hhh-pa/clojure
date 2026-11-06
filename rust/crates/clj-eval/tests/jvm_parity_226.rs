// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #226 JVM-parity fixes: `try` clauses built by macros (seq forms),
//! form-by-form loading (`::kw` after `ns`, `load-reader`),
//! namespace and `StringBuilder` display, JVM fn display names,
//! `clojure.string/split` argument checks, evaluated `def`
//! metadata, and `deftype`/`defrecord` factories. Every expected
//! value is the JVM's (Clojure 1.12 `clojure.main`; `evalN`/`--N`
//! stand for the JVM's own counters).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

/// `pr-str` of the form's value, or `Class: message` when it throws,
/// with fn-name counters normalized (`eval12` → `evalN`, `--7` → `--N`).
fn show(eval: &mut Evaluator, form: &str) -> String {
    let src = format!(
        "(try (pr-str {form}) (catch Throwable t (str (.getName (class t)) \": \" (.getMessage t))))"
    );
    let vs = eval
        .eval_source(&src)
        .unwrap_or_else(|e| panic!("{form}: {e}"));
    let s = pr_to_string(vs.last().unwrap(), &PrintOpts::print());
    normalize(&s)
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(|c: char| c.is_ascii_digit()) {
        let (head, tail) = rest.split_at(i);
        let digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        out.push_str(head);
        if head.ends_with("--") || head.ends_with("eval") {
            out.push('N');
        } else {
            out.push_str(&tail[..digits]);
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

const SETUP: &str = r#"(defmacro safely [& body] `(try ~@body (catch Exception e# (str "caught " (.getMessage e#)))))
(defmacro safely2 [& body] (list* 'try (concat body [(cons 'catch '(Exception e (str "c2 " (.getMessage e))))])))
(defmacro safely3 [& body] (concat (list 'try) body (list (concat '(catch Exception e) '((str "c3 " (.getMessage e)))))))
(def fin-log (atom nil))
(defmacro fin [& body] `(try ~@body (finally (reset! fin-log :ran))))
(defmacro body-seq [] (list 'try (cons 'inc '(1)) (cons 'catch '(Exception e :c))))
(load-string "(ns parity.kw) (def k ::kw) (require '[clojure.set :as cs]) (def j ::cs/x) (in-ns 'user)")
(defn bar [x] x)
(def f (fn [x] x))
(def h11 #(inc %))
(defn- priv [x] x)
(defn multi ([x] x) ([x y] y))
(defn outer [] (fn [x] x))
(defn outer2 [] (fn inner [x] x))
(defn outer3 [] (let [g (fn [x] x)] g))
(defn outer19 [] (letfn [(lf19 [x] x)] lf19))
(def ^:dynamic *d23* (fn [x] x))
(def x25 (fn* [x] x))
(defn outer22 [] #(inc %))
(defrecord Pt [x y])
(deftype Ty [a])
(def ^String x1 "a")
(def ^long x2 1)
(def ^{:tag 'long} x3 1)
(def ^{:a [1 (inc 1)] :b #{(inc 2)} :c {:d (inc 3)} :e '(1)} x5 1)
(def ^{:k :v} x7 1)
(def ^{:b #{(inc 2)}} x10 1)
(def ^{:c {:d (inc 3)}} x11 1)
(def ^{:k #{:v} :m {:n [1]}} x12 1)
(defmacro mk8 [] (list 'def (with-meta 'x8 {:q (concat '(+) '(1 2))}) 1))
(mk8)
(def order (atom []))
(def ^{:m (swap! order conj :meta)} x9 (swap! order conj :init))
(with-in-str "(def rl (read-line))   :after\n42" (load-reader *in*))
"#;

const CASES: &[(&str, &str)] = &[
    (
        r#"(safely (throw (Exception. "boom")))"#,
        r#""caught boom""#,
    ),
    (r#"(safely2 (throw (Exception. "boom2")))"#, r#""c2 boom2""#),
    (r#"(safely3 (throw (Exception. "boom3")))"#, r#""c3 boom3""#),
    (r#"(do (fin 1) @fin-log)"#, r#":ran"#),
    (r#"(body-seq)"#, r#"2"#),
    (
        r#"[parity.kw/k parity.kw/j]"#,
        r#"[:parity.kw/kw :clojure.set/x]"#,
    ),
    (
        r#"[(str *ns*) (format "%s|%s" *ns* (find-ns (quote clojure.core))) (.toString *ns*) (str "ns:" *ns*)]"#,
        r#"["user" "user|clojure.core" "user" "ns:user"]"#,
    ),
    (
        r##"(boolean (re-matches #"#object\[clojure.lang.Namespace 0x[0-9a-f]+ user\]" (print-str *ns*)))"##,
        r#"true"#,
    ),
    (
        r##"(boolean (re-matches #"#object\[clojure.lang.Namespace 0x[0-9a-f]+ \"user\"\]" (pr-str *ns*)))"##,
        r#"true"#,
    ),
    (
        r##"(let [sb (StringBuilder. "a \"b")] [(str sb) (boolean (re-matches #"#object\[java.lang.StringBuilder 0x[0-9a-f]+ \"a \\\"b\"\]" (pr-str sb))) (boolean (re-matches #"#object\[java.lang.StringBuilder 0x[0-9a-f]+ a \"b\]" (print-str sb)))])"##,
        r#"["a \"b" true true]"#,
    ),
    (
        r#"(bar)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/bar"#,
    ),
    (
        r#"(f)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/f"#,
    ),
    (
        r#"(h11)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/h11"#,
    ),
    (
        r#"(priv)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/priv"#,
    ),
    (
        r#"(multi)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/multi"#,
    ),
    (
        r#"((outer))"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/outer/fn--N"#,
    ),
    (
        r#"((outer2))"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/outer2/inner--N"#,
    ),
    (
        r#"((outer3))"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/outer3/g--N"#,
    ),
    (
        r#"((outer19))"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/outer19/lf19--N"#,
    ),
    (
        r#"(*d23*)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/*d23*"#,
    ),
    (
        r#"(x25)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/x25"#,
    ),
    (
        r#"((outer22))"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/outer22/fn--N"#,
    ),
    (
        r#"((comparator <) 1)"#,
        r#"clojure.lang.ArityException: Wrong number of args (1) passed to: clojure.core/comparator/fn--N"#,
    ),
    (
        r#"(->Pt 1)"#,
        r#"clojure.lang.ArityException: Wrong number of args (1) passed to: user/evalN/->Pt--N"#,
    ),
    (
        r#"(map->Pt)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/evalN/map->Pt--N"#,
    ),
    (
        r#"(->Ty)"#,
        r#"clojure.lang.ArityException: Wrong number of args (0) passed to: user/evalN/->Ty--N"#,
    ),
    (r#"(.-a (->Ty 5))"#, r#"5"#),
    (
        r#"(:doc (meta #'->Ty))"#,
        r#""Positional factory function for class user.Ty.""#,
    ),
    (r#"(:arglists (meta #'->Ty))"#, r#"([a])"#),
    (
        r#"(:doc (meta #'map->Pt))"#,
        r#""Factory function for class user.Pt, taking a map of keywords to field values.""#,
    ),
    (r#"(:arglists (meta #'->Pt))"#, r#"([x y])"#),
    (
        r#"(str/split "a b" nil)"#,
        r#"java.lang.NullPointerException: Cannot invoke "java.util.regex.Pattern.split(java.lang.CharSequence)" because "re" is null"#,
    ),
    (
        r#"(str/split "a b" 1)"#,
        r#"java.lang.ClassCastException: class java.lang.Long cannot be cast to class java.util.regex.Pattern (java.lang.Long and java.util.regex.Pattern are in module java.base of loader 'bootstrap')"#,
    ),
    (
        r#"(str/split "a b" #" " "x")"#,
        r#"java.lang.ClassCastException: class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"#,
    ),
    (
        r#"(str/split "a b c" #" " nil)"#,
        r#"java.lang.NullPointerException: Cannot invoke "java.lang.Character.charValue()" because "x" is null"#,
    ),
    (
        r#"(str/split 1 nil)"#,
        r#"java.lang.ClassCastException: class java.lang.Long cannot be cast to class java.lang.CharSequence (java.lang.Long and java.lang.CharSequence are in module java.base of loader 'bootstrap')"#,
    ),
    (r#"(:tag (meta #'x1))"#, r#"java.lang.String"#),
    (r#"(fn? (:tag (meta #'x2)))"#, r#"true"#),
    (r#"(:tag (meta #'x3))"#, r#"long"#),
    (
        r#"((juxt :a :b :c :e) (meta #'x5))"#,
        r#"[[1 2] #{3} {:d 4} (1)]"#,
    ),
    (r#"(:k (meta #'x7))"#, r#":v"#),
    (r#"(:b (meta #'x10))"#, r#"#{3}"#),
    (r#"(:c (meta #'x11))"#, r#"{:d 4}"#),
    (r#"((juxt :k :m) (meta #'x12))"#, r#"[#{:v} {:n [1]}]"#),
    (r#"(:q (meta #'x8))"#, r#"3"#),
    (r#"@order"#, r#"[:init :meta]"#),
    (r#"rl"#, r#"":after""#),
    (
        r#"(with-in-str "(ns lr.x) (def y ::k) (in-ns 'user) 42" [(load-reader *in*) (str *ns*)])"#,
        r#"[42 "user"]"#,
    ),
    (r#"lr.x/y"#, r#":lr.x/k"#),
    (
        r#"[(nth (.split "a:b:" ":") 0) (count (.split "a:b:" ":")) (reduce str (.split "ab" "x")) (vec (seq (.split "ab" "(?<=a)"))) (str/split "a b c" #" " 2) (str/split "a b " #" ")]"#,
        r#"["a" 2 "ab" ["a" "b"] ["a" "b c"] ["a" "b"]]"#,
    ),
];

#[test]
fn jvm_parity_rows() {
    let mut eval = boot();
    // `clojure.main` runs scripts inside a `*warn-on-reflection*` frame.
    eval.eval_source(
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.string :as str]))",
    )
    .unwrap();
    eval.eval_source(SETUP)
        .unwrap_or_else(|e| panic!("setup: {e}"));
    for (form, want) in CASES {
        assert_eq!(&show(&mut eval, form), want, "{form}");
    }
}
