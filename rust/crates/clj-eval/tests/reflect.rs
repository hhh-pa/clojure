// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! #120 sub-slice 4 tests: the `clojure.reflect` facsimile.
//!
//! Oracles (text only, never executed): upstream
//! `clojure/reflect.clj` + `clojure/reflect/java.clj` for the
//! `type-reflect`/`reflect` map shapes (keys, member keys, ctor
//! naming, the `:method` flags context for ctors, the `:ancestors`
//! walk, the `TypeReference`/`Reflector` protocols). Member
//! *coverage* is the port's own (the emulated surface only —
//! `..` members, `Class/forName`, the `String`/`Object` ctors),
//! pinned member by member below; unemulated JVM members are
//! absent by design (see `reflect.clj`).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// Require reflect (as `r`).
fn require_reflect(eval: &mut Evaluator) {
    eval.eval_source("(require '[clojure.reflect :as r])")
        .unwrap();
}

fn fresh() -> Evaluator {
    let mut eval = boot();
    require_reflect(&mut eval);
    eval
}

/// Assert an uncaught throw whose class and message contain the parts.
fn assert_throws(eval: &mut Evaluator, src: &str, class_part: &str, msg_part: &str) {
    let got = uncaught(eval, src);
    assert!(
        got.contains(class_part),
        "class mismatch for {src}: got {got}"
    );
    assert!(
        got.contains(msg_part),
        "message mismatch for {src}: got {got}"
    );
}

// ---- java.lang.Object: baseless, public, five members ----

#[test]
fn object_bases_flags() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.lang.Object)] [(:bases m) (:flags m)])"
        ),
        "[nil #{:public}]"
    );
}

#[test]
fn object_members() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (:members (r/type-reflect 'java.lang.Object))
                '#{{:name java.lang.Object :declaring-class java.lang.Object
                    :parameter-types [] :exception-types [] :flags #{:public}}
                   {:name equals :declaring-class java.lang.Object
                    :parameter-types [java.lang.Object] :exception-types []
                    :return-type boolean :flags #{:public}}
                   {:name hashCode :declaring-class java.lang.Object
                    :parameter-types [] :exception-types []
                    :return-type int :flags #{:public}}
                   {:name toString :declaring-class java.lang.Object
                    :parameter-types [] :exception-types []
                    :return-type java.lang.String :flags #{:public}}
                   {:name getClass :declaring-class java.lang.Object
                    :parameter-types [] :exception-types []
                    :return-type java.lang.Class :flags #{:public}}})"
        ),
        "true"
    );
}

// ---- java.lang.String: bases, ctors, isEmpty/isBlank ----

#[test]
fn string_bases_flags() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.lang.String)]
               [(= (:flags m) #{:public})
                (= (:bases m) '#{java.lang.Object java.io.Serializable java.lang.Comparable
                                 java.lang.CharSequence java.lang.constant.Constable
                                 java.lang.constant.ConstantDesc})])"
        ),
        "[true true]"
    );
}

#[test]
fn string_ctor_member() {
    let mut eval = fresh();
    // The 4-arg ctor, pinned whole (name is the binary name, like upstream).
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'java.lang.String))
               '{:name java.lang.String :declaring-class java.lang.String
                 :parameter-types [byte<> int int java.lang.String]
                 :exception-types [] :flags #{:public}})"
        ),
        "true"
    );
    // Ten ctors (the port-constructible matrix) plus isEmpty/isBlank.
    assert_eq!(
        run(
            &mut eval,
            "(count (:members (r/type-reflect 'java.lang.String)))"
        ),
        "12"
    );
}

#[test]
fn string_methods() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (set (map :name (:members (r/type-reflect 'java.lang.String))))
                '#{java.lang.String isEmpty isBlank})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'java.lang.String))
               '{:name isBlank :declaring-class java.lang.String
                 :parameter-types [] :exception-types []
                 :return-type boolean :flags #{:public}})"
        ),
        "true"
    );
}

// ---- java.lang.Class: instance members + static forName ----

#[test]
fn class_members() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(count (:members (r/type-reflect 'java.lang.Class)))"
        ),
        "12"
    );
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'java.lang.Class))
               '{:name getInterfaces :declaring-class java.lang.Class
                 :parameter-types [] :exception-types []
                 :return-type java.lang.Class<> :flags #{:public}})"
        ),
        "true"
    );
}

#[test]
fn class_forname_static() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'java.lang.Class))
               '{:name forName :declaring-class java.lang.Class
                 :parameter-types [java.lang.String boolean java.lang.ClassLoader]
                 :exception-types [] :return-type java.lang.Class
                 :flags #{:public :static}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(count (filter #(= (:name %) 'forName) (:members (r/type-reflect 'java.lang.Class))))"
        ),
        "3"
    );
}

// ---- interfaces: abstract flags, abstract members ----

#[test]
fn interface_flags() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.util.List)]
               [(= (:flags m) #{:public :abstract :interface})
                (= (:bases m) '#{java.util.SequencedCollection})])"
        ),
        "[true true]"
    );
}

#[test]
fn interface_member_abstract() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (:members (r/type-reflect 'java.util.Collection))
                '#{{:name isEmpty :declaring-class java.util.Collection
                    :parameter-types [] :exception-types []
                    :return-type boolean :flags #{:public :abstract}}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (set (map :name (:members (r/type-reflect 'java.util.List))))
                '#{getFirst getLast})"
        ),
        "true"
    );
}

// ---- the remaining emulated members ----

#[test]
fn gated_members() {
    let mut eval = fresh();
    // Map$Entry, Named (+ the concrete Keyword/Symbol), Throwable.
    assert_eq!(
        run(
            &mut eval,
            "(= (set (map :name (:members (r/type-reflect 'java.util.Map$Entry))))
                '#{getKey getValue})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'clojure.lang.Named))
               '{:name getNamespace :declaring-class clojure.lang.Named
                 :parameter-types [] :exception-types []
                 :return-type java.lang.String :flags #{:public :abstract}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(contains? (:members (r/type-reflect 'clojure.lang.Keyword))
               '{:name getName :declaring-class clojure.lang.Keyword
                 :parameter-types [] :exception-types []
                 :return-type java.lang.String :flags #{:public}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (set (map :name (:members (r/type-reflect 'java.lang.Throwable))))
                '#{getMessage getLocalizedMessage getCause})"
        ),
        "true"
    );
}

#[test]
fn exinfo_pending_writer_members() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (:members (r/type-reflect 'clojure.lang.ExceptionInfo))
                '#{{:name getData :declaring-class clojure.lang.ExceptionInfo
                    :parameter-types [] :exception-types []
                    :return-type java.lang.Object :flags #{:public}}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:members (r/type-reflect 'clojure.lang.IPending))
                '#{{:name isRealized :declaring-class clojure.lang.IPending
                    :parameter-types [] :exception-types []
                    :return-type boolean :flags #{:public :abstract}}})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:members (r/type-reflect 'java.io.Writer))
                '#{{:name close :declaring-class java.io.Writer
                    :parameter-types [] :exception-types []
                    :return-type void :flags #{:public}}
                   {:name flush :declaring-class java.io.Writer
                    :parameter-types [] :exception-types []
                    :return-type void :flags #{:public}}})"
        ),
        "true"
    );
}

#[test]
fn empty_members_class() {
    let mut eval = fresh();
    // Lattice classes without emulated members reflect empty members.
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.lang.Integer)] [(:flags m) (:members m)])"
        ),
        "[#{:public} #{}]"
    );
}

// ---- reflect/type-reflect entry points ----

#[test]
fn reflect_instance_and_class() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (r/reflect \"hi\") (r/type-reflect 'java.lang.String))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (r/reflect (class \"hi\")) (r/type-reflect 'java.lang.String))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (:bases (r/reflect :kw)) (:bases (r/type-reflect 'clojure.lang.Keyword)))"
        ),
        "true"
    );
}

#[test]
fn ancestors_walk() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.lang.String :ancestors true)]
               [(contains? (:ancestors m) 'java.lang.Object)
                (contains? (:ancestors m) 'java.io.Serializable)
                (contains? (set (map :name (:members m))) 'equals)
                (contains? (set (map :name (:members m))) 'isEmpty)])"
        ),
        "[true true true true]"
    );
}

#[test]
fn ancestors_empty() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (r/type-reflect 'java.lang.Object :ancestors true)]
               [(:ancestors m) (count (:members m))])"
        ),
        "[#{} 5]"
    );
}

#[test]
fn custom_reflector_option() {
    let mut eval = fresh();
    // The default reflector answers through the option too.
    assert_eq!(
        run(
            &mut eval,
            "(= (r/type-reflect 'java.lang.Object :reflector @#'r/default-reflector)
                (r/type-reflect 'java.lang.Object))"
        ),
        "true"
    );
    // Any Reflector satisfies the option (pins the protocol is open).
    assert_eq!(
        run(
            &mut eval,
            "(let [rfl (reify clojure.reflect/Reflector
                         (do-reflect [_ t] {:bases nil :flags #{} :members #{}}))]
               (r/type-reflect 'java.lang.Object :reflector rfl))"
        ),
        "{:bases nil, :flags #{}, :members #{}}"
    );
}

// ---- typename ----

#[test]
fn typename_shapes() {
    let mut eval = fresh();
    assert_eq!(run(&mut eval, "(r/typename 'byte<>)"), "\"byte[]\"");
    assert_eq!(run(&mut eval, "(r/typename 'a/b)"), "\"a/b\"");
    assert_eq!(
        run(&mut eval, "(r/typename (class \"x\"))"),
        "\"java.lang.String\""
    );
}

#[test]
fn typename_miss() {
    let mut eval = fresh();
    assert_throws(
        &mut eval,
        "(r/typename 5)",
        "IllegalArgumentException",
        "No implementation of method: :typename of protocol: #'clojure.reflect/TypeReference found for class: java.lang.Long",
    );
    assert_throws(
        &mut eval,
        "(r/typename nil)",
        "IllegalArgumentException",
        "found for class: null",
    );
    // Strings stay unextended, as upstream.
    assert_throws(
        &mut eval,
        "(r/typename \"x\")",
        "IllegalArgumentException",
        "found for class: java.lang.String",
    );
}

// ---- errors ----

#[test]
fn unknown_class() {
    let mut eval = fresh();
    assert_throws(
        &mut eval,
        "(r/type-reflect 'no.such.Klass)",
        "ClassNotFoundException",
        "no.such.Klass",
    );
    // Array spellings normalize, then miss like forName.
    assert_throws(
        &mut eval,
        "(r/type-reflect 'byte<>)",
        "ClassNotFoundException",
        "byte[]",
    );
}

#[test]
fn typeref_validates_through_protocol() {
    let mut eval = fresh();
    assert_throws(
        &mut eval,
        "(r/type-reflect 5)",
        "IllegalArgumentException",
        "No implementation of method: :typename",
    );
}

// ---- HostDef classes ----

#[test]
fn hostdef_reflect() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype ReflectFoo [x])
                 (let [m (r/type-reflect 'user.ReflectFoo)]
                   [(:flags m) (:bases m) (:members m)]))"
        ),
        "[#{:public} #{java.lang.Object clojure.lang.IType} #{}]"
    );
}

// ---- parse-flags ----

#[test]
fn parse_flags() {
    let mut eval = fresh();
    assert_eq!(
        run(
            &mut eval,
            "(= (@#'r/parse-flags 9 :method) #{:public :static})"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (@#'r/parse-flags 1537 :class) #{:public :abstract :interface})"
        ),
        "true"
    );
}

// ---- hidden natives, direct ----

#[test]
fn native_arity() {
    let mut eval = fresh();
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect*)",
        "ArityException",
        "clojure.core/__reflect*",
    );
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect_typename* 1 2)",
        "ArityException",
        "clojure.core/__reflect_typename*",
    );
}

#[test]
fn native_direct() {
    let mut eval = fresh();
    // Strings resolve (the do-reflect call path).
    assert_eq!(
        run(
            &mut eval,
            "(let [m (clojure.core/__reflect* \"java.lang.Object\")]
               [(:bases m) (:flags-raw m) (count (:members m))])"
        ),
        "[nil 1 5]"
    );
    // Classes pass through.
    assert_eq!(
        run(
            &mut eval,
            "(:flags-raw (clojure.core/__reflect* (class \"x\")))"
        ),
        "1"
    );
    assert_eq!(
        run(&mut eval, "(clojure.core/__reflect_typename* 'a<>)"),
        "\"a[]\""
    );
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect* 5)",
        "ClassCastException",
        "java.lang.String",
    );
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect* nil)",
        "NullPointerException",
        "do-reflect",
    );
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect_typename* nil)",
        "NullPointerException",
        "do-reflect",
    );
    assert_throws(
        &mut eval,
        "(clojure.core/__reflect_typename* 5)",
        "ClassCastException",
        "java.lang.String",
    );
}
