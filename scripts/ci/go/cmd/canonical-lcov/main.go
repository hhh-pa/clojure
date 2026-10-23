// Command canonical-lcov reproduces CI's canonical LCOV locally (T1174;
// dev-only helper).
//
// CI's published lcov.info is NOT cargo-llvm-cov's output:
// rust-quality-gates re-exports the same profile data with raw
// `llvm-cov export` and merges twin path spellings (`tests/../src` ->
// `src`), recovering genuinely-executed lines cargo's filter drops (see
// the T1174 coverage note in rust-quality-gates). A plain local
// `cargo llvm-cov` run therefore under-reports (72% vs CI's 93% on the
// same suite).
//
// This tool replicates that pipeline over an existing instrumented run:
//
//	cargo llvm-cov nextest --run-ignored all --no-fail-fast   # or via gates
//	go run ./scripts/ci/go/cmd/canonical-lcov [out.lcov]
//
// and prints the floor-style line % over the canonical record set — the
// same number coverage-floor and codecov/project compute in CI.
//
// Run the test suite with an isolated HOME (the repo's hermetic pattern);
// several tests read the user config layer and fail or mis-cover
// otherwise. Unix-only in practice: like its Python predecessor, object
// discovery skips dotted names, so Windows `.exe` test objects are not
// found. Stdlib only.
package main

import (
	"fmt"
	"maps"
	"math"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
)

var excluded = []string{
	"/tests/",
	"/benches/",
	"/examples/",
	"/target/",
	"/.cargo/",
	"/rustup/",
	"/rustc/",
}

const clamp = uint64(1) << 62
const saturate = math.MaxInt64

func run(cmd *exec.Cmd) string {
	out, err := cmd.Output()
	if err != nil {
		msg := ""
		if exit, ok := err.(*exec.ExitError); ok {
			msg = string(exit.Stderr)
		} else {
			msg = err.Error()
		}
		if len(msg) > 2000 {
			msg = msg[len(msg)-2000:]
		}
		shown := cmd.Args
		if len(shown) > 4 {
			shown = shown[:4]
		}
		fmt.Fprintf(os.Stderr, "FAILED: %s\n %s\n", strings.Join(shown, " "), msg)
		os.Exit(1)
	}
	return string(out)
}

func main() {
	topOut := run(exec.Command("git", "rev-parse", "--show-toplevel"))
	top := strings.TrimSpace(topOut)
	rust := filepath.Join(top, "rust")
	covtarget := filepath.Join(rust, "target", "llvm-cov-target")
	sysroot := strings.TrimSpace(run(exec.Command("rustc", "--print", "sysroot")))
	found, err := filepath.Glob(filepath.Join(sysroot, "lib", "rustlib", "*", "bin", "llvm-cov"))
	if err != nil || len(found) == 0 {
		fmt.Fprintln(os.Stderr, "llvm-cov not found under rustc sysroot (need llvm-tools-preview)")
		os.Exit(1)
	}
	slices.Sort(found)
	llvmBin := filepath.Dir(found[0])
	out := filepath.Join(rust, "target", "llvm-cov", "canon.lcov.info")
	if len(os.Args) > 1 {
		out = os.Args[1]
	}
	if err := os.MkdirAll(filepath.Dir(out), 0o755); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	entries, err := os.ReadDir(covtarget)
	if err != nil {
		fmt.Fprintf(os.Stderr, "no profraw files under %s (run an instrumented suite first)\n", covtarget)
		os.Exit(1)
	}
	var profraws []string
	for _, entry := range entries {
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".profraw") {
			profraws = append(profraws, filepath.Join(covtarget, entry.Name()))
		}
	}
	if len(profraws) == 0 {
		fmt.Fprintf(os.Stderr, "no profraw files under %s (run an instrumented suite first)\n", covtarget)
		os.Exit(1)
	}
	slices.Sort(profraws)
	profdata := filepath.Join(covtarget, "canonical.profdata")
	fmt.Printf("merging %d profraws...\n", len(profraws))
	mergeArgs := []string{"merge", "-sparse"}
	mergeArgs = append(mergeArgs, profraws...)
	mergeArgs = append(mergeArgs, "-o", profdata)
	mergeCmd := exec.Command(filepath.Join(llvmBin, "llvm-profdata"), mergeArgs...)
	mergeCmd.Dir = rust
	run(mergeCmd)

	var objs []string
	for _, sub := range []string{"debug", "release"} {
		deps := filepath.Join(covtarget, sub, "deps")
		depEntries, err := os.ReadDir(deps)
		if err != nil {
			continue
		}
		for _, entry := range depEntries {
			if strings.Contains(entry.Name(), ".") {
				continue
			}
			full := filepath.Join(deps, entry.Name())
			info, err := os.Stat(full)
			if err != nil || info.IsDir() || info.Mode()&0o111 == 0 {
				continue
			}
			objs = append(objs, full)
		}
	}
	if len(objs) == 0 {
		fmt.Fprintf(os.Stderr, "no test objects under %s\n", covtarget)
		os.Exit(1)
	}
	fmt.Printf("exporting %d objects...\n", len(objs))
	exportArgs := []string{"export", "-instr-profile=" + profdata, "--format=lcov"}
	for _, obj := range objs {
		exportArgs = append(exportArgs, "-object", obj)
	}
	exportCmd := exec.Command(filepath.Join(llvmBin, "llvm-cov"), exportArgs...)
	exportCmd.Dir = rust
	raw := run(exportCmd)

	merged := parseCanonical(raw, top)
	var hit, total int64
	var files []string
	for sf := range maps.Keys(merged) {
		files = append(files, sf)
	}
	slices.Sort(files)
	handle, err := os.Create(out)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	for _, sf := range files {
		fmt.Fprintf(handle, "SF:%s\n", sf)
		var covered int64
		lines := slices.Sorted(maps.Keys(merged[sf]))
		for _, lineno := range lines {
			count := merged[sf][lineno]
			fmt.Fprintf(handle, "DA:%d,%d\n", lineno, count)
			if count > 0 {
				covered++
			}
		}
		size := int64(len(merged[sf]))
		fmt.Fprintf(handle, "LF:%d\nLH:%d\nend_of_record\n", size, covered)
		hit += covered
		total += size
	}
	handle.Close()
	denom := total
	if denom < 1 {
		denom = 1
	}
	fmt.Printf("canonical: %d/%d = %.2f%%  files: %d\n", hit, total, 100*float64(hit)/float64(denom), len(merged))
	fmt.Println("wrote", out)
}

// canonicalPath absolutizes an llvm-cov SF path against the repo top,
// normalizes twin spellings (`tests/../src` -> `src`), and rejects
// out-of-tree and third-party paths. False means skip the record.
func canonicalPath(top, sf string) (string, bool) {
	p := sf
	if !filepath.IsAbs(p) {
		p = top + "/" + p
	}
	norm := path.Clean(p)
	if !strings.HasPrefix(norm, top+"/") {
		return "", false
	}
	for _, part := range excluded {
		if strings.Contains(norm, part) {
			return "", false
		}
	}
	return norm, true
}

// addCount merges one DA hit count into a record: counts at/above the
// clamp (llvm's "unreachable" marker) read as zero, and totals saturate
// instead of overflowing. The line is always recorded, even at zero,
// matching llvm-cov's LF accounting.
func addCount(record map[int]int64, lineno int, count uint64) {
	if count >= clamp {
		count = 0
	}
	// ParseUint yields a uint64. The clamp leaves the value below
	// 2^62, which fits in int64, but CodeQL only accepts a constant
	// bound before the narrower conversion.
	if count > uint64(math.MaxInt64) {
		record[lineno] = saturate
		return
	}
	hits := int64(count)
	prev := record[lineno]
	if uint64(hits) > uint64(saturate-prev) {
		record[lineno] = saturate
	} else {
		record[lineno] = prev + hits
	}
}

// parseCanonical folds raw llvm-cov LCOV into path-normalized per-line
// records, merging twin path spellings by summing their counts.
func parseCanonical(raw, top string) map[string]map[int]int64 {
	merged := make(map[string]map[int]int64)
	current := ""
	haveCurrent := false
	for rawLine := range strings.Lines(raw) {
		line := strings.TrimSuffix(rawLine, "\n")
		switch {
		case strings.HasPrefix(line, "SF:"):
			norm, ok := canonicalPath(top, line[3:])
			current, haveCurrent = norm, ok
		case strings.HasPrefix(line, "DA:") && haveCurrent:
			rest := strings.SplitN(line[3:], ",", 3)
			if len(rest) < 2 {
				continue
			}
			lineno, err1 := strconv.Atoi(rest[0])
			count, err2 := strconv.ParseUint(rest[1], 10, 64)
			if err1 != nil || err2 != nil {
				continue
			}
			record := merged[current]
			if record == nil {
				record = make(map[int]int64)
				merged[current] = record
			}
			addCount(record, lineno, count)
		}
	}
	return merged
}
