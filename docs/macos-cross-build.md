# Кросс-сборка под macOS из NixOS/Linux

Замерено 18.10.2026 на NixOS 26.05 (x86_64-linux). Задача: убедиться, что проект
компилируется под darwin, **не имея Mac и не дожидаясь CI**.

## Итог

| Путь | Результат |
|---|---|
| `pkgsCross.<arch>-apple-darwin` из nixpkgs | **не работает** — сборка падает на пакете Apple `Csu-88` |
| `cargo check --target aarch64-apple-darwin` без тулчейна | **не работает** — у rustc из nixpkgs нет std под darwin, а `-sys` крейты всё равно требуют darwin-компилятор |
| `cross build --target aarch64-apple-darwin` (Docker-инструмент `cross`) | **не работает** — цели Apple/Darwin в `cross` не поддерживаются |
| **Docker-образ `joseluisq/rust-linux-darwin-builder` (osxcross внутри)** | **работает** — собраны настоящие Mach-O бинарники под arm64 и x86_64 |

## 1. Почему nixpkgs не годится

Даже с `config.allowUnsupportedSystem = true` (без него падение раньше, на `cctools`)
сборка не проходит:

```
building 'Csu-88.drv'...
  > build flags: ... CC=clang ...
  > clang -r ... -o crt1.v1.o ...
  > bash: line 1: clang: command not found
  > make: *** [Makefile:72: crt1.v1.o] Error 127
```

Причина: пакеты Apple в nixpkgs рассчитаны на **нативную** darwin-машину, где `clang`
есть в PATH; под Linux их никто не собирает. По цепочке падают `apple-sdk-*` и
`arm64-apple-darwin-clang-wrapper`, SDK в хранилище так и не появляется.
`osxcross` в nixpkgs тоже нет.

## 2. Рабочий путь: готовый Docker-образ

`joseluisq/rust-linux-darwin-builder` = Debian + osxcross + macOS SDK + Rust,
с уже установленными целями `x86_64-apple-darwin` и `aarch64-apple-darwin`.

```bash
docker pull joseluisq/rust-linux-darwin-builder:1.89.0

# Разовая операция: в образе Rust 1.89, а дереву нужно 1.95 (несколько крейтов
# объявляют rust-version = 1.95 и на 1.89 падают с E0658). Ставим нужный
# тулчейн и сохраняем образ, чтобы не повторять это каждый раз:
docker run -d --name b joseluisq/rust-linux-darwin-builder:1.89.0 sh -c \
  'rustup toolchain install 1.95.0 --profile minimal && rustup target add --toolchain 1.95.0 aarch64-apple-darwin'
docker commit b rust-darwin-builder:1.95
```

Дальше проверка — одна команда. Внутри контейнера должен лежать **только исходник**
(без `target/`, `.git/` и `.cargo/` — файл `.cargo/config.toml` от флейка содержит
Linux-пути и всё сломает):

```bash
docker run --rm -v <каталог, видимый докеру>:/root/src -w /root/src/ai-translator \
  -e CC_aarch64_apple_darwin=aarch64-apple-darwin22.4-cc \
  -e CXX_aarch64_apple_darwin=aarch64-apple-darwin22.4-c++ \
  -e CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=aarch64-apple-darwin22.4-clang \
  -e AR_aarch64_apple_darwin=aarch64-apple-darwin22.4-ar \
  rust-darwin-builder:1.95 sh -c 'cargo +1.95.0 build --target aarch64-apple-darwin'
```

Для `x86_64-apple-darwin` — та же команда с заменой суффиксов на `x86_64` и
`CC_x86_64_apple_darwin=o64-clang`. Пакетов для Intel-маков проект больше не
выпускает: движка под эту архитектуру нет, а сборка без трансляции в релиз не идёт.

**Результат замерен:** оба варианта дали настоящие бинарники, включая `transono`
как зависимость:

```
ai-translator: Mach-O 64-bit arm64 executable   (38 МБ)
ai-translator: Mach-O 64-bit x86_64 executable  (45 МБ)
```

Проверять надо именно артефакт (`file target/aarch64-apple-darwin/debug/ai-translator`),
а не строку `Finished`: первые четыре байта Mach-O — `cf fa ed fe`.

**Образы не удалять** — они дорогая часть: `rust-darwin-builder:1.95` (наш, с нужным
тулчейном) и `joseluisq/rust-linux-darwin-builder:1.89.0` (базовый). Слои общие,
поэтому второй почти ничего не добавляет.

## 3. Грабли (каждая стоила прогона)

1. **Rust в образе старее, чем нужно дереву.** `--ignore-rust-version` не лечит: если
   зависимость реально использует новые возможности, будет `E0658`. Нужен свой тулчейн
   внутри контейнера (см. выше).
2. **Демон докера видит хостовые пути, а не пути песочницы.** Смонтированный каталог
   должен быть доступен демону, иначе `cp: cannot stat ...: Permission denied`.
3. **Не монтировать каталог с `.cargo/config.toml` от флейка** — там Linux-пути и
   `PKG_CONFIG_PATH`.
4. **Для `aarch64` линковщик надо переопределять.** В конфиге образа прописан
   `arm64e-apple-darwin…` — это другой вариант архитектуры; используем
   `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER`.
5. **`-sys` крейтам нужен C-компилятор с суффиксом цели** (`CC_aarch64_apple_darwin`,
   `CXX_…`), а не общий `CC`; типичная жертва — `ring` в TLS-стеке.
6. **Артефакты, созданные root-ом.** Если запускать контейнер от root, `target/`
   окажется root-овым; владельца возвращать контейнером, а не `sudo` на хосте:
   `docker run --rm -v <каталог>:/x <образ> chown -R <uid>:<gid> /x`.
7. **Долгая сборка не выживает в песочнице.** Запускать как
   `docker run -d --name …`, прогресс смотреть `docker logs --tail`, а не держать
   клиент на переднем плане (его убивают примерно через две минуты).
8. **Linux-only зависимости надо закрыть гейтом заранее** — иначе сборка под darwin
   падает на графе зависимостей и ничего не говорит о своём коде. В `transono` это
   сделано: `pipewire`/`libspa-sys` и фичи `cpal` уехали в
   `[target.'cfg(target_os = "linux")'.dependencies]`, а модуль PipeWire, его
   реэкспорт, вариант ошибки `Pipewire` и докссылки на него — под `#[cfg(target_os = "linux")]`.
   Бинарник `transonovirt` (он сам управляет устройствами PipeWire) для darwin
   непереносим по природе — собирается только библиотека (`--lib`).

## 4. Чего эта проверка не даёт

Рантайма: микрофона, запроса разрешения TCC, задержек, работы страницы в браузере.
И подписи — неподписанный бинарник на Mac встретит Gatekeeper. Это проверяется только
на живой машине (или прогоном тестов на macOS-раннере CI, что закрывает сборку и тесты,
но не рантайм).

## 5. Если собирать osxcross самому

SDK: `https://github.com/joseluisq/macosx-sdks` — упакованные SDK (`MacOSX26.1.sdk.tar.xz`,
84 МиБ, sha256 опубликован в `macosx_sdks.json`). Тулчейн: `tpoechtrager/osxcross`.
Смысл имеет только если готовый образ почему-то не подходит.

## 6. Дешёвый вариант: харнесс подмены cfg

Быстрая проверка (~20 секунд, без докера) **только своего macOS-кода**: копия крейта,
в которой `target_os = "macos"` и `target_os = "linux"` меняются местами, затем
`cargo check --lib`. Тогда на Linux компилируются наши macOS-ветки. Харнесс проверен на
ложное срабатывание: с намеренно подсунутой ошибкой он падает, без неё проходит.
Darwin-крейты (`cpal`/CoreAudio, `objc2-*`, `ring`) он не проверяет — для них нужен
путь из раздела 2.
