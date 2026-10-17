# Quality-gates image for the `quality gates` job (.github/workflows/rust.yml).
#
# Vendored from the arcanist repo and adapted: LLVM 22.1.8 layer for the
# clj-aot (inkwell) build, and a workspace-aware warm layer (this repo's
# rust/ is a Cargo workspace, not a single package).
#
# Cached on Docker Hub content-addressed by dependency inputs (see the job's
# key): a cache hit pulls this image and runs the gates inside it; a miss
# rebuilds here and pushes, then runs the gates. Pinned tool versions come
# from the Go gates binary via --provision-only, so the image and the run
# cannot disagree; only the base stable toolchain floats (same as the
# dtolnay stable toolchain this replaces, lagged by rebuilds).

# The gates binary for provisioning (siblings are never called by
# --provision-only, so only it is built here).
ARG GO_VERSION=1.27
FROM golang:${GO_VERSION}-trixie AS gobuild
WORKDIR /src
COPY scripts/ci/go/ ./go/
RUN cd go && CGO_ENABLED=0 go build -o /tools/rust-quality-gates ./cmd/rust-quality-gates

FROM rust:slim-trixie

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        gnupg \
        libssl-dev \
        lsb-release \
        pkg-config \
        python3 \
        wget \
    && rm -rf /var/lib/apt/lists/*

# The CI workspace mounts at /work owned by the host user; mark it safe so
# git works as root inside the container (else "dubious ownership" errors).
# System level: some tests override HOME, which would hide a global entry.
RUN git config --system --add safe.directory /work

# Pinned LLVM for clj-aot (inkwell needs the exact major) via
# apt.llvm.org, same major the setup-llvm action installs in the
# other jobs. llvm.sh runs as root here (no sudo in the image) and
# knows trixie explicitly.
ARG LLVM_VERSION=22
ENV LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
RUN wget -q https://apt.llvm.org/llvm.sh \
    && chmod +x llvm.sh \
    && ./llvm.sh "${LLVM_VERSION}" \
    && apt-get install -y llvm-22-dev libpolly-22-dev libffi-dev libzstd-dev zlib1g-dev libxml2-dev libtinfo-dev libedit-dev \
    && rm llvm.sh \
    && rm -rf /var/lib/apt/lists/* \
    && /usr/lib/llvm-22/bin/llvm-config --version | grep -q '^22\.1\.'
ENV PATH=/usr/lib/llvm-22/bin:$PATH

# Provision every pinned gate tool. Prebuilt downloads that cannot execute
# on this libc compile here once (see ensure_tool), so runtime provisioning
# inside CI is always a fast no-op.
COPY --from=gobuild /tools/rust-quality-gates /tmp/gates-bin/rust-quality-gates
RUN mkdir -p /tmp/prov-work \
    && /tmp/gates-bin/rust-quality-gates /tmp/prov-work --provision-only \
    && rm -rf /tmp/prov-work /tmp/gates-bin /usr/local/cargo/registry

# Prebuilt dependency tree ("builder" layer): compiling the workspace's
# dependencies once here means gate runs rebuild clojure-rs itself only.
# CARGO_TARGET_DIR lives outside the /work mount so the prebuilt tree
# survives it; the gates script already honors CARGO_TARGET_DIR, and its
# llvm-cov seed copies this debug tree so coverage deps stay fresh too.
# The nightly udeps tree warms with the script's exact command and target
# dir (pin extracted from the script: no drift). The warm stubs must COMPILE
# as normal targets: an empty main.rs passes `cargo test --no-run` (the test
# harness supplies main) but fails udeps with E0601, hence `fn main() {}`.
# A nonzero udeps exit on the stub is EXPECTED (the stub uses no
# dependencies, so every dep reports unused): only a genuine build failure
# ("could not compile" in the log, or zero fingerprints) fails the image,
# otherwise the log stays quiet and the verdict stays a runtime one.
# Registry sources stay extracted: cargo fingerprints compare mtimes, and
# re-extraction would stale every prebuilt unit.
ENV CARGO_TARGET_DIR=/opt/target
COPY rust/Cargo.toml rust/Cargo.lock /tmp/warm/rust/
COPY rust/crates/clj-aot/Cargo.toml /tmp/warm/rust/crates/clj-aot/Cargo.toml
COPY rust/crates/clj-core/Cargo.toml /tmp/warm/rust/crates/clj-core/Cargo.toml
COPY rust/crates/clj-eval/Cargo.toml /tmp/warm/rust/crates/clj-eval/Cargo.toml
COPY rust/crates/clj-reader/Cargo.toml /tmp/warm/rust/crates/clj-reader/Cargo.toml
COPY rust/crates/clj-repl/Cargo.toml /tmp/warm/rust/crates/clj-repl/Cargo.toml
COPY rust/crates/clj-value/Cargo.toml /tmp/warm/rust/crates/clj-value/Cargo.toml
COPY scripts/ci/go/cmd/rust-quality-gates/main.go /tmp/warm/pins.go
RUN mkdir -p /tmp/warm/rust/src/bin/clojure-rs \
    && printf 'fn main() {}\n' > /tmp/warm/rust/src/bin/clojure-rs/main.rs \
    && for c in clj-aot clj-core clj-eval clj-reader clj-repl clj-value; do \
        mkdir -p /tmp/warm/rust/crates/$c/src; \
        printf '' > /tmp/warm/rust/crates/$c/src/lib.rs; \
    done \
    && cd /tmp/warm/rust && cargo fetch \
    && cargo test --workspace --no-run \
    && NIGHTLY=$(grep -E '^\s*UDEPSNightly = ' /tmp/warm/pins.go | cut -d'"' -f2) \
    && [ -n "$NIGHTLY" ] \
    && UDEPS_LOG=/tmp/warm/udeps.log \
    && { NIGHTLY_TARGET="${CARGO_TARGET_DIR}/nightly"; \
        CARGO_TARGET_DIR="$NIGHTLY_TARGET" cargo +"$NIGHTLY" udeps --all-targets >"$UDEPS_LOG" 2>&1; \
        STATUS=$?; \
        if [ $STATUS -ne 0 ] \
            && { grep -q "could not compile" "$UDEPS_LOG" \
                || [ -z "$(ls -A "$NIGHTLY_TARGET/debug/.fingerprint" 2>/dev/null)" ]; }; then \
            echo "udeps warm: nightly build FAILED:"; cat "$UDEPS_LOG"; exit 1; \
        fi; \
        echo "udeps warm: nightly tree prebuilt (udeps exit $STATUS; unused-deps verdict deferred to runtime)"; } \
    && rm -rf /tmp/warm

WORKDIR /work
