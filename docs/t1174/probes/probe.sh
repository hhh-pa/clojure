#!/bin/bash
# Reproduce the S2 oracle probes against the locally built JVM jar.
# Usage: ./probe.sh [outdir]  (default: /tmp/t1174-repro)
# Requires: target/clojure-1.13.0-master-SNAPSHOT.jar
#   (mvn -ntp -B -Plocal -Dmaven.test.skip=true package)
set -u
cd "$(dirname "$0")/../../.."
OUT=${1:-/tmp/t1174-repro}
mkdir -p "$OUT"
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:$HOME/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:$HOME/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar:$HOME/.m2/repository/org/clojure/test.check/1.1.3/test.check-1.1.3.jar"
run() { # name, file, expect-exit
  java -cp "$CP" clojure.main "docs/t1174/probes/$2" > "$OUT/$1.out" 2>&1
  code=$?
  if [ "$code" != "$3" ]; then echo "UNEXPECTED exit $code (want $3): $1"; else echo "ok ($code): $1"; fi
}
run probe probe.clj 0
cat docs/t1174/probes/probe.clj docs/t1174/probes/probe2.clj > "$OUT/probe-all.clj"
java -cp "$CP" clojure.main "$OUT/probe-all.clj" > "$OUT/probe-all.out" 2>&1
echo "ok ($?): probe-all (want nonzero: probe2 ends in a deliberate reader error)"
run probe3 probe3.clj 1
run probe4 probe4.clj 0
run probe5 probe5.clj 0
run probe6 probe6.clj 0
run probe7 probe7.clj 0
run probe8 probe8.clj 0
run probe9 probe9.clj 0
run probe10 probe10.clj 0
run probe11 probe11.clj 0
echo "outputs in $OUT (gensym ids, @hex suffixes, and inst dates vary run to run)"
