// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure-rs`: native Clojure runtime, REPL, and LLVM AOT compiler.
//!
//! Slice S1: CLI shell only. `--help` and `--version` work; the
//! subcommands exit with their documented codes but report "not yet
//! implemented" until their slice lands (spec `docs/t1174/`).

use clap::{Parser, Subcommand};
use std::path::PathBuf;
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
        Command::Eval { form, edn } => {
            let mode = if edn { " (edn reader)" } else { "" };
            eprintln!("eval: not yet implemented (lands in slice S4){mode}: {form}");
            ExitCode::from(1)
        }
        Command::Run { file, args } => {
            if !file.is_file() {
                eprintln!("run: no such file: {}", file.display());
                return ExitCode::from(3);
            }
            eprintln!(
                "run: not yet implemented (lands in slice S5): {} ({} args)",
                file.display(),
                args.len()
            );
            ExitCode::from(1)
        }
        Command::Repl {
            history_file,
            no_history,
        } => {
            let history = match (no_history, history_file) {
                (true, _) => "disabled".to_owned(),
                (false, Some(path)) => path.display().to_string(),
                (false, None) => "~/.clojure-rs/history".to_owned(),
            };
            eprintln!("repl: not yet implemented (lands in slice S7); history would be {history}");
            ExitCode::from(1)
        }
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
