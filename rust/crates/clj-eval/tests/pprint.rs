// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W3 (pprint) smoke tests: `require` of `clojure.pprint` plus
//! upstream-documented behavior of `pprint`/`write`, `cl-format`, and
//! `print-table` on representative inputs (issue #204, spec section 4).
//!
//! Expected outputs come from the upstream docstrings, the Common Lisp
//! HyperSpec/CLtL format semantics the docstrings cite, and hand-traces of
//! the upstream XP/pretty algorithms (no JVM anywhere). Known port
//! divergences pinned here:
//! - D1: error paths throw `ex-info` (`:pp/error` carries the upstream host
//!   class keyword) because host exception ctors are unavailable.
//! - D2: `~X`/`~16R` print lowercase (`ff`), mirroring upstream's `%x`
//!   (Common Lisp prints uppercase).
//! - D3: a custom dispatch fn calling `print`/`pr` directly hits a
//!   `ClassCastException`: the port `*out*` is an atom, not a Writer.
//!   Dispatch fns must use `write-out`/`pprint-logical-block` (as documented).

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    let s = pr_to_string(v, &PrintOpts::pr());
    // Windows only: pretty-printer line breaks and cl-format newline
    // directives emit the platform line separator, which `pr` escapes to
    // literal `\r\n` sequences. Canonicalize so expectations pin LF.
    // Unix builds compare byte-exact: a stray CR there is a real bug.
    #[cfg(windows)]
    let s = s.replace("\\r\\n", "\\n");
    s
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(clj_eval::Signal::Throw(t)) => panic!(
                "unexpected throw while printing: {} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            ),
            Err(other) => panic!("unexpected signal while printing: {other:?}"),
        })
        .collect()
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
}

/// Boot and require `clojure.pprint` as `pp` (the `binding` frame is for the
/// `set!` of `*warn-on-reflection*` during load, like the loader test).
fn boot_pp() -> Evaluator {
    let mut eval = boot();
    run_last(
        &mut eval,
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.pprint :as pp]))",
    );
    eval
}

#[test]
fn pprint_basics_and_write() {
    let mut eval = boot_pp();
    // Scalars and small collections print on one line plus a newline.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint {:a 1 :b [1 2 3]}))"),
        "\"{:a 1, :b [1 2 3]}\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint [1 2 3]))"),
        "\"[1 2 3]\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint nil))"),
        "\"nil\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint \"str\"))"),
        "\"\\\"str\\\"\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint 3/4))"),
        "\"3/4\\n\""
    );
    // BigInt keeps its N suffix (no radix/base rewriting at defaults).
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint 5N))"),
        "\"5N\\n\""
    );
    // Reader-macro back-translation.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint ''foo))"),
        "\"'foo\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint '(quote foo)))"),
        "\"'foo\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/pprint '(clojure.core/deref a)))"
        ),
        "\"@a\\n\""
    );
    // Lazy seqs dispatch as seqs.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint (map inc [1 2])))"),
        "\"(2 3)\\n\""
    );
    // Map entries print as vectors.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint (first {:a 1})))"),
        "\"[:a 1]\\n\""
    );
    // Sets and sorted maps.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint #{1}))"),
        "\"#{1}\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/pprint (sorted-map :b 2 :a 1)))"
        ),
        "\"{:a 1, :b 2}\\n\""
    );
    // Arrays use the vector-shaped pprint-array format.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint (int-array [1 2])))"),
        "\"[1, 2]\\n\""
    );
    // Records print as plain maps (no tag), like upstream.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defrecord PPR [x]) (with-out-str (pp/pprint (->PPR 1))))"
        ),
        "\"{:x 1}\\n\""
    );
    // pprint returns nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def pp-ret1 (pp/pprint 1))) pp-ret1"
        ),
        "nil"
    );

    // `write` to a nil stream returns the string with no trailing newline.
    assert_eq!(
        run_last(&mut eval, "(pp/write {:a 1} :stream nil)"),
        "\"{:a 1}\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/write [1 2] :stream nil :pretty false)"),
        "\"[1 2]\""
    );
    // An explicit writer stream writes there and returns nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def pp-ret4 (pp/write [1] :stream *out*))) pp-ret4"
        ),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/write [1] :stream *out*))"),
        "\"[1]\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/cl-format *out* \"~D\" 5))"),
        "\"5\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def pp-ret2 (pp/write [1] :stream true))) pp-ret2"
        ),
        "nil"
    );
    // `pp` pretty-prints `*1`.
    assert_eq!(
        run_last(&mut eval, "(binding [*1 42] (with-out-str (pp/pp)))"),
        "\"42\\n\""
    );
    // More scalars, empty collections, and another seq type.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint :a))"),
        "\":a\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint 'a))"),
        "\"a\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint \\a))"),
        "\"\\\\a\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint '()))"),
        "\"()\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint []))"),
        "\"[]\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint {}))"),
        "\"{}\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint #{}))"),
        "\"#{}\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/pprint (reverse [1 2])))"),
        "\"(2 1)\\n\""
    );
    // :refer brings the entry points in unqualified.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.pprint :refer [pprint]])) (with-out-str (pprint [1]))"
        ),
        "\"[1]\\n\""
    );
}

#[test]
fn pprint_line_breaking_and_limits() {
    let mut eval = boot_pp();
    // Narrow margin breaks linear newlines; continuation indents one space.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 10] (with-out-str (pp/pprint (vec (range 10)))))"
        ),
        "\"[0\\n 1\\n 2\\n 3\\n 4\\n 5\\n 6\\n 7\\n 8\\n 9]\\n\""
    );
    // A nil right margin never breaks.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* nil] (with-out-str (pp/pprint (vec (range 20)))))"
        ),
        "\"[0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19]\\n\""
    );
    // Broken output still reads back to the same value.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 10] (read-string (with-out-str (pp/pprint {:a (vec (range 8))}))))"
        ),
        "{:a [0 1 2 3 4 5 6 7]}"
    );
    // *print-length* truncates with ... and *print-level* with #.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-length* 2] (with-out-str (pp/pprint [1 2 3 4])))"
        ),
        "\"[1 2 ...]\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-level* 1] (with-out-str (pp/pprint [[1] [2]])))"
        ),
        "\"[# #]\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/write [1 [2 [3]]] :stream nil :level 2)"),
        "\"[1 [2 #]]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(pp/write (quote (1 2 3 4)) :stream nil :length 2)"
        ),
        "\"(1 2 ...)\""
    );
    // :right-margin as a write option (no trailing newline from write).
    assert_eq!(
        run_last(
            &mut eval,
            "(pp/write (vec (range 4)) :stream nil :right-margin 5)"
        ),
        "\"[0\\n 1\\n 2\\n 3]\""
    );
    // Nested maps break at the outer linear newlines; indentation accumulates.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 15] (with-out-str (pp/pprint {:aa {:bb {:cc 1}}})))"
        ),
        "\"{:aa\\n {:bb\\n  {:cc 1}}}\\n\""
    );
    // *print-miser-width* decides pair-internal (:miser) breaks in cond: the
    // default width breaks inside the pair, width 0 leaves it overflowing.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 12] (with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(cond aaaaaaaa bbbbbbbb)))))"
        ),
        "\"(cond\\n  aaaaaaaa\\n  bbbbbbbb)\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 12 pp/*print-miser-width* 0] (with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(cond aaaaaaaa bbbbbbbb)))))"
        ),
        "\"(cond\\n  aaaaaaaa bbbbbbbb)\\n\""
    );
}

#[test]
fn pprint_dispatch_and_options() {
    let mut eval = boot_pp();
    // Code dispatch formats defn and anonymous fn shorthand.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(defn foo [x] (+ x 1)))))"
        ),
        "\"(defn foo [x] (+ x 1))\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '#(inc %))))"
        ),
        "\"#(inc %)\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(let [a 1 b 2] (+ a b)))))"
        ),
        "\"(let [a 1 b 2] (+ a b))\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(ns foo \"doc\" (:require [clojure.string :as s])))))"
        ),
        "\"(ns foo\\n  \\\"doc\\\"\\n  (:require [clojure.string :as s]))\\n\""
    );
    // cond breaks at the outer linear newlines while the pairs fit.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-right-margin* 12] (with-out-str (pp/with-pprint-dispatch pp/code-dispatch (pp/pprint '(cond a b c d)))))"
        ),
        "\"(cond\\n  a b\\n  c d)\\n\""
    );
    // :dispatch as a write option.
    assert_eq!(
        run_last(
            &mut eval,
            "(pp/write '(defn f [x] x) :stream nil :dispatch pp/code-dispatch)"
        ),
        "\"(defn f [x] x)\""
    );
    // A custom dispatch fn built from the public logical-block API.
    assert_eq!(
        run_last(
            &mut eval,
            "(pp/write [1 2] :stream nil :dispatch (fn [o] (if (number? o) (pp/pprint-logical-block :prefix \"<\" :suffix \">\" (pp/simple-dispatch o)) (pp/simple-dispatch o))))"
        ),
        "\"[<1> <2>]\""
    );
    // The dispatch tables stay open: a user defmethod takes effect once the
    // dispatch var is refreshed (defmethod is copy-on-write here, so a
    // *print-pprint-dispatch* captured earlier never sees late methods; the
    // remove-method in finally leaves later asserts unaffected).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defmethod pp/simple-dispatch clojure.lang.Keyword [k] (pp/write-out \"KW!\")) (pp/set-pprint-dispatch pp/simple-dispatch) (try (pp/write :ab :stream nil) (finally (remove-method pp/simple-dispatch clojure.lang.Keyword) (pp/set-pprint-dispatch pp/simple-dispatch))))"
        ),
        "\"\\\"KW!\\\"\""
    );
    // set-pprint-dispatch swaps the root dispatch (restored after).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (pp/set-pprint-dispatch pp/code-dispatch) (try (pp/write '(def x 1) :stream nil) (finally (pp/set-pprint-dispatch pp/simple-dispatch))))"
        ),
        "\"(def x 1)\""
    );
    // Namespace-map lifting follows *print-namespace-maps*.
    assert_eq!(
        run_last(&mut eval, "(pp/write {:a/x 1 :a/y 2} :stream nil)"),
        "\"#:a{:x 1, :y 2}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-namespace-maps* false] (pp/write {:a/x 1 :a/y 2} :stream nil))"
        ),
        "\"{:a/x 1, :a/y 2}\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/write {:a/x 1 :b/y 2} :stream nil)"),
        "\"{:a/x 1, :b/y 2}\""
    );
    // Derefables print the #object tag plus the deref-as-map.
    let atom_out = run_last(&mut eval, "(with-out-str (pp/pprint (atom 1)))");
    assert!(
        atom_out.starts_with("\"#object[clojure.lang.Atom 0x"),
        "atom tag: {atom_out}"
    );
    assert!(
        atom_out.ends_with("{:status :ready, :val 1}]\\n\""),
        "atom map: {atom_out}"
    );
    let pending = run_last(&mut eval, "(with-out-str (pp/pprint (promise)))");
    assert!(pending.starts_with("\"#object["), "promise tag: {pending}");
    assert!(
        pending.contains(":status :pending"),
        "promise pending: {pending}"
    );
    let delay_pending = run_last(&mut eval, "(with-out-str (pp/pprint (delay 1)))");
    assert!(
        delay_pending.contains("{:status :pending"),
        "delay pending: {delay_pending}"
    );
    let delay_ready = run_last(
        &mut eval,
        "(let [d (delay 5)] @d (with-out-str (pp/pprint d)))",
    );
    assert!(
        delay_ready.contains("{:status :ready, :val 5}"),
        "delay ready: {delay_ready}"
    );
    // Metadata prints under *print-meta*.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (binding [*print-meta* true] (pp/pprint (with-meta [1] {:a 1}))))"
        ),
        "\"^{:a 1} [1]\\n\""
    );
    // Suppress-namespaces strips symbol namespaces.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-suppress-namespaces* true] (with-out-str (pp/pprint 'foo/bar)))"
        ),
        "\"bar\\n\""
    );
    // Base/radix printing.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-base* 16] (pp/write 255 :stream nil))"
        ),
        "\"ff\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-base* 16 pp/*print-radix* true] (pp/write 255 :stream nil))"
        ),
        "\"#xff\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-base* 8] (with-out-str (pp/pprint [8 9])))"
        ),
        "\"[10 11]\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-base* 2] (pp/write 5 :stream nil :pretty false))"
        ),
        "\"101\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-base* 16] (pp/write 3/4 :stream nil))"
        ),
        "\"3/4\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [pp/*print-radix* true] (pp/write 5 :stream nil))"
        ),
        "\"5.\""
    );
    // get-pretty-writer reuses a pretty writer and preserves columns across calls.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [w (pp/get-pretty-writer *out*)] (identical? w (pp/get-pretty-writer w)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (binding [*out* (pp/get-pretty-writer *out*)] (pp/cl-format true \"~4D\" 1) (pp/cl-format true \"~4D\" 2)))"
        ),
        "\"   1   2\""
    );
    // The logical-block/newline/indent building blocks, used directly. A bare
    // *out* binding needs an explicit buffer flush at the end (upstream too).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (binding [*out* (pp/get-pretty-writer *out*)] (pp/pprint-logical-block :prefix \"[\" :suffix \"]\" (pp/write-out 1) (#'pp/pp-write *out* \" \") (pp/pprint-newline :linear) (pp/write-out 2)) (#'pp/pp-flush *out*)))"
        ),
        "\"[1 2]\""
    );
}

#[test]
fn cl_format_integers_chars_and_text() {
    let mut eval = boot_pp();
    // The docstring example, verbatim.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [results [46 38 22]] (pp/cl-format nil \"There ~[are~;is~:;are~]~:* ~d result~:p: ~{~d~^, ~}~%\" (count results) results))"
        ),
        "\"There are 3 results: 46, 38, 22\\n\""
    );
    // Integer directives, padding, and grouping.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D\" 42)"),
        "\"42\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~3D\" 42)"),
        "\" 42\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~3,'0D\" 42)"),
        "\"042\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~VD\" 4 42)"),
        "\"  42\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~#D\" 42)"),
        "\"42\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:D\" 1234567)"),
        "\"1,234,567\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~,,'_:D\" 1234567)"),
        "\"1_234_567\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~B\" 5)"),
        "\"101\""
    );
    assert_eq!(run_last(&mut eval, "(pp/cl-format nil \"~O\" 8)"), "\"10\"");
    // D2: lowercase hex, mirroring upstream's %x (CL prints uppercase).
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~X\" 255)"),
        "\"ff\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~16R\" 255)"),
        "\"ff\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~X\" 255N)"),
        "\"ff\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~2R\" 5)"),
        "\"101\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~2:R\" 255)"),
        "\"11,111,111\""
    );
    // Non-integral numbers fall back to ~A-ish printing.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D\" 1.5)"),
        "\"1.5\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D\" 3/2)"),
        "\"3/2\""
    );
    // English cardinals/ordinals and roman numerals.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~R\" 5)"),
        "\"five\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~R\" -5)"),
        "\"minus five\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:R\" 2)"),
        "\"second\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@R\" 4)"),
        "\"IV\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:@R\" 4)"),
        "\"IIII\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@R\" 4000)"),
        "\"4,000\""
    );
    // ~A/~S padding and justification.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~10A\" \"ab\")"),
        "\"ab        \""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~10@A\" \"ab\")"),
        "\"        ab\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~S\" \"xy\")"),
        "\"\\\"xy\\\"\""
    );
    assert_eq!(run_last(&mut eval, "(pp/cl-format nil \"~A\" 5N)"), "\"5\"");
    // Characters: plain, pretty (:), and readable (@).
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~C\" \\a)"),
        "\"a\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:C\" \\tab)"),
        "\"Tab\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@C\" \\a)"),
        "\"\\\\a\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~'o@C\" \\a)"),
        "\"\\\\o141\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~'u@C\" \\a)"),
        "\"\\\\u0061\""
    );
    // Plurals.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D item~:P\" 1)"),
        "\"1 item\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D item~:P\" 2)"),
        "\"2 items\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D bab~:@P\" 1)"),
        "\"1 baby\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D bab~:@P\" 2)"),
        "\"2 babies\""
    );
    // Conditionals.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~[zero~;one~;two~]\" 1)"),
        "\"one\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~1[zero~;one~]\" 99)"),
        "\"one\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:[false~;true~]\" nil)"),
        "\"false\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@[~D~]\" nil)"),
        "\"\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@[~D~]\" 5)"),
        "\"5\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"a~@[b~]c\" true)"),
        "\"abc\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"a~@[b~]c\" false)"),
        "\"ac\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~[a~:;b~]\" 5)"),
        "\"b\""
    );
    // Iteration.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~{~D~^, ~}\" [1 2 3])"),
        "\"1, 2, 3\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~2{~D~}\" [1 2 3])"),
        "\"12\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(pp/cl-format nil \"~:{ ~D ~D ~}\" [[1 2] [3 4]])"
        ),
        "\" 1 2  3 4 \""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~{~}\" \"~D\" [1 2])"),
        "\"12\""
    );
    // Argument navigation and indirect formats.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D~*~D\" 1 2 3)"),
        "\"13\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D~:*~D\" 1 2)"),
        "\"11\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~D~@*~D\" 1 2 3)"),
        "\"11\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~?\" \"~D+~D\" [1 2])"),
        "\"1+2\""
    );
    // Literal newline/formfeed/tilde and ~%.
    assert_eq!(run_last(&mut eval, "(pp/cl-format nil \"~~\")"), "\"~\"");
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~5~\" )"),
        "\"~~~~~\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~2%\" )"),
        "\"\\n\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"a~|b\")"),
        "\"a\\fb\""
    );
    assert_eq!(run_last(&mut eval, "(pp/cl-format nil \"\")"), "\"\"");
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~{~D~}\" [])"),
        "\"\""
    );
    // Case conversion.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~(hello WORLD~)\")"),
        "\"hello world\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:(hello WORLD~)\")"),
        "\"Hello World\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~@(hello WORLD~)\")"),
        "\"Hello world\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~:@(hello WORLD~)\")"),
        "\"HELLO WORLD\""
    );
    // The word-capitalizer, pinned directly (first-word flag on and off).
    assert_eq!(
        run_last(&mut eval, "(#'pp/capitalize-string \"hello-world x\" true)"),
        "\"Hello-World X\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(#'pp/capitalize-string \"hello-world x\" false)"
        ),
        "\"hello-World X\""
    );
    // Stream true writes to *out* and returns nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def pp-ret3 (pp/cl-format true \"~D\" 7))) pp-ret3"
        ),
        "nil"
    );
    // formatter builds reusable format fns.
    assert_eq!(
        run_last(&mut eval, "((pp/formatter \"~D\") nil 42)"),
        "\"42\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str ((pp/formatter \"~A~A\") true \"x\" \"y\"))"
        ),
        "\"xy\""
    );
}

#[test]
fn cl_format_floats_pretty_and_tables() {
    let mut eval = boot_pp();
    // Fixed, exponential, general, and dollar floats.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" 3.14)"),
        "\"3.14\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~,2F\" 3.14159)"),
        "\"3.14\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~6,2F\" 3.14159)"),
        "\"  3.14\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~3,2,,'*F\" 3.14159)"),
        "\"***\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~E\" 123.456)"),
        "\"1.23456E+2\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~E\" 0.000123)"),
        "\"1.23E-4\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~4,,,,'*E\" 123.456)"),
        "\"****\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~G\" 3.14)"),
        "\"3.14    \""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~$\" 3.14159)"),
        "\"3.14\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~$\" 5)"),
        "\"5.00\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~2,1,8$\" -5)"),
        "\"   -5.00\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" 3.14M)"),
        "\"3.14\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" 1/2)"),
        "\"0.5\""
    );
    // Infinities spell like upstream (normalized from ##Inf).
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" ##Inf)"),
        "\"infinity.0\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" ##-Inf)"),
        "\"-infinity.0\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~F\" ##NaN)"),
        "\"nan.0\""
    );
    // ~W writes through the pretty printer.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~W\" [1 2])"),
        "\"[1 2]\""
    );
    // Logical blocks: prefix/suffix clauses, newlines, and indentation.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~<~W ~_~W~:>\" '(1 2))"),
        "\"1 2\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~<[~;~W~;]~:>\" [1])"),
        "\"[1]\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~<~W~:@_~W~:>\" '(1 2))"),
        "\"1\\n2\""
    );
    // Justification.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~10<~A~;~A~>\" \"a\" \"b\")"),
        "\"a        b\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~<aaa~;bb~>\")"),
        "\"aaabb\""
    );
    // Fresh-line and tabulation are column-aware.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"a~&b\")"),
        "\"a\\nb\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"a~2&b\")"),
        "\"a\\n\\nb\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~A~10T~A\" \"a\" \"b\")"),
        "\"a         b\""
    );
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~A~@T~A\" \"a\" \"b\")"),
        "\"a b\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/fresh-line))"),
        "\"\\n\""
    );

    // print-table.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (pp/print-table [:a :b] [{:a 1 :b 2} {:a 10 :b 20}]))"
        ),
        "\"\\n| :a | :b |\\n|----+----|\\n|  1 |  2 |\\n| 10 | 20 |\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/print-table [{:a 1}]))"),
        "\"\\n| :a |\\n|----|\\n|  1 |\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pp/print-table [] []))"),
        "\"\""
    );
}

#[test]
fn pprint_errors() {
    let mut eval = boot_pp();
    // D1: the ex-info message matches upstream; :pp/error names the class.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/cl-format nil \"~Q\") (catch Exception e [(ex-message e) (:pp/error (ex-data e))]))"
        ),
        "[\"Directive \\\"Q\\\" is undefined\\n~Q\\n ^\\n\" :runtime-exception]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/cl-format nil \"~D\") (catch Exception e [(ex-message e) (:pp/error (ex-data e))]))"
        ),
        "[\"Not enough arguments for format definition\" :exception]"
    );
    // Extra args are ignored; mincol still pads.
    assert_eq!(
        run_last(&mut eval, "(pp/cl-format nil \"~5D\" 1 2)"),
        "\"    1\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/pprint-newline :bogus) (catch Exception e (ex-message e)))"
        ),
        "\"Bad argument: :bogus. It must be one of #{:linear :miser :fill :mandatory}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/pprint-indent :bogus 1) (catch Exception e (ex-message e)))"
        ),
        "\"Bad argument: :bogus. It must be one of #{:block :current}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/pprint-tab :line 1 1) (catch Exception e [(ex-message e) (:pp/error (ex-data e))]))"
        ),
        "[\"pprint-tab is not yet implemented\" :unsupported-operation]"
    );
    // Too many directive params raise a format error.
    let too_many = run_last(
        &mut eval,
        "(try (pp/cl-format nil \"~1,2,3,4,5D\" 4) (catch Exception e (ex-message e)))",
    );
    assert!(
        too_many.contains("Too many parameters"),
        "too many params: {too_many}"
    );
    // A misplaced justify else-clause raises the upstream placement error.
    let misplaced = run_last(
        &mut eval,
        "(try (pp/cl-format nil \"~<a~;b~:;c~>\") (catch Exception e (ex-message e)))",
    );
    assert!(
        misplaced.contains("only allowed in the first position"),
        "misplaced else: {misplaced}"
    );
    // A leading else-clause without column params reads the pretty writer's
    // column (nil there), throwing exactly like upstream's NPE shape.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (pp/cl-format nil \"~10<aaa~:;bb~>\") (catch Exception e :threw))"
        ),
        ":threw"
    );
    // Reloading the namespace works and keeps behavior.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.pprint :as pp] :reload)) (pp/write [1] :stream nil)"
        ),
        "\"[1]\""
    );
}
