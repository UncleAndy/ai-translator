//! Translation line wiring over transono: devices, provider session, bridge.
//!
//! A [`transono::line::TranslationLine`] reports what happens inside a provider
//! session as a stream of [`SessionEvent`]s. The bridge here is the other half
//! of that link: it converts session events into translation events and
//! publishes them on the bus, and it owns the timer that closes a phrase once
//! the deltas stop arriving.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use transono::core::session_event::SessionEvent;

use crate::bus::{EventBus, TranslationEvent};
use crate::core_glue::adapt::EventAdapter;

/// Shortest timer period, so that a tiny gap does not spin the timer.
const MIN_PERIOD: Duration = Duration::from_millis(10);

/// Session → bus bridge.
pub struct EventBridge {
    adapter: EventAdapter,
    bus: EventBus,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    period: Duration,
}

impl EventBridge {
    /// Create a bridge over a session event channel.
    ///
    /// The timer period is a quarter of the phrase gap: close enough to the
    /// boundary to feel immediate, and rare enough not to matter.
    pub fn new(
        adapter: EventAdapter,
        bus: EventBus,
        events: mpsc::UnboundedReceiver<SessionEvent>,
    ) -> Self {
        let period = (adapter.gap() / 4).max(MIN_PERIOD);

        Self {
            adapter,
            bus,
            events,
            period,
        }
    }

    /// Timer period used to close phrases.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Feed the bus until the session event channel is closed.
    ///
    /// When the channel closes, the phrase in progress is flushed: the session
    /// is over, so waiting out the pause would only lose the last phrase.
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.period);

        loop {
            tokio::select! {
                event = self.events.recv() => match event {
                    Some(event) => {
                        let now = Instant::now();
                        let events = self.adapter.adapt(event, now);
                        self.publish(events);
                    }
                    None => break,
                },
                _ = ticker.tick() => {
                    let now = Instant::now();
                    let events = self.adapter.tick(now);
                    self.publish(events);
                }
            }
        }

        let now = Instant::now();
        let events = self.adapter.flush(now);
        self.publish(events);
    }

    /// Publish every event of a batch.
    fn publish(&self, events: Vec<TranslationEvent>) {
        for event in events {
            self.bus.publish(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId};
    use std::time::Duration;

    fn adapter(gap: Duration) -> EventAdapter {
        EventAdapter::new(
            LineId(0),
            LangPair {
                source: None,
                target: Language::code("ru"),
            },
        )
        .with_gap(gap)
    }

    async fn recv(subscriber: &mut crate::bus::Subscriber) -> TranslationEvent {
        tokio::time::timeout(Duration::from_secs(2), subscriber.recv())
            .await
            .expect("событие пришло вовремя")
            .expect("событие получено")
            .as_ref()
            .clone()
    }

    #[tokio::test]
    async fn bridge_closes_the_phrase_by_timer() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("test", 32);
        let (tx, rx) = mpsc::unbounded_channel();

        let bridge = EventBridge::new(adapter(Duration::from_millis(60)), bus.clone(), rx);
        assert_eq!(bridge.period(), Duration::from_millis(15));

        let task = tokio::spawn(bridge.run());
        tx.send(SessionEvent::Text("Привет".into()))
            .expect("событие отправлено");

        let first = recv(&mut subscriber).await;
        assert_eq!(first.text(), Some("Привет"));
        assert!(!first.is_final(), "первое событие — ревизия фразы");

        let closed = recv(&mut subscriber).await;
        assert!(closed.is_final(), "по таймеру фраза закрывается");
        assert_eq!(closed.text(), Some("Привет"));

        drop(tx);
        task.await.expect("мост завершился");
    }

    #[tokio::test]
    async fn bridge_flushes_an_open_phrase_when_the_session_ends() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("test", 32);
        let (tx, rx) = mpsc::unbounded_channel();

        // Пауза длиннее теста: закрыть фразу может только конец сессии.
        let bridge = EventBridge::new(adapter(Duration::from_secs(30)), bus.clone(), rx);
        let task = tokio::spawn(bridge.run());

        tx.send(SessionEvent::Text("Незаконченная".into()))
            .expect("событие отправлено");

        let first = recv(&mut subscriber).await;
        assert!(!first.is_final());

        drop(tx);

        let closed = recv(&mut subscriber).await;
        assert!(closed.is_final(), "конец сессии закрывает фразу");
        assert_eq!(closed.text(), Some("Незаконченная"));

        task.await.expect("мост завершился");
    }

    #[tokio::test]
    async fn bridge_publishes_status_and_audio() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("test", 32);
        let (tx, rx) = mpsc::unbounded_channel();

        let bridge = EventBridge::new(adapter(Duration::from_millis(60)), bus.clone(), rx);
        let task = tokio::spawn(bridge.run());

        tx.send(SessionEvent::ResponseStarted)
            .expect("событие отправлено");

        let status = recv(&mut subscriber).await;
        assert!(
            matches!(status, TranslationEvent::Status { .. }),
            "{status:?}"
        );

        drop(tx);
        task.await.expect("мост завершился");
    }
}
