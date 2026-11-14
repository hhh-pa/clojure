// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Build script for `clj-aot` (#260): compiles `clj-bc` to LLVM bitcode
//! when the active `rustc`'s LLVM major matches the backend's LLVM 23
//! so compiled Clojure modules can link and inline runtime helpers with
//! LTO, and falls back to `HELPER_BC = None` otherwise.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

const BACKEND_LLVM_MAJOR: u32 = 23;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../clj-bc/Cargo.toml");
    println!("cargo:rerun-if-changed=../clj-bc/src");
    println!("cargo:rerun-if-changed=../clj-value/Cargo.toml");
    println!("cargo:rerun-if-changed=../clj-value/src");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=RUSTC");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR missing"));
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR missing"));
    let target = std::env::var("TARGET").expect("TARGET missing");

    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let (rustc_ver, rustc_major) = detect_rustc_llvm(&rustc);

    let bc_path = if rustc_major == Some(BACKEND_LLVM_MAJOR) {
        Some(compile_helper_bc(&manifest_dir, &out_dir, &target))
    } else {
        None
    };

    write_helper_rs(&out_dir, &rustc_ver, rustc_major, bc_path.as_deref());
}

fn detect_rustc_llvm(rustc: &OsString) -> (String, Option<u32>) {
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("failed to invoke `rustc -vV`");
    assert!(output.status.success(), "`rustc -vV` failed");
    let text = String::from_utf8_lossy(&output.stdout);
    parse_rustc_llvm_version(&text)
}

fn parse_rustc_llvm_version(vv: &str) -> (String, Option<u32>) {
    for line in vv.lines() {
        if let Some(rest) = line.strip_prefix("LLVM version:") {
            let ver = rest.trim().to_owned();
            let major = ver
                .split(['.', '-'])
                .next()
                .and_then(|s| s.parse::<u32>().ok());
            return (ver, major);
        }
    }
    ("unknown".to_owned(), None)
}

fn should_strip_env(key: &str) -> bool {
    matches!(
        key,
        "RUSTFLAGS"
            | "CARGO_ENCODED_RUSTFLAGS"
            | "RUSTDOCFLAGS"
            | "RUSTC_WRAPPER"
            | "RUSTC_WORKSPACE_WRAPPER"
            | "LLVM_PROFILE_FILE"
            | "CARGO_INCREMENTAL"
            | "CARGO_TARGET_DIR"
    ) || key.starts_with("CARGO_LLVM_COV")
        || (key.starts_with("CARGO_TARGET_") && key.ends_with("_RUSTFLAGS"))
        || (key.starts_with("CARGO_BUILD_") && key.ends_with("_DIR"))
}

fn compile_helper_bc(manifest_dir: &Path, out_dir: &Path, target: &str) -> PathBuf {
    let bc_manifest = manifest_dir.join("../clj-bc/Cargo.toml");
    let bc_target = out_dir.join("bc-target");

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut cmd = Command::new(&cargo);
    for (k, _) in std::env::vars_os() {
        if let Some(name) = k.to_str()
            && should_strip_env(name)
        {
            cmd.env_remove(&k);
        }
    }
    // Compile with `target-cpu=generic` so helper IR never carries host-specific CPU
    // features (e.g. `apple-m1`) that would block LLVM inlining into `"generic"` modules.
    let output = cmd
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            "-C\x1fpanic=abort\x1f-C\x1ftarget-cpu=generic",
        )
        .current_dir(manifest_dir)
        .args([
            "rustc",
            "--offline",
            "--locked",
            "--release",
            "--message-format=json",
        ])
        .arg("--manifest-path")
        .arg(&bc_manifest)
        .arg("--target")
        .arg(target)
        .arg("--target-dir")
        .arg(&bc_target)
        .args(["--", "--emit=llvm-bc", "-C", "codegen-units=1"])
        .output()
        .expect("failed to invoke cargo rustc for clj-bc");
    assert!(
        output.status.success(),
        "cargo rustc for clj-bc failed ({}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    find_bc_from_json(&stdout)
}

/// Locate the emitted `clj_bc-<hash>.bc` alongside the `clj_bc` artifact's `.rmeta`.
///
/// Cargo <1.100 writes both in `<target-dir>/<triple>/release/deps/` while Cargo >=1.100
/// places them in `<target-dir>/<triple>/release/build/clj-bc/<hash>/out/`; reading the
/// compiler-artifact JSON avoids hardcoding either layout.
fn find_bc_from_json(stdout: &str) -> PathBuf {
    for line in stdout.lines() {
        if !line.contains("\"compiler-artifact\"") || !line.contains("\"clj_bc\"") {
            continue;
        }
        for raw in json_filenames(line) {
            if let Some(bc) = bc_from_artifact_path(Path::new(&raw)) {
                return bc;
            }
        }
    }
    panic!("missing clj_bc llvm-bc artifact in cargo json output:\n{stdout}");
}

fn bc_from_artifact_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let hash = name.strip_prefix("libclj_bc-")?.strip_suffix(".rmeta")?;
    let bc = path.with_file_name(format!("clj_bc-{hash}.bc"));
    bc.is_file().then_some(bc)
}

fn json_filenames(line: &str) -> Vec<String> {
    let Some(start) = line.find("\"filenames\":[") else {
        return Vec::new();
    };
    let mut chars = line[start + "\"filenames\":[".len()..].chars();
    let mut out = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            ']' => break,
            '"' => out.push(unescape_json_str(&mut chars)),
            _ => {}
        }
    }
    out
}

fn unescape_json_str(chars: &mut std::str::Chars<'_>) -> String {
    let mut s = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            break;
        }
        if c != '\\' {
            s.push(c);
            continue;
        }
        let Some(esc) = chars.next() else {
            break;
        };
        match esc {
            '"' | '\\' | '/' => s.push(esc),
            'n' => s.push('\n'),
            'r' => s.push('\r'),
            't' => s.push('\t'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(code)
                {
                    s.push(ch);
                }
            }
            other => {
                s.push('\\');
                s.push(other);
            }
        }
    }
    s
}

fn write_helper_rs(
    out_dir: &Path,
    rustc_ver: &str,
    rustc_major: Option<u32>,
    bc_path: Option<&Path>,
) {
    let bc_expr = match bc_path {
        Some(path) => {
            let s = path.to_str().expect("non-UTF-8 bitcode path");
            format!("Some(include_bytes!({s:?}))")
        }
        None => "None".to_owned(),
    };
    let generated = format!(
        "pub const BACKEND_LLVM_MAJOR: u32 = {BACKEND_LLVM_MAJOR};\n\
         pub const RUSTC_LLVM_VERSION: &str = {rustc_ver:?};\n\
         pub const RUSTC_LLVM_MAJOR: Option<u32> = {rustc_major:?};\n\
         pub const HELPER_BC: Option<&[u8]> = {bc_expr};\n"
    );
    std::fs::write(out_dir.join("helper_bc.rs"), generated).expect("failed to write helper_bc.rs");
}
