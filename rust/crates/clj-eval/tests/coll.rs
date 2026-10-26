// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D collections: probe replay for the `Collections` matrix rows
//! (`count`, `conj`/`assoc`/`dissoc`, `get`/`nth`, `vec`/`merge`,
//! transients, `peek`/`pop`, `subvec`, `replace`, `into`,
//! `distinct?` — see `docs/t1174/s5d-coll-pins.md`).

use clj_eval::{Evaluator, SourceError, boot, jvm_class_of};
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

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
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
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// The JVM class of the last value (`class` is out of scope, so
/// class assertions read Rust-side and never force).
fn class_of(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    jvm_class_of(vs.last().unwrap())
}

#[test]
fn count_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(count nil)"), "0");
    assert_eq!(run_last(&mut eval, "(count [1 2])"), "2");
    assert_eq!(run_last(&mut eval, "(count \"ab\")"), "2");
    assert_eq!(run_last(&mut eval, "(count \"😀\")"), "2");
    assert_eq!(run_last(&mut eval, "(count {:a 1})"), "1");
    assert_eq!(run_last(&mut eval, "(count #{1 2})"), "2");
    assert_eq!(run_last(&mut eval, "(count '(1 2 3))"), "3");
    assert_eq!(run_last(&mut eval, "(count (transient [1 2]))"), "2");
    assert_eq!(run_last(&mut eval, "(count (subvec [1 2 3] 1))"), "2");
    assert_eq!(run_last(&mut eval, "(count (range 5))"), "5");
    assert_eq!(run_last(&mut eval, "(count (range 0 5.0))"), "5");
    assert_eq!(run_last(&mut eval, "(count (repeat 5 :x))"), "5");
    assert_eq!(run_last(&mut eval, "(count (repeat 0 :x))"), "0");
    assert_eq!(
        uncaught(&mut eval, "(count 5)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(count :a)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(count \\a)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Character"
    );
}

#[test]
fn empty_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(empty? [])"), "true");
    assert_eq!(run_last(&mut eval, "(empty? [1])"), "false");
    assert_eq!(run_last(&mut eval, "(empty? nil)"), "true");
    assert_eq!(run_last(&mut eval, "(empty? \"\")"), "true");
    assert_eq!(run_last(&mut eval, "(empty? \"ab\")"), "false");
    assert_eq!(run_last(&mut eval, "(empty? (transient [1]))"), "false");
    assert_eq!(run_last(&mut eval, "(empty? (transient []))"), "true");
    assert_eq!(
        uncaught(&mut eval, "(empty? 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(not-empty [1])"), "[1]");
    assert_eq!(run_last(&mut eval, "(not-empty [])"), "nil");
    assert_eq!(run_last(&mut eval, "(not-empty nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(not-empty \"ab\")"), "\"ab\"");
}

#[test]
fn pos_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(second [1 2 3])"), "2");
    assert_eq!(run_last(&mut eval, "(second nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(last [1 2 3])"), "3");
    assert_eq!(run_last(&mut eval, "(last nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(last \"ab\")"), "\\b");
    assert_eq!(
        uncaught(&mut eval, "(last 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(nnext [1 2 3])"), "(3)");
    assert_eq!(run_last(&mut eval, "(nnext [1])"), "nil");
    assert_eq!(run_last(&mut eval, "(fnext [1 2 3])"), "2");
    assert_eq!(run_last(&mut eval, "(nfirst [[1 2] 3])"), "(2)");
    assert_eq!(
        uncaught(&mut eval, "(nfirst [1])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn list_star_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(list* [1 2])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(list* 1 [2 3])"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(list* 1 2 [3])"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(list* nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(list*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/list*"
    );
    assert_eq!(
        uncaught(&mut eval, "(list* 1 2)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn conj_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(conj)"), "[]");
    assert_eq!(run_last(&mut eval, "(conj nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(conj 5)"), "5");
    assert_eq!(run_last(&mut eval, "(conj nil 1)"), "(1)");
    assert_eq!(run_last(&mut eval, "(conj nil 1 2 3)"), "(3 2 1)");
    assert_eq!(run_last(&mut eval, "(conj [1] 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(conj '(1) 0)"), "(0 1)");
    assert_eq!(run_last(&mut eval, "(conj {:a 1} [:b 2])"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(conj {:a 1} {:b 2})"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(conj {:a 1} nil)"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(conj {:a 1} '())"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(conj #{1} 2)"), "#{1 2}");
    assert_eq!(
        run_last(&mut eval, "(conj (subvec [1 2 3] 1) 9)"),
        "[2 3 9]"
    );
    assert_eq!(
        class_of(&mut eval, "(conj (subvec [1 2 3] 1) 9)"),
        "clojure.lang.APersistentVector$SubVector"
    );
    assert_eq!(run_last(&mut eval, "(conj (map inc [1]) 0)"), "(0 2)");
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} [:b])"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} :b)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {} (seq [[1 2]]))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj \"ab\" \\c)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentCollection (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentCollection is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj (transient [1]) 2)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector$TransientVector cannot be cast to class clojure.lang.IPersistentCollection (clojure.lang.PersistentVector$TransientVector and clojure.lang.IPersistentCollection are in unnamed module of loader 'app')"
    );
}

#[test]
fn assoc_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(assoc nil :a 1)"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(assoc {:a 1} :b 2)"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(assoc [1 2] 0 9)"), "[9 2]");
    assert_eq!(run_last(&mut eval, "(assoc [1 2] 2 9)"), "[1 2 9]");
    assert_eq!(run_last(&mut eval, "(assoc [] 0 9)"), "[9]");
    assert_eq!(run_last(&mut eval, "(assoc [1 2] 0 9 1 8)"), "[9 8]");
    assert_eq!(
        run_last(&mut eval, "(assoc (subvec [1 2 3] 0) 0 9)"),
        "[9 2 3]"
    );
    assert_eq!(
        class_of(&mut eval, "(assoc (subvec [1 2 3] 0) 0 9)"),
        "clojure.lang.APersistentVector$SubVector"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc [1 2] 1.0 9)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc [1 2] :a 9)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc [1 2] -1 9)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc {:a 1} :b 2 :c)"),
        "java.lang.IllegalArgumentException | assoc expects even number of arguments after map/vector, found odd number"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc {:a 1})"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/assoc"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc 5 :a 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Associative (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
}

#[test]
fn dissoc_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(dissoc {:a 1 :b 2} :a)"), "{:b 2}");
    assert_eq!(run_last(&mut eval, "(dissoc {:a 1} :z)"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(dissoc nil :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(dissoc {:a 1})"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(dissoc 5)"), "5");
    assert_eq!(
        uncaught(&mut eval, "(dissoc [1 2] 0)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.PersistentVector and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(dissoc (transient {:a 1}) :a)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap$TransientArrayMap cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.PersistentArrayMap$TransientArrayMap and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
}

#[test]
fn get_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(get {:a 1} :a)"), "1");
    assert_eq!(run_last(&mut eval, "(get {:a 1} :z)"), "nil");
    assert_eq!(run_last(&mut eval, "(get {:a 1} :z :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get [1 2] 0)"), "1");
    assert_eq!(run_last(&mut eval, "(get [1 2] :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(get [1 2] :a :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get [1 2] 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(get [1 2] 5 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get [1 2] 1.0)"), "nil");
    assert_eq!(run_last(&mut eval, "(get \"abc\" 1)"), "\\b");
    assert_eq!(run_last(&mut eval, "(get \"abc\" 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(get \"abc\" 5 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get \"abc\" :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(get #{1} 1)"), "1");
    assert_eq!(run_last(&mut eval, "(get #{1} 2)"), "nil");
    assert_eq!(run_last(&mut eval, "(get nil :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(get 5 0)"), "nil");
    assert_eq!(run_last(&mut eval, "(get (transient {:a 1}) :a)"), "1");
    assert_eq!(
        run_last(&mut eval, "(get (transient {:a 1}) :b :nf)"),
        ":nf"
    );
    assert_eq!(run_last(&mut eval, "(get (transient [1 2]) :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(get (subvec [1 2] 0) 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(get (subvec [1 2] 0) :a)"), "nil");
}

#[test]
fn req_rows() {
    // M2 `req!` (probes `req-present`, `req-absent`, `req-nilval`,
    // `req-vec`, `req-vec-miss`, `req-str`, `req-nil`, `req-arity3`,
    // `req-arity1` on alpha10): arity-2 `get` that throws when the
    // key is absent.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(req! {:a 1} :a)"), "1");
    assert_eq!(run_last(&mut eval, "(req! {:a nil} :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(req! [1 2] 1)"), "2");
    assert_eq!(run_last(&mut eval, "(req! \"ab\" 1)"), "\\b");
    assert_eq!(
        uncaught(&mut eval, "(req! {} :a)"),
        "java.lang.IllegalArgumentException | Missing required key: :a"
    );
    assert_eq!(
        uncaught(&mut eval, "(req! [1 2] 5)"),
        "java.lang.IllegalArgumentException | Missing required key: 5"
    );
    assert_eq!(
        uncaught(&mut eval, "(req! nil :a)"),
        "java.lang.IllegalArgumentException | Missing required key: :a"
    );
    assert_eq!(
        uncaught(&mut eval, "(req! {:a 1} :a :nf)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/req!"
    );
    assert_eq!(
        uncaught(&mut eval, "(req! {:a 1})"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/req!"
    );
}

#[test]
fn contains_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(contains? {:a 1} :a)"), "true");
    assert_eq!(run_last(&mut eval, "(contains? {:a 1} :z)"), "false");
    assert_eq!(run_last(&mut eval, "(contains? [1 2] 0)"), "true");
    assert_eq!(run_last(&mut eval, "(contains? [1 2] :a)"), "false");
    assert_eq!(run_last(&mut eval, "(contains? [1 2] 5)"), "false");
    assert_eq!(run_last(&mut eval, "(contains? \"ab\" 1)"), "true");
    assert_eq!(run_last(&mut eval, "(contains? \"ab\" 5)"), "false");
    assert_eq!(run_last(&mut eval, "(contains? nil :a)"), "false");
    assert_eq!(run_last(&mut eval, "(contains? #{1} 1)"), "true");
    assert_eq!(run_last(&mut eval, "(contains? (transient [1]) 0)"), "true");
    assert_eq!(
        run_last(&mut eval, "(contains? (subvec [1 2] 0) :a)"),
        "false"
    );
    assert_eq!(
        uncaught(&mut eval, "(contains? 5 0)"),
        "java.lang.IllegalArgumentException | contains? not supported on type: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(contains? '(1) 0)"),
        "java.lang.IllegalArgumentException | contains? not supported on type: clojure.lang.PersistentList"
    );
    assert_eq!(
        uncaught(&mut eval, "(contains? \"ab\" :a)"),
        "java.lang.IllegalArgumentException | contains? not supported on type: java.lang.String"
    );
}

#[test]
fn find_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(find {:a 1} :a)"), "[:a 1]");
    assert_eq!(run_last(&mut eval, "(find {:a 1} :z)"), "nil");
    assert_eq!(run_last(&mut eval, "(find nil :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(find [1 2] 0)"), "[0 1]");
    assert_eq!(run_last(&mut eval, "(find [1 2] 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(find [1 2] :a)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(find \"ab\" 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(find {:a 1} :a :nf)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/find"
    );
}

#[test]
fn nth_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(nth [1 2] 0)"), "1");
    assert_eq!(run_last(&mut eval, "(nth [1 2] 1)"), "2");
    assert_eq!(run_last(&mut eval, "(nth \"abc\" 1)"), "\\b");
    assert_eq!(run_last(&mut eval, "(nth '(1 2) 1)"), "2");
    assert_eq!(run_last(&mut eval, "(nth nil 0)"), "nil");
    assert_eq!(run_last(&mut eval, "(nth nil 0 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(nth [1 2] 5 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(nth \"abc\" 5 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(nth (transient [1 2]) 1)"), "2");
    assert_eq!(run_last(&mut eval, "(nth (transient [1 2]) 5 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(nth [1 2] 1.9)"), "2");
    assert_eq!(run_last(&mut eval, "(nth (map inc [1 2]) 1)"), "3");
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2] 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2] -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(nth \"abc\" 5)"),
        "java.lang.StringIndexOutOfBoundsException | Index 5 out of bounds for length 3"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth \"ab\" -1)"),
        "java.lang.StringIndexOutOfBoundsException | Index -1 out of bounds for length 2"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth 5 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth {:a 1} 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: PersistentArrayMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (transient {:a 1}) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: TransientArrayMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1] nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1] :a)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2 3] 4294967296)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2 3] 1e300)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    // f64 indexes truncate (T1192 FX1 fx1-rest probe); past-2^53
    // doubles fail `toIntExact` like the JVM.
    assert_eq!(run_last(&mut eval, "(nth [1 2 3] 1.0)"), "2");
    assert_eq!(
        uncaught(&mut eval, "(nth [1] 9.007199254740992E15)"),
        "java.lang.ArithmeticException | integer overflow"
    );
}

#[test]
fn vec_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(vec [1 2])"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(vec '(1 2))"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(vec \"ab\")"), "[\\a \\b]");
    assert_eq!(run_last(&mut eval, "(vec {:a 1})"), "[[:a 1]]");
    assert_eq!(run_last(&mut eval, "(vec nil)"), "[]");
    assert_eq!(
        class_of(&mut eval, "(vec (subvec [1 2] 0))"),
        "clojure.lang.APersistentVector$SubVector"
    );
    assert_eq!(
        class_of(&mut eval, "(vec (map inc [1]))"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (vec (with-meta [1] {:a 1})))"),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(vec 5)"),
        "java.lang.RuntimeException | Unable to convert: class java.lang.Long to Object[]"
    );
    assert_eq!(
        uncaught(&mut eval, "(vec (transient [1 2]))"),
        "java.lang.RuntimeException | Unable to convert: class clojure.lang.PersistentVector$TransientVector to Object[]"
    );
}

#[test]
fn merge_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(merge)"), "nil");
    assert_eq!(run_last(&mut eval, "(merge nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(merge 5)"), "5");
    assert_eq!(run_last(&mut eval, "(merge {:a 1} {:b 2})"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(merge {:a 1} nil)"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(merge nil {:a 1})"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(merge [1] [2])"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(merge [1] nil [2])"), "[1 2]");
    assert_eq!(
        uncaught(&mut eval, "(merge {:a 1} [:b 2])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
}

#[test]
fn merge_with_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(merge-with +)"), "nil");
    assert_eq!(run_last(&mut eval, "(merge-with + {:a 1})"), "{:a 1}");
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} {:a 2})"),
        "{:a 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} [:x :y])"),
        "{:a 1, 0 :x, 1 :y}"
    );
    assert_eq!(run_last(&mut eval, "(merge-with + [1 2] [3 4])"), "[4 6]");
    assert_eq!(
        uncaught(&mut eval, "(merge-with + {:a 1} \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.util.Map$Entry (java.lang.Character and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn select_keys_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(select-keys {:a 1 :b 2} [:a :z])"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(select-keys [1 2] [0])"), "{0 1}");
    assert_eq!(run_last(&mut eval, "(select-keys nil [:a])"), "{}");
    assert_eq!(run_last(&mut eval, "(select-keys {:a 1} [])"), "{}");
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (select-keys (with-meta {:a 1} {:m 1}) [:a]))"
        ),
        "{:m 1}"
    );
}

#[test]
fn update_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(update {:a 1} :a inc)"), "{:a 2}");
    assert_eq!(
        run_last(&mut eval, "(update {} :a vector 1 2)"),
        "{:a [nil 1 2]}"
    );
    assert_eq!(run_last(&mut eval, "(update [1 2] 0 inc)"), "[2 2]");
    assert_eq!(
        uncaught(&mut eval, "(update 5 :a (fn [_] 1))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Associative (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
}

#[test]
fn in_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(get-in {:a {:b 1}} [:a :b])"), "1");
    assert_eq!(run_last(&mut eval, "(get-in {:a {:b 1}} [:a :z])"), "nil");
    assert_eq!(
        run_last(&mut eval, "(get-in {:a {:b 1}} [:a :z] :nf)"),
        ":nf"
    );
    assert_eq!(run_last(&mut eval, "(get-in nil [:a] :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get-in {:a nil} [:a :b] :nf)"), ":nf");
    assert_eq!(
        run_last(&mut eval, "(assoc-in {} [:a :b] 1)"),
        "{:a {:b 1}}"
    );
    assert_eq!(run_last(&mut eval, "(assoc-in {} [] 1)"), "{nil 1}");
    assert_eq!(run_last(&mut eval, "(assoc-in nil [:a] 1)"), "{:a 1}");
    assert_eq!(
        run_last(&mut eval, "(update-in {} [:a] (fnil inc 0))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(update-in nil [:a] vector)"),
        "{:a [nil]}"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc-in {:a 1} [:a :b] 2)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Associative (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(update-in {} 5 vector)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn zipmap_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(zipmap [:a :b] [1 2])"),
        "{:a 1, :b 2}"
    );
    assert_eq!(run_last(&mut eval, "(zipmap [1 2 3] [:a])"), "{1 :a}");
    assert_eq!(run_last(&mut eval, "(zipmap [1] nil)"), "{}");
    assert_eq!(run_last(&mut eval, "(zipmap nil nil)"), "{}");
    assert_eq!(
        uncaught(&mut eval, "(zipmap 5 [1])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn array_map_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(array-map)"), "{}");
    assert_eq!(run_last(&mut eval, "(array-map :a 1 :b 2)"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(array-map :a 1 :a 2)"), "{:a 2}");
    assert_eq!(
        class_of(&mut eval, "(array-map :a 1)"),
        "clojure.lang.PersistentArrayMap"
    );
    assert_eq!(
        class_of(&mut eval, "(hash-map :a 1)"),
        "clojure.lang.PersistentHashMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(array-map :a)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :a"
    );
}

#[test]
fn transient_rows() {
    let mut eval = boot();
    assert_eq!(
        class_of(&mut eval, "(transient [1])"),
        "clojure.lang.PersistentVector$TransientVector"
    );
    assert_eq!(
        run_last(&mut eval, "(persistent! (transient [1 2]))"),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient [1])] (conj! t 2) (persistent! t))"
        ),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient {:a 1})] (assoc! t :b 2) (persistent! t))"
        ),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient {:a 1})] (dissoc! t :a) (persistent! t))"
        ),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient [1 2])] (pop! t) (persistent! t))"
        ),
        "[1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient #{1})] (conj! t 2) (persistent! t))"
        ),
        "#{1 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [t (transient {:a 1})] (conj! t [:b 2]) (persistent! t))"
        ),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (persistent! (transient (with-meta [1] {:a 1}))))"
        ),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(meta (transient [1]))"), "nil");
    assert_eq!(
        class_of(&mut eval, "(conj!)"),
        "clojure.lang.PersistentVector$TransientVector"
    );
    assert_eq!(run_last(&mut eval, "(conj! [1])"), "[1]");
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [t (transient [1])] (persistent! t) (persistent! t))"
        ),
        "java.lang.IllegalAccessError | Transient used after persistent! call"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [t (transient [1])] (persistent! t) (conj! t 2))"
        ),
        "java.lang.IllegalAccessError | Transient used after persistent! call"
    );
    assert_eq!(
        uncaught(&mut eval, "(transient 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IEditableCollection (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IEditableCollection is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(transient nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IEditableCollection.asTransient()\" because \"coll\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(persistent! [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.ITransientCollection (clojure.lang.PersistentVector and clojure.lang.ITransientCollection are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj! nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientCollection.conj(Object)\" because \"coll\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! nil :a 1)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientAssociative.assoc(Object, Object)\" because \"coll\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! (transient [1]) :a 1)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! (transient [1]) 5 :x)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(pop! (transient []))"),
        "java.lang.IllegalStateException | Can't pop empty vector"
    );
    assert_eq!(
        uncaught(&mut eval, "(dissoc! {:a 1} :a)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class clojure.lang.ITransientMap (clojure.lang.PersistentArrayMap and clojure.lang.ITransientMap are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop! (transient {}))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap$TransientArrayMap cannot be cast to class clojure.lang.ITransientVector (clojure.lang.PersistentArrayMap$TransientArrayMap and clojure.lang.ITransientVector are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! {:a 1} :b 2)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class clojure.lang.ITransientAssociative (clojure.lang.PersistentArrayMap and clojure.lang.ITransientAssociative are in unnamed module of loader 'app')"
    );
    // Wrong-kind bangs read the transient's class without
    // re-locking (each of these deadlocked once).
    assert_eq!(
        uncaught(&mut eval, "(dissoc! (transient [1]) :a)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector$TransientVector cannot be cast to class clojure.lang.ITransientMap (clojure.lang.PersistentVector$TransientVector and clojure.lang.ITransientMap are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! (transient #{1}) :a 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentHashSet$TransientHashSet cannot be cast to class clojure.lang.ITransientAssociative (clojure.lang.PersistentHashSet$TransientHashSet and clojure.lang.ITransientAssociative are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(find (transient #{1}) 1)"),
        "java.lang.IllegalArgumentException | find not supported on type: clojure.lang.PersistentHashSet$TransientHashSet"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [t (transient {:a 1})] (conj! t (list t)))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap$TransientArrayMap cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentArrayMap$TransientArrayMap is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn peek_pop_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(peek [1 2])"), "2");
    assert_eq!(run_last(&mut eval, "(peek '(1 2))"), "1");
    assert_eq!(run_last(&mut eval, "(peek nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(peek (subvec [1 2] 1))"), "2");
    assert_eq!(run_last(&mut eval, "(pop [1 2])"), "[1]");
    assert_eq!(run_last(&mut eval, "(pop '(1 2))"), "(2)");
    assert_eq!(run_last(&mut eval, "(pop nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(pop (subvec [1 2 3] 0))"), "[1 2]");
    assert_eq!(
        class_of(&mut eval, "(pop (subvec [1 2 3] 0))"),
        "clojure.lang.APersistentVector$SubVector"
    );
    assert_eq!(run_last(&mut eval, "(pop (subvec [1 2] 1))"), "[]");
    assert_eq!(
        class_of(&mut eval, "(pop (subvec [1 2] 1))"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop [])"),
        "java.lang.IllegalStateException | Can't pop empty vector"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop '())"),
        "java.lang.IllegalStateException | Can't pop empty list"
    );
    assert_eq!(
        uncaught(&mut eval, "(peek \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentStack (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentStack is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(peek (map inc [1]))"),
        "java.lang.ClassCastException | class clojure.lang.LazySeq cannot be cast to class clojure.lang.IPersistentStack (clojure.lang.LazySeq and clojure.lang.IPersistentStack are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop (range 3))"),
        "java.lang.ClassCastException | class clojure.lang.LongRange cannot be cast to class clojure.lang.IPersistentStack (clojure.lang.LongRange and clojure.lang.IPersistentStack are in unnamed module of loader 'app')"
    );
}

#[test]
fn subvec_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(subvec [1 2 3] 1)"), "[2 3]");
    assert_eq!(run_last(&mut eval, "(subvec [1 2 3] 1 2)"), "[2]");
    assert_eq!(run_last(&mut eval, "(subvec [1 2 3] 0 0)"), "[]");
    assert_eq!(
        class_of(&mut eval, "(subvec [1 2 3] 1)"),
        "clojure.lang.APersistentVector$SubVector"
    );
    assert_eq!(
        class_of(&mut eval, "(subvec [1 2 3] 0 0)"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run_last(&mut eval, "(subvec [1 2 3] 1.9)"), "[2 3]");
    assert_eq!(
        run_last(&mut eval, "(subvec (subvec [1 2 3] 0) 1)"),
        "[2 3]"
    );
    assert_eq!(
        class_of(&mut eval, "(seq (subvec [1 2 3] 0))"),
        "clojure.lang.APersistentVector$Seq"
    );
    assert_eq!(run_last(&mut eval, "(count (subvec [1 2 3] 1))"), "2");
    assert_eq!(run_last(&mut eval, "(nth (subvec [1 2 3] 1) 0)"), "2");
    assert_eq!(run_last(&mut eval, "((subvec [1 2] 0) 1)"), "2");
    assert_eq!(
        run_last(&mut eval, "(assoc (subvec [1 2] 0) 2 9)"),
        "[1 2 9]"
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec [1] nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec [1] :a)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec [1 2] 2 1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec [1] 0 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec \"ab\" 0)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentVector (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec 5 0)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: Long"
    );
    assert_eq!(
        uncaught(&mut eval, "((subvec [1 2] 0) :a)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "((subvec [1 2] 0) 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.lang.APersistentVector/SubVector"
    );
}

#[test]
fn replace_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(replace {1 :a} [1 2])"), "[:a 2]");
    assert_eq!(run_last(&mut eval, "(replace {1 :a} '(1 2))"), "(:a 2)");
    assert_eq!(
        class_of(&mut eval, "(replace {1 :a} '(1 2))"),
        "clojure.lang.LazySeq"
    );
    assert_eq!(run_last(&mut eval, "(replace nil [1 2])"), "[1 2]");
    assert_eq!(
        class_of(&mut eval, "(replace {1 :a} (subvec [1 2] 0))"),
        "clojure.lang.APersistentVector$SubVector"
    );
}

#[test]
fn into_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(into)"), "[]");
    assert_eq!(run_last(&mut eval, "(into [1])"), "[1]");
    assert_eq!(run_last(&mut eval, "(into [] [1 2])"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(into [] nil)"), "[]");
    assert_eq!(run_last(&mut eval, "(into nil [1 2])"), "(2 1)");
    assert_eq!(run_last(&mut eval, "(into {} [[:a 1]])"), "{:a 1}");
    assert_eq!(run_last(&mut eval, "(into #{} [1 1 2])"), "#{1 2}");
    assert_eq!(
        uncaught(&mut eval, "(into [] :bad)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
}

#[test]
fn distinct_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(distinct? 1)"), "true");
    assert_eq!(run_last(&mut eval, "(distinct? 1 2)"), "true");
    assert_eq!(run_last(&mut eval, "(distinct? 1 1)"), "false");
    assert_eq!(run_last(&mut eval, "(distinct? 1 2 3)"), "true");
    assert_eq!(run_last(&mut eval, "(distinct? 1 2 1)"), "false");
    assert_eq!(
        uncaught(&mut eval, "(distinct?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/distinct?"
    );
}

#[test]
fn counted_pred_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(counted? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(counted? \"ab\")"), "false");
    assert_eq!(run_last(&mut eval, "(counted? [1])"), "true");
    assert_eq!(run_last(&mut eval, "(counted? (range 5))"), "true");
    assert_eq!(run_last(&mut eval, "(counted? (range 0 5.0))"), "false");
    assert_eq!(run_last(&mut eval, "(counted? (repeat 5 :x))"), "false");
    assert_eq!(run_last(&mut eval, "(counted? (cons 1 nil))"), "true");
    // `Cons` is never `Counted`, even over a vector (fixprobe7).
    assert_eq!(run_last(&mut eval, "(counted? (cons 1 [2]))"), "false");
    assert_eq!(
        run_last(&mut eval, "(counted? (conj (range 5) 9))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(counted? (conj (map inc [1 2]) 0))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(count (cons 1 [2]))"), "2");
    assert_eq!(run_last(&mut eval, "(counted? (keys {:a 1}))"), "false");
    assert_eq!(run_last(&mut eval, "(counted? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(counted? (transient [1]))"), "true");
    assert_eq!(run_last(&mut eval, "(vector? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(reversible? (subvec [1] 0))"), "true");
    assert_eq!(
        run_last(&mut eval, "(reversible? (transient [1]))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(sequential? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(associative? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(coll? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(seqable? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(ifn? (subvec [1] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(map? (subvec [1] 0))"), "false");
    assert_eq!(run_last(&mut eval, "(seq? (subvec [1] 0))"), "false");
}

#[test]
fn empty_fn_rows() {
    let mut eval = boot();
    // Values.
    assert_eq!(run_last(&mut eval, "(empty [1])"), "[]");
    assert_eq!(run_last(&mut eval, "(empty '(1))"), "()");
    assert_eq!(run_last(&mut eval, "(empty {:a 1})"), "{}");
    assert_eq!(run_last(&mut eval, "(empty #{1})"), "#{}");
    assert_eq!(run_last(&mut eval, "(empty (range 5))"), "()");
    assert_eq!(run_last(&mut eval, "(empty (map inc [1]))"), "()");
    // Seqs empty to a bare `()` with no meta (JVM `RT.empty`);
    // strict kinds keep theirs (T1192 FX1).
    assert_eq!(
        run_last(&mut eval, "(empty (with-meta (seq [1 2]) {:a 1}))"),
        "()"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (empty (with-meta (seq [1 2]) {:a 1})))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (empty (with-meta [1] {:a 1})))"),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "(empty (subvec [1 2 3] 0 2))"), "[]");
    // Non-collections (and transients) are nil.
    for form in [
        "(empty nil)",
        "(empty \"ab\")",
        "(empty 5)",
        "(empty :a)",
        "(empty \\c)",
        "(empty true)",
        "(empty 'sym)",
        "(empty (transient [1]))",
        "(empty (transient {:a 1}))",
    ] {
        assert_eq!(run_last(&mut eval, form), "nil", "{form}");
    }
    // Maps are class-sticky; subvectors fall back to plain vectors.
    assert_eq!(
        class_of(&mut eval, "(empty {:a 1})"),
        "clojure.lang.PersistentArrayMap"
    );
    assert_eq!(
        class_of(&mut eval, "(empty (hash-map :a 1))"),
        "clojure.lang.PersistentHashMap"
    );
    assert_eq!(
        class_of(&mut eval, "(empty (subvec [1 2 3] 0 2))"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(
        class_of(&mut eval, "(empty [1])"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(
        class_of(&mut eval, "(empty '(1))"),
        "clojure.lang.PersistentList$EmptyList"
    );
    assert_eq!(
        class_of(&mut eval, "(empty #{1})"),
        "clojure.lang.PersistentHashSet"
    );
    assert_eq!(
        class_of(&mut eval, "(empty (range 5))"),
        "clojure.lang.PersistentList$EmptyList"
    );
    assert_eq!(
        class_of(&mut eval, "(empty (map inc [1]))"),
        "clojure.lang.PersistentList$EmptyList"
    );
    // Metadata is kept on the strict kinds.
    assert_eq!(
        run_last(&mut eval, "(meta (empty (with-meta [1] {:m 1})))"),
        "{:m 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(meta (empty (with-meta {:a 1} {:m 1})))"),
        "{:m 1}"
    );
}

/// Corpus gaps (P1 corpus divergences: port behavior asserted, JVM differs (see comments).)
#[test]
fn corpus_p1_divergences() {
    let mut eval = boot();
    // :update-tmap — CONFORMANT (T1192 FX1): single-shot JVM throws the full
    // checkcast text below, byte-identical (fx1-rest probe); the old "bare CCE"
    // record was HotSpot OmitStackTraceInFastThrow after ~1k hot throws
    // (fx1-fastthrow probe), which the port does not mirror (N3).
    assert_eq!(
        uncaught(&mut eval, "(update (transient {:a 1}) :a inc)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap$TransientArrayMap cannot be cast to class clojure.lang.Associative (clojure.lang.PersistentArrayMap$TransientArrayMap and clojure.lang.Associative are in unnamed module of loader 'app')"
    );
    // :kv-next-first — CONFORMANT (T1192 FX1): single-shot JVM throws the
    // full Long->Map$Entry text below (fx1-rest probe); "bare" was fast-throw
    // elision (fx1-fastthrow probe), not mirrored (N3).
    assert_eq!(
        uncaught(&mut eval, "(first (next (keys [1 2])))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    // :kv-rest-first — CONFORMANT (T1192 FX1): same as :kv-next-first
    // (fx1-rest + fx1-fastthrow probes).
    assert_eq!(
        uncaught(&mut eval, "(first (rest (keys [1 2])))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Map$Entry (java.lang.Long and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    // :keys-vecpairs — CONFORMANT (T1192 FX2): only real entries project.
    assert_eq!(
        uncaught(&mut eval, "(vec (keys [[1 2]]))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    // :keys-strpairs — CONFORMANT (T1192 FX1): single-shot JVM throws the
    // full String->Map$Entry text below (fx1-rest probe); "bare" was
    // fast-throw elision (fx1-fastthrow probe), not mirrored (N3).
    assert_eq!(
        uncaught(&mut eval, "(vec (keys [\"ab\"]))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.Map$Entry (java.lang.String and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    // :a-count — DIVERGENCE: JVM CompilerException (compile-time arity check); port runtime ArityException(0); JVM clojure.lang.Compiler$CompilerException (machine-local text)
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'count))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/count"
    );
    // :entry-class — CONFORMANT (T1192 FX2): real `MapEntry` values.
    assert_eq!(
        class_of(&mut eval, "(first {:a 1})"),
        "clojure.lang.MapEntry"
    );
    // :entry-vec
    assert_eq!(run_last(&mut eval, "(vector? (first {:a 1}))"), "true");
    // :entry-count
    assert_eq!(run_last(&mut eval, "(count (first {:a 1}))"), "2");
    // :entry-seq
    assert_eq!(run_last(&mut eval, "(seq? (first {:a 1}))"), "false");
    // :entry-counted
    assert_eq!(run_last(&mut eval, "(counted? (first {:a 1}))"), "true");
    // :entry-assoc
    assert_eq!(run_last(&mut eval, "(associative? (first {:a 1}))"), "true");
    // :entry-sequential
    assert_eq!(run_last(&mut eval, "(sequential? (first {:a 1}))"), "true");
    // :entry-coll
    assert_eq!(run_last(&mut eval, "(coll? (first {:a 1}))"), "true");
    // :entry-list
    assert_eq!(run_last(&mut eval, "(list? (first {:a 1}))"), "false");
    // :entry-eq
    assert_eq!(run_last(&mut eval, "(= (first {:a 1}) [:a 1])"), "true");
    // :entry-conj
    assert_eq!(run_last(&mut eval, "(conj (first {:a 1}) :x)"), "[:a 1 :x]");
    // :entry-call
    assert_eq!(run_last(&mut eval, "((first {:a 1}) 0)"), ":a");
    // :into-extra — T1200 D1: 3-arg `into` transduces now, and the
    // old DIVERGENCE is gone: vectors are `IFn`, so applying `[1]`
    // as the transducer looks up a fn key — IAE "Key must be
    // integer", JVM-verbatim.
    assert_eq!(
        uncaught(&mut eval, "(into [] [1] [2])"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
}

/// Corpus gaps (P2 map-class facets, invocation rows, keys/vals happy paths.)
#[test]
fn corpus_p2_classes_invocation() {
    let mut eval = boot();
    // :dissoc-bang-stays (class facet; JVM "class clojure.lang.PersistentHashMap")
    assert_eq!(
        class_of(
            &mut eval,
            "(persistent! (apply dissoc! (transient (into {} (map (fn [i] [i i]) (range 40)))) (range 38)))"
        ),
        "clojure.lang.PersistentHashMap"
    );
    // :map-zipmap-kw40 (class facet; JVM "class clojure.lang.PersistentArrayMap")
    assert_eq!(
        class_of(
            &mut eval,
            "(zipmap (map (fn [i] (keyword (str \"k\" i))) (range 40)) (range 40))"
        ),
        "clojure.lang.PersistentArrayMap"
    );
    // :kw-call
    assert_eq!(run_last(&mut eval, "(:a {:a 1})"), "1");
    // :kw-call-dflt
    assert_eq!(run_last(&mut eval, "(:b {:a 1} :dflt)"), ":dflt");
    // :kw-call-extra
    assert_eq!(
        uncaught(&mut eval, "(:a {:a 1} :d :extra)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: :a"
    );
    // :map-call
    assert_eq!(run_last(&mut eval, "({:a 1} :a)"), "1");
    // :map-call-extra
    assert_eq!(
        uncaught(&mut eval, "({:a 1} :a :b :c)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.lang.PersistentArrayMap"
    );
    // :vec-call
    assert_eq!(run_last(&mut eval, "([10 20] 1)"), "20");
    // :vec-call-extra
    assert_eq!(
        uncaught(&mut eval, "([10 20] 1 :extra)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.lang.PersistentVector"
    );
    // :set-call-hit
    assert_eq!(run_last(&mut eval, "(#{1 2} 2)"), "2");
    // :set-call-miss
    assert_eq!(run_last(&mut eval, "(#{1 2} 9)"), "nil");
    // :set-call-nf
    assert_eq!(run_last(&mut eval, "(#{1 2} 9 :nf)"), ":nf");
    // :set-call-extra
    assert_eq!(
        uncaught(&mut eval, "(#{1 2} 1 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.lang.PersistentHashSet"
    );
    // :keys-order
    assert_eq!(run_last(&mut eval, "(keys {:b 2 :a 1})"), "(:b :a)");
    // :keys-sorted
    assert_eq!(run_last(&mut eval, "(sort (keys {:b 2 :a 1}))"), "(:a :b)");
    // :vals-sorted
    assert_eq!(run_last(&mut eval, "(sort (vals {:b 2 :a 1}))"), "(1 2)");
    // :vals-map
    assert_eq!(run_last(&mut eval, "(vals {:a 1})"), "(1)");
    // :kv-ok
    assert_eq!(run_last(&mut eval, "(vec (keys {:a 1 :b 2}))"), "[:a :b]");
    // :kv-vals-ok
    assert_eq!(run_last(&mut eval, "(vec (vals {:a 1 :b 2}))"), "[1 2]");
}

/// Corpus gaps (P3 conj/into happy-path + error gaps (07).)
#[test]
fn corpus_conj_into_gaps() {
    let mut eval = boot();
    // :conj-num (also :conj-noncoll, same form)
    assert_eq!(
        uncaught(&mut eval, "(conj 5 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentCollection (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentCollection is in unnamed module of loader 'app')"
    );
    // :conj-vec-multi
    assert_eq!(run_last(&mut eval, "(conj [1] 2 3)"), "[1 2 3]");
    // :conj-list-multi
    assert_eq!(run_last(&mut eval, "(conj '(1) 2 3)"), "(3 2 1)");
    // :conj-set-dup
    assert_eq!(
        run_last(&mut eval, "[(set? (conj #{1} 1)) (sort (conj #{1} 1))]"),
        "[true (1)]"
    );
    // :conj-int-key
    assert_eq!(run_last(&mut eval, "(conj [1 2] 3)"), "[1 2 3]");
    // :c-conj-range
    assert_eq!(
        class_of(&mut eval, "(conj (range 5) 9)"),
        "clojure.lang.Cons"
    );
    assert_eq!(run_last(&mut eval, "(seq? (conj (range 5) 9))"), "true");
    assert_eq!(
        run_last(&mut eval, "(counted? (conj (range 5) 9))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(list? (conj (range 5) 9))"), "false");
    assert_eq!(
        run_last(&mut eval, "(sequential? (conj (range 5) 9))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(coll? (conj (range 5) 9))"), "true");
    // :c-conj-list
    assert_eq!(
        class_of(&mut eval, "(conj '(1 2) 0)"),
        "clojure.lang.PersistentList"
    );
    assert_eq!(run_last(&mut eval, "(seq? (conj '(1 2) 0))"), "true");
    assert_eq!(run_last(&mut eval, "(counted? (conj '(1 2) 0))"), "true");
    assert_eq!(run_last(&mut eval, "(list? (conj '(1 2) 0))"), "true");
    assert_eq!(run_last(&mut eval, "(sequential? (conj '(1 2) 0))"), "true");
    assert_eq!(run_last(&mut eval, "(coll? (conj '(1 2) 0))"), "true");
    // :c-conj-nil
    assert_eq!(
        class_of(&mut eval, "(conj nil 1)"),
        "clojure.lang.PersistentList"
    );
    assert_eq!(run_last(&mut eval, "(seq? (conj nil 1))"), "true");
    assert_eq!(run_last(&mut eval, "(counted? (conj nil 1))"), "true");
    assert_eq!(run_last(&mut eval, "(list? (conj nil 1))"), "true");
    assert_eq!(run_last(&mut eval, "(sequential? (conj nil 1))"), "true");
    assert_eq!(run_last(&mut eval, "(coll? (conj nil 1))"), "true");
    // :c-conj-lazy
    assert_eq!(
        class_of(&mut eval, "(conj (map inc [1 2]) 0)"),
        "clojure.lang.Cons"
    );
    assert_eq!(
        run_last(&mut eval, "(seq? (conj (map inc [1 2]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(counted? (conj (map inc [1 2]) 0))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(list? (conj (map inc [1 2]) 0))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(sequential? (conj (map inc [1 2]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(coll? (conj (map inc [1 2]) 0))"),
        "true"
    );
    // :cm-entry-elem
    assert_eq!(run_last(&mut eval, "(conj {} (first {:a 1}))"), "{:a 1}");
    // :conj-meta
    assert_eq!(
        run_last(&mut eval, "(meta (conj (with-meta [1] {:m 1}) 2))"),
        "{:m 1}"
    );
    // :into0
    assert_eq!(run_last(&mut eval, "(into [])"), "[]");
    // :into-nil-1
    assert_eq!(run_last(&mut eval, "(into nil)"), "nil");
    // :into-meta
    assert_eq!(
        run_last(&mut eval, "(meta (into (with-meta [] {:a 1}) [1]))"),
        "{:a 1}"
    );
    // :into-map-meta
    assert_eq!(
        run_last(&mut eval, "(meta (into (with-meta {:a 1} {:m 1}) {:b 2}))"),
        "{:m 1}"
    );
    // :into-list
    assert_eq!(run_last(&mut eval, "(into () [1 2])"), "(2 1)");
    // :into-str-coll
    assert_eq!(run_last(&mut eval, "(into [] \"abc\")"), "[\\a \\b \\c]");
    // :into-strempty
    assert_eq!(run_last(&mut eval, "(into [] \"\")"), "[]");
    // :into-str
    assert_eq!(
        uncaught(&mut eval, "(into \"ab\" \"cd\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentCollection (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentCollection is in unnamed module of loader 'app')"
    );
}

/// Corpus gaps (P3 assoc/dissoc gaps (07).)
#[test]
fn corpus_assoc_dissoc_gaps() {
    let mut eval = boot();
    // :assoc-nil-0
    assert_eq!(run_last(&mut eval, "(assoc nil 0 1)"), "{0 1}");
    // :assoc-nilnil
    assert_eq!(run_last(&mut eval, "(assoc nil nil 1)"), "{nil 1}");
    // :assoc-multi
    assert_eq!(run_last(&mut eval, "(assoc {} :a 1 :b 2)"), "{:a 1, :b 2}");
    // :assoc-2args — assoc--NNNN impl suffix stripped (cf. :assoc1); JVM clojure.lang.ArityException: Wrong number of args (2) passed to: clojure.core/assoc--5596
    assert_eq!(
        uncaught(&mut eval, "(assoc {} :a)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/assoc"
    );
    // :assoc-arities — assoc--NNNN impl suffix stripped (cf. :assoc1); JVM clojure.lang.ArityException: Wrong number of args (1) passed to: clojure.core/assoc--5596
    assert_eq!(
        uncaught(
            &mut eval,
            "(vector (assoc {:a 1}) (try (assoc {:a 1} :b) (catch Throwable t :throw)) (try (assoc {:a 1} :b 2 :c) (catch Throwable t :throw)))"
        ),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/assoc"
    );
    // :assoc-meta
    assert_eq!(
        run_last(&mut eval, "(meta (assoc (with-meta {:a 1} {:m 1}) :b 2))"),
        "{:m 1}"
    );
    // :assoc-map-meta-keep
    assert_eq!(run_last(&mut eval, "(meta (assoc {} :a 1))"), "nil");
    // :dissoc-nilnil
    assert_eq!(run_last(&mut eval, "(dissoc nil nil)"), "nil");
    // :dissoc-hit
    assert_eq!(run_last(&mut eval, "(dissoc {:a 1} :a)"), "{}");
    // :dissoc-multi
    assert_eq!(run_last(&mut eval, "(dissoc {:a 1 :b 2} :a :b :c)"), "{}");
    // :dissoc-0
    assert_eq!(
        uncaught(&mut eval, "(dissoc)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/dissoc"
    );
    // :dissoc-arities
    assert_eq!(
        run_last(
            &mut eval,
            "(vector (dissoc {:a 1}) (try (dissoc) (catch Throwable t :throw)))"
        ),
        "[{:a 1} :throw]"
    );
    // :dissoc-meta
    assert_eq!(
        run_last(
            &mut eval,
            "(meta (dissoc (with-meta {:a 1 :b 2} {:m 1}) :b))"
        ),
        "{:m 1}"
    );
}

/// Corpus gaps (P3 merge/merge-with/update gaps (07).)
#[test]
fn corpus_merge_update_gaps() {
    let mut eval = boot();
    // :merge-over
    assert_eq!(
        run_last(&mut eval, "(merge {:a 1} {:b 2} {:a 3})"),
        "{:a 3, :b 2}"
    );
    // :merge-meta
    assert_eq!(
        run_last(&mut eval, "(meta (merge (with-meta {:a 1} {:m 1}) {:b 2}))"),
        "{:m 1}"
    );
    // :merge-1
    assert_eq!(run_last(&mut eval, "(merge {:a 1})"), "{:a 1}");
    // :mergewith-rnil
    assert_eq!(run_last(&mut eval, "(merge-with + {:a 1} nil)"), "{:a 1}");
    // :mergewith-lnil
    assert_eq!(run_last(&mut eval, "(merge-with + nil {:a 1})"), "{:a 1}");
    // :mergewith-nilnil
    assert_eq!(run_last(&mut eval, "(merge-with + nil nil)"), "nil");
    // :mergewith3
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} {:a 2} {:a 3})"),
        "{:a 6}"
    );
    // :mergewith-b
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} {:a 2} {:b 3})"),
        "{:a 3, :b 3}"
    );
    // :mergewith-nilval
    assert_eq!(
        run_last(
            &mut eval,
            "(merge-with (fn [a b] [:f a b]) {:a nil} {:a 1})"
        ),
        "{:a [:f nil 1]}"
    );
    // :mergewith-miss
    assert_eq!(
        run_last(&mut eval, "(merge-with (fn [a b] [:f a b]) {} {:a 1})"),
        "{:a 1}"
    );
    // :update-miss (also :update-missing, same form)
    assert_eq!(
        run_last(&mut eval, "(update {} :a (fn [x] [:was x]))"),
        "{:a [:was nil]}"
    );
    // :update-nil
    assert_eq!(run_last(&mut eval, "(update nil :a (fn [x] 1))"), "{:a 1}");
    // :update-extra
    assert_eq!(run_last(&mut eval, "(update {:a 1} :a + 10 20)"), "{:a 31}");
    // :update-100
    assert_eq!(
        run_last(&mut eval, "(update {:a 1} :a + 10 100)"),
        "{:a 111}"
    );
    // :update-2
    assert_eq!(
        uncaught(&mut eval, "(update {:a 1} :a)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/update"
    );
    // :update-badcoll
    assert_eq!(
        uncaught(&mut eval, "(update 5 :a inc)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    // :update-inc-miss
    assert_eq!(
        uncaught(&mut eval, "(update {:a 1} :b inc)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    // :update-arities
    assert_eq!(
        uncaught(&mut eval, "(update {:a 1})"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/update"
    );
    // :update-0
    assert_eq!(
        uncaught(&mut eval, "(update)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/update"
    );
}

/// Corpus gaps (P3 update-in/assoc-in/get-in gaps (07).)
#[test]
fn corpus_in_gaps() {
    let mut eval = boot();
    // :updatein-nest
    assert_eq!(
        run_last(&mut eval, "(update-in {:a {:b 1}} [:a :b] inc)"),
        "{:a {:b 2}}"
    );
    // :updatein-nil
    assert_eq!(
        run_last(&mut eval, "(update-in nil [:a] (fn [x] 1))"),
        "{:a 1}"
    );
    // :updatein-emptyks
    assert_eq!(
        run_last(&mut eval, "(update-in {} [] (fn [x] 1))"),
        "{nil 1}"
    );
    // :updatein-midmiss
    assert_eq!(
        run_last(&mut eval, "(update-in {} [:a :b] (fn [x] 1))"),
        "{:a {:b 1}}"
    );
    // :updatein-vec
    assert_eq!(run_last(&mut eval, "(update-in [1 2] [1] inc)"), "[1 3]");
    // :update-in-empty
    assert_eq!(
        uncaught(&mut eval, "(update-in {:a 1} [] inc)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    // :update-in-vec0
    assert_eq!(run_last(&mut eval, "(update-in [1 2] [0] inc)"), "[2 2]");
    // :update-in-nil-mid
    assert_eq!(
        run_last(&mut eval, "(update-in {:a nil} [:a :b] (fn [x] 1))"),
        "{:a {:b 1}}"
    );
    // :update-in-fnil
    assert_eq!(
        run_last(&mut eval, "(update-in {} [:x :y] (fnil inc 0))"),
        "{:x {:y 1}}"
    );
    // :update-in-plus
    assert_eq!(
        run_last(&mut eval, "(update-in {:a {:b 1}} [:a :b] + 10)"),
        "{:a {:b 11}}"
    );
    // :associn-nil
    assert_eq!(
        run_last(&mut eval, "(assoc-in nil [:a :b] 1)"),
        "{:a {:b 1}}"
    );
    // :associn-basic
    assert_eq!(
        run_last(&mut eval, "(assoc-in {:a {}} [:a :b] 1)"),
        "{:a {:b 1}}"
    );
    // :assoc-in-vec
    assert_eq!(
        run_last(&mut eval, "(assoc-in {:a [1 2]} [:a 0] 9)"),
        "{:a [9 2]}"
    );
    // :assoc-in-empty-path
    assert_eq!(
        run_last(&mut eval, "(assoc-in {:a 1} [] 2)"),
        "{:a 1, nil 2}"
    );
    // :assoc-in-replace
    assert_eq!(run_last(&mut eval, "(assoc-in {:a 1} [:a] 2)"), "{:a 2}");
    // :assoc-in-0
    assert_eq!(
        uncaught(&mut eval, "(assoc-in)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/assoc-in"
    );
    // :getin-leafnil
    assert_eq!(run_last(&mut eval, "(get-in {:a nil} [:a] :nf)"), "nil");
    // :getin-nilks
    assert_eq!(run_last(&mut eval, "(get-in {:a 1} nil)"), "{:a 1}");
    // :getin-nilks-nf
    assert_eq!(run_last(&mut eval, "(get-in nil nil :nf)"), "nil");
    // :getin-miss
    assert_eq!(run_last(&mut eval, "(get-in {:a 1} [:b] :nf)"), ":nf");
    // :getin-badks
    assert_eq!(
        uncaught(&mut eval, "(get-in {:a 1} 5 :nf)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // :getin-vec
    assert_eq!(run_last(&mut eval, "(get-in {:a [1 2]} [:a 1])"), "2");
    // :getin-vec-oob
    assert_eq!(run_last(&mut eval, "(get-in [1 2] [5])"), "nil");
    // :getin-nil
    assert_eq!(run_last(&mut eval, "(get-in nil [:a :b])"), "nil");
    // :getin-empty-dflt
    assert_eq!(run_last(&mut eval, "(get-in {:a 1} [] :d)"), "{:a 1}");
    // :getin-tmap
    assert_eq!(run_last(&mut eval, "(get-in (transient {:a 1}) [:a])"), "1");
    // :getin-0
    assert_eq!(
        uncaught(&mut eval, "(get-in)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/get-in"
    );
}

/// Corpus gaps (P3 select-keys/zipmap/replace gaps (07).)
#[test]
fn corpus_select_zipmap_replace_gaps() {
    let mut eval = boot();
    // :selectkeys-order
    assert_eq!(
        run_last(&mut eval, "(select-keys {:a 1 :b 2} [:b :a])"),
        "{:b 2, :a 1}"
    );
    // :selectkeys-ab
    assert_eq!(
        run_last(&mut eval, "(select-keys {:b 2 :a 1} [:a :b])"),
        "{:a 1, :b 2}"
    );
    // :selectkeys-nilval
    assert_eq!(
        run_last(&mut eval, "(select-keys {:a nil} [:a :b])"),
        "{:a nil}"
    );
    // :selectkeys-badks
    assert_eq!(
        uncaught(&mut eval, "(select-keys {:a 1} 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // :sel-nilks
    assert_eq!(run_last(&mut eval, "(select-keys {:a 1} nil)"), "{}");
    // :sel-nilnil
    assert_eq!(run_last(&mut eval, "(select-keys nil nil)"), "{}");
    // :selectkeys-0
    assert_eq!(
        uncaught(&mut eval, "(select-keys)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/select-keys"
    );
    // :zipmap-basic
    assert_eq!(run_last(&mut eval, "(zipmap [1 2] [3 4])"), "{1 3, 2 4}");
    // :zipmap-order
    assert_eq!(
        run_last(&mut eval, "(zipmap [:b :a] [1 2])"),
        "{:b 1, :a 2}"
    );
    // :replace-vec
    assert_eq!(run_last(&mut eval, "(replace {:a :b} [:a :c])"), "[:b :c]");
    // :replace-seq
    assert_eq!(run_last(&mut eval, "(replace {:a :b} '(:a :c))"), "(:b :c)");
    // :replace-str
    assert_eq!(
        run_last(&mut eval, "(replace {\\a \\b} \"ac\")"),
        "(\\b \\c)"
    );
    // :replace-empty
    assert_eq!(run_last(&mut eval, "(replace {} [1])"), "[1]");
    // :replace-nil
    assert_eq!(run_last(&mut eval, "(replace {:a 1} nil)"), "()");
    // :replace-nilnil
    assert_eq!(run_last(&mut eval, "(replace nil nil)"), "()");
    // :replace-nums
    assert_eq!(run_last(&mut eval, "(replace {1 :a} [1 2 1])"), "[:a 2 :a]");
}

/// Corpus gaps (P3 vec/vector/list/list*/hash-map/hash-set gaps (07).)
#[test]
fn corpus_ctor_gaps() {
    let mut eval = boot();
    // :vec-2args
    assert_eq!(
        uncaught(&mut eval, "(vec [1] [2])"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/vec"
    );
    // :vector-0
    assert_eq!(run_last(&mut eval, "(vector)"), "[]");
    // :vector-2
    assert_eq!(run_last(&mut eval, "(vector 1 2)"), "[1 2]");
    // :list-0
    assert_eq!(run_last(&mut eval, "(list)"), "()");
    // :liststar-1234
    assert_eq!(run_last(&mut eval, "(list* 1 2 [3 4])"), "(1 2 3 4)");
    // :liststar-123
    assert_eq!(
        uncaught(&mut eval, "(pr-str (list* 1 2 3))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // :liststar-nest
    assert_eq!(run_last(&mut eval, "(list* 1 2 nil)"), "(1 2)");
    // :apply-liststar
    assert_eq!(run_last(&mut eval, "(apply list* [1 [2 3]])"), "(1 2 3)");
    // :hashmap-dup (also :hash-map-dup, same form)
    assert_eq!(run_last(&mut eval, "(hash-map :a 1 :a 2)"), "{:a 2}");
    // :hashmap-2 — order-normalized per s5d-pins corpus rule (JVM prints {:b 2, :a 1}; order unpinned)
    assert_eq!(
        run_last(&mut eval, "(= (hash-map :a 1 :b 2) {:a 1 :b 2})"),
        "true"
    );
    // :hashmap-1
    assert_eq!(run_last(&mut eval, "(hash-map :a 1)"), "{:a 1}");
    // :hashset-112
    assert_eq!(
        run_last(
            &mut eval,
            "[(set? (hash-set 1 1 2)) (sort (hash-set 1 1 2))]"
        ),
        "[true (1 2)]"
    );
    // :hashset-0
    assert_eq!(
        run_last(&mut eval, "[(set? (hash-set)) (sort (hash-set))]"),
        "[true ()]"
    );
}

/// Corpus gaps (P3 nth/get/contains?/find + nthrest/count gaps (09).)
#[test]
fn corpus_nth_get_find_gaps() {
    let mut eval = boot();
    // :nth-list-ab
    assert_eq!(run_last(&mut eval, "(nth '(a b) 0)"), "a");
    // :nth-cons
    assert_eq!(run_last(&mut eval, "(nth (cons 1 [2 3]) 2)"), "3");
    // :nth-entry
    assert_eq!(run_last(&mut eval, "(nth (first {:a 1}) 0)"), ":a");
    // :nth-entry1
    assert_eq!(run_last(&mut eval, "(nth (first {:a 1}) 1)"), "1");
    // :nth-map-entry-vec
    assert_eq!(run_last(&mut eval, "(nth [:a 1] 1)"), "1");
    // :nth-range
    assert_eq!(run_last(&mut eval, "(nth (range 10) 3)"), "3");
    // :nth-range5
    assert_eq!(run_last(&mut eval, "(nth (range 10) 5)"), "5");
    // :nth-chunked
    assert_eq!(run_last(&mut eval, "(nth (seq [1 2 3]) 2)"), "3");
    // :nth-strseq
    assert_eq!(run_last(&mut eval, "(nth (seq \"ab\") 1)"), "\\b");
    // :nth-repeat
    assert_eq!(run_last(&mut eval, "(nth (repeat 5 :x) 4 :nf)"), ":x");
    // :nth-cycle
    assert_eq!(run_last(&mut eval, "(nth (cycle [1 2]) 3)"), "2");
    // :nth-iterate
    assert_eq!(run_last(&mut eval, "(nth (iterate inc 10) 2)"), "12");
    // :nth-sort
    assert_eq!(run_last(&mut eval, "(nth (sort [3 1 2]) 1)"), "2");
    // :nth-range-neg
    assert_eq!(
        uncaught(&mut eval, "(nth (range 10) -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // :nth-1
    assert_eq!(
        uncaught(&mut eval, "(nth [1])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/nth"
    );
    // :nth-0
    assert_eq!(
        uncaught(&mut eval, "(nth)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/nth"
    );
    // :get-str
    assert_eq!(run_last(&mut eval, "(get \"abc\" 0)"), "\\a");
    // :get-str-dbl
    assert_eq!(run_last(&mut eval, "(get \"abc\" 1.5)"), "\\b");
    // :get-set-miss
    assert_eq!(run_last(&mut eval, "(get #{1} 2 :nf)"), ":nf");
    // :get-list
    assert_eq!(run_last(&mut eval, "(get '(1 2) 0)"), "nil");
    // :get-map-miss-nf
    assert_eq!(run_last(&mut eval, "(get {:a 1} :b :d)"), ":d");
    // :get-nil-nf
    assert_eq!(run_last(&mut eval, "(get nil :a :nf)"), ":nf");
    // :get-nilnil
    assert_eq!(run_last(&mut eval, "(get nil nil)"), "nil");
    // :get-map-nilk
    assert_eq!(run_last(&mut eval, "(get {:a 1} nil)"), "nil");
    // :get-num
    assert_eq!(run_last(&mut eval, "(get 5 :k)"), "nil");
    // :get-range
    assert_eq!(run_last(&mut eval, "(get (range 10) 5)"), "nil");
    // :get-1
    assert_eq!(
        uncaught(&mut eval, "(get [1])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/get"
    );
    // :contains-str-dbl
    assert_eq!(run_last(&mut eval, "(contains? \"abc\" 1.5)"), "true");
    // :contains-vec-neg
    assert_eq!(run_last(&mut eval, "(contains? [1] -1)"), "false");
    // :contains-nilnil
    assert_eq!(run_last(&mut eval, "(contains? nil nil)"), "false");
    // :contains-nilval
    assert_eq!(run_last(&mut eval, "(contains? {:a nil} :a)"), "true");
    // :contains-0
    assert_eq!(
        uncaught(&mut eval, "(contains?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/contains?"
    );
    // :find-vec-nil
    assert_eq!(run_last(&mut eval, "(find [1 2] nil)"), "nil");
    // :nthrest-1
    assert_eq!(run_last(&mut eval, "(eval '(nthrest [1 2 3] 1))"), "(2 3)");
    // :nthrest-basic
    assert_eq!(run_last(&mut eval, "(nthrest [1 2 3] 1)"), "(2 3)");
    // :nthrest-over
    assert_eq!(run_last(&mut eval, "(nthrest [1 2] 5)"), "()");
    // :count-lazy
    assert_eq!(run_last(&mut eval, "(count (map inc [1 2 3]))"), "3");
}

/// Corpus gaps (P3 peek/pop/subvec/empty?/not-empty gaps (09).)
#[test]
fn corpus_peek_pop_subvec_empty_gaps() {
    let mut eval = boot();
    // :empty-map
    assert_eq!(run_last(&mut eval, "(empty? {})"), "true");
    // :notempty-str
    assert_eq!(run_last(&mut eval, "(not-empty \"\")"), "nil");
    // :notempty-strx
    assert_eq!(run_last(&mut eval, "(not-empty \"x\")"), "\"x\"");
    // :peek-num
    assert_eq!(
        uncaught(&mut eval, "(peek 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentStack (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentStack is in unnamed module of loader 'app')"
    );
    // :peek-vec-empty
    assert_eq!(run_last(&mut eval, "(peek [])"), "nil");
    // :peek-list-empty
    assert_eq!(run_last(&mut eval, "(peek '())"), "nil");
    // :pop-vec1
    assert_eq!(run_last(&mut eval, "(pop [1])"), "[]");
    // :pop-list1
    assert_eq!(run_last(&mut eval, "(pop '(1))"), "()");
    // :subvec-oob
    assert_eq!(
        uncaught(&mut eval, "(subvec [1] 2)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // :subvec-neg
    assert_eq!(
        uncaught(&mut eval, "(subvec [1] -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // :subvec-neg3
    assert_eq!(
        uncaught(&mut eval, "(subvec [1 2 3] -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // :subvec-start-oob
    assert_eq!(
        uncaught(&mut eval, "(subvec [1 2 3] 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // :subvec-nil-vec — DIVERGENCE: JVM NPE v.count(); port CCE null->IPersistentVector (count-first, nil counts 0); JVM java.lang.NullPointerException: Cannot invoke "clojure.lang.IPersistentVector.count()" because "v" is null
    assert_eq!(
        uncaught(&mut eval, "(subvec nil 0)"),
        "java.lang.ClassCastException | class null cannot be cast to class clojure.lang.IPersistentVector (null and clojure.lang.IPersistentVector are in unnamed module of loader 'app')"
    );
}

/// Corpus gaps (P4 arity-0 probes: (eval (list 'F)) (07/09).)
#[test]
fn corpus_arity0_gaps() {
    let mut eval = boot();
    // :a-assoc — assoc--NNNN impl suffix stripped (cf. :assoc1); JVM clojure.lang.ArityException: Wrong number of args (0) passed to: clojure.core/assoc--5596
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'assoc))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/assoc"
    );
    // :a-dissoc
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'dissoc))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/dissoc"
    );
    // :a-mergewith
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'merge-with))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/merge-with"
    );
    // :a-updatein
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'update-in))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/update-in"
    );
    // :a-update
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'update))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/update"
    );
    // :a-associn
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'assoc-in))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/assoc-in"
    );
    // :a-getin
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'get-in))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/get-in"
    );
    // :a-selectkeys
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'select-keys))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/select-keys"
    );
    // :a-zipmap
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'zipmap))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/zipmap"
    );
    // :a-replace
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'replace))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/replace"
    );
    // :a-vec
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'vec))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/vec"
    );
    // :a-list
    assert_eq!(run_last(&mut eval, "(eval (list 'list))"), "()");
    // :a-vector
    assert_eq!(run_last(&mut eval, "(eval (list 'vector))"), "[]");
    // :a-hashset
    assert_eq!(run_last(&mut eval, "(eval (list 'hash-set))"), "#{}");
    // :a-nth
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'nth))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/nth"
    );
    // :a-get
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'get))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/get"
    );
    // :a-contains
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'contains?))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/contains?"
    );
    // :a-find
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'find))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/find"
    );
    // :a-keys
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'keys))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/keys"
    );
    // :a-vals
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'vals))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/vals"
    );
    // :a-emptyq
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'empty?))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/empty?"
    );
    // :a-notempty
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'not-empty))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/not-empty"
    );
    // :a-pop
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'pop))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/pop"
    );
    // :a-peek
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'peek))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/peek"
    );
    // :a-subvec
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'subvec))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/subvec"
    );
}

#[test]
fn disj_set_rows() {
    let mut eval = boot();
    // `:disj-*` (corpus 09): 1-arg passes through, nil-puns,
    // missing members are a no-op.
    assert_eq!(run_last(&mut eval, "(disj #{1} 1)"), "#{}");
    assert_eq!(run_last(&mut eval, "(disj #{1})"), "#{1}");
    assert_eq!(run_last(&mut eval, "(sort (disj #{1 2} 1))"), "(2)");
    assert_eq!(run_last(&mut eval, "(sort (disj #{1} 9))"), "(1)");
    assert_eq!(run_last(&mut eval, "(disj nil 1)"), "nil");
    assert_eq!(run_last(&mut eval, "(disj nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(sort (disj #{1 2 3} 1 2))"), "(3)");
    assert_eq!(
        uncaught(&mut eval, "(disj [1] 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.IPersistentSet (clojure.lang.PersistentVector and clojure.lang.IPersistentSet are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'disj))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/disj"
    );
    // `:set-*` (corpus 07/09): rebuilds through `seq`.
    assert_eq!(run_last(&mut eval, "(sort (set [1 1 2]))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(set nil)"), "#{}");
    assert_eq!(run_last(&mut eval, "(sort (set \"ab\"))"), "(\\a \\b)");
    assert_eq!(
        run_last(&mut eval, "(meta (set (with-meta [1] {:a 1})))"),
        "nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(set 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn gap2_transient_get_edge_rows() {
    let mut eval = boot();
    // A negative transient-vector index reads missing (coll.rs:124).
    assert_eq!(run_last(&mut eval, "(get (transient [1 2]) -1)"), "nil");
    // Transient sets read hit-or-miss (coll.rs:129-133).
    assert_eq!(run_last(&mut eval, "(get (transient #{1 2}) 1)"), "1");
    assert_eq!(run_last(&mut eval, "(get (transient #{1 2}) 9)"), "nil");
}

#[test]
fn gap2_find_transient_subvec_rows() {
    let mut eval = boot();
    // Transient maps/vectors read entries (coll.rs:162-176).
    assert_eq!(
        run_last(&mut eval, "(find (transient {:a 1}) :a)"),
        "[:a 1]"
    );
    assert_eq!(
        run_last(&mut eval, "(find (transient [10 20]) 1)"),
        "[1 20]"
    );
    assert_eq!(run_last(&mut eval, "(find (transient [10 20]) :a)"), "nil");
    assert_eq!(run_last(&mut eval, "(find (transient [10 20]) -1)"), "nil");
    // Subvectors read entries too (coll.rs:188-192).
    assert_eq!(
        run_last(&mut eval, "(find (subvec [10 20 30] 1) 0)"),
        "[0 20]"
    );
}

#[test]
fn gap2_map_conj_subvec_rows() {
    let mut eval = boot();
    // A 2-subvector conjs as a pair; any other length is the pair error.
    assert_eq!(
        run_last(&mut eval, "(get (conj {:a 1} (subvec [10 20 30] 0 2)) 10)"),
        "20"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} (subvec [10 20 30] 0 1))"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
}

#[test]
fn gap2_subvector_conj_clobber() {
    let mut eval = boot();
    // Conj inside the backing window clobbers the next slot (coll.rs:286-288).
    assert_eq!(
        run_last(&mut eval, "(conj (subvec [1 2 3] 0 2) 9)"),
        "[1 2 9]"
    );
}

#[test]
fn gap2_assoc_edge_rows() {
    let mut eval = boot();
    // Past-the-end and non-integer keys (coll.rs:329, 341-344, 349-352).
    assert_eq!(
        uncaught(&mut eval, "(assoc [1 2] 5 :x)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (subvec [1 2] 0) :a 1)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (subvec [1 2 3] 1) 5 :x)"),
        "java.lang.IndexOutOfBoundsException | "
    );
}

#[test]
fn gap2_count_transient_kind_rows() {
    let mut eval = boot();
    // Map/set buffers count like vectors (coll.rs:416-417).
    assert_eq!(run_last(&mut eval, "(count (transient {:a 1 :b 2}))"), "2");
    assert_eq!(run_last(&mut eval, "(count (transient #{1 2 3}))"), "3");
}

#[test]
fn gap2_merge_with_tail_rows() {
    let mut eval = boot();
    // Subvector right-hand sides fold index pairs (coll.rs:543-547).
    assert_eq!(
        run_last(
            &mut eval,
            "(get (merge-with + {:a 1} (subvec [10 20] 0)) 1)"
        ),
        "20"
    );
    // An empty sequential right-hand side keeps the left map (coll.rs:569).
    assert_eq!(run_last(&mut eval, "(merge-with + {:a 1} '())"), "{:a 1}");
}

#[test]
fn gap2_contains_subvec_rows() {
    let mut eval = boot();
    // Integer keys probe the window (coll.rs:861-862).
    assert_eq!(
        run_last(&mut eval, "(contains? (subvec [1 2 3] 1) 0)"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (subvec [1 2 3] 1) 9)"),
        "false"
    );
}

#[test]
fn gap2_nth_edge_rows() {
    let mut eval = boot();
    // Negative transient indexes throw; negative + not-found returns it.
    assert_eq!(
        uncaught(&mut eval, "(nth (transient [1 2]) -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(run_last(&mut eval, "(nth [1 2] -1 :nf)"), ":nf");
}

#[test]
fn gap2_bang_nil_foreign_rows() {
    let mut eval = boot();
    // Nil and non-transient bang targets (coll.rs:1230-1234, 1327, 1393-1397, 1426-1433).
    assert_eq!(
        uncaught(&mut eval, "(persistent! nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientCollection.persistent()\" because \"coll\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj! [1] 2)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.ITransientCollection (clojure.lang.PersistentVector and clojure.lang.ITransientCollection are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(dissoc! nil :a)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientMap.without(Object)\" because \"map\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop! nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.ITransientVector.pop()\" because \"coll\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop! [1])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.ITransientVector (clojure.lang.PersistentVector and clojure.lang.ITransientVector are in unnamed module of loader 'app')"
    );
}

#[test]
fn gap2_conj_bang_map_rows() {
    let mut eval = boot();
    // Nil passes through; maps merge entry-wise (coll.rs:1264, 1268-1270).
    assert_eq!(
        run_last(&mut eval, "(persistent! (conj! (transient {:a 1}) nil))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(get (persistent! (conj! (transient {:a 1}) {:b 2})) :b)"
        ),
        "2"
    );
}

#[test]
fn gap2_assoc_bang_extra_rows() {
    let mut eval = boot();
    // Extra triples fold; an odd tail assocs nil (coll.rs:1351-1353, 1360-1361, 1384).
    assert_eq!(
        run_last(
            &mut eval,
            "(persistent! (assoc! (transient [1 2 3]) 0 9 1 8))"
        ),
        "[9 8 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(persistent! (assoc! (transient [1 2 3]) 0 9 1))"
        ),
        "[9 nil 3]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(get (persistent! (assoc! (transient {:a 1}) :b 2 :c 3)) :c)"
        ),
        "3"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc! (transient [1 2]) -1 :x)"),
        "java.lang.IndexOutOfBoundsException | "
    );
}

#[test]
fn gap2_distinct_first_pair() {
    let mut eval = boot();
    // Equal first two short-circuit (coll.rs:1610).
    assert_eq!(run_last(&mut eval, "(distinct? 1 1 2)"), "false");
}

#[test]
fn strict_seq_class_rows() {
    let mut eval = boot();
    // Each producer reports the JVM class for the value it builds
    // (covers the matching `class_name`/`jvm_name` arms in seq.rs).
    assert_eq!(
        class_of(&mut eval, "(seq [1 2])"),
        "clojure.lang.PersistentVector$ChunkedSeq"
    );
    assert_eq!(class_of(&mut eval, "(sort [2 1])"), "clojure.lang.ArraySeq");
    assert_eq!(
        class_of(&mut eval, "(seq \"ab\")"),
        "clojure.lang.StringSeq"
    );
    assert_eq!(
        class_of(&mut eval, "(seq {:a 1})"),
        "clojure.lang.PersistentArrayMap$Seq"
    );
    assert_eq!(
        class_of(&mut eval, "(seq #{1})"),
        "clojure.lang.APersistentMap$KeySeq"
    );
    assert_eq!(class_of(&mut eval, "(repeat 1)"), "clojure.lang.Repeat");
    assert_eq!(class_of(&mut eval, "(cycle [1])"), "clojure.lang.Cycle");
    assert_eq!(
        class_of(&mut eval, "(iterate inc 0)"),
        "clojure.lang.Iterate"
    );
    assert_eq!(class_of(&mut eval, "(range 1.5)"), "clojure.lang.Range");
    assert_eq!(
        class_of(&mut eval, "(keys {:a 1})"),
        "clojure.lang.APersistentMap$KeySeq"
    );
    assert_eq!(
        class_of(&mut eval, "(vals {:a 1})"),
        "clojure.lang.APersistentMap$ValSeq"
    );
}

#[test]
fn subvec_invoke_out_of_bounds() {
    let mut eval = boot();
    // OOB invoke is the empty-message IOOBE.
    assert_eq!(
        uncaught(&mut eval, "((subvec [1 2 3] 0 2) 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
}

/// Issue #31 (F-sorted, red first): sorted collections. Oracle
/// `docs/i31-sorted/probes/sorted.out` (alpha10 jars).
#[test]
fn sorted_construction_rows() {
    let mut eval = boot();
    // Constructors order by key/comparator.
    assert_eq!(
        run_last(&mut eval, "(sorted-map :b 2 :a 1)"),
        "{:a 1, :b 2}"
    );
    assert_eq!(run_last(&mut eval, "(sorted-set 3 1 2)"), "#{1 2 3}");
    assert_eq!(run_last(&mut eval, "(sorted-set-by > 3 1 2)"), "#{3 2 1}");
    assert_eq!(run_last(&mut eval, "(sorted-set :b :a)"), "#{:a :b}");
    // Duplicate (compare-0) keys collapse like repeated assoc.
    assert_eq!(run_last(&mut eval, "(sorted-map :a 1 :a 2)"), "{:a 2}");
    // Odd arg count is the JVM IAE.
    assert_eq!(
        uncaught(&mut eval, "(sorted-map :a)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :a"
    );
    // A throwing comparator surfaces the call's own error (the
    // port's `>` raises the full JVM CCE text verbatim).
    assert_eq!(
        uncaught(&mut eval, "(sorted-map-by > :b 2 :a 1)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(run_last(&mut eval, "(count (sorted-map :a 1 :b 2))"), "2");
    assert_eq!(
        run_last(&mut eval, "(keys (sorted-map :b 2 :a 1))"),
        "(:a :b)"
    );
    assert_eq!(
        run_last(&mut eval, "(vals (sorted-map :b 2 :a 1))"),
        "(1 2)"
    );
}

/// Issue #31 (F-sorted, red first): sortedness is sticky and
/// observable. Oracle `docs/i31-sorted/probes/sorted.out`.
#[test]
fn sorted_sticky_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(assoc (sorted-map :a 1) :b 2)"),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(&mut eval, "(dissoc (sorted-map :a 1 :b 2) :a)"),
        "{:b 2}"
    );
    assert_eq!(run_last(&mut eval, "(conj (sorted-set 1) 2 1)"), "#{1 2}");
    assert_eq!(run_last(&mut eval, "(disj (sorted-set 1 2) 1)"), "#{2}");
    assert_eq!(run_last(&mut eval, "(sorted? (sorted-map :a 1))"), "true");
    assert_eq!(run_last(&mut eval, "(sorted? {:a 1})"), "false");
    assert_eq!(
        run_last(&mut eval, "(sorted? (empty (sorted-map :a 1)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(reversible? (sorted-map :a 1))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(rseq (sorted-map :a 1 :b 2))"),
        "([:b 2] [:a 1])"
    );
    assert_eq!(run_last(&mut eval, "(rseq (sorted-set 1 2))"), "(2 1)");
    // Content equality (and hash) with plain collections.
    assert_eq!(run_last(&mut eval, "(= (sorted-map :a 1) {:a 1})"), "true");
    assert_eq!(run_last(&mut eval, "(= (sorted-set 1 2) #{1 2})"), "true");
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (sorted-map :a 1) {:x 1}))"),
        "{:x 1}"
    );
    // `nth` is unsupported on sorted colls, like the JVM.
    assert_eq!(
        uncaught(&mut eval, "(nth (sorted-map :a 1 :b 2) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: PersistentTreeMap"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (sorted-set 1 2) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: PersistentTreeSet"
    );
    // Sorted maps are not editable (probe `trans-sorted`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(persistent! (assoc! (transient (sorted-map :a 1)) :b 2))"
        ),
        "java.lang.ClassCastException | class clojure.lang.PersistentTreeMap cannot be cast to class clojure.lang.IEditableCollection (clojure.lang.PersistentTreeMap and clojure.lang.IEditableCollection are in unnamed module of loader 'app')"
    );
    // A compare-0 assoc replaces the value but keeps the original
    // key object (probe `sorted-keyobj`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (sorted-map-by (fn [a b] (compare (:id a) (:id b))) {:id 1} :a)] (keys (assoc m {:id 1 :x 0} :b)))"
        ),
        "({:id 1})"
    );
    // Read/update paths route through the comparator (probes
    // `s-get`, `s-get-miss`, `s-contains`, `s-find`, `s-update`,
    // `s-merge`, `s-conj-map`, `s-into`).
    assert_eq!(
        run_last(&mut eval, "(conj (sorted-map :b 2 :a 1) [:c 3])"),
        "{:a 1, :b 2, :c 3}"
    );
    assert_eq!(run_last(&mut eval, "(get (sorted-map :a 1) :a)"), "1");
    assert_eq!(run_last(&mut eval, "(get (sorted-map :a 1) :z)"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "[(contains? (sorted-map :a 1) :a) (contains? (sorted-map :a 1) :z)]"
        ),
        "[true false]"
    );
    assert_eq!(run_last(&mut eval, "(find (sorted-map :a 1) :a)"), "[:a 1]");
    assert_eq!(
        run_last(&mut eval, "(update (sorted-map :a 1) :a inc)"),
        "{:a 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(merge (sorted-map :a 1) {:b 2} (sorted-map :c 3))"
        ),
        "{:a 1, :b 2, :c 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(into {} (sorted-map :b 2 :a 1))"),
        "{:a 1, :b 2}"
    );
    // Empty sets stay sorted; `rseq` of empty is nil (probes
    // `s-empty-set`, `s-rseq-empty`).
    assert_eq!(
        run_last(&mut eval, "(sorted? (empty (sorted-set 1)))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(rseq (sorted-set))"), "nil");
    // Bounds under a custom comparator (probe `s-subseq-by`).
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-set-by > 3 1 2) >= 2)"),
        "(2 1)"
    );
}

/// Issue #31 (F-sorted, red first): `subseq`/`rsubseq` bound
/// walks. Oracle `docs/i31-sorted/probes/sorted.out`.
#[test]
fn sorted_subseq_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1 :b 2 :c 3) >= :b)"),
        "([:b 2] [:c 3])"
    );
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1 :b 2 :c 3) > :b)"),
        "([:c 3])"
    );
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-map :a 1 :b 2 :c 3) <= :b)"),
        "([:b 2] [:a 1])"
    );
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-set 1 2 3) >= 2)"),
        "(3 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-set 1 2 3) < 3)"),
        "(1 2)"
    );
    // Start+end bounds, both directions.
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1 :b 2 :c 3) > :a < :c)"),
        "([:b 2])"
    );
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-map :a 1 :b 2 :c 3) > :a < :c)"),
        "([:b 2])"
    );
    // A non-start test scans from the head, so `=` matches nothing.
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1 :b 2) = :b)"),
        "()"
    );
    // Past the end is `nil`.
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1) >= :z)"),
        "nil"
    );
    // Unsorted sources take the `Sorted` cast (probe
    // `subseq-unsorted`, full JVM text verbatim).
    assert_eq!(
        uncaught(&mut eval, "(subseq [] >= 1)"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.Sorted (clojure.lang.PersistentVector and clojure.lang.Sorted are in unnamed module of loader 'app')"
    );
}

/// Issue #31 (F-sorted): patch-gate pins for the sorted `conj`
/// nil/entries arms, transient-set rejection, `dissoc`/`disj`
/// misses, set `contains?`/`get`, the `subseq` drop-to-nil and
/// `rsubseq` descending/empty/nil paths, and non-symbol bound
/// tests. Oracles `docs/i31-sorted/probes/sorted.out` (`conj-map-nil`,
/// `conj-map-map`, `dissoc-miss`, `disj-miss`, `subseq-drop`,
/// `subseq5e`, `rsubseq-empty`, `rsubseq-5b`, `rsubseq-below`,
/// `rsubseq-5set`, `rsubseq-5keep`, `trans-set`, `set-find`,
/// `conj-nil`) and `/tmp/sorted9.clj` (`set-get`, `set-contains`).
#[test]
fn sorted_gap_rows() {
    let mut eval = boot();
    // `conj` nil onto a sorted map is a no-op (probe
    // `conj-map-nil`).
    assert_eq!(
        run_last(&mut eval, "(conj (sorted-map :a 1) nil)"),
        "{:a 1}"
    );
    // `conj` a map onto a sorted map merges entry-wise (probe
    // `conj-map-map`).
    assert_eq!(
        run_last(&mut eval, "(conj (sorted-map :a 1) {:b 2})"),
        "{:a 1, :b 2}"
    );
    // `conj` nil onto a sorted set stores nil first (probe
    // `conj-nil`).
    assert_eq!(
        run_last(&mut eval, "(= (conj (sorted-set 1 2) nil) #{1 2 nil})"),
        "true"
    );
    // Sorted sets are not editable either (probe `trans-set`, full
    // JVM text verbatim).
    assert_eq!(
        uncaught(&mut eval, "(transient (sorted-set 1))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentTreeSet cannot be cast to class clojure.lang.IEditableCollection (clojure.lang.PersistentTreeSet and clojure.lang.IEditableCollection are in unnamed module of loader 'app')"
    );
    // Missing keys leave sorted collections alone (probes
    // `dissoc-miss`, `disj-miss`).
    assert_eq!(
        run_last(&mut eval, "(dissoc (sorted-map :a 1) :z)"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(= (disj (sorted-set 1 2) 9) #{1 2})"),
        "true"
    );
    // Set `contains?`/`get` route through the comparator (probe
    // `set-contains`, `set-get`).
    assert_eq!(
        run_last(
            &mut eval,
            "[(contains? (sorted-set 1 2) 2) (contains? (sorted-set 1 2) 9)]"
        ),
        "[true false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(get (sorted-set 1 2) 2) (get (sorted-set 1 2) 9)]"
        ),
        "[2 nil]"
    );
    // `find` stays unsupported on sorted sets, like the JVM (probe
    // `set-find`, JVM text verbatim).
    assert_eq!(
        uncaught(&mut eval, "(find (sorted-set 1 2) 2)"),
        "java.lang.IllegalArgumentException | find not supported on type: clojure.lang.PersistentTreeSet"
    );
    // A head dropped past the end is nil (probe `subseq-drop`).
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1) > :a)"),
        "nil"
    );
    // An empty start scan is nil (probe `subseq5e`).
    assert_eq!(
        run_last(&mut eval, "(subseq (sorted-map :a 1 :b 2) > :z < :zz)"),
        "nil"
    );
    // The 5-arg `rsubseq` walks end-first, descending (probes
    // `rsubseq-5b`, `rsubseq-5set`, `rsubseq-5keep`).
    assert_eq!(
        run_last(
            &mut eval,
            "(rsubseq (sorted-map :a 1 :b 2 :c 3) >= :a < :c)"
        ),
        "([:b 2] [:a 1])"
    );
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-set 1 2 3) >= 1 < 3)"),
        "(2 1)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(rsubseq (sorted-map :a 1 :b 2 :c 3) > :a <= :c)"
        ),
        "([:c 3] [:b 2])"
    );
    // A dropped-to-empty head is `()` (probe `rsubseq-empty`), while
    // an empty end scan is nil (probe `rsubseq-below`).
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-map :a 1 :b 2 :c 3) > :z < :a)"),
        "()"
    );
    assert_eq!(
        run_last(&mut eval, "(rsubseq (sorted-map :a 1 :b 2) > :a < :0)"),
        "nil"
    );
    // A non-symbol bound test is not callable (JVM: `Long` to `IFn`
    // cast error).
    assert!(
        uncaught(&mut eval, "(subseq (sorted-map :a 1) 42 :a)")
            .starts_with("java.lang.ClassCastException")
    );
    assert!(
        uncaught(&mut eval, "(rsubseq (sorted-map :a 1) 42 :a)")
            .starts_with("java.lang.ClassCastException")
    );
}
