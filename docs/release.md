# Релизы

Пакеты собираются **локально** и оттуда же публикуются. Ничего не уходит на GitHub до момента релиза: ни артефактов прогона, ни прав на запись у CI.

## Две команды

```sh
bash packaging/build-all.sh
nix shell nixpkgs#gh -c gh release create v0.1.0 --verify-tag --generate-notes dist/*
```

Первая собирает всё в `dist/` и кладёт туда же `SHA256SUMS`. Вторая создаёт релиз с уже существующего тега и прикладывает к нему файлы из `dist/` — имя тега обязано совпадать с версией в `Cargo.toml`.

Вариант с живой трансляцией в голосовой чат Telegram собирается той же командой с указанием фичи — движок (`libntgcalls.so`) кладётся рядом с бинарником, поэтому пакеты отличаются размером и самой функцией:

```sh
FEATURES=telegram-call bash packaging/build-all.sh
```

Для этого варианта в образе сборки нужен `zlib1g-dev` (он уже добавлен в `packaging/Dockerfile.linux`), а под NixOS локальная сборка требует `RUSTFLAGS="-L $(nix eval --raw nixpkgs#zlib.outPath)/lib"`.

Тег ставится отдельно, до публикации:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

## Что получается в `dist/`

| Артефакт | Платформа | Требования на машине |
|---|---|---|
| `ai-translator-<версия>-x86_64-linux.tar.gz` | Linux x86_64 | `libpipewire-0.3-0` ≥ 1.0, `libasound2`, glibc ≥ 2.34 |
| `ai-translator_<версия>_amd64.deb` | Ubuntu 24.04+, Debian 13+ | то же, объявлено в `Depends` |
| `ai-translator-<версия>-1.x86_64.rpm` | Fedora 40+, RHEL 10+ | то же, объявлено в `Requires` (`pipewire-libs >= 1.0`) |
| `ai-translator-<версия>-x86_64.AppImage` | любой Linux x86_64 | те же библиотеки **в системе**: внутрь образа они не упаковываются |
| `ai-translator-<версия>-aarch64-macos.tar.gz` и `-aarch64-macos.dmg` | macOS на Apple Silicon | ничего |
| `ai-translator-<версия>-x86_64-macos.tar.gz` и `-x86_64-macos.dmg` | macOS на Intel | ничего |
| `SHA256SUMS` | — | контрольные суммы всех файлов |

## Почему сборка идёт в контейнерах

**Linux.** Бинарник, собранный флейком, несёт интерпретатор `/nix/store/.../ld-linux-x86-64.so.2` и RUNPATH в nix-store: вне Nix он не стартует. Поэтому пакеты собирает образ `ai-translator-linux-builder:24.04` (Ubuntu 24.04 + Rust 1.95 + nfpm + `libclang` для bindgen): итоговый бинарник имеет интерпретатор `/lib64/ld-linux-x86-64.so.2`, пустой RUNPATH и требует glibc не новее 2.34.

**Почему PipeWire ≥ 1.0.** `libspa-sys` генерирует биндинги из системных заголовков PipeWire: на 0.3.48 (Ubuntu 22.04) сборка падает на `spa_meta_first`. Отсюда 24.04 и то же требование в зависимостях пакетов.

**macOS.** Образ `ai-translator-macos-builder:1.95` — osxcross с SDK 26.1 (см. [macos-cross-build.md](macos-cross-build.md)); собирает Mach-O под обе архитектуры. Подписи Developer ID нет, поэтому бинарник получает ad-hoc подпись от линкера (`-adhoc_codesign`): без неё arm64 не запускается вовсе.

**Файл `.dmg` без macOS.** `hdiutil` — системная утилита Apple, в Linux-контейнере её нет, но сам образ диска собирается: `mkfs.hfsplus` делает HFS+, содержимое копируется через loop-устройство, `dmg` из libdmg-hfsplus заворачивает образ в UDIF (zlib). Отсюда `--privileged` у этого шага. Проверено обратным разбором: `dmg2img` + монтирование — внутри `ai-translator`, `README.md`, `LICENSE-*`, `examples/`, бинарник `Mach-O 64-bit arm64`.

**AppImage без библиотек.** PipeWire и ALSA — клиентские библиотеки с загружаемыми модулями; упакованные внутрь образа, они расходятся с демоном на хосте. AppImage даёт «без установки», но не «без зависимостей».

## Рабочие каталоги

`dist/` — только файлы релиза. `build/` — промежуточные: распакованные каталоги, скачанный `appimagetool`, кэш реестра cargo, отдельный `target` на каждую платформу (иначе Linux-сборка на glibc 2.39 ломает сборку в контейнере с более старым glibc). Оба каталога в `.gitignore`, в git пакетов нет.

Контейнеры работают от root, поэтому в конце `build-all.sh` возвращает владельца файлов вызывающему пользователю.

## Подпись и Gatekeeper

- **macOS.** Ad-hoc подпись есть, Developer ID и нотаризации нет: Gatekeeper считает приложение неподписанным. Первый запуск — правый клик → «Открыть», либо `xattr -d com.apple.quarantine ./ai-translator`.
- **Linux.** Пакеты не подписаны (GPG-ключа нет), контрольные суммы — в `SHA256SUMS`.

## Проверено

18 октября 2026, версия `0.1.0`: собраны все восемь файлов — `x86_64-linux.tar.gz` 3.4 МБ, `.deb` 3.5 МБ, `.rpm` 3.5 МБ, `.AppImage` 4.0 МБ, `aarch64-macos.tar.gz` 2.9 МБ и `.dmg` 3.0 МБ, `x86_64-macos.tar.gz` 3.2 МБ и `.dmg` 3.2 МБ. `.deb` ставится в чистый Ubuntu 24.04 и запускается (`--help`); в arm64-бинарнике есть `LC_CODE_SIGNATURE`; содержимое `.dmg` читается обратно.
