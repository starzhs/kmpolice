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

Check two directories directly (`paths` is best-effort without diff context, use at your own risk):

```bash
cargo run -- check paths --kotlin /path/to/shared/src --ios /path/to/ios/Sources
```

Check two refs inside a git repository:

```bash
cargo run -- check git --repo /path/to/repo --base-ref develop --head-ref HEAD --introduced-only
```

Check the current merge request branch against `develop` using `merge-base`:

```bash
cargo run -- check mr --repo /path/to/repo --target develop
```

Render JSON for CI:

```bash
cargo run -- --format json check mr --repo /path/to/repo --target develop
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
- git mode analyzes `base` and `head` independently and can report only newly introduced diagnostics.
- `paths` mode is best-effort without git diff context. Use it at your own risk.

## Homebrew Distribution

Repository contains brew packaging scaffolding:

- Formula template: `packaging/homebrew/Formula/kmpolice.rb`
- Release workflow: `.github/workflows/release.yml`
- Formula renderer helper: `scripts/render_brew_formula.sh`

Recommended publish flow:

1. Push git tag, e.g. `v0.1.0`.
2. Wait for GitHub Actions `release` workflow to publish archives and `.sha256` files.
3. Create/update tap repository (usually `homebrew-kmpolice`) and place final `Formula/kmpolice.rb`.
4. Fill URLs/SHA256 in formula from release artifacts.
5. Validate:
   - `brew install --formula ./Formula/kmpolice.rb`
   - `brew test kmpolice`
   - `brew audit --strict --formula ./Formula/kmpolice.rb`
