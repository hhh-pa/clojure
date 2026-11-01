// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure.java.shell` substrate (#120 slice 3): the `proc`
//! native behind `sh` — spawn via `std::process`, feed stdin,
//! capture stdout/stderr, report `{:exit :out :err}`. Decode
//! reuses the slice-2 charset tables (`lookup_charset` errors
//! with the same `UnsupportedEncodingException` upstream throws).
//! Installed lazily by [`install_shell_ns`] (the gap-123
//! `install_async_ns` shape): requiring the namespace installs
//! the native and marks it loaded.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use clj_value::{Array, ArrayDesc, ArrayStore, BaseType, Keyword, ThrowClass, Value, new_map};

use crate::core::cce_err;
use crate::ctor::{decode, decode_utf8, lookup_charset};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity, str_of};

/// The spawn-helper namespace (required by `clojure.java.shell`).
pub(crate) const SHELL_NS: &str = "clojure.java.shell.impl";

/// The shell-substrate natives (installed lazily by
/// [`install_shell_ns`] — never referred into `user`).
pub(crate) const SHELL_NATIVES: &[(&str, NativeFnPtr)] = &[("proc", native_proc)];

/// Install the `clojure.java.shell.impl` namespace
/// (idempotent — reinstalling over itself is a `:reload` no-op).
pub(crate) fn install_shell_ns(eval: &mut Evaluator) {
    eval.registry.ensure_ns(SHELL_NS);
    for (name, f) in SHELL_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(SHELL_NS, name, func);
    }
}

/// `IOException` naming the failing program (port-canonical —
/// upstream quotes the OS text after the program too).
fn cannot_run(prog: &str, e: std::io::Error) -> NativeError {
    NativeError::new(
        ThrowClass::IOException,
        format!("Cannot run program \"{prog}\": {e}"),
    )
}

/// The command vector as owned strings (non-empty — upstream
/// throws on an empty command as well).
fn cmd_parts(cmd: &Value) -> Result<Vec<String>, NativeError> {
    let items = match cmd {
        Value::Vector(v) => v,
        _ => return Err(cce_err(cmd, "clojure.lang.IPersistentVector")),
    };
    let mut parts = Vec::with_capacity(items.len());
    for item in items.iter() {
        match item {
            Value::Str(s) => parts.push(s.to_string()),
            _ => return Err(cce_err(item, "java.lang.String")),
        }
    }
    if parts.is_empty() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "sh requires at least one command string".to_owned(),
        ));
    }
    Ok(parts)
}

/// Snapshot a `byte[]`'s unsigned slots (`None` for other
/// stores, mirroring the slice-2 reader).
fn input_bytes(a: &Array) -> Option<Vec<u8>> {
    match &*a.lock() {
        ArrayStore::Bytes(v) => Some(v.iter().map(|b| *b as u8).collect()),
        _ => None,
    }
}

/// The stdin payload: strings encode as UTF-8 (`:in-enc` names
/// anything else — upstream encodes, the port has decoders
/// only), byte arrays pass through, nil omits (upstream closes
/// stdin, which reads the same EOF).
fn stdin_of(input: &Value, in_enc: &Value) -> Result<Option<Vec<u8>>, NativeError> {
    match input {
        Value::Nil => Ok(None),
        Value::Str(s) => match in_enc {
            Value::Nil => Ok(Some(s.as_bytes().to_vec())),
            Value::Str(name) if name.as_ref() == "UTF-8" || name.as_ref() == "UTF8" => {
                Ok(Some(s.as_bytes().to_vec()))
            }
            // Unknown names travel verbatim (like `lookup_charset`
            // — upstream quotes the name too).
            Value::Str(name) => Err(NativeError::new(
                ThrowClass::UnsupportedEncodingException,
                name.to_string(),
            )),
            _ => Err(NativeError::new(
                ThrowClass::UnsupportedEncodingException,
                str_of(in_enc),
            )),
        },
        Value::Array(a) => match input_bytes(a) {
            Some(bytes) => Ok(Some(bytes)),
            None => Err(cce_err(input, "[B")),
        },
        _ => Err(cce_err(input, "java.lang.String")),
    }
}

/// One env entry's `NAME=value` text (upstream `name`s the key
/// and `str`s the value).
fn env_entry(key: &Value, val: &Value) -> Result<(String, String), NativeError> {
    let name = match key {
        Value::Keyword(k) => k.name().to_owned(),
        Value::Str(s) => s.to_string(),
        Value::Symbol(s) => s.name().to_owned(),
        _ => return Err(cce_err(key, "clojure.lang.Named")),
    };
    Ok((name, str_of(val)))
}

/// The replacement environment (upstream replaces the whole
/// environment when `:env` is non-nil, inherits otherwise).
fn env_of(env: &Value) -> Result<Option<Vec<(String, String)>>, NativeError> {
    match env {
        Value::Nil => Ok(None),
        Value::Map(m) => {
            let mut out = Vec::new();
            for (k, v) in m.iter() {
                out.push(env_entry(k, v)?);
            }
            Ok(Some(out))
        }
        _ => Err(cce_err(env, "clojure.lang.IPersistentMap")),
    }
}

/// The working directory (strings only — `File` dirs need the
/// `clojure.java.io` slice, which has not landed).
fn dir_of(dir: &Value) -> Result<Option<String>, NativeError> {
    match dir {
        Value::Nil => Ok(None),
        Value::Str(s) => Ok(Some(s.to_string())),
        _ => Err(cce_err(dir, "java.lang.String")),
    }
}

/// Run the command: stdin (when given) is written on a scoped
/// thread while the parent drains both streams, so large
/// inputs cannot deadlock against large outputs.
fn run_command(
    prog: &str,
    args: &[String],
    input: Option<&[u8]>,
    env: Option<&[(String, String)]>,
    dir: Option<&str>,
) -> Result<std::process::Output, NativeError> {
    let mut child = Command::new(prog);
    child
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        child.env_clear();
        for (k, v) in env {
            child.env(k, v);
        }
    }
    if let Some(dir) = dir {
        child.current_dir(dir);
    }
    let mut spawned = child.spawn().map_err(|e| cannot_run(prog, e))?;
    match input {
        Some(input) => {
            let mut stdin = spawned.stdin.take().expect("piped stdin");
            std::thread::scope(|scope| {
                scope.spawn(move || {
                    let _ = stdin.write_all(input);
                });
                spawned.wait_with_output()
            })
            .map_err(|e| cannot_run(prog, e))
        }
        None => spawned.wait_with_output().map_err(|e| cannot_run(prog, e)),
    }
}

/// Decode stdout per `:out-enc` (`:bytes` stays a `byte[]`,
/// names decode through the shared tables).
fn out_of(out_enc: &Value, bytes: &[u8]) -> Result<Value, NativeError> {
    match out_enc {
        Value::Nil => Ok(Value::Str(Arc::from(decode_utf8(bytes)))),
        Value::Keyword(k) if k.name() == "bytes" => Ok(Value::Array(Array::new(
            ArrayDesc::flat(BaseType::Byte),
            ArrayStore::Bytes(bytes.iter().map(|b| *b as i8).collect()),
        ))),
        Value::Str(name) => Ok(Value::Str(Arc::from(decode(lookup_charset(name)?, bytes)))),
        Value::Keyword(k) => Ok(Value::Str(Arc::from(decode(
            lookup_charset(k.name())?,
            bytes,
        )))),
        _ => Err(NativeError::new(
            ThrowClass::UnsupportedEncodingException,
            str_of(out_enc),
        )),
    }
}

/// `{:exit :out :err}` for a finished run (stderr decodes lossy
/// UTF-8 — upstream uses the platform default, identical on
/// UTF-8 platforms).
fn result_map(
    out_enc: &Value,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Value, NativeError> {
    let exit = status.code().unwrap_or(-1);
    let mut map = new_map();
    map = map.insert(
        Value::Keyword(Keyword::new(None, "exit")),
        Value::Int(i64::from(exit)),
    );
    map = map.insert(
        Value::Keyword(Keyword::new(None, "out")),
        out_of(out_enc, stdout)?,
    );
    map = map.insert(
        Value::Keyword(Keyword::new(None, "err")),
        Value::Str(Arc::from(decode_utf8(stderr))),
    );
    Ok(Value::Map(map))
}

/// `(proc cmd in in-enc out-enc env dir)`: the six pre-parsed
/// `sh` arguments straight through to [`run_command`].
fn native_proc(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.java.shell.impl/proc", args, &[6])?;
    let parts = cmd_parts(&args[0])?;
    let input = stdin_of(&args[1], &args[2])?;
    let env = env_of(&args[4])?;
    let dir = dir_of(&args[5])?;
    let output = run_command(
        &parts[0],
        &parts[1..],
        input.as_deref(),
        env.as_deref(),
        dir.as_deref(),
    )?;
    result_map(&args[3], output.status, &output.stdout, &output.stderr)
}
