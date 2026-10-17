package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"

	"github.com/hhh-pa/arcanist/scripts/ci/go/internal/cmdtest"
)

func TestUsageGolden(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{nil, {"-h"}} {
		out, code := cmdtest.Run(t, bin, "testdata", args...)
		if code != 2 {
			t.Fatalf("%v: exit = %d, want 2", args, code)
		}
		if want := cmdtest.Golden(t, "testdata/usage.golden"); out != want {
			t.Fatalf("%v mismatch:\n got: %q\nwant: %q", args, out, want)
		}
	}
}

func stubEnv(t *testing.T, log string, extra ...string) []string {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("stub rca needs sh")
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

// stubWS copies the wssrc fixture (a.rs, b.rs, sub/c.rs plus excluded
// target/.git trees) to a temp workspace holding baseline ("" omits).
func stubWS(t *testing.T, baseline string) string {
	t.Helper()
	ws := t.TempDir()
	entries, err := os.ReadDir("testdata/wssrc")
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		src := filepath.Join("testdata/wssrc", e.Name())
		dst := filepath.Join(ws, e.Name())
		if e.IsDir() {
			copyDir(t, src, dst)
			continue
		}
		data, err := os.ReadFile(src)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(dst, data, 0o666); err != nil {
			t.Fatal(err)
		}
	}
	if baseline != "" {
		if err := os.WriteFile(filepath.Join(ws, "complexity-baseline.json"), []byte(baseline), 0o666); err != nil {
			t.Fatal(err)
		}
	}
	// A nested .git tree cannot be committed, so stage the .git
	// exclusion fixture here (collect must skip it like target/).
	if err := os.MkdirAll(filepath.Join(ws, ".git"), 0o777); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(ws, ".git", "z.rs"), []byte("fn z() {}\n"), 0o666); err != nil {
		t.Fatal(err)
	}
	return ws
}

func copyDir(t *testing.T, src, dst string) {
	t.Helper()
	if err := os.MkdirAll(dst, 0o777); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(src)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		data, err := os.ReadFile(filepath.Join(src, e.Name()))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dst, e.Name()), data, 0o666); err != nil {
			t.Fatal(err)
		}
	}
}

const (
	basePartial = `{"violations": [{"file": "a.rs", "function": "big", "cyclomatic": 30, "cognitive": 5}]}`
	baseFull    = `{"violations": [` +
		`{"file": "a.rs", "function": "big", "cyclomatic": 30, "cognitive": 5}, ` +
		`{"file": "a.rs", "function": "big::inner", "cyclomatic": 16, "cognitive": 1}, ` +
		`{"file": "b.rs", "function": "cog", "cyclomatic": 3, "cognitive": 25}, ` +
		`{"file": "sub/c.rs", "function": "Foo::method", "cyclomatic": 20, "cognitive": 1}]}`
	baseWorse = `{"violations": [` +
		`{"file": "a.rs", "function": "big", "cyclomatic": 29, "cognitive": 5}, ` +
		`{"file": "a.rs", "function": "big::inner", "cyclomatic": 16, "cognitive": 1}, ` +
		`{"file": "b.rs", "function": "cog", "cyclomatic": 3, "cognitive": 25}, ` +
		`{"file": "sub/c.rs", "function": "Foo::method", "cyclomatic": 20, "cognitive": 1}]}`
	baseBetter = `{"violations": [` +
		`{"file": "a.rs", "function": "big", "cyclomatic": 40, "cognitive": 9}, ` +
		`{"file": "a.rs", "function": "big::inner", "cyclomatic": 16, "cognitive": 1}, ` +
		`{"file": "b.rs", "function": "cog", "cyclomatic": 3, "cognitive": 25}, ` +
		`{"file": "sub/c.rs", "function": "Foo::method", "cyclomatic": 20, "cognitive": 1}]}`
	baseStale = `{"violations": [` +
		`{"file": "a.rs", "function": "big", "cyclomatic": 30, "cognitive": 5}, ` +
		`{"file": "a.rs", "function": "big::inner", "cyclomatic": 16, "cognitive": 1}, ` +
		`{"file": "b.rs", "function": "cog", "cyclomatic": 3, "cognitive": 25}, ` +
		`{"file": "sub/c.rs", "function": "Foo::method", "cyclomatic": 20, "cognitive": 1}, ` +
		`{"file": "gone.rs", "function": "old", "cyclomatic": 99, "cognitive": 99}]}`
	baseNoKey = `{"nope": 1}`
	baseBad   = `{oops`
)

func TestStubGoldens(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	cases := []struct {
		name     string
		baseline string
		env      []string
		args     []string
		exit     int
	}{
		{"new", basePartial, nil, nil, 1},
		{"ok-full", baseFull, nil, nil, 0},
		{"worsened", baseWorse, nil, nil, 1},
		{"improved", baseBetter, nil, nil, 0},
		{"stale", baseStale, nil, nil, 0},
		{"zero", baseFull, []string{"STUB_EMPTY=1"}, nil, 1},
		{"regen", "", nil, []string{"--regen"}, 0},
		{"rca-fail", baseFull, []string{"STUB_RCA_FAIL=1"}, nil, 2},
		{"nobase", "", nil, nil, 2},
		{"missing-key", baseNoKey, nil, nil, 2},
		// bad-json pins Go's JSON error text: CPython's json
		// messages cannot be reproduced (exit code still matches).
		{"bad-json", baseBad, nil, nil, 2},
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
			got := strings.ReplaceAll(out, ws, "$WS")
			want := cmdtest.Golden(t, filepath.Join("testdata", c.name+".golden"))
			if got != want {
				t.Fatalf("output mismatch:\n got: %q\nwant: %q", got, want)
			}
			if c.name == "regen" {
				data, err := os.ReadFile(filepath.Join(ws, "complexity-baseline.json"))
				if err != nil {
					t.Fatal(err)
				}
				if want := cmdtest.Golden(t, "testdata/regen-baseline.golden"); string(data) != want {
					t.Fatalf("baseline mismatch:\n got: %q\nwant: %q", data, want)
				}
			}
			if c.name == "new" {
				argv, err := os.ReadFile(log)
				if err != nil {
					t.Fatal(err)
				}
				got := regexp.MustCompile(`-o \S+`).ReplaceAllString(string(argv), "-o OUT")
				if want := cmdtest.Golden(t, "testdata/argv.golden"); got != want {
					t.Fatalf("argv mismatch:\n got: %q\nwant: %q", got, want)
				}
			}
		})
	}
}

func TestNoRustFiles(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("stub rca needs sh")
	}
	bin := cmdtest.Build(t, ".")
	ws := t.TempDir()
	for _, dir := range []string{"target", "target-foo"} {
		if err := os.MkdirAll(filepath.Join(ws, dir), 0o777); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(ws, dir, "x.rs"), []byte("fn x() {}\n"), 0o666); err != nil {
			t.Fatal(err)
		}
	}
	out, code := cmdtest.RunEnv(t, bin, ".", stubEnv(t, filepath.Join(ws, "argv.log")), ws)
	if code != 2 {
		t.Fatalf("exit = %d, want 2\n%s", code, out)
	}
	got := strings.ReplaceAll(out, ws, "$WS")
	if want := cmdtest.Golden(t, "testdata/no-rs.golden"); got != want {
		t.Fatalf("output mismatch:\n got: %q\nwant: %q", got, want)
	}
}

// TestUnparseableOutput asserts the shape of the blind-spot error
// (the rca outdir path is random per run, so no golden).
func TestUnparseableOutput(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("stub rca needs sh")
	}
	bin := cmdtest.Build(t, ".")
	ws := stubWS(t, baseFull)
	out, code := cmdtest.RunEnv(t, bin, ".",
		stubEnv(t, filepath.Join(t.TempDir(), "argv.log"), "STUB_BADJSON=a"), ws)
	if code != 2 {
		t.Fatalf("exit = %d, want 2\n%s", code, out)
	}
	lines := strings.Split(strings.TrimSuffix(out, "\n"), "\n")
	if len(lines) != 2 ||
		lines[0] != "unparseable rca output (1 file(s), blind spot):" ||
		!strings.HasSuffix(lines[1], "/a.rs.json") {
		t.Fatalf("unexpected shape:\n%q", out)
	}
}

// TestOracleDiff runs the Python oracle under the same stubs and
// requires identical stdout, argv, exit codes, and regen bytes.
func TestOracleDiff(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("stub rca needs sh")
	}
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("python3 not on PATH")
	}
	oracle, err := filepath.Abs("../../../lib/complexity-check.py")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(oracle); err != nil {
		t.Skip("oracle removed; golden tests own the contract now")
	}
	bin := cmdtest.Build(t, ".")
	outdir := regexp.MustCompile(`-o \S+`)
	cases := []struct {
		name     string
		baseline string
		env      []string
		args     []string
	}{
		{"new", basePartial, nil, nil},
		{"ok-full", baseFull, nil, nil},
		{"worsened", baseWorse, nil, nil},
		{"improved", baseBetter, nil, nil},
		{"stale", baseStale, nil, nil},
		{"zero", baseFull, []string{"STUB_EMPTY=1"}, nil},
		{"regen", "", nil, []string{"--regen"}},
		{"rca-fail", baseFull, []string{"STUB_RCA_FAIL=1"}, nil},
		{"nobase", "", nil, nil},
		{"missing-key", baseNoKey, nil, nil},
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
			if outdir.ReplaceAllString(string(goArgv), "-o OUT") !=
				outdir.ReplaceAllString(string(pyArgvLog), "-o OUT") {
				t.Fatalf("argv mismatch:\n got: %q\nwant: %q", goArgv, pyArgvLog)
			}
			if len(c.args) > 0 && c.args[0] == "--regen" {
				goBase, _ := os.ReadFile(filepath.Join(goWS, "complexity-baseline.json"))
				pyBase, _ := os.ReadFile(filepath.Join(pyWS, "complexity-baseline.json"))
				if string(goBase) != string(pyBase) {
					t.Fatalf("baseline mismatch:\n got: %q\nwant: %q", goBase, pyBase)
				}
			}
		})
	}
}
