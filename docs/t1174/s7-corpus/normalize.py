#!/usr/bin/env python3
"""Normalize a piped-JVM REPL transcript for comparison with ours.

Strips the ambient differences pinned in s7-pins.md P6 (banner,
`<ns>=> ` prompts) plus the JVM's non-deterministic `Full report
at:` lines. Value lines must then match `expected/*.ours.out`
byte-for-byte (01/02/04); 03-errors is ours-pinned only (error text
is a documented format divergence — see MANIFEST.md).
Usage: normalize.py expected/01-basics.jvm.out | diff - expected/01-basics.ours.out
"""
import re
import sys

PROMPT = re.compile(r"^(?:[\w.!?*+<>=/$-]+)=> ?")


def main() -> None:
    with open(sys.argv[1], encoding="utf-8") as f:
        for line in f:
            if line.startswith("Clojure "):
                continue
            if line.startswith("Full report at:"):
                continue
            if PROMPT.match(line) and PROMPT.sub("", line, count=1).strip() == "":
                # A lone prompt (`user=> ` with no result): the
                # trailing prompt and any prompt echoing blank input.
                continue
            sys.stdout.write(PROMPT.sub("", line, count=1))


main()
