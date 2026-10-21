// Command coverage-patch fails when LCOV line coverage of the
// unified-diff added lines drops below the floor (S9A patch gate:
// new/changed lines can never land dark). Added lines without a DA
// record are not coverable (comments, blanks, decls) and stay out
// of the denominator, like Codecov patch coverage.
//
// The diff must be git-format (`git diff --unified=0 ...`, as the
// rust.yml patch step produces): a non-empty diff without `diff
// --git` headers fails closed instead of passing vacuously, so a
// future workflow change that alters the diff format can never
// silently disable the gate.
package main

import (
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/lcov"
)

const usage = "Usage: coverage-patch <lcov.info> <patch.diff> [floor=100.0]\n"

type row struct {
	pct float64
	hit int
	tot int
	sf  string
}

type darkLine struct {
	sf   string
	line int
}

// parseDiff returns the added-line set per repo-relative path from a
// unified diff. Only `diff --git` / `---` / `+++` / `@@` headers are
// read, in that order per file block, so content lines starting with
// `+`, `-`, or `@` can never confuse the parser. Deletions
// (`+++ /dev/null`) and hunks adding zero lines contribute nothing.
func parseDiff(text string) map[string]map[int]bool {
	added := map[string]map[int]bool{}
	const (
		seekOld = iota
		seekNew
		inHunks
	)
	state := inHunks
	rel := ""
	for _, line := range strings.Split(text, "\n") {
		switch {
		case strings.HasPrefix(line, "diff --git "):
			state = seekOld
			rel = ""
		case state == seekOld && strings.HasPrefix(line, "--- "):
			state = seekNew
		case state == seekNew && strings.HasPrefix(line, "+++ "):
			rest := strings.TrimSpace(line[len("+++ "):])
			rest = strings.Trim(rest, "\"")
			rest = strings.TrimPrefix(rest, "b/")
			if rest == "/dev/null" {
				rel = ""
			} else {
				rel = rest
			}
			state = inHunks
		case state == inHunks && rel != "" && strings.HasPrefix(line, "@@ -"):
			start, count, ok := parseHunkNew(line)
			if !ok {
				continue
			}
			if added[rel] == nil {
				added[rel] = map[int]bool{}
			}
			for n := start; n < start+count; n++ {
				added[rel][n] = true
			}
		}
	}
	return added
}

// parseHunkNew reads the +newStart[,newCount] range from a hunk
// header (`@@ -a[,b] +c[,d] @@`). A missing count means one line; a
// zero count (deletion-only hunk) yields no lines.
func parseHunkNew(header string) (int, int, bool) {
	fields := strings.Fields(header)
	for _, f := range fields[1:] {
		if !strings.HasPrefix(f, "+") {
			continue
		}
		rest := f[1:]
		start, count := 0, 1
		if i := strings.Index(rest, ","); i >= 0 {
			var err error
			start, err = strconv.Atoi(rest[:i])
			if err != nil {
				return 0, 0, false
			}
			count, err = strconv.Atoi(rest[i+1:])
			if err != nil {
				return 0, 0, false
			}
		} else {
			var err error
			start, err = strconv.Atoi(rest)
			if err != nil {
				return 0, 0, false
			}
		}
		if start < 0 || count < 0 {
			return 0, 0, false
		}
		return start, count, true
	}
	return 0, 0, false
}

func sortedKeys(m map[string]map[int]bool) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func run(argv []string) int {
	if len(argv) < 2 || len(argv) > 3 {
		fmt.Print(usage)
		return 2
	}
	lcovPath, diffPath := argv[0], argv[1]
	floor := 100.0
	if len(argv) > 2 {
		parsed, err := strconv.ParseFloat(argv[2], 64)
		if err != nil {
			fmt.Printf("floor must be a number, got '%s'\n", argv[2])
			return 2
		}
		floor = parsed
	}
	if info, err := os.Stat(lcovPath); err != nil || info.IsDir() {
		fmt.Printf("coverage patch: %s not found\n", lcovPath)
		return 1
	}
	lcovText, err := os.ReadFile(lcovPath)
	if err != nil {
		fmt.Printf("coverage patch: %s not found\n", lcovPath)
		return 1
	}
	if info, err := os.Stat(diffPath); err != nil || info.IsDir() {
		fmt.Printf("coverage patch: %s not found\n", diffPath)
		return 1
	}
	diffText, err := os.ReadFile(diffPath)
	if err != nil {
		fmt.Printf("coverage patch: %s not found\n", diffPath)
		return 1
	}
	diffString := string(diffText)
	// Fail closed on headerless input (L1): without `diff --git`
	// headers parseDiff yields zero added lines, which the vacuous-OK
	// path below would bless. A blank diff stays a pass (the
	// intentional no-rust-changes case); anything else must be a git
	// unified diff. The prefix mirrors parseDiff exactly, so no input
	// the parser accepts is rejected here.
	headers := 0
	for _, line := range strings.Split(diffString, "\n") {
		if strings.HasPrefix(line, "diff --git ") {
			headers++
		}
	}
	if headers == 0 && strings.TrimSpace(diffString) != "" {
		fmt.Printf("coverage patch: %s holds no `diff --git` headers; refusing vacuous pass (regenerate with `git diff --unified=0`; see the patch step in rust.yml)\n", diffPath)
		return 1
	}
	files := lcov.ParseDA(string(lcovText))
	added := parseDiff(diffString)
	nlines, nfiles := 0, 0
	for _, lines := range added {
		if len(lines) == 0 {
			continue
		}
		nfiles++
		nlines += len(lines)
	}
	lineWord, fileWord := "lines", "files"
	if nlines == 1 {
		lineWord = "line"
	}
	if nfiles == 1 {
		fileWord = "file"
	}
	fmt.Printf("coverage patch: parsed %d added %s in %d %s\n", nlines, lineWord, nfiles, fileWord)
	var rows []row
	var dark []darkLine
	hit, tot := 0, 0
	for _, rel := range sortedKeys(added) {
		counts := map[string]int64{}
		for sf, das := range files {
			if sf == rel || strings.HasSuffix(sf, "/"+rel) {
				for line, c := range das {
					counts[line] += c
				}
			}
		}
		h, t := 0, 0
		for n := range added[rel] {
			c, present := counts[strconv.Itoa(n)]
			if !present {
				continue
			}
			t++
			if c > 0 {
				h++
			} else {
				dark = append(dark, darkLine{rel, n})
			}
		}
		if t == 0 {
			continue
		}
		rows = append(rows, row{float64(h) / float64(t), h, t, rel})
		hit += h
		tot += t
	}
	if tot == 0 {
		fmt.Printf("coverage patch: no coverable added lines (%s vs %s)\n", lcovPath, diffPath)
		fmt.Println("coverage patch: OK")
		return 0
	}
	pct := 100 * float64(hit) / float64(tot)
	fmt.Printf(
		"coverage patch: %.2f%% lines (%d/%d, %d files) vs %.2f%% floor (%s vs %s)\n",
		pct, hit, tot, len(rows), floor, lcovPath, diffPath,
	)
	if pct < floor {
		sort.Slice(rows, func(i, j int) bool {
			a, b := rows[i], rows[j]
			if a.pct != b.pct {
				return a.pct < b.pct
			}
			if a.hit != b.hit {
				return a.hit < b.hit
			}
			if a.tot != b.tot {
				return a.tot < b.tot
			}
			return a.sf < b.sf
		})
		fmt.Println("per-file table (lowest first):")
		for _, r := range rows {
			fmt.Printf("  %6.2f%% %5d/%-5d %s\n", 100*r.pct, r.hit, r.tot, r.sf)
		}
		sort.Slice(dark, func(i, j int) bool {
			if dark[i].sf != dark[j].sf {
				return dark[i].sf < dark[j].sf
			}
			return dark[i].line < dark[j].line
		})
		fmt.Println("uncovered added lines:")
		for _, d := range dark {
			fmt.Printf("  %s:%d\n", d.sf, d.line)
		}
		fmt.Printf(
			"FAIL: patch coverage %.2f%% is below the %.2f%% floor\n",
			pct, floor,
		)
		return 1
	}
	fmt.Println("coverage patch: OK")
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
