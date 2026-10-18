#!/bin/bash
# Copyright (c) Rich Hickey. All rights reserved.
# The use and distribution terms for this software are covered by the
# Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
# which can be found in the file epl-v10.html at the root of this distribution.
# By using this software in any fashion, you are agreeing to be bound by
# the terms of this license.
# You must not remove this notice, or any other, from this software.
# Regenerate the S2 differential corpus expectations with the JVM oracle.
# Usage: ./regen.sh [outdir]  (default: /tmp/t1174-corpus)
# Requires: target/clojure-1.13.0-master-SNAPSHOT.jar (do not rebuild
#   unless missing) plus the spec/test.check jars (same classpath as
#   docs/t1174/probes/probe.sh).
# Writes: expectations.edn + files.edn (checked in; :ours snapshots and
#   :exclude marks carried forward), raw dumps + cross-check report
#   under outdir.
set -u
cd "$(dirname "$0")/../../../../.."
OUT=${1:-/tmp/t1174-corpus}
mkdir -p "$OUT"
JAR=target/clojure-1.13.0-master-SNAPSHOT.jar
if [ ! -f "$JAR" ]; then echo "missing $JAR" >&2; exit 1; fi
CP="$JAR:$HOME/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:$HOME/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar:$HOME/.m2/repository/org/clojure/test.check/1.1.3/test.check-1.1.3.jar"
export OUT
echo "== driver (DUMP=1 for the cross-check) =="
DUMP=1 java -cp "$CP" clojure.main rust/crates/clj-reader/tests/corpus/driver.clj 2>"$OUT/driver.err" | tee "$OUT/driver.log"
code=${PIPESTATUS[0]}
if [ "$code" != 0 ]; then echo "driver failed ($code); see $OUT/driver.err"; exit 1; fi
echo "== cross-check vs docs/t1174/probes outputs =="
java -cp "$CP" clojure.main rust/crates/clj-reader/tests/corpus/check.clj "$OUT/raw-dump.edn" 2>&1 | tee "$OUT/check.log"
echo "outputs in $OUT"
