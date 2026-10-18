# AI-TRANSLATOR

**Synchronous speech and text translator with parallel delivery to arbitrary sinks.**

AI-TRANSLATOR listens to speech (or takes text), obtains a translation into a target
language, and delivers the result to any number of independent sinks **at the same time** —
text sinks (captions, chat messages, on-screen displays, studio mixers, files) and voice
sinks (translated speech routed to virtual audio devices, network audio streams).

The defining property of the project: **the translation core does not know where its result
goes.** The core produces a stream of translation events; delivery is performed by isolated
sink modules that can be enabled and disabled individually. One and the same translation can
go to a messenger, a stage screen, broadcast captions and a voice channel simultaneously.

## Status

Early development. The architecture is being implemented in stages; interfaces may change
before the 1.0 release.

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

### Translation event

The unit exchanged between the core and the sinks is a normalized, immutable event:

| Variant | Meaning |
|---|---|
| `SourceText` | fragment of recognized source speech |
| `Translated` | fragment of translated text, with `delta`, `is_final` and a monotonic `seq` |
| `TranslatedAudio` | fragment of translated speech (48 kHz, f32, mono) |
| `Status` | line state change (started, running, stopped, failed) |

`is_final` marks the end of a phrase: sinks that update a single record (a chat message, a
mixer title) use it to release the current record and start the next one. `seq` is a strictly
increasing counter for protocols that require one (HTTP caption endpoints).

## Sinks

Text sinks:

| Sink | Purpose |
|---|---|
| `websocket` | local HTTP server plus a full-screen page for a stage display or OBS Browser Source |
| `console` | terminal output for observation and debugging |
| `transcript` | recording to TXT, SRT, VTT or JSONL, optionally bilingual |
| `captions_http` | HTTP POST captions endpoint (YouTube-compatible contract) |
| `telegram` | channel, group or private chat, using a single message updated as the phrase grows |
| `webhook` | Discord, Slack or an arbitrary JSON body template |
| `vmix` | vMix HTTP API and TCP ticker |
| `obs` | OBS WebSocket text sources |

Voice sinks:

| Sink | Purpose |
|---|---|
| `virtual_sink` | virtual audio device (OBS, Discord, Zoom, vMix) with optional monitoring of the original voice |
| `icecast` | compressed network audio stream for remote listeners |
| `webrtc` | browser listeners through a media server |
| `srt_rtmp` | translated voice mixed into a broadcast stream |
| `tg_voice` | Telegram voice chat |
| `ndi` | local studio mixers |

Translated speech comes from the AI provider itself as part of the same session that
produces the translated text. Speech synthesis (TTS) is an optional additional source of
audio, used only when a provider or a source does not produce speech of its own.

## Configuration

One TOML file describes the core and every sink. Each sink has a `type` and an `enabled`
flag; secrets are referenced through environment variables, never stored in the file.

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

## Requirements

- Nix with flakes (development shell is provided)
- A reachable AI provider account for translation
- External infrastructure for sinks that need it (vMix, OBS, an audio server)

## Build and run

```bash
nix develop
cargo run -- check --config examples/config.dev.toml   # validate configuration
cargo run -- run   --config examples/config.dev.toml   # start translating
```

Tests and lints:

```bash
nix develop --command bash -c 'cargo test'
nix develop --command bash -c 'cargo clippy -- -D warnings'
```

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
docs/               technical specification and source material
examples/           example configurations
```

## Built on TRANSONO

The audio capture, DSP, audio graph and AI provider layers come from
[TRANSONO](https://github.com/uncleandy/transono), a real-time speech processing library.
AI-TRANSLATOR adds the application layer on top of it: the event bus, the sink subsystem,
configuration and the command-line interface.

## Documentation

- `docs/TZ_ai-translator_v0.1.md` — technical specification (Russian)
- `docs/methods.txt` — source material on delivery protocols (Russian)

## License

Dual-licensed under your choice of either:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in AI-TRANSLATOR by you shall be dual-licensed as above, without any additional
terms or conditions.
