## The broadcast outlives the provider session

From 0.2.6 the translation no longer dies at the sixty-minute mark.

The limit is not ours. OpenAI closes a translation session after about an hour of work, and until now that was the end of the broadcast: the only way back was to restart the program. The cause was never the key or the network — the program opened exactly one session and never another. Now a supervisor inside the same process replaces it.

### What happens at the hour boundary

- A minute or two before the limit — at 55 minutes by default — the current session is closed and a new one is opened. The change lands in a pause between phrases, so speech does not break mid-word.
- If a session ends earlier — a network drop, a closed socket, an idle timeout — the replacement starts at once instead of waiting for the timer.
- Sinks, the transcript and the screen are not rebuilt: it is the same bus, so the screen and the recording keep going, and the phrase that was playing when the session ended is flushed to its end.
- The screen stops showing a frozen last phrase and says «переподключение…» instead; afterwards it goes back to the live state.
- The log records both ends of the event: `сессия перевода завершилась (…)` and `сессия перевода восстановлена: попытка N, разрыв …`.

### When the provider refuses

- Attempts back off: half a second, a second, two, up to thirty.
- A wrong key or an exhausted quota (401, 403, 429) is not hammered every half second: the program reports the failure and then retries once a minute.
- After six quick attempts the reconnection does not stop — it becomes rare. A broadcast matters more than a tidy retry budget: silence never becomes permanent.

### Configuration

One field in `[core]`:

```toml
[core]
session_reconnect_minutes = 55   # default: five minutes before the provider limit
```

- `0` turns the timer off and leaves only the reconnect after an actual end.
- A value above 60 is flagged by `check`: the provider will close the session before the replacement begins.

For a quick test set it to `1` — the cycle fires after a minute, and the whole sequence becomes visible in the log.

### What did not change

The configuration format, the providers, the sinks, the screen, the Windows installer and the packaging scripts are the same as in 0.2.5. Nothing changed in the `transono` library: the end of a session is observed by a wrapper around the provider inside this application.

### Tested and not tested

Tested: 168 unit tests, eight of them on the supervisor itself (an ended session, a refused attempt, stopping during a backoff, the timer, a refused key, exhausted attempts); a build with the voice-chat sink; clippy without warnings; the logic of the screen page.

Not tested: a two-hour live broadcast across the hour boundary. That needs a provider key and a sound card, so the first real run is yours. The sign of success: the broadcast continues without a restart, the log shows one handover, and the screen blinks «переподключение…» and returns.

### Files

| File | Platform |
|---|---|
| `ai-translator-0.2.6-x86_64-linux.tar.gz`, `.deb`, `.rpm`, `.AppImage` | Linux x86_64 |
| `ai-translator-0.2.6-aarch64-macos.tar.gz`, `.dmg` | macOS, Apple Silicon |
| `ai-translator-0.2.6-x86_64-windows.zip` | Windows 10/11 x86_64, portable |
| `ai-translator-0.2.6-x86_64-windows.msi` | Windows 10/11 x86_64, installer |
| `SHA256SUMS` | checksums of every file |

### Known limitations

- **Nothing is signed.** The installer will show an unknown publisher, and the archive trips SmartScreen once; `SHA256SUMS` covers integrity.
- **No packages for Intel Macs** — the voice-chat engine has no build for that architecture.
- **A gap of a second or two is possible.** A phrase playing at the moment of the handover may be lost; it shows as a pause, not as a duplicate.
- **One session, one charge.** If the provider bills each session separately, an hour of broadcast costs like two connections; check the invoice.
