# Kmpolice MR Process Report

## Scope
This report documents the MR analysis algorithm and maps each stage to concrete code entry points (`file.rs:line`).

Important status note:
- The current active CLI flow is simplified and runs path-based analysis from `--shared-lib` and `--ios`.
- Git/MR helper functions exist and are reusable, but full MR orchestration is not currently wired in `run()`.

Code references:
- `src/lib.rs:23` (`run`) 
- `src/cli.rs:14` (`Cli`)

## Data Model Used by the Pipeline
Core structures used by all analysis stages:
- `ProjectSnapshot`: `src/model.rs:86`
- `SourceFile`: `src/model.rs:79`
- `AnalysisResult`: `src/model.rs:71`
- `Diagnostic`: `src/model.rs:93`

Render/output:
- Text renderer: `src/report.rs:21`
- JSON renderer: `src/report.rs:68`

---

## 1) MR context resolution (target/head/base)
Goal:
- Resolve `target` ref.
- Resolve `HEAD` ref.
- Compute merge-base for true MR delta.

Implementations available:
- `merge_base(repo, target, head_ref)`: `src/git.rs:7`
- `resolve_ref(repo, git_ref)`: `src/git.rs:12`
- helper command wrapper: `src/git.rs:164`

Additional repo state checks:
- `is_worktree_dirty`: `src/git.rs:17`
- `has_unmerged_paths`: `src/git.rs:22`
- `is_head_detached`: `src/git.rs:27`
- `is_shallow_repository`: `src/git.rs:63`

Current orchestration status:
- These functions are present, but currently not called from `run()` (`src/lib.rs:23`).

---

## 2) Forming the MR change-set
Goal:
- Build unified delta = `merge-base..HEAD` + staged + unstaged + untracked.

Implementations available:
- Commit-range changed files:
  - `git_changed_files_between(repo, base_ref, head_ref)`: `src/git.rs:68`
- Worktree changed files (porcelain parser, includes rename normalization):
  - `git_changed_files_worktree(repo)`: `src/git.rs:82`

Underlying mechanics:
- Path normalization (`\\` -> `/`): `src/git.rs:160`
- Rename handling in worktree parsing (`old -> new`): `src/git.rs:93`

Noise-reduction primitives:
- Generated/build path filter:
  - `is_ignored_generated_path`: `src/source.rs:343`

Current orchestration status:
- Change-set collection helpers exist.
- Unified MR-level change-set builder is not currently exposed through active CLI flow.

---

## 3) Snapshot loading (base/head/worktree)
Goal:
- Load relevant Kotlin/Swift files from git snapshots or worktree.

Implementations available:
- Git snapshot loader:
  - `load_from_git`: `src/source.rs:32`
  - `load_from_git_scoped`: `src/source.rs:36`
- Worktree loader:
  - `load_from_worktree`: `src/source.rs:72`
  - `load_from_worktree_scoped`: `src/source.rs:76`
- Path loader (current active flow):
  - `load_from_paths`: `src/source.rs:16`

Scoped loading by changed path sets:
- `collect_git_files(..., changed_paths)`: `src/source.rs:206`
- `collect_worktree_git_list_files(..., changed_paths)`: `src/source.rs:290`

Git-backed file list/content helpers used by loaders:
- `git_ls_tree`: `src/git.rs:146`
- `git_ls_files_worktree`: `src/git.rs:122`
- `git_show`: `src/git.rs:156`

Progress currently present for large git snapshot loads:
- periodic elapsed-time logs in `collect_git_files`: `src/source.rs:248`, `src/source.rs:257`, `src/source.rs:277`

---

## 4) Kotlin API extraction (source of truth)
Goal:
- Parse Kotlin public contracts and derive API facts to compare with Swift usage.

Parser entrypoints:
- Unified parse entry:
  - `analyze(snapshot)`: `src/parser/mod.rs:8`
- Kotlin parser:
  - `parse_kotlin_files`: `src/parser/kotlin.rs:9`

Kotlin AST contract collection:
- contract traversal: `collect_contracts`: `src/parser/kotlin.rs:29`
- interface parse: `parse_interface`: `src/parser/kotlin.rs:57`
- class parse: `parse_class`: `src/parser/kotlin.rs:126`
- class members parse: `collect_class_members`: `src/parser/kotlin.rs:187`

Kotlin API index used for additional usage checks:
- `build_kotlin_api_index`: `src/analyzer.rs:376`

What this index currently includes:
- class constructors: `src/analyzer.rs:377`, regex `src/analyzer.rs:385`
- class properties: `src/analyzer.rs:378`, extraction from parsed contracts `src/analyzer.rs:499`
- enum/sealed cases: `src/analyzer.rs:379`, regexes `src/analyzer.rs:388`, `src/analyzer.rs:390`
- typealiases: `src/analyzer.rs:380`, regex `src/analyzer.rs:392`
- declared types: `src/analyzer.rs:381`, regex `src/analyzer.rs:394`
- top-level members: `src/analyzer.rs:383`, regex `src/analyzer.rs:398`
- companion members: `src/analyzer.rs:382`, regexes `src/analyzer.rs:400`, `src/analyzer.rs:403`

Parameter signature handling:
- Kotlin constructor parameter parsing:
  - `parse_kotlin_parameter_list`: `src/analyzer.rs:532`

Important implementation detail:
- This stage is hybrid: AST-based contract extraction + regex-based extraction for some additional Kotlin API surfaces.

---

## 5) Swift side extraction and usage matching
Goal:
- Parse Swift types/protocols and detect usage sites affected by Kotlin API changes.

Swift parser entrypoint:
- `parse_swift_files`: `src/parser/swift.rs:11`

Swift model enrichment:
- generic placeholder extraction:
  - `collect_swift_generic_placeholders`: `src/parser/swift.rs:72`

Main analysis entry:
- `compare_project`: `src/analyzer.rs:13`
- internal pipeline: `compare_analysis`: `src/analyzer.rs:28`

Usage-check stages currently executed:
- class method invocation compatibility:
  - `compare_class_method_invocations`: `src/analyzer.rs:174`
- additional Kotlin API checks dispatcher:
  - `compare_additional_kotlin_api_usages`: `src/analyzer.rs:147`
  - constructor calls: `check_constructor_calls`: `src/analyzer.rs:590`
  - properties: `check_property_usages`: `src/analyzer.rs:663`
  - enum/sealed switches: `check_enum_and_sealed_switches`: `src/analyzer.rs:835`
  - typealias/nested type usages: `check_typealias_and_nested_type_usages`: `src/analyzer.rs:999`
  - top-level usages: `check_top_level_usages`: `src/analyzer.rs:1069`
  - companion usages: `check_companion_member_usages`: `src/analyzer.rs:1116`

Label-aware argument diagnostics:
- expected/actual label hint formatting:
  - method mismatch hint: `src/analyzer.rs:356`
  - constructor mismatch hint: `src/analyzer.rs:626`

---

## 6) Diagnostic production, filtering, and diff-introduced comparison
Goal:
- Build diagnostics with evidence and locations.
- Optionally keep only introduced diagnostics (`base -> head`).

Core diagnostic assembly:
- many checks build `Diagnostic` directly, e.g.:
  - constructor mismatch diagnostic: `src/analyzer.rs:612`
  - property missing diagnostic: `src/analyzer.rs:702`
  - enum/sealed missing cases diagnostic: `src/analyzer.rs:972`

Global ignore filtering:
- applied in `compare_analysis`: `src/analyzer.rs:118`
- rule implementation: `Config::should_ignore_diagnostic`: `src/config.rs:99`

Base/head introduced-only filtering primitive:
- `introduced_diagnostics(base, head)`: `src/analyzer.rs:18`

Current runtime post-processing in path mode:
- downgrade `kotlin_type_usage_missing` severity without diff context:
  - `downgrade_unverified_type_usage`: `src/lib.rs:59`

---

## 7) Output and exit behavior
Goal:
- Render deterministic text/JSON output.
- Fail CI on found errors.

Implementations:
- render switch in runtime: `src/lib.rs:49`
- text renderer: `src/report.rs:21`
- json renderer: `src/report.rs:68`
- process exit code rule: `src/lib.rs:56`

---

## Current Gap Between Desired MR Mode and Active Runtime
Desired MR behavior:
1. build `base_ref` via merge-base
2. compute full MR change-set (`base..HEAD + worktree`)
3. load `base` and `head/worktree` snapshots
4. compare and report introduced impacts

Current active runtime path:
- `Cli` accepts only `--shared-lib`/`--ios` (+ mock mode): `src/cli.rs:14`
- `run()` calls only `load_from_paths` + `compare_project`: `src/lib.rs:43`, `src/lib.rs:44`

What is already reusable for MR restoration:
- git ref/change helpers (`src/git.rs`)
- scoped snapshot loaders (`src/source.rs:36`, `src/source.rs:76`)
- introduced diagnostics diff primitive (`src/analyzer.rs:18`)

---

## Recommended Minimal MR Wiring Plan (Implementation Order)
1. Reintroduce MR CLI inputs (`--repo`, `--target`, roots) in `src/cli.rs`.
2. In `run()` add MR branch that:
   - resolves refs and merge-base (`src/git.rs:7`, `src/git.rs:12`),
   - unions commit + worktree changed files (`src/git.rs:68`, `src/git.rs:82`),
   - loads base/head snapshots with scoped loaders (`src/source.rs:36`, `src/source.rs:76`),
   - runs `compare_project` for both and filters via `introduced_diagnostics` (`src/analyzer.rs:18`).
3. Keep existing renderers and exit semantics unchanged.

This gives predictable MR behavior while reusing current stable parser/analyzer code.
