# Релизы

Пакеты собираются **локально** и оттуда же публикуются. Ничего не уходит на GitHub до момента релиза: ни артефактов прогона, ни прав на запись у CI.

## Две команды

```sh
bash packaging/build-all.sh
nix shell nixpkgs#gh -c gh release create v0.2.2 --verify-tag --generate-notes dist/*
```

Первая собирает всё в `dist/` и кладёт туда же `SHA256SUMS`. Вторая создаёт релиз с уже существующего тега и прикладывает к нему файлы из `dist/` — имя тега обязано совпадать с версией в `Cargo.toml`.

Живая трансляция в голосовой чат Telegram входит в **каждый** пакет: фича `telegram-call` зашита в сборочные скрипты (`build-linux.sh`, `build-macos.sh`, `build-windows.ps1`), переключателя нет. Движок кладётся рядом с бинарником — `libntgcalls.so` в Linux, `libntgcalls.dylib` на Apple Silicon, `ntgcalls.dll` на Windows — и находится по rpath или как обычная DLL.

Пакеты собираются для трёх платформ: Linux x86_64, macOS **Apple Silicon** и Windows x86_64. Для Intel-маков пакета нет: движка под `x86_64-apple-darwin` не существует, а пакет без трансляции не публикуется, поэтому `build-macos.sh` отказывается собирать эту цель (код возврата 2).

Для этого варианта в образе сборки нужен `zlib1g-dev` (он уже добавлен в `packaging/Dockerfile.linux`). Под NixOS сборка и запуск идут через `packaging/run-nix.sh`, отдельных переменных не нужно.

Тег ставится отдельно, до публикации:

```sh
git tag -a v0.2.2 -m "ai-translator 0.2.1" && git push origin v0.2.2
```

## Что получается в `dist/`

| Артефакт | Платформа | Требования на машине |
|---|---|---|
| `ai-translator-<версия>-x86_64-linux.tar.gz` | Linux x86_64 | `libpipewire-0.3-0` ≥ 1.0, `libasound2`, glibc ≥ 2.34 |
| `ai-translator_<версия>_amd64.deb` | Ubuntu 24.04+, Debian 13+ | то же, объявлено в `Depends` |
| `ai-translator-<версия>-1.x86_64.rpm` | Fedora 40+, RHEL 10+ | то же, объявлено в `Requires` (`pipewire-libs >= 1.0`) |
| `ai-translator-<версия>-x86_64.AppImage` | любой Linux x86_64 | те же библиотеки **в системе**: внутрь образа они не упаковываются |
| `ai-translator-<версия>-aarch64-macos.tar.gz` и `-aarch64-macos.dmg` | macOS на Apple Silicon | ничего; внутри `libntgcalls.dylib`, трансляция в голосовой чат есть |
| `ai-translator-<версия>-x86_64-windows.zip` | Windows 10/11 x86_64 | Visual C++ 2015-2022; внутри `ai-translator.exe` и `ntgcalls.dll` |
| `ai-translator-<версия>-x86_64-windows.msi` | Windows 10/11 x86_64 | то же; ставит в `Program Files`, делает ярлык, обновляется поверх прежней версии |
| `SHA256SUMS` | — | контрольные суммы всех файлов |

## Windows собирается на Windows

`build-all.sh` покрывает Linux и macOS: обе сборки идут в контейнерах, потому что бинарник нужен для чужой системы. Для Windows это невозможно и не нужно — движок звонка там лежит в формате MSVC, поэтому сборка идёт родным тулчейном:

```powershell
pwsh -File packaging/build-windows.ps1
```

Скрипт берёт версию из `Cargo.toml`, гоняет тесты (если не указан `-SkipTests`), скачивает движок через `packaging/fetch-ntgcalls.ps1` (там закреплён SHA256 архива `ntgcalls.windows-x86_64-shared_libs.zip`), собирает `--release --features telegram-call` и кладёт в `dist/` портативный ZIP с `ai-translator.exe`, `ntgcalls.dll`, лицензиями и `INSTALL-windows.txt`, рядом — файл `.sha256`. Варианта без движка нет: фича зашита в скрипт.

Требуется MSVC-тулчейн (Visual Studio Build Tools с компонентом C++) и PowerShell 7. CI-задание `windows-latest` делает то же самое на каждом push — и это единственное место, где линковка с движком проверяется автоматически.

Установщик собирается из того же подготовленного каталога отдельным шагом:

```powershell
pwsh -File packaging/build-msi.ps1 -Stage build\windows\work\ai-translator-0.2.4
```

WiX работает только на Windows (он сам об этом предупреждает при запуске на другой ОС), поэтому MSI на Linux-хосте не собрать. Версия WiX закреплена на `5.*`: начиная с 6 требуется оплата Open Source Maintenance Fee. `build-windows.ps1` вызывает этот шаг сам; `-NoMsi` его пропускает. Установщик ставит файлы в `Program Files\ai-translator`, добавляет ярлык в меню «Пуск», регистрируется в «Программах и компонентах» и обновляется поверх предыдущей версии (`MajorUpgrade`).

**Если Windows-машины нет.** Обычный прогон пакет никуда не выгружает. Но ручной запуск (`workflow_dispatch`) оставляет его артефактом прогона на 90 дней — это способ забрать файл с Linux-хоста:

```sh
gh workflow run ci.yml                      # или Actions → ci → Run workflow
gh run watch                                # дождаться зелёного windows
gh run download --name ai-translator-windows-x86_64 --dir ~/win-package
cd ~/win-package && sha256sum -c *.sha256
```

Артефакт живёт только в прогоне: в репозиторий, релиз и теги он не попадает и через 90 дней исчезает.

## Почему сборка идёт в контейнерах

**Linux.** Бинарник, собранный флейком, несёт интерпретатор `/nix/store/.../ld-linux-x86-64.so.2` и RUNPATH в nix-store: вне Nix он не стартует. Поэтому пакеты собирает образ `ai-translator-linux-builder:24.04` (Ubuntu 24.04 + Rust 1.95 + nfpm + `libclang` для bindgen): итоговый бинарник имеет интерпретатор `/lib64/ld-linux-x86-64.so.2`, пустой RUNPATH и требует glibc не новее 2.34.

**Почему PipeWire ≥ 1.0.** `libspa-sys` генерирует биндинги из системных заголовков PipeWire: на 0.3.48 (Ubuntu 22.04) сборка падает на `spa_meta_first`. Отсюда 24.04 и то же требование в зависимостях пакетов.

**macOS.** Образ `ai-translator-macos-builder:1.95-call` — osxcross с SDK 26.1 (см. [macos-cross-build.md](macos-cross-build.md)); собирает Mach-O под обе архитектуры. Подписи Developer ID нет, поэтому бинарник получает ad-hoc подпись от линкера (`-adhoc_codesign`): без неё arm64 не запускается вовсе.

**Файл `.dmg` без macOS.** `hdiutil` — системная утилита Apple, в Linux-контейнере её нет, но сам образ диска собирается: `mkfs.hfsplus` делает HFS+, содержимое копируется через loop-устройство, `dmg` из libdmg-hfsplus заворачивает образ в UDIF (zlib). Отсюда `--privileged` у этого шага. Проверено обратным разбором: `dmg2img` + монтирование — внутри `ai-translator`, `README.md`, `LICENSE-*`, `examples/`, бинарник `Mach-O 64-bit arm64`.

**AppImage без библиотек.** PipeWire и ALSA — клиентские библиотеки с загружаемыми модулями; упакованные внутрь образа, они расходятся с демоном на хосте. AppImage даёт «без установки», но не «без зависимостей».

## Рабочие каталоги

`dist/` — только файлы релиза. `build/` — промежуточные: распакованные каталоги, скачанный `appimagetool`, кэш реестра cargo, отдельный `target` на каждую платформу (иначе Linux-сборка на glibc 2.39 ломает сборку в контейнере с более старым glibc). Оба каталога в `.gitignore`, в git пакетов нет.

Контейнеры работают от root, поэтому в конце `build-all.sh` возвращает владельца файлов вызывающему пользователю.

## Подпись и Gatekeeper

- **macOS.** Ad-hoc подпись есть, Developer ID и нотаризации нет: Gatekeeper считает приложение неподписанным. Запускать прямо с тома DMG нельзя: том только для чтения, а карантин снять надо **со всего каталога**, включая `libntgcalls.dylib` — библиотека, не прошедшая проверку, отвергается с `library load disallowed by system policy`, и бинарник падает ещё до `--version`. Порядок:

  ```sh
  cp -R "/Volumes/AI Translator/." ~/ai-translator
  xattr -dr com.apple.quarantine ~/ai-translator
  ~/ai-translator/ai-translator --version
  ```

  Все эти шаги делает скрипт `install-macOS.sh`, который лежит в tar.gz и в DMG: копирует программу с движком в `~/Applications/ai-translator`, снимает метку со всего каталога, ставит ссылку в `~/.local/bin`, если тот в `PATH`, и печатает версию, которую удалось запустить. Достаточно:

  ```sh
  ./install-macOS.sh
  ```

  Быстрее всего — вообще без DMG: `curl` не ставит карантинную метку (её ставит браузер), поэтому распакованный архив запускается сразу:

  ```sh
  mkdir -p ~/ai-translator
  curl -fsSL https://github.com/UncleAndy/ai-translator/releases/download/v0.2.2/ai-translator-0.2.2-aarch64-macos.tar.gz \
    | tar xz -C ~/ai-translator --strip-components=1
  ~/ai-translator/install-macOS.sh
  ```

  Проверено на живом Mac (Apple Silicon, macOS 26): после копирования и `xattr -dr` CLI запускается. Скрипт прогнан отдельно на подставном бинарнике: копирование, ссылка и проверка версии работают, отсутствие `xattr` не валит установку.
- **Linux.** Пакеты не подписаны (GPG-ключа нет), контрольные суммы — в `SHA256SUMS`.

## Проверено

18 октября 2026, версия `0.1.0`: собраны все восемь файлов — `x86_64-linux.tar.gz` 3.4 МБ, `.deb` 3.5 МБ, `.rpm` 3.5 МБ, `.AppImage` 4.0 МБ, `aarch64-macos.tar.gz` 2.9 МБ и `.dmg` 3.0 МБ, `x86_64-macos.tar.gz` 3.2 МБ и `.dmg` 3.2 МБ. `.deb` ставится в чистый Ubuntu 24.04 и запускается (`--help`); в arm64-бинарнике есть `LC_CODE_SIGNATURE`; содержимое `.dmg` читается обратно.
