//! Adapter from provider session events to normalized translation events.
//!
//! The synchronous translation provider sends only deltas: there is no event
//! that says "the phrase is complete". The adapter therefore synthesizes the
//! phrase boundary from a pause between deltas ([`EventAdapter::gap`]), and
//! [`EventAdapter::tick`] is called by a timer so that the last phrase of a
//! stream is closed even when no new deltas arrive.
//!
//! Mapping of provider events:
//!
//! | `SessionEvent` | `TranslationEvent` |
//! |---|---|
//! | `Text(delta)` | `Translated`: text accumulates, `delta` is the increment, `is_final = false` |
//! | `Audio(chunk)` | `TranslatedAudio` |
//! | `InputText(delta)` | `SourceText`: text accumulates, `is_final = false` |
//! | `SessionStarted` / `SessionConfigured` | `Status { Starting }` |
//! | `ResponseStarted` | `Status { Running }` |
//! | `RequestFinished` / `ResponseFinished` | the current phrase is closed at once (`is_final = true`) |
//! | a pause longer than `gap` (via `tick`) | the translated phrase and the source phrase are closed |
//!
//! `RequestStarted` produces no events. One session event may produce zero, one
//! or several translation events, hence the `Vec` return values.

use std::time::{Duration, Instant};

use transono::core::session_event::SessionEvent;

use crate::bus::{LangPair, LineId, LineState, TranslationEvent};

/// Adapter turning provider session events into translation events.
#[derive(Debug)]
pub struct EventAdapter {
    line: LineId,
    langs: LangPair,
    gap: Duration,
    seq: u64,
    translated: String,
    source: String,
    last_translated_at: Option<Instant>,
    last_source_at: Option<Instant>,
}

impl EventAdapter {
    /// Default pause after which a phrase is considered complete.
    pub const DEFAULT_GAP: Duration = Duration::from_millis(700);

    /// Create an adapter with the default phrase gap.
    pub fn new(line: LineId, langs: LangPair) -> Self {
        Self {
            line,
            langs,
            gap: Self::DEFAULT_GAP,
            seq: 0,
            translated: String::new(),
            source: String::new(),
            last_translated_at: None,
            last_source_at: None,
        }
    }

    /// Set the pause after which a phrase is considered complete.
    pub fn with_gap(mut self, gap: Duration) -> Self {
        self.gap = gap;
        self
    }

    /// Pause after which a phrase is considered complete.
    pub fn gap(&self) -> Duration {
        self.gap
    }

    /// Line the adapter belongs to.
    pub fn line(&self) -> LineId {
        self.line
    }

    /// Handle a session event.
    ///
    /// Phrases whose pause has already expired are closed first, so a delta
    /// arriving after a long silence starts a new phrase instead of extending
    /// the previous one.
    pub fn adapt(&mut self, ev: SessionEvent, now: Instant) -> Vec<TranslationEvent> {
        let mut out = self.tick(now);

        match ev {
            SessionEvent::Text(delta) => {
                self.translated.push_str(&delta);
                self.last_translated_at = Some(now);
                self.seq += 1;

                out.push(TranslationEvent::Translated {
                    line: self.line,
                    langs: self.langs.clone(),
                    text: self.translated.clone(),
                    delta: Some(delta),
                    is_final: false,
                    seq: self.seq,
                    at: now,
                });
            }

            SessionEvent::InputText(delta) => {
                self.source.push_str(&delta);
                self.last_source_at = Some(now);

                out.push(TranslationEvent::SourceText {
                    line: self.line,
                    text: self.source.clone(),
                    delta: Some(delta),
                    is_final: false,
                    at: now,
                });
            }

            SessionEvent::Audio(audio) => {
                out.push(TranslationEvent::TranslatedAudio {
                    line: self.line,
                    audio,
                    at: now,
                });
            }

            SessionEvent::SessionStarted(_) | SessionEvent::SessionConfigured(_) => {
                out.push(self.status(LineState::Starting, now));
            }

            SessionEvent::ResponseStarted => {
                out.push(self.status(LineState::Running, now));
            }

            SessionEvent::RequestFinished | SessionEvent::ResponseFinished => {
                out.extend(self.flush(now));
            }

            SessionEvent::RequestStarted => {}
        }

        out
    }

    /// Close the phrases whose pause has expired.
    ///
    /// Called by a timer: without it the last phrase of a stream would stay
    /// open forever, because no new delta arrives to reveal the pause.
    pub fn tick(&mut self, now: Instant) -> Vec<TranslationEvent> {
        let mut out = Vec::new();

        let translated_expired = self
            .last_translated_at
            .is_some_and(|at| now.saturating_duration_since(at) >= self.gap);
        if translated_expired && let Some(event) = self.close_translated(now) {
            out.push(event);
        }

        let source_expired = self
            .last_source_at
            .is_some_and(|at| now.saturating_duration_since(at) >= self.gap);
        if source_expired && let Some(event) = self.close_source(now) {
            out.push(event);
        }

        out
    }

    /// Close every open phrase, ignoring the pause.
    ///
    /// Used when the session ends, so that a phrase in progress is not lost.
    pub fn flush(&mut self, now: Instant) -> Vec<TranslationEvent> {
        let mut out = Vec::new();

        if let Some(event) = self.close_translated(now) {
            out.push(event);
        }
        if let Some(event) = self.close_source(now) {
            out.push(event);
        }

        out
    }

    /// Whether a phrase is currently open.
    pub fn is_open(&self) -> bool {
        self.last_translated_at.is_some() || self.last_source_at.is_some()
    }

    fn status(&self, state: LineState, now: Instant) -> TranslationEvent {
        TranslationEvent::Status {
            line: self.line,
            state,
            at: now,
        }
    }

    /// Close the translated phrase, if any text has accumulated.
    fn close_translated(&mut self, now: Instant) -> Option<TranslationEvent> {
        self.last_translated_at = None;

        if self.translated.is_empty() {
            return None;
        }

        self.seq += 1;

        Some(TranslationEvent::Translated {
            line: self.line,
            langs: self.langs.clone(),
            text: std::mem::take(&mut self.translated),
            delta: None,
            is_final: true,
            seq: self.seq,
            at: now,
        })
    }

    /// Close the source phrase, if any text has accumulated.
    fn close_source(&mut self, now: Instant) -> Option<TranslationEvent> {
        self.last_source_at = None;

        if self.source.is_empty() {
            return None;
        }

        Some(TranslationEvent::SourceText {
            line: self.line,
            text: std::mem::take(&mut self.source),
            delta: None,
            is_final: true,
            at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transono::audio::{Audio, PcmAudio};
    use transono::providers::openai::translation::OpenAITranslationConfig;

    fn adapter() -> EventAdapter {
        EventAdapter::new(
            LineId(0),
            LangPair {
                source: None,
                target: crate::bus::Language::code("ru"),
            },
        )
        .with_gap(Duration::from_millis(700))
    }

    fn test_audio() -> Audio {
        let spec = OpenAITranslationConfig::default()
            .audio_format()
            .spec()
            .clone();
        Audio::from_pcm(&PcmAudio::new(spec, 240)).expect("тестовый фрагмент")
    }

    #[test]
    fn deltas_accumulate_and_carry_increment() {
        let mut a = adapter();
        let t0 = Instant::now();

        let out: Vec<_> = vec![
            a.adapt(SessionEvent::Text("При".into()), t0),
            a.adapt(
                SessionEvent::Text("вет".into()),
                t0 + Duration::from_millis(100),
            ),
        ]
        .into_iter()
        .flatten()
        .collect();

        assert_eq!(out[0].text(), Some("При"));
        assert_eq!(out[0].delta(), Some("При"));
        assert!(!out[0].is_final());
        assert_eq!(out[1].text(), Some("Привет"), "текст накапливается");
        assert_eq!(out[1].delta(), Some("вет"), "delta — только приращение");
        assert_eq!(out[1].seq(), Some(2), "seq монотонно растёт");
        assert_eq!(out[1].line(), LineId(0));
    }

    #[test]
    fn pause_closes_the_phrase() {
        let mut a = adapter();
        let t0 = Instant::now();
        a.adapt(SessionEvent::Text("Привет".into()), t0);

        assert!(
            a.tick(t0 + Duration::from_millis(300)).is_empty(),
            "пауза ещё не истекла"
        );

        let closed = a.tick(t0 + Duration::from_millis(800));
        assert_eq!(closed.len(), 1);
        assert!(closed[0].is_final(), "по паузе фраза закрывается");
        assert_eq!(closed[0].text(), Some("Привет"));
        assert_eq!(closed[0].seq(), Some(2));
        assert_eq!(closed[0].delta(), None, "нового текста в закрытии нет");

        // Следующая фраза начинается с чистого накопителя.
        let next = a.adapt(
            SessionEvent::Text("Мир".into()),
            t0 + Duration::from_millis(900),
        );
        assert_eq!(next[0].text(), Some("Мир"));
        assert_eq!(next[0].seq(), Some(3));
    }

    #[test]
    fn delta_after_a_pause_starts_a_new_phrase() {
        let mut a = adapter();
        let t0 = Instant::now();
        a.adapt(SessionEvent::Text("Первая".into()), t0);

        let out = a.adapt(
            SessionEvent::Text("Вторая".into()),
            t0 + Duration::from_millis(2000),
        );

        assert_eq!(out.len(), 2, "сначала закрытие, затем новая фраза: {out:?}");
        assert!(out[0].is_final());
        assert_eq!(out[0].text(), Some("Первая"));
        assert_eq!(out[1].text(), Some("Вторая"));
        assert!(!out[1].is_final());
    }

    #[test]
    fn source_text_closes_on_its_own_pause() {
        let mut a = adapter();
        let t0 = Instant::now();

        let out = a.adapt(SessionEvent::InputText("hello".into()), t0);
        assert!(matches!(out[0], TranslationEvent::SourceText { .. }));
        assert!(!out[0].is_final());

        let closed = a.tick(t0 + Duration::from_millis(800));
        assert!(
            closed
                .iter()
                .any(|e| matches!(e, TranslationEvent::SourceText { is_final: true, .. })),
            "{closed:?}"
        );
        assert_eq!(closed[0].text(), Some("hello"));
    }

    #[test]
    fn audio_becomes_translated_audio_event() {
        let mut a = adapter();
        let out = a.adapt(SessionEvent::Audio(test_audio()), Instant::now());

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], TranslationEvent::TranslatedAudio { .. }));
        assert!(!out[0].is_final(), "звук фразу не закрывает");
    }

    #[test]
    fn session_events_become_status_events() {
        let mut a = adapter();
        let t0 = Instant::now();

        let starting = a.adapt(SessionEvent::SessionStarted("s-1".into()), t0);
        assert!(matches!(
            starting[0],
            TranslationEvent::Status {
                state: LineState::Starting,
                ..
            }
        ));

        let running = a.adapt(SessionEvent::ResponseStarted, t0);
        assert!(matches!(
            running[0],
            TranslationEvent::Status {
                state: LineState::Running,
                ..
            }
        ));

        assert!(
            a.adapt(SessionEvent::RequestStarted, t0).is_empty(),
            "RequestStarted событий не порождает"
        );
    }

    #[test]
    fn response_finished_closes_the_phrase_at_once() {
        let mut a = adapter();
        let t0 = Instant::now();
        a.adapt(SessionEvent::Text("Привет".into()), t0);

        let out = a.adapt(
            SessionEvent::ResponseFinished,
            t0 + Duration::from_millis(10),
        );

        assert_eq!(out.len(), 1);
        assert!(out[0].is_final(), "завершение ответа закрывает фразу сразу");
        assert_eq!(out[0].text(), Some("Привет"));
    }

    #[test]
    fn flush_closes_an_open_phrase_ignoring_the_pause() {
        let mut a = adapter();
        let t0 = Instant::now();
        a.adapt(SessionEvent::Text("Незаконченная".into()), t0);
        a.adapt(SessionEvent::InputText("unfinished".into()), t0);

        assert!(a.is_open());
        let out = a.flush(t0 + Duration::from_millis(10));

        assert_eq!(
            out.len(),
            2,
            "закрылись и перевод, и исходный текст: {out:?}"
        );
        assert!(out.iter().all(TranslationEvent::is_final));
        assert!(!a.is_open());
        assert!(a.flush(t0).is_empty(), "повторное закрытие ничего не даёт");
    }

    #[test]
    fn empty_pause_produces_nothing() {
        let mut a = adapter();
        let t0 = Instant::now();

        assert!(a.tick(t0).is_empty(), "закрывать нечего");
        assert!(a.tick(t0 + Duration::from_secs(10)).is_empty());
    }

    #[test]
    fn seq_grows_across_phrases() {
        let mut a = adapter();
        let t0 = Instant::now();

        a.adapt(SessionEvent::Text("раз".into()), t0);
        a.tick(t0 + Duration::from_millis(800));
        let out = a.adapt(
            SessionEvent::Text("два".into()),
            t0 + Duration::from_millis(900),
        );

        assert_eq!(
            out[0].seq(),
            Some(3),
            "нумерация не сбрасывается между фразами"
        );
    }
}
