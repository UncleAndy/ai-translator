//! Telegram voice sink: the translated speech goes to a group as voice messages.
//!
//! The Bot API has no live audio stream — a bot cannot speak into a group in real
//! time, and the voice-chat stream needs an MTProto user session rather than a
//! bot. What a bot can do is post voice notes, and that is what this sink does:
//! the speech of the translation is gathered as it arrives and sent as one voice
//! message per fragment, so the group hears the translation while it is spoken,
//! with a delay of one fragment.
//!
//! A fragment ends when the audio stops for [`TelegramVoiceSettings::gap`] (the
//! same pause that closes a phrase in the text path) or when it reaches
//! [`TelegramVoiceSettings::max_chunk`], whichever comes first. Telegram plays
//! voice notes only as OGG with Opus inside, so the PCM the provider returns is
//! encoded here — in pure Rust, so no codec library has to be installed next to
//! the binary.
//!
//! The bot is its own: a separate token in `token_env`, so the voice bot can be
//! a different bot from the one that posts the text. New voice messages are held
//! to the same per-minute limit as text, because Telegram counts them alike.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use opus_pure::{Application, MAX_PACKET_BYTES, OggOpusWriter, OpusEncoder, OpusHead};
use transono::audio::{Audio, PcmAudio};

use crate::bus::LineId;
use crate::sink::rate::SendWindow;
use crate::sink::{Result, SinkError, VoiceSink};

/// Bot API root.
pub const API_BASE: &str = "https://api.telegram.org";

/// Opus encodes 20 ms at a time; the standard frame for speech.
const FRAME_MS: u32 = 20;

/// Bitrate of the voice note: speech, mono or stereo, needs no more.
const VOICE_BITRATE_BPS: i32 = 24_000;

/// Longest audio in one voice message by default.
pub const DEFAULT_MAX_CHUNK: Duration = Duration::from_secs(45);

/// Pause that closes a fragment by default: the phrase boundary of the text path.
pub const DEFAULT_GAP: Duration = Duration::from_millis(700);

/// How many new messages may be posted per minute by default.
pub const DEFAULT_SENDS_PER_MINUTE: u32 = 20;

/// How long a rate limit is waited out before the sink gives up.
const RETRY_AFTER_LIMIT_SECS: u64 = 30;

/// Request timeout: a stuck upload must not hold the sink forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Parameters of the Telegram voice sink.
pub struct TelegramVoiceSettings {
    /// Group the voice messages go to: a number or `@name`.
    pub chat_id: String,
    /// Bot token of the voice bot.
    pub token: String,
    /// Bot API root: `https://api.telegram.org` unless a compatible server is used.
    pub base: String,
    /// Forum topic to post into.
    pub thread: Option<i64>,
    /// Longest audio in one voice message.
    pub max_chunk: Duration,
    /// Pause that closes a fragment.
    pub gap: Duration,
    /// New messages allowed per minute; zero removes the limit.
    pub max_sends_per_minute: u32,
}

impl TelegramVoiceSettings {
    /// Parameters with the default Bot API root and no optional behaviour.
    pub fn new(chat_id: &str, token: &str) -> Self {
        Self {
            chat_id: chat_id.to_string(),
            token: token.to_string(),
            base: API_BASE.to_string(),
            thread: None,
            max_chunk: DEFAULT_MAX_CHUNK,
            gap: DEFAULT_GAP,
            max_sends_per_minute: DEFAULT_SENDS_PER_MINUTE,
        }
    }
}

impl std::fmt::Debug for TelegramVoiceSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token must not reach a log line.
        f.debug_struct("TelegramVoiceSettings")
            .field("chat_id", &self.chat_id)
            .field("base", &self.base)
            .field("thread", &self.thread)
            .field("max_chunk", &self.max_chunk)
            .field("gap", &self.gap)
            .field("max_sends_per_minute", &self.max_sends_per_minute)
            .finish_non_exhaustive()
    }
}

/// Everything the sink takes from the configuration.
#[derive(Debug, Clone, Copy)]
pub struct TelegramVoiceSpec<'a> {
    /// Group the voice messages go to.
    pub chat_id: &'a str,
    /// Variable holding the token of the voice bot.
    pub token_env: &'a str,
    /// Forum topic, when the group is a forum.
    pub thread: Option<i64>,
    /// Longest audio in one voice message.
    pub max_chunk: Duration,
    /// Pause that closes a fragment.
    pub gap: Duration,
    /// New messages allowed per minute; zero removes the limit.
    pub max_sends_per_minute: u32,
}

/// Audio gathered for the fragment that is not sent yet.
struct Fragment {
    /// Samples per second.
    rate: u32,
    /// Channel count.
    channels: usize,
    /// Samples of each channel, planar.
    planes: Vec<Vec<f32>>,
    /// Media time where the gathered audio ends.
    ends_at: Option<Instant>,
}

impl Fragment {
    fn new(rate: u32, channels: usize) -> Self {
        Self {
            rate,
            channels,
            planes: vec![Vec::new(); channels],
            ends_at: None,
        }
    }

    /// Number of audio frames gathered so far.
    fn frames(&self) -> usize {
        self.planes.first().map_or(0, Vec::len)
    }

    /// Duration of the gathered audio.
    fn duration(&self) -> Duration {
        if self.rate == 0 {
            return Duration::ZERO;
        }

        Duration::from_secs_f64(self.frames() as f64 / f64::from(self.rate))
    }

    /// Append one PCM chunk, which is planar with `frames` frames per channel.
    fn push(&mut self, pcm: &PcmAudio, frames: usize) {
        for (index, plane) in self.planes.iter_mut().enumerate() {
            let start = index * frames;
            let end = start + frames;
            plane.extend_from_slice(&pcm.data[start..end]);
        }
    }
}

/// How often the flusher looks for a fragment that has gone quiet.
const FLUSH_TICK: Duration = Duration::from_millis(100);

/// State shared between the sink and the task that sends a fragment after a pause.
struct Shared {
    /// Audio gathered for the fragment that is not sent yet.
    fragment: Mutex<Option<Fragment>>,
    /// When audio was appended last: a pause is measured from here.
    last_audio: Mutex<Instant>,
    /// Sliding window of the messages posted in the last minute.
    sends: SendWindow,
    /// Voice messages posted.
    sent: AtomicU64,
    /// Milliseconds of speech posted.
    spoken_ms: AtomicU64,
}

impl Shared {
    fn new(limit: u32) -> Self {
        Self {
            fragment: Mutex::new(None),
            last_audio: Mutex::new(Instant::now()),
            sends: SendWindow::new(limit),
            sent: AtomicU64::new(0),
            spoken_ms: AtomicU64::new(0),
        }
    }

    fn take(&self) -> Option<Fragment> {
        self.fragment
            .lock()
            .map(|mut fragment| fragment.take())
            .unwrap_or(None)
    }

    fn put(&self, fragment: Fragment) {
        if let Ok(mut slot) = self.fragment.lock() {
            *slot = Some(fragment);
        }
    }

    /// Format and length of the fragment in hand, when there is one.
    fn gathered(&self) -> Option<(u32, usize, Duration)> {
        self.fragment.lock().ok().and_then(|fragment| {
            fragment
                .as_ref()
                .map(|fragment| (fragment.rate, fragment.channels, fragment.duration()))
        })
    }

    /// How long the fragment in hand has been waiting for more audio.
    fn silent_for(&self) -> Duration {
        self.last_audio
            .lock()
            .map(|moment| moment.elapsed())
            .unwrap_or_default()
    }

    /// Mark that audio arrived just now.
    fn touch(&self) {
        if let Ok(mut moment) = self.last_audio.lock() {
            *moment = Instant::now();
        }
    }
}

/// Sends the translated speech into a Telegram group as voice messages.
pub struct TelegramVoiceSink {
    id: String,
    settings: Arc<TelegramVoiceSettings>,
    client: reqwest::Client,
    shared: Arc<Shared>,
    /// Task sending a fragment once the speech pauses.
    flusher: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for TelegramVoiceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramVoiceSink")
            .field("id", &self.id)
            .field("settings", &self.settings)
            .field("sent", &self.sent())
            .field("spoken", &self.spoken_seconds())
            .finish_non_exhaustive()
    }
}

impl TelegramVoiceSink {
    /// Create a sink from ready-made parameters.
    ///
    /// # Errors
    ///
    /// Returns an error when the chat or the token is empty, or when the HTTP
    /// client cannot be built.
    pub fn new(id: &str, settings: TelegramVoiceSettings) -> Result<Self> {
        if settings.chat_id.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустой идентификатор группы Telegram".into(),
            ));
        }
        if settings.token.trim().is_empty() {
            return Err(SinkError::Protocol("пустой токен бота Telegram".into()));
        }

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| SinkError::Unavailable(format!("не удалось создать HTTP-клиент: {e}")))?;

        let shared = Arc::new(Shared::new(settings.max_sends_per_minute));

        Ok(Self {
            id: id.to_string(),
            settings: Arc::new(settings),
            client,
            shared,
            flusher: None,
        })
    }

    /// Create a sink from the configuration, reading the bot token from
    /// `spec.token_env`.
    ///
    /// # Errors
    ///
    /// Returns an error when the chat or the variable name is empty, when the
    /// variable is not set, or when the sink itself cannot be created.
    pub fn from_spec(id: &str, spec: TelegramVoiceSpec<'_>) -> Result<Self> {
        if spec.token_env.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустое имя переменной с токеном бота".into(),
            ));
        }

        let token = std::env::var(spec.token_env).map_err(|_| {
            SinkError::Protocol(format!("не задана переменная окружения {}", spec.token_env))
        })?;

        let settings = TelegramVoiceSettings {
            chat_id: spec.chat_id.to_string(),
            token,
            base: API_BASE.to_string(),
            thread: spec.thread,
            max_chunk: spec.max_chunk,
            gap: spec.gap,
            max_sends_per_minute: spec.max_sends_per_minute,
        };

        Self::new(id, settings)
    }

    /// Number of voice messages posted.
    pub fn sent(&self) -> u64 {
        self.shared.sent.load(Ordering::Relaxed)
    }

    /// Seconds of speech posted.
    pub fn spoken_seconds(&self) -> f64 {
        self.shared.spoken_ms.load(Ordering::Relaxed) as f64 / 1000.0
    }

    /// Start the task that sends a fragment when the speech pauses.
    fn start_flusher(&mut self) {
        if self.flusher.is_some() {
            return;
        }

        let client = self.client.clone();
        let settings = Arc::clone(&self.settings);
        let shared = Arc::clone(&self.shared);

        self.flusher = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(FLUSH_TICK).await;

                // The fragment is complete when no audio has come for a pause:
                // that is what makes the group hear the phrase while the next
                // one is still being spoken, instead of waiting for the end.
                if shared.gathered().is_none() || shared.silent_for() < settings.gap {
                    continue;
                }

                let Some(fragment) = shared.take() else {
                    continue;
                };

                if let Err(e) = deliver(&client, &settings, &shared, fragment).await {
                    tracing::warn!("голосовое сообщение не отправлено: {e}");
                }
            }
        }));
    }

    /// Send the fragment in hand, if there is one.
    async fn flush(&mut self) -> Result<()> {
        match self.shared.take() {
            None => Ok(()),
            Some(fragment) => deliver(&self.client, &self.settings, &self.shared, fragment).await,
        }
    }
}

/// Encode a fragment and post it as one voice message.
async fn deliver(
    client: &reqwest::Client,
    settings: &TelegramVoiceSettings,
    shared: &Shared,
    fragment: Fragment,
) -> Result<()> {
    if fragment.frames() == 0 {
        return Ok(());
    }

    let voice = encode_voice(&fragment.planes, fragment.rate)
        .map_err(|e| SinkError::Protocol(format!("речь не кодируется в Opus: {e}")))?;

    let seconds = fragment.duration();
    post_voice(client, settings, shared, &voice, seconds).await?;

    shared.sent.fetch_add(1, Ordering::Relaxed);
    shared
        .spoken_ms
        .fetch_add(seconds.as_millis() as u64, Ordering::Relaxed);

    Ok(())
}

/// Post one OGG/Opus file as a voice message.
async fn post_voice(
    client: &reqwest::Client,
    settings: &TelegramVoiceSettings,
    shared: &Shared,
    voice: &[u8],
    seconds: Duration,
) -> Result<()> {
    // Telegram counts voice messages against the same minute as text.
    shared.sends.acquire().await;

    let url = format!(
        "{}/bot{}/sendVoice",
        settings.base.trim_end_matches('/'),
        settings.token
    );

    let mut waited_out = false;

    loop {
        // A form is consumed by the request: a retry after a rate limit builds a
        // fresh one from the same bytes.
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", settings.chat_id.clone())
            .text("duration", seconds.as_secs().to_string())
            .part(
                "voice",
                reqwest::multipart::Part::bytes(voice.to_vec())
                    .file_name("phrase.ogg")
                    .mime_str("audio/ogg")
                    .map_err(|e| SinkError::Protocol(format!("не собрана форма: {e}")))?,
            );

        if let Some(thread) = settings.thread {
            form = form.text("message_thread_id", thread.to_string());
        }

        let response = client
            .post(&url)
            .multipart(form)
            .send()
            .await
            // A transport error carries the URL, and the URL carries the token:
            // drop it before the message can reach a log.
            .map_err(|e| {
                SinkError::Unavailable(format!("Telegram недоступен: {}", e.without_url()))
            })?;

        let status = response.status();
        let payload: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);

        if payload["ok"] == serde_json::json!(true) {
            return Ok(());
        }

        let description = payload["description"]
            .as_str()
            .unwrap_or("ответ без описания");

        if status.as_u16() == 429
            && !waited_out
            && let Some(wait) = payload["parameters"]["retry_after"].as_u64()
            && wait <= RETRY_AFTER_LIMIT_SECS
        {
            tracing::warn!("Telegram придержал голосовое сообщение на {wait} с");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            waited_out = true;
            continue;
        }

        return Err(SinkError::Unavailable(format!(
            "Telegram ответил {status}: {description}"
        )));
    }
}

#[async_trait]
impl VoiceSink for TelegramVoiceSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_audio(&mut self, _line: LineId, audio: &Audio) -> Result<()> {
        let pcm = audio
            .to_pcm()
            .map_err(|e| SinkError::Protocol(format!("звук не приводится к PCM: {e}")))?;

        let frames = pcm.frames();
        let channels = pcm.spec.channels().count();
        let rate = pcm.spec.rate();

        if frames == 0 || channels == 0 || rate == 0 {
            return Ok(());
        }

        self.start_flusher();

        let start = audio.capture_timestamp();
        let end = start + audio.duration();

        // A different format, a pause in the stream or a fragment that would grow
        // past the limit closes the fragment in hand right away.
        let closes = match self.shared.gathered() {
            None => false,
            Some((gathered_rate, gathered_channels, gathered_length)) => {
                gathered_rate != rate
                    || gathered_channels != channels
                    || gathered_length + audio.duration() > self.settings.max_chunk
                    || self
                        .shared
                        .fragment
                        .lock()
                        .ok()
                        .and_then(|fragment| {
                            fragment.as_ref().and_then(|fragment| fragment.ends_at)
                        })
                        .is_some_and(|ends| {
                            start.saturating_duration_since(ends) > self.settings.gap
                        })
            }
        };

        if closes {
            self.flush().await?;
        }

        let mut fragment = self
            .shared
            .take()
            .unwrap_or_else(|| Fragment::new(rate, channels));
        fragment.push(&pcm, frames);
        fragment.ends_at = Some(end);
        self.shared.put(fragment);
        self.shared.touch();

        Ok(())
    }

    async fn shutdown(mut self: Box<Self>) -> Result<()> {
        if let Some(flusher) = self.flusher.take() {
            flusher.abort();
        }

        // The last fragment must not be lost when the run ends.
        self.flush().await
    }
}

/// Encode planar PCM into an OGG/Opus stream, which Telegram plays as a voice note.
///
/// # Errors
///
/// Returns the reason the encoder or the container refused the audio.
pub fn encode_voice(planes: &[Vec<f32>], rate: u32) -> std::result::Result<Vec<u8>, String> {
    let channels = planes.len();
    if channels == 0 || rate == 0 {
        return Err("пустой звук".into());
    }

    let frames = planes.iter().map(Vec::len).min().unwrap_or(0);
    if frames == 0 {
        return Err("пустой звук".into());
    }

    let mut encoder = OpusEncoder::new(rate as i32, channels, Application::Voip)
        .map_err(|e| format!("кодек не открылся: {e}"))?;
    encoder.bitrate_bps = VOICE_BITRATE_BPS;

    let head = OpusHead::for_encoder(&encoder, rate);
    let mut writer =
        OggOpusWriter::new(Vec::new(), head).map_err(|e| format!("контейнер не открылся: {e}"))?;

    let block = (rate / 1000 * FRAME_MS) as usize;
    let mut packet = vec![0u8; MAX_PACKET_BYTES];
    let mut interleaved = vec![0.0f32; block * channels];

    let mut offset = 0usize;
    while offset < frames {
        let take = block.min(frames - offset);

        for (channel, plane) in planes.iter().enumerate() {
            for index in 0..block {
                // The tail of the last block is padded with silence.
                let sample = if index < take {
                    plane[offset + index]
                } else {
                    0.0
                };
                interleaved[index * channels + channel] = sample;
            }
        }

        let written = encoder
            .encode(&interleaved, block, &mut packet)
            .map_err(|e| format!("кадр не закодирован: {e}"))?;
        writer
            .write_packet(&packet[..written])
            .map_err(|e| format!("пакет не записан в контейнер: {e}"))?;

        offset += take;
    }

    writer
        .finish()
        .map_err(|e| format!("контейнер не закрылся: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Router, body::Bytes, routing::post};
    use opus_pure::OggOpusReader;
    use transono::audio::{AudioFormat, Endianness, PcmFormat};

    use super::*;

    /// Test token: the request paths carry it, and it must stay out of errors.
    const TEST_TOKEN: &str = "123456:VOICE-TOKEN";

    /// One request the stand-in received.
    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        body: Vec<u8>,
    }

    type Captured = Arc<Mutex<Vec<Seen>>>;

    /// Bot API stand-in answering everything with `reply`.
    async fn stand_in(reply: fn() -> (u16, serde_json::Value)) -> (String, Captured) {
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let seen = captured.clone();

        let app = Router::new().route(
            "/bot123456:VOICE-TOKEN/sendVoice",
            post(move |body: Bytes| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(Seen {
                        path: "/sendVoice".into(),
                        body: body.to_vec(),
                    });
                    let (code, payload) = reply();
                    (
                        axum::http::StatusCode::from_u16(code).unwrap(),
                        axum::Json(payload),
                    )
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{addr}"), captured)
    }

    fn ok() -> (u16, serde_json::Value) {
        (
            200,
            serde_json::json!({ "ok": true, "result": { "message_id": 1 } }),
        )
    }

    fn chat_not_found() -> (u16, serde_json::Value) {
        (
            400,
            serde_json::json!({"ok": false, "error_code": 400, "description": "Bad Request: chat not found"}),
        )
    }

    fn sink(base: &str, max_chunk: Duration, gap: Duration) -> TelegramVoiceSink {
        let mut settings = TelegramVoiceSettings::new("-1001234567890", TEST_TOKEN);
        settings.base = base.to_string();
        settings.max_chunk = max_chunk;
        settings.gap = gap;
        TelegramVoiceSink::new("telegram-voice", settings).unwrap()
    }

    /// A chunk of a sine wave, as the provider returns speech.
    fn speech(rate: u32, millis: u64, at: Instant) -> Audio {
        let frames = (rate as u64 * millis / 1000) as usize;
        let format = AudioFormat {
            sample_rate: rate,
            channels: 1,
            sample_format: PcmFormat::F32(Endianness::Little),
        };

        let mut pcm = PcmAudio::new(format.spec(), frames);
        pcm.capture_timestamp = at;

        for index in 0..frames {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / rate as f32;
            pcm.data[index] = 0.2 * phase.sin();
        }

        Audio::from_pcm(&pcm).unwrap()
    }

    #[test]
    fn a_stream_is_encoded_as_ogg_opus() {
        let rate = 24_000;
        let plane = vec![0.1f32; rate as usize];

        let voice = encode_voice(&[plane], rate).unwrap();

        assert_eq!(&voice[..4], b"OggS", "это контейнер Ogg");
        assert!(voice.len() > 100, "звук не пустой: {} байт", voice.len());
    }

    #[test]
    fn an_encoded_stream_reads_back() {
        let rate = 24_000;
        let frames = rate as usize / 2;
        let mut plane = vec![0.0f32; frames];
        for (index, sample) in plane.iter_mut().enumerate() {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / rate as f32;
            *sample = 0.3 * phase.sin();
        }

        let voice = encode_voice(&[plane], rate).unwrap();

        let mut reader = OggOpusReader::new(std::io::Cursor::new(&voice)).unwrap();
        let head = reader.head().clone();
        let mut decoder = head.decoder(rate as i32).unwrap();

        let mut decoded = 0usize;
        let mut energy = 0.0f64;
        let mut block = vec![0.0f32; opus_pure::MAX_PACKET_SAMPLES];
        for packet in reader.packets() {
            let packet = packet.unwrap();
            let produced = decoder
                .decode(&packet.data, opus_pure::MAX_PACKET_SAMPLES, &mut block)
                .unwrap();
            decoded += produced;
            energy += block[..produced]
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>();
        }

        // The encoder pads its last frame and keeps its own delay, so the count
        // lands within a frame or two of what went in.
        let expected = frames;
        assert!(
            decoded.abs_diff(expected) <= 2 * (rate as usize / 50),
            "декодировалось {decoded} кадров из {expected}"
        );

        // Sound came back, not silence: the sine went in at amplitude 0.3, which
        // is an RMS near 0.21, and the codec keeps it well above a tenth.
        let rms = (energy / decoded as f64).sqrt();
        assert!(rms > 0.1, "декодированный звук не тишина: RMS {rms}");
    }

    /// Wait until the stand-in has seen `count` requests, or give up and hand back
    /// what it has: the flusher sends on its own schedule.
    async fn wait_for(captured: &Captured, count: usize) -> Vec<Seen> {
        for _ in 0..150 {
            let seen = captured.lock().unwrap().clone();
            if seen.len() >= count {
                return seen;
            }
            // Yielding keeps the runtime free to run the flusher task.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        captured.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_fragment_is_sent_as_a_voice_note() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::from_secs(45), Duration::from_millis(300));

        let start = Instant::now();
        sink.on_audio(LineId(0), &speech(24_000, 500, start))
            .await
            .unwrap();
        sink.on_audio(
            LineId(0),
            &speech(24_000, 500, start + Duration::from_secs(2)),
        )
        .await
        .unwrap();

        let requests = wait_for(&seen, 2).await;
        assert_eq!(requests.len(), 2, "пауза закрывает фрагмент");

        let body = &requests[0].body;
        assert_eq!(requests[0].path, "/sendVoice", "метод Bot API");
        let text = String::from_utf8_lossy(body);
        assert!(text.contains("name=\"voice\""), "файл в поле voice");
        assert!(text.contains("phrase.ogg"), "имя файла с расширением ogg");
        assert!(text.contains("-1001234567890"), "адресат из настроек");
        assert!(
            body.windows(4).any(|window| window == b"OggS"),
            "внутри — поток Ogg"
        );
        assert_eq!(sink.sent(), 2);
        assert!(sink.spoken_seconds() > 0.9, "{}", sink.spoken_seconds());
    }

    #[tokio::test]
    async fn audio_without_a_pause_stays_one_message() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::from_secs(45), Duration::from_millis(500));

        let start = Instant::now();
        for index in 0..4u64 {
            sink.on_audio(
                LineId(0),
                &speech(24_000, 200, start + Duration::from_millis(index * 200)),
            )
            .await
            .unwrap();
        }

        assert!(
            seen.lock().unwrap().is_empty(),
            "без паузы фрагмент ещё не отправлен"
        );

        // The pause closes the fragment: it goes out by itself, without waiting
        // for more speech or for the end of the run.
        let requests = wait_for(&seen, 1).await;
        assert_eq!(
            requests.len(),
            1,
            "одно голосовое сообщение на весь фрагмент"
        );

        // And nothing is left over once the run ends.
        Box::new(sink).shutdown().await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1, "повторной отправки нет");
    }

    #[tokio::test]
    async fn a_long_stream_is_cut_into_voice_messages() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::from_millis(400), Duration::from_secs(10));

        let start = Instant::now();
        for index in 0..5u64 {
            sink.on_audio(
                LineId(0),
                &speech(24_000, 200, start + Duration::from_millis(index * 200)),
            )
            .await
            .unwrap();
        }

        let requests = seen.lock().unwrap().len();
        assert!(requests >= 2, "предел фрагмента режет поток: {requests}");
    }

    #[tokio::test]
    async fn a_refusal_is_reported_with_the_reason_and_without_the_token() {
        let (base, _seen) = stand_in(chat_not_found).await;
        let mut sink = sink(&base, Duration::from_secs(45), Duration::from_millis(300));

        let start = Instant::now();
        sink.on_audio(LineId(0), &speech(24_000, 200, start))
            .await
            .unwrap();
        let err = Box::new(sink).shutdown().await.unwrap_err();

        let text = err.to_string();
        assert!(text.contains("chat not found"), "причина сохранена: {text}");
        assert!(
            !text.contains(TEST_TOKEN),
            "токен не должен попасть в лог: {text}"
        );
    }

    #[test]
    fn an_empty_chat_is_rejected() {
        let err =
            TelegramVoiceSink::new("t", TelegramVoiceSettings::new("  ", TEST_TOKEN)).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn a_missing_variable_is_named() {
        let err = TelegramVoiceSink::from_spec(
            "t",
            TelegramVoiceSpec {
                chat_id: "-100",
                token_env: "AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN",
                thread: None,
                max_chunk: DEFAULT_MAX_CHUNK,
                gap: DEFAULT_GAP,
                max_sends_per_minute: DEFAULT_SENDS_PER_MINUTE,
            },
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN"),
            "{err}"
        );
    }
}
