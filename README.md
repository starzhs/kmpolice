# kmpolice

Static checker for Kotlin Multiplatform interface changes against iOS Swift contracts.

## What it catches

- missing Swift protocols for Kotlin interfaces
- missing or stale members on Swift protocols
- parameter count, name, and type mismatches
- return type mismatches
- property mutability mismatches
- broken Swift conformances after contract changes

## Usage

Check merge-request impact against target branch:

```bash
kmpolice mr --repo /path/to/repo --target main --kotlin-root Fruitties/shared/src/commonMain/kotlin --ios-root Fruitties/iosApp/iosApp
```

`mr` behavior:
- compares from `merge-base(target, HEAD)` to current `HEAD`
- always includes local worktree changes (staged, unstaged, untracked)
- Kotlin is scoped by changed paths; iOS is scanned fully inside `ios_root`

Check explicit refs inside a git repository:

```bash
kmpolice git --repo /path/to/repo --base-ref main --head-ref HEAD --introduced-only --kotlin-root Fruitties/shared/src/commonMain/kotlin --ios-root Fruitties/iosApp/iosApp
```

`git` mode is useful when you need strict ref-to-ref comparison (for CI gates, release branches, or custom base/head pairs) instead of MR semantics.

Check two directories directly:

```bash
kmpolice paths --kotlin /path/to/shared/src --ios /path/to/ios/Sources
```

`paths` behavior:
- prefers git-indexed files when the path is inside a git repository
- falls back to filesystem walk when outside git
- skips generated/build noise paths

Render JSON (CI-friendly):

```bash
kmpolice --format json mr --repo /path/to/repo --target main
```

## Config

Pass `--config /path/to/kmpolice.toml` (any TOML filename is fine).

Supported config keys:

- `kotlin_roots`
- `ios_roots`
- `include`
- `exclude`
- `mappings`
- `naming`
- `ignore`
- `severity`

See [`kmp-interface-checker.toml.example`](./kmp-interface-checker.toml.example).

## Notes

- Kotlin is treated as the source of truth.
- v1 focuses on Kotlin interfaces, Swift protocols, and Swift implementations.
- `git` mode analyzes exact refs and can report only newly introduced diagnostics.
- `mr` mode is branch-vs-target with worktree changes included.
- `paths` mode has no diff context and is best-effort.

## Install (Homebrew)

Tap install:

```bash
brew tap starzhs/kmpolice https://github.com/starzhs/homebrew-kmpolice
brew install kmpolice
```

Upgrade:

```bash
brew update
brew upgrade kmpolice
```

## Homebrew publishing notes

Repository contains brew packaging scaffolding:
- Formula template: `packaging/homebrew/Formula/kmpolice.rb`
- Release workflow: `.github/workflows/release.yml`
- Formula renderer helper: `scripts/render_brew_formula.sh`

Recommended publish flow:

1. Push git tag, e.g. `v0.1.4`.
2. Wait for GitHub Actions `release` workflow to publish archives and `.sha256` files.
3. Create/update tap repository (usually `homebrew-kmpolice`) and place final `Formula/kmpolice.rb`.
4. Fill URLs/SHA256 in formula from release artifacts.
5. Validate:
   - `brew install --formula ./Formula/kmpolice.rb`
   - `brew test kmpolice`
   - `brew audit --strict --formula ./Formula/kmpolice.rb`
