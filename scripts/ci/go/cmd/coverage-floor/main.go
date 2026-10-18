// Command coverage-floor fails when LCOV line coverage drops below
// the floor (vendored from arcanist's scripts/ci/go). Usage text
// names this binary; all other output matches the arcanist oracle
// byte-for-byte.
package main

import (
	"fmt"
	"os"
	"sort"
	"strconv"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/lcov"
)

const usage = "Usage: coverage-floor <lcov.info> [floor=80.0]\n"

type row struct {
	pct float64
	hit int
	tot int
	sf  string
}

func run(argv []string) int {
	if len(argv) < 1 || len(argv) > 2 {
		fmt.Print(usage)
		return 2
	}
	path := argv[0]
	floor := 80.0
	if len(argv) > 1 {
		parsed, err := strconv.ParseFloat(argv[1], 64)
		if err != nil {
			fmt.Printf("floor must be a number, got '%s'\n", argv[1])
			return 2
		}
		floor = parsed
	}
	info, err := os.Stat(path)
	if err != nil || info.IsDir() {
		fmt.Printf("coverage floor: %s not found\n", path)
		return 1
	}
	data, err := os.ReadFile(path)
	if err != nil {
		fmt.Printf("coverage floor: %s not found\n", path)
		return 1
	}
	files := lcov.ParseDA(string(data))
	var rows []row
	hit, tot := 0, 0
	for _, sf := range lcov.SortedSFs(files) {
		das := files[sf]
		h := 0
		for _, c := range das {
			if c > 0 {
				h++
			}
		}
		t := len(das)
		rate := float64(h) / float64(max(1, t))
		rows = append(rows, row{rate, h, t, sf})
		hit += h
		tot += t
	}
	pct := 100 * float64(hit) / float64(max(1, tot))
	fmt.Printf(
		"coverage floor: %.2f%% lines (%d/%d, %d files) vs %.2f%% floor (%s)\n",
		pct, hit, tot, len(rows), floor, path,
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
		fmt.Printf(
			"FAIL: line coverage %.2f%% is below the %.2f%% floor\n",
			pct, floor,
		)
		return 1
	}
	fmt.Println("coverage floor: OK")
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
