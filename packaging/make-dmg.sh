#!/usr/bin/env bash
# Wraps a directory into an unsigned UDIF disk image. Runs inside the container
# built from packaging/Dockerfile.dmg, which needs --privileged for the loop
# mount. The image stays unsigned, exactly like the binary inside it.
#
# Environment: SRC (the directory to put in the image), OUT (the .dmg to write).
set -euo pipefail

SRC="${SRC:?SRC is required: the directory to wrap}"
OUT="${OUT:?OUT is required: the dmg to write}"
test -d "$SRC"

WORK=$(mktemp -d)
IMG="$WORK/disk.hfs"

# Room for the files plus a little slack, in MiB.
SIZE=$(( $(du -sk "$SRC" | cut -f1) / 1024 + 32 ))
dd if=/dev/zero of="$IMG" bs=1M count="$SIZE" status=none
mkfs.hfsplus -v "AI Translator" "$IMG" >/dev/null

mkdir -p "$WORK/mnt"
mount -o loop "$IMG" "$WORK/mnt"
cp -a "$SRC/." "$WORK/mnt/"
sync
umount "$WORK/mnt"

dmg "$IMG" "$OUT"
rm -rf "$WORK"

echo "== $OUT =="
ls -la "$OUT"
