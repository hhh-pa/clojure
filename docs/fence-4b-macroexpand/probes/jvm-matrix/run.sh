#!/bin/sh
# Reproduce probes/monitor-matrix-jvms.txt (fence 4b B3 evidence).
# Needs: a JDK (javac+java) and org.ow2.asm:asm:9.8 from Maven Central.
# Run: sh run.sh [/path/to/java]   (default: java on PATH)
set -eu
JAVA="${1:-java}"
ASM_URL='https://repo1.maven.org/maven2/org/ow2/asm/asm/9.8/asm-9.8.jar'
[ -f asm-9.8.jar ] || curl -sSL "$ASM_URL" -o asm-9.8.jar
JAVAC="$(dirname "$("$JAVA" -XshowSettings:properties -version 2>&1 | grep 'java.home' | awk '{print $3}')")/bin/javac"
[ -x "$JAVAC" ] || JAVAC=javac
"$JAVAC" -cp asm-9.8.jar GenModes.java
"$JAVA" -cp .:asm-9.8.jar GenModes
"$JAVAC" RunModes.java
echo '== default =='
"$JAVA" -cp . RunModes
echo '== -Xint =='
"$JAVA" -Xint -cp . RunModes
