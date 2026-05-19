# iOS Usage Search Logic

This document describes how `kmpolice` finds Swift usages for Kotlin API changes in MR mode.

## Entry Point

- `find_ios_usages(api_changes, repo, ios_paths, shared_sdk_name, swift_changed_paths)`
- Code: `src/ios_usage.rs`

## Pipeline

1. Build search index from Kotlin API changes.
2. Enumerate Swift paths.
3. Read files in parallel.
4. Keep files that import shared SDK (`import <module>`, `@testable import <module>`, etc.).
5. Apply token prefilter (fast textual narrowing).
6. Parse candidate files with Swift tree-sitter in parallel.
7. Match parsed Swift usage against each Kotlin API change.
8. Build `IosUsageReport` with touched/untouched stats.

## Data Produced per Parsed Swift File

- `identifiers`: identifier set from AST.
- `bindings`: variable/property/parameter bindings (`name -> type`).
- `inheritance`: local type inheritance/conformance graph.
- `member_calls`: member call sites as `(receiver, member)`.

## Matching Strategy

### Non-`member` changes

- Uses strict token set matching (`all expected tokens must be present`).

### `member` changes

Type-aware matching is applied first:

1. Resolve changed owner type from Kotlin symbol (root type segment).
2. Resolve changed member name from `ApiChange.details`.
3. Inspect Swift member calls.
4. Resolve receiver type from bindings.
5. Match only if:
   - receiver type equals owner type, or
   - receiver type is a subtype/conforming type of owner (via local inheritance graph).

If receiver type cannot be resolved from local AST context, fallback to token-based matching is allowed.

## Import Filter Rules

The shared SDK import check supports attribute-prefixed import forms, including:

- `import SharedSdk`
- `@testable import SharedSdk`
- `@preconcurrency import SharedSdk`

## Output

- `IosUsageHit { file, symbol, kind, evidence, already_touched }`
- `IosUsageReport { swift_files_total, candidate_files, parsed_files, touched_hits, untouched_hits, hits }`

Evidence examples:

- `type_aware_call:child:ChildType -> trace`
- `member_only_fallback:trace`

## Progress Stages

Rendered with `indicatif`:

- `Swift enumerate`
- `Swift import filter`
- `Swift token filter`
- `Swift AST parse`
- `Swift usage match`

Each stage reports processed/total and the last processed file.
