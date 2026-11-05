// Command h2h-compare is the head-to-head gate: reference leg vs
// challenger leg on cpu_s + maxrss_kb (+ pin_instructions and
// pin_allocated_bytes via --metrics).
//
// Usage:
//
//	h2h-compare --jvm BASE.json [BASE.json ...] --rs CHALLENGER.json [...]
//	            --label SUITE [--summary-file FILE] [--report-only]
//	            [--merged-jvm FILE] [--merged-rs FILE]
//	            [--base-name NAME] [--challenger-name NAME]
//	            [--metrics cpu,mem[,ins]] [--margin F]
//
// This is a Go port of scripts/ci/h2h_compare.py: stdout, the
// summary Markdown, verdicts, and exit codes (0 pass, 1 gate fail,
// 2 usage/load error) match the Python oracle byte-for-byte. Two
// deliberate differences: --help/usage errors print a short
// h2h_compare-prefixed line instead of argparse's usage text, and
// the merged leg JSONs may order object keys differently (they are
// consumed as JSON, so only the parsed content matters).
//
// The ins metric reads summary.pin_instructions (the native --pin
// signal), falling back to summary.instructions (the retired
// pin_sweep.py shape).
package main

import (
	"fmt"
	"os"
	"strconv"
	"strings"
)

// options are the parsed CLI flags.
type options struct {
	jvm            []string
	rs             []string
	baseName       string
	challengerName string
	label          string
	summaryFile    string
	reportOnly     bool
	mergedJVM      string
	mergedRS       string
	metrics        string
	margin         float64
}

const usage = `h2h-compare: reference-vs-challenger CPU/RSS gate

Usage: h2h-compare --jvm BASE.json [BASE.json ...] --rs CHALLENGER.json [...]
                   --label SUITE [--summary-file FILE] [--report-only]
                   [--merged-jvm FILE] [--merged-rs FILE]
                   [--base-name NAME] [--challenger-name NAME]
                   [--metrics CSv] [--margin F]
`

// parseArgs ports argparse's flags: --jvm/--rs collect every
// following token until the next --flag (repeatable, appends);
// scalars take the next token or the --flag=value form.
func parseArgs(argv []string) (options, error) {
	o := options{baseName: "jvm", challengerName: "rs", metrics: "cpu,mem"}
	setScalar := func(name, value string) error {
		switch name {
		case "base-name":
			o.baseName = value
		case "challenger-name":
			o.challengerName = value
		case "label":
			o.label = value
		case "summary-file":
			o.summaryFile = value
		case "merged-jvm":
			o.mergedJVM = value
		case "merged-rs":
			o.mergedRS = value
		case "metrics":
			o.metrics = value
		case "margin":
			f, err := strconv.ParseFloat(value, 64)
			if err != nil {
				return fmt.Errorf("invalid --margin %q", value)
			}
			o.margin = f
		default:
			return fmt.Errorf("unknown flag '--%s'", name)
		}
		return nil
	}
	i := 0
	for i < len(argv) {
		tok := argv[i]
		name := tok
		inline := ""
		hasInline := false
		if v, ok := strings.CutPrefix(tok, "--"); ok {
			name = v
			if n, v, found := strings.Cut(v, "="); found {
				name, inline, hasInline = n, v, true
			}
		} else {
			return o, fmt.Errorf("unexpected argument %q", tok)
		}
		switch name {
		case "jvm", "rs":
			var paths []string
			if hasInline {
				paths = append(paths, inline)
				i++
			} else {
				i++
				for i < len(argv) && !strings.HasPrefix(argv[i], "--") {
					paths = append(paths, argv[i])
					i++
				}
			}
			if len(paths) == 0 {
				return o, fmt.Errorf("--%s needs at least one JSON", name)
			}
			if name == "jvm" {
				o.jvm = append(o.jvm, paths...)
			} else {
				o.rs = append(o.rs, paths...)
			}
		case "report-only":
			if hasInline {
				return o, fmt.Errorf("--report-only takes no value")
			}
			o.reportOnly = true
			i++
		case "help":
			return o, errHelp
		default:
			if hasInline {
				if err := setScalar(name, inline); err != nil {
					return o, err
				}
				i++
				continue
			}
			if i+1 >= len(argv) {
				return o, fmt.Errorf("--%s needs a value", name)
			}
			if err := setScalar(name, argv[i+1]); err != nil {
				return o, err
			}
			i += 2
		}
	}
	if len(o.jvm) == 0 {
		return o, fmt.Errorf("missing required --jvm")
	}
	if len(o.rs) == 0 {
		return o, fmt.Errorf("missing required --rs")
	}
	if o.label == "" {
		return o, fmt.Errorf("missing required --label")
	}
	return o, nil
}

// errHelp requests the usage text on stdout with exit 0.
var errHelp = fmt.Errorf("help")

func run(argv []string) int {
	o, err := parseArgs(argv)
	if err == errHelp {
		fmt.Print(usage)
		return 0
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "h2h_compare: %v\n", err)
		return 2
	}
	return compare(o)
}

func main() {
	os.Exit(run(os.Args[1:]))
}
