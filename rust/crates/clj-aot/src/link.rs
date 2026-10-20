// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Platform link step: find the `clj-rt` archive, link the object
//! into a native binary (S8C; LLVM-free).
//!
//! Discovery is explicit and testable: [`find_archive`] takes the
//! executable directory and an optional override file (the CLI
//! passes its own directory plus `--runtime-archive`); the link
//! command is built by [`link_command`] over an explicit
//! [`Platform`] so both spellings pin on every host.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Link target family (host-only; S8 has no cross-compilation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

/// The host [`Platform`].
#[cfg(unix)]
#[must_use]
pub fn current_platform() -> Platform {
    Platform::Unix
}

/// The host [`Platform`].
#[cfg(windows)]
#[must_use]
pub fn current_platform() -> Platform {
    Platform::Windows
}

/// The runtime archive file name per platform.
#[must_use]
pub fn archive_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Unix => "libcljrt.a",
        Platform::Windows => "cljrt.lib",
    }
}

/// Link failures (the CLI renders these as `build: ...`, exit 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// No runtime archive at the override or next to the binary.
    ArchiveNotFound(String),
    /// The linker failed to run or reported an error.
    LinkFailed(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArchiveNotFound(msg) | Self::LinkFailed(msg) => write!(f, "{msg}"),
        }
    }
}

/// Find the runtime archive: the `--runtime-archive` override file
/// when given, else [`archive_name`] next to the executable.
pub fn find_archive(
    exe_dir: &Path,
    override_file: Option<&Path>,
    platform: Platform,
) -> Result<PathBuf, LinkError> {
    if let Some(path) = override_file {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(LinkError::ArchiveNotFound(format!(
            "runtime archive not found: {}",
            path.display()
        )));
    }
    let candidate = exe_dir.join(archive_name(platform));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(LinkError::ArchiveNotFound(format!(
        "runtime archive not found next to the binary ({}); pass --runtime-archive",
        candidate.display()
    )))
}

/// A link invocation: linker program plus arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCommand {
    /// Linker program (`cc`, `link`, or the test override).
    pub linker: String,
    /// Arguments (paths passed through, never re-rendered).
    pub args: Vec<String>,
}

/// Build the link command (`cc` on Unix, `link` on Windows; the
/// override names the linker program for tests). The Windows arm
/// carries the system libs a Rust staticlib needs: `rustc` links
/// these itself, but a raw `link` invocation does not (R1
/// finding 4 — without them every `clj-rt` import is LNK2019).
/// `bcryptprimitives.lib` is deliberately absent: `link` cannot
/// open it on CI images (LNK1181) and nothing in `clj-rt`
/// references CNG primitives — `rustc` links the same staticlib
/// on that runner without it.
#[must_use]
pub fn link_command(
    platform: Platform,
    obj: &Path,
    archive: &Path,
    out: &Path,
    linker_override: Option<&str>,
) -> LinkCommand {
    let exe = |p: &Path| p.as_os_str().to_string_lossy().into_owned();
    match platform {
        Platform::Unix => LinkCommand {
            linker: linker_override.unwrap_or("cc").to_owned(),
            args: vec![exe(obj), exe(archive), "-o".to_owned(), exe(out)],
        },
        Platform::Windows => LinkCommand {
            linker: linker_override.unwrap_or("link").to_owned(),
            args: vec![
                "/NOLOGO".to_owned(),
                exe(obj),
                exe(archive),
                format!("/OUT:{}", exe(out)),
                "kernel32.lib".to_owned(),
                "advapi32.lib".to_owned(),
                "userenv.lib".to_owned(),
                "ws2_32.lib".to_owned(),
                "bcrypt.lib".to_owned(),
                "ntdll.lib".to_owned(),
                // Static CRT trio (CI builds `+crt-static`, see
                // setup-llvm): `memcpy`/`exit`/`mainCRTStartup`
                // live in `libcmt`, the C++ EH + `__chkstk` in
                // `libvcruntime`, the C library in `libucrt`.
                "libcmt.lib".to_owned(),
                "libvcruntime.lib".to_owned(),
                "libucrt.lib".to_owned(),
            ],
        },
    }
}

/// Pick the macOS SDK root for the link child: an explicit
/// `SDKROOT` wins, else the `xcrun` query, else none (plain `cc`
/// behavior). Pure so the table pins without touching the
/// process environment.
#[cfg(target_os = "macos")]
fn choose_sdkroot(env: Option<&str>, queried: Option<&str>) -> Option<String> {
    match (env, queried) {
        (Some(explicit), _) => Some(explicit.to_owned()),
        (None, Some(found)) => Some(found.to_owned()),
        (None, None) => None,
    }
}

/// Query `xcrun` for the macOS SDK (rustc does the same before
/// every macOS link; without it a skewed CLT/Xcode pair links
/// against an SDK its own linker cannot parse).
#[cfg(target_os = "macos")]
fn query_sdkroot() -> Option<String> {
    let out = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let trimmed = path.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Set `SDKROOT` on the link child (macOS only).
#[cfg(target_os = "macos")]
fn apply_macos_sdkroot(child: &mut Command) {
    let env = std::env::var("SDKROOT").ok();
    if let Some(sdk) = choose_sdkroot(env.as_deref(), query_sdkroot().as_deref()) {
        child.env("SDKROOT", sdk);
    }
}

/// Render a link failure: exit status plus the linker's chatter
/// when it says anything — stdout AND stderr, since MSVC `link`
/// reports LNK errors on stdout while `cc` uses stderr (R1
/// finding 7). Success stays silent, so build stderr pins clean.
fn link_failure_text(linker: &str, out: &std::process::Output) -> String {
    let mut chatter = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        if !chatter.is_empty() {
            chatter.push('\n');
        }
        chatter.push_str(stderr);
    }
    if chatter.is_empty() {
        format!("{linker} exited with {}", out.status)
    } else {
        format!("{linker} exited with {}: {chatter}", out.status)
    }
}

/// Run the link command (success is exit 0; a missing linker
/// program reports as not-found, never as a crash; linker chatter
/// is captured — failures carry it, successes stay silent).
pub fn run_link(cmd: &LinkCommand) -> Result<(), LinkError> {
    let mut child = Command::new(&cmd.linker);
    child.args(&cmd.args);
    #[cfg(target_os = "macos")]
    apply_macos_sdkroot(&mut child);
    match child.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(LinkError::LinkFailed(link_failure_text(&cmd.linker, &out))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(LinkError::LinkFailed(format!(
            "linker not found: {}",
            cmd.linker
        ))),
        Err(e) => Err(LinkError::LinkFailed(format!("{}: {e}", cmd.linker))),
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkCommand, LinkError};
    use super::{Platform, find_archive, link_command, run_link};
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("clj-aot-link-{}-{name}", std::process::id()))
    }

    #[test]
    fn archive_names() {
        assert_eq!(super::archive_name(Platform::Unix), "libcljrt.a");
        assert_eq!(super::archive_name(Platform::Windows), "cljrt.lib");
        assert_eq!(format!("{:?}", Platform::Unix), "Unix");
    }

    #[test]
    fn override_file_wins_when_present() {
        let dir = tmp("override-hit");
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let file = dir.join("custom.a");
        std::fs::write(&file, b"archive").expect("tmp file");
        assert_eq!(
            find_archive(&dir, Some(&file), Platform::Unix),
            Ok(file.clone())
        );
        assert_eq!(
            find_archive(&dir, Some(&dir.join("missing.a")), Platform::Unix),
            Err(LinkError::ArchiveNotFound(format!(
                "runtime archive not found: {}",
                dir.join("missing.a").display()
            )))
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn exe_relative_lookup_per_platform() {
        let dir = tmp("exe-rel");
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let unix = dir.join("libcljrt.a");
        std::fs::write(&unix, b"archive").expect("tmp file");
        assert_eq!(find_archive(&dir, None, Platform::Unix), Ok(unix));
        assert_eq!(
            find_archive(&dir, None, Platform::Windows),
            Err(LinkError::ArchiveNotFound(format!(
                "runtime archive not found next to the binary ({}); pass --runtime-archive",
                dir.join("cljrt.lib").display()
            )))
        );
        assert_eq!(
            format!(
                "{}",
                find_archive(&dir, None, Platform::Windows).expect_err("missing")
            ),
            format!(
                "runtime archive not found next to the binary ({}); pass --runtime-archive",
                dir.join("cljrt.lib").display()
            )
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn link_commands_per_platform() {
        let obj = PathBuf::from("f.o");
        let archive = PathBuf::from("libcljrt.a");
        let out = PathBuf::from("f");
        assert_eq!(
            link_command(Platform::Unix, &obj, &archive, &out, None),
            LinkCommand {
                linker: "cc".to_owned(),
                args: vec![
                    "f.o".to_owned(),
                    "libcljrt.a".to_owned(),
                    "-o".to_owned(),
                    "f".to_owned()
                ],
            }
        );
        assert_eq!(
            link_command(Platform::Windows, &obj, &archive, &out, None),
            LinkCommand {
                linker: "link".to_owned(),
                args: vec![
                    "/NOLOGO".to_owned(),
                    "f.o".to_owned(),
                    "libcljrt.a".to_owned(),
                    "/OUT:f".to_owned(),
                    "kernel32.lib".to_owned(),
                    "advapi32.lib".to_owned(),
                    "userenv.lib".to_owned(),
                    "ws2_32.lib".to_owned(),
                    "bcrypt.lib".to_owned(),
                    "ntdll.lib".to_owned(),
                    "libcmt.lib".to_owned(),
                    "libvcruntime.lib".to_owned(),
                    "libucrt.lib".to_owned(),
                ],
            }
        );
        assert_eq!(
            link_command(Platform::Unix, &obj, &archive, &out, Some("true")).linker,
            "true"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sdkroot_prefers_explicit_env() {
        assert_eq!(
            super::choose_sdkroot(Some("/env.sdk"), Some("/xcrun.sdk")),
            Some("/env.sdk".to_owned())
        );
        assert_eq!(
            super::choose_sdkroot(None, Some("/xcrun.sdk")),
            Some("/xcrun.sdk".to_owned())
        );
        assert_eq!(super::choose_sdkroot(None, None), None);
        assert!(super::query_sdkroot().is_some_and(|p| !p.is_empty()));
    }

    #[test]
    fn run_link_maps_every_outcome() {
        #[cfg(unix)]
        let (ok, bad) = (
            LinkCommand {
                linker: "true".to_owned(),
                args: Vec::new(),
            },
            LinkCommand {
                linker: "false".to_owned(),
                args: Vec::new(),
            },
        );
        #[cfg(windows)]
        let (ok, bad) = (
            LinkCommand {
                linker: "cmd".to_owned(),
                args: vec!["/c".to_owned(), "exit".to_owned(), "0".to_owned()],
            },
            LinkCommand {
                linker: "cmd".to_owned(),
                args: vec!["/c".to_owned(), "exit".to_owned(), "1".to_owned()],
            },
        );
        assert_eq!(run_link(&ok), Ok(()));
        let text = format!("{}", run_link(&bad).expect_err("failing linker"));
        assert!(
            text.starts_with(&format!("{} exited with ", bad.linker)),
            "{text}"
        );
        #[cfg(unix)]
        let noisy = LinkCommand {
            linker: "sh".to_owned(),
            args: vec!["-c".to_owned(), "echo oops >&2; exit 3".to_owned()],
        };
        #[cfg(windows)]
        let noisy = LinkCommand {
            linker: "cmd".to_owned(),
            args: vec!["/c".to_owned(), "echo oops 1>&2 & exit 3".to_owned()],
        };
        let text = format!("{}", run_link(&noisy).expect_err("noisy linker"));
        assert!(text.contains("oops"), "{text}");
        // Stdout chatter counts too (MSVC `link` reports there),
        // alone and joined with stderr.
        #[cfg(unix)]
        let (out_only, both) = (
            LinkCommand {
                linker: "sh".to_owned(),
                args: vec!["-c".to_owned(), "echo oops; exit 3".to_owned()],
            },
            LinkCommand {
                linker: "sh".to_owned(),
                args: vec!["-c".to_owned(), "echo out; echo err >&2; exit 3".to_owned()],
            },
        );
        #[cfg(windows)]
        let (out_only, both) = (
            LinkCommand {
                linker: "cmd".to_owned(),
                args: vec!["/c".to_owned(), "echo oops & exit 3".to_owned()],
            },
            LinkCommand {
                linker: "cmd".to_owned(),
                args: vec![
                    "/c".to_owned(),
                    "echo out & echo err 1>&2 & exit 3".to_owned(),
                ],
            },
        );
        let text = format!("{}", run_link(&out_only).expect_err("stdout linker"));
        assert!(text.contains("oops"), "{text}");
        let text = format!("{}", run_link(&both).expect_err("both-stream linker"));
        assert!(text.contains("out"), "{text}");
        assert!(text.contains("err"), "{text}");
        let missing = LinkCommand {
            linker: "clj-aot-definitely-not-a-linker".to_owned(),
            args: Vec::new(),
        };
        assert_eq!(
            run_link(&missing),
            Err(LinkError::LinkFailed(
                "linker not found: clj-aot-definitely-not-a-linker".to_owned()
            ))
        );
        // A directory is not executable: the non-NotFound IO arm.
        let dir = LinkCommand {
            linker: tmp("is-a-dir").to_string_lossy().into_owned(),
            args: Vec::new(),
        };
        std::fs::create_dir_all(tmp("is-a-dir")).expect("tmp dir");
        let text = format!("{}", run_link(&dir).expect_err("dir linker"));
        assert!(text.starts_with(&format!("{}: ", dir.linker)), "{text}");
        assert!(!text.contains("not found"), "{text}");
        std::fs::remove_dir_all(tmp("is-a-dir")).expect("cleanup");
    }
}
