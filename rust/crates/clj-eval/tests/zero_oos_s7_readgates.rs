// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S7 read gates: probe replay for `*read-eval*`,
//! `*suppress-read*`, `*reader-resolver*`, `read+string`,
//! `load-reader`, and `with-loading-context` (see
//! `docs/zero-oos-s7-readgates/probes/{gates,trio}.{clj,out}`).
//! Every test names the probe labels it replays; port-only pins
//! (host-fenced divergences) say so inline.

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
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

const GATE: &str = "EvalReader not allowed when *read-eval* is false.";
const UNKNOWN: &str = "Reading disallowed - *read-eval* bound to :unknown";

#[test]
fn gate_var_roots() {
    // Probes `a-readeval-root`, `a-suppress-root`, `a-resolver-root`,
    // `a-resolver-resolves`: the JVM roots (no `:dynamic` meta,
    // like the other `RT.java` dynamics).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*read-eval*"), "true");
    assert_eq!(run_last(&mut eval, "*suppress-read*"), "nil");
    assert_eq!(run_last(&mut eval, "*reader-resolver*"), "nil");
    assert_eq!(
        run_last(&mut eval, "(resolve '*reader-resolver*)"),
        "#'clojure.core/*reader-resolver*"
    );
    // The port carries no `:added`/`:doc` on boot vars (house-wide,
    // not S7's to start).
    assert_eq!(
        run_last(
            &mut eval,
            "(select-keys (meta #'*read-eval*) [:dynamic :private :added :macro])"
        ),
        "{}"
    );
}

#[test]
fn read_eval_gate_both_arms() {
    // Probes `b-eval`, `b-false`, `b-nil`: `#=` applies by default
    // and throws the gate text when false or nil.
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(read-string \"#=(+ 1 2)\")"), "3");
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* false] (read-string \"#=(+ 1 2)\"))"
        ),
        format!("java.lang.RuntimeException | {GATE}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* nil] (read-string \"#=1\"))"
        ),
        format!("java.lang.RuntimeException | {GATE}")
    );
}

#[test]
fn read_eval_gate_before_operand() {
    // Port-only (no oracle label): the gate fires before the
    // operand is touched (even `#=` at EOF reports the gate,
    // not EOF).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* false] (read-string \"#=\"))"
        ),
        format!("java.lang.RuntimeException | {GATE}")
    );
}

#[test]
fn read_eval_unknown_refuses() {
    // Probes `b-unknown-plain`, `b-unknown-eval`: `:unknown`
    // refuses every read (raw, like the JVM entry check — probes
    // `h-unknown-lnpr`, `p-unknown`).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read-string \"42\"))"
        ),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read-string \"#=(+ 1 2)\"))"
        ),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (with-in-str \"1\" (read *in*)))"
        ),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (with-in-str \"1\" (read+string *in*)))"
        ),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
}

#[test]
fn read_eval_no_such_opt() {
    // Probe `b-opts-ignored`: `read` has no `:read-eval` option.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#=(+ 1 2)\" (read {:read-eval false} *in*))"
        ),
        "3"
    );
}

#[test]
fn read_gate_wraps_and_positions() {
    // Probes `b-read-false`, `h-gate-lnpr`: port readers are
    // `LineNumberingPushbackReader`s, so `read` wraps the gate
    // (the JVM only wraps `LineNumberingPushbackReader` reads)
    // with the entry span.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* false] (with-in-str \"#=1\" (read *in*)))"
        ),
        format!("clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: {GATE}")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (binding [*read-eval* false] (with-in-str \"#=1\" (read *in*))) :no-throw (catch Throwable t (ex-data t)))"
        ),
        "#:clojure.error{:line 1, :column 3}"
    );
}

#[test]
fn eval_reader_applies_to_raw_args() {
    // Probes `c-rawlist`, `c-map`, `c-nested`: `#=` is restricted —
    // the resolved head applies to the UNEVALUATED operand forms.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(str (+ 1 2))\")"),
        "\"(+ 1 2)\""
    );
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(map inc [1 2])\")"),
        "(nil nil)"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(inc (inc 1))\")"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class java.lang.Number (clojure.lang.PersistentList is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn eval_reader_read_time_positions() {
    // Probes `c-quote`, `c-sq`: `#=` applies during the read, even
    // under quote and syntax-quote.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(read-string \"'#=(+ 1 2)\")"),
        "(quote 3)"
    );
    assert_eq!(run_last(&mut eval, "(read-string \"`#=(+ 1 2)\")"), "3");
}

#[test]
fn eval_reader_head_resolution() {
    // Probes `c-qq`, `c-qq-miss`, `c-qualmiss`, `c-if`,
    // `c-quotehead`, `c-nofn`, `c-macrohead`: qualified heads resolve through
    // namespaces/aliases (unknown qualifiers fail the class
    // lookup); unresolvable heads report. (`c-classhead` pins
    // "Can't resolve String" — but the port refers an import-stub
    // `clojure.core/String` var holding a string, so the head
    // resolves and fails the `IFn` cast instead; pre-existing
    // house behavior, not S7's to change.)
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(clojure.core/+ 1 2)\")"),
        "3"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(clojure.core/nosuchfn 1)\")"),
        "java.lang.RuntimeException | Can't resolve clojure.core/nosuchfn"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(nosuchqual/x 1)\")"),
        "java.lang.ClassNotFoundException | nosuchqual"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(if true 1 2)\")"),
        "java.lang.RuntimeException | Can't resolve if"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(quote x)\")"),
        "java.lang.RuntimeException | Can't resolve quote"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(String 1)\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IFn (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(nosuchfn_xyz 1)\")"),
        "java.lang.RuntimeException | Can't resolve nosuchfn_xyz"
    );
    // L4; oracle `c-macrohead` (`gates.out:30`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(fn [x] x)\")"),
        "java.lang.IllegalArgumentException | Parameter declaration missing"
    );
}

#[test]
fn eval_reader_alias_and_ns_context() {
    // Probes `c-alias-head`, `c-nsctx-hit`, `c-nsctx-miss`: alias
    // heads resolve, and unqualified heads resolve in the bound
    // `*ns*`. (The probe aliases `clojure.string`, which the port
    // classpath lacks — a local aliased ns exercises the same
    // head-resolution arm.)
    let mut eval = boot();
    run(
        &mut eval,
        "(create-ns 's7str) (intern 's7str 'sjoin (fn [sep xs] (apply str (interpose sep xs)))) (alias 's7s 's7str)",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(read-string \"#=(s7s/sjoin \\\", \\\" [1 2])\")"
        ),
        "\"1, 2\""
    );
    // (`in-ns` + `defn` like the probe trips the port's missing
    // `alter-meta!` in fresh namespaces — `intern` sets up the
    // same `*ns*`-visible var.)
    run(
        &mut eval,
        "(do (create-ns 'evn) (intern 'evn 'evfn (fn [x] (+ x 1))))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*ns* (find-ns 'evn)] (read-string \"#=(evfn 1)\"))"
        ),
        "2"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(evfn 1)\")"),
        "java.lang.RuntimeException | Can't resolve evfn"
    );
}

#[test]
fn eval_reader_bad_heads() {
    // Probes `c-empty`, `c-listhead`, `c-numhead`: missing and
    // non-symbol heads.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=()\")"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Symbol.equals(Object)\" because \"fs\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=((fn [x] x) 1)\")"),
        "java.lang.ClassCastException | class clojure.lang.PersistentList cannot be cast to class clojure.lang.Symbol (clojure.lang.PersistentList and clojure.lang.Symbol are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(1 2)\")"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Symbol (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
}

#[test]
fn eval_reader_non_fn_root() {
    // Probe `c-nonfn`: a var holding a non-fn fails the `IFn`
    // cast with module text.
    let mut eval = boot();
    run(&mut eval, "(def c-num 42)");
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(c-num 1)\")"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
}

#[test]
fn eval_reader_invokable_roots() {
    // Port-only (no oracle labels): every `invoke` arm stays
    // callable through `#=` (the `is_invokable` mirror):
    // collections look up, nil errors like a nil call, unbound
    // like an unbound call.
    let mut eval = boot();
    run(
        &mut eval,
        "(def ik :a) (def is 's) (def iv [9]) (def im {:a 1}) (def it #{7}) (def ime (first {:a 1})) (def inil nil) (def ied (delay 1))",
    );
    assert_eq!(run_last(&mut eval, "(read-string \"#=(ik 1)\")"), "nil");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(is 1)\")"), "nil");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(iv 0)\")"), "9");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(im :a)\")"), "1");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(it 7)\")"), "7");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(ime 0)\")"), ":a");
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(inil 1)\")"),
        "java.lang.NullPointerException | Cannot invoke nil as a function"
    );
    run(&mut eval, "(def iub)");
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(iub 1)\")"),
        "java.lang.IllegalStateException | Attempting to call unbound fn: #'user/iub"
    );
    // Promises and delays dispatch to `invoke` faithfully: the
    // port's promise reify lacks the 0-arity (same `ArityException`
    // a normal `(ip)` call raises — pre-existing, out of fence),
    // and a delay root fails the `IFn` checkcast like the JVM.
    run(&mut eval, "(def ip (promise)) (deliver ip 9)");
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(ip)\")"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core$promise$reify"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(ied)\")"),
        "java.lang.ClassCastException | class clojure.lang.Delay cannot be cast to class clojure.lang.IFn (clojure.lang.Delay and clojure.lang.IFn are in unnamed module of loader 'app')"
    );
    run(&mut eval, "(def ivv #'im)");
    assert_eq!(run_last(&mut eval, "(read-string \"#=(ivv :a)\")"), "1");
}

#[test]
fn eval_reader_unsupported_forms() {
    // Probes `c-num`, `c-kw`: non-list non-symbol operands.
    let mut eval = boot();
    for src in ["(read-string \"#=42\")", "(read-string \"#=:k\")"] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | Unsupported #= form"
        );
    }
}

#[test]
fn eval_reader_eof_operand() {
    // Probe `c-eof`: the operand read errors like any read.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=\")"),
        "java.lang.RuntimeException | EOF while reading"
    );
}

#[test]
fn eval_reader_throws_through_read() {
    // Probes `c-div-rs`, `c-div-read`, `h-div-lnpr`: a throwing
    // application surfaces raw under `read-string`, and wrapped
    // under `read` (port readers are LNPRs; the plain-reader raw
    // probes have no port counterpart).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(/ 1 0)\")"),
        "java.lang.ArithmeticException | Divide by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#=(/ 1 0)\" (read *in*))"),
        "clojure.lang.LispReader$ReaderException | java.lang.ArithmeticException: Divide by zero"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#=(/ 1 0)\" (read *in*)) :no-throw (catch Throwable t (ex-data t)))"
        ),
        "#:clojure.error{:line 1, :column 10}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#=(/ 1 0)\" (read *in*)) :no-throw (catch Throwable t (ex-message (ex-cause t))))"
        ),
        "\"Divide by zero\""
    );
}

#[test]
fn eval_reader_wraps_under_read() {
    // Probe `h-div-lnpr` (the LNPR wrap rule): `read` wraps every
    // `#=` outcome. (`c-unsupported-read`/`c-nofn-read` pin the
    // plain-`PushbackReader` raw side, which has no port
    // counterpart — port readers are always line-numbered.)
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#=42\" (read *in*))"),
        "clojure.lang.LispReader$ReaderException | java.lang.IllegalArgumentException: Unsupported #= form"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#=(nosuchfn_xyz 1)\" (read *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Can't resolve nosuchfn_xyz"
    );
}

#[test]
fn eval_reader_preserve_cond() {
    // Probe `c-preserve`: `:preserve` does not affect `#=`.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#=(+ 1 2)\" (read {:read-cond :preserve} *in*))"
        ),
        "3"
    );
}

#[test]
fn eval_reader_runs_during_discard() {
    // Probe `d-discard`: suppression never stops `#=` (the eval
    // error escapes the discarded branch; the port wraps it per
    // the LNPR rule — the probe's plain reader surfaces raw).
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(read {:read-cond :allow} (with-in-str \"#?(:clj 1 :bb #=(/ 1 0))\" *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.ArithmeticException: Divide by zero"
    );
}

#[test]
fn eval_reader_class_heads() {
    // Probes `d-class-miss`, `d-class-full`: bare symbols name
    // classes, and the port has none (host fence) — every one
    // fails the lookup, including fully-qualified names the JVM
    // would load (documented divergence).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=nosuchclass_xyz\")"),
        "java.lang.ClassNotFoundException | nosuchclass_xyz"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=java.lang.String\")"),
        "java.lang.ClassNotFoundException | java.lang.String"
    );
    // Probes `d-ctor`, `d-static-miss`: constructor and static
    // heads need host classes too.
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(String. \\\"x\\\")\")"),
        "java.lang.ClassNotFoundException | String"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(nosuch.Klass/m 1)\")"),
        "java.lang.ClassNotFoundException | nosuch.Klass"
    );
}

#[test]
fn eval_reader_var_heads() {
    // Probes `d-varq`, `d-var-miss`, `d-var-new*`, `d-var-unqual`,
    // `d-var-nosec`, `d-var-nonsym`, `d-varhead-q`, `d-var-miss-ns`:
    // `(var s)` interns (creating) and returns the var; only bare
    // `var` matches.
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(var clojure.core/map)\")"),
        "#'clojure.core/map"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(read-string \"#=(var nosuchns_xyz/nosuchv_xyz)\")"
        ),
        "#'nosuchns_xyz/nosuchv_xyz"
    );
    assert_eq!(
        run_last(&mut eval, "(str (ns-name (find-ns 'nosuchns_xyz)))"),
        "\"nosuchns_xyz\""
    );
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(var user/nosuchv_xyz2)\")"),
        "#'user/nosuchv_xyz2"
    );
    assert_eq!(run_last(&mut eval, "(bound? #'user/nosuchv_xyz2)"), "false");
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(var map)\")"),
        "java.lang.NullPointerException | Cannot invoke \"String.hashCode()\" because \"this.name\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(var)\")"),
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"vs\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(var 42)\")"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Symbol (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#=(clojure.core/var map)\")"),
        "java.lang.RuntimeException | Can't resolve clojure.core/var"
    );
}

#[test]
fn suppress_plain_reads_unaffected() {
    // Probes `e-basic`, `e-consume`, `e-trunc`, `e-nonnil`:
    // `*suppress-read*` never touches plain reads (no nil return,
    // no skip, no error swallowing).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"42\"))"
        ),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"42 43\" [(binding [*suppress-read* true] (read *in*)) (read *in*)])"
        ),
        "[42 43]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"(1 2\"))"
        ),
        "java.lang.RuntimeException | EOF while reading"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* :yes] (read-string \"42\"))"
        ),
        "42"
    );
}

#[test]
fn suppress_eval_and_gate() {
    // Probes `e-readeval`, `e-readeval-false`, `e-eval-sup-tag`:
    // `#=` still applies under suppression (and the gate still
    // bites); the suppressed tag inside still preserves.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#=(+ 1 2)\"))"
        ),
        "3"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*suppress-read* true *read-eval* false] (read-string \"#=(+ 1 2)\"))"
        ),
        format!("java.lang.RuntimeException | {GATE}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#=(quote #foo [1])\"))"
        ),
        "java.lang.RuntimeException | Can't resolve quote"
    );
}

#[test]
fn suppress_entry_seed_survives_root_flip() {
    // M2 (review round 1): resolve/drain follow the ENTRY seed,
    // never a live re-deref — read-time user code may mutate
    // the root mid-read.
    let mut eval = boot();
    run(
        &mut eval,
        "(def flip-sr (fn [x] (alter-var-root #'clojure.core/*suppress-read* (constantly true)) :flipped))",
    );
    // Entry false + mid-read flip to true: the tag deferred
    // under the live table still resolves.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'sr (fn [x] [:ok x])}] (read-string \"#sr #=(user/flip-sr 0)\"))"
        ),
        "[:ok :flipped]"
    );
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*suppress-read* (constantly nil))",
    );
    run(
        &mut eval,
        "(def flip-sr-f (fn [x] (alter-var-root #'clojure.core/*suppress-read* (constantly false)) :flipped2))",
    );
    // Entry true + mid-read flip to false: the tag was
    // preserved, so nothing resolves (no `No reader function`).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#sr #=(user/flip-sr-f 0)\"))"
        ),
        "#sr :flipped2"
    );
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*suppress-read* (constantly nil))",
    );
}

#[test]
fn suppress_preserves_unknown_tags() {
    // Probes `e-tag`, `e-tag-sup`, `e-tag-sup-truthy`: only `#tag`
    // reads consult the flag (unknown tags preserve instead of
    // erroring).
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#foo [1 2]\")"),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#foo [1 2]\"))"
        ),
        "#foo [1 2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* :yes] (read-string \"#foo [1 2]\"))"
        ),
        "#foo [1 2]"
    );
}

#[test]
fn suppress_inst_divergence() {
    // Probe `e-inst-sup`: the JVM runs `#inst`/`#uuid` under
    // suppression (they bypass it); the port keeps its
    // suppress-before-consult order now that S6 has landed, so
    // it preserves like any tag (documented divergence,
    // retained — see the spec).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#inst \\\"2020-01-01\\\"\"))"
        ),
        "#inst \"2020-01-01\""
    );
    // L1: the sharp edge — an invalid `#inst` under suppression
    // is silently preserved by the port while the JVM throws
    // (unsuppressed, S6 pins the throw: `d-inst-bad`).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (read-string \"#inst \\\"bogus\\\"\"))"
        ),
        "#inst \"bogus\""
    );
}

/// Bind the scripted test resolver (answers by op; `nil` ops
/// answer nil) and read `src` with it.
fn with_resolver(eval: &mut Evaluator, answers: &str, src: &str) -> String {
    run_last(
        eval,
        &format!(
            "(binding [*reader-resolver* (fn [op s] (get {answers} op))] (read-string \"{src}\"))"
        ),
    )
}

#[test]
fn resolver_auto_resolve() {
    // Probes `f-alias`, `f-current`, `f-alias-2part`, `f-nilalias`,
    // `f-current-nil`: `::` consults the fn (the answer's NAME
    // becomes the keyword ns); nil is invalid.
    let mut eval = boot();
    assert_eq!(
        with_resolver(
            &mut eval,
            "{:resolve-alias 'aliased-ns :current-ns 'cur-ns}",
            "::str/foo"
        ),
        ":aliased-ns/foo"
    );
    assert_eq!(
        with_resolver(
            &mut eval,
            "{:resolve-alias 'aliased-ns :current-ns 'cur-ns}",
            "::foo"
        ),
        ":cur-ns/foo"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-alias 'A/B}", "::str/foo"),
        ":B/foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* (fn [op s] nil)] (read-string \"::str/foo\"))"
        ),
        "java.lang.RuntimeException | Invalid token: ::str/foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* (fn [op s] nil)] (read-string \"::foo\"))"
        ),
        "java.lang.RuntimeException | Invalid token: ::foo"
    );
}

#[test]
fn resolver_syntax_quote() {
    // Probes `f-sq-var`, `f-sq-unqual-class`, `f-sq-unqual-allnil`,
    // `f-sq-qual-class`, `f-sq-qual-alias`, `f-sq-qual-bothnil`,
    // `f-sq-dotclass`, `f-sq-dotclass-nil`, `f-sq-dotted`,
    // `f-sq-dotfirst`: the consult orders and fallbacks.
    let mut eval = boot();
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-var 'vvar/vname}", "`x"),
        "(quote vvar/vname)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-class 'C/D}", "`x"),
        "(quote C/D)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:current-ns 'cur-ns}", "`x"),
        "(quote cur-ns/x)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-class 'R/S}", "`String/foo"),
        "(quote S/foo)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-alias 'X/Y}", "`alias/foo"),
        "(quote Y/foo)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:current-ns 'cur-ns}", "`alias/foo"),
        "(quote alias/foo)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:resolve-class 'R/S}", "`String."),
        "(quote S.)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:current-ns 'cur-ns}", "`String."),
        "(quote String.)"
    );
    assert_eq!(
        with_resolver(&mut eval, "{:current-ns 'cur-ns}", "`clojure.core"),
        "(quote cur-ns/clojure.core)"
    );
    assert_eq!(
        with_resolver(
            &mut eval,
            "{:resolve-class 'R/S :resolve-var 'V/W}",
            "`.foo"
        ),
        "(quote .foo)"
    );
}

#[test]
fn resolver_gensym_shape() {
    // Probe `f-sq-gensym`: gensyms bypass the resolver (the id
    // varies, so this pins the shape).
    let mut eval = boot();
    let out = with_resolver(
        &mut eval,
        "{:resolve-class 'R/S :resolve-alias 'X/Y :resolve-var 'V/W :current-ns 'cur-ns}",
        "`x#",
    );
    assert!(
        out.starts_with("(quote x__") && out.ends_with("__auto__)"),
        "gensym shape: {out}"
    );
}

#[test]
fn resolver_nil_current_ns() {
    // Probe `f-sq-currentnil`: a nil `currentNS()` NPEs, raw
    // under `read-string` like every `#=`/resolver throwable.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* (fn [op s] nil)] (read-string \"`x\"))"
        ),
        "java.lang.NullPointerException | Cannot read field \"name\" because the return value of \"clojure.lang.LispReader$Resolver.currentNS()\" is null"
    );
}

#[test]
fn resolver_non_fn_cast() {
    // Probes `f-nonresolver`, `f-nonresolver-plain`,
    // `h-resolver-lnpr`: a bound non-fn fails every read (raw,
    // like the JVM entry check — even plain forms).
    let mut eval = boot();
    for src in [
        "(binding [*reader-resolver* 42] (read-string \"::foo\"))",
        "(binding [*reader-resolver* 42] (read-string \"1\"))",
        "(binding [*reader-resolver* 42] (with-in-str \"1\" (read *in*)))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.LispReader$Resolver (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.LispReader$Resolver is in unnamed module of loader 'app')",
            "{src}"
        );
    }
}

#[test]
fn resolver_throws_carry_data() {
    // Probe `f-throws`: a throwing consult propagates with data
    // (the cause under `read`, raw under `read-string` — the
    // latter wraps per its landed contract).
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(try (binding [*reader-resolver* (fn [op s] (throw (ex-info \"rx\" {:k 1})))] (with-in-str \"::a/b\" (read *in*))) :no-throw (catch Throwable t [(ex-message (ex-cause t)) (ex-data (ex-cause t))]))"
        ),
        "[\"rx\" {:k 1}]"
    );
}

#[test]
fn resolver_bad_answer_cast() {
    // Port pin (no JVM counterpart — the interface is typed): a
    // non-symbol answer fails the `Symbol` cast, raw like every
    // resolver throwable under `read-string`.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* (fn [op s] 42)] (read-string \"::foo\"))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Symbol (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
}

#[test]
fn resolver_var_and_baseline() {
    // A var holding a fn serves (the `Var` consult arm), and the
    // unbound baseline leaves `String/foo` alone (no imports —
    // pre-existing N1 fence, probe `f-sq-baseline` context).
    let mut eval = boot();
    run(
        &mut eval,
        "(def rf (fn [op s] (get {:current-ns 'cur-ns} op)))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*reader-resolver* #'rf] (read-string \"::foo\"))"
        ),
        ":cur-ns/foo"
    );
    assert_eq!(
        run_last(&mut eval, "(read-string \"`String/foo\")"),
        "(quote String/foo)"
    );
}

#[test]
fn entry_check_order() {
    // Probes `g-read-nil`, `g-read-1nil`, `g-read-badopts`,
    // `g-read-badstream`, `g-rs-nil`, `g-rs-nonstring`, `h-order`,
    // `h-order2`, `h-order3`: stream cast, resolver cast,
    // `:unknown`, opts, nil NPE.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read {:eof 1} nil))"
        ),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*read-eval* :unknown] (read nil))"),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*read-eval* :unknown] (read 42 nil))"),
        format!("java.lang.RuntimeException | {UNKNOWN}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read {:eof 1} \"s\"))"
        ),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.PushbackReader (java.lang.String and java.io.PushbackReader are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read-string nil))"
        ),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown] (read-string 42))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* :unknown *reader-resolver* 42] (with-in-str \"1\" (read *in*)))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.LispReader$Resolver (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.LispReader$Resolver is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*reader-resolver* 42] (read \"s\"))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.PushbackReader (java.lang.String and java.io.PushbackReader are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*reader-resolver* 42] (read nil))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.LispReader$Resolver (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.LispReader$Resolver is in unnamed module of loader 'app')"
    );
}

#[test]
fn read_plus_string_basics() {
    // Probes `p-basic`, `p-ws`, `p-tabs`, `p-second`, `p-0arity`,
    // `p-3arity`, `p-newline`, `p-comment`: the `[form string]`
    // return with the Java-trimmed capture (leading trivia
    // included, per-call capture).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"42\" (read+string *in*))"),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"  42  \" (read+string *in*))"),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \" \\t42\\t \" (read+string *in*))"),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"42 43\" [(read+string *in*) (read+string *in*)])"
        ),
        "[[42 \"42\"] [43 \"43\"]]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"42\" (read+string))"),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"42\" (read+string *in* false :eof))"
        ),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"42\\n43\" (read+string *in*))"),
        "[42 \"42\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"; hi\\n42\" (read+string *in*))"),
        "[42 \"; hi\\n42\"]"
    );
}

#[test]
fn read_plus_string_eof() {
    // Probes `p-eof`, `p-eofval`, `p-wseof`, `p-opts`: EOF honors
    // the flags (empty capture) and the opts arity works.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"\" (read+string *in*))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"\" (read+string *in* false :eof))"
        ),
        "[:eof \"\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"   \" (read+string *in* false :eof))"
        ),
        "[:eof \"\"]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"\" (read+string {:eof :eof} *in*))"
        ),
        "[:eof \"\"]"
    );
}

#[test]
fn recursive_read_rides_throwable_intact() {
    // M1-reader (review round 1): a raw recursive rethrow still
    // rides `#=` throwables intact (data and cause survive,
    // like `read-string`). (`throw` is not a `#=` head, so the
    // throw hides in a user fn, whose body evaluates normally.)
    let mut eval = boot();
    run(
        &mut eval,
        "(def boom-fn (fn [x] (throw (ex-info \"boom\" {:k 1}))))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#=(user/boom-fn 0)\" (read *in* true nil true)) :no-throw (catch Throwable t [(ex-message t) (ex-data t)]))"
        ),
        "[\"boom\" {:k 1}]"
    );
}

#[test]
fn read_plus_string_errors() {
    // Probes `p-err`, `p-recursive`, `p-gate`, `p-sup-tag`: reader
    // errors wrap like `read` (recursive stays raw), the gate
    // wraps, and suppression preserves with the full capture.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"(1 2\" (read+string *in*))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading, starting at line 1"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"(1\" (read+string *in* true nil true))"
        ),
        "java.lang.RuntimeException | EOF while reading, starting at line 1"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*read-eval* false] (with-in-str \"#=1\" (read+string *in*)))"
        ),
        format!("clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: {GATE}")
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*suppress-read* true] (with-in-str \"#foo [1]\" (read+string *in*)))"
        ),
        "[#foo [1] \"#foo [1]\"]"
    );
}

const GETSTRING_NPE: &str = "Cannot invoke \"clojure.lang.LineNumberingPushbackReader.getString()\" because \"stream\" is null";
const LNPR_CCE: &str = "class java.lang.String cannot be cast to class clojure.lang.LineNumberingPushbackReader (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.LineNumberingPushbackReader is in unnamed module of loader 'app')";

#[test]
fn read_plus_string_stream_checks() {
    // Probes `p-strarg`, `p-nilarg`, `p-nil-badopts`,
    // `p-str-badopts`, `p-opts-badstream`, `p-nil-in`: the hint
    // cast at entry, the nil `getString` NPE ahead of opts
    // parsing. (The port has one reader kind, so the
    // plain-`PushbackReader` probe has no counterpart.)
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(read+string \"42\")"),
        format!("java.lang.ClassCastException | {LNPR_CCE}")
    );
    assert_eq!(
        uncaught(&mut eval, "(read+string nil)"),
        format!("java.lang.NullPointerException | {GETSTRING_NPE}")
    );
    assert_eq!(
        uncaught(&mut eval, "(read+string 42 nil)"),
        format!("java.lang.NullPointerException | {GETSTRING_NPE}")
    );
    assert_eq!(
        uncaught(&mut eval, "(read+string 42 \"s\")"),
        format!("java.lang.ClassCastException | {LNPR_CCE}")
    );
    assert_eq!(
        uncaught(&mut eval, "(read+string {:eof 1} \"s\")"),
        format!("java.lang.ClassCastException | {LNPR_CCE}")
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* nil] (read+string))"),
        format!("java.lang.NullPointerException | {GETSTRING_NPE}")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"42\" (read+string *in* true nil false :extra))"
        ),
        "clojure.lang.ArityException | Wrong number of args (5) passed to: clojure.core/read+string"
    );
}

#[test]
fn load_reader_basics() {
    // Probes `l-basic`, `l-defined`, `l-empty`, `l-file`, `l-ns`,
    // `l-nsescape`, `l-pending`, `l-readeval-visible`,
    // `l-midstream`: the last value (nil when empty), `*file*`
    // nil, `*ns*` isolated, `*pending-paths*` untouched,
    // `*read-eval*` visibly true, no rewind.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"(def lr-a 1) :lr-ret\" (load-reader *in*))"
        ),
        ":lr-ret"
    );
    assert_eq!(run_last(&mut eval, "lr-a"), "1");
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (load-reader *in*))"),
        "nil"
    );
    run(
        &mut eval,
        "(with-in-str \"(def lr-file *file*)\" (load-reader *in*))",
    );
    assert_eq!(run_last(&mut eval, "lr-file"), "nil");
    run(
        &mut eval,
        "(with-in-str \"(def lr-ns (ns-name *ns*))\" (load-reader *in*))",
    );
    assert_eq!(run_last(&mut eval, "lr-ns"), "user");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"(in-ns 'lrx)\" (load-reader *in*)) (ns-name *ns*)"
        ),
        "user"
    );
    run(
        &mut eval,
        "(with-in-str \"(def lpp @(resolve 'clojure.core/*pending-paths*))\" (load-reader *in*))",
    );
    assert_eq!(run_last(&mut eval, "lpp"), "()");
    run(
        &mut eval,
        "(with-in-str \"(def lre *read-eval*)\" (load-reader *in*))",
    );
    assert_eq!(run_last(&mut eval, "lre"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"1 2\" (read *in*) (load-reader *in*))"
        ),
        "2"
    );
}

#[test]
fn load_reader_runs_data_readers() {
    // B1 (review round 1): `load-reader` runs the S6 pipeline
    // per form (snapshot/drain/resolve), like `load-string`.
    let mut eval = boot();
    // A table tag resolves through the load.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'lr (fn [x] [:lr x])}] (with-in-str \"#lr 41\" (load-reader *in*)))"
        ),
        "[:lr 41]"
    );
    // `#inst` validates through the load.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#inst \\\"2020-01-01\\\"\" (load-reader *in*))"
        ),
        "#inst \"2020-01-01\""
    );
    // A throwing reader fn fails the load raw (resolve arm —
    // like `load`, no `CompilerException` wrap).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'lr (fn [x] (throw (ex-info \"lr-boom\" {})))}] (with-in-str \"#lr 1\" (load-reader *in*)))"
        ),
        "clojure.lang.ExceptionInfo | lr-boom"
    );
    // A tag deferred under a live table but resolved after a
    // mid-resolve root flip fails `No reader function` raw (one
    // form: both tags defer under the live table, then the first
    // fn flips the root before the second resolves).
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*data-readers* (constantly {'lr1 (fn [x] (do (alter-var-root #'clojure.core/*data-readers* (constantly nil)) :f)) 'lr2 (fn [x] :s)}))",
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"[#lr1 1 #lr2 2]\" (load-reader *in*))"
        ),
        "java.lang.RuntimeException | No reader function for tag lr2"
    );
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*data-readers* (constantly nil))",
    );
    // A throwing discard-drain fails the load raw (drain arm).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'lr (fn [x] (throw (ex-info \"drain-boom\" {})))}] (with-in-str \"#_ #lr 1 :done\" (load-reader *in*)))"
        ),
        "clojure.lang.ExceptionInfo | drain-boom"
    );
}

#[test]
fn load_reader_forces_read_eval() {
    // Probe `l-gate`: the load path ignores the entry binding.
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*read-eval* false] (with-in-str \"#=(+ 1 2)\" (load-reader *in*)))"
        ),
        "3"
    );
}

#[test]
fn load_reader_errors() {
    // Probes `l-err-eval`, `l-err-read`, `l-err-trunc`: read-phase
    // failures wrap `Compiler.load`-style with catch-time spans.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"(def evlr #=(/ 1 0))\" (load-reader *in*))"
        ),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (1:20)."
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"(def evlr2 1) ]\" (load-reader *in*))"
        ),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (1:16)."
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"(def evlr3 1) (3\" (load-reader *in*))"
        ),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
    );
    // The port reads all forms before evaluating (pre-existing
    // load-loop semantics — the JVM interleaves, probes
    // `l-err-read-defined`, `l-err-trunc-defined`).
    assert_eq!(run_last(&mut eval, "(resolve 'evlr2)"), "nil");
}

#[test]
fn load_reader_arg_checks() {
    // Probes `l-strarg`, `l-nilarg`, `l-0args`, `l-2args`,
    // `l-order`: the `Reader` cast, the null-message NPE (nil
    // first), the 1-arity. (The port has one reader kind, so the
    // `StringReader`/plain-pushback probes have no counterpart.)
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(load-reader \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.Reader (java.lang.String and java.io.Reader are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(load-reader nil)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(load-reader)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/load-reader"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"1\" (load-reader *in* *in*))"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/load-reader"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* 42] (load-reader nil))"
        ),
        "java.lang.NullPointerException | "
    );
}

#[test]
fn load_reader_resolver_midstream() {
    // Probe `l-resolver-mid`: the entry cast blames the post-skip
    // position, `Compiler.load`-style.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"1 2\" (read *in*) (binding [*reader-resolver* 42] (load-reader *in*)))"
        ),
        "clojure.lang.Compiler$CompilerException | Syntax error executing at (1:3)."
    );
}

#[test]
fn load_forces_read_eval() {
    // Probes `m-loadstring-false`, `m-loadstring-unknown`,
    // `m-u-defined`, `m-loadstring-visible`: every text load
    // forces `*read-eval*` true, visibly.
    let mut eval = boot();
    run(
        &mut eval,
        "(binding [*read-eval* false] (load-string \"(def evs #=(+ 1 2))\"))",
    );
    assert_eq!(run_last(&mut eval, "evs"), "3");
    run(
        &mut eval,
        "(binding [*read-eval* :unknown] (load-string \"(def evu 1)\"))",
    );
    assert_eq!(run_last(&mut eval, "evu"), "1");
    run(&mut eval, "(load-string \"(def lrs *read-eval*)\")");
    assert_eq!(run_last(&mut eval, "lrs"), "true");
}

#[test]
fn load_string_eval_read_errors() {
    // Probes `m-loadstring-err`, `m-loadstring-nofn`,
    // `m-loadstring-unsupported`, `m-loadstring-trunc`: `#=`
    // outcomes wrap read-phase style with post-operand spans.
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(load-string \"(def evz #=(/ 1 0))\")"),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (1:19)."
    );
    assert_eq!(
        uncaught(&mut eval, "(load-string \"(def evz #=(nosuchf_xyz 1))\")"),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (1:27)."
    );
    assert_eq!(
        uncaught(&mut eval, "(load-string \"(def evz #=42)\")"),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (1:14)."
    );
    assert_eq!(
        uncaught(&mut eval, "(load-string \"(def evw 1) (w\")"),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
    );
    // Read-all-first again (probe `m-loadstring-trunc-defined`).
    assert_eq!(run_last(&mut eval, "(resolve 'evw)"), "nil");
}

#[test]
fn load_reader_eval_error_restores() {
    // Coverage for `run_loaded_values`' eval-error arm (no
    // oracle — the probes pin read-phase failures): a form
    // that reads but throws surfaces raw, like `load`, with
    // the namespace restored and earlier defs kept.
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"(def evlr-ok 1) (throw (ex-info \\\"ev-boom\\\" {}))\" (load-reader *in*))"
        ),
        "clojure.lang.ExceptionInfo | ev-boom"
    );
    assert_eq!(run_last(&mut eval, "evlr-ok"), "1");
    assert_eq!(run_last(&mut eval, "(ns-name *ns*)"), "user");
}

#[test]
fn resolver_midread_unbind_skips_consult() {
    // Coverage for the resolve-time nil arm (M2-class root
    // flip, no oracle): entry-bound fn, unbound before the
    // consult → the consult answers no-consult, and the
    // auto-keyword falls to `Invalid token` (never-bound
    // would resolve via the maps).
    let mut eval = boot();
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*reader-resolver* (constantly (fn [op s] :consulted)))",
    );
    run(
        &mut eval,
        "(def flip-rsv-nil (fn [] (alter-var-root #'clojure.core/*reader-resolver* (constantly nil)) nil))",
    );
    assert_eq!(
        uncaught(&mut eval, "(load-string \"#=(user/flip-rsv-nil) ::x\")",),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
    );
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*reader-resolver* (constantly nil))",
    );
}

#[test]
fn resolver_midread_badflip_throws_cce() {
    // Coverage for the resolve-time CCE arm (M2-class root
    // flip, no oracle): entry-bound fn, rebound to a non-fn
    // before the consult → the `Resolver` cast fires.
    let mut eval = boot();
    run(
        &mut eval,
        "(alter-var-root #'clojure.core/*reader-resolver* (constantly (fn [op s] :consulted)))",
    );
    run(
        &mut eval,
        "(def flip-rsv-bad (fn [] (alter-var-root #'clojure.core/*reader-resolver* (constantly 42)) nil))",
    );
    assert_eq!(
        uncaught(&mut eval, "(load-string \"#=(user/flip-rsv-bad) ::x\")",),
        "clojure.lang.Compiler$CompilerException | Syntax error reading source at (2:1)."
    );
    // No root restore: with 42 still bound, even the restore
    // form would trip the entry cast (f-nonresolver); the
    // evaluator is dropped with the test.
}

#[test]
fn eval_reader_plain_ns_head_skips_aliases() {
    // Coverage for `resolve_ns_target`'s fallback (no oracle —
    // every probed qualified head is alias-mapped): a head
    // qualified with a real, unaliased namespace resolves
    // without alias mapping. (The unrelated alias forces a
    // non-matching loop iteration.)
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'evn2) (intern 'evn2 'evfn (fn [x] (+ x 1))) (create-ns 'other2) (alias 'zz2 'other2))",
    );
    assert_eq!(
        run_last(&mut eval, "(read-string \"#=(evn2/evfn 1)\")"),
        "2"
    );
}

#[test]
fn load_seeds_suppress_and_resolver() {
    // Probes `m-load-sup`, `m-load-resolver`: loads consult the
    // ambient suppression flag, and a bad resolver fails
    // `Compiler.load`-style.
    let mut eval = boot();
    run(
        &mut eval,
        "(binding [*suppress-read* true] (load-string \"(def lts #foo [1])\"))",
    );
    assert_eq!(run_last(&mut eval, "lts"), "#foo [1]");
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*reader-resolver* 42] (load-string \"1\"))"
        ),
        "clojure.lang.Compiler$CompilerException | Syntax error executing at (1:1)."
    );
}

#[test]
fn loading_context_basics() {
    // Probes `w-basic`, `w-empty`: the body runs in order, the
    // last value returns (nil when empty).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(with-loading-context 1 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "(with-loading-context)"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(def wlc-side []) (with-loading-context (def wlc-side (conj wlc-side 1)) (def wlc-side (conj wlc-side 2))) wlc-side"
        ),
        "[1 2]"
    );
}

#[test]
fn loading_context_expansion_shape() {
    // Probe `w-expand` (shape): an immediately-invoked fn with a
    // fresh `loading` name around the body (the port has no
    // classloader to bind).
    let mut eval = boot();
    let out = run_last(
        &mut eval,
        "(pr-str (macroexpand-1 '(with-loading-context (load \"a\") (load \"b\"))))",
    );
    assert!(
        out.starts_with("\"((clojure.core/fn loading")
            && out.ends_with("[] (load \\\"a\\\") (load \\\"b\\\")))\"")
            && out
                .trim_start_matches("\"((clojure.core/fn loading")
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()),
        "expansion shape: {out}"
    );
    assert_eq!(
        run_last(&mut eval, "(:macro (meta #'with-loading-context))"),
        "true"
    );
}

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop; mirrors `loader.rs`).
struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s7-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn within(&self, src: &str) -> String {
        let s = self.dir.to_string_lossy().replace('\\', "/");
        format!("(binding [*load-path* [\"{s}\"]] {src})")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn loading_context_guards_cycles() {
    // The BDD cycle case: a cyclic `load` through the macro still
    // hits the native `*pending-paths*` guard (which the macro
    // never forks).
    let fx = Fixture::new("cycle");
    fx.write("cyc_a.clj", "(load \"cyc_b\")\n");
    fx.write("cyc_b.clj", "(load \"cyc_a\")\n");
    let mut eval = boot();
    let err = uncaught(
        &mut eval,
        &fx.within("(with-loading-context (load \"cyc_a\"))"),
    );
    assert!(err.contains("Cyclic load dependency"), "cycle guard: {err}");
}

#[test]
fn read_plus_string_arglists_match_oracle() {
    // M4; oracle `p-arglists` (`trio.out:1`).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta #'read+string))"),
        "([] [stream] [stream eof-error? eof-value] [stream eof-error? eof-value recursive?] [opts stream])"
    );
}

#[test]
fn load_reader_arglists_match_oracle() {
    // M4; oracle `l-arglists` (`trio.out:27`).
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta #'load-reader))"),
        "([rdr])"
    );
}
