# Brew Release Guide

## 1. Build and publish release assets

Push a version tag:

```bash
git tag v0.1.1
git push origin v0.1.1
```

Workflow `.github/workflows/release.yml` publishes:

- `kmpolice-aarch64-apple-darwin.tar.gz`
- `kmpolice-x86_64-apple-darwin.tar.gz`
- checksum files (`.sha256`)

## 2. Prepare Homebrew tap repository

Create a separate repository (recommended):

- `<ORG_OR_USER>/homebrew-kmpolice`

Inside it place:

- `Formula/kmpolice.rb`

Use template from this repo:

- `packaging/homebrew/Formula/kmpolice.rb`

## 3. Fill formula values

Replace placeholders:

- `<ORG_OR_USER>`
- `<SHA256_ARM64_MACOS>`
- `<SHA256_X64_MACOS>`

Or render with helper:

```bash
./scripts/render_brew_formula.sh <ORG_OR_USER> v0.1.1 <SHA_ARM> <SHA_X64>
```

## 4. Validate formula locally

```bash
brew install --formula ./Formula/kmpolice.rb
brew test kmpolice
brew audit --strict --formula ./Formula/kmpolice.rb
```

## 5. Install from tap

```bash
brew tap <ORG_OR_USER>/kmpolice https://github.com/<ORG_OR_USER>/homebrew-kmpolice
brew install kmpolice
```
