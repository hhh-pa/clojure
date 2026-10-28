// Command lcov-merge sums per-shard LCOV files into one merged report
// (CI matrix sharding: each quality-gates shard covers a test
// partition; the merge job reunites them before the floor/patch
// gates). Counts add with saturation, BRDA taken merges the lcov
// way, summaries are recomputed.
package main

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/lcov"
)

const usage = "Usage: lcov-merge <workspace-dir> <out.lcov> <in1.lcov> [in2.lcov ...]\n"

func run(argv []string) int {
	if len(argv) < 3 {
		fmt.Print(usage)
		return 2
	}
	workspace, err := filepath.Abs(argv[0])
	if err != nil {
		fmt.Fprintf(os.Stderr, "lcov-merge: %s\n", err)
		return 1
	}
	var all []lcov.SFRecord
	for _, in := range argv[2:] {
		data, err := os.ReadFile(in)
		if err != nil {
			fmt.Fprintf(os.Stderr, "lcov-merge: %s\n", err)
			return 1
		}
		records, _ := lcov.ParseRecords(lcov.SanitizeUTF8(string(data)))
		all = append(all, records...)
	}
	merged := lcov.Canonicalize(all, workspace)
	if len(merged) == 0 {
		fmt.Fprintf(os.Stderr, "lcov-merge: no records under %s\n", workspace)
		return 1
	}
	out := argv[1]
	if dir := filepath.Dir(out); dir != "" {
		if err := os.MkdirAll(dir, 0o777); err != nil {
			fmt.Fprintf(os.Stderr, "lcov-merge: %s\n", err)
			return 1
		}
	}
	if err := os.WriteFile(out, []byte(lcov.Emit(merged)), 0o644); err != nil {
		fmt.Fprintf(os.Stderr, "lcov-merge: %s\n", err)
		return 1
	}
	fmt.Printf("lcov-merge: %d files from %d inputs -> %s\n", len(merged), len(argv)-2, out)
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
