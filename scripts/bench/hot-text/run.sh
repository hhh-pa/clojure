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
# --aot regenerates rust/hot-text-aot.ld instead (#271): the same
# workloads plus aot-workloads/ (stdin from each NAME.in) compiled
# with `clojure-rs build`, profiled as native binaries. clj-aot embeds
# that script for its Linux static links.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
if [ "${1:-}" = "--aot" ]; then
  shift
  out=${1:-$root/rust/hot-text-aot.ld}
  (cd "$root/rust" && cargo build --release --locked -p clojure-rs \
    && cargo rustc --release --locked -p clj-rt --crate-type staticlib)
  bin=$root/rust/target/release/clojure-rs
  profiles=$(mktemp -d)
  trap 'rm -rf "$profiles"' EXIT
  # One path for every compiled workload: hot_text.py keeps the
  # functions of the profiled object by its basename.
  prog=$profiles/prog
  for w in "$here"/workloads/*.clj "$here"/aot-workloads/*.clj; do
    name=$(basename "$w" .clj)
    input=${w%.clj}.in
    [ -f "$input" ] || input=/dev/null
    "$bin" build "$w" -o "$prog" > /dev/null 2> "$profiles/$name.err" || {
      tail -n 5 "$profiles/$name.err" >&2
      echo "hot-text: workload $name failed to compile" >&2
      exit 1
    }
    valgrind --tool=callgrind --demangle=no --dump-instr=no \
      --callgrind-out-file="$profiles/$name.cg" "$prog" < "$input" > /dev/null 2>> "$profiles/$name.err" || {
      tail -n 5 "$profiles/$name.err" >&2
      echo "hot-text: workload $name failed" >&2
      exit 1
    }
  done
  python3 "$here/hot_text.py" "$prog" "$out" "$profiles"/*.cg
  exit 0
fi
out=${1:-$root/rust/hot-text.ld}
(cd "$root/rust" && cargo build --release --locked --no-default-features)
bin=$root/rust/target/release/clojure-rs
profiles=$(mktemp -d)
trap 'rm -rf "$profiles"' EXIT
for w in "$here"/workloads/*.clj; do
  name=$(basename "$w" .clj)
  valgrind --tool=callgrind --demangle=no --dump-instr=no \
    --callgrind-out-file="$profiles/$name.cg" "$bin" run "$w" > /dev/null 2> "$profiles/$name.err" || {
    tail -n 5 "$profiles/$name.err" >&2
    echo "hot-text: workload $name failed" >&2
    exit 1
  }
done
python3 "$here/hot_text.py" "$bin" "$out" "$profiles"/*.cg
