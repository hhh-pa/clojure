// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure-rs`: native Clojure runtime, REPL, and LLVM AOT compiler.
//!
//! Slices S1+S7A: CLI shell plus `eval`/`run` wiring over the booted
//! `clj-eval` core. `repl` lands in S7B/C, `build` in S8 (spec
//! `docs/t1174/`).

use clap::{Parser, Subcommand};
use clj_eval::{SourceError, boot};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "clojure-rs",
    version,
    about = "Native Clojure runtime, REPL, and compiler"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Evaluate a single form and print the result.
    Eval {
        /// The form to evaluate.
        form: String,
        /// Restrict the reader to EDN.
        #[arg(long)]
        edn: bool,
    },
    /// Load and run a script file.
    Run {
        /// The script file to run.
        file: PathBuf,
        /// Bound to `*command-line-args*`.
        args: Vec<String>,
    },
    /// Start the interactive REPL.
    Repl {
        /// History file (default `~/.clojure-rs/history`).
        #[arg(long)]
        history_file: Option<PathBuf>,
        /// Disable history read/write.
        #[arg(long)]
        no_history: bool,
    },
    /// AOT-compile the pure subset to a native binary.
    Build {
        /// The source file to compile.
        file: PathBuf,
        /// The native binary to write.
        #[arg(short, long)]
        output: PathBuf,
    },
}

/// Exit codes per spec C8: 0 ok, 1 evaluation/compile error,
/// 2 usage error (clap's default), 3 I/O error.
fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Eval { form, edn } => cmd_eval(&form, edn),
        Command::Run { file, args } => cmd_run(&file, &args),
        Command::Repl {
            history_file,
            no_history,
        } => cmd_repl(history_file, no_history),
        Command::Build { file, output } => {
            eprintln!(
                "build: not yet implemented (lands in slice S8): {} -> {} [{}]",
                file.display(),
                output.display(),
                llvm_backend(),
            );
            ExitCode::from(1)
        }
    }
}

/// The `<eval>` file token (pins P5).
const EVAL_FILE: &str = "<eval>";

/// `eval` evaluates `(prn (do <input>))` rather than `<input>` bare:
/// the binary crate cannot print the returned value itself (neither
/// the printer nor the value type is a dependency of the binary, and
/// S7A may not touch `Cargo.toml`), so printing goes through the
/// evaluator's own `prn` — which also realizes lazy seqs, like the
/// JVM's print walk. Observable effects:
/// - single-form input (spec 1.4's contract) prints exactly its value;
/// - multi-form input evaluates every form and prints the LAST (the
///   JVM `-e` prints EVERY form — a documented gap until a one-form
///   reader API exists);
/// - spans on line 1 shift by the prefix width ([`dewrap_col`]).
const EVAL_PREFIX: &str = "(prn (do ";

fn cmd_eval(form: &str, edn: bool) -> ExitCode {
    let scan = EvalScanner::new(form).run();
    // (Match guard, not a let-chain: MSRV 1.85.)
    match &scan.edn_violation {
        Some(v) if edn => {
            eprintln!(
                "{EVAL_FILE}:{}:{}: non-EDN reader syntax: '{}'",
                v.line, v.col, v.lexeme
            );
            return ExitCode::from(1);
        }
        _ => {}
    }
    if !scan.has_code {
        // S7A decision, probed against the JVM
        // (1.13.0-master-SNAPSHOT): `clojure.main -e ''` exits 0
        // printing nothing, but spec 1.4 promises ONE form and empty
        // input holds none, so the port reports exit 1 with a
        // diagnostic instead.
        eprintln!("{EVAL_FILE}:1:1: empty input: nothing to evaluate");
        return ExitCode::from(1);
    }
    if let Some((open_line, end_line, end_col)) = scan.unclosed_eof {
        // The wrap's closers would silently balance these (or skew
        // the reader's span), so unbalanced input never reaches the
        // evaluator; the message mirrors the reader's own EOF text.
        eprintln!(
            "{EVAL_FILE}:{end_line}:{end_col}: EOF while reading, starting at line {open_line}"
        );
        return ExitCode::from(1);
    }
    let mut eval = boot();
    let wrapped = format!("{EVAL_PREFIX}{form}))");
    match eval.eval_source(&wrapped) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", eval_diagnostic(form, &e));
            ExitCode::from(1)
        }
    }
}

/// A `SourceError` as `file:line:col: message` (pins P5: col always
/// present). `Throwable` carries no span, so uncaught runtime errors
/// blame the single form's start (1:1).
fn eval_diagnostic(form: &str, e: &SourceError) -> String {
    let lens = line_lens(form);
    match e {
        SourceError::Reader(e) => {
            let span = e.span();
            let line = span.line;
            let col = dewrap_col(line, span.col, &lens);
            let msg = e.message();
            format!("{EVAL_FILE}:{line}:{col}: {msg}")
        }
        SourceError::Compile(e) => {
            let line = e.span.line;
            let col = dewrap_col(line, e.span.col, &lens);
            if e.bare {
                let class = e.class;
                let msg = &e.message;
                format!("{EVAL_FILE}:{line}:{col}: {class}: {msg}")
            } else {
                let wrapper = e.wrapper();
                let head = compile_head(&wrapper);
                let msg = &e.message;
                format!("{EVAL_FILE}:{line}:{col}: {head}: {msg}")
            }
        }
        SourceError::Uncaught(t) => {
            let class = t.class.jvm_name();
            let msg = t.message.as_deref().unwrap_or("");
            format!("{EVAL_FILE}:1:1: {class}: {msg}")
        }
        SourceError::Internal(msg) => format!("{EVAL_FILE}:1:1: internal error: {msg}"),
    }
}

/// Undo the [`EVAL_PREFIX`] shift and clamp to the input's own
/// width: line 1 loses the prefix width, and every line caps at
/// end-of-line + 1. (The form starts where the prefix ends, so
/// line-1 spans into it always clear the width; spans past end + 1
/// are suffix-absorption artifacts — the wrap's closers skew
/// EOF-position spans — so the clamp restores the input's own EOF.)
fn dewrap_col(line: u32, col: u32, lens: &[usize]) -> u32 {
    let shifted = if line == 1 {
        col.saturating_sub(EVAL_PREFIX.len() as u32).max(1)
    } else {
        col
    };
    let idx = (line.max(1) as usize) - 1;
    match lens.get(idx) {
        Some(len) => shifted
            .min(u32::try_from(len + 1).unwrap_or(u32::MAX))
            .max(1),
        None => shifted,
    }
}

/// Char lengths per normalized line (`\r\n`/`\r` break lines, like
/// the reader's stream).
fn line_lens(src: &str) -> Vec<usize> {
    let mut lens = vec![0usize];
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            lens.push(0);
            continue;
        }
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            lens.push(0);
            continue;
        }
        if let Some(last) = lens.last_mut() {
            *last += 1;
        }
    }
    lens
}

/// The wrapper line minus its ` at (site).` tail, e.g. `Syntax error
/// compiling if` (`CompileError::wrapper` always ends with one; the
/// whole line is the fallback).
fn compile_head(wrapper: &str) -> &str {
    match wrapper.rfind(" at (") {
        Some(i) => &wrapper[..i],
        None => wrapper,
    }
}

fn cmd_run(file: &Path, args: &[String]) -> ExitCode {
    if !file.is_file() {
        eprintln!("run: no such file: {}", file.display());
        return ExitCode::from(3);
    }
    let path = file.to_string_lossy();
    let mut eval = boot();
    // `*command-line-args*` is a dynamic `clojure.core` var (S7B);
    // bind the script's args around the `load-file` (pins P7).
    let inits = args_expr(args);
    let target = clj_string(&path);
    let script = format!("(binding [*command-line-args* {inits}] (load-file {target}))");
    match eval.eval_source(&script) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", run_diagnostic(&path, &e));
            ExitCode::from(1)
        }
    }
}

/// The `*command-line-args*` init (pins P4): nil without args, else a
/// quoted list of strings (a list value on read).
fn args_expr(args: &[String]) -> String {
    if args.is_empty() {
        return "nil".to_owned();
    }
    let mut out = "(quote (".to_owned();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&clj_string(arg));
    }
    out.push_str("))");
    out
}

/// A Rust string as a Clojure string literal.
fn clj_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The load wrapper's class: only `load-file` compile/reader failures
/// carry a parseable ` at (site).` tail.
const COMPILER_EXCEPTION: &str = "clojure.lang.Compiler$CompilerException";

/// A `run` failure as `file:line:col: message` naming the path as
/// passed. `load-file` wraps file errors as `... at (site).` with the
/// real message on the cause chain, so the site is recovered from the
/// wrapper text; anything else (notably span-less runtime throws)
/// blames 1:1.
fn run_diagnostic(path: &str, e: &SourceError) -> String {
    match e {
        SourceError::Reader(e) => {
            let msg = e.message();
            format!("{path}:1:1: {msg}")
        }
        SourceError::Compile(e) => {
            if e.bare {
                let class = e.class;
                let msg = &e.message;
                format!("{path}:1:1: {class}: {msg}")
            } else {
                let wrapper = e.wrapper();
                let head = compile_head(&wrapper);
                let msg = &e.message;
                format!("{path}:1:1: {head}: {msg}")
            }
        }
        SourceError::Uncaught(t) => {
            let outer = t.message.as_deref().unwrap_or("");
            let compiler = t.class.jvm_name() == COMPILER_EXCEPTION;
            match load_site(outer) {
                Some((line, col)) if compiler => {
                    let mut msg = outer;
                    let mut cur = t;
                    while let Some(next) = cur.cause.as_deref() {
                        if let Some(m) = next.message.as_deref() {
                            msg = m;
                        }
                        cur = next;
                    }
                    let head = compile_head(outer);
                    format!("{path}:{line}:{col}: {head}: {msg}")
                }
                _ => {
                    let class = t.class.jvm_name();
                    format!("{path}:1:1: {class}: {outer}")
                }
            }
        }
        SourceError::Internal(msg) => format!("{path}:1:1: internal error: {msg}"),
    }
}

/// Recover `(line, col)` from a load wrapper's trailing ` at (site).`
/// (`site` is `file:line:col`, so the last two `:` segments parse;
/// the file part itself may hold colons).
fn load_site(message: &str) -> Option<(u32, u32)> {
    let rest = message.strip_suffix(").")?;
    let i = rest.rfind(" at (")?;
    let site = &rest[i + 5..];
    let mut parts = site.rsplit(':');
    let col: u32 = parts.next()?.parse().ok()?;
    let line: u32 = parts.next()?.parse().ok()?;
    parts.next()?;
    Some((line, col))
}

/// The eval pre-scan: whether the input holds code outside comments,
/// the first non-EDN reader lexeme (if any), and the unbalanced-input
/// EOF site (if any: first unclosed opener's line plus the input's own
/// end position).
struct EvalScan {
    has_code: bool,
    edn_violation: Option<EdnViolation>,
    unclosed_eof: Option<(u32, u32, u32)>,
}

/// A non-EDN reader lexeme and its 1-based position.
struct EdnViolation {
    line: u32,
    col: u32,
    lexeme: String,
}

/// Whether a closer matches its opener kind.
fn opener_matches(open: char, close: char) -> bool {
    matches!((open, close), ('(', ')') | ('[', ']') | ('{', '}'))
}

/// One pass over eval input, tracking line/col while skipping string
/// bodies, `;` comments, and `\` char literals. Commas and ASCII
/// whitespace are blank (mirroring the reader's `is_whitespace` for
/// the ASCII range); entering a string counts as code, so an
/// unterminated string still reaches the reader's diagnostic. Openers
/// and closers also feed a depth stack so unclosed input (which the
/// eval wrap would silently balance) is reported instead of run.
struct EvalScanner<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    line: u32,
    col: u32,
    scan: EvalScan,
    stack: Vec<(char, u32)>,
    stack_clean: bool,
    ended_in_string: bool,
    bumpable: bool,
}

impl<'a> EvalScanner<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().peekable(),
            line: 1,
            col: 1,
            scan: EvalScan {
                has_code: false,
                edn_violation: None,
                unclosed_eof: None,
            },
            stack: Vec::new(),
            stack_clean: true,
            ended_in_string: false,
            bumpable: false,
        }
    }

    fn run(mut self) -> EvalScan {
        while let Some(c) = self.next() {
            let (at_line, at_col) = (self.line, self.col);
            self.advance(c);
            match c {
                '\n' => {}
                ';' => self.skip_comment(),
                '"' => {
                    self.scan.has_code = true;
                    self.skip_string_counted();
                }
                '\\' => {
                    self.scan.has_code = true;
                    self.skip_char_tail();
                }
                '#' => {
                    self.scan.has_code = true;
                    self.hash_dispatch(at_line, at_col);
                }
                '\'' | '`' | '~' | '@' | '^' => {
                    self.scan.has_code = true;
                    self.flag_edn(at_line, at_col, c.to_string());
                }
                '(' | '[' | '{' => {
                    self.scan.has_code = true;
                    self.stack.push((c, at_line));
                }
                ')' | ']' | '}' => {
                    self.scan.has_code = true;
                    self.close(c);
                }
                _ if c.is_ascii_whitespace() || c == ',' => {}
                _ => {
                    self.scan.has_code = true;
                }
            }
        }
        self.finish_unclosed();
        self.scan
    }

    /// One `#` dispatch: flag the `#x` lexeme as non-EDN (a regex
    /// body skins string rules — escapes honored — so its parens
    /// must not skew the depth count).
    fn hash_dispatch(&mut self, at_line: u32, at_col: u32) {
        match self.chars.peek().copied() {
            Some(d) if matches!(d, '(' | '"' | '\'' | '_' | '?' | '!' | '=' | ':') => {
                self.flag_edn(at_line, at_col, format!("#{d}"));
                self.next();
                self.advance(d);
                if d == '"' {
                    self.skip_string_counted();
                }
            }
            _ => {}
        }
    }

    /// Clean-but-unclosed input (still in code at EOF, not inside
    /// a string) earns the reader's EOF diagnostic; mismatches and
    /// stray closers go to the reader itself (clamped spans). The
    /// EOF site mirrors the reader's line-bump rule (col 1, line
    /// bumped past a non-empty last line).
    /// (Match guards, not let-chains: MSRV 1.85.)
    fn finish_unclosed(&mut self) {
        match self.stack.first() {
            Some((_, open_line)) if self.stack_clean && !self.ended_in_string => {
                let end_line = self.line + u32::from(self.bumpable);
                self.scan.unclosed_eof = Some((*open_line, end_line, 1));
            }
            _ => {}
        }
    }

    /// Pop one opener for a closer; underflow or kind mismatch marks
    /// the stack unclean (the reader reports those itself).
    fn close(&mut self, c: char) {
        match self.stack.pop() {
            Some((open, _)) if opener_matches(open, c) => {}
            _ => {
                self.stack_clean = false;
            }
        }
    }

    /// Next normalized char (`\r\n`/`\r` arrive as `\n`, like the
    /// reader's stream).
    fn next(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\r' {
            if self.chars.peek() == Some(&'\n') {
                self.chars.next();
            }
            return Some('\n');
        }
        Some(c)
    }

    /// Consume one already-read char's worth of position (plus the
    /// reader's EOF bump flag: set by content, cleared by newlines).
    fn advance(&mut self, c: char) {
        if c == '\n' {
            self.line += 1;
            self.col = 1;
            self.bumpable = false;
        } else {
            self.col += 1;
            self.bumpable = true;
        }
    }

    /// Record the first non-EDN lexeme only.
    fn flag_edn(&mut self, line: u32, col: u32, lexeme: String) {
        if self.scan.edn_violation.is_none() {
            self.scan.edn_violation = Some(EdnViolation { line, col, lexeme });
        }
    }

    /// Skip a `;` comment body (the newline re-syncs position).
    fn skip_comment(&mut self) {
        while let Some(d) = self.next() {
            self.advance(d);
            if d == '\n' {
                break;
            }
        }
    }

    /// Skip a string body after the opening quote (escapes honored),
    /// noting an EOF inside (the reader's string/regex diagnostic
    /// then wins over the unbalanced-input one).
    fn skip_string_counted(&mut self) {
        while let Some(d) = self.next() {
            self.advance(d);
            if d == '\\' {
                if let Some(e) = self.next() {
                    self.advance(e);
                }
                continue;
            }
            if d == '"' {
                return;
            }
        }
        self.ended_in_string = true;
    }

    /// Skip a char literal's tail after the backslash (one char plus
    /// the alnum run for `\newline`/`\u0041`-style spellings).
    fn skip_char_tail(&mut self) {
        let Some(first) = self.next() else {
            return;
        };
        self.advance(first);
        if !first.is_ascii_alphabetic() {
            return;
        }
        while let Some(e) = self.chars.peek().copied() {
            if !e.is_ascii_alphanumeric() {
                break;
            }
            self.next();
            self.advance(e);
        }
    }
}

#[cfg(feature = "aot")]
fn llvm_backend() -> String {
    // Touch the real backend so `build` reports linkage truthfully
    // (and the `aot` feature stays a used dependency).
    let ir = clj_aot::probe_ir();
    debug_assert!(ir.contains("ModuleID"));
    "LLVM backend linked".to_owned()
}

#[cfg(not(feature = "aot"))]
fn llvm_backend() -> String {
    "LLVM backend not linked (built with --no-default-features)".to_owned()
}

/// `repl`: tty stdin gets the banner + rustyline driver (history,
/// completion, multiline); anything else degrades to streaming eval
/// with no banner and no prompts (pins P6 purity).
fn cmd_repl(history_file: Option<PathBuf>, no_history: bool) -> ExitCode {
    use std::io::{BufReader, IsTerminal};
    if !std::io::stdin().is_terminal() {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut session =
            clj_repl::Session::new(boot(), BufReader::new(stdin.lock()), stdout.lock());
        return ExitCode::from(session.run() as u8);
    }
    println!("clojure-rs {}", env!("CARGO_PKG_VERSION"));
    let stdout = std::io::stdout();
    let mut session = clj_repl::Session::new(boot(), std::io::empty(), stdout.lock());
    session.set_prompt(Some("{ns}=> ".to_owned()));
    let names = core_names(&mut session);
    let source = match clj_repl::RustyLineSource::new(names, clj_repl::read_is_complete) {
        Ok(source) => source,
        Err(msg) => {
            // Terminal backend unavailable: degrade to streaming
            // rather than dying (D3 — a REPL that cannot edit
            // still evaluates).
            eprintln!("clojure-rs: warning: line editing unavailable ({msg}); streaming input");
            let stdin = std::io::stdin();
            let (eval, _, _) = session.into_parts();
            let mut fallback = clj_repl::Session::new(eval, BufReader::new(stdin.lock()), stdout);
            return ExitCode::from(fallback.run() as u8);
        }
    };
    let history = match (no_history, history_file) {
        (true, _) => clj_repl::HistoryOptions::disabled(),
        (false, Some(path)) => clj_repl::HistoryOptions::with_path(path),
        (false, None) => clj_repl::HistoryOptions::default(),
    };
    let mut editor = clj_repl::TtyEditor::with_history(source, history);
    let prompt_text = std::rc::Rc::new(std::cell::RefCell::new(
        session.render_prompt().unwrap_or_default(),
    ));
    let prompt = {
        let text = std::rc::Rc::clone(&prompt_text);
        move || text.borrow().clone()
    };
    let mut handler = SessionHandler {
        session,
        prompt_text,
        status: std::rc::Rc::new(std::cell::Cell::new(0)),
    };
    let code = editor.run(&prompt, &mut handler);
    let status = handler.status.get();
    ExitCode::from(if code == 0 { status as u8 } else { code as u8 })
}

/// [`clj_repl::LineHandler`] over a [`clj_repl::Session`]: submit
/// each completed unit, re-render the prompt (so `in-ns`
/// re-prompts at once), and remember the worst IO status (the
/// driver always reports 0, so broken-stdout surfaces here).
struct SessionHandler<W: std::io::Write> {
    session: clj_repl::Session<std::io::Empty, W>,
    prompt_text: std::rc::Rc<std::cell::RefCell<String>>,
    status: std::rc::Rc<std::cell::Cell<i32>>,
}

impl<W: std::io::Write> clj_repl::LineHandler for SessionHandler<W> {
    fn handle_line(&mut self, line: &str) -> clj_repl::LoopAction {
        let status = self.session.submit_complete(line);
        if status != 0 {
            self.status.set(status);
        }
        if let Some(rendered) = self.session.render_prompt() {
            *self.prompt_text.borrow_mut() = rendered;
        }
        clj_repl::LoopAction::Continue
    }
}

/// Push the unqualified name of every symbol in `items`.
fn push_symbol_names<'a>(
    names: &mut Vec<String>,
    items: impl Iterator<Item = &'a clj_value::Value>,
) {
    use clj_value::Value;
    for item in items {
        if let Value::Symbol(sym) = item {
            names.push(sym.name().to_owned());
        }
    }
}

/// The completion vocabulary: every interned `clojure.core` public
/// name (plus the `*1`/`*e` session vars, which users reach for in
/// a repl). Read once at startup; later `def`s complete next time.
fn core_names(session: &mut clj_repl::Session<std::io::Empty, impl std::io::Write>) -> Vec<String> {
    use clj_value::Value;
    let mut names = vec![
        "*1".to_owned(),
        "*2".to_owned(),
        "*3".to_owned(),
        "*e".to_owned(),
    ];
    let listed = session
        .evaluator_mut()
        .eval_source("(keys (ns-publics 'clojure.core))");
    if let Ok(values) = listed {
        for value in values {
            match &value {
                Value::List(items) => push_symbol_names(&mut names, items.iter()),
                Value::Vector(items) => push_symbol_names(&mut names, items.iter()),
                _ => {}
            }
        }
    }
    names.sort();
    names.dedup();
    names
}
