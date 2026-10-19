// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Shared integration-test harness: binary lookup and output helpers.

use std::ffi::OsStr;
use std::process::{Command, Output};

/// A `Command` for the `clojure-rs` binary under test.
pub fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clojure-rs"))
}

/// Run the binary with `args`, returning its output.
pub fn run<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    bin().args(args).output().expect("spawn clojure-rs")
}

/// Run the binary with `args` and `input` on stdin (piped stdin is
/// never a tty, so `repl` takes the streaming path).
pub fn run_with_stdin<I, S>(args: I, input: &str) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use std::io::Write;
    use std::process::Stdio;
    let mut child = bin()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clojure-rs");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait clojure-rs")
}

/// Lossy stdout of an output.
pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Lossy stderr of an output.
pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
