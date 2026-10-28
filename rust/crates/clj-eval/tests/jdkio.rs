// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S17 JDK/IO adapters: handle shapes, seq adapters, stream
//! consumers, resultset rows, file walks, xml walks.
//! Oracle: `docs/zero-oos-s17-jdkio/probes/jdkio.out`
//! (labels cited per assertion).

use clj_eval::{Evaluator, SourceError, boot, jdkio};
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

/// Intern a Rust-built value as `user/<name>` (the
/// `concurrency.rs` promise-test pattern): the only way
/// handles enter evaluated code.
fn intern(eval: &mut Evaluator, name: &str, v: Value) {
    let sym = clj_value::Symbol::new(Some("user"), name);
    let id = eval.registry.intern(&sym);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(v);
    });
}

/// Fetch `user/<name>`'s root back out (for cursor pins).
fn lookup(eval: &Evaluator, name: &str) -> Value {
    let sym = clj_value::Symbol::new(Some("user"), name);
    let id = eval.registry.intern(&sym);
    eval.registry
        .with_state_mut(id, |st| st.root.clone())
        .unwrap()
        .expect("interned root")
}

/// Normalize `#object` identity hex (`0x…` → `0xH`, `@…` →
/// `@L`), like the oracle's `normhex`.
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '0' && it.peek() == Some(&'x') {
            out.push_str("0xH");
            it.next();
            while it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                it.next();
            }
        } else if c == '@' && it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push_str("@L");
            while it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn ints(n: i64) -> Vec<Value> {
    (1..=n).map(Value::Int).collect()
}

#[test]
fn handle_print_rows() {
    let mut eval = boot();
    // `f-*`: the port reports the stable interfaces (the JVM
    // reports concrete impl classes — documented divergence).
    let e = jdkio::enumeration(&mut eval, ints(1));
    let i = jdkio::iterator(&mut eval, ints(1));
    let s = jdkio::stream(&mut eval, ints(1));
    let r = jdkio::result_set(&mut eval, vec!["A".to_owned()], vec![vec![Value::Int(1)]]);
    let f = jdkio::file(&mut eval, "some/path.txt");
    assert_eq!(
        norm(&pr(&e)),
        "#object[java.util.Enumeration 0xH \"java.util.Enumeration@L\"]"
    );
    assert_eq!(
        norm(&pr(&i)),
        "#object[java.util.Iterator 0xH \"java.util.Iterator@L\"]"
    );
    assert_eq!(
        norm(&pr(&s)),
        "#object[java.util.stream.Stream 0xH \"java.util.stream.Stream@L\"]"
    );
    assert_eq!(
        norm(&pr(&r)),
        "#object[java.sql.ResultSet 0xH \"java.sql.ResultSet@L\"]"
    );
    // `d-file-print`: quoted part is the path.
    assert_eq!(norm(&pr(&f)), "#object[java.io.File 0xH \"some/path.txt\"]");
    intern(&mut eval, "s17-e", e.clone());
    intern(&mut eval, "s17-i", i.clone());
    intern(&mut eval, "s17-s", s.clone());
    intern(&mut eval, "s17-r", r.clone());
    intern(&mut eval, "s17-f", f.clone());
    assert_eq!(
        norm(&run_last(&mut eval, "(pr-str s17-e)")),
        "\"#object[java.util.Enumeration 0xH \\\"java.util.Enumeration@L\\\"]\""
    );
    // `d-file-str`: `str` of a file is the path; other
    // handles stringify through `print`.
    assert_eq!(run_last(&mut eval, "(str s17-f)"), "\"some/path.txt\"");
    assert_eq!(
        norm(&run_last(&mut eval, "(str s17-e)")),
        "\"#object[java.util.Enumeration 0xH \\\"java.util.Enumeration@L\\\"]\""
    );
    // Identity: handles equal themselves only — except files,
    // which compare by path like `java.io.File.equals`.
    // (`=` dispatches on the left operand, so every handle
    // takes the left seat.)
    assert_eq!(run_last(&mut eval, "(= s17-e s17-e)"), "true");
    assert_eq!(run_last(&mut eval, "(= s17-e s17-i)"), "false");
    assert_eq!(run_last(&mut eval, "(= s17-i s17-i)"), "true");
    assert_eq!(run_last(&mut eval, "(= s17-i s17-e)"), "false");
    assert_eq!(run_last(&mut eval, "(= s17-s s17-s)"), "true");
    assert_eq!(run_last(&mut eval, "(= s17-s s17-e)"), "false");
    assert_eq!(run_last(&mut eval, "(= s17-r s17-r)"), "true");
    assert_eq!(run_last(&mut eval, "(= s17-r s17-e)"), "false");
    assert_eq!(run_last(&mut eval, "(= s17-f s17-f)"), "true");
    let f2 = jdkio::file(&mut eval, "some/path.txt");
    intern(&mut eval, "s17-f2", f2);
    let f3 = jdkio::file(&mut eval, "other.txt");
    intern(&mut eval, "s17-f3", f3);
    assert_eq!(run_last(&mut eval, "(= s17-f s17-f2)"), "true");
    assert_eq!(run_last(&mut eval, "(= s17-f s17-f3)"), "false");
    assert_eq!(run_last(&mut eval, "(= s17-f \"some/path.txt\")"), "false");
    // Every handle hashes, and round-trips as a map key
    // (path-consistent for files).
    for h in ["s17-e", "s17-i", "s17-s", "s17-r", "s17-f"] {
        assert_eq!(
            run_last(&mut eval, &format!("(integer? (hash {h}))")),
            "true",
            "{h}"
        );
        assert_eq!(
            run_last(&mut eval, &format!("(get (assoc {{}} {h} :v) {h})")),
            ":v",
            "{h}"
        );
    }
    assert_eq!(
        run_last(&mut eval, "(get (assoc {} s17-f :v) s17-f2)"),
        ":v"
    );
    // CCEs *from* stream/resultset/file handles (their class names).
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq s17-s)"),
        "java.lang.ClassCastException | class java.util.stream.Stream cannot be cast to class java.util.Iterator (java.util.stream.Stream and java.util.Iterator are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq s17-r)"),
        "java.lang.ClassCastException | class java.sql.ResultSet cannot be cast to class java.util.Enumeration (java.sql.ResultSet is in module java.sql of loader 'platform'; java.util.Enumeration is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq s17-f)"),
        "java.lang.ClassCastException | class java.io.File cannot be cast to class java.util.Iterator (java.io.File and java.util.Iterator are in module java.base of loader 'bootstrap')"
    );
    // Cursor probes answer `None` off-handle (and files have no cursor).
    assert_eq!(jdkio::remaining(&Value::Int(1)), None);
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-f")), None);
    assert_eq!(jdkio::stream_consumed(&Value::Int(1)), None);
    // Handles are not seqable.
    assert_eq!(
        uncaught(&mut eval, "(seq s17-e)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.util.Enumeration"
    );
}

#[test]
fn enumeration_seq_rows() {
    let mut eval = boot();
    // `a-enum-vals`: strict 3-item spine.
    let h_s17_e3 = jdkio::enumeration(&mut eval, ints(3));
    intern(&mut eval, "s17-e3", h_s17_e3);
    assert_eq!(
        run_last(&mut eval, "(vec (enumeration-seq s17-e3))"),
        "[1 2 3]"
    );
    // `a-enum-empty-nil` / `a-enum-empty-vec`: empty answers nil.
    let h_s17_e0 = jdkio::enumeration(&mut eval, vec![]);
    intern(&mut eval, "s17-e0", h_s17_e0);
    assert_eq!(run_last(&mut eval, "(enumeration-seq s17-e0)"), "nil");
    assert_eq!(run_last(&mut eval, "(vec (enumeration-seq s17-e0))"), "[]");
    // Strictness: construction drains the handle (re-seq is nil).
    let h_s17_ed = jdkio::enumeration(&mut eval, ints(2));
    intern(&mut eval, "s17-ed", h_s17_ed);
    assert_eq!(
        run_last(&mut eval, "(vec (enumeration-seq s17-ed))"),
        "[1 2]"
    );
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-ed")), Some(0));
    assert_eq!(run_last(&mut eval, "(enumeration-seq s17-ed)"), "nil");
    // `a-enum-lazy`: not `IPending` — `realized?` throws CCE.
    let h_s17_er = jdkio::enumeration(&mut eval, ints(1));
    intern(&mut eval, "s17-er", h_s17_er);
    assert_eq!(
        uncaught(&mut eval, "(realized? (enumeration-seq s17-er))"),
        "java.lang.ClassCastException | class clojure.lang.ArraySeq cannot be cast to class clojure.lang.IPending (clojure.lang.ArraySeq and clojure.lang.IPending are in unnamed module of loader 'app')"
    );
    // Seq behavior: `seq?` but not `list?`, uncounted.
    let h_s17_es = jdkio::enumeration(&mut eval, ints(3));
    intern(&mut eval, "s17-es", h_s17_es);
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (enumeration-seq s17-es)] [(seq? s) (list? s) (counted? s) (count s) (first s) (vec (rest s)) (vec (next s))])"
        ),
        "[true false false 3 1 [2 3] [2 3]]"
    );
    // `n-enum-nil`: nil is the oracle NPE.
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Enumeration.hasMoreElements()\" because \"iter\" is null"
    );
    // `g-enum-argerr`: CCE to the interface (same- and
    // mixed-module shapes, plus cross-handle).
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Enumeration (java.lang.Long and java.util.Enumeration are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq [1])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.Enumeration (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.Enumeration is in module java.base of loader 'bootstrap')"
    );
    let h_s17_ex = jdkio::iterator(&mut eval, ints(1));
    intern(&mut eval, "s17-ex", h_s17_ex);
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq s17-ex)"),
        "java.lang.ClassCastException | class java.util.Iterator cannot be cast to class java.util.Enumeration (java.util.Iterator and java.util.Enumeration are in module java.base of loader 'bootstrap')"
    );
    // `g-enum-arity*`.
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/enumeration-seq"
    );
    assert_eq!(
        uncaught(&mut eval, "(enumeration-seq s17-e3 s17-e3)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/enumeration-seq"
    );
}

#[test]
fn iterator_seq_rows() {
    let mut eval = boot();
    // `a-iter-vals`.
    let h_s17_i3 = jdkio::iterator(&mut eval, ints(3));
    intern(&mut eval, "s17-i3", h_s17_i3);
    assert_eq!(
        run_last(&mut eval, "(vec (iterator-seq s17-i3))"),
        "[1 2 3]"
    );
    // `a-iter-empty-nil` / `a-iter-empty-vec`.
    let h_s17_i0 = jdkio::iterator(&mut eval, vec![]);
    intern(&mut eval, "s17-i0", h_s17_i0);
    assert_eq!(run_last(&mut eval, "(iterator-seq s17-i0)"), "nil");
    // `a-iter-lazy`: unforced `realized?` is false.
    let h_s17_il = jdkio::iterator(&mut eval, ints(3));
    intern(&mut eval, "s17-il", h_s17_il);
    assert_eq!(
        run_last(&mut eval, "(realized? (iterator-seq s17-il))"),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (iterator-seq s17-il)] (doall s) (realized? s))"
        ),
        "true"
    );
    // Chunked-32: `first` pulls one chunk off a 40-handle.
    let h_s17_i40 = jdkio::iterator(&mut eval, ints(40));
    intern(&mut eval, "s17-i40", h_s17_i40);
    assert_eq!(run_last(&mut eval, "(first (iterator-seq s17-i40))"), "1");
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-i40")), Some(8));
    // Forcing advances the shared handle; re-seq sees the rest.
    let h_s17_ia = jdkio::iterator(&mut eval, ints(3));
    intern(&mut eval, "s17-ia", h_s17_ia);
    assert_eq!(run_last(&mut eval, "(first (iterator-seq s17-ia))"), "1");
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-ia")), Some(0));
    assert_eq!(run_last(&mut eval, "(iterator-seq s17-ia)"), "nil");
    // `n-iter-nil` / `g-iter-argerr`.
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Iterator.hasNext()\" because \"iter\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.util.Iterator (java.lang.Long and java.util.Iterator are in module java.base of loader 'bootstrap')"
    );
    let h_s17_ix = jdkio::enumeration(&mut eval, ints(1));
    intern(&mut eval, "s17-ix", h_s17_ix);
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq s17-ix)"),
        "java.lang.ClassCastException | class java.util.Enumeration cannot be cast to class java.util.Iterator (java.util.Enumeration and java.util.Iterator are in module java.base of loader 'bootstrap')"
    );
    // `g-iter-arity0`.
    assert_eq!(
        uncaught(&mut eval, "(iterator-seq)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/iterator-seq"
    );
}

#[test]
fn stream_seq_rows() {
    let mut eval = boot();
    // `b-seq-vals`.
    let h_s17_s3 = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-s3", h_s17_s3);
    assert_eq!(run_last(&mut eval, "(vec (stream-seq! s17-s3))"), "[1 2 3]");
    // `b-seq-empty-nil`: empty answers nil — but still latches.
    let h_s17_s0 = jdkio::stream(&mut eval, vec![]);
    intern(&mut eval, "s17-s0", h_s17_s0);
    assert_eq!(run_last(&mut eval, "(stream-seq! s17-s0)"), "nil");
    assert_eq!(jdkio::stream_consumed(&lookup(&eval, "s17-s0")), Some(true));
    // `b-seq-lazy`: unforced `realized?` is false.
    let h_s17_sl = jdkio::stream(&mut eval, ints(2));
    intern(&mut eval, "s17-sl", h_s17_sl);
    assert_eq!(
        run_last(&mut eval, "(realized? (stream-seq! s17-sl))"),
        "false"
    );
    // Chunked-32 like `iterator-seq`.
    let h_s17_s40 = jdkio::stream(&mut eval, ints(40));
    intern(&mut eval, "s17-s40", h_s17_s40);
    assert_eq!(run_last(&mut eval, "(first (stream-seq! s17-s40))"), "1");
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-s40")), Some(8));
    // `b-reuse` / `b-reuse2`: terminal ops latch (both orders).
    let h_s17_sr = jdkio::stream(&mut eval, ints(2));
    intern(&mut eval, "s17-sr", h_s17_sr);
    assert_eq!(run_last(&mut eval, "(stream-seq! s17-sr)"), "(1 2)");
    assert_eq!(jdkio::stream_consumed(&lookup(&eval, "s17-sr")), Some(true));
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! + s17-sr)"),
        "java.lang.IllegalStateException | stream has already been operated upon or closed"
    );
    let h_s17_sr2 = jdkio::stream(&mut eval, ints(2));
    intern(&mut eval, "s17-sr2", h_s17_sr2);
    assert_eq!(run_last(&mut eval, "(stream-reduce! + s17-sr2)"), "3");
    assert_eq!(
        uncaught(&mut eval, "(stream-seq! s17-sr2)"),
        "java.lang.IllegalStateException | stream has already been operated upon or closed"
    );
    // `n-seq-nil` names `stream`; `g-stream-argerr` casts to `BaseStream`.
    assert_eq!(
        uncaught(&mut eval, "(stream-seq! nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"stream\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(stream-seq! [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.stream.BaseStream (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.stream.BaseStream is in module java.base of loader 'bootstrap')"
    );
    // `g-seq-arity0`.
    assert_eq!(
        uncaught(&mut eval, "(stream-seq!)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/stream-seq!"
    );
}

#[test]
fn stream_reduce_rows() {
    let mut eval = boot();
    // `b-reduce` / `b-reduce-init` / `b-reduce-empty` / `b-reduce-empty-init`.
    let h_s17_r3 = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-r3", h_s17_r3);
    assert_eq!(run_last(&mut eval, "(stream-reduce! + s17-r3)"), "6");
    let h_s17_r3b = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-r3b", h_s17_r3b);
    assert_eq!(run_last(&mut eval, "(stream-reduce! + 10 s17-r3b)"), "16");
    let h_s17_r0 = jdkio::stream(&mut eval, vec![]);
    intern(&mut eval, "s17-r0", h_s17_r0);
    assert_eq!(run_last(&mut eval, "(stream-reduce! + s17-r0)"), "0");
    let h_s17_r0b = jdkio::stream(&mut eval, vec![]);
    intern(&mut eval, "s17-r0b", h_s17_r0b);
    assert_eq!(run_last(&mut eval, "(stream-reduce! + 10 s17-r0b)"), "10");
    // `b-reduce-reduced`: `reduced` unwraps.
    let h_s17_rr = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-rr", h_s17_rr);
    assert_eq!(
        run_last(
            &mut eval,
            "(stream-reduce! (fn [a x] (if (= x 2) (reduced :stop) (+ a x))) 0 s17-rr)"
        ),
        ":stop"
    );
    // `n-reduce-nonfn-single`: one item never calls `f`.
    let h_s17_r1 = jdkio::stream(&mut eval, ints(1));
    intern(&mut eval, "s17-r1", h_s17_r1);
    assert_eq!(run_last(&mut eval, "(stream-reduce! 1 s17-r1)"), "1");
    // `n-reduce-nonfn-empty`: empty calls `(f)` — CCE to `IFn`.
    let h_s17_r0c = jdkio::stream(&mut eval, vec![]);
    intern(&mut eval, "s17-r0c", h_s17_r0c);
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! 1 s17-r0c)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    // The consumer drains the handle.
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-r3")), Some(0));
    // `n-reduce-nil` names `s`; `o-reduce-stream-first`: the
    // stream check precedes any `f` call.
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! + nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! 1 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"s\" is null"
    );
    // `g-reduce-argerr`.
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! + [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.util.stream.BaseStream (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.util.stream.BaseStream is in module java.base of loader 'bootstrap')"
    );
    // `g-reduce-arity1`.
    assert_eq!(
        uncaught(&mut eval, "(stream-reduce! +)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/stream-reduce!"
    );
}

#[test]
fn stream_transduce_rows() {
    let mut eval = boot();
    // `b-transduce` / `b-transduce-init` / `b-transduce-complete`.
    let h_s17_t3 = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-t3", h_s17_t3);
    assert_eq!(
        run_last(&mut eval, "(stream-transduce! (map inc) + 0 s17-t3)"),
        "9"
    );
    let h_s17_t3b = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-t3b", h_s17_t3b);
    assert_eq!(
        run_last(&mut eval, "(stream-transduce! (map inc) + s17-t3b)"),
        "9"
    );
    let h_s17_t3c = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-t3c", h_s17_t3c);
    assert_eq!(
        run_last(
            &mut eval,
            "(stream-transduce! (map inc) (completing + str) 0 s17-t3c)"
        ),
        "\"9\""
    );
    // `take` halts early (completion still runs): 1 + 2.
    let h_s17_t5 = jdkio::stream(&mut eval, ints(5));
    intern(&mut eval, "s17-t5", h_s17_t5);
    assert_eq!(
        run_last(&mut eval, "(stream-transduce! (take 2) + 0 s17-t5)"),
        "3"
    );
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-t5")), Some(3));
    // `o-transduce-init-first`: 3-arity evaluates `(f)` before
    // applying the xform (the `Long` is `f`, not the xform).
    let h_s17_to = jdkio::stream(&mut eval, ints(1));
    intern(&mut eval, "s17-to", h_s17_to);
    assert_eq!(
        uncaught(&mut eval, "(stream-transduce! \"x\" 6 s17-to)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    // `n-transduce-xnonfn`: a bad xform fails on application.
    let h_s17_tx = jdkio::stream(&mut eval, ints(1));
    intern(&mut eval, "s17-tx", h_s17_tx);
    assert_eq!(
        uncaught(&mut eval, "(stream-transduce! 5 + 0 s17-tx)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    // `n-transduce-nil`.
    assert_eq!(
        uncaught(&mut eval, "(stream-transduce! (map inc) + 0 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"s\" is null"
    );
    // `g-transduce-arity2`.
    assert_eq!(
        uncaught(&mut eval, "(stream-transduce! (map inc) +)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/stream-transduce!"
    );
}

#[test]
fn stream_into_rows() {
    let mut eval = boot();
    // `b-into-vec` / `b-into-xf`.
    let h_s17_n3 = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-n3", h_s17_n3);
    assert_eq!(run_last(&mut eval, "(stream-into! [] s17-n3)"), "[1 2 3]");
    let h_s17_n3b = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-n3b", h_s17_n3b);
    assert_eq!(
        run_last(&mut eval, "(stream-into! [] (map inc) s17-n3b)"),
        "[2 3 4]"
    );
    // `b-into-set` / `b-into-xf-set` (compared as sets — print
    // order is hash order).
    let h_s17_ns = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-ns", h_s17_ns);
    assert_eq!(
        run_last(&mut eval, "(= (stream-into! #{} s17-ns) #{1 2 3})"),
        "true"
    );
    let h_s17_nsb = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-nsb", h_s17_nsb);
    assert_eq!(
        run_last(
            &mut eval,
            "(= (stream-into! #{} (map inc) s17-nsb) #{2 3 4})"
        ),
        "true"
    );
    // `b-into-map` / `b-into-list`.
    let pairs = vec![
        Value::Vector(clj_value::Vector::from_vec(vec![
            Value::Keyword(clj_value::Keyword::new(None, "a")),
            Value::Int(1),
        ])),
        Value::Vector(clj_value::Vector::from_vec(vec![
            Value::Keyword(clj_value::Keyword::new(None, "b")),
            Value::Int(2),
        ])),
    ];
    let h_s17_nm = jdkio::stream(&mut eval, pairs);
    intern(&mut eval, "s17-nm", h_s17_nm);
    assert_eq!(
        run_last(&mut eval, "(stream-into! {} s17-nm)"),
        "{:a 1, :b 2}"
    );
    let h_s17_nl = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-nl", h_s17_nl);
    assert_eq!(run_last(&mut eval, "(stream-into! () s17-nl)"), "(3 2 1)");
    // Non-editable `to` with an xform (conj path, no transient).
    let h_s17_nlx = jdkio::stream(&mut eval, ints(3));
    intern(&mut eval, "s17-nlx", h_s17_nlx);
    assert_eq!(
        run_last(&mut eval, "(stream-into! () (map inc) s17-nlx)"),
        "(4 3 2)"
    );
    let h_s17_nn = jdkio::stream(&mut eval, ints(2));
    intern(&mut eval, "s17-nn", h_s17_nn);
    assert_eq!(
        run_last(&mut eval, "(stream-into! nil (map inc) s17-nn)"),
        "(3 2)"
    );
    // Editable targets persist (no transient escapes).
    let h_s17_nv = jdkio::stream(&mut eval, ints(2));
    intern(&mut eval, "s17-nv", h_s17_nv);
    assert_eq!(
        run_last(&mut eval, "(vector? (stream-into! [] s17-nv))"),
        "true"
    );
    // Transient `to` throws at the `(transient to)` call like
    // the JVM (differential pins: the `transient` CCE for
    // 2-arity, the xform-`into` CCE for 3-arity).
    let h_s17_nt = jdkio::stream(&mut eval, ints(1));
    intern(&mut eval, "s17-nt", h_s17_nt);
    let transient_err = uncaught(&mut eval, "(transient (transient []))");
    let stream_err = uncaught(&mut eval, "(stream-into! (transient []) s17-nt)");
    assert_eq!(stream_err, transient_err);
    let h_s17_nt3 = jdkio::stream(&mut eval, ints(1));
    intern(&mut eval, "s17-nt3", h_s17_nt3);
    let into_err = uncaught(&mut eval, "(into (transient []) (map inc) [1])");
    let stream_err3 = uncaught(&mut eval, "(stream-into! (transient []) (map inc) s17-nt3)");
    assert_eq!(stream_err3, into_err);
    // `o-into-xf-first`: the xform applies before the stream
    // is touched.
    assert_eq!(
        uncaught(&mut eval, "(stream-into! [] 5 nil)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    // `n-into-nil`.
    assert_eq!(
        uncaught(&mut eval, "(stream-into! [] nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.stream.BaseStream.iterator()\" because \"s\" is null"
    );
    // `g-into-arity1`.
    assert_eq!(
        uncaught(&mut eval, "(stream-into! [])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/stream-into!"
    );
}

#[test]
fn resultset_seq_rows() {
    let mut eval = boot();
    let labels = || vec!["NAME".to_owned(), "AGE".to_owned()];
    let rows2 = || {
        vec![
            vec![Value::Str("a".into()), Value::Int(1)],
            vec![Value::Str("b".into()), Value::Int(2)],
        ]
    };
    // `c-rows`: labels lowercase to keywords, rows print as maps.
    let h_s17_c2 = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-c2", h_s17_c2);
    assert_eq!(
        run_last(&mut eval, "(vec (resultset-seq s17-c2))"),
        "[{:name \"a\", :age 1} {:name \"b\", :age 2}]"
    );
    // `c-row-keys`: column order; rows equal plain maps.
    let h_s17_ck = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-ck", h_s17_ck);
    assert_eq!(
        run_last(&mut eval, "(vec (keys (first (resultset-seq s17-ck))))"),
        "[:name :age]"
    );
    let h_s17_ck2 = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-ck2", h_s17_ck2);
    assert_eq!(
        run_last(
            &mut eval,
            "(= (first (resultset-seq s17-ck2)) {:name \"a\", :age 1})"
        ),
        "true"
    );
    // Eager head, lazy tail: construction pulls one row.
    let h_s17_ch = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-ch", h_s17_ch);
    assert_eq!(
        run_last(&mut eval, "(first (resultset-seq s17-ch))"),
        "{:name \"a\", :age 1}"
    );
    assert_eq!(jdkio::remaining(&lookup(&eval, "s17-ch")), Some(1));
    let h_s17_ch2 = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-ch2", h_s17_ch2);
    assert_eq!(
        run_last(
            &mut eval,
            "(let [s (resultset-seq s17-ch2)] [(first s) (vec (rest s))])"
        ),
        "[{:name \"a\", :age 1} [{:name \"b\", :age 2}]]"
    );
    // `c-lazy`: Cons-headed — `realized?` throws CCE.
    let h_s17_cl = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-cl", h_s17_cl);
    assert_eq!(
        uncaught(&mut eval, "(realized? (resultset-seq s17-cl))"),
        "java.lang.ClassCastException | class clojure.lang.Cons cannot be cast to class clojure.lang.IPending (clojure.lang.Cons and clojure.lang.IPending are in unnamed module of loader 'app')"
    );
    // `c-empty-nil` / `c-empty-vec`.
    let h_s17_c0 = jdkio::result_set(&mut eval, vec!["A".to_owned()], vec![]);
    intern(&mut eval, "s17-c0", h_s17_c0);
    assert_eq!(run_last(&mut eval, "(resultset-seq s17-c0)"), "nil");
    assert_eq!(run_last(&mut eval, "(vec (resultset-seq s17-c0))"), "[]");
    // `c-dupkeys`: the eager `Exception` (fires at construction).
    let h_s17_cd = jdkio::result_set(
        &mut eval,
        vec!["A".to_owned(), "a".to_owned()],
        vec![vec![Value::Int(1), Value::Int(2)]],
    );
    intern(&mut eval, "s17-cd", h_s17_cd);
    assert_eq!(
        uncaught(&mut eval, "(resultset-seq s17-cd)"),
        "java.lang.Exception | ResultSet must have unique column labels"
    );
    // `c-row-second`: both rows read per-column values.
    let h_s17_cs = jdkio::result_set(&mut eval, labels(), rows2());
    intern(&mut eval, "s17-cs", h_s17_cs);
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (map (juxt :name :age) (resultset-seq s17-cs)))"
        ),
        "[[\"a\" 1] [\"b\" 2]]"
    );
    // `n-rs-nil` / `g-rs-argerr` (the `java.sql` platform module).
    assert_eq!(
        uncaught(&mut eval, "(resultset-seq nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.sql.ResultSet.getMetaData()\" because \"rs\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(resultset-seq 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.sql.ResultSet (java.lang.Long is in module java.base of loader 'bootstrap'; java.sql.ResultSet is in module java.sql of loader 'platform')"
    );
    // `g-rs-arity0`.
    assert_eq!(
        uncaught(&mut eval, "(resultset-seq)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/resultset-seq"
    );
}

/// Paths of a walk as Rust strings (`str` of each `File`).
fn walk_paths(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let vs = eval.eval_source(src).unwrap();
    let realized = eval.realize_deep(vs.last().unwrap()).unwrap();
    let Value::Vector(v) = realized else {
        panic!("expected a vector, got {realized:?}");
    };
    v.iter()
        .map(|item| {
            let Value::Str(s) = item else {
                panic!("expected a path string, got {item:?}");
            };
            s.to_string()
        })
        .collect()
}

#[test]
fn file_seq_rows() {
    let mut eval = boot();
    // Fixture tree (Rust owns the dirs): root/{mid/deep.txt, top.txt}.
    let dir = "./target/tmp-s17-jdkio";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(format!("{dir}/root/mid")).unwrap();
    std::fs::write(format!("{dir}/root/mid/deep.txt"), "deep").unwrap();
    std::fs::write(format!("{dir}/root/top.txt"), "top").unwrap();
    let root = format!("{dir}/root");
    let h_s17_d = jdkio::file(&mut eval, &root);
    intern(&mut eval, "s17-d", h_s17_d);
    // `d-root`: the root answers first (the same value).
    assert_eq!(
        run_last(&mut eval, "(= s17-d (first (file-seq s17-d)))"),
        "true"
    );
    // `d-names` / `d-count`: set-equality (walk order is OS-defined).
    let mut names: Vec<String> = walk_paths(&mut eval, "(vec (map str (file-seq s17-d)))")
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["deep.txt", "mid", "root", "top.txt"]);
    assert_eq!(run_last(&mut eval, "(count (file-seq s17-d))"), "4");
    // `d-preorder`: every parent precedes its children (the
    // root walks first, so every other node's parent is on-walk).
    let paths = walk_paths(&mut eval, "(vec (map str (file-seq s17-d)))");
    for child in paths.iter().skip(1) {
        let parent = std::path::Path::new(child)
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let pi = paths.iter().position(|p| p == &parent).unwrap();
        let ci = paths.iter().position(|p| p == child).unwrap();
        assert!(pi < ci, "parent {parent} must precede child {child}");
    }
    // `d-lazy`: unforced `realized?` is false.
    assert_eq!(run_last(&mut eval, "(realized? (file-seq s17-d))"), "false");
    // `d-file-arg`: a file root walks itself.
    let h_s17_df = jdkio::file(&mut eval, &format!("{dir}/root/top.txt"));
    intern(&mut eval, "s17-df", h_s17_df);
    assert_eq!(run_last(&mut eval, "(count (file-seq s17-df))"), "1");
    assert_eq!(
        run_last(&mut eval, "(= s17-df (first (file-seq s17-df)))"),
        "true"
    );
    // `d-missing`: a missing root walks itself (no error).
    let h_s17_dm = jdkio::file(&mut eval, "./target/tmp-s17-jdkio/nope");
    intern(&mut eval, "s17-dm", h_s17_dm);
    assert_eq!(run_last(&mut eval, "(count (file-seq s17-dm))"), "1");
    // `n-file-unforced` / `n-file-str-unforced`: string
    // roots construct lazily — and (wave-2 union with S12
    // `fs-str-arg`) walk as paths: the oracle CCE-on-string
    // (`g-file-argerr`, `n-file-vec-force`) has no live call
    // site while strings stay valid roots.
    assert_eq!(
        run_last(&mut eval, "(realized? (file-seq \"foo\"))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(do (file-seq \"foo\") :ok)"), ":ok");
    assert_eq!(run_last(&mut eval, "(vec (file-seq \"foo\"))"), "[\"foo\"]");
    // `n-file-nil-unforced` / `n-file-nil`: nil likewise.
    assert_eq!(run_last(&mut eval, "(realized? (file-seq nil))"), "false");
    assert_eq!(
        uncaught(&mut eval, "(vec (file-seq nil))"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.File.isDirectory()\" because \"f\" is null"
    );
    // `g-file-arity0`.
    assert_eq!(
        uncaught(&mut eval, "(file-seq)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/file-seq"
    );
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        let _ = std::fs::remove_dir("./target");
    }
}

/// Unreadable directories contribute no children (the JVM's
/// silent null `listFiles`); skipped where the chmod does not
/// take (e.g. root).
#[test]
#[cfg(unix)]
fn file_seq_unreadable_rows() {
    use std::os::unix::fs::PermissionsExt;
    let dir = "./target/tmp-s17-jdkio-unread";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(format!("{dir}/root/locked")).unwrap();
    std::fs::write(format!("{dir}/root/locked/hidden.txt"), "x").unwrap();
    std::fs::write(format!("{dir}/root/seen.txt"), "y").unwrap();
    std::fs::set_permissions(
        format!("{dir}/root/locked"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    if std::fs::read_dir(format!("{dir}/root/locked")).is_ok() {
        // (chmod did not take — restore and skip.)
        std::fs::set_permissions(
            format!("{dir}/root/locked"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        if !target_preexisted {
            let _ = std::fs::remove_dir("./target");
        }
        return;
    }
    let mut eval = boot();
    let h_s17_du = jdkio::file(&mut eval, &format!("{dir}/root"));
    intern(&mut eval, "s17-du", h_s17_du);
    let mut names: Vec<String> = walk_paths(&mut eval, "(vec (map str (file-seq s17-du)))")
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["locked", "root", "seen.txt"]);
    std::fs::set_permissions(
        format!("{dir}/root/locked"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        let _ = std::fs::remove_dir("./target");
    }
}

#[test]
fn xml_seq_rows() {
    let mut eval = boot();
    let tree = "{:tag :a :content [{:tag :b :content [\"hi\" {:tag :c :content []}]} \"tail\"]}";
    // `e-walk` / `e-count`: root-first depth order, string
    // leaves included.
    assert_eq!(
        run_last(&mut eval, &format!("(vec (map :tag (xml-seq {tree})))")),
        "[:a :b nil :c nil]"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(count (xml-seq {tree}))")),
        "5"
    );
    // `e-string-root` / `e-no-content` / `e-empty-content` / `e-nil`.
    assert_eq!(
        run_last(&mut eval, "(vec (xml-seq \"just a string\"))"),
        "[\"just a string\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(vec (xml-seq {:tag :x}))"),
        "[{:tag :x}]"
    );
    assert_eq!(
        run_last(&mut eval, "(vec (xml-seq {:tag :x :content []}))"),
        "[{:tag :x, :content []}]"
    );
    assert_eq!(run_last(&mut eval, "(vec (xml-seq nil))"), "[nil]");
    // `e-deep`.
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (map :tag (xml-seq {:tag :r :content [{:tag :a :content [{:tag :b :content [\"x\"]}]} {:tag :c :content nil}]})))"
        ),
        "[:r :a :b nil :c]"
    );
    // `e-lazy`: unforced `realized?` is false.
    assert_eq!(
        run_last(&mut eval, &format!("(realized? (xml-seq {tree}))")),
        "false"
    );
    // The port carries the docstring and `:added`.
    assert_eq!(
        run_last(&mut eval, "(-> #'xml-seq meta :doc)"),
        "\"A tree seq on the xml elements as per xml/parse\""
    );
    assert_eq!(run_last(&mut eval, "(-> #'xml-seq meta :added)"), "\"1.0\"");
    // `g-xml-arity*` (prefix-pinned: `core.clj` closures name
    // the eval site — the S3/S4a convention).
    for (src, count) in [
        ("(xml-seq)".to_owned(), 0),
        (format!("(xml-seq {tree} {tree})"), 2),
    ] {
        let got = uncaught(&mut eval, &src);
        assert!(
            got.starts_with(&format!(
                "clojure.lang.ArityException | Wrong number of args ({count}) passed to: user/"
            )) && got.contains("/fn--"),
            "for {src}: {got}"
        );
    }
}
