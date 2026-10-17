//go:build windows

package main

import (
	"os"
	"strconv"
	"time"

	"github.com/hhh-pa/arcanist/scripts/ci/go/internal/executil"
)

const winHWScript = `$c = Get-CimInstance Win32_Processor | Select-Object -First 1
$s = Get-CimInstance Win32_ComputerSystem
@($c.Name, $c.NumberOfCores, $c.NumberOfLogicalProcessors, $c.MaxClockSpeed, $c.L3CacheSize, $s.TotalPhysicalMemory) -join "|"
`

const winDiskScript = `$d = Get-PSDrive C
$p = Get-PhysicalDisk | Select-Object -First 1
"C: $($d.Used/1GB)GB used / $($d.Free/1GB)GB free; $($p.FriendlyName): $($p.MediaType) $($p.Size/1GB)GB"
`

func powershell(script string) string {
	out, _, err := executil.CaptureTimeout(
		[]string{"powershell", "-NoProfile", "-NonInteractive", "-Command", script},
		os.Environ(), "", 30*time.Second)
	if err != nil {
		return ""
	}
	return firstLine(out)
}

func orUnknown(value string) string {
	if value == "" {
		return "unknown"
	}
	return value
}

func probeOS() []Probe {
	hw := parseWinHW(powershell(winHWScript))
	memBytes, err := strconv.ParseUint(hw[5], 10, 64)
	memory := "unknown"
	if err == nil && memBytes > 0 {
		memory = fmtGB(memBytes)
	}
	return []Probe{
		{Label: "cpu_model", Value: orUnknown(hw[0])},
		{Label: "cpu_cores", Value: orUnknown(hw[1]) + "C/" + orUnknown(hw[2]) + "T"},
		{Label: "cpu_max_mhz", Value: orUnknown(hw[3])},
		{Label: "cpu_l3_cache_kb", Value: orUnknown(hw[4])},
		{Label: "memory", Value: memory},
		{Label: "disk", Value: orUnknown(powershell(winDiskScript))},
	}
}
