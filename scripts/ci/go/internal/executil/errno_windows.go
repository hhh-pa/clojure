//go:build windows

package executil

func isDirError(_ error) bool { return false }
