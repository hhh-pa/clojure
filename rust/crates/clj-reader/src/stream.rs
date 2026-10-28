// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The character stream: `LineNumberReader` normalization plus
//! `LineNumberingPushbackReader` positions (`reader-contract.md` §1).
//!
//! CR, LF, and CRLF all yield a single `\n` (pre-normalized once at
//! construction — downstream positions live in normalized space,
//! exactly as on the JVM). Line starts at 1 and bumps when `\n`
//! comes from the underlying input (pushback re-reads never bump it).
//! Column starts at 1: reading `\n` or EOF sets it to 1, other chars
//! increment it, and `unread` decrements it unconditionally (line
//! unchanged — column 0 is observable). Reading EOF bumps the line
//! iff a non-`\n` char was read from the underlying input since the
//! last `\n`/EOF/start, once per content run.

/// The §1 position tracker: normalized chars plus line/column state.
#[derive(Clone, Debug)]
pub struct CharStream {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
    bumpable: bool,
    pushback: Option<char>,
    pending_cr: bool,
}

impl CharStream {
    /// Normalize `input` (`\r\n` and lone `\r` become `\n`) and start
    /// at line 1, column 1. Normalization runs through [`Self::extend`],
    /// so a trailing `\r` stages exactly like any chunk boundary.
    #[must_use]
    pub fn new(input: &str) -> Self {
        let mut this = Self {
            chars: Vec::with_capacity(input.len()),
            pos: 0,
            line: 1,
            col: 1,
            bumpable: false,
            pushback: None,
            pending_cr: false,
        };
        this.extend(input);
        this
    }

    /// Append `chunk` with the same normalization as [`Self::new`].
    /// A chunk-trailing `\r` stages instead of pushing (a later leading
    /// `\n` completes the split `\r\n`); empty chunks preserve the flag.
    pub fn extend(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let mut iter = chunk.chars().peekable();
        if self.pending_cr {
            self.pending_cr = false;
            self.chars.push('\n');
            if iter.peek() == Some(&'\n') {
                iter.next();
            }
        }
        while let Some(c) = iter.next() {
            if c == '\r' {
                if iter.peek() == Some(&'\n') {
                    iter.next();
                    self.chars.push('\n');
                } else if iter.peek().is_none() {
                    self.pending_cr = true;
                } else {
                    self.chars.push('\n');
                }
            } else {
                self.chars.push(c);
            }
        }
    }

    /// Read one `\n`-split line (`\n` consumed and stripped); `None`
    /// at EOF, the tail when EOF lands mid-line.
    pub fn read_line(&mut self) -> Option<String> {
        let mut line = String::new();
        loop {
            match self.read() {
                Some('\n') => return Some(line),
                Some(c) => line.push(c),
                None => {
                    if line.is_empty() {
                        return None;
                    }
                    return Some(line);
                }
            }
        }
    }

    /// Read one char, or `None` at EOF (with the EOF line-bump rule).
    /// A staged chunk-trailing `\r` materializes first, past pushback.
    pub fn read(&mut self) -> Option<char> {
        if let Some(c) = self.pushback.take() {
            // Re-reads from the pushback buffer move the column but
            // never the line and never set the bumpable flag.
            if c == '\n' {
                self.col = 1;
            } else {
                self.col += 1;
            }
            return Some(c);
        }
        if self.pending_cr {
            self.pending_cr = false;
            self.chars.push('\n');
        }
        let Some(&c) = self.chars.get(self.pos) else {
            self.col = 1;
            if self.bumpable {
                self.line += 1;
                self.bumpable = false;
            }
            return None;
        };
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
            self.bumpable = false;
        } else {
            self.col += 1;
            self.bumpable = true;
        }
        Some(c)
    }

    /// Push one char back (at most one pending, never EOF — the
    /// reader never double-unreads); the column drops unconditionally.
    pub fn unread(&mut self, c: char) {
        debug_assert!(self.pushback.is_none(), "single-char pushback");
        self.pushback = Some(c);
        self.col -= 1;
    }

    /// The logical read position: unread chars count as unconsumed
    /// (S7 `read+string` capture brackets).
    #[must_use]
    pub fn capture_mark(&self) -> usize {
        self.pos
            .saturating_sub(usize::from(self.pushback.is_some()))
    }

    /// The normalized text consumed since `mark` (S7 `read+string`
    /// capture; a mark past the position captures nothing).
    #[must_use]
    pub fn captured_since(&self, mark: usize) -> String {
        self.chars
            .get(mark..self.capture_mark())
            .unwrap_or_default()
            .iter()
            .collect()
    }

    /// Current 1-based line.
    #[must_use]
    pub fn line(&self) -> u32 {
        self.line
    }

    /// Current column: 1-based, 0 after `unread('\n')`.
    #[must_use]
    pub fn col(&self) -> u32 {
        self.col
    }
}

#[cfg(test)]
mod tests {
    use super::CharStream;

    /// Drive `ops` (`r` = read, `u(c)` = unread) and record every
    /// `(char, line, col)`, mirroring the `POS` oracle probes.
    fn drive(input: &str, ops: &[(bool, char)]) -> Vec<(Option<char>, u32, u32)> {
        let mut s = CharStream::new(input);
        let mut out = Vec::new();
        for &(read, c) in ops {
            if read {
                let got = s.read();
                out.push((got, s.line(), s.col()));
            } else {
                s.unread(c);
                out.push((None, s.line(), s.col()));
            }
        }
        out
    }

    #[test]
    fn eof_bump_once_per_content_run() {
        let r = |n: usize| vec![(true, '\0'); n];
        // "" → (1,1), no bump.
        assert_eq!(drive("", &r(2)), vec![(None, 1, 1), (None, 1, 1)]);
        // " " → bump once, then stable.
        assert_eq!(
            drive(" ", &r(3)),
            vec![(Some(' '), 1, 2), (None, 2, 1), (None, 2, 1)]
        );
        // Trailing newline: no bump.
        assert_eq!(
            drive("a\n", &r(4)),
            vec![
                (Some('a'), 1, 2),
                (Some('\n'), 2, 1),
                (None, 2, 1),
                (None, 2, 1),
            ]
        );
        // "a\nb" → EOF bumps to line 3.
        assert_eq!(drive("a\nb", &r(5))[3..], vec![(None, 3, 1), (None, 3, 1)]);
    }

    #[test]
    fn crlf_and_lone_cr_normalize() {
        let r = |n: usize| vec![(true, '\0'); n];
        assert_eq!(
            drive("\r\n", &r(3)),
            vec![(Some('\n'), 2, 1), (None, 2, 1), (None, 2, 1)]
        );
        assert_eq!(
            drive("\r", &r(3)),
            vec![(Some('\n'), 2, 1), (None, 2, 1), (None, 2, 1)]
        );
        assert_eq!(
            drive("a\rb", &r(5)),
            vec![
                (Some('a'), 1, 2),
                (Some('\n'), 2, 1),
                (Some('b'), 2, 2),
                (None, 3, 1),
                (None, 3, 1),
            ]
        );
    }

    #[test]
    fn unread_drops_col_and_rereads_skip_bumpable() {
        // unread-nl: unread('\n') shows column 0; the re-read restores 1.
        let ops = [
            (true, '\0'),
            (true, '\0'),
            (false, '\n'),
            (true, '\0'),
            (true, '\0'),
            (true, '\0'),
            (true, '\0'),
        ];
        assert_eq!(
            drive("a\nb", &ops),
            vec![
                (Some('a'), 1, 2),
                (Some('\n'), 2, 1),
                (None, 2, 0),
                (Some('\n'), 2, 1),
                (Some('b'), 2, 2),
                (None, 3, 1),
                (None, 3, 1),
            ]
        );
        // unread-eof-after: the re-read after EOF does not re-arm the bump.
        let ops = [
            (true, '\0'),
            (true, '\0'),
            (true, '\0'),
            (false, 'b'),
            (true, '\0'),
            (true, '\0'),
        ];
        assert_eq!(
            drive("ab", &ops),
            vec![
                (Some('a'), 1, 2),
                (Some('b'), 1, 3),
                (None, 2, 1),
                (None, 2, 0),
                (Some('b'), 2, 1),
                (None, 2, 1),
            ]
        );
    }

    #[test]
    fn extend_coalesces_split_crlf() {
        let mut s = CharStream::new("a");
        s.extend("\r");
        s.extend("\nb");
        assert_eq!(s.read(), Some('a'));
        assert_eq!((s.line(), s.col()), (1, 2));
        assert_eq!(s.read(), Some('\n'));
        assert_eq!((s.line(), s.col()), (2, 1));
        assert_eq!(s.read(), Some('b'));
        assert_eq!((s.line(), s.col()), (2, 2));
        assert_eq!(s.read(), None);
        assert_eq!((s.line(), s.col()), (3, 1));
    }

    #[test]
    fn extend_empty_chunk_preserves_staged_cr() {
        let mut s = CharStream::new("");
        s.extend("\r");
        s.extend("");
        s.extend("\n");
        assert_eq!(s.read(), Some('\n'));
        assert_eq!((s.line(), s.col()), (2, 1));
        assert_eq!(s.read(), None);
        assert_eq!((s.line(), s.col()), (2, 1));
        // Empty chunks without a staged `\r` are plain no-ops.
        let mut t = CharStream::new("a");
        t.extend("");
        assert_eq!(t.read(), Some('a'));
        assert_eq!(t.read(), None);
    }

    #[test]
    fn extend_staged_cr_before_plain_chunk_is_lone_cr() {
        let mut s = CharStream::new("");
        s.extend("a\r");
        s.extend("b");
        assert_eq!(s.read(), Some('a'));
        assert_eq!(s.read(), Some('\n'));
        assert_eq!(s.read(), Some('b'));
        assert_eq!(s.read(), None);
        // A chunk-final `\r\n` never stages: same reads, no flag.
        let mut t = CharStream::new("");
        t.extend("a\r\n");
        t.extend("b");
        assert_eq!(t.read(), Some('a'));
        assert_eq!(t.read(), Some('\n'));
        assert_eq!(t.read(), Some('b'));
        assert_eq!(t.read(), None);
    }

    #[test]
    fn read_flushes_staged_cr_past_pushback() {
        let mut s = CharStream::new("a");
        s.extend("b\r");
        assert_eq!(s.read(), Some('a'));
        s.unread('a');
        // Pushback drains first, then buffered chars, then the flush.
        assert_eq!(s.read(), Some('a'));
        assert_eq!(s.read(), Some('b'));
        assert_eq!(s.read(), Some('\n'));
        assert_eq!((s.line(), s.col()), (2, 1));
        assert_eq!(s.read(), None);
        // The flush cleared the flag, so `extend` appends plainly.
        s.extend("c");
        assert_eq!(s.read(), Some('c'));
    }

    #[test]
    fn read_line_splits_newlines() {
        let mut s = CharStream::new("a\nb");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!((s.line(), s.col()), (2, 1));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!((s.line(), s.col()), (3, 1));
        assert_eq!(s.read_line(), None);
        // A second call at EOF stays `None`.
        assert_eq!(s.read_line(), None);
    }

    #[test]
    fn read_line_normalizes_crlf_and_cr() {
        let mut s = CharStream::new("a\r\nb");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!(s.read_line(), None);
        let mut s = CharStream::new("a\rb");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!(s.read_line(), None);
    }

    #[test]
    fn read_line_lone_cr_runs_and_empty_lines() {
        // Lone-`\r` runs split like `\n` runs (oracle `rl-mixed` shape).
        let mut s = CharStream::new("a\r\rb");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!(s.read_line(), Some(String::new()));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!(s.read_line(), None);
        // Empty lines and a trailing newline.
        let mut s = CharStream::new("a\n\nb");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!(s.read_line(), Some(String::new()));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!(s.read_line(), None);
        let mut s = CharStream::new("a\n");
        assert_eq!(s.read_line(), Some("a".to_owned()));
        assert_eq!(s.read_line(), None);
        let mut s = CharStream::new("");
        assert_eq!(s.read_line(), None);
    }

    #[test]
    fn new_with_trailing_cr_extends_cleanly() {
        // `new` stages a trailing `\r` exactly like `extend` (no
        // double newline when the next chunk opens with `\n`).
        let mut s = CharStream::new("a\r");
        s.extend("\nb");
        assert_eq!(s.read(), Some('a'));
        assert_eq!(s.read(), Some('\n'));
        assert_eq!(s.read(), Some('b'));
        assert_eq!(s.read(), None);
        // Reads without any extend are unchanged.
        let mut t = CharStream::new("a\r");
        assert_eq!(t.read(), Some('a'));
        assert_eq!(t.read(), Some('\n'));
        assert_eq!(t.read(), None);
    }

    #[test]
    fn capture_brackets_exclude_pushback() {
        // S7 `read+string`: the mark/excluded-unread bracketing (an
        // unread terminator belongs to neither side) and the
        // defensive past-position mark.
        let mut s = CharStream::new("42 43");
        let mark = s.capture_mark();
        assert_eq!(mark, 0);
        assert_eq!(s.read(), Some('4'));
        assert_eq!(s.read(), Some('2'));
        assert_eq!(s.read(), Some(' '));
        s.unread(' ');
        assert_eq!(s.captured_since(mark), "42");
        assert_eq!(s.capture_mark(), 2);
        assert_eq!(s.captured_since(99), "");
        assert_eq!(s.captured_since(3), "");
    }

    #[test]
    fn read_line_consumes_pushback() {
        let mut s = CharStream::new("ab");
        assert_eq!(s.read(), Some('a'));
        s.unread('a');
        assert_eq!(s.read_line(), Some("ab".to_owned()));
        assert_eq!(s.read_line(), None);
        // An unread `\n` ends the line immediately.
        let mut s = CharStream::new("a\nb");
        assert_eq!(s.read(), Some('a'));
        assert_eq!(s.read(), Some('\n'));
        s.unread('\n');
        assert_eq!(s.read_line(), Some(String::new()));
        assert_eq!(s.read_line(), Some("b".to_owned()));
        assert_eq!(s.read_line(), None);
    }
}
