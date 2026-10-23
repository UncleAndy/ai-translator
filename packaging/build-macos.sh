#!/usr/bin/env bash
# Builds the macOS release artefacts for one architecture: a tarball, plus a
# disk image where hdiutil exists. Both are unsigned in the Gatekeeper sense.
#
# AIT_TARGET picks the architecture; Apple Silicon is the only one shipped, since
# the media engine that carries the voice-chat sink exists for it alone.
# and names the artefacts. On macOS it runs with the command line tools; on Linux
# packaging/build-all.sh runs it inside the osxcross image, where STRIP has to be
# llvm-strip and there is no hdiutil, so the disk image is simply skipped.
#
# Environment: AIT_TARGET, DIST (default dist), WORK (default build), STRIP
# (default strip), CARGO_TARGET_DIR.
set -euo pipefail

cd "$(dirname "$0")/.."

NAME=ai-translator
# AIT_TARGET rather than TARGET, and no version from the environment: build
# images define generic names like TARGET, VERSION and ARCH for their own
# purposes, and a release must not pick those up.
AIT_TARGET="${AIT_TARGET:-$(uname -m | sed 's/arm64/aarch64/')-apple-darwin}"
case "$AIT_TARGET" in
  aarch64-apple-darwin) ARCH=aarch64 ;;
  x86_64-apple-darwin)
    echo "no package for Intel Macs: the media engine has no build for them," >&2
    echo "and a release without the voice-chat sink is not published" >&2
    exit 2
    ;;
  *) echo "unsupported target: $AIT_TARGET" >&2; exit 1 ;;
esac
VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
DIST="${DIST:-dist}"
WORK="${WORK:-build}"
STRIP="${STRIP:-strip}"

echo "== ai-translator $VERSION, $AIT_TARGET =="
test -n "$VERSION"

# The voice-chat sink streams through the media engine, and the engine exists for
# Apple Silicon only — which is why Intel Macs get no package. The feature is
# fixed here rather than taken from the environment: a build without the sink is
# not published.
FEATURES=telegram-call
ENGINE_DIR="$(bash packaging/fetch-ntgcalls.sh aarch64-apple-darwin)"
CALL_LIB="$ENGINE_DIR/lib/libntgcalls.dylib"
test -f "$CALL_LIB"

# The engine is a shared library beside the binary: a static copy would drag its
# own C++ runtime into the process, which is exactly what broke the Linux build.
# The rpath is the directory of the binary itself.
export NTGCALLS_DYLIB=1
export NTGCALLS_LIB_DIR="$(dirname "$CALL_LIB")"
# The library names its own dependencies (libz, libc++, the frameworks), so only
# the place it lies next to the binary has to be said.
call_flags="-C link-arg=-Wl,-rpath,@loader_path"

BUILD=(cargo build --release --locked --target "$AIT_TARGET" --features "$FEATURES")

# Every flag below belongs to the target, so that build scripts for the host are
# left alone.
flag="CARGO_TARGET_$(printf '%s' "$AIT_TARGET" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"

if [ -n "$call_flags" ]; then
  export "$flag"="${!flag:-} $call_flags"
fi

# Outside macOS the linker has to add the ad-hoc signature itself: Apple Silicon
# refuses to start a binary that has none at all.
if ! command -v codesign >/dev/null; then
  export "$flag"="${!flag:-} -C link-arg=-adhoc_codesign"

  # C code (ring, the audio backend) needs the osxcross compiler by name: cc-rs
  # reads CC_<target> with the dashes turned into underscores.
  triple_u=$(printf '%s' "$AIT_TARGET" | tr '-' '_')
  cc=$(command -v "$AIT_TARGET-cc" || true)
  if [ -z "$cc" ]; then
    cc=$(ls /usr/local/osxcross/target/bin/"$AIT_TARGET"*-cc 2>/dev/null | head -1 || true)
  fi
  cxx=$(command -v "$AIT_TARGET-c++" || true)
  if [ -z "$cxx" ]; then
    cxx=$(ls /usr/local/osxcross/target/bin/"$AIT_TARGET"*-c++ 2>/dev/null | head -1 || true)
  fi
  if [ -n "$cc" ] && [ -n "$cxx" ]; then
    export "CC_$triple_u=$cc" "CXX_$triple_u=$cxx"
  fi
fi

"${BUILD[@]}"

rm -rf "$DIST" "$WORK"
mkdir -p "$DIST" "$WORK"

BIN="${CARGO_TARGET_DIR:-target}/$AIT_TARGET/release/$NAME"
# dist/ holds nothing but the files that go into a release: a bare binary there
# would collide between the two architectures when the artefacts are unpacked
# side by side.
"$STRIP" -x "$BIN" -o "$WORK/$NAME"

# On macOS this is a second, formal signature; elsewhere it is skipped and the
# linker's ad-hoc one stands.
if command -v codesign >/dev/null; then
  codesign --force --sign - "$WORK/$NAME"
fi

STAGE="$WORK/$NAME-$VERSION"
mkdir -p "$STAGE"
install -m 0755 "$WORK/$NAME" "$STAGE/$NAME"
cp -r examples README.md LICENSE-MIT LICENSE-APACHE "$STAGE/"
# The same text a reader needs before the first start: the folder has to leave the
# disk image and lose the quarantine mark, or macOS will not load the engine.
install -m 0644 packaging/INSTALL-macOS.txt "$STAGE/"
install -m 0755 packaging/install-macOS.sh "$STAGE/"

# The engine travels next to the binary, where its rpath looks. Apple Silicon
# refuses to load an unsigned arm64 library, so it is signed the same way the
# binary is; the prebuilt library of the engine usually carries its own ad-hoc
# signature already.
if [ -n "$CALL_LIB" ]; then
  install -m 0755 "$CALL_LIB" "$STAGE/libntgcalls.dylib"
  if command -v codesign >/dev/null; then
    codesign --force --sign - "$STAGE/libntgcalls.dylib"
  fi
fi

tar -czf "$DIST/$NAME-$VERSION-$ARCH-macos.tar.gz" -C "$WORK" "$NAME-$VERSION"

if command -v hdiutil >/dev/null; then
  hdiutil create -quiet -volname "AI Translator" -srcfolder "$STAGE" -ov \
    -format UDZO "$DIST/$NAME-$VERSION-$ARCH.dmg"
else
  # Off macOS the image is made by packaging/build-all.sh in its own container
  # (hfsprogs + libdmg-hfsplus), because hdiutil is a macOS system tool.
  echo "hdiutil is missing: packaging/build-all.sh makes the disk image" >&2
fi

echo "== artefacts in $DIST =="
ls -la "$DIST"
