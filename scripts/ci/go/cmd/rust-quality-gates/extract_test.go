package main

import (
	"archive/tar"
	"compress/gzip"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type tarEntry struct {
	name     string
	linkname string
	flag     byte
	body     string
	mode     int64
}

func writeTar(t *testing.T, path string, entries []tarEntry) {
	t.Helper()
	fh, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	gz := gzip.NewWriter(fh)
	tw := tar.NewWriter(gz)
	for _, e := range entries {
		mode := e.mode
		if mode == 0 {
			mode = 0o644
		}
		if err := tw.WriteHeader(&tar.Header{
			Name:     e.name,
			Linkname: e.linkname,
			Typeflag: e.flag,
			Mode:     mode,
			Size:     int64(len(e.body)),
		}); err != nil {
			t.Fatal(err)
		}
		if _, err := tw.Write([]byte(e.body)); err != nil {
			t.Fatal(err)
		}
	}
	if err := tw.Close(); err != nil {
		t.Fatal(err)
	}
	if err := gz.Close(); err != nil {
		t.Fatal(err)
	}
	if err := fh.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestExtractTarGzKeepsNormalTree(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "tool.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "tool-dir/", flag: tar.TypeDir, mode: 0o755},
		{name: "tool-dir/tool", flag: tar.TypeReg, body: "binary"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "tool-dir", "tool"))
	if err != nil || string(blob) != "binary" {
		t.Fatalf("blob = %q, %v", blob, err)
	}
}

func TestExtractTarGzSkipsDegenerateDotEntries(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "dot.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "./", flag: tar.TypeDir, mode: 0o755},
		{name: "ok", flag: tar.TypeReg, body: "fine"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "ok"))
	if err != nil || string(blob) != "fine" {
		t.Fatalf("blob = %q, %v", blob, err)
	}
}

func TestExtractTarGzRejectsEscapes(t *testing.T) {
	cases := []struct {
		name  string
		entry tarEntry
		want  string
	}{
		{"dotdot", tarEntry{name: "../evil", flag: tar.TypeReg, body: "x"}, "escapes"},
		{"nested-dotdot", tarEntry{name: "a/../../evil", flag: tar.TypeReg, body: "x"}, "escapes"},
		{"absolute", tarEntry{name: "/tmp/evil", flag: tar.TypeReg, body: "x"}, "absolute"},
		{
			"symlink-out",
			tarEntry{name: "link", linkname: "../evil", flag: tar.TypeSymlink},
			"symlink escapes",
		},
		{
			"symlink-absolute",
			tarEntry{name: "link", linkname: "/etc/passwd", flag: tar.TypeSymlink},
			"symlink escapes",
		},
		{
			"hardlink-out",
			tarEntry{name: "link", linkname: "../evil", flag: tar.TypeLink},
			"hardlink escapes",
		},
		{
			"hardlink-absolute",
			tarEntry{name: "link", linkname: "/etc/passwd", flag: tar.TypeLink},
			"hardlink escapes",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			archive := filepath.Join(dir, "evil.tar.gz")
			dest := filepath.Join(dir, "out")
			writeTar(t, archive, []tarEntry{c.entry})
			err := extractTarGz(archive, dest)
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Fatalf("err = %v, want %q", err, c.want)
			}
			if _, statErr := os.Lstat(filepath.Join(dir, "evil")); !os.IsNotExist(statErr) {
				t.Fatal("escape wrote outside dest")
			}
		})
	}
}

func TestExtractTarGzRejectsAbsoluteDotDotSymlink(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "evil.tar.gz")
	dest := filepath.Join(dir, "out")
	// Absolute target that starts with dest lexically but resolves
	// outside it: must fail the (cleaned) containment check.
	writeTar(t, archive, []tarEntry{{
		name:     "link",
		linkname: dest + string(filepath.Separator) + ".." + string(filepath.Separator) + "evil",
		flag:     tar.TypeSymlink,
	}})
	err := extractTarGz(archive, dest)
	if err == nil || !strings.Contains(err.Error(), "symlink escapes") {
		t.Fatalf("err = %v, want %q", err, "symlink escapes")
	}
	if _, statErr := os.Lstat(filepath.Join(dir, "evil")); !os.IsNotExist(statErr) {
		t.Fatal("escape wrote outside dest")
	}
}

func TestExtractTarGzKeepsValidSymlink(t *testing.T) {
	dir := t.TempDir()
	probe := filepath.Join(dir, "probe")
	if err := os.Symlink("target.txt", probe); err != nil {
		t.Skipf("symlink creation unavailable: %v", err)
	}
	os.Remove(probe)
	archive := filepath.Join(dir, "tool.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "target.txt", flag: tar.TypeReg, body: "data"},
		{name: "link", linkname: "target.txt", flag: tar.TypeSymlink},
		// Link stored before its destination: the lenient
		// resolution must tolerate the missing leaf.
		{name: "early", linkname: "later.txt", flag: tar.TypeSymlink},
		{name: "later.txt", flag: tar.TypeReg, body: "late"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	got, err := os.Readlink(filepath.Join(dest, "link"))
	if err != nil || got != "target.txt" {
		t.Fatalf("readlink = %q, %v", got, err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "link"))
	if err != nil || string(blob) != "data" {
		t.Fatalf("through link = %q, %v", blob, err)
	}
	blob, err = os.ReadFile(filepath.Join(dest, "early"))
	if err != nil || string(blob) != "late" {
		t.Fatalf("through early link = %q, %v", blob, err)
	}
}

func TestExtractTarGzKeepsValidHardlink(t *testing.T) {
	dir := t.TempDir()
	archive := filepath.Join(dir, "tool.tar.gz")
	dest := filepath.Join(dir, "out")
	writeTar(t, archive, []tarEntry{
		{name: "orig.txt", flag: tar.TypeReg, body: "data"},
		{name: "hard", linkname: "orig.txt", flag: tar.TypeLink},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "hard"))
	if err != nil || string(blob) != "data" {
		t.Fatalf("through hardlink = %q, %v", blob, err)
	}
}

func TestExtractTarGzRejectsLinksThroughPlantedSymlink(t *testing.T) {
	cases := []struct {
		name  string
		entry tarEntry
		want  string
	}{
		{"link-location", tarEntry{name: "escape/link", linkname: "victim", flag: tar.TypeSymlink}, "symlink location escapes"},
		{"link-target", tarEntry{name: "link", linkname: "escape/victim", flag: tar.TypeSymlink}, "symlink escapes"},
		{"hardlink-location", tarEntry{name: "escape/hard", linkname: "orig.txt", flag: tar.TypeLink}, "hardlink location escapes"},
		{"hardlink-target", tarEntry{name: "hard", linkname: "escape/victim", flag: tar.TypeLink}, "hardlink escapes"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			outside := t.TempDir()
			dest := filepath.Join(dir, "out")
			if err := os.MkdirAll(dest, 0o755); err != nil {
				t.Fatal(err)
			}
			// Planted redirect, as an earlier archive entry could leave one.
			if err := os.Symlink(outside, filepath.Join(dest, "escape")); err != nil {
				t.Skipf("symlink creation unavailable: %v", err)
			}
			archive := filepath.Join(dir, "evil.tar.gz")
			writeTar(t, archive, []tarEntry{c.entry})
			err := extractTarGz(archive, dest)
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Fatalf("err = %v, want %q", err, c.want)
			}
			left, readErr := os.ReadDir(outside)
			if readErr != nil || len(left) != 0 {
				t.Fatalf("planted escape wrote outside dest: %v %v", left, readErr)
			}
		})
	}
}

func TestExtractTarGzRejectsAbsoluteLinkThroughPlantedSymlink(t *testing.T) {
	dir := t.TempDir()
	outside := t.TempDir()
	dest := filepath.Join(dir, "out")
	if err := os.MkdirAll(dest, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(dest, "escape")); err != nil {
		t.Skipf("symlink creation unavailable: %v", err)
	}
	// Absolute target under dest/escape: passes the lexical Clean
	// check, so only real resolution can catch the redirect.
	archive := filepath.Join(dir, "evil.tar.gz")
	writeTar(t, archive, []tarEntry{{
		name:     "link",
		linkname: filepath.Join(dest, "escape", "victim"),
		flag:     tar.TypeSymlink,
	}})
	err := extractTarGz(archive, dest)
	if err == nil || !strings.Contains(err.Error(), "symlink escapes") {
		t.Fatalf("err = %v, want %q", err, "symlink escapes")
	}
	left, readErr := os.ReadDir(outside)
	if readErr != nil || len(left) != 0 {
		t.Fatalf("planted escape wrote outside dest: %v %v", left, readErr)
	}
}

func TestExtractTarGzKeepsFirstEntryLinkUnderSymlinkedAncestor(t *testing.T) {
	dir := t.TempDir()
	real := filepath.Join(dir, "real")
	if err := os.MkdirAll(real, 0o755); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(dir, "alias")
	if err := os.Symlink(real, alias); err != nil {
		t.Skipf("symlink creation unavailable: %v", err)
	}
	// Dest missing with a symlinked ancestor (macOS TMPDIR): the
	// root and the candidate must resolve against the same real
	// prefix or a valid link is falsely rejected.
	dest := filepath.Join(alias, "out")
	archive := filepath.Join(dir, "tool.tar.gz")
	writeTar(t, archive, []tarEntry{
		{name: "link", linkname: "target.txt", flag: tar.TypeSymlink},
		{name: "target.txt", flag: tar.TypeReg, body: "data"},
	})
	if err := extractTarGz(archive, dest); err != nil {
		t.Fatal(err)
	}
	blob, err := os.ReadFile(filepath.Join(dest, "link"))
	if err != nil || string(blob) != "data" {
		t.Fatalf("through link = %q, %v", blob, err)
	}
}
