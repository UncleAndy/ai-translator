# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-10-19

The release that turns the translator into a broadcaster: the translation can now
be published into Telegram, as text in a channel and as live speech in a voice
chat or a channel livestream.

### Added

#### Telegram text broadcast

- New sink `[[sink.text]] type = "telegram"`. It publishes the translation into
  one channel, named in `chat_id`, and into nothing else: no subscriber
  bookkeeping, no state file, no commands from readers.
- The message is written while the phrase is still being spoken: the first
  revision creates the message, every following revision edits the same one in
  place, and the finished phrase stays as the last revision. Intermediate
  revisions are never dropped, so the channel shows the translation arriving.
- Posting limits are part of the configuration: `max_sends_per_minute` (default
  20) and `edit_interval_ms` (default 1000). Zero lifts a limit. The window is
  shared by the whole sink, so a burst of phrases cannot exceed it.
- `bilingual` puts the source phrase and the translation into one message,
  `silent` posts without a notification, `message_thread_id` targets a topic.
  Text goes as plain text without a parse mode, split into 4096-character parts.
- The bot token is read through `token_env`; it never appears in the file.

#### Telegram voice-chat broadcast

- New sink `[[sink.voice]] type = "telegram_call"`: a continuous live broadcast
  of the translated speech into a group voice chat or a channel livestream.
  This is not a voice message and not a stream of recordings — the call hears the
  translation from the moment it appears until the broadcast stops, with silence
  during pauses so the connection never drops.
- The broadcast runs from a user account over MTProto: a bot cannot enter a voice
  chat, so the sink needs `api_id`/`api_hash` of an application and a signed-in
  account. Both come through `api_id_env` and `api_hash_env`.
- New command `telegram-login`: signs the account in once (phone number, the code
  from Telegram, and the two-step password when the account has one), writes the
  session file with mode 0600 and prints the account it signed in. The session
  file is named in `session_file` and is a secret — it is never committed.
- `create_if_missing` creates the group call when the chat has none; otherwise the
  sink joins the live one.
- Speech is fed to the media engine as 480-sample frames (10 ms, 48 kHz, 16-bit,
  mono) on a fixed schedule kept in absolute time, so a late send does not drag a
  burst of frames behind it. Speech starts after a hundred milliseconds have
  gathered, which keeps the call from running dry in the middle of a word.
- The joins between provider chunks, and between speech and silence, are ramped
  instead of stepped: a hard join there is heard as a click.
- Feature `telegram-call` (off by default). The engine is available for Linux
  x86_64, Linux arm64 and macOS arm64; a build without the feature, or on an
  unsupported architecture, refuses a configuration with `telegram_call` and says
  why.

#### Big screen

- New poster options for the `websocket` display sink: `image_path` (a picture
  shown while nothing is translated), `image_text_top` and `image_text_bottom`
  (a line above and below the picture), and `image_show_timeout` (seconds without
  a new translation before the poster appears). The poster is served at `/poster`
  and read from disk on every request, so the file can be replaced while the
  translation runs. It is on screen from the start of the run, before the first
  phrase, and returns in every pause.

#### Playback

- New option `core.output_device`: `default` is the system output, `none` plays
  nothing locally while the voice sinks still receive the audio, and any other
  value is an output device by the name `ai-translator devices` prints. An
  unknown name fails with the list of the known ones.

#### Packaging

- The voice-chat build ships the media engine as a shared library
  `libntgcalls.so` beside the binary (and in `/usr/lib/ai-translator` inside the
  `.deb` and `.rpm`), found through an rpath of `$ORIGIN`.
- `packaging/run-nix.sh` builds and runs the program on NixOS, where the
  distribution packages do not survive `nix-ld`; `FEATURES=` builds the version
  without the voice-chat sink.
- `packaging/test-call.sh` runs the whole test suite of the voice-chat build,
  including the engine probe and the sink probe.
- `docs/voice-chat.md` describes what the broadcast needs and what has been
  checked; `examples/config.all.toml` lists every parameter.

### Changed

- The voice-chat build is no longer statically linked against the engine. A static
  copy brought its own C++ runtime and allocator into the process and the program
  stopped working — even when the sink was disabled. The binary went from 60 MB to
  19 MB and the engine travels beside it as an LGPL-3.0 shared library, loaded only
  when a call starts.
- The Nix dev shell carries the C++ runtime and zlib of the engine, so the engine
  loads on NixOS without extra setup.

### Fixed

- Segmentation fault on NixOS right after the list of sinks, in any run of a build
  that carried the voice-chat sink: the cause was the static engine, not the call.
- Clicking and stuttering in the broadcast: the sink sent 20 ms of audio every
  20 ms while the engine pulls a frame every 10 ms, so every second slot had
  nothing to send. Also fixed: a full queue silently dropped speech, the schedule
  drifted into bursts after a slow send, and the backlog limit cut phrases at one
  second.
- Translated speech was always played on the system default output, so a silent
  broadcast was impossible.

### Known limits

- macOS: the release packages of this version are built without the voice-chat
  sink — `packaging/build-all.sh` passes `FEATURES` to the Linux build only, and
  cross-building the engine for macOS is not wired yet. The sink has not been run
  on a real Mac; Intel macOS cannot run it at all.
- The `.AppImage` on NixOS was not re-checked after the engine became a shared
  library; `packaging/run-nix.sh` is the supported way to run there.
- Packages are unsigned: macOS has an ad-hoc signature and no notarisation, Linux
  has no repository signature.

## [0.1.0] - 2026-10-18

The first working translator: speech in, translation out, several receivers at
once.

### Added

- `run` captures the microphone, translates through the synchronous
  `openai/translation` provider and publishes both text and speech of the
  translation on an internal bus; `check` validates a configuration without
  starting anything, `devices` lists capture and playback devices.
- Text sinks: `console`, `websocket` (the big-screen page, embedded into the
  binary, with a fixed and configurable text size), `transcript` (SRT and plain
  text, one file per session), `webhook`.
- Voice sink `virtual_sink`: the translation is played into a virtual device, so
  any program can capture it.
- Supervisor with a per-sink restart policy, structured logs and a clean stop on
  `Ctrl-C`; providers, sources and sinks are separate stages behind traits, so the
  text and scripted sources run the whole pipeline without a microphone or a key.
- Configuration in one TOML file, secrets only through `*_env` names, and
  `examples/config.all.toml` as the reference list of parameters.
- Packaging: `.tar.gz`, `.deb`, `.rpm`, `.AppImage` for Linux x86_64 and
  `.tar.gz`/`.dmg` for macOS on Apple Silicon and Intel, all built locally in
  containers; `SHA256SUMS` beside the artefacts.
- Documentation: `docs/release.md`, `docs/macos-cross-build.md`,
  `docs/TZ_ai-translator_v0.1.md`.

[0.2.0]: https://github.com/UncleAndy/ai-translator/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/UncleAndy/ai-translator/releases/tag/v0.1.0
