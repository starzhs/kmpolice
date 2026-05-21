# MR Diagnostics Algorithm

This document defines how diagnostics are produced in MR mode from Kotlin API changes and iOS usage evidence.

## Entry Points
- MR runner: `run_mr(...)`
  - Code: `src/mr.rs:36`
- CLI exit code decision:
  - Code: `src/lib.rs` (`exit_code_for_diagnostics`)

## Inputs
1. Kotlin-side diagnostics diff (base -> head/worktree)
- `introduced_diagnostics(base_diags, head_diags)`
- Code: `src/mr.rs:49`

2. Kotlin API change set
- Produced by Kotlin diff pipeline (`api_changes`) in MR run.

3. iOS usage report
- `find_ios_usages(api_changes, repo, ios_paths, shared_sdk_name, swift_changed_paths)`
- Code: `src/mr.rs`, implementation in `src/ios_usage.rs`

## Diagnostic Assembly Pipeline

1. Start from introduced diagnostics
- Base diagnostics are compared with head/worktree diagnostics.
- Only newly introduced items are kept.

2. Add iOS impact diagnostics
- Function: `build_ios_impact_diagnostics(api_changes, usage, config)`
- Code: `src/mr.rs:65`
- For each usage hit:
  - find corresponding API change by key `(kind, symbol)`
  - deduplicate by `(kind, symbol, file)`
  - emit a diagnostic with:
    - code by change kind
    - message: Kotlin change + iOS file
    - hint: change details + match evidence + Swift file touched/untouched status
    - evidence: `mr_mode:diff_aware`, `kotlin_change_detected`, `ios_usage_index_hit`

Member-specific matching behavior:
- `member` hits are matched with a strict type-aware pass first:
  - receiver type binding from Swift AST
  - local inheritance/conformance graph
  - member call extraction
- token fallback is used when receiver type cannot be resolved.

3. Final diagnostics list
- `introduced_diagnostics` + `mr_*_ios_impact` additions.

## Diagnostic Code Mapping
Mapping function:
- `impact_code_for_kind(kind)`
- Code: `src/mr.rs:115`

Mapping table:
- `constructor` -> `mr_constructor_ios_impact`
- `enum_sealed` -> `mr_enum_sealed_ios_impact`
- `top_level` -> `mr_top_level_ios_impact`
- `companion` -> `mr_companion_ios_impact`
- `typealias` -> `mr_typealias_ios_impact`
- `member` -> `mr_member_ios_impact`
- `type` -> `mr_type_ios_impact`
- fallback -> `mr_kotlin_api_ios_impact`

Severity source:
- `config.severity_for(code)`
- Code usage: `src/mr.rs:89`

## Output
1. Main diagnostics report
- text/json renderer is selected in `lib.rs`
- text/json generation:
  - `src/lib.rs:32`

2. Verbose sections (text mode)
- Kotlin API changes:
  - `render_verbose_changes(...)` at `src/mr.rs:402`
- iOS usage index summary:
  - `render_ios_usage_report(...)` at `src/mr.rs:426`
- Wiring in CLI output:
  - `src/lib.rs:36`
  - `src/lib.rs:38`

## Program Termination Rule
- Exit code `0` when final diagnostics contain no `error` severity.
- Exit code `1` when final diagnostics contain at least one `error`.
- Code: `src/lib.rs` (`exit_code_for_diagnostics`)

## Practical Meaning
- If a Kotlin API change exists and matching iOS usage is found, the tool emits category-specific MR impact diagnostics.
- This makes MR output actionable even when generic contract diffs are not enough.
