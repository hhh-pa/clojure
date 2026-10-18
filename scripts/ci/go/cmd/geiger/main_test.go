package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

func TestUsageGolden(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{nil, {"--help"}} {
		out, code := cmdtest.Run(t, bin, "testdata", args...)
		if code != 2 {
			t.Fatalf("%v: exit = %d, want 2", args, code)
		}
		if want := cmdtest.Golden(t, "testdata/usage.golden"); out != want {
			t.Fatalf("%v mismatch:\n got: %q\nwant: %q", args, out, want)
		}
	}
}

// stubEnv builds the child environment for stub runs: stub cargo
// first on PATH, fixture data, per-call argv log, plus extra vars.
func stubEnv(t *testing.T, log string, extra ...string) []string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("stub cargo needs sh")
	}
	stubs, err := filepath.Abs("testdata/stubs")
	if err != nil {
		t.Fatal(err)
	}
	data, err := filepath.Abs("testdata/stub-data")
	if err != nil {
		t.Fatal(err)
	}
	env := []string{
		"PATH=" + stubs + string(os.PathListSeparator) + os.Getenv("PATH"),
		"STUB_DATA=" + data,
		"STUB_LOG=" + log,
	}
	return append(env, extra...)
}

// stubWS creates a workspace dir holding baseline ("" omits the file).
func stubWS(t *testing.T, baseline string) string {
	t.Helper()
	ws := t.TempDir()
	if baseline != "" {
		if err := os.WriteFile(filepath.Join(ws, "geiger-baseline.json"), []byte(baseline), 0o666); err != nil {
			t.Fatal(err)
		}
	}
	return ws
}

func TestStubGoldens(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	cases := []struct {
		name     string
		baseline string
		env      []string
		args     []string
		exit     int
	}{
		{"ok", `{"unsafe_total": 4}`, nil, nil, 0},
		{"grew", `{"unsafe_total": 3}`, nil, nil, 1},
		{"fell", `{"unsafe_total": 9}`, nil, nil, 0},
		{"regen", "", nil, []string{"--regen"}, 0},
		{"meta-fail", `{"unsafe_total": 4}`, []string{"STUB_FAIL_METADATA=1"}, nil, 2},
		{"geiger-fail", `{"unsafe_total": 4}`, []string{"STUB_FAIL_GEIGER=alpha"}, nil, 2},
		{"nojson", `{"unsafe_total": 4}`, []string{"STUB_NOJSON=beta"}, nil, 2},
		{"nobase", "", nil, nil, 2},
		{"missing-key", `{"nope": 1}`, nil, nil, 2},
		// bad-json pins Go's JSON error text: CPython's json
		// messages cannot be reproduced (exit code still matches).
		{"bad-json", `{not json`, nil, nil, 2},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			ws := stubWS(t, c.baseline)
			log := filepath.Join(t.TempDir(), "argv.log")
			args := append([]string{ws}, c.args...)
			out, code := cmdtest.RunEnv(t, bin, ".", stubEnv(t, log, c.env...), args...)
			if code != c.exit {
				t.Fatalf("exit = %d, want %d\n%s", code, c.exit, out)
			}
			// Goldens use $WS for the temp workspace path.
			got := strings.ReplaceAll(out, ws, "$WS")
			want := cmdtest.Golden(t, filepath.Join("testdata", c.name+".golden"))
			if got != want {
				t.Fatalf("output mismatch:\n got: %q\nwant: %q", got, want)
			}
			if c.name == "regen" {
				data, err := os.ReadFile(filepath.Join(ws, "geiger-baseline.json"))
				if err != nil {
					t.Fatal(err)
				}
				if want := cmdtest.Golden(t, "testdata/regen-baseline.golden"); string(data) != want {
					t.Fatalf("baseline mismatch:\n got: %q\nwant: %q", data, want)
				}
			}
			if c.name == "ok" {
				argv, err := os.ReadFile(log)
				if err != nil {
					t.Fatal(err)
				}
				if want := cmdtest.Golden(t, "testdata/argv.golden"); string(argv) != want {
					t.Fatalf("argv mismatch:\n got: %q\nwant: %q", argv, want)
				}
			}
		})
	}
}

// TestOracleDiff runs the Python oracle under the same stubs and
// requires identical stdout, argv, exit codes, and regen bytes.
func TestOracleDiff(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("stub cargo needs sh")
	}
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("python3 not on PATH")
	}
	oracle, err := filepath.Abs("../../../lib/geiger-check.py")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(oracle); err != nil {
		t.Skip("oracle removed; golden tests own the contract now")
	}
	bin := cmdtest.Build(t, ".")
	cases := []struct {
		name     string
		baseline string
		env      []string
		args     []string
	}{
		{"ok", `{"unsafe_total": 4}`, nil, nil},
		{"grew", `{"unsafe_total": 3}`, nil, nil},
		{"fell", `{"unsafe_total": 9}`, nil, nil},
		{"regen", "", nil, []string{"--regen"}},
		{"meta-fail", `{"unsafe_total": 4}`, []string{"STUB_FAIL_METADATA=1"}, nil},
		{"geiger-fail", `{"unsafe_total": 4}`, []string{"STUB_FAIL_GEIGER=alpha"}, nil},
		{"nojson", `{"unsafe_total": 4}`, []string{"STUB_NOJSON=beta"}, nil},
		{"nobase", "", nil, nil},
		{"missing-key", `{"nope": 1}`, nil, nil},
		// bad-json excluded: JSON-engine error text differs by
		// design (exit code 2 still matches; see goldens).
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			goWS := stubWS(t, c.baseline)
			pyWS := stubWS(t, c.baseline)
			goLog := filepath.Join(t.TempDir(), "go.log")
			pyLog := filepath.Join(t.TempDir(), "py.log")
			goOut, goCode := cmdtest.RunEnv(t, bin, ".",
				stubEnv(t, goLog, c.env...), append([]string{goWS}, c.args...)...)
			pyArgv := append([]string{oracle, pyWS}, c.args...)
			pyOut, pyCode := cmdtest.RunEnv(t, python, ".",
				stubEnv(t, pyLog, c.env...), pyArgv...)
			if goCode != pyCode {
				t.Fatalf("exit = %d, oracle = %d\n%s", goCode, pyCode, goOut)
			}
			goNorm := strings.ReplaceAll(goOut, goWS, "$WS")
			pyNorm := strings.ReplaceAll(pyOut, pyWS, "$WS")
			if goNorm != pyNorm {
				t.Fatalf("output mismatch:\n got: %q\nwant: %q", goNorm, pyNorm)
			}
			goArgv, _ := os.ReadFile(goLog)
			pyArgvLog, _ := os.ReadFile(pyLog)
			if string(goArgv) != string(pyArgvLog) {
				t.Fatalf("argv mismatch:\n got: %q\nwant: %q", goArgv, pyArgvLog)
			}
			if len(c.args) > 0 && c.args[0] == "--regen" {
				goBase, _ := os.ReadFile(filepath.Join(goWS, "geiger-baseline.json"))
				pyBase, _ := os.ReadFile(filepath.Join(pyWS, "geiger-baseline.json"))
				if string(goBase) != string(pyBase) {
					t.Fatalf("baseline mismatch:\n got: %q\nwant: %q", goBase, pyBase)
				}
			}
		})
	}
}
