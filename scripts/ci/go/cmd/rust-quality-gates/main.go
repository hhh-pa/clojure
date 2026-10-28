// Command rust-quality-gates runs the Rust quality gates (vendored
// from arcanist's scripts/ci/go): audit, deny, udeps, geiger,
// complexity, coverage. Usage text names this binary; internal gates
// run the sibling binaries resolved next to the running executable.
// All other stdout matches the arcanist oracle byte-for-byte; fatal
// errors go to stderr with exit 1.
package main

import (
	"archive/tar"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/executil"
	"github.com/hhh-pa/clojure/scripts/ci/go/internal/lcov"
)

const usage = `Rust quality gates (T1174): audit, deny, udeps, geiger, complexity, coverage.

Usage: rust-quality-gates <workspace-dir> <report-dir> <cov-runner> [cov-args...]
       rust-quality-gates <workspace-dir> --provision-only
       rust-quality-gates <workspace-dir> --gates-only
       rust-quality-gates <workspace-dir> <report-dir> --coverage-only <cov-runner> [cov-args...]
  workspace-dir: Rust workspace root (Cargo.toml, Cargo.lock, deny.toml,
    .cargo/audit.toml, *-baseline.json)
  report-dir: where coverage reports land (lcov.info + html/)
  cov-runner + cov-args: how llvm-cov runs the suite, e.g.
    nextest --profile ci --run-ignored all
    test --workspace
  --provision-only: install every pinned tool plus the toolchains they
    need, then exit without running any gate. Used to bake the CI gate
    image (scripts/ci/quality-gates.Dockerfile).
  --gates-only: run audit, deny, udeps, geiger, complexity, skip
    coverage. The merge leg of a sharded coverage run.
  --coverage-only: run only the coverage suite and write lcov.info,
    skipping the gates and the floor/patch enforcement (those run on
    the merged report). One leg of a sharded coverage run.
  NOTE: for nextest, OMIT the run subcommand. ` + "`cargo llvm-cov nextest`" + `
  calls ` + "`cargo nextest run`" + ` internally, so a literal run would be treated
  as a test-name filter and the coverage run would silently execute only
  the tests with "run" in their name. The script refuses such args.

Runs every gate plus coverage on every build; exits nonzero citing the
first failure (a red gate never hides the others' signal, and coverage —
report-only observability — always publishes so Codecov trends never gap).
Tools are pinned below and provisioned idempotently (prebuilt binaries
into the stage's cargo bin with a cargo-install fallback when a prebuilt
cannot execute here — upstream gnu builds may need newer glibc than
bookworm's 2.36 — plus rustup components).
Linux-only:
every call site is a Linux container. Pure Go, stdlib only.

Coverage note (T1174): rustc records #[path]-included modules in binary
crates under ` + "`<crate>/tests/../src/*.rs`" + `,
which cargo-llvm-cov 0.9.1 silently drops from every report. The LCOV
step therefore re-exports from the same profile data with raw llvm-cov
and merges those records onto their canonical paths (pure test sources
stay excluded), validated as a line-superset of cargo's view with a
fallback to cargo's reports. The recovered lines are genuinely executed
by the suite; without this the binary workspaces report ~10-66% for
suites that actually cover 100%. Execution counts above 2**62 (llvm
occasionally emits u64::MAX on async-block boundaries) are clamped to 0
on every published LCOV — they crash the Coverage plugin's parser.

`

// Pinned tool versions. Bump deliberately (see CLAUDE.md toolchain
// pinning): unpinned "latest" would let a new tool release break
// master, and these tools judge code content the same way
// rustfmt/clippy do.
const (
	auditVersion   = "0.22.2"
	denyVersion    = "0.20.2"
	llvmCovVersion = "0.9.1"
	geigerVersion  = "0.13.0"
	udepsVersion   = "0.1.61"
	rcaVersion     = "0.0.25"
	nextestVersion = "0.9.148"
	// UDEPSNightly is exported so the gate Dockerfile can grep the
	// pin from this source (single pin source on purpose).
	UDEPSNightly = "nightly-2026-10-01"
)

// fail reports a fatal error: the message on stderr, exit 1.
func fail(err error) int {
	fmt.Fprintln(os.Stderr, err.Error())
	return 1
}

func machineArch() (string, error) {
	m := runtime.GOARCH
	if out, _, err := executil.Capture([]string{"uname", "-m"}, os.Environ(), ""); err == nil {
		if trimmed := strings.TrimSpace(out); trimmed != "" {
			m = trimmed
		}
	}
	// Arcanist import: Windows reports AMD64 for x86_64.
	switch m {
	case "x86_64", "AMD64", "amd64":
		return "x86_64", nil
	case "aarch64", "arm64":
		return "aarch64", nil
	}
	return "", errors.New("unsupported arch: " + m)
}

func downloadOnce(url, dest string) error {
	// Pinned https hosts, retried 3x by fetch, like urlretrieve.
	resp, err := http.Get(url)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return fmt.Errorf("GET %s: %s", url, resp.Status)
	}
	out, err := os.Create(dest)
	if err != nil {
		return err
	}
	defer out.Close()
	_, err = io.Copy(out, resp.Body)
	return err
}

func fetch(url, dest string) error {
	var last error
	for i := 0; i < 3; i++ {
		if err := downloadOnce(url, dest); err == nil {
			return nil
		} else {
			last = err
		}
	}
	return fmt.Errorf("download failed %s: %s", url, last)
}

// smoked reports whether the tool executes (catching stale/wrong-libc
// binaries). Cargo plugins are probed via cargo dispatch, exactly as
// the gates run them: several (llvm-cov, udeps) reject a bare direct
// --version.
func smoked(name string, env []string) bool {
	var argv []string
	if strings.HasPrefix(name, "cargo-") {
		argv = []string{"cargo", name[len("cargo-"):], "--version"}
	} else {
		if executil.Which(name, executil.PathOf(env)) == "" {
			return false
		}
		argv = []string{name, "--version"}
	}
	_, _, err := executil.CaptureTimeout(argv, env, "", 60*time.Second)
	return err == nil
}

func cargoInstall(env []string, toolsRoot, crate, version, features string) error {
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}
	cmd := []string{"cargo", "install", "--force", "--locked", "--version", version, "--root", toolsRoot}
	if features != "" {
		cmd = append(cmd, "--features", features)
	}
	cmd = append(cmd, crate)
	return run(cmd, env, cwd)
}

// extractTarGz unpacks a tool tarball like tarfile.extractall, with
// zip-slip protection the oracle predates (real tool tarballs are
// unaffected). Containment checks stay inline at each sink in the
// canonical prefix form so static analysis can verify them. Link
// creation additionally gates on resolvesWithinDir: CodeQL's
// go/unsafe-unzip-symlink query credits no lexical guard, only a
// header-derived path reaching filepath.EvalSymlinks.
func extractTarGz(archive, dest string) error {
	dest = filepath.Clean(dest)
	fh, err := os.Open(archive)
	if err != nil {
		return err
	}
	defer fh.Close()
	gz, err := gzip.NewReader(fh)
	if err != nil {
		return err
	}
	defer gz.Close()
	tr := tar.NewReader(gz)
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
		if filepath.IsAbs(hdr.Name) {
			return fmt.Errorf("tarball entry is absolute: %s", hdr.Name)
		}
		target := filepath.Join(dest, hdr.Name)
		if target == dest {
			continue // degenerate "." entry: the destination itself
		}
		if !strings.HasPrefix(target, filepath.Clean(dest)+string(os.PathSeparator)) {
			return fmt.Errorf("tarball entry escapes: %s", hdr.Name)
		}
		switch hdr.Typeflag {
		case tar.TypeDir:
			if err := os.MkdirAll(target, fs.FileMode(hdr.Mode&0o777)); err != nil {
				return err
			}
		case tar.TypeReg, tar.TypeRegA:
			if err := os.MkdirAll(filepath.Dir(target), 0o777); err != nil {
				return err
			}
			os.Remove(target)
			out, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, fs.FileMode(hdr.Mode&0o777))
			if err != nil {
				return err
			}
			_, copyErr := io.Copy(out, tr)
			closeErr := out.Close()
			if copyErr != nil {
				return copyErr
			}
			if closeErr != nil {
				return closeErr
			}
		case tar.TypeSymlink:
			linkTarget := hdr.Linkname
			if !filepath.IsAbs(linkTarget) {
				linkTarget = filepath.Join(filepath.Dir(target), linkTarget)
			} else {
				// Absolute targets skip Join's Clean: normalize so a
				// `<dest>/../evil` Linkname cannot pass the prefix
				// check lexically while resolving outside dest.
				linkTarget = filepath.Clean(linkTarget)
			}
			if !strings.HasPrefix(linkTarget, dest+string(os.PathSeparator)) {
				return fmt.Errorf("tarball symlink escapes: %s -> %s", hdr.Name, hdr.Linkname)
			}
			// Real resolution: a lexical prefix cannot see links planted
			// on disk by earlier entries redirecting these paths out.
			if !resolvesWithinDir(dest, target) {
				return fmt.Errorf("tarball symlink location escapes: %s", hdr.Name)
			}
			if !resolvesWithinDir(dest, linkTarget) {
				return fmt.Errorf("tarball symlink escapes: %s -> %s", hdr.Name, hdr.Linkname)
			}
			if err := os.MkdirAll(filepath.Dir(target), 0o777); err != nil {
				return err
			}
			os.Remove(target)
			if err := os.Symlink(hdr.Linkname, target); err != nil {
				return err
			}
		case tar.TypeLink:
			if filepath.IsAbs(hdr.Linkname) {
				return fmt.Errorf("tarball hardlink escapes: %s -> %s", hdr.Name, hdr.Linkname)
			}
			src := filepath.Join(dest, hdr.Linkname)
			if !strings.HasPrefix(src, dest+string(os.PathSeparator)) {
				return fmt.Errorf("tarball hardlink escapes: %s -> %s", hdr.Name, hdr.Linkname)
			}
			if !resolvesWithinDir(dest, target) {
				return fmt.Errorf("tarball hardlink location escapes: %s", hdr.Name)
			}
			if !resolvesWithinDir(dest, src) {
				return fmt.Errorf("tarball hardlink escapes: %s -> %s", hdr.Name, hdr.Linkname)
			}
			if err := os.MkdirAll(filepath.Dir(target), 0o777); err != nil {
				return err
			}
			os.Remove(target)
			if err := os.Link(src, target); err != nil {
				return err
			}
		}
	}
}

// resolvesWithinDir reports whether path, with every symlink already
// on disk along it resolved for real, still lands inside dir. Unlike
// the lexical prefix checks it follows links planted by earlier
// archive entries, so it catches a link whose parent was itself
// redirected out of the tree.
func resolvesWithinDir(dir, path string) bool {
	// Resolve leniently too: dir may not exist yet (a link stored
	// before any dir entry), and under a symlinked ancestor (macOS
	// /tmp) a lexical fallback would never prefix-match the
	// resolved candidate.
	realDir, err := evalSymlinksLenient(dir)
	if err != nil {
		realDir = dir // unresolvable root; fall back lexically
	}
	resolved, err := evalSymlinksLenient(path)
	if err != nil {
		return false
	}
	return resolved == realDir || strings.HasPrefix(resolved, realDir+string(os.PathSeparator))
}

// evalSymlinksLenient behaves like filepath.EvalSymlinks but tolerates
// trailing components that do not exist yet: it resolves the longest
// existing prefix and re-appends the missing tail. A tar may store a
// link before its destination exists, so a plain EvalSymlinks (which
// fails on any missing component) would reject valid links.
func evalSymlinksLenient(path string) (string, error) {
	remainder := ""
	cur := path
	for {
		resolved, err := filepath.EvalSymlinks(cur)
		if err == nil {
			if remainder == "" {
				return resolved, nil
			}
			return filepath.Join(resolved, remainder), nil
		}
		if !errors.Is(err, fs.ErrNotExist) {
			return "", err
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return "", err // reached the root with nothing existing
		}
		remainder = filepath.Join(filepath.Base(cur), remainder)
		cur = parent
	}
}

func installTarball(env []string, name, url string) error {
	tmpdir, err := os.MkdirTemp("", "rqg-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmpdir)
	archive := filepath.Join(tmpdir, "tool.tar.gz")
	if err := fetch(url, archive); err != nil {
		return err
	}
	if err := extractTarGz(archive, tmpdir); err != nil {
		return err
	}
	var found string
	walkErr := filepath.WalkDir(tmpdir, func(path string, d fs.DirEntry, err error) error {
		if err != nil || found != "" || d.IsDir() || d.Name() != name {
			return nil
		}
		found = path
		return nil
	})
	if walkErr != nil {
		return walkErr
	}
	if found == "" {
		return fmt.Errorf("tarball for %s has no %s binary: %s", name, name, url)
	}
	dest := filepath.Join(envVal(env, "TOOLS_BIN"), name)
	data, err := os.ReadFile(found)
	if err != nil {
		return err
	}
	if err := os.WriteFile(dest, data, 0o755); err != nil {
		return err
	}
	return os.Chmod(dest, 0o755)
}

// ensureTool provisions one pinned tool: prebuilt when it runs, else
// compile. A download never overwrites a present binary:
// present-but-broken goes straight to the authoritative compile (and
// off Linux there are no downloads at all — the local proxy needs
// its tools installed by hand). Empty url means pinned compile.
func ensureTool(env []string, toolsRoot, name, crate, version, url, features string) error {
	if smoked(name, env) {
		return nil
	}
	if runtime.GOOS != "linux" {
		return fmt.Errorf("%s is missing or broken and this script only provisions "+
			"Linux; install %s %s by hand for the local proxy", name, crate, version)
	}
	if executil.Which(name, executil.PathOf(env)) == "" && url != "" {
		fmt.Printf("installing %s (pinned prebuilt)...\n", name)
		if err := installTarball(env, name, url); err != nil {
			return err
		}
		if smoked(name, env) {
			return nil
		}
		fmt.Printf("%s prebuilt cannot execute here; compiling...\n", name)
	} else if url == "" {
		fmt.Printf("installing %s (pinned compile)...\n", name)
	} else {
		fmt.Printf("%s is present but cannot execute; compiling...\n", name)
	}
	if err := cargoInstall(env, toolsRoot, crate, version, features); err != nil {
		return err
	}
	if !smoked(name, env) {
		return errors.New("cannot provision " + name)
	}
	return nil
}

func run(cmd []string, env []string, cwd string) error {
	fmt.Printf("+ %s\n", strings.Join(cmd, " "))
	if err := executil.Passthrough(cmd, env, cwd, os.Stdin, os.Stdout, os.Stderr); err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			return fmt.Errorf("FAILED: %s (exit %d)", strings.Join(cmd, " "), exit.Code)
		}
		return err
	}
	return nil
}

// attempt is run that records failure instead of raising (deferred
// exit): every gate plus coverage runs on every build, so one red
// gate never hides the others' signal or gaps the coverage trend.
// Returns the failure line ("" on success); a launch failure is
// fatal, as the oracle's traceback is.
func attempt(label string, cmd []string, env []string, cwd string) (string, error) {
	fmt.Printf("==> %s: %s\n", cwd, label)
	fmt.Printf("+ %s\n", strings.Join(cmd, " "))
	if err := executil.Passthrough(cmd, env, cwd, os.Stdin, os.Stdout, os.Stderr); err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			msg := fmt.Sprintf("FAILED: %s (exit %d)", strings.Join(cmd, " "), exit.Code)
			fmt.Println(msg)
			return msg, nil
		}
		return "", err
	}
	return "", nil
}

// envVal returns the last value of key in env (exec semantics).
func envVal(env []string, key string) string {
	val := ""
	for _, kv := range env {
		if strings.HasPrefix(kv, key+"=") {
			val = kv[len(key)+1:]
		}
	}
	return val
}

func cargoTargetDir(env []string, workspace string) string {
	base := envVal(env, "CARGO_TARGET_DIR")
	if base == "" {
		base = filepath.Join(workspace, "target")
	}
	if !filepath.IsAbs(base) {
		base = filepath.Join(workspace, base)
	}
	return base
}

// restampTree applies a uniform mtime stamp over a seeded target
// tree (T1174): every file and dir gets one shared timestamp, so no
// dependency output is newer than its dependent and dep-info files
// are newer than every input they list. Best-effort: a missing tree
// just compiles from scratch.
func restampTree(root string) {
	info, err := os.Stat(root)
	if err != nil || !info.IsDir() {
		return
	}
	now := time.Now()
	filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil || path == root {
			return nil
		}
		_ = os.Chtimes(path, now, now)
		return nil
	})
	_ = os.Chtimes(root, now, now)
}

func copyFileStat(src, dest string) error {
	info, err := os.Stat(src)
	if err != nil {
		return err
	}
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	if err := os.WriteFile(dest, data, info.Mode().Perm()); err != nil {
		return err
	}
	mtime := info.ModTime()
	return os.Chtimes(dest, mtime, mtime)
}

// copyTree mirrors shutil.copytree(src, dest, symlinks=True): links
// recreated as links, file/dir modes and mtimes preserved (atime is
// set to mtime; only mtimes feed fingerprint freshness).
func copyTree(src, dest string) error {
	info, err := os.Lstat(src)
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 {
		target, err := os.Readlink(src)
		if err != nil {
			return err
		}
		return os.Symlink(target, dest)
	}
	if !info.IsDir() {
		return copyFileStat(src, dest)
	}
	if err := os.MkdirAll(dest, 0o777); err != nil {
		return err
	}
	entries, err := os.ReadDir(src)
	if err != nil {
		return err
	}
	for _, e := range entries {
		if err := copyTree(filepath.Join(src, e.Name()), filepath.Join(dest, e.Name())); err != nil {
			return err
		}
	}
	if err := os.Chmod(dest, info.Mode().Perm()); err != nil {
		return err
	}
	mtime := info.ModTime()
	return os.Chtimes(dest, mtime, mtime)
}

// seedLLVMCovTarget copies the seeded test-profile target into
// llvm-cov's target dir: llvm-cov builds in
// <target>/llvm-cov-target, fresh every CI build, but instruments
// workspace crates only, so the already-seeded debug tree makes
// every dependency fresh and the coverage run compiles workspace
// crates only. Skipped when the dest already has content.
func seedLLVMCovTarget(env []string, workspace string) {
	base := cargoTargetDir(env, workspace)
	src := filepath.Join(base, "debug")
	covtarget := filepath.Join(base, "llvm-cov-target")
	dest := filepath.Join(covtarget, "debug")
	if info, err := os.Stat(dest); err == nil && info.IsDir() {
		if entries, err := os.ReadDir(dest); err == nil && len(entries) > 0 {
			return
		}
	}
	if info, err := os.Stat(src); err != nil || !info.IsDir() {
		fmt.Printf("WARNING: no %s to seed llvm-cov from; running unseeded\n", src)
		return
	}
	fmt.Printf("seeding llvm-cov target dir from %s\n", src)
	start := time.Now()
	os.RemoveAll(covtarget)
	if err := copyTree(src, dest); err != nil {
		fmt.Printf("WARNING: llvm-cov seed failed (%s); running unseeded\n", err)
		os.RemoveAll(covtarget)
		return
	}
	fmt.Printf("llvm-cov target seeded in %.1fs\n", time.Since(start).Seconds())
}

// llvmToolPath locates an llvm tool (llvm-cov, llvm-profdata) for the
// active toolchain: the sysroot's rustlib bin first, then PATH.
func llvmToolPath(env []string, workspace, name string) (string, error) {
	sysroot := ""
	if out, _, err := executil.CaptureTimeout(
		[]string{"rustc", "--print", "sysroot"}, env, workspace, 60*time.Second,
	); err == nil {
		sysroot = strings.TrimSpace(out)
	}
	if sysroot != "" {
		host := ""
		if out, _, err := executil.CaptureTimeout(
			[]string{"rustc", "-vV"}, env, workspace, 60*time.Second,
		); err == nil {
			for _, line := range strings.Split(out, "\n") {
				if strings.HasPrefix(line, "host: ") {
					host = strings.TrimSpace(line[len("host: "):])
				}
			}
		}
		if host != "" {
			cand := filepath.Join(sysroot, "lib", "rustlib", host, "bin", name)
			if executil.IsExecutable(cand) {
				return cand, nil
			}
			// Arcanist import: Windows tool files carry .exe.
			if executil.IsExecutable(cand + ".exe") {
				return cand + ".exe", nil
			}
		}
	}
	if found := executil.Which(name, executil.PathOf(env)); found != "" {
		return found, nil
	}
	return "", fmt.Errorf("cannot find %s (the llvm-tools-preview rustup component provides it)", name)
}

// rawLLVMCovExport runs raw `llvm-cov export --format=lcov` over
// every test object. Unlike `cargo llvm-cov report`, this keeps the
// `<crate>/tests/../src` records that cargo's built-in filter drops;
// the caller merges and validates them.
func rawLLVMCovExport(env []string, workspace, covtarget, llvmCov, llvmProfdata string) (string, []string, string, error) {
	entries, err := os.ReadDir(covtarget)
	if err != nil {
		return "", nil, "", err
	}
	var profraws []string
	for _, e := range entries {
		if strings.HasSuffix(e.Name(), ".profraw") {
			profraws = append(profraws, filepath.Join(covtarget, e.Name()))
		}
	}
	sort.Strings(profraws)
	if len(profraws) == 0 {
		return "", nil, "", errors.New("no profraw files under " + covtarget)
	}
	profdata := filepath.Join(covtarget, "canonical.profdata")
	fmt.Printf("+ %s merge -sparse ... (%d profraw) -o %s\n", llvmProfdata, len(profraws), profdata)
	merge := append([]string{llvmProfdata, "merge", "-sparse"}, profraws...)
	merge = append(merge, "-o", profdata)
	_, serr, err := executil.CaptureTimeout(merge, env, workspace, 600*time.Second)
	if err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			return "", nil, "", fmt.Errorf("llvm-profdata merge failed: %s",
				executil.Tail(strings.TrimSpace(serr), 500))
		}
		return "", nil, "", err
	}
	var objs []string
	for _, sub := range []string{"debug", "release"} {
		deps := filepath.Join(covtarget, sub, "deps")
		info, err := os.Stat(deps)
		if err != nil || !info.IsDir() {
			continue
		}
		entries, err := os.ReadDir(deps)
		if err != nil {
			return "", nil, "", err
		}
		for _, e := range entries {
			if strings.Contains(e.Name(), ".") {
				continue
			}
			p := filepath.Join(deps, e.Name())
			if info, err := os.Stat(p); err == nil && !info.IsDir() && executil.IsExecutable(p) {
				objs = append(objs, p)
			}
		}
	}
	if len(objs) == 0 {
		return "", nil, "", errors.New("no test objects under " + covtarget)
	}
	cmd := []string{llvmCov, "export", "-instr-profile=" + profdata, "--format=lcov"}
	for _, obj := range objs {
		cmd = append(cmd, "-object", obj)
	}
	fmt.Printf("+ %s ... (%d objects)\n", strings.Join(cmd[:4], " "), len(objs))
	raw, serr, err := executil.CaptureTimeout(cmd, env, workspace, 600*time.Second)
	if err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			return "", nil, "", fmt.Errorf("llvm-cov export failed: %s",
				executil.Tail(strings.TrimSpace(serr), 500))
		}
		return "", nil, "", err
	}
	return raw, objs, profdata, nil
}

// upgradeLCOVWithCanonical replaces cargo's LCOV/HTML with the
// canonicalized raw export, validated as a line-superset of cargo's
// view (same suite, same profile data — the raw path may only ADD
// genuinely-executed lines, never lose any). Returns false to keep
// cargo's reports when the check fails.
func upgradeLCOVWithCanonical(env []string, workspace, reportDir string) (bool, error) {
	base := cargoTargetDir(env, workspace)
	covtarget := filepath.Join(base, "llvm-cov-target")
	llvmCov, err := llvmToolPath(env, workspace, "llvm-cov")
	if err != nil {
		return false, err
	}
	llvmProfdata, err := llvmToolPath(env, workspace, "llvm-profdata")
	if err != nil {
		return false, err
	}
	raw, objs, profdata, err := rawLLVMCovExport(env, workspace, covtarget, llvmCov, llvmProfdata)
	if err != nil {
		return false, err
	}
	rawRecords, rawSanitized := lcov.ParseRecords(lcov.SanitizeUTF8(raw))
	for i, s := range rawSanitized {
		if i >= 10 {
			break
		}
		fmt.Printf("WARNING: raw export has absurd count %s at %s %s; clamped to 0\n",
			s.Raw, s.SF, s.What)
	}
	canon := lcov.Canonicalize(rawRecords, workspace)
	lcovPath := filepath.Join(reportDir, "lcov.info")
	data, err := os.ReadFile(lcovPath)
	if err != nil {
		return false, err
	}
	cargoRecords, _ := lcov.ParseRecords(lcov.SanitizeUTF8(string(data)))
	cargoCanon := lcov.NormSFs(cargoRecords)
	canonTotal, canonCovered := lcov.LineSets(canon)
	cargoTotal, cargoCovered := lcov.LineSets(cargoCanon)
	// NOTE: the oracle reads `not A and B`, i.e. (!totalsSub &&
	// coveredSub) — `not` binds tighter than `and`. Replicated
	// exactly, including the cases where the upgrade proceeds
	// despite a non-superset (arguably an oracle bug; fix in both
	// together if ever changed).
	totalsSub := lcov.IsSubset(cargoTotal, canonTotal)
	coveredSub := lcov.IsSubset(cargoCovered, canonCovered)
	if !totalsSub && coveredSub {
		fmt.Printf("WARNING: canonical coverage is not a superset of cargo's "+
			"view (cargo %d/%d lines, canonical %d/%d); keeping "+
			"cargo-llvm-cov reports\n",
			len(cargoCovered), len(cargoTotal), len(canonCovered), len(canonTotal))
		return false, nil
	}
	// HTML first: it cannot merge the twin path spellings the way
	// LCOV can, so it shows every file with true per-file numbers —
	// its index average will differ from lcov.info's merged total,
	// which is the published truth.
	htmlDir := filepath.Join(reportDir, "html")
	htmlTmp := filepath.Join(reportDir, "html.canonical")
	os.RemoveAll(htmlTmp)
	cmd := []string{llvmCov, "show", "-instr-profile=" + profdata, "--format=html", "--output-dir=" + htmlTmp}
	for _, obj := range objs {
		cmd = append(cmd, "-object", obj)
	}
	fmt.Printf("+ %s ... (%d objects)\n", strings.Join(cmd[:6], " "), len(objs))
	_, serr, err := executil.CaptureTimeout(cmd, env, workspace, 600*time.Second)
	if err != nil {
		var exit executil.ExitError
		if errors.As(err, &exit) {
			return false, fmt.Errorf("llvm-cov show failed: %s",
				executil.Tail(strings.TrimSpace(serr), 500))
		}
		return false, err
	}
	os.RemoveAll(htmlDir)
	if err := os.Rename(htmlTmp, htmlDir); err != nil {
		return false, err
	}
	if err := os.WriteFile(lcovPath, []byte(lcov.Emit(canon)), 0o666); err != nil {
		return false, err
	}
	recovered := 0
	for key := range canonCovered {
		if !cargoCovered[key] {
			recovered++
		}
	}
	fmt.Printf("canonical coverage: cargo %d/%d lines -> canonical %d/%d "+
		"(recovered %d genuinely-executed lines)\n",
		len(cargoCovered), len(cargoTotal), len(canonCovered), len(canonTotal), recovered)
	return true, nil
}

// sanitizeLCOVFile clamps absurd counts in an LCOV file in place,
// protecting the Coverage plugin even on the cargo-report fallback
// path. Returns the number of clamped values.
func sanitizeLCOVFile(lcovPath, workspace string) (int, error) {
	data, err := os.ReadFile(lcovPath)
	if err != nil {
		return 0, err
	}
	records, sanitized := lcov.ParseRecords(lcov.SanitizeUTF8(string(data)))
	for i, s := range sanitized {
		if i >= 10 {
			break
		}
		short := s.SF
		if strings.HasPrefix(s.SF, workspace) {
			if len(s.SF) > len(workspace)+1 {
				short = s.SF[len(workspace)+1:]
			} else {
				short = ""
			}
		}
		fmt.Printf("WARNING: %s has absurd count %s at %s %s; clamped to 0\n",
			lcovPath, s.Raw, short, s.What)
	}
	if len(sanitized) > 0 {
		canonical := lcov.Canonicalize(records, workspace)
		if err := os.WriteFile(lcovPath, []byte(lcov.Emit(canonical)), 0o666); err != nil {
			return 0, err
		}
	}
	return len(sanitized), nil
}

// printLCOVSummary logs the total + per-file table parsed from the
// published lcov.info.
func printLCOVSummary(lcovPath, workspace string) error {
	data, err := os.ReadFile(lcovPath)
	if err != nil {
		return err
	}
	records, _ := lcov.ParseRecords(lcov.SanitizeUTF8(string(data)))
	canonical := lcov.Canonicalize(records, workspace)
	type row struct {
		pct  float64
		hit  int
		tot  int
		sf   string
		disp string
	}
	var rows []row
	hit, tot := 0, 0
	for _, sr := range canonical {
		h := 0
		for _, c := range sr.Rec.DA {
			if c > 0 {
				h++
			}
		}
		t := len(sr.Rec.DA)
		disp := sr.SF
		if strings.HasPrefix(sr.SF, workspace) {
			if len(sr.SF) > len(workspace)+1 {
				disp = sr.SF[len(workspace)+1:]
			} else {
				disp = ""
			}
		}
		rows = append(rows, row{float64(h) / float64(max(1, t)), h, t, sr.SF, disp})
		hit += h
		tot += t
	}
	sort.Slice(rows, func(i, j int) bool {
		a, b := rows[i], rows[j]
		if a.pct != b.pct {
			return a.pct < b.pct
		}
		if a.hit != b.hit {
			return a.hit < b.hit
		}
		if a.tot != b.tot {
			return a.tot < b.tot
		}
		return a.sf < b.sf
	})
	fmt.Printf("coverage for %s: %.2f%% lines (%d/%d, %d files)\n",
		workspace, 100*float64(hit)/float64(max(1, tot)), hit, tot, len(rows))
	for _, r := range rows {
		fmt.Printf("  %6.2f%% %5d/%-5d %s\n", 100*r.pct, r.hit, r.tot, r.disp)
	}
	return nil
}

// sibling resolves a sibling Go tool next to the running executable.
func sibling(name string) (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	cand := filepath.Join(filepath.Dir(exe), name)
	if runtime.GOOS == "windows" {
		cand += ".exe"
	}
	return cand, nil
}

func coverageReports(env []string, workspace, reportDir string, covRunner []string, enforce bool) error {
	// The suite run stays fail-fast (a red suite is the primary
	// signal and its partial profile data would mislead the trend);
	// the report steps below then always publish from whatever cargo
	// produced.
	seedLLVMCovTarget(env, workspace)
	covtarget := filepath.Join(cargoTargetDir(env, workspace), "llvm-cov-target")
	if entries, err := os.ReadDir(covtarget); err == nil {
		for _, e := range entries {
			if strings.HasSuffix(e.Name(), ".profraw") || strings.HasSuffix(e.Name(), ".profdata") {
				if err := os.Remove(filepath.Join(covtarget, e.Name())); err != nil {
					break
				}
			}
		}
	}
	if err := run(append([]string{"cargo", "llvm-cov", "--no-report", "--workspace"}, covRunner...), env, workspace); err != nil {
		return err
	}
	lcovPath := filepath.Join(reportDir, "lcov.info")
	if err := run([]string{"cargo", "llvm-cov", "report", "--lcov", "--workspace", "--output-path", lcovPath}, env, workspace); err != nil {
		return err
	}
	if _, err := sanitizeLCOVFile(lcovPath, workspace); err != nil {
		return err
	}
	if enforce {
		if err := run([]string{"cargo", "llvm-cov", "report", "--html", "--workspace", "--output-dir", reportDir}, env, workspace); err != nil {
			return err
		}
	}
	if _, err := upgradeLCOVWithCanonical(env, workspace, reportDir); err != nil {
		fmt.Printf("WARNING: canonical coverage unavailable (%s); keeping cargo-llvm-cov reports\n", err)
	}
	if err := printLCOVSummary(lcovPath, workspace); err != nil {
		return err
	}
	if !enforce {
		// Shard leg: the floor and patch gates run on the
		// merged report, where partial coverage is whole.
		return nil
	}
	// The 100% bar, enforced: every line stays covered, no
	// regressions, no uncovered tail.
	floorBin, err := sibling("coverage-floor")
	if err != nil {
		return err
	}
	if err := run([]string{floorBin, lcovPath, "100"}, env, workspace); err != nil {
		return err
	}
	// S9A patch gate: added-lines coverage from the
	// workflow-computed diff. PATCH_DIFF is unset outside CI, so
	// the line floor alone applies there and the gates goldens
	// stay byte-identical.
	if patch := os.Getenv("PATCH_DIFF"); patch != "" {
		patchBin, err := sibling("coverage-patch")
		if err != nil {
			return err
		}
		if err := run([]string{patchBin, lcovPath, patch, "100"}, env, workspace); err != nil {
			return err
		}
	}
	return nil
}

// provisionDeny installs cargo-deny: upstream ships only musl
// binaries for Linux (both arches); they run on the glibc builders,
// checksum-verified.
func provisionDeny(env []string, toolsRoot, arch string) error {
	if smoked("cargo-deny", env) {
		return nil
	}
	if runtime.GOOS != "linux" {
		return fmt.Errorf("cargo-deny is missing or broken and this script only " +
			"provisions Linux; install cargo-deny " + denyVersion + " by hand for the local proxy")
	}
	if executil.Which("cargo-deny", executil.PathOf(env)) == "" {
		target := arch + "-unknown-linux-musl"
		fmt.Println("installing cargo-deny (pinned prebuilt)...")
		tmpdir, err := os.MkdirTemp("", "rqg-")
		if err != nil {
			return err
		}
		install := func() error {
			defer os.RemoveAll(tmpdir)
			base := fmt.Sprintf(
				"https://github.com/EmbarkStudios/cargo-deny/releases/download/%s/cargo-deny-%s-%s.tar.gz",
				denyVersion, denyVersion, target)
			archive := filepath.Join(tmpdir, "deny.tar.gz")
			if err := fetch(base, archive); err != nil {
				return err
			}
			want := filepath.Join(tmpdir, "deny.sha256")
			if err := fetch(base+".sha256", want); err != nil {
				return err
			}
			sumData, err := os.ReadFile(want)
			if err != nil {
				return err
			}
			fields := strings.Fields(strings.TrimSpace(string(sumData)))
			if len(fields) == 0 {
				return errors.New("cargo-deny checksum file is empty")
			}
			archiveData, err := os.ReadFile(archive)
			if err != nil {
				return err
			}
			sum := sha256.Sum256(archiveData)
			if hex.EncodeToString(sum[:]) != fields[0] {
				return errors.New("cargo-deny checksum mismatch")
			}
			if err := extractTarGz(archive, tmpdir); err != nil {
				return err
			}
			data, err := os.ReadFile(filepath.Join(tmpdir,
				fmt.Sprintf("cargo-deny-%s-%s", denyVersion, target), "cargo-deny"))
			if err != nil {
				return err
			}
			dest := filepath.Join(envVal(env, "TOOLS_BIN"), "cargo-deny")
			if err := os.WriteFile(dest, data, 0o755); err != nil {
				return err
			}
			return os.Chmod(dest, 0o755)
		}
		if err := install(); err != nil {
			return err
		}
		if !smoked("cargo-deny", env) {
			fmt.Println("cargo-deny prebuilt cannot execute here; compiling...")
			if err := cargoInstall(env, toolsRoot, "cargo-deny", denyVersion, ""); err != nil {
				return err
			}
			if !smoked("cargo-deny", env) {
				return errors.New("cannot provision cargo-deny")
			}
		}
		return nil
	}
	fmt.Println("cargo-deny is present but cannot execute; compiling...")
	if err := cargoInstall(env, toolsRoot, "cargo-deny", denyVersion, ""); err != nil {
		return err
	}
	if !smoked("cargo-deny", env) {
		return errors.New("cannot provision cargo-deny")
	}
	return nil
}

func runMain(argv []string) int {
	if len(argv) < 2 || argv[0] == "-h" || argv[0] == "--help" {
		fmt.Print(usage)
		return 2
	}
	workspace, err := filepath.Abs(argv[0])
	if err != nil {
		return fail(err)
	}
	var provisionOnly, gatesOnly, coverageOnly bool
	var reportDir string
	var covRunner []string
	if len(argv) == 2 && argv[1] == "--provision-only" {
		provisionOnly = true
	} else if argv[1] == "--gates-only" {
		if len(argv) != 2 {
			fmt.Print(usage)
			return 2
		}
		gatesOnly = true
	} else {
		if len(argv) < 3 {
			fmt.Print(usage)
			return 2
		}
		reportDir, err = filepath.Abs(argv[1])
		if err != nil {
			return fail(err)
		}
		covRunner = argv[2:]
		if covRunner[0] == "--coverage-only" {
			coverageOnly = true
			covRunner = covRunner[1:]
			if len(covRunner) == 0 {
				fmt.Print(usage)
				return 2
			}
		}
		if covRunner[0] == "nextest" {
			for _, a := range covRunner {
				if a == "run" {
					// `cargo llvm-cov nextest` calls `cargo
					// nextest run` internally; a literal run
					// becomes a test-name filter and silently
					// guts coverage.
					return fail(errors.New("refusing nextest cov-runner containing 'run': omit the " +
						"subcommand, llvm-cov inserts it (else it filters tests by name)"))
				}
			}
		}
	}
	arch, err := machineArch()
	if err != nil {
		return fail(err)
	}

	var toolsBin string
	if home := os.Getenv("CARGO_HOME"); home != "" {
		toolsBin = filepath.Join(home, "bin")
	} else {
		// Courriel builder: cargo lives in the image, no
		// CARGO_HOME set.
		cargo := executil.Which("cargo", os.Getenv("PATH"))
		if cargo == "" {
			cargo = "/usr/bin"
		}
		toolsBin = filepath.Dir(cargo)
	}
	if err := os.MkdirAll(toolsBin, 0o777); err != nil {
		return fail(err)
	}

	// Child environment: tools on PATH, and none of Apple's
	// python-injected macOS SDK overrides.
	env := append(executil.ChildEnv(),
		"PATH="+toolsBin+string(os.PathListSeparator)+os.Getenv("PATH"),
		"TOOLS_BIN="+toolsBin)

	fmt.Println("==> rust quality gates: provisioning tools")
	toolsRoot := filepath.Dir(toolsBin)
	if err := ensureTool(env, toolsRoot, "cargo-audit", "cargo-audit", auditVersion,
		fmt.Sprintf("https://github.com/rustsec/rustsec/releases/download/cargo-audit/v%s/cargo-audit-%s-unknown-linux-gnu-v%s.tgz",
			auditVersion, arch, auditVersion), ""); err != nil {
		return fail(err)
	}
	if err := provisionDeny(env, toolsRoot, arch); err != nil {
		return fail(err)
	}
	if err := ensureTool(env, toolsRoot, "cargo-llvm-cov", "cargo-llvm-cov", llvmCovVersion,
		fmt.Sprintf("https://github.com/taiki-e/cargo-llvm-cov/releases/download/v%s/cargo-llvm-cov-%s-unknown-linux-gnu.tar.gz",
			llvmCovVersion, arch), ""); err != nil {
		return fail(err)
	}
	if err := ensureTool(env, toolsRoot, "cargo-udeps", "cargo-udeps", udepsVersion,
		fmt.Sprintf("https://github.com/est31/cargo-udeps/releases/download/v%s/cargo-udeps-v%s-%s-unknown-linux-gnu.tar.gz",
			udepsVersion, udepsVersion, arch), ""); err != nil {
		return fail(err)
	}
	rcaURL := ""
	if arch == "x86_64" {
		rcaURL = fmt.Sprintf(
			"https://github.com/mozilla/rust-code-analysis/releases/download/v%s/rust-code-analysis-linux-cli-x86_64.tar.gz",
			rcaVersion)
	}
	// No aarch64 prebuilt upstream; compile (fast, cached).
	if err := ensureTool(env, toolsRoot, "rust-code-analysis-cli", "rust-code-analysis-cli",
		rcaVersion, rcaURL, ""); err != nil {
		return fail(err)
	}
	// No prebuilt binaries upstream; every other stage compiles once
	// into its cache.
	if err := ensureTool(env, toolsRoot, "cargo-geiger", "cargo-geiger",
		geigerVersion, "", "vendored-openssl"); err != nil {
		return fail(err)
	}
	if err := ensureTool(env, toolsRoot, "cargo-nextest", "cargo-nextest",
		nextestVersion, "", ""); err != nil {
		return fail(err)
	}
	if err := run([]string{"rustup", "component", "add", "llvm-tools-preview"}, env, workspace); err != nil {
		return fail(err)
	}
	if err := run([]string{"rustup", "toolchain", "install", UDEPSNightly,
		"--profile", "minimal", "--no-self-update"}, env, workspace); err != nil {
		return fail(err)
	}
	if provisionOnly {
		fmt.Printf("rust quality gates: tools provisioned for %s\n", workspace)
		return 0
	}

	var failures []string
	keep := func(msg string) {
		if msg != "" {
			failures = append(failures, msg)
		}
	}
	if !coverageOnly {
		// Per-workspace advisory-db, freshly cloned every run: the
		// default ~/.cargo/advisory-db is shared across parallel stages
		// on the node cache, and audit cannot update an existing
		// checkout anyway.
		auditDB := filepath.Join(workspace, "target", "advisory-db")
		os.RemoveAll(auditDB)
		msg, err := attempt("cargo audit --deny warnings",
			[]string{"cargo", "audit", "--db", auditDB, "--deny", "warnings"}, env, workspace)
		if err != nil {
			return fail(err)
		}
		keep(msg)
		msg, err = attempt("cargo deny check licenses bans sources",
			[]string{"cargo", "deny", "check", "licenses", "bans", "sources"}, env, workspace)
		if err != nil {
			return fail(err)
		}
		keep(msg)

		// udeps runs in the cooked nested target dir (T1174): its
		// fingerprints match only udeps-cooked artifacts, and the
		// separate dir keeps its nightly .rustc_info.json from dirtying
		// the stable tree.
		udepsTargetDir := filepath.Join(cargoTargetDir(env, workspace), "nightly")
		udepsEnv := append(env, "CARGO_TARGET_DIR="+udepsTargetDir)
		nightlyFP := filepath.Join(udepsTargetDir, "debug", ".fingerprint")
		if entries, err := os.ReadDir(nightlyFP); err == nil {
			fmt.Printf("==> %s: udeps seed holds %d fingerprints\n", workspace, len(entries))
		} else {
			fmt.Printf("==> %s: udeps seed MISSING (%s)\n", workspace, nightlyFP)
		}
		// Restamp after provisioning (T1174).
		restampTree(udepsTargetDir)
		msg, err = attempt("cargo udeps ("+UDEPSNightly+")",
			[]string{"cargo", "+" + UDEPSNightly, "udeps", "--workspace", "--all-targets"},
			udepsEnv, workspace)
		if err != nil {
			return fail(err)
		}
		keep(msg)

		geigerBin, err := sibling("geiger")
		if err != nil {
			return fail(err)
		}
		msg, err = attempt("geiger unsafe ratchet",
			[]string{geigerBin, workspace}, env, workspace)
		if err != nil {
			return fail(err)
		}
		keep(msg)

		complexityBin, err := sibling("complexity")
		if err != nil {
			return fail(err)
		}
		msg, err = attempt("complexity ratchet",
			[]string{complexityBin, workspace}, env, workspace)
		if err != nil {
			return fail(err)
		}
		keep(msg)
	} // !coverageOnly

	if !gatesOnly {
		fmt.Printf("==> %s: coverage reports -> %s\n", workspace, reportDir)
		if err := os.MkdirAll(reportDir, 0o777); err != nil {
			return fail(err)
		}
		if err := coverageReports(env, workspace, reportDir, covRunner, !coverageOnly); err != nil {
			failures = append(failures, "coverage: "+err.Error())
		}
	} // !gatesOnly
	if len(failures) > 0 {
		fmt.Fprintf(os.Stderr, "rust quality gates: %d failure(s); first: %s\n",
			len(failures), failures[0])
		return 1
	}

	fmt.Printf("rust quality gates: all green for %s\n", workspace)
	return 0
}

func main() {
	os.Exit(runMain(os.Args[1:]))
}
