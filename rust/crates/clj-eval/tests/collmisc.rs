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
//! `docs/zero-oos-s12-collmisc/probes/*.{clj,out}` — 228 pins).
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
    // sm-defstruct-class (derived: the `defstruct` value answers
    // the `$Def` CCE from-name through `chunk-first`; the
    // `#object[…$Def …]` print shape itself is pinned for
    // `create-struct` values by sm-basis-print).
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
    // sm-accessor-arity0
    assert_eq!(
        uncaught(&mut eval, "((accessor person :name))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.lang.PersistentStructMap/1"
    );
    // sm-accessor-arity2
    assert_eq!(
        uncaught(
            &mut eval,
            "((accessor person :name) (struct-map person :name \"Ada\") (struct-map person :name \"Bo\"))"
        ),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.lang.PersistentStructMap/1"
    );
    // sm-accessor-nil
    assert_eq!(
        uncaught(&mut eval, "((accessor person :name) nil)"),
        "java.lang.NullPointerException | Cannot read field \"def\" because \"m\" is null"
    );
    // sm-accessor-mismatch
    assert_eq!(
        uncaught(
            &mut eval,
            "((accessor person :name) (struct-map other :name \"X\" :age 1))"
        ),
        "java.lang.RuntimeException | Accessor/struct mismatch"
    );
    // Coverage pins (JVM-verified in the R1 fixup round; no oracle
    // rows): odd inits fail the key supply, a struct instance is
    // not a basis.
    assert_eq!(
        uncaught(&mut eval, "(struct-map person :name)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :name"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(struct-map (struct-map person :name \"A\") :a 1)"
        ),
        cce(STRUCT, DEF)
    );
    // Coverage pins (no oracle rows): structs work as map keys
    // and set members, interchangeable with equal plain maps
    // (JVM-verified).
    assert_eq!(
        run_last(
            &mut eval,
            "(get {(struct-map person :name \"Ada\") :f} {:name \"Ada\" :age nil})"
        ),
        ":f"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(get {(struct-map person :name \"Ada\") :f} (struct-map person :name \"Ada\" :age nil))"
        ),
        ":f"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(get {(struct-map person :name \"Ada\") :f} [:not :map])"
        ),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(contains? #{(struct-map person :name \"Ada\")} {:name \"Ada\" :age nil})"
        ),
        "true"
    );
    // Coverage pins (no oracle rows): a struct never `=` a
    // non-map collection, in either order (JVM-verified).
    assert_eq!(
        run_last(
            &mut eval,
            "(= (struct-map person :name \"Ada\") [:not :map])"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= [:not :map] (struct-map person :name \"Ada\"))"
        ),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(= (struct-map person :name \"Ada\") 1)"),
        "false"
    );
    // Coverage pins (no oracle rows): accessors, buffers, and
    // chunks compare by identity (like their JVM objects).
    assert_eq!(
        run_last(
            &mut eval,
            "(= (accessor person :name) (accessor person :name))"
        ),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (accessor person :name)))"),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (accessor person :name))"),
        cce(
            "clojure.lang.PersistentStructMap$1",
            "clojure.lang.IChunkedSeq"
        )
    );
    assert_eq!(
        run_last(&mut eval, "(= (chunk-buffer 4) (chunk-buffer 4))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (chunk-buffer 4)))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (chunk-buffer 4) b (chunk-buffer 4)] (= (chunk a) (chunk b)))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 4)] (chunk-append b 1) (let [c1 (chunk b) c2 (chunk b) c3 (chunk b)] [(= c2 c3) (= c1 c2) (integer? (hash c1)) (integer? (hash c2))]))"
        ),
        "[false false true true]"
    );
    // Coverage pins (no oracle rows): extension values hash by
    // identity, so distinct objects are distinct map keys
    // (JVM-verified).
    for (src, want) in [
        (
            "(count {(accessor person :name) 1, (accessor person :age) 2})",
            "2",
        ),
        ("(count {(chunk-buffer 4) 1, (chunk-buffer 5) 2})", "2"),
        (
            "(let [a (chunk-buffer 4)] (chunk-append a 1) (let [x (chunk-buffer 4)] (count {(chunk a) 1, (chunk x) 2})))",
            "2",
        ),
        (
            "(let [a (chunk-buffer 4)] (chunk-append a 1) (count {(chunk a) 1, (chunk (do a)) 2}))",
            "2",
        ),
        ("(count {(iteration inc) 1, (iteration dec) 2})", "2"),
        (
            "(count {(re-matcher #\"a\" \"a\") 1, (re-matcher #\"b\" \"b\") 2})",
            "2",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // R1 walk-path regressions (JVM-verified in the fixup round;
    // no oracle rows): seq-stepping fns walk struct content.
    run(
        &mut eval,
        "(def sm9 (struct-map person :name \"Ada\" :age 36 :zzz 9))",
    );
    for (src, want) in [
        ("(pr-str (last sm9))", "\"[:zzz 9]\""),
        (
            "(pr-str (reverse sm9))",
            "\"([:zzz 9] [:age 36] [:name \\\"Ada\\\"])\"",
        ),
        (
            "(pr-str (butlast sm9))",
            "\"([:name \\\"Ada\\\"] [:age 36])\"",
        ),
        ("(pr-str (map first sm9))", "\"(:name :age :zzz)\""),
        ("(pr-str (take 1 sm9))", "\"([:name \\\"Ada\\\"])\""),
        ("(pr-str (drop 1 sm9))", "\"([:age 36] [:zzz 9])\""),
        (
            "(pr-str (sort sm9))",
            "\"([:age 36] [:name \\\"Ada\\\"] [:zzz 9])\"",
        ),
        (
            "(pr-str (apply list sm9))",
            "\"([:name \\\"Ada\\\"] [:age 36] [:zzz 9])\"",
        ),
        (
            "(pr-str (vec sm9))",
            "\"[[:name \\\"Ada\\\"] [:age 36] [:zzz 9]]\"",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // R1 `(empty struct)` regressions (JVM-verified; no oracle
    // rows): same basis nil-defaulted, extras dropped, meta
    // dropped, still dissoc-guarded.
    assert_eq!(
        run_last(&mut eval, "(pr-str (empty sm9))"),
        "\"{:name nil, :age nil}\""
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (empty sm9))"),
        cce(STRUCT, "clojure.lang.IChunkedSeq")
    );
    assert_eq!(run_last(&mut eval, "(count (empty sm9))"), "2");
    assert_eq!(
        run_last(&mut eval, "(= (empty sm9) {:name nil :age nil})"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str (meta (empty (with-meta sm9 {:a 1}))))"),
        "\"nil\""
    );
    assert_eq!(
        uncaught(&mut eval, "(dissoc (empty sm9) :age)"),
        "java.lang.RuntimeException | Can't remove struct key"
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str (dissoc (empty sm9) :zzz))"),
        "\"{:name nil, :age nil}\""
    );
    // R1 interop regressions (JVM-verified; no oracle rows):
    // `hashCode` is the content map's Java hash, `toString` is
    // the readable map print (which single-arg `str` shares).
    // The JVM pins the same int for the plain map
    // (`w-hashcode-eq-map`); `.hashCode` on non-host values is a
    // pre-existing gap (see `rec_hash_code_matches_map`), so the
    // map half is not re-proven here.
    assert_eq!(run_last(&mut eval, "(.hashCode sm9)"), "-1023392294");
    assert_eq!(
        run_last(&mut eval, "(= (.toString sm9) (pr-str sm9))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(= (str sm9) (.toString sm9))"), "true");
    // R1 protocol regressions (JVM-verified; no oracle rows):
    // structs dispatch through the `IPersistentMap` entry and
    // satisfy there; the struct class still answers `extends?`
    // false; a direct class extension hits.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol SMP (smp-foo [x])) (extend-protocol SMP clojure.lang.IPersistentMap (smp-foo [x] :map-impl)) (smp-foo (struct-map person :name \"Ada\")))"
        ),
        ":map-impl"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(satisfies? SMP (struct-map person :name \"Ada\"))"
        ),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(extends? SMP person)"), "false");
    assert_eq!(
        run_last(
            &mut eval,
            "(extends? SMP \"clojure.lang.PersistentStructMap\")"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defprotocol SMQ (smq-foo [x])) (extend-protocol SMQ clojure.lang.PersistentStructMap (smq-foo [x] :struct-impl)) (smq-foo (struct-map person :name \"Ada\")))"
        ),
        ":struct-impl"
    );
    // (derived) `(seq basis)` throws the ISeq error naming `$Def`.
    assert_eq!(
        uncaught(&mut eval, "(seq person)"),
        format!("java.lang.IllegalArgumentException | Don't know how to create ISeq from: {DEF}")
    );
    // sm-ifn
    assert_eq!(
        run_last(
            &mut eval,
            "(ifn? (struct-map person :name \"Ada\" :age 36 :zzz 9))"
        ),
        "true"
    );
    // sm-call
    assert_eq!(
        run_last(
            &mut eval,
            "((struct-map person :name \"Ada\" :age 36 :zzz 9) :age)"
        ),
        "36"
    );
    // sm-call-nf
    assert_eq!(
        run_last(
            &mut eval,
            "((struct-map person :name \"Ada\" :age 36 :zzz 9) :missing :nf)"
        ),
        ":nf"
    );
    // sm-call-arity0
    assert_eq!(
        uncaught(
            &mut eval,
            "((struct-map person :name \"Ada\" :age 36 :zzz 9))"
        ),
        format!("clojure.lang.ArityException | Wrong number of args (0) passed to: {STRUCT}")
    );
    // sm-call-arity3
    assert_eq!(
        uncaught(
            &mut eval,
            "((struct-map person :name \"Ada\" :age 36 :zzz 9) :a :b :c)"
        ),
        format!("clojure.lang.ArityException | Wrong number of args (3) passed to: {STRUCT}")
    );
    // sm-apply
    assert_eq!(
        run_last(
            &mut eval,
            "(apply (struct-map person :name \"Ada\" :age 36 :zzz 9) [:age])"
        ),
        "36"
    );
    // sm-mapfn
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (map (struct-map person :name \"Ada\" :age 36 :zzz 9) [:age :name :zzz :nope :nf-x]))"
        ),
        "\"(36 \\\"Ada\\\" 9 nil nil)\""
    );
    // (derived) records stay non-`IFn` — pinned by
    // `types.rs rec_invoke_not_supported` (the `is_ifn` change is
    // struct-only).
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
    // ch-buf-count-fresh
    assert_eq!(run_last(&mut eval, "(count (chunk-buffer 4))"), "0");
    // ch-buf-count-used
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 4)] (chunk-append b 1) (count b))"
        ),
        "1"
    );
    // ch-buf-count-dead
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 4)] (chunk-append b 1) (chunk b) (count b))"
        ),
        "1"
    );
    // ch-buf-count-dead0
    assert_eq!(
        run_last(&mut eval, "(let [b (chunk-buffer 4)] (chunk b) (count b))"),
        "0"
    );
    // ch-buf-empty
    assert_eq!(run_last(&mut eval, "(empty? (chunk-buffer 4))"), "true");
    // ch-buf-counted
    assert_eq!(run_last(&mut eval, "(counted? (chunk-buffer 4))"), "true");
    // (derived) a used buffer is not empty (the count path).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 4)] (chunk-append b 1) (empty? b))"
        ),
        "false"
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
    // Coverage pins (JVM-verified in the R1 fixup round; no oracle
    // rows): wrong-type args take the cast errors.
    assert_eq!(
        uncaught(&mut eval, "(chunk-append 1 2)"),
        cce("java.lang.Long", CHUNK_BUF)
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk 1)"),
        cce("java.lang.Long", CHUNK_BUF)
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-cons [1 2] '(3))"),
        cce("clojure.lang.PersistentVector", "clojure.lang.IChunk")
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-rest 1)"),
        cce("java.lang.Long", ICHUNKEDSEQ)
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-next 1)"),
        cce("java.lang.Long", ICHUNKEDSEQ)
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (map inc [1 2]))"),
        cce("clojure.lang.LazySeq", ICHUNKEDSEQ)
    );
    // Coverage pins (no oracle rows): 3-arity `nth` reads hits
    // and answers not-found past the count or below zero.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (let [c (chunk b)] [(nth c 0 :nf) (nth c 99 :nf) (nth c -1 :nf)]))"
        ),
        "[1 :nf :nf]"
    );
    // Coverage pins (no oracle rows): `nth` on a degenerate chunk
    // raises the same array-load NPE `count` does.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (chunk b) (nth (chunk b) 0))"
        ),
        "java.lang.NullPointerException | Cannot load from object array because \"this.array\" is null"
    );
    // Coverage pins (no oracle rows): projectors over bounded
    // ranges (head chunk, rest walk, end, overflow cut, inexact
    // step error).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chunk-first (range 100))] [(count c) (nth c 0) (nth c 31)])"
        ),
        "[32 0 31]"
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str (take 3 (chunk-rest (range 100))))"),
        "\"(32 33 34)\""
    );
    assert_eq!(run_last(&mut eval, "(nil? (chunk-next (range 3)))"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [c (chunk-first (range 5 9223372036854775807 9223372036854775807))] [(count c) (nth c 0)])"
        ),
        "[1 5]"
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (range 0M 1M 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // Coverage pins (no oracle rows): projectors over forced
    // chunk-cons states (rest chain, forced head).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (chunk-append b 2) (let [s (chunk-cons (chunk b) '(3))] (first s) (let [c (chunk-first (rest s))] [(count c) (nth c 0)])))"
        ),
        "[1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [b (chunk-buffer 32)] (chunk-append b 1) (chunk-append b 2) (let [s (chunk-cons (chunk b) '(3))] (first s) (let [c (chunk-first s)] [(count c) (nth c 0)])))"
        ),
        "[2 1]"
    );
}

#[test]
fn mixhash_rows() {
    let mut eval = boot();
    // mh-mix-* value rows.
    for (src, want) in [
        ("(mix-collection-hash 1 0)", "-2017569654"), // mh-mix-1-0
        ("(mix-collection-hash 0 0)", "-15128758"),   // mh-mix-0-0
        ("(mix-collection-hash 1 1)", "1978132887"),  // mh-mix-1-1
        ("(mix-collection-hash 1 2)", "1734656966"),  // mh-mix-1-2
        ("(mix-collection-hash -1 3)", "-196466786"), // mh-mix-neg
        ("(mix-collection-hash 123456789 10)", "1485565228"), // mh-mix-big
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
    // Coverage pins (JVM-verified in the R1 fixup round; no oracle
    // rows): tower seeds narrow checked, nil trips the
    // `doubleValue` NPE.
    for (src, want) in [
        ("(mix-collection-hash 5N 1)", "1398391747"),
        ("(mix-collection-hash 1/2 1)", "-1684229222"),
        ("(mix-collection-hash 1.5M 1)", "1978132887"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(mix-collection-hash nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
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
    // Coverage pins (no oracle rows): unbounded inputs take the
    // same bare UOE `hash` does (pre-, and post-realize behind a
    // pending `map`).
    for src in [
        "(hash-combine 0 (range))",
        "(hash-combine 0 (map identity [(repeat 1)]))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.UnsupportedOperationException | ",
            "{src}"
        );
    }
    // Coverage pins (no oracle rows): a struct combines exactly
    // like its content map (JVM-verified).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defstruct hcb :name :age) (= (hash-combine 0 (struct-map hcb :name \"Ada\")) (hash-combine 0 {:name \"Ada\" :age nil})))"
        ),
        "true"
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
    // Coverage pins (JVM-verified in the R1 fixup round; no oracle
    // rows): an unseqable input, a first-step failure, and an
    // inexact range (whose `seq` succeeds but whose first step
    // raises) all drain to end-of-stream (the producer drops).
    assert_eq!(run_last(&mut eval, "(nil? (seq (seque 42)))"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (seq (seque (map (fn [x] (/ 1 0)) '(1)))))"
        ),
        "\"nil\""
    );
    assert_eq!(
        run_last(&mut eval, "(pr-str (seq (seque (range 0M 1M 1/3))))"),
        "\"nil\""
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
    // it-opts-strkey
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 1 (iteration inc \"x\" 1 :initk 0)))"
        ),
        "\"(1)\""
    );
    // it-opts-nskey
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 1 (iteration inc :foo/bar 1 :initk 0)))"
        ),
        "\"(1)\""
    );
    // it-opts-lonemap
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity {:initk 5})))"
        ),
        "\"(5 5 5)\""
    );
    // it-opts-loneset
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity #{:initk})))"
        ),
        "\"(:initk :initk :initk)\""
    );
    // it-opts-trailmap
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity :vf identity {:initk 5})))"
        ),
        "\"(5 5 5)\""
    );
    // it-opts-trailnil
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity :vf identity nil)))"
        ),
        "\"()\""
    );
    // it-opts-trailpair
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity :vf identity [:a :b])))"
        ),
        "\"()\""
    );
    // it-opts-trailstr
    assert_eq!(
        uncaught(
            &mut eval,
            "(take 3 (iteration identity :vf identity \"x\"))"
        ),
        cce("java.lang.Character", "java.util.Map$Entry")
    );
    // it-opts-traillist
    assert_eq!(
        uncaught(
            &mut eval,
            "(take 3 (iteration identity :vf identity '([:initk 5])))"
        ),
        cce("clojure.lang.PersistentVector", "java.util.Map$Entry")
    );
    // it-opts-trailvec1
    assert_eq!(
        uncaught(
            &mut eval,
            "(take 3 (iteration identity :vf identity [[:initk 5]]))"
        ),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    // it-opts-lonekw
    assert_eq!(
        run_last(&mut eval, "(pr-str (take 1 (iteration (fn [_] 7) :initk)))"),
        "\"(7)\""
    );
    // it-opts-trailkw
    assert_eq!(
        uncaught(&mut eval, "(take 1 (iteration inc :initk 0 :vf))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    // (derived) every seqed trailing element folds — later wins
    // (JVM: `(6 6)`), and the trailing wins over pairs (JVM:
    // `(5 5)`).
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 2 (iteration identity :vf identity (list (first {:initk 5}) (first {:initk 6})))))"
        ),
        "\"(6 6)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 2 (iteration identity :initk 1 :vf identity {:initk 5})))"
        ),
        "\"(5 5)\""
    );
    // (derived) an entry trailing conjs as its pair, a struct
    // trailing merges its content, and a set trailing casts its
    // elements (JVM: `(5 5)`, `(9 9)`, CCE Keyword).
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (take 3 (iteration identity :vf identity (first {:initk 5}))))"
        ),
        "\"(5 5 5)\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defstruct ittiny :initk)
                 (pr-str (take 2 (iteration identity :vf identity (struct-map ittiny :initk 9)))))"
        ),
        "\"(9 9)\""
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(take 3 (iteration identity :vf identity #{:a}))"
        ),
        cce("clojure.lang.Keyword", "java.util.Map$Entry")
    );
    // Coverage pins (no oracle rows): iterations compare by
    // identity (like the JVM's reify object).
    assert_eq!(
        run_last(&mut eval, "(= (iteration inc) (iteration inc))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (iteration inc)))"),
        "true"
    );
    // Coverage pins (no oracle rows): the port-stable print and
    // class-name shapes (the JVM names a reify class).
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(pr-str (iteration inc))")),
        prs("#object[clojure.core$iteration 0xH \"clojure.core$iteration@L\"]")
    );
    assert_eq!(
        uncaught(&mut eval, "(chunk-first (iteration inc))"),
        cce("clojure.core$iteration", ICHUNKEDSEQ)
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
    // Coverage pins (no oracle rows): handles compare by identity
    // (like the JVM's `Matcher`).
    assert_eq!(
        run_last(
            &mut eval,
            "(= (re-matcher #\"a\" \"a\") (re-matcher #\"a\" \"a\"))"
        ),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (re-matcher #\"a\" \"a\")))"),
        "true"
    );
    // Coverage pins (no oracle rows): an uncompilable pattern
    // reports `PatternSyntaxException` with the crate message
    // (same shape as `re-pattern`).
    assert_eq!(
        uncaught(&mut eval, "(re-matcher #\"(a\" \"x\")"),
        "java.util.regex.PatternSyntaxException | regex parse error:\n    (a\n    ^\nerror: unclosed group"
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
    // fs-names (leaf names through `re-seq`, like `.getName`;
    // the class splits on both separators — `file-seq` yields
    // platform-native paths, so `/`-only breaks on Windows).
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(pr-str (sort (map (fn [p] (last (re-seq #\"[^/\\\\]+\" p))) (file-seq {lit}))))"
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
