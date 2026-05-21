# Kmpolice MR Process Report

This document reflects the current MR runtime flow.

## Runtime Entry

- CLI/runtime entry: `src/lib.rs`
- MR runner: `run_mr(...)` in `src/mr.rs`

The active flow is MR-oriented:

1. Resolve `merge-base(target, HEAD)`.
2. Build changed-path sets from:
   - `merge-base..HEAD`
   - current worktree (staged/unstaged/untracked)
3. Scope Kotlin changes to `*.kt` under `commonMain` and `iosMain`.
4. Load scoped snapshots:
   - base snapshot from git ref (parallel file reads)
   - head snapshot from worktree (parallel file reads)
5. Compute Kotlin API changes (`api_changes`) from scoped files.
6. Scan Swift usage for these changes.
7. Emit introduced diagnostics + MR impact diagnostics.

## Kotlin API Change Extraction

Primary categories detected in diffed Kotlin files:

- `type`
- `member`
- `constructor`
- `enum_sealed`
- `top_level`
- `companion`
- `typealias`

Implementation:

- `diff_kotlin_api_changes(...)` in `src/mr.rs`
- first-class symbol extraction via Kotlin AST in `src/mr.rs`

## Swift Usage Search

Implementation:

- `find_ios_usages(...)` in `src/ios_usage.rs`

Current search model:

1. Shared-SDK import filtering.
2. Token prefilter.
3. Swift AST parsing on candidates.
4. Matching against `api_changes`.

For `member` changes:

- strict type-aware matching on Swift AST:
  - receiver bindings (`name -> type`)
  - local inheritance/conformance graph
  - member call resolution (`receiver.member(...)`)
- fallback remains for unresolved type context.

## Diagnostics Emitted

MR impact codes:

- `mr_constructor_ios_impact`
- `mr_enum_sealed_ios_impact`
- `mr_top_level_ios_impact`
- `mr_companion_ios_impact`
- `mr_typealias_ios_impact`
- `mr_member_ios_impact`
- `mr_type_ios_impact`
- `mr_kotlin_api_ios_impact` (fallback)

Diagnostics include evidence and touched/untouched Swift status.

## Output and Exit Codes

- Output formats: `text`, `json`
- Exit code:
  - `0` only `info`/`warning` diagnostics (or no diagnostics)
  - `1` at least one `error` diagnostic
  - `2` runtime/tooling error
