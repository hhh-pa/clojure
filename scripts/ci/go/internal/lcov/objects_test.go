package lcov

import (
	"debug/elf"
	"encoding/binary"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
)

func makeSyntheticELF(sections ...string) []byte {
	names := append([]string{".shstrtab"}, sections...)
	shstrtab := []byte{0}
	nameOffs := make([]uint32, len(names))
	for i, n := range names {
		nameOffs[i] = uint32(len(shstrtab))
		shstrtab = append(shstrtab, n...)
		shstrtab = append(shstrtab, 0)
	}
	const hdrSize = 64
	const shSize = 64
	shNum := 1 + len(names)
	strOff := uint64(hdrSize + shNum*shSize)

	out := make([]byte, int(strOff)+len(shstrtab))
	copy(out[0:4], "\x7fELF")
	out[4] = byte(elf.ELFCLASS64)
	out[5] = byte(elf.ELFDATA2LSB)
	out[6] = byte(elf.EV_CURRENT)
	le := binary.LittleEndian
	le.PutUint16(out[16:18], uint16(elf.ET_EXEC))
	le.PutUint16(out[18:20], uint16(elf.EM_X86_64))
	le.PutUint32(out[20:24], uint32(elf.EV_CURRENT))
	le.PutUint64(out[40:48], hdrSize)
	le.PutUint16(out[52:54], hdrSize)
	le.PutUint16(out[58:60], shSize)
	le.PutUint16(out[60:62], uint16(shNum))
	le.PutUint16(out[62:64], 1)

	for i := range names {
		off := hdrSize + (i+1)*shSize
		shType := uint32(elf.SHT_PROGBITS)
		shSizeVal := uint64(0)
		if i == 0 {
			shType = uint32(elf.SHT_STRTAB)
			shSizeVal = uint64(len(shstrtab))
		}
		le.PutUint32(out[off:off+4], nameOffs[i])
		le.PutUint32(out[off+4:off+8], shType)
		le.PutUint64(out[off+24:off+32], strOff)
		le.PutUint64(out[off+32:off+40], shSizeVal)
		le.PutUint64(out[off+48:off+56], 1)
	}
	copy(out[strOff:], shstrtab)
	return out
}

func writeTestFile(t *testing.T, path string, data []byte, mode os.FileMode) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, data, mode); err != nil {
		t.Fatal(err)
	}
}

func TestTestObjectsSupportsLegacyAndNewLayouts(t *testing.T) {
	root := t.TempDir()

	legacy := filepath.Join(root, "debug", "deps", "legacy_stub")
	newDebug := filepath.Join(root, "debug", "build", "clj-core", "abc123", "out", "clj_core-abc123")
	newRelease := filepath.Join(root, "release", "build", "clj-value", "def456", "out", "clj_value-def456")

	writeTestFile(t, legacy, []byte("#!/bin/sh\n"), 0o755)
	writeTestFile(t, filepath.Join(root, "debug", "deps", "libclj.rlib"), []byte("archive"), 0o755)
	writeTestFile(t, newDebug, makeSyntheticELF("__llvm_covmap"), 0o755)
	writeTestFile(t, newRelease, makeSyntheticELF(".llvm_covmap"), 0o755)

	// Ignored executables: build scripts, uninstrumented ELFs, nested OUT_DIR subdirs.
	writeTestFile(t, filepath.Join(root, "debug", "build", "num-traits", "111", "out", "build_script_build"),
		makeSyntheticELF("__llvm_covmap"), 0o755)
	writeTestFile(t, filepath.Join(root, "debug", "build", "helper", "222", "out", "helper"),
		makeSyntheticELF(".text"), 0o755)
	writeTestFile(t, filepath.Join(root, "debug", "build", "clj-aot", "333", "out", "bc-target", "release", "build", "x", "444", "out", "nested"),
		makeSyntheticELF("__llvm_covmap"), 0o755)
	if runtime.GOOS != "windows" {
		writeTestFile(t, filepath.Join(root, "debug", "deps", "nonexec"), makeSyntheticELF("__llvm_covmap"), 0o644)
	}

	got, err := TestObjects(root)
	if err != nil {
		t.Fatalf("TestObjects error: %v", err)
	}
	want := []string{newDebug, legacy, newRelease}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("TestObjects = %v, want %v", got, want)
	}
}

func TestTestObjectsEmptyReturnsError(t *testing.T) {
	root := t.TempDir()
	_, err := TestObjects(root)
	if err == nil || !strings.Contains(err.Error(), "no test objects under") {
		t.Fatalf("err = %v, want 'no test objects under ...'", err)
	}
}
