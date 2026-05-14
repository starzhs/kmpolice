# iOS Usage Search Logic

This document describes how `kmpolice` finds iOS usages for Kotlin API changes in MR mode.

## Entry Point
- `find_ios_usages(api_changes, ios_files)`
- Code: `src/ios_usage.rs:27`

## Data Structures
- `IosUsageHit { file, symbol, kind, evidence }`
  - Code: `src/ios_usage.rs:12`
- `IosUsageReport { candidate_files, parsed_files, hits }`
  - Code: `src/ios_usage.rs:20`
- `SearchIndex { tokens }`
  - Code: `src/ios_usage.rs:102`

## Pipeline

1. Build token index from Kotlin API changes
- Function: `build_search_index`
- Code: `src/ios_usage.rs:107`
- Sources for tokens:
  - root type name extracted from symbol (`A.B` -> `A`)
  - whole symbol if identifier-like
  - member name parsed from change details (e.g. from backticks)

2. Fast candidate filter over all iOS files
- Candidate condition:
  - file contains shared import (`import shared` or `import shared.*`)
  - file contains at least one indexed token by word-boundary-like check
- Code:
  - `contains_shared_import`: `src/ios_usage.rs:141`
  - `contains_any_token`: `src/ios_usage.rs:148`
  - `contains_word`: `src/ios_usage.rs:152`

3. Parallel AST parse of candidate files
- Uses `rayon` with `par_iter`
- One Swift parser per worker/file
- Code: `src/ios_usage.rs:53`

4. Identifier extraction from Swift AST
- DFS over named nodes, collect identifier-like texts
- Code: `collect_identifiers` at `src/ios_usage.rs:179`

5. Per-change matching against file identifiers
- For each `ApiChange`, build expected tokens (`expected_tokens_for_change`)
- Match rule: all expected tokens must exist in file identifiers set
- Code:
  - `expected_tokens_for_change`: `src/ios_usage.rs:123`
  - match check: `src/ios_usage.rs:72`

6. Aggregate report
- `candidate_files`: number of files after prefilter
- `parsed_files`: number of files successfully parsed
- `hits`: matched `(file, symbol, kind, evidence)`
- Code: `src/ios_usage.rs:88`

## Progress Reporting
- Progress is rendered via `indicatif` to `stderr`.
- Stage name: `iOS usage AST`
- Includes dynamic "last file" message.
- Code:
  - progress bar setup: `src/ios_usage.rs:43`
  - update last file: `src/ios_usage.rs:81`
  - finish message: `src/ios_usage.rs:86`

## Nested-Type Behavior
Current behavior for nested symbols is root-driven in index:
- `root_type_name(symbol)` extracts the first uppercase identifier segment.
- This is used for coarse candidate narrowing.
- Code: `src/ios_usage.rs:199`

## Current Matching Model (Important)
- Matching is token-based and AST-identifier-based.
- It is designed for fast narrowing + practical hits.
- It is not yet a full semantic call/property/type resolver.

## Output Integration
- MR runner calls usage search after collecting `api_changes`.
- `MrResult` carries `ios_usage` report.
- Text verbose renderer prints usage summary and hits.
- Code references:
  - `src/mr.rs` (call site in `run_mr`)
  - `src/lib.rs` (verbose output wiring)
