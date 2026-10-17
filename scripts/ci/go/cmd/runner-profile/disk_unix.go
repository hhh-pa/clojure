//go:build linux || darwin

package main

import (
	"fmt"
	"syscall"
)

// diskUsage reports total/free space for path's filesystem.
func diskUsage(path string) string {
	var stat syscall.Statfs_t
	if err := syscall.Statfs(path, &stat); err != nil {
		return "unknown"
	}
	block := uint64(stat.Bsize)
	return fmt.Sprintf("%s total / %s free", fmtGB(block*stat.Blocks), fmtGB(block*stat.Bavail))
}
