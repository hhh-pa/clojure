package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"reflect"
	"sort"
	"strconv"
	"strings"
)

// varyPassLabels mirrors the report-compare job of the
// clojure-algorithms perf-callgrind workflow: namespaces whose
// :pass count varies run to run by design (per-edge asserts on a
// random graph); :test/:fail/:error still compare exactly.
var varyPassLabels = map[string]bool{
	"clojure-algorithms.graphs.karger-test":                 true,
	"clojure-algorithms.graphs.random-graph-generator-test": true,
}

// metric describes one gated signal: the summary key, the display
// tag, and the loss-table value format.
type metric struct {
	key     string // summary key (ins tries pinKey first)
	tag     string
	unit    string
	summary string // summary-table row label
	format  func(float64) string
}

const pinKey = "pin_instructions"

var metrics = map[string]metric{
	"cpu":   {key: "cpu_s", tag: "CPU", unit: "s", summary: "median cpu_s", format: fmt3},
	"mem":   {key: "maxrss_kb", tag: "MEM", unit: "K", summary: "median maxrss_kb", format: fmt0},
	"ins":   {key: "instructions", tag: "INS", unit: "", summary: "median instructions", format: fmtComma},
	"alloc": {key: "pin_allocated_bytes", tag: "ALLOC", unit: "B", summary: "median allocated bytes", format: fmtComma},
}

// fmt3 renders a ratio like Python's f"{x:.3f}" (including inf).
func fmt3(x float64) string {
	if math.IsInf(x, 1) {
		return "inf"
	}
	if math.IsInf(x, -1) {
		return "-inf"
	}
	return strconv.FormatFloat(x, 'f', 3, 64)
}

// fmt0 renders like Python's f"{x:.0f}".
func fmt0(x float64) string {
	return strconv.FormatFloat(x, 'f', 0, 64)
}

// fmtComma renders like Python's f"{x:,.0f}" (thousands separators).
func fmtComma(x float64) string {
	s := fmt0(x)
	neg := strings.HasPrefix(s, "-")
	s = strings.TrimPrefix(s, "-")
	n := len(s)
	if n <= 3 {
		if neg {
			return "-" + s
		}
		return s
	}
	var b strings.Builder
	if neg {
		b.WriteByte('-')
	}
	lead := n % 3
	if lead == 0 {
		lead = 3
	}
	b.WriteString(s[:lead])
	for i := lead; i < n; i += 3 {
		b.WriteByte(',')
		b.WriteString(s[i : i+3])
	}
	return b.String()
}

// caseKey identifies one case across shards.
type caseKey [3]string // label, stem, case

// lessKey orders cases lexicographically (Go arrays lack <).
func lessKey(a, b caseKey) bool {
	if a[0] != b[0] {
		return a[0] < b[0]
	}
	if a[1] != b[1] {
		return a[1] < b[1]
	}
	return a[2] < b[2]
}

// countPair is one test_counts entry in document order.
type countPair struct {
	key   string
	value string // raw JSON number literal
}

// entry is one decoded result object plus its ordered test_counts.
type entry struct {
	raw    map[string]any
	counts []countPair // nil unless test_counts is an object
	hasTC  bool        // test_counts key present (any type)
}

func strOf(v any, dflt string) string {
	if s, ok := v.(string); ok {
		return s
	}
	return dflt
}

func numOf(v any) (float64, bool) {
	if n, ok := v.(json.Number); ok {
		f, err := n.Float64()
		return f, err == nil
	}
	return 0, false
}

// keyOf builds the union key: (label, stem|"", case|"").
func keyOf(raw map[string]any) caseKey {
	return caseKey{strOf(raw["label"], ""), strOf(raw["stem"], ""), strOf(raw["case"], "")}
}

// short renders label/case, or just label for algos "run" cases.
func short(k caseKey) string {
	if k[2] != "run" {
		return k[0] + "/" + k[2]
	}
	return k[0]
}

// runsLen is len(runs), 0 when runs is missing or not a list.
func (e entry) runsLen() int {
	if runs, ok := e.raw["runs"].([]any); ok {
		return len(runs)
	}
	return 0
}

// summaryMap is the summary object, or nil when absent/not an object.
func (e entry) summaryMap() map[string]any {
	if s, ok := e.raw["summary"].(map[string]any); ok {
		return s
	}
	return nil
}

// isSkipMarker ports _is_skip_marker: run-less with no usable
// summary (no summary dict carrying a non-null median).
func (e entry) isSkipMarker() bool {
	if e.runsLen() > 0 {
		return false
	}
	for _, v := range e.summaryMap() {
		if d, ok := v.(map[string]any); ok {
			if m, present := d["median"]; present && m != nil {
				return false
			}
		}
	}
	return true
}

// normCounts ports norm_counts: comparable test_counts with pass
// zeroed on the known-varying labels.
func (e entry) normCounts(label string) map[string]string {
	if !e.hasTC {
		return nil
	}
	out := make(map[string]string, len(e.counts))
	for _, p := range e.counts {
		out[p.key] = p.value
	}
	if varyPassLabels[label] {
		out["pass"] = "0"
	}
	return out
}

// pyRepr renders test_counts like Python's str(dict): document
// order, single-quoted keys, raw number literals.
func (e entry) pyRepr() string {
	var b strings.Builder
	b.WriteByte('{')
	for i, p := range e.counts {
		if i > 0 {
			b.WriteString(", ")
		}
		fmt.Fprintf(&b, "'%s': %s", p.key, p.value)
	}
	b.WriteByte('}')
	return b.String()
}

// normNum canonicalizes decoded JSON for marker comparison:
// numbers become float64 (Python 1 == 1.0), maps/slices recurse.
func normNum(v any) any {
	switch t := v.(type) {
	case json.Number:
		if f, err := t.Float64(); err == nil {
			return f
		}
		return t.String()
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, val := range t {
			out[k] = normNum(val)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, val := range t {
			out[i] = normNum(val)
		}
		return out
	default:
		return v
	}
}

// decodeDoc parses one leg JSON with number-preserving literals.
func decodeDoc(path string) (map[string]any, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	var doc map[string]any
	if err := dec.Decode(&doc); err != nil {
		return nil, err
	}
	return doc, nil
}

// collectTestCounts re-scans one leg JSON for the ordered
// test_counts pairs of each result (encoding/json maps lose
// document order, which the drift report prints).
func collectTestCounts(path string) ([][]countPair, []bool, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, nil, err
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	skip := func() error {
		var v any
		return dec.Decode(&v)
	}
	tok, err := dec.Token()
	if err != nil || tok != json.Delim('{') {
		return nil, nil, fmt.Errorf("top-level object expected")
	}
	var counts [][]countPair
	var present []bool
	for dec.More() {
		keyTok, err := dec.Token()
		if err != nil {
			return nil, nil, err
		}
		if keyTok != "results" {
			if err := skip(); err != nil {
				return nil, nil, err
			}
			continue
		}
		if tok, err := dec.Token(); err != nil || tok != json.Delim('[') {
			return nil, nil, fmt.Errorf("results list expected")
		}
		for dec.More() {
			if tok, err := dec.Token(); err != nil || tok != json.Delim('{') {
				return nil, nil, fmt.Errorf("result object expected")
			}
			var pairs []countPair
			has := false
			for dec.More() {
				keyTok, err := dec.Token()
				if err != nil {
					return nil, nil, err
				}
				if keyTok != "test_counts" {
					if err := skip(); err != nil {
						return nil, nil, err
					}
					continue
				}
				has = true
				if tok, err := dec.Token(); err != nil {
					return nil, nil, err
				} else if tok == json.Delim('{') {
					for dec.More() {
						kt, err := dec.Token()
						if err != nil {
							return nil, nil, err
						}
						ks, _ := kt.(string)
						var v any
						if err := dec.Decode(&v); err != nil {
							return nil, nil, err
						}
						val := ""
						if n, ok := v.(json.Number); ok {
							val = n.String()
						}
						pairs = append(pairs, countPair{ks, val})
					}
					if _, err := dec.Token(); err != nil { // closing }
						return nil, nil, err
					}
				}
				// A null/non-object test_counts stays has=true
				// with no pairs; normCounts then yields {}.
			}
			if _, err := dec.Token(); err != nil { // closing }
				return nil, nil, err
			}
			counts = append(counts, pairs)
			present = append(present, has)
		}
		if _, err := dec.Token(); err != nil { // closing ]
			return nil, nil, err
		}
	}
	return counts, present, nil
}

// union ports union(): shard reports become results-by-key plus
// sorted failures/missing; firstDoc feeds the merged output.
func union(paths []string, leg string) (map[caseKey]entry, []string, []string, map[string]any, error) {
	merged := map[caseKey]entry{}
	failures := map[string]bool{}
	missing := map[string]bool{}
	var firstDoc map[string]any
	for _, path := range paths {
		doc, err := decodeDoc(path)
		if err != nil {
			return nil, nil, nil, nil, err
		}
		results, ok := doc["results"].([]any)
		if !ok {
			return nil, nil, nil, nil, fmt.Errorf("%s: no results list", path)
		}
		counts, present, err := collectTestCounts(path)
		if err != nil || len(counts) != len(results) {
			if err == nil {
				err = fmt.Errorf("test_counts scan diverged")
			}
			return nil, nil, nil, nil, fmt.Errorf("%s: %v", path, err)
		}
		if firstDoc == nil {
			firstDoc = doc
		}
		for i, rv := range results {
			raw, ok := rv.(map[string]any)
			if !ok {
				return nil, nil, nil, nil, fmt.Errorf("%s: result %d not an object", path, i)
			}
			k := keyOf(raw)
			e := entry{raw: raw, counts: counts[i], hasTC: present[i]}
			if prev, dup := merged[k]; dup {
				// Sweep-skipped namespaces are recorded
				// run-less in EVERY shard by design; dedupe
				// identical markers, keep the hard error
				// for real dupes.
				if prev.isSkipMarker() && e.isSkipMarker() &&
					reflect.DeepEqual(normNum(prev.raw), normNum(e.raw)) {
					continue
				}
				return nil, nil, nil, nil, fmt.Errorf(
					"duplicate case %s/%s/%s across %s shards (shards must partition cases; check --shard flags)",
					k[0], k[1], k[2], leg)
			}
			merged[k] = e
		}
		for _, f := range strList(doc["failures"]) {
			failures[f] = true
		}
		for _, m := range strList(doc["missing"]) {
			missing[m] = true
		}
	}
	return merged, sortedKeys(failures), sortedKeys(missing), firstDoc, nil
}

func strList(v any) []string {
	items, _ := v.([]any)
	var out []string
	for _, it := range items {
		if s, ok := it.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

func sortedKeys(set map[string]bool) []string {
	out := make([]string, 0, len(set))
	for k := range set {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// writeMerged ports write_merged: the unioned leg as one JSON
// document (results in key order, config shard "merged").
func writeMerged(path string, firstDoc map[string]any, results map[caseKey]entry, failures, missing []string) error {
	doc := make(map[string]any, len(firstDoc)+1)
	for k, v := range firstDoc {
		doc[k] = v
	}
	keys := make([]caseKey, 0, len(results))
	for k := range results {
		keys = append(keys, k)
	}
	sort.Slice(keys, func(i, j int) bool { return lessKey(keys[i], keys[j]) })
	arr := make([]any, 0, len(keys))
	for _, k := range keys {
		arr = append(arr, results[k].raw)
	}
	doc["results"] = arr
	doc["failures"] = failures
	doc["missing"] = missing
	config := map[string]any{}
	if c, ok := firstDoc["config"].(map[string]any); ok {
		for k, v := range c {
			config[k] = v
		}
	}
	config["shard"] = "merged"
	doc["config"] = config
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	return enc.Encode(doc)
}

// lossRow is one gated loss: challenger/base ratio plus medians.
type lossRow struct {
	ratio float64
	name  string
	base  float64
	chal  float64
}

// metricStat accumulates one metric's verdicts.
type metricStat struct {
	win, loss, tie int
	losses         []lossRow
	worst          *lossRow // ratio+name only
}

// medianOf extracts one summary median; the ins metric prefers
// pin_instructions over the legacy instructions key.
func medianOf(e entry, m metric, name string) (float64, error) {
	summary := e.summaryMap()
	if summary == nil {
		return 0, fmt.Errorf("h2h_compare: missing %s summary for %s", m.key, name)
	}
	key := m.key
	if m.tag == "INS" {
		if _, ok := summary[pinKey]; ok {
			key = pinKey
		}
	}
	block, ok := summary[key].(map[string]any)
	if !ok {
		return 0, fmt.Errorf("h2h_compare: missing %s summary for %s", m.key, name)
	}
	med, ok := numOf(block["median"])
	if !ok {
		return 0, fmt.Errorf("h2h_compare: missing %s summary for %s", m.key, name)
	}
	return med, nil
}

// boundOf extracts a min/max bound for the README cross-check.
func boundOf(e entry, key, bound string) (float64, bool) {
	summary := e.summaryMap()
	if summary == nil {
		return 0, false
	}
	block, ok := summary[key].(map[string]any)
	if !ok {
		return 0, false
	}
	return numOf(block[bound])
}

// boundRaw renders a min/max bound like Python's {}: integer
// literals print verbatim, floats take the shortest form.
func boundRaw(e entry, key, bound string) string {
	summary := e.summaryMap()
	if summary == nil {
		return "?"
	}
	block, ok := summary[key].(map[string]any)
	if !ok {
		return "?"
	}
	if n, ok := block[bound].(json.Number); ok {
		if !strings.ContainsAny(n.String(), ".eE") {
			return n.String()
		}
		if f, err := n.Float64(); err == nil {
			return strconv.FormatFloat(f, 'f', -1, 64)
		}
		return n.String()
	}
	return "?"
}

// okOf reads the ok flag; malformed entries are a load error
// (the oracle would crash with a KeyError traceback here).
func okOf(e entry, name string) (bool, error) {
	ok, present := e.raw["ok"]
	if !present {
		return false, fmt.Errorf("h2h_compare: missing ok flag for %s", name)
	}
	b, isBool := ok.(bool)
	if !isBool {
		return false, fmt.Errorf("h2h_compare: missing ok flag for %s", name)
	}
	return b, nil
}

// errOf renders an entry's error like Python: string verbatim,
// null/missing as None.
func errOf(e entry) string {
	if s, ok := e.raw["error"].(string); ok {
		return s
	}
	return "None"
}

// compare runs the gate and returns the process exit code.
func compare(o options) int {
	jm, jFail, jMiss, firstJVM, err := union(o.jvm, o.baseName)
	if err == nil {
		var rm map[caseKey]entry
		var rFail, rMiss []string
		var firstRS map[string]any
		rm, rFail, rMiss, firstRS, err = union(o.rs, o.challengerName)
		if err == nil {
			if o.mergedJVM != "" {
				err = writeMerged(o.mergedJVM, firstJVM, jm, jFail, jMiss)
			}
			if err == nil && o.mergedRS != "" {
				err = writeMerged(o.mergedRS, firstRS, rm, rFail, rMiss)
			}
			if err == nil {
				return compareLegs(o, jm, rm, jFail, rFail, jMiss, rMiss)
			}
		}
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "h2h_compare: error loading leg JSONs: %v\n", err)
		return 2
	}
	return 0
}

func compareLegs(o options, jm, rm map[caseKey]entry, jFail, rFail, jMiss, rMiss []string) int {
	an, bn := o.baseName, o.challengerName
	var out strings.Builder
	fmt.Fprintf(&out, "== %s ==\n", o.label)
	fmt.Fprintf(&out, "%s: shards=%d results=%d failures=%d missing=%d\n",
		an, len(o.jvm), len(jm), len(jFail), len(jMiss))
	fmt.Fprintf(&out, "%s: shards=%d results=%d failures=%d missing=%d\n",
		bn, len(o.rs), len(rm), len(rFail), len(rMiss))
	var onlyBase, onlyChal, common []caseKey
	for k := range jm {
		if _, ok := rm[k]; ok {
			common = append(common, k)
		} else {
			onlyBase = append(onlyBase, k)
		}
	}
	for k := range rm {
		if _, ok := jm[k]; !ok {
			onlyChal = append(onlyChal, k)
		}
	}
	sort.Slice(common, func(i, j int) bool { return lessKey(common[i], common[j]) })
	sort.Slice(onlyBase, func(i, j int) bool { return lessKey(onlyBase[i], onlyBase[j]) })
	sort.Slice(onlyChal, func(i, j int) bool { return lessKey(onlyChal[i], onlyChal[j]) })
	fmt.Fprintf(&out, "common cases: %d only-%s: %d only-%s: %d\n",
		len(common), an, len(onlyBase), bn, len(onlyChal))
	for i, k := range onlyBase {
		if i >= 10 {
			break
		}
		fmt.Fprintf(&out, "  only-%s: %s/%s/%s\n", an, k[0], k[1], k[2])
	}
	for i, k := range onlyChal {
		if i >= 10 {
			break
		}
		fmt.Fprintf(&out, "  only-%s: %s/%s/%s\n", bn, k[0], k[1], k[2])
	}

	var selected []string
	for _, m := range strings.Split(o.metrics, ",") {
		selected = append(selected, strings.TrimSpace(m))
	}
	for _, m := range selected {
		if _, ok := metrics[m]; !ok {
			fmt.Fprintf(os.Stderr, "h2h_compare: unknown metric '%s' (want csv of cpu,mem,ins,alloc)\n", m)
			return 2
		}
	}
	stat := map[string]*metricStat{}
	for _, m := range selected {
		stat[m] = &metricStat{}
	}
	nBothWin := 0
	var notOK [][3]string // name, base err, chal err
	var skipped [][3]string
	var drift [][3]string // name, base counts, chal counts
	inSelected := func(m string) bool {
		for _, s := range selected {
			if s == m {
				return true
			}
		}
		return false
	}
	for _, k := range common {
		a, b := jm[k], rm[k]
		name := short(k)
		aOK, err := okOf(a, name)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 2
		}
		bOK, err := okOf(b, name)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 2
		}
		if !aOK || !bOK {
			notOK = append(notOK, [3]string{name, errOf(a), errOf(b)})
			continue
		}
		if a.isSkipMarker() || b.isSkipMarker() {
			skipped = append(skipped, [3]string{name, strconv.Itoa(a.runsLen()), strconv.Itoa(b.runsLen())})
			continue
		}
		// Behavioral drift gate (algos carry test_counts).
		if a.hasTC || b.hasTC {
			if !reflect.DeepEqual(a.normCounts(k[0]), b.normCounts(k[0])) {
				drift = append(drift, [3]string{name, a.pyRepr(), b.pyRepr()})
			}
		}
		verdicts := map[string]string{}
		for _, m := range selected {
			mm := metrics[m]
			jmed, err := medianOf(a, mm, name)
			if err != nil {
				fmt.Fprintln(os.Stderr, err)
				return 2
			}
			rmed, err := medianOf(b, mm, name)
			if err != nil {
				fmt.Fprintln(os.Stderr, err)
				return 2
			}
			ratio := math.Inf(1)
			if jmed != 0 {
				ratio = rmed / jmed
			}
			s := stat[m]
			switch {
			case jmed == 0 && rmed == 0:
				s.tie++
				verdicts[m] = "tie"
			case ratio > 1+o.margin:
				s.loss++
				s.losses = append(s.losses, lossRow{ratio, name, jmed, rmed})
				verdicts[m] = "loss"
			case rmed < jmed:
				s.win++
				verdicts[m] = "win"
			default:
				s.tie++
				verdicts[m] = "tie"
			}
			if s.worst == nil || ratio > s.worst.ratio {
				s.worst = &lossRow{ratio: ratio, name: name}
			}
		}
		allWin := true
		for _, m := range selected {
			if verdicts[m] != "win" {
				allWin = false
			}
		}
		if allWin {
			nBothWin++
		}
		// README-convention cross-check (cpu: min-of-repeats, rss: max).
		var notes []string
		if inSelected("cpu") {
			jmin, jok := boundOf(a, "cpu_s", "min")
			rmin, rok := boundOf(b, "cpu_s", "min")
			if jok && rok && (rmin < jmin) != (verdicts["cpu"] == "win") {
				notes = append(notes, fmt.Sprintf("cpu min %s vs %s",
					fmt3(rmin), fmt3(jmin)))
			}
		}
		if inSelected("mem") {
			jmax, jok := boundOf(a, "maxrss_kb", "max")
			rmax, rok := boundOf(b, "maxrss_kb", "max")
			if jok && rok && (rmax < jmax) != (verdicts["mem"] == "win") {
				notes = append(notes, fmt.Sprintf("rss max %s vs %s",
					boundRaw(b, "maxrss_kb", "max"), boundRaw(a, "maxrss_kb", "max")))
			}
		}
		if len(notes) > 0 {
			fmt.Fprintf(&out, "  NOTE %s: median verdict differs from README convention (%s)\n",
				name, strings.Join(notes, "; "))
		}
	}

	n := stat[selected[0]].win + stat[selected[0]].loss + stat[selected[0]].tie
	fmt.Fprintf(&out, "compared ok-cases: %d\n", n)
	for _, m := range selected {
		s := stat[m]
		fmt.Fprintf(&out, "%s (median): %s wins %d, losses %d, ties %d\n",
			metrics[m].tag, bn, s.win, s.loss, s.tie)
	}
	fmt.Fprintf(&out, "both-win cases: %d/%d\n", nBothWin, n)
	for _, m := range selected {
		if w := stat[m].worst; w == nil {
			fmt.Fprintf(&out, "worst %s %s ratio: n/a (no compared cases)\n", bn, m)
		} else {
			fmt.Fprintf(&out, "worst %s %s ratio: %sx (%s)\n", bn, m, fmt3(w.ratio), w.name)
		}
	}
	for _, m := range selected {
		s := stat[m]
		if len(s.losses) == 0 {
			continue
		}
		mm := metrics[m]
		fmt.Fprintf(&out, "--- %s LOSSES (%d) ---\n", mm.tag, len(s.losses))
		sort.Slice(s.losses, func(i, j int) bool {
			x, y := s.losses[i], s.losses[j]
			if x.ratio != y.ratio {
				return x.ratio > y.ratio
			}
			if x.name != y.name {
				return x.name > y.name
			}
			if x.base != y.base {
				return x.base > y.base
			}
			return x.chal > y.chal
		})
		for i, l := range s.losses {
			if i >= 50 {
				break
			}
			fmt.Fprintf(&out, "  %sx %s %s=%s%s %s=%s%s\n",
				fmt3(l.ratio), l.name, an, mm.format(l.base), mm.unit,
				bn, mm.format(l.chal), mm.unit)
		}
	}
	if len(skipped) > 0 {
		fmt.Fprintf(&out, "--- SKIPPED-NO-RUNS (%d) ---\n", len(skipped))
		for i, sk := range skipped {
			if i >= 20 {
				break
			}
			fmt.Fprintf(&out, "  %s %s_runs=%s %s_runs=%s\n", sk[0], an, sk[1], bn, sk[2])
		}
	}
	if len(notOK) > 0 {
		fmt.Fprintf(&out, "--- NOT-OK (%d) ---\n", len(notOK))
		for i, no := range notOK {
			if i >= 50 {
				break
			}
			fmt.Fprintf(&out, "  %s %s_err=%s %s_err=%s\n", no[0], an, no[1], bn, no[2])
		}
	}
	if len(drift) > 0 {
		fmt.Fprintf(&out, "--- TEST-COUNT DRIFT (%d) ---\n", len(drift))
		for i, d := range drift {
			if i >= 20 {
				break
			}
			fmt.Fprintf(&out, "  %s jvm=%s rs=%s\n", d[0], d[1], d[2])
		}
	}
	printList := func(kind string, leg string, items []string) {
		if len(items) == 0 {
			return
		}
		fmt.Fprintf(&out, "--- %s %s (%d) ---\n", strings.ToUpper(leg), kind, len(items))
		for i, f := range items {
			if i >= 20 {
				break
			}
			fmt.Fprintf(&out, "  %s\n", f)
		}
	}
	printList("failures", an, jFail)
	printList("failures", bn, rFail)
	printList("missing", an, jMiss)
	printList("missing", bn, rMiss)

	var problems []string
	for _, m := range selected {
		if stat[m].loss > 0 {
			problems = append(problems, fmt.Sprintf("%d %s losses", stat[m].loss, metrics[m].tag))
		}
	}
	if len(notOK) > 0 {
		problems = append(problems, fmt.Sprintf("%d not-ok cases", len(notOK)))
	}
	if len(drift) > 0 {
		problems = append(problems, fmt.Sprintf("%d test-count drifts", len(drift)))
	}
	if len(onlyBase) > 0 || len(onlyChal) > 0 {
		problems = append(problems, fmt.Sprintf("leg mismatch (only-%s %d, only-%s %d)",
			an, len(onlyBase), bn, len(onlyChal)))
	}
	if len(jFail) > 0 || len(rFail) > 0 {
		problems = append(problems, fmt.Sprintf("sweep failures (%s %d, %s %d)",
			an, len(jFail), bn, len(rFail)))
	}
	if len(jMiss) > 0 || len(rMiss) > 0 {
		problems = append(problems, fmt.Sprintf("missing cases (%s %d, %s %d)",
			an, len(jMiss), bn, len(rMiss)))
	}
	if n == 0 {
		problems = append(problems, "no compared cases")
	}

	gatePass := len(problems) == 0
	var verdict string
	if o.reportOnly {
		verdict = "PASS (report-only)"
		if !gatePass {
			verdict = fmt.Sprintf("FAIL ignored (--report-only): %s", strings.Join(problems, "; "))
		}
	} else if gatePass {
		verdict = "PASS"
	} else {
		verdict = fmt.Sprintf("FAIL: %s", strings.Join(problems, "; "))
	}
	fmt.Fprintf(&out, "GATE %s: %s\n", o.label, verdict)

	if o.summaryFile != "" {
		worstStr := func(m string) string {
			if w := stat[m].worst; w != nil {
				return fmt.Sprintf("%sx (%s)", fmt3(w.ratio), w.name)
			}
			return "n/a"
		}
		var lines []string
		lines = append(lines,
			fmt.Sprintf("## h2h %s: %s vs %s", o.label, an, bn),
			"",
			fmt.Sprintf("common cases: %d (%s shards %d, %s shards %d); both-win %d/%d",
				n, an, len(o.jvm), bn, len(o.rs), nBothWin, n),
			"",
			fmt.Sprintf("| metric | %s wins | losses | ties |", bn),
			"| --- | ---: | ---: | ---: |",
		)
		for _, m := range selected {
			s := stat[m]
			lines = append(lines, fmt.Sprintf("| %s (%s) | %d | %d | %d |",
				metrics[m].tag, metrics[m].summary, s.win, s.loss, s.tie))
		}
		var worsts []string
		for _, m := range selected {
			worsts = append(worsts, fmt.Sprintf("worst %s %s ratio: %s", bn, m, worstStr(m)))
		}
		verdictWord := "PASS"
		if !gatePass {
			verdictWord = "FAIL"
		}
		lines = append(lines, "", strings.Join(worsts, "; "), "", "### GATE: "+verdictWord)
		if len(problems) > 0 {
			lines = append(lines, "", strings.Join(problems, "; "))
		}
		for _, m := range selected {
			s := stat[m]
			if len(s.losses) == 0 {
				continue
			}
			mm := metrics[m]
			lines = append(lines, "",
				fmt.Sprintf("<details><summary>%s losses (%d, worst first)</summary>", mm.tag, len(s.losses)),
				"",
				fmt.Sprintf("| ratio | case | %s | %s |", an, bn),
				"| ---: | --- | ---: | ---: |")
			for i, l := range s.losses {
				if i >= 30 {
					break
				}
				lines = append(lines, fmt.Sprintf("| %sx | %s | %s%s | %s%s |",
					fmt3(l.ratio), l.name, mm.format(l.base), mm.unit,
					mm.format(l.chal), mm.unit))
			}
			lines = append(lines, "</details>")
		}
		f, err := os.OpenFile(o.summaryFile, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o666)
		if err != nil {
			fmt.Fprintf(os.Stderr, "h2h_compare: cannot write summary: %v\n", err)
			return 2
		}
		fmt.Fprintln(f, strings.Join(lines, "\n"))
		f.Close()
	}

	fmt.Print(out.String())
	if o.reportOnly {
		return 0
	}
	if gatePass {
		return 0
	}
	return 1
}
