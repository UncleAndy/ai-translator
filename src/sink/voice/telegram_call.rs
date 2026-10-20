//! Telegram voice-chat sink: the translated speech goes into a group call.
//!
//! Telegram carries live audio only inside a group call — a voice chat in a group
//! or a livestream in a channel — and a bot cannot join one. The sink therefore
//! acts as the account whose session `telegram-login` saved: it resolves the group
//! or channel, finds the call there or starts one, joins it with the WebRTC
//! parameters the media engine builds, and feeds the speech of the translation
//! into the call frame by frame.
//!
//! Two halves meet here:
//!
//! * MTProto (through `grammers`) — who the account is, which call exists,
//!   `phone.joinGroupCall`, and the connection parameters Telegram answers with;
//! * the media engine (`ntgcalls`) — DTLS-SRTP, RTP and Opus, plus the external
//!   PCM source this sink pushes into it.
//!
//! The engine is not shareable between threads, so it lives in one task of its
//! own: the sink sends it commands — join, a frame of speech, leave — and never
//! touches the engine directly.
//!
//! The speech arrives as chunks of whatever rate the provider produced; the
//! packer resamples it to the 48 kHz mono the call expects and cuts it into
//! 20 ms frames. The task sends one frame every 20 ms, so the call hears the
//! translation at the speed it was spoken; when the translation pauses, silence
//! keeps the stream alive instead of tearing the connection down.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use grammers_client::Client;
use grammers_client::peer::Peer;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use grammers_tl_types as tl;
use ntgcalls::{
    AudioDescription, FrameData, MediaDescription, MediaSource, NTgCalls, StreamDevice, StreamMode,
    VideoRotation,
};
use tokio::sync::{mpsc, oneshot};
use transono::audio::Audio;

use crate::bus::LineId;
use crate::sink::{Result, SinkError, VoiceSink};

/// Rate the call expects.
pub const CALL_RATE: u32 = 48_000;

/// One frame of the call: ten milliseconds, four hundred and eighty samples.
///
/// The engine hands its own sink a frame every ten milliseconds and takes a
/// frame of that length from the outside. A longer chunk at a longer interval
/// leaves every second slot without audio, which is heard as clicking and
/// stuttering.
pub const FRAME_SAMPLES: usize = (CALL_RATE as usize / 1000) * 10;

/// How often a frame goes out: the engine consumes one every ten milliseconds.
const FRAME_INTERVAL: Duration = Duration::from_millis(10);

/// How many frames may wait for the call before the oldest is given up.
///
/// The provider hands a phrase over faster than it is spoken, so the sink has to
/// hold it and give it to the call in real time. Dropping part of a phrase is
/// what makes the broadcast stutter and click, therefore the backlog is measured
/// in tens of seconds and only a runaway stream hits the limit.
const MAX_QUEUED_FRAMES: usize = (1000 / 10) * 60;

/// How many frames of speech must gather before the call starts speaking them:
/// a hundred milliseconds of cushion against an uneven provider.
const PRIME_FRAMES: usize = 10;

/// How long the sound fades between speech and silence, in samples of the call.
const EDGE_FADE: usize = (CALL_RATE as usize / 1000) * 5;

/// How far apart two neighbouring samples of the speech may be at a seam before
/// the join is smoothed, in units of the full scale.
const SEAM_JUMP: f32 = 0.08;

/// How long the account is given to enter the call. Without a bound a join that
/// Telegram never answers holds the sink in limbo that looks exactly like a
/// working one.
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

/// How many frames in a row the call may refuse before it is called gone. Twenty
/// frames are a fifth of a second: a call that lags that long has dropped out,
/// and silence from now on would be lost without a word.
const FAILED_FRAMES_BEFORE_RESTART: u32 = 20;

/// How often a refused frame is reported. The task produces a hundred frames a
/// second, and a hundred lines of log per second hide the reason instead of
/// telling it.
const FAILURE_REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Parameters of the voice-chat sink.
pub struct TelegramCallSettings {
    /// Group or channel whose call carries the broadcast.
    pub chat_id: String,
    /// `api_id` of the application.
    pub api_id: i32,
    /// `api_hash` of the application.
    pub api_hash: String,
    /// Session file written by `telegram-login`.
    pub session_file: PathBuf,
    /// Start the call when the group or channel has none active.
    pub create_if_missing: bool,
}

impl std::fmt::Debug for TelegramCallSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramCallSettings")
            .field("chat_id", &self.chat_id)
            .field("api_id", &self.api_id)
            .field("session_file", &self.session_file)
            .field("create_if_missing", &self.create_if_missing)
            .finish_non_exhaustive()
    }
}

/// Turns the speech of the translation into frames of the call.
///
/// Resampling is linear: the speech is understood by ear, not shown as a signal,
/// so the cheapest interpolation that keeps the timbre is enough. Chunks are
/// interpolated inside themselves, which costs at most one sample at a seam of a
/// few milliseconds and never shifts the timeline.
///
/// A provider hands the phrase over in chunks that do not meet sample to sample:
/// the last sample of one chunk and the first of the next may sit far apart, and
/// a hard join there is heard as a click. Such a seam is bridged by a short
/// ramp, and only when the jump is audible — chunks that continue each other are
/// left untouched.
#[derive(Debug)]
pub struct Packer {
    in_rate: u32,
    /// Read position inside the stream, in input samples.
    position: f64,
    /// Output samples that do not fill a frame yet.
    pending: Vec<i16>,
    /// Last input sample of the previous chunk, for the seam check.
    last: Option<f32>,
}

impl Packer {
    /// A packer for speech arriving at `in_rate`.
    pub fn new(in_rate: u32) -> Self {
        Self {
            in_rate,
            position: 0.0,
            pending: Vec::new(),
            last: None,
        }
    }

    /// Append one chunk of mono speech.
    pub fn push(&mut self, mono: &[f32]) {
        if mono.is_empty() || self.in_rate == 0 {
            return;
        }

        match self.bridged(mono) {
            Some(joined) => self.push_bridged(&joined),
            None => self.push_bridged(mono),
        }
    }

    /// A version of the chunk with a ramp in front, when it starts far from where
    /// the previous one ended.
    fn bridged(&self, mono: &[f32]) -> Option<Vec<f32>> {
        let previous = self.last?;
        let first = mono[0];

        if (first - previous).abs() <= SEAM_JUMP {
            return None;
        }

        // Two milliseconds of the input rate: long enough to remove the step,
        // short enough to stay inaudible.
        let length = (self.in_rate as usize / 500).max(2);
        let mut joined: Vec<f32> = (0..length)
            .map(|index| {
                let fraction = index as f32 / length as f32;
                previous + (first - previous) * fraction
            })
            .collect();
        joined.extend_from_slice(mono);

        Some(joined)
    }

    /// Append samples that have already been joined.
    fn push_bridged(&mut self, mono: &[f32]) {
        let step = f64::from(self.in_rate) / f64::from(CALL_RATE);
        let mut position = self.position;
        let last = mono.len() as f64;

        while position < last {
            let index = position.floor() as usize;
            let fraction = position - index as f64;
            let current = f64::from(mono[index]);
            let next = mono
                .get(index + 1)
                .map_or(current, |sample| f64::from(*sample));
            let value = current + (next - current) * fraction;

            self.pending
                .push((value.clamp(-1.0, 1.0) * f64::from(i16::MAX)) as i16);
            position += step;
        }

        self.position = position - last;
        self.last = mono.last().copied();
    }

    /// Take one frame, when a whole one has gathered.
    pub fn frame(&mut self) -> Option<Vec<i16>> {
        if self.pending.len() < FRAME_SAMPLES {
            return None;
        }

        Some(self.pending.drain(..FRAME_SAMPLES).collect())
    }

    /// How much speech is waiting for a frame, in samples.
    pub fn backlog(&self) -> usize {
        self.pending.len()
    }
}

/// What the task that owns the engine is asked to do.
enum Command {
    /// Resolve the group or channel, join its call and start streaming.
    Join {
        settings: Box<TelegramCallSettings>,
        reply: oneshot::Sender<std::result::Result<(), String>>,
    },
    /// One frame of translated speech.
    Frame(Vec<i16>),
    /// Leave the call and stop.
    Leave { reply: oneshot::Sender<()> },
}

/// What the frame task knows about the call, and the sink needs to know too.
///
/// The engine lives on its own thread and cannot hand the sink an error: it is
/// the sink's `on_audio` that must fail for the supervisor to restart it. This
/// cell is the shortest honest bridge between the two — the task writes the
/// reason, the sink reads it at the next frame of speech.
#[derive(Debug, Default)]
struct CallHealth {
    broken: std::sync::Mutex<Option<String>>,
}

impl CallHealth {
    /// Call the stream dead, keeping the first reason: later frames of the same
    /// broken call have nothing to add.
    fn mark_broken(&self, reason: String) {
        let mut slot = self
            .broken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(reason);
        }
    }

    /// The reason the stream died, if it has.
    fn reason(&self) -> Option<String> {
        self.broken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Forget an old failure: a new join starts with a clean slate.
    fn reset(&self) {
        self.broken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }
}

/// What to do about a frame the call did not take.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// Say it out loud, no more often than once a second.
    Report,
    /// Enough: the call is not taking audio any more.
    GiveUp,
}

/// Counts frames the call refused in a row.
#[derive(Debug, Default)]
struct FrameFailures {
    consecutive: u32,
    last_reported: Option<tokio::time::Instant>,
}

impl FrameFailures {
    /// Note a refused frame and say what, if anything, deserves a word.
    fn record(&mut self, now: tokio::time::Instant) -> Option<Failure> {
        self.consecutive += 1;

        if self.consecutive >= FAILED_FRAMES_BEFORE_RESTART {
            return Some(Failure::GiveUp);
        }

        let due = self
            .last_reported
            .is_none_or(|at| now.duration_since(at) >= FAILURE_REPORT_INTERVAL);

        if due {
            self.last_reported = Some(now);
            Some(Failure::Report)
        } else {
            None
        }
    }

    /// The call took a frame: the stream is alive.
    fn recovered(&mut self) {
        self.consecutive = 0;
    }

    /// How many frames in a row were refused.
    fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

/// The call the task is streaming into.
struct Live {
    call_id: i64,
    client: Client,
    pool: tokio::task::JoinHandle<()>,
}

/// Sends the translated speech into a Telegram voice chat.
pub struct TelegramCallSink {
    id: String,
    settings: TelegramCallSettings,
    task: Option<mpsc::UnboundedSender<Command>>,
    packer: Option<Packer>,
    /// Filled by the frame task when the call stops taking audio.
    health: Arc<CallHealth>,
}

impl std::fmt::Debug for TelegramCallSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramCallSink")
            .field("id", &self.id)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl TelegramCallSink {
    /// Create the sink from ready-made parameters.
    ///
    /// # Errors
    ///
    /// Returns an error when the chat is empty or the session file is missing:
    /// the account has to sign in first.
    pub fn new(id: &str, settings: TelegramCallSettings) -> Result<Self> {
        if settings.chat_id.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустой идентификатор группы или канала Telegram".into(),
            ));
        }
        if !settings.session_file.exists() {
            return Err(SinkError::Protocol(format!(
                "нет файла сессии {}: сначала выполните `ai-translator telegram-login`",
                settings.session_file.display()
            )));
        }

        Ok(Self {
            id: id.to_string(),
            settings,
            task: None,
            packer: None,
            health: Arc::new(CallHealth::default()),
        })
    }

    /// Declare the call dead: the next frame of speech fails the sink, and the
    /// supervisor brings up a fresh one that joins again.
    ///
    /// The frame task holds the health cell itself and does not need this; the
    /// method is the way for anyone else — a test, or a future supervisor of the
    /// call — to report that the stream is gone.
    pub fn report_broken(&self, reason: impl Into<String>) {
        self.health.mark_broken(reason.into());
    }

    /// Start the task that owns the engine and wait until the call is joined.
    async fn join(&mut self) -> Result<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.health.reset();
        spawn_engine(rx, Arc::clone(&self.health))
            .map_err(|e| SinkError::Unavailable(format!("движок звонка не запустился: {e}")))?;

        let (reply, answer) = oneshot::channel();
        let settings = Box::new(TelegramCallSettings {
            chat_id: self.settings.chat_id.clone(),
            api_id: self.settings.api_id,
            api_hash: self.settings.api_hash.clone(),
            session_file: self.settings.session_file.clone(),
            create_if_missing: self.settings.create_if_missing,
        });

        tx.send(Command::Join { settings, reply })
            .map_err(|_| SinkError::Unavailable("движок звонка не запустился".into()))?;

        match answer.await {
            Ok(Ok(())) => {
                self.task = Some(tx);
                Ok(())
            }
            Ok(Err(reason)) => Err(SinkError::Protocol(reason)),
            Err(_) => Err(SinkError::Unavailable(
                "движок звонка не ответил на вход в звонок".into(),
            )),
        }
    }
}

#[async_trait]
impl VoiceSink for TelegramCallSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_audio(&mut self, _line: LineId, audio: &Audio) -> Result<()> {
        // The frame task cannot fail the sink itself: it says why the call died
        // here, and the supervisor takes it from there — a fresh sink joins anew.
        if let Some(reason) = self.health.reason() {
            return Err(SinkError::Unavailable(reason));
        }

        if self.task.is_none() {
            tracing::info!(
                "первый звук у приёмника: вхожу в звонок {}",
                self.settings.chat_id
            );
            self.join().await?;
        }

        let pcm = audio
            .to_pcm()
            .map_err(|e| SinkError::Protocol(format!("звук не приводится к PCM: {e}")))?;

        let frames = pcm.frames();
        if frames == 0 {
            return Ok(());
        }

        let channels = pcm.spec.channels().count().max(1);

        // The call is mono: the channels of the provider are mixed down.
        let mut mono = Vec::with_capacity(frames);
        for index in 0..frames {
            let mut sum = 0.0f32;
            for channel in 0..channels {
                sum += pcm.data[channel * frames + index];
            }
            mono.push(sum / channels as f32);
        }

        let rate = pcm.spec.rate();
        match &mut self.packer {
            Some(packer) if packer.in_rate == rate => packer.push(&mono),
            _ => {
                let mut packer = Packer::new(rate);
                packer.push(&mono);
                self.packer = Some(packer);
            }
        }

        let Some(packer) = self.packer.as_mut() else {
            return Ok(());
        };
        let Some(task) = self.task.as_ref() else {
            return Ok(());
        };

        while let Some(frame) = packer.frame() {
            // A full queue is not an error: the newest speech wins over a
            // backlog that would otherwise translate the past. A closed channel
            // is: the engine task is gone, and nothing will carry the speech.
            if task.send(Command::Frame(frame)).is_err() {
                return Err(SinkError::Unavailable(
                    "движок звонка остановился: трансляция прервана".into(),
                ));
            }
        }

        Ok(())
    }

    async fn shutdown(mut self: Box<Self>) -> Result<()> {
        if let Some(task) = self.task.take() {
            let (reply, answer) = oneshot::channel();
            if task.send(Command::Leave { reply }).is_ok() {
                let _ = answer.await;
            }
            tracing::info!("трансляция остановлена, звонок покинут");
        }

        Ok(())
    }
}

/// Run the engine task on a thread of its own.
///
/// The engine cannot leave its thread, so the task gets a whole runtime instead
/// of a slot in the shared one; the sink talks to it through the channel.
fn spawn_engine(
    commands: mpsc::UnboundedReceiver<Command>,
    health: Arc<CallHealth>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("telegram-call".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!("движок звонка не запустился: {error}");
                    return;
                }
            };

            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(engine_task(commands, health)));
        })
        .map(|_| ())
}

/// The task that owns the media engine: joins the call, then sends it frames.
async fn engine_task(mut commands: mpsc::UnboundedReceiver<Command>, health: Arc<CallHealth>) {
    let engine = NTgCalls::new();
    let mut failures = FrameFailures::default();
    let mut live: Option<Live> = None;
    let mut queue: std::collections::VecDeque<Vec<i16>> = std::collections::VecDeque::new();
    let silence = vec![0i16; FRAME_SAMPLES];
    // The next frame is due at an absolute moment: a send that took longer than
    // the interval must not make the frames after it hurry one after another,
    // which is heard as a burst and a dropout.
    let mut due = tokio::time::Instant::now();
    // Where the speech last stopped, so silence can start from there.
    let mut tail = 0i16;
    let mut given_up = 0usize;
    // Speech starts only when a cushion has gathered: the provider hands the
    // phrase over unevenly, and starting on the very first frame makes the call
    // run dry in the middle of a word.
    let mut primed = false;

    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    Some(Command::Join { settings, reply }) => {
                        // A join Telegram never answers must not hold the sink in
                        // a limbo that looks like a working broadcast.
                        let answer =
                            tokio::time::timeout(JOIN_TIMEOUT, join_call(&engine, &settings)).await;
                        match answer {
                            Ok(Ok(joined)) => {
                                live = Some(joined);
                                failures = FrameFailures::default();
                                due = tokio::time::Instant::now();
                                primed = false;
                                let _ = reply.send(Ok(()));
                            }
                            Ok(Err(reason)) => {
                                let _ = reply.send(Err(reason));
                            }
                            Err(_elapsed) => {
                                let _ = reply.send(Err(format!(
                                    "Telegram не принял вход в звонок за {} с",
                                    JOIN_TIMEOUT.as_secs()
                                )));
                            }
                        }
                    }
                    Some(Command::Frame(frame)) => {
                        while queue.len() >= MAX_QUEUED_FRAMES {
                            queue.pop_front();
                            given_up += 1;
                        }
                        if given_up > 0 {
                            tracing::warn!(
                                "звонок отстаёт: {} кадров речи пропущено",
                                given_up
                            );
                            given_up = 0;
                        }
                        queue.push_back(frame);
                    }
                    Some(Command::Leave { reply }) => {
                        if let Some(joined) = live.take() {
                            leave_call(&engine, joined).await;
                        }
                        queue.clear();
                        primed = false;
                        let _ = reply.send(());
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(due), if live.is_some() => {
                let Some(call_id) = live.as_ref().map(|joined| joined.call_id) else {
                    continue;
                };

                if !primed && queue.len() >= PRIME_FRAMES {
                    primed = true;
                }

                let frame = if primed && !queue.is_empty() {
                    let frame = queue.pop_front().expect("очередь не пуста");
                    tail = frame.last().copied().unwrap_or(0);
                    frame
                } else {
                    if primed {
                        primed = false;
                    }
                    // Nothing is waiting: the call hears silence that starts
                    // where the speech ended, not a step down to zero.
                    let frame = fade_out(tail, &silence);
                    tail = 0;
                    frame
                };

                match send_frame(&engine, call_id, &frame).await {
                    Ok(()) => failures.recovered(),
                    Err(error) => match failures.record(tokio::time::Instant::now()) {
                        Some(Failure::Report) => tracing::warn!(
                            "кадр не отправлен в звонок ({} подряд): {error}",
                            failures.consecutive()
                        ),
                        Some(Failure::GiveUp) => {
                            // Silence from a dead call is not a broadcast: leave
                            // it and let the sink fail, so the supervisor brings
                            // up a fresh one that joins again.
                            tracing::error!(
                                "звонок не принимает звук ({} кадров подряд): {error}; \
                                 выхожу из звонка, приёмник будет перезапущен",
                                failures.consecutive()
                            );
                            health.mark_broken(format!("звонок не принимает звук: {error}"));
                            if let Some(joined) = live.take() {
                                leave_call(&engine, joined).await;
                            }
                            queue.clear();
                            primed = false;
                            tail = 0;
                        }
                        None => {}
                    },
                }

                due += FRAME_INTERVAL;
                let now = tokio::time::Instant::now();
                if due < now {
                    due = now;
                }
            }
        }
    }

    if let Some(joined) = live.take() {
        leave_call(&engine, joined).await;
    }
}

/// Silence that starts at the level the speech stopped at and fades down, so the
/// join between them is not a step.
fn fade_out(tail: i16, silence: &[i16]) -> Vec<i16> {
    if tail == 0 {
        return silence.to_vec();
    }

    let mut frame = silence.to_vec();
    let length = EDGE_FADE.min(frame.len());

    for (index, sample) in frame.iter_mut().take(length).enumerate() {
        let fraction = 1.0 - index as f32 / length as f32;
        *sample = (f32::from(tail) * fraction) as i16;
    }

    frame
}

/// Put one frame of speech into the call.
async fn send_frame(
    engine: &NTgCalls,
    call_id: i64,
    frame: &[i16],
) -> std::result::Result<(), String> {
    let mut bytes = Vec::with_capacity(frame.len() * 2);
    for sample in frame {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    engine
        .send_external_frame(
            call_id,
            StreamDevice::Microphone,
            &bytes,
            &FrameData {
                absolute_capture_timestamp_ms: 0,
                rotation: VideoRotation::VideoRotation0,
                width: 0,
                height: 0,
            },
        )
        .await
        .map_err(|e| e.to_string())
}

/// Resolve the group or channel and join the call it carries.
async fn join_call(
    engine: &NTgCalls,
    settings: &TelegramCallSettings,
) -> std::result::Result<Live, String> {
    let session = Arc::new(
        SqliteSession::open(&settings.session_file)
            .await
            .map_err(|e| {
                format!(
                    "файл сессии не открывается ({}): {e}",
                    settings.session_file.display()
                )
            })?,
    );

    let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), settings.api_id);
    let client = Client::new(handle);
    let pool = tokio::spawn(runner.run());

    match enter_call(engine, &client, settings).await {
        Ok(call_id) => Ok(Live {
            call_id,
            client,
            pool,
        }),
        Err(reason) => {
            // The session and its pool belong to the call that never happened.
            drop(client);
            let _ = pool.await;
            Err(reason)
        }
    }
}

/// The steps of the join itself, once the account is connected.
async fn enter_call(
    engine: &NTgCalls,
    client: &Client,
    settings: &TelegramCallSettings,
) -> std::result::Result<i64, String> {
    if !client
        .is_authorized()
        .await
        .map_err(|e| format!("Telegram недоступен: {e}"))?
    {
        return Err("сессия не авторизована: выполните `ai-translator telegram-login`".to_string());
    }

    let (peer, call) = find_or_create_call(client, settings).await?;
    let call_id = peer_id(&peer);

    // The engine builds its WebRTC parameters; Telegram answers with its own,
    // which the engine then completes into a media connection.
    let local = engine
        .create_call(call_id)
        .await
        .map_err(|e| format!("движок не создал параметры звонка: {e}"))?;

    tracing::info!("движок: параметры звонка получены, вхожу в звонок");

    let answer = client
        .invoke(&tl::functions::phone::JoinGroupCall {
            muted: false,
            video_stopped: false,
            call,
            join_as: input_peer_of(&peer).await?,
            invite_hash: None,
            public_key: None,
            block: None,
            params: tl::enums::DataJson::Json(tl::types::DataJson { data: local }),
        })
        .await
        .map_err(|e| format!("вход в звонок не выполнен: {e}"))?;

    tracing::info!("вход в звонок: ответ Telegram получен");

    let remote = connection_params(&answer)
        .ok_or_else(|| "Telegram не прислал параметры соединения звонка".to_string())?;

    engine
        .connect(call_id, &remote, false)
        .await
        .map_err(|e| format!("движок не подключился к звонку: {e}"))?;

    tracing::info!("движок: соединение установлено");

    engine
        .set_stream_sources(
            call_id,
            StreamMode::Capture,
            &MediaDescription {
                microphone: Some(AudioDescription {
                    media_source: MediaSource::External,
                    sample_rate: CALL_RATE,
                    channel_count: 1,
                    input: String::new(),
                    keep_open: true,
                }),
                speaker: None,
                camera: None,
                screen: None,
            },
        )
        .await
        .map_err(|e| format!("движок не принял источник звука: {e}"))?;

    tracing::info!("движок: источник звука принят");

    tracing::info!("трансляция идёт в {}", settings.chat_id);

    Ok(call_id)
}
async fn leave_call(engine: &NTgCalls, joined: Live) {
    let _ = engine.stop(joined.call_id).await;
    drop(joined.client);
    let _ = joined.pool.await;
}

/// Find the call of the group or channel, starting one when allowed.
async fn find_or_create_call(
    client: &Client,
    settings: &TelegramCallSettings,
) -> std::result::Result<(Peer, tl::enums::InputGroupCall), String> {
    let name = settings.chat_id.trim();
    let username = name.strip_prefix('@').unwrap_or(name);

    let peer = client
        .resolve_username(username)
        .await
        .map_err(|e| format!("{name} не найден: {e}"))?
        .ok_or_else(|| {
            format!("группа или канал {name} не найдены: проверьте имя и что аккаунт в них состоит")
        })?;

    let mut call = active_call(client, &peer).await?;

    if call.is_none() && settings.create_if_missing {
        client
            .invoke(&tl::functions::phone::CreateGroupCall {
                rtmp_stream: false,
                peer: input_peer_of(&peer).await?,
                random_id: random_id(),
                title: None,
                schedule_date: None,
            })
            .await
            .map_err(|e| {
                format!("голосовой чат не создан (нужно право управления видеочатами): {e}")
            })?;

        // The call appears through an update, so it is read back from the group.
        call = active_call(client, &peer).await?;
    }

    match call {
        Some(call) => Ok((peer, call)),
        None => Err(format!(
            "в {name} нет активного голосового чата, а создавать его запрещено \
             (create_if_missing = false)"
        )),
    }
}

/// The call currently attached to the group or channel.
async fn active_call(
    client: &Client,
    peer: &Peer,
) -> std::result::Result<Option<tl::enums::InputGroupCall>, String> {
    let full = match channel_of(peer).await? {
        Some(channel) => {
            let answer = client
                .invoke(&tl::functions::channels::GetFullChannel { channel })
                .await
                .map_err(|e| format!("канал не прочитан: {e}"))?;
            let tl::enums::messages::ChatFull::Full(full) = answer;
            full.full_chat
        }
        None => {
            let chat_id = peer.id().bare_id().unwrap_or_default();
            let answer = client
                .invoke(&tl::functions::messages::GetFullChat { chat_id })
                .await
                .map_err(|e| format!("группа не прочитана: {e}"))?;
            let tl::enums::messages::ChatFull::Full(full) = answer;
            full.full_chat
        }
    };

    Ok(match full {
        tl::enums::ChatFull::Full(chat) => chat.call,
        tl::enums::ChatFull::ChannelFull(chat) => chat.call,
        // A community carries no group call of its own.
        tl::enums::ChatFull::CommunityFull(_) => None,
    })
}

/// Identifier of the peer as a bare signed number.
fn peer_id(peer: &Peer) -> i64 {
    let raw = peer.id().bare_id().unwrap_or_default();

    // Channels and megagroups are addressed with the -100 prefix in MTProto.
    match peer {
        Peer::Channel(_) | Peer::Community(_) => -(1_000_000_000_000 + raw),
        _ => raw,
    }
}

/// The peer as the input reference the phone methods expect.
async fn input_peer_of(peer: &Peer) -> std::result::Result<tl::enums::InputPeer, String> {
    let reference = peer
        .to_ref()
        .await
        .map_err(|e| format!("ссылка на группу или канал не получена: {e}"))?
        .ok_or_else(|| "Telegram не дал доступ к группе или каналу".to_string())?;

    let hash = reference.auth.hash();
    let id = reference.id.bare_id().unwrap_or_default();

    Ok(match peer {
        Peer::User(_) => tl::enums::InputPeer::User(tl::types::InputPeerUser {
            user_id: id,
            access_hash: hash,
        }),
        Peer::Group(_) => tl::enums::InputPeer::Chat(tl::types::InputPeerChat { chat_id: id }),
        Peer::Channel(_) | Peer::Community(_) => {
            tl::enums::InputPeer::Channel(tl::types::InputPeerChannel {
                channel_id: id,
                access_hash: hash,
            })
        }
    })
}

/// The channel input reference, when the peer is a channel.
async fn channel_of(peer: &Peer) -> std::result::Result<Option<tl::enums::InputChannel>, String> {
    if !matches!(peer, Peer::Channel(_) | Peer::Community(_)) {
        return Ok(None);
    }

    let reference = peer
        .to_ref()
        .await
        .map_err(|e| format!("ссылка на канал не получена: {e}"))?
        .ok_or_else(|| "Telegram не дал доступ к каналу".to_string())?;

    Ok(Some(tl::enums::InputChannel::Channel(
        tl::types::InputChannel {
            channel_id: reference.id.bare_id().unwrap_or_default(),
            access_hash: reference.auth.hash(),
        },
    )))
}

/// Pull the connection parameters out of the answer to `phone.joinGroupCall`.
fn connection_params(updates: &tl::enums::Updates) -> Option<String> {
    let list = match updates {
        tl::enums::Updates::Updates(updates) => &updates.updates,
        tl::enums::Updates::Combined(updates) => &updates.updates,
        _ => return None,
    };

    list.iter().find_map(|update| match update {
        tl::enums::Update::GroupCallConnection(connection) => match &connection.params {
            tl::enums::DataJson::Json(json) => Some(json.data.clone()),
        },
        _ => None,
    })
}

/// A random identifier for `phone.createGroupCall`.
fn random_id() -> i32 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();

    (nanos ^ (std::process::id() << 8)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_of_speech_is_a_hundred_frames() {
        let input = vec![0.2f32; CALL_RATE as usize];
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&input);

        let mut frames = 0;
        let mut samples = 0;
        while let Some(frame) = packer.frame() {
            assert_eq!(frame.len(), FRAME_SAMPLES);
            frames += 1;
            samples += frame.len();
        }

        assert_eq!(frames, 100, "десять миллисекунд на кадр");
        assert_eq!(samples, CALL_RATE as usize);
    }

    #[test]
    fn speech_of_another_rate_keeps_its_duration() {
        // 24 kHz, one second: twice as many samples must come out.
        let input = vec![0.1f32; 24_000];
        let mut packer = Packer::new(24_000);
        packer.push(&input);

        let mut samples = 0;
        while let Some(frame) = packer.frame() {
            samples += frame.len();
        }

        assert!(
            samples.abs_diff(CALL_RATE as usize) <= 1,
            "секунда речи остаётся секундой: {samples}"
        );
    }

    #[test]
    fn amplitude_survives_the_conversion() {
        let input = vec![0.5f32; 960];
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&input);

        let frame = packer.frame().expect("кадр собран");
        let peak = frame
            .iter()
            .map(|sample| i16::abs(*sample))
            .max()
            .unwrap_or(0);

        assert!(
            (16_000..=16_400).contains(&peak),
            "половина шкалы — примерно 16383: {peak}"
        );
    }

    #[test]
    fn a_jump_between_chunks_is_smoothed() {
        // The provider hands a phrase over in chunks: this one ends at half the
        // scale, the next starts at the opposite half. A hard join is a click.
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&vec![0.5f32; 1_000]);
        packer.push(&vec![-0.5f32; 1_000]);

        let frame = packer.frame().expect("кадр собран");
        let jump = frame
            .windows(2)
            .map(|pair| (pair[1] as i32 - pair[0] as i32).abs())
            .max()
            .unwrap_or(0);

        assert!(jump < 2_000, "стык сглажен, а не ступенькой: скачок {jump}");
    }

    #[test]
    fn chunks_that_continue_each_other_are_left_alone() {
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&vec![0.0f32; 1_000]);
        packer.push(&vec![0.0f32; 1_000]);

        assert_eq!(
            packer.backlog(),
            2_000,
            "без лишней перемычки сэмплов ровно вдвое меньше кадров"
        );
    }

    #[test]
    fn silence_starts_where_the_speech_ended() {
        let silence = vec![0i16; FRAME_SAMPLES];
        let frame = fade_out(i16::MAX / 2, &silence);

        assert_eq!(frame.len(), FRAME_SAMPLES);
        assert!(frame[0] > i16::MAX / 2 - 10, "начало на уровне речи");
        assert_eq!(frame[EDGE_FADE], 0, "через пять миллисекунд — тишина");
    }

    #[test]
    fn a_runaway_stream_gives_up_a_minute_of_speech_at_most() {
        assert_eq!(
            MAX_QUEUED_FRAMES, 6_000,
            "шестьдесят секунд по десять миллисекунд"
        );
    }

    #[test]
    fn silence_stays_silent() {
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&vec![0.0f32; 960]);

        let frame = packer.frame().expect("кадр собран");
        assert!(frame.iter().all(|sample| *sample == 0));
    }

    #[test]
    fn an_incomplete_frame_waits() {
        let mut packer = Packer::new(CALL_RATE);
        packer.push(&vec![0.1f32; FRAME_SAMPLES - 1]);

        assert!(packer.frame().is_none(), "кадр неполный");
        assert_eq!(packer.backlog(), FRAME_SAMPLES - 1);

        packer.push(&[0.1f32]);
        assert!(packer.frame().is_some(), "кадр дополнился");
    }

    /// A refused frame is worth a line — but not a hundred lines a second.
    #[test]
    fn a_refused_frame_is_reported_once_a_second() {
        let start = tokio::time::Instant::now();
        let mut failures = FrameFailures::default();

        assert_eq!(failures.record(start), Some(Failure::Report));
        assert_eq!(
            failures.record(start + Duration::from_millis(10)),
            None,
            "второй кадр за ту же секунду молчит"
        );
        assert_eq!(failures.record(start + Duration::from_millis(500)), None);
        assert_eq!(
            failures.record(start + Duration::from_secs(1)),
            Some(Failure::Report),
            "через секунду снова можно сказать"
        );
    }

    /// A call that refuses frames in a row is gone, and the log says it once.
    #[test]
    fn a_call_that_refuses_frames_is_declared_gone() {
        let start = tokio::time::Instant::now();
        let mut failures = FrameFailures::default();

        let decisions: Vec<Option<Failure>> = (0..FAILED_FRAMES_BEFORE_RESTART)
            .map(|index| failures.record(start + Duration::from_millis(index as u64 * 10)))
            .collect();

        assert_eq!(
            decisions.last(),
            Some(&Some(Failure::GiveUp)),
            "двадцатый кадр подряд — приговор"
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| **decision == Some(Failure::GiveUp))
                .count(),
            1
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|decision| **decision == Some(Failure::Report))
                .count(),
            1,
            "пятая доля секунды — это одно сообщение, а не двадцать: {decisions:?}"
        );
    }

    /// One frame the call took and the stream is alive again.
    #[test]
    fn a_frame_the_call_took_clears_the_count() {
        let start = tokio::time::Instant::now();
        let mut failures = FrameFailures::default();

        for index in 0..5 {
            failures.record(start + Duration::from_millis(index * 10));
        }
        assert_eq!(failures.consecutive(), 5);

        failures.recovered();
        assert_eq!(failures.consecutive(), 0);
        assert_ne!(
            failures.record(start + Duration::from_secs(2)),
            Some(Failure::GiveUp),
            "счёт начат заново"
        );
    }

    /// The reason a call died is what the supervisor will report, so the first
    /// one is kept and a new join starts clean.
    #[test]
    fn the_first_reason_of_a_broken_call_is_kept() {
        let health = CallHealth::default();
        assert_eq!(health.reason(), None);

        health.mark_broken("звонок не принимает звук: connection closed".into());
        health.mark_broken("что-то другое".into());

        assert_eq!(
            health.reason().as_deref(),
            Some("звонок не принимает звук: connection closed")
        );

        health.reset();
        assert_eq!(health.reason(), None, "новый вход начинает с чистого листа");
    }
}
