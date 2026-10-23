// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! T1192 FX2 MapEntry: probe replay for the `fx2-mapentry*.clj`
//! oracle matrices (stock Clojure 1.13.0-alpha9 — every `//`
//! cite names its probe file and row). `class`/`type` are out of
//! scope as fns, so class rows read Rust-side via [`class_of`].

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

/// A booted evaluator with `(def e (first {:a 1}))`, like every
/// FX2 probe file.
fn entry_eval() -> Evaluator {
    let mut eval = boot();
    eval.eval_source("(def e (first {:a 1}))").unwrap();
    eval
}

#[test]
fn class_and_predicates() {
    let mut eval = entry_eval();
    // fx2-mapentry.class-e/class-f/class-fv/type-e (`type` ≡ `class`
    // on these values; both are `jvm_class_of` Rust-side).
    assert_eq!(class_of(&mut eval, "e"), "clojure.lang.MapEntry");
    assert_eq!(
        class_of(&mut eval, "(find {:a 1} :a)"),
        "clojure.lang.MapEntry"
    );
    assert_eq!(
        class_of(&mut eval, "(find [1 2] 0)"),
        "clojure.lang.MapEntry"
    );
    assert_eq!(class_of(&mut eval, "e"), "clojure.lang.MapEntry");
    // fx2-mapentry.preds
    assert_eq!(
        run_last(
            &mut eval,
            "[(map? e) (vector? e) (seq? e) (list? e) (coll? e) (sequential? e) (associative? e) (counted? e) (ifn? e)]"
        ),
        "[false true false false true true true true true]"
    );
    // fx2-mapentry2.moredpreds
    assert_eq!(
        run_last(&mut eval, "[(set? e) (sorted? e) (empty? e)]"),
        "[false false false]"
    );
    // fx2-mapentry.pr-e / fx2-mapentry2.str-e / fx2-mapentry3.entry-pr
    assert_eq!(run_last(&mut eval, "(pr-str e)"), "\"[:a 1]\"");
    assert_eq!(run_last(&mut eval, "(str e)"), "\"[:a 1]\"");
    // fx2-mapentry.seq-first-class: map seqs yield real entries.
    assert_eq!(
        class_of(&mut eval, "(first (seq {:a 1}))"),
        "clojure.lang.MapEntry"
    );
}

#[test]
fn indexed_access() {
    let mut eval = entry_eval();
    // fx2-mapentry.count/nth/seqs/get
    assert_eq!(run_last(&mut eval, "(count e)"), "2");
    assert_eq!(run_last(&mut eval, "[(nth e 0) (nth e 1)]"), "[:a 1]");
    assert_eq!(
        run_last(&mut eval, "[(first e) (vec (rest e)) (vec (seq e))]"),
        "[:a [1] [:a 1]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(get e 0) (get e 1) (get e 2) (get e :a) (get e 2 :dflt)]"
        ),
        "[:a 1 nil nil :dflt]"
    );
    // fx2-mapentry2.nth-nf
    assert_eq!(
        run_last(&mut eval, "[(nth e 0 :nf) (nth e 5 :nf)]"),
        "[:a :nf]"
    );
    // fx2-mapentry.contains
    assert_eq!(
        run_last(
            &mut eval,
            "[(contains? e 0) (contains? e 1) (contains? e 2) (contains? e :a)]"
        ),
        "[true true false false]"
    );
    // fx2-mapentry.nth-2/nth-neg: the IOOBE carries no message.
    assert_eq!(
        uncaught(&mut eval, "(nth e 2)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(nth e -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
}

#[test]
fn invoke_entry() {
    let mut eval = entry_eval();
    // fx2-mapentry.invoke
    assert_eq!(run_last(&mut eval, "[(e 0) (e 1)]"), "[:a 1]");
    // fx2-mapentry.invoke-2/invoke-kw/invoke-0
    assert_eq!(
        uncaught(&mut eval, "(e 2)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(e :a)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "(e)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.lang.MapEntry"
    );
    // fx2-mapentry2.invoke-15
    assert_eq!(
        uncaught(&mut eval, "(e 1.5)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
}

#[test]
fn conj_assoc_peek_pop() {
    let mut eval = entry_eval();
    // fx2-mapentry.conj/assoc0/assoc2/assoc-kw
    assert_eq!(run_last(&mut eval, "(conj e :x)"), "[:a 1 :x]");
    assert_eq!(
        class_of(&mut eval, "(conj e :x)"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run_last(&mut eval, "(assoc e 0 :b)"), "[:b 1]");
    assert_eq!(
        class_of(&mut eval, "(assoc e 0 :b)"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run_last(&mut eval, "(assoc e 2 :x)"), "[:a 1 :x]");
    assert_eq!(
        uncaught(&mut eval, "(assoc e :k :v)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    // fx2-mapentry2.assoc1/peek-pop/pop-class
    assert_eq!(run_last(&mut eval, "(assoc e 1 :b)"), "[:a :b]");
    assert_eq!(
        class_of(&mut eval, "(assoc e 1 :b)"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run_last(&mut eval, "(peek e)"), "1");
    assert_eq!(run_last(&mut eval, "(pop e)"), "[:a]");
    assert_eq!(
        class_of(&mut eval, "(pop e)"),
        "clojure.lang.PersistentVector"
    );
    // fx2-mapentry.empty: the empty of an entry is nil.
    assert_eq!(run_last(&mut eval, "(pr-str (empty e))"), "\"nil\"");
    // fx2-mapentry2.subvec-class: subvec answers a real subvector.
    assert_eq!(
        class_of(&mut eval, "(subvec e 0 1)"),
        "clojure.lang.APersistentVector$SubVector"
    );
    // fx2-mapentry2.dissoc-e
    assert_eq!(
        uncaught(&mut eval, "(dissoc e 0)"),
        "java.lang.ClassCastException | class clojure.lang.MapEntry cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.MapEntry and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
}

#[test]
fn equality_hash_compare() {
    let mut eval = entry_eval();
    // fx2-mapentry.eq/hash-eq/compare
    assert_eq!(
        run_last(
            &mut eval,
            "[(= e [:a 1]) (= [:a 1] e) (= e (first {:a 1}))]"
        ),
        "[true true true]"
    );
    assert_eq!(run_last(&mut eval, "(= (hash e) (hash [:a 1]))"), "true");
    assert_eq!(run_last(&mut eval, "(hash e)"), "-1809393901");
    assert_eq!(run_last(&mut eval, "(hash [:a 1])"), "-1809393901");
    assert_eq!(run_last(&mut eval, "(compare e [:a 1])"), "0");
    // fx2-mapentry3: sequential `=` crosses entry/vector/list/seq.
    assert_eq!(run_last(&mut eval, "(= [:a 1] '(:a 1))"), "true");
    assert_eq!(run_last(&mut eval, "(= e '(:a 1))"), "true");
    assert_eq!(run_last(&mut eval, "(= '(:a 1) e)"), "true");
    assert_eq!(run_last(&mut eval, "(= e (seq [:a 1]))"), "true");
    assert_eq!(
        class_of(&mut eval, "[:a 1]"),
        "clojure.lang.PersistentVector"
    );
    // fx2-mapentry16.compare-vec-entry/compare-entry-entry
    assert_eq!(run_last(&mut eval, "(compare [:a 1] e)"), "0");
    assert_eq!(run_last(&mut eval, "(compare e (first {:a 1}))"), "0");
    // fx2-mapentry8.compare-entry-list/compare-entry-str: entries
    // name `IPersistentVector`, like vectors.
    assert_eq!(
        uncaught(&mut eval, "(compare e '(:a 2))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class clojure.lang.IPersistentVector (clojure.lang.PersistentList and clojure.lang.IPersistentVector are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(compare e \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentVector (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')"
    );
    // fx2-mapentry8.compare-vec-list (control: vectors behave the same).
    assert_eq!(
        uncaught(&mut eval, "(compare [1] '(2))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class clojure.lang.IPersistentVector (clojure.lang.PersistentList and clojure.lang.IPersistentVector are in unnamed module of loader 'app')"
    );
    // fx2-mapentry16.set-member/map-lookup/pcth-eq
    assert_eq!(run_last(&mut eval, "(contains? #{[:a 1]} e)"), "true");
    assert_eq!(run_last(&mut eval, "({[:a 1] :v} e)"), ":v");
    assert_eq!(
        run_last(&mut eval, "(= (format \"%h\" e) (format \"%h\" [:a 1]))"),
        "true"
    );
}

#[test]
fn key_and_val() {
    let mut eval = entry_eval();
    // fx2-mapentry.keyval
    assert_eq!(run_last(&mut eval, "[(key e) (val e)]"), "[:a 1]");
    // fx2-mapentry.key-vec/val-kw/key-nil: only real entries answer.
    assert_eq!(
        uncaught(&mut eval, "(key [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(val :a)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(key nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Map$Entry.getKey()\" because \"e\" is null"
    );
    // fx2-mapentry6.val-nil (`val` reads `getValue`).
    assert_eq!(
        uncaught(&mut eval, "(val nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Map$Entry.getValue()\" because \"e\" is null"
    );
    // fx2-mapentry4.keys-of-vec: `keys` over 2-vectors fails entrywise.
    assert_eq!(
        uncaught(&mut eval, "(keys [[:a 1]])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn find_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry.find-num/find-miss
    assert_eq!(
        uncaught(&mut eval, "(find 5 5)"),
        "java.lang.IllegalArgumentException | find not supported on type: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(find {:a 1} :zz)"), "nil");
    // fx2-mapentry2.find-str
    assert_eq!(
        uncaught(&mut eval, "(find \"ab\" 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: java.lang.String"
    );
    // fx2-mapentry4.find1/find1class: `find` on entries answers entries.
    assert_eq!(run_last(&mut eval, "(find e 1)"), "[1 1]");
    assert_eq!(class_of(&mut eval, "(find e 1)"), "clojure.lang.MapEntry");
    // fx2-mapentry16.find-miss-entry
    assert_eq!(run_last(&mut eval, "(find e 9)"), "nil");
}

#[test]
fn conj_onto_map() {
    let mut eval = entry_eval();
    // fx2-mapentry.conj-pair/conj-kw/conj-entry
    assert_eq!(run_last(&mut eval, "(conj {:a 1} [:b 2])"), "{:a 1, :b 2}");
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} :x)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    assert_eq!(run_last(&mut eval, "(conj {:a 1} e)"), "{:a 1}");
    // fx2-mapentry2.conj-listpair/conj-3vec/conj-str/conj-num
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} '(:b 2))"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} [1 2 3])"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.util.Map$Entry (java.lang.Character and java.util.Map$Entry are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // fx2-mapentry2.conj-nest1/conj-nest2/conj-vec-in-list
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} '((:b 2)))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentList is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} [[1 2 3]])"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {:a 1} '([:b 2]))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    // fx2-mapentry9: seqs of real entries conj each pair.
    assert_eq!(
        run_last(&mut eval, "(conj {:a 1} (seq {:b 2}))"),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(&mut eval, "(conj {:a 1} (list (first {:b 2})))"),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(&mut eval, "(conj {:a 0 :b 0} (seq {:b 2 :c 3}))"),
        "{:a 0, :b 2, :c 3}"
    );
    // fx2-mapentry16.conj-empty-seq/transient-conj-entry
    assert_eq!(run_last(&mut eval, "(conj {:a 1} '())"), "{:a 1}");
    assert_eq!(
        run_last(
            &mut eval,
            "(persistent! (conj! (transient {:a 1}) (first {:b 2})))"
        ),
        "{:a 1, :b 2}"
    );
}

#[test]
fn meta_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry2.meta-e: `meta` reads nil (entries are no `IObj`).
    assert_eq!(run_last(&mut eval, "(meta e)"), "nil");
    // fx2-mapentry.with-meta-e / fx2-mapentry2.vary-meta-e
    assert_eq!(
        uncaught(&mut eval, "(with-meta e {:m 1})"),
        "java.lang.ClassCastException | class clojure.lang.MapEntry cannot be cast to class clojure.lang.IObj (clojure.lang.MapEntry and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(vary-meta e assoc :m 1)"),
        "java.lang.ClassCastException | class clojure.lang.MapEntry cannot be cast to class clojure.lang.IObj (clojure.lang.MapEntry and clojure.lang.IObj are in unnamed module of loader 'app')"
    );
    // fx2-mapentry4.selectkeys
    assert_eq!(run_last(&mut eval, "(select-keys e [0])"), "{0 :a}");
}

#[test]
fn rseq_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry2.rsub
    assert_eq!(
        run_last(&mut eval, "[(pr-str (rseq e)) (pr-str (subvec e 0 1))]"),
        "[\"(1 :a)\" \"[:a]\"]"
    );
    // fx2-mapentry7.rseq-vec/rseq-vec-class
    assert_eq!(run_last(&mut eval, "(rseq [:a 1])"), "(1 :a)");
    assert_eq!(
        class_of(&mut eval, "(rseq [:a 1])"),
        "clojure.lang.APersistentVector$RSeq"
    );
    // fx2-mapentry7.rseq-str: strings are not reversible.
    assert_eq!(
        uncaught(&mut eval, "(rseq \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Reversible (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Reversible is in unnamed module of loader 'app')"
    );
    // fx2-mapentry8.rseq-list/rseq-map/rseq-nil
    assert_eq!(
        uncaught(&mut eval, "(rseq '(1 2 3))"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class clojure.lang.Reversible (clojure.lang.PersistentList and clojure.lang.Reversible are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rseq {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class clojure.lang.Reversible (clojure.lang.PersistentArrayMap and clojure.lang.Reversible are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rseq nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Reversible.rseq()\" because \"rev\" is null"
    );
    // fx2-mapentry8.rseq-empty/rseq-empty-class/rseq-counted/rseq-count/rseq-first
    // (`(class nil)` is nil; `class_of` reports the `jvm_class_of`
    // convention `"null"` for it.)
    assert_eq!(run_last(&mut eval, "(rseq [])"), "nil");
    assert_eq!(class_of(&mut eval, "(rseq [])"), "null");
    assert_eq!(run_last(&mut eval, "(counted? (rseq [:a 1]))"), "true");
    assert_eq!(run_last(&mut eval, "(count (rseq [:a 1]))"), "2");
    assert_eq!(run_last(&mut eval, "(first (rseq [:a 1]))"), "1");
    // fx2-mapentry9.rseq-entry-class/rseq-entry-empty/subvec-empty-class
    assert_eq!(
        class_of(&mut eval, "(rseq e)"),
        "clojure.lang.APersistentVector$RSeq"
    );
    assert_eq!(run_last(&mut eval, "(rseq (subvec [:a] 0 0))"), "nil");
    assert_eq!(
        class_of(&mut eval, "(subvec [:a] 0 0)"),
        "clojure.lang.PersistentVector"
    );
    // fx2-mapentry9.drop-rseq-class/seq?-rseq/nth-rseq/rseq-subvec/rseq-subvec-class
    assert_eq!(
        class_of(&mut eval, "(drop 1 (rseq [1 2 3]))"),
        "clojure.lang.LazySeq"
    );
    assert_eq!(run_last(&mut eval, "(seq? (rseq [1 2]))"), "true");
    assert_eq!(run_last(&mut eval, "(nth (rseq [1 2 3]) 0)"), "3");
    assert_eq!(run_last(&mut eval, "(rseq (subvec [1 2 3] 0 2))"), "(2 1)");
    assert_eq!(
        class_of(&mut eval, "(rseq (subvec [1 2 3] 0 2))"),
        "clojure.lang.APersistentVector$RSeq"
    );
}

#[test]
fn conversions() {
    let mut eval = entry_eval();
    // fx2-mapentry5.vec-of-entry/vec-of-entry-class/set-of-entry
    // (the set row asserts order-free: the port seqs small sets in
    // insertion order while the JVM hashes them).
    assert_eq!(run_last(&mut eval, "(vec e)"), "[:a 1]");
    assert_eq!(
        class_of(&mut eval, "(vec e)"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run_last(&mut eval, "(= (set e) #{:a 1})"), "true");
    // fx2-mapentry5.liststar/apply-vec/into-vec/mapv-entry
    assert_eq!(run_last(&mut eval, "(list* e)"), "(:a 1)");
    assert_eq!(run_last(&mut eval, "(apply vector e)"), "[:a 1]");
    assert_eq!(run_last(&mut eval, "(into [] e)"), "[:a 1]");
    assert_eq!(run_last(&mut eval, "(mapv identity e)"), "[:a 1]");
    // fx2-mapentry5.sort-entries/distinct/frequencies/maxkey
    assert_eq!(
        run_last(&mut eval, "(sort [(first {:b 2}) (first {:a 1})])"),
        "([:a 1] [:b 2])"
    );
    assert_eq!(run_last(&mut eval, "(distinct [e [:a 1]])"), "([:a 1])");
    assert_eq!(run_last(&mut eval, "(frequencies [e e])"), "{[:a 1] 2}");
    assert_eq!(run_last(&mut eval, "(max-key hash e [:a 1])"), "[:a 1]");
}

#[test]
fn replace_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry6.replace-entry/replace-entry-class: no match
    // passes the entry itself through.
    assert_eq!(run_last(&mut eval, "(replace {0 :x} e)"), "[:a 1]");
    assert_eq!(
        class_of(&mut eval, "(replace {0 :x} e)"),
        "clojure.lang.MapEntry"
    );
    // fx2-mapentry7.replace-changed/replace-changed-class
    assert_eq!(run_last(&mut eval, "(replace {:a :x} e)"), "[:x 1]");
    assert_eq!(
        class_of(&mut eval, "(replace {:a :x} e)"),
        "clojure.lang.PersistentVector"
    );
}

#[test]
fn merge_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry5.merge-entry/merge-vec (`merge` conjs onto its
    // first arg, which stays a vector here).
    assert_eq!(run_last(&mut eval, "(merge e {:b 2})"), "[:a 1 [:b 2]]");
    assert_eq!(
        run_last(&mut eval, "(merge [:a 1] {:b 2})"),
        "[:a 1 [:b 2]]"
    );
    // fx2-mapentry11.merge-with-vec/merge-with-entry
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} [:b 2])"),
        "{:a 1, 0 :b, 1 2}"
    );
    assert_eq!(
        uncaught(&mut eval, "(merge-with + {:a 1} e)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    // fx2-mapentry12.merge-with-entry-left
    assert_eq!(
        uncaught(&mut eval, "(merge-with + e {:b 2})"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    // fx2-mapentry16.merge-with-seq
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} (seq {:b 2}))"),
        "{:a 1, :b 2}"
    );
}

#[test]
fn meta_value_casts() {
    let mut eval = entry_eval();
    eval.eval_source("(def x 1) (def ^:dynamic *dyn* 0)")
        .unwrap();
    // fx2-mapentry12.with-meta-nonmap
    assert_eq!(
        uncaught(&mut eval, "(with-meta [1] 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    // fx2-mapentry14.alter-meta-ret/reset-meta-nonmap
    assert_eq!(
        uncaught(&mut eval, "(alter-meta! #'x (fn [_] 5))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(reset-meta! #'x 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    // fx2-mapentry13.alter-meta-nonmap (`alter-meta!` calls its fn;
    // the invoke path keeps its pinned §E8 text while the oracle
    // reports the JDK text — a frozen-wedge divergence for FX7).
    assert_eq!(
        uncaught(&mut eval, "(alter-meta! #'x 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    // fx2-mapentry13.push-vec-entry / fx2-mapentry15.push-vec-scalar
    assert_eq!(
        run_last(
            &mut eval,
            "(do (push-thread-bindings [(first {#'*dyn* 1})]) (pop-thread-bindings) :ok)"
        ),
        ":ok"
    );
    assert_eq!(
        uncaught(&mut eval, "(push-thread-bindings [1])"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IMapEntry (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IMapEntry is in unnamed module of loader 'app')"
    );
}

#[test]
fn destructure_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry9.let-fixed/let-rest
    assert_eq!(run_last(&mut eval, "(let [[a b] e] [a b])"), "[:a 1]");
    assert_eq!(run_last(&mut eval, "(let [[a & r] e] [a r])"), "[:a (1)]");
    // fx2-mapentry10: map destructuring reads entries associatively.
    assert_eq!(run_last(&mut eval, "(let [{a 0} e] a)"), ":a");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{a 0, zz :missing, :or {zz :dflt}} e] [a zz])"
        ),
        "[:a :dflt]"
    );
    assert_eq!(run_last(&mut eval, "(let [{:keys [k]} e] k)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(let [{s 0} [\"x\" \"y\"]] s)"),
        "\"x\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{a 0, zz :m, :or {zz :dflt}} [:x :y]] [a zz])"
        ),
        "[:x :dflt]"
    );
    // fx2-mapentry17: entry trailers conj as their pairs.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{a :a, b :b} (list :a 1 (first {:b 2}))] [a b])"
        ),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{a :a, b :b} (list :a 1 (list (first {:b 2})))] [a b])"
        ),
        "[1 2]"
    );
}

#[test]
fn eval_case_update() {
    let mut eval = entry_eval();
    // fx2-mapentry9.eval-entry/eval-entry-class: evaluating an entry
    // answers a plain vector, like the JVM's `VectorExpr`.
    assert_eq!(run_last(&mut eval, "(eval e)"), "[:a 1]");
    assert_eq!(
        class_of(&mut eval, "(eval e)"),
        "clojure.lang.PersistentVector"
    );
    // fx2-mapentry8.case-entry / fx2-mapentry15.case-miss
    assert_eq!(run_last(&mut eval, "(case e [:a 1] :hit :miss)"), ":hit");
    assert_eq!(run_last(&mut eval, "(case e [:b 2] :hit :miss)"), ":miss");
    // fx2-mapentry15.get-in-nested/update-entry
    assert_eq!(run_last(&mut eval, "(get-in [e] [0 1])"), "1");
    assert_eq!(run_last(&mut eval, "(update e 0 name)"), "[\"a\" 1]");
}

#[test]
fn seq_fn_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry6.drop-class/drop-vec-class: entries are not
    // `IDrop` (drops stay lazy), vectors chunk.
    assert_eq!(class_of(&mut eval, "(drop 1 e)"), "clojure.lang.LazySeq");
    assert_eq!(
        class_of(&mut eval, "(drop 1 [:a 1])"),
        "clojure.lang.PersistentVector$ChunkedSeq"
    );
    // fx2-mapentry6.transient-entry: entries are not editable.
    assert_eq!(
        uncaught(&mut eval, "(transient e)"),
        "java.lang.ClassCastException | class clojure.lang.MapEntry cannot be cast to class clojure.lang.IEditableCollection (clojure.lang.MapEntry and clojure.lang.IEditableCollection are in unnamed module of loader 'app')"
    );
    // fx2-mapentry6.lazy-val-entry: printing realizes entry contents.
    assert_eq!(
        run_last(&mut eval, "(pr-str (find {:a (map inc [1])} :a))"),
        "\"[:a (2)]\""
    );
    // fx2-mapentry6 seq-function sweep (all seq-path legs).
    assert_eq!(run_last(&mut eval, "(nthnext e 1)"), "(1)");
    assert_eq!(
        run_last(&mut eval, "(map vec (split-at 1 e))"),
        "([:a] [1])"
    );
    // No `flatten` row: `flatten` needs lazy tree-walk machinery the
    // port lacks (`tree-seq` is unimplemented) — the probe6
    // `flatten` row stands as the oracle pin for that slice.
    assert_eq!(
        run_last(&mut eval, "(reduce (fn [a b] (str a b)) \"\" e)"),
        "\":a1\""
    );
    assert_eq!(run_last(&mut eval, "(last e)"), "1");
    assert_eq!(run_last(&mut eval, "(butlast e)"), "(:a)");
    assert_eq!(run_last(&mut eval, "(take 1 e)"), "(:a)");
    assert_eq!(run_last(&mut eval, "(take 3 (cycle e))"), "(:a 1 :a)");
    assert_eq!(run_last(&mut eval, "(interpose :x e)"), "(:a :x 1)");
}

#[test]
fn outlet_entry_rows() {
    // Port-defined `:all` outlets over entries (no JVM oracle — the
    // outlets are a port extension): entries behave as their
    // 2-vectors, mirroring `outlet_merge_conj_entry_non_map`.
    let mut eval = entry_eval();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [a] :or {a 1}} :m, :all all} (first {:k :v})] all)"
        ),
        "[:k :v [:m {:a 1}]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :all a} [(first {:k :v})]] a)"
        ),
        "{:x 1, :k :v}"
    );
    // A lone entry `from` explodes into its items (consistent with
    // `into`/`conj-kw`: conjing `:k` alone is the `ISeq` error).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [{x :x, :or {x 1}, :all a} (list (first {:k :v}))] a)"
        ),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    // Non-entry elements keep the outlet path's stable cast text.
    assert_eq!(
        uncaught(&mut eval, "(let [{x :x, :or {x 1}, :all a} [(list :k)]] a)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to java.util.Map$Entry"
    );
    // A multi-entry list `from`: the one-element `from` unwraps
    // to `((e1 e2))`, so `into1` hands the entry list to
    // `conj_map` as one element and the seq loop conjs each
    // pair (`eval.rs conj_map` insert + `Ok` lines).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :all a} (list (list (list (first {:k :v}) (first {:j :w}))))] a)"
        ),
        "{:x 1, :k :v, :j :w}"
    );
}

#[test]
fn hash_infinite_entry() {
    let mut eval = entry_eval();
    // Like the vector case (`core_r4_hash_post_realize_uoe`): an
    // infinite hiding in an entry is the bare UOE.
    assert_eq!(
        uncaught(&mut eval, "(hash (find {:a (repeat 1)} :a))"),
        "java.lang.UnsupportedOperationException | "
    );
}

#[test]
fn probe18_seqable() {
    let mut eval = entry_eval();
    // fx2-mapentry18.seqable-e/seqable-vec/seqable-num
    // (finding 8): entries seq like their 2-vectors.
    assert_eq!(run_last(&mut eval, "(seqable? e)"), "true");
    assert_eq!(run_last(&mut eval, "(seqable? [1 2])"), "true");
    assert_eq!(run_last(&mut eval, "(seqable? 5)"), "false");
}

#[test]
fn probe18_nthnext_edges() {
    let mut eval = entry_eval();
    // fx2-mapentry18.nthnext-e0/nthnext-e2 (finding 9):
    // `n = 0` seqs the whole entry, past-the-end is nil.
    // (`nthnext-e1` is already pinned as `(nthnext e 1)` in
    // `seq_fn_rows`.)
    assert_eq!(run_last(&mut eval, "(nthnext e 0)"), "(:a 1)");
    assert_eq!(run_last(&mut eval, "(nthnext e 2)"), "nil");
    // fx2-mapentry18.nthnext-vec0/nthnext-vec5
    assert_eq!(run_last(&mut eval, "(nthnext [1 2 3] 0)"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(nthnext [1 2 3] 5)"), "nil");
    // fx2-mapentry18.nthnext-neg: negative `n` keeps the whole
    // seq.
    assert_eq!(run_last(&mut eval, "(nthnext [1 2 3] -1)"), "(1 2 3)");
    // fx2-mapentry18.nthnext-nonn: a fractional `n` drops past
    // the truncation (`RT.nth` semantics, not `int`).
    assert_eq!(run_last(&mut eval, "(nthnext [1 2 3] 1.5)"), "(3)");
    // fx2-mapentry18.nthnext-unseq: unseqables are the `ISeq`
    // error.
    assert_eq!(
        uncaught(&mut eval, "(nthnext 5 1)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn probe18_merge_with_seqs() {
    let mut eval = entry_eval();
    // fx2-mapentry18.merge-with-empty (finding 10): an empty
    // seq arg merges nothing.
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} (quote ()))"),
        "{:a 1}"
    );
    // fx2-mapentry18.merge-with-seq: a multi-entry seq conjs
    // each pair.
    assert_eq!(
        run_last(&mut eval, "(merge-with + {:a 1} (seq {:b 2 :c 3}))"),
        "{:a 1, :b 2, :c 3}"
    );
}

#[test]
fn probe18_find_invoke_vals_sorted() {
    let mut eval = entry_eval();
    // fx2-mapentry18.find-e0 (finding 18): the `0` leg of the
    // `coll.rs find_entry` entry arm (the `1`/miss legs are
    // pinned in `find_rows`).
    assert_eq!(run_last(&mut eval, "(find e 0)"), "[0 :a]");
    // fx2-mapentry18.find-e1/find-ekw
    assert_eq!(run_last(&mut eval, "(find e 1)"), "[1 1]");
    assert_eq!(run_last(&mut eval, "(find e :kw)"), "nil");
    // fx2-mapentry18.invoke-2/invoke-neg: two args is the
    // `MapEntry` arity error, `-1` the bare IOOBE.
    assert_eq!(
        uncaught(&mut eval, "(e 0 1)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.lang.MapEntry"
    );
    assert_eq!(
        uncaught(&mut eval, "(e -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // fx2-mapentry18.vals-vec: `vals` over 2-vectors fails
    // entrywise on realization (like `keys` in `key_and_val`).
    assert_eq!(
        uncaught(&mut eval, "(vals [[1 2]])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Map$Entry (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    // fx2-mapentry18.sorted-vec: vectors are not sorted. (The
    // `sorted-map` row is oracle-true but the port has no
    // sorted maps yet — FT6 owns it, so there is no
    // port-false pin against the oracle-true row.)
    assert_eq!(run_last(&mut eval, "(sorted? [1])"), "false");
}

#[test]
fn probe18_entry_binding_forms() {
    let mut eval = entry_eval();
    // fx2-mapentry18.let-entry-kw (finding 7): an entry in the
    // `let` bindings position analyzes as its 2-vector, so a
    // keyword binding fails ...
    assert_eq!(
        uncaught(&mut eval, "(eval (list (quote let) (first {:x 1}) 1))"),
        "clojure.lang.Compiler$CompilerException | Syntax error macroexpanding clojure.core/let at (user:1:1)."
    );
    // fx2-mapentry18.let-kw-vec: ... identically to the plain
    // vector control (the failure is about `:x`, not entryness).
    assert_eq!(
        uncaught(&mut eval, "(eval (list (quote let) [(quote :x) 1] 1))"),
        "clojure.lang.Compiler$CompilerException | Syntax error macroexpanding clojure.core/let at (user:1:1)."
    );
    // fx2-mapentry18.fn-entry-invoke/fn-entry-fnq: an entry
    // params vector makes a real fn.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (eval (list (quote fn) (first {(quote a) (quote b)}) (quote a)))] (f 10 20))"
        ),
        "10"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(fn? (eval (list (quote fn) (first {(quote a) (quote b)}) (quote a))))"
        ),
        "true"
    );
    // fx2-mapentry18.let-entry-pattern: an entry as a
    // sequential destructuring pattern.
    assert_eq!(
        run_last(
            &mut eval,
            "(eval (list (quote let) [(first {(quote a) (quote b)}) [10 20]] (quote a)))"
        ),
        "10"
    );
    // fx2-mapentry18.let-entry-keys: an entry as the `:keys`
    // binding vector.
    assert_eq!(
        run_last(
            &mut eval,
            "(eval (list (quote let) [{:keys (first {(quote a) (quote b)})} {:a 10 :b 20}] (quote a)))"
        ),
        "10"
    );
    // fx2-mapentry18.let-entry-bindings: an entry of valid
    // bindings binds.
    assert_eq!(
        run_last(
            &mut eval,
            "(eval (list (quote let) (first {(quote x) 1}) (quote x)))"
        ),
        "1"
    );
}

#[test]
fn probe18_arity_errors() {
    let mut eval = entry_eval();
    // fx2-mapentry18.rseq-0/rseq-2/nthnext-1/nthnext-0/
    // sorted-0/sorted-2 (finding 20): fixed-arity natives
    // name the `clojure.core/F` display form (the
    // `seqs.rs mapcat_rows` convention).
    assert_eq!(
        uncaught(&mut eval, "(rseq)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/rseq"
    );
    assert_eq!(
        uncaught(&mut eval, "(rseq [1] [2])"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/rseq"
    );
    assert_eq!(
        uncaught(&mut eval, "(nthnext [1])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/nthnext"
    );
    assert_eq!(
        uncaught(&mut eval, "(nthnext)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/nthnext"
    );
    assert_eq!(
        uncaught(&mut eval, "(sorted?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/sorted?"
    );
    assert_eq!(
        uncaught(&mut eval, "(sorted? [1] [2])"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/sorted?"
    );
}

#[test]
fn probe18_destructure_entry_keys() {
    let mut eval = entry_eval();
    // fx2-mapentry18.mapdest-0/mapdest-1: map destructuring
    // reads entries by int key (`eval.rs lookup` entry legs).
    assert_eq!(run_last(&mut eval, "(let [{x 0} (first {:a 1})] x)"), ":a");
    assert_eq!(run_last(&mut eval, "(let [{x 1} (first {:a 1})] x)"), "1");
    // fx2-mapentry19.mapdest-5/mapdest-neg: out-of-range int
    // keys bind nil (the `eval.rs lookup` entry fallthrough).
    assert_eq!(run_last(&mut eval, "(let [{x 5} (first {:a 1})] x)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(let [{x -1} (first {:a 1})] x)"),
        "nil"
    );
    // fx2-mapentry18.seqdest-missing: a missing sequential
    // index binds nil (no `&`, so this routes through the
    // `eval.rs nth_or_nil` entry fallthrough at `i >= 2`).
    assert_eq!(
        run_last(&mut eval, "(let [[a b c] (first {:x 1})] c)"),
        "nil"
    );
}

#[test]
fn probe18_dead_rows() {
    let mut eval = entry_eval();
    // fx2-mapentry18.merge-with-entry-class (finding 16):
    // re-probe of the probe11 row. The probe wraps the form
    // in `class`, which is out of scope as a fn (see the
    // module docs), so the pin asserts the inner throw
    // directly — it fires before `class` runs on the JVM
    // too (same text as `merge_rows`).
    assert_eq!(
        uncaught(&mut eval, "(merge-with + {:a 1} e)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.util.Map$Entry (clojure.lang.Keyword is in unnamed module of loader 'app'; java.util.Map$Entry is in module java.base of loader 'bootstrap')"
    );
    // fx2-mapentry18.rseq-str-class: re-probe of the probe7
    // row (same text as `rseq_rows`).
    assert_eq!(
        uncaught(&mut eval, "(rseq \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Reversible (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Reversible is in unnamed module of loader 'app')"
    );
}
