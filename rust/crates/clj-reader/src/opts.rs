// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! [`ReaderOpts`]: read options and resolver maps (`reader-contract.md` §23).
//!
//! Defaults are load semantics: `:read-cond :allow`, features
//! `#{:clj}` (`:clj` is always installed, even when the caller passes
//! other features), line numbers on, current namespace `user`, empty
//! resolver maps (native unit tests resolve nothing; the differential
//! harness supplies live `*ns*` maps).

use std::collections::{HashMap, HashSet};

use clj_value::Keyword;

/// `:read-cond` handling: process conditionals, preserve them as
/// [`Value::ReaderConditional`](clj_value::Value::ReaderConditional),
/// or reject them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadCond {
    /// Process reader conditionals (load semantics).
    #[default]
    Allow,
    /// Preserve conditionals (and nested tags) as data.
    Preserve,
    /// Reject conditionals (`Conditional read not allowed`); S5's
    /// `read-string` passes this.
    Deny,
}

/// Reader options and resolver maps.
#[derive(Clone, Debug)]
pub struct ReaderOpts {
    /// `:read-cond` (`Allow` by default).
    pub read_cond: ReadCond,
    /// `:features` (`:clj` always installed on top).
    pub features: HashSet<Keyword>,
    /// Line/column tracking: `false` gives `read-string` behavior
    /// (bare `EOF while reading`, no list `:line`/`:column`).
    pub line_numbers: bool,
    /// Current namespace name (for `::` and syntax-quote).
    pub current_ns: String,
    /// Namespace aliases (`str` → `clojure.string`).
    pub aliases: HashMap<String, String>,
    /// Referred vars (`map` → `(clojure.core, map)`).
    pub refers: HashMap<String, (String, String)>,
    /// Imported classes (`String` → `java.lang.String`).
    pub class_map: HashMap<String, String>,
}

impl Default for ReaderOpts {
    fn default() -> Self {
        Self {
            read_cond: ReadCond::Allow,
            features: HashSet::from([Keyword::new(None, "clj")]),
            line_numbers: true,
            current_ns: "user".to_owned(),
            aliases: HashMap::new(),
            refers: HashMap::new(),
            class_map: HashMap::new(),
        }
    }
}

impl ReaderOpts {
    /// Mirror `installPlatformFeature`: `:clj` is always present.
    pub(crate) fn normalized(mut self) -> Self {
        self.features.insert(Keyword::new(None, "clj"));
        self
    }
}
