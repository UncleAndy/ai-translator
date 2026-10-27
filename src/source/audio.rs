//! Audio source: microphone → transono translation provider → bus.
//!
//! This is the real stage 1 path: the default input device is captured, the
//! synchronous OpenAI translation provider turns speech into translated text
//! and speech, and the translation line reports both as session events which
//! the bridge publishes on the bus.
//!
//! Starting the source needs an audio device and an API key; the text and script
//! sources exist so that everything downstream can be exercised without them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::BoxStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use cpal::traits::DeviceTrait;
use transono::audio::output::BoxSink;
use transono::audio::{
    Audio, AudioDevicesCpal, AudioInputCpal, AudioOutput, AudioOutputCpal, EncodedAudioFormat,
    LatencyStats, Pipelines,
};
use transono::core::error::{CoreError, Result as CoreResult};
use transono::core::provider::{Provider, ProviderSession};
use transono::core::session_event::SessionEvent;

use crate::source::reconnect::{
    LiveSession, OpenError, OpenFuture, ReconnectPolicy, ReconnectReport, supervise,
};
use crate::source::silent::SilentOutput;
use transono::line::TranslationLine;
use transono::providers::openai::translation::{
    OpenAITranslationConfig, OpenAITranslationProvider,
};

use crate::bus::{EventBus, LangPair, LineId, LineState, TranslationEvent};
use crate::config::CoreConfig;
use crate::core_glue::adapt::EventAdapter;
use crate::core_glue::line::EventBridge;

/// How long to wait for the bridge to finish after the line has stopped.
const BRIDGE_STOP_TIMEOUT: Duration = Duration::from_millis(500);

/// Why a source could not be started or stopped.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The configured provider is not known.
    #[error("неизвестный провайдер перевода: {0}")]
    UnknownProvider(String),

    /// An audio device could not be opened.
    #[error("аудиоустройство недоступно: {0}")]
    Device(String),

    /// The provider refused the session.
    #[error("провайдер перевода не принял сессию: {0}")]
    Provider(String),

    /// The translation line failed.
    #[error("линия перевода не работает: {0}")]
    Line(String),
}

/// How long the supervisor may take to let go of the line when the program
/// stops; a stuck session task must not hold Ctrl+C.
const SOURCE_STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// Running audio source.
///
/// The provider session lives in a task of its own: the provider closes
/// sessions after about an hour, and that task is what opens the next one, so
/// a broadcast outlives the limit instead of stopping with it.
pub struct AudioSource {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    stats: Arc<LatencyStats>,
}

impl AudioSource {
    /// Start translation from the default input device.
    ///
    /// Fails before any audio is captured if the provider is unknown, the
    /// devices are unavailable or the provider rejects the session — a broken
    /// run should be visible immediately, not at the first phrase. Interruptions
    /// after that are handled by the supervisor and do not end the program.
    pub async fn start(cfg: &CoreConfig, bus: EventBus) -> Result<Self, SourceError> {
        // The provider speaks TLS; rustls needs a process-level crypto provider.
        // A provider already installed by someone else is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();

        // The name is checked before the devices are touched, as it always was.
        if cfg.provider != "openai/translation" {
            return Err(SourceError::UnknownProvider(cfg.provider.clone()));
        }

        let stats = Arc::new(LatencyStats::default());
        let first = open_session(cfg, &bus, stats.clone()).await?;

        let policy = ReconnectPolicy::from_minutes(cfg.session_reconnect_minutes);
        let (stop, stop_rx) = watch::channel(false);
        let task = tokio::spawn(supervise_sessions(
            cfg.clone(),
            bus,
            stats.clone(),
            first,
            stop_rx,
            policy,
        ));

        Ok(Self {
            stop,
            task: Some(task),
            stats,
        })
    }

    /// Latency counters of the audio pipelines, shared across reconnects.
    pub fn stats(&self) -> &Arc<LatencyStats> {
        &self.stats
    }

    /// Stop the session and its supervisor.
    ///
    /// The supervisor is signalled rather than simply left alone: it may be
    /// waiting out a backoff, and the program must not wait for that.
    pub async fn stop(mut self) -> Result<(), SourceError> {
        let _ = self.stop.send(true);

        if let Some(task) = self.task.take() {
            match tokio::time::timeout(SOURCE_STOP_TIMEOUT, task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::error!("надзиратель источника упал: {error}"),
                Err(_) => tracing::warn!("источник не остановился за отведённое время"),
            }
        }

        Ok(())
    }
}

/// Run the reconnect supervisor and publish what happens on the bus.
async fn supervise_sessions(
    cfg: CoreConfig,
    bus: EventBus,
    stats: Arc<LatencyStats>,
    first: LiveLine,
    stop: watch::Receiver<bool>,
    policy: ReconnectPolicy,
) {
    let line = LineId(0);

    let opener_cfg = cfg.clone();
    let opener_bus = bus.clone();
    let opener_stats = stats.clone();
    let open = move || -> OpenFuture<LiveLine> {
        let cfg = opener_cfg.clone();
        let bus = opener_bus.clone();
        let stats = opener_stats.clone();
        Box::pin(async move { open_session(&cfg, &bus, stats).await.map_err(opening_error) })
    };

    let report_bus = bus.clone();
    let report = move |report: ReconnectReport| {
        let state = match &report {
            ReconnectReport::SessionEnded { .. } | ReconnectReport::Reconnecting { .. } => {
                LineState::Reconnecting
            }
            ReconnectReport::Restored { .. } => LineState::Running,
            ReconnectReport::GaveUp { .. } => LineState::Failed,
        };
        if let ReconnectReport::GaveUp { attempts, error } = &report {
            tracing::error!("перевод остановлен: {attempts} попыток, {error}");
        }
        report_bus.publish(TranslationEvent::Status {
            line,
            state,
            at: Instant::now(),
        });
    };

    supervise(open, first, stop, policy, report).await;
}

/// One running line with its bridge: everything a provider session owns.
struct LiveLine {
    line: Option<TranslationLine<WatchingProvider<OpenAITranslationProvider>>>,
    bridge: Option<JoinHandle<()>>,
    stopper: watch::Sender<bool>,
    finished: watch::Receiver<u64>,
}

impl LiveSession for LiveLine {
    fn finished(&self) -> watch::Receiver<u64> {
        self.finished.clone()
    }

    fn close(mut self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            if let Some(mut line) = self.line.take() {
                // An error here is the session that just died, not news: the
                // supervisor is already replacing it.
                if let Err(error) = line.stop().await {
                    tracing::debug!("линия остановлена с ошибкой: {error}");
                }
            }

            // The bridge flushes the phrase in progress, so the last words are
            // not lost between two sessions.
            let _ = self.stopper.send(true);
            if let Some(bridge) = self.bridge.take() {
                match tokio::time::timeout(BRIDGE_STOP_TIMEOUT, bridge).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::error!("мост событий упал: {error}"),
                    Err(_) => tracing::warn!("мост событий не завершился за отведённое время"),
                }
            }
        })
    }
}

/// Open one provider session and wire it to the bus.
async fn open_session(
    cfg: &CoreConfig,
    bus: &EventBus,
    stats: Arc<LatencyStats>,
) -> Result<LiveLine, SourceError> {
    let provider = build_provider(cfg)?;

    let devices = AudioDevicesCpal::new();
    let capture = if cfg.input_device == "default" {
        devices.default_input()
    } else {
        devices.find_input(&cfg.input_device)
    }
    .map_err(|e| SourceError::Device(e.to_string()))?;

    let input = AudioInputCpal::new(capture, stats.clone())
        .map_err(|e| SourceError::Device(e.to_string()))?;
    let output = playback(&cfg.output_device, &devices, stats.clone())?;

    let mut line = TranslationLine::new(provider, Box::new(input), output, stats.clone())
        .await
        .map_err(|e| SourceError::Line(e.to_string()))?;

    let (tx, rx) = mpsc::unbounded_channel();
    line.set_event_sender(tx);

    let adapter = EventAdapter::new(LineId(0), langs(cfg));
    let bridge = EventBridge::new(adapter, bus.clone(), rx);
    let stopper = bridge.stopper();

    // Subscribe before the session starts: a session that fails immediately
    // must still be noticed, and a receiver created afterwards would wait for
    // the next change instead of this one.
    let finished = line.provider().finished();

    line.run()
        .await
        .map_err(|e| SourceError::Line(e.to_string()))?;

    let bridge = tokio::spawn(bridge.run());

    Ok(LiveLine {
        line: Some(line),
        bridge: Some(bridge),
        stopper,
        finished,
    })
}

/// Turn a start-up failure into what the supervisor needs to know about it.
fn opening_error(error: SourceError) -> OpenError {
    match error {
        // The provider refusing a session is worth a different pace than a
        // device that is busy or a network that is down.
        SourceError::Provider(message) | SourceError::Line(message) => OpenError::classify(message),
        other => OpenError::Transient(other.to_string()),
    }
}

/// A provider that tells the outside world when one of its sessions ends.
///
/// The line owns its provider, so this wrapper is where the supervisor's signal
/// comes from without changing `transono`: it adds one counter and nothing else.
struct WatchingProvider<P> {
    inner: P,
    ended: watch::Sender<u64>,
}

impl<P> WatchingProvider<P> {
    fn new(inner: P) -> Self {
        let (ended, _) = watch::channel(0);
        Self { inner, ended }
    }

    /// Changes whenever a session of this provider ends.
    fn finished(&self) -> watch::Receiver<u64> {
        self.ended.subscribe()
    }
}

#[async_trait::async_trait]
impl<P> Provider for WatchingProvider<P>
where
    P: Provider + Sync,
    P::Session: Send + 'static,
{
    type Session = WatchingSession<P::Session>;

    async fn create_session(&self) -> CoreResult<Self::Session> {
        let session = self.inner.create_session().await?;
        Ok(WatchingSession {
            inner: session,
            ended: self.ended.clone(),
        })
    }

    fn audio_format(&self) -> EncodedAudioFormat {
        self.inner.audio_format()
    }
}

/// Session wrapper that counts its own end.
struct WatchingSession<S> {
    inner: S,
    ended: watch::Sender<u64>,
}

impl<S: ProviderSession + Send + 'static> ProviderSession for WatchingSession<S> {
    fn spawn(
        self,
        capture_stream: BoxStream<'static, Audio>,
        playback_sink: BoxSink<'static, Audio, CoreError>,
        pipelines: Pipelines,
        cancel: tokio_util::sync::CancellationToken,
        event_tx: Option<mpsc::UnboundedSender<SessionEvent>>,
    ) -> JoinHandle<CoreResult<Pipelines>> {
        let handle = self
            .inner
            .spawn(capture_stream, playback_sink, pipelines, cancel, event_tx);
        let ended = self.ended;

        tokio::spawn(async move {
            let result = handle.await;
            // Signal first, so the supervisor reconnects even if the session
            // task panicked rather than returned.
            ended.send_modify(|generation| *generation = generation.wrapping_add(1));
            match result {
                Ok(inner) => inner,
                Err(error) => Err(CoreError::Internal(format!("задача сессии: {error}"))),
            }
        })
    }
}

/// Build the translation provider named by the configuration.
///
/// The provider is wrapped so that the supervisor learns when its session ends:
/// the line owns the provider, and this is the only place where that fact can
/// be observed without touching the library.
fn build_provider(
    cfg: &CoreConfig,
) -> Result<WatchingProvider<OpenAITranslationProvider>, SourceError> {
    match cfg.provider.as_str() {
        "openai/translation" => {
            let mut config = OpenAITranslationConfig::from_env()
                .map_err(|e| SourceError::Provider(e.to_string()))?;
            config.with_lang(&cfg.target_lang);

            Ok(WatchingProvider::new(OpenAITranslationProvider::new(
                config,
            )))
        }
        other => Err(SourceError::UnknownProvider(other.to_string())),
    }
}

/// Language pair of the line.
fn langs(cfg: &CoreConfig) -> LangPair {
    crate::source::lang_pair(cfg)
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// Подставной список устройств: проверяет формат вывода без звукового железа.
    struct FakeLister {
        inputs: Result<Vec<String>, String>,
        outputs: Result<Vec<String>, String>,
    }

    impl DeviceLister for FakeLister {
        fn inputs(&self) -> Result<Vec<String>, String> {
            self.inputs.clone()
        }

        fn outputs(&self) -> Result<Vec<String>, String> {
            self.outputs.clone()
        }
    }

    fn fake(inputs: &[&str], outputs: &[&str]) -> FakeLister {
        FakeLister {
            inputs: Ok(inputs.iter().map(|name| name.to_string()).collect()),
            outputs: Ok(outputs.iter().map(|name| name.to_string()).collect()),
        }
    }

    fn names(text: &str) -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with("  ") && !line.starts_with("  ("))
            .map(|line| line.trim().to_string())
            .collect()
    }

    #[test]
    fn devices_are_listed_by_section_and_alphabetically() {
        let text = render_devices(&fake(&["Микрофон", "BlackHole 2ch"], &["Speakers"]));

        assert!(text.starts_with("входы:\n"), "{text}");
        assert!(text.contains("выходы:\n"), "{text}");

        let listed = names(&text);
        assert_eq!(
            listed,
            vec![
                "default",
                "BlackHole 2ch",
                "Микрофон",
                "default",
                "Speakers"
            ],
            "порядок: default, затем по алфавиту, входы перед выходами: {text}"
        );
    }

    #[test]
    fn the_default_device_is_named_in_the_list_and_in_the_hint() {
        let text = render_devices(&fake(&["Микрофон"], &["Speakers"]));

        assert!(text.contains("default"), "default назван в списке: {text}");
        assert!(
            text.contains("input_device"),
            "подсказка про поле конфигурации: {text}"
        );
    }

    #[test]
    fn duplicates_are_removed() {
        let text = render_devices(&fake(&["Микрофон", "Микрофон"], &[]));

        let listed = names(&text);
        assert_eq!(listed, vec!["default", "Микрофон", "default"], "{text}");
    }

    #[test]
    fn an_empty_list_is_reported_in_words() {
        let text = render_devices(&fake(&[], &[]));

        assert!(
            text.contains("устройств не найдено"),
            "пустой список назван словами, а не пустотой: {text}"
        );
    }

    #[test]
    fn a_failed_enumeration_is_reported_with_its_reason() {
        let lister = FakeLister {
            inputs: Err("звуковой системы нет".to_string()),
            outputs: Ok(vec![]),
        };

        let text = render_devices(&lister);

        assert!(
            text.contains("звуковой системы нет"),
            "причина названа: {text}"
        );
    }
}

/// Список доступных звуковых устройств.
///
/// Поле `input_device` в конфигурации задаётся **именем** устройства, и имя
/// сверяется ровно с этим списком — с тем, из которого выбирает сам источник
/// звука. Имя, взятое из системной утилиты, может не совпасть, поэтому
/// показывать надо именно этот список.
pub trait DeviceLister {
    /// Имена устройств входа в том порядке, в каком их отдал звуковой бэкенд.
    fn inputs(&self) -> Result<Vec<String>, String>;

    /// Имена устройств выхода в том порядке, в каком их отдал звуковой бэкенд.
    fn outputs(&self) -> Result<Vec<String>, String>;
}

/// Печатает список устройств так, как его выводит команда `devices`.
///
/// Устройства сортируются без учёта регистра и без повторов, а особое имя
/// `default` называется первым: именно оно в конфигурации означает «устройство
/// по умолчанию системы».
pub fn render_devices(lister: &dyn DeviceLister) -> String {
    let mut text = String::new();

    for (title, result) in [("входы", lister.inputs()), ("выходы", lister.outputs())] {
        text.push_str(title);
        text.push_str(":\n");

        match result {
            Ok(names) => {
                // В списке самого бэкенда имени `default` может и не быть,
                // а в конфигурации оно допустимо всегда.
                text.push_str("  default\n");

                let names = sorted_unique(names);

                if names.is_empty() {
                    text.push_str(
                        "  (устройств не найдено: звуковой системы нет или она не отвечает)\n",
                    );
                } else {
                    for name in names {
                        text.push_str("  ");
                        text.push_str(&name);
                        text.push('\n');
                    }
                }
            }
            Err(reason) => {
                text.push_str("  (список не получен: ");
                text.push_str(&reason);
                text.push_str(")\n");
            }
        }
    }

    text.push_str(
        "\nполе input_device в конфигурации задаётся одним из этих имён \
         (default — устройство по умолчанию системы)\n",
    );

    text
}

/// Имена без повторов, по алфавиту и без учёта регистра.
fn sorted_unique(names: Vec<String>) -> Vec<String> {
    let mut names = names;
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// Устройства, которые видит cpal, — ровно тот список, из которого выбирает
/// источник звука.
#[derive(Debug, Default, Clone, Copy)]
pub struct CpalDeviceLister;

impl CpalDeviceLister {
    /// Список устройств звукового бэкенда по умолчанию.
    pub fn new() -> Self {
        Self
    }
}

impl DeviceLister for CpalDeviceLister {
    fn inputs(&self) -> Result<Vec<String>, String> {
        let devices = AudioDevicesCpal::new()
            .input_devices()
            .map_err(|error| error.to_string())?;

        Ok(device_names(devices))
    }

    fn outputs(&self) -> Result<Vec<String>, String> {
        let devices = AudioDevicesCpal::new()
            .output_devices()
            .map_err(|error| error.to_string())?;

        Ok(device_names(devices))
    }
}

/// Имена устройств; устройство, не сообщившее описание, пропускается.
///
/// Через `description()` намеренно, а не через `to_string()`: `Display`
/// устройства тоже печатает имя, но на отключённом устройстве возвращает
/// ошибку форматирования, то есть панику, — а команда обязана пережить
/// отсутствие звука.
fn device_names(devices: Vec<cpal::Device>) -> Vec<String> {
    devices
        .iter()
        .filter_map(|device| device.description().ok())
        .map(|description| description.name().to_string())
        .collect()
}

/// Playback of the translated speech, chosen by `core.output_device`.
///
/// `none` keeps the line running without a sound card, `default` follows the
/// system default output, and any other value is matched against the device
/// names the `devices` command prints.
fn playback(
    wanted: &str,
    devices: &AudioDevicesCpal,
    stats: Arc<LatencyStats>,
) -> Result<Box<dyn AudioOutput>, SourceError> {
    let wanted = wanted.trim();

    if wanted == "none" {
        return Ok(Box::new(SilentOutput::new()));
    }

    let device = if wanted == "default" {
        devices
            .default_output()
            .map_err(|e| SourceError::Device(e.to_string()))?
    } else {
        let mut names = Vec::new();
        let mut chosen = None;

        for device in devices
            .output_devices()
            .map_err(|e| SourceError::Device(e.to_string()))?
        {
            let name = device.description().ok().map(|d| d.name().to_string());

            if name.as_deref() == Some(wanted) {
                chosen = Some(device);
                break;
            }
            if let Some(name) = name {
                names.push(name);
            }
        }

        chosen.ok_or_else(|| {
            let known = if names.is_empty() {
                "ни одного".to_string()
            } else {
                names.join(", ")
            };
            SourceError::Device(format!(
                "устройство вывода «{wanted}» не найдено; есть: {known}"
            ))
        })?
    };

    Ok(Box::new(
        AudioOutputCpal::new(device, stats).map_err(|e| SourceError::Device(e.to_string()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CoreConfig;
    use futures_util::{SinkExt, StreamExt};

    /// Провайдерская сессия, которая заканчивается сразу: проверяет ту самую
    /// связку, на которой держится переподключение, — «сессия кончилась» должно
    /// быть видно снаружи линии.
    struct EndingSession;

    impl ProviderSession for EndingSession {
        fn spawn(
            self,
            _capture: BoxStream<'static, Audio>,
            _sink: BoxSink<'static, Audio, CoreError>,
            pipelines: Pipelines,
            _cancel: tokio_util::sync::CancellationToken,
            _events: Option<mpsc::UnboundedSender<SessionEvent>>,
        ) -> JoinHandle<CoreResult<Pipelines>> {
            tokio::spawn(async move { Ok(pipelines) })
        }
    }

    #[tokio::test]
    async fn a_session_that_ends_is_reported_to_the_supervisor() {
        let (ended, mut finished) = watch::channel(0u64);
        let session = WatchingSession {
            inner: EndingSession,
            ended,
        };

        let handle = session.spawn(
            futures_util::stream::empty::<Audio>().boxed(),
            Box::pin(
                futures_util::sink::drain()
                    .sink_map_err(|never: std::convert::Infallible| match never {}),
            ),
            Pipelines::with_stats(Arc::new(LatencyStats::default())),
            tokio_util::sync::CancellationToken::new(),
            None,
        );

        assert!(
            handle.await.expect("задача сессии").is_ok(),
            "сессия завершилась без ошибки"
        );
        tokio::time::timeout(Duration::from_secs(1), finished.changed())
            .await
            .expect("сигнал об окончании сессии не пришёл")
            .expect("канал сигнала жив");
        assert_eq!(*finished.borrow(), 1, "окончание сессии обязано считаться");
    }

    fn config(provider: &str) -> CoreConfig {
        CoreConfig {
            provider: provider.to_string(),
            source_lang: "auto".to_string(),
            target_lang: "ru".to_string(),
            input_device: "default".to_string(),
            output_device: "default".to_string(),
            voice: None,
            transcription_model: None,
            session_reconnect_minutes: 55,
        }
    }

    #[test]
    fn no_playback_needs_no_sound_card() {
        let devices = AudioDevicesCpal::new();
        let output = playback("none", &devices, Arc::new(LatencyStats::default()))
            .expect("тихий вывод не требует устройства");

        assert_eq!(output.format().sample_rate, 24_000);
    }

    #[test]
    fn unknown_provider_is_rejected_by_name() {
        let error = build_provider(&config("deepseek/whatever"))
            .err()
            .expect("провайдер не поддержан");

        assert!(
            matches!(error, SourceError::UnknownProvider(ref name) if name == "deepseek/whatever"),
            "{error:?}"
        );
        assert!(error.to_string().contains("deepseek/whatever"));
    }

    #[test]
    fn the_stage_one_provider_is_recognised() {
        // Ключ не задаём: провайдер строится из окружения, и без ключа
        // ошибка должна быть внятной, а не паникой.
        let result = build_provider(&config("openai/translation"));

        match result {
            Ok(_) => {}
            Err(SourceError::Provider(message)) => {
                assert!(
                    message.contains("OPENAI_API_KEY"),
                    "нехватка ключа названа прямо: {message}"
                );
            }
            Err(other) => panic!("неожиданная ошибка: {other:?}"),
        }
    }
}
