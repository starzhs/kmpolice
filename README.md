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
- `--shared-sdk-name` Swift import module name for shared KMP SDK (CLI override)
- `--verbose-changes` append human-readable Kotlin API changes + iOS usage index section (text mode)

## What MR mode does

1. Computes `merge-base(target, HEAD)`.
2. Builds Kotlin changed-path scope from:
- `merge-base..HEAD`
- staged/unstaged/untracked worktree changes
3. Filters Kotlin scope to `commonMain` and `iosMain` (`.kt` only).
4. Builds Kotlin API change set (AST-first) for changed files.
5. Scans iOS files using a cascade:
- enumerate Swift files
- `import <shared-sdk-name>` filter
- token prefilter from Kotlin changes
- Swift AST parse only for candidates
- usage match against changed Kotlin symbols
6. Emits diagnostics, including category-specific MR impact codes:
- `mr_constructor_ios_impact`
- `mr_enum_sealed_ios_impact`
- `mr_top_level_ios_impact`
- `mr_companion_ios_impact`
- `mr_typealias_ios_impact`
- `mr_member_ios_impact`
- `mr_type_ios_impact`

## Kotlin API Change Categories Detected

`kmpolice` currently detects these Kotlin API change categories in `commonMain` and `iosMain`, and then searches their impact in Swift:

- `type`: a public type (`class`/`interface`) was added or removed.
- `member`: a public type member (method/property) was added, removed, or changed.
- `constructor`: a public class constructor signature changed (parameter set, names, overloads).
- `enum_sealed`: enum/sealed case set changed.
- `top_level`: top-level `fun` / `val` / `var` changed.
- `companion`: `companion object` API changed.
- `typealias`: `typealias` was added, removed, or changed.

## Member Matching in Swift (Type-aware)

For `member` changes, Swift call-site matching is strict and type-aware:

- It extracts receiver bindings (`name -> type`) from Swift AST.
- It builds local Swift inheritance/conformance relations from AST.
- It matches member calls only when receiver type is:
  - the owner type itself, or
  - a subtype/conforming type of the owner.

Example covered by strict matching:
- Kotlin: `Tracer.trace()` changed on parent/protocol.
- Swift: `child.trace()` where `child` is typed as a conforming/subtype.

Fallback behavior:
- If receiver type cannot be resolved from local AST context, token-based fallback is still used to avoid dropping valid hits in unresolved cross-file/module scenarios.

Exit code:
- `0` when diagnostics contain only `info`/`warning` (or no diagnostics)
- `1` when at least one `error` diagnostic exists
- `2` on runtime/tooling error

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

## Install (APT, Linux)

APT repository is published on GitHub Pages:
- `https://starzhs.github.io/kmpolice`

Signed repository (recommended, if `kmpolice-archive-keyring.asc` is present):

```bash
curl -fsSL https://starzhs.github.io/kmpolice/kmpolice-archive-keyring.asc \
  | gpg --dearmor \
  | sudo tee /usr/share/keyrings/kmpolice.gpg > /dev/null

echo "deb [signed-by=/usr/share/keyrings/kmpolice.gpg] https://starzhs.github.io/kmpolice stable main" \
  | sudo tee /etc/apt/sources.list.d/kmpolice.list > /dev/null

sudo apt-get update
sudo apt-get install -y kmpolice
```

Unsigned fallback (testing only):

```bash
echo "deb [trusted=yes] https://starzhs.github.io/kmpolice stable main" \
  | sudo tee /etc/apt/sources.list.d/kmpolice.list > /dev/null
sudo apt-get update
sudo apt-get install -y kmpolice
```

## Documentation

- MR process report: `docs/mr-process-report.md`
- MR vNext algorithm: `docs/mr-algorithm-vnext.md`
- iOS usage search logic: `docs/ios-usage-search.md`
- MR diagnostics algorithm: `docs/mr-diagnostics-algorithm.md`
