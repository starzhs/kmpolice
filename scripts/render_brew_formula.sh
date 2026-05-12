#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <repo-owner> <version> <sha256_arm64_macos> <sha256_x64_macos>" >&2
  echo "example: $0 myorg v0.1.1 abc... def..." >&2
  exit 1
fi

OWNER="$1"
VERSION="$2"
SHA_ARM="$3"
SHA_X64="$4"

cat <<RUBY
class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/${OWNER}/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/${OWNER}/kmpolice/releases/download/${VERSION}/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "${SHA_ARM}"
    else
      url "https://github.com/${OWNER}/kmpolice/releases/download/${VERSION}/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "${SHA_X64}"
    end
  end

  def install
    bin.install "kmpolice"
  end

  test do
    output = shell_output("#{bin}/kmpolice --help")
    assert_match "Usage: kmpolice", output
  end
end
RUBY
