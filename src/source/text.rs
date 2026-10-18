//! Text source: feeds the bus with text, bypassing the translation provider.
//!
//! The synchronous translation provider accepts audio only, so on stage 1 text
//! is published to the bus directly. This is what makes a run without a
//! microphone, a network or an API key possible — and what the tests use to
//! exercise the whole chain from the bus to the sinks.

use std::time::{Duration, Instant};

use crate::bus::{EventBus, LangPair, LineId, TranslationEvent};
use crate::source::script::{DEFAULT_DELAY, ScriptSource};

/// Split a text into phrases.
///
/// A phrase ends at `.`, `!`, `?`, `…` or a line break; the punctuation stays
/// with its phrase. Empty phrases are dropped.
pub fn split_phrases(text: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == '\n' {
            push_phrase(&mut phrases, &mut current);
            continue;
        }

        current.push(ch);

        if matches!(ch, '.' | '!' | '?' | '…') {
            push_phrase(&mut phrases, &mut current);
        }
    }

    push_phrase(&mut phrases, &mut current);
    phrases
}

fn push_phrase(phrases: &mut Vec<String>, current: &mut String) {
    let phrase = current.trim();
    if !phrase.is_empty() {
        phrases.push(phrase.to_string());
    }
    current.clear();
}

/// Turn a text into a sequence of translation events.
///
/// Each phrase is fed word by word, so the sinks see a growing phrase and then
/// its final revision — exactly what they would see from a live session.
/// Timestamps are spread by `delay`, matching the pace of [`ScriptSource`].
pub fn script_from_text(
    text: &str,
    line: LineId,
    langs: LangPair,
    delay: Duration,
) -> Vec<TranslationEvent> {
    let base = Instant::now();
    let mut out = Vec::new();
    let mut seq = 0;
    let mut previous = String::new();

    for phrase in split_phrases(text) {
        let words: Vec<&str> = phrase.split_whitespace().collect();
        previous.clear();

        for (word_index, _) in words.iter().enumerate() {
            let text = words[..=word_index].join(" ");
            let is_final = word_index + 1 == words.len();

            seq += 1;
            let at = base + delay * (out.len() as u32);

            out.push(TranslationEvent::Translated {
                line,
                langs: langs.clone(),
                delta: Some(text[previous.len()..].to_string()),
                text: text.clone(),
                is_final,
                seq,
                at,
            });

            previous = text;
        }
    }

    out
}

/// Text source: publishes a text to the bus as a sequence of phrases.
#[derive(Debug, Clone)]
pub struct TextSource {
    line: LineId,
    langs: LangPair,
    text: String,
    delay: Duration,
}

impl TextSource {
    /// Create a source for a text.
    pub fn new(line: LineId, langs: LangPair, text: impl Into<String>) -> Self {
        Self {
            line,
            langs,
            text: text.into(),
            delay: DEFAULT_DELAY,
        }
    }

    /// Set the pause between events.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Pause between events.
    pub fn delay(&self) -> Duration {
        self.delay
    }

    /// Events this source will publish.
    pub fn events(&self) -> Vec<TranslationEvent> {
        script_from_text(&self.text, self.line, self.langs.clone(), self.delay)
    }

    /// Publish the text to the bus; returns the number of events published.
    pub async fn run(self, bus: &EventBus) -> usize {
        ScriptSource::new(self.events())
            .with_delay(self.delay)
            .run(bus)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{Language, Subscriber};

    fn langs() -> LangPair {
        LangPair {
            source: None,
            target: Language::code("ru"),
        }
    }

    #[test]
    fn phrases_split_at_sentence_ends_and_line_breaks() {
        assert_eq!(
            split_phrases("Привет. Как дела? Хорошо!"),
            vec!["Привет.", "Как дела?", "Хорошо!"]
        );
        assert_eq!(
            split_phrases("Первая строка\nВторая строка"),
            vec!["Первая строка", "Вторая строка"]
        );
        assert_eq!(split_phrases("   \n  "), Vec::<String>::new());
        assert_eq!(split_phrases("Без точки"), vec!["Без точки"]);
    }

    #[test]
    fn script_grows_word_by_word_and_closes_each_phrase() {
        let events = script_from_text(
            "Привет мир. Пока.",
            LineId(0),
            langs(),
            Duration::from_millis(10),
        );

        let texts: Vec<_> = events.iter().filter_map(|e| e.text()).collect();
        assert_eq!(
            texts,
            vec!["Привет", "Привет мир.", "Пока."],
            "знак конца предложения остаётся при своей фразе"
        );

        let finals: Vec<_> = events
            .iter()
            .filter(|e| e.is_final())
            .filter_map(|e| e.text())
            .collect();
        assert_eq!(
            finals,
            vec!["Привет мир.", "Пока."],
            "фраза закрывается один раз"
        );

        let deltas: Vec<_> = events.iter().filter_map(|e| e.delta()).collect();
        assert_eq!(
            deltas,
            vec!["Привет", " мир.", "Пока."],
            "delta — приращение к предыдущему тексту"
        );

        let seqs: Vec<_> = events.iter().filter_map(|e| e.seq()).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn script_timestamps_are_monotonic() {
        let events = script_from_text(
            "раз. два. три.",
            LineId(0),
            langs(),
            Duration::from_millis(100),
        );

        let times: Vec<_> = events.iter().map(|e| e.at()).collect();
        assert!(
            times.windows(2).all(|pair| pair[0] <= pair[1]),
            "время не убывает: {times:?}"
        );
    }

    #[test]
    fn empty_text_produces_no_events() {
        assert!(
            script_from_text("  \n ", LineId(0), langs(), DEFAULT_DELAY).is_empty(),
            "пустой текст не порождает событий"
        );
    }

    async fn collect(subscriber: &mut Subscriber, count: usize) -> Vec<TranslationEvent> {
        let mut out = Vec::new();

        for _ in 0..count {
            let event = tokio::time::timeout(Duration::from_secs(2), subscriber.recv())
                .await
                .expect("событие пришло вовремя")
                .expect("событие получено");
            out.push(event.as_ref().clone());
        }

        out
    }

    #[tokio::test]
    async fn text_source_publishes_the_whole_text() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("test", 32);

        let source = TextSource::new(LineId(0), langs(), "Привет мир.").with_delay(Duration::ZERO);
        let published = source.run(&bus).await;
        assert_eq!(published, 2);

        let events = collect(&mut subscriber, 2).await;
        assert_eq!(events[0].text(), Some("Привет"));
        assert!(events[1].is_final());
        assert_eq!(events[1].text(), Some("Привет мир."));
    }
}
