package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

// Goldens were generated from the Python oracle
// (scripts/ci/h2h_compare.py, now removed) and diffed byte-for-byte
// at generation time:
//
//	python3 h2h_compare.py --label recipes \
//	  --jvm testdata/base-shard-0.json testdata/base-shard-1.json \
//	  --rs testdata/chal.json \
//	  --merged-jvm testdata/golden-merged-base.json \
//	  --merged-rs testdata/golden-merged-chal.json \
//	  --summary-file testdata/golden-summary-default.md \
//	  > testdata/golden-default.txt
//
// The ins/report-only goldens use the same fixtures with
// --metrics ins --margin 0.01 --base-name rs-to
// --challenger-name rs-from and --report-only respectively.

func TestGoldenDefault(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	work := t.TempDir()
	summary := filepath.Join(work, "summary.md")
	mergedJVM := filepath.Join(work, "merged-jvm.json")
	mergedRS := filepath.Join(work, "merged-rs.json")
	stdout, stderr, code := cmdtest.RunBothEnv(t, bin, ".", nil,
		"--label", "recipes",
		"--jvm", "testdata/base-shard-0.json", "testdata/base-shard-1.json",
		"--rs", "testdata/chal.json",
		"--merged-jvm", mergedJVM, "--merged-rs", mergedRS,
		"--summary-file", summary)
	if stderr != "" {
		t.Fatalf("stderr:\n%s", stderr)
	}
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	if stdout != cmdtest.Golden(t, "testdata/golden-default.txt") {
		t.Fatalf("stdout differs from golden:\n%s", diffText(cmdtest.Golden(t, "testdata/golden-default.txt"), stdout))
	}
	gotSummary, err := os.ReadFile(summary)
	if err != nil {
		t.Fatal(err)
	}
	if string(gotSummary) != cmdtest.Golden(t, "testdata/golden-summary-default.md") {
		t.Fatalf("summary differs from golden:\n%s", diffText(cmdtest.Golden(t, "testdata/golden-summary-default.md"), string(gotSummary)))
	}
	assertMergedEqual(t, "testdata/golden-merged-base.json", mergedJVM)
	assertMergedEqual(t, "testdata/golden-merged-chal.json", mergedRS)
}

func TestGoldenIns(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	work := t.TempDir()
	summary := filepath.Join(work, "summary.md")
	stdout, code := cmdtest.Run(t, bin, ".",
		"--label", "recipes-rsrs",
		"--base-name", "rs-to", "--challenger-name", "rs-from",
		"--jvm", "testdata/base-shard-0.json", "testdata/base-shard-1.json",
		"--rs", "testdata/chal.json",
		"--metrics", "ins", "--margin", "0.01",
		"--summary-file", summary)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	if stdout != cmdtest.Golden(t, "testdata/golden-ins.txt") {
		t.Fatalf("stdout differs from golden:\n%s", diffText(cmdtest.Golden(t, "testdata/golden-ins.txt"), stdout))
	}
	gotSummary, err := os.ReadFile(summary)
	if err != nil {
		t.Fatal(err)
	}
	if string(gotSummary) != cmdtest.Golden(t, "testdata/golden-summary-ins.md") {
		t.Fatalf("summary differs from golden:\n%s", diffText(cmdtest.Golden(t, "testdata/golden-summary-ins.md"), string(gotSummary)))
	}
}

func TestGoldenReportOnly(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	stdout, code := cmdtest.Run(t, bin, ".",
		"--label", "recipes",
		"--jvm", "testdata/base-shard-0.json", "testdata/base-shard-1.json",
		"--rs", "testdata/chal.json",
		"--report-only")
	if code != 0 {
		t.Fatalf("exit = %d, want 0", code)
	}
	if stdout != cmdtest.Golden(t, "testdata/golden-reportonly.txt") {
		t.Fatalf("stdout differs from golden:\n%s", diffText(cmdtest.Golden(t, "testdata/golden-reportonly.txt"), stdout))
	}
}

func TestUsageErrors(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{
		{},
		{"--label", "x"},
		{"--label", "x", "--jvm", "testdata/chal.json"},
		{"--label", "x", "--jvm", "testdata/chal.json", "--rs", "testdata/chal.json", "--bogus"},
		{"--label", "x", "--jvm", "testdata/chal.json", "--rs", "testdata/chal.json", "--margin", "abc"},
		{"--label", "x", "--jvm", "testdata/chal.json", "--rs", "testdata/chal.json", "--metrics", "cpu,bogus"},
		{"--label", "x", "--jvm", "testdata/does-not-exist.json", "--rs", "testdata/chal.json"},
	} {
		_, stderr, code := cmdtest.RunBothEnv(t, bin, ".", nil, args...)
		if code != 2 {
			t.Fatalf("%v: exit = %d, want 2", args, code)
		}
		if !strings.HasPrefix(stderr, "h2h_compare: ") {
			t.Fatalf("%v: stderr = %q, want h2h_compare prefix", args, stderr)
		}
	}
}

// assertMergedEqual compares merged leg JSONs semantically: Go sorts
// object keys where Python preserves insertion order, so only the
// parsed content must match.
func assertMergedEqual(t *testing.T, goldenPath, gotPath string) {
	t.Helper()
	decode := func(path string) any {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var v any
		if err := json.Unmarshal(data, &v); err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		return v
	}
	if !reflect.DeepEqual(decode(goldenPath), decode(gotPath)) {
		t.Fatalf("merged %s differs semantically from %s", gotPath, goldenPath)
	}
}

func diffText(want, got string) string {
	w, g := strings.Split(want, "\n"), strings.Split(got, "\n")
	var b strings.Builder
	for i := 0; i < len(w) || i < len(g); i++ {
		var x, y string
		if i < len(w) {
			x = w[i]
		}
		if i < len(g) {
			y = g[i]
		}
		mark := " "
		if x != y {
			mark = "!"
		}
		b.WriteString(mark + " want: " + x + "\n")
		b.WriteString(mark + "  got: " + y + "\n")
		if i > 40 {
			b.WriteString("... (truncated)\n")
			break
		}
	}
	return b.String()
}

func TestParseArgs(t *testing.T) {
	o, err := parseArgs([]string{"--label", "r", "--jvm", "a.json", "b.json", "--rs", "c.json"})
	if err != nil {
		t.Fatal(err)
	}
	if len(o.jvm) != 2 || o.jvm[1] != "b.json" || len(o.rs) != 1 {
		t.Fatalf("nargs collect: %+v", o)
	}
	o, err = parseArgs([]string{"--label=x", "--jvm=a.json", "--jvm=b.json", "--rs=c.json", "--margin=0.01", "--metrics=ins"})
	if err != nil {
		t.Fatal(err)
	}
	if o.label != "x" || len(o.jvm) != 2 || o.margin != 0.01 || o.metrics != "ins" {
		t.Fatalf("= form: %+v", o)
	}
	if _, err := parseArgs([]string{"--label", "x", "--jvm", "a", "--rs"}); err == nil {
		t.Fatal("dangling --rs must fail")
	}
}

func TestFormats(t *testing.T) {
	if got := fmtComma(1000000); got != "1,000,000" {
		t.Fatalf("fmtComma(1e6) = %q", got)
	}
	if got := fmtComma(999); got != "999" {
		t.Fatalf("fmtComma(999) = %q", got)
	}
	if got := fmtComma(0); got != "0" {
		t.Fatalf("fmtComma(0) = %q", got)
	}
	if got := fmt3(2.5); got != "2.500" {
		t.Fatalf("fmt3(2.5) = %q", got)
	}
}

func TestShort(t *testing.T) {
	if got := short(caseKey{"l", "s", "run"}); got != "l" {
		t.Fatalf("short run = %q", got)
	}
	if got := short(caseKey{"l", "s", "c"}); got != "l/c" {
		t.Fatalf("short case = %q", got)
	}
}
