#!/bin/bash
# Install ai-translator on macOS with as few steps as possible.
#
# Run it from the unpacked folder or from the mounted disk image:
#
#     ./install-macOS.sh
#
# It copies the program with the engine beside it, clears the quarantine mark that
# macOS puts on downloaded files (without that the engine is refused by the system
# policy check), links the command into ~/.local/bin when that directory is in
# PATH, and finally asks the binary for its version — a smoke check that catches a
# broken copy right away.
#
# Environment:
#   AIT_PREFIX   where to install, default ~/Applications/ai-translator
#   AIT_BIN_DIR  where to put the command link, default ~/.local/bin
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
prefix="${AIT_PREFIX:-$HOME/Applications/ai-translator}"
bin_dir="${AIT_BIN_DIR:-$HOME/.local/bin}"

if [ ! -x "$here/ai-translator" ]; then
  echo "рядом со скриптом нет ai-translator: запустите его из распакованного каталога" >&2
  exit 2
fi

# A read-only image cannot lose the quarantine mark, so the copy is not optional.
mkdir -p "$prefix"
cp -R "$here/." "$prefix/"

if command -v xattr >/dev/null; then
  xattr -dr com.apple.quarantine "$prefix" 2>/dev/null || true
  echo "-- карантинная метка снята с $prefix"
else
  echo "-- xattr недоступен: карантин не трогаю"
fi

# The command link is a convenience, not a requirement: a directory that is not in
# PATH would silently do nothing, so the choice is made by what PATH says.
case ":$PATH:" in
  *":$bin_dir:"*)
    mkdir -p "$bin_dir"
    ln -sf "$prefix/ai-translator" "$bin_dir/ai-translator"
    echo "-- команда доступна как ai-translator ($bin_dir)"
    ;;
esac

if version=$("$prefix/ai-translator" --version 2>&1); then
  echo "-- проверка запуска: $version"
else
  echo "-- программа не запустилась: $version" >&2
  echo "   проверьте, что libntgcalls.dylib лежит рядом с бинарником" >&2
  exit 1
fi

echo
echo "Готово. Дальше:"
echo "  $prefix/ai-translator check -c <конфигурация.toml>"
echo "  $prefix/ai-translator run   -c <конфигурация.toml>"
echo "Микрофон спрашивается при первом run."
