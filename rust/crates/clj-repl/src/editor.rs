// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Tty line-editing adapter over `rustyline` (slice S7, step S7C).
//!
//! This module owns everything the REPL needs on an interactive terminal —
//! line editing, persistent history, tab completion, Ctrl-C/Ctrl-D handling,
//! and multiline input — while the sibling session module owns
//! prompt/read/eval/print over generic IO. The adapter feeds complete input
//! units ("lines", possibly multiline) to the session through [`LineHandler`].
//!
//! ```text
//! Wiring (for main; the sibling `Session` implements the handler side):
//!
//!   let source = RustyLineSource::new(core_names(), Session::is_complete_form)?;
//!   let mut editor = TtyEditor::with_history(source, history_options_from_cli());
//!   let mut handler = SessionHandler::new(&mut session);
//!   exit(editor.run(&|| session.prompt_string(), &mut handler));
//! ```
//!
//! Contracts:
//!
//! * [`LineHandler::handle_line`] receives one complete input unit per call
//!   (newlines preserved for multiline forms) and answers [`LoopAction`].
//! * The `prompt` closure is called fresh before every read so namespace
//!   changes are reflected immediately.
//! * History loads on start and appends on exit; every history/tty failure
//!   warns on stderr and the session continues (D3). No `unwrap`/`expect`
//!   on any IO path in the loop.
//! * Ctrl-C cancels the current input (a partial multiline buffer is
//!   discarded) and re-prompts; Ctrl-D on empty input exits 0, discarding an
//!   incomplete buffer.
//! * The `rustyline` readline call hides behind [`LineSource`] so tests stub
//!   it without a tty; only [`RustyLineSource::readline`] needs a terminal.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::{DefaultHistory, History};
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Editor, Helper};

// ---------------------------------------------------------------------------
// Session interface
// ---------------------------------------------------------------------------

/// What the [`LineHandler`] wants the loop to do after one submitted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    /// Read another line (fresh prompt).
    Continue,
    /// Append history and return from [`TtyEditor::run`] with exit code 0.
    Exit,
}

/// Minimal callback interface between [`TtyEditor`] and the REPL session.
///
/// The sibling session implements this: `handle_line` evals one complete
/// input unit, prints the result, and usually returns [`LoopAction::Continue`].
/// A `:quit`-style command (or whatever main decides) returns
/// [`LoopAction::Exit`].
///
/// Prompt rendering is intentionally not part of this trait: [`TtyEditor::run`]
/// takes a separate `prompt` closure so the `<ns>=> ` text stays owned by the
/// session and is re-rendered before every read.
pub trait LineHandler {
    /// Handle one complete input unit (no trailing newline; embedded
    /// newlines preserved for multiline forms).
    fn handle_line(&mut self, line: &str) -> LoopAction;
}

/// Outcome of a single readline attempt, free of any tty types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// A line of input, without the trailing newline.
    Line(String),
    /// Ctrl-C: cancel the current input, show a fresh prompt.
    Interrupted,
    /// Ctrl-D on empty input: exit 0.
    Eof,
    /// Any other backend failure; the loop warns and continues.
    Error(String),
}

/// Seam hiding the `rustyline` readline call so tests can stub it.
///
/// The real implementation is [`RustyLineSource`]; tests replay scripted
/// [`ReadOutcome`]s through a fake. Only `readline` needs a tty — history
/// and `is_complete` work headless.
pub trait LineSource {
    /// Read one line under `prompt`.
    fn readline(&mut self, prompt: &str) -> ReadOutcome;
    /// Whether `input` is a complete unit, ready to submit. On a tty the
    /// same predicate backs the `rustyline` validator (Enter continues the
    /// form while it reports incomplete); the loop rechecks it so piped and
    /// stubbed sources accumulate identically.
    fn is_complete(&self, input: &str) -> bool;
    /// Load history from `path`. A missing file is a first run, not an
    /// error (`Ok`); an unreadable file reports `Err(message)`.
    fn load_history(&mut self, path: &Path) -> Result<(), String>;
    /// Append new entries to `path`, creating parent dirs as needed.
    fn append_history(&mut self, path: &Path) -> Result<(), String>;
    /// Record a submitted line in memory (the file write happens at exit).
    fn add_history_entry(&mut self, line: &str);
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

/// True for chars that can never appear inside a completable word.
///
/// `/` is deliberately not a delimiter so namespace-qualified prefixes like
/// `foo/ba` complete as one word; `.`, `:`, `-`, `?` and friends likewise
/// stay inside words so `str/`, `:key` and `map?` behave.
fn is_word_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '"'
                | '\''
                | '`'
                | ','
                | ';'
                | '~'
                | '@'
                | '^'
                | '#'
        )
}

/// Prefix-complete `line[..pos]` against `names`.
///
/// Returns `(word_start, matches)`: the byte index where the word being
/// completed starts, and the matching names in sorted order. `pos` is
/// clamped to `line.len()` and snapped back to a char boundary, so callers
/// passing a stale cursor never panic.
#[must_use]
pub fn complete_names(names: &[String], line: &str, pos: usize) -> (usize, Vec<String>) {
    let mut pos = pos.min(line.len());
    while pos > 0 && !line.is_char_boundary(pos) {
        pos -= 1;
    }
    let head = &line[..pos];
    let start = head
        .char_indices()
        .rev()
        .find(|(_, c)| is_word_delimiter(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let prefix = &head[start..];
    let mut matches: Vec<String> = names
        .iter()
        .filter(|name| name.starts_with(prefix))
        .cloned()
        .collect();
    matches.sort();
    matches.dedup();
    (start, matches)
}

/// Tab completer over a fixed name list (e.g. interned `clojure.core` names).
#[derive(Debug, Clone, Default)]
pub struct NameCompleter {
    names: Vec<String>,
}

impl NameCompleter {
    /// Build a completer over `names`.
    #[must_use]
    pub fn new(names: Vec<String>) -> Self {
        Self { names }
    }

    /// Complete `line[..pos]`; same contract as [`complete_names`].
    #[must_use]
    pub fn complete_names(&self, line: &str, pos: usize) -> (usize, Vec<String>) {
        complete_names(&self.names, line, pos)
    }
}

impl Completer for NameCompleter {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        Ok(self.complete_names(line, pos))
    }
}

// ---------------------------------------------------------------------------
// rustyline helper (completion + multiline validation)
// ---------------------------------------------------------------------------

/// `rustyline` helper: [`NameCompleter`] plus an injectable completeness
/// check driving multiline editing. Hints and highlighting stay passive
/// (trait defaults).
#[derive(Clone)]
pub struct ReplHelper {
    completer: NameCompleter,
    is_complete: Arc<dyn Fn(&str) -> bool>,
}

impl ReplHelper {
    /// `is_complete` is the sibling session's incomplete-form check; any
    /// closure works (tests use simple predicates).
    pub fn new(names: Vec<String>, is_complete: impl Fn(&str) -> bool + 'static) -> Self {
        Self {
            completer: NameCompleter::new(names),
            is_complete: Arc::new(is_complete),
        }
    }

    /// Whether `input` is a complete unit. Shared by the validator below
    /// and (via the source) the run loop, so both agree.
    #[must_use]
    pub fn is_complete(&self, input: &str) -> bool {
        (self.is_complete)(input)
    }
}

impl Completer for ReplHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        self.completer.complete(line, pos, ctx)
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {}

impl Validator for ReplHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        if self.is_complete(ctx.input()) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Helper for ReplHelper {}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// Default history file: `$HOME/.clojure-rs/history`, falling back to
/// `%USERPROFILE%` when `HOME` is unset. No new deps; `Path::join` keeps
/// separators Windows-safe. Returns `None` when no home resolves.
#[must_use]
pub fn default_history_path() -> Option<PathBuf> {
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))?;
    if home.is_empty() {
        return None;
    }
    Some(Path::new(&home).join(".clojure-rs").join("history"))
}

/// History persistence policy for [`TtyEditor`].
#[derive(Debug, Clone)]
pub struct HistoryOptions {
    path: Option<PathBuf>,
    enabled: bool,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        Self {
            path: None,
            enabled: true,
        }
    }
}

impl HistoryOptions {
    /// Persist to `path` instead of [`default_history_path`] (`--history-file`).
    #[must_use]
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            enabled: true,
        }
    }

    /// Disable history load/append entirely (`--no-history`).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            path: None,
            enabled: false,
        }
    }

    /// Resolve the effective path, or `None` when disabled or unresolvable
    /// (in which case the session simply runs without history).
    #[must_use]
    pub fn resolve_path(&self) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        if let Some(path) = &self.path {
            return Some(path.clone());
        }
        default_history_path()
    }
}

// ---------------------------------------------------------------------------
// rustyline-backed source
// ---------------------------------------------------------------------------

/// Real [`LineSource`] over `rustyline::Editor`. Constructing it is safe
/// without a tty; only `readline` needs one.
pub struct RustyLineSource {
    editor: Editor<ReplHelper, DefaultHistory>,
    is_complete: Arc<dyn Fn(&str) -> bool>,
}

impl RustyLineSource {
    /// Build the editor and install the [`ReplHelper`]. Fails only if the
    /// terminal backend cannot initialize; the error is a plain string so
    /// callers can warn instead of panic.
    pub fn new(
        names: Vec<String>,
        is_complete: impl Fn(&str) -> bool + 'static,
    ) -> Result<Self, String> {
        let is_complete: Arc<dyn Fn(&str) -> bool> = Arc::new(is_complete);
        let helper = ReplHelper {
            completer: NameCompleter::new(names),
            is_complete: Arc::clone(&is_complete),
        };
        let mut editor = Editor::new().map_err(|err| err.to_string())?;
        editor.set_helper(Some(helper));
        Ok(Self {
            editor,
            is_complete,
        })
    }

    /// Number of in-memory history entries (history round-trip assertions).
    #[must_use]
    pub fn history_len(&self) -> usize {
        self.editor.history().len()
    }
}

impl LineSource for RustyLineSource {
    fn readline(&mut self, prompt: &str) -> ReadOutcome {
        match self.editor.readline(prompt) {
            Ok(line) => ReadOutcome::Line(line),
            Err(ReadlineError::Eof) => ReadOutcome::Eof,
            Err(ReadlineError::Interrupted) => ReadOutcome::Interrupted,
            Err(other) => ReadOutcome::Error(other.to_string()),
        }
    }

    fn is_complete(&self, input: &str) -> bool {
        (self.is_complete)(input)
    }

    fn load_history(&mut self, path: &Path) -> Result<(), String> {
        // A missing file is a first run, not a failure.
        if !path.exists() {
            return Ok(());
        }
        self.editor
            .load_history(path)
            .map_err(|err| err.to_string())
    }

    fn append_history(&mut self, path: &Path) -> Result<(), String> {
        // MSRV 1.85: no let-chains, so filter-then-bind instead of nesting.
        let missing_parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty() && !p.exists());
        if let Some(parent) = missing_parent {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        self.editor
            .append_history(path)
            .map_err(|err| err.to_string())
    }

    fn add_history_entry(&mut self, line: &str) {
        // Admission (blanks/dupes) is the backend's call, and an in-memory
        // add cannot fail with IO; discard the outcome either way.
        let _ = self.editor.add_history_entry(line);
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

/// Tty REPL driver: prompt → readline → accumulate → [`LineHandler`].
pub struct TtyEditor<S> {
    source: S,
    history: HistoryOptions,
}

impl<S: LineSource> TtyEditor<S> {
    /// Default history location, enabled.
    pub fn new(source: S) -> Self {
        Self {
            source,
            history: HistoryOptions::default(),
        }
    }

    /// Explicit history policy (`--history-file` / `--no-history`).
    pub fn with_history(source: S, history: HistoryOptions) -> Self {
        Self { source, history }
    }

    /// Borrow the line source (lets tests inspect a stub after the run).
    pub fn source(&self) -> &S {
        &self.source
    }

    /// Load history, warning (not failing) when the file is unreadable.
    fn warn_load_history(&mut self, path: &Path) {
        if let Err(msg) = self.source.load_history(path) {
            warn_history("cannot load history file", path, &msg);
        }
    }

    /// Append history, warning (not failing) when the file is unwritable.
    fn warn_append_history(&mut self, path: &Path) {
        if let Err(msg) = self.source.append_history(path) {
            warn_history("cannot save history file", path, &msg);
        }
    }

    /// Run the loop until EOF or [`LoopAction::Exit`]; returns the process
    /// exit code (0).
    ///
    /// * `prompt` renders each prompt and is called fresh before every read.
    /// * Reads accumulate into one buffer until `is_complete` accepts it;
    ///   Ctrl-C clears a partial buffer, Ctrl-D exits 0 (an incomplete
    ///   buffer is discarded — `rustyline` only reports EOF on empty input).
    /// * Every completed line is submitted to `handler`; non-empty ones are
    ///   also recorded in history.
    /// * History load/append failures warn on stderr and the session
    ///   continues; readline backend errors warn, clear the buffer, and
    ///   re-prompt.
    pub fn run(&mut self, prompt: &dyn Fn() -> String, handler: &mut impl LineHandler) -> i32 {
        let history_path = self.history.resolve_path();
        if let Some(path) = &history_path {
            self.warn_load_history(path);
        }

        let mut buffer = String::new();
        loop {
            let line_prompt = prompt();
            match self.source.readline(&line_prompt) {
                ReadOutcome::Line(line) => {
                    if buffer.is_empty() {
                        buffer.push_str(&line);
                    } else {
                        buffer.push('\n');
                        buffer.push_str(&line);
                    }
                    if self.source.is_complete(&buffer) {
                        let submitted = std::mem::take(&mut buffer);
                        if !submitted.is_empty() {
                            self.source.add_history_entry(&submitted);
                        }
                        if matches!(handler.handle_line(&submitted), LoopAction::Exit) {
                            break;
                        }
                    }
                }
                ReadOutcome::Interrupted => {
                    buffer.clear();
                }
                ReadOutcome::Eof => {
                    break;
                }
                ReadOutcome::Error(msg) => {
                    eprintln!("clojure-rs: warning: input error: {msg}");
                    buffer.clear();
                }
            }
        }

        if let Some(path) = &history_path {
            self.warn_append_history(path);
        }
        0
    }
}

fn warn_history(what: &str, path: &Path, msg: &str) {
    eprintln!("clojure-rs: warning: {what} {}: {msg}", path.display());
}
