// Package executil mirrors the Python subprocess/shutil/os idioms the
// CI tools rely on: child-PATH command resolution, capture runs,
// Apple-injected SDK variables scrubbing, rune-tail slicing, and
// CPython OSError text for the common errnos.
package executil

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// Tail returns the last n runes of s, mirroring Python s[-n:] on str.
func Tail(s string, n int) string {
	r := []rune(s)
	if len(r) > n {
		return string(r[len(r)-n:])
	}
	return s
}

// ChildEnv returns os.Environ minus Apple's python-injected macOS SDK
// overrides (SDKROOT/CPATH/LIBRARY_PATH): they pin the SDK/linker to
// a combination the linker rejects, and CI never sets them.
func ChildEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		if strings.HasPrefix(kv, "SDKROOT=") ||
			strings.HasPrefix(kv, "CPATH=") ||
			strings.HasPrefix(kv, "LIBRARY_PATH=") {
			continue
		}
		env = append(env, kv)
	}
	return env
}

// PathOf returns the PATH value carried by env ("" when absent).
// The last entry wins, matching exec resolution of duplicate keys.
func PathOf(env []string) string {
	val := ""
	for _, kv := range env {
		if strings.HasPrefix(kv, "PATH=") {
			val = kv[len("PATH="):]
		}
	}
	return val
}

// IsExecutable mirrors os.access(path, X_OK): a non-directory with
// any execute bit (plain existence on Windows).
func IsExecutable(path string) bool {
	info, err := os.Stat(path)
	if err != nil || info.IsDir() {
		return false
	}
	if os.PathSeparator == '\\' {
		return true
	}
	return info.Mode()&0o111 != 0
}

// Which resolves name against pathEnv like shutil.which (empty path
// entries mean the working directory; argv[0] values already holding
// a separator are probed directly).
func Which(name, pathEnv string) string {
	try := func(cand string) string {
		if IsExecutable(cand) {
			return cand
		}
		// Arcanist import: Windows tool files carry .exe.
		if os.PathSeparator == '\\' && IsExecutable(cand+".exe") {
			return cand + ".exe"
		}
		return ""
	}
	if strings.ContainsRune(name, os.PathSeparator) {
		return try(name)
	}
	for _, dir := range strings.Split(pathEnv, string(os.PathListSeparator)) {
		if dir == "" {
			dir = "."
		}
		if found := try(filepath.Join(dir, name)); found != "" {
			return found
		}
	}
	return ""
}

// ExitError reports a child that ran and exited nonzero (mirroring a
// returncode check, as opposed to a launch failure).
type ExitError struct{ Code int }

func (e ExitError) Error() string { return fmt.Sprintf("exit %d", e.Code) }

// Capture runs argv with env in cwd, capturing stdout/stderr as text.
// argv[0] resolves against the child PATH, as subprocess does. A
// nonzero exit returns ExitError (with captured output); a launch
// failure returns a plain error.
func Capture(argv []string, env []string, cwd string) (string, string, error) {
	return CaptureTimeout(argv, env, cwd, 0)
}

// CaptureTimeout is Capture with a kill deadline (0 disables it). A
// timeout returns a plain error, never ExitError.
func CaptureTimeout(argv []string, env []string, cwd string, timeout time.Duration) (string, string, error) {
	resolved := Which(argv[0], PathOf(env))
	if resolved == "" {
		return "", "", fmt.Errorf("cannot run %s: not found on PATH", argv[0])
	}
	ctx := context.Background()
	var cancel context.CancelFunc
	if timeout > 0 {
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
	cmd := exec.CommandContext(ctx, resolved, argv[1:]...)
	cmd.Env = env
	if cwd != "" {
		cmd.Dir = cwd
	}
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return stdout.String(), stderr.String(),
				fmt.Errorf("timed out after %s: %s", timeout, strings.Join(argv, " "))
		}
		if exit, ok := err.(*exec.ExitError); ok {
			return stdout.String(), stderr.String(), ExitError{exit.ExitCode()}
		}
		return stdout.String(), stderr.String(), err
	}
	return stdout.String(), stderr.String(), nil
}

// Passthrough runs argv with env in cwd, inheriting stdio, and
// returns ExitError on a nonzero exit.
func Passthrough(argv []string, env []string, cwd string, stdin io.Reader, stdout, stderr io.Writer) error {
	resolved := Which(argv[0], PathOf(env))
	if resolved == "" {
		return fmt.Errorf("cannot run %s: not found on PATH", argv[0])
	}
	cmd := exec.Command(resolved, argv[1:]...)
	cmd.Env = env
	if cwd != "" {
		cmd.Dir = cwd
	}
	cmd.Stdin = stdin
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	if err := cmd.Run(); err != nil {
		if exit, ok := err.(*exec.ExitError); ok {
			return ExitError{exit.ExitCode()}
		}
		return err
	}
	return nil
}

// OSErrorText renders a file error in CPython OSError form for the
// common errnos, so "cannot read ..." lines match the oracle
// byte-for-byte; other errors use Go's text. (Python quotes the path
// with repr; the single-quote form below matches unless the path
// itself holds a quote.)
func OSErrorText(path string, err error) string {
	switch {
	case errors.Is(err, os.ErrNotExist):
		return fmt.Sprintf("[Errno 2] No such file or directory: '%s'", path)
	case errors.Is(err, os.ErrPermission):
		return fmt.Sprintf("[Errno 13] Permission denied: '%s'", path)
	case isDirError(err):
		return fmt.Sprintf("[Errno 21] Is a directory: '%s'", path)
	default:
		return err.Error()
	}
}
