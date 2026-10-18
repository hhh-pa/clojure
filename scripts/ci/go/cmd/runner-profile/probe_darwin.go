//go:build darwin

package main

import (
	"os"
	"strconv"
	"time"

	"github.com/hhh-pa/clojure/scripts/ci/go/internal/executil"
)

func sysctl(name string) string {
	out, _, err := executil.CaptureTimeout([]string{"sysctl", "-n", name}, os.Environ(), "", 10*time.Second)
	if err != nil {
		return "unknown"
	}
	return firstLine(out)
}

func probeOS() []Probe {
	memBytes, err := strconv.ParseUint(sysctl("hw.memsize"), 10, 64)
	if err != nil {
		memBytes = 0
	}
	return []Probe{
		{Label: "cpu_model", Value: sysctl("machdep.cpu.brand_string")},
		{Label: "cpu_physical", Value: sysctl("hw.physicalcpu")},
		{Label: "cpu_l3_cache", Value: sysctl("hw.l3cachesize")},
		{Label: "memory", Value: formatBytesGB(memBytes)},
		{Label: "disk_root", Value: diskUsage("/")},
	}
}

func formatBytesGB(bytes uint64) string {
	if bytes == 0 {
		return "unknown"
	}
	return fmtGB(bytes)
}
