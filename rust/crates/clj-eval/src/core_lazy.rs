// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Lazy `core.clj` (#226): boot registers each `defn`/`defn-`/
//! `defmacro` form (and the `Inst` protocol pair) without reading or
//! evaluating it, and the first resolution of one of its vars
//! evaluates the form ([`force`]). Interpreting all ~140 forms was
//! most of every process's startup, while a script touches a few.
//! Embedded libraries every run requires (`clojure.string`) load the
//! same way ([`load_lazy`]).
//!
//! Until forced, a lazy var is interned (resolution, refers, and
//! `ns-publics` see it), unbound, and flagged `:macro`/`:private`
//! like the evaluated form would flag it — the only metadata the
//! analyzer consults before it resolves (and so forces) the var.
//! Forcing evaluates the form as boot would have: in `clojure.core`,
//! under boot's source name and line numbers, with a fresh binding
//! stack; the gensym/eval/`fn` counters are restored afterwards, so
//! user-visible numbering never depends on what got forced.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use clj_value::{Keyword, Symbol, Value, new_map};

use crate::env::Bindings;
use crate::eval::{Evaluator, SourceError};
use crate::ext::NativeImpl;
use crate::ns::{core_var, core_var_handle, ns_value};

/// The source name boot evaluates under (`Evaluator::new_bare`'s
/// argument in `boot`, so forced vars keep `:file "user"`).
const BOOT_SOURCE: &str = "user";

/// One lazily evaluated top-level form: its text and 1-based start
/// line (forms start in column 1).
#[derive(Clone, Copy, Debug)]
pub(crate) struct LazyForm {
    text: &'static str,
    line: usize,
}

/// The forms one force evaluates (in order) and every var they
/// define (forcing any of them forces all).
#[derive(Debug)]
pub(crate) struct LazyGroup {
    forms: Vec<LazyForm>,
    pub(crate) ids: Vec<u64>,
    /// The namespace and source name its file evaluated under.
    ns: Arc<str>,
    source: Arc<str>,
}

/// Lock-free membership over the lazy var ids (a bitset from the
/// lowest id): the analyzer tests every resolution against it.
#[derive(Debug)]
pub(crate) struct LazyIndex {
    lo: u64,
    words: Box<[AtomicU64]>,
}

impl LazyIndex {
    pub(crate) fn new(ids: &[u64]) -> Self {
        let lo = ids.iter().copied().min().unwrap_or(0);
        let span = ids.iter().map(|id| id - lo).max().map_or(0, |m| m + 1);
        let words: Box<[AtomicU64]> = (0..span.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
        for id in ids {
            let off = id - lo;
            words[(off / 64) as usize].fetch_or(1 << (off % 64), Ordering::Relaxed);
        }
        Self { lo, words }
    }

    /// Whether `id` is still lazy (acquire: a cleared bit publishes
    /// the forced root).
    pub(crate) fn test(&self, id: u64) -> bool {
        let Some(off) = id.checked_sub(self.lo) else {
            return false;
        };
        self.words
            .get((off / 64) as usize)
            .is_some_and(|w| w.load(Ordering::Acquire) & (1 << (off % 64)) != 0)
    }

    pub(crate) fn clear(&self, id: u64) {
        if let Some(off) = id.checked_sub(self.lo) {
            if let Some(w) = self.words.get((off / 64) as usize) {
                w.fetch_and(!(1 << (off % 64)), Ordering::Release);
            }
        }
    }
}

/// A top-level form's byte range and start line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TopForm {
    start: usize,
    end: usize,
    line: usize,
}

const NO_FORM: TopForm = TopForm {
    start: 0,
    end: 0,
    line: 0,
};

/// Scan `b` for its top-level forms (whitespace, commas, and line
/// comments between them skipped), storing the first `out.len()`
/// and returning how many there are. Inside a form, strings (and
/// regexes), char literals, and comments are skipped so their
/// brackets never count; the embedded files hold lists only at top
/// level. `const`, so the embedded files' tables are computed at
/// compile time ([`forms_of`]) — every boot used to rescan
/// `core.clj` (#226).
const fn scan_top_forms(b: &[u8], out: &mut [TopForm]) -> usize {
    let (mut i, mut line, mut n) = (0, 1, 0);
    while i < b.len() {
        match b[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b';' => i = skip_comment(b, i),
            b'(' => {
                let start = i;
                let start_line = line;
                i = skip_form(b, i, &mut line);
                if n < out.len() {
                    out[n] = TopForm {
                        start,
                        end: i,
                        line: start_line,
                    };
                }
                n += 1;
            }
            _ => i += 1,
        }
    }
    n
}

/// Split `src` into its top-level forms (see [`scan_top_forms`]).
fn split_top_forms(src: &str) -> Vec<TopForm> {
    let b = src.as_bytes();
    let mut out = vec![NO_FORM; scan_top_forms(b, &mut [])];
    scan_top_forms(b, &mut out);
    out
}

/// An embedded file's form table, computed at compile time.
macro_rules! form_table {
    ($src:expr) => {{
        const N: usize = scan_top_forms($src.as_bytes(), &mut []);
        static FORMS: [TopForm; N] = {
            let mut out = [NO_FORM; N];
            scan_top_forms($src.as_bytes(), &mut out);
            out
        };
        &FORMS
    }};
}

/// `src`'s top-level forms: the compile-time table for the embedded
/// files that load lazily (boot's `core.clj`, `clojure.string` —
/// statics, so one address each), else a scan now.
fn forms_of(src: &'static str) -> std::borrow::Cow<'static, [TopForm]> {
    if std::ptr::eq(src, crate::ns::CORE_CLJ) {
        std::borrow::Cow::Borrowed(form_table!(include_str!("core.clj")))
    } else if std::ptr::eq(src, crate::ns::STDLIB_STRING_CLJ) {
        std::borrow::Cow::Borrowed(form_table!(include_str!("stdlib/clojure/string.clj")))
    } else {
        std::borrow::Cow::Owned(split_top_forms(src))
    }
}

const fn skip_comment(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    i
}

/// Skip the bracketed form opening at `i`, returning the index past
/// its closer (`line` counts the newlines crossed).
const fn skip_form(b: &[u8], mut i: usize, line: &mut usize) -> usize {
    let mut depth = 0usize;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b'"' => {
                i = skip_string(b, i + 1, line);
                continue;
            }
            // A char literal: the next char is literal (`\(`, `\"`);
            // named chars (`\space`) are plain letters after it.
            b'\\' => i += 1,
            b';' => {
                i = skip_comment(b, i);
                continue;
            }
            b'\n' => *line += 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Skip a string body from `i` (past the opening quote), returning
/// the index past the closing quote.
const fn skip_string(b: &[u8], mut i: usize, line: &mut usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'"' => return i + 1,
            b'\\' => i += 1,
            b'\n' => *line += 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// The token at the start of `s` and the rest after it (tokens end
/// at whitespace or a bracket).
fn token(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| c.is_whitespace() || "()[]{}\"".contains(c))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// What a top-level form defines, read off its head.
enum Head<'a> {
    /// `defn`/`defn-`/`defmacro`: one var, flagged as evaluation
    /// would flag it.
    Def {
        name: &'a str,
        macro_: bool,
        private: bool,
    },
    /// `defprotocol`: the protocol and `<name>-interface` vars plus
    /// one var per method.
    Protocol { names: Vec<String> },
    /// `extend-protocol P …`: completes protocol `P`'s group.
    Extends(&'a str),
    /// `declare`: forward declarations of the named vars.
    Declare(Vec<&'a str>),
    /// Anything else evaluates eagerly at boot.
    Eager,
}

fn head_of(form: &str) -> Head<'_> {
    let (op, rest) = token(&form[1..]);
    let rest = rest.trim_start();
    match op {
        "defn" | "defn-" | "defmacro" => {
            let (private_meta, rest) = skip_name_meta(rest);
            Head::Def {
                name: token(rest).0,
                macro_: op == "defmacro",
                private: op == "defn-" || private_meta,
            }
        }
        "defprotocol" => {
            let (name, body) = token(rest);
            let mut names = vec![name.to_owned(), format!("{name}-interface")];
            names.extend(protocol_methods(body).into_iter().map(str::to_owned));
            Head::Protocol { names }
        }
        "extend-protocol" => Head::Extends(token(rest).0),
        "declare" => Head::Declare(rest.trim_end_matches(')').split_whitespace().collect()),
        _ => Head::Eager,
    }
}

/// Skip `^:private`/`^{…}`/`^Tag` metadata before a def name,
/// reporting whether it marks the var private.
fn skip_name_meta(mut s: &str) -> (bool, &str) {
    let mut private = false;
    while let Some(after) = s.strip_prefix('^') {
        if let Some(map) = after.strip_prefix('{') {
            // Name metadata maps in `core.clj` are flat.
            let close = map.find('}').unwrap_or(map.len());
            private |= map[..close].contains(":private true");
            s = map.get(close + 1..).unwrap_or("").trim_start();
        } else {
            let (m, more) = token(after);
            private |= m == ":private";
            s = more.trim_start();
        }
    }
    (private, s)
}

/// The method names of a `defprotocol` body: the head symbol of
/// each nested list one level in.
fn protocol_methods(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut line = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' => {
                out.push(token(&body[i + 1..]).0);
                i = skip_form(b, i, &mut line);
            }
            b'"' => i = skip_string(b, i + 1, &mut line),
            _ => i += 1,
        }
    }
    out
}

/// The deferred defs of one file plus the forms that still run
/// eagerly (in file order), split out by [`plan`].
struct Plan {
    groups: Vec<LazyGroup>,
    /// Metadata sources of native-backed macros (`core_macros`).
    meta_groups: Vec<LazyGroup>,
    eager: Vec<LazyForm>,
}

/// Register `forms`' defs in namespace `ns` (unbound, flagged) and
/// collect the eager remainder. Declarations whose names are all
/// deferred are dropped: forcing defines them, and evaluating them
/// would replace the registered flags (a declared private var would
/// look public to a `refer`).
fn plan(
    eval: &mut Evaluator,
    src: &'static str,
    forms: &[TopForm],
    ns: &str,
    source: &Arc<str>,
) -> Plan {
    let mut planner = Planner {
        ns,
        ns_arc: Arc::from(ns),
        source: Arc::clone(source),
        plan: Plan {
            groups: Vec::new(),
            meta_groups: Vec::new(),
            eager: Vec::new(),
        },
        protocols: Vec::new(),
        declares: Vec::new(),
    };
    for f in forms {
        planner.add(
            eval,
            LazyForm {
                text: &src[f.start..f.end],
                line: f.line,
            },
        );
    }
    planner.finish(eval)
}

/// [`plan`]'s accumulator: the plan so far, the protocol groups an
/// `extend-protocol` can join, and the declarations awaiting the
/// all-deferred check.
struct Planner<'a> {
    ns: &'a str,
    ns_arc: Arc<str>,
    source: Arc<str>,
    plan: Plan,
    protocols: Vec<(String, usize)>,
    declares: Vec<(LazyForm, Vec<&'static str>)>,
}

impl Planner<'_> {
    fn group(&self, forms: Vec<LazyForm>, ids: Vec<u64>) -> LazyGroup {
        LazyGroup {
            forms,
            ids,
            ns: Arc::clone(&self.ns_arc),
            source: Arc::clone(&self.source),
        }
    }

    /// File one top-level form by its head.
    fn add(&mut self, eval: &mut Evaluator, form: LazyForm) {
        let ns = self.ns;
        match head_of(form.text) {
            Head::Def { name, .. } if native_backed(ns, name) => {
                // The native runs; the form is metadata (and the
                // native's slow path) only.
                let q = Symbol::new(Some(ns), name);
                if let Some(id) = eval.registry.lookup_in_ns(ns, &q) {
                    let g = self.group(vec![form], vec![id]);
                    self.plan.meta_groups.push(g);
                }
            }
            Head::Def {
                name,
                macro_,
                private,
            } => {
                let id = register(eval, ns, name, macro_, private);
                let g = self.group(vec![form], vec![id]);
                self.plan.groups.push(g);
            }
            Head::Protocol { names } => {
                let ids = names
                    .iter()
                    .map(|n| register(eval, ns, n, false, false))
                    .collect();
                self.protocols
                    .push((names[0].clone(), self.plan.groups.len()));
                let g = self.group(vec![form], ids);
                self.plan.groups.push(g);
            }
            Head::Extends(p) => match self.protocols.iter().find(|(n, _)| *n == p) {
                Some(&(_, g)) => self.plan.groups[g].forms.push(form),
                None => self.plan.eager.push(form),
            },
            Head::Declare(names) => self.declares.push((form, names)),
            Head::Eager => self.plan.eager.push(form),
        }
    }

    /// Keep the declarations naming any eager var, then order the
    /// eager forms by line.
    fn finish(mut self, eval: &Evaluator) -> Plan {
        if !self.declares.is_empty() {
            let deferred_ids: std::collections::HashSet<u64> = self
                .plan
                .groups
                .iter()
                .flat_map(|g| g.ids.iter().copied())
                .collect();
            let ns = self.ns;
            let deferred = |n: &&str| {
                eval.registry
                    .lookup_in_ns(ns, &Symbol::new(Some(ns), n))
                    .is_some_and(|id| deferred_ids.contains(&id))
            };
            for (form, names) in self.declares {
                if !names.iter().all(deferred) {
                    self.plan.eager.push(form);
                }
            }
        }
        self.plan.eager.sort_by_key(|f| f.line);
        self.plan
    }
}

/// Whether `ns/name` is native-backed (a `core_macros` macro or a
/// `stdlib_natives` fn).
fn native_backed(ns: &str, name: &str) -> bool {
    match ns {
        "clojure.core" => crate::core_macros::CORE_MACROS
            .iter()
            .any(|(n, _)| *n == name),
        "clojure.string" => crate::stdlib_natives::STRING_NATIVES
            .iter()
            .any(|(n, _)| *n == name),
        _ => false,
    }
}

/// Install a library's native-backed fns into its namespace (just
/// made by the file's `ns` form).
fn install_lib_natives(eval: &mut Evaluator, ns: &str) {
    if ns == "clojure.string" {
        for &(name, f) in crate::stdlib_natives::STRING_NATIVES {
            eval.install_native(ns, name, Arc::new(f) as Arc<NativeImpl>);
        }
    }
}

/// Install the native-backed `core.clj` macros (macro-flagged
/// natives; their forms register as metadata sources in [`plan`]).
fn install_core_macros(eval: &mut Evaluator) {
    let macro_key = Value::Keyword(Keyword::new(None, "macro"));
    for &(name, f) in crate::core_macros::CORE_MACROS {
        eval.install_native("clojure.core", name, Arc::new(f) as Arc<NativeImpl>);
        let id = eval
            .registry
            .intern(&Symbol::new(Some("clojure.core"), name));
        eval.registry.with_state_mut(id, |st| {
            let meta = st.meta.take().unwrap_or_else(new_map);
            st.meta = Some(meta.insert(macro_key.clone(), Value::Bool(true)));
        });
    }
}

/// Boot's `core.clj` load (current namespace `clojure.core`): defs
/// defer, the rest evaluates in order.
pub(crate) fn boot_lazy(eval: &mut Evaluator, src: &'static str) {
    install_core_macros(eval);
    let forms = forms_of(src);
    let source = Arc::from(BOOT_SOURCE);
    let plan = plan(eval, src, &forms, "clojure.core", &source);
    // Boot's segment always installs (the lineage is fresh).
    let _ = eval.registry.install_lazy(plan.groups);
    eval.registry.install_meta_lazy(plan.meta_groups);
    for form in &plan.eager {
        if let Err(e) = eval_form(eval, form) {
            unreachable!("core.clj form failed ({}): {e:?}", form.text);
        }
    }
}

/// Load an embedded library lazily inside the caller's load frame:
/// the forms before its first def evaluate eagerly (the `ns` form
/// makes the namespace), then its defs defer into that namespace
/// and the rest evaluates in order. `false` (nothing evaluated)
/// when the file cannot defer: no def follows its prefix, a later
/// form switches namespaces, or every lazy index segment is taken.
pub(crate) fn load_lazy(
    eval: &mut Evaluator,
    source: &str,
    src: &'static str,
) -> Result<bool, SourceError> {
    let forms = forms_of(src);
    let first_def = forms
        .iter()
        .position(|f| matches!(head_of(&src[f.start..f.end]), Head::Def { .. }))
        .unwrap_or(forms.len());
    let switches = |f: &TopForm| {
        let op = token(&src[f.start + 1..f.end]).0;
        op == "ns" || op == "in-ns"
    };
    if first_def == forms.len()
        || forms[first_def..].iter().any(switches)
        || !eval.registry.lazy_room()
    {
        return Ok(false);
    }
    let source: Arc<str> = Arc::from(source);
    for f in &forms[..first_def] {
        eval_form(
            eval,
            &LazyForm {
                text: &src[f.start..f.end],
                line: f.line,
            },
        )?;
    }
    let ns = eval.current_ns_name();
    install_lib_natives(eval, &ns);
    let plan = plan(eval, src, &forms[first_def..], &ns, &source);
    eval.registry.install_meta_lazy(plan.meta_groups);
    assert!(
        eval.registry.install_lazy(plan.groups),
        "lazy segment vanished under the load lock"
    );
    for form in &plan.eager {
        eval_form(eval, form)?;
    }
    Ok(true)
}

/// Intern one lazy var of `ns`: unbound, carrying only the flags
/// the analyzer reads before resolving it.
fn register(eval: &mut Evaluator, ns: &str, name: &str, macro_: bool, private: bool) -> u64 {
    let id = eval.registry.intern(&Symbol::new(Some(ns), name));
    eval.registry.with_state_mut(id, |st| {
        st.root = None;
        let mut meta = st.meta.take().unwrap_or_else(new_map);
        if macro_ {
            meta = meta.insert(
                Value::Keyword(Keyword::new(None, "macro")),
                Value::Bool(true),
            );
        }
        if private {
            meta = meta.insert(
                Value::Keyword(Keyword::new(None, "private")),
                Value::Bool(true),
            );
        }
        st.meta = Some(meta);
    });
    id
}

/// Evaluate one form as a whole-file read would have: read from its
/// line, so spans and `:line` metadata match.
fn eval_form(eval: &mut Evaluator, form: &LazyForm) -> Result<Vec<Value>, SourceError> {
    let line = u32::try_from(form.line).expect("embedded files stay under 4G lines");
    eval.eval_source_at(form.text, line)
}

/// Force the lazy group defining `id` (`false` when another force
/// already did, or `id` is mid-force on this thread — a form
/// resolving its own name sees the unbound var, exactly as when
/// its file evaluated it).
pub(crate) fn force(eval: &mut Evaluator, id: u64) -> bool {
    let _serial = eval.registry.lock_lazy_force();
    let Some(group) = eval.registry.take_lazy(id) else {
        return false;
    };
    eval_group(eval, &group);
    eval.registry.finish_lazy(&group);
    true
}

/// Give a native-backed macro its `core.clj` metadata: evaluate the
/// form (which defines the interpreted macro and its metadata),
/// then put the native root back. `false` when `id` has no pending
/// metadata source.
pub(crate) fn force_meta(eval: &mut Evaluator, id: u64) -> bool {
    let _serial = eval.registry.lock_lazy_force();
    let Some(group) = eval.registry.take_meta_lazy(id) else {
        return false;
    };
    let native = eval.registry.root_cloned(id);
    eval_group(eval, &group);
    // The form just defined the interpreted fn: keep it for the
    // native's slow path (see [`interpreted`]) — unless the def
    // landed elsewhere (a recreated namespace), when the root is
    // still the native and must never be its own fallback.
    if let Some(f) = eval
        .registry
        .root_cloned(id)
        .filter(|f| !same_root(f, native.as_ref()))
    {
        eval.registry.set_fallback(id, f);
    }
    eval.registry.with_state_mut(id, |st| st.root = native);
    true
}

/// Whether `f` is the very root `root` (natives compare by identity).
fn same_root(f: &Value, root: Option<&Value>) -> bool {
    matches!((f, root), (Value::Extension(a), Some(Value::Extension(b))) if Arc::ptr_eq(a, b))
}

/// The interpreted definition behind a native-backed var (a
/// native's slow path): its file form evaluated once — defining
/// the upstream fn — with the var's state put back as it was
/// (`None` when the var has no source).
pub(crate) fn interpreted(eval: &mut Evaluator, id: u64) -> Option<Value> {
    // Under the force lock (only argument shapes outside the fast
    // paths get here): the cached fallback, else the pending
    // metadata force captures one (settling the metadata with the
    // same evaluation — a def inside the source would drop it
    // otherwise), else the permanent source runs.
    let _serial = eval.registry.lock_lazy_force();
    if let Some(f) = eval.registry.fallback(id) {
        return Some(f);
    }
    if force_meta(eval, id) {
        return eval.registry.fallback(id);
    }
    let group = eval.registry.native_source(id)?;
    let saved = eval.registry.state(id)?;
    eval_group(eval, &group);
    let f = eval.registry.root_cloned(id);
    let native = saved.root.clone();
    eval.registry.with_state_mut(id, |st| *st = saved);
    let f = f.filter(|f| !same_root(f, native.as_ref()))?;
    eval.registry.set_fallback(id, f.clone());
    Some(f)
}

/// Evaluate a group's forms in its file's context (see [`force`]).
fn eval_group(eval: &mut Evaluator, group: &LazyGroup) {
    let counters = eval.registry.counters();
    let saved = eval.swap_context(Bindings::new(), Arc::clone(&group.source));
    // The file's `*ns*`, as a permanent-style base frame (its
    // tombstone after `remove-ns`, which then absorbs the defs).
    if let (Some(var), Some(ns)) = (
        core_var(eval, "*ns*"),
        eval.registry.ns_live_or_removed_id(&group.ns),
    ) {
        eval.push_binding(core_var_handle(var, "*ns*"), ns_value(&group.ns, ns));
    }
    for form in &group.forms {
        if let Err(e) = eval_form(eval, form) {
            unreachable!("deferred form failed ({}): {e:?}", form.text);
        }
    }
    eval.swap_context(saved.0, saved.1);
    eval.registry.restore_counters(counters);
}

#[cfg(test)]
mod tests {
    use super::{
        Head, LazyIndex, boot_lazy, head_of, install_lib_natives, load_lazy, native_backed,
        split_top_forms,
    };
    use crate::ns::boot;

    #[test]
    fn scans_stray_tokens_escapes_and_unterminated_forms() {
        // Top-level atoms are skipped (the embedded files hold none);
        // string escapes and newlines keep their brackets literal and
        // their lines counted; an unterminated form runs to the end.
        let src = "x 1 :k\n(def s \"a\\\"(\nb\")\n(def t \"open";
        let forms = split_top_forms(src);
        let texts: Vec<(&str, usize)> = forms
            .iter()
            .map(|f| (&src[f.start..f.end], f.line))
            .collect();
        assert_eq!(
            texts,
            vec![("(def s \"a\\\"(\nb\")", 2), ("(def t \"open", 4)]
        );
    }

    #[test]
    fn plan_keeps_unknown_extends_and_partly_eager_declares() {
        // `extend-protocol` of a protocol the file never defined, and a
        // `declare` naming an eagerly defined var, both stay eager (the
        // eager forms run in the current namespace, `user` here).
        let mut eval = boot();
        boot_lazy(
            &mut eval,
            "(declare lz-eager lz-later) (def lz-eager 1) (defn lz-later [] 2)
             (extend-protocol clojure.core/Inst java.util.Date (inst-ms* [d] 0))",
        );
        let out = eval
            .eval_source("[lz-eager (clojure.core/lz-later)]")
            .unwrap();
        assert_eq!(
            clj_value::pr_to_string(&out[0], &clj_value::PrintOpts::pr()),
            "[1 2]"
        );
        assert!(!native_backed("other.ns", "split"));
        // Only `clojure.string` has library natives.
        install_lib_natives(&mut eval, "other.ns");
    }

    #[test]
    fn lazy_load_declines_files_it_cannot_defer() {
        let mut eval = boot();
        // No def after the prefix, and a later namespace switch: both
        // decline untouched (the caller loads them eagerly).
        assert!(!load_lazy(&mut eval, "a.clj", "(ns lz-a) (+ 1 2)").unwrap());
        assert!(!load_lazy(&mut eval, "b.clj", "(ns lz-b) (defn f [] 1) (in-ns 'lz-c)").unwrap());
        // A failing eager form after the defs is the load's error.
        let err = load_lazy(&mut eval, "c.clj", "(ns lz-d) (defn g [] 1) (no-such-fn)");
        assert!(err.is_err());
    }

    // The embedded boot sources are fixed, so a failing form is a
    // port bug: loud at boot (eager) or at the first use (deferred).
    #[test]
    #[should_panic(expected = "core.clj form failed")]
    fn broken_eager_form_panics_loudly() {
        let mut eval = boot();
        boot_lazy(&mut eval, "(do (broken");
    }

    #[test]
    #[should_panic(expected = "deferred form failed")]
    fn broken_deferred_form_panics_on_force() {
        let mut eval = boot();
        boot_lazy(&mut eval, "(defn broken-lazy [)");
        let _ = eval.eval_source("(clojure.core/broken-lazy)");
    }

    #[test]
    fn splits_top_forms_past_strings_chars_and_comments() {
        let src = "; c (\n(defn a \"doc (\" [x] \\( x)\n\n(defmacro b [] ;; ) x\n `(\\) ~@[]))\n(def c 1)";
        let forms = split_top_forms(src);
        let texts: Vec<(&str, usize)> = forms
            .iter()
            .map(|f| (&src[f.start..f.end], f.line))
            .collect();
        assert_eq!(
            texts,
            vec![
                ("(defn a \"doc (\" [x] \\( x)", 2),
                ("(defmacro b [] ;; ) x\n `(\\) ~@[]))", 4),
                ("(def c 1)", 6),
            ]
        );
    }

    #[test]
    fn heads_name_vars_and_flags() {
        let def = |s| match head_of(s) {
            Head::Def {
                name,
                macro_,
                private,
            } => Some((name.to_owned(), macro_, private)),
            _ => None,
        };
        assert_eq!(def("(defn f [x] x)"), Some(("f".into(), false, false)));
        assert_eq!(def("(defn- g [] 1)"), Some(("g".into(), false, true)));
        assert_eq!(
            def("(defmacro ^{:private true} m [])"),
            Some(("m".into(), true, true))
        );
        assert_eq!(
            def("(defn ^:private ^String h [])"),
            Some(("h".into(), false, true))
        );
        assert_eq!(def("(def x 1)"), None);
        assert!(matches!(
            head_of("(defprotocol P \"doc\" (m1 [x]) (m2 [x y]))"),
            Head::Protocol { names } if names == ["P", "P-interface", "m1", "m2"]
        ));
        assert!(matches!(
            head_of("(extend-protocol P T (m1 [x] x))"),
            Head::Extends("P")
        ));
    }

    #[test]
    fn index_tests_and_clears() {
        let ix = LazyIndex::new(&[5, 7, 70]);
        assert!(ix.test(5) && ix.test(7) && ix.test(70));
        assert!(!ix.test(4) && !ix.test(6) && !ix.test(71) && !ix.test(1000));
        ix.clear(7);
        ix.clear(3);
        assert!(!ix.test(7) && ix.test(70));
        let empty = LazyIndex::new(&[]);
        assert!(!empty.test(0));
    }
}
