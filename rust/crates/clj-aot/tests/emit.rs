// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Hermetic `--emit-ir` / `--emit-obj` paths (no linker, no
//! archive): outputs land, linking is skipped, failures name
//! their cause.

use std::path::PathBuf;

use clj_aot::{CompileError, EmitOptions, LinkOptions, compile_file, compile_str};

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("clj-aot-emit-{}-{name}", std::process::id()))
}

fn no_link() -> LinkOptions {
    LinkOptions {
        runtime_archive: None,
        exe_dir: tmp("no-such-dir"),
        linker: Some("clj-aot-definitely-not-a-linker".to_owned()),
    }
}

#[test]
fn emit_ir_writes_module_and_skips_link() {
    let dir = tmp("ir");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let src = dir.join("f.clj");
    std::fs::write(&src, "(defn f [x] (+ x 1)) (print (f 41))").expect("src");
    let ir = dir.join("f.ll");
    let out = dir.join("f");
    compile_file(
        &src,
        &out,
        &EmitOptions {
            emit_ir: Some(ir.clone()),
            emit_obj: None,
        },
        &no_link(),
    )
    .expect("emit ir");
    let text = std::fs::read_to_string(&ir).expect("read ir");
    assert!(text.contains("define ptr @cljfn_f_0"), "{text}");
    assert!(text.contains("define i32 @main()"), "{text}");
    assert!(!out.exists(), "linking must be skipped");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn emit_obj_writes_object_and_skips_link() {
    let dir = tmp("obj");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let obj = dir.join("f.o");
    let out = dir.join("f");
    compile_str(
        "(print 1)",
        "f.clj",
        &out,
        &EmitOptions {
            emit_ir: None,
            emit_obj: Some(obj.clone()),
        },
        &no_link(),
    )
    .expect("emit obj");
    assert!(!std::fs::read(&obj).expect("read obj").is_empty());
    assert!(!out.exists(), "linking must be skipped");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn emit_covers_arity_branches_through_the_rlib() {
    // The unit snapshots verify these shapes; emitting them here
    // executes the rlib copy of the arity joins too (the libtest
    // copy's profile is dropped as mismatched on merge).
    for (i, source) in [
        "(defn f [x] x) (if true (f 1 2) 3)",
        "(defn f [x] x) (if true 3 (f 1 2))",
        "(defn f [x] x) (if true (f 1 2) (f 1 2))",
        "(defn f [x] x) (if (f 1 2) 1 2)",
        "(defn f [x] x) (+ (f 1 2) 4)",
    ]
    .iter()
    .enumerate()
    {
        let dir = tmp(&format!("arity-{i}"));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let ir = dir.join("f.ll");
        compile_str(
            source,
            "f.clj",
            &dir.join("f"),
            &EmitOptions {
                emit_ir: Some(ir.clone()),
                emit_obj: None,
            },
            &no_link(),
        )
        .expect("emit arity branch");
        let text = std::fs::read_to_string(&ir).expect("read ir");
        assert!(
            text.contains("call ptr @rt_arity_error"),
            "{source}\n{text}"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

#[test]
fn out_of_subset_names_the_form() {
    let err = compile_str(
        "(def x 1)",
        "f.clj",
        &tmp("nope"),
        &EmitOptions::default(),
        &no_link(),
    )
    .expect_err("must reject def");
    assert_eq!(
        err,
        CompileError::Plan(
            "f.clj:1:1: unsupported form in AOT subset: def (only defn defines in the AOT subset)"
                .to_owned()
        )
    );
    assert_eq!(
        format!("{err}"),
        "f.clj:1:1: unsupported form in AOT subset: def (only defn defines in the AOT subset)"
    );
}

#[test]
fn io_failures_name_the_path() {
    let missing = tmp("no-such-dir").join("f.clj");
    let err = compile_file(&missing, &tmp("nope"), &EmitOptions::default(), &no_link())
        .expect_err("missing input");
    assert!(matches!(err, CompileError::Io { .. }), "{err:?}");
    assert!(format!("{err}").contains("f.clj"), "{err}");

    let err = compile_str(
        "(print 1)",
        "f.clj",
        &tmp("nope"),
        &EmitOptions {
            emit_ir: Some(tmp("no-such-dir").join("f.ll")),
            emit_obj: None,
        },
        &no_link(),
    )
    .expect_err("unwritable ir");
    assert!(matches!(err, CompileError::Io { .. }), "{err:?}");
}

#[test]
fn display_covers_every_variant() {
    assert_eq!(format!("{}", CompileError::Target("t".to_owned())), "t");
    assert_eq!(format!("{}", CompileError::Emit("e".to_owned())), "e");
    assert_eq!(
        format!(
            "{}",
            CompileError::Link(clj_aot::link::LinkError::LinkFailed("l".to_owned()))
        ),
        "l"
    );
}
