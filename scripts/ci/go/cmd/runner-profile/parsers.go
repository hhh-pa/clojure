package main

import (
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

// parseCPUInfo returns the first processor's model name, MHz, and cache
// size from /proc/cpuinfo bytes. Missing fields read "unknown".
func parseCPUInfo(data string) (model, mhz, cache string) {
	model, mhz, cache = "unknown", "unknown", "unknown"
	for _, line := range strings.Split(data, "\n") {
		key, value, found := strings.Cut(line, ":")
		if !found {
			continue
		}
		value = strings.TrimSpace(value)
		if value == "" {
			continue
		}
		switch strings.TrimSpace(key) {
		case "model name":
			if model == "unknown" {
				model = value
			}
		case "cpu MHz":
			if mhz == "unknown" {
				mhz = value
			}
		case "cache size":
			if cache == "unknown" {
				cache = value
			}
		}
	}
	return model, mhz, cache
}

// parseMeminfo returns total and available bytes from /proc/meminfo
// bytes. Missing fields read 0.
func parseMeminfo(data string) (total, avail uint64) {
	for _, line := range strings.Split(data, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 2 {
			continue
		}
		kb, err := strconv.ParseUint(fields[1], 10, 64)
		if err != nil {
			continue
		}
		switch fields[0] {
		case "MemTotal:":
			total = kb * 1024
		case "MemAvailable:":
			avail = kb * 1024
		}
	}
	return total, avail
}

// rotationalTypes reports each non-virtual block device as name:ssd|hdd
// from its queue/rotational flag (0 is flash). Used to tell SSD from
// spinning runners; failures inside one device skip that device.
func rotationalTypes(sysBlock string) string {
	entries, err := os.ReadDir(sysBlock)
	if err != nil {
		return "unknown"
	}
	var out []string
	for _, entry := range entries {
		name := entry.Name()
		if strings.HasPrefix(name, "loop") || strings.HasPrefix(name, "ram") || strings.HasPrefix(name, "dm-") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(sysBlock, name, "queue", "rotational"))
		if err != nil {
			continue
		}
		kind := "hdd"
		if strings.TrimSpace(string(raw)) == "0" {
			kind = "ssd"
		}
		out = append(out, name+":"+kind)
	}
	if len(out) == 0 {
		return "unknown"
	}
	sort.Strings(out)
	return strings.Join(out, " ")
}

// parseWinHW splits the |-joined CPU/memory line from the Windows
// profiler call into its six fields, padding short rows with "".
func parseWinHW(line string) [6]string {
	var out [6]string
	for i, field := range strings.Split(line, "|") {
		if i >= len(out) {
			break
		}
		out[i] = strings.TrimSpace(field)
	}
	return out
}
