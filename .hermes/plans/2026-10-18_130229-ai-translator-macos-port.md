# План порта ai-translator на macOS

> **Для Hermes:** выполнять по задачам, после каждой — прогон защиты от регрессий (§0.1) и коммит. Плановый документ, ничего не менять до подтверждения.

**Цель:** собрать и запустить `ai-translator` (и библиотеку `transono`, на которой он стоит) на macOS, не ослабив ни на шаг сборку под Linux.

**Архитектура порта:** правки аддитивные и только платформенные — Linux-only зависимости уходят в `[target.'cfg(target_os = "linux")'.dependencies]`, Linux-only модули получают `#[cfg(target_os = "linux")]`, а для macOS добавляются ветки, использующие уже существующий в `transono` бэкенд `cpal` (на macOS это CoreAudio). Ни один файл с логикой перевода, шины, приёмников или страницы не трогается.

**Стек:** Rust (edition 2024, cargo 1.95), `cpal 0.18` (CoreAudio на macOS, PipeWire на Linux), `tokio`, `axum`, `rustls/ring`; Nix — только для Linux, на macOS достаточно Xcode Command Line Tools.

---

## 0. Жёсткие условия

1. **Сборка под Linux обязана оставаться полностью рабочей** после каждой задачи: тот же набор зависимостей, тот же бэкенд звука (cpal с фичей `pipewire`), тот же `nix develop`, те же тесты. Никаких «временно отключим, потом вернём».
2. Все платформенные различия выражаются **только** через `[target.'cfg(...)'.dependencies]` и `#[cfg(target_os = ...)]`. Условная компиляция в рантайме (`if cfg!(...)`) для выбора бэкенда не применяется — она тащит обе зависимости в сборку.
3. Комментарии в коде `transono` — по-английски (стиль библиотеки), сообщения об ошибках и тесты — по-русски (стиль проекта).
4. Секреты — только через `*_env`. README `ai-translator` — только по-английски.
5. Правки в `transono` — это правки **чужого** репозитория: они оформляются отдельным коммитом в `~/projects/RUST/transono` (см. открытый вопрос 1 в §5).

## 0.1. Защита от регрессий: прогон после каждой задачи

Выполняется здесь, на Linux, до всякой проверки на Mac. Все команды — с целевым каталогом вне дерева проекта (иначе cargo упирается в `target/`, принадлежащий `nobody`).

```bash
# transono — сборка и тесты. fmt и clippy у transono СЕЙЧАС не чисты (см. §0.2),
# поэтому они не входят в защиту от регрессий: гейт по ним был бы красным с первого дня.
cd ~/projects/RUST/transono
CARGO_TARGET_DIR=$HOME/.cache/ai-translator-target nix develop --command bash -c \
  'cargo build --bins && cargo test 2>&1 | grep "test result"'

# ai-translator
cd ~/projects/RUST/ai-translator
nix develop --command bash -c 'cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | grep "test result"'
nix shell nixpkgs#nodejs -c node web/test/page.mjs | tail -2
```

**Ожидание (числа сняты 18 октября 2026, задача 0):**

| Проект | База | Что обязано сохраниться |
|---|---|---|
| `transono` | **117 + 6 = 123 теста, 0 провалов**; сборка бинарников проходит | сборка и тесты; число тестов не уменьшилось. Под Linux бинарники собираются как раньше; в кросс-проверку под darwin они не входят — там проверяется только библиотека |
| `ai-translator` | **109 модульных + 3 интеграционных + 3 doctest** (104 + 5 у команды `devices`), `fmt` и `clippy -D warnings` чисто | всё перечисленное |
| Страница | 28 проверок, «ВСЕ ПРОВЕРКИ ПРОЙДЕНЫ» | то же |

## 0.2. Что в `transono` уже не чисто (не наша вина и не наша задача)

Замерено 18 октября 2026, до всяких правок:

- `cargo test` — **123 теста, 0 провалов** (зелёное; тесты не требуют звукового железа — прошли на стенде, где звука нет вовсе);
- `cargo fmt --check` — **495 расхождений** в `src/` и `examples/`: дерево просто не прогонялось через `rustfmt`;
- `cargo clippy` — **падает сам по себе, без `-D warnings`**: 5 ошибок в deny-by-default линтах группы correctness (`this loop never actually loops`, `mutable borrow from immutable input(s)`, `an async construct yields a type which is itself awaitable`), плюс около 200 предупреждений.

**Следствия для порта:**

1. Гейт «Linux остаётся рабочим» для `transono` — это **сборка и тесты**, а не fmt/clippy. Требовать чистый clippy значило бы требовать отдельной уборки чужого кода, к порту не относящейся.
2. Приведение `transono` в порядок (`cargo fmt` по всему дереву + разбор ~200 предупреждений и 5 ошибок) — **отдельное решение владельца**, не задача порта. Если делать, то отдельным коммитом и отдельным планом: `cargo fmt` перепишет почти весь код, а это мешает сравнивать с историей.
3. В CI для `transono` проверки `fmt`/`clippy` либо не ставятся, либо ставятся как необязательные (`continue-on-error`) — чтобы видеть шум, но не краснеть из-за него.

**Ключевая проверка платформенного гейта, доступная без Mac** (`cargo tree` умеет строить граф под чужую цель, SDK для этого не нужен):

```bash
cd ~/projects/RUST/transono
nix develop --command bash -c \
  'cargo tree --target aarch64-apple-darwin -e normal --prefix none | sort -u | grep -E "^(pipewire|pipewire-sys|libspa|libspa-sys|cpal|objc2-core-audio|alsa)"'
```

**Сейчас** (до правок) вывод такой — Linux-only крейты присутствуют в графе под macOS, то есть сборка там упадёт на зависимостях:

```
cpal v0.18.1
libspa v0.10.0
libspa-sys v0.10.0
objc2-core-audio v0.3.2
pipewire v0.10.0
pipewire-sys v0.10.0
```

**После правок** в выводе обязаны остаться `cpal` и `objc2-core-audio` (это CoreAudio-путь cpal, он уже резолвится) и **исчезнуть** `pipewire`, `pipewire-sys`, `libspa`, `libspa-sys`, `alsa`.

## 0.3. Как проверяется сборка под Darwin до появления Mac

Три пути, все замерены или оценены по факту:

| Путь | Что даёт | Чего стоит | Проверено |
|---|---|---|---|
| `cargo tree --target aarch64-apple-darwin` | видно **граф зависимостей** под darwin: ловит возврат PipeWire, но не компилирует код | ничего, работает сразу | **да** — команда из §0.1 отработала, SDK не потребовался |
| `cargo check --target aarch64-apple-darwin` (просто, без тулчейна) | **не работает**: у rustc из nixpkgs нет std для darwin — падает на `the aarch64-apple-darwin target may not be installed`, ещё на `bitflags`/`cfg-if`/`log` | — | **да**, проверено и не прошло |
| Кросс-тулчейн из **nixpkgs** (путь из заметки владельца `docs/crossbuild.md`) | — | **не работает**, замерено 18.10.2026: сборка падает на пакете Apple `Csu-88` (`clang: command not found`), SDK так и не собирается | **да, проверено и не прошло** — разбор в задаче 13 |
| **Вариант В: Docker `joseluisq/rust-linux-darwin-builder` + osxcross** | **полную сборку под darwin локально**, вплоть до настоящих Mach-O бинарников (arm64 и x86_64) | `docker pull` + доустановка тулчейна 1.95 внутри (в образе 1.89 — нашему дереву мало) | **да, замерено 18.10.2026**: `Mach-O 64-bit arm64 executable` и `Mach-O 64-bit x86_64 executable` — рецепт и грабли в задаче 13 |
| **Харнесс подмены cfg** (`/home/hermes/tmp/ait/macos-cfg-check.sh`) | компиляцию **наших** macOS-веток (`src/ctl/macos.rs`, ветка macOS в `create_backend()`), но не darwin-крейтов | одна команда, ~20 секунд | **да**, вместе с проверкой на ложное срабатывание (подсунутая ошибка ловится) |
| Кросс-сборка через **osxcross руками** | то же, что вариант В, но своим тулчейном | SDK берётся из `joseluisq/macosx-sdks` (26.1 — 84 МиБ, sha256 опубликован), дальше сборка osxcross: час-два | нет — смысла нет, пока работает вариант В |
| Задание `macos` в CI (задача 12) | **полную сборку и тесты на настоящем Darwin** | push в публичный репозиторий; Actions бесплатны | **прогнано 18.10.2026, зелёное**: `transono` — linux ✓ 11:44:35→11:46:14, macos ✓ 11:44:40→11:46:31; `ai-translator` — linux ✓ 12:19:47→12:22:19, macos ✓ 12:19:55→12:22:58 |

**Практический вывод:** сборку под Darwin можно подтвердить локально — вариантом В (Docker + osxcross), и он уже дал настоящие Mach-O бинарники; задание `macos` в CI (задача 12) делает то же на настоящей машине и остаётся финальной проверкой. Кросс-тулчейн из nixpkgs не собрался вовсе. Порядок: задачи 1–3 → 13 (вариант В — сделано) → 12 (CI) → и уже потом 11 на живом Mac, где остаётся проверить **рантайм** (микрофон, разрешение TCC, живой перевод, задержки), а не сборку.

**Про `docs/crossbuild.md`:** файл лежит в дереве и не добавлен в git (`git status` показывает его как неотслеживаемый). Он полезен как справка по osxcross; решать, коммитить его или нет, — владельцу. В плане он учтён как путь «если захочется компилировать локально без Mac».

---

## 1. Что переносится как есть (проверено по коду)

| Что | Факт |
|---|---|
| Звук в `ai-translator` | `src/source/audio.rs:17` берёт `transono::audio::{AudioDevicesCpal, AudioInputCpal, AudioOutputCpal}` — **бэкенд cpal**, не PipeWire-модуль |
| Ядро `transono` | `src/line/`, `src/providers/openai/`, `src/core/` не знают про аудио-бэкенд (проверено: ни одного упоминания `cpal`/`pipewire`) |
| cpal-бэкенд `transono` | `src/audio/cpal/{device_cpal,input_cpal,output_cpal}.rs` — 723 строки, полноценная реализация (перечисление устройств, вход, выход), платформенно-нейтральный код |
| Остальные зависимости | tokio, axum, serde, toml, clap, ratatui, crossterm, dotenvy, tracing, rustls/ring, async-trait — кросс-платформенные |
| Наш код | единственное Unix-место — `src/main.rs:203` `tokio::signal::unix::signal(SignalKind::terminate())`; на macOS этот модуль есть, правок не требует |
| Веб-страница | `web/index.html` + `web/test/page.mjs` — платформа не важна |

**Вывод:** в коде `ai-translator` для macOS менять нечего.

## 2. Что мешает (три точки в `transono` + flake)

1. `transono/Cargo.toml`:
   - строка 18: `cpal = { version = "0.18", features = ["pipewire", "realtime"] }` — по исходникам cpal 0.18.1 `pipewire = ["dep:pipewire"]`, `realtime = ["dep:audio_thread_priority", "dep:alsa-sys"]`, то есть **обе фичи Linux-only**;
   - строка 41: `pipewire = "0.10"`; строка 44: `libspa-sys = "0.10"` — линкуются с `libpipewire-0.3`, на macOS не собираются.
2. `transono/src/audio/mod.rs:48-50` и `:70`: `pub mod pipewire;` и `pub use pipewire::*;` без условий.
3. `transono/src/ctl/mod.rs`: ветки есть только для `linux` и `windows`; на macOS `create_backend()` останется без тела — ошибка компиляции. Заодно: `WindowsBackend::new()` в ветке вызывается, но в `src/ctl/windows.rs` (2 строки) не существует — под Windows проект не собирается и сейчас; это не наша задача, но менять `windows.rs` не нужно.
4. `ai-translator/flake.nix` и `transono/flake.nix`: в `buildInputs`/`LD_LIBRARY_PATH`/`PKG_CONFIG_PATH` безусловно `glibc.dev`, `alsa-lib`, `pipewire` — на darwin таких атрибутов нет, вычисление флейка упадёт.
5. `transono/src/bin/transonovirt.rs` использует `daemonize` — на macOS это работает (Unix fork/setsid), правок не требует. Бинарники в сборку зависимости `ai-translator` не входят, но в самом репозитории `transono` должны собираться.

---

## 3. Задачи

### Задача 0. Зафиксировать базовое состояние Linux — **выполнено 18 октября 2026**

**Цель:** записать числа, с которыми будем сравнивать после каждой задачи.

**Результат (замерено до всяких правок):**

| Что | Значение |
|---|---|
| `transono`, `cargo test` | 117 + 6 = **123 теста, 0 провалов** |
| `transono`, `cargo fmt --check` / `cargo clippy` | **не чисто** (495 расхождений fmt; clippy падает: 5 ошибок + ~200 предупреждений) — см. §0.2 |
| `ai-translator`, `cargo test` | **104 + 3 + 3, 0 провалов**; `fmt` и `clippy -D warnings` чисто |
| Страница, `web/test/page.mjs` | **28 проверок**, «ВСЕ ПРОВЕРКИ ПРОЙДЕНЫ» |
| `cargo tree --target aarch64-apple-darwin` | в графе присутствуют `pipewire`, `pipewire-sys`, `libspa`, `libspa-sys`; присутствуют `cpal`, `objc2-core-audio` |

Коммит не нужен (изменений нет).

---

### Задача 1. `transono`: платформенные зависимости — **выполнено 18 октября 2026, коммит `c23748f`**

**Файл:** `~/projects/RUST/transono/Cargo.toml`

**Шаг 1.** Убрать фичи у cpal в общем блоке (строка 18):

```toml
cpal = "0.18"
```

**Шаг 2.** Убрать из общего блока строки 41 и 44 (`pipewire = "0.10"`, `libspa-sys = "0.10"`) и добавить в конец файла:

```toml
# Linux-only dependencies: the PipeWire backend of transono and the PipeWire
# host of cpal. On macOS cpal uses CoreAudio and the pipewire module is not
# compiled at all (see src/audio/mod.rs).
[target.'cfg(target_os = "linux")'.dependencies]
cpal = { version = "0.18", features = ["pipewire", "realtime"] }
pipewire = "0.10"
libspa-sys = "0.10"
```

**Почему cpal указан дважды:** cargo объединяет фичи по цели. На Linux остаётся ровно сегодняшний набор (`pipewire` + `realtime`), на macOS — cpal без фич, то есть CoreAudio. Поведение Linux не меняется.

**Шаг 3.** Проверка графа: команда из §0.1. Ожидание — в выводе нет `pipewire`, `pipewire-sys`, `libspa`, `libspa-sys`, `alsa`; есть `cpal` и `objc2-core-audio`.

**Шаг 4.** Прогон §0.1 (Linux). Ожидание — без изменений.

**Шаг 5.** Коммит в `transono`:

```bash
git add Cargo.toml Cargo.lock
git -c user.name="Andrey Velikoredchanin" -c user.email="andy@uncleandyv.com" \
  commit -m "Move the PipeWire dependencies under the Linux target"
```

---

### Задача 2. `transono`: гейт модуля PipeWire — **выполнено 18 октября 2026, коммит `c41e8ff`**

**Что оказалось шире плана (найдено grep-ом, план предполагал две строки в `mod.rs`):**

| Место | Что было | Что сделано |
|---|---|---|
| `src/audio/mod.rs` | `pub mod pipewire;`, `pub use pipewire::*;` | оба под `#[cfg(target_os = "linux")]` |
| `src/core/error.rs` | вариант `Pipewire(#[from] pipewire::Error)` | вариант под `#[cfg(target_os = "linux")]` — иначе библиотека не собирается на macOS |
| `src/audio/device.rs` | докссылка `[`crate::audio::pipewire`]` | переписана в обычный код-спан: на macOS ссылка битая |
| `src/bin/transonovirt.rs` | использует крейт `pipewire` напрямую | **не тронут по решению владельца:** бинарники `transono` для кросс-проверки не нужны, проверяется библиотека (`--lib`) |

**Файл:** `~/projects/RUST/transono/src/audio/mod.rs`

**Шаг 1.** Перед `pub mod pipewire;` (строка 48) поставить атрибут:

```rust
#[cfg(target_os = "linux")]
/// PipeWire audio backend.
pub mod pipewire;
```

**Шаг 2.** То же для реэкспорта (строка 70):

```rust
#[cfg(target_os = "linux")]
pub use pipewire::*;
```

**Шаг 3.** Убедиться, что больше нигде в крейте нет безусловных ссылок на этот модуль:

```bash
grep -rn "pipewire" --include=*.rs src/ | grep -v "^src/audio/pipewire/" | grep -v "^src/ctl/"
```

Ожидание: только две изменённые строки в `src/audio/mod.rs` (плюс `src/ctl/pipewire.rs`, он уже под гейтом).

**Шаг 4.** Проверка графа (§0.1) + прогон §0.1 (Linux). Ожидание — как в задаче 1.

**Шаг 5.** Коммит:

```bash
git add src/audio/mod.rs
git -c user.name="Andrey Velikoredchanin" -c user.email="andy@uncleandyv.com" \
  commit -m "Compile the PipeWire audio backend on Linux only"
```

---

### Задача 3. `transono`: ветка macOS в `ctl` (честная заглушка) — **выполнено 18 октября 2026, коммит `c20ecc4`**

**Отклонение от плана:** сообщение заглушки написано **на английском**, а не по-русски: в `transono` сообщения об ошибках в `ctl/` на английском (`failed to execute 'pactl list short modules'`), и отклоняться от стиля крейта не стоит.

**Побочная находка:** `src/ctl/windows.rs` — двухстрочная заглушка (`pub struct WindowsBackend;`) без реализации `Backend`, при этом ветка Windows в `create_backend()` её вызывает. То есть Windows-путь не собирается и сегодня; это чужая незавершённая работа, не наша. macOS-заглушка сделана рабочей, чтобы библиотека собиралась по-настоящему.

**Зачем:** без неё `create_backend()` не компилируется на macOS. Виртуальные звуковые устройства на macOS — это сторонние драйверы (BlackHole, Loopback), и притворяться, что мы ими управляем, нельзя: ветка обязана честно сообщать о неготовности.

**Файлы:**
- Создать: `~/projects/RUST/transono/src/ctl/macos.rs`
- Изменить: `~/projects/RUST/transono/src/ctl/mod.rs`

**Шаг 1.** Создать `src/ctl/macos.rs`:

```rust
use crate::core::error::{CoreError, Result};
use crate::ctl::backend::{Backend, DeviceSet, DeviceStatus, DoctorReport};

/// Backend implementation for macOS.
///
/// Virtual audio devices on macOS are third-party drivers (BlackHole, Loopback)
/// and are not managed by transono yet, so every operation reports this
/// honestly instead of pretending to work.
pub struct MacosBackend;

impl MacosBackend {
    /// Creates the backend. Nothing to initialise while devices are unmanaged.
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    fn unsupported() -> CoreError {
        CoreError::Internal(
            "виртуальные звуковые устройства на macOS не поддерживаются".to_string(),
        )
    }
}

impl Backend for MacosBackend {
    fn init(&self, _lang: &str) -> Result<()> {
        Err(Self::unsupported())
    }

    fn remove(&self, _lang: &str) -> Result<()> {
        Err(Self::unsupported())
    }

    fn devices(&self, _lang: &str) -> Result<DeviceSet> {
        Err(Self::unsupported())
    }

    fn status(&self, _lang: &str) -> Result<Vec<DeviceStatus>> {
        Err(Self::unsupported())
    }

    fn doctor(&self) -> Result<DoctorReport> {
        Err(Self::unsupported())
    }
}
```

Перед записью сверить с `src/ctl/backend.rs` точные имена и поля `DeviceSet`, `DeviceStatus`, `DoctorReport` и сигнатуру `doctor()` — в плане они приведены по факту чтения, но проверить ещё раз.

**Шаг 2.** В `src/ctl/mod.rs` добавить импорт и модуль по образцу linux/windows:

```rust
#[cfg(target_os = "macos")]
use crate::ctl::macos::MacosBackend;
...
#[cfg(target_os = "macos")]
/// macOS-specific backend (virtual devices are not managed yet).
pub mod macos;
```

**Шаг 3.** Добавить ветку в `create_backend()` и честный отказ для прочих платформ:

```rust
    #[cfg(target_os = "macos")]
    {
        MacosBackend::new().map(|backend| Box::new(backend) as Box<dyn Backend>)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err(crate::core::error::CoreError::Internal(
            "платформа не поддерживается".to_string(),
        ))
    }
```

**Шаг 4.** Прогон §0.1 (Linux). На Linux эта ветка не компилируется, но `create_backend` обязан собираться без изменений.

**Шаг 5.** Коммит:

```bash
git add src/ctl/macos.rs src/ctl/mod.rs
git -c user.name="Andrey Velikoredchanin" -c user.email="andy@uncleandyv.com" \
  commit -m "Add an honest macOS backend stub for virtual audio devices"
```

---

### Задача 4. `transono`: бинарники под macOS — **выполнено 18 октября 2026, коммит `79ba8ae`**

**Шаг 1 (Linux, здесь):** `cargo build --bins` проходит; `src/bin/transono.rs` платформенно-нейтрален (grep по `pipewire|/dev/snd|libc::` пуст). Вся Linux-специфика — в `src/bin/transonovirt.rs`: он напрямую грузит модули PipeWire (`pw_context_load_module`), то есть Linux-only по природе.

**Шаг 2 (darwin, проверено кросс-сборкой, а не отложено до Mac):** `cargo build --bins --target aarch64-apple-darwin` **падал** — 6 ошибок (`cannot find module or crate pipewire`, `unresolved import transono::ctl::pipewire`). Закрыто в `transono`: импорты, `Cli`, три вспомогательные функции и `main` — под `#[cfg(target_os = "linux")]`, а для остальных платформ добавлен свой `main`, который печатает «transonovirt manages PipeWire virtual audio devices and exists on Linux only» и выходит с кодом 1.

**Проверено после правки:** под darwin собираются **оба** бинарника — `transono` и `transonovirt`, оба `Mach-O 64-bit arm64 executable`; под Linux `cargo build --bins` и 123 теста как были, лишних предупреждений нет.

**Две поправки к плану:** предлагалось закрыть файл через `#[cfg(unix)]` — macOS тоже unix, это ничего бы не дало, нужно именно `target_os = "linux"`; и атрибут `#![cfg(...)]` на весь файл не годится — бинарник остался бы без `main`, и cargo выдал бы «main function not found».

**Цель:** убедиться, что `src/bin/transono.rs` и `src/bin/transonovirt.rs` не тянут Linux-специфику сверх уже закрытой.

**Шаги:**
1. Проверить здесь, на Linux: `cargo build --bins` проходит; `grep -rn "pipewire\|/dev/snd\|libc::" src/bin/` — пусто (кроме `ctl::`, который закрыт задачей 3).
2. На macOS (задача 11) отдельно: `cargo build --bins`. Ожидание — собирается; `transonovirt` использует `daemonize`, который на macOS работает через fork/setsid. Если сборка `daemonize` под darwin не пройдёт, честный вариант — закрыть `src/bin/transonovirt.rs` атрибутом `#[cfg(unix)]` (он и так Unix-only) и записать это в отчёт.
3. Коммит — только если правка потребовалась.

---

### Задача 5. `ai-translator`: `flake.nix` без Linux-пакетов на darwin — **выполнено 18 октября 2026, коммит `c0972c9`**

**Как проверено (Linux, без Mac):**
- `.cargo/config.toml` до и после пересборки шелла — **побайтово совпадает**: Linux-ветка дословна (порядок пакетов в `PKG_CONFIG_PATH` сохранён намеренно, иначе `diff` ловит перестановку).
- В darwin-шелле Linux-пакетов **нет**, в linux-шелле они **есть** — контрольная проверка, что тест их вообще видит. Способ: оценка `devShells.aarch64-darwin.default.drvPath` из **чистой копии флейка** (на грязном git-дереве `builtins.getFlake` падает с `Permission denied` на `.git/objects`, и тогда «пусто» в выводе — ложь, а не результат), затем `nix derivation show <drv>` и поиск `pipewire|alsa|glibc` среди входов.
- Регрессия §0.1 зелёная: `fmt` чисто, `clippy -D warnings` exit=0 (код возврата снят без маскировки пайпом), 104 + 3 + 3 теста, страница 28 проверок, `transono` — сборка бинарников и 123 теста.

**Флейк `transono` — сделано 18 октября 2026, коммит `53f27e7`:** те же три пакета (`glibc.dev`, `alsa-lib`, `pipewire`) и хелпер `fixRights` (`sudo chown -R andy:hermes ./`, хост-имена внутри) уехали в Linux-условие; `PKG_CONFIG_PATH` под darwin теперь `openssl`, под Linux — ровно прежний `pipewire:alsa`; `BINDGEN_EXTRA_CLANG_ARGS` выдаётся только на Linux, и в `shellHook`, и в `.cargo/config.toml`. Проверено: в darwin-шелле нет ни `pipewire/alsa/glibc`, ни `fixr`; в linux-шелле все три на месте (контроль); Linux — сборка бинарников, 123 теста, набор зависимостей прежний, `.cargo/config.toml` побайтово тот же.

**Файл:** `~/projects/RUST/ai-translator/flake.nix`

**Шаг 1.** Разложить Linux-пакеты по условию, сохранив Linux-ветку дословно:

```nix
      let
        pkgs = import nixpkgs { inherit system; };
        isLinux = pkgs.stdenv.isLinux;
        # PipeWire, ALSA и glibc нужны только сборке под Linux: cpal берёт их
        # через фичу pipewire, а libspa-sys требует заголовков PipeWire.
        linuxOnly = pkgs.lib.optionals isLinux;
      in {
        devShells.default = pkgs.mkShell {
          name = "ai-translator-dev";

          shellHook = ''
            mkdir -p .cargo
            cat <<EOF > .cargo/config.toml
            [env]
            PKG_CONFIG_PATH = { value = "${pkgs.lib.makeSearchPath "lib/pkgconfig" ([ pkgs.openssl.dev ] ++ linuxOnly [ pkgs.pipewire.dev pkgs.alsa-lib.dev ])}", relative = false }
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib"
            EOF
            ${pkgs.lib.optionalString isLinux ''
              echo 'BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include"' >> .cargo/config.toml
            ''}
            export CARGO_TARGET_DIR="$HOME/.cache/ai-translator-target"
          '';

          nativeBuildInputs = with pkgs; [ pkg-config clang rustc cargo rustfmt clippy rust-analyzer ];

          buildInputs = with pkgs; [ openssl llvmPackages.libclang ]
            ++ linuxOnly [ glibc.dev alsa-lib pipewire ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath ([ pkgs.openssl ] ++ linuxOnly [ pkgs.alsa-lib pkgs.pipewire ]);
        };
      });
```

**Важно:** текст heredoc внутри `shellHook` должен остаться таким, чтобы на Linux получился ровно сегодняшний `.cargo/config.toml` (те же три переменные с теми же значениями). Проверяется сравнением файла до и после:

```bash
cp .cargo/config.toml /tmp/config-before.toml
nix develop --command true
diff /tmp/config-before.toml .cargo/config.toml && echo "Linux-конфигурация не изменилась"
```

**Шаг 2.** Прогон §0.1 (Linux) — сборка и тесты как были.

**Шаг 3.** Коммит:

```bash
git add flake.nix
git -c user.name="Andrey Velikoredchanin" -c user.email="andy@uncleandyv.com" \
  commit -m "Keep the Linux packages out of the Darwin dev shell"
```

**Замечание:** `transono/flake.nix` содержит те же три пакета плюс хелпер `fixRights` с `sudo chown andy:hermes`. Тот же приём применяется к нему отдельной задачей внутри `transono` (не блокирует сборку `ai-translator`, но нужен, чтобы `nix develop` там работал на macOS).

---

### Задача 6. Сборка на macOS без Nix

**Цель:** зафиксировать самый дешёвый путь проверки на Mac — без флейка вообще.

**Шаг 1.** На macOS достаточно Xcode Command Line Tools (`xcode-select --install`) и rustup-тулчейна: на пути cpal/CoreAudio нативных библиотек не требуется (проверено по графу зависимостей: PipeWire/ALSA/glibc на darwin не нужны, CoreAudio — системный фреймворк).

```bash
cd ~/projects/RUST/transono && cargo build
cd ~/projects/RUST/ai-translator && cargo build
```

**Шаг 2.** Записать в README `ai-translator` короткий раздел (по-английски) — «Building on macOS» с этими двумя командами и оговоркой, что Nix-флейк рассчитан на Linux.

---

### Задача 7. Команда `devices` (список звуковых устройств) — **выполнено 18 октября 2026, коммит `9d11559`**

**Как сделано:** трейт `DeviceLister` (`inputs`/`outputs`), реализация `CpalDeviceLister` через `AudioDevicesCpal` и чистая функция `render_devices` — её и проверяют тесты с подставным списком. Тест-первым: пять тестов сначала падали на `todo!` (RED), затем реализация (GREEN). Тестов в `ai-translator` стало **109 вместо 104**, `fmt`/`clippy -D warnings` чисто, страница 28 проверок, `transono` 123.

**Что выяснилось про cpal 0.18 — пригодится и в других местах:**
- метода `name()` у устройства больше нет; имя берётся через `DeviceTrait::description()?.name()`;
- `Display` устройства печатает ровно имя, **но** на отключённом устройстве возвращает ошибку форматирования, то есть `to_string()` паникует — поэтому взят `description()` с отбрасыванием ошибки (команда обязана пережить отсутствие звука);
- `cpal` пришлось объявить **прямой** зависимостью `ai-translator`: трейт должен быть в области видимости. Версия — та же `0.18`, что у `transono` (`Cargo.lock` держит одну 0.18.2), иначе типы `Device` не совпадут.

**Прогон на стенде:** команда нашла устройства (ALSA/PipeWire-псевдоустройства видны и как входы, и как выходы — это честный ответ cpal), `exit=0`. Оговорка «звука в песочнице нет» из плана оказалась неверной: звуковой системы нет как устройства вывода, но псевдоустройства PipeWire cpal перечисляет.

**Что это:** новая подкоманда CLI, которая печатает список доступных устройств входа и выхода с их именами:

```
$ ai-translator devices
входы:
  default
  MacBook Pro Microphone
  BlackHole 2ch
выходы:
  default
  MacBook Pro Speakers
```

**Зачем:** поле `input_device` в конфигурации задаётся **именем** устройства, а посмотреть, какие имена существуют, сегодня нечем — их надо угадывать или выуживать системными утилитами (`system_profiler SPAudioDataType` на macOS, `wpctl status` на Linux). Имя из системной утилиты и имя, которое видит приложение, — не всегда одно и то же: `ai-translator` получает список через cpal, а cpal берёт имена у своего бэкенда. Команда показывает ровно тот список, из которого приложение выбирает устройство, поэтому ошибиться уже негде. На macOS это заметнее: имена вида `MacBook Pro Microphone`, `AirPods Pro`, `BlackHole 2ch` не угадываются. На Linux польза меньше, но есть.

**Стоимость:** около сотни строк с тестами, код платформенно-нейтральный. Проверить совпадение имён с системными утилитами на этом стенде нельзя: звука в песочнице нет (`/dev/snd` отсутствует).

**Решение владельца: делаем.** (Альтернатива, если бы отказались: написать в README, как узнать имена системными средствами, и принять риск несовпадения имени с тем, что видит cpal.)

**Файлы:**
- Изменить: `src/main.rs` (новая подкоманда), `src/source/audio.rs` (перечисление через cpal)
- Тест: модульные тесты в `src/source/audio.rs`

**Шаг 1. Тест.** Ввести тонкую обёртку над перечислением, чтобы её можно было проверить без железа:

```rust
/// Список доступных звуковых устройств.
pub trait DeviceLister {
    /// Имена устройств входа.
    fn inputs(&self) -> Vec<String>;
    /// Имена устройств выхода.
    fn outputs(&self) -> Vec<String>;
}
```

Реализация `CpalDeviceLister` вызывает `AudioDevicesCpal::{input_devices, output_devices}` и берёт `name()`. Тест подставляет фальшивый список и проверяет формат вывода (устройства по алфавиту, входы и выходы помечены).

**Шаг 2. Проверка, что тест падает** до реализации команды, затем реализация и повторный прогон.

**Шаг 3. Прогон на Linux** (реальный список устройств на стенде может быть пустым — команда обязана сказать это внятно, а не падать):

```bash
nix develop --command bash -c 'cargo run -q -- devices'
```

**Шаг 4.** Прогон §0.1 и коммит.

**Пометка:** если решите не добавлять команду — задача пропускается, но тогда в README нужно написать, как узнать имена устройств на macOS (`system_profiler SPAudioDataType`).

---

### Задача 8. Голосовой приёмник на macOS — **выполнено 18 октября 2026, коммит `743dcb9`**

Кода не потребовалось: честная недоступность приёмника — правильное поведение на всех платформах. Записано: ADR-002 в `docs/decisions.md` (на macOS виртуальное устройство — сторонний драйвер BlackHole/Loopback, который ставит пользователь; реализация — Этап 4 поверх `transono::ctl::Backend`) и абзац в README в разделе Sinks. Регрессия зелёная.

**Файл:** `src/sink/voice/virtual_sink.rs` (сейчас заглушка, возвращающая `SinkError::Unavailable`)

**Шаги:**
1. Ничего не менять в коде: честная недоступность — правильное поведение на всех платформах, пока виртуальные устройства не реализованы.
2. Дописать в `README.md` и `docs/decisions.md`: на macOS виртуальное звуковое устройство — это сторонний драйвер (BlackHole или Loopback), устанавливаемый пользователем; реализация — отдельный этап (Этап 4), и делать её надо поверх `transono::ctl::Backend`, где для macOS уже появится честная ветка из задачи 3.
3. Прогон §0.1 и коммит (только документация).

---

### Задача 9. Разрешение на микрофон (TCC) — **выполнено 18 октября 2026, коммит `bf7751d`**

Раздел «Microphone permission on macOS» в README: разрешение выдаётся **терминалу**, из которого запущена команда, а не бинарнику; принятый запрос — и `run` работает; если запрос закрыли — System Settings → Privacy & Security → Microphone либо `tccutil reset Microphone` (упомянуто как способ перезапросить, не выполнялось); для будущего `.app` — `NSMicrophoneUsageDescription` в `Info.plist` плюс подпись и нотаризация, это Этап 4.

**Цель:** описать поведение, а не бороться с ним.

**Шаги:**
1. Зафиксировать факт: при запуске из терминала macOS выдаёт разрешение на микрофон **терминалу**, а не нашему бинарнику; после этого `ai-translator run` работает. При первом обращении система показывает запрос — его нужно принять.
2. Записать в README (по-английски): раздел «Microphone permission on macOS» — что запрос появляется при первом `run`, что разрешение привязано к терминалу (Terminal/iTerm/VS Code), как его снять и выдать заново (`tccutil reset Microphone` — упомянуть как способ перезапросить, без выполнения).
3. Для будущей поставки в виде `.app` — записать в план Этапа 4: `NSMicrophoneUsageDescription` в `Info.plist` и подпись/нотаризация.
4. Коммит — только документация.

---

### Задача 10. Документация и правила — **выполнено 18 октября 2026, коммит `d6abefa`**

README: раздел «Platforms» — Linux (основная, Nix-флейк, PipeWire/ALSA через `transono`, виртуальные устройства через его PipeWire-модуль), macOS (сборка через Xcode CLT и rustup без Nix, звук через CoreAudio/cpal, PipeWire-модуль не компилируется, голосовой приёмник недоступен, разрешение спрашивается у терминала), Windows — не поддерживается (бэкенд `ctl` там незавершённая заглушка). `AGENTS.md`: раздел «Платформы» — платформенный код только через `cfg` и target-секции, Linux-путь не ослаблять, Linux-only пакеты не должны попадать в darwin-граф, после каждой такой правки — прогон «Сборка и проверки» плюс проверка сборки под darwin.

**Файлы:** `README.md` (англ.), `AGENTS.md`

**Шаги:**
1. README: раздел «Platforms» — Linux (основная, Nix-флейк, PipeWire-модуль `transono` и виртуальные устройства) и macOS (сборка через Xcode CLT, звук через CoreAudio/cpal, голосовой приёмник недоступен, виртуальные устройства — сторонний драйвер).
2. AGENTS.md: правило «платформенный код — только через target-секции и `cfg`, Linux-путь не ослаблять; после каждой такой правки прогонять §0.1».
3. Прогон §0.1, коммит.

---

### Задача 11. Приёмка на реальном Mac (выполняет владелец)

**Статус:** Mac будет найден владельцем позже. **Что снимается раньше:** после задачи 12 (CI, задание `macos`) компиляция и прогон тестов на настоящем Darwin проверяются автоматически, без Mac. **Что остаётся именно на живую машину:** рантайм — запрос доступа к микрофону, живой перевод из микрофона, задержки, имена и поведение устройств, работа страницы в настоящем браузере. До прохождения этой таблицы порт не считается принятым.

Проверки, которые невозможно выполнить здесь (нужен Mac и микрофон). Каждая — с ожидаемым результатом.

| № | Команда | Ожидание |
|---|---|---|
| 1 | `cd transono && cargo build && cargo test` | сборка без PipeWire-зависимостей, тесты как на Linux (число из задачи 0) |
| 2 | `cd transono && cargo build --bins` | `transono`, `transonovirt` собираются |
| 3 | `cd ai-translator && cargo build && cargo test` | 104 + 3 + 3, все проходят |
| 4 | `cargo run -q -- check --config examples/config.dev.toml` | «конфигурация корректна» |
| 5 | `cargo run -q -- script --config <конфиг без звука> --text "..."` | фразы доходят до консоли и в транскрипт, экран отдаётся |
| 6 | `cargo run -q -- run --config examples/config.dev.toml` (с `OPENAI_API_KEY`) | запрос доступа к микрофону, затем живой перевод; в консоли фразы |
| 7 | `curl -s http://127.0.0.1:8643/settings.js` | `window.TRANSLATOR_SETTINGS = {"line_size_vw": 4.5, "min_line_size_vw": 2}` |
| 8 | страница `http://127.0.0.1:8643/` в браузере | текст показывается, размер не «дышит» между фразами |
| 9 | `Ctrl+C` во время `run` | «останавливаюсь», «остановлено», процесс выходит, файл дописан |
| 10 | `cargo run -q -- devices` (если задача 7 сделана) | список устройств входа/выхода, `input_device` из него принимается |
| 11 | `cargo run -q -- check` на конфигурации с `input_device = "<имя из п.10>"` | корректна и работает |

**Что считать провалом порта:** любое расхождение по пунктам 3–6 (поведение, а не косметика) — фиксируется в отчёте, а не «обходится» правкой конфигурации.

---

### Задача 12. CI: полный вариант — Linux и macOS (решение владельца: делаем)

**Решение:** полный вариант с двумя платформами. Репозитории публичные (`github.com/UncleAndy/transono`, `github.com/UncleAndy/ai-translator`, ветка `main`), а для публичных репозиториев Actions бесплатны, включая macOS-раннеры, — множитель минут значения не имеет.

**Ключевая деталь, из-за которой «просто добавить ci.yml» не сработает:** `ai-translator` зависит от `transono` **по пути** — `transono = { path = "../transono" }`. Один чекаут такого репозитория не соберётся: рядом не окажется `transono`. Поэтому в workflow `ai-translator` делается **два чекаута** в соседние каталоги, и путь `../transono` сходится ровно как на машине разработчика.

**Второй факт, влияющий на набор проверок:** `transono` сегодня не проходит ни `fmt`, ни `clippy` (§0.2), поэтому в его workflow стоят только сборка и тесты. Требовать от порта чистый clippy чужого кода — значит гарантированно получить красный CI с первого дня.

**Файл 1: `transono/.github/workflows/ci.yml`**

```yaml
name: ci

on: [push, pull_request]

jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Системные зависимости
        run: |
          sudo apt-get update
          sudo apt-get install -y libpipewire-0.3-dev libasound2-dev libclang-dev pkg-config
      - uses: dtolnay/rust-toolchain@stable
      # fmt и clippy здесь сознательно не гейт: дерево сейчас не проходит ни то, ни другое
      - run: cargo build --bins
      - run: cargo test

  macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      # Бинарники тоже: transonovirt управляет устройствами PipeWire и по своей
      # природе Linux-only, поэтому под darwin он сведён к заглушке (задача 4).
      # Сборка бинарников не даёт Linux-only импорту вернуться незамеченным —
      # проверка --lib его не увидит.
      - run: cargo build --bins
      - run: cargo test --lib
```

**Обновлено 18 октября 2026, коммит `b8f11fb`:** задание `macos` собирает и бинарники (`--bins` вместо `--lib`) — после задачи 4 они собираются под darwin, а `--lib` не увидел бы Linux-only импорт в бинарнике. YAML проверен `yamllint` (exit=0; единственное предупреждение о длине строки — в строке `apt-get`, было до правки). Настоящий прогон — по первому push.

**Файл 2: `ai-translator/.github/workflows/ci.yml`**

```yaml
name: ci

on: [push, pull_request]

jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          path: ai-translator
      # Зависимость по пути: transono должен лежать рядом
      - uses: actions/checkout@v4
        with:
          repository: UncleAndy/transono
          path: transono
      - name: Системные зависимости
        run: |
          sudo apt-get update
          sudo apt-get install -y libpipewire-0.3-dev libasound2-dev libclang-dev pkg-config
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - name: fmt
        working-directory: ai-translator
        run: cargo fmt --check
      - name: clippy
        working-directory: ai-translator
        run: cargo clippy --all-targets -- -D warnings
      - name: tests
        working-directory: ai-translator
        run: cargo test
      - name: логика страницы
        working-directory: ai-translator
        run: node web/test/page.mjs
      - name: macOS не должен тянуть PipeWire
        working-directory: ai-translator
        run: |
          ! cargo tree --target aarch64-apple-darwin -e normal --prefix none \
            | grep -qE '^(pipewire|pipewire-sys|libspa|libspa-sys)'

  macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
        with:
          path: ai-translator
      - uses: actions/checkout@v4
        with:
          repository: UncleAndy/transono
          path: transono
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - name: fmt
        working-directory: ai-translator
        run: cargo fmt --check
      - name: clippy
        working-directory: ai-translator
        run: cargo clippy --all-targets -- -D warnings
      - name: tests
        working-directory: ai-translator
        run: cargo test
```

**Почему проверки именно такие:**

| Шаг | Зачем |
|---|---|
| `cargo tree … \| grep -qE '^(pipewire\|libspa…)'` с `!` впереди | сторож платформенного гейта: падает, если Linux-only крейты вернутся в граф под darwin. Работает без Mac-раннера |
| `cargo test` в `transono` на macOS | тесты `transono` не требуют звукового железа (123 теста прошли на стенде без звука), значит на раннере они осмысленны |
| `node web/test/page.mjs` | 28 проверок логики страницы; node на `ubuntu-latest` уже есть |
| `clippy -D warnings` только в `ai-translator` | наше дерево чисто, чужое — нет |

**Шаги:**
1. Создать оба файла.
2. Проверить синтаксис YAML локально: `nix shell nixpkgs#yamllint -c yamllint <файл>`.
3. Локально воспроизвести содержимое Linux-заданий теми же командами (они совпадают с §0.1) — это единственная часть CI, которую можно проверить до заливки.
4. Коммит в каждый репозиторий. Заливку владелец делает сам; до неё workflow не запускается, поэтому «CI зелёный» подтверждается только после push и первого прогона.
5. После первого прогона: если задание `macos` в `transono` упадёт — это ожидаемо **до** выполнения задач 1–3 и означает ровно то, ради чего порт и делается.

**Оценка времени:** создание двух файлов и проверка YAML — около пятнадцати минут. Чего нельзя проверить локально: что workflow реально запустится и пройдёт — для этого нужен push.

**Если решите ограничиться Linux-заданием** (убрать job `macos` из обоих файлов) — файлы сократятся вдвое, а сторож графа останется. Полный вариант оставлен по вашему решению.

---

### Задача 13. Локальная проверка компиляции под darwin — **выполнено: вариант В (Docker + osxcross) собирает настоящие Mach-O бинарники**

**Рецепт целиком записан в `docs/macos-cross-build.md`** (128 строк, вне git — рядом с заметкой владельца `docs/crossbuild.md`), опыт — в скилл `rust-macos-cross-from-linux`. Образы оставлены намеренно (решение владельца): `rust-darwin-builder:1.95` (с тулчейном 1.95 внутри) и базовый `joseluisq/rust-linux-darwin-builder:1.89.0`.

**Итог по варианту А (nixpkgs), замерено 18 октября 2026: тулчейн собрать нельзя.** `nix build pkgsCross.aarch64-darwin.hello` доходит до пакета Apple `Csu-88` и падает:

    building 'Csu-88.drv'...
      > build flags: ... CC=clang ...
      > clang -r ... -o crt1.v1.o ...
      > bash: line 1: clang: command not found
      > make: *** [Makefile:72: crt1.v1.o] Error 127

Причина не в `allowUnsupportedSystem`: пакеты Apple в nixpkgs рассчитаны на **нативную** darwin-машину (там `clang` есть в PATH), под Linux их не собирает никто. Дальше по цепочке падают `apple-sdk-14.4` и `arm64-apple-darwin-clang-wrapper`. SDK в хранилище так и не появился, `osxcross` в nixpkgs отсутствует. Значит утверждение заметки владельца «на NixOS всё уже в nixpkgs и SDK не нужен» **на практике неверно** — проверено, а не предположено.

**Вариант В (Docker + osxcross) — РАБОТАЕТ, замерено 18 октября 2026.** Это то, что закрывает вопрос полностью:

1. `docker pull joseluisq/rust-linux-darwin-builder:1.89.0` — Debian + osxcross + macOS SDK внутри. Родственный репозиторий `joseluisq/macosx-sdks` отдаёт сами SDK (26.1 — 84 МиБ, sha256 опубликован) для случая, когда osxcross собирают руками; в готовом образе SDK уже есть.
2. **Поправка, без которой ничего не соберётся:** Rust в образе — 1.89, а нашему дереву нужно 1.95 (`non-empty-slice`/`non-empty-iter`/`non-zero-size` объявляют `rust-version = 1.95` и на 1.89 не компилируются — проверено: 15 ошибок `E0658`). Лечится внутри контейнера: `rustup toolchain install 1.95.0 --profile minimal && rustup target add --toolchain 1.95.0 aarch64-apple-darwin`.
3. Сборка: `cargo +1.95.0 build --target aarch64-apple-darwin` (и `x86_64-apple-darwin` — тоже проходит).
4. **Результат замерен:** оба прогона дали настоящие бинарники — `file` говорит `Mach-O 64-bit arm64 executable` (38 МБ) и `Mach-O 64-bit x86_64 executable` (45 МБ); `transono` при этом собирается как зависимость ✓.

**Грабли, которые пришлось обойти:** монтировать надо каталог, видимый демону докера (в песочнице это `/run/hermes-tmp/...`, а не `/tmp` — иначе «Permission denied»); в контейнер нельзя тащить `.cargo/config.toml` от флейка (в нём Linux-пути); для C-крейтов (`ring`) нужны `CC_aarch64_apple_darwin` и `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=aarch64-apple-darwin22.4-clang` — в конфиге образа для aarch64 прописан `arm64e`, а это другой вариант архитектуры; после установки тулчейна образ сохранён как `rust-darwin-builder:1.95` (`docker commit`), чтобы не ставить 1.95 каждый раз.

**Чего это не даёт:** рантайма (микрофон, разрешение TCC, задержки) и подписи — неподписанный бинарник на Mac встретит Gatekeeper. Это остаётся задачей 11.

**Харнесс подмены cfg (`/home/hermes/tmp/ait/macos-cfg-check.sh`)** — быстрая проверка на ~20 секунд, без докера: компилирует наши macOS-ветки (`ctl/macos.rs`, ветка в `create_backend()`) на Linux. Проверен на ложное срабатывание (подсунутая ошибка ловится). Остаётся дешёвым дымовым тестом, но полную сборку теперь даёт вариант В.

**Задание `macos` в CI (задача 12)** — то же самое, но на настоящей машине, без локальной возни и без докера; остаётся финальной проверкой перед приёмкой.

**Что даёт и чего не даёт:** типизацию и линковку ветвей `#[cfg(target_os = "macos")]` (в нашем случае — `src/ctl/macos.rs` и `ctl/mod.rs` в `transono`), которые на Linux не компилируются вообще. Рантайм (микрофон, TCC, задержки) не проверяет — это остаётся задаче 11.

#### Вариант А. Средствами nixpkgs (проверено на этом стенде)

Заметка владельца `docs/crossbuild.md` предлагает именно этот путь, но рецепт в ней неполон. Проверенные поправки:

1. **Обязательно** `config.allowUnsupportedSystem = true` — без него падает сразу:
   `Refusing to evaluate package 'arm64-apple-darwin-cctools-1010.6' … because it is not available on the requested hostPlatform: hostPlatform.system = "x86_64-linux"`.
2. С флагом всё нужное действительно собирается из nixpkgs: **58 дериваций, 533 МиБ скачать, ~4 ГиБ развернуть**, среди них `apple-sdk-14.4`, `ld64-956.6`, `cctools-1010.6`, `xcbuild`, `compiler-rt-arm64-apple-darwin`, `arm64-apple-darwin-clang-wrapper`. То есть SDK добывать руками не нужно — утверждение заметки в этой части верно.
3. Имя линковщика — **`arm64-apple-darwin-clang`** (проверено: `targetPrefix` = `arm64-apple-darwin-`), а не `aarch64-apple-darwin<version>-clang`, как в заметке. С неверным именем сборка упадёт на линковке.
4. `rustup target add aarch64-apple-darwin` — **обязателен** и в заметке указан верно: у rustc из nixpkgs нет std для darwin (проверено: `cargo check --target aarch64-apple-darwin` падает на `the aarch64-apple-darwin target may not be installed`).
5. Для C-крейтов (`ring`) cc-rs читает переменную с суффиксом цели — `CC_aarch64_apple_darwin` (и `AR_aarch64_apple_darwin`), а не только общий `CC`.

**Файлы:** флейк `ai-translator` (или `transono`) — окружение; правку линковщика вносить в `shellHook`, потому что `.cargo/config.toml` генерируется флейком и лежит в `.gitignore` (иначе затрётся при следующем входе).

#### Вариант Б. osxcross (как в первой части заметки)

1. macOS SDK от Apple (учётная запись Apple Developer либо SDK из `tpoechtrager/osxcross`) — **лицензия Apple не разрешает распространение**, поэтому в nixpkgs и в Docker-образах его нет;
2. собрать osxcross, получить `aarch64-apple-darwin*-clang`;
3. `rustup target add aarch64-apple-darwin`;
4. в `.cargo/config.toml` — **и линковщик, и `ar`** (в заметке `ar` пропущен, а для `ring` он нужен);
5. `cargo build --target aarch64-apple-darwin --release`.

**Чего делать не стоит:** `cross` из «Способа 2» заметки. По документации `cross` цели Apple/Darwin не поддерживает и перенаправляет на сторонние тулчейны (в образах нет SDK — по той же лицензионной причине). Команда `cross build --target aarch64-apple-darwin` из заметки не заработает. К тому же URL в заметке обрезан (`cargo install cross --git https://github.com`).

**Приговор:** вариант А — самый дешёвый локальный путь (без Apple-аккаунта, всё декларативно), но первая сборка тулчейна небыстрая; вариант Б — проверенный временем, но SDK надо добывать. Оба дают только компиляцию. Поэтому по умолчанию идём через задачу 12, а эту задачу трогаем лишь при желании проверить сборку локально до push.

---

## 4. Приёмка

**Linux (проверяется здесь, после каждой задачи):**

| Проверка | Ожидание |
|---|---|
| `transono`: сборка (`cargo build --bins`) и тесты | 123 теста, 0 провалов (как в задаче 0); fmt/clippy не гейт — см. §0.2 |
| `transono`: набор зависимостей под Linux | `cpal` с фичами `pipewire`+`realtime`, `pipewire`, `libspa-sys` — как сейчас |
| `ai-translator`: fmt, clippy, тесты | 109 + 3 + 3 (104 + 5 у команды `devices`), без изменений |
| Страница (`web/test/page.mjs`) | 28 проверок, «ВСЕ ПРОВЕРКИ ПРОЙДЕНЫ» |
| `nix develop` + `.cargo/config.toml` | файл побайтово как до правки |
| `cargo tree --target aarch64-apple-darwin` | нет `pipewire`/`libspa`/`alsa`, есть `cpal` + `objc2-core-audio` |

**macOS (задача 11):** пункты 1–11 таблицы выше.

## 5. Риски, размены и открытые вопросы

**Риски:**

1. **cpal/CoreAudio на живом железе.** Устройства, форматы (f32 ожидается), число каналов, частоты дискретизации и ресемплинг (`rubato`) — платформенно-нейтральный код, но проверяется только на Mac. Это главный источник «мелочей», которые вылезут после сборки.
2. **Нет `realtime`-фичи на macOS.** `audio_thread_priority` Linux-only, значит на macOS нет того же подъёма приоритета аудиопотока. CoreAudio обычно сам выставляет приоритет своим потокам, но задержки и стабильность могут отличаться — проверять на пунктах 6–9 приёмки.
3. **Проверка только на Mac.** Здесь я могу проверить граф зависимостей, гейты и Linux-регрессии, но не сборку и не звук под darwin.
4. **TCC.** Разрешение привязано к терминалу; при запуске из другого окружения (launchd, .app) потребуется отдельная выдача и `Info.plist`.
5. **`daemonize` под darwin** — снят: единственный, кто его использует, — `transonovirt`, а он закрыт гейтом `target_os = "linux"` (под darwin на его месте честная заглушка). Сборка бинарников под darwin проверена кросс-сборкой в задаче 4.
6. **Двойной `cpal` в манифесте** (общий + linux-секция) — приём корректный, но требует проверки, что на Linux набор фич не изменился (проверяется `cargo tree` под Linux, задача 1, шаг 4).

**Вне области этого порта (отдельные решения):**

- Виртуальное звуковое устройство на macOS (BlackHole/Loopback) — Этап 4, поверх `Backend`.
- Захват **системного** звука: **решение владельца — не нужен, переводим только звук с аудио-входов.** Для справки: CoreAudio loopback не даёт, понадобился бы виртуальный драйвер или ScreenCaptureKit (macOS 13+, с разрешением на запись экрана).
- Подпись и нотаризация `.app`.
- Windows: наш `main.rs` использует `tokio::signal::unix`, плюс в `transono` `WindowsBackend::new()` не существует — отдельная работа, и она больше.

**Открытые вопросы и решения владельца:**

| Вопрос | Решение |
|---|---|
| 1. Правки `transono` — коммитить в его репозиторий или держать локально? | **Коммитить в `transono`** (совпадает с заявленной в его README целью «кроссплатформенность»). Коммиты — задачами 1–3, каждым своим сообщением; заливку на удалённый репозиторий владелец делает сам |
| 2. Нужна ли команда `devices` (задача 7)? | **Да, делаем** — что это и зачем, см. врезку в задаче 7 |
| 3. Нужен ли CI (задача 12)? | **Да, полный вариант: Linux и macOS.** Репозитории публичные, поэтому Actions бесплатны и множитель минут для macOS не важен. Два файла workflow, в `ai-translator` — два чекаута, потому что зависимость `transono` идёт по пути. Детали и готовые YAML — в задаче 12 |
| 4. Системный звук? | **Не нужен** — только звук с аудио-входов (задача 11, «Вне области») |
| 5. Mac для приёмки? | **Появится позже**; до этого сборка под darwin проверяется кросс-тулчейном (задача 13) и заданием `macos` в CI (задача 12), а рантайм ждёт живую машину |
| 6. Локальная кросс-сборка под darwin? | **Вариант А (nixpkgs) не работает** (замерено: `Csu-88`, `clang: command not found`). **Вариант В (Docker `joseluisq/rust-linux-darwin-builder` + osxcross) работает** — собран настоящий бинарник под arm64 и x86_64. SDK для ручной сборки — из `joseluisq/macosx-sdks` (26.1, 84 МиБ). Детали и грабли — в задаче 13 |

## 6. Порядок работ

Задачи 1 → 2 → 3 (в `transono`, платформенный гейт), затем 13 (**сделано**: Docker-кросс собрал настоящие Mach-O бинарники под arm64 и x86_64), затем 12 (CI: задание `macos` — то же на настоящем Darwin), затем 5 (флейк `ai-translator`), 6–10 (документация, `devices`), затем 11 (рантайм на живом Mac). Задача 0 — выполнена (числа в §0.1 и §0.2). Задача 4 проверяется в двух местах: сборка бинарников здесь и в CI.

Задачи 1–10 можно выполнять сейчас: их результат проверяется на Linux (регрессии) и по графу зависимостей. Задача 11 требует Mac и до её прохождения порт не считается принятым.

**Оценка объёма:** правки в `transono` — порядка 60 строк (манифест, два атрибута, файл заглушки `ctl/macos.rs`); в `ai-translator` — правка флейка и документация, кода перевода не касается; задача 7 — около 100 строк с тестами. Остальное — проверки.

## Задача 14. Пакетирование и релиз — выполнено 18 октября 2026, **коммит не сделан**

**Что добавлено:** `packaging/build-linux.sh` (tar.gz, `.deb` и `.rpm` через nfpm, AppImage через appimagetool), `packaging/build-macos.sh` (tar.gz и `.dmg`, ad-hoc подпись после `strip`), `packaging/nfpm.yaml`, `packaging/ai-translator.desktop`, `assets/icon.svg` + `icon.png` (256×256, метка временная), `.github/workflows/release.yml` (тег `v*` → сборки на `ubuntu-24.04` и `macos-14` → черновик релиза через `gh release create --draft`), `docs/release.md`, раздел «Releases» в README, каталоги `dist/` и `build/` в `.gitignore`.

**Замеры, на которых всё построено:**

- бинарник из Nix-окружения переносить нельзя: `readelf` показывает интерпретатор и RUNPATH в `/nix/store`;
- в обычном дистрибутиве — интерпретатор `/lib64/ld-linux-x86-64.so.2`, RUNPATH пуст, максимальный требуемый символ glibc **2.34** (сборка на `ubuntu:24.04`);
- `NEEDED`: `libpipewire-0.3.so.0`, `libasound.so.2`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`;
- **Ubuntu 22.04 не годится:** PipeWire 0.3.48, а `libspa-sys 0.10` генерирует биндинги из системных заголовков и падает на `spa_meta_first` и `spa_video_info_raw.flags`. Отсюда сборка на 24.04 (PipeWire 1.0.5) и `Depends: libpipewire-0.3-0 (>= 1.0)`;
- `.deb` реально ставится в чистый `ubuntu:24.04` (`apt-get install ./…deb` подтянул `libpipewire-0.3-0t64`), `ai-translator --help` работает;
- AppImage распаковывается и запускается (`--appimage-extract-and-run`); на хосте без PipeWire падает на `dlopen` — библиотеки намеренно не упаковываются;
- macOS: release-бинарник arm64 — 6.7 МБ после strip, внешних зависимостей нет; `.dmg` собирается только на macOS (`hdiutil`).

**Найденная и исправленная ошибка:** скрипты брали версию и архитектуру из окружения, а образ osxcross определяет свои `VERSION=1.89.0-amd64`, `TARGET=musl`, `ARCH` → артефакт назывался `ai-translator-1.89.0-amd64-aarch64-macos.tar.gz`. Теперь версия читается только из `Cargo.toml`, цель передаётся через `AIT_TARGET`, а workflow на обеих платформах сверяет тег с `Cargo.toml` и падает при расхождении.

**Проверки:** `actionlint` exit=0 (вместе с shellcheck), `yamllint` exit=0 (остались только предупреждения о длине строк), `bash -n` обоих скриптов, управляющих символов нет, в README кириллицы нет.

**Не сделано — коммит.** В `.git/objects` 41 каталог принадлежит `nobody:nogroup`, `git add` не может туда писать (`insufficient permission for adding an object`). Нужен `sudo chown -R andy:hermes .git` (или хелпер `fixr`), после чего `git add -A && git commit`. Дальше: тег `v0.1.0` и push — workflow соберёт артефакты и создаст черновик релиза.
