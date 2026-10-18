#!/usr/bin/env bash
# Builds the macOS release artefacts: a tarball and a disk image, both unsigned.
#
# TARGET picks the architecture (aarch64-apple-darwin or x86_64-apple-darwin)
# and ARCH names the artefacts. Runs on macOS with the command line tools; the
# disk image is skipped where hdiutil is missing, so the tarball half can be
# exercised on Linux too.
#
# Environment: VERSION (default: the version in Cargo.toml).
set -euo pipefail

cd "$(dirname "$0")/.."

NAME=ai-translator
# AIT_TARGET rather than TARGET, and no version from the environment: build
# images define generic names like TARGET, VERSION and ARCH for their own
# purposes, and a release must not pick those up.
AIT_TARGET="${AIT_TARGET:-$(uname -m | sed 's/arm64/aarch64/')-apple-darwin}"
case "$AIT_TARGET" in
  aarch64-apple-darwin) ARCH=aarch64 ;;
  x86_64-apple-darwin)  ARCH=x86_64 ;;
  *) echo "unsupported target: $AIT_TARGET" >&2; exit 1 ;;
esac
VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
DIST=dist

echo "== ai-translator $VERSION, $AIT_TARGET =="
test -n "$VERSION"

cargo build --release --locked --target "$AIT_TARGET"

rm -rf "$DIST" build
mkdir -p "$DIST" build

BIN="target/$AIT_TARGET/release/$NAME"
strip -x "$BIN" -o "$DIST/$NAME"

# arm64 refuses to start a binary without a signature, and stripping drops the
# ad-hoc one the linker added, so sign again. This is not a Developer ID and
# not notarisation: Gatekeeper still sees an unsigned application.
if command -v codesign >/dev/null; then
  codesign --force --sign - "$DIST/$NAME"
fi

STAGE="build/$NAME-$VERSION"
mkdir -p "$STAGE"
install -m 0755 "$DIST/$NAME" "$STAGE/$NAME"
cp -r examples README.md LICENSE-MIT LICENSE-APACHE "$STAGE/"

tar -czf "$DIST/$NAME-$VERSION-$ARCH-macos.tar.gz" -C build "$(basename "$STAGE")"

if command -v hdiutil >/dev/null; then
  hdiutil create -quiet -volname "AI Translator" -srcfolder "$STAGE" -ov \
    -format UDZO "$DIST/$NAME-$VERSION-$ARCH.dmg"
else
  echo "hdiutil is missing: the disk image was skipped" >&2
fi

echo "== artefacts =="
ls -la "$DIST"
