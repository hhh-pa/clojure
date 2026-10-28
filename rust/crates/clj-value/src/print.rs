// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Printing values: [`pr_to_string`] (readable, round-trips through
//! the reader) and [`print_to_string`] (human-readable: strings and
//! chars unquoted). Follows `core_print.clj`: `*print-length*` /
//! `*print-level*` truncation, `", "` map separators, regex
//! `\Q..\E` quoting, and `##Inf`/`##-Inf`/`##NaN` doubles.
//!
//! Doubles print exactly like `Double.toString`: shortest round-trip
//! digits (via `ryu`) laid out by the JDK magnitude rules (plain in
//! `[1e-3, 1e7)`, scientific otherwise). `*print-meta*` prints the
//! `^meta` prefix when `meta` is set (and `readably`, as on the JVM).

use std::fmt::Write;
use std::sync::Arc;

use crate::Value;

pub(crate) mod print_opts {
    /// Printer options mirroring the `*print-*` dynamic vars.
    #[derive(Clone, Debug)]
    pub struct PrintOpts {
        /// `*print-length*`: max items per collection, then `...`.
        pub length: Option<usize>,
        /// `*print-level*`: max nesting depth; deeper collections print `#`.
        pub level: Option<usize>,
        /// `*print-namespace-maps*`: lift single-ns maps to `#:ns{...}`.
        /// Defaults to true, matching `clojure.main` (which binds it
        /// for scripts and the REPL).
        pub namespace_maps: bool,
        /// `*print-readably*`: quote strings/chars (pr) or not (print).
        pub readably: bool,
        /// `*print-meta*`: print the `^meta` prefix (needs `readably`).
        pub meta: bool,
        /// `*print-dup*`: constructor shapes for dupable kinds
        /// (length/level ignored, meta forced, strings/chars
        /// readable). Undupable kinds render the plain fallback —
        /// the driver throws before the engine ever asks.
        pub dup: bool,
    }

    impl Default for PrintOpts {
        fn default() -> Self {
            Self {
                length: None,
                level: None,
                namespace_maps: true,
                readably: false,
                meta: false,
                dup: false,
            }
        }
    }

    impl PrintOpts {
        /// Options for `pr`: readable, no limits.
        #[must_use]
        pub fn pr() -> Self {
            Self {
                readably: true,
                ..Self::default()
            }
        }

        /// Options for `print`: human-readable, no limits.
        #[must_use]
        pub fn print() -> Self {
            Self::default()
        }
    }
}
pub use print_opts::PrintOpts;

/// The `char-escape-string` entries (single source: the string
/// printer consults this table and boot builds the var from it).
/// Oracle `c-escape`: 7 entries, Char → escape text.
pub const CHAR_ESCAPES: &[(char, &str)] = &[
    ('\u{8}', "\\b"),
    ('\t', "\\t"),
    ('\n', "\\n"),
    ('\u{c}', "\\f"),
    ('\r', "\\r"),
    ('"', "\\\""),
    ('\\', "\\\\"),
];

/// The `char-name-string` entries (single source: the char printer
/// consults this table and boot builds the var from it). Oracle
/// `c-name`: 6 entries, Char → name text.
pub const CHAR_NAMES: &[(char, &str)] = &[
    ('\u{8}', "backspace"),
    ('\t', "tab"),
    ('\n', "newline"),
    ('\u{c}', "formfeed"),
    ('\r', "return"),
    (' ', "space"),
];

/// Look up a char in a single-source table.
fn char_table_lookup(table: &'static [(char, &'static str)], c: char) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

/// Print a value readably (`pr` semantics) with options.
#[must_use]
pub fn pr_to_string(v: &Value, opts: &PrintOpts) -> String {
    let mut out = String::new();
    let level = opts.level.map(|l| i64::try_from(l).unwrap_or(i64::MAX));
    print_into(v, opts, level, &mut out);
    out
}

/// Print a value human-readably (`print` semantics: strings/chars
/// unquoted, no limits).
#[must_use]
pub fn print_to_string(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::print())
}

/// Compatibility alias: `print` with explicit options.
#[must_use]
pub fn print(v: &Value, opts: &PrintOpts) -> String {
    pr_to_string(v, opts)
}

/// Remaining level budget (`None` = unlimited), threaded exactly like
/// `print-sequential`: each collection decrements first, then prints
/// `#` when the budget goes negative.
fn print_into(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    if !opts.dup {
        print_meta_prefix(v, opts, level, out);
        print_scalars(v, opts, level, out);
        return;
    }
    // Dup policy (no production pure path passes `dup: true` — the
    // driver renders dup shapes through the multis; this exists for
    // unit tests and documents the §1c pure shapes): length/level
    // ignored, meta forced; undupable kinds render the plain
    // fallback (dup ignored entirely).
    let eff = dup_effective_opts(v, opts);
    let level = if eff.dup { None } else { level };
    print_meta_prefix(v, &eff, level, out);
    // Dup-only shapes pre-dispatch here, so `print_scalars` keeps
    // its branch count (complexity ratchet).
    if eff.dup && print_dup_shape(v, &eff, level, out) {
        return;
    }
    print_scalars(v, &eff, level, out);
}

/// Effective options under `dup: true` (split for the complexity
/// budget): the dup policy for dupable kinds, the plain fallback
/// for undupable ones.
fn dup_effective_opts(v: &Value, opts: &PrintOpts) -> PrintOpts {
    if is_dup_throw_kind(v) {
        PrintOpts {
            dup: false,
            ..opts.clone()
        }
    } else {
        PrintOpts {
            length: None,
            level: None,
            meta: true,
            ..opts.clone()
        }
    }
}

/// Kinds with no `print-dup` shape (the driver throws `No method`
/// for these; the engine renders the plain fallback). Extensions
/// always take the dup policy: fixed renders ignore it, realized
/// seqs recurse through it, and ctor extensions (`dup_shape`) need
/// it.
fn is_dup_throw_kind(v: &Value) -> bool {
    match v {
        Value::Unbound(_)
        | Value::Throwable(_)
        | Value::Transient(_)
        | Value::Atom(_)
        | Value::Delay(_)
        | Value::Future(_)
        | Value::Promise(_)
        | Value::Agent(_)
        | Value::ReaderConditional { .. }
        | Value::MultiFn(_) => true,
        Value::HostType(h) => match h.as_ref() {
            crate::HostVal::Def(_) => false,
            crate::HostVal::Inst(i) => i.def.kind != crate::HostKind::Record,
        },
        _ => false,
    }
}

/// Print the dup-only shape for a dupable value; `true` when
/// handled (`→method` kinds answer `false` and fall through to
/// the plain rendering under dup policy).
fn print_dup_shape(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) -> bool {
    match v {
        // Dup beats `readably: false` for strings/chars only.
        Value::Str(s) => {
            print_string(s, true, out);
            true
        }
        Value::Char(c) => {
            print_char(*c, true, out);
            true
        }
        Value::Map(m) if !m.is_hashy() => {
            let class = if m.is_sorted() {
                DUP_TREEMAP
            } else {
                DUP_ARRAYMAP
            };
            print_dup_map_ctor(m, class, opts, level, out);
            true
        }
        Value::Set(s) if s.is_sorted() => {
            let items: Vec<&Value> = s.iter().collect();
            print_dup_seq_ctor(DUP_TREESET, &items, opts, level, out);
            true
        }
        Value::MapEntry { key, val } => {
            print_dup_seq_ctor(
                DUP_MAPENTRY,
                &[key.as_ref(), val.as_ref()],
                opts,
                level,
                out,
            );
            true
        }
        Value::Var(v) => {
            write!(out, "#=(var {})", v.sym).unwrap();
            true
        }
        // Classes dup as `#=name` (the JVM's `print-dup` on `Class`).
        Value::Class(c) => {
            out.push_str("#=");
            out.push_str(c.name());
            true
        }
        Value::HostType(h) => print_host_dup_shape(h, opts, level, out),
        Value::Extension(e) => print_ext_dup_shape(e, opts, level, out),
        _ => false,
    }
}

/// Dup-only host shapes (split for the complexity budget):
/// descriptors and records; anything else is a `→method` kind.
fn print_host_dup_shape(
    h: &crate::HostVal,
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) -> bool {
    match h {
        crate::HostVal::Def(d) => {
            out.push_str("#=");
            out.push_str(&d.name);
            true
        }
        crate::HostVal::Inst(i) if i.def.kind == crate::HostKind::Record => {
            print_dup_record(i, opts, level, out);
            true
        }
        _ => false,
    }
}

/// Dup-only extension shapes (split for the complexity budget):
/// subvector ctors and `dup_shape` hooks; anything else falls
/// through to the plain extension rendering.
fn print_ext_dup_shape(
    e: &Arc<dyn crate::ExtensionValue>,
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) -> bool {
    if let Some(sv) = e.as_any().downcast_ref::<crate::SubVectorValue>() {
        let items = sv.items();
        let refs: Vec<&Value> = items.iter().collect();
        print_dup_seq_ctor(DUP_SUBVEC, &refs, opts, level, out);
        return true;
    }
    match e.dup_shape() {
        Some(s) => {
            out.push_str(&s);
            true
        }
        None => false,
    }
}

fn print_scalars(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    match v {
        Value::Nil => out.push_str("nil"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => write!(out, "{i}").unwrap(),
        Value::Double(d) => out.push_str(&print_double(*d)),
        Value::BigInt(_) | Value::BigInteger(_) | Value::BigDec(_) | Value::Ratio(_) => {
            print_tower(v, opts, out)
        }
        Value::Str(s) => print_string(s, opts.readably, out),
        Value::Char(c) => print_char(*c, opts.readably, out),
        Value::Symbol(s) => write!(out, "{s}").unwrap(),
        Value::Keyword(k) => write!(out, "{k}").unwrap(),
        Value::Regex(pattern) => print_regex(pattern, out),
        Value::List(l) => {
            print_sequential(
                "(",
                l.iter().collect::<Vec<_>>(),
                ")",
                " ",
                opts,
                level,
                out,
            );
        }
        Value::Vector(v) => {
            print_sequential(
                "[",
                v.iter().collect::<Vec<_>>(),
                "]",
                " ",
                opts,
                level,
                out,
            );
        }
        other => print_more(other, opts, level, out),
    }
}

/// Readable tower suffixes (`5N`, `5.0M`); `str`/`print` use
/// the plain `toString` (`5`, `5.0`). `BigInteger` prints bare
/// either way (S2 oracle `bi-pr`/`bi-str`); ratios print `n/d`
/// either way. Chained so each match stays under the complexity
/// gate.
fn print_tower(v: &Value, opts: &PrintOpts, out: &mut String) {
    match v {
        Value::BigInt(n) => {
            write!(out, "{n}").unwrap();
            if opts.readably {
                out.push('N');
            }
        }
        Value::BigInteger(n) => {
            write!(out, "{n}").unwrap();
        }
        Value::BigDec(d) => {
            out.push_str(&d.to_bigdec_string());
            if opts.readably {
                out.push('M');
            }
        }
        _ => {
            if let Value::Ratio(r) = v {
                write!(out, "{}/{}", r.numer(), r.denom()).unwrap();
            }
        }
    }
}

/// Whether a var handle is anonymous (S4b `with-local-vars`):
/// no namespace plus the JVM `--unnamed--` marker name
/// (unqualified-but-named handles keep the `#'` print).
fn is_anon_var(sym: &crate::Symbol) -> bool {
    sym.namespace().is_none() && sym.name() == "--unnamed--"
}

/// Print a var. Anonymous vars render the JVM
/// `#<Var: --unnamed-->` (oracle `wv-pr`).
fn print_var(v: &crate::Var, out: &mut String) {
    if is_anon_var(&v.sym) {
        out.push_str("#<Var: --unnamed-->");
    } else {
        out.push_str("#'");
        write!(out, "{}", v.sym).unwrap();
    }
}

/// Print an unbound sentinel. Anonymous vars label through
/// their own print (oracle `wv-unbound`).
fn print_unbound(u: &crate::Unbound, out: &mut String) {
    write!(out, "#object[clojure.lang.Var$Unbound 0x{:x} ", u.var_id).unwrap();
    let label = if is_anon_var(&u.sym) {
        "Unbound: #<Var: --unnamed-->".to_owned()
    } else {
        format!("Unbound: #'{}", u.sym)
    };
    print_string(&label, true, out);
    out.push(']');
}

fn print_more(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    match v {
        // Entries print exactly like their 2-vector (Oracle FX2
        // `pr-e`, `str-e`, `entry-pr`).
        Value::MapEntry { key, val } => {
            print_sequential(
                "[",
                vec![key.as_ref(), val.as_ref()],
                "]",
                " ",
                opts,
                level,
                out,
            );
        }
        Value::Set(s) => {
            print_sequential(
                "#{",
                s.iter().collect::<Vec<_>>(),
                "}",
                " ",
                opts,
                level,
                out,
            );
        }
        Value::Map(m) => print_map_body(m, opts, level, out),
        Value::TaggedLiteral { tag, form } => {
            write!(out, "#{tag} ").unwrap();
            print_into(form, opts, level, out);
        }
        Value::Var(v) => print_var(v, out),
        Value::Unbound(u) => print_unbound(u, out),
        Value::Throwable(t) => print_throwable(t, opts, level, out),
        Value::Transient(t) => {
            let class = t.jvm_class();
            let id = t.id();
            write!(out, "#object[{class} 0x{id:x} \"{class}@{id:x}\"]").unwrap();
        }
        Value::Atom(a) => {
            let id = a.id();
            write!(
                out,
                "#object[clojure.lang.Atom 0x{id:x} \"clojure.lang.Atom@{id:x}\"]"
            )
            .unwrap();
        }
        Value::Delay(d) => {
            let id = d.id();
            write!(
                out,
                "#object[clojure.lang.Delay 0x{id:x} \"clojure.lang.Delay@{id:x}\"]"
            )
            .unwrap();
        }
        Value::Future(f) => {
            let class = f.jvm_class();
            let id = f.id();
            write!(out, "#object[{class} 0x{id:x} \"{class}@{id:x}\"]").unwrap();
        }
        Value::Promise(p) => {
            let class = p.jvm_class();
            let id = p.id();
            write!(out, "#object[{class} 0x{id:x} \"{class}@{id:x}\"]").unwrap();
        }
        Value::Volatile(v) => {
            let id = v.id();
            write!(
                out,
                "#object[clojure.lang.Volatile 0x{id:x} \"clojure.lang.Volatile@{id:x}\"]"
            )
            .unwrap();
        }
        other => print_port_or_rest(other, opts, level, out),
    }
}

/// Port-defined values (split from `print_more` for the complexity
/// budget): host types, multifns, and classes print here; anything
/// else falls through to `print_rest` unchanged.
fn print_port_or_rest(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    match v {
        Value::HostType(h) => print_host(h.as_ref(), opts, level, out),
        Value::MultiFn(m) => print_multifn(m, out),
        // Classes print their bare binary name in both modes —
        // interfaces too (S16 oracle `c-print-iface`).
        Value::Class(c) => out.push_str(c.name()),
        other => print_rest(other, opts, level, out),
    }
}

/// Port-defined types (S-typechain): records print `#Name{entries}`
/// in both modes (oracle `j-pr-rec`, `j-print-rec`; length and level
/// behave exactly like maps — the shared path); descriptors
/// print their name (`interface ` prefixed for interfaces, oracle
/// `rec-iface`); struct bases print the pinned `#object[…$Def …]`
/// shape and struct instances print exactly like their content map
/// (S12 oracles `sm-basis-print`, `sm-make-print`); anything else
/// prints the default `#object[Name 0xH fallback]` shape (oracle
/// `m-pr-dt` — the JVM embeds the live `toString` there, but print
/// is pure here, so the fallback renders instead; a custom
/// `toString` still runs through `str`).
fn print_host(h: &crate::HostVal, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    match h {
        crate::HostVal::Def(d) if d.kind == crate::HostKind::Interface => {
            out.push_str("interface ");
            out.push_str(&d.name);
        }
        crate::HostVal::Def(d) if d.kind == crate::HostKind::Struct => {
            let name = format!("{}$Def", d.name);
            let hex = crate::identity_hex(h);
            write!(out, "#object[{name} 0x{hex} \"{name}@{hex}\"]").unwrap();
        }
        crate::HostVal::Def(d) => out.push_str(&d.name),
        crate::HostVal::Inst(i) if i.def.kind == crate::HostKind::Record => {
            let entries: Vec<(&Value, &Value)> = i.content.iter().collect();
            print_map_entries(&format!("#{}", i.def.name), &entries, opts, level, out);
        }
        crate::HostVal::Inst(i) if i.def.kind == crate::HostKind::Struct => {
            print_map_body(&i.content, opts, level, out);
        }
        other => {
            let name = other.type_name();
            let hex = crate::identity_hex(other);
            write!(out, "#object[{name} 0x{hex} \"{name}@{hex}\"]").unwrap();
        }
    }
}

/// Multimethods (S-typechain stage 3): the default
/// `#object[clojure.lang.MultiFn 0xH fallback]` shape (oracle
/// `d-prefer` — the JVM embeds the live identity hash; stable
/// digits here, derived from the name).
fn print_multifn(m: &crate::MultiFn, out: &mut String) {
    let hex = crate::multifn_hex(&m.name);
    write!(
        out,
        "#object[clojure.lang.MultiFn 0x{hex} \"clojure.lang.MultiFn@{hex}\"]"
    )
    .unwrap();
}

fn print_rest(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    match v {
        Value::ReaderConditional { form, splicing } => {
            out.push_str(if *splicing { "#?@" } else { "#?" });
            print_into(form, opts, level, out);
        }
        Value::Agent(a) => {
            let id = a.id();
            write!(
                out,
                "#object[clojure.lang.Agent 0x{id:x} \"clojure.lang.Agent@{id:x}\"]"
            )
            .unwrap();
        }
        Value::Extension(e) => print_extension_body(e.as_ref(), opts, level, out),
        // All other variants are handled by `print_scalars`/`print_more`
        // (the single caller partitions the variants).
        _ => unreachable!("print partition"),
    }
}

fn print_map_body(m: &crate::Map, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    let entries: Vec<(&Value, &Value)> = m.iter().collect();
    let lifted = if opts.namespace_maps {
        lift_ns(&entries)
    } else {
        None
    };
    if let Some((ns, lifted)) = lifted {
        let flat: Vec<(&Value, &Value)> = lifted.iter().map(|(k, v)| (k, v)).collect();
        print_map_entries(&format!("#:{ns}"), &flat, opts, level, out);
    } else {
        print_map_entries("", &entries, opts, level, out);
    }
}

fn print_extension_body(
    e: &dyn crate::ExtensionValue,
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) {
    // Realized seqs print as lists with ambient options
    // (length/level truncation applies, like the JVM);
    // every other extension prints its fixed rendering.
    if let Some(s) = e.as_any().downcast_ref::<crate::RealizedSeq>() {
        let items = s.items();
        let refs: Vec<&Value> = items.iter().collect();
        print_sequential("(", refs, ")", " ", opts, level, out);
    } else if let Some(sv) = e.as_any().downcast_ref::<crate::SubVectorValue>() {
        let items = sv.items();
        let refs: Vec<&Value> = items.iter().collect();
        print_sequential("[", refs, "]", " ", opts, level, out);
    } else if !opts.readably {
        // Extensions with a `str` display render it here (only
        // `MathContext`, S2 oracle `wp-str`); the rest keep
        // `print()` (one line — region coverage over the
        // default is not gated).
        out.push_str(&e.print_str().unwrap_or_else(|| e.print()));
    } else {
        out.push_str(&e.print());
    }
}

/// Dup constructor classes (the `#=(<class>/create …)` shapes).
const DUP_ARRAYMAP: &str = "clojure.lang.PersistentArrayMap";
const DUP_TREEMAP: &str = "clojure.lang.PersistentTreeMap";
const DUP_TREESET: &str = "clojure.lang.PersistentTreeSet";
const DUP_MAPENTRY: &str = "clojure.lang.MapEntry";
const DUP_SUBVEC: &str = "clojure.lang.APersistentVector$SubVector";

/// `#=(<class>/create {k v, …})`: dup map entries print dup-aware
/// with no truncation (dup policy) and no namespace lifting (the
/// JVM `print-map` path).
fn print_dup_map_ctor(
    m: &crate::Map,
    class: &str,
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) {
    out.push_str("#=(");
    out.push_str(class);
    out.push_str("/create {");
    let mut first = true;
    for (k, v) in m.iter() {
        if !first {
            out.push_str(", ");
        }
        first = false;
        print_into(k, opts, level, out);
        out.push(' ');
        print_into(v, opts, level, out);
    }
    out.push_str("})");
}

/// `#=(<class>/create [items])`: dup sequential ctors (tree sets,
/// map entries, subvectors).
fn print_dup_seq_ctor(
    class: &str,
    items: &[&Value],
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) {
    out.push_str("#=(");
    out.push_str(class);
    out.push_str("/create [");
    let mut first = true;
    for item in items {
        if !first {
            out.push(' ');
        }
        first = false;
        print_into(item, opts, level, out);
    }
    out.push_str("])");
}

/// `#Name[v1, v2, ...]`: dup records print `(vals r)` — fields
/// in definition order, then extmap entries in
/// content-iteration order (always non-verbose —
/// `*verbose-defrecords*` is out of scope).
fn print_dup_record(i: &crate::HostInst, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    out.push('#');
    out.push_str(&i.def.name);
    out.push('[');
    let field_keys: Vec<Value> = i
        .def
        .fields
        .iter()
        .map(|f| Value::Keyword(crate::Keyword::new(None, f)))
        .collect();
    let mut first = true;
    for key in &field_keys {
        let Some(val) = i.content.get(key) else {
            continue;
        };
        if !first {
            out.push_str(", ");
        }
        first = false;
        print_into(val, opts, level, out);
    }
    // The extension map: content entries past the field keys, in
    // content-iteration order (the second half of `(vals r)`).
    for (k, v) in i.content.iter() {
        if field_keys.contains(k) {
            continue;
        }
        if !first {
            out.push_str(", ");
        }
        first = false;
        print_into(v, opts, level, out);
    }
    out.push(']');
}

/// `#error` rendering (the `print-throwable` shape): the outermost
/// cause and data, then one `:via` entry per cause-chain link (each
/// with its own `:data` when present), and an empty `:trace` (no JVM
/// stacks here). Messages always print as Clojure string literals;
/// data prints recursively with the ambient options.
fn print_throwable(t: &crate::Throwable, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    out.push_str("#error {:cause ");
    match &t.message {
        Some(m) => print_string(m, true, out),
        None => out.push_str("nil"),
    }
    out.push_str(" :data ");
    match &t.data {
        Some(d) => print_into(d, opts, level, out),
        None => out.push_str("nil"),
    }
    out.push_str(" :via [");
    let mut first = true;
    let mut current = Some(t);
    while let Some(th) = current {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str("{:type ");
        out.push_str(th.class.jvm_name());
        out.push_str(" :message ");
        match &th.message {
            Some(m) => print_string(m, true, out),
            None => out.push_str("nil"),
        }
        if let Some(d) = &th.data {
            out.push_str(" :data ");
            print_into(d, opts, level, out);
        }
        out.push('}');
        current = th.cause.as_deref();
    }
    out.push_str("] :trace []}");
}

/// `print-meta` (`core_print.clj`): with `meta` and `readably`, print
/// `^tag ` for a single truthy `:tag`, else `^map `. Empty metas and
/// non-`readably` output print nothing.
fn print_meta_prefix(v: &Value, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    if !opts.meta || (!opts.readably && !opts.dup) {
        return;
    }
    // Atoms hold their meta behind the lock (`Value::meta` borrows,
    // so it cannot serve them — clone it out instead). Futures and
    // promises carry per-handle meta outside `Value::meta` too.
    let extra = match v {
        Value::Atom(a) => Some(a.meta()),
        Value::Agent(a) => Some(a.meta()),
        Value::Future(f) => Some(f.meta()),
        Value::Promise(p) => Some(p.meta()),
        _ => None,
    };
    if let Some(meta) = extra {
        if let Some(m) = meta {
            print_meta_map(&m, opts, level, out);
        }
        return;
    }
    let Some(m) = v.meta() else {
        return;
    };
    print_meta_map(m, opts, level, out);
}

fn print_meta_map(m: &crate::Map, opts: &PrintOpts, level: Option<i64>, out: &mut String) {
    if m.is_empty() {
        return;
    }
    out.push('^');
    let tag_key = Value::Keyword(crate::Keyword::new(None, "tag"));
    match m.get(&tag_key) {
        Some(tag) if m.size() == 1 && tag.is_truthy() => print_into(tag, opts, level, out),
        _ => print_into(&Value::Map(m.clone()), opts, level, out),
    }
    out.push(' ');
}

fn print_sequential(
    begin: &str,
    items: Vec<&Value>,
    end: &str,
    sep: &str,
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) {
    let level = level.map(|l| l - 1);
    if level.is_some_and(|l| l < 0) {
        out.push('#');
        return;
    }
    out.push_str(begin);
    let mut first = true;
    for (i, item) in items.iter().enumerate() {
        if opts.length.is_some_and(|max| i >= max) {
            // The separator precedes `...` exactly as it would the
            // next item (trace `print-sequential`).
            if !first {
                out.push_str(sep);
            }
            out.push_str("...");
            break;
        }
        if !first {
            out.push_str(sep);
        }
        first = false;
        print_into(item, opts, level, out);
    }
    out.push_str(end);
}

fn print_map_entries(
    prefix: &str,
    entries: &[(&Value, &Value)],
    opts: &PrintOpts,
    level: Option<i64>,
    out: &mut String,
) {
    let level = level.map(|l| l - 1);
    if level.is_some_and(|l| l < 0) {
        out.push('#');
        return;
    }
    out.push_str(prefix);
    out.push('{');
    let mut first = true;
    for (i, (k, v)) in entries.iter().enumerate() {
        if opts.length.is_some_and(|max| i >= max) {
            if !first {
                out.push_str(", ");
            }
            out.push_str("...");
            break;
        }
        if !first {
            out.push_str(", ");
        }
        first = false;
        print_into(k, opts, level, out);
        out.push(' ');
        print_into(v, opts, level, out);
    }
    out.push('}');
}

/// `lift-ns`: when every key is qualified with the same namespace,
/// return that namespace plus the stripped entries. Public for the
/// evaluator-level print driver (single source — the driver must
/// lift exactly like the engine).
#[must_use]
pub fn lift_ns<'a>(entries: &[(&'a Value, &'a Value)]) -> Option<(String, Vec<(Value, Value)>)> {
    if entries.is_empty() {
        return None;
    }
    let mut ns: Option<String> = None;
    let mut lifted = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let (key_ns, stripped) = match k {
            Value::Keyword(kw) => (
                kw.namespace().map(str::to_owned),
                Value::Keyword(crate::Keyword::new(None, kw.name())),
            ),
            Value::Symbol(sym) => (
                sym.namespace().map(str::to_owned),
                Value::Symbol(crate::Symbol::new(None, sym.name())),
            ),
            _ => return None,
        };
        let key_ns = key_ns?;
        match &ns {
            None => ns = Some(key_ns),
            Some(want) if *want == key_ns => {}
            Some(_) => return None,
        }
        lifted.push((stripped, (*v).clone()));
    }
    ns.map(|ns| (ns, lifted))
}

fn print_string(s: &str, readably: bool, out: &mut String) {
    if !readably {
        out.push_str(s);
        return;
    }
    out.push('"');
    for c in s.chars() {
        match char_table_lookup(CHAR_ESCAPES, c) {
            Some(e) => out.push_str(e),
            None => out.push(c),
        }
    }
    out.push('"');
}

fn print_char(c: char, readably: bool, out: &mut String) {
    if !readably {
        out.push(c);
        return;
    }
    out.push('\\');
    match char_table_lookup(CHAR_NAMES, c) {
        Some(n) => out.push_str(n),
        None => out.push(c),
    }
}

/// Regex printer, transcribing `print-method` for `Pattern`
/// (including `\Q..\E` quoting awareness).
fn print_regex(pattern: &str, out: &mut String) {
    out.push_str("#\"");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut qmode = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            // Regex sources from the reader always pair backslashes;
            // a trailing lone backslash prints as-is (cannot occur
            // from `read`, only from constructed values).
            let c2 = chars.get(i + 1).copied().unwrap_or('\\');
            out.push('\\');
            out.push(c2);
            i += 2;
            qmode = if qmode { c2 != 'E' } else { c2 == 'Q' };
        } else if c == '"' {
            if qmode {
                out.push_str("\\E\\\"\\Q");
            } else {
                out.push_str("\\\"");
            }
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out.push('"');
}

/// Print a finite double exactly like `Double.toString`: shortest
/// round-trip digits (via `ryu`) laid out by magnitude — plain in
/// `[1e-3, 1e7)`, scientific otherwise. Non-finite values print as
/// `##Inf`/`##-Inf`/`##NaN` (the `print-method` for `Double`).
fn print_double(d: f64) -> String {
    if d.is_nan() {
        return "##NaN".to_owned();
    }
    if d.is_infinite() {
        return if d.is_sign_positive() {
            "##Inf".to_owned()
        } else {
            "##-Inf".to_owned()
        };
    }
    java_double_string(d)
}

/// Render a value the way the JVM reader interpolates it into error
/// messages (`"" + x`: scalars raw, collections readably; doubles
/// via [`java_double_string`], never `##`-forms).
#[must_use]
pub fn java_debug(v: &Value) -> String {
    match v {
        Value::Nil => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Double(d) => java_double_string(*d),
        Value::Str(s) => s.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Symbol(s) => s.to_string(),
        Value::Keyword(k) => k.to_string(),
        Value::Regex(pattern) => pattern.to_string(),
        Value::Var(v) => format!("#'{}", v.sym),
        Value::Unbound(u) => format!("Unbound: #'{}", u.sym),
        Value::Throwable(t) => java_debug_throwable(t),
        other => java_debug_rest(other),
    }
}

/// Host objects and readable collections. Chained so each match stays
/// under the complexity gate.
/// Port-defined type rendering for `java_debug` (split from
/// `java_debug_rest` for the complexity budget).
fn host_java_debug(h: &crate::HostVal) -> String {
    match h {
        crate::HostVal::Def(d) if d.kind == crate::HostKind::Interface => {
            format!("interface {}", d.name)
        }
        crate::HostVal::Def(d) => d.name.to_string(),
        h => crate::fallback_str(h),
    }
}

fn java_debug_rest(v: &Value) -> String {
    match v {
        Value::Transient(t) => format!("{}@{:x}", t.jvm_class(), t.id()),
        Value::Atom(a) => format!("clojure.lang.Atom@{:x}", a.id()),
        Value::Delay(d) => format!("clojure.lang.Delay@{:x}", d.id()),
        Value::Future(f) => format!("{}@{:x}", f.jvm_class(), f.id()),
        Value::Promise(p) => format!("{}@{:x}", p.jvm_class(), p.id()),
        Value::Agent(a) => format!("clojure.lang.Agent@{:x}", a.id()),
        Value::Volatile(v) => format!("clojure.lang.Volatile@{:x}", v.id()),
        Value::Extension(e) => e.print(),
        // Port-defined types: descriptors render their name;
        // instances render the `toString` fallback (pure — a custom
        // `toString` method only runs through `str` in `clj-eval`).
        Value::HostType(h) => host_java_debug(h.as_ref()),
        // Multimethods render the `toString` fallback (like the
        // JVM, which never overrides it with the name).
        Value::MultiFn(m) => crate::multifn_str(&m.name),
        // `Class.toString`: `class ` (or `interface `) plus the name.
        Value::Class(c) => class_java_debug(c),
        Value::List(_)
        | Value::Vector(_)
        | Value::MapEntry { .. }
        | Value::Map(_)
        | Value::Set(_)
        | Value::TaggedLiteral { .. }
        | Value::ReaderConditional { .. } => pr_to_string(v, &PrintOpts::pr()),
        // All other variants are handled by `java_debug` (the
        // single caller partitions the variants).
        _ => unreachable!("java_debug partition"),
    }
}

/// `Class.toString` (S16): `class ` (or `interface `) plus the
/// binary name. Shared with `str` in `clj-eval` so the two cannot
/// drift.
#[must_use]
pub fn class_java_debug(c: &crate::ClassVal) -> String {
    format!(
        "{} {}",
        if c.is_iface() { "interface" } else { "class" },
        c.name()
    )
}

/// `Throwable.toString`: class name, plus `: message` when present.
fn java_debug_throwable(t: &crate::Throwable) -> String {
    match &t.message {
        None => t.class.jvm_name().to_owned(),
        Some(m) => format!("{}: {}", t.class.jvm_name(), m),
    }
}

/// Shortest digits + decimal exponent parsed back from ryu's
/// unambiguous output: value = `d1.d2..dn × 10^exp10` with `d1 != 0`.
fn ryu_parts(d: f64) -> (bool, String, i32) {
    let text = ryu::Buffer::new().format(d).to_owned();
    let neg = text.starts_with('-');
    let text = text.strip_prefix('-').unwrap_or(text.as_str());
    let (mantissa, exp): (&str, i32) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse().unwrap_or(0)),
        None => (text, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let mut digits = format!("{int_part}{frac_part}");
    while digits.len() > 1 && digits.starts_with('0') {
        digits.remove(0);
    }
    let digits_len = i32::try_from(digits.len()).unwrap_or(0);
    let frac_len = i32::try_from(frac_part.len()).unwrap_or(0);
    // value = digits × 10^(exp - frac_len); as d1.d2.. × 10^exp10:
    let exp10 = exp - frac_len + digits_len - 1;
    // ryu pads integral values with ".0" (e.g. "10000000.0"); strip
    // the insignificant zeros (the exponent is unaffected).
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    (neg, digits, exp10)
}

/// `Double.toString`'s rendering for the subnormal doubles where it
/// is not shortest: `ryu` rounds these eight up to one digit (e.g.
/// `5e-324`) but `dtoa` keeps two (`4.9E-324`). The magnitude bits
/// select the fixed rendering; the caller re-applies the sign.
/// Verified against the JVM oracle over all `k = 1..100000`, every
/// subnormal parsed from a `<= 3`-digit decimal, every subnormal
/// parsed from a `<= 4`-digit decimal ending in `9`, and thousands
/// of random normals/subnormals: exactly these eight diverge.
fn dtoa_subnormal_patch(magnitude_bits: u64) -> Option<&'static str> {
    match magnitude_bits {
        0x1 => Some("4.9E-324"),
        0x2 => Some("9.9E-324"),
        0xa => Some("4.9E-323"),
        0xc => Some("5.9E-323"),
        0xe => Some("6.9E-323"),
        0x10 => Some("7.9E-323"),
        0x12 => Some("8.9E-323"),
        0x14 => Some("9.9E-323"),
        _ => None,
    }
}

/// `Double.toString`: [`print_double`]'s finite rendering, with
/// non-finite values as `Infinity`/`-Infinity`/`NaN` (used by error
/// interpolation, never by the value printer).
#[must_use]
pub fn java_double_string(d: f64) -> String {
    if d.is_nan() {
        return "NaN".to_owned();
    }
    if d.is_infinite() {
        return java_infinite_string(d);
    }
    if d == 0.0 {
        return java_zero_string(d);
    }
    if let Some(patched) = dtoa_subnormal_patch(d.to_bits() & 0x7FFF_FFFF_FFFF_FFFF) {
        return java_patched_string(d, patched);
    }
    let (neg, digits, exp10) = ryu_parts(d);
    java_finite_digits(neg, &digits, exp10)
}

fn java_infinite_string(d: f64) -> String {
    if d.is_sign_positive() {
        "Infinity".to_owned()
    } else {
        "-Infinity".to_owned()
    }
}

fn java_zero_string(d: f64) -> String {
    if d.is_sign_negative() {
        "-0.0".to_owned()
    } else {
        "0.0".to_owned()
    }
}

fn java_patched_string(d: f64, patched: &str) -> String {
    if d.is_sign_negative() {
        format!("-{patched}")
    } else {
        patched.to_owned()
    }
}

/// Finite `Double.toString` layout: plain in `[1e-3, 1e7)` (`exp10`
/// in `[-3, 6]`), scientific otherwise. Digits are `d1.d2..dn` with
/// `d1 != 0`.
fn java_finite_digits(neg: bool, digits: &str, exp10: i32) -> String {
    debug_assert!(!digits.is_empty() && !digits.starts_with('0'));
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-3..=6).contains(&exp10) {
        push_plain_digits(&mut out, digits, exp10);
    } else {
        push_scientific_digits(&mut out, digits, exp10);
    }
    out
}

fn push_plain_digits(out: &mut String, digits: &str, exp10: i32) {
    if exp10 >= 0 {
        push_plain_nonneg(out, digits, exp10);
    } else {
        out.push_str("0.");
        out.push_str(&"0".repeat((-exp10 - 1) as usize));
        out.push_str(digits);
    }
}

fn push_plain_nonneg(out: &mut String, digits: &str, exp10: i32) {
    let int_len = exp10 as usize + 1;
    if digits.len() >= int_len {
        out.push_str(&digits[..int_len]);
        out.push('.');
        let frac = &digits[int_len..];
        if frac.is_empty() {
            out.push('0');
        } else {
            out.push_str(frac);
        }
    } else {
        out.push_str(digits);
        out.push_str(&"0".repeat(int_len - digits.len()));
        out.push_str(".0");
    }
}

fn push_scientific_digits(out: &mut String, digits: &str, exp10: i32) {
    let (d1, rest) = digits.split_at(1);
    out.push_str(d1);
    out.push('.');
    if rest.is_empty() {
        out.push('0');
    } else {
        out.push_str(rest);
    }
    out.push('E');
    write!(out, "{exp10}").unwrap();
}

#[cfg(test)]
mod tests {
    use super::{PrintOpts, java_double_string, pr_to_string, print_to_string};
    use crate::{Keyword, Symbol, Value};
    use std::sync::Arc;

    fn pr(v: &Value) -> String {
        pr_to_string(v, &PrintOpts::pr())
    }

    #[test]
    fn scalars() {
        assert_eq!(pr(&Value::Nil), "nil");
        assert_eq!(pr(&Value::Bool(true)), "true");
        assert_eq!(pr(&Value::Bool(false)), "false");
        assert_eq!(pr(&Value::Int(42)), "42");
        assert_eq!(pr(&Value::Int(-7)), "-7");
        assert_eq!(pr(&Value::Str(Arc::from("hi"))), "\"hi\"");
        assert_eq!(pr(&Value::Char('a')), "\\a");
        assert_eq!(pr(&Value::Char('\n')), "\\newline");
        assert_eq!(pr(&Value::Char(' ')), "\\space");
        assert_eq!(pr(&Value::Symbol(Symbol::new(None, "x"))), "x");
        assert_eq!(pr(&Value::Symbol(Symbol::new(Some("a"), "b"))), "a/b");
        assert_eq!(pr(&Value::Keyword(Keyword::new(None, "k"))), ":k");
        assert_eq!(pr(&Value::Keyword(Keyword::new(Some("u"), "id"))), ":u/id");
        assert_eq!(pr(&Value::Regex(Arc::from("a+"))), "#\"a+\"");
    }

    #[test]
    fn anon_var_prints() {
        // Anonymous vars (S4b `with-local-vars`) carry no
        // namespace: the JVM `#<Var: --unnamed-->` print, and
        // unbounds label through it (oracle `wv-pr`,
        // `wv-unbound`).
        use crate::{Unbound, Var};
        let var = Value::Var(Var {
            id: 7,
            sym: Symbol::new(None, "--unnamed--"),
        });
        assert_eq!(pr(&var), "#<Var: --unnamed-->");
        let named = Value::Var(Var {
            id: 8,
            sym: Symbol::new(Some("user"), "x"),
        });
        assert_eq!(pr(&named), "#'user/x");
        let unb = Value::Unbound(Unbound {
            sym: Symbol::new(None, "--unnamed--"),
            var_id: 9,
        });
        assert_eq!(
            pr(&unb),
            "#object[clojure.lang.Var$Unbound 0x9 \"Unbound: #<Var: --unnamed-->\"]"
        );
    }

    #[test]
    fn string_escapes() {
        assert_eq!(
            pr(&Value::Str(Arc::from("a\nb\t\"\\"))),
            "\"a\\nb\\t\\\"\\\\\""
        );
        assert_eq!(pr(&Value::Str(Arc::from("\u{8}\u{c}\r"))), "\"\\b\\f\\r\"");
        // Other control chars print raw (as on the JVM).
        assert_eq!(pr(&Value::Str(Arc::from("\u{1}é"))), "\"\u{1}é\"");
        // Non-readable prints raw.
        assert_eq!(print_to_string(&Value::Str(Arc::from("a\nb"))), "a\nb");
        assert_eq!(print_to_string(&Value::Char('\n')), "\n");
    }

    #[test]
    fn regex_quoting() {
        // `"` outside \Q..\E is backslash-escaped.
        assert_eq!(pr(&Value::Regex(Arc::from("a\"b"))), "#\"a\\\"b\"");
        // Inside \Q..\E the printer closes, escapes, and reopens quoting.
        assert_eq!(
            pr(&Value::Regex(Arc::from("\\Qa\"b\\E"))),
            "#\"\\Qa\\E\\\"\\Qb\\E\""
        );
    }

    #[test]
    fn doubles() {
        assert_eq!(pr(&Value::Double(1.5)), "1.5");
        assert_eq!(pr(&Value::Double(100.0)), "100.0");
        assert_eq!(pr(&Value::Double(0.0)), "0.0");
        assert_eq!(pr(&Value::Double(-0.0)), "-0.0");
        assert_eq!(pr(&Value::Double(0.001)), "0.001");
        assert_eq!(pr(&Value::Double(0.0001)), "1.0E-4");
        assert_eq!(pr(&Value::Double(1e7)), "1.0E7");
        assert_eq!(pr(&Value::Double(9_999_999.0)), "9999999.0");
        assert_eq!(pr(&Value::Double(1e300)), "1.0E300");
        assert_eq!(pr(&Value::Double(1.234e-7)), "1.234E-7");
        assert_eq!(pr(&Value::Double(0.1 + 0.2)), "0.30000000000000004");
        assert_eq!(pr(&Value::Double(f64::INFINITY)), "##Inf");
        assert_eq!(pr(&Value::Double(f64::NEG_INFINITY)), "##-Inf");
        assert_eq!(pr(&Value::Double(f64::NAN)), "##NaN");
        assert_eq!(
            pr(&Value::Double(1.797_693_134_862_315_7e308)),
            "1.7976931348623157E308"
        );
    }

    #[test]
    fn doubles_dtoa_subnormal_patch() {
        // `Double.toString` keeps two digits here; `ryu` would round
        // up to one. Oracle-pinned (see `dtoa_subnormal_patch`).
        let cases = [
            (0x1u64, "4.9E-324"),
            (0x2, "9.9E-324"),
            (0xa, "4.9E-323"),
            (0xc, "5.9E-323"),
            (0xe, "6.9E-323"),
            (0x10, "7.9E-323"),
            (0x12, "8.9E-323"),
            (0x14, "9.9E-323"),
        ];
        for (bits, want) in cases {
            assert_eq!(pr(&Value::Double(f64::from_bits(bits))), want);
            assert_eq!(
                pr(&Value::Double(f64::from_bits(bits | 0x8000_0000_0000_0000))),
                format!("-{want}")
            );
            assert_eq!(java_double_string(f64::from_bits(bits)), want);
        }
        // Neighbors still take the `ryu` path.
        assert_eq!(pr(&Value::Double(f64::from_bits(0x3))), "1.5E-323");
        assert_eq!(pr(&Value::Double(f64::from_bits(0x16))), "1.1E-322");
    }

    #[test]
    fn collections() {
        // push_front builds from the right: (1 2).
        let list = Value::List(
            crate::List::new_sync()
                .push_front(Value::Int(2))
                .push_front(Value::Int(1)),
        );
        assert_eq!(pr(&list), "(1 2)");
        let vector = Value::Vector(
            crate::Vector::new_sync()
                .push_back(Value::Int(1))
                .push_back(Value::Str(Arc::from("x"))),
        );
        assert_eq!(pr(&vector), "[1 \"x\"]");
        assert_eq!(pr(&Value::Vector(crate::Vector::new_sync())), "[]");
        assert_eq!(pr(&Value::List(crate::List::new_sync())), "()");
        assert_eq!(pr(&Value::Map(crate::new_map())), "{}");
        assert_eq!(pr(&Value::Set(crate::new_set())), "#{}");
    }

    #[test]
    fn map_separators_and_single_entry() {
        let m = Value::Map(
            crate::new_map().insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1)),
        );
        assert_eq!(pr(&m), "{:a 1}");
        // Multi-entry order is hasher-defined; assert setwise below.
        let m2 = Value::Map(
            crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "b")), Value::Int(2)),
        );
        let text = pr(&m2);
        assert!(text.starts_with('{') && text.ends_with('}'), "{text}");
        assert!(text.contains(":a 1") && text.contains(":b 2"), "{text}");
        assert!(text.contains(", "), "{text}");
    }

    #[test]
    fn print_length_level() {
        let v = Value::Vector(
            crate::Vector::new_sync()
                .push_back(Value::Int(1))
                .push_back(Value::Int(2))
                .push_back(Value::Int(3)),
        );
        let opts = PrintOpts {
            length: Some(2),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&v, &opts), "[1 2 ...]");
        let exact = PrintOpts {
            length: Some(3),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&v, &exact), "[1 2 3]");

        let nested = Value::Vector(crate::Vector::new_sync().push_back(Value::Vector(
            crate::Vector::new_sync().push_back(Value::Int(1)),
        )));
        let deep = PrintOpts {
            level: Some(1),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&nested, &deep), "[#]");
        let top = PrintOpts {
            level: Some(0),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&nested, &top), "#");
        let unlimited = PrintOpts {
            level: Some(2),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&nested, &unlimited), "[[1]]");
    }

    #[test]
    fn namespace_maps_lifting() {
        let m = Value::Map(
            crate::new_map()
                .insert(
                    Value::Keyword(Keyword::new(Some("user"), "a")),
                    Value::Int(1),
                )
                .insert(
                    Value::Keyword(Keyword::new(Some("user"), "b")),
                    Value::Int(2),
                ),
        );
        // Lifting is on by default (matching `clojure.main`).
        let lifted = pr(&m);
        assert!(lifted.starts_with("#:user{"), "{lifted}");
        assert!(
            lifted.contains(":a 1") && lifted.contains(":b 2"),
            "{lifted}"
        );
        let flat_opts = PrintOpts {
            namespace_maps: false,
            ..PrintOpts::pr()
        };
        let plain = pr_to_string(&m, &flat_opts);
        assert!(plain.contains(":user/a"), "{plain}");
        // Mixed namespaces do not lift.
        let mixed = Value::Map(
            crate::new_map()
                .insert(Value::Keyword(Keyword::new(Some("u"), "a")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "b")), Value::Int(2)),
        );
        assert!(!pr(&mixed).starts_with("#:"),);
    }

    #[test]
    fn print_meta_prefix() {
        use crate::Vector;
        let tag_key = || Value::Keyword(Keyword::new(None, "tag"));
        // ^String short form: single truthy :tag prints bare.
        let tag_meta =
            crate::new_map().insert(tag_key(), Value::Symbol(Symbol::new(None, "String")));
        let v = Value::Vector(Vector::from_vec(vec![Value::Symbol(Symbol::new(
            None, "x",
        ))]));
        assert_eq!(pr(&v), "[x]");
        let with_tag = v.clone().with_meta(Some(tag_meta.clone()));
        let opts = PrintOpts {
            meta: true,
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&with_tag, &opts), "^String [x]");
        // Full map otherwise (two entries: order-insensitive check).
        let full_meta = tag_meta.insert(Value::Keyword(Keyword::new(None, "k")), Value::Bool(true));
        let full = v.clone().with_meta(Some(full_meta));
        let text = pr_to_string(&full, &opts);
        assert!(text.starts_with("^{") && text.ends_with("} [x]"), "{text}");
        assert!(
            text.contains(":tag String") && text.contains(":k true"),
            "{text}"
        );
        // Falsy :tag prints the full map.
        let falsy = crate::new_map().insert(tag_key(), Value::Nil);
        let with_falsy = v.clone().with_meta(Some(falsy));
        assert_eq!(pr_to_string(&with_falsy, &opts), "^{:tag nil} [x]");
        // Needs readably: `print` never shows meta.
        assert_eq!(print_to_string(&with_tag), "[x]");
        let no_readably = PrintOpts {
            meta: true,
            ..PrintOpts::print()
        };
        assert_eq!(pr_to_string(&with_tag, &no_readably), "[x]");
        // Symbols print meta too.
        let sym = Value::Symbol(Symbol::new(None, "x")).with_meta(Some(
            crate::new_map().insert(Value::Keyword(Keyword::new(None, "k")), Value::Bool(true)),
        ));
        assert_eq!(pr_to_string(&sym, &opts), "^{:k true} x");
    }

    #[test]
    fn tagged_and_conditional_printing() {
        let tl = Value::TaggedLiteral {
            tag: Symbol::new(None, "inst"),
            form: Box::new(Value::Str(std::sync::Arc::from("2020-01-01"))),
        };
        assert_eq!(pr(&tl), "#inst \"2020-01-01\"");
        let rc = Value::ReaderConditional {
            form: Box::new(Value::List(crate::List::from_vec(vec![
                Value::Keyword(Keyword::new(None, "a")),
                Value::Int(1),
            ]))),
            splicing: false,
        };
        assert_eq!(pr(&rc), "#?(:a 1)");
        let rcs = Value::ReaderConditional {
            form: Box::new(Value::List(crate::List::from_vec(vec![Value::Int(1)]))),
            splicing: true,
        };
        assert_eq!(pr(&rcs), "#?@(1)");
    }

    #[test]
    fn java_debug_rendering() {
        use super::java_debug;
        assert_eq!(java_debug(&Value::Nil), "null");
        assert_eq!(java_debug(&Value::Bool(true)), "true");
        assert_eq!(java_debug(&Value::Int(-7)), "-7");
        assert_eq!(java_debug(&Value::Double(f64::INFINITY)), "Infinity");
        assert_eq!(java_debug(&Value::Double(f64::NEG_INFINITY)), "-Infinity");
        assert_eq!(java_debug(&Value::Double(f64::NAN)), "NaN");
        assert_eq!(java_debug(&Value::Double(1.5)), "1.5");
        assert_eq!(java_debug(&Value::Str(std::sync::Arc::from("s"))), "s");
        assert_eq!(java_debug(&Value::Char('c')), "c");
        assert_eq!(java_debug(&Value::Char('\n')), "\n");
        assert_eq!(
            java_debug(&Value::Symbol(Symbol::new(Some("a"), "b"))),
            "a/b"
        );
        assert_eq!(java_debug(&Value::Keyword(Keyword::new(None, "k"))), ":k");
        assert_eq!(java_debug(&Value::Regex(std::sync::Arc::from("a+"))), "a+");
        let v = Value::Vector(crate::Vector::from_vec(vec![Value::Int(1)]));
        assert_eq!(java_debug(&v), "[1]");
        // Oracle T1192 FX2 `pr-e`: entries render as their 2-vector.
        let e = crate::map_entry(Value::Keyword(Keyword::new(None, "a")), Value::Int(1));
        assert_eq!(java_debug(&e), "[:a 1]");
        // Classes render `Class.toString` (the totality arm —
        // production `str` calls `class_java_debug` directly).
        let class = |name: &str, iface: bool| {
            Value::Class(Arc::new(crate::ClassVal::new(
                Arc::from(name),
                iface,
                Arc::from([]),
            )))
        };
        assert_eq!(
            java_debug(&class("java.lang.String", false)),
            "class java.lang.String"
        );
        assert_eq!(
            java_debug(&class("java.io.Serializable", true)),
            "interface java.io.Serializable"
        );
    }

    #[test]
    fn java_debug_host_values() {
        // Port-defined types: descriptors render their name
        // (`interface ` prefixed for interfaces); instances render
        // the `toString` fallback.
        use super::java_debug;
        fn def(kind: crate::HostKind) -> std::sync::Arc<crate::HostDef> {
            Arc::new(crate::HostDef {
                name: Arc::from("user.T"),
                kind,
                fields: Vec::new(),
                methods: std::collections::HashMap::new(),
                interfaces: Vec::new(),
                prefix: None,
                impl_ns: None,
                sigs: std::collections::HashMap::new(),
                takes_this: true,
                struct_keys: Vec::new(),
            })
        }
        let d = def(crate::HostKind::Type);
        assert_eq!(
            java_debug(&Value::HostType(Arc::new(crate::HostVal::Def(d.clone())))),
            "user.T"
        );
        let i = def(crate::HostKind::Interface);
        assert_eq!(
            java_debug(&Value::HostType(Arc::new(crate::HostVal::Def(i)))),
            "interface user.T"
        );
        let inst = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def: d,
            content: crate::new_map(),
            meta: None,
        })));
        let s = java_debug(&inst);
        assert!(
            s.starts_with("user.T@") && s.len() > "user.T@".len(),
            "instance fallback shape, got {s}"
        );
    }

    #[test]
    fn java_debug_multifn() {
        // Multimethods render the `toString` fallback (unreachable
        // from Clojure — `str`/`pr` take their own arms — so pinned
        // directly).
        use super::java_debug;
        let mf = Value::MultiFn(Arc::new(crate::MultiFn {
            name: Arc::from("m"),
            dispatch: Value::Nil,
            default: Value::Nil,
            methods: crate::new_map(),
            prefers: crate::new_map(),
        }));
        assert_eq!(java_debug(&mf), crate::multifn_str("m"));
    }

    #[test]
    fn print_alias_matches_pr_to_string() {
        use super::print;
        let v = Value::Vector(crate::Vector::from_vec(vec![Value::Int(1)]));
        assert_eq!(print(&v, &PrintOpts::pr()), pr(&v));
        assert_eq!(print(&v, &PrintOpts::print()), print_to_string(&v));
    }

    #[test]
    fn transient_prints_object_form() {
        use crate::{Transient, TransientKind, TransientVector};
        let t = Transient::new(TransientKind::Vector(TransientVector::new()));
        let id = t.id();
        let class = "clojure.lang.PersistentVector$TransientVector";
        assert_eq!(
            pr(&Value::Transient(t)),
            format!("#object[{class} 0x{id:x} \"{class}@{id:x}\"]")
        );
    }

    #[test]
    fn throwable_without_messages_prints_nils() {
        use crate::{ThrowClass, Throwable};
        let t = Value::Throwable(Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: None,
            data: None,
            cause: None,
        });
        assert_eq!(
            pr(&t),
            "#error {:cause nil :data nil :via [{:type java.lang.Exception :message nil}] :trace []}"
        );
        let inner = Throwable {
            id: 2,
            class: ThrowClass::RuntimeException,
            message: None,
            data: None,
            cause: None,
        };
        let outer = Value::Throwable(Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: Some(Arc::from("top")),
            data: None,
            cause: Some(Box::new(inner)),
        });
        assert_eq!(
            pr(&outer),
            "#error {:cause \"top\" :data nil :via [{:type java.lang.Exception :message \"top\"} {:type java.lang.RuntimeException :message nil}] :trace []}"
        );
    }

    #[test]
    fn future_and_promise_meta_prefix() {
        use crate::{Future, Promise};
        let meta =
            crate::new_map().insert(Value::Keyword(Keyword::new(None, "k")), Value::Bool(true));
        let opts = PrintOpts {
            meta: true,
            ..PrintOpts::pr()
        };
        let class = "clojure.core$future_call$reify";
        let f = Future::new().with_meta(Some(meta.clone()));
        let id = f.id();
        assert_eq!(
            pr_to_string(&Value::Future(f), &opts),
            format!("^{{:k true}} #object[{class} 0x{id:x} \"{class}@{id:x}\"]")
        );
        let class = "clojure.core$promise$reify";
        let p = Promise::new().with_meta(Some(meta));
        let id = p.id();
        assert_eq!(
            pr_to_string(&Value::Promise(p), &opts),
            format!("^{{:k true}} #object[{class} 0x{id:x} \"{class}@{id:x}\"]")
        );
    }

    #[test]
    fn empty_meta_prints_no_prefix() {
        let v = Value::Vector(crate::Vector::from_vec(vec![Value::Int(1)]))
            .with_meta(Some(crate::new_map()));
        let opts = PrintOpts {
            meta: true,
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&v, &opts), "[1]");
    }

    #[test]
    fn map_level_truncation() {
        let m = Value::Map(
            crate::new_map().insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1)),
        );
        let top = PrintOpts {
            level: Some(0),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&m, &top), "#");
    }

    #[test]
    fn map_length_truncation() {
        let m = Value::Map(
            crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "b")), Value::Int(2)),
        );
        let one = PrintOpts {
            length: Some(1),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&m, &one), "{:a 1, ...}");
        let none = PrintOpts {
            length: Some(0),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&m, &none), "{...}");
    }

    #[test]
    fn java_debug_identities() {
        use super::java_debug;
        use crate::{
            RangeNum, RangeValue, ThrowClass, Throwable, Transient, TransientKind, TransientVector,
            Unbound, Var,
        };
        let var = Value::Var(Var {
            id: 1,
            sym: Symbol::new(Some("user"), "x"),
        });
        assert_eq!(java_debug(&var), "#'user/x");
        let unb = Value::Unbound(Unbound {
            sym: Symbol::new(None, "y"),
            var_id: 2,
        });
        assert_eq!(java_debug(&unb), "Unbound: #'y");
        let bare = Value::Throwable(Throwable {
            id: 1,
            class: ThrowClass::Exception,
            message: None,
            data: None,
            cause: None,
        });
        assert_eq!(java_debug(&bare), "java.lang.Exception");
        let msg = Value::Throwable(Throwable {
            id: 1,
            class: ThrowClass::RuntimeException,
            message: Some(Arc::from("boom")),
            data: None,
            cause: None,
        });
        assert_eq!(java_debug(&msg), "java.lang.RuntimeException: boom");
        let t = Transient::new(TransientKind::Vector(TransientVector::new()));
        let id = t.id();
        assert_eq!(
            java_debug(&Value::Transient(t)),
            format!("clojure.lang.PersistentVector$TransientVector@{id:x}")
        );
        let range = Value::Extension(Arc::new(RangeValue::new(
            RangeNum::Int(0),
            Some(RangeNum::Int(1)),
            RangeNum::Int(1),
            true,
            Some(1),
        )));
        assert_eq!(java_debug(&range), "(...)");
    }

    #[test]
    #[should_panic(expected = "print partition")]
    fn print_rest_rejects_upstream_variants() {
        // Defensive arm: the single caller partitions the variants,
        // so scalars never arrive here (only a dispatch bug could).
        let mut out = String::new();
        super::print_rest(&Value::Nil, &PrintOpts::pr(), None, &mut out);
    }

    #[test]
    #[should_panic(expected = "java_debug partition")]
    fn java_debug_rest_rejects_upstream_variants() {
        // Defensive arm: the single caller partitions the variants,
        // so scalars never arrive here (only a dispatch bug could).
        let _ = super::java_debug_rest(&Value::Nil);
    }

    fn dup(v: &Value) -> String {
        pr_to_string(
            v,
            &PrintOpts {
                dup: true,
                ..PrintOpts::pr()
            },
        )
    }

    #[test]
    fn char_tables_match_oracle_literals() {
        // Single-source contents (oracle `c-escape`/`c-name`), entry
        // for entry — boot builds the vars from these same tables.
        assert_eq!(
            super::CHAR_ESCAPES,
            &[
                ('\u{8}', "\\b"),
                ('\t', "\\t"),
                ('\n', "\\n"),
                ('\u{c}', "\\f"),
                ('\r', "\\r"),
                ('"', "\\\""),
                ('\\', "\\\\"),
            ]
        );
        assert_eq!(
            super::CHAR_NAMES,
            &[
                ('\u{8}', "backspace"),
                ('\t', "tab"),
                ('\n', "newline"),
                ('\u{c}', "formfeed"),
                ('\r', "return"),
                (' ', "space"),
            ]
        );
    }

    #[test]
    fn dup_delegating_kinds_keep_method_shapes() {
        // `→method` kinds render exactly the readable shapes.
        assert_eq!(dup(&Value::Nil), "nil");
        assert_eq!(dup(&Value::Bool(true)), "true");
        assert_eq!(dup(&Value::Int(5)), "5");
        assert_eq!(dup(&Value::Double(1.5)), "1.5");
        assert_eq!(dup(&Value::Double(f64::NAN)), "##NaN");
        assert_eq!(dup(&Value::Str(Arc::from("hi"))), "\"hi\"");
        assert_eq!(dup(&Value::Char('a')), "\\a");
        assert_eq!(dup(&Value::Char('\n')), "\\newline");
        assert_eq!(dup(&Value::Keyword(Keyword::new(Some("u"), "id"))), ":u/id");
        assert_eq!(dup(&Value::Symbol(Symbol::new(Some("a"), "b"))), "a/b");
        assert_eq!(dup(&Value::Regex(Arc::from("a+"))), "#\"a+\"");
        assert_eq!(
            dup(&Value::List(crate::List::from_vec(vec![
                Value::Int(1),
                Value::Int(2)
            ]))),
            "(1 2)"
        );
        assert_eq!(
            dup(&Value::Vector(crate::Vector::from_vec(vec![
                Value::Int(1),
                Value::Int(2)
            ]))),
            "[1 2]"
        );
        assert_eq!(dup(&Value::Set(crate::new_set())), "#{}");
    }

    #[test]
    fn dup_ignores_length_and_level() {
        let v = Value::Vector(crate::Vector::from_vec(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]));
        let opts = PrintOpts {
            dup: true,
            length: Some(0),
            level: Some(0),
            ..PrintOpts::pr()
        };
        assert_eq!(pr_to_string(&v, &opts), "[1 2 3]");
        let nested = Value::Vector(crate::Vector::from_vec(vec![Value::Vector(
            crate::Vector::from_vec(vec![Value::Int(1)]),
        )]));
        assert_eq!(pr_to_string(&nested, &opts), "[[1]]");
        // Array-map ctors ignore length too.
        let m = Value::Map(
            crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "b")), Value::Int(2)),
        );
        assert_eq!(
            pr_to_string(&m, &opts),
            "#=(clojure.lang.PersistentArrayMap/create {:a 1, :b 2})"
        );
    }

    #[test]
    fn dup_forces_meta_and_readable_strings() {
        // Meta prints with no `*print-meta*`, and meta values render
        // dup-aware (array-map → ctor form).
        let v = Value::Vector(crate::Vector::from_vec(vec![Value::Int(1)])).with_meta(Some(
            crate::new_map().insert(Value::Keyword(Keyword::new(None, "a")), Value::Int(1)),
        ));
        assert_eq!(
            dup(&v),
            "^#=(clojure.lang.PersistentArrayMap/create {:a 1}) [1]"
        );
        // Dup beats `readably: false` for strings and chars only.
        let unreadable = PrintOpts {
            dup: true,
            ..PrintOpts::print()
        };
        assert_eq!(
            pr_to_string(&Value::Str(Arc::from("hi")), &unreadable),
            "\"hi\""
        );
        assert_eq!(pr_to_string(&Value::Char('a'), &unreadable), "\\a");
        // BigInt keeps ambient readably (no forced `N`).
        assert_eq!(pr_to_string(&Value::BigInt(5.into()), &unreadable), "5");
    }

    #[test]
    fn dup_ctor_shapes() {
        let entry = |k: &str, v: Value| (Value::Keyword(Keyword::new(None, k)), v);
        let (ka, va) = entry("a", Value::Int(1));
        // Array maps (small, insertion-ordered).
        let array = Value::Map(crate::new_map().insert(ka.clone(), va.clone()));
        assert_eq!(
            dup(&array),
            "#=(clojure.lang.PersistentArrayMap/create {:a 1})"
        );
        // Tree maps (sorted — keys stored in sort order, like
        // `sorted-map` builds them).
        let sorted = Value::Map(
            crate::new_map()
                .with_sort(Some(crate::SortSpec::Natural))
                .with_ordered_entries(vec![
                    (ka.clone(), va.clone()),
                    (Value::Keyword(Keyword::new(None, "b")), Value::Int(2)),
                ]),
        );
        assert_eq!(
            dup(&sorted),
            "#=(clojure.lang.PersistentTreeMap/create {:a 1, :b 2})"
        );
        // Hash maps delegate (readable shape, order-free pin).
        let hash = Value::Map(
            crate::new_map()
                .with_hashy(true)
                .insert(ka.clone(), va.clone()),
        );
        assert_eq!(dup(&hash), "{:a 1}");
        // Tree sets take the ctor; hash sets delegate.
        let tset = Value::Set(
            crate::new_set()
                .with_sort(Some(crate::SortSpec::Natural))
                .with_ordered_members(vec![Value::Int(1), Value::Int(2)]),
        );
        assert_eq!(
            dup(&tset),
            "#=(clojure.lang.PersistentTreeSet/create [1 2])"
        );
        // Map entries.
        let e = crate::map_entry(ka, va);
        assert_eq!(dup(&e), "#=(clojure.lang.MapEntry/create [:a 1])");
        // Subvectors.
        let sv = Value::Extension(Arc::new(crate::SubVectorValue::new(
            crate::Vector::from_vec(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            1,
            3,
        )));
        assert_eq!(
            dup(&sv),
            "#=(clojure.lang.APersistentVector$SubVector/create [2 3])"
        );
        // Vars (no meta prefix even with the forcing — vars carry
        // no meta in the port).
        let var = Value::Var(crate::Var {
            id: 1,
            sym: Symbol::new(Some("user"), "x"),
        });
        assert_eq!(dup(&var), "#=(var user/x)");
        // Type descriptors (interfaces lose the `interface ` prefix).
        let def = Arc::new(crate::HostDef {
            name: Arc::from("user.T"),
            kind: crate::HostKind::Type,
            fields: Vec::new(),
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        assert_eq!(
            dup(&Value::HostType(Arc::new(crate::HostVal::Def(def.clone())))),
            "#=user.T"
        );
        // Records: field-order vals (insertion order scrambled to
        // prove field order wins).
        let rdef = Arc::new(crate::HostDef {
            name: Arc::from("user.RP"),
            kind: crate::HostKind::Record,
            fields: vec![Arc::from("x"), Arc::from("y")],
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        let content = crate::new_map()
            .insert(Value::Keyword(Keyword::new(None, "y")), Value::Int(2))
            .insert(Value::Keyword(Keyword::new(None, "x")), Value::Int(1));
        let rec = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def: rdef,
            content,
            meta: None,
        })));
        assert_eq!(dup(&rec), "#user.RP[1, 2]");
    }

    #[test]
    fn dup_record_appends_extmap_in_content_order() {
        // `(vals r)`: fields in definition order, then extmap
        // entries in content-iteration order (oracle
        // `d-record-ext`, `d-record-ext2`).
        let rdef = Arc::new(crate::HostDef {
            name: Arc::from("user.RP"),
            kind: crate::HostKind::Record,
            fields: vec![Arc::from("x"), Arc::from("y")],
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        let one = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def: rdef.clone(),
            content: crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "x")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "y")), Value::Int(2))
                .insert(Value::Keyword(Keyword::new(None, "z")), Value::Int(9)),
            meta: None,
        })));
        assert_eq!(dup(&one), "#user.RP[1, 2, 9]");
        let two = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def: rdef,
            content: crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "x")), Value::Int(1))
                .insert(Value::Keyword(Keyword::new(None, "y")), Value::Int(2))
                .insert(Value::Keyword(Keyword::new(None, "z")), Value::Int(8))
                .insert(Value::Keyword(Keyword::new(None, "w")), Value::Int(9)),
            meta: None,
        })));
        assert_eq!(dup(&two), "#user.RP[1, 2, 8, 9]");
    }

    #[test]
    fn dup_class_writes_name_shape() {
        // The pure dup path (production print-dup renders through
        // the multis instead): classes dup as `#=` plus the name
        // (JVM-verified: `(binding [*print-dup* true] (prn String))`
        // prints `#=java.lang.String`).
        let class = || {
            Value::Class(Arc::new(crate::ClassVal::new(
                Arc::from("java.lang.String"),
                false,
                Arc::from([]),
            )))
        };
        assert_eq!(dup(&class()), "#=java.lang.String");
    }

    #[test]
    fn dup_throw_kinds_render_plain_fallback() {
        // Undupable kinds ignore `dup` entirely (the driver throws
        // before ever asking — these pins document the fallback).
        let plain = PrintOpts::pr();
        let dup_opts = PrintOpts {
            dup: true,
            length: Some(0),
            ..PrintOpts::pr()
        };
        let atom = Value::Atom(crate::Atom::new(Value::Int(1)));
        assert_eq!(pr_to_string(&atom, &dup_opts), pr_to_string(&atom, &plain));
        assert!(pr_to_string(&atom, &dup_opts).starts_with("#object[clojure.lang.Atom 0x"));
        let d = Value::Delay(crate::Delay::new(Value::Int(1)));
        assert_eq!(pr_to_string(&d, &dup_opts), pr_to_string(&d, &plain));
        let agent = Value::Agent(crate::Agent::new(Value::Int(1)));
        assert_eq!(
            pr_to_string(&agent, &dup_opts),
            pr_to_string(&agent, &plain)
        );
        // Non-record instances (deftype shapes) fall back too.
        let def = Arc::new(crate::HostDef {
            name: Arc::from("user.DT"),
            kind: crate::HostKind::Type,
            fields: vec![Arc::from("x")],
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        let inst = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def,
            content: crate::new_map(),
            meta: None,
        })));
        assert_eq!(pr_to_string(&inst, &dup_opts), pr_to_string(&inst, &plain));
        // A dupable collection holding an undupable child: the child
        // renders the fallback inline (length still ignored).
        let v = Value::Vector(crate::Vector::from_vec(vec![
            Value::Int(1),
            atom.clone(),
            Value::Int(2),
        ]));
        let text = pr_to_string(&v, &dup_opts);
        assert!(
            text.starts_with("[1 #object[clojure.lang.Atom 0x"),
            "{text}"
        );
        assert!(text.ends_with(" 2]"), "{text}");
        // Agents take the fallback inline too (the behind-lock meta
        // path all cells share).
        let va = Value::Vector(crate::Vector::from_vec(vec![
            Value::Int(1),
            agent.clone(),
            Value::Int(2),
        ]));
        let text = pr_to_string(&va, &dup_opts);
        assert!(
            text.starts_with("[1 #object[clojure.lang.Agent 0x"),
            "{text}"
        );
        assert!(text.ends_with(" 2]"), "{text}");
    }

    #[test]
    fn dup_shape_extension_hook() {
        // The `dup_shape` trait hook: `Some` wins under dup, `None`
        // keeps the plain `print` fallback.
        use crate::ExtensionValue;
        #[derive(Debug)]
        struct CtorExt;
        impl ExtensionValue for CtorExt {
            fn class_name(&self) -> String {
                "demo.Ctor".to_owned()
            }
            fn print(&self) -> String {
                "#object[demo.Ctor]".to_owned()
            }
            fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
                other.as_any().downcast_ref::<CtorExt>().is_some()
            }
            fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
                state.write_u64(0xC70);
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn dup_shape(&self) -> Option<String> {
                Some("#=(demo.Ctor. )".to_owned())
            }
        }
        let v = Value::Extension(Arc::new(CtorExt));
        assert_eq!(dup(&v), "#=(demo.Ctor. )");
        assert_eq!(pr(&v), "#object[demo.Ctor]");
        // The remaining trait methods (the hook only needs
        // `as_any`/`dup_shape`; these pin the double directly).
        assert_eq!(CtorExt.class_name(), "demo.Ctor");
        assert!(CtorExt.dyn_eq(&CtorExt));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        CtorExt.dyn_hash(&mut h);
        let mut expect = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hasher::write_u64(&mut expect, 0xC70);
        assert_eq!(
            std::hash::Hasher::finish(&h),
            std::hash::Hasher::finish(&expect)
        );
    }

    #[test]
    fn dup_shape_non_record_instance_defers_to_method() {
        // `print_host_dup_shape` answers `false` for non-record
        // instances (defensive: `is_dup_throw_kind` routes them to
        // the plain fallback before `print_dup_shape` ever runs —
        // pinned by `dup_throw_kinds_render_plain_fallback` — so
        // only a direct call reaches this arm).
        let def = Arc::new(crate::HostDef {
            name: Arc::from("user.DT"),
            kind: crate::HostKind::Type,
            fields: vec![Arc::from("x")],
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        let inst = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def,
            content: crate::new_map(),
            meta: None,
        })));
        let mut out = String::new();
        assert!(!super::print_dup_shape(
            &inst,
            &PrintOpts::pr(),
            None,
            &mut out
        ));
        assert!(out.is_empty());
    }

    #[test]
    fn dup_record_skips_missing_fields() {
        // A record missing a declared field skips it (defensive:
        // the ctors always fill every field and `dissoc` of a field
        // loses recordness, so surface values never trigger this).
        let rdef = Arc::new(crate::HostDef {
            name: Arc::from("user.RP"),
            kind: crate::HostKind::Record,
            fields: vec![Arc::from("x"), Arc::from("y")],
            struct_keys: Vec::new(),
            methods: std::collections::HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: std::collections::HashMap::new(),
            takes_this: true,
        });
        let rec = Value::HostType(Arc::new(crate::HostVal::Inst(crate::HostInst {
            def: rdef,
            content: crate::new_map()
                .insert(Value::Keyword(Keyword::new(None, "x")), Value::Int(1)),
            meta: None,
        })));
        assert_eq!(dup(&rec), "#user.RP[1]");
    }
}
