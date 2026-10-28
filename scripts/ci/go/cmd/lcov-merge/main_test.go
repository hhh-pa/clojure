package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

func TestMerge(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	out := filepath.Join(t.TempDir(), "merged.lcov")
	stdout, code := cmdtest.Run(t, bin, "testdata", ".", out, "merge-a.lcov", "merge-b.lcov")
	if code != 0 {
		t.Fatalf("exit = %d\n%s", code, stdout)
	}
	ws, err := filepath.Abs("testdata")
	if err != nil {
		t.Fatal(err)
	}
	norm := func(s string) string {
		s = strings.ReplaceAll(s, ws, "$WS")
		return strings.ReplaceAll(s, out, "$OUT")
	}
	if want := cmdtest.Golden(t, "testdata/merge.golden"); norm(stdout) != want {
		t.Fatalf("stdout mismatch:\n got: %q\nwant: %q", norm(stdout), want)
	}
	data, err := os.ReadFile(out)
	if err != nil {
		t.Fatal(err)
	}
	if want := cmdtest.Golden(t, "testdata/merged.lcov.golden"); norm(string(data)) != want {
		t.Fatalf("lcov mismatch:\n got: %q\nwant: %q", norm(string(data)), want)
	}
}

func TestUsageGolden(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{nil, {"onlyws"}, {"ws", "out.lcov"}} {
		out, stderr, code := cmdtest.RunBothEnv(t, bin, "testdata", nil, args...)
		if code != 2 {
			t.Fatalf("%v: exit = %d, want 2", args, code)
		}
		if stderr != "" {
			t.Fatalf("%v: unexpected stderr %q", args, stderr)
		}
		if want := cmdtest.Golden(t, "testdata/usage.golden"); out != want {
			t.Fatalf("%v mismatch:\n got: %q\nwant: %q", args, out, want)
		}
	}
}

func TestMissingInput(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	out := filepath.Join(t.TempDir(), "merged.lcov")
	stdout, stderr, code := cmdtest.RunBothEnv(t, bin, "testdata", nil,
		".", out, "merge-a.lcov", "nope.lcov")
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	if stdout != "" {
		t.Fatalf("stdout should be empty, got %q", stdout)
	}
	if !strings.HasPrefix(stderr, "lcov-merge: open nope.lcov: ") {
		t.Fatalf("stderr = %q", stderr)
	}
}

func TestEmptyMerge(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	ws, err := filepath.Abs("testdata")
	if err != nil {
		t.Fatal(err)
	}
	empty := filepath.Join(t.TempDir(), "empty.lcov")
	if err := os.WriteFile(empty, []byte("# nothing here\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "merged.lcov")
	stdout, stderr, code := cmdtest.RunBothEnv(t, bin, "testdata", nil, ".", out, empty)
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	if stdout != "" {
		t.Fatalf("stdout should be empty, got %q", stdout)
	}
	want := "lcov-merge: no records under " + ws + "\n"
	if stderr != want {
		t.Fatalf("stderr mismatch:\n got: %q\nwant: %q", stderr, want)
	}
}
