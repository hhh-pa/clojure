package main

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/cmdtest"
)

func TestProbeCommon(t *testing.T) {
	t.Setenv("RUNNER_NAME", "test-runner-1")
	t.Setenv("RUNNER_OS", "TestOS")
	t.Setenv("RUNNER_ARCH", "TestArch")
	probes := probeCommon()
	if len(probes) != 3 {
		t.Fatalf("len(probes) = %d, want 3", len(probes))
	}
	if probes[0] != (Probe{Label: "runner", Value: "test-runner-1 (TestOS/TestArch)"}) {
		t.Fatalf("runner probe = %+v", probes[0])
	}
	if probes[1].Label != "os" || probes[1].Value == "" {
		t.Fatalf("os probe = %+v", probes[1])
	}
	if probes[2].Label != "cpu_count" || probes[2].Value == "" || probes[2].Value == "0" {
		t.Fatalf("cpu_count probe = %+v", probes[2])
	}
}

func TestFmtGB(t *testing.T) {
	if got := fmtGB(16_000_000_000); got != "16.0G" {
		t.Fatalf("fmtGB = %q, want 16.0G", got)
	}
	if got := fmtGB(0); got != "0.0G" {
		t.Fatalf("fmtGB(0) = %q", got)
	}
}

func TestFirstLine(t *testing.T) {
	if got := firstLine("  hello\nworld\n"); got != "hello" {
		t.Fatalf("firstLine = %q", got)
	}
	if got := firstLine("\n  \n"); got != "unknown" {
		t.Fatalf("firstLine(blank) = %q", got)
	}
}

func TestRender(t *testing.T) {
	probes := []Probe{{Label: "cpu_count", Value: "4"}, {Label: "memory", Value: "16.0G"}}
	wantLog := "runner hardware profile\ncpu_count: 4\nmemory: 16.0G\n"
	if got := renderLog(probes); got != wantLog {
		t.Fatalf("renderLog:\n got: %q\nwant: %q", got, wantLog)
	}
	wantMD := "## Runner hardware profile\n\n| probe | value |\n| ----- | ----- |\n" +
		"| cpu_count | 4 |\n| memory | 16.0G |\n"
	if got := renderMarkdown(probes); got != wantMD {
		t.Fatalf("renderMarkdown:\n got: %q\nwant: %q", got, wantMD)
	}
}

func TestParseCPUInfo(t *testing.T) {
	data := "processor\t: 0\nmodel name\t: Test CPU 9000\ncpu MHz\t\t: 1234.500\ncache size\t: 8192 KB\n" +
		"processor\t: 1\nmodel name\t: Second (ignored)\n"
	model, mhz, cache := parseCPUInfo(data)
	if model != "Test CPU 9000" || mhz != "1234.500" || cache != "8192 KB" {
		t.Fatalf("parseCPUInfo = %q %q %q", model, mhz, cache)
	}
	model, mhz, cache = parseCPUInfo("processor\t: 0\n")
	if model != "unknown" || mhz != "unknown" || cache != "unknown" {
		t.Fatalf("parseCPUInfo(sparse) = %q %q %q", model, mhz, cache)
	}
}

func TestParseMeminfo(t *testing.T) {
	total, avail := parseMeminfo("MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\n")
	if total != 16384000*1024 || avail != 8000000*1024 {
		t.Fatalf("parseMeminfo = %d %d", total, avail)
	}
	if total, avail := parseMeminfo("bogus\n"); total != 0 || avail != 0 {
		t.Fatalf("parseMeminfo(bogus) = %d %d", total, avail)
	}
}

func TestRotationalTypes(t *testing.T) {
	root := t.TempDir()
	write := func(dev, value string) {
		dir := filepath.Join(root, dev, "queue")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "rotational"), []byte(value), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("sda", "0\n")
	write("sdb", "1\n")
	write("loop0", "0\n")
	if got := rotationalTypes(root); got != "sda:ssd sdb:hdd" {
		t.Fatalf("rotationalTypes = %q", got)
	}
	if got := rotationalTypes(filepath.Join(root, "missing")); got != "unknown" {
		t.Fatalf("rotationalTypes(missing) = %q", got)
	}
}

func TestParseWinHW(t *testing.T) {
	fields := parseWinHW("Test CPU|4|8|3500|12288|17179869184")
	want := [6]string{"Test CPU", "4", "8", "3500", "12288", "17179869184"}
	if fields != want {
		t.Fatalf("parseWinHW = %q", fields)
	}
	short := parseWinHW("only|two")
	if short[0] != "only" || short[1] != "two" || short[5] != "" {
		t.Fatalf("parseWinHW(short) = %q", short)
	}
}

func TestAzureVMSize(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Metadata") != "true" {
			w.WriteHeader(http.StatusBadRequest)
			return
		}
		fmt.Fprint(w, "Standard_D4s_v3")
	}))
	defer server.Close()
	client := &http.Client{Timeout: 5 * time.Second}
	if got := azureVMSize(client, server.URL); got != "Standard_D4s_v3" {
		t.Fatalf("azureVMSize = %q", got)
	}
	broken := &http.Client{Timeout: time.Millisecond}
	if got := azureVMSize(broken, "http://127.0.0.1:1/"); got != "unknown" {
		t.Fatalf("azureVMSize(unreachable) = %q", got)
	}
}

func TestSmokeBinary(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	out, code := cmdtest.Run(t, bin, t.TempDir())
	if code != 0 {
		t.Fatalf("exit = %d\n%s", code, out)
	}
	for _, want := range []string{"runner hardware profile", "cpu_count", "memory"} {
		if !strings.Contains(out, want) {
			t.Fatalf("output lacks %q:\n%s", want, out)
		}
	}
}

func TestUsage(t *testing.T) {
	bin := cmdtest.Build(t, ".")
	out, code := cmdtest.Run(t, bin, t.TempDir(), "--bogus")
	if code != 2 {
		t.Fatalf("exit = %d, want 2\n%s", code, out)
	}
	if !strings.Contains(out, "Usage: runner-profile") {
		t.Fatalf("output lacks usage:\n%s", out)
	}
}
