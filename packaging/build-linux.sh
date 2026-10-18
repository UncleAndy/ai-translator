#!/usr/bin/env bash
# Builds the Linux release artefacts: a tarball, a .deb, an .rpm and an AppImage.
#
# Run it on an ordinary distribution, not from the Nix flake: a binary built
# under Nix carries a /nix/store interpreter and does not start anywhere else.
# The release workflow uses ubuntu-24.04, because libspa-sys generates its
# bindings from the system PipeWire headers and needs PipeWire 1.0. The binary
# that comes out wants libpipewire-0.3-0 >= 1.0 at run time (Ubuntu 24.04+,
# Debian 13+, Fedora 40+) and no glibc newer than 2.34.
#
# Environment: VERSION (default: the version in Cargo.toml).
# Needs: cargo, strip, tar, curl, nfpm on PATH.
set -euo pipefail

cd "$(dirname "$0")/.."

NAME=ai-translator
DEB_ARCH=amd64
RPM_ARCH=x86_64
TRIPLE=x86_64-linux
# The version comes from Cargo.toml, not from the environment: build images
# define VERSION for their own purposes.
VERSION="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
DIST=dist

echo "== ai-translator $VERSION =="
test -n "$VERSION"

cargo build --release --locked

rm -rf "$DIST" build
mkdir -p "$DIST" build

# Strip into DIST so the binary in target/ stays untouched.
strip --strip-all "target/release/$NAME" -o "$DIST/$NAME"

# The layout the tarball and the AppImage share.
STAGE="build/$NAME-$VERSION"
mkdir -p "$STAGE"
install -m 0755 "$DIST/$NAME" "$STAGE/$NAME"
cp -r examples README.md LICENSE-MIT LICENSE-APACHE "$STAGE/"

tar -czf "$DIST/$NAME-$VERSION-$TRIPLE.tar.gz" -C build "$(basename "$STAGE")"

# The distribution packages. nfpm reads the architecture and the version from
# the environment, so run it once per format.
ARCH="$DEB_ARCH" VERSION="$VERSION" nfpm package -f packaging/nfpm.yaml -p deb \
  -t "$DIST/${NAME}_${VERSION}_${DEB_ARCH}.deb"
ARCH="$RPM_ARCH" VERSION="$VERSION" nfpm package -f packaging/nfpm.yaml -p rpm \
  -t "$DIST/$NAME-$VERSION-1.$RPM_ARCH.rpm"

# The AppImage. appimagetool is fetched rather than installed because no
# distribution packages it; --appimage-extract-and-run works without FUSE.
if [ ! -x build/appimagetool ]; then
  curl -fsSL -o build/appimagetool \
    "https://github.com/AppImage/appimagetool/releases/latest/download/appimagetool-x86_64.AppImage"
  chmod +x build/appimagetool
fi

APPDIR=build/AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/$NAME"
install -m 0755 "$DIST/$NAME" "$APPDIR/usr/bin/$NAME"
cp -r examples "$APPDIR/usr/share/$NAME/examples"
cp packaging/ai-translator.desktop "$APPDIR/$NAME.desktop"
cp assets/icon.png "$APPDIR/$NAME.png"
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
# The application reads its configuration from the path given on the command
# line, so the wrapper only has to find the binary inside the image.
exec "$APPDIR/usr/bin/ai-translator" "$@"
EOF
chmod +x "$APPDIR/AppRun"

ARCH=x86_64 build/appimagetool --appimage-extract-and-run -n \
  "$APPDIR" "$DIST/$NAME-$VERSION-x86_64.AppImage"

echo "== artefacts =="
ls -la "$DIST"
