# AI-TRANSLATOR

**Synchronous speech and text translator with parallel delivery to arbitrary sinks.**

AI-TRANSLATOR listens to speech (or takes text), obtains a translation into a target
language, and delivers the result to any number of independent sinks **at the same time** —
text sinks (captions, on-screen displays, files) and voice sinks (translated speech routed to
virtual audio devices, network audio streams).

The defining property of the project: **the translation core does not know where its result
goes.** The core produces a stream of translation events; delivery is performed by isolated
sink modules that can be enabled and disabled individually. One and the same translation can
go to a stage screen, broadcast captions and a voice channel simultaneously.

## Status

Stage 1 (MVP) is implemented and covered by tests: the event bus, the sink subsystem, the
console, WebSocket, transcript and Telegram sinks, the supervisor, the audio source on the synchronous
translation provider, the offline text and scripted sources, and the command-line interface.
Interfaces may still change before the 1.0 release.

## Platforms

Linux is the primary platform: the Nix flake provides the development shell, `transono`
drives PipeWire and ALSA, and virtual audio devices are created through its PipeWire module.

macOS is supported for building and running: the project builds with Xcode Command Line Tools
and a rustup toolchain, without Nix; audio capture and playback go through CoreAudio via
`cpal`, and the PipeWire module of `transono` is not compiled at all. The voice sink is
unavailable there — a virtual audio device is a third-party driver the user installs — and the
microphone permission is asked from the terminal. Linux, macOS and Windows are built and tested
by CI on every push. Release packages are made for Apple Silicon only: the media engine has no
build for Intel Macs, and a package without the voice-chat sink is not published.

Windows x86_64 is supported for building and running: audio capture and playback go through
WASAPI via `cpal`, the media engine of the voice-chat sink exists for `x86_64-pc-windows-msvc`,
and the release is a portable ZIP with `ai-translator.exe` and `ntgcalls.dll`. A virtual audio
device there is a third-party driver (VB-Cable, VoiceMeeter), so `virtual_sink` is refused by
the configuration check instead of pretending to work. Windows arm64 has no media engine and is
not shipped.

## Architecture

```
Sources
  microphone / system audio            text input
        │                                   │
        ▼                                   ▼
Translation core  ──  transono::TranslationLine<P> + Provider
  speech recognition → translation → translated text + translated speech
        │  SessionEvent
        ▼
Translation event bus
  normalization into TranslationEvent, monotonic seq, timestamps
  fan-out to subscribers with isolation and an overflow policy
        │
        ├──────────────► Text sinks   (any number, in parallel)
        └──────────────► Voice sinks  (any number, in parallel)
```

Layers know only about their neighbour: the core knows nothing about sinks, sinks know
nothing about each other, and the bus knows nothing about delivery protocols.

Assembly order is fixed and matters: the bus first, then the sinks (so that they are
subscribed before the first event exists), and only then the source. Each sink gets its own
queue and its own task, so a slow or failing sink cannot delay the others.

### Translation event

The unit exchanged between the core and the sinks is a normalized, immutable event:

| Variant | Meaning |
|---|---|
| `SourceText` | fragment of recognized source speech |
| `Translated` | fragment of translated text, with `delta`, `is_final` and a monotonic `seq` |
| `TranslatedAudio` | fragment of translated speech, as `transono::audio::Audio` |
| `Status` | line state change (created, starting, running, stopped, failed) |

`is_final` marks the end of a phrase: sinks that update a single record (a screen, a caption
line) use it to release the current record and start the next one. `seq` is a strictly
increasing counter for protocols that require one.

The synchronous translation protocol has no event that says "the phrase is complete" — it
sends deltas only. The phrase boundary is therefore synthesized from a pause between deltas
(700 ms by default), and a timer closes the last phrase even when no new deltas arrive.

## Sinks

Implemented on stage 1:

| Sink | Purpose |
|---|---|
| `console` | terminal output for observation and debugging |
| `websocket` | local HTTP server plus a full-screen page for a stage display or OBS Browser Source |
| `transcript` | recording to TXT, SRT, VTT or JSONL, optionally bilingual |
| `telegram` | publishing the translation into the configured channel through a bot, one message per phrase, edited as the phrase grows, within the limits of 20 new messages a minute and one edit a second |

Declared in the configuration and reserved for the next stages:

| Sink | Purpose |
|---|---|
| `webhook` | Discord, Slack or an arbitrary JSON body template |
| `virtual_sink` | virtual audio device (OBS, Discord, Zoom, vMix) with optional monitoring of the original voice |

A sink that is not ready says so instead of silently swallowing the translation: a disabled
sink is never started, and a sink that cannot deliver reports an error, which the supervisor
records and acts on. A sink that fails is restarted with a bounded number of attempts and
does not affect the other sinks.

On macOS and Windows a virtual audio device is not part of the system audio stack but a
third-party driver the user installs (BlackHole or Loopback on macOS, VB-Cable or VoiceMeeter on
Windows); the application never installs a driver itself. The `virtual_sink` is still a stub on
every platform, so the configuration check refuses an enabled one with an explanation instead of
letting the supervisor restart it — see `docs/decisions.md`, ADR-002.

Translated speech comes from the AI provider itself as part of the same session that produces
the translated text. Speech synthesis (TTS) is an optional additional source of audio, used
only when a provider or a source does not produce speech of its own.

### Live broadcast into a Telegram voice chat

A group can carry the translation as live audio inside its voice chat. That path is not the
Bot API: a bot cannot join a voice chat at all, so the broadcast runs as a user account with
`api_id`, `api_hash` and a saved session.

```toml
[[sink.voice]]
type = "telegram_call"
chat_id = "-1001234567890"
api_id_env = "TELEGRAM_API_ID"
api_hash_env = "TELEGRAM_API_HASH"
session_file = "~/.local/state/ai-translator/telegram-user.session"
create_if_missing = true
```

The account signs in once with `ai-translator telegram-login`; the session file is a full
access credential and is treated as a secret. Speech is fed to the call frame by frame from
the same pipeline that feeds the text sinks, so the group hears the translation as it is
spoken.

The transport is a native media engine (`libntgcalls`, the one behind pytgcalls), so the sink is
a build feature, `telegram-call`. Every package is built with it: Linux x86_64, macOS on Apple
Silicon and Windows x86_64 carry the engine beside the binary, and there is no switch to build one
without it. A plain `cargo build` leaves the feature off, so the program compiles without
downloading the engine; such a build still parses the sink and refuses an enabled one with the
reason, instead of dropping it silently.

## Configuration

One TOML file describes the core and every sink. Each sink has a `type` and an `enabled`
flag; secrets are referenced through environment variables, never stored in the file.

Every field has a default, so a configuration may name only what differs from it:

```toml
[core]
target_lang = "ru"
```

That file translates into Russian, recognises the source language, uses the system
input and output devices, and prints the result in the terminal. An empty file is
also valid and means the same in English. The defaults are: `provider =
"openai/translation"`, `source_lang = "auto"`, `target_lang = "en"`,
`input_device = "default"`, `output_device = "default"`, no `[sink]` section means a
console sink, a screen listens on `127.0.0.1:8642`, a transcript without a `path` is
written to `transcript.<format>`, and the Telegram sinks keep their limit defaults.

A field that is misspelt is a parse error, not a silent fallback to its default: the
diagnostic names the field and lists the accepted ones. The full list of parameters
with comments is `examples/config.all.toml`; the shortest one is
`examples/config.minimal.toml`.

A provider session does not live forever: OpenAI closes one after an hour, and the
translation used to stop there until the program was restarted. It now replaces the
session itself — before the limit, at 55 minutes by default
(`[core] session_reconnect_minutes`, `0` turns the timer off and leaves only the
reconnect after an actual end), or as soon as a session ends for any other reason:
a broken network, a closed socket, an idle timeout. The sinks, the transcript and the
screen keep running across the change; the screen says «переподключение…» while it
happens, and the log records the gap. A wrong key or an exhausted quota is not retried
every second: the program reports the failure and then tries rarely, once a minute.

```toml
[core]
provider = "openai/translation"
source_lang = "auto"
target_lang = "ru"
input_device = "default"
output_device = "default"

[[sink.text]]
type = "console"
enabled = true

[[sink.text]]
type = "websocket"
enabled = true
# The screen listens on 127.0.0.1 unless you say otherwise. A hall needs
# 0.0.0.0 to be reachable from another machine; `check` then warns that anyone
# on the network can read the translation and the settings.
bind = "0.0.0.0:8643"
# Translation size in percent of the screen width, and the lower bound it
# shrinks to when a long phrase does not fit. The size stays fixed while the
# translation runs.
line_size_vw = 4.5
min_line_size_vw = 2.0

[[sink.text]]
type = "transcript"
enabled = true
path = "transcript.srt"
format = "srt"
bilingual = true
```

The `websocket` sink serves the display page on `bind`; the page itself is
`web/index.html`, embedded into the binary. It asks the sink for its display
settings at `/settings.js`, so `line_size_vw` and `min_line_size_vw` are the only
knobs for the text size on the big screen.

The page can also hold a poster while there is nothing to translate: `image_path`
points at a picture (PNG, JPEG, GIF, WebP, AVIF or SVG), `image_text_top` and
`image_text_bottom` put a line above and below it, and `image_show_timeout` is the
number of seconds without a new translation before the poster appears. The poster
is shown from the very start as well, before the first phrase — so the screen is
never an empty rectangle while the run is being prepared.

The translated speech always has a local playback: `core.output_device` names the
device it goes to, `default` is the system default output, and `none` turns the
local playback off while the voice sinks keep receiving the audio. Voice sinks
(`virtual_sink`, `telegram_call`) are extra receivers of the same stream, not a
replacement for it.

Ready-made examples live in `examples/`: `config.dev.toml` for development,
`config.stream.toml` for broadcasting, `config.offline.toml` for a run without network.

## Requirements

- Nix with flakes (the development shell is provided **for Linux**)
- On macOS: Xcode Command Line Tools and a rustup toolchain — the Nix shell is not used
  there, see *Building on macOS* below
- On Windows: the MSVC toolchain (Visual Studio Build Tools with the C++ component) and,
  for packaging, PowerShell 7 — see *Building on Windows* below
- A reachable AI provider account for translation
- External infrastructure for sinks that need it (an audio server, a browser)

## Build and run

```bash
nix develop

# validate the configuration without starting anything
nix develop --command bash -c 'cargo run -- check --config examples/config.dev.toml'

# translate from the microphone
nix develop --command bash -c 'cargo run -- run --config examples/config.dev.toml'

# feed a text through the bus: no microphone, no network, no API key
nix develop --command bash -c \
  'cargo run -- script --config examples/config.offline.toml --text "Hello, world."'
```

The provider key is read from `OPENAI_API_KEY`; a `.env` file next to the project is read
automatically. `check` exits with a non-zero code when the configuration is not usable.
Logs go to stderr, the translation itself to stdout.

`Ctrl+C` — or `SIGTERM` when the program runs as a service — stops the run properly: the
phrase in progress is written out, connections are closed and files are saved. A second
`Ctrl+C` exits at once, for a shutdown that will not finish on its own.

Tests and lints:

```bash
nix develop --command bash -c 'cargo test'                        # unit, pipeline, doc tests
nix develop --command bash -c 'cargo clippy --all-targets -- -D warnings'
nix develop --command bash -c 'cargo fmt --check'
```

### Building on macOS

The Nix development shell targets Linux: it provides PipeWire, ALSA and glibc, which exist
there only. On macOS the audio backend is CoreAudio — a system framework — so a plain
toolchain is enough, with no audio server and no Nix:

```bash
xcode-select --install            # once: compiler and system headers
cd ../transono && cargo build     # the library ai-translator depends on
cd ../ai-translator && cargo build
```

`transono` keeps its PipeWire dependencies, and the `transonovirt` binary that drives them,
on Linux only; on macOS the library builds without them. The CI job `macos` builds and tests
both repositories on a macOS runner, so this path is checked on every push.

Building for macOS *from* Linux is also possible, with osxcross in a container — see
`docs/macos-cross-build.md`.

### Building on Windows

Windows has its own toolchain: the MSVC one, which is what the media engine of the voice-chat
sink is built for. A plain Rust installation with the *Visual Studio Build Tools* C++ component
is enough, no Nix and no cross-compilation:

```powershell
cd ..\transono; cargo build          # the library ai-translator depends on
cd ..\ai-translator; cargo build
```

The release package is built by one script, which fetches the engine with a pinned checksum,
runs the tests, builds the `telegram-call` variant and writes a portable ZIP with the executable
and `ntgcalls.dll` next to it:

```powershell
pwsh -File packaging/build-windows.ps1
```

`-SkipTests` skips the test run. PowerShell 7 is needed for the packaging scripts; building the
program itself only needs cargo.

The same script also produces an MSI installer through WiX — version 5, the last one without
the Open Source Maintenance Fee — which installs into `Program Files`, adds a Start Menu
shortcut and upgrades in place:

```powershell
pwsh -File packaging/build-windows.ps1 -SkipTests     # ZIP and MSI
pwsh -File packaging/build-msi.ps1                    # MSI only, from an existing stage
```

`-NoMsi` limits the script to the portable archive. WiX builds MSI on Windows only, so the
installer is produced by the Windows CI job as well, next to the archive.

### Microphone permission on macOS

macOS asks for microphone access the first time `ai-translator run` opens an input device.
The permission is granted to the **terminal application** the command is started from
(Terminal, iTerm, VS Code), not to the `ai-translator` binary: that is what the system sees
asking. A signed `.app` bundle would be asked in its own name. Accept the prompt once and
`run` works from that terminal.

If the prompt was dismissed, grant the permission in *System Settings → Privacy & Security →
Microphone*, or ask again with `tccutil reset Microphone` — it resets the decision for all
applications, so every other application will ask again as well.

A future `.app` bundle will need `NSMicrophoneUsageDescription` in its `Info.plist`, together
with signing and notarization; that belongs to stage 4.

## Project layout

```
src/
  app.rs            wiring: sources → core → bus → sinks
  supervisor.rs     start, isolate and restart sinks
  config.rs         configuration model and validation
  bus/              event bus and the translation event model
  core_glue/        adapter from provider session events, translation line wiring
  source/           audio, text and scripted sources
  sink/             sink traits, registry, text and voice sinks
  tts/              optional speech synthesis
web/index.html      full-screen display page
web/test/page.mjs   run of the page logic without a browser
docs/               technical specification, decisions and source material
examples/           example configurations
tests/pipeline.rs   end-to-end run of source → bus → sinks
```

## Dependencies

| Crate | Role |
|---|---|
| `transono` | audio capture, DSP, audio graph, AI providers |
| `tokio` | asynchronous runtime |
| `axum` | HTTP and WebSocket server of the display page |
| `serde`, `serde_json`, `toml` | configuration and event serialization |
| `clap` | command-line interface |
| `thiserror` | error types |
| `tracing`, `tracing-subscriber` | logging |
| `rustls` | TLS for the provider connection |
| `dotenvy` | reading `.env` |

`ratatui` and `crossterm` are declared for the terminal interface of the next stage and are
not used yet.

## Built on TRANSONO

The audio capture, DSP, audio graph and AI provider layers come from
[TRANSONO](https://github.com/uncleandy/transono), a real-time speech processing library.
AI-TRANSLATOR adds the application layer on top of it: the event bus, the sink subsystem,
configuration and the command-line interface.

## Releases

Packages are built locally and published from the local repository; CI uploads nothing:

```sh
bash packaging/build-all.sh
nix shell nixpkgs#gh -c gh release create v0.2.2 --verify-tag --generate-notes dist/*
```

The first command fills `dist/` with the Linux and the macOS files plus `SHA256SUMS`, the second attaches them to a release of an existing tag. Linux binaries need glibc 2.34 or newer, `libpipewire-0.3-0` 1.0 or newer and `libasound2`; the AppImage expects them on the system, it does not bundle them. macOS needs nothing beyond the system. Nothing is signed: the binary carries an ad-hoc signature, without which Apple Silicon will not start it at all, but Gatekeeper still treats the application as unsigned. Use the installer that ships in the archive — it copies the program with the engine beside it, clears the quarantine mark from the whole folder (a library that fails that check is refused, and the mark cannot be removed on the read-only disk image), links the command into `~/.local/bin` and prints the version it started:

```sh
./install-macOS.sh
```

The disk image is for people who prefer Finder: drag the folder out, then run the same script from there. Unpacking a `curl` download skips the quarantine altogether, since only the browser sets it:

```sh
mkdir -p ~/ai-translator
curl -fsSL https://github.com/UncleAndy/ai-translator/releases/download/v0.2.2/ai-translator-0.2.2-aarch64-macos.tar.gz \
  | tar xz -C ~/ai-translator --strip-components=1
~/ai-translator/install-macOS.sh
```

Details are in [docs/release.md](docs/release.md).

## Documentation

- `docs/TZ_ai-translator_v0.1.md` — technical specification (Russian)
- `docs/decisions.md` — decisions that shape the project (Russian)
- `docs/methods.txt` — source material on delivery protocols (Russian)
- `docs/macos-cross-build.md` — building for macOS from Linux with osxcross (Russian)
- `docs/release.md` — how releases are built and what the artefacts need (Russian)

## License

Dual-licensed under your choice of either:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in AI-TRANSLATOR by you shall be dual-licensed as above, without any additional
terms or conditions.
