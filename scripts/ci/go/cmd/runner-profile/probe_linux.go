//go:build linux

package main

import (
	"fmt"
	"net/http"
	"os"
	"time"
)

func probeOS() []Probe {
	model, mhz, cache := parseCPUInfo(readFileOrEmpty("/proc/cpuinfo"))
	total, avail := parseMeminfo(readFileOrEmpty("/proc/meminfo"))
	client := &http.Client{Timeout: 3 * time.Second}
	return []Probe{
		{Label: "cpu_model", Value: model},
		{Label: "cpu_mhz", Value: mhz},
		{Label: "cpu_cache", Value: cache},
		{Label: "memory", Value: fmt.Sprintf("%s total / %s avail", fmtGB(total), fmtGB(avail))},
		{Label: "disk_root", Value: diskUsage("/")},
		{Label: "disk_media", Value: rotationalTypes("/sys/block")},
		{Label: "azure_vm_size", Value: azureVMSize(client,
			"http://169.254.169.254/metadata/instance/compute/vmSize?api-version=2021-02-01&format=text")},
	}
}

func readFileOrEmpty(path string) string {
	data, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	return string(data)
}
