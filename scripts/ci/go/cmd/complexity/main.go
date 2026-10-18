// Command complexity is the rust-code-analysis gate (vendored from
// arcanist's scripts/ci/go), failing on new or worsened complexity
// violations. Usage text names this binary; otherwise stdout matches
// the arcanist oracle byte-for-byte: metric numbers keep their raw
// JSON literals, and internal tool failures go to stderr (exit 1)
// or stdout (exit 2) respectively.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/executil"
	"github.com/hhh-pa/clojure/scripts/ci/go/internal/pyjson"
)

const usage = `rust-code-analysis gate (T1174): fail on new/worsened complexity violations.

Collects per-function cyclomatic/cognitive metrics over every .rs file in the
workspace (excluding build output) and compares named-function violations
(cyclomatic > 15 or cognitive > 20) against the checked-in
<workspace>/complexity-baseline.json ratchet:

* a violation not in the baseline -> error (fix the function or, with a
  reviewer, extend the baseline)
* a baselined violation whose metrics grew -> error
* a baseline entry that no longer violates -> note (prune at leisure)

A baselined entry may carry a justification string naming why the
function is inherently complex; --regen preserves justifications
on entries that still violate.

Fail-closed: no .rs files collected, an empty result against a non-empty
baseline, or any unparseable rca output file is an error, never a silent
OK — a green gate must mean "analyzed, nothing new", not "analyzed
nothing".

Usage: complexity <workspace-dir> [--regen]
  --regen rewrites the baseline instead of checking (adoption/updates only).

Requires rust-code-analysis-cli on PATH.

`

// Thresholds mirrored with arcanist's scripts/ci/complexity-scan.ps1.
const (
	cyThreshold = 15
	coThreshold = 20
)

const baselineCommentFmt = "rust-code-analysis baseline (T1174): named functions with " +
	"cyclomatic>%d or cognitive>%d. CI fails on NEW violations " +
	"or worsened ones; prune entries that no longer violate."

type violation struct {
	File          string      `json:"file"`
	Function      string      `json:"function"`
	Cyclomatic    json.Number `json:"cyclomatic"`
	Cognitive     json.Number `json:"cognitive"`
	Justification string      `json:"justification,omitempty"`
}

type violationKey struct {
	file     string
	function string
}

func numVal(n json.Number) float64 {
	f, err := n.Float64()
	if err != nil {
		return 0
	}
	return f
}

// rcaSpace mirrors one rust-code-analysis "spaces" node. Metrics stay
// untyped so malformed nodes fail the walk (like the oracle's
// AttributeError) instead of the parse.
type rcaSpace struct {
	Kind    string                 `json:"kind"`
	Name    *string                `json:"name"`
	Metrics map[string]interface{} `json:"metrics"`
	Spaces  []rcaSpace             `json:"spaces"`
}

type rcaDoc struct {
	Spaces []rcaSpace `json:"spaces"`
}

// errShape reports valid JSON the walk cannot interpret (the oracle
// crashes with AttributeError/TypeError here: stderr, exit 1).
type errShape struct{ msg string }

func (e errShape) Error() string { return e.msg }

// metric extracts one "sum" number (missing metrics count 0).
func metric(m map[string]interface{}, key string) (json.Number, error) {
	raw, ok := m[key]
	if !ok || raw == nil {
		return json.Number("0"), nil
	}
	obj, ok := raw.(map[string]interface{})
	if !ok {
		return "", errShape{"metric " + key + " is not an object"}
	}
	sum, ok := obj["sum"]
	if !ok || sum == nil {
		return json.Number("0"), nil
	}
	num, ok := sum.(json.Number)
	if !ok {
		return "", errShape{"metric " + key + " sum is not a number"}
	}
	return num, nil
}

func walk(spaces []rcaSpace, stack []string, path string, out *[]violation) error {
	for _, s := range spaces {
		name := "?"
		if s.Name != nil {
			name = *s.Name
		}
		switch {
		case s.Kind == "function" && name != "<anonymous>":
			cy, err := metric(s.Metrics, "cyclomatic")
			if err != nil {
				return err
			}
			co, err := metric(s.Metrics, "cognitive")
			if err != nil {
				return err
			}
			if numVal(cy) > cyThreshold || numVal(co) > coThreshold {
				*out = append(*out, violation{
					File:       path,
					Function:   strings.Join(append(append([]string(nil), stack...), name), "::"),
					Cyclomatic: cy,
					Cognitive:  co,
				})
			}
			if err := walk(s.Spaces, append(stack, name), path, out); err != nil {
				return err
			}
		case s.Kind == "closure":
			// Parent metrics are inclusive of closures, so gating
			// named functions alone covers them.
			if err := walk(s.Spaces, stack, path, out); err != nil {
				return err
			}
		default:
			extra := stack
			if name != "" && name != "?" {
				extra = append(stack, name)
			}
			if err := walk(s.Spaces, extra, path, out); err != nil {
				return err
			}
		}
	}
	return nil
}

// collect gathers current violations, mirroring collect(). It returns
// exit code 2 with the oracle's stdout line for the fail-closed tool
// errors, or a shape error for uninterpretable rca JSON.
func collect(workspace string) ([]violation, int, error) {
	var rss []string
	err := filepath.WalkDir(workspace, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			if path != workspace {
				name := d.Name()
				if name == ".git" || name == "target" || strings.HasPrefix(name, "target-") {
					return filepath.SkipDir
				}
			}
			return nil
		}
		if strings.HasSuffix(d.Name(), ".rs") {
			rel, err := filepath.Rel(workspace, path)
			if err != nil {
				return err
			}
			rss = append(rss, "./"+rel)
		}
		return nil
	})
	if err != nil {
		fmt.Fprintf(os.Stderr, "cannot walk %s: %v\n", workspace, err)
		return nil, 1, errShape{"walk failed"}
	}
	sort.Strings(rss)
	if len(rss) == 0 {
		fmt.Printf("no .rs files collected under %s (wrong workspace?)\n", workspace)
		return nil, 2, nil
	}
	outdir, err := os.MkdirTemp("", "rca-")
	if err != nil {
		fmt.Fprintf(os.Stderr, "cannot create temp dir: %v\n", err)
		return nil, 1, errShape{"tempdir failed"}
	}
	defer os.RemoveAll(outdir)
	cmd := []string{"rust-code-analysis-cli", "-m", "-O", "json", "-o", outdir}
	for _, f := range rss {
		cmd = append(cmd, "-p", f)
	}
	_, serr, err := executil.Capture(cmd, os.Environ(), workspace)
	if err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			fmt.Printf("rust-code-analysis-cli failed:\n%s\n", executil.Tail(serr, 2000))
			return nil, 2, nil
		}
		fmt.Fprintf(os.Stderr, "rust-code-analysis-cli failed: %v\n", err)
		return nil, 1, errShape{"rca launch failed"}
	}
	viol := []violation{}
	var skipped []string
	err = filepath.WalkDir(outdir, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(d.Name(), ".json") {
			return nil
		}
		data, err := os.ReadFile(path)
		if err != nil {
			skipped = append(skipped, path)
			return nil
		}
		var doc rcaDoc
		dec := json.NewDecoder(bytes.NewReader(data))
		dec.UseNumber()
		if err := dec.Decode(&doc); err != nil {
			// Syntax errors are blind spots; valid JSON of the
			// wrong shape crashes the oracle's walk instead.
			var probe interface{}
			pdec := json.NewDecoder(bytes.NewReader(data))
			pdec.UseNumber()
			if pdec.Decode(&probe) != nil {
				skipped = append(skipped, path)
				return nil
			}
			return errShape{"cannot interpret " + path}
		}
		rel, err := filepath.Rel(outdir, strings.TrimSuffix(path, ".json"))
		if err != nil {
			return err
		}
		rel = strings.TrimPrefix(rel, "./")
		if err := walk(doc.Spaces, nil, rel, &viol); err != nil {
			return err
		}
		return nil
	})
	if err != nil {
		var shape errShape
		if errors.As(err, &shape) {
			fmt.Fprintf(os.Stderr, "cannot interpret rca output: %v\n", err)
			return nil, 1, errShape{"rca shape failed"}
		}
		fmt.Fprintf(os.Stderr, "cannot read rca output: %v\n", err)
		return nil, 1, errShape{"rca read failed"}
	}
	if len(skipped) > 0 {
		sort.Strings(skipped)
		shown := skipped
		if len(shown) > 10 {
			shown = shown[:10]
		}
		fmt.Printf("unparseable rca output (%d file(s), blind spot):\n%s\n",
			len(skipped), strings.Join(shown, "\n"))
		return nil, 2, nil
	}
	sort.SliceStable(viol, func(i, j int) bool {
		if viol[i].File != viol[j].File {
			return viol[i].File < viol[j].File
		}
		return viol[i].Function < viol[j].Function
	})
	return viol, 0, nil
}

func keyOf(v violation) violationKey { return violationKey{v.File, v.Function} }

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
	baselinePath := filepath.Join(workspace, "complexity-baseline.json")

	current, code, err := collect(workspace)
	if err != nil {
		return code
	}
	if code != 0 {
		return code
	}
	curMap := map[violationKey]violation{}
	for _, v := range current {
		curMap[keyOf(v)] = v
	}

	if regen {
		keep := map[violationKey]string{}
		if raw, err := os.ReadFile(baselinePath); err == nil {
			var prev struct {
				Violations []violation `json:"violations"`
			}
			if json.Unmarshal(raw, &prev) == nil {
				for _, v := range prev.Violations {
					if v.Justification != "" {
						keep[keyOf(v)] = v.Justification
					}
				}
			}
		}
		out := append([]violation(nil), current...)
		for i := range out {
			out[i].Justification = keep[keyOf(out[i])]
		}
		base := struct {
			Comment    string      `json:"_comment"`
			Violations []violation `json:"violations"`
		}{
			fmt.Sprintf(baselineCommentFmt, cyThreshold, coThreshold),
			out,
		}
		data, err := pyjson.MarshalIndent1(base)
		if err != nil {
			fmt.Fprintf(os.Stderr, "cannot write baseline %s: %v\n", baselinePath, err)
			return 1
		}
		if err := os.WriteFile(baselinePath, data, 0o666); err != nil {
			fmt.Fprintf(os.Stderr, "cannot write baseline %s: %v\n", baselinePath, err)
			return 1
		}
		fmt.Printf("%s: wrote %d baseline violations\n", workspace, len(current))
		return 0
	}

	raw, err := os.ReadFile(baselinePath)
	if err != nil {
		fmt.Printf("cannot read baseline %s: %s\n",
			baselinePath, executil.OSErrorText(baselinePath, err))
		return 2
	}
	var probe map[string]json.RawMessage
	if err := json.Unmarshal(raw, &probe); err != nil {
		var shape interface{}
		if json.Unmarshal(raw, &shape) == nil {
			fmt.Fprintf(os.Stderr, "cannot use baseline %s: not a JSON object\n", baselinePath)
			return 1
		}
		fmt.Printf("cannot read baseline %s: %v\n", baselinePath, err)
		return 2
	}
	rawViol, ok := probe["violations"]
	if !ok {
		fmt.Printf("cannot read baseline %s: 'violations'\n", baselinePath)
		return 2
	}
	if string(rawViol) == "null" {
		fmt.Fprintf(os.Stderr, "cannot use baseline %s: violations is null\n", baselinePath)
		return 1
	}
	var baseViol []violation
	dec := json.NewDecoder(bytes.NewReader(rawViol))
	dec.UseNumber()
	if err := dec.Decode(&baseViol); err != nil {
		fmt.Fprintf(os.Stderr, "cannot use baseline %s: bad violations: %v\n", baselinePath, err)
		return 1
	}
	for i := range baseViol {
		if baseViol[i].Cyclomatic == "" {
			baseViol[i].Cyclomatic = json.Number("0")
		}
		if baseViol[i].Cognitive == "" {
			baseViol[i].Cognitive = json.Number("0")
		}
	}
	baseMap := map[violationKey]violation{}
	for _, v := range baseViol {
		baseMap[keyOf(v)] = v
	}

	if len(curMap) == 0 && len(baseMap) > 0 {
		fmt.Printf("FAIL: collected zero violations but the baseline lists %d "+
			"(tool failure or mass fix?); regen explicitly if the code is "+
			"really clean\n", len(baseMap))
		return 1
	}

	var newKeys, bothKeys []violationKey
	for k := range curMap {
		if _, ok := baseMap[k]; ok {
			bothKeys = append(bothKeys, k)
		} else {
			newKeys = append(newKeys, k)
		}
	}
	sort.Slice(newKeys, func(i, j int) bool {
		if newKeys[i].file != newKeys[j].file {
			return newKeys[i].file < newKeys[j].file
		}
		return newKeys[i].function < newKeys[j].function
	})
	sort.Slice(bothKeys, func(i, j int) bool {
		if bothKeys[i].file != bothKeys[j].file {
			return bothKeys[i].file < bothKeys[j].file
		}
		return bothKeys[i].function < bothKeys[j].function
	})
	failed := false
	for _, k := range newKeys {
		v := curMap[k]
		fmt.Printf("NEW complexity violation: %s:%s cyclomatic=%s cognitive=%s\n",
			v.File, v.Function, v.Cyclomatic.String(), v.Cognitive.String())
		failed = true
	}
	for _, k := range bothKeys {
		c, b := curMap[k], baseMap[k]
		if numVal(c.Cyclomatic) > numVal(b.Cyclomatic) || numVal(c.Cognitive) > numVal(b.Cognitive) {
			fmt.Printf("WORSENED complexity violation: %s:%s was (cy=%s, co=%s), now (cy=%s, co=%s)\n",
				c.File, c.Function,
				b.Cyclomatic.String(), b.Cognitive.String(),
				c.Cyclomatic.String(), c.Cognitive.String())
			failed = true
		}
	}
	stale := 0
	for k := range baseMap {
		if _, ok := curMap[k]; !ok {
			stale++
		}
	}
	if stale > 0 {
		noun := "ies"
		if stale == 1 {
			noun = "y"
		}
		fmt.Printf("note: %d baseline entr%s no longer violate (prune at leisure)\n", stale, noun)
	}
	worst := append([]violation(nil), current...)
	sort.SliceStable(worst, func(i, j int) bool {
		return numVal(worst[i].Cyclomatic)+numVal(worst[i].Cognitive) >
			numVal(worst[j].Cyclomatic)+numVal(worst[j].Cognitive)
	})
	if len(worst) > 10 {
		worst = worst[:10]
	}
	fmt.Println("--- most complex functions ---")
	for _, v := range worst {
		fmt.Printf("  %s:%s cyclomatic=%s cognitive=%s\n",
			v.File, v.Function, v.Cyclomatic.String(), v.Cognitive.String())
	}
	status := "OK"
	if failed {
		status = "FAIL"
	}
	fmt.Printf("complexity: %d violations (%d baselined) -> %s\n",
		len(current), len(baseMap), status)
	if failed {
		return 1
	}
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
