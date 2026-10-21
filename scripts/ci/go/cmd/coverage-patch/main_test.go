package main

import (
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
		{"pass", []string{"patch.lcov", "pass.diff"}, 0, "testdata"},
		{"fail", []string{"patch.lcov", "fail.diff", "100"}, 1, "testdata"},
		{"default-floor", []string{"patch.lcov", "fail.diff"}, 1, "testdata"},
		{"empty", []string{"patch.lcov", "empty.diff"}, 0, "testdata"},
		{"usage", nil, 2, "testdata"},
		{"usage-many", []string{"a", "b", "c", "d"}, 2, "testdata"},
		{"badfloor", []string{"patch.lcov", "pass.diff", "xx"}, 2, "testdata"},
		{"missing-lcov", []string{"nope.lcov", "pass.diff"}, 1, "testdata"},
		{"missing-diff", []string{"patch.lcov", "nope.diff"}, 1, "testdata"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			out, code := cmdtest.Run(t, bin, c.works, c.args...)
			if code != c.exit {
				t.Fatalf("exit = %d, want %d\n%s", code, c.exit, out)
			}
			golden := c.name + ".golden"
			if c.name == "usage-many" {
				golden = "usage.golden"
			}
			want := cmdtest.Golden(t, filepath.Join("testdata", golden))
			if out != want {
				t.Fatalf("output mismatch:\n got: %q\nwant: %q", out, want)
			}
		})
	}
}
