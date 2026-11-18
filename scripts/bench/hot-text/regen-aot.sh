#!/usr/bin/env bash
# Regenerate the AOT hot-text order (rust/hot-text-aot.ld) for one
# revision (#271). Linux AOT links pack the functions the order lists
# into one `.text.hot` section, so a run faults in a few 64 KiB windows
# of the runtime's text instead of most of it. The order names runtime
# functions, so it goes stale whenever runtime code moves (renamed or
# new functions fall back to the cold `.text`): CI regenerates it for
# every AOT build it ships or measures (rust-release.yml, the `hot-text`
# job of h2h-cpu-mem.yml).
#
# Training set: this directory's workloads/*.clj (empty stdin) and
# aot-workloads/*.clj (stdin from NAME.in). These are small generic
# programs, never the benchmark snippets or their inputs, so the order
# follows the runtime's own hot paths, whatever program links it.
#
# Profiles: callgrind of each workload compiled against the revision's
# runtime, linked with an empty order (callgrind attributes the
# `.text.hot` output section to an unnamed object, so an order in place
# would hide its own functions); the startup set is the `trivial`
# workload's. aot_order.py lays them out.
#
# Needs Linux, valgrind, python3, and LLVM for `clojure-rs build`.
#
# usage: regen-aot.sh RUST_WORKSPACE [OUT.ld]
#   OUT.ld defaults to RUST_WORKSPACE/hot-text-aot.ld (in place).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
ws=$(cd "$1" && pwd)
out=${2:-$ws/hot-text-aot.ld}
order=$ws/hot-text-aot.ld
work=$(mktemp -d)
cp "$order" "$work/previous.ld"
# Restore the revision's order on any exit (the build below links with
# a blank one).
trap 'cp "$work/previous.ld" "$order"; rm -rf "$work"' EXIT
printf '%s\n' '/* Placeholder while scripts/bench/hot-text/regen-aot.sh profiles. */' \
  'SECTIONS' '{' '  .text.hot :' '  {' '  }' '}' 'INSERT BEFORE .text;' > "$order"

echo "== build $ws (blank order)"
(cd "$ws" && cargo build --release --locked -p clojure-rs \
  && cargo rustc --release --locked -p clj-rt --crate-type staticlib) > "$work/build.log" 2>&1 \
  || { tail -30 "$work/build.log" >&2; exit 1; }
compiler=$ws/target/release/clojure-rs

echo "== profile the workloads"
mkdir -p "$work/fns"
for w in "$here"/workloads/*.clj "$here"/aot-workloads/*.clj; do
  name=$(basename "$w" .clj)
  input=${w%.clj}.in
  [ -f "$input" ] || input=/dev/null
  "$compiler" build "$w" -o "$work/prog" > /dev/null 2> "$work/$name.err" || {
    tail -n 5 "$work/$name.err" >&2
    echo "regen-aot: workload $name failed to compile" >&2
    exit 1
  }
  valgrind --tool=callgrind --demangle=no --dump-instr=no \
    --callgrind-out-file="$work/$name.cg" "$work/prog" < "$input" > /dev/null 2>> "$work/$name.err" || {
    tail -n 5 "$work/$name.err" >&2
    echo "regen-aot: workload $name failed" >&2
    exit 1
  }
  python3 "$here/aot_order.py" callgrind "$work/$name.cg" > "$work/fns/$name.txt"
  echo "   $name: $(wc -l < "$work/fns/$name.txt") functions"
  rm -f "$work/$name.cg"
done
[ -s "$work/fns/trivial.txt" ] || { echo "regen-aot: no startup profile (workloads/trivial.clj)" >&2; exit 1; }

echo "== order"
nm -S --size-sort -t d "$ws/target/release/libcljrt.a" > "$work/sizes.txt" 2> /dev/null || true
python3 "$here/aot_order.py" order --fns "$work/fns" --startup "$work/fns/trivial.txt" \
  --sizes "$work/sizes.txt" --previous "$work/previous.ld" --out "$work/new.ld"
# Restore the revision's order, then write OUT (which may be that
# order itself), and drop the restoring trap.
cp "$work/previous.ld" "$order"
cp "$work/new.ld" "$out"
trap 'rm -rf "$work"' EXIT
