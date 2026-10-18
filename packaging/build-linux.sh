#!/usr/bin/env bash
# Builds the Linux release artefacts: a tarball, a .deb, an .rpm and an AppImage.
#
# Run it on an ordinary distribution, not from the Nix flake: a binary built
# under Nix carries a /nix/store interpreter and does not start anywhere else.
# packaging/build-all.sh does exactly that, in an ubuntu 24.04 container,
# because libspa-sys generates its bindings from the system PipeWire headers and
# needs PipeWire 1.0. The binary wants libpipewire-0.3-0 >= 1.0 at run time
# (Ubuntu 24.04+, Debian 13+, Fedora 40+) and no glibc newer than 2.34.
#
# Environment: DIST (default dist), WORK (default build), CARGO_TARGET_DIR,
# FEATURES (default empty; `telegram-call` adds the live voice-chat sink; that
# build links the media engine statically, so nothing extra ships beside the
# binary and no new runtime dependency appears).
# Needs: cargo, strip, tar, gzip, curl, nfpm on PATH.
set -euo pipefail

cd "$(dirname "$0")/.."

NAME=ai-translator
DEB_ARCH=amd64
RPM_ARCH=x86_64
TRIPLE=x86_64-linux
DIST="${DIST:-dist}"
WORK="${WORK:-build}"
FEATURES="${FEATURES:-}"
# The version comes from Cargo.toml, not from the environment: build images
# define VERSION for their own purposes.
VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
BIN_DIR="${CARGO_TARGET_DIR:-target}/release"

echo "== ai-translator $VERSION =="
test -n "$VERSION"

BUILD_ARGS=(--release --locked)
if [ -n "$FEATURES" ]; then
  BUILD_ARGS+=(--features "$FEATURES")
fi

cargo build "${BUILD_ARGS[@]}"

rm -rf "$DIST" "$WORK"
mkdir -p "$DIST" "$WORK"

# dist/ holds nothing but the files that go into a release.
strip --strip-all "$BIN_DIR/$NAME" -o "$WORK/$NAME"

# The layout the tarball and the AppImage share.
STAGE="$WORK/$NAME-$VERSION"
mkdir -p "$STAGE"
install -m 0755 "$WORK/$NAME" "$STAGE/$NAME"
cp -r examples README.md LICENSE-MIT LICENSE-APACHE "$STAGE/"

tar -czf "$DIST/$NAME-$VERSION-$TRIPLE.tar.gz" -C "$WORK" "$NAME-$VERSION"

# The distribution packages. nfpm reads the architecture, the version and the
# binary location from the environment, so run it once per format. The binary
# lives in the work directory, so its path is substituted into a copy of the
# config: nfpm expands environment variables only in the version and the arch.
CONFIG="$WORK/nfpm.yaml"
sed "s|src: build/ai-translator|src: $WORK/ai-translator|" packaging/nfpm.yaml > "$CONFIG"
ARCH="$DEB_ARCH" VERSION="$VERSION" nfpm package \
  -f "$CONFIG" -p deb -t "$DIST/${NAME}_${VERSION}_${DEB_ARCH}.deb"
ARCH="$RPM_ARCH" VERSION="$VERSION" nfpm package \
  -f "$CONFIG" -p rpm -t "$DIST/$NAME-$VERSION-1.$RPM_ARCH.rpm"

# The AppImage. appimagetool is fetched rather than installed because no
# distribution packages it; --appimage-extract-and-run works without FUSE.
if [ ! -x "$WORK/appimagetool" ]; then
  curl -fsSL -o "$WORK/appimagetool" \
    "https://github.com/AppImage/appimagetool/releases/latest/download/appimagetool-x86_64.AppImage"
  chmod +x "$WORK/appimagetool"
fi

APPDIR="$WORK/AppDir"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/$NAME"
install -m 0755 "$WORK/$NAME" "$APPDIR/usr/bin/$NAME"
cp -r examples "$APPDIR/usr/share/$NAME/examples"
cp packaging/ai-translator.desktop "$APPDIR/$NAME.desktop"
cp assets/icon.png "$APPDIR/$NAME.png"
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
# The application takes its configuration on the command line, so the wrapper
# only has to find the binary inside the image.
exec "$APPDIR/usr/bin/ai-translator" "$@"
EOF
chmod +x "$APPDIR/AppRun"

ARCH=x86_64 "$WORK/appimagetool" --appimage-extract-and-run -n \
  "$APPDIR" "$DIST/$NAME-$VERSION-x86_64.AppImage"

echo "== artefacts in $DIST =="
ls -la "$DIST"
