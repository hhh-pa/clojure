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
		{"pass", []string{"floor.lcov", "50"}, 0, "testdata"},
		{"fail", []string{"floor.lcov", "80"}, 1, "testdata"},
		{"usage", nil, 2, "testdata"},
		{"badfloor", []string{"floor.lcov", "xx"}, 2, "testdata"},
		{"missing", []string{"nope.lcov"}, 1, "testdata"},
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
