# Changelog

## 0.1.10

### Summary
- Fixed MR scoped loading so Kotlin changes under `iosMain` are not dropped by `kotlin_roots`.

### Changes
- In scoped (`changed_paths`) snapshot loading, root-based path filtering is bypassed.
- Applied to both:
  - git snapshot collection (`collect_git_files`)
  - worktree snapshot collection (`collect_worktree_git_list_files`)
- This ensures changed Kotlin files from `commonMain` and `iosMain` are included consistently in MR analysis.

## 0.1.9

### Summary
- Scoped MR snapshot iOS loading to changed Swift paths.
- Replaced legacy `loaded 200/...` snapshot logs with interactive progress bars.

### Changes
- `mr` now passes `ios_scope = swift_changed_paths` to snapshot loaders to avoid full Swift snapshot reads when unnecessary.
- Snapshot loading in `source::collect_git_files` now uses `indicatif` with:
  - progress counters
  - `last file` message
  - explicit stage completion message
- Removed old periodic text logs emitted every 200 files during snapshot load.

## 0.1.8

### Summary
- Implemented cascade Swift call-site scan from Kotlin MR changes.
- Removed mock progress mode and switched runtime to single MR flow.
- Added configurable shared SDK import filter (`--shared-sdk-name` + config fallback).
- Added touched/untouched Swift usage classification and diagnostics hints.
- Added tests for new iOS usage pipeline.

### Changes
- Removed CLI/runtime mock branch (`--mock-progress`, synthetic loaders).
- Added `shared_sdk_name` support:
  - CLI: `--shared-sdk-name`
  - config: `shared_sdk_name`
  - default: `shared`
- Reworked iOS usage search to path-based cascade with parallel stages:
  - enumerate
  - import filter
  - token filter
  - AST parse
  - usage match
- Added cascade progress reporting with per-stage counters and `last file`.
- Added Swift changed-file awareness (`already_touched` vs `untouched`) into:
  - usage report counters
  - MR impact diagnostic hints/evidence
- Added helper to collect scoped Swift paths from worktree for the new scanner.
- Added unit tests in `src/ios_usage.rs` covering:
  - custom shared SDK import
  - import-filter exclusion
  - untouched-hit classification

## 0.1.7

### Summary
- Reworked runtime to MR-focused flow.
- Added AST-first Kotlin API extraction for first-class categories in MR diff.
- Added iOS usage indexing/search module with parallel Swift AST scan.
- Added category-specific MR impact diagnostics and improved verbose reporting.
- Added architecture/process documentation in `docs/`.

### Changes
- New MR module and pipeline wiring:
  - `src/mr.rs`
- New git helper module split:
  - `src/git.rs`
- New iOS usage module:
  - `src/ios_usage.rs`
- First-class Kotlin API diff entities include:
  - constructor
  - enum/sealed
  - top-level
  - companion
  - typealias
- Parallel processing with progress bars for:
  - Kotlin AST expansion stage
  - iOS usage AST stage
- Verbose text output now includes:
  - grouped Kotlin API change summary
  - iOS usage index summary with matched files
- Added MR-specific diagnostic codes:
  - `mr_constructor_ios_impact`
  - `mr_enum_sealed_ios_impact`
  - `mr_top_level_ios_impact`
  - `mr_companion_ios_impact`
  - `mr_typealias_ios_impact`
  - `mr_member_ios_impact`
  - `mr_type_ios_impact`
  - fallback `mr_kotlin_api_ios_impact`

### Docs
- `docs/mr-process-report.md`
- `docs/mr-algorithm-vnext.md`
- `docs/ios-usage-search.md`
- `docs/mr-diagnostics-algorithm.md`

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
