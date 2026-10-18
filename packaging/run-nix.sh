#!/usr/bin/env bash
# Build and run ai-translator on NixOS.
#
# The release packages are built on Ubuntu 24.04 for ordinary distributions and
# do not survive nix-ld, so on NixOS the program is built and run here.
#
# Usage: packaging/run-nix.sh [arguments of ai-translator]
#   bash packaging/run-nix.sh run -c config.toml
#   bash packaging/run-nix.sh telegram-login -c config.toml
#
# Environment: FEATURES (default `telegram-call`; empty builds the version without
# the voice-chat sink).
set -euo pipefail

cd "$(dirname "$0")/.."

FEATURES="${FEATURES-telegram-call}"
BIN="${CARGO_TARGET_DIR:-$HOME/.cache/ai-translator-target}/release/ai-translator"

BUILD=(cargo build --release)
if [ -n "$FEATURES" ]; then
  BUILD+=(--features "$FEATURES")
fi

# The media engine is a shared library, and the binary finds it through an rpath
# to the directory it was built against. A static copy would drag its own C++
# runtime and allocator into the process and break everything else in it.
if [[ ",$FEATURES," == *",telegram-call,"* ]]; then
  ENGINE_DIR="$(bash packaging/fetch-ntgcalls.sh)"
  ENGINE_LIB="$ENGINE_DIR/lib"
  # The engine leaves zlib to the program that loads it, so the link names it.
  ZLIB="$(nix eval --raw nixpkgs#zlib)"
  echo "-- движок: $ENGINE_DIR" >&2
  NTGCALLS_DYLIB=1 \
    NTGCALLS_LIB_DIR="$ENGINE_LIB" \
    RUSTFLAGS="${RUSTFLAGS:-} -L $ZLIB/lib -C link-arg=-lz -C link-arg=-Wl,-rpath,$ENGINE_LIB" \
    nix develop --command "${BUILD[@]}"
else
  nix develop --command "${BUILD[@]}"
fi

# The run goes through the dev shell as well: PipeWire, ALSA, OpenSSL and the C++
# runtime of the engine come from its environment.
exec nix develop --command "$BIN" "$@"
