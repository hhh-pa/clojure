// Package cmdtest builds cmd binaries and runs them against golden
// files. Goldens were generated from the Python oracles and diffed
// byte-for-byte at generation time.
package cmdtest

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// Build compiles the main package in dir to a temp binary.
func Build(t *testing.T, dir string) string {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "tool")
	BuildTo(t, dir, bin)
	return bin
}

// BuildTo compiles the main package in srcdir to dest.
func BuildTo(t *testing.T, srcdir, dest string) {
	t.Helper()
	cmd := exec.Command("go", "build", "-o", dest, ".")
	cmd.Dir = srcdir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("go build %s: %v\n%s", srcdir, err, out)
	}
}

// Run executes bin with args in workdir, returning stdout and exit code.
// Stderr must stay empty: the Python oracles print everything to stdout.
func Run(t *testing.T, bin, workdir string, args ...string) (string, int) {
	t.Helper()
	stdout, stderr, code := RunBothEnv(t, bin, workdir, nil, args...)
	if stderr != "" {
		t.Fatalf("%s %v wrote to stderr:\n%s", bin, args, stderr)
	}
	return stdout, code
}

// RunEnv is Run with extra environment entries appended (last wins,
// so tests can override ambient variables like GITHUB_*).
func RunEnv(t *testing.T, bin, workdir string, extraEnv []string, args ...string) (string, int) {
	t.Helper()
	stdout, stderr, code := RunBothEnv(t, bin, workdir, extraEnv, args...)
	if stderr != "" {
		t.Fatalf("%s %v wrote to stderr:\n%s", bin, args, stderr)
	}
	return stdout, code
}

// RunBothEnv executes bin with args in workdir, returning stdout,
// stderr, and exit code. Used where the tool itself writes stderr
// (rust-quality-gates reports fatal errors on stderr with exit 1).
func RunBothEnv(t *testing.T, bin, workdir string, extraEnv []string, args ...string) (string, string, int) {
	t.Helper()
	cmd := exec.Command(bin, args...)
	cmd.Dir = workdir
	if extraEnv != nil {
		cmd.Env = append(os.Environ(), extraEnv...)
	}
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	code := 0
	if err != nil {
		if exit, ok := err.(*exec.ExitError); ok {
			code = exit.ExitCode()
		} else {
			t.Fatalf("%s %v failed to run: %v", bin, args, err)
		}
	}
	return stdout.String(), stderr.String(), code
}

// Golden reads a testdata golden file.
func Golden(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}
