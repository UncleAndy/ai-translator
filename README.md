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
console, WebSocket and transcript sinks, the supervisor, the audio source on the synchronous
translation provider, the offline text and scripted sources, and the command-line interface.
Interfaces may still change before the 1.0 release.

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

Declared in the configuration and reserved for the next stages:

| Sink | Purpose |
|---|---|
| `webhook` | Discord, Slack or an arbitrary JSON body template |
| `virtual_sink` | virtual audio device (OBS, Discord, Zoom, vMix) with optional monitoring of the original voice |

A sink that is not ready says so instead of silently swallowing the translation: a disabled
sink is never started, and a sink that cannot deliver reports an error, which the supervisor
records and acts on. A sink that fails is restarted with a bounded number of attempts and
does not affect the other sinks.

Translated speech comes from the AI provider itself as part of the same session that produces
the translated text. Speech synthesis (TTS) is an optional additional source of audio, used
only when a provider or a source does not produce speech of its own.

## Configuration

One TOML file describes the core and every sink. Each sink has a `type` and an `enabled`
flag; secrets are referenced through environment variables, never stored in the file.

```toml
[core]
provider = "openai/translation"
source_lang = "auto"
target_lang = "ru"
input_device = "default"

[[sink.text]]
type = "console"
enabled = true

[[sink.text]]
type = "websocket"
enabled = true
bind = "0.0.0.0:8643"

[[sink.text]]
type = "transcript"
enabled = true
path = "transcript.srt"
format = "srt"
bilingual = true
```

Ready-made examples live in `examples/`: `config.dev.toml` for development,
`config.stream.toml` for broadcasting, `config.offline.toml` for a run without network.

## Requirements

- Nix with flakes (development shell is provided)
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

## Documentation

- `docs/TZ_ai-translator_v0.1.md` — technical specification (Russian)
- `docs/decisions.md` — decisions that shape the project (Russian)
- `docs/methods.txt` — source material on delivery protocols (Russian)

## License

Dual-licensed under your choice of either:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in AI-TRANSLATOR by you shall be dual-licensed as above, without any additional
terms or conditions.
