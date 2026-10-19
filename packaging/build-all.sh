#!/usr/bin/env bash
# Builds every release artefact locally, in containers, and leaves them in dist/
# with SHA256SUMS. Nothing is built by the Nix flake: its Linux binaries carry a
# /nix/store interpreter, and macOS needs an Apple toolchain.
#
#   bash packaging/build-all.sh
#
# The macOS disk images are made in a privileged container: they are filled
# through a loop device.
set -euo pipefail

cd "$(dirname "$0")/.."

ROOT=$(pwd)
PARENT=$(dirname "$ROOT")
VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)
LINUX_IMAGE=ai-translator-linux-builder:24.04
MACOS_IMAGE=ai-translator-macos-builder:1.95-call
DMG_IMAGE=ai-translator-dmg-builder:24.04

echo "== ai-translator $VERSION =="
test -n "$VERSION"

if ! docker image inspect "$LINUX_IMAGE" >/dev/null 2>&1; then
  echo "-- building $LINUX_IMAGE (once, a few minutes)"
  docker build -t "$LINUX_IMAGE" -f packaging/Dockerfile.linux packaging
fi
# The daemon has to see the tree: mounting a path it cannot resolve gives an
# empty directory and a confusing "No such file or directory" later.
if [ -z "$(docker run --rm -v "$ROOT:/probe" "$LINUX_IMAGE" ls -A /probe 2>/dev/null | head -1)" ]; then
  echo "docker cannot see $ROOT: the daemon and this shell do not share the tree" >&2
  exit 1
fi

# A killed run leaves its directories owned by root; take them back before the
# cleanup, otherwise the next run cannot remove them.
docker run --rm -v "$ROOT:/mnt" "$LINUX_IMAGE" \
  chown -R "$(id -u):$(id -g)" /mnt/dist /mnt/build >/dev/null 2>&1 || true

rm -rf dist build/linux build/macos
mkdir -p dist build/cargo-registry

if ! docker image inspect "$DMG_IMAGE" >/dev/null 2>&1; then
  echo "-- building $DMG_IMAGE (once, a couple of minutes)"
  docker build -t "$DMG_IMAGE" -f packaging/Dockerfile.dmg packaging
fi
if ! docker image inspect "$MACOS_IMAGE" >/dev/null 2>&1; then
  echo "-- building $MACOS_IMAGE (once, a few seconds)"
  docker build -t "$MACOS_IMAGE" -f packaging/Dockerfile.macos packaging
fi

echo "-- Linux: tar.gz, .deb, .rpm, AppImage (ubuntu 24.04)"
docker run --rm \
  -v "$PARENT:/work" \
  -v "$ROOT/build/cargo-registry:/root/.cargo/registry" \
  -w /work/ai-translator \
  -e CARGO_TARGET_DIR=/work/ai-translator/build/linux/target \
  -e DIST=/work/ai-translator/build/linux/dist \
  -e WORK=/work/ai-translator/build/linux/work \
  -e FEATURES="${FEATURES:-}" \
  "$LINUX_IMAGE" bash packaging/build-linux.sh

# The tree is mounted with its sibling transono: ai-translator depends on it
# by path.
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  arch=${target%%-apple-darwin}
  echo "-- macOS $arch: binary and tarball (osxcross)"
  docker run --rm \
    -v "$PARENT:/work" \
    -v "$ROOT/build/cargo-registry:/root/.cargo/registry" \
    -w /work/ai-translator \
    -e CARGO_TARGET_DIR="/work/ai-translator/build/macos/$arch/target" \
    -e AIT_TARGET="$target" \
    -e DIST="/work/ai-translator/build/macos/$arch/dist" \
    -e WORK="/work/ai-translator/build/macos/$arch/work" \
    -e STRIP=llvm-strip \
    -e RUSTUP_TOOLCHAIN=1.95.0 \
    -e FEATURES="${FEATURES:-}" \
    "$MACOS_IMAGE" bash packaging/build-macos.sh

  echo "-- macOS $arch: disk image (hfsprogs + libdmg-hfsplus)"
  docker run --rm --privileged \
    -v "$PARENT:/work" \
    -e SRC="/work/ai-translator/build/macos/$arch/work/ai-translator-$VERSION" \
    -e OUT="/work/ai-translator/build/macos/$arch/dist/ai-translator-$VERSION-$arch-macos.dmg" \
    "$DMG_IMAGE" bash /work/ai-translator/packaging/make-dmg.sh
done

cp build/linux/dist/* build/macos/*/dist/* dist/
(cd dist && sha256sum -- * > SHA256SUMS)

# The containers run as root, the files belong to the caller.
docker run --rm -v "$ROOT:/mnt" "$LINUX_IMAGE" \
  chown -R "$(id -u):$(id -g)" /mnt/dist /mnt/build

echo "== ready to publish: dist/ =="
ls -la dist
