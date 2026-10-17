//go:build !windows

package executil

import (
	"errors"
	"syscall"
)

func isDirError(err error) bool { return errors.Is(err, syscall.EISDIR) }
