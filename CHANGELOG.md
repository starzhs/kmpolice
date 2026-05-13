# Changelog

## 0.1.6

### Summary
- Git robustness pass for `git`/`mr` modes (single improvement batch).
- Added runtime roots via CLI and elapsed-time progress for snapshot loading.
- Refined diff-scope strategy: Kotlin is changed-path scoped, iOS is full-scan in configured roots.
- `mr` now uses branch+worktree changed-path scope consistently and reports current branch diagnostics.
- Added optional `--shared-sdk-name` prefilter for Swift files.

### Changes
- Added unmerged state guard (`git ls-files -u`) with early explicit failure.
- Added detached HEAD awareness log.
- Added shallow repository awareness with merge-base remediation hint.
- Added meaningful code-diff guard to ignore non-code/noise-only diffs (`*.kt`, `*.swift`, EOL-aware).
- Added pragmatic filtering for obvious generated/build directories while collecting source files.
- Added `--kotlin-root` / `--ios-root` for `git` and `mr` commands.
- Added elapsed time to snapshot loading progress logs.
- `mr/git` now include dirty worktree changes in changed-path scope.
- `mr/git` now scan Swift files in full iOS scope to avoid missing breakages outside changed files.
- `paths` now prefers git-indexed tracked/untracked files (with fallback to filesystem walk).
- `--shared-sdk-name <module>` now filters iOS files to those importing the shared KMP module.

## 0.1.1

### Summary
- Maintenance release: progress reporting, fast-exit for identical refs, and pure Swift enum/sealed matching.

### Changes
- Added stderr progress reporting for `paths`, `git`, `mr`.
- Added fast-exit in `git`/`mr` when `base == head`.
- Added pure Swift support for enum/sealed checks (`switch value` and `if value is Type`).

## 0.1.0

### Summary
- First release focused on fast Kotlin->Swift impact checks in KMP repositories.
- Main reliability target is diff-aware analysis (`git` / `mr` modes).
- Tool is best-effort and designed to complement (not replace) full project builds.

### Core Functionality
- Kotlin interface vs Swift protocol compatibility checks.
- Swift conformance checks after Kotlin contract changes.
- Kotlin class method call checks from Swift (argument count and labels).
- Kotlin constructor call checks from Swift.
- Kotlin property usage checks from Swift:
  - missing/renamed properties,
  - mutability mismatches (`val` vs assignment),
  - nullability mismatches,
  - typed-read mismatches.
- Enum/sealed case coverage checks in Swift `switch onEnum`.
- Top-level Kotlin symbol checks (`*Kt.member`).
- Companion object/member checks.

### Diff-aware Reliability Enhancements
- `kotlin_type_usage_missing` now uses diff facts:
  - Kotlin symbol removed in diff,
  - same-name Swift symbol added in diff,
  - dependency manifest changes in diff.
- In ambiguous cases severity is softened to warning.
- In strong factual cases severity stays error.

### Explainability
- Added `evidence[]` to diagnostics JSON output.
- Text output now includes evidence lines when available.

### Test Coverage
- Added/expanded unit tests for constructor/property/enum-sealed/type/top-level/companion scenarios.
