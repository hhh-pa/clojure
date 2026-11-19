#!/usr/bin/env bash
# Regenerate rust/hot-text.ld (#226): profile the training workloads
# under callgrind and list the functions they execute, so a Linux
# release link packs them into one `.text.hot` section (see
# hot_text.py for why that cuts peak RSS).
#
# Needs Linux, valgrind and python3. Rerun when the hot paths move
# (new natives, a reorganized evaluator): functions missing from the
# script still link, they just stay in the cold `.text`.
#
# usage: scripts/bench/hot-text/run.sh [--aot] [OUT.ld]
#
# --aot regenerates rust/hot-text-aot.ld instead (#271), through
# regen-aot.sh: the same workloads plus aot-workloads/ (stdin from each
# NAME.in) and the compiler corpus compiled with `clojure-rs build`,
# profiled as native binaries, laid out by hot_text.py. Without --aot,
# delegates to regen-run.sh for the interpreter's rust/hot-text.ld. CI
# regenerates both orders for every build it ships or measures, so the
# committed copies only serve local builds.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
if [ "${1:-}" = "--aot" ]; then
  shift
  exec bash "$here/regen-aot.sh" "$root/rust" "${1:-$root/rust/hot-text-aot.ld}"
fi
exec bash "$here/regen-run.sh" "$root/rust" "${1:-$root/rust/hot-text.ld}"
