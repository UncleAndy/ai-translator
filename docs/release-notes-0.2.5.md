## A Windows installer, and the voice-chat sink in every package

0.2.5 adds the thing Windows users kept asking for by hand: a normal installer. The portable ZIP stays — some people want to unpack and run, and for a tool that reads a configuration file that is a fair way to work. For everyone else there is now `ai-translator-0.2.5-x86_64-windows.msi`.

The same release settles a question that used to depend on how a build was started: the Telegram voice-chat sink is now part of **every** package. No archive ships without it, and there is no switch to make one.

### What the installer does

- Installs into `Program Files\ai-translator`: the program, `ntgcalls.dll` beside it, the licences and `INSTALL-windows.txt`.
- Adds **AI Translator** to the Start Menu, with the working directory set, so a configuration can sit next to the binary.
- Registers itself in **Apps & features**, where it is removed without leftovers.
- **Upgrades in place**: a newer version replaces the old one instead of installing a second copy beside it.
- Carries no custom actions, no service and no registry keys beyond the uninstall entry — nothing to go wrong during a silent install (`msiexec /i … /qn`).

### How it is built

- `packaging/build-msi.ps1` turns the staged directory into the MSI; `packaging/build-windows.ps1` calls it and now produces both the archive and the installer in one run (`-NoMsi` skips the installer).
- The WiX source is `packaging/ai-translator.wxs`, with a stable `UpgradeCode` so upgrades and removals are tracked as one product.
- **WiX is pinned to 5.x**: versions 6 and later ask for the Open Source Maintenance Fee, and the toolchain runs on Windows only — its own CLI says so when started elsewhere.
- The Windows CI job builds the installer together with the archive and fails if either is missing. A manual run keeps both as build artifacts for 90 days; ordinary runs upload nothing.

### The voice-chat sink is no longer optional

- `build-linux.sh`, `build-macos.sh` and `build-windows.ps1` now fix the `telegram-call` feature themselves. Previously it arrived through the `FEATURES` environment variable, and a release built without it silently lost the sink: the option looked identical in a configuration file, and only an error at start-up told the difference.
- `-NoEngine` is gone from the Windows script for the same reason. A plain `cargo build` still leaves the feature off, so the program compiles on a machine that has not downloaded the media engine — that path is for development, not for a package.
- A small check answers the question for any archive: run `ai-translator check` on a configuration with a `telegram_call` sink. A package built with the sink asks for the account; one without it says the build cannot stream to voice chats.

### Intel Macs no longer get a package

- The media engine has no build for `x86_64-apple-darwin`, and shipping a macOS archive that cannot join a voice chat is not something this project does any more. `build-macos.sh` refuses that target with exit code 2, and `build-all.sh` no longer walks it.
- Apple Silicon keeps both the archive and the disk image, with `libntgcalls.dylib` beside the binary. Windows keeps the ZIP and the MSI.

### Upgrading

Nothing changes for existing setups: the configuration format, the cloud models, the sinks and the Telegram voice-chat sink are the same as in 0.2.4. Windows support itself — WASAPI capture and playback, the engine DLL, the `telegram-call` feature — arrived in 0.2.4; this release adds the installer on top of it and makes the sink mandatory in what is published.

### Assets

| Asset | Platform |
|---|---|
| `ai-translator-0.2.5-x86_64-linux.tar.gz`, `.deb`, `.rpm`, `.AppImage` | Linux x86_64 — with `libntgcalls.so` |
| `ai-translator-0.2.5-aarch64-macos.tar.gz`, `.dmg` | macOS, Apple Silicon — with `libntgcalls.dylib` |
| `ai-translator-0.2.5-x86_64-windows.msi` | Windows 10/11 x86_64 — installer, with `ntgcalls.dll` |
| `ai-translator-0.2.5-x86_64-windows.zip` | Windows 10/11 x86_64 — portable, with `ntgcalls.dll` |
| `SHA256SUMS` | checksums of every file above |

### Known limitations

- **Nothing is signed.** The installer shows an unknown publisher in the elevation prompt, and the archive still triggers SmartScreen once. A certificate is not part of this release; both files are covered by `SHA256SUMS`.
- **The Visual C++ runtime is a prerequisite**, bundled with nothing: it is present on most machines and installs from Microsoft's site when it is not.
- **A virtual audio device is still a third-party driver** — the application never installs one.
- **Windows arm64 is not shipped**: there is no media engine for it.
- **Intel Macs are not supported by the packages.** The project still builds for `x86_64-apple-darwin` from source, without the voice-chat sink.
