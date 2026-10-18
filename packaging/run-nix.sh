#!/usr/bin/env bash
# Build and run ai-translator on NixOS.
#
# The release packages are built on Ubuntu 24.04 for ordinary distributions: on
# NixOS such a binary starts only through `nix-ld`, and the media engine of the
# voice-chat sink — a static C++ WebRTC library inside the binary — does not
# survive that translation and takes the process down with a segmentation fault.
# A build made here links against the store and runs as it was built.
#
# Usage: packaging/run-nix.sh [arguments of ai-translator]
#   bash packaging/run-nix.sh run -c config.toml
#   bash packaging/run-nix.sh telegram-login -c config.toml
#
# Environment: FEATURES (default `telegram-call`; set it to empty for a build
# without the voice-chat sink).
set -euo pipefail

cd "$(dirname "$0")/.."

FEATURES="${FEATURES-telegram-call}"
BIN="${CARGO_TARGET_DIR:-$HOME/.cache/ai-translator-target}/release/ai-translator"

# The static engine is linked against zlib, which the Nix linker does not search
# by default.
ZLIB="$(nix eval --raw nixpkgs#zlib)"

BUILD=(cargo build --release)
if [ -n "$FEATURES" ]; then
  BUILD+=(--features "$FEATURES")
fi

echo "-- сборка: ${BUILD[*]}" >&2
RUSTFLAGS="-L $ZLIB/lib" nix develop --command "${BUILD[@]}"

# The run goes through the dev shell as well: PipeWire, ALSA and OpenSSL come
# from its environment.
exec nix develop --command "$BIN" "$@"
