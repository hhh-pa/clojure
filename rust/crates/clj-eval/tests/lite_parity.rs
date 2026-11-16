// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! #271 `BootMode::Lite` parity: every native that stands in for an
//! interpreted `clojure.string` / `core.clj` definition under Lite must
//! answer exactly what `run` answers — values, exception classes, and
//! messages. The oracle is the full boot (`run`'s
//! own evaluator, whose interpreted definitions back every input the
//! natives do not take); each case evaluates the same source text in both
//! evaluators and the printed outcomes must match.

use clj_eval::aot_boot::boot_lite;
use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Symbol, pr_to_string};

/// Every var a case below references (Lite installs exactly these).
const LITE_VARS: &[(&str, &str)] = &[
    ("clojure.string", "split"),
    ("clojure.string", "split-lines"),
    ("clojure.string", "join"),
    ("clojure.string", "trim"),
    ("clojure.string", "blank?"),
    ("clojure.string", "starts-with?"),
    ("clojure.string", "ends-with?"),
    ("clojure.string", "includes?"),
    ("clojure.core", "reduce-kv"),
    ("clojure.core", "run!"),
    ("clojure.core", "sorted-set"),
    ("clojure.core", "sorted-map"),
    ("clojure.core", "with-meta"),
    ("clojure.core", "meta"),
    ("clojure.core", "odd?"),
    ("clojure.core", "="),
    ("clojure.core", "first"),
    ("clojure.core", "atom"),
    ("clojure.core", "swap!"),
    ("clojure.core", "deref"),
    ("clojure.core", "conj"),
    ("clojure.core", "reduced"),
    ("clojure.core", "str"),
    ("clojure.core", "count"),
    ("clojure.core", "inc"),
    ("clojure.core", "map"),
    ("clojure.core", "vector"),
    ("clojure.core", "list"),
    ("clojure.core", "partial"),
    ("clojure.core", "assoc"),
    ("clojure.core", "identity"),
    ("clojure.core", "map?"),
    ("clojure.core", "some?"),
    ("clojure.core", "nil?"),
    ("clojure.core", "subvec"),
    ("clojure.core", "vector-of"),
    ("clojure.core", "transient"),
    ("clojure.core", "__new*"),
];

/// Strip evaluator-specific gensym counters (`eval16`, `fn--4`) from
/// printed fn names: both evaluators name the same fns differently.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(|c: char| c.is_ascii_digit()) {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if head.ends_with("fn--") || head.ends_with("/eval") {
            out.push('N');
        } else {
            out.push_str(&tail[..digits]);
        }
        rest = &tail[digits..];
    }
    out.push_str(rest);
    out
}

fn outcome(eval: &mut Evaluator, src: &str) -> String {
    normalize(&raw_outcome(eval, src))
}

fn raw_outcome(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Ok(vs) => match vs.last() {
            Some(v) => match eval.realize_deep(v) {
                Ok(v) => pr_to_string(&v, &PrintOpts::pr()),
                Err(e) => format!("REALIZE {e:?}"),
            },
            None => "NOTHING".to_owned(),
        },
        Err(SourceError::Uncaught(t)) => format!(
            "THROW {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => format!("ERR {other}"),
    }
}

struct Pair {
    full: Evaluator,
    lite: Evaluator,
}

impl Pair {
    fn new() -> Self {
        let mut full = boot();
        full.eval_source(
            "(binding [*warn-on-reflection* *warn-on-reflection*] \
             (require 'clojure.string))",
        )
        .expect("full requires");
        // The compiler resolves symbols at build time, so Lite never
        // refers `clojure.core`; refer the installed core vars into
        // `user` so the same source text analyzes in both evaluators.
        let lite = boot_lite(LITE_VARS, false);
        for &(ns, name) in LITE_VARS.iter().filter(|(ns, _)| *ns == "clojure.core") {
            let sym = Symbol::new(Some(ns), name);
            let id = lite.registry.lookup_in_ns(ns, &sym).expect("lite core var");
            lite.registry
                .with_ns_mut("user", |u| u.add_refer(name, sym.clone(), id));
        }
        Self { full, lite }
    }

    /// Every case's mismatch, as `src\n  run:  …\n  lite: …`. A case
    /// either evaluator cannot even read or analyze (`ERR`) or realize
    /// (`REALIZE`) is a harness bug, never a vacuous match.
    fn mismatches(&mut self, cases: &[String]) -> Vec<String> {
        cases
            .iter()
            .filter_map(|src| {
                let want = outcome(&mut self.full, src);
                let got = outcome(&mut self.lite, src);
                let broken = [&want, &got]
                    .iter()
                    .any(|o| o.starts_with("ERR ") || o.starts_with("REALIZE "));
                (broken || want != got).then(|| format!("{src}\n  run:  {want}\n  lite: {got}"))
            })
            .collect()
    }
}

fn assert_parity(cases: &[String]) {
    let mut pair = Pair::new();
    let bad = pair.mismatches(cases);
    assert!(
        bad.is_empty(),
        "{} of {} cases diverge from run:\n{}",
        bad.len(),
        cases.len(),
        bad.join("\n")
    );
}

/// Argument shapes every string native sees: strings, nil, numbers,
/// chars, keywords, colls, regexes, symbols, booleans.
const STR_ARGS: &[&str] = &[
    "\"a b\\nc \"",
    "\"\"",
    "\" \\t\"",
    "nil",
    "5",
    "1.5",
    "\\a",
    ":k",
    "[1 \"x\"]",
    "(quote (\"p\" \"q\"))",
    "#\"b\"",
    "(quote ab)",
    "false",
    "true",
    "{:a 1}",
    "(StringBuilder. \" a b \")",
    "(StringBuilder. \"  \")",
    "(StringBuilder.)",
    "(java.io.File. \"lite-parity-no-such-file\")",
];

fn calls(f: &str, arity: usize, args: &[&str]) -> Vec<String> {
    let mut out = vec![String::new()];
    for _ in 0..arity {
        out = out
            .iter()
            .flat_map(|prefix| args.iter().map(move |a| format!("{prefix} {a}")))
            .collect();
    }
    out.into_iter().map(|a| format!("({f}{a})")).collect()
}

#[test]
fn lite_string_natives_match_run_on_every_shape() {
    let mut cases = Vec::new();
    for f in ["trim", "blank?", "split-lines", "join"] {
        cases.extend(calls(&format!("clojure.string/{f}"), 1, STR_ARGS));
    }
    for f in ["join", "split", "starts-with?", "ends-with?", "includes?"] {
        cases.extend(calls(&format!("clojure.string/{f}"), 2, STR_ARGS));
    }
    for limit in ["2", "-1", "0", "\"x\"", "1.7", "nil", "5000000000"] {
        for s in ["\"a,b,,c,,\"", "nil", "5", "\"\""] {
            for re in ["#\",\"", "nil", "\",\""] {
                cases.push(format!("(clojure.string/split {s} {re} {limit})"));
            }
        }
    }
    for f in [
        "trim",
        "blank?",
        "split-lines",
        "join",
        "split",
        "starts-with?",
        "ends-with?",
        "includes?",
    ] {
        cases.push(format!("(clojure.string/{f})"));
        cases.push(format!("(clojure.string/{f} \"a\" \"b\" \"c\" \"d\")"));
    }
    assert_parity(&cases);
}

#[test]
fn lite_reduce_kv_and_run_bang_match_run() {
    let colls = [
        "{:a 1}",
        "[1 2]",
        "nil",
        "{}",
        "5",
        "\"ab\"",
        "#{1}",
        "(first {:a 1})",
        "(list (first {:a 1}))",
        "(sorted-map 2 :b 1 :a)",
        "(map identity [(first {:a 1})])",
        "(subvec [1 2 3] 1)",
        "(vector-of :long 1 2)",
        "(transient [1])",
        "(transient {:a 1})",
    ];
    // Lite evaluators hold no interpreted closures (a compiled binary's
    // fns are native), so every fn argument here is a native.
    let kv_fns = [
        "assoc",
        "vector",
        "5",
        "nil",
        ":a",
        "inc",
        "(partial vector :p)",
        "(partial reduced)",
    ];
    let one_fns = [
        "identity",
        "5",
        "nil",
        ":a",
        "vector",
        "reduced",
        "(partial vector 1)",
    ];
    let mut cases = Vec::new();
    for c in colls {
        for f in kv_fns {
            cases.push(format!("(reduce-kv {f} [] {c})"));
            cases.push(format!("(reduce-kv {f} {{}} {c})"));
        }
        for f in one_fns {
            cases.push(format!("(run! {f} {c})"));
        }
        cases.push(format!(
            "(let* [a (atom [])] [(run! (partial swap! a conj) {c}) (deref a)])"
        ));
    }
    cases.push("(reduce-kv assoc 0)".to_owned());
    cases.push("(run! identity)".to_owned());
    assert_parity(&cases);
}
