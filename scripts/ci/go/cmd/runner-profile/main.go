// Command runner-profile prints the CI runner's hardware profile (T1170
// PROF): CPU count/model/frequency/cache, memory, and storage, one block
// on stdout plus a Markdown table appended to GITHUB_STEP_SUMMARY when
// set. Every probe is best-effort and fast; failures degrade to
// "unknown" and the exit code stays 0 so profiling never fails CI.
//
// Usage: runner-profile
package main

import (
	"fmt"
	"os"
	"runtime"
	"strings"
)

const usage = `Runner hardware profile (T1170 PROF): log CPU, memory, and
storage for the machine executing this CI job, to stdout and the job
summary, so per-execution performance can be understood.

Usage: runner-profile
`

// Probe is one label/value hardware fact.
type Probe struct {
	Label string
	Value string
}

// probeCommon gathers OS-independent facts from the runtime and the
// standard GitHub Actions environment.
func probeCommon() []Probe {
	runner := os.Getenv("RUNNER_NAME")
	if runner == "" {
		runner = "unknown"
	}
	arch := os.Getenv("RUNNER_ARCH")
	if arch == "" {
		arch = runtime.GOARCH
	}
	osName := os.Getenv("RUNNER_OS")
	if osName == "" {
		osName = runtime.GOOS
	}
	return []Probe{
		{Label: "runner", Value: fmt.Sprintf("%s (%s/%s)", runner, osName, arch)},
		{Label: "os", Value: fmt.Sprintf("%s/%s", runtime.GOOS, runtime.GOARCH)},
		{Label: "cpu_count", Value: fmt.Sprintf("%d", runtime.NumCPU())},
	}
}

// fmtGB renders bytes as decimal gigabytes with one decimal.
func fmtGB(bytes uint64) string {
	return fmt.Sprintf("%.1fG", float64(bytes)/1e9)
}

// firstLine trims command output to its first non-empty line, or
// "unknown" when there is none.
func firstLine(out string) string {
	for _, line := range strings.Split(out, "\n") {
		if trimmed := strings.TrimSpace(line); trimmed != "" {
			return trimmed
		}
	}
	return "unknown"
}

// renderLog formats probes as the stdout block.
func renderLog(probes []Probe) string {
	var out strings.Builder
	out.WriteString("runner hardware profile\n")
	for _, probe := range probes {
		fmt.Fprintf(&out, "%s: %s\n", probe.Label, probe.Value)
	}
	return out.String()
}

// renderMarkdown formats probes as the job-summary table.
func renderMarkdown(probes []Probe) string {
	var out strings.Builder
	out.WriteString("## Runner hardware profile\n\n")
	out.WriteString("| probe | value |\n")
	out.WriteString("| ----- | ----- |\n")
	for _, probe := range probes {
		fmt.Fprintf(&out, "| %s | %s |\n", probe.Label, probe.Value)
	}
	return out.String()
}

func run(argv []string) int {
	if len(argv) != 0 {
		fmt.Print(usage)
		return 2
	}
	probes := append(probeCommon(), probeOS()...)
	fmt.Print(renderLog(probes))
	if summary := os.Getenv("GITHUB_STEP_SUMMARY"); summary != "" {
		handle, err := os.OpenFile(summary, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o666)
		if err != nil {
			return 0
		}
		fmt.Fprint(handle, renderMarkdown(probes))
		handle.Close()
	}
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
