// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Integration tests for the tty editor adapter (`clj_repl::editor`, S7C).
//!
//! Nothing here requires a tty: the `rustyline` readline call sits behind
//! the `LineSource` seam, which these tests stub with a scripted fake, and
//! the completion/history helpers are exercised directly. Only
//! `RustyLineSource::readline` itself needs a real terminal, and it is
//! deliberately never called here (`rustyline` is an untested dependency).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clj_repl::editor::{
    HistoryOptions, LineHandler, LineSource, LoopAction, NameCompleter, ReadOutcome, ReplHelper,
    RustyLineSource, TtyEditor, complete_names, default_history_path,
};
use rustyline::Context;
use rustyline::completion::Completer;
use rustyline::history::MemHistory;

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// Scripted `LineSource`: replays `ReadOutcome`s and records history traffic.
struct FakeSource {
    script: VecDeque<ReadOutcome>,
    complete: fn(&str) -> bool,
    load_result: Result<(), String>,
    append_result: Result<(), String>,
    loads: Vec<PathBuf>,
    appends: Vec<PathBuf>,
    entries: Vec<String>,
    prompts_seen: Vec<String>,
}

impl FakeSource {
    fn new(script: Vec<ReadOutcome>, complete: fn(&str) -> bool) -> Self {
        Self {
            script: script.into(),
            complete,
            load_result: Ok(()),
            append_result: Ok(()),
            loads: Vec::new(),
            appends: Vec::new(),
            entries: Vec::new(),
            prompts_seen: Vec::new(),
        }
    }

    fn with_load(mut self, result: Result<(), String>) -> Self {
        self.load_result = result;
        self
    }

    fn with_append(mut self, result: Result<(), String>) -> Self {
        self.append_result = result;
        self
    }
}

impl LineSource for FakeSource {
    fn readline(&mut self, prompt: &str) -> ReadOutcome {
        self.prompts_seen.push(prompt.to_string());
        self.script.pop_front().unwrap_or(ReadOutcome::Eof)
    }

    fn is_complete(&self, input: &str) -> bool {
        (self.complete)(input)
    }

    fn load_history(&mut self, path: &Path) -> Result<(), String> {
        self.loads.push(path.to_path_buf());
        self.load_result.clone()
    }

    fn append_history(&mut self, path: &Path) -> Result<(), String> {
        self.appends.push(path.to_path_buf());
        self.append_result.clone()
    }

    fn add_history_entry(&mut self, line: &str) {
        self.entries.push(line.to_string());
    }
}

/// `LineHandler` that records submitted lines and optionally exits.
struct RecordingHandler {
    lines: Vec<String>,
    exit_after: Option<usize>,
}

impl RecordingHandler {
    fn new(exit_after: Option<usize>) -> Self {
        Self {
            lines: Vec::new(),
            exit_after,
        }
    }
}

impl LineHandler for RecordingHandler {
    fn handle_line(&mut self, line: &str) -> LoopAction {
        self.lines.push(line.to_string());
        if self.exit_after == Some(self.lines.len()) {
            LoopAction::Exit
        } else {
            LoopAction::Continue
        }
    }
}

fn always_complete(_: &str) -> bool {
    true
}

fn prompt() -> String {
    "user=> ".to_string()
}

// ---------------------------------------------------------------------------
// Temp dirs (unique per test; removed on drop even on failure)
// ---------------------------------------------------------------------------

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("clj-repl-editor-{tag}-{pid}-{id}"));
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

fn core_names() -> Vec<String> {
    [
        "map", "mapcat", "filter", "foo/bar", "foo/baz", "str", "+", "defn",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

#[test]
fn completion_prefix_table() {
    let names = core_names();
    // (line, pos, expected start, expected matches)
    let cases: &[(&str, usize, usize, &[&str])] = &[
        ("ma", 2, 0, &["map", "mapcat"]),
        ("(str ma", 7, 5, &["map", "mapcat"]),
        ("foo/b", 5, 0, &["foo/bar", "foo/baz"]),
        ("(foo/ba", 7, 1, &["foo/bar", "foo/baz"]),
        ("foo/", 4, 0, &["foo/bar", "foo/baz"]),
        ("zzz", 3, 0, &[]),
        (
            "(+ ",
            3,
            3,
            &[
                "+", "defn", "filter", "foo/bar", "foo/baz", "map", "mapcat", "str",
            ],
        ),
        ("map x", 3, 0, &["map", "mapcat"]),
        ("(map", 4, 1, &["map", "mapcat"]),
        ("\"str", 4, 1, &["str"]),
        (":ke", 3, 0, &[]),
    ];
    for (line, pos, start, expected) in cases {
        let (got_start, got) = complete_names(&names, line, *pos);
        assert_eq!(got_start, *start, "word start for {line:?} @ {pos}");
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(got, expected, "matches for {line:?} @ {pos}");
    }
}

#[test]
fn completion_clamps_pos_to_char_boundary() {
    let names = core_names();
    // pos 1 is mid-`é`: snaps back to 0, so the prefix is empty and every
    // name matches.
    let (start, matches) = complete_names(&names, "é", 1);
    assert_eq!(start, 0);
    assert_eq!(matches.len(), names.len());
    // pos past the end clamps to the end.
    let (start, matches) = complete_names(&names, "é", 99);
    assert_eq!(start, 0);
    assert!(matches.is_empty());
}

#[test]
fn completer_trait_impl_agrees_with_free_fn() {
    let names = core_names();
    let completer = NameCompleter::new(names.clone());
    let history = MemHistory::new();
    let ctx = Context::new(&history);
    let (start, candidates) = completer
        .complete("(foo/ba", 7, &ctx)
        .expect("completion cannot fail");
    assert_eq!(start, 1);
    assert_eq!(
        candidates,
        vec!["foo/bar".to_string(), "foo/baz".to_string()]
    );
    assert_eq!(completer.complete_names("(foo/ba", 7), (start, candidates));
}

#[test]
fn helper_and_source_share_is_complete() {
    let helper = ReplHelper::new(core_names(), |s: &str| s.ends_with(')'));
    assert!(!helper.is_complete("(def x"));
    assert!(helper.is_complete("(def x)"));

    let source = RustyLineSource::new(core_names(), |s: &str| s.ends_with(')'))
        .expect("backend constructs without a tty");
    assert!(!source.is_complete("(def x"));
    assert!(source.is_complete("(def x)"));
}

#[test]
fn real_vocabulary_covers_core_and_session_vars() {
    // The completion vocabulary over a booted evaluator (not the
    // 8-name stub above), sorted + deduped. The floor is 300: the
    // port interns ~350 `clojure.core` publics (349 observed —
    // the JVM holds 600+, but the test pins the port's own
    // vocabulary, not the JVM's).
    let mut eval = clj_eval::boot();
    let names = clj_repl::core_names(&mut eval);
    assert!(names.len() >= 300, "vocabulary size: {}", names.len());
    for want in ["map", "defn", "*1", "*2", "*3", "*e"] {
        assert!(names.contains(&want.to_owned()), "missing {want}");
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(names, sorted, "vocabulary sorted + deduped");
}

// ---------------------------------------------------------------------------
// History (real rustyline file backend; no tty needed)
// ---------------------------------------------------------------------------

#[test]
fn history_round_trip_in_temp_dir() {
    let dir = TempDir::new("roundtrip");
    // Parent does not exist yet: append must create it.
    let path = dir.path.join("nested").join("history");

    let mut first = RustyLineSource::new(Vec::new(), always_complete)
        .expect("backend constructs without a tty");
    assert!(first.load_history(&path).is_ok());
    first.add_history_entry("(def x 1)");
    first.add_history_entry("(+ 1 2)");
    first
        .append_history(&path)
        .expect("append creates parents and file");
    assert!(path.is_file());

    let mut second = RustyLineSource::new(Vec::new(), always_complete)
        .expect("backend constructs without a tty");
    second.load_history(&path).expect("load round-trips");
    assert_eq!(second.history_len(), 2);
}

#[test]
fn history_append_to_unwritable_path_errors() {
    let dir = TempDir::new("unwritable");
    std::fs::create_dir_all(&dir.path).expect("mkdir temp dir");
    let mut source = RustyLineSource::new(Vec::new(), always_complete)
        .expect("backend constructs without a tty");
    // Queue an entry so append must actually open the file (an empty append
    // short-circuits). A directory is not writable on any platform.
    source.add_history_entry("(+ 1 2)");
    let err = source
        .append_history(&dir.path)
        .expect_err("append to a directory must fail");
    assert!(!err.is_empty());
}

#[test]
fn history_failures_warn_and_continue() {
    let dir = TempDir::new("warn");
    let mut editor = TtyEditor::with_history(
        FakeSource::new(
            vec![ReadOutcome::Line("(+ 1 2)".to_string()), ReadOutcome::Eof],
            always_complete,
        )
        .with_load(Err("disk on fire".to_string()))
        .with_append(Err("read-only".to_string())),
        HistoryOptions::with_path(dir.path.join("history")),
    );
    let mut handler = RecordingHandler::new(None);
    // Warnings go to stderr (visible with --nocapture); the contract under
    // test is that the session continues and exits normally.
    let code = editor.run(&prompt, &mut handler);
    assert_eq!(code, 0);
    assert_eq!(handler.lines, vec!["(+ 1 2)".to_string()]);
    assert_eq!(editor.source().loads.len(), 1);
    assert_eq!(editor.source().appends.len(), 1);
}

#[test]
fn history_disabled_skips_all_history_io() {
    let mut editor = TtyEditor::with_history(
        FakeSource::new(vec![ReadOutcome::Eof], always_complete),
        HistoryOptions::disabled(),
    );
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert!(editor.source().loads.is_empty());
    assert!(editor.source().appends.is_empty());
    assert!(handler.lines.is_empty());
}

#[test]
fn history_override_path_is_used_for_load_and_append() {
    let dir = TempDir::new("override");
    let path = dir.path.join("history");
    let mut editor = TtyEditor::with_history(
        FakeSource::new(vec![ReadOutcome::Eof], always_complete),
        HistoryOptions::with_path(path.clone()),
    );
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(editor.source().loads, vec![path.clone()]);
    assert_eq!(editor.source().appends, vec![path]);
}

#[test]
fn default_history_option_matches_helper() {
    assert_eq!(
        HistoryOptions::default().resolve_path(),
        default_history_path()
    );
    assert_eq!(HistoryOptions::disabled().resolve_path(), None);
    // No env mutation here (parallel tests share the process): only assert
    // the shape when a home dir resolves in this environment.
    if let Some(path) = default_history_path() {
        assert_eq!(path.file_name().and_then(|s| s.to_str()), Some("history"));
        let parent_name = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str());
        assert_eq!(parent_name, Some(".clojure-rs"));
    }
}

#[test]
fn history_tilde_expands_against_home() {
    // No env mutation (see above): read HOME, then assert the
    // `~/` expansion agrees with it.
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return;
    };
    let home = PathBuf::from(home);
    assert_eq!(
        HistoryOptions::with_path("~/hist").resolve_path(),
        Some(home.join("hist"))
    );
    assert_eq!(HistoryOptions::with_path("~").resolve_path(), Some(home));
    // Non-leading tildes pass through (the shell's job).
    assert_eq!(
        HistoryOptions::with_path("/x/~/hist").resolve_path(),
        Some(PathBuf::from("/x/~/hist"))
    );
}

// ---------------------------------------------------------------------------
// Loop: interrupt / EOF / multiline / exit
// ---------------------------------------------------------------------------

#[test]
fn interrupt_cancels_input_and_continues() {
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![
            ReadOutcome::Line("(def x 1)".to_string()),
            ReadOutcome::Interrupted,
            ReadOutcome::Line("(+ 1 2)".to_string()),
            ReadOutcome::Eof,
        ],
        always_complete,
    ));
    let mut handler = RecordingHandler::new(None);
    let code = editor.run(&prompt, &mut handler);
    assert_eq!(code, 0);
    assert_eq!(
        handler.lines,
        vec!["(def x 1)".to_string(), "(+ 1 2)".to_string()]
    );
    // Only submitted lines reach history, and every read re-renders the prompt.
    assert_eq!(editor.source().entries, handler.lines);
    assert_eq!(editor.source().prompts_seen, vec![prompt(); 4]);
}

#[test]
fn interrupt_clears_partial_multiline_buffer() {
    fn done(s: &str) -> bool {
        s == "done"
    }
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![
            ReadOutcome::Line("aaa".to_string()),
            ReadOutcome::Interrupted,
            ReadOutcome::Line("done".to_string()),
            ReadOutcome::Eof,
        ],
        done,
    ));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec!["done".to_string()]);
}

#[test]
fn incomplete_input_accumulates_until_complete() {
    fn balanced(s: &str) -> bool {
        s.contains(')')
    }
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![
            ReadOutcome::Line("(def x".to_string()),
            ReadOutcome::Line("1)".to_string()),
            ReadOutcome::Eof,
        ],
        balanced,
    ));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec!["(def x\n1)".to_string()]);
    assert_eq!(editor.source().entries, handler.lines);
}

#[test]
fn complete_input_submits_immediately() {
    fn balanced(s: &str) -> bool {
        s.contains(')')
    }
    let mut editor = TtyEditor::with_history(
        FakeSource::new(
            vec![ReadOutcome::Line("(+ 1 2)".to_string()), ReadOutcome::Eof],
            balanced,
        ),
        HistoryOptions::disabled(),
    );
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec!["(+ 1 2)".to_string()]);
}

#[test]
fn exit_action_stops_loop() {
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![
            ReadOutcome::Line("first".to_string()),
            ReadOutcome::Line("never-read".to_string()),
            ReadOutcome::Eof,
        ],
        always_complete,
    ));
    let mut handler = RecordingHandler::new(Some(1));
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec!["first".to_string()]);
    // History is still appended on the Exit path.
    assert_eq!(editor.source().appends.len(), 1);
}

#[test]
fn eof_exits_zero_with_no_calls() {
    let mut editor = TtyEditor::new(FakeSource::new(vec![ReadOutcome::Eof], always_complete));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert!(handler.lines.is_empty());
}

#[test]
fn eof_discards_incomplete_buffer() {
    fn never(_: &str) -> bool {
        false
    }
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![ReadOutcome::Line("aaa".to_string()), ReadOutcome::Eof],
        never,
    ));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert!(handler.lines.is_empty());
    assert!(editor.source().entries.is_empty());
}

#[test]
fn readline_error_warns_and_continues() {
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![
            ReadOutcome::Error("boom".to_string()),
            ReadOutcome::Line("ok".to_string()),
            ReadOutcome::Eof,
        ],
        always_complete,
    ));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec!["ok".to_string()]);
}

#[test]
fn empty_lines_submit_but_skip_history() {
    let mut editor = TtyEditor::new(FakeSource::new(
        vec![ReadOutcome::Line(String::new()), ReadOutcome::Eof],
        always_complete,
    ));
    let mut handler = RecordingHandler::new(None);
    assert_eq!(editor.run(&prompt, &mut handler), 0);
    assert_eq!(handler.lines, vec![String::new()]);
    assert!(editor.source().entries.is_empty());
}

#[test]
fn helper_complete_delegates_to_names() {
    let helper = ReplHelper::new(core_names(), always_complete);
    let history = MemHistory::new();
    let ctx = Context::new(&history);
    let (start, candidates) = helper
        .complete("(foo/ba", 7, &ctx)
        .expect("completion cannot fail");
    assert_eq!(start, 1);
    assert_eq!(
        candidates,
        vec!["foo/bar".to_string(), "foo/baz".to_string()]
    );
}

#[test]
fn core_names_without_boot_yields_session_vars_only() {
    // On a non-booted evaluator the vocabulary query cannot succeed
    // (`into`/`keys`/`ns-publics` resolve nothing in the lone `user`
    // ns), so only the four sorted seeds remain.
    let mut eval = clj_eval::Evaluator::new("test");
    assert_eq!(
        clj_repl::core_names(&mut eval),
        vec![
            "*1".to_owned(),
            "*2".to_owned(),
            "*3".to_owned(),
            "*e".to_owned()
        ]
    );
}
