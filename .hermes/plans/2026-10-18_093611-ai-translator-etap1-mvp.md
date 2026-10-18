# План разработки: ai-translator, Этап 1 (MVP)

**Цель:** получить работающий каркас приложения — перевод идёт из источника через ядро на `transono`, событие нормализуется шиной и параллельно доставляется в несколько независимых текстовых приёмников, включаемых конфигурацией.

**Архитектура:** ядро `transono` (`TranslationLine<P>` + `Provider`) отдаёт поток `SessionEvent`; адаптер превращает его в `TranslationEvent` (фразовая склейка, нумерация `seq`); шина раздаёт события подписчикам через индивидуальные ограниченные каналы; каждый приёмник — отдельная задача под управлением супервизора.

**Стек:** Rust (edition 2024), Tokio, `transono` (path-зависимость), serde/serde_json, toml, clap, thiserror, ratatui + crossterm, axum (WebSocket), tokio-tungstenite.

**Порядок работ:** настоящий план покрывает Этап 1 ТЗ (`docs/TZ_ai-translator_v0.1.md`). Этапы 2–5 приведены в конце как сводка, без разбивки на задачи.

> Работа выполняется в текущей сессии, без делегирования подагентам. Команды `git add/commit/push` выполняются только по отдельной явной команде.

---

## 0. Контекст и предпосылки

### 0.1. Состояние репозитория

```
~/projects/RUST/ai-translator/
├── .git/                 (коммиты: docs; README/LICENSE/.gitignore добавлены)
├── .hermes/plans/        настоящий план
├── .gitignore            исключения проекта
├── LICENSE               указатель двойного лицензирования
├── LICENSE-APACHE        Apache License 2.0
├── LICENSE-MIT           MIT License
├── README.md             описание проекта (на английском)
└── docs/
    ├── methods.txt       исходные материалы по протоколам доставки
    └── TZ_ai-translator_v0.1.md   техническое задание
```

Кода нет. Всё, что описано ниже, создаётся с нуля. README, `.gitignore` и файлы лицензии уже готовы и в задачах 1 и 18 не создаются повторно.

### 0.2. Что уже даёт `transono` (проверено по исходникам)

| Что нужно | Где в `transono` | Как используется |
|---|---|---|
| Фабрика сессии провайдера | `core::provider::Provider` | `create_session()`, `audio_format()` |
| Провайдер синхронного перевода | `providers::openai::translation::OpenAITranslationProvider` | `::new(OpenAITranslationConfig::from_env()?)` |
| Конфиг перевода | `providers::openai::translation::OpenAITranslationConfig` | `.with_lang("ru")` — целевой язык перевода |
| Поток событий | `core::session_event::SessionEvent` | `Audio` (звук перевода), `Text` (текст перевода), `InputText` (распознавание исходной речи), `SessionStarted`, `SessionConfigured` |
| Линия перевода | `line::TranslationLine<P>` | `::new(provider, input, output, stats)`, `set_event_sender(tx)`, `run()` |
| Устройства звука | `audio::{AudioDevicesCpal, AudioInputCpal, AudioOutputCpal, LatencyStats}` | `devices.default_input()`, `::new(device, stats)` |
| Граф аудио | `runtime::{AudioLink, AudioSplitter, AudioMixer}` | раздача и подмешивание звука |
| Виртуальные устройства | `ctl::create_backend()` | Этап 4 |
| Аудиоконтейнер | `audio::Audio` | `Clone` + `Debug`, внутри `Arc` (дешёвое клонирование, без копии сэмплов); `PcmAudio` — форма для DSP, без `Debug`/`Clone` |

**Базовый провайдер — синхронного перевода.** `providers::openai::translation` работает с потоковым API перевода (`wss://api.openai.com/v1/realtime/translations`, модель `gpt-realtime-translate`): аудио говорящего уходит в сессию, обратно приходит **переведённая речь** (`session.output_audio.delta` → `SessionEvent::Audio`), **текст перевода** (`session.output_transcript.delta` → `SessionEvent::Text`) и **распознавание исходной речи** (`session.input_transcript.delta` → `SessionEvent::InputText`). Это и есть синхронный перевод: один поток, один язык-цель (`config.lang` → `session.audio.output.language`), без ожидания конца фразы. Распознавание исходной речи включается самим провайдером при `session.update` (`whisper-1`), отдельной настройки не требует.

**Провайдер Realtime (`providers::openai::realtime`) — альтернатива, не основа.** Он решает другую задачу (диалог с моделью), и у него `output_modalities` принимает либо `["audio"]`, либо `["text"]`. На Этапе 1 он не используется.

**Ключевая особенность протокола перевода: в нём нет событий завершения.** Сервер присылает только `*.delta` — ни `response.finished`, ни `completed`. Поэтому `RequestFinished`/`ResponseFinished` у этого провайдера **никогда не приходят**, и признак конца фразы (`is_final`) нельзя взять из потока: его синтезирует адаптер по паузе в поступлении дельт (задача 13). Порядковый номер `seq` назначает шина (задача 5).

### 0.3. Грабли сборки на этом хосте

- `target/` в каталоге проекта принадлежит `nobody` → сборка падает на записи. Обход без root: `export CARGO_TARGET_DIR=$HOME/.cache/ai-translator-target`.
- `bindgen`-крейты (`pipewire`, `alsa-sys`, `libspa-sys`) требуют glibc-заголовков: нужны `LIBCLANG_PATH` и `BINDGEN_EXTRA_CLANG_ARGS` (в `flake.nix` через `.cargo/config.toml`).
- `nix develop` требует, чтобы `flake.nix` был под контролем Git (проверка flakes) — файл нужно добавить в индекс.
- Тестовый бинарник может не найти `libssl.so.3` → запускать с `LD_LIBRARY_PATH` на каталог openssl из `/nix/store`.
- Инструмент записи файлов может дописать в конец исходника управляющие байты (`ESC]0;BEL`) → после правок проверять хвост файла.

### 0.4. Приоритеты (что делать первым и почему)

| Приоритет | Работа | Почему первым |
|---|---|---|
| 1 | Окружение сборки (`flake.nix`) | без него нельзя собрать ничего |
| 2 | Зависимость от `transono` + каркас модулей | проверяет главный риск: линковку с библиотекой и её системными зависимостями |
| 3 | Модель события + шина | определяет контракт между ядром и приёмниками |
| 4 | Конфигурация | без неё приёмники не включаются и не выключаются |
| 5 | Трейты и реестр приёмников | фиксирует единый интерфейс всех будущих приёмников |
| 6 | Первые приёмники: консоль, большой экран (WebSocket), файл | дают наблюдаемый результат без внешних сервисов |
| 7 | Супервизор | изоляция и перезапуск — требование приёмки |
| 8 | Адаптер событий + ядро (аудио) | подключает реальный перевод |
| 9 | Приложение и CLI, примеры конфигураций | делает всё запускаемым |
| 10 | Интеграционный тест, README, AGENTS.md | закрепляет результат |

Приёмники, требующие внешних сервисов (Telegram, титры, vMix, OBS), сознательно отложены на Этап 2: они не нужны, чтобы доказать работоспособность шины и изоляции.

---

## 1. Задачи

### Задача 1: Окружение сборки

**Результат:** `nix develop --command cargo --version` работает; переменные для bindgen и pkg-config установлены.

**Файлы:**
- Создать: `flake.nix`
- Создать: `.envrc`
- Уже есть (созданы заранее, не пересоздавать): `.gitignore`, `README.md`, `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`

**Шаг 1.** `flake.nix` — dev-shell с составом, необходимым для линковки с `transono` (pipewire, alsa-lib, openssl, clang, pkg-config, rust-analyzer):

```nix
{
  description = "ai-translator development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          name = "ai-translator-dev";

          shellHook = ''
            mkdir -p .cargo
            cat <<EOF > .cargo/config.toml
[env]
PKG_CONFIG_PATH = { value = "${pkgs.lib.makeSearchPath "lib/pkgconfig" [ pkgs.pipewire.dev pkgs.alsa-lib.dev pkgs.openssl.dev ]}", relative = false }
LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib"
BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include"
EOF
            export CARGO_TARGET_DIR="$HOME/.cache/ai-translator-target"
          '';

          nativeBuildInputs = with pkgs; [
            pkg-config clang rustc cargo rustfmt clippy rust-analyzer
          ];

          buildInputs = with pkgs; [
            glibc.dev alsa-lib pipewire openssl llvmPackages.libclang
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.alsa-lib pkgs.pipewire pkgs.openssl
          ];
        };
      });
}
```

**Шаг 2.** `.envrc` — одна строка: `use flake`.

**Шаг 3.** Проверка.

```bash
cd ~/projects/RUST/ai-translator
git add flake.nix
nix develop --command cargo --version
```

Ожидается: строка вида `cargo 1.9x.x`. Если появляется `path not tracked by Git` — повторить `git add flake.nix`.

---

### Задача 2: Крейт и зависимость от `transono`

**Результат:** пустой крейт собирается вместе с библиотекой `transono` (линковка pipewire/alsa проверена).

**Файлы:**
- Создать: `Cargo.toml`
- Создать: `src/lib.rs`
- Создать: `src/main.rs`

**Шаг 1.** `Cargo.toml`:

```toml
[package]
name = "ai-translator"
version = "0.1.0"
edition = "2024"
authors = ["Andrey Velikoredchanin"]
license = "MIT OR Apache-2.0"

[dependencies]
transono = { path = "../transono" }

tokio = { version = "1", features = ["rt-multi-thread", "macros", "signal", "sync", "time", "io-std", "io-util"] }
tokio-tungstenite = { version = "0.29", features = ["rustls-tls-webpki-roots"] }
axum = { version = "0.8", features = ["ws"] }
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.9"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
ratatui = "0.30"
crossterm = { version = "0.28", features = ["event-stream"] }
dotenvy = "0.15"
tracing = "0.1"
tracing-subscriber = "0.3"
async-trait = "0.1"
rustls = { version = "0.23", default-features = false, features = ["ring", "std"] }

[dev-dependencies]
tempfile = "3"
```

**Шаг 2.** `src/lib.rs` — только объявление модулей (на этом шаге — заглушки):

```rust
//! ai-translator: синхронный перевод речи и текста с параллельной
//! трансляцией результата в независимые приёмники.

#![warn(missing_docs)]

pub mod bus;
pub mod config;
pub mod sink;
```

**Шаг 3.** `src/main.rs`:

```rust
fn main() {
    println!("ai-translator");
}
```

**Шаг 4.** Создать минимальные заглушки, чтобы модули существовали:

- `src/bus/mod.rs` — пустой файл с `//! Шина событий перевода.`
- `src/config.rs` — пустой файл с `//! Конфигурация приложения.`
- `src/sink/mod.rs` — пустой файл с `//! Приёмники результата перевода.`

**Шаг 5.** Проверка сборки (главный риск — линковка с `transono`):

```bash
cd ~/projects/RUST/ai-translator
nix develop --command bash -c 'cargo build 2>&1 | tail -20'
```

Ожидается: `Finished`. Если падает на `pipewire`/`alsa-sys`/`libspa-sys` — проверить, что `.cargo/config.toml` создан shellHook'ом и содержит `PKG_CONFIG_PATH` и `BINDGEN_EXTRA_CLANG_ARGS`.

---

### Задача 3: Каркас модулей проекта

**Результат:** дерево модулей из ТЗ (п. 5.2) существует и компилируется.

**Файлы (создать, содержимое — только doc-комментарий и объявления подмодулей):**

```
src/app.rs
src/supervisor.rs
src/bus/event.rs
src/source/mod.rs
src/source/audio.rs
src/source/text.rs
src/source/script.rs
src/core_glue/mod.rs
src/core_glue/adapt.rs
src/core_glue/line.rs
src/tts/mod.rs
src/sink/registry.rs
src/sink/text/mod.rs
src/sink/text/console.rs
src/sink/text/websocket.rs
src/sink/text/transcript.rs
src/sink/voice/mod.rs
web/index.html
examples/.gitkeep
```

Каталог `src/core_glue/` отделён от `src/core/` (модуль `core` — имя внутреннего слоя `transono`, чтобы не путать).

**Шаг 1.** В `src/lib.rs` добавить модули: `app`, `supervisor`, `source`, `core_glue`, `tts`.
**Шаг 2.** В `src/sink/mod.rs` добавить `pub mod registry; pub mod text; pub mod voice;`.
**Шаг 3.** Проверка: `cargo build` — `Finished`, предупреждений нет.
---

### Задача 4: Модель события перевода

**Результат:** типы `LineId`, `LangPair`, `TranslationEvent` с тестами на инварианты.

**Файлы:**
- Изменить: `src/bus/event.rs`
- Изменить: `src/bus/mod.rs` (реэкспорт)

**Шаг 1. Сначала тесты** — в `src/bus/event.rs` в модуле `#[cfg(test)]`:

```rust
#[test]
fn final_events_are_marked_final() {
    let ev = TranslationEvent::Translated {
        line: LineId(0),
        langs: LangPair { source: None, target: Language::Russian },
        text: "привет".into(),
        delta: Some("привет".into()),
        is_final: true,
        seq: 7,
        at: Instant::now(),
    };
    assert!(ev.is_final());
    assert!(ev.is_durable()); // финальные события не вытесняются
}

#[test]
fn status_is_durable_and_not_final() {
    let ev = TranslationEvent::Status {
        line: LineId(0),
        state: LineState::Running,
        at: Instant::now(),
    };
    assert!(!ev.is_final());
    assert!(ev.is_durable());
}

#[test]
fn intermediate_text_is_not_durable() {
    let ev = TranslationEvent::Translated {
        line: LineId(0),
        langs: LangPair { source: None, target: Language::Russian },
        text: "при".into(),
        delta: Some("при".into()),
        is_final: false,
        seq: 3,
        at: Instant::now(),
    };
    assert!(!ev.is_durable());
}
```

**Шаг 2.** Запустить, убедиться, что не компилируется (типы не определены):

```bash
nix develop --command bash -c 'cargo test --lib 2>&1 | tail -5'
```

**Шаг 3. Реализация:**

```rust
//! Нормализованное событие перевода — контракт между ядром и приёмниками.

use std::time::Instant;

/// Идентификатор линии перевода.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineId(pub u16);

/// Язык перевода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    /// Исходный язык определяется провайдером автоматически.
    Auto,
    /// Конкретный язык (ISO-639-1).
    Code(String),
}

impl Language {
    /// Язык по коду ISO-639-1.
    pub fn code(c: &str) -> Self { Language::Code(c.to_ascii_lowercase()) }
}

/// Направление перевода.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangPair {
    /// Исходный язык, если провайдер его определил.
    pub source: Option<Language>,
    /// Целевой язык.
    pub target: Language,
}

/// Состояние линии перевода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineState {
    /// Линия создана, но ещё не запущена.
    Created,
    /// Идёт соединение с провайдером.
    Starting,
    /// Перевод идёт.
    Running,
    /// Линия остановлена штатно.
    Stopped,
    /// Линия остановлена из-за ошибки.
    Failed,
}

/// Нормализованное событие перевода.
#[derive(Debug, Clone)]
pub enum TranslationEvent {
    /// Фрагмент распознанной речи на исходном языке.
    SourceText {
        /// Линия, к которой относится событие.
        line: LineId,
        /// Накопленный текст текущей фразы.
        text: String,
        /// Приращение с прошлого события.
        delta: Option<String>,
        /// Фраза завершена.
        is_final: bool,
        /// Момент события.
        at: Instant,
    },
    /// Фрагмент перевода на целевом языке.
    Translated {
        /// Линия, к которой относится событие.
        line: LineId,
        /// Направление перевода.
        langs: LangPair,
        /// Накопленный текст текущей фразы.
        text: String,
        /// Приращение с прошлого события.
        delta: Option<String>,
        /// Фраза завершена.
        is_final: bool,
        /// Монотонный номер события перевода в пределах линии.
        seq: u64,
        /// Момент события.
        at: Instant,
    },
    /// Фрагмент звука перевода.
    TranslatedAudio {
        /// Линия, к которой относится событие.
        line: LineId,
        /// Аудиофрагмент в контейнере библиотеки (`Audio` — счётчик ссылок,
        /// дешёвое клонирование; приведение к PCM выполняет приёмник).
        audio: transono::audio::Audio,
        /// Момент события.
        at: Instant,
    },
    /// Смена состояния линии.
    Status {
        /// Линия, к которой относится событие.
        line: LineId,
        /// Новое состояние.
        state: LineState,
        /// Момент события.
        at: Instant,
    },
}

impl TranslationEvent {
    /// Фраза (или состояние) завершена и не подлежит вытеснению.
    pub fn is_final(&self) -> bool {
        match self {
            TranslationEvent::SourceText { is_final, .. }
            | TranslationEvent::Translated { is_final, .. } => *is_final,
            TranslationEvent::TranslatedAudio { .. } | TranslationEvent::Status { .. } => false,
        }
    }

    /// Событие не подлежит вытеснению при перегрузке приёмника.
    pub fn is_durable(&self) -> bool {
        self.is_final() || matches!(self, TranslationEvent::Status { .. })
    }

    /// Линия, к которой относится событие.
    pub fn line(&self) -> LineId {
        match self {
            TranslationEvent::SourceText { line, .. }
            | TranslationEvent::Translated { line, .. }
            | TranslationEvent::TranslatedAudio { line, .. }
            | TranslationEvent::Status { line, .. } => *line,
        }
    }
}
```

**Шаг 4.** `cargo test --lib` → ожидается `3 passed`.

**Шаг 5.** В `src/bus/mod.rs` добавить `pub mod event; pub use event::*;`.

---

### Задача 5: Шина событий

**Результат:** шина с веерной раздачей, изоляцией подписчиков, политикой перегрузки и счётчиком вытеснений.

**Файлы:**
- Изменить: `src/bus/mod.rs`

**Шаг 1. Тесты** (в `src/bus/mod.rs`, `#[cfg(test)]`):

```rust
#[tokio::test]
async fn all_subscribers_receive_every_event() {
    let bus = EventBus::new();
    let mut a = bus.subscribe("a", 16);
    let mut b = bus.subscribe("b", 16);
    bus.publish(translated("one", false, 1));
    assert_eq!(a.recv().await.unwrap().text_of(), "one");
    assert_eq!(b.recv().await.unwrap().text_of(), "one");
}

#[tokio::test]
async fn slow_subscriber_does_not_block_the_rest() {
    let bus = EventBus::new();
    let mut slow = bus.subscribe("slow", 2);   // никто не читает
    let mut fast = bus.subscribe("fast", 64);
    for i in 0..10 {
        bus.publish(translated("x", false, i));
    }
    // Быстрый получил все 10, медленный потерял промежуточные.
    let mut got = 0;
    while let Ok(ev) = fast.try_recv() { let _ = ev; got += 1; }
    assert_eq!(got, 10);
    assert!(bus.dropped_for("slow") >= 1);
    assert!(bus.dropped_for("fast") == 0);
}

#[tokio::test]
async fn final_events_are_never_dropped() {
    let bus = EventBus::new();
    let mut slow = bus.subscribe("slow", 1);
    bus.publish(translated("intermediate", false, 1));
    bus.publish(translated("final", true, 2));
    let first = slow.recv().await.unwrap();
    assert!(first.is_final(), "финальное событие обязано дойти");
}
```

**Шаг 2.** Запустить — тесты не компилируются.

**Шаг 3. Реализация:**

```rust
//! Шина событий перевода: веерная раздача с изоляцией подписчиков.

/// События перевода и их модель.
pub mod event;
pub use event::*;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Время ожидания отправки «непропускаемого» события в переполненный канал.
const DURABLE_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Подписчик шины.
pub struct Subscriber {
    rx: mpsc::Receiver<Arc<TranslationEvent>>,
}

impl Subscriber {
    /// Принять следующее событие.
    pub async fn recv(&mut self) -> Option<Arc<TranslationEvent>> { self.rx.recv().await }

    /// Принять событие без ожидания.
    pub fn try_recv(&mut self) -> Result<Arc<TranslationEvent>, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

struct Slot {
    tx: mpsc::Sender<Arc<TranslationEvent>>,
    dropped: Arc<AtomicU64>,
    lagging: bool,
}

/// Шина событий перевода.
pub struct EventBus {
    slots: Mutex<Vec<(String, Slot)>>,
}

impl EventBus {
    /// Создать пустую шину.
    pub fn new() -> Self { Self { slots: Mutex::new(Vec::new()) } }

    /// Подписать получателя с буфером `capacity` событий.
    pub fn subscribe(&self, id: &str, capacity: usize) -> Subscriber {
        let (tx, rx) = mpsc::channel(capacity);
        self.slots.lock().unwrap().push((
            id.to_string(),
            Slot { tx, dropped: Arc::new(AtomicU64::new(0)), lagging: false },
        ));
        Subscriber { rx }
    }

    /// Раздать событие всем подписчикам.
    ///
    /// Промежуточные события при переполнении канала отбрасываются со счётчиком;
    /// финальные и события состояния дожидаются места, но не дольше
    /// [`DURABLE_SEND_TIMEOUT`].
    pub fn publish(&self, ev: TranslationEvent) {
        let ev = Arc::new(ev);
        let mut slots = self.slots.lock().unwrap();
        for (id, slot) in slots.iter_mut() {
            match slot.tx.try_send(ev.clone()) {
                Ok(()) => { slot.lagging = false; }
                Err(mpsc::error::TrySendError::Closed(_)) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    if ev.is_durable() {
                        // Непропускаемое событие: ждём место, но не бесконечно.
                        let tx = slot.tx.clone();
                        let payload = ev.clone();
                        let dropped = slot.dropped.clone();
                        let id = id.clone();
                        tokio::spawn(async move {
                            if tokio::time::timeout(DURABLE_SEND_TIMEOUT, tx.send(payload))
                                .await
                                .is_err()
                            {
                                dropped.fetch_add(1, Ordering::Relaxed);
                                tracing::error!(subscriber = %id, "приёмник не принимает события");
                            }
                        });
                    } else {
                        slot.dropped.fetch_add(1, Ordering::Relaxed);
                        if !slot.lagging {
                            slot.lagging = true;
                            tracing::warn!(subscriber = %id, "приёмник отстаёт, промежуточные события вытесняются");
                        }
                    }
                }
            }
        }
    }

    /// Число вытесненных событий для подписчика.
    pub fn dropped_for(&self, id: &str) -> u64 {
        self.slots
            .lock()
            .unwrap()
            .iter()
            .find(|(name, _)| name == id)
            .map(|(_, slot)| slot.dropped.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Число активных подписчиков.
    pub fn subscriber_count(&self) -> usize { self.slots.lock().unwrap().len() }
}
```

В тестах нужны помощники `translated(text, final, seq)` и `TranslationEvent::text_of()` — добавить в `#[cfg(test)] mod tests` (вне тестового модуля `text_of` не нужен).

**Шаг 4.** `cargo test --lib` → все тесты шины проходят.

**Замечание по реализации.** `publish` не является `async`: он не блокирует ядро. Для непропускаемых событий ожидание вынесено в отдельную задачу, поэтому даже «залипший» приёмник не тормозит остальных.

---

### Задача 6: Конфигурация

**Результат:** разбор TOML, значения по умолчанию, отчёт о проблемах без запуска перевода.

**Файлы:**
- Изменить: `src/config.rs`

**Шаг 1. Тесты:**

```rust
#[test]
fn parses_minimal_config() {
    let cfg: Config = toml::from_str(r#"
        [core]
        target_lang = "ru"
        [[sink.text]]
        type = "console"
        enabled = true
    "#).unwrap();
    assert_eq!(cfg.sink.text.len(), 1);
    assert!(cfg.validate().is_empty());
}

#[test]
fn unknown_sink_type_is_reported_with_its_name() {
    let err = toml::from_str::<Config>(r#"
        [core]
        target_lang = "ru"
        [[sink.text]]
        type = "telegramm"
        enabled = true
    "#).unwrap_err().to_string();
    assert!(err.contains("telegramm"), "ошибка должна называть неизвестный тип: {err}");
}

#[test]
fn missing_env_var_is_reported() {
    let cfg: Config = toml::from_str(r#"
        [core]
        target_lang = "ru"
        [[sink.text]]
        type = "webhook"
        enabled = true
        url = "http://127.0.0.1:9/hook"
        body_template = "{\"text\":\"{text}\"}"
        token_env = "AI_TRANSLATOR_DEFINITELY_MISSING"
    "#).unwrap();
    let problems = cfg.validate();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].to_string().contains("AI_TRANSLATOR_DEFINITELY_MISSING"));
}

#[test]
fn disabled_sinks_are_not_validated() {
    let cfg: Config = toml::from_str(r#"
        [core]
        target_lang = "ru"
        [[sink.text]]
        type = "console"
        enabled = true
        [[sink.text]]
        type = "webhook"
        enabled = false
        url = "not-a-url"
        body_template = ""
        token_env = "AI_TRANSLATOR_DEFINITELY_MISSING"
    "#).unwrap();
    assert!(cfg.validate().is_empty());
}
```

**Шаг 2. Реализация (модель):**

```rust
//! Конфигурация приложения.

use std::path::PathBuf;
use serde::Deserialize;

/// Корневая конфигурация.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Параметры ядра перевода.
    pub core: CoreConfig,
    /// Приёмники.
    #[serde(default)]
    pub sink: Sinks,
}

/// Секция `[core]`.
#[derive(Debug, Clone, Deserialize)]
pub struct CoreConfig {
    /// Идентификатор провайдера, например `openai/translation`.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Исходный язык (`auto` — определяет провайдер).
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    /// Целевой язык.
    pub target_lang: String,
    /// Устройство захвата (`default` — устройство по умолчанию).
    #[serde(default = "default_device")]
    pub input_device: String,
    /// Голос синтеза провайдера (используется только провайдером Realtime).
    #[serde(default)]
    pub voice: Option<String>,
    /// Модель распознавания исходной речи (только для провайдера Realtime;
    /// провайдер синхронного перевода включает распознавание сам).
    #[serde(default)]
    pub transcription_model: Option<String>,
}

/// Секция `[sink]` с массивами `[[sink.text]]` и `[[sink.voice]]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Sinks {
    /// Текстовые приёмники.
    #[serde(default)]
    pub text: Vec<TextSinkConfig>,
    /// Голосовые приёмники.
    #[serde(default)]
    pub voice: Vec<VoiceSinkConfig>,
}

/// Конфигурация текстового приёмника. Имя типа задаётся полем `type`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextSinkConfig {
    /// Вывод в терминал.
    Console { /// Включён ли приёмник.
        #[serde(default = "yes")] enabled: bool },
    /// Локальный WebSocket-сервер со страницей большого экрана.
    Websocket {
        /// Включён ли приёмник.
        #[serde(default = "yes")] enabled: bool,
        /// Адрес прослушивания.
        #[serde(default = "default_bind")] bind: String,
    },
    /// Запись транскрипта и перевода в файл.
    Transcript {
        /// Включён ли приёмник.
        #[serde(default = "yes")] enabled: bool,
        /// Путь к файлу.
        path: PathBuf,
        /// Формат записи.
        #[serde(default)] format: TranscriptFormat,
        /// Писать исходный текст вместе с переводом.
        #[serde(default)] bilingual: bool,
    },
    /// Отправка через HTTP POST по шаблону тела.
    Webhook {
        /// Включён ли приёмник.
        #[serde(default = "yes")] enabled: bool,
        /// Адрес приёмника.
        url: String,
        /// Шаблон тела запроса с подстановками `{text}`, `{source_text}`,
        /// `{is_final}`, `{seq}`, `{target_lang}`.
        body_template: String,
        /// Имя переменной окружения с токеном (необязательно).
        #[serde(default)] token_env: Option<String>,
    },
}

/// Формат файла транскрипта.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptFormat {
    /// Простой текст.
    #[default]
    Txt,
    /// Субтитры SubRip.
    Srt,
    /// Субтитры WebVTT.
    Vtt,
    /// Журнал событий.
    Jsonl,
}

/// Конфигурация голосового приёмника.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceSinkConfig {
    /// Вывод в виртуальное аудиоустройство.
    VirtualSink {
        /// Включён ли приёмник.
        #[serde(default = "yes")] enabled: bool,
        /// Имя устройства.
        device: String,
        /// Громкость мониторинга оригинала.
        #[serde(default)] monitor_original: f32,
    },
}
```

**Шаг 3. Реализация (загрузка и проверка):**

```rust
/// Проблема конфигурации, обнаруженная до запуска.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    /// Ни одного включённого приёмника.
    NoEnabledSink,
    /// Не задана переменная окружения.
    MissingEnv(String),
    /// Некорректное значение.
    Invalid { field: String, reason: String },
}

impl std::fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigProblem::NoEnabledSink => write!(f, "ни один приёмник не включён"),
            ConfigProblem::MissingEnv(v) => write!(f, "не задана переменная окружения {v}"),
            ConfigProblem::Invalid { field, reason } => write!(f, "поле {field}: {reason}"),
        }
    }
}

impl Config {
    /// Прочитать конфигурацию из файла.
    pub fn load(path: &std::path::Path) -> std::io::Result<Config> {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Проверить конфигурацию без запуска перевода.
    ///
    /// Выключенные приёмники не проверяются.
    pub fn validate(&self) -> Vec<ConfigProblem> {
        let mut problems = Vec::new();
        let mut enabled = 0usize;

        for sink in &self.sink.text {
            let (on, check) = match sink {
                TextSinkConfig::Console { enabled } => (*enabled, None),
                TextSinkConfig::Websocket { enabled, bind } => (*enabled, Some(("bind", bind.clone()))),
                TextSinkConfig::Transcript { enabled, path, .. } => (*enabled, Some(("path", path.display().to_string()))),
                TextSinkConfig::Webhook { enabled, url, body_template, token_env } => {
                    if *enabled {
                        enabled += 1;
                        if !url.starts_with("http") {
                            problems.push(ConfigProblem::Invalid { field: "url".into(), reason: "должен начинаться с http".into() });
                        }
                        if body_template.is_empty() {
                            problems.push(ConfigProblem::Invalid { field: "body_template".into(), reason: "пустой шаблон".into() });
                        }
                        if let Some(var) = token_env
                            && std::env::var(var).is_err()
                        {
                            problems.push(ConfigProblem::MissingEnv(var.clone()));
                        }
                    }
                    continue;
                }
            };
            if on {
                enabled += 1;
                if let Some((field, value)) = check {
                    if value.is_empty() {
                        problems.push(ConfigProblem::Invalid { field: field.into(), reason: "пустое значение".into() });
                    }
                }
            }
        }

        for sink in &self.sink.voice {
            match sink {
                VoiceSinkConfig::VirtualSink { enabled, device, .. } => {
                    if *enabled {
                        enabled += 1;
                        if device.is_empty() {
                            problems.push(ConfigProblem::Invalid { field: "device".into(), reason: "пустое значение".into() });
                        }
                    }
                }
            }
        }

        if enabled == 0 {
            problems.push(ConfigProblem::NoEnabledSink);
        }
        problems
    }
}
```

Вспомогательные функции: `fn yes() -> bool { true }`, `fn default_provider() -> String { "openai/translation".into() }`, `fn default_source_lang() -> String { "auto".into() }`, `fn default_device() -> String { "default".into() }`, `fn default_bind() -> String { "0.0.0.0:8642".into() }`. Дополнительно к модели: методы `TextSinkConfig::enabled()` / `kind()`, `VoiceSinkConfig::enabled()` / `kind()` и `Config::enabled_sinks()`.

**Шаг 4.** `cargo test --lib` → тесты конфигурации проходят.

---

### Задача 7: Трейты и реестр приёмников

**Результат:** единый интерфейс приёмников и сборка приёмника по имени типа из конфигурации.

**Файлы:**
- Изменить: `src/sink/mod.rs`
- Изменить: `src/sink/registry.rs`

**Шаг 1. Реализация трейтов** в `src/sink/mod.rs`:

```rust
//! Приёмники результата перевода.

/// Реестр приёмников и сборка по конфигурации.
pub mod registry;
/// Текстовые приёмники.
pub mod text;
/// Голосовые приёмники.
pub mod voice;

use async_trait::async_trait;
use crate::bus::{LineId, TranslationEvent};
use transono::audio::Audio;

/// Ошибка приёмника.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// Внешняя система недоступна.
    #[error("внешний сервис недоступен: {0}")]
    Unavailable(String),
    /// Ошибка протокола.
    #[error("ошибка протокола: {0}")]
    Protocol(String),
    /// Реестру неизвестен такой тип приёмника.
    #[error("неизвестный тип приёмника: {0}")]
    UnknownType(String),
    /// Ошибка ввода-вывода.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Результат работы приёмника.
pub type Result<T> = std::result::Result<T, SinkError>;

/// Приёмник текстового результата.
#[async_trait]
pub trait TextSink: Send {
    /// Стабильный идентификатор приёмника.
    fn id(&self) -> &str;

    /// Обработать событие перевода.
    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()>;

    /// Корректно завершить работу.
    async fn shutdown(self: Box<Self>) -> Result<()>;
}

/// Приёмник звука перевода.
#[async_trait]
pub trait VoiceSink: Send {
    /// Стабильный идентификатор приёмника.
    fn id(&self) -> &str;

    /// Обработать фрагмент звука перевода.
    async fn on_audio(&mut self, line: LineId, audio: &Audio) -> Result<()>;

    /// Корректно завершить работу.
    async fn shutdown(self: Box<Self>) -> Result<()>;
}
```

**Шаг 2. Тесты реестра** в `src/sink/registry.rs`:

```rust
#[test]
fn builds_console_sink_by_type_name() {
    let cfg = TextSinkConfig::Console { enabled: true };
    let sink = build_text_sink("console-1", &cfg).unwrap();
    assert_eq!(sink.id(), "console-1");
}

#[test]
fn builds_every_configured_text_sink() {
    // console, websocket, transcript, webhook — каждый строится по своему типу
}

#[test]
fn builds_voice_sink_by_type_name() { /* virtual_sink */ }

#[test]
fn unusable_listen_address_is_reported() {
    let err = build_text_sink("screen", &websocket("не-адрес"))
        .err().expect("ожидалась ошибка сборки");
    assert!(matches!(err, SinkError::Protocol(_)));
}

#[test]
fn unknown_type_is_named_in_the_error() {
    assert_eq!(unknown_type("telegramm").to_string(), "неизвестный тип приёмника: telegramm");
}

#[tokio::test]
async fn not_yet_implemented_sinks_report_unavailability() {
    // заглушка (webhook) обязана вернуть SinkError::Unavailable, а не молчать
}
```

Конструкторы приёмников проверяют свои параметры (`bind`, `url`, `body_template`, `path`, `device`, громкость), поэтому сборка приёмника может отказать с настоящей ошибкой, а не с паникой позже. Все приёмники выводят `#[derive(Debug)]`: тесты на отказ используют `unwrap_err`/`unwrap`.

**Шаг 3. Реализация реестра:**

```rust
//! Реестр приёмников: сопоставление имени типа и конструктора.

use crate::config::{TextSinkConfig, VoiceSinkConfig};
use super::{Result, SinkError, TextSink, VoiceSink};

/// Собрать текстовый приёмник по конфигурации.
pub fn build_text_sink(id: &str, cfg: &TextSinkConfig) -> Result<Box<dyn TextSink>> {
    match cfg {
        TextSinkConfig::Console { .. } => Ok(Box::new(super::text::console::ConsoleSink::new(id))),
        TextSinkConfig::Websocket { bind, .. } => Ok(Box::new(super::text::websocket::WebsocketSink::new(id, bind)?)),
        TextSinkConfig::Transcript { path, format, bilingual, .. } => {
            Ok(Box::new(super::text::transcript::TranscriptSink::new(id, path, *format, *bilingual)?))
        }
        TextSinkConfig::Webhook { url, body_template, token_env, .. } => {
            Ok(Box::new(super::text::webhook::WebhookSink::new(id, url, body_template, token_env.as_deref())?))
        }
    }
}

/// Собрать голосовой приёмник по конфигурации.
pub fn build_voice_sink(id: &str, cfg: &VoiceSinkConfig) -> Result<Box<dyn VoiceSink>> {
    match cfg {
        VoiceSinkConfig::VirtualSink { device, monitor_original, .. } => {
            Ok(Box::new(super::voice::virtual_sink::VirtualSink::new(id, device, *monitor_original)?))
        }
    }
}

/// Ошибка сборки приёмника с указанием имени типа.
pub fn unknown_type(kind: &str) -> SinkError {
    SinkError::UnknownType(kind.to_string())
}
```

`WebhookSink` и `VirtualSink` на Этапе 1 — заглушки, возвращающие `SinkError::Unavailable` из `on_event`/`on_audio`; они появляются в Этапах 2 и 4. Создать файлы `src/sink/text/webhook.rs`, `src/sink/voice/virtual_sink.rs` с заглушками, чтобы реестр компилировался.

---

### Задача 8: Консольный приёмник

**Результат:** первый работающий приёмник; на нём проверяется трейт.

**Файлы:**
- Изменить: `src/sink/text/console.rs`

**Шаг 1. Тест:**

```rust
#[tokio::test]
async fn keeps_only_latest_revision_of_a_phrase() {
    let mut sink = ConsoleSink::new("console");
    sink.on_event(&translated("при", false, 1)).await.unwrap();
    sink.on_event(&translated("привет", false, 2)).await.unwrap();
    sink.on_event(&translated("привет мир", true, 3)).await.unwrap();
    assert_eq!(sink.phrase_count(), 1, "внутри фразы записи не размножаются");
    assert_eq!(sink.current_text(), "привет мир");
    assert!(sink.last_was_final());
}
```

**Шаг 2. Реализация:**

```rust
//! Консольный приёмник: наблюдение за переводом в терминале.

use async_trait::async_trait;
use crate::bus::TranslationEvent;
use crate::sink::{Result, TextSink};

/// Выводит перевод в стандартный вывод.
#[derive(Debug)]
pub struct ConsoleSink {
    id: String,
    phrases: usize,
    current: String,
    last_final: bool,
}

impl ConsoleSink {
    /// Создать приёмник с заданным идентификатором.
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string(), phrases: 0, current: String::new(), last_final: false }
    }

    /// Число завершённых фраз.
    pub fn phrase_count(&self) -> usize { self.phrases }

    /// Текущий текст фразы.
    pub fn current_text(&self) -> &str { &self.current }

    /// Последнее событие было финальным.
    pub fn last_was_final(&self) -> bool { self.last_final }
}

#[async_trait]
impl TextSink for ConsoleSink {
    fn id(&self) -> &str { &self.id }

    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()> {
        match ev {
            TranslationEvent::Translated { text, is_final, .. } => {
                self.current = text.clone();
                self.last_final = *is_final;
                if *is_final {
                    self.phrases += 1;
                    println!("[{}] {}", self.id, self.current);
                } else {
                    println!("[{}] {} …", self.id, self.current);
                }
            }
            TranslationEvent::SourceText { text, .. } => {
                tracing::debug!(sink = %self.id, "исходный текст: {text}");
            }
            TranslationEvent::Status { state, .. } => {
                println!("[{}] состояние: {state:?}", self.id);
            }
            TranslationEvent::TranslatedAudio { .. } => {}
        }
        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        println!("[{}] завершено, фраз: {}", self.id, self.phrases);
        Ok(())
    }
}
```

**Шаг 3.** `cargo test --lib` → тест проходит.

**Примечание.** Терминальный интерфейс (ratatui) поверх этого приёмника добавляется в задаче 15; здесь достаточно построчного вывода, чтобы приёмник был тестируемым без терминала.
---

### Задача 9: Страница большого экрана

**Результат:** готовая HTML-страница, которую будет отдавать приёмник WebSocket.

**Файлы:**
- Изменить: `web/index.html`

**Шаг 1.** Страница подключается к `ws://<host>/ws`, держит список строк, обновляет текущую фразу и «отпускает» её по `is_final`, удерживая последние три строки. Обязательные свойства: шрифт в единицах `vw` (адаптивность от ноутбука до 4К-проектора), тёмный фон, текст растёт снизу вверх, активная фраза выделена цветом, завершённые приглушены, при обрыве связи показывается статус и происходит переподключение.

```html
<!DOCTYPE html>
<html lang="ru">
<head>
  <meta charset="UTF-8">
  <title>Экран перевода</title>
  <style>
    :root { --accent: #00ff66; --max-lines: 3; }
    body { margin: 0; padding: 40px; background: #050505; color: #fff;
           font-family: Inter, system-ui, sans-serif; height: 100vh; box-sizing: border-box;
           display: flex; flex-direction: column; justify-content: flex-end; overflow: hidden; }
    #lines { display: flex; flex-direction: column; gap: 24px; max-height: 80vh; }
    .line { font-size: 4.5vw; font-weight: 600; line-height: 1.3; opacity: .4;
            word-wrap: break-word; transition: opacity .3s ease; }
    .line.active { opacity: 1; color: var(--accent); }
    .line.final  { opacity: .85; color: #fff; }
    .source { font-size: 2.2vw; opacity: .55; }
    #status { position: fixed; top: 16px; right: 24px; font-size: 1rem; opacity: .6; }
  </style>
</head>
<body>
  <div id="status">подключение…</div>
  <div id="lines"><div class="line active" id="current">Ожидание перевода…</div></div>
  <script>
    const MAX_LINES = 3;
    const lines = document.getElementById('lines');
    const status = document.getElementById('status');
    let current = document.getElementById('current');
    let currentSource = null;

    function connect() {
      const ws = new WebSocket(`ws://${location.host}/ws`);
      ws.onopen = () => { status.textContent = 'на связи'; };
      ws.onmessage = (e) => {
        const d = JSON.parse(e.data);
        current.textContent = d.text;
        if (d.source_text) {
          if (!currentSource) {
            currentSource = document.createElement('div');
            currentSource.className = 'line source';
            current.parentElement.insertBefore(currentSource, current);
          }
          currentSource.textContent = d.source_text;
        }
        if (d.is_final) {
          current.classList.remove('active');
          current.classList.add('final');
          currentSource = null;
          current = document.createElement('div');
          current.className = 'line active';
          current.textContent = '…';
          lines.appendChild(current);
          while (lines.children.length > MAX_LINES) lines.removeChild(lines.firstChild);
        }
      };
      ws.onclose = () => {
        status.textContent = 'связь потеряна, переподключение…';
        setTimeout(connect, 1500);
      };
    }
    connect();
  </script>
</body>
</html>
```

**Шаг 2.** Проверка вручную после задачи 10: `curl -s localhost:8642/ | head -3` возвращает HTML.

**Реализовано** (отличия от наброска выше):

- схема подключения выбирается по `location.protocol` (`ws:`/`wss:`), при открытии страницы как файла берётся `127.0.0.1:8642` — иначе на https-странице браузер блокирует `ws:`, а из файла `location.host` пуст;
- фраза — отдельный элемент `.phrase` (исходный текст сверху, перевод снизу), а не одиночная строка: иначе исходный текст «прилипал» бы к переводу предыдущей фразы;
- `seq` хранится **по линии** (`Map`), поэтому повторы после переподключения отбрасываются, а не переписывают текст; одна переменная на все линии ломала бы экран при двух линиях;
- финальный текст записывается **в завершаемую фразу**, и только потом фраза отпускается; в наброске было наоборот, из-за чего финал попадал в новую фразу, а завершённая оставалась с предыдущей ревизией;
- показывается состояние линии (`created`/`starting`/`running`/`stopped`/`failed`), сбой выделяется цветом;
- при длинной фразе шрифт автоматически ужимается (шаг 0.25vw, не ниже 2vw) и возвращается к базовому 4.5vw при новой фразе;
- клавиша F разворачивает страницу на весь экран, курсор прячется после 5 с простоя.

Логика страницы проверяется прогоном `web/test/page.mjs` под node (24 проверки: адрес подключения, ревизии фразы, финал, удержание трёх фраз, повторы, состояния, обрыв связи):

```
nix shell nixpkgs#nodejs -c node web/test/page.mjs
```

Прогон сразу нашёл ошибку: `lastSeq` был объявлен дважды, из-за чего скрипт не запускался вовсе.

---

### Задача 10: Приёмник «локальный WebSocket-сервер»

**Результат:** HTTP-сервер отдаёт страницу и рассылает события всем подключённым экранам.

**Файлы:**
- Изменить: `src/sink/text/websocket.rs`

**Шаг 1. Тест** (без сети — на уровне формата сообщения):

```rust
#[test]
fn message_json_contains_required_fields() {
    let msg = BroadcastMessage::from_event(&translated("привет", true, 5), None);
    let v: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
    assert_eq!(v["text"], "привет");
    assert_eq!(v["is_final"], true);
    assert_eq!(v["seq"], 5);
    assert!(v.get("source_text").is_some());
}
```

**Шаг 2. Реализация.** Сервер на `axum` с двумя маршрутами:

- `GET /` → отдаёт `web/index.html`, встроенную в бинарник через `include_str!("../../web/index.html")`;
- `GET /ws` → WebSocket: соединение подписывается на `tokio::sync::broadcast::Sender<String>`, задача соединения пересылает сообщения клиенту; входящие сообщения от клиента игнорируются (кроме закрытия).

```rust
/// Сообщение, отправляемое странице большого экрана.
#[derive(serde::Serialize)]
pub struct BroadcastMessage {
    /// Идентификатор линии.
    pub line: u16,
    /// Текст перевода.
    pub text: String,
    /// Исходный текст (для двуязычного режима).
    pub source_text: Option<String>,
    /// Фраза завершена.
    pub is_final: bool,
    /// Порядковый номер события перевода.
    pub seq: Option<u64>,
}
```

**Шаг 3. Требования к поведению:**
- сервер поднимается в `new()` и продолжает работать в фоновой задаче; привязка к адресу выполняется сразу, ошибка привязки возвращается как `SinkError::Unavailable`;
- в `on_event` событие сериализуется и отправляется в broadcast-канал; отсутствие подписчиков — не ошибка;
- `shutdown` останавливает сервер (через `CancellationToken`).
- События `SourceText` запоминаются в поле «последний исходный текст» и добавляются к сообщению перевода — так работает двуязычный режим.

**Шаг 4. Проверка вручную:**

```bash
nix develop --command bash -c 'cargo run -- run --config examples/config.dev.toml' &
curl -s localhost:8642/ | head -3
```

**Реализовано** (отличия от наброска):

- `BroadcastMessage` — enum с `#[serde(untagged)]`: сообщение перевода (`text`/`is_final`/`seq`/`source_text`) и сообщение состояния (`state`). Так на страницу уходят ровно те две формы, которые она разбирает, без пустых полей-заглушек.
- `BroadcastMessage::from_event` возвращает `Option`: события `SourceText` и `TranslatedAudio` на экран не отправляются (исходный текст приклеивается к сообщению перевода, звук идёт голосовым приёмникам).
- В модели события добавлен `LineState::as_str()` (`created`/`starting`/`running`/`stopped`/`failed`) — протокол страницы использует эти имена, а не `Debug`-вывод.
- Привязка к адресу выполняется в `new()` **синхронно** (`std::net::TcpListener`), поэтому ошибка занятого адреса возвращается сразу и `new()` работает вне рантайма; сокет переводится в tokio внутри задачи сервера. Если рантайм уже есть, сервер стартует сразу, иначе — на первом событии.
- Остановка — через `tokio::sync::watch` (без новой зависимости `tokio-util`); `shutdown` освобождает адрес.
- Вместо `bind()` приёмник отдаёт `addr()` — фактический адрес (при порте 0 его назначает система), плюс `sent()` и `last_source()`.
- Портов в тестах нет по умолчанию: проверки используют `127.0.0.1:0`, чтобы не конфликтовать с запущенным приложением.

Тесты приёмника — настоящие, через сеть: `serves_the_page_over_http` запрашивает `GET /` по TCP и проверяет, что отдана встроенная страница; `broadcasts_translation_to_a_connected_screen` и `source_text_is_attached_to_the_translation` проходят рукопожатие WebSocket вручную и читают кадр; `shutdown_stops_the_server` убеждается, что после остановки адрес больше не принимает соединения.

---

### Задача 11: Приёмник «файл транскрипта»

**Результат:** запись TXT/SRT/VTT/JSONL, корректный последний блок при остановке.

**Файлы:**
- Изменить: `src/sink/text/transcript.rs`

**Шаг 1. Тесты — сначала на чистой функции форматирования:**

```rust
#[test]
fn srt_block_has_correct_timestamps() {
    let out = format_srt_block(1, Duration::from_millis(1500), Duration::from_millis(4250), "привет мир");
    assert_eq!(out, "1\n00:00:01,500 --> 00:00:04,250\nпривет мир\n\n");
}

#[test]
fn vtt_block_uses_dot_separator() {
    let out = format_vtt_block(Duration::from_millis(1500), Duration::from_millis(4250), "привет мир");
    assert!(out.starts_with("00:00:01.500 --> 00:00:04.250"));
}

#[tokio::test]
async fn srt_file_ends_with_a_complete_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.srt");
    let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Srt, false).unwrap();
    sink.on_event(&translated("привет", true, 1)).await.unwrap();
    sink.on_event(&translated("мир", true, 2)).await.unwrap();
    Box::new(sink).shutdown().await.unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.trim_end().ends_with("мир"), "последний блок дописан: {text:?}");
}
```

**Шаг 2. Реализация.** Начало фразы запоминается при первом событии `Translated { is_final: false }` (или при первом событии с непустым текстом), конец — по событию `is_final`. Формат JSONL — по строке на событие (`serde_json::to_string`). Файл открывается в `new()` и держится открытым; запись выполняется немедленно (`flush` после каждого блока).

**Шаг 3.** `cargo test --lib` → тесты проходят.

**Реализовано** (уточнения к наброску):

- чистые функции форматирования вынесены наружу и покрыты отдельно: `format_srt_timestamp`, `format_vtt_timestamp`, `format_srt_block`, `format_vtt_block` (включая переход через час);
- `Txt` — строка на завершённую фразу (ревизии внутри фразы не размножаются), `Vtt` — файл начинается с заголовка `WEBVTT`, блоки без номеров, `Jsonl` — запись на **каждое** событие (`kind`: `source`/`translated`/`status`, время `t_ms` от старта приёмника);
- время считается от момента создания приёмника: событие несёт монотонный `Instant`, а не настенное время; для SRT/VTT это время сеанса;
- фраза, пришедшая одним событием, получает блок минимальной длины 500 мс — иначе получался бы блок нулевой длины, который не принимают проигрыватели субтитров;
- пустой текст игнорируется, двуязычный режим пишет исходный текст строкой выше перевода;
- файл открывается в `new()` с усечением (новый прогон — новый транскрипт), ошибка создания файла возвращается как `SinkError::Io` сразу при сборке приёмника; запись идёт с `flush` после каждого блока;
- запись в файл — блокирующая, но блок субтитров это сотни байт, поэтому пул потоков ради неё не заводится (отмечено в комментарии к модулю).

---

### Задача 12: Супервизор приёмников

**Результат:** запуск включённых приёмников, изоляция отказов, перезапуск по политике.

**Файлы:**
- Изменить: `src/supervisor.rs`

**Шаг 1. Тест:**

```rust
#[tokio::test]
async fn failing_sink_does_not_stop_the_others() {
    let bus = EventBus::new();
    let mut healthy = bus.subscribe("healthy", 64);
    let mut supervisor = Supervisor::new(bus.clone(), RestartPolicy::limited(2, Duration::from_millis(10)));
    supervisor.spawn("always-fails", Box::new(AlwaysFails));
    supervisor.spawn("healthy", Box::new(RecordingSink::new("healthy")));

    supervisor.publish(translated("привет", true, 1));
    let ev = tokio::time::timeout(Duration::from_secs(1), healthy.recv()).await.unwrap().unwrap();
    assert_eq!(ev.text_of(), "привет");
    assert!(supervisor.state_of("always-fails").is_error());
}

#[tokio::test]
async fn restart_is_limited() {
    // Приёмник, падающий всегда: после исчерпания попыток состояние Failed,
    // число запусков равно 1 + попытки.
}
```

**Шаг 2. Реализация:**

```rust
//! Супервизор приёмников: запуск, изоляция отказов, перезапуск.

/// Политика перезапуска упавшего приёмника.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    /// Максимальное число перезапусков.
    pub attempts: u32,
    /// Задержка перед перезапуском.
    pub delay: std::time::Duration,
}

impl RestartPolicy {
    /// Ограниченное число попыток с задержкой.
    pub fn limited(attempts: u32, delay: std::time::Duration) -> Self { Self { attempts, delay } }
}

/// Состояние приёмника, наблюдаемое извне.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkState {
    /// Приёмник работает.
    Running,
    /// Приёмник упал и будет перезапущен.
    Restarting { attempt: u32 },
    /// Приёмник отказал окончательно.
    Error(String),
    /// Приёмник остановлен штатно.
    Stopped,
}
```

Каждый приёмник получает собственного подписчика шины и собственную задачу: цикл «принять событие → `on_event`». При ошибке задача завершается, супервизор это видит (по `JoinHandle`), фиксирует `SinkState::Error` и, если попытки не исчерпаны, перезапускает приёмник после задержки — при этом фабрика приёмника вызывается заново (передать в `spawn` замыкание-конструктор `Box<dyn Fn() -> Box<dyn TextSink> + Send>`), потому что после `shutdown` приёмник непригоден.

**Шаг 3.** Требование приёмки: падение одного приёмника не меняет состояние остальных; ошибка видна в состоянии и логе.

**Реализовано** (уточнения к наброску):

- в `SinkState` добавлен вариант `Unknown`: `state_of` для незапущенного приёмника не должен выдавать `Running` или `Stopped`;
- `spawn` принимает **фабрику** (`supervisor.spawn("console", || Box::new(ConsoleSink::new("console")))`), а не готовый приёмник: после ошибки приёмник израсходован (`shutdown` потребляет его), поэтому нужен новый экземпляр. В наброске теста передавался экземпляр — при перезапуске это невозможно;
- цикл перезапуска живёт внутри задачи приёмника, а не снаружи по `JoinHandle`: супервизор наблюдает результат через разделяемое состояние. Поведение то же, кода меньше;
- **отказ и перезапуск — не одно и то же событие.** Новый экземпляр приёмника ждёт следующего события: если после сбоя перевод больше не идёт, приёмник останется в `Restarting`, а не дойдёт до `Error`. Это правильное поведение (сбой внешнего сервиса лечится следующим событием), но именно поэтому тесты публикуют по событию на каждую попытку — иначе проверка «число запусков = 1 + попытки» не воспроизводится;
- счётчик попыток считает **подряд идущие** отказы: после успешно обработанного события он сбрасывается, иначе долгоживущий приёмник рано или поздно «исчерпал» бы попытки за всю сессию;
- `spawn_voice` и внутренний `SinkKind` (текст/голос) — один цикл обслуживает оба вида приёмников, голосовому достаются только события `TranslatedAudio`;
- `shutdown(&mut self)` останавливает приёмники и **дожидается** их задач, состояния после остановки остаются читаемыми (`Stopped`/`Error`);
- наблюдаемость: `states()`, `starts_of(id)`, `state_of(id)`, `with_capacity(n)`; сбой виден в логе (`warn` при перезапуске, `error` при окончательном отказе).

Тесты супервизора: падающий приёмник не мешает исправному (второй получает все события и остаётся `Running`), ограничение попыток и повторный вызов фабрики, политика без попыток, сброс счётчика после успешного события, остановка с сохранением состояний, неизвестный приёмник.

---

### Задача 13: Адаптер событий сессии в события перевода

**Результат:** `SessionEvent` из `transono` превращается в `TranslationEvent` с корректными `is_final`, `delta` и `seq`.

**Файлы:**
- Изменить: `src/core_glue/adapt.rs`

**Шаг 1. Тесты — на скриптованной последовательности с явным временем** (провайдер перевода не присылает событий завершения, поэтому время передаётся параметром и проверяется детерминированно):

```rust
fn adapter() -> EventAdapter {
    EventAdapter::new(LineId(0), LangPair { source: None, target: Language::code("ru") })
        .with_gap(Duration::from_millis(700))
}

#[test]
fn deltas_accumulate_and_carry_increment() {
    let mut a = adapter();
    let t0 = Instant::now();
    let out: Vec<_> = vec![
        a.adapt(SessionEvent::Text("При".into()), t0),
        a.adapt(SessionEvent::Text("вет".into()), t0 + Duration::from_millis(100)),
    ].into_iter().flatten().collect();

    assert_eq!(text_of(&out[0]), "При");
    assert_eq!(delta_of(&out[0]), Some("При"));
    assert!(!out[0].is_final());
    assert_eq!(text_of(&out[1]), "Привет", "текст накапливается");
    assert_eq!(delta_of(&out[1]), Some("вет"), "delta — только приращение");
    assert_eq!(seq_of(&out[1]), 2, "seq монотонно растёт");
}

#[test]
fn pause_closes_the_phrase() {
    let mut a = adapter();
    let t0 = Instant::now();
    a.adapt(SessionEvent::Text("Привет".into()), t0);

    assert!(a.tick(t0 + Duration::from_millis(300)).is_empty(), "пауза ещё не истекла");
    let closed = a.tick(t0 + Duration::from_millis(800));
    assert_eq!(closed.len(), 1);
    assert!(closed[0].is_final(), "по паузе фраза закрывается");
    assert_eq!(text_of(&closed[0]), "Привет");
    assert_eq!(seq_of(&closed[0]), 2);

    // Следующая фраза начинается с чистого накопителя.
    let next = a.adapt(SessionEvent::Text("Мир".into()), t0 + Duration::from_millis(900));
    assert_eq!(text_of(&next[0]), "Мир");
}

#[test]
fn source_text_closes_on_its_own_pause() {
    let mut a = adapter();
    let t0 = Instant::now();
    let out = a.adapt(SessionEvent::InputText("hello".into()), t0);
    assert!(matches!(out[0], TranslationEvent::SourceText { .. }));
    let closed = a.tick(t0 + Duration::from_millis(800));
    assert!(closed.iter().any(|e| matches!(e, TranslationEvent::SourceText { is_final: true, .. })));
}

#[test]
fn audio_becomes_translated_audio_event() {
    let mut a = adapter();
    let spec = OpenAITranslationConfig::default().audio_format().spec().clone();
    let pcm = transono::audio::PcmAudio::new(spec, 240);
    let audio = transono::audio::Audio::from_pcm(&pcm).expect("тестовый фрагмент");
    let out = a.adapt(SessionEvent::Audio(audio), Instant::now());
    assert!(matches!(out[0], TranslationEvent::TranslatedAudio { .. }));
}
```

**Шаг 2. Интерфейс адаптера:**

```rust
/// Преобразует события сессии провайдера в события перевода.
pub struct EventAdapter {
    line: LineId,
    langs: LangPair,
    gap: Duration,
    seq: u64,
    translated_buf: String,
    source_buf: String,
    last_translated_at: Option<Instant>,
    last_source_at: Option<Instant>,
}

impl EventAdapter {
    /// Создать адаптер с паузой закрытия фразы по умолчанию (700 мс).
    pub fn new(line: LineId, langs: LangPair) -> Self;

    /// Задать паузу, после которой фраза считается завершённой.
    pub fn with_gap(self, gap: Duration) -> Self;

    /// Обработать событие сессии.
    pub fn adapt(&mut self, ev: SessionEvent, now: Instant) -> Vec<TranslationEvent>;

    /// Закрыть фразы, по которым истекла пауза. Вызывается по таймеру
    /// и перед обработкой каждого следующего события.
    pub fn tick(&mut self, now: Instant) -> Vec<TranslationEvent>;
}
```

**Шаг 3. Правила отображения (зафиксировать в doc-комментарии):**

| `SessionEvent` | `TranslationEvent` |
|---|---|
| `Text(delta)` | `Translated` — текст накапливается, `delta` = приращение, `is_final = false`, `seq` = следующий |
| `Audio(a)` | `TranslatedAudio` |
| `InputText(delta)` | `SourceText` — накопление, `is_final = false` |
| `SessionStarted` / `SessionConfigured` | `Status { state: Starting }` |
| `ResponseStarted` | `Status { state: Running }` |
| `RequestFinished` / `ResponseFinished` | закрыть текущую фразу немедленно (`is_final = true`) |
| пауза больше `gap` (по `tick`) | закрыть фразу перевода и фразу исходного текста (`is_final = true`) |

Два последних правила нужны, потому что провайдер перевода присылает только дельты: границу фразы определяет пауза. Правила для `ResponseFinished`/`ResponseStarted` сохраняются — их используют другие провайдеры (в том числе Realtime, если он будет подключён позже), и они не мешают потоку перевода.

Один `SessionEvent` может породить ноль, одно или несколько `TranslationEvent` — поэтому `adapt` и `tick` возвращают `Vec<TranslationEvent>`.

**Реализовано** (уточнения к наброску):

- добавлен `EventAdapter::flush(now)` — закрывает открытые фразы, не дожидаясь паузы. Нужен на конце сессии: иначе последняя фраза, начатая и не законченная, теряется вместе с накопителем;
- `adapt` **сам** закрывает просроченные фразы перед обработкой события: дельта, пришедшая после долгой тишины, начинает новую фразу, а не продолжает предыдущую. Без этого проверка паузы работала бы только по таймеру и одна длинная пауза склеивала бы две фразы;
- закрывающее событие тоже занимает номер `seq` и несёт `delta: None` — нового текста в нём нет, весь текст уже пришёл ревизиями;
- добавлены `gap()`, `line()`, `is_open()`;
- мост «сессия → шина» (`EventBridge`) живёт в `src/core_glue/line.rs` вместе с остальной обвязкой линии: период таймера = `gap / 4`, но не меньше 10 мс; при закрытии канала сессии мост делает `flush`, поэтому незаконченная фраза доходит до приёмников.

**Шаг 4.** Мост «сессия → шина»: `TranslationLine::set_event_sender` принимает `tokio::sync::mpsc::UnboundedSender<SessionEvent>`; задача-мост читает канал, прогоняет события через `EventAdapter` и вызывает `bus.publish`. Там же живёт таймер: `tokio::time::interval(gap / 4)` вызывает `adapter.tick(Instant::now())` и публикует закрытые фразы — иначе последняя фраза потока не закроется, когда новые дельты перестанут приходить.

---

### Задача 14: Источники

**Результат:** аудиоисточник (реальный перевод) и скриптованный источник (для тестов и отладки без сети).

**Файлы:**
- Изменить: `src/source/audio.rs`
- Изменить: `src/source/script.rs`
- Изменить: `src/source/text.rs`

**Шаг 1. `src/source/script.rs`** — источник фиксированной последовательности событий; используется интеграционным тестом и режимом `--script`:

```rust
/// Источник, проигрывающий заранее заданную последовательность событий.
pub struct ScriptSource {
    events: Vec<TranslationEvent>,
    delay: Duration,
}
```

**Шаг 2. `src/source/audio.rs`** — обвязка провайдера синхронного перевода по образцу `transono/src/bin/transono.rs`:

```rust
rustls::crypto::ring::default_provider().install_default()
    .map_err(|_| CoreError::Internal("rustls provider".into()))?;

let devices = AudioDevicesCpal::new();
let capture = devices.default_input()?;
let playback = devices.default_output()?;

let stats = Arc::new(LatencyStats::default());
let input  = AudioInputCpal::new(capture,  stats.clone())?;
let output = AudioOutputCpal::new(playback, stats.clone())?;

// Провайдер синхронного перевода: аудио → переведённая речь + текст перевода
// + распознавание исходной речи, в одном соединении.
let mut config = OpenAITranslationConfig::from_env()?;  // читает OPENAI_API_KEY
config.with_lang(&cfg.core.target_lang);                 // целевой язык перевода

let provider = OpenAITranslationProvider::new(config);
let mut line = TranslationLine::new(provider, Box::new(input), Box::new(output), stats).await?;
line.set_event_sender(session_tx);                       // мост в шину (задача 13)
let handle = line.run().await?;
```

Замечания по провайдеру:

- распознавание исходной речи (`SessionEvent::InputText`) включается самим провайдером при `session.update` (модель `whisper-1`) — настраивать не нужно;
- `source_lang` в конфигурации приложения провайдеру не передаётся: язык источника определяется автоматически, у API перевода нет поля для него. Поле остаётся в конфигурации для будущих провайдеров и для подстановки в шаблоны сообщений;
- поля `voice` и `transcription_model` относятся к провайдеру Realtime и на Этапе 1 не используются — в документации конфигурации это помечается.

Провайдер выбирается по `cfg.core.provider` (`"openai/translation"` по умолчанию); неизвестный провайдер — ошибка запуска с его именем.

**Шаг 3. Текстовый источник.** `OpenAITranslationProvider` принимает только аудио: единственные клиентские команды — `session.update` и `session.input_audio_buffer.append` (`src/providers/openai/translation/commands.rs`), поля для текстового входа в протоколе нет. Поэтому на Этапе 1:

- `source/text.rs` реализуется как источник для шины **без обращения к провайдеру**: текст подаётся в шину как `Translated`/`SourceText` (режим `script`, отладка, интеграционный тест);
- полноценный текстовый перевод (текст → провайдер → перевод) переносится в Этап 2 и выполняется отдельным провайдером текстового перевода.

Решение зафиксировать в `docs/decisions.md` как ADR: «текстовый перевод не идёт через провайдер синхронного перевода речи».

**Шаг 4.** Проверка: `cargo run -- run --config examples/config.dev.toml` с реальным ключом → консольный приёмник печатает перевод.

**Реализовано** (уточнения к наброску):

- `rustls::crypto::ring::default_provider().install_default()` — результат **не** превращается в ошибку: этот вызов не удаётся только тогда, когда провайдер уже установлен, а это не беда;
- устройства открываются прямо в `start()`: вспомогательная функция заставляла бы называть тип `cpal::Device`, а он в `transono` не переэкспортирован. Имя устройства из конфигурации ищется через `find_input`, значение `default` берёт системное устройство по умолчанию;
- `AudioSource` держит `TranslationLine<OpenAITranslationProvider>` и **сигнал остановки моста**. Ждать закрытия канала событий нельзя: линия держит отправителя у себя (`event_tx` — её поле), канал сам не закроется, и последняя фраза не была бы дописана. Поэтому `EventBridge` получил `stopper()` (`watch`-канал), а `AudioSource::stop` после `line.stop()` шлёт сигнал и ждёт мост не дольше 500 мс;
- у моста теперь два пути завершения — конец канала сессии и сигнал остановки; оба дописывают открытую фразу;
- ошибки запуска разделены по причине: `UnknownProvider` (с именем провайдера), `Device`, `Provider` (сюда попадает и отсутствие `OPENAI_API_KEY` — текст ошибки провайдера сохраняется), `Line`. Всё это видно **до** захвата звука;
- `script_from_text` подаёт каждую фразу по словам: приёмники видят растущую фразу и её финальную ревизию — то же, что и в живой сессии. Знак конца предложения остаётся при своей фразе (`Привет мир.`), `delta` — приращение к предыдущему тексту;
- `split_phrases` режет по `.`, `!`, `?`, `…` и переводу строки, пустые фразы отбрасываются;
- ADR про текстовый источник заведён в `docs/decisions.md` (ADR-001).

**Проверка без ключа:** `AudioSource::start` с провайдером `openai/translation` и без `OPENAI_API_KEY` возвращает `SourceError::Provider` с текстом провайдера — падения нет, и это проверено тестом.
---

### Задача 15: Приложение и командный интерфейс

**Результат:** `ai-translator run|check|script` — запуск из конфигурации, проверка конфигурации, прогон без сети.

**Файлы:**
- Изменить: `src/app.rs`
- Изменить: `src/main.rs`

**Шаг 1. CLI (clap):**

```
ai-translator check  --config <файл>          # проверить конфигурацию, не запуская перевод
ai-translator run    --config <файл>          # запустить перевод
ai-translator script --config <файл> --text "…"   # прогнать заданный текст через шину
```

**Шаг 2. `App` — сборка системы:**

```rust
/// Собранное приложение: шина, супервизор, ядро.
pub struct App {
    bus: EventBus,
    supervisor: Supervisor,
    core: Option<CoreHandle>,
}

impl App {
    /// Собрать приложение по конфигурации.
    pub fn build(cfg: &Config) -> Result<Self, AppError> { /* … */ }

    /// Запустить приёмники и ядро.
    pub async fn run(self) -> Result<(), AppError> { /* … */ }

    /// Остановить всё: фразы дописаны, соединения закрыты, файлы сохранены.
    pub async fn shutdown(self) -> Result<(), AppError> { /* … */ }
}
```

Порядок сборки строго такой: (1) шина → (2) супервизор с приёмниками → (3) ядро/источник. Приёмники должны быть подписаны до первых событий.

**Шаг 3. Обработка сигналов:** `tokio::signal::ctrl_c` → `app.shutdown()`; второй сигнал — немедленный выход.

**Шаг 4.** Проверка:

```bash
nix develop --command bash -c 'cargo run -- check --config examples/config.dev.toml'
```

Ожидается: список проблем либо «конфигурация корректна».

**Реализовано** (уточнения к наброску):

- **фабрика приёмника теперь возвращает `Result`**: занятый адрес или недоступный путь — это отказ приёмника, а не повод для паники внутри задачи. Такой отказ проходит через ту же политику перезапуска и виден в состоянии (`SinkState::Error`), а `starts_of` неудачную сборку запуском не считает;
- `App::build` собирает каждый включённый приёмник один раз — как проверку, — чтобы занятый адрес и недоступный файл были видны до запуска, а не после первой фразы. **Уточнено в задаче 16:** проверочная сборка оказалась лишней и вредной (приёмник занимает ресурс при сборке), теперь построенный приёмник передаётся супервизору как первый экземпляр;
- **супервизор дописывает очередь при остановке.** Раньше остановка выходила из цикла сразу, теряя события, уже доставленные в очередь приёмника. Теперь на сигнале остановки приёмник сначала получает всё, что в очереди (`drain`), и только потом закрывается. Это видно на реальном прогоне: последняя фраза `script`-режима печатается уже во время остановки;
- идентификаторы приёмников выдаются по типу: первый — `console`, второй того же типа — `console-2`. Идентификаторы печатаются по алфавиту (порядок в словаре был произвольным);
- `--config` обязателен для всех трёх команд: молчаливый выбор файла по умолчанию скрывал бы опечатку в пути;
- логи идут в **stderr** (`RUST_LOG`, по умолчанию `info`), перевод — в stdout: консольный приёмник и диагностика не смешиваются;
- `.env` читается автоматически (`dotenvy`), поэтому ключ провайдера можно держать рядом с конфигурацией;
- `check` возвращает ненулевой код, если конфигурация не готова, — пригодно для скриптов;
- `App::publish_text` — вход в `script`-режим: текст идёт в шину через `TextSource`, провайдер не поднимается.

**Проверено вживую** (без сети и ключа, конфигурация во временном файле):

- `check` на исправной конфигурации → «конфигурация корректна», код 0;
- `check` на пустом `target_lang` → две проблемы (пустое поле и «ни один приёмник не включён»), код 1;
- `check` на отсутствующем файле → внятное сообщение, код 1;
- `script --text "Привет, мир. Это проверка шины. Работает."` → консоль напечатала растущие фразы и их финальные ревизии, в SRT-транскрипте три фразы с временами 0.0–2.0 с, код 0;
- `run` без `OPENAI_API_KEY` → «провайдер перевода не принял сессию: … OPENAI_API_KEY environment variable required!», код 1, до захвата звука.

---

### Задача 16: Примеры конфигураций

**Файлы:**
- Создать: `examples/config.dev.toml`
- Создать: `examples/config.stream.toml`
- Создать: `examples/config.offline.toml`

**Шаг 1.** `examples/config.dev.toml` — минимум для разработки:

```toml
[core]
provider = "openai/translation"
source_lang = "auto"
target_lang = "ru"

[[sink.text]]
type = "console"
enabled = true

[[sink.text]]
type = "websocket"
enabled = true
bind = "0.0.0.0:8642"

[[sink.text]]
type = "transcript"
enabled = true
path = "transcript.srt"
format = "srt"
bilingual = true
```

**Шаг 2.** `config.stream.toml` — приёмники для трансляции (вебхук-приёмник, титры — Этап 2, здесь как заготовка с `enabled = false`).
**Шаг 3.** `config.offline.toml` — только консоль и файл, без сети (для проверки шины).

**Реализовано** (уточнения к наброску):

- **порт изменён с 8642 на 8643**: 8642 на стенде занят посторонним сервисом. Проверка «занят адрес» при этом сработала как задумано — приёмник не собрался, приложение сказало об этом и вышло с кодом 1, до всякого перевода;
- **найдена и исправлена ошибка сборки приёмников.** Проверка конфигурации разработки вскрыла, что приёмник собирался дважды: сначала проверочно, потом супервизором. Приёмник занимает ресурс при сборке (слушающий сокет, открытый файл), поэтому вторая сборка упиралась в «Address already in use» — проверка адреса сама ломала запуск. Теперь `App::build` собирает приёмник **один раз** и передаёт готовый супервизору (`spawn_ready` / `spawn_voice_ready`); фабрика вызывается только при перезапуске. Это уточнение к задаче 15, где приёмник собирался дважды;
- `config.dev.toml` — минимум для разработки: консоль, экран и двуязычные субтитры рядом с проектом;
- `config.stream.toml` — трансляция: экран, субтитры, вебхук во внешнюю систему (выключен, приёмник Этапа 2) и виртуальное звуковое устройство (выключено, заготовка Этапа 2). Выключенные приёмники остаются в файле как описание того, куда пойдёт перевод;
- `config.offline.toml` — консоль и файл, без сети; в комментарии указана команда `script`;
- комментарии в примерах — на русском: это пользовательская документация, а не код.

**Проверено вживую:**

- `check` на всех трёх примерах → «конфигурация корректна», код 0;
- `script` на `config.offline.toml` → консоль и JSONL-транскрипт, код 0; созданный `offline.jsonl` в репозиторий не попадает (правила `.gitignore`);
- `script` на `config.dev.toml` → все три приёмника поднялись (консоль, субтитры, экран), код 0; во время прогона `http://127.0.0.1:8643/` отдаёт страницу экрана (HTTP 200, 7007 байт, заголовок «Экран перевода»);
- длинный прогон на `config.dev.toml`: 30 фраз, 180 событий, все дошли до консоли, потерь нет.

---

### Задача 17: Интеграционный тест

**Результат:** сквозной путь «источник → шина → два приёмника» проверен без сети.

**Файлы:**
- Создать: `tests/pipeline.rs`

**Шаг 1. Тест:**

```rust
#[tokio::test]
async fn text_source_reaches_two_text_sinks_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.srt");

    let bus = EventBus::new();
    let mut supervisor = Supervisor::new(bus.clone(), RestartPolicy::limited(1, Duration::from_millis(5)));
    supervisor.spawn("console", Box::new(ConsoleSink::new("console")));
    supervisor.spawn("file", Box::new(TranscriptSink::new("file", &path, TranscriptFormat::Srt, true).unwrap()));

    let source = ScriptSource::new(vec![
        translated("Привет,", false, 1),
        translated("Привет, это тест.", true, 2),
    ], Duration::ZERO);
    source.run(bus.clone()).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    supervisor.shutdown().await.unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Привет, это тест."), "файловый приёмник получил событие: {text:?}");
}
```

**Шаг 2.** Запуск: `nix develop --command bash -c 'cargo test --test pipeline'` → `1 passed`.

**Шаг 3. Второй тест** — изоляция: один приёмник всегда падает, второй получает все события (дублирует проверку задачи 12, но на уровне полной сборки `App`).

**Реализовано** (уточнения к наброску):

- код наброска писался до задач 12–15 и текущему API не соответствует: `spawn` принимает фабрику, возвращающую `Result`, а не готовый приёмник; у `ScriptSource` задержка задаётся `with_delay`; `run` принимает `&EventBus`; `shutdown` возвращает `()`. Тест переписан под фактический API;
- три теста вместо двух: (1) ручная сборка — скриптованный источник, консоль и файловый приёмник, проверка содержимого и разметки SRT; (2) тот же путь через собранное приложение (`publish_text`, как это делает команда `script`) — 4 события, 4 строки JSONL; (3) изоляция на уровне `App`: рядом с консолью включён приёмник вебхука (заготовка Этапа 2, всегда отвечает «недоступен») — консоль остаётся в `Running`, вебхук уходит в `Error` с сохранённой причиной;
- для наблюдаемости добавлены `App::sink_state` и `App::sink_states` (сортированные по идентификатору); `sink_ids` теперь выражается через них;
- **моя ошибка в ожидании теста:** текст «Первое. Второе.» даёт по одному событию на фразу (в фразе одно слово), а не по два; тестовый текст теперь по два слова на фразу, и проверка «4 события» верна.

**Проверено:** `cargo test --test pipeline` → 3 passed; полный прогон — 102 модульных + 3 интеграционных + 3 doctest, clippy с `-D warnings` и `cargo fmt --check` чисто.

---

### Задача 18: Документация проекта

**Файлы:**
- Поддерживать: `README.md` (уже создан на английском: назначение, схема, таблица приёмников, пример конфигурации, сборка и запуск)
- Создать: `AGENTS.md`

**Шаг 1.** `README.md` — по ходу работ дополнять: привести таблицу приёмников в соответствие с реализованным, добавить реальные команды запуска и актуальный список зависимостей. Язык — английский.
**Шаг 2.** `AGENTS.md`: правила работы с кодом — без `unwrap()`/`panic!` в рабочем коде, `Result` проекта, без аллокаций на аудиопотоке, doc-комментарии на публичных интерфейсах, `clippy` без предупреждений, общение на русском.

---

### Задача 19: Прогон критериев приёмки Этапа 1

**Результат:** проверены пункты ТЗ, достижимые на Этапе 1.

| Пункт ТЗ | Как проверяется |
|---|---|
| 1. Запуск одной командой из `flake.nix` | `nix develop --command cargo run -- run --config examples/config.dev.toml` |
| 2. Три текстовых приёмника параллельно | `examples/config.dev.toml` + задача 17 |
| 3. Выключенный приёмник не работает | убрать `enabled = true` → в логе нет его идентификатора, соединений нет |
| 4. Отказ приёмника не роняет ядро | тест задачи 12 и второй тест задачи 17 |
| 5. Медленный приёмник не задерживает остальных | тест `slow_subscriber_does_not_block_the_rest` (задача 5) |
| 10. Корректная остановка | `Ctrl+C` → в консоли «завершено», файл SRT дописан |
| 11. Проверка конфигурации | `cargo run -- check` на файле с ошибкой |
| 12. `clippy` и тесты | `cargo clippy -- -D warnings`, `cargo test` |

Пункты 6–9 (Telegram, титры, виртуальное аудиоустройство) относятся к Этапам 2 и 4 и на Этапе 1 не проверяются.

---

## 2. Файлы, которые появятся

| Путь | Назначение |
|---|---|
| `flake.nix`, `.envrc` | окружение сборки |
| `README.md`, `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`, `.gitignore` | уже созданы |
| `AGENTS.md` | правила работы с кодом |
| `Cargo.toml` | зависимости |
| `src/lib.rs`, `src/main.rs` | вход |
| `src/app.rs` | сборка и жизненный цикл системы |
| `src/supervisor.rs` | запуск, изоляция, перезапуск приёмников |
| `src/config.rs` | конфигурация и её проверка |
| `src/bus/mod.rs`, `src/bus/event.rs` | шина и модель события |
| `src/core_glue/adapt.rs` | адаптер `SessionEvent` → `TranslationEvent` |
| `src/core_glue/line.rs` | обвязка `TranslationLine` |
| `src/source/{audio,script,text}.rs` | источники |
| `src/sink/mod.rs`, `src/sink/registry.rs` | трейты и реестр |
| `src/sink/text/{console,websocket,transcript,webhook}.rs` | текстовые приёмники |
| `src/sink/voice/{mod,virtual_sink}.rs` | голосовые приёмники (заглушки) |
| `src/tts/mod.rs` | синтез речи (опционально, заглушка) |
| `web/index.html` | страница большого экрана |
| `examples/config.*.toml` | примеры конфигураций |
| `tests/pipeline.rs` | интеграционный тест |

## 3. Проверка и тесты

Обязательные команды на каждом шаге:

```bash
cd ~/projects/RUST/ai-translator
export CARGO_TARGET_DIR=$HOME/.cache/ai-translator-target

nix develop --command bash -c 'cargo test --lib'                 # модульные тесты
nix develop --command bash -c 'cargo test --test pipeline'       # интеграционный тест
nix develop --command bash -c 'cargo clippy -- -D warnings'      # чистота кода
nix develop --command bash -c 'cargo fmt --check'                # форматирование
```

Если тестовый бинарник не находит `libssl.so.3`:

```bash
SSL=$(dirname $(find /nix/store -name "libssl.so.3" 2>/dev/null | head -1))
nix develop --command bash -c "LD_LIBRARY_PATH=$SSL:\$LD_LIBRARY_PATH cargo test"
```

## 4. Риски и открытые вопросы

| Риск / вопрос | Влияние | Что делать |
|---|---|---|
| В протоколе перевода нет событий завершения фразы (только `*.delta`) | приёмники, обновляющие одну запись, не знают, когда её «отпускать» | граница фразы синтезируется адаптером по паузе (задача 13); значение `gap` настраивается |
| Текстовый вход у `OpenAITranslationProvider` не поддерживается (только `input_audio_buffer.append`) | текстовый источник на Этапе 1 не переводится провайдером | источник подаёт текст прямо в шину (задача 14, шаг 3); текстовый перевод — Этап 2 |
| Провайдер Realtime отдаёт текст и звук только в режиме `["audio"]` | иной набор событий, чем ожидалось | на Этапе 1 Realtime не используется; факт учтён для будущего подключения |
| `bindgen`/`pipewire` не собираются в новом flake | блокирует всё | `PKG_CONFIG_PATH`, `LIBCLANG_PATH`, `BINDGEN_EXTRA_CLANG_ARGS` в `.cargo/config.toml` (задача 1) |
| `target/` в каталоге проекта принадлежит `nobody` | сборка падает на записи | `CARGO_TARGET_DIR` в `$HOME/.cache`; root-действия не применять |
| Медленный приёмник теряет промежуточные события | расхождение текста на разных экранах | текст накопительный: последнее событие несёт полный текст фразы; потери считаются и логируются |
| Ключ `OPENAI_API_KEY` | без него нельзя проверить сквозной путь | режим `script`/`offline` проверяет всё, кроме обращения к провайдеру |

**Открытые вопросы:** значение паузы закрытия фразы (`gap`, по умолчанию 700 мс) стоит уточнить на реальной речи — оно влияет на то, как часто приёмники начинают новую запись. Провайдер на Этапе 1 определён: `openai/translation`.

## 5. Дальнейшие этапы (сводка, без разбивки)

| Этап | Содержание | Готовый результат |
|---|---|---|
| 2 | HTTP-титры трансляций, Telegram (буфер + обновление), вебхуки Discord/Slack/JSON, текстовый источник | перевод параллельно на экран, в Telegram, в титры |
| 3 | vMix (HTTP + TCP), OBS WebSocket, расширенный файловый приёмник | интеграция с видеомикшерами |
| 4 | Раздача аудиопакетов голосового перевода, виртуальное аудиоустройство с мониторингом оригинала, Icecast, опциональный TTS | голос перевода в программах звонков и по ссылке |
| 5 | WebRTC (медиасервер), SRT/RTMP, голосовой чат Telegram (MTProto), NDI | полный охват способов доставки |

## 6. Что делать прямо сейчас

Первые три задачи дают проверяемый результат и снимают главный технический риск:

1. `flake.nix` + `.envrc` + `.gitignore` → `nix develop --command cargo --version` (задача 1).
2. `Cargo.toml` с зависимостью `transono` и каркас модулей → `cargo build` проходит, линковка с pipewire/alsa проверена (задачи 2–3).
3. Модель события и шина → тесты раздачи, изоляции и вытеснения проходят (задачи 4–5).

Дальше — конфигурация, трейты, консольный приёмник, экран и файл: к концу задач 1–11 приложение уже запускается и параллельно отдаёт перевод в три приёмника без обращения к внешним сервисам.
