#!/usr/bin/env bash
# Run the whole test suite of the build with the voice-chat sink.
#
# The media engine is a shared library, so both the linker and the test binaries
# need to know where it lies: this script fetches it, points the link at it and
# puts it on the loader path.
#
# Usage: packaging/test-call.sh [extra cargo test arguments]
set -euo pipefail

cd "$(dirname "$0")/.."

ENGINE_DIR="$(bash packaging/fetch-ntgcalls.sh)"
ENGINE_LIB="$ENGINE_DIR/lib"
ZLIB="$(nix eval --raw nixpkgs#zlib)"

NTGCALLS_DYLIB=1 \
  NTGCALLS_LIB_DIR="$ENGINE_LIB" \
  RUSTFLAGS="-L $ZLIB/lib -C link-arg=-lz -C link-arg=-Wl,-rpath,$ENGINE_LIB" \
  nix develop --command bash -c \
  "LD_LIBRARY_PATH=\"$ENGINE_LIB:\$LD_LIBRARY_PATH\" cargo test --features telegram-call $*"
