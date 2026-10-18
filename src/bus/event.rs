//! Normalized translation event model shared by the core and the sinks.
//!
//! The translation core produces [`TranslationEvent`] values; sinks consume
//! them. The type is immutable and cheap to clone for the text variants; the
//! audio variant carries [`transono::audio::Audio`], which is reference
//! counted, so cloning a chunk does not copy samples.

use std::time::Instant;

use transono::audio::Audio;

/// Identifier of a translation line (a participant or a translation direction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineId(pub u16);

/// A language involved in translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    /// The source language is determined by the provider.
    Auto,
    /// A concrete language, as an ISO-639-1 code.
    Code(String),
}

impl Language {
    /// Build a language from an ISO-639-1 code, normalizing it to lower case.
    ///
    /// # Examples
    ///
    /// ```
    /// use ai_translator::bus::Language;
    ///
    /// assert_eq!(Language::code("RU"), Language::Code("ru".to_string()));
    /// ```
    pub fn code(code: &str) -> Self {
        Language::Code(code.to_ascii_lowercase())
    }

    /// Language code, or `"auto"` when the language is not fixed.
    pub fn as_str(&self) -> &str {
        match self {
            Language::Auto => "auto",
            Language::Code(code) => code.as_str(),
        }
    }
}

/// Direction of a translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LangPair {
    /// Source language, when the provider has determined it.
    pub source: Option<Language>,
    /// Target language.
    pub target: Language,
}

/// State of a translation line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineState {
    /// The line is created but not started.
    Created,
    /// The provider session is being established.
    Starting,
    /// Translation is running.
    Running,
    /// The line stopped in an orderly way.
    Stopped,
    /// The line stopped because of an error.
    Failed,
}

/// A normalized translation event.
#[derive(Debug, Clone)]
pub enum TranslationEvent {
    /// A fragment of recognized source speech.
    SourceText {
        /// Line the event belongs to.
        line: LineId,
        /// Accumulated text of the current phrase.
        text: String,
        /// Increment since the previous event.
        delta: Option<String>,
        /// The phrase is complete.
        is_final: bool,
        /// Moment of the event.
        at: Instant,
    },

    /// A fragment of the translated text.
    Translated {
        /// Line the event belongs to.
        line: LineId,
        /// Translation direction.
        langs: LangPair,
        /// Accumulated text of the current phrase.
        text: String,
        /// Increment since the previous event.
        delta: Option<String>,
        /// The phrase is complete.
        is_final: bool,
        /// Monotonic event number within the line.
        seq: u64,
        /// Moment of the event.
        at: Instant,
    },

    /// A fragment of the translated speech.
    TranslatedAudio {
        /// Line the event belongs to.
        line: LineId,
        /// Audio chunk in the library's internal container.
        audio: Audio,
        /// Moment of the event.
        at: Instant,
    },

    /// A line state change.
    Status {
        /// Line the event belongs to.
        line: LineId,
        /// New state.
        state: LineState,
        /// Moment of the event.
        at: Instant,
    },
}

impl TranslationEvent {
    /// The phrase (or state change) is complete and must not be dropped.
    pub fn is_final(&self) -> bool {
        match self {
            TranslationEvent::SourceText { is_final, .. }
            | TranslationEvent::Translated { is_final, .. } => *is_final,
            TranslationEvent::TranslatedAudio { .. } | TranslationEvent::Status { .. } => false,
        }
    }

    /// The event must not be dropped when a subscriber is overloaded.
    ///
    /// Final text events and state changes are durable: losing them would make
    /// a sink keep a phrase open forever or miss a failure. Intermediate text
    /// revisions are not durable, because the next revision carries the whole
    /// accumulated phrase text.
    pub fn is_durable(&self) -> bool {
        self.is_final() || matches!(self, TranslationEvent::Status { .. })
    }

    /// Line the event belongs to.
    pub fn line(&self) -> LineId {
        match self {
            TranslationEvent::SourceText { line, .. }
            | TranslationEvent::Translated { line, .. }
            | TranslationEvent::TranslatedAudio { line, .. }
            | TranslationEvent::Status { line, .. } => *line,
        }
    }

    /// Text payload: accumulated translated text, or recognized source text.
    pub fn text(&self) -> Option<&str> {
        match self {
            TranslationEvent::SourceText { text, .. }
            | TranslationEvent::Translated { text, .. } => Some(text.as_str()),
            TranslationEvent::TranslatedAudio { .. } | TranslationEvent::Status { .. } => None,
        }
    }

    /// Increment of text since the previous event.
    pub fn delta(&self) -> Option<&str> {
        match self {
            TranslationEvent::SourceText { delta, .. }
            | TranslationEvent::Translated { delta, .. } => delta.as_deref(),
            TranslationEvent::TranslatedAudio { .. } | TranslationEvent::Status { .. } => None,
        }
    }

    /// Monotonic number of a translated-text event.
    pub fn seq(&self) -> Option<u64> {
        match self {
            TranslationEvent::Translated { seq, .. } => Some(*seq),
            _ => None,
        }
    }

    /// Moment of the event.
    pub fn at(&self) -> Instant {
        match self {
            TranslationEvent::SourceText { at, .. }
            | TranslationEvent::Translated { at, .. }
            | TranslationEvent::TranslatedAudio { at, .. }
            | TranslationEvent::Status { at, .. } => *at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transono::audio::PcmAudio;
    use transono::providers::openai::translation::OpenAITranslationConfig;

    fn langs() -> LangPair {
        LangPair {
            source: None,
            target: Language::code("ru"),
        }
    }

    fn translated(text: &str, is_final: bool, seq: u64) -> TranslationEvent {
        TranslationEvent::Translated {
            line: LineId(0),
            langs: langs(),
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final,
            seq,
            at: Instant::now(),
        }
    }

    /// A chunk of silence in the format the translation provider expects.
    fn test_audio() -> Audio {
        let spec = OpenAITranslationConfig::default()
            .audio_format()
            .spec()
            .clone();
        let pcm = PcmAudio::new(spec, 240);
        Audio::from_pcm(&pcm).expect("test audio chunk")
    }

    #[test]
    fn final_text_events_are_final_and_durable() {
        let ev = translated("привет", true, 7);
        assert!(ev.is_final());
        assert!(ev.is_durable());
        assert_eq!(ev.text(), Some("привет"));
        assert_eq!(ev.delta(), Some("привет"));
        assert_eq!(ev.seq(), Some(7));
    }

    #[test]
    fn intermediate_text_is_not_durable() {
        let ev = translated("при", false, 3);
        assert!(!ev.is_final());
        assert!(!ev.is_durable());
    }

    #[test]
    fn status_is_durable_and_not_final() {
        let ev = TranslationEvent::Status {
            line: LineId(4),
            state: LineState::Running,
            at: Instant::now(),
        };
        assert!(!ev.is_final());
        assert!(ev.is_durable());
        assert_eq!(ev.line(), LineId(4));
        assert_eq!(ev.text(), None);
        assert_eq!(ev.seq(), None);
    }

    #[test]
    fn source_text_closes_like_a_phrase() {
        let ev = TranslationEvent::SourceText {
            line: LineId(1),
            text: "hello".to_string(),
            delta: Some("hello".to_string()),
            is_final: true,
            at: Instant::now(),
        };
        assert!(ev.is_final());
        assert!(ev.is_durable());
        assert_eq!(ev.line(), LineId(1));
        assert_eq!(ev.text(), Some("hello"));
    }

    #[test]
    fn translated_audio_is_never_final_but_clones_cheaply() {
        let ev = TranslationEvent::TranslatedAudio {
            line: LineId(2),
            audio: test_audio(),
            at: Instant::now(),
        };
        assert!(!ev.is_final());
        assert!(!ev.is_durable());
        assert_eq!(ev.line(), LineId(2));
        assert_eq!(ev.text(), None);

        let cloned = ev.clone();
        assert!(matches!(cloned, TranslationEvent::TranslatedAudio { .. }));
    }

    #[test]
    fn language_code_is_normalized() {
        assert_eq!(Language::code("RU"), Language::Code("ru".to_string()));
        assert_eq!(Language::Auto.as_str(), "auto");
        assert_eq!(Language::code("ru").as_str(), "ru");
    }
}
