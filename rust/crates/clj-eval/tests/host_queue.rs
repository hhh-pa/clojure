// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure.lang.PersistentQueue`: the `EMPTY` field, `conj`/
//! `peek`/`pop`/`empty?`/`count`/`seq`/`first`/`nth`, queue
//! equality, and the `#object` print. Oracle probes
//! `/tmp/queue.clj`, `/tmp/queue2.clj`, `/tmp/queue3.clj`,
//! `/tmp/pqsup.clj`, `/tmp/pqsup2.clj` under:
//! `java -cp clojure-1.12.0.jar:spec.alpha-0.5.238.jar:
//! core.specs.alpha-0.4.74.jar clojure.main`.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw (strict: compile
/// errors fail — phase pins must not silently pass).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(_) => panic!("expected an uncaught throw, got success"),
    }
}

// Scrub hex identities like the oracle's `normhex`.
fn normhex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 'x' && out.ends_with("0") && chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push('x');
            out.push('H');
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

// q-empty/class/count/seq/peek/pop: the `EMPTY` field reads an
// empty queue (nil peek/seq, zero count, popping stays empty).
#[test]
fn queue_empty_field() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(class clojure.lang.PersistentQueue/EMPTY)"),
        "clojure.lang.PersistentQueue"
    );
    assert_eq!(
        run(&mut eval, "(empty? clojure.lang.PersistentQueue/EMPTY)"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(count clojure.lang.PersistentQueue/EMPTY)"),
        "0"
    );
    assert_eq!(
        run(&mut eval, "(seq clojure.lang.PersistentQueue/EMPTY)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(peek clojure.lang.PersistentQueue/EMPTY)"),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(count (pop clojure.lang.PersistentQueue/EMPTY))"
        ),
        "0"
    );
    assert_eq!(
        run(
            &mut eval,
            "(empty? (pop clojure.lang.PersistentQueue/EMPTY))"
        ),
        "true"
    );
}

// q-conj/peek/pop/persist/nil-elem: `conj` appends (multi-arg
// included), `peek`/`pop` take the front, ops persist (the
// source queue is unchanged), and nil is a storable element.
#[test]
fn queue_conj_peek_pop_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [q (conj clojure.lang.PersistentQueue/EMPTY 1 2 3)] [(peek q) (count q) (empty? q)])"
        ),
        "[1 3 false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(peek (pop (conj clojure.lang.PersistentQueue/EMPTY 1 2 3)))"
        ),
        "2"
    );
    assert_eq!(
        run(
            &mut eval,
            "(peek (pop (pop (pop (conj clojure.lang.PersistentQueue/EMPTY 1 2 3)))))"
        ),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [q (conj clojure.lang.PersistentQueue/EMPTY 1 2)] [(peek (conj q 9)) (peek q) (count q)])"
        ),
        "[1 1 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [q (conj clojure.lang.PersistentQueue/EMPTY 1 2)] [(peek (pop q)) (peek q) (count q)])"
        ),
        "[2 1 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [q (conj clojure.lang.PersistentQueue/EMPTY nil)] [(count q) (peek q) (seq q)])"
        ),
        "[1 nil (nil)]"
    );
}

// q-seq/first/rest/nth/map: queues seq front-to-back (so
// `first`/`rest`/`map` work) and read by index with the
// indexed miss rule.
#[test]
fn queue_seq_first_nth_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(seq (conj clojure.lang.PersistentQueue/EMPTY 1 2 3))"
        ),
        "(1 2 3)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [q (conj clojure.lang.PersistentQueue/EMPTY 1 2 3)] [(first q) (rest q) (nth q 1)])"
        ),
        "[1 (2 3) 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(map inc (conj clojure.lang.PersistentQueue/EMPTY 1 2 3))"
        ),
        "(2 3 4)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(nth (conj clojure.lang.PersistentQueue/EMPTY 1 2 3) 9 :nf)"
        ),
        ":nf"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(nth (conj clojure.lang.PersistentQueue/EMPTY 1 2 3) 9)"
        ),
        "java.lang.IndexOutOfBoundsException | "
    );
}

// q-eq/print/str: element-wise equality between queues and the
// `#object` print (identity scrubbed).
#[test]
fn queue_eq_print_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(= (conj clojure.lang.PersistentQueue/EMPTY 1 2 3) (conj clojure.lang.PersistentQueue/EMPTY 1 2 3))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (conj clojure.lang.PersistentQueue/EMPTY 1 2 3) (conj clojure.lang.PersistentQueue/EMPTY 1 2))"
        ),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= clojure.lang.PersistentQueue/EMPTY clojure.lang.PersistentQueue/EMPTY)"
        ),
        "true"
    );
    assert_eq!(
        normhex(&run(
            &mut eval,
            "(pr-str (conj clojure.lang.PersistentQueue/EMPTY 1 2 3))"
        )),
        "\"#object[clojure.lang.PersistentQueue 0xH \\\"(1 2 3)\\\"]\""
    );
    assert_eq!(
        normhex(&run(
            &mut eval,
            "(str (conj clojure.lang.PersistentQueue/EMPTY 1 2 3))"
        )),
        "\"#object[clojure.lang.PersistentQueue 0xH \\\"(1 2 3)\\\"]\""
    );
}

// q-lattice: the `PersistentQueue` row resolves with its probed
// superclass + interfaces as ordered bases.
#[test]
fn queue_lattice_row() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [c (clojure.core/__import* \"clojure.lang.PersistentQueue\")] [(.isInterface c) (vec (map #(.getName ^Class %) (bases c)))])"
        ),
        "[false [\"clojure.lang.Obj\" \"clojure.lang.IPersistentList\" \"java.util.Collection\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\"]]"
    );
}
