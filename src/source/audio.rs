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
