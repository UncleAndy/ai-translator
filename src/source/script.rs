//! Script source: replays a prepared sequence of translation events.
//!
//! Used for runs without a microphone, a network or an API key — demonstrations,
//! tests and the `script` mode of the command line.

use std::time::Duration;

use crate::bus::{EventBus, TranslationEvent};

/// Default pause between replayed events.
pub const DEFAULT_DELAY: Duration = Duration::from_millis(300);

/// Source that replays a fixed sequence of events into the bus.
#[derive(Debug, Clone)]
pub struct ScriptSource {
    events: Vec<TranslationEvent>,
    delay: Duration,
}

impl ScriptSource {
    /// Create a source over a prepared sequence.
    pub fn new(events: Vec<TranslationEvent>) -> Self {
        Self {
            events,
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

    /// The sequence being replayed.
    pub fn events(&self) -> &[TranslationEvent] {
        &self.events
    }

    /// Replay the sequence into the bus; returns the number of events published.
    ///
    /// The pause comes before every event but the first, so the sequence ends
    /// right after the last event instead of trailing off.
    pub async fn run(self, bus: &EventBus) -> usize {
        let mut published = 0;

        for event in self.events {
            if published > 0 && !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }

            bus.publish(event);
            published += 1;
        }

        published
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId, Subscriber};
    use std::time::Instant;

    fn langs() -> LangPair {
        LangPair {
            source: None,
            target: Language::code("ru"),
        }
    }

    fn events() -> Vec<TranslationEvent> {
        let at = Instant::now();
        vec![
            TranslationEvent::Translated {
                line: LineId(0),
                langs: langs(),
                text: "Привет".into(),
                delta: Some("Привет".into()),
                is_final: false,
                seq: 1,
                at,
            },
            TranslationEvent::Translated {
                line: LineId(0),
                langs: langs(),
                text: "Привет мир".into(),
                delta: Some(" мир".into()),
                is_final: true,
                seq: 2,
                at,
            },
        ]
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
    async fn replay_publishes_every_event_in_order() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("test", 16);

        let source = ScriptSource::new(events()).with_delay(Duration::from_millis(10));
        assert_eq!(source.events().len(), 2);
        assert_eq!(source.delay(), Duration::from_millis(10));

        assert_eq!(source.run(&bus).await, 2);

        let received = collect(&mut subscriber, 2).await;
        assert_eq!(received[0].text(), Some("Привет"));
        assert!(!received[0].is_final());
        assert_eq!(received[1].text(), Some("Привет мир"));
        assert!(received[1].is_final());
    }

    #[tokio::test]
    async fn replay_waits_between_events() {
        let bus = EventBus::new();
        let source = ScriptSource::new(events()).with_delay(Duration::from_millis(120));

        let started = Instant::now();
        source.run(&bus).await;

        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "пауза между событиями соблюдается: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn empty_script_publishes_nothing() {
        let bus = EventBus::new();
        assert_eq!(ScriptSource::new(Vec::new()).run(&bus).await, 0);
    }
}
