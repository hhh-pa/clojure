package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

func TestGoldenPaths(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	cases := []struct {
		name  string
		args  []string
		exit  int
		works string
	}{
		{"mixed", []string{"mixed.lcov"}, 0, "testdata"},
		{"empty", []string{"empty.lcov"}, 0, "testdata"},
		{"missing", []string{"nope.lcov"}, 0, "testdata"},
		{"usage", nil, 2, "testdata"},
		{"usage-out", []string{"--out"}, 2, "testdata"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			out, code := cmdtest.Run(t, bin, c.works, c.args...)
			if code != c.exit {
				t.Fatalf("exit = %d, want %d\n%s", code, c.exit, out)
			}
			want := cmdtest.Golden(t, filepath.Join("testdata", c.name+".golden"))
			if out != want {
				t.Fatalf("output mismatch:\n got: %q\nwant: %q", out, want)
			}
		})
	}
}

// scrub clears the GITHUB_* link variables so ambient CI values cannot
// leak into the comment flavor.
var scrub = []string{
	"GITHUB_SERVER_URL=",
	"GITHUB_REPOSITORY=",
	"GITHUB_RUN_ID=",
}

func TestOutFile(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	dir := t.TempDir()
	comment := filepath.Join(dir, "sub", "comment.md")
	out, code := cmdtest.RunEnv(t, bin, "testdata", scrub, "--out", comment, "mixed.lcov")
	if code != 0 {
		t.Fatalf("exit = %d, want 0\n%s", code, out)
	}
	if out != cmdtest.Golden(t, "testdata/mixed.golden") {
		t.Fatalf("stdout mismatch:\n got: %q", out)
	}
	data, err := os.ReadFile(comment)
	if err != nil {
		t.Fatal(err)
	}
	if want := cmdtest.Golden(t, "testdata/comment.golden"); string(data) != want {
		t.Fatalf("comment mismatch:\n got: %q\nwant: %q", data, want)
	}
}

func TestOutFileRunLink(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	comment := filepath.Join(t.TempDir(), "comment.md")
	env := []string{
		"GITHUB_SERVER_URL=https://github.example",
		"GITHUB_REPOSITORY=acme/widgets",
		"GITHUB_RUN_ID=4242",
	}
	_, code := cmdtest.RunEnv(t, bin, "testdata", env, "--out", comment, "mixed.lcov")
	if code != 0 {
		t.Fatalf("exit = %d, want 0", code)
	}
	data, err := os.ReadFile(comment)
	if err != nil {
		t.Fatal(err)
	}
	if want := cmdtest.Golden(t, "testdata/comment-link.golden"); string(data) != want {
		t.Fatalf("comment mismatch:\n got: %q\nwant: %q", data, want)
	}
}

// TestOracleDiff runs the Python oracle on the same fixtures and
// requires byte-identical stdout.
func TestOracleDiff(t *testing.T) {
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("python3 not on PATH")
	}
	oracle, err := filepath.Abs("../../../lib/coverage-summary.py")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(oracle); err != nil {
		t.Skip("oracle removed; golden tests own the contract now")
	}
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{
		{"mixed.lcov"},
		{"empty.lcov"},
		{"nope.lcov"},
	} {
		argv := append([]string{oracle}, args...)
		want, code := cmdtest.Run(t, python, "testdata", argv...)
		if code != 0 {
			t.Fatalf("oracle %v exited %d", args, code)
		}
		got, code := cmdtest.Run(t, bin, "testdata", args...)
		if code != 0 {
			t.Fatalf("go %v exited %d", args, code)
		}
		if got != want {
			t.Fatalf("%v mismatch:\n got: %q\nwant: %q", args, got, want)
		}
	}
	// --out: stdout and file bytes must both match (scrubbed env).
	goOut := filepath.Join(t.TempDir(), "go.md")
	pyOut := filepath.Join(t.TempDir(), "py.md")
	want, _ := cmdtest.RunEnv(t, python, "testdata", scrub, oracle, "--out", pyOut, "mixed.lcov")
	got, _ := cmdtest.RunEnv(t, bin, "testdata", scrub, "--out", goOut, "mixed.lcov")
	if got != want {
		t.Fatalf("--out stdout mismatch:\n got: %q\nwant: %q", got, want)
	}
	goData, _ := os.ReadFile(goOut)
	pyData, _ := os.ReadFile(pyOut)
	if string(goData) != string(pyData) {
		t.Fatalf("--out file mismatch:\n got: %q\nwant: %q", goData, pyData)
	}
}
