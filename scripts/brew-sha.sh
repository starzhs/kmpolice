#!/usr/bin/env bash
set -euo pipefail

# usage:
#   ./scripts/brew-sha.sh starzhs/kmpolice v0.1.12
#   ./scripts/brew-sha.sh starzhs/kmpolice latest
REPO="${1:-starzhs/kmpolice}"
TAG="${2:-latest}"

if [[ "$TAG" == "latest" ]]; then
  API_URL="https://api.github.com/repos/${REPO}/releases/latest"
else
  API_URL="https://api.github.com/repos/${REPO}/releases/tags/${TAG}"
fi

RELEASE_JSON="$(curl -fsSL "$API_URL")"
BODY="$(printf '%s' "$RELEASE_JSON" | jq -r '.body // empty' | tr -d '\r')"
RESOLVED_TAG="$(printf '%s' "$RELEASE_JSON" | jq -r '.tag_name')"

extract_from_body() {
  local arch="$1"
  printf '%s\n' "$BODY" \
    | grep -Eio "kmpolice-${arch}-apple-darwin\\.tar\\.gz[[:space:]]*sha256:[[:space:]]*[0-9a-f]{64}" \
    | head -n1 \
    | sed -E 's/.*sha256:[[:space:]]*([0-9a-f]{64}).*/\1/I'
}

extract_from_assets_digest() {
  local arch="$1"
  printf '%s' "$RELEASE_JSON" \
    | jq -r --arg name "kmpolice-${arch}-apple-darwin.tar.gz" '
        .assets[]
        | select(.name == $name)
        | (.digest // "")
      ' \
    | sed -E 's/^sha256://'
}

ARM_SHA="$(extract_from_body aarch64 || true)"
X86_SHA="$(extract_from_body x86_64 || true)"

if [[ -z "$ARM_SHA" ]]; then
  ARM_SHA="$(extract_from_assets_digest aarch64 || true)"
fi
if [[ -z "$X86_SHA" ]]; then
  X86_SHA="$(extract_from_assets_digest x86_64 || true)"
fi

if [[ -z "${ARM_SHA}" || -z "${X86_SHA}" ]]; then
  echo "Failed to parse both SHA256 values from release body or assets digest" >&2
  echo "--- tag ---" >&2
  echo "$RESOLVED_TAG" >&2
  echo "--- release body ---" >&2
  echo "$BODY" >&2
  exit 1
fi

# Ready-to-paste Homebrew formula block
cat <<EOF
version "${RESOLVED_TAG#v}"

on_macos do
  if Hardware::CPU.arm?
    url "https://github.com/${REPO}/releases/download/${RESOLVED_TAG}/kmpolice-aarch64-apple-darwin.tar.gz"
    sha256 "${ARM_SHA}"
  else
    url "https://github.com/${REPO}/releases/download/${RESOLVED_TAG}/kmpolice-x86_64-apple-darwin.tar.gz"
    sha256 "${X86_SHA}"
  end
end
EOF
