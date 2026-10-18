//! Console sink: watching the translation in a terminal.
//!
//! The sink prints one line per phrase revision, so a live translation can be
//! followed without any external system. A full-screen terminal interface is
//! built on top of it later; keeping the sink line-based makes it testable
//! without a terminal.

use async_trait::async_trait;

use crate::bus::TranslationEvent;
use crate::sink::{Result, TextSink};

/// Prints the translation to standard output.
#[derive(Debug)]
pub struct ConsoleSink {
    id: String,
    phrases: usize,
    current: String,
    last_final: bool,
}

impl ConsoleSink {
    /// Create a sink with the given identifier.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            phrases: 0,
            current: String::new(),
            last_final: false,
        }
    }

    /// Number of completed phrases.
    pub fn phrase_count(&self) -> usize {
        self.phrases
    }

    /// Text of the current phrase.
    pub fn current_text(&self) -> &str {
        &self.current
    }

    /// Whether the last event was final.
    pub fn last_was_final(&self) -> bool {
        self.last_final
    }
}

#[async_trait]
impl TextSink for ConsoleSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()> {
        match ev {
            TranslationEvent::Translated {
                text,
                is_final,
                seq,
                ..
            } => {
                self.current = text.clone();
                self.last_final = *is_final;

                if *is_final {
                    self.phrases += 1;
                    println!("[{}] {} (#{seq})", self.id, self.current);
                } else {
                    println!("[{}] {} …", self.id, self.current);
                }
            }
            TranslationEvent::SourceText { text, .. } => {
                tracing::debug!(sink = %self.id, "исходный текст: {text}");
            }
            TranslationEvent::Status { state, .. } => {
                println!("[{}] состояние: {state:?}", self.id);
            }
            TranslationEvent::TranslatedAudio { .. } => {}
        }

        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        println!("[{}] завершено, фраз: {}", self.id, self.phrases);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId, LineState};
    use std::time::Instant;

    fn translated(text: &str, is_final: bool, seq: u64) -> TranslationEvent {
        TranslationEvent::Translated {
            line: LineId(0),
            langs: LangPair {
                source: None,
                target: Language::code("ru"),
            },
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final,
            seq,
            at: Instant::now(),
        }
    }

    #[tokio::test]
    async fn keeps_only_latest_revision_of_a_phrase() {
        let mut sink = ConsoleSink::new("console");

        sink.on_event(&translated("при", false, 1)).await.unwrap();
        sink.on_event(&translated("привет", false, 2))
            .await
            .unwrap();
        sink.on_event(&translated("привет мир", true, 3))
            .await
            .unwrap();

        assert_eq!(
            sink.phrase_count(),
            1,
            "внутри фразы записи не размножаются"
        );
        assert_eq!(sink.current_text(), "привет мир");
        assert!(sink.last_was_final());
    }

    #[tokio::test]
    async fn counts_phrases_and_ignores_audio() {
        let mut sink = ConsoleSink::new("console");

        sink.on_event(&translated("один", true, 1)).await.unwrap();
        sink.on_event(&TranslationEvent::SourceText {
            line: LineId(0),
            text: "один".into(),
            delta: Some("один".into()),
            is_final: true,
            at: Instant::now(),
        })
        .await
        .unwrap();
        sink.on_event(&TranslationEvent::Status {
            line: LineId(0),
            state: LineState::Running,
            at: Instant::now(),
        })
        .await
        .unwrap();
        sink.on_event(&translated("два", true, 2)).await.unwrap();

        assert_eq!(sink.phrase_count(), 2);
        assert_eq!(sink.id(), "console");
        Box::new(sink).shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn intermediate_revision_is_not_counted_as_a_phrase() {
        let mut sink = ConsoleSink::new("console");
        sink.on_event(&translated("при", false, 1)).await.unwrap();

        assert_eq!(sink.phrase_count(), 0);
        assert!(!sink.last_was_final());
    }
}
