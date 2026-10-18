# clojure-rs: native Clojure in Rust

Rust workspace for [T1174](../docs/t1174/specification.md): a native
Clojure runtime, REPL, and LLVM AOT compiler. The JVM build at the
repo root is untouched; this workspace is the native port.

## Prerequisites

- Stable Rust, MSRV 1.85 (`rustup toolchain install stable`).
- LLVM 22 for the default `aot` feature. Without LLVM, build with
  `cargo build --no-default-features` (interpreter crates only).

## LLVM setup (the `aot` feature)

inkwell pins one LLVM major: 22 (feature `llvm22-1`). Point llvm-sys
at it with `LLVM_SYS_221_PREFIX`:

- macOS with Homebrew: `brew install llvm@22`, then
  `export LLVM_SYS_221_PREFIX=$(brew --prefix llvm@22)`.
- macOS without Homebrew: pour the `llvm@22` bottle by hand. Pick
  the bottle for your macOS version from
  `https://formulae.brew.sh/api/formula/llvm@22.json` (`arm64_tahoe`
  / `arm64_sequoia` / newer), download the blob with an anonymous
  `ghcr.io` pull token, and extract it:
  ```sh
  TOKEN=$(curl -s 'https://ghcr.io/token?scope=repository:homebrew/core/llvm/22:pull' \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['token'])")
  curl -sL -H "Authorization: Bearer $TOKEN" -o llvm22-bottle.tar.gz '<blob url>'
  mkdir -p ~/.local/brew && tar -xzf llvm22-bottle.tar.gz -C ~/.local/brew
  ```
  Then build a prefix farm (llvm-sys looks for `bin/llvm-config`
  under the prefix; the farm also lets `-lzstd` resolve — build
  zstd from source or pour its bottle the same way):
  ```sh
  farm=~/.local/llvm-22-local src=~/.local/brew/llvm@22/22.1.8
  mkdir -p "$farm/bin"
  for f in "$src"/bin/*; do ln -sf "$f" "$farm/bin/"; done
  ln -sfn "$src/lib" "$farm/lib"   # if needed for -L flags
  export LLVM_SYS_221_PREFIX="$farm"
  export RUSTFLAGS="-L <dir holding libzstd.a>"
  ```
- Linux: `wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh &&
  sudo ./llvm.sh 22 && sudo apt-get install -y llvm-22-dev`,
  then `export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`.
- Windows: download `clang+llvm-22.1.8-x86_64-pc-windows-msvc.tar.xz`
  from the LLVM project releases (the choco package omits
  llvm-config.exe, which llvm-sys requires), extract it, and set
  `LLVM_SYS_221_PREFIX` to the extracted dir (plus its `bin` on
  `PATH` so test binaries find `LLVM-C.dll` at runtime).

## Build and test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --release --locked
```

## Layout

- `src/bin/clojure-rs/` — the `clojure-rs` binary (CLI shell).
- `crates/clj-reader/` — reader + printer (slice S2).
- `crates/clj-value/` — value model + persistent collections (S3).
- `crates/clj-eval/` — analyzer + interpreter (S4).
- `crates/clj-core/` — namespaces/vars/macros + core subset (S5/S6).
- `crates/clj-repl/` — REPL loop (S7).
- `crates/clj-aot/` — LLVM backend via inkwell (S8; linked from S1).
- `tests/` — integration suites (`common/` harness, `fixtures/`).
- `core-coverage.md` — normative core var coverage list.
- `deny.toml` — license/bans/sources policy for CI.
