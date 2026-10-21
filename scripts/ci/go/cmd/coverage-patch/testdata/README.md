# coverage-patch testdata

All `*.diff` fixtures must be git-format unified diffs with `diff
--git` headers, as produced by the rust.yml patch step:

    git diff --unified=0 FETCH_HEAD HEAD -- rust/ > patch.diff

Context lines are ignored (only `diff --git` / `---` / `+++` / `@@`
headers are read), but `-U0` keeps the artifact small and the added
ranges exact.

- `pass.diff` / `fail.diff`: covered / dark added lines (3 / 5 added
  lines across 2 files).
- `empty.diff`: deletions only; zero added lines, vacuous pass.
- `headerless.diff`: plain `diff -u` output with no `diff --git`
  headers; the tool fails closed (exit 1) instead of passing
  vacuously.
