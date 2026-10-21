package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

func TestUsageGolden(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	for _, args := range [][]string{nil, {"--help"}, {"onlyws"}} {
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

func TestNextestRefusal(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	stdout, stderr, code := cmdtest.RunBothEnv(t, bin, "testdata", nil,
		"ws", "report", "nextest", "--run-ignored", "run", "--no-fail-fast")
	if code != 1 {
		t.Fatalf("exit = %d, want 1", code)
	}
	if stdout != "" {
		t.Fatalf("stdout should be empty, got %q", stdout)
	}
	if want := cmdtest.Golden(t, "testdata/refusal.stderr.golden"); stderr != want {
		t.Fatalf("stderr mismatch:\n got: %q\nwant: %q", stderr, want)
	}
	// "--run-ignored" is membership-safe: only a literal "run" refuses.
	if runtime.GOOS == "windows" {
		t.Skip("stub tools need sh")
	}
	tools := buildTools(t)
	ws, report, env := stubWorld(t, nil)
	_, _, code = cmdtest.RunBothEnv(t, filepath.Join(tools, "rust-quality-gates"), ".", env, ws, report,
		"nextest", "--run-ignored", "all", "--no-fail-fast")
	if code != 0 {
		t.Fatalf("CI runner refused, exit = %d", code)
	}
}

// buildTools compiles the gates plus their sibling tools into one dir.
func buildTools(t *testing.T) string {
	t.Helper()
	bin := t.TempDir()
	cmdtest.BuildTo(t, ".", filepath.Join(bin, "rust-quality-gates"))
	cmdtest.BuildTo(t, "../coverage-floor", filepath.Join(bin, "coverage-floor"))
	cmdtest.BuildTo(t, "../coverage-patch", filepath.Join(bin, "coverage-patch"))
	cmdtest.BuildTo(t, "../geiger", filepath.Join(bin, "geiger"))
	cmdtest.BuildTo(t, "../complexity", filepath.Join(bin, "complexity"))
	return bin
}

// stubWorld copies the wsg fixture workspace, stages a stub llvm
// sysroot, and returns (workspace, reportDir, env). Paths are
// symlink-resolved so stub-emitted physical paths match.
func stubWorld(t *testing.T, extra []string) (string, string, []string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("stub tools need sh")
	}
	stubs, err := filepath.Abs("testdata/stubs")
	if err != nil {
		t.Fatal(err)
	}
	data, err := filepath.Abs("testdata/stub-data")
	if err != nil {
		t.Fatal(err)
	}
	ws, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	os.Remove(ws)
	copyTreeFixture(t, "testdata/wsg", ws)
	report, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	sysroot, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	binDir := filepath.Join(sysroot, "lib", "rustlib", "x86_64-stub", "bin")
	if err := os.MkdirAll(binDir, 0o777); err != nil {
		t.Fatal(err)
	}
	for _, tool := range []string{"llvm-cov", "llvm-profdata"} {
		blob, err := os.ReadFile(filepath.Join(stubs, tool))
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(binDir, tool), blob, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	for _, kv := range extra {
		if kv != "STUB_PATCH=1" {
			continue
		}
		// Stage a patch touching covered stub lines and point
		// PATCH_DIFF at it, so the case below exercises the
		// PATCH_DIFF-set wiring end to end (L2). A patch-fail
		// stub is unconstructible: the floor runs first at 100%,
		// and every patch line sits inside the floor
		// denominator, so floor-pass plus patch-fail cannot
		// co-occur; coverage-patch's own fail golden covers
		// the fail path.
		blob, err := os.ReadFile(filepath.Join(data, "patch.diff"))
		if err != nil {
			t.Fatal(err)
		}
		patch := filepath.Join(ws, "patch.diff")
		if err := os.WriteFile(patch, blob, 0o644); err != nil {
			t.Fatal(err)
		}
		extra = append(extra, "PATCH_DIFF="+patch)
	}
	env := []string{
		"PATH=" + stubs + string(os.PathListSeparator) + os.Getenv("PATH"),
		"STUB_DATA=" + data,
		"STUB_SYSROOT=" + sysroot,
		"STUB_HOST=x86_64-stub",
		"CARGO_HOME=" + t.TempDir(),
		"CARGO_TARGET_DIR=",
	}
	return ws, report, append(env, extra...)
}

func copyTreeFixture(t *testing.T, src, dst string) {
	t.Helper()
	if err := os.MkdirAll(dst, 0o777); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(src)
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		s, d := filepath.Join(src, e.Name()), filepath.Join(dst, e.Name())
		if e.IsDir() {
			copyTreeFixture(t, s, d)
			continue
		}
		blob, err := os.ReadFile(s)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(d, blob, 0o666); err != nil {
			t.Fatal(err)
		}
	}
}

var seedTime = regexp.MustCompile(`seeded in [0-9.]+s`)

// normalize replaces machine paths with placeholders (stubs covers
// the PATH-fallback llvm tool lines when the stub sysroot is refused).
func normalize(out, ws, report, bin, sysroot, stubs string) string {
	for old, new := range map[string]string{
		ws: "$WS", report: "$REPORT", bin: "$BIN", sysroot: "$SYS",
		stubs: "$STUBS",
	} {
		if old != "" {
			out = strings.ReplaceAll(out, old, new)
		}
	}
	return seedTime.ReplaceAllString(out, "seeded in Ns")
}

// stubsDir is the absolute stub-tools dir, for normalization.
func stubsDir(t *testing.T) string {
	t.Helper()
	abs, err := filepath.Abs("testdata/stubs")
	if err != nil {
		t.Fatal(err)
	}
	return abs
}

func sysrootOf(env []string) string {
	for _, kv := range env {
		if strings.HasPrefix(kv, "STUB_SYSROOT=") {
			return kv[len("STUB_SYSROOT="):]
		}
	}
	return ""
}

func runGates(t *testing.T, name string, extra []string, runner []string, wantExit int) (string, string, string, string) {
	t.Helper()
	bin := buildTools(t)
	ws, report, env := stubWorld(t, extra)
	gates := filepath.Join(bin, "rust-quality-gates")
	args := append([]string{ws, report}, runner...)
	stdout, stderr, code := cmdtest.RunBothEnv(t, gates, ".", env, args...)
	if code != wantExit {
		t.Fatalf("%s: exit = %d, want %d\nstdout:\n%s\nstderr:\n%s", name, code, wantExit, stdout, stderr)
	}
	return normalize(stdout, ws, report, bin, sysrootOf(env), stubsDir(t)),
		normalize(stderr, ws, report, bin, sysrootOf(env), stubsDir(t)), ws, report
}

func TestStubGoldens(t *testing.T) {
	runner := []string{"nextest", "--run-ignored", "all", "--no-fail-fast"}
	cases := []struct {
		name  string
		extra []string
		exit  int
	}{
		{"full-run", nil, 0},
		{"raw-small", []string{"STUB_RAW=small"}, 0},
		{"raw-narrow", []string{"STUB_RAW=narrow"}, 1},
		{"audit-fail", []string{"STUB_AUDIT_FAIL=1"}, 1},
		{"llvmcov-fail", []string{"STUB_LLVMCOV_FAIL=1"}, 1},
		{"rustc-fail", []string{"STUB_RUSTC_FAIL=1"}, 0},
		{"patch-pass", []string{"STUB_PATCH=1"}, 0},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			stdout, stderr, ws, report := runGates(t, c.name, c.extra, runner, c.exit)
			if want := cmdtest.Golden(t, "testdata/"+c.name+".golden"); stdout != want {
				t.Fatalf("stdout mismatch:\n got: %q\nwant: %q", stdout, want)
			}
			stderrGolden := c.name + ".stderr.golden"
			if _, err := os.Stat("testdata/" + stderrGolden); err == nil {
				if want := cmdtest.Golden(t, "testdata/"+stderrGolden); stderr != want {
					t.Fatalf("stderr mismatch:\n got: %q\nwant: %q", stderr, want)
				}
			} else if stderr != "" {
				t.Fatalf("unexpected stderr:\n%s", stderr)
			}
			if c.name == "full-run" {
				// Seed landed, canonical HTML won, lcov pinned.
				if _, err := os.Stat(filepath.Join(ws, "target", "llvm-cov-target", "debug", "seed.txt")); err != nil {
					t.Fatalf("seed missing: %v", err)
				}
				html, err := os.ReadFile(filepath.Join(report, "html", "index.html"))
				if err != nil || string(html) != "canonical-html\n" {
					t.Fatalf("html = %q, %v", html, err)
				}
				lcov, err := os.ReadFile(filepath.Join(report, "lcov.info"))
				if err != nil {
					t.Fatal(err)
				}
				got := strings.ReplaceAll(string(lcov), ws, "$WS")
				if want := cmdtest.Golden(t, "testdata/full-run.lcov.golden"); got != want {
					t.Fatalf("lcov mismatch:\n got: %q\nwant: %q", got, want)
				}
			}
			if c.name == "raw-narrow" {
				html, err := os.ReadFile(filepath.Join(report, "html", "index.html"))
				if err != nil || string(html) != "cargo-html\n" {
					t.Fatalf("fallback html = %q, %v", html, err)
				}
			}
		})
	}
}

func TestPreseededSkipsSeedAndPurges(t *testing.T) {
	bin := buildTools(t)
	ws, report, env := stubWorld(t, nil)
	covtarget := filepath.Join(ws, "target", "llvm-cov-target")
	if err := os.MkdirAll(filepath.Join(covtarget, "debug"), 0o777); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(covtarget, "debug", "keep"), []byte("x"), 0o666); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(covtarget, "stale.profraw"), []byte("stale"), 0o666); err != nil {
		t.Fatal(err)
	}
	gates := filepath.Join(bin, "rust-quality-gates")
	stdout, stderr, code := cmdtest.RunBothEnv(t, gates, ".", env, ws, report, "test", "--workspace")
	if code != 0 {
		t.Fatalf("exit = %d\n%s\n%s", code, stdout, stderr)
	}
	if stderr != "" {
		t.Fatalf("unexpected stderr:\n%s", stderr)
	}
	got := normalize(stdout, ws, report, bin, sysrootOf(env), stubsDir(t))
	if want := cmdtest.Golden(t, "testdata/preseeded.golden"); got != want {
		t.Fatalf("stdout mismatch:\n got: %q\nwant: %q", got, want)
	}
	if _, err := os.Stat(filepath.Join(covtarget, "stale.profraw")); !os.IsNotExist(err) {
		t.Fatalf("stale profraw survived the purge")
	}
}

func TestProvisionOnlyGolden(t *testing.T) {
	bin := buildTools(t)
	ws, _, env := stubWorld(t, nil)
	gates := filepath.Join(bin, "rust-quality-gates")
	stdout, stderr, code := cmdtest.RunBothEnv(t, gates, ".", env, ws, "--provision-only")
	if code != 0 {
		t.Fatalf("exit = %d\n%s\n%s", code, stdout, stderr)
	}
	if stderr != "" {
		t.Fatalf("unexpected stderr:\n%s", stderr)
	}
	got := normalize(stdout, ws, "", bin, sysrootOf(env), stubsDir(t))
	if want := cmdtest.Golden(t, "testdata/provision-only.golden"); got != want {
		t.Fatalf("stdout mismatch:\n got: %q\nwant: %q", got, want)
	}
}

// TestOracleDiff covers the paths with no sibling invocations, where
// the oracle and the port must agree byte-for-byte with no
// normalization beyond $WS.
func TestOracleDiff(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("stub tools need sh")
	}
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("python3 not on PATH")
	}
	oracle, err := filepath.Abs("../../../lib/rust-quality-gates.py")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(oracle); err != nil {
		t.Skip("oracle removed; golden tests own the contract now")
	}
	bin := buildTools(t)
	gates := filepath.Join(bin, "rust-quality-gates")
	t.Run("provision-only", func(t *testing.T) {
		goWS, _, goEnv := stubWorld(t, nil)
		pyWS, _, pyEnv := stubWorld(t, nil)
		goOut, goErr, goCode := cmdtest.RunBothEnv(t, gates, ".", goEnv, goWS, "--provision-only")
		pyOut, pyErr, pyCode := cmdtest.RunBothEnv(t, python, ".", pyEnv, oracle, pyWS, "--provision-only")
		if goCode != pyCode || goErr != pyErr {
			t.Fatalf("exit/stderr: go %d %q, oracle %d %q", goCode, goErr, pyCode, pyErr)
		}
		goNorm := strings.ReplaceAll(goOut, goWS, "$WS")
		pyNorm := strings.ReplaceAll(pyOut, pyWS, "$WS")
		if goNorm != pyNorm {
			t.Fatalf("output mismatch:\n got: %q\nwant: %q", goNorm, pyNorm)
		}
	})
	t.Run("refusal", func(t *testing.T) {
		args := []string{"ws", "report", "nextest", "run"}
		goOut, goErr, goCode := cmdtest.RunBothEnv(t, gates, "testdata", nil, args...)
		pyOut, pyErr, pyCode := cmdtest.RunBothEnv(t, python, "testdata", nil,
			append([]string{oracle}, args...)...)
		if goCode != pyCode || goOut != pyOut || goErr != pyErr {
			t.Fatalf("go (%d %q %q) vs oracle (%d %q %q)",
				goCode, goOut, goErr, pyCode, pyOut, pyErr)
		}
	})
}
