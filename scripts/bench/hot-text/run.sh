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
# NAME.in) compiled with `clojure-rs build`, profiled as native
# binaries, laid out by aot_order.py. clj-aot embeds that script for
# its Linux AOT links; CI regenerates it for every AOT build it ships
# or measures, so the committed copy only serves local builds.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
if [ "${1:-}" = "--aot" ]; then
  shift
  exec bash "$here/regen-aot.sh" "$root/rust" "${1:-$root/rust/hot-text-aot.ld}"
fi
profiles=$(mktemp -d)
trap 'rm -rf "$profiles"' EXIT

# Profile binaries linked with an empty order: callgrind attributes the
# `.text.hot` output section to an unnamed object, not the binary, so an
# order in place would hide its own hot functions from the regeneration.
# `blank_order ORDER` swaps in the placeholder (restored on any exit);
# `finish_order ORDER OUT` restores ORDER, then writes the new order to
# OUT (which may be ORDER itself).
blank_order() {
  cp "$1" "$profiles/order.saved"
  trap 'cp "$profiles/order.saved" "'"$1"'"; rm -rf "$profiles"' EXIT
  printf '%s\n' '/* Placeholder: regenerate with scripts/bench/hot-text/run.sh' \
    '   (Linux; see scripts/bench/hot-text/hot_text.py). Do not edit. */' \
    'SECTIONS' '{' '  .text.hot :' '  {' '  }' '}' 'INSERT BEFORE .text;' > "$1"
}
finish_order() {
  cp "$profiles/order.saved" "$1"
  trap 'rm -rf "$profiles"' EXIT
  cp "$profiles/new.ld" "$2"
}
out=${1:-$root/rust/hot-text.ld}
blank_order "$root/rust/hot-text.ld"
(cd "$root/rust" && cargo build --release --locked --no-default-features)
bin=$root/rust/target/release/clojure-rs
for w in "$here"/workloads/*.clj; do
  name=$(basename "$w" .clj)
  valgrind --tool=callgrind --demangle=no --dump-instr=no \
    --callgrind-out-file="$profiles/$name.cg" "$bin" run "$w" > /dev/null 2> "$profiles/$name.err" || {
    tail -n 5 "$profiles/$name.err" >&2
    echo "hot-text: workload $name failed" >&2
    exit 1
  }
done
python3 "$here/hot_text.py" "$bin" "$profiles/new.ld" "$profiles"/*.cg
finish_order "$root/rust/hot-text.ld" "$out"
