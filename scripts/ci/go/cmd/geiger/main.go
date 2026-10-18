// Command geiger is the cargo-geiger gate (vendored from arcanist's
// scripts/ci/go), failing when workspace `unsafe` grows. Usage text
// names this binary; otherwise stdout matches the arcanist oracle
// byte-for-byte. Internal tool failures go to stderr with exit 1.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/executil"
	"github.com/hhh-pa/clojure/scripts/ci/go/internal/pyjson"
)

const usage = `cargo-geiger gate (T1174): fail when workspace ` + "`unsafe`" + ` grows.

geiger 0.13 cannot run on virtual manifests, so this runs it once per
workspace member and sums each member's own unsafe counters (used + unused,
so used/unused reclassification alone never trips the gate). Crates whose
bins carry required-features get those
features enabled from cargo metadata, so the bins are analyzed too.

The sum is compared against <workspace>/geiger-baseline.json: a higher total
fails, a lower one prints a tighten-at-leisure note. Dependency unsafe is
reported (informational) but never gated: new deps routinely allow unsafe.

Usage: geiger <workspace> [--regen]
  --regen rewrites the baseline instead of checking (adoption/updates only).

Requires cargo-geiger on PATH.

`

const baselineComment = "cargo-geiger baseline (T1174): total unsafe items/expressions " +
	"summed over workspace crates (used + unused). CI fails when " +
	"the total grows; lower it when unsafe is removed."

type metaTarget struct {
	RequiredFeatures []string `json:"required-features"`
}

type metaPkg struct {
	Name         string       `json:"name"`
	ManifestPath string       `json:"manifest_path"`
	Targets      []metaTarget `json:"targets"`
}

type metaDoc struct {
	Packages []metaPkg `json:"packages"`
}

type geigerCount struct {
	Unsafe int64 `json:"unsafe_"`
}

type geigerCounters struct {
	Functions  geigerCount `json:"functions"`
	Exprs      geigerCount `json:"exprs"`
	ItemImpls  geigerCount `json:"item_impls"`
	ItemTraits geigerCount `json:"item_traits"`
	Methods    geigerCount `json:"methods"`
}

type geigerUnsafety struct {
	Used   geigerCounters `json:"used"`
	Unused geigerCounters `json:"unused"`
}

type geigerPkg struct {
	Package struct {
		ID struct {
			Name string `json:"name"`
		} `json:"id"`
	} `json:"package"`
	Unsafety geigerUnsafety `json:"unsafety"`
}

type geigerDoc struct {
	Packages []geigerPkg `json:"packages"`
}

func countersTotal(c geigerCounters) int64 {
	return c.Functions.Unsafe + c.Exprs.Unsafe + c.ItemImpls.Unsafe +
		c.ItemTraits.Unsafe + c.Methods.Unsafe
}

// readBaseline returns the baselined unsafe total. Read/parse/key
// failures print the oracle's "cannot read baseline" line and report
// exit 2; valid JSON of the wrong shape or type is a Python
// traceback (exit 1), mirrored as a stderr line with exit 1.
func readBaseline(baselinePath string) (int64, int, bool) {
	raw, err := os.ReadFile(baselinePath)
	if err != nil {
		fmt.Printf("cannot read baseline %s: %s\n",
			baselinePath, executil.OSErrorText(baselinePath, err))
		return 0, 2, false
	}
	var probe map[string]json.RawMessage
	if err := json.Unmarshal(raw, &probe); err != nil {
		var shape interface{}
		if json.Unmarshal(raw, &shape) == nil {
			fmt.Fprintf(os.Stderr, "cannot use baseline %s: not a JSON object\n", baselinePath)
			return 0, 1, false
		}
		fmt.Printf("cannot read baseline %s: %v\n", baselinePath, err)
		return 0, 2, false
	}
	rawTotal, ok := probe["unsafe_total"]
	if !ok {
		fmt.Printf("cannot read baseline %s: 'unsafe_total'\n", baselinePath)
		return 0, 2, false
	}
	dec := json.NewDecoder(bytes.NewReader(rawTotal))
	dec.UseNumber()
	var num interface{}
	if err := dec.Decode(&num); err != nil {
		fmt.Fprintf(os.Stderr, "cannot use baseline %s: unsafe_total is not a number\n", baselinePath)
		return 0, 1, false
	}
	literal, ok := num.(json.Number)
	if !ok {
		fmt.Fprintf(os.Stderr, "cannot use baseline %s: unsafe_total is not a number\n", baselinePath)
		return 0, 1, false
	}
	total, err := literal.Int64()
	if err != nil {
		fmt.Fprintf(os.Stderr, "cannot use baseline %s: unsafe_total is not an integer\n", baselinePath)
		return 0, 1, false
	}
	return total, 0, true
}

func run(argv []string) int {
	if len(argv) < 1 || argv[0] == "-h" || argv[0] == "--help" {
		fmt.Print(usage)
		return 2
	}
	workspace, err := filepath.Abs(argv[0])
	if err != nil {
		fmt.Fprintf(os.Stderr, "cannot resolve %s: %v\n", argv[0], err)
		return 1
	}
	regen := len(argv) > 1 && argv[1] == "--regen"
	baselinePath := filepath.Join(workspace, "geiger-baseline.json")
	targetDir := filepath.Join(workspace, "target-geiger")

	metaOut, metaErr, err := executil.Capture(
		[]string{"cargo", "metadata", "--no-deps", "--format-version", "1"},
		os.Environ(), workspace,
	)
	if err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			fmt.Printf("cargo metadata failed:\n%s\n", executil.Tail(metaErr, 2000))
			return 2
		}
		// Cargo itself would not launch (Python: traceback).
		fmt.Fprintf(os.Stderr, "cargo metadata failed: %v\n", err)
		return 1
	}
	var meta metaDoc
	if err := json.Unmarshal([]byte(metaOut), &meta); err != nil {
		// Python: json.loads crash (traceback).
		fmt.Fprintf(os.Stderr, "cargo metadata emitted no JSON: %v\n", err)
		return 1
	}
	members := append([]metaPkg(nil), meta.Packages...)
	sort.Slice(members, func(i, j int) bool { return members[i].Name < members[j].Name })

	memberNames := map[string]bool{}
	for _, p := range members {
		memberNames[p.Name] = true
	}
	env := append(executil.ChildEnv(), "CARGO_TARGET_DIR="+targetDir)
	var total int64
	type crateTotal struct {
		name string
		own  int64
	}
	var perCrate []crateTotal
	depUnsafe := map[string]bool{}
	for _, pkg := range members {
		featSet := map[string]bool{}
		for _, t := range pkg.Targets {
			for _, f := range t.RequiredFeatures {
				featSet[f] = true
			}
		}
		var features []string
		for f := range featSet {
			features = append(features, f)
		}
		sort.Strings(features)
		cmd := []string{
			"cargo", "geiger",
			"--manifest-path", pkg.ManifestPath,
			"--output-format", "Json",
		}
		if len(features) > 0 {
			cmd = append(cmd, "--features", strings.Join(features, " "))
		}
		fmt.Printf("geiger %s...\n", pkg.Name)
		out, serr, err := executil.Capture(cmd, env, "")
		if err != nil {
			var exit executil.ExitError
			if errors.As(err, &exit) {
				fmt.Printf("cargo geiger failed for %s: %s\n", pkg.Name, strings.Join(cmd, " "))
				var errorLines []string
				for _, ln := range strings.Split(serr, "\n") {
					if strings.HasPrefix(ln, "error") {
						errorLines = append(errorLines, ln)
					}
				}
				if len(errorLines) > 0 {
					if len(errorLines) > 20 {
						errorLines = errorLines[:20]
					}
					fmt.Println(strings.Join(errorLines, "\n"))
				}
				fmt.Println("--- stderr tail ---")
				fmt.Println(executil.Tail(serr, 4000))
				return 2
			}
			fmt.Fprintf(os.Stderr, "cargo geiger failed for %s: %v\n", pkg.Name, err)
			return 1
		}
		var doc geigerDoc
		if err := json.Unmarshal([]byte(out), &doc); err != nil {
			fmt.Printf("cargo geiger emitted no JSON for %s\n", pkg.Name)
			return 2
		}
		var hits []geigerPkg
		for _, p := range doc.Packages {
			if p.Package.ID.Name == pkg.Name {
				hits = append(hits, p)
			}
		}
		if len(hits) != 1 {
			fmt.Fprintf(os.Stderr,
				"expected exactly 1 geiger entry for %s, found %d\n",
				pkg.Name, len(hits))
			return 1
		}
		own := countersTotal(hits[0].Unsafety.Used) + countersTotal(hits[0].Unsafety.Unused)
		perCrate = append(perCrate, crateTotal{pkg.Name, own})
		total += own
		for _, p := range doc.Packages {
			n := p.Package.ID.Name
			if memberNames[n] {
				continue
			}
			if countersTotal(p.Unsafety.Used)+countersTotal(p.Unsafety.Unused) > 0 {
				// Python checks any single counter > 0; a sum > 0
				// differs only with negative counters, which
				// geiger never emits.
				depUnsafe[n] = true
			}
		}
	}

	for _, pc := range perCrate {
		fmt.Printf("  unsafe in %-45s %d\n", pc.name, pc.own)
	}
	fmt.Printf(
		"geiger: workspace unsafe total %d; deps allowing unsafe: %d (informational)\n",
		total, len(depUnsafe),
	)

	if regen {
		base := struct {
			Comment     string `json:"_comment"`
			UnsafeTotal int64  `json:"unsafe_total"`
		}{baselineComment, total}
		data, err := pyjson.MarshalIndent1(base)
		if err != nil {
			fmt.Fprintf(os.Stderr, "cannot write baseline %s: %v\n", baselinePath, err)
			return 1
		}
		if err := os.WriteFile(baselinePath, data, 0o666); err != nil {
			fmt.Fprintf(os.Stderr, "cannot write baseline %s: %v\n", baselinePath, err)
			return 1
		}
		fmt.Printf("%s: wrote unsafe_total=%d\n", workspace, total)
		return 0
	}

	baseline, code, ok := readBaseline(baselinePath)
	if !ok {
		return code
	}
	if total > baseline {
		fmt.Printf("FAIL: workspace unsafe total grew %d -> %d\n", baseline, total)
		return 1
	}
	if total < baseline {
		fmt.Printf("note: unsafe total fell %d -> %d (tighten the baseline at leisure)\n",
			baseline, total)
	}
	fmt.Println("geiger: OK")
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
