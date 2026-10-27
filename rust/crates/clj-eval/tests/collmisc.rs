// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S12 collections misc (issue #54): probe replay for the
//! structmap surface, the chunk machinery, `hash-combine` /
//! `mix-collection-hash`, `seque`, `iteration`, `file-seq`, and
//! the `re-matcher` / `re-groups` handle pair (see
//! `docs/zero-oos-s12-collmisc/probes/*.{clj,out}` — 199 pins).
//! Every `sm-*`/`ch-*`/`mh-*`/`sq-*`/`it-*`/`fs-*`/`rm-*` label
//! below names its oracle row; `(derived)` marks a port-test
//! assertion derived from an oracle pin rather than quoted by it.

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

/// `pr-str` of a string: wrap in quotes, escaping inner quotes
/// (oracle `.out` rows show this shape).
fn prs(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Normalize identity hex (`0x…` → `0xH`, `@…` → `@L`), exactly
/// like the oracle `normhex`.
fn norm_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            out.push_str("0xH");
            chars.next();
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else if c == '@' && chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push_str("@L");
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `Util/hash` of each evaluated form. The port's private
/// `hash-code` is unreachable from test code, so these rows pin
/// the same `hash_code` the `hash-combine` native composes over
/// (precedent: the `oracle_7g8` hash pins).
fn hash_codes(eval: &mut Evaluator, srcs: &[&str]) -> Vec<i32> {
    srcs.iter()
        .map(|s| {
            let vs = eval.eval_source(s).unwrap();
            clj_value::hash_code(&vs[0])
        })
        .collect()
}

/// The JVM `ClassCastException` text with module verbiage (same
/// rule as the port's `cce_message`).
fn cce(from: &str, to: &str) -> String {
    fn module(cls: &str) -> &'static str {
        if cls.starts_with("java.") {
            "module java.base of loader 'bootstrap'"
        } else {
            "unnamed module of loader 'app'"
        }
    }
    let (fm, tm) = (module(from), module(to));
    let msg = if fm == tm {
        format!("class {from} cannot be cast to class {to} ({from} and {to} are in {fm})")
    } else {
        format!("class {from} cannot be cast to class {to} ({from} is in {fm}; {to} is in {tm})")
    };
    format!("java.lang.ClassCastException | {msg}")
}

const DEF: &str = "clojure.lang.PersistentStructMap$Def";
const STRUCT: &str = "clojure.lang.PersistentStructMap";

fn struct_setup(eval: &mut Evaluator) {
    run(
        eval,
        "(defstruct person :name :age) (def other (create-struct :name :age))",
    );
}

#[test]
fn structmap_rows() {
    let mut eval = boot();
    struct_setup(&mut eval);
    // sm-basis-class is a `jvm_class_of` mapping in the port
    // (derived: `class` is N1) — pinned through the CCE
    // from-names below (sm-map-nonbasis and friends).
    // sm-basis-print
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(pr-str (create-struct :a :b))")),
        prs(&format!("#object[{DEF} 0xH \"{DEF}@L\"]"))
    );
    // sm-defstruct-expand
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (macroexpand-1 '(defstruct person2 :x :y)))"
        ),
        "\"(def person2 (clojure.core/create-struct :x :y))\""
    );
    // sm-defstruct-class (derived: basis answers the `$Def` CCE
    // from-name through `chunk-first`, and prints `$Def`).
    assert_eq!(
        uncaught(&mut eval, "(chunk-first person)"),
        cce(DEF, "clojure.lang.IChunkedSeq")
    );
    // sm-basis-eq
    assert_eq!(
        run_last(&mut eval, "(= (create-struct :a :b) (create-struct :a :b))"),
        "false"
    );
    // sm-make-print
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (struct-map person :name \"Ada\" :age 36))"
        ),
        prs("{:name \"Ada\", :age 36}")
    );
    // sm-make-class (derived: CCE from-name through `chunk-first`).
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (struct-map person :name \"Ada\"))"),
        cce(STRUCT, "clojure.lang.IChunkedSeq")
    );
    // sm-map?
    assert_eq!(run_last(&mut eval, "(map? (struct-map person))"), "true");
    // sm-eq-map
    assert_eq!(
        run_last(
            &mut eval,
            "(= (struct-map person :name \"Ada\" :age 36) {:name \"Ada\" :age 36})"
        ),
        "true"
    );
    // sm-hash-eq
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash (struct-map person :name \"Ada\" :age 36)) (hash {:name \"Ada\" :age 36}))"
        ),
        "true"
    );
    // sm-keys
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (keys (struct-map person :name \"Ada\")))"
        ),
        "\"(:name :age)\""
    );
    // sm-vals
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (vals (struct-map person :name \"Ada\")))"
        ),
        prs("(\"Ada\" nil)")
    );
    // sm-seq
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (seq (struct-map person :name \"Ada\")))"
        ),
        prs("([:name \"Ada\"] [:age nil])")
    );
    // sm-count
    assert_eq!(
        run_last(&mut eval, "(count (struct-map person :name \"Ada\"))"),
        "2"
    );
    // sm-get-basis
    assert_eq!(
        run_last(&mut eval, "(get (struct-map person :name \"Ada\") :age)"),
        "nil"
    );
    // sm-contains
    assert_eq!(
        run_last(&mut eval, "(contains? (struct-map person) :age)"),
        "true"
    );
    // sm-extra
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (struct-map person :name \"Ada\" :zzz 9))"
        ),
        prs("{:name \"Ada\", :age nil, :zzz 9}")
    );
    // sm-empty
    assert_eq!(
        run_last(&mut eval, "(pr-str (struct-map person))"),
        "\"{:name nil, :age nil}\""
    );
    // sm-cross-basis
    assert_eq!(
        run_last(
            &mut eval,
            "(= (struct-map person :name \"Ada\") (struct-map other :name \"Ada\"))"
        ),
        "true"
    );
    // sm-struct-print
    assert_eq!(
        run_last(&mut eval, "(pr-str (struct person \"Ada\" 36))"),
        prs("{:name \"Ada\", :age 36}")
    );
    // sm-struct-short
    assert_eq!(
        run_last(&mut eval, "(pr-str (struct person \"Ada\"))"),
        prs("{:name \"Ada\", :age nil}")
    );
    // sm-struct-extra
    assert_eq!(
        uncaught(&mut eval, "(struct person \"Ada\" 36 \"EXTRA\")"),
        "java.lang.IllegalArgumentException | Too many arguments to struct constructor"
    );
    // sm-assoc-print
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (assoc (struct-map person :name \"Ada\") :age 37))"
        ),
        prs("{:name \"Ada\", :age 37}")
    );
    // sm-assoc-class (derived: CCE from-name through `chunk-first`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(chunk-first (assoc (struct-map person :name \"Ada\") :age 37))"
        ),
        cce(STRUCT, "clojure.lang.IChunkedSeq")
    );
    // sm-assoc-extra
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (assoc (struct-map person :name \"Ada\") :zzz 1))"
        ),
        prs("{:name \"Ada\", :age nil, :zzz 1}")
    );
    // sm-dissoc-basis
    assert_eq!(
        uncaught(
            &mut eval,
            "(pr-str (dissoc (struct-map person :name \"Ada\" :age 1) :age))"
        ),
        "java.lang.RuntimeException | Can't remove struct key"
    );
    // sm-dissoc-extra
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (dissoc (struct-map person :name \"Ada\" :zzz 1) :zzz))"
        ),
        prs("{:name \"Ada\", :age nil}")
    );
    // sm-accessor
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (accessor person :name)] (a (struct-map person :name \"Ada\")))"
        ),
        "\"Ada\""
    );
    // sm-accessor-miss
    assert_eq!(
        uncaught(&mut eval, "(accessor person :nope)"),
        "java.lang.IllegalArgumentException | Not a key of struct"
    );
    // sm-accessor-plain
    assert_eq!(
        uncaught(&mut eval, "((accessor person :name) {:name \"Bob\"})"),
        cce("clojure.lang.PersistentArrayMap", STRUCT)
    );
    // sm-accessor-fn?
    assert_eq!(
        run_last(&mut eval, "(fn? (accessor person :name))"),
        "false"
    );
    // sm-accessor-class (derived: `jvm_class_of` is `…$1` —
    // `class` is N1 in the port; pinned through print + `ifn?`).
    assert_eq!(
        run_last(&mut eval, "(ifn? (accessor person :name))"),
        "true"
    );
    // sm-accessor-print
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(pr-str (accessor person :name))")),
        prs(&format!("#object[{STRUCT}$1 0xH \"{STRUCT}$1@L\"]"))
    );
    // sm-assoc-extra-class (derived: CCE from-name).
    assert_eq!(
        uncaught(
            &mut eval,
            "(chunk-first (assoc (struct-map person :name \"Ada\") :zzz 1))"
        ),
        cce(STRUCT, "clojure.lang.IChunkedSeq")
    );
    // sm-dissoc-extra-class (derived: CCE from-name).
    assert_eq!(
        uncaught(
            &mut eval,
            "(chunk-first (dissoc (struct-map person :name \"Ada\" :zzz 1) :zzz))"
        ),
        cce(STRUCT, "clojure.lang.IChunkedSeq")
    );
    // sm-empty-basis
    assert_eq!(
        uncaught(&mut eval, "(pr-str (struct-map (create-struct)))"),
        "java.lang.IllegalArgumentException | Must supply keys"
    );
    // sm-map-nonbasis
    assert_eq!(
        uncaught(&mut eval, "(struct-map {:a 0} :a 1)"),
        cce("clojure.lang.PersistentArrayMap", DEF)
    );
    // sm-struct-nonbasis
    assert_eq!(
        uncaught(&mut eval, "(struct \"x\" 1)"),
        cce("java.lang.String", DEF)
    );
    // sm-accessor-nonbasis
    assert_eq!(
        uncaught(&mut eval, "(accessor {:a 0} :a)"),
        cce("clojure.lang.PersistentArrayMap", DEF)
    );
    // (derived) `(seq basis)` throws the ISeq error naming `$Def`.
    assert_eq!(
        uncaught(&mut eval, "(seq person)"),
        format!("java.lang.IllegalArgumentException | Don't know how to create ISeq from: {DEF}")
    );
}

const CHUNK: &str = "clojure.lang.ArrayChunk";
const CHUNK_BUF: &str = "clojure.lang.ChunkBuffer";
const CHUNKED_CONS: &str = "clojure.lang.ChunkedCons";
const ICHUNKEDSEQ: &str = "clojure.lang.IChunkedSeq";

#[test]
fn chunk_rows() {
    let mut eval = boot();
    // ch-buf-class (derived: CCE from-name through `chunk-first`).
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (chunk-buffer 32))"),
        cce(CHUNK_BUF, ICHUNKEDSEQ)
    );
    // ch-buf-print
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(pr-str (chunk-buffer 32))")),
        prs(&format!("#object[{CHUNK_BUF} 0xH \"{CHUNK_BUF}@L\"]"))
    );
    // ch-chunk-class (derived: CCE from-name through `chunk-first`).
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (chunk (chunk-buffer 32)))"),
        cce(CHUNK, ICHUNKEDSEQ)
    );
    // ch-build
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b :a) (chunk-append b :b)
               (let [c (chunk b)] [(count c) (nth c 0) (nth c 1)]))"
        ),
        "[2 :a :b]"
    );
    // ch-print
    assert_eq!(
        norm_hex(&run_last(
            &mut eval,
            "(pr-str (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)))"
        )),
        prs(&format!("#object[{CHUNK} 0xH \"{CHUNK}@L\"]"))
    );
    // ch-empty-count
    assert_eq!(
        run_last(&mut eval, "(count (chunk (chunk-buffer 32)))"),
        "0"
    );
    // ch-nil
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b nil)
               (let [c (chunk b)] [(count c) (nil? (nth c 0))]))"
        ),
        "[1 true]"
    );
    // ch-overflow
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [b (chunk-buffer 2)] (chunk-append b 1) (chunk-append b 2) (chunk-append b 3))"
        ),
        "java.lang.ArrayIndexOutOfBoundsException | Index 2 out of bounds for length 2"
    );
    // ch-bigcap
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 64)] (dotimes [i 40] (chunk-append b i)) (count (chunk b)))"
        ),
        "40"
    );
    // ch-zero-cap
    assert_eq!(run_last(&mut eval, "(count (chunk (chunk-buffer 0)))"), "0");
    // ch-zero-append
    assert_eq!(
        uncaught(&mut eval, "(let [b (chunk-buffer 0)] (chunk-append b 1))"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 0 out of bounds for length 0"
    );
    // ch-neg-cap
    assert_eq!(
        uncaught(&mut eval, "(chunk-buffer -1)"),
        "java.lang.NegativeArraySizeException | -1"
    );
    // ch-seq
    assert_eq!(
        uncaught(
            &mut eval,
            "(seq (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)))"
        ),
        format!("java.lang.IllegalArgumentException | Don't know how to create ISeq from: {CHUNK}")
    );
    // ch-nth-oob
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (nth (chunk b) 5))"
        ),
        "nil"
    );
    // ch-nth-cap
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (nil? (nth (chunk b) 31)))"
        ),
        "true"
    );
    // ch-nth-cap-oob
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (nth (chunk b) 32))"
        ),
        "java.lang.ArrayIndexOutOfBoundsException | Index 32 out of bounds for length 32"
    );
    // ch-use-once
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1)
               (let [c (chunk b)] (chunk-append b 2) [(count c) (nth c 0) (nth c 1)]))"
        ),
        "java.lang.NullPointerException | Cannot store to object array because \"this.buffer\" is null"
    );
    // ch-chunk-twice
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)
               (let [c2 (chunk b)] [(count c2) (nil? (nth c2 0))]))"
        ),
        "java.lang.NullPointerException | Cannot load from object array because \"this.array\" is null"
    );
    // ch-append-ret
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] [(nil? (chunk-append b 1)) (count (chunk b))])"
        ),
        "[true 1]"
    );
    // ch-cons-empty (`identical?` is N1 out-of-scope in the
    // port — identity is derived two ways: a non-chunked rest
    // stays non-chunked, and a forced lazy rest keeps its memo
    // cell).
    assert_eq!(
        run_last(
            &mut eval,
            "(chunked-seq? (chunk-cons (chunk (chunk-buffer 32)) '(1 2)))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (map inc [1 2])] (doall r)
               (realized? (chunk-cons (chunk (chunk-buffer 32)) r)))"
        ),
        "true"
    );
    // ch-cons-print
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk-append b 2) (chunk b)) '(3)))"
        ),
        "\"(1 2 3)\""
    );
    // ch-cons-class (derived: the emulated cons is chunked and
    // seqable, so neither stock projector fails on it — the class
    // surfaces through the arithmetic `Number` cast).
    assert_eq!(
        uncaught(
            &mut eval,
            "(+ (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil) 1)"
        ),
        cce(CHUNKED_CONS, "java.lang.Number")
    );
    // ch-cons-nil-print
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil))"
        ),
        "\"(1)\""
    );
    // ch-first-vec
    assert_eq!(
        run_last(&mut eval, "(count (chunk-first (seq (vec (range 100)))))"),
        "32"
    );
    // ch-first-small
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chunk-first (seq [1 2 3]))] [(count c) (nth c 0) (nth c 2)])"
        ),
        "[3 1 3]"
    );
    // ch-rest-vec
    assert_eq!(
        run_last(&mut eval, "(pr-str (chunk-rest (seq (vec (range 40)))))"),
        "\"(32 33 34 35 36 37 38 39)\""
    );
    // ch-next-vec
    assert_eq!(
        run_last(&mut eval, "(pr-str (chunk-next (seq (vec (range 40)))))"),
        "\"(32 33 34 35 36 37 38 39)\""
    );
    // ch-rest-end
    assert_eq!(
        run_last(
            &mut eval,
            "(let [r (chunk-rest (seq [1]))] [(pr-str r) (nil? r)])"
        ),
        "[\"()\" false]"
    );
    // ch-next-end
    assert_eq!(run_last(&mut eval, "(nil? (chunk-next (seq [1])))"), "true");
    // ch-first-list
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (seq '(1 2)))"),
        cce("clojure.lang.PersistentList", ICHUNKEDSEQ)
    );
    // ch-first-str
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (seq \"ab\"))"),
        cce("clojure.lang.StringSeq", ICHUNKEDSEQ)
    );
    // ch-rest-cons
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (chunk-rest (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) '(2 3))))"
        ),
        "\"(2 3)\""
    );
    // ch-next-cons-end
    assert_eq!(
        run_last(
            &mut eval,
            "(nil? (chunk-next (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil)))"
        ),
        "true"
    );
    // ch-first-cons
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chunk-first (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 7) (chunk b)) '(8)))] [(count c) (nth c 0)])"
        ),
        "[1 7]"
    );
    // `chunked-seq?` battery.
    for (src, want) in [
        ("(chunked-seq? (seq [1 2 3]))", "true"),      // ch-cq-vec
        ("(chunked-seq? (seq '(1 2)))", "false"),      // ch-cq-list
        ("(chunked-seq? (seq {:a 1 :b 2}))", "false"), // ch-cq-map
        ("(chunked-seq? (seq #{1 2}))", "false"),      // ch-cq-set
        ("(chunked-seq? (seq \"ab\"))", "false"),      // ch-cq-str
        ("(chunked-seq? (seq (range 100)))", "true"),  // ch-cq-range
        ("(chunked-seq? (seq (repeat 5)))", "false"),  // ch-cq-repeat
        ("(chunked-seq? (seq (cycle [1])))", "false"), // ch-cq-cycle
        ("(chunked-seq? (seq (iterate inc 0)))", "false"), // ch-cq-iterate
        ("(chunked-seq? (lazy-seq [1 2]))", "false"),  // ch-cq-lazy
        ("(chunked-seq? (cons 1 [2]))", "false"),      // ch-cq-cons
        ("(chunked-seq? (concat [1] [2]))", "false"),  // ch-cq-concat
        ("(chunked-seq? (map inc [1 2]))", "false"),   // ch-cq-mapfn
        ("(chunked-seq? nil)", "false"),               // ch-cq-nil
        ("(chunked-seq? [1 2])", "false"),             // ch-cq-vecval
        (
            "(chunked-seq? (chunk-cons (let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b)) nil))",
            "true",
        ), // ch-cq-cons-chunk
        ("(chunked-seq? (take-last 2 (range 100)))", "true"), // ch-cq-takelast
        ("(chunked-seq? (drop 40 (range 100)))", "true"), // ch-cq-drop
        ("(chunked-seq? (rseq [1 2 3]))", "false"),    // ch-cq-rseq
        ("(chunked-seq? (seq (subvec [1 2 3 4] 1)))", "false"), // ch-cq-subvec
        ("(chunked-seq? (re-seq #\"a\" \"aba\"))", "false"), // ch-cq-reseq
        // ch-cq-treeseq is unrunnable: `tree-seq` is out-of-scope
        // (future work, another slice). Its shape — a lazy seq —
        // is covered by every other lazy row above.
        ("(chunked-seq? (range))", "false"), // ch-cq-range-inf
        ("(chunked-seq? (repeat 3 :x))", "false"), // ch-cq-repeat-n
        ("(chunked-seq? (seq (eduction (map inc) [1 2])))", "false"), // ch-cq-eduction
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
}

#[test]
fn mixhash_rows() {
    let mut eval = boot();
    // mh-mix-1-0 .. mh-mix-big
    for (src, want) in [
        ("(mix-collection-hash 1 0)", "-2017569654"),
        ("(mix-collection-hash 0 0)", "-15128758"),
        ("(mix-collection-hash 1 1)", "1978132887"),
        ("(mix-collection-hash 1 2)", "1734656966"),
        ("(mix-collection-hash -1 3)", "-196466786"),
        ("(mix-collection-hash 123456789 10)", "1485565228"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // mh-mix-max
    assert_eq!(
        uncaught(&mut eval, "(mix-collection-hash 9223372036854775807 1)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // mh-mix-vec
    assert_eq!(
        run_last(&mut eval, "(= (mix-collection-hash 1 0) (hash []))"),
        "true"
    );
    // mh-mix-set
    assert_eq!(
        run_last(&mut eval, "(= (mix-collection-hash 0 0) (hash #{}))"),
        "true"
    );
    // mh-mix-arity
    assert_eq!(
        uncaught(&mut eval, "(mix-collection-hash 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/mix-collection-hash"
    );
    // mh-mix-str
    assert_eq!(
        uncaught(&mut eval, "(mix-collection-hash \"a\" 1)"),
        cce("java.lang.String", "java.lang.Number")
    );
    // mh-mix-double
    assert_eq!(
        run_last(&mut eval, "(mix-collection-hash 1.9 1)"),
        "1978132887"
    );
    // mh-comb-formula (derived): the port has no `Util/` fns, so
    // the formula is pinned by its hand-verified literal —
    // `Util/hashCombine(0, Util/hash(:a))` is `mh-comb-0-kw`.
    assert_eq!(run_last(&mut eval, "(hash-combine 0 :a)"), "-626620958");
    // mh-comb-* value rows.
    for (src, want) in [
        ("(hash-combine 0 nil)", "-1640531527"),     // mh-comb-0-nil
        ("(hash-combine 0 0)", "-1640531527"),       // mh-comb-0-0
        ("(hash-combine 0 1)", "-1640531526"),       // mh-comb-0-1
        ("(hash-combine 1 1)", "-1640531461"),       // mh-comb-1-1
        ("(hash-combine 0 \"a\")", "-1640531430"),   // mh-comb-0-str
        ("(hash-combine 0 :a)", "-626620958"),       // mh-comb-0-kw
        ("(hash-combine 5 :a)", "-626620634"),       // mh-comb-5-kw
        ("(hash-combine 0 [1 2])", "-1640530533"),   // mh-comb-0-vec
        ("(hash-combine 0 {:a 1})", "-626620959"),   // mh-comb-0-map
        ("(hash-combine 0 #{1})", "-1640531526"),    // mh-comb-0-set
        ("(hash-combine -1 \"ab\")", "1640528486"),  // mh-comb-neg
        ("(hash-combine 12345 nil)", "-1639734210"), // mh-comb-big
        ("(hash-combine 0 1.5)", "-567313991"),      // mh-comb-double
        ("(hash-combine 0 \\a)", "-1640531430"),     // mh-comb-char
        ("(hash-combine 0 true)", "-1640530296"),    // mh-comb-bool
        ("(hash-combine 0 -0.0)", "506952121"),      // mh-comb-negzero
        ("(hash-combine 0 9223372036854775807)", "506952121"), // mh-comb-bigint
        ("(hash-combine 0 '(1 2))", "-1640530533"),  // mh-comb-list
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // mh-tower-hash (divergent, §2.4: the port hashes tower
    // values structurally — pinned port literals below).
    assert_eq!(
        run_last(&mut eval, "[(hash 5N) (hash 1/2) (hash 1.5M)]"),
        "[1740791543 1777801658 335926639]"
    );
    // mh-tower-util (divergent, §2.4: structural `Util/hash` —
    // pinned port literals below).
    assert_eq!(
        hash_codes(&mut eval, &["5N", "1/2", "1.5M"]),
        vec![528065744, -1524795076, -1547456877]
    );
    // mh-comb-bign / mh-comb-dec / mh-comb-ratio (divergent,
    // §2.4 — pinned port literals below).
    assert_eq!(run_last(&mut eval, "(hash-combine 0 5N)"), "-1112465783");
    assert_eq!(run_last(&mut eval, "(hash-combine 0 1.5M)"), "1106978892");
    assert_eq!(run_last(&mut eval, "(hash-combine 0 1/2)"), "1129640693");
    // mh-util-spot (JVM-identical — no tower involved).
    assert_eq!(
        hash_codes(&mut eval, &["[1 2]", ":a", "{:a 1}", "nil"]),
        vec![994, 1013910569, 1013910568, 0]
    );
    // mh-tower-agree
    assert_eq!(
        run_last(&mut eval, "[(= (hash 5) (hash 5N)) (= 5 5N)]"),
        "[true true]"
    );
    // mh-comb-seed-str
    assert_eq!(
        uncaught(&mut eval, "(hash-combine \"a\" 1)"),
        cce("java.lang.String", "java.lang.Number")
    );
    // mh-comb-bigseed
    assert_eq!(
        uncaught(&mut eval, "(hash-combine 1099511627776 1)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // mh-comb-arity
    assert_eq!(
        uncaught(&mut eval, "(hash-combine 1)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/hash-combine"
    );
}

#[test]
fn seque_rows() {
    let mut eval = boot();
    // sq-basic
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque [1 2 3])))"),
        "\"(1 2 3)\""
    );
    // sq-class (derived: CCE from-name through the arithmetic
    // `Number` cast — a seque is a `LazySeq`).
    assert_eq!(
        uncaught(&mut eval, "(+ (seque [1 2]) 1)"),
        cce("clojure.lang.LazySeq", "java.lang.Number")
    );
    // sq-cq
    assert_eq!(run_last(&mut eval, "(chunked-seq? (seque [1 2]))"), "false");
    // sq-nil-elems
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque [1 nil 3])))"),
        "\"(1 nil 3)\""
    );
    // sq-empty
    assert_eq!(run_last(&mut eval, "(pr-str (doall (seque [])))"), "\"()\"");
    // sq-empty-seq
    assert_eq!(run_last(&mut eval, "(nil? (seq (seque [])))"), "true");
    // sq-nil-input
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque nil)))"),
        "\"()\""
    );
    // sq-cap
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque 2 (range 10))))"),
        "\"(0 1 2 3 4 5 6 7 8 9)\""
    );
    // sq-queue (reference-only arg shape: queues don't exist in
    // the port — pinned as the plain capacity path it reduces to).
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque 2 (range 5))))"),
        "\"(0 1 2 3 4)\""
    );
    // sq-zero-cap
    assert_eq!(
        uncaught(&mut eval, "(doall (seque 0 [1]))"),
        "java.lang.IllegalArgumentException | "
    );
    // sq-take
    assert_eq!(
        run_last(&mut eval, "(pr-str (take 3 (seque (range 100))))"),
        "\"(0 1 2)\""
    );
    // sq-count
    assert_eq!(run_last(&mut eval, "(count (seque (range 50)))"), "50");
    // sq-inf
    assert_eq!(
        run_last(&mut eval, "(pr-str (take 5 (seque (map inc (range)))))"),
        "\"(1 2 3 4 5)\""
    );
    // sq-neg-cap
    assert_eq!(
        uncaught(&mut eval, "(doall (seque -1 [1]))"),
        "java.lang.IllegalArgumentException | "
    );
    // sq-cap-str
    assert_eq!(
        uncaught(&mut eval, "(doall (seque \"x\" [1]))"),
        cce("java.lang.String", "java.lang.Character")
    );
    // sq-cap-float
    assert_eq!(
        run_last(&mut eval, "(pr-str (doall (seque 2.9 (range 5))))"),
        "\"(0 1 2 3 4)\""
    );
}

#[test]
fn iteration_rows() {
    let mut eval = boot();
    // it-basic
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (seq (iteration (fn [k] (when (< k 3) (inc k))) :initk 0))))"
        ),
        "\"(1 2 3)\""
    );
    // it-take-inf
    assert_eq!(
        run_last(&mut eval, "(pr-str (take 3 (iteration inc :initk 0)))"),
        "\"(1 2 3)\""
    );
    // it-empty
    assert_eq!(
        run_last(&mut eval, "(nil? (seq (iteration (fn [_] nil))))"),
        "true"
    );
    // it-reduce-empty
    assert_eq!(
        run_last(&mut eval, "(reduce + 7 (iteration (fn [_] nil)))"),
        "7"
    );
    // it-reduce
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce + 0 (iteration (fn [k] (when (< k 3) (inc k))) :initk 0))"
        ),
        "6"
    );
    // it-reduced
    assert_eq!(
        run_last(
            &mut eval,
            "(reduce (fn [a v] (if (> v 1) (reduced a) (+ a v))) 0
               (iteration (fn [k] (when (< k 3) (inc k))) :initk 0))"
        ),
        "1"
    );
    // it-vf
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (seq (iteration (fn [page] (when (< page 3) {:page page :items [page]}))
               :initk 0 :vf :items :kf (fn [ret] (inc (:page ret)))))))"
        ),
        "\"([0] [1] [2])\""
    );
    // it-kf-stop
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (seq (iteration (fn [k] k) :initk 3 :kf (fn [k] (when (pos? k) (dec k)))))))"
        ),
        "\"(3 2 1 0)\""
    );
    // it-somef
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (doall (seq (iteration inc :initk 0 :somef (fn [r] (< r 3))))))"
        ),
        "\"(1 2)\""
    );
    // it-somef-skips
    assert_eq!(
        run_last(
            &mut eval,
            "(let [calls (atom [])]
               (doall (iteration inc :initk 0 :somef (fn [r] (< r 2))
                         :vf (fn [r] (swap! calls conj r) r)))
               @calls)"
        ),
        "[1]"
    );
    // it-seqable (derived: `instance?` is N1 — Seqable-ness is the
    // working `seq` above plus the reducible path below).
    assert_eq!(
        run_last(
            &mut eval,
            "(seq? (seq (iteration (fn [k] (when (< k 1) (inc k))) :initk 0)))"
        ),
        "true"
    );
    // it-reducible (derived: `instance?` is N1 — IReduceInit-ness
    // is `reduce` stepping through the failing return).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)]
               (reduce + 0 (iteration (fn [k] (swap! c inc) (when (< k 2) (inc k))) :initk 0))
               (= @c 3))"
        ),
        "true"
    );
    // it-cq
    assert_eq!(
        run_last(&mut eval, "(chunked-seq? (seq (iteration inc :initk 0)))"),
        "false"
    );
    // it-step-once
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)] [(nil? (seq (iteration (fn [_] (swap! c inc) nil)))) @c])"
        ),
        "[true 1]"
    );
    // it-take-count
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)]
               [(pr-str (take 2 (iteration (fn [_] (let [n (swap! c inc)] (when (<= n 5) n))) :initk nil))) @c])"
        ),
        "[\"(1 2)\" 2]"
    );
    // it-twice
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0) it (iteration (fn [_] (swap! c inc) nil))]
               [(pr-str (doall (seq it))) (pr-str (doall (seq it))) @c])"
        ),
        "[\"nil\" \"nil\" 2]"
    );
    // it-reduce-count
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (atom 0)]
               (reduce + 0 (iteration (fn [k] (swap! c inc) (when (< k 2) (inc k))) :initk 0))
               @c)"
        ),
        "3"
    );
    // it-unknown-opt
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 1 (iteration inc :initk 0 :bogus 1)))"
        ),
        "\"(1)\""
    );
}

const MATCHER: &str = "java.util.regex.Matcher";

#[test]
fn rematcher_rows() {
    let mut eval = boot();
    // rm-class (derived: CCE from-name through `chunk-first`).
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (re-matcher #\"a+\" \"aaab\"))"),
        cce(MATCHER, ICHUNKEDSEQ)
    );
    // rm-print
    assert_eq!(
        norm_hex(&run_last(
            &mut eval,
            "(pr-str (re-matcher #\"a+\" \"aaab\"))"
        )),
        prs(&format!(
            "#object[{MATCHER} 0xH \"{MATCHER}[pattern=a+ region=0,4 lastmatch=]\"]"
        ))
    );
    // rm-find-groups
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"(\\d+)-(\\d+)\" \"12-34\")] [(re-find m) (re-groups m)])"
        ),
        "[[\"12-34\" \"12\" \"34\"] [\"12-34\" \"12\" \"34\"]]"
    );
    // rm-find-plain
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"a+\" \"xxaay\")] [(re-find m) (re-groups m)])"
        ),
        "[\"aa\" \"aa\"]"
    );
    // rm-find-none
    assert_eq!(
        run_last(
            &mut eval,
            "(nil? (let [m (re-matcher #\"z+\" \"abc\")] (re-find m)))"
        ),
        "true"
    );
    // rm-groups-nomatch
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"z+\" \"abc\")] (re-find m) (re-groups m))"
        ),
        "java.lang.IllegalStateException | No match found"
    );
    // rm-groups-fresh
    assert_eq!(
        uncaught(&mut eval, "(re-groups (re-matcher #\"a\" \"a\"))"),
        "java.lang.IllegalStateException | No match found"
    );
    // rm-multi
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"a\" \"aba\")] [(re-find m) (re-find m) (re-find m)])"
        ),
        "[\"a\" \"a\" nil]"
    );
    // rm-groups-each
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"(a)(b)?\" \"ab a\")] [(re-find m) (re-groups m) (re-find m) (re-groups m)])"
        ),
        "[[\"ab\" \"a\" \"b\"] [\"ab\" \"a\" \"b\"] [\"a\" \"a\" nil] [\"a\" \"a\" nil]]"
    );
    // rm-exhaust
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"a\" \"a\")] [(re-find m) (re-find m) (re-find m)])"
        ),
        "[\"a\" nil nil]"
    );
    // rm-bad-pattern
    assert_eq!(
        uncaught(&mut eval, "(re-matcher \"a+\" \"aa\")"),
        cce("java.lang.String", "java.util.regex.Pattern")
    );
    // rm-bad-subject
    assert_eq!(
        uncaught(&mut eval, "(re-matcher #\"a\" 42)"),
        cce("java.lang.Long", "java.lang.CharSequence")
    );
    // rm-nil-subject
    assert_eq!(
        uncaught(&mut eval, "(re-matcher #\"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
    // rm-groups-bad
    assert_eq!(
        uncaught(&mut eval, "(re-groups \"nope\")"),
        cce("java.lang.String", MATCHER)
    );
    // rm-empty-advance
    assert_eq!(
        run_last(
            &mut eval,
            "(let [m (re-matcher #\"a*\" \"b\")] [(re-find m) (re-find m) (re-find m)])"
        ),
        "[\"\" \"\" nil]"
    );
}

/// A Rust string as a Clojure string literal.
fn clj_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[test]
fn fileseq_rows() {
    // Mirror the oracle fixture: root/a.txt, root/sub/b.txt,
    // root/sub/deep/c.txt, root/empty/.
    let root = std::env::temp_dir().join(format!("s12-collmisc-fileseq-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for d in ["", "/sub", "/sub/deep", "/empty"] {
        std::fs::create_dir_all(format!("{}{d}", root.display())).unwrap();
    }
    for f in ["/a.txt", "/sub/b.txt", "/sub/deep/c.txt"] {
        std::fs::write(format!("{}{f}", root.display()), &f[1..1]).unwrap();
    }
    let root_s = root.to_string_lossy().into_owned();
    let lit = clj_str(&root_s);
    let leaf = std::path::Path::new(&root_s)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let mut eval = boot();
    // fs-classes (divergent, §1.7: entries are strings, not
    // `java.io.File` — every walked entry answers `string?`).
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(pr-str (distinct (map string? (file-seq {lit}))))")
        ),
        "\"(true)\""
    );
    // fs-one (divergent, §1.7: the head is the root string).
    assert_eq!(
        run_last(&mut eval, &format!("(= (first (file-seq {lit})) {lit})")),
        "true"
    );
    // fs-count
    assert_eq!(
        run_last(&mut eval, &format!("(count (file-seq {lit}))")),
        "7"
    );
    // fs-names (leaf names through `re-seq`, like `.getName`).
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(pr-str (sort (map (fn [p] (last (re-seq #\"[^/]+\" p))) (file-seq {lit}))))"
            )
        ),
        prs(&format!(
            "(\"a.txt\" \"b.txt\" \"c.txt\" \"deep\" \"empty\" \"{leaf}\" \"sub\")"
        ))
    );
    // fs-root-first
    assert_eq!(
        run_last(&mut eval, &format!("(= (first (file-seq {lit})) {lit})")),
        "true"
    );
    // fs-cq
    assert_eq!(
        run_last(&mut eval, &format!("(chunked-seq? (file-seq {lit}))")),
        "false"
    );
    // fs-str-arg (*) — the documented divergence: the port
    // ACCEPTS the string root (the JVM throws its CCE here).
    assert_eq!(
        run_last(&mut eval, &format!("(count (file-seq {lit}))")),
        "7"
    );
    // fs-missing
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(count (file-seq (str {lit} \"-nope\")))")
        ),
        "1"
    );
    // fs-file-root
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(count (file-seq (str {lit} \"/a.txt\")))")
        ),
        "1"
    );
    // fs-empty-dir
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(count (file-seq (str {lit} \"/empty\")))")
        ),
        "1"
    );
    // fs-seq?
    assert_eq!(
        run_last(&mut eval, &format!("(seq? (file-seq {lit}))")),
        "true"
    );
    // (§1.7) a non-string arg throws the slurp-shaped IAE.
    assert_eq!(
        uncaught(&mut eval, "(file-seq 42)"),
        "java.lang.IllegalArgumentException | Cannot open <42> as a directory."
    );

    std::fs::remove_dir_all(&root).unwrap();
}
