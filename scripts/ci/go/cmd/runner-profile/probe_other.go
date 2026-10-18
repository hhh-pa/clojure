//go:build !linux && !darwin && !windows

package main

func probeOS() []Probe {
	return []Probe{{Label: "note", Value: "no OS probes for this platform"}}
}
