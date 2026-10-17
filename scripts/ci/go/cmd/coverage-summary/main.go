// Command coverage-summary is the Go port of
// scripts/ci/lib/coverage-summary.py: render an lcov.info report as
// GitHub Markdown (job-summary flavor to stdout, PR-comment flavor to
// --out). Usage text names this binary (the Python usage hardcodes its
// own filename); all other output matches the oracle byte-for-byte.
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/hhh-pa/arcanist/scripts/ci/go/internal/lcov"
)

// Must match the floor enforced by rust-quality-gates (the
// coverage-floor invocation): the gates fail the run, this only
// reports the verdict.
const threshold = 80.0

const usage = `Render an lcov.info coverage report as GitHub Markdown.

Usage: coverage-summary [--out COMMENT.md] <lcov.info>
  Prints the job-summary flavor to stdout; with --out, also writes the
  PR-comment flavor to FILE (parents created). Exits 0 with an
  "unavailable" note when the report is missing (an earlier gate failed
  before coverage ran).

Both flavors show the total line rate plus the 15 lowest-covered files;
the full HTML report rides along as the ` + "`coverage-report`" + ` workflow
artifact. The comment flavor adds the threshold verdict plus a link to
the run that produced it.

`

func displayPath(path string) string {
	const marker = "rust/"
	slash := strings.ReplaceAll(path, string(os.PathSeparator), "/")
	if i := strings.LastIndex(slash, marker); i >= 0 {
		return path[i:]
	}
	if i := strings.LastIndex(path, string(os.PathSeparator)); i >= 0 {
		return path[i+1:]
	}
	return path
}

func runURL() string {
	server := os.Getenv("GITHUB_SERVER_URL")
	repo := os.Getenv("GITHUB_REPOSITORY")
	runID := os.Getenv("GITHUB_RUN_ID")
	if server == "" || repo == "" || runID == "" {
		return ""
	}
	return server + "/" + repo + "/actions/runs/" + runID
}

func rate(found, hit int64) float64 {
	if found != 0 {
		return float64(hit) / float64(found)
	}
	return 1.0
}

func tableLines(files []lcov.FileSummary) []string {
	lines := []string{"| File | Lines | Hit | %% |", "| ---- | ----- | --- | - |"}
	rows := append([]lcov.FileSummary(nil), files...)
	sort.SliceStable(rows, func(i, j int) bool {
		ri, rj := rate(rows[i].Found, rows[i].Hit), rate(rows[j].Found, rows[j].Hit)
		if ri != rj {
			return ri < rj
		}
		return rows[i].Path < rows[j].Path
	})
	shown := rows
	if len(shown) > 15 {
		shown = shown[:15]
	}
	for _, f := range shown {
		r := 100.0
		if f.Found != 0 {
			r = 100.0 * float64(f.Hit) / float64(f.Found)
		}
		lines = append(lines, fmt.Sprintf(
			"| `%s` | %d | %d | %.2f%% |",
			displayPath(f.Path), f.Found, f.Hit, r,
		))
	}
	shownCount := len(rows)
	if shownCount > 15 {
		shownCount = 15
	}
	lines = append(lines,
		"",
		fmt.Sprintf(
			"Lowest %d of %d files. Full HTML report: download the "+
				"`coverage-report` artifact.",
			shownCount, len(rows),
		),
	)
	return lines
}

func renderSummary(total float64, files []lcov.FileSummary) string {
	return strings.Join(
		append([]string{fmt.Sprintf("## Coverage: %.2f%% (lines)", total), ""},
			tableLines(files)...),
		"\n",
	) + "\n"
}

func renderComment(total float64, files []lcov.FileSummary) string {
	verdict := "PASS"
	if total < threshold {
		verdict = "FAIL"
	}
	lines := append([]string{
		fmt.Sprintf("## Coverage report: %.2f%% (lines)", total),
		"",
		fmt.Sprintf("Threshold %.0f%%: **%s** (enforced by the quality gates).",
			threshold, verdict),
		"",
	}, tableLines(files)...)
	if url := runURL(); url != "" {
		lines = append(lines,
			"",
			fmt.Sprintf("<sub>Updated by the [quality gates run](%s) on every push.</sub>", url),
		)
	} else {
		lines = append(lines, "", "<sub>Posted by the quality gates.</sub>")
	}
	return strings.Join(lines, "\n") + "\n"
}

func run(argv []string) int {
	out := ""
	args := append([]string(nil), argv...)
	for i, a := range args {
		if a == "--out" {
			if i+1 >= len(args) {
				fmt.Print(usage)
				return 2
			}
			out = args[i+1]
			args = append(args[:i], args[i+2:]...)
			break
		}
	}
	if len(args) != 1 {
		fmt.Print(usage)
		return 2
	}
	var summary, comment string
	data, err := os.ReadFile(args[0])
	if err != nil {
		summary = "## Coverage: unavailable\n" +
			"\n" +
			"No `lcov.info` was produced (an earlier gate failed first).\n"
		comment = strings.Replace(summary, "## Coverage:", "## Coverage report:", 1)
	} else {
		// Mirror errors="replace": Go strings may hold invalid
		// UTF-8; Python would have replaced it.
		found, hit, files := lcov.ParseSummaries(lcov.SanitizeUTF8(string(data)))
		total := 100.0
		if found != 0 {
			total = 100.0 * float64(hit) / float64(found)
		}
		summary = renderSummary(total, files)
		comment = renderComment(total, files)
	}
	fmt.Print(summary)
	if out != "" {
		abs, err := filepath.Abs(out)
		if err != nil {
			fmt.Fprintf(os.Stderr, "cannot write %s: %v\n", out, err)
			return 1
		}
		if err := os.MkdirAll(filepath.Dir(abs), 0o777); err != nil {
			fmt.Fprintf(os.Stderr, "cannot write %s: %v\n", out, err)
			return 1
		}
		if err := os.WriteFile(out, []byte(comment), 0o666); err != nil {
			fmt.Fprintf(os.Stderr, "cannot write %s: %v\n", out, err)
			return 1
		}
	}
	return 0
}

func main() {
	os.Exit(run(os.Args[1:]))
}
