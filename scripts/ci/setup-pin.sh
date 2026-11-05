#!/bin/bash
# Provision Intel Pin + the vendored inscount tool for the h2h gate.
#
# Idempotent: when PIN_DIR already holds a kit and a built tool
# (the workflow caches PIN_DIR across runs), it just re-exports
# the paths. Otherwise it downloads the kit from Intel, verifies
# its sha256, extracts it, and builds
# scripts/ci/pintool/inscount.cpp against it.
#
# Outputs (stdout for logs, $GITHUB_ENV when present):
#   PIN_BIN   absolute path to the pin launcher
#   PIN_TOOL  absolute path to obj-intel64/inscount.so
set -euo pipefail

PIN_URL="${PIN_URL:-https://software.intel.com/sites/landingpage/pintool/downloads/pin-external-4.4-99977-g54fbb8814-gcc-linux.tar.gz}"
PIN_SHA256="${PIN_SHA256:-3138183a5d2b51ea5c8f29edd148b67ac9c249e2c510cc0e9e2a5880f523a34c}"
PIN_TOPDIR="pin-external-4.4-99977-g54fbb8814-gcc-linux"
PIN_DIR="${PIN_DIR:-$HOME/pin}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TOOL_SRC="$SCRIPT_DIR/pintool"

KIT_DIR="$PIN_DIR/$PIN_TOPDIR"
PIN_BIN="$KIT_DIR/pin"
TOOL_BUILD="$PIN_DIR/toolbuild"
PIN_TOOL="$TOOL_BUILD/obj-intel64/inscount.so"

need_tool() {
  [ ! -x "$PIN_BIN" ] || [ ! -f "$PIN_TOOL" ]
}

if need_tool; then
  command -v g++ >/dev/null || { echo "setup-pin: g++ is required to build the inscount tool" >&2; exit 1; }
  command -v curl >/dev/null || { echo "setup-pin: curl is required to fetch the Pin kit" >&2; exit 1; }
  [ -f "$TOOL_SRC/inscount.cpp" ] || { echo "setup-pin: tool sources missing at $TOOL_SRC" >&2; exit 1; }
  mkdir -p "$PIN_DIR"
  if [ ! -x "$PIN_BIN" ]; then
    echo "setup-pin: downloading Pin 4.4 kit..." >&2
    curl -sSfL --retry 3 --max-time 600 -o "$PIN_DIR/pin-kit.tar.gz" "$PIN_URL"
    echo "$PIN_SHA256  $PIN_DIR/pin-kit.tar.gz" | sha256sum -c - || {
      echo "setup-pin: kit checksum mismatch" >&2; exit 1
    }
    tar xzf "$PIN_DIR/pin-kit.tar.gz" -C "$PIN_DIR"
    rm -f "$PIN_DIR/pin-kit.tar.gz"
  fi
  echo "setup-pin: building inscount tool..." >&2
  rm -rf "$TOOL_BUILD"
  mkdir -p "$TOOL_BUILD"
  cp "$TOOL_SRC/inscount.cpp" "$TOOL_SRC/makefile" "$TOOL_SRC/makefile.rules" "$TOOL_BUILD/"
  make -C "$TOOL_BUILD" PIN_ROOT="$KIT_DIR" >/tmp/pin-tool-build.log 2>&1 || {
    tail -n 20 /tmp/pin-tool-build.log >&2; echo "setup-pin: tool build failed" >&2; exit 1
  }
fi

[ -x "$PIN_BIN" ] || { echo "setup-pin: pin launcher missing at $PIN_BIN" >&2; exit 1; }
[ -f "$PIN_TOOL" ] || { echo "setup-pin: tool missing at $PIN_TOOL" >&2; exit 1; }
"$PIN_BIN" -version >/dev/null 2>&1 || { echo "setup-pin: pin launcher does not run" >&2; exit 1; }

echo "PIN_BIN=$PIN_BIN"
echo "PIN_TOOL=$PIN_TOOL"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "PIN_BIN=$PIN_BIN" >>"$GITHUB_ENV"
  echo "PIN_TOOL=$PIN_TOOL" >>"$GITHUB_ENV"
fi
