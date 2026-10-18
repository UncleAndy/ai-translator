//! Webhook sink: sending the translation over HTTP POST.
//!
//! The sink is a stub for now: the registry must be able to build it from the
//! configuration, but the HTTP delivery itself is not written yet. It reports
//! that honestly instead of silently dropping the translation.

use async_trait::async_trait;

use crate::bus::TranslationEvent;
use crate::sink::{Result, SinkError, TextSink};

/// Sends the translation to an HTTP endpoint using a body template.
#[derive(Debug)]
pub struct WebhookSink {
    id: String,
    url: String,
    body_template: String,
    token_env: Option<String>,
}

impl WebhookSink {
    /// Create a sink posting to `url` with the given body template.
    ///
    /// # Errors
    ///
    /// Returns an error when the address does not start with `http` or the body
    /// template is empty.
    pub fn new(id: &str, url: &str, body_template: &str, token_env: Option<&str>) -> Result<Self> {
        if !url.starts_with("http") {
            return Err(SinkError::Protocol(format!(
                "адрес вебхука должен начинаться с http: {url}"
            )));
        }
        if body_template.is_empty() {
            return Err(SinkError::Protocol("пустой шаблон тела запроса".into()));
        }

        Ok(Self {
            id: id.to_string(),
            url: url.to_string(),
            body_template: body_template.to_string(),
            token_env: token_env.map(str::to_string),
        })
    }

    /// Endpoint address.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Request body template.
    pub fn body_template(&self) -> &str {
        &self.body_template
    }

    /// Name of the environment variable holding the token.
    pub fn token_env(&self) -> Option<&str> {
        self.token_env.as_deref()
    }
}

#[async_trait]
impl TextSink for WebhookSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
        Err(SinkError::Unavailable(
            "приёмник «вебхук» ещё не реализован".into(),
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
    fn rejects_address_without_scheme() {
        let err = WebhookSink::new("hook", "127.0.0.1:9/h", "{}", None).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn rejects_empty_body_template() {
        let err = WebhookSink::new("hook", "http://127.0.0.1:9/h", "", None).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn keeps_delivery_settings() {
        let sink = WebhookSink::new(
            "hook",
            "http://127.0.0.1:9/h",
            "{\"text\":\"{text}\"}",
            Some("AI_TRANSLATOR_HOOK_TOKEN"),
        )
        .unwrap();

        assert_eq!(sink.url(), "http://127.0.0.1:9/h");
        assert_eq!(sink.body_template(), "{\"text\":\"{text}\"}");
        assert_eq!(sink.token_env(), Some("AI_TRANSLATOR_HOOK_TOKEN"));
        assert_eq!(sink.id(), "hook");
    }
}
