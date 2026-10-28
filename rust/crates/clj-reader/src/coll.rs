// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Delimited lists and the collection readers
//! (`reader-contract.md` §19).
//!
//! `(` captures `:line`/`:column` meta for non-empty lists (`()` is
//! bare); `[`/`{`/`#{` carry none. Delimited EOF reports the body's
//! first line (bare without line numbers). Maps check oddness after
//! the full read, then duplicate-check; sets duplicate-check.

use clj_value::{Keyword, List, Map, Set, Value, Vector, java_debug};

use crate::Reader;
use crate::error::ReaderError;
use crate::hooks::ReadHooks;

impl Reader {
    /// `readDelimitedList`: forms until `delim` (nested reads —
    /// delimited content is never top-level); EOF reports the body's
    /// first line.
    pub(crate) fn read_delimited(
        &mut self,
        delim: char,
        hook: &mut dyn ReadHooks,
    ) -> Result<Vec<Value>, ReaderError> {
        let firstline = self.firstline();
        let mut items = Vec::new();
        loop {
            match self.read_inner(false, Some(delim), false, hook)? {
                crate::Form::Eof => {
                    return Err(match firstline {
                        Some(line) => ReaderError::EofDelimited {
                            span: self.span(),
                            line,
                        },
                        None => ReaderError::Eof { span: self.span() },
                    });
                }
                crate::Form::Finished => return Ok(items),
                crate::Form::Value(v) => items.push(v),
            }
        }
    }

    /// `ListReader`.
    pub(crate) fn list_reader(
        &mut self,
        hook: &mut dyn ReadHooks,
    ) -> Result<Option<Value>, ReaderError> {
        let meta_pos = self.meta_pos();
        let items = self.read_delimited(')', hook)?;
        if items.is_empty() {
            return Ok(Some(Value::List(List::new_sync())));
        }
        let mut list = List::from_vec(items);
        if let Some((line, col)) = meta_pos {
            let meta = clj_value::new_map()
                .insert(
                    Value::Keyword(Keyword::new(None, "line")),
                    Value::Int(i64::from(line)),
                )
                .insert(
                    Value::Keyword(Keyword::new(None, "column")),
                    Value::Int(i64::from(col)),
                );
            list = list.with_meta(Some(meta));
        }
        Ok(Some(Value::List(list)))
    }

    /// `VectorReader`.
    pub(crate) fn vector_reader(
        &mut self,
        hook: &mut dyn ReadHooks,
    ) -> Result<Option<Value>, ReaderError> {
        Ok(Some(Value::Vector(Vector::from_vec(
            self.read_delimited(']', hook)?,
        ))))
    }

    /// `MapReader`: odd check after the full read, then `RT.map`
    /// duplicate rules.
    pub(crate) fn map_reader(
        &mut self,
        hook: &mut dyn ReadHooks,
    ) -> Result<Option<Value>, ReaderError> {
        let items = self.read_delimited('}', hook)?;
        if items.len() % 2 == 1 {
            return Err(ReaderError::MapOdd { span: self.span() });
        }
        Self::checked_map(pair_up(items), self.span()).map(|m| Some(Value::Map(m)))
    }

    /// `SetReader`.
    pub(crate) fn set_reader(
        &mut self,
        hook: &mut dyn ReadHooks,
    ) -> Result<Option<Value>, ReaderError> {
        let items = self.read_delimited('}', hook)?;
        match Set::from_items_checked(items) {
            Ok(set) => Ok(Some(Value::Set(set))),
            Err(dup) => Err(ReaderError::DupKey {
                span: self.span(),
                key: java_debug(&dup),
            }),
        }
    }

    /// Pair up a flat even-length form list and duplicate-check it
    /// (shared by map literals and namespaced maps).
    pub(crate) fn checked_map(
        pairs: Vec<(Value, Value)>,
        span: crate::error::Span,
    ) -> Result<Map, ReaderError> {
        match Map::from_kvs_checked(pairs) {
            Ok(map) => Ok(map),
            Err(dup) => Err(ReaderError::DupKey {
                span,
                key: java_debug(&dup),
            }),
        }
    }
}

/// Pair consecutive items (the caller guarantees an even count).
pub(crate) fn pair_up(items: Vec<Value>) -> Vec<(Value, Value)> {
    let mut pairs = Vec::with_capacity(items.len() / 2);
    let mut iter = items.into_iter();
    while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
        pairs.push((k, v));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use crate::Reader;
    use crate::ReaderOpts;
    use crate::error::ReaderError;
    use crate::hooks::NoHooks;

    #[test]
    fn delimited_eof_is_bare_without_line_numbers() {
        // `read-string` behavior: no `:line`, so EOF in a
        // delimiter reports bare `Eof`, not `EofDelimited`.
        let opts = ReaderOpts {
            line_numbers: false,
            ..ReaderOpts::default()
        };
        let mut reader = Reader::new("(", &opts);
        let err = reader.read_delimited(')', &mut NoHooks).expect_err("eof");
        assert!(matches!(err, ReaderError::Eof { .. }), "{err:?}");
    }
}
