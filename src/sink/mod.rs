//! Result sinks: text and voice.
//!
//! A sink is an isolated delivery module subscribed to the event bus. Sinks
//! know nothing about each other and about the translation core: they receive
//! normalized events and deliver them to an external system. The bus already
//! isolates a slow sink, so a sink implementation may simply do its work.

/// Posting rate shared by the Telegram sinks.
pub mod rate;
/// Sink registry: building a sink from its configuration entry.
pub mod registry;
/// Text sinks: delivering the translated text.
pub mod text;
/// Voice sinks: delivering the translated speech.
pub mod voice;

use async_trait::async_trait;
use transono::audio::Audio;

use crate::bus::{LineId, TranslationEvent};

/// Sink error.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// The external system is not available.
    #[error("внешний сервис недоступен: {0}")]
    Unavailable(String),
    /// Protocol error.
    #[error("ошибка протокола: {0}")]
    Protocol(String),
    /// The sink type is not known to the registry.
    #[error("неизвестный тип приёмника: {0}")]
    UnknownType(String),
    /// Input/output error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Sink result.
pub type Result<T> = std::result::Result<T, SinkError>;

/// A sink delivering the translated text.
#[async_trait]
pub trait TextSink: Send {
    /// Stable sink identifier.
    fn id(&self) -> &str;

    /// Handle a translation event.
    ///
    /// # Errors
    ///
    /// Returns an error when the event could not be delivered; the supervisor
    /// decides whether to restart the sink.
    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()>;

    /// Shut the sink down in an orderly way.
    ///
    /// # Errors
    ///
    /// Returns an error when the shutdown itself failed.
    async fn shutdown(self: Box<Self>) -> Result<()>;
}

/// A sink delivering the translated speech.
#[async_trait]
pub trait VoiceSink: Send {
    /// Stable sink identifier.
    fn id(&self) -> &str;

    /// Handle a chunk of translated speech.
    ///
    /// # Errors
    ///
    /// Returns an error when the audio could not be delivered.
    async fn on_audio(&mut self, line: LineId, audio: &Audio) -> Result<()>;

    /// Shut the sink down in an orderly way.
    ///
    /// # Errors
    ///
    /// Returns an error when the shutdown itself failed.
    async fn shutdown(self: Box<Self>) -> Result<()>;
}
