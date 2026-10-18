//! Local WebSocket server sink serving the full-screen display page.
//!
//! The sink is a stub for now: the registry must be able to build it from the
//! configuration, but the server itself is not written yet. It reports that
//! honestly instead of silently dropping the translation.

use async_trait::async_trait;

use crate::bus::TranslationEvent;
use crate::sink::{Result, SinkError, TextSink};

/// Serves the translation to clients of the full-screen page.
#[derive(Debug)]
pub struct WebsocketSink {
    id: String,
    bind: String,
}

impl WebsocketSink {
    /// Create a sink with the given identifier and listen address.
    ///
    /// # Errors
    ///
    /// Returns an error when the listen address is not a `host:port` socket
    /// address.
    pub fn new(id: &str, bind: &str) -> Result<Self> {
        bind.parse::<std::net::SocketAddr>().map_err(|e| {
            SinkError::Protocol(format!("некорректный адрес прослушивания {bind}: {e}"))
        })?;

        Ok(Self {
            id: id.to_string(),
            bind: bind.to_string(),
        })
    }

    /// Listen address.
    pub fn bind(&self) -> &str {
        &self.bind
    }
}

#[async_trait]
impl TextSink for WebsocketSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
        Err(SinkError::Unavailable(
            "приёмник «локальный WebSocket-сервер» ещё не реализован".into(),
        ))
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unusable_listen_address() {
        let err = WebsocketSink::new("screen", "не-адрес").unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn keeps_the_listen_address() {
        let sink = WebsocketSink::new("screen", "127.0.0.1:8642").unwrap();
        assert_eq!(sink.bind(), "127.0.0.1:8642");
        assert_eq!(sink.id(), "screen");
    }
}
