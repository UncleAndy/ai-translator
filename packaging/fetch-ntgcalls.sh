#!/usr/bin/env bash
# Fetch the prebuilt ntgcalls media engine for a target and unpack it under
# build/ntgcalls/<target>.
#
# The engine is the C++ WebRTC layer the voice-chat sink streams through. It is
# native code, so a build that enables the `telegram-call` feature needs the
# library for its own target: the releases exist for Linux x86_64, Linux arm64
# and macOS arm64, and no other target gets the feature.
#
# Usage: packaging/fetch-ntgcalls.sh [target]
#   target defaults to the host triple; pass `x86_64-unknown-linux-gnu`,
#   `aarch64-unknown-linux-gnu` or `aarch64-apple-darwin` to prepare a cross build.
#
# Prints the directory holding libntgcalls (header and library) on success.

set -euo pipefail

VERSION=${NTGCALLS_VERSION:-3.0.4}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

TARGET=${1:-}
if [ -z "$TARGET" ]; then
  TARGET=$(rustc -vV | sed -n 's/^host: //p')
fi

case "$TARGET" in
  x86_64-unknown-linux-gnu) ASSET=ntgcalls.linux-x86_64-shared_libs.zip ;;
  aarch64-unknown-linux-gnu) ASSET=ntgcalls.linux-arm64-shared_libs.zip ;;
  aarch64-apple-darwin) ASSET=ntgcalls.macos-arm64-shared_libs.zip ;;
  *)
    echo "no prebuilt ntgcalls for $TARGET: the voice-chat sink is not built for it" >&2
    exit 2
    ;;
esac

OUT="$ROOT/build/ntgcalls/$TARGET"
if [ -f "$OUT/lib/libntgcalls.so" ] || [ -f "$OUT/lib/libntgcalls.dylib" ]; then
  echo "$OUT"
  exit 0
fi

mkdir -p "$OUT"
ZIP=$(mktemp)
trap 'rm -f "$ZIP"' EXIT

URL="https://github.com/pytgcalls/ntgcalls/releases/download/v$VERSION/$ASSET"
echo "-- fetching $ASSET ($VERSION)" >&2
curl -fsSL -o "$ZIP" "$URL"
unzip -o -q "$ZIP" -d "$OUT"

if [ ! -d "$OUT/lib" ]; then
  echo "the archive has no lib/: $ASSET" >&2
  exit 1
fi

echo "$OUT"
