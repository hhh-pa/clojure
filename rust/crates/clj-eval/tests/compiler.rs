// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S18 (issue #62) compiler-surface rows: the ten matrix
//! rows' covering tests (spec §1.10), the fake-driver LOCAL suite
//! (gating, paths, the T1–T12 taxonomy mapping, atomicity,
//! gen-class survival), and the loom mirror of the driver registry.
//!
//! Port adaptations (spec-directed, never silent): artifacts are one
//! root-derived `.o` (never JVM `.class`); plan-first atomicity (no
//! partial marks); `*compile-path*` root `"classes"` (folded
//! launcher binding); `add-classpath` appends strings to
//! `*load-path*`; `primitives-classnames` keys are class-name
//! strings; cast texts are the port's stable `Cannot cast` shape.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use clj_eval::compiler::{CompileDriver, DriveError, install_driver};
use clj_eval::{Evaluator, SourceError, SubsetProgram, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    pr(&realized(eval, v))
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

/// A per-test fixture dir (unique across parallel tests, removed on
/// drop) holding `*load-path*` sources and `*compile-path*` outputs.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s18-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// The dir as a Clojure string literal.
    fn lit(&self) -> String {
        lit(&self.dir)
    }

    /// The `classes` output dir as a Clojure string literal (created).
    fn classes_lit(&self) -> String {
        let dir = self.dir.join("classes");
        std::fs::create_dir_all(&dir).unwrap();
        lit(&dir)
    }

    /// The `classes` output dir (created).
    fn classes_dir(&self) -> PathBuf {
        let dir = self.dir.join("classes");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Wrap `src` in `*load-path*` + `*compile-path*` bindings to
    /// this dir (qualified: the current namespace is often a fresh
    /// fixture).
    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}] clojure.core/*compile-path* {}] {src})",
            self.lit(),
            self.classes_lit()
        )
    }
}

/// A path as a Clojure string literal.
fn lit(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The full uncaught chain, outermost first.
fn uncaught_chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let Err(SourceError::Uncaught(t)) = eval.eval_source(src) else {
        panic!("expected an uncaught throw for {src}");
    };
    let mut out = Vec::new();
    let mut cur = Some(&t);
    while let Some(th) = cur {
        out.push(format!(
            "{} | {}",
            th.class.jvm_name(),
            th.message.as_deref().unwrap_or("")
        ));
        cur = th.cause.as_deref();
    }
    out
}

/// Serializes every test that installs the driver or emits: the
/// registry is process-global, so two such tests must never overlap.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// What the fake driver does on emit.
#[derive(Clone, Copy, Debug)]
enum FakeMode {
    Record,
    FailTarget,
    FailEmit,
    FailIo,
}

/// One recorded emission attempt.
#[derive(Clone, Debug)]
struct FakeCall {
    source: String,
    obj: PathBuf,
    fns: usize,
}

/// The fake driver state (reset per test under [`TEST_LOCK`]).
struct FakeState {
    mode: FakeMode,
    calls: Vec<FakeCall>,
}

/// The fake AOT driver: records emissions or fails each
/// [`DriveError`] variant on demand. Never touches the disk.
struct FakeDriver {
    state: Mutex<FakeState>,
}

static FAKE: FakeDriver = FakeDriver {
    state: Mutex::new(FakeState {
        mode: FakeMode::Record,
        calls: Vec::new(),
    }),
};

impl CompileDriver for FakeDriver {
    fn emit(
        &self,
        program: &SubsetProgram,
        source_path: &str,
        obj_path: &Path,
    ) -> Result<(), DriveError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match state.mode {
            FakeMode::Record => {
                state.calls.push(FakeCall {
                    source: source_path.to_owned(),
                    obj: obj_path.to_path_buf(),
                    fns: program.fns.len(),
                });
                Ok(())
            }
            FakeMode::FailTarget => Err(DriveError::Target("boom-target".to_owned())),
            FakeMode::FailEmit => Err(DriveError::Emit("boom-emit".to_owned())),
            FakeMode::FailIo => Err(DriveError::Io {
                path: obj_path.to_path_buf(),
                message: "boom-io".to_owned(),
            }),
        }
    }
}

impl FakeDriver {
    fn reset(&self, mode: FakeMode) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.mode = mode;
        state.calls.clear();
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .clone()
    }
}

/// Hold [`TEST_LOCK`], reset the fake to `mode`, and install it.
/// The guard must live across the whole test body.
fn use_fake(mode: FakeMode) -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    FAKE.reset(mode);
    install_driver(Some(&FAKE));
    guard
}

/// The recorded emission attempts.
fn fake_calls() -> Vec<FakeCall> {
    FAKE.calls()
}

/// `*compile-path*` root (folded `"classes"`), slot-only dynamics
/// (no `:dynamic` meta), and rebinding (`k-compile-path`,
/// `k-root-compile-path`, `k-dyn-compile-path`, `k-bind-compile-path`).
#[test]
fn root_classes_and_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compile-path*"), "\"classes\"");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compile-path*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*compile-path* \"/tmp/x\"] *compile-path*)"
        ),
        "\"/tmp/x\""
    );
}

/// `*compile-files*` root false, slot-only (`k-compile-files`,
/// `k-root-compile-files`, `k-dyn-compile-files`).
#[test]
fn root_false_and_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compile-files*"), "false");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compile-files*))"),
        "nil"
    );
}

/// `*compiler-options*` and `*fn-loader*` roots nil, slot-only,
/// rebindable (`k-compiler-options`, `k-root-compiler-options`,
/// `k-dyn-compiler-options`, `k-bind-options`, `k-root-fn-loader`,
/// `k-dyn-fn-loader` — one test: §1.10 names both rows
/// `root_nil_slot_only_and_binds`).
#[test]
fn root_nil_slot_only_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*compiler-options*"), "nil");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*compiler-options*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*compiler-options* {:a 1}] *compiler-options*)"
        ),
        "{:a 1}"
    );
    assert_eq!(run_last(&mut eval, "*fn-loader*"), "nil");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*fn-loader*))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*fn-loader* 7] *fn-loader*)"),
        "7"
    );
}

/// `*source-path*` root `"NO_SOURCE_FILE"`, slot-only
/// (`k-root-source-path`, `k-dyn-source-path`).
#[test]
fn root_no_source_file_slot_only() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*source-path*"), "\"NO_SOURCE_FILE\"");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*source-path*))"),
        "nil"
    );
}

/// `*use-context-classloader*` root true, slot-only, rebindable
/// (`k-use-context`, `k-root-use-context`, `k-dyn-use-context`,
/// `k-bind-use-context`).
#[test]
fn root_true_slot_only_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*use-context-classloader*"), "true");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*use-context-classloader*))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*use-context-classloader* false] *use-context-classloader*)"
        ),
        "false"
    );
}

/// `print-dup` records honor the knob: positional by default, the
/// `#Name{map}` body when verbose (`k-dup-false`, `k-verbose-true`).
#[test]
fn dup_record_both_shapes() {
    let mut eval = boot();
    run_last(&mut eval, "(defrecord Rec [x])");
    assert_eq!(
        run_last(&mut eval, "(binding [*print-dup* true] (pr-str (->Rec 1)))"),
        "\"#user.Rec[1]\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-dup* true *verbose-defrecords* true] (pr-str (->Rec 1)))"
        ),
        "\"#user.Rec{:x 1}\""
    );
}

/// `*verbose-defrecords*` root false, `:dynamic` true, rebindable
/// (`k-verbose`, `k-root-verbose`, `k-dyn-verbose`).
#[test]
fn root_false_dynamic_and_binds() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*verbose-defrecords*"), "false");
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*verbose-defrecords*))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*verbose-defrecords* true] *verbose-defrecords*)"
        ),
        "true"
    );
}

/// `primitives-classnames` count, values, string keys, lookup, and
/// plain-def meta (`k-prims-count/vals/keys/lookup`, `k-dyn-prims`).
#[test]
fn shape_count_vals_keys() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(count primitives-classnames)"), "8");
    assert_eq!(
        run_last(&mut eval, "(sort (vals primitives-classnames))"),
        "(\"Boolean/TYPE\" \"Byte/TYPE\" \"Character/TYPE\" \"Double/TYPE\" \"Float/TYPE\" \"Integer/TYPE\" \"Long/TYPE\" \"Short/TYPE\")"
    );
    assert_eq!(
        run_last(&mut eval, "(sort (keys primitives-classnames))"),
        "(\"boolean\" \"byte\" \"char\" \"double\" \"float\" \"int\" \"long\" \"short\")"
    );
    assert_eq!(
        run_last(&mut eval, "(primitives-classnames \"long\")"),
        "\"Long/TYPE\""
    );
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'primitives-classnames))"),
        "nil"
    );
}

/// Unknown classes miss to nil (`k-prims-miss`, derived shape).
#[test]
fn miss_is_nil() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(primitives-classnames \"java.lang.String\")"),
        "nil"
    );
}

/// The add answers nil and prints the verbatim WARNING (`a-warning`,
/// `a-warn-text`, `a-ret`).
#[test]
fn warns_and_returns_nil() {
    let fx = Fixture::new("addcp-warn");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", fx.lit())),
        "nil"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(with-out-str (add-classpath {}))", fx.lit())
        ),
        "\"WARNING: add-classpath is deprecated\\n\""
    );
}

/// Appends in order; the first-added dir wins resolution; later dirs
/// resolve new libs (`a-urls`, `a-order`, `a-effect`, `a-ret`,
/// `a-add2`).
#[test]
fn appends_first_wins() {
    let d1 = Fixture::new("addcp-d1");
    let d2 = Fixture::new("addcp-d2");
    d1.write("my/lib.clj", "(ns my.lib)\n(def marker \"d1\")\n");
    d2.write("my/lib.clj", "(ns my.lib)\n(def marker \"d2\")\n");
    d2.write("new/lib.clj", "(ns new.lib)\n(def marker \"new\")\n");
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d1.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d2.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{} {}]", d1.lit(), d2.lit())
    );
    assert_eq!(
        run_last(&mut eval, "(require 'my.lib) @#'my.lib/marker"),
        "\"d1\""
    );
    assert_eq!(
        run_last(&mut eval, "(require 'new.lib) @#'new.lib/marker"),
        "\"new\""
    );
}

/// Re-adding is a no-op returning nil (`a-dup-ret`, `a-dup-urls`).
#[test]
fn dup_is_noop() {
    let d1 = Fixture::new("addcp-dup1");
    let d2 = Fixture::new("addcp-dup2");
    let mut eval = boot();
    run_last(&mut eval, &format!("(add-classpath {})", d1.lit()));
    run_last(&mut eval, &format!("(add-classpath {})", d2.lit()));
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", d1.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{} {}]", d1.lit(), d2.lit())
    );
}

/// Nonexistent dirs are accepted silently (`a-missing`).
#[test]
fn missing_accepted() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(add-classpath \"/nonexistent-s18-dir\")"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        "[\"/nonexistent-s18-dir\"]"
    );
}

/// Non-strings throw CCE `String` after the WARNING
/// (`a-broken-file/num`, port-narrowed).
#[test]
fn nonstr_rejected() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (try (add-classpath 42) (catch Exception _ :caught)))"
        ),
        "\"WARNING: add-classpath is deprecated\\n\""
    );
    assert_eq!(
        uncaught(&mut eval, "(add-classpath 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

/// Arity ≠ 1 throws with no WARNING (`a-arity0/2`; the row-1
/// `compile` sections join this test — §1.10 names both `arity`).
#[test]
fn arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(add-classpath)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/add-classpath"
    );
    assert_eq!(
        uncaught(&mut eval, "(add-classpath \"a\" \"b\")"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/add-classpath"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (try (add-classpath) (catch Exception _ :caught)))"
        ),
        "\"\""
    );
    assert_eq!(
        uncaught(&mut eval, "(compile)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/compile"
    );
    assert_eq!(
        uncaught(&mut eval, "(compile 'a 'b)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/compile"
    );
}

/// A nil `*load-path*` root normalizes to `[]` first (append holds).
#[test]
fn nil_path_normalizes() {
    let fx = Fixture::new("addcp-nil");
    let mut eval = boot();
    run_last(&mut eval, "(alter-var-root #'*load-path* (fn [_] nil))");
    assert_eq!(
        run_last(&mut eval, &format!("(add-classpath {})", fx.lit())),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "*load-path*"),
        format!("[{}]", fx.lit())
    );
}

/// Non-vector coll roots get `conj` semantics (lists prepend).
#[test]
fn list_root_conj_semantics() {
    let mut eval = boot();
    run_last(
        &mut eval,
        "(alter-var-root #'*load-path* (fn [_] (quote (\"a\"))))",
    );
    assert_eq!(run_last(&mut eval, "(add-classpath \"b\")"), "nil");
    assert_eq!(run_last(&mut eval, "*load-path*"), "(\"b\" \"a\")");
}

/// `compile` answers the lib symbol (a symbol — the port has no
/// `class` surface, so `symbol?` is the `c-ret-class` pin), plans
/// one fn, and emits one object under the bound dir (`c-ret`,
/// `c-ret-class`, `c-files`, `c-ns-loaded`, `c-loaded-libs`).
#[test]
fn returns_lib_symbol() {
    let fx = Fixture::new("ret");
    fx.write("my/ret.clj", "(ns my.ret)\n(defn answer [] 41)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.ret)")),
        "my.ret"
    );
    assert_eq!(run_last(&mut eval, "(symbol? 'my.ret)"), "true");
    assert!(run_last(&mut eval, "(find-ns 'my.ret)").contains("my.ret"));
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.ret}");
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].fns, 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/ret.o"));
    assert!(calls[0].source.ends_with("my/ret.clj"));
}

/// During a compile `*compile-files*` is true (emission happens)
/// and `*compile-path*` is the bound value (the object lands under
/// it): the subset cannot name vars, so these pins are indirect but
/// load-bearing (`c-captured`, `c-files`).
#[test]
fn binds_files_true() {
    let fx = Fixture::new("binds");
    fx.write("my/ctx.clj", "(ns my.ctx)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.ctx)")),
        "my.ctx"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/ctx.o"));
    assert_eq!(calls[0].fns, 1);
}

/// `compile` always reloads: no loaded-libs shortcut (`c-twice` —
/// the print runs on both compiles).
#[test]
fn always_reloads() {
    let fx = Fixture::new("twice");
    fx.write("my/twice.clj", "(ns my.twice)\n(print 7)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(with-out-str (compile 'my.twice) (compile 'my.twice))")
        ),
        "\"77\""
    );
    assert_eq!(fake_calls().len(), 2);
}

/// First `*load-path*` hit wins under compile, like `require`.
#[test]
fn uses_load_path_order() {
    let d1 = Fixture::new("ord-d1");
    let d2 = Fixture::new("ord-d2");
    d1.write("my/lib.clj", "(ns my.lib)\n(print 1)\n");
    d2.write("my/lib.clj", "(ns my.lib)\n(print 2)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let src = format!(
        "(binding [clojure.core/*load-path* [{} {}] clojure.core/*compile-path* {}] (with-out-str (compile 'my.lib)))",
        d1.lit(),
        d2.lit(),
        d1.classes_lit()
    );
    assert_eq!(run_last(&mut eval, &src), "\"1\"");
    assert_eq!(fake_calls().len(), 1);
}

/// Flag-false loads skip plan+emit exactly: evaluation runs with
/// zero driver calls (`c-false-files`, `c-false-eval` — arbitrary
/// fixtures plan nothing when the flag is false).
#[test]
fn leaves_flag_false_path_untouched() {
    let fx = Fixture::new("untouched");
    fx.write("my/plain.clj", "(ns my.plain)\n(def marker \"plain\")\n");
    fx.write("my/plain2.clj", "(ns my.plain2)\n(def marker \"plain2\")\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compile-files* false] (load \"/my/plain\"))")
        ),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "@#'my.plain/marker"), "\"plain\"");
    assert!(fake_calls().is_empty());
    assert_eq!(
        run_last(&mut eval, &fx.within("(require 'my.plain2)")),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "@#'my.plain2/marker"), "\"plain2\"");
    assert!(fake_calls().is_empty());
}

/// Unresolvable libs throw the verbatim FNF before anything else
/// (T4, `c-missing`).
#[test]
fn missing() {
    let fx = Fixture::new("missing");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'no.such.lib)")),
        "java.io.FileNotFoundException | Could not locate no/such/lib__init.class, no/such/lib.clj or no/such/lib.cljc on classpath."
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
}

/// String libs load, then fail the check on the original arg (T5,
/// `c-str` — emission runs before the check).
#[test]
fn str_arg() {
    let fx = Fixture::new("strarg");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile \"my/plain\")")),
        "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Symbol"
    );
    assert!(run_last(&mut eval, "(find-ns 'my.plain)").contains("my.plain"));
    assert_eq!(fake_calls().len(), 1);
}

/// Non-named libs fail `name` before any load (T2, `c-num`).
#[test]
fn num_arg() {
    let fx = Fixture::new("numarg");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 42)")),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Named"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
}

/// Nil libs fail `name` before any load (T3, `c-nil-lib` — the
/// port's stable name-NPE matches the JDK text here).
#[test]
fn nil_arg() {
    let fx = Fixture::new("nilarg");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile nil)")),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
}

/// Unqualified keywords load, then fail the check (T5, `c-keyword`).
#[test]
fn keyword_arg() {
    let fx = Fixture::new("kwarg");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile :my.plain)")),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to clojure.lang.Symbol"
    );
    assert!(run_last(&mut eval, "(find-ns 'my.plain)").contains("my.plain"));
    assert_eq!(fake_calls().len(), 1);
}

/// `/`-qualified keywords resolve by `name` only (T4,
/// `c-keyword-ns` — the FNF names the bare root).
#[test]
fn keyword_ns_arg() {
    let fx = Fixture::new("kwnsarg");
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile :my/plain)")),
        "java.io.FileNotFoundException | Could not locate plain__init.class, plain.clj or plain.cljc on classpath."
    );
}

/// Plan failures reuse the load's exact shape (T6a, `c-broken`):
/// the compile chain equals the load chain, and D3 atomicity leaves
/// the lib unmarked with no calls. Both the resolve and reader
/// shapes are pinned.
#[test]
fn broken_plan_diagnostic() {
    let fx = Fixture::new("broken");
    fx.write(
        "my/broken.clj",
        "(ns my.broken)\n(defn f [x] (no-such-fn-xyz x))\n",
    );
    fx.write("my/rd.clj", "(ns my.rd)\n(defn broken [)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.broken)"));
    assert_eq!(chain.len(), 2);
    assert_eq!(
        chain[1],
        "java.lang.RuntimeException | Unable to resolve symbol: no-such-fn-xyz in this context"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    let rd_chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.rd)"));
    assert_eq!(rd_chain.len(), 2);
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert!(fake_calls().is_empty());
    let mut eval = boot();
    assert_eq!(
        uncaught_chain(&mut eval, &fx.within("(load \"/my/broken\")")),
        chain
    );
    // The load self-marks (`ns` evaluates before the later form
    // throws); the compile marks nothing (D3).
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.broken}");
    assert_eq!(
        uncaught_chain(&mut eval, &fx.within("(load \"/my/rd\")")),
        rd_chain
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.broken}");
}

/// Eval failures reuse the load's exact shape with no artifact
/// (T12): the `ns` self-mark persists (the port's `eval_ns_form`
/// marks, like the JVM — `c-broken-unmarked` transfers for eval
/// failures), the requested mark does not happen.
#[test]
fn broken_marks_and_no_artifact() {
    let fx = Fixture::new("bmarks");
    fx.write("my/b12.clj", "(ns my.b12)\n(defn f [] (quot 1 0))\n(f)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.b12)"));
    assert_eq!(chain, ["java.lang.ArithmeticException | / by zero"]);
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.b12}");
    assert!(fake_calls().is_empty());
    assert_eq!(
        uncaught_chain(&mut eval, &fx.within("(load \"/my/b12\")")),
        chain
    );
}

/// Mismatched namespaces fail the check after emission (T5,
/// `c-mismatch*`): the requested lib is unmarked, the evaluated ns
/// self-marks, and the root-derived object emits.
#[test]
fn mismatch_check_and_unmarked() {
    let fx = Fixture::new("mismatch");
    fx.write("my/mismatch.clj", "(ns other.thing)\n(defn m [] 1)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.mismatch)")),
        "java.lang.Exception | namespace 'my.mismatch' not found after loading '/my/mismatch'"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (loaded-libs) 'my.mismatch)"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (loaded-libs) 'other.thing)"),
        "true"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/mismatch.o"));
}

/// Nil `*compile-path*` throws verbatim (T7, `c-nil-path`).
#[test]
fn nil_path() {
    let fx = Fixture::new("nilpath");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within("(binding [*compile-path* nil] (compile 'my.plain))")
        ),
        "java.lang.RuntimeException | *compile-path* not set"
    );
    assert!(fake_calls().is_empty());
}

/// Non-string `*compile-path*` throws the `String` cast (T8,
/// `c-bad-path`).
#[test]
fn bad_path() {
    let fx = Fixture::new("badpath");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within("(binding [*compile-path* 42] (compile 'my.plain))")
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
    assert!(fake_calls().is_empty());
}

/// Path validation runs before planning: a broken lib under a bad
/// path reports T7/T8, never T6.
#[test]
fn nil_path_wins_over_broken_plan() {
    let fx = Fixture::new("pathwins");
    fx.write("my/broken.clj", "(ns my.broken)\n(defrecord R [x])\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within("(binding [*compile-path* nil] (compile 'my.broken))")
        ),
        "java.lang.RuntimeException | *compile-path* not set"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &fx.within("(binding [*compile-path* 42] (compile 'my.broken))")
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

/// The strip is offset-exact: later plan diagnostics blame their
/// original lines (leading trivia included).
#[test]
fn strip_keeps_spans() {
    let fx = Fixture::new("spans");
    fx.write(
        "my/sp.clj",
        "; leading comment\n(ns my.sp\n  (:refer-clojure :exclude [x]))\n(defn ok [] 1)\n(defrecord R [x])\n",
    );
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.sp)"));
    assert_eq!(chain.len(), 1);
    assert!(
        chain[0].contains(":5:1: unsupported form in AOT subset: defrecord"),
        "unexpected chain: {chain:?}"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert!(fake_calls().is_empty());
}

/// Plan-first atomicity (D3, `c-broken-files` port side): a plan
/// failure evaluates nothing (no print), emits nothing, marks
/// nothing.
#[test]
fn plan_first_atomic_fake() {
    let fx = Fixture::new("atomic");
    fx.write("my/at.clj", "(ns my.at)\n(print 7)\n(defrecord R [x])\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(with-out-str (try (compile 'my.at) (catch Exception _ :threw)))")
        ),
        "\"\""
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert!(fake_calls().is_empty());
}

/// Transitive AOT: every root loaded during the compile emits its
/// own object (dependencies first).
#[test]
fn transitive_emits_both_fake() {
    let fx = Fixture::new("trans");
    fx.write("my/dep.clj", "(ns my.dep)\n(defn f [] 42)\n");
    fx.write(
        "my/top.clj",
        "(ns my.top (:require [my.dep :as d]))\n(defn g [] 41)\n",
    );
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.top)")),
        "my.top"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/dep.o"));
    assert_eq!(calls[1].obj, fx.classes_dir().join("my/top.o"));
    assert!(run_last(&mut eval, "(find-ns 'my.dep)").contains("my.dep"));
}

/// A non-subset transitive dep fails the whole compile with its own
/// plan diagnostic (subset-transitivity limitation).
#[test]
fn transitive_subset_failure() {
    let fx = Fixture::new("transfail");
    fx.write("my/dep.clj", "(ns my.dep)\n(defrecord R [x])\n");
    fx.write(
        "my/top.clj",
        "(ns my.top (:require [my.dep :as d]))\n(defn g [] 41)\n",
    );
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.top)"));
    assert_eq!(chain.len(), 1);
    assert!(
        chain[0].contains("my/dep.clj")
            && chain[0].contains("unsupported form in AOT subset: defrecord"),
        "unexpected chain: {chain:?}"
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert!(fake_calls().is_empty());
}

/// The unset hook reports the CLI's exact unlinked line (T11).
#[test]
fn driver_unset() {
    let fx = Fixture::new("unset");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_driver(None);
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.plain)")),
        "java.io.IOException | build: LLVM backend not linked (built with --no-default-features; rebuild with default features to compile)"
    );
    drop(_lock);
}

/// Driver failures map to `build:`-prefixed `IOException`s (T9/T10)
/// with no mark: target, emit, io, plus the emit-failure mark pin
/// (mismatch lib, failing driver: requested unmarked, ns self-marked,
/// and — unlike the check failure — no artifact attempted).
#[test]
fn driver_target_emit_io_fake() {
    let fx = Fixture::new("driverfail");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    fx.write("my/mismatch.clj", "(ns other.thing)\n(defn m [] 1)\n");
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut eval = boot();
    FAKE.reset(FakeMode::FailTarget);
    install_driver(Some(&FAKE));
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.plain)")),
        "java.io.IOException | build: boom-target"
    );
    FAKE.reset(FakeMode::FailEmit);
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.plain)")),
        "java.io.IOException | build: boom-emit"
    );
    FAKE.reset(FakeMode::FailIo);
    let err = uncaught(&mut eval, &fx.within("(compile 'my.plain)"));
    let expect = format!(
        "java.io.IOException | build: {}: boom-io",
        fx.classes_dir().join("my/plain.o").display()
    );
    assert_eq!(err, expect);
    FAKE.reset(FakeMode::FailEmit);
    assert_eq!(
        uncaught(&mut eval, &fx.within("(compile 'my.mismatch)")),
        "java.io.IOException | build: boom-emit"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (loaded-libs) 'my.mismatch)"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (loaded-libs) 'other.thing)"),
        "true"
    );
}

/// `gen-class` descriptors survive a compile round-trip: resolve,
/// construct, and forward after a compile runs.
#[test]
fn genclass_survives_fake() {
    let fx = Fixture::new("gcsurv");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    run_last(
        &mut eval,
        "(gen-class :name user.Klass :prefix \"klass-\" :methods [[hello [] String]]) (defn klass-hello [this] \"yo\")",
    );
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_eq!(
        run_last(&mut eval, "[(.hello (Klass.)) (str Klass)]"),
        "[\"yo\" \"user.Klass\"]"
    );
    assert_eq!(fake_calls().len(), 1);
}

/// Bare plan failures escape bare and load-identical (T6b).
#[test]
fn bare_macro_arity_plans_bare() {
    let fx = Fixture::new("bare");
    fx.write("my/ba.clj", "(ns my.ba)\n(when)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.ba)"));
    assert_eq!(
        chain,
        ["clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/when"]
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    assert_eq!(
        uncaught_chain(&mut eval, &fx.within("(load \"/my/ba\")")),
        chain
    );
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.ba}");
    assert!(fake_calls().is_empty());
}

/// `compile` restores `*compile-files*` after throws and successes.
#[test]
fn compile_restores_flag_after_throw() {
    let fx = Fixture::new("restore");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(try (compile 'no.such.lib) (catch Exception _ :caught)) *compile-files*")
        ),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain) *compile-files*")),
        "false"
    );
}

/// Only the first form, only unqualified `ns`: qualified heads,
/// `in-ns`, later `ns` forms, and non-list first forms all plan
/// their text as-is (subset diagnostics, never silent).
#[test]
fn strip_only_first_unqualified_ns() {
    let fx = Fixture::new("stripnarrow");
    fx.write("my/q.clj", "(clojure.core/ns my.q)\n(defn f [] 1)\n");
    fx.write("my/i.clj", "(in-ns 'my.i)\n(defn f [] 1)\n");
    fx.write("my/later.clj", "(defn f [] 1)\n(ns other)\n");
    fx.write("my/nl.clj", "42\n(ns my.nl)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    for (lib, head) in [
        ("my.q", "clojure.core/ns"),
        ("my.i", "in-ns"),
        ("my.later", "ns"),
        ("my.nl", "ns"),
    ] {
        let chain = uncaught_chain(&mut eval, &fx.within(&format!("(compile '{lib})")));
        assert_eq!(chain.len(), 1, "for {lib}: {chain:?}");
        assert!(
            chain[0].contains("unsupported form in AOT subset") && chain[0].contains(head),
            "for {lib}: {chain:?}"
        );
        assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{}");
    }
    assert!(fake_calls().is_empty());
}

/// The strip mirrors the reader's CRLF normalization: spans stay
/// exact under `\r\n` line endings.
#[test]
fn strip_handles_crlf() {
    let fx = Fixture::new("crlf");
    fx.write(
        "my/cr.clj",
        "(ns my.cr)\r\n(defn f [] 1)\r\n(defrecord R [x])\r\n",
    );
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.cr)"));
    assert_eq!(chain.len(), 1);
    assert!(
        chain[0].contains(":3:1: unsupported form in AOT subset: defrecord"),
        "unexpected chain: {chain:?}"
    );
    assert!(fake_calls().is_empty());
}

/// An `ns`-only file plans the empty program and compiles clean
/// (the local half of R5; the real driver's empty acceptance is
/// `empty_ns_only`).
#[test]
fn ns_only_emits_empty_fake() {
    let fx = Fixture::new("nsonly");
    fx.write("my/e.clj", "(ns my.e)");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(compile 'my.e)")), "my.e");
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.e}");
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].fns, 0);
}

/// The bound `*compile-path*` drives emission (`c-bind-compile-path`).
#[test]
fn bound_path_drives_emit_fake() {
    let fx = Fixture::new("boundpath");
    fx.write("my/p.clj", "(ns my.p)\n(defn f [] 1)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(compile 'my.p)")), "my.p");
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/p.o"));
}

/// Relative roots join without resolving; absolute dirs join
/// exactly (no writes — the fake never touches the disk).
#[test]
fn relative_and_absolute() {
    let fx = Fixture::new("relabs");
    fx.write("my/rp.clj", "(ns my.rp)\n(defn f [] 1)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &format!(
                "(binding [clojure.core/*load-path* [{}]] (compile 'my.rp))",
                fx.lit()
            )
        ),
        "my.rp"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].obj,
        std::path::Path::new("classes").join("my/rp.o")
    );
    FAKE.reset(FakeMode::Record);
    assert_eq!(run_last(&mut eval, &fx.within("(compile 'my.rp)")), "my.rp");
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/rp.o"));
}

/// A false flag skips even the plan: arbitrary fixtures load with
/// zero calls (`c-false-files`, `c-true-files` negative).
#[test]
fn false_skips_plan_and_emit() {
    let fx = Fixture::new("falseskip");
    fx.write(
        "my/plain.clj",
        "(ns my.plain)\n(def marker \"plain\")\n(defrecord R [x])\n",
    );
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compile-files* false] (load \"/my/plain\"))")
        ),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "@#'my.plain/marker"), "\"plain\"");
    assert!(fake_calls().is_empty());
}

/// A true flag on `load` emits exactly what `compile` emits for the
/// same root (`c-true-load`, `c-true-files`).
#[test]
fn true_load_emits_fake() {
    let fx = Fixture::new("trueload");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compile-files* true] (load \"/my/plain\"))")
        ),
        "nil"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/plain.o"));
    FAKE.reset(FakeMode::Record);
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    let calls = fake_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].obj, fx.classes_dir().join("my/plain.o"));
}

/// Any truthy flag emits (not just `true`, `c-files-truthy`).
#[test]
fn truthy_one_emits_fake() {
    let fx = Fixture::new("truthy");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compile-files* 1] (load \"/my/plain\"))")
        ),
        "nil"
    );
    assert_eq!(fake_calls().len(), 1);
}

/// `compile` binds `true` unconditionally: an outer `false` does not
/// skip emission.
#[test]
fn compile_forces_true_over_outer_false() {
    let fx = Fixture::new("forces");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compile-files* false] (compile 'my.plain))")
        ),
        "my.plain"
    );
    assert_eq!(fake_calls().len(), 1);
}

/// A bound options map is tolerated through a compile
/// (`c-options-bound`, `c-options-loaded`).
#[test]
fn bound_map_tolerated_fake() {
    let fx = Fixture::new("optbound");
    fx.write("my/op.clj", "(ns my.op)\n(defn f [] 1)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            &fx.within("(binding [*compiler-options* {:elide-meta [:x]}] (compile 'my.op))")
        ),
        "my.op"
    );
    assert_eq!(fake_calls().len(), 1);
    assert_eq!(run_last(&mut eval, "(loaded-libs)"), "#{my.op}");
}

/// Loads bind the basename (`c-load-captured`).
#[test]
fn load_binds_basename() {
    let fx = Fixture::new("sourcebase");
    fx.write("my/ctx.clj", "(ns my.ctx)\n(def captured *source-path*)\n");
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, &fx.within("(load \"/my/ctx\")")), "nil");
    assert_eq!(run_last(&mut eval, "@#'my.ctx/captured"), "\"ctx.clj\"");
}

/// `load-file` binds the basename of the absolute path while `*file*`
/// stays absolute (`c-loadfile-source`).
#[test]
fn loadfile_binds_basename() {
    let fx = Fixture::new("sourcefile");
    fx.write(
        "my/ctx.clj",
        "(ns my.ctx)\n(def captured [*source-path* *file*])\n",
    );
    let mut eval = boot();
    let abs = fx.dir.join("my/ctx.clj");
    run_last(&mut eval, &format!("(load-file {})", lit(&abs)));
    assert_eq!(
        run_last(&mut eval, "(first @#'my.ctx/captured)"),
        "\"ctx.clj\""
    );
    let file = run_last(&mut eval, "(second @#'my.ctx/captured)");
    assert!(file.ends_with("my/ctx.clj\""), "file was {file}");
    assert!(file.starts_with("\"/"), "file was {file}");
}

/// `load-string` binds the root with `*file*` nil
/// (`c-loadstring-source`).
#[test]
fn loadstring_binds_no_source_file() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(load-string \"(ns my.ls) (def lscap [*source-path* *file* *fn-loader*])\") @#'my.ls/lscap"
        ),
        "[\"NO_SOURCE_FILE\" nil nil]"
    );
}

/// `eval` never rebinds `*source-path*`: bindings inherit, and the
/// top level sees the root (`k-eval-source`).
#[test]
fn eval_does_not_rebind() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*source-path* \"X\"] (eval '*source-path*))"
        ),
        "\"X\""
    );
    assert_eq!(
        run_last(&mut eval, "(eval '*source-path*)"),
        "\"NO_SOURCE_FILE\""
    );
}

/// The compile path derives its file identity from the same
/// candidate the load binds `*source-path*` from: direct
/// observation during a compile is precluded (subset files cannot
/// name vars), so this pins the shared basename derivation across
/// both paths instead.
#[test]
fn compile_observes_basename() {
    let fx = Fixture::new("csource");
    fx.write("my/x.clj", "(ns my.x)\n(defrecord R [a])\n");
    fx.write("my/ctx.clj", "(ns my.ctx)\n(def captured *source-path*)\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    let chain = uncaught_chain(&mut eval, &fx.within("(compile 'my.x)"));
    assert_eq!(chain.len(), 1);
    let candidate = format!("{}/my/x.clj", fx.dir.to_string_lossy().replace('\\', "/"));
    assert!(chain[0].contains(&candidate), "unexpected chain: {chain:?}");
    assert!(candidate.ends_with("my/x.clj"));
    let mut eval = boot();
    run_last(&mut eval, &fx.within("(load \"/my/ctx\")"));
    assert_eq!(run_last(&mut eval, "@#'my.ctx/captured"), "\"ctx.clj\"");
}

/// `*fn-loader*` is nil during loads and compiles (data-only: never
/// bound, never consulted — `c-fn-loader-load`, `c-fn-loader-file`,
/// `c-fn-loader-string`).
#[test]
fn nil_during_load_and_compile() {
    let fx = Fixture::new("fnloader");
    fx.write("my/ctx.clj", "(ns my.ctx)\n(def captured *fn-loader*)\n");
    fx.write("my/plain.clj", "(ns my.plain)\n(defn f [x] (+ x 1))\n");
    let _lock = use_fake(FakeMode::Record);
    let mut eval = boot();
    run_last(&mut eval, &fx.within("(load \"/my/ctx\")"));
    assert_eq!(run_last(&mut eval, "@#'my.ctx/captured"), "nil");
    assert_eq!(
        run_last(&mut eval, &fx.within("(compile 'my.plain)")),
        "my.plain"
    );
    assert_eq!(run_last(&mut eval, "*fn-loader*"), "nil");
}

/// The context-classloader flag is data-only: every loader entry
/// succeeds identically under both values (`a-use-true/false`,
/// `c-use-true/false`).
#[test]
fn loads_succeed_under_both() {
    let fx = Fixture::new("usectx");
    fx.write("my/u.clj", "(ns my.u)\n(defn f [] 7)\n(print 8)\n");
    let _lock = use_fake(FakeMode::Record);
    for flag in ["true", "false"] {
        let mut eval = boot();
        assert_eq!(
            run_last(
                &mut eval,
                &fx.within(&format!(
                    "(binding [*use-context-classloader* {flag}] (with-out-str (load \"/my/u\")))"
                ))
            ),
            "\"8\""
        );
        let mut eval = boot();
        assert_eq!(
            run_last(
                &mut eval,
                &fx.within(&format!(
                    "(binding [*use-context-classloader* {flag}] (compile 'my.u))"
                ))
            ),
            "my.u"
        );
        let mut eval = boot();
        assert_eq!(
            run_last(
                &mut eval,
                &fx.within(&format!(
                    "(binding [*use-context-classloader* {flag}] (require 'my.u))"
                ))
            ),
            "nil"
        );
        assert_eq!(run_last(&mut eval, "((deref #'my.u/f))"), "7");
    }
}

/// Loom mirror of the driver registry (`compiler.rs` install/read):
/// a `RwLock<Option<u8>>` standing in for the
/// `RwLock<Option<&'static dyn CompileDriver>>`, with the same two
/// operations (install swaps under the write lock, reads copy out
/// under the read lock). Property: concurrent installs and reads
/// never tear — every read sees `None` or an installed value, and
/// the joined install wins.
#[test]
fn driver_registry_install_read_race() {
    use std::sync::Arc;

    use loom::sync::RwLock;
    use loom::thread;

    fn install(reg: &RwLock<Option<u8>>, driver: Option<u8>) {
        *reg.write().unwrap() = driver;
    }

    fn current(reg: &RwLock<Option<u8>>) -> Option<u8> {
        *reg.read().unwrap()
    }

    loom::model(|| {
        let reg = Arc::new(RwLock::new(None));
        let writer = {
            let reg = Arc::clone(&reg);
            thread::spawn(move || {
                install(&reg, Some(1));
            })
        };
        let seen = current(&reg);
        assert!(seen.is_none() || seen == Some(1));
        writer.join().unwrap();
        install(&reg, Some(2));
        assert_eq!(current(&reg), Some(2));
    });
}
