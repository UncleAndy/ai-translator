# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The provider ends a translation session after about an hour; the program now opens
the next one by itself.

### Added

- The provider session is replaced automatically, so a broadcast outlives the
  provider's one-hour limit instead of stopping with it. The default timer reconnects
  at 55 minutes, before the limit, in a pause between phrases;
  `[core] session_reconnect_minutes` changes it and `0` leaves only the reconnect
  after an actual end (a broken network, a closed socket, an idle timeout).
- The replacement runs in its own task (`src/source/reconnect.rs`), so sinks, the
  transcript and the screen keep working across it: the phrase in progress is flushed
  by the bridge before the old session is closed, the new one continues on the same
  bus. Attempts back off from half a second to half a minute; a refused key or an
  exhausted quota pauses for a minute instead of hammering the provider, and the
  supervisor keeps trying slowly rather than leaving the line silent.
- `LineState::Reconnecting` reaches the screen, which now says «переподключение…»
  instead of showing the last phrase as if nothing happened.
- Eight tests cover the supervisor with a fake session: replacement after an end, a
  transient failure retried, a refusal not hammered, a stop during a backoff, the
  proactive timer, a session that never comes back, error classification and the
  timer setting.

## [0.2.5] - 2026-10-23

An installer for Windows, and one rule for every package: the Telegram voice-chat
sink is always inside.

### Added

- `ai-translator-<version>-x86_64-windows.msi` installs the program into
  `Program Files\ai-translator`, adds a Start Menu shortcut, registers itself in
  "Apps & features" and upgrades over a previous version instead of living beside
  it. It is built by `packaging/build-msi.ps1` from the very directory the archive
  is packed from, so both contain the same executable, engine DLL and licences.
- The Windows CI job builds the MSI together with the archive and fails if either
  is missing; a manual run keeps both as artifacts. WiX is pinned to version 5 —
  versions 6 and later ask for the Open Source Maintenance Fee, and its toolchain
  runs on Windows only.

### Changed

- Every package carries the voice-chat sink: `telegram-call` is fixed in
  `build-linux.sh`, `build-macos.sh` and `build-windows.ps1` instead of arriving
  through `FEATURES`, so a release without the engine can no longer be built by
  accident. `-NoEngine` is gone from the Windows script for the same reason; a
  plain `cargo build` still leaves the feature off, so the program compiles
  without downloading the engine.

### Removed

- The Intel macOS packages (`x86_64-macos.tar.gz` and `.dmg`). The media engine
  has no build for `x86_64-apple-darwin`, and a package without the voice-chat
  sink is not published: `build-macos.sh` refuses that target with exit code 2,
  and `build-all.sh` no longer walks it.

## [0.2.4] - 2026-10-23

Windows x86_64 joins the platforms, and the stubs stop pretending.

### Added

- Windows x86_64 builds and runs: capture and playback go through WASAPI, and the
  release is a portable ZIP with `ai-translator.exe` and `ntgcalls.dll`. The
  voice-chat sink works there too — the engine ships as a shared library for MSVC,
  fetched by `packaging/fetch-ntgcalls.ps1` with the archive's SHA256 pinned the same
  way the Linux and macOS engines are.
- `packaging/build-windows.ps1` builds that ZIP: tests, the `telegram-call` variant,
  the licences and `INSTALL-windows.txt`, plus a `.sha256` file beside it.
- CI runs the whole suite on `windows-latest` and builds the package there, which is
  also the only place where linking against the Windows engine is exercised.

### Changed

- `transono` is 0.5.1: its Unix-only `daemonize` dependency is Linux-only now, and the
  Windows virtual-device backend answers honestly instead of being an empty stub that
  the backend factory could not even construct.
- Stopping with SIGTERM is Unix-only; on Windows the console handler behind Ctrl+C is
  the whole story.

### Fixed

- An enabled `virtual_sink` is refused by the configuration check with an explanation.
  It was accepted and then failed on the first translated phrase, which the supervisor
  retried six times — six restarts for a sink that was never written.

## [0.2.3] - 2026-10-20

The failures that used to be invisible: a dropped call, a sink that gave up after a
second of trouble, and a screen anyone on the network could read.

### Changed

- A failed sink is restarted six times instead of twice, and the wait doubles from
  half a second up to half a minute. Two restarts half a second apart meant that a
  network outage longer than a second ended a channel for the rest of the session;
  a short hiccup is now covered in seconds and a real outage is waited out.
- The screen listens on `127.0.0.1:8642` by default instead of `0.0.0.0:8642`. It
  has no authentication: on a shared network every translation and every setting was
  readable by anyone who reached the port. A hall that needs the screen remotely
  keeps its explicit `bind`, and `check` warns about it instead of refusing it.
- Every field of the configuration has a default, so a file may name only what
  differs: `[core]` and `target_lang` are optional (`target_lang` is `en`), the devices
  are `default`, no `[sink]` section means a console sink, a transcript without a
  `path` goes to `transcript.<format>`, and a video sink without a `device` uses the
  system output. An empty file is a valid configuration, and `examples/config.minimal.toml`
  shows the shortest one.
- The configuration is strict about names: an unknown field is a parse error that
  names the field and lists the accepted ones, rather than a typo that quietly falls
  back to a default and makes the run do something other than the file says.

### Fixed

- The voice-chat sink did not notice that the call had stopped taking audio. A failed
  frame was logged and the broadcast went on claiming to run while the group heard
  silence. Twenty refused frames in a row (a fifth of a second) now declare the call
  dead: the account leaves it, the sink returns the reason, and the supervisor builds
  a fresh one that joins again.
- Entering a call is bounded by thirty seconds. A join that Telegram never answered
  held the sink in limbo that looked exactly like a working broadcast.
- A dead engine task made the sink drop frames without a word; a closed channel is now
  a sink failure, so it is restarted instead of muted.
- A refused frame used to be logged a hundred times a second, which hid the reason
  instead of telling it; it is reported once a second now.
- `ntgcalls-sys` unpacked the engine archive with `zip` 2.6.1, which carries three
  advisories, one critical (extraction follows symlinks out of the destination and
  keeps SUID bits). The vendored crate now uses `zip` 9.0.5; the build script's two
  calls are unchanged.
- The engine archive is pinned by `sha256` for every asset of the version in use.
  `fetch-ntgcalls.sh` refuses an archive that does not match (exit code 4) and refuses
  a version with no pin at all (exit code 3): the engine is loaded into the process,
  so a substituted release would have been arbitrary code.

## [0.2.2] - 2026-10-19

The first start on a real Mac, and what it taught: the packages were fine, the
instructions were not.

### Added

- `install-macOS.sh` ships inside the macOS tarball and the disk image. It copies
  the program with the engine into `~/Applications/ai-translator`, clears the
  quarantine mark from the whole folder, links the command into `~/.local/bin` when
  that directory is in `PATH`, and asks the binary for its version as a smoke check.
  One command instead of three, and the disk image can no longer be used in place
  by mistake.
- `INSTALL-macOS.txt` ships there too: the same steps by hand, and the reason the
  engine is refused when the mark stays on it.

### Fixed

- macOS: the package built in 0.2.1 refused to start with `Library not loaded:
  @rpath/libntgcalls.dylib ... library load disallowed by system policy`. The
  rpath was right — dyld found the file beside the binary — but the engine is a
  library, macOS checks it the same way it checks an executable, and the
  quarantine mark of the mounted disk image made the check fail. Clearing the mark
  from the binary alone was not enough, and on a read-only image it cannot be
  cleared at all. Verified on Apple Silicon with macOS 26: after
  `cp -R "/Volumes/AI Translator/." ~/ai-translator` and
  `xattr -dr com.apple.quarantine ~/ai-translator` the CLI starts and reports its
  version.
- `README.md` and `docs/release.md` no longer tell the reader to clear the mark
  from a single file or to open the application from the disk image.
- An error reported by the translation service used to reach the reader as
  `Transport error: invalid value: map, expected map with a single key`, and the
  session closed without a word about the cause: the error body was modelled as an
  enum where the API sends an object of several fields. The message, code and
  offending parameter are now printed as they arrive, a frame that cannot be parsed
  is shown with its text, and the realtime backend no longer drops error events at
  all. Fixed in `transono`.

## [0.2.1] - 2026-10-19

A packaging release: the voice-chat sink now travels in the Apple Silicon macOS
packages too, and the engine can be linked on macOS at all.

### Changed

- The Apple Silicon macOS packages (`.tar.gz` and `.dmg`) now carry the
  voice-chat sink: the engine is fetched for `aarch64-apple-darwin`, shipped as
  `libntgcalls.dylib` beside the binary and resolved at `@loader_path`. The Intel
  macOS packages stay as the engine requires — built without the feature, and the
  binary refuses a configuration with `telegram_call` and says why.
- The `ntgcalls-sys` crate is vendored in `vendor/`: its build script handed GNU
  linker options (`--allow-shlib-undefined`, `-rpath`) to the linker whenever the
  engine was a shared library, and ld64 rejects both. The vendored copy passes
  them to the Linux linker only, so the macOS cross build links at all.
- `packaging/build-all.sh` passes `FEATURES` to the macOS builds; the macOS build
  image gained `curl` and `unzip` and is tagged `1.95-call`, so it is rebuilt once.
- `docs/release.md` and `docs/voice-chat.md` state the target matrix: Linux
  x86_64, Linux arm64 and macOS arm64 carry the sink, Intel macOS does not.
- The engine is an ordinary shared dependency of the binary, not a library opened
  on demand: the program needs the file to start at all, `--version` included. In
  the Linux tarball it lies next to the binary, in `.deb` and `.rpm` it goes to
  `/usr/lib/ai-translator` and is reached through the relative rpath
  (`$ORIGIN/../lib/ai-translator`); on Apple Silicon it lies next to the binary at
  `@loader_path`. A binary copied out of the package on its own will not run.

### Known limits

- The Apple Silicon package is cross-built on Linux and has not been run on a real
  Mac yet: the sink is verified on Linux only.

## [0.2.0] - 2026-10-19

The release that turns the translator into a broadcaster: the translation can now
be published into Telegram, as text in a channel and as live speech in a voice
chat or a channel livestream, and the big screen shows a poster with a picture and
two lines of text instead of a black rectangle while there is nothing to translate.

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
  x86_64, Linux arm64 and macOS arm64, and the release packages for those three
  carry the sink. A build without the feature, or on a target the engine does not
  exist for — Intel macOS among them — refuses a configuration with
  `telegram_call` and says why.

#### Big screen

- New poster for the `websocket` display sink: a picture in the middle of the
  screen with one line of text above it and one below, shown whenever there is
  nothing to translate.
- Configuration: `image_path` (the picture: PNG, JPEG, GIF, WebP, AVIF or SVG),
  `image_text_top` and `image_text_bottom` (the lines above and below it), and
  `image_show_timeout` (seconds without a new translation before the poster
  appears; zero means the first pause). Without `image_path` there is no poster
  and the screen behaves as before.
- The picture is served by the sink at `/poster` and read from disk on every
  request, so it can be replaced while the translation runs; the content type
  follows the extension and caching is off.
- The poster is on screen from the moment the page opens — before the first
  phrase — disappears as soon as a translation arrives, and comes back in every
  pause. At `image_show_timeout = 0` a pause still has to last half a second, so
  the poster does not flicker between the words of a running phrase.
- The page asks for the poster through `/settings.js`, next to the text sizes, so
  a screen that is already open picks up a changed configuration on reload.

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
  19 MB and the engine travels beside it as an LGPL-3.0 shared library, found
  through a path relative to the binary.
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

- macOS: the Apple Silicon package carries the voice-chat sink and the Intel one
  cannot — the engine has no build for that architecture, so the Intel binary is
  made without the feature and refuses a configuration with `telegram_call`. The
  Apple Silicon package is cross-built on Linux and has not been run on a real Mac
  yet: the sink is verified on Linux only.
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

[0.2.5]: https://github.com/UncleAndy/ai-translator/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/UncleAndy/ai-translator/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/UncleAndy/ai-translator/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/UncleAndy/ai-translator/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/UncleAndy/ai-translator/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/UncleAndy/ai-translator/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/UncleAndy/ai-translator/releases/tag/v0.1.0
