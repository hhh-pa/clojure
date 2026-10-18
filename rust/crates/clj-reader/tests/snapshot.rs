// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Refresh the reviewed `:ours` snapshots (`tests/corpus/ours.edn`)
//! for `:diverge` cases. Gated on `UPDATE_OURS=1` (plain `cargo test`
//! skips); always review the diff — a snapshot change is either an
//! intended behavior change or a regression.

#[path = "harness.rs"]
mod harness;

use std::path::PathBuf;

use harness::{
    Outcome, case_opts, our_err, our_outcome, parse_cases, parse_resolver, read_data_file, run_ours,
};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn edn_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (0x7F..=0x9F).contains(&(c as u32)) => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn edn_outcome(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Ok(forms) => {
            let mut s = String::from("{:ok [");
            for f in forms {
                let render = |v: &Option<String>| match v {
                    Some(text) => edn_escape(text),
                    None => ":unrepresentable".to_owned(),
                };
                s.push_str(&format!(
                    "{{:raw {} :canon {} :canon-meta {}}} ",
                    render(&f.raw),
                    render(&f.canon),
                    render(&f.canon_meta)
                ));
            }
            s.push_str("]}");
            s
        }
        Outcome::Err(e) => {
            let pos = |v: Option<u32>| v.map_or("nil".to_owned(), |n| n.to_string());
            format!(
                "{{:err {{:class {} :message {} :line {} :col {}}}}}",
                edn_escape(&e.class),
                edn_escape(&e.message),
                pos(e.line),
                pos(e.col)
            )
        }
    }
}

#[test]
fn snapshot_ours() {
    if std::env::var("UPDATE_OURS").as_deref() != Ok("1") {
        eprintln!("skipped (UPDATE_OURS=1 to refresh ours.edn)");
        return;
    }
    let dir = corpus_dir();
    let cases = parse_cases(&read_data_file(&dir.join("cases.edn")));
    let resolver = parse_resolver(&read_data_file(&dir.join("expectations.edn")));

    let mut ids: Vec<&str> = cases
        .iter()
        .filter(|c| c.diverge.is_some())
        .map(|c| c.id.as_str())
        .collect();
    ids.sort_unstable();

    let mut out = String::from(";; Reviewed :ours snapshots for :diverge cases.\n");
    out.push_str(";; Refresh: UPDATE_OURS=1 cargo test -p clj-reader --test snapshot\n");
    out.push('{');
    for id in &ids {
        let case = cases.iter().find(|c| &c.id == id).unwrap();
        let opts = case_opts(case, &resolver);
        let outcome = match run_ours(&case.input, &opts) {
            Ok(values) => our_outcome(values),
            Err(e) => our_err(&e),
        };
        out.push_str(&format!("{} {} ", edn_escape(id), edn_outcome(&outcome)));
    }
    out.push_str("}\n");
    std::fs::write(dir.join("ours.edn"), out).unwrap();
    eprintln!("wrote {} snapshots", ids.len());
}
