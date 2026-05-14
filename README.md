# kmpolice

Static checker for Kotlin Multiplatform API changes and their impact on iOS Swift usage.

## Current CLI (MR-focused)

`kmpolice` currently runs MR-oriented analysis by default.

```bash
kmpolice --repo /path/to/repo --target main --format text --verbose-changes
```

Arguments:
- `--repo` path to git repository (default `.`)
- `--target` target branch/ref for merge-base (default `develop`)
- `--format` `text` or `json`
- `--config` optional TOML config
- `--verbose-changes` append human-readable Kotlin API changes + iOS usage index section (text mode)

## What MR mode does

1. Computes `merge-base(target, HEAD)`.
2. Builds Kotlin changed-path scope from:
- `merge-base..HEAD`
- staged/unstaged/untracked worktree changes
3. Filters Kotlin scope to `commonMain` and `iosMain` (`.kt` only).
4. Builds Kotlin API change set (AST-first) for changed files.
5. Scans iOS files for usage impact using indexed token prefilter + Swift AST identifiers.
6. Emits diagnostics, including category-specific MR impact codes:
- `mr_constructor_ios_impact`
- `mr_enum_sealed_ios_impact`
- `mr_top_level_ios_impact`
- `mr_companion_ios_impact`
- `mr_typealias_ios_impact`
- `mr_member_ios_impact`
- `mr_type_ios_impact`

Exit code:
- `0` when no diagnostics
- `1` when diagnostics exist
- `2` on runtime/tooling error

## Mock progress mode

For local UX/debugging of progress without a large repo:

```bash
kmpolice --mock-progress --mock-kotlin-files 6000 --mock-ios-files 6000 --format text
```

This runs parallel synthetic loaders and shows interactive progress bars.

## Install (Homebrew)

```bash
brew tap starzhs/kmpolice https://github.com/starzhs/homebrew-kmpolice
brew install kmpolice
```

Upgrade:

```bash
brew update
brew upgrade kmpolice
```

## Documentation

- MR process report: `docs/mr-process-report.md`
- MR vNext algorithm: `docs/mr-algorithm-vnext.md`
- iOS usage search logic: `docs/ios-usage-search.md`
- MR diagnostics algorithm: `docs/mr-diagnostics-algorithm.md`
