# MR Algorithm vNext (Diff-aware, Incremental AST)

## Goal
Analyze only MR-relevant Kotlin changes and map impact to Swift usage with minimal noise and predictable performance.

## Scope rules
- Source of truth: Kotlin changes only.
- Kotlin files considered: `*.kt` under `commonMain` and `iosMain`.
- `androidMain` is ignored.
- Visibility: only public API (`public` and default-public where applicable).

## Pipeline

1. Build `MrChangeSet`
- Collect changed files from:
  - `merge-base(target, HEAD)..HEAD`
  - staged + unstaged + untracked
- Normalize paths and merge duplicates/renames.
- Filter generated/build noise and non-Kotlin files.

2. Build before/after file pairs
- For each changed Kotlin file, load:
  - `before` from merge-base snapshot
  - `after` from worktree (with staged/unstaged state)
- Parse both into AST.

3. Produce `ApiChangeSeed`
- Diff only public symbols in file pairs.
- Symbol kinds:
  - type (class/interface/object/enum/sealed/typealias)
  - constructor
  - method
  - property
  - enum/sealed cases
  - top-level fun/val
  - companion API
- Capture before/after signatures (including parameter names/labels).

4. Build expansion queue
- For each seed change enqueue dependencies:
  - owner type
  - supertypes
  - nested types
  - companion members
  - typealias targets
  - top-level facade container (`*Kt`)
  - referenced Kotlin types in signatures

5. Collect relevant files in one pass
- One indexed pass over Kotlin roots (`commonMain`, `iosMain`) to map symbols -> file candidates.
- Parse only relevant files, not the full repo.
- Parallelize file parsing where safe.

6. Expand until stable
- Re-run dependency resolution while new unresolved symbols appear.
- Stop at fixpoint.

7. Finalize `KotlinApiDelta`
- Emit final normalized changes with factual payload:
  - symbol
  - change kind
  - before
  - after
  - evidence

8. Verbose human-readable report
- Print discovered Kotlin API changes in readable form.
- Example:
  - `type shared.MainViewModel changed`
  - `method addItemToCart changed: before (fruittie,name), after (fruittie)`

## Current implementation status
- Wired as default runtime path: MR-only execution.
- Implemented:
  - MR changed Kotlin file collection (`commonMain`/`iosMain` filter)
  - before/head snapshot compare
  - introduced diagnostics filtering
  - initial AST-based per-file contract/member diff summary in verbose mode
- Next steps:
  - explicit `constructor` symbol-kind diff
  - enum/sealed/top-level/companion/typealias as first-class AST diff entities
  - dependency expansion queue + one-pass symbol indexing
