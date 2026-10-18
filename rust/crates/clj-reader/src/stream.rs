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
}

impl CharStream {
    /// Normalize `input` (`\r\n` and lone `\r` become `\n`) and start
    /// at line 1, column 1.
    #[must_use]
    pub fn new(input: &str) -> Self {
        let mut chars = Vec::with_capacity(input.len());
        let mut iter = input.chars().peekable();
        while let Some(c) = iter.next() {
            if c == '\r' {
                if iter.peek() == Some(&'\n') {
                    iter.next();
                }
                chars.push('\n');
            } else {
                chars.push(c);
            }
        }
        Self {
            chars,
            pos: 0,
            line: 1,
            col: 1,
            bumpable: false,
            pushback: None,
        }
    }

    /// Read one char, or `None` at EOF (with the EOF line-bump rule).
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
}
