#!/usr/bin/env bash
set -euo pipefail

BIN_PATH="${1:?usage: build-deb.sh <bin-path> <version> [arch] [out-dir]}"
VERSION="${2:?usage: build-deb.sh <bin-path> <version> [arch] [out-dir]}"
ARCH="${3:-amd64}"
OUT_DIR="${4:-dist}"

PKG_ROOT="$(mktemp -d)"
trap 'rm -rf "$PKG_ROOT"' EXIT

mkdir -p "$PKG_ROOT/DEBIAN" "$PKG_ROOT/usr/bin" "$PKG_ROOT/usr/share/doc/kmpolice"
install -m 0755 "$BIN_PATH" "$PKG_ROOT/usr/bin/kmpolice"

if [ -f LICENSE ]; then
  cp LICENSE "$PKG_ROOT/usr/share/doc/kmpolice/copyright"
fi

cat >"$PKG_ROOT/DEBIAN/control" <<EOF
Package: kmpolice
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: kmpolice maintainers <opensource@kmpolice.dev>
Description: Static checker for Kotlin Multiplatform -> iOS Swift API impact
EOF

mkdir -p "$OUT_DIR"
dpkg-deb --build "$PKG_ROOT" "$OUT_DIR/kmpolice_${VERSION}_${ARCH}.deb"
