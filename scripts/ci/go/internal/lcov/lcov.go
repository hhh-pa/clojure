// Package lcov parses, merges, and emits LCOV coverage records,
// mirroring scripts/ci/lib: coverage-floor-check.py's DA-only merge,
// coverage-summary.py's LF/LH reader, and rust-quality-gates.py's full
// DA/BRDA/FNDA/FN record model with absurd-count clamping.
package lcov

import (
	"math"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode/utf8"
)

// AbsurdCount bounds real execution counts: llvm occasionally emits
// u64::MAX on async-block boundaries. The full parser clamps counts
// above this (or below zero) to 0, mirroring _ABSURD_COUNT.
const AbsurdCount = int64(1) << 62

// Record is one file's coverage: DA line->count, BRDA
// (line,block,branch)->taken ("-" or decimal), FNDA name->count, and
// the FN set of (name,line).
type Record struct {
	DA   map[string]int64
	BRDA map[BRKey]string
	FNDA map[string]int64
	FN   map[FNKey]bool
}

// BRKey identifies one branch record.
type BRKey struct {
	Line   string
	Block  string
	Branch string
}

// FNKey identifies one function record.
type FNKey struct {
	Name string
	Line string
}

// SFRecord pairs a source path with its record. Duplicate SFs stay
// separate entries (merged later by Canonicalize), mirroring the
// Python list-of-tuples.
type SFRecord struct {
	SF  string
	Rec Record
}

// Sanitized reports one clamped absurd count.
type Sanitized struct {
	SF   string
	What string
	Raw  string
}

// NewRecord returns an empty Record.
func NewRecord() Record {
	return Record{
		DA:   map[string]int64{},
		BRDA: map[BRKey]string{},
		FNDA: map[string]int64{},
		FN:   map[FNKey]bool{},
	}
}

// splitLines mirrors text.splitlines + per-line strip: "\n"-split,
// "\r" kept unless the line strip removes it (strip trims all
// whitespace, including "\r").
func splitLines(text string) []string {
	raw := strings.Split(text, "\n")
	out := raw[:0]
	for _, line := range raw {
		// Drop the trailing empty element a final newline leaves,
		// which splitlines does not yield.
		out = append(out, strings.TrimSpace(line))
	}
	if len(out) > 0 && out[len(out)-1] == "" && strings.HasSuffix(text, "\n") {
		out = out[:len(out)-1]
	}
	return out
}

// parseCount parses a decimal count. Huge magnitudes saturate (Python
// ints are unbounded); non-numeric input fails.
func parseCount(raw string) (int64, bool) {
	count, err := strconv.ParseInt(strings.TrimSpace(raw), 10, 64)
	if err == nil {
		return count, true
	}
	if num, ok := err.(*strconv.NumError); ok && num.Err == strconv.ErrRange {
		if strings.HasPrefix(strings.TrimSpace(raw), "-") {
			return math.MinInt64, true
		}
		return math.MaxInt64, true
	}
	return 0, false
}

// saneCount clamps profiler-garbage counts to 0, recording the event.
// Non-numeric input fails (the record is skipped), mirroring _sane_count.
func saneCount(raw, sf, what string, sanitized *[]Sanitized) (int64, bool) {
	count, ok := parseCount(raw)
	if !ok {
		return 0, false
	}
	if count > AbsurdCount || count < 0 {
		*sanitized = append(*sanitized, Sanitized{SF: sf, What: what, Raw: raw})
		return 0, true
	}
	return count, true
}

func mergeBRDATaken(slot map[BRKey]string, key BRKey, taken string) {
	prev, present := slot[key]
	if !present || prev == "-" {
		slot[key] = taken
		return
	}
	if taken == "-" {
		return
	}
	a, aok := parseCount(prev)
	b, bok := parseCount(taken)
	if aok && bok {
		slot[key] = strconv.FormatInt(addSat(a, b), 10)
	}
}

// addSat adds with int64 saturation. Python ints are unbounded, but
// merged counts near 2**62 are profiler garbage that ParseRecords
// clamps; saturation keeps them non-negative instead of wrapping.
func addSat(a, b int64) int64 {
	if b > 0 && a > math.MaxInt64-b {
		return math.MaxInt64
	}
	if b < 0 && a < math.MinInt64-b {
		return math.MinInt64
	}
	return a + b
}

// ParseRecords parses full LCOV text, mirroring parse_lcov_records:
// summary lines are dropped (recomputed on emit), absurd counts are
// clamped and reported.
func ParseRecords(text string) ([]SFRecord, []Sanitized) {
	var records []SFRecord
	var sanitized []Sanitized
	var sf string
	haveSF := false
	rec := NewRecord()
	flush := func() {
		if haveSF {
			records = append(records, SFRecord{SF: sf, Rec: rec})
		}
	}
	for _, line := range splitLines(text) {
		switch {
		case strings.HasPrefix(line, "SF:"):
			flush()
			sf = line[len("SF:"):]
			rec = NewRecord()
			haveSF = true
		case line == "end_of_record":
			flush()
			haveSF = false
		case !haveSF:
			continue
		case strings.HasPrefix(line, "DA:"):
			parts := strings.Split(line[len("DA:"):], ",")
			if len(parts) < 2 {
				continue
			}
			count, ok := saneCount(parts[1], sf, "DA:"+parts[0], &sanitized)
			if !ok {
				continue
			}
			rec.DA[parts[0]] += count
		case strings.HasPrefix(line, "BRDA:"):
			parts := strings.Split(line[len("BRDA:"):], ",")
			if len(parts) < 4 {
				continue
			}
			taken := parts[3]
			if taken != "-" {
				sane, ok := saneCount(
					taken, sf,
					"BRDA:"+parts[0]+","+parts[1]+","+parts[2],
					&sanitized,
				)
				if !ok {
					taken = "-"
				} else {
					taken = strconv.FormatInt(sane, 10)
				}
			}
			mergeBRDATaken(rec.BRDA, BRKey{parts[0], parts[1], parts[2]}, taken)
		case strings.HasPrefix(line, "FNDA:"):
			parts := strings.SplitN(line[len("FNDA:"):], ",", 2)
			if len(parts) != 2 {
				continue
			}
			count, ok := saneCount(parts[0], sf, "FNDA:"+parts[1], &sanitized)
			if !ok {
				continue
			}
			rec.FNDA[parts[1]] += count
		case strings.HasPrefix(line, "FN:"):
			parts := strings.SplitN(line[len("FN:"):], ",", 2)
			if len(parts) == 2 {
				rec.FN[FNKey{parts[1], parts[0]}] = true
			}
		}
	}
	flush()
	return records, sanitized
}

// ParseDA merges DA-only records per file, mirroring
// coverage-floor-check.py's parse_lcov_lines (no count clamping).
func ParseDA(text string) map[string]map[string]int64 {
	files := map[string]map[string]int64{}
	var sf string
	haveSF := false
	for _, line := range splitLines(text) {
		switch {
		case strings.HasPrefix(line, "SF:"):
			sf = line[len("SF:"):]
			haveSF = true
			if _, ok := files[sf]; !ok {
				files[sf] = map[string]int64{}
			}
		case line == "end_of_record":
			haveSF = false
		case haveSF && strings.HasPrefix(line, "DA:"):
			parts := strings.Split(line[len("DA:"):], ",")
			if len(parts) < 2 {
				continue
			}
			count, ok := parseCount(parts[1])
			if !ok {
				continue
			}
			files[sf][parts[0]] += count
		}
	}
	return files
}

// FileSummary is one file's LF/LH totals, mirroring
// coverage-summary.py's parse_lcov rows.
type FileSummary struct {
	Path  string
	Found int64
	Hit   int64
}

// ParseSummaries reads raw LF/LH totals per file (trusted as-is, never
// recomputed), mirroring coverage-summary.py. Lines keep a trailing
// "\r" (Python rstrips "\n" only).
func ParseSummaries(text string) (found, hit int64, files []FileSummary) {
	var current *FileSummary
	for _, raw := range strings.Split(text, "\n") {
		line := strings.TrimSuffix(raw, "\n")
		switch {
		case strings.HasPrefix(line, "SF:"):
			current = &FileSummary{Path: line[len("SF:"):]}
		case current != nil && strings.HasPrefix(line, "LF:"):
			current.Found = mustCount(line[len("LF:"):])
		case current != nil && strings.HasPrefix(line, "LH:"):
			current.Hit = mustCount(line[len("LH:"):])
		case line == "end_of_record" && current != nil:
			files = append(files, *current)
			found += current.Found
			hit += current.Hit
			current = nil
		}
	}
	return found, hit, files
}

// mustCount parses summary counts. Python int() would crash on garbage
// here (no such input exists in real LCOV); fail closed at zero.
func mustCount(raw string) int64 {
	count, ok := parseCount(raw)
	if !ok {
		return 0
	}
	return count
}

// SanitizeUTF8 replaces each invalid UTF-8 byte with U+FFFD,
// mirroring Python errors="replace" on LCOV reads (a literal U+FFFD
// re-encodes identically).
func SanitizeUTF8(s string) string {
	if utf8.ValidString(s) {
		return s
	}
	var b strings.Builder
	b.Grow(len(s))
	for len(s) > 0 {
		r, w := utf8.DecodeRuneInString(s)
		if r == utf8.RuneError && w == 1 {
			b.WriteString("\uFFFD")
		} else {
			b.WriteString(s[:w])
		}
		s = s[w:]
	}
	return b.String()
}

// SortedSFs returns SFs in byte order.
func SortedSFs(files map[string]map[string]int64) []string {
	sfs := make([]string, 0, len(files))
	for sf := range files {
		sfs = append(sfs, sf)
	}
	sort.Strings(sfs)
	return sfs
}

// CovExcludedParts mirrors cargo-llvm-cov's report filtering
// (_COV_EXCLUDED_PARTS): test/bench/example sources, build output,
// and non-workspace code, matched AFTER normpath so
// `<crate>/tests/../src/*.rs` (#[path]-included production code)
// survives while `<crate>/tests/*.rs` does not.
var CovExcludedParts = []string{
	"/tests/",
	"/benches/",
	"/examples/",
	"/target/",
	"/.cargo/",
	"/rustup/",
	"/rustc/",
}

// NormPath mirrors posixpath.normpath. path.Clean differs only on a
// leading double slash, which POSIX preserves.
func NormPath(p string) string {
	if strings.HasPrefix(p, "//") && (len(p) == 2 || p[2] != '/') {
		return "/" + path.Clean(p)
	}
	return path.Clean(p)
}

func copyRecord(rec Record) Record {
	out := NewRecord()
	for k, v := range rec.DA {
		out.DA[k] = v
	}
	for k, v := range rec.BRDA {
		out.BRDA[k] = v
	}
	for k, v := range rec.FNDA {
		out.FNDA[k] = v
	}
	for k := range rec.FN {
		out.FN[k] = true
	}
	return out
}

// Canonicalize merges raw-export records onto canonical workspace
// paths, mirroring canonicalize_lcov_records. Lexical normpath
// collapses `<crate>/tests/../src/*.rs` twins onto the binary's own
// absolute paths so both copies merge; pure test sources,
// non-workspace paths, and excluded parts are dropped. Returns
// records sorted by SF.
func Canonicalize(records []SFRecord, workspace string) []SFRecord {
	merged := map[string]*Record{}
	prefix := workspace + string(os.PathSeparator)
	for _, sr := range records {
		p := sr.SF
		if !filepath.IsAbs(p) {
			p = filepath.Join(workspace, p)
		}
		norm := NormPath(p)
		if !strings.HasPrefix(norm, prefix) {
			continue
		}
		excluded := false
		for _, part := range CovExcludedParts {
			if strings.Contains(norm, part) {
				excluded = true
				break
			}
		}
		if excluded {
			continue
		}
		slot := merged[norm]
		if slot == nil {
			rec := copyRecord(sr.Rec)
			merged[norm] = &rec
			continue
		}
		for line, count := range sr.Rec.DA {
			slot.DA[line] = addSat(slot.DA[line], count)
		}
		for key, taken := range sr.Rec.BRDA {
			mergeBRDATaken(slot.BRDA, key, taken)
		}
		for name, count := range sr.Rec.FNDA {
			slot.FNDA[name] = addSat(slot.FNDA[name], count)
		}
		for key := range sr.Rec.FN {
			slot.FN[key] = true
		}
	}
	sfs := make([]string, 0, len(merged))
	for sf := range merged {
		sfs = append(sfs, sf)
	}
	sort.Strings(sfs)
	out := make([]SFRecord, 0, len(sfs))
	for _, sf := range sfs {
		out = append(out, SFRecord{SF: sf, Rec: *merged[sf]})
	}
	return out
}

// NormSFs normpaths record SFs without filtering, mirroring the
// cargo_canon view in upgrade_lcov_with_canonical.
func NormSFs(records []SFRecord) []SFRecord {
	out := make([]SFRecord, 0, len(records))
	for _, sr := range records {
		out = append(out, SFRecord{SF: NormPath(sr.SF), Rec: sr.Rec})
	}
	return out
}

// intKey parses a numeric sort key, mirroring the int() sort keys in
// emit_lcov (non-numeric keys, which crash the oracle, sort by raw
// string instead).
func intKey(s string) (int64, bool) {
	return parseCount(s)
}

func lessNumOrStr(a, b string) bool {
	ai, aok := intKey(a)
	bi, bok := intKey(b)
	if aok && bok && ai != bi {
		return ai < bi
	}
	return a < b
}

// Emit renders canonical [(sf, record)] back to LCOV with recomputed
// summaries, mirroring emit_lcov. Empty input yields "".
func Emit(canonical []SFRecord) string {
	var out []string
	for _, sr := range canonical {
		rec := sr.Rec
		out = append(out, "SF:"+sr.SF)
		fns := make([]FNKey, 0, len(rec.FN))
		for key := range rec.FN {
			fns = append(fns, key)
		}
		sort.Slice(fns, func(i, j int) bool {
			if fns[i].Name != fns[j].Name {
				return fns[i].Name < fns[j].Name
			}
			return fns[i].Line < fns[j].Line
		})
		for _, key := range fns {
			out = append(out, "FN:"+key.Line+","+key.Name)
		}
		fndas := make([]string, 0, len(rec.FNDA))
		for name := range rec.FNDA {
			fndas = append(fndas, name)
		}
		sort.Strings(fndas)
		for _, name := range fndas {
			out = append(out, "FNDA:"+strconv.FormatInt(rec.FNDA[name], 10)+","+name)
		}
		if len(rec.FN) > 0 || len(rec.FNDA) > 0 {
			names := map[string]bool{}
			for name := range rec.FNDA {
				names[name] = true
			}
			for _, key := range fns {
				names[key.Name] = true
			}
			hit := 0
			for name := range names {
				if rec.FNDA[name] > 0 {
					hit++
				}
			}
			out = append(out, "FNF:"+strconv.Itoa(len(names)))
			out = append(out, "FNH:"+strconv.Itoa(hit))
		}
		brs := make([]BRKey, 0, len(rec.BRDA))
		for key := range rec.BRDA {
			brs = append(brs, key)
		}
		sort.Slice(brs, func(i, j int) bool {
			a, b := brs[i], brs[j]
			if a.Line != b.Line {
				return lessNumOrStr(a.Line, b.Line)
			}
			if a.Block != b.Block {
				return lessNumOrStr(a.Block, b.Block)
			}
			if a.Branch != b.Branch {
				return lessNumOrStr(a.Branch, b.Branch)
			}
			return false
		})
		for _, key := range brs {
			out = append(out, "BRDA:"+key.Line+","+key.Block+","+key.Branch+","+rec.BRDA[key])
		}
		if len(rec.BRDA) > 0 {
			hit := 0
			for _, taken := range rec.BRDA {
				if taken != "-" && taken != "0" {
					hit++
				}
			}
			out = append(out, "BRF:"+strconv.Itoa(len(rec.BRDA)))
			out = append(out, "BRH:"+strconv.Itoa(hit))
		}
		das := make([]string, 0, len(rec.DA))
		for line := range rec.DA {
			das = append(das, line)
		}
		sort.Slice(das, func(i, j int) bool { return lessNumOrStr(das[i], das[j]) })
		for _, line := range das {
			out = append(out, "DA:"+line+","+strconv.FormatInt(rec.DA[line], 10))
		}
		hit := 0
		for _, count := range rec.DA {
			if count > 0 {
				hit++
			}
		}
		out = append(out, "LF:"+strconv.Itoa(len(rec.DA)))
		out = append(out, "LH:"+strconv.Itoa(hit))
		out = append(out, "end_of_record")
	}
	if len(out) == 0 {
		return ""
	}
	return strings.Join(out, "\n") + "\n"
}

// LineKey is one (file, line) pair in a coverage line set.
type LineKey struct {
	SF   string
	Line string
}

// LineSets returns the (total, covered) line sets for canonical
// records, mirroring lcov_line_sets.
func LineSets(canonical []SFRecord) (total, covered map[LineKey]bool) {
	total = map[LineKey]bool{}
	covered = map[LineKey]bool{}
	for _, sr := range canonical {
		for line, count := range sr.Rec.DA {
			key := LineKey{sr.SF, line}
			total[key] = true
			if count > 0 {
				covered[key] = true
			}
		}
	}
	return total, covered
}

// IsSubset reports whether every key of sub is in super.
func IsSubset(sub, super map[LineKey]bool) bool {
	for key := range sub {
		if !super[key] {
			return false
		}
	}
	return true
}
