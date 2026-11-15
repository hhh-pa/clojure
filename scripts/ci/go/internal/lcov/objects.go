package lcov

import (
	"bytes"
	"debug/elf"
	"debug/macho"
	"encoding/binary"
	"errors"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// TestObjects returns sorted coverage-instrumented test object paths under
// covtarget, supporting both legacy Cargo (<1.100) `<profile>/deps/` and
// Cargo >=1.100 `<profile>/build/<pkg>/<hash>/out/` layouts.
func TestObjects(covtarget string) ([]string, error) {
	var objs []string
	seen := make(map[string]bool)
	for _, sub := range []string{"debug", "release"} {
		profile := filepath.Join(covtarget, sub)
		collectFlatDir(profile, seen, &objs)
		collectFlatDir(filepath.Join(profile, "deps"), seen, &objs)
		collectNewLayout(filepath.Join(profile, "build"), seen, &objs)
	}
	if len(objs) == 0 {
		return nil, errors.New("no test objects under " + covtarget)
	}
	slices.Sort(objs)
	return objs, nil
}

func collectFlatDir(dir string, seen map[string]bool, objs *[]string) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return
	}
	for _, e := range entries {
		addObjectCandidate(seen, objs, filepath.Join(dir, e.Name()), e.Name())
	}
}

func collectNewLayout(buildDir string, seen map[string]bool, objs *[]string) {
	pkgs, err := os.ReadDir(buildDir)
	if err != nil {
		return
	}
	for _, pkg := range pkgs {
		if !pkg.IsDir() {
			continue
		}
		pkgDir := filepath.Join(buildDir, pkg.Name())
		hashes, err := os.ReadDir(pkgDir)
		if err != nil {
			continue
		}
		for _, h := range hashes {
			if !h.IsDir() {
				continue
			}
			collectFlatDir(filepath.Join(pkgDir, h.Name(), "out"), seen, objs)
		}
	}
}

func addObjectCandidate(seen map[string]bool, objs *[]string, full, name string) {
	if seen[full] || strings.Contains(name, ".") {
		return
	}
	if name == "build-script-build" || strings.HasPrefix(name, "build_script_") {
		return
	}
	info, err := os.Stat(full)
	if err != nil || !isExecutable(info) {
		return
	}
	if !hasCoverageOrNonObject(full) {
		return
	}
	seen[full] = true
	*objs = append(*objs, full)
}

func isExecutable(info os.FileInfo) bool {
	if info.IsDir() {
		return false
	}
	if os.PathSeparator == '\\' {
		return true
	}
	return info.Mode()&0o111 != 0
}

func hasCoverageOrNonObject(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	var magic [4]byte
	_, err = io.ReadFull(f, magic[:])
	_ = f.Close()
	if err != nil {
		// Tiny non-ELF/Mach-O stub files in tests.
		return true
	}
	if bytes.Equal(magic[:], []byte("\x7fELF")) {
		return elfHasCoverage(path)
	}
	be := binary.BigEndian.Uint32(magic[:])
	le := binary.LittleEndian.Uint32(magic[:])
	switch {
	case be == macho.Magic32 || be == macho.Magic64 || le == macho.Magic32 || le == macho.Magic64:
		return machoHasCoverage(path)
	case be == macho.MagicFat || le == macho.MagicFat:
		ff, err := macho.OpenFat(path)
		if err != nil {
			return true
		}
		defer ff.Close()
		for _, arch := range ff.Arches {
			if machoFileHasCoverage(arch.File) {
				return true
			}
		}
		return false
	default:
		// Non-binary executable (e.g. shell-script stubs in tests).
		return true
	}
}

func elfHasCoverage(path string) bool {
	ef, err := elf.Open(path)
	if err != nil {
		return false
	}
	defer ef.Close()
	for _, sec := range ef.Sections {
		if isCoverageSection(sec.Name) {
			return true
		}
	}
	return false
}

func machoHasCoverage(path string) bool {
	mf, err := macho.Open(path)
	if err != nil {
		return false
	}
	defer mf.Close()
	return machoFileHasCoverage(mf)
}

func machoFileHasCoverage(mf *macho.File) bool {
	for _, sec := range mf.Sections {
		if isCoverageSection(sec.Name) {
			return true
		}
	}
	return false
}

func isCoverageSection(name string) bool {
	return strings.Contains(name, "llvm_cov") || strings.Contains(name, "lcovmap")
}
