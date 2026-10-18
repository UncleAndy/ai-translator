//! Audio source: microphone → transono translation provider → bus.
//!
//! This is the real stage 1 path: the default input device is captured, the
//! synchronous OpenAI translation provider turns speech into translated text
//! and speech, and the translation line reports both as session events which
//! the bridge publishes on the bus.
//!
//! Starting the source needs an audio device and an API key; the text and script
//! sources exist so that everything downstream can be exercised without them.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use cpal::traits::DeviceTrait;
use transono::audio::{AudioDevicesCpal, AudioInputCpal, AudioOutputCpal, LatencyStats};
use transono::line::TranslationLine;
use transono::providers::openai::translation::{
    OpenAITranslationConfig, OpenAITranslationProvider,
};

use crate::bus::{EventBus, LangPair, LineId};
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

/// Running audio source.
pub struct AudioSource {
    line: TranslationLine<OpenAITranslationProvider>,
    bridge: Option<JoinHandle<()>>,
    stopper: watch::Sender<bool>,
}

impl AudioSource {
    /// Start translation from the default input device.
    ///
    /// Fails before any audio is captured if the provider is unknown, the
    /// devices are unavailable or the provider rejects the session — a broken
    /// run should be visible immediately, not at the first phrase.
    pub async fn start(cfg: &CoreConfig, bus: EventBus) -> Result<Self, SourceError> {
        // The provider speaks TLS; rustls needs a process-level crypto provider.
        // A provider already installed by someone else is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let provider = build_provider(cfg)?;

        let devices = AudioDevicesCpal::new();
        let capture = if cfg.input_device == "default" {
            devices.default_input()
        } else {
            devices.find_input(&cfg.input_device)
        }
        .map_err(|e| SourceError::Device(e.to_string()))?;
        let playback = devices
            .default_output()
            .map_err(|e| SourceError::Device(e.to_string()))?;

        let stats = Arc::new(LatencyStats::default());
        let input = AudioInputCpal::new(capture, stats.clone())
            .map_err(|e| SourceError::Device(e.to_string()))?;
        let output = AudioOutputCpal::new(playback, stats.clone())
            .map_err(|e| SourceError::Device(e.to_string()))?;

        let mut line = TranslationLine::new(provider, Box::new(input), Box::new(output), stats)
            .await
            .map_err(|e| SourceError::Line(e.to_string()))?;

        let (tx, rx) = mpsc::unbounded_channel();
        line.set_event_sender(tx);

        let adapter = EventAdapter::new(LineId(0), langs(cfg));
        let bridge = EventBridge::new(adapter, bus, rx);
        let stopper = bridge.stopper();

        line.run()
            .await
            .map_err(|e| SourceError::Line(e.to_string()))?;

        let bridge = tokio::spawn(bridge.run());

        Ok(Self {
            line,
            bridge: Some(bridge),
            stopper,
        })
    }

    /// The translation line, for latency statistics and state.
    pub fn line(&self) -> &TranslationLine<OpenAITranslationProvider> {
        &self.line
    }

    /// Stop the session and the bridge.
    ///
    /// The bridge is signalled rather than left to notice the closed channel:
    /// the line keeps its event sender alive, so the channel never closes on
    /// its own, and the last phrase would never be flushed.
    pub async fn stop(mut self) -> Result<(), SourceError> {
        let result = self
            .line
            .stop()
            .await
            .map_err(|e| SourceError::Line(e.to_string()));

        let _ = self.stopper.send(true);

        if let Some(bridge) = self.bridge.take() {
            match tokio::time::timeout(BRIDGE_STOP_TIMEOUT, bridge).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::error!("мост событий упал: {error}"),
                Err(_) => tracing::warn!("мост событий не завершился за отведённое время"),
            }
        }

        result
    }
}

/// Build the translation provider named by the configuration.
fn build_provider(cfg: &CoreConfig) -> Result<OpenAITranslationProvider, SourceError> {
    match cfg.provider.as_str() {
        "openai/translation" => {
            let mut config = OpenAITranslationConfig::from_env()
                .map_err(|e| SourceError::Provider(e.to_string()))?;
            config.with_lang(&cfg.target_lang);

            Ok(OpenAITranslationProvider::new(config))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CoreConfig;

    fn config(provider: &str) -> CoreConfig {
        CoreConfig {
            provider: provider.to_string(),
            source_lang: "auto".to_string(),
            target_lang: "ru".to_string(),
            input_device: "default".to_string(),
            voice: None,
            transcription_model: None,
        }
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
