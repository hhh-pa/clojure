// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Interactive REPL loop over `rustyline`: prompt/history/completion,
//! `*1`/`*2`/`*3`/`*e` bindings, streaming fallback off-tty.
//!
//! Lands in slice S7 (`docs/t1174/specification.md`).
//!
//! S7B ships the PTY-less core first: [`Session`] runs one loop over
//! generic IO (the S7C tty adapter owns line editing, history, and
//! the banner).

pub mod editor;
pub mod session;

pub use editor::{HistoryOptions, LineHandler, LoopAction, RustyLineSource, TtyEditor};
pub use session::{
    Session, core_var, is_incomplete_error, is_incomplete_message, read_is_complete,
};
