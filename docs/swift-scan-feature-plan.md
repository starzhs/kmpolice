# Swift Scan Architecture

This file documents the currently implemented Swift scan architecture for MR mode.

## Goals

- Fast candidate narrowing on large iOS codebases.
- Deterministic matching against Kotlin diff changes.
- Actionable diagnostics for impacted Swift call sites.

## Implemented Stages

1. **Enumerate Swift paths**
   - Input: scoped path set from repository.
2. **Import filter**
   - Keep only files importing shared SDK module.
3. **Token prefilter**
   - Fast text filter from Kotlin change index.
4. **Parallel Swift AST parse**
   - Parse only candidate files.
5. **Usage matching**
   - Match each `ApiChange` against parsed Swift structures.

## Matching Modes

### Generic matching

- Token-based strict matching (`all expected tokens present`).

### Member matching

- Type-aware strict matching first:
  - receiver variable/property/parameter type bindings
  - local inheritance/conformance graph
  - member call extraction
- Fallback to token-only when receiver type cannot be resolved.

## Progress Reporting

Progress is stage-based with `indicatif`:

- `Swift enumerate`
- `Swift import filter`
- `Swift token filter`
- `Swift AST parse`
- `Swift usage match`

Each stage reports counters and last processed file.

## Diagnostic Integration

Swift hits are converted into MR impact diagnostics (`mr_*_ios_impact`) with:

- change detail
- evidence
- Swift file touched/untouched status
