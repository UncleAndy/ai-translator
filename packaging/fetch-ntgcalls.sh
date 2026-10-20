#!/usr/bin/env bash
# Fetch the prebuilt ntgcalls media engine for a target and unpack it under
# build/ntgcalls/<target>.
#
# The engine is the C++ WebRTC layer the voice-chat sink streams through. It is
# loaded as a shared library, not linked into the binary: a static copy brings its
# own C++ runtime and allocator into the process and breaks everything else that
# runs there, so the build links against the shared one even though the engine is
# only used when the voice-chat sink is enabled.
#
# Prebuilt releases exist for Linux x86_64, Linux arm64 and macOS arm64; no other
# target gets the feature.
#
# Usage: packaging/fetch-ntgcalls.sh [target]
#   target defaults to the host triple; pass `x86_64-unknown-linux-gnu`,
#   `aarch64-unknown-linux-gnu` or `aarch64-apple-darwin` for a cross build.
#
# Prints the directory holding the library on success.

set -euo pipefail

VERSION=${NTGCALLS_VERSION:-3.0.4}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

TARGET=${1:-}
if [ -z "$TARGET" ]; then
  # The host triple without asking cargo, so the script works outside the dev
  # shell too.
  case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) TARGET=x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) TARGET=aarch64-unknown-linux-gnu ;;
    Darwin/arm64) TARGET=aarch64-apple-darwin ;;
    *)
      echo "no prebuilt ntgcalls for $(uname -s)/$(uname -m)" >&2
      exit 2
      ;;
  esac
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

# The engine is a library that ends up in the user's process, so the archive is
# pinned: the sha256 below is the file this version was reviewed with. A mismatch
# is a reason to stop, not to warn — a substituted archive is arbitrary code in the
# translator.
case "$VERSION:$ASSET" in
  3.0.4:ntgcalls.linux-x86_64-shared_libs.zip)
    EXPECTED=e539eec7a570a00ab51035750b522be3a11cb83462745a15633994fc7cd45659 ;;
  3.0.4:ntgcalls.linux-arm64-shared_libs.zip)
    EXPECTED=abe48c916a4a838c4547f4c18b0bcaeba70c1ca099d6e1ac1cd87c7f6374d5f4 ;;
  3.0.4:ntgcalls.macos-arm64-shared_libs.zip)
    EXPECTED=e4afdbc61543a9fe319548eb545e133b9b985d54cd8245d954aaa1706854426a ;;
  *)
    echo "no pinned checksum for $ASSET $VERSION: refusing an unverified engine" >&2
    exit 3
    ;;
esac

# sha256sum is not part of macOS; shasum is.
hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

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

ACTUAL=$(hash_of "$ZIP")
if [ "$ACTUAL" != "$EXPECTED" ]; then
  echo "checksum mismatch for $ASSET ($VERSION)" >&2
  echo "  expected $EXPECTED" >&2
  echo "  got      $ACTUAL" >&2
  echo "the archive is not the release this version was reviewed with: refusing it" >&2
  exit 4
fi
echo "-- checksum matches the pinned one" >&2

unzip -o -q "$ZIP" -d "$OUT"

if [ ! -d "$OUT/lib" ]; then
  echo "the archive has no lib/: $ASSET" >&2
  exit 1
fi

echo "$OUT"
