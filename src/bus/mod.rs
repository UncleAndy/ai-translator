//! Translation event bus: normalized events and their fan-out to sinks.
//!
//! The bus is the only link between the translation core and the sinks. Every
//! subscriber gets every event, but through its own queue, so a slow or stuck
//! subscriber neither blocks the core nor affects the other subscribers.
//!
//! Overload policy, applied when a subscriber's queue is full:
//!
//! * a durable event (final text, state change) is always accepted — the oldest
//!   *intermediate* event is evicted to make room for it;
//! * an intermediate event is accepted when it can evict an older intermediate
//!   event, and is itself dropped when the queue holds only durable events.
//!
//! For live translation the current revision of a phrase matters more than a
//! superseded one, and every revision carries the whole accumulated phrase
//! text, so evicting intermediate events loses nothing but latency. Delivery
//! order is always preserved.

/// Normalized translation event model shared by the core and the sinks.
pub mod event;

pub use event::*;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// A subscriber's queue of pending events.
#[derive(Debug)]
struct Queue {
    items: VecDeque<Arc<TranslationEvent>>,
    capacity: usize,
    /// Events evicted because the queue was full.
    dropped: u64,
    /// Durable events accepted over capacity because nothing could be evicted.
    forced: u64,
    /// The subscriber is currently behind, so losses were already reported.
    lagging: bool,
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
            dropped: 0,
            forced: 0,
            lagging: false,
        }
    }

    /// Append an event, applying the overload policy.
    ///
    /// Returns `true` when the queue was full and something had to give.
    fn push(&mut self, event: Arc<TranslationEvent>) -> bool {
        if self.items.len() >= self.capacity {
            match self.items.iter().position(|e| !e.is_durable()) {
                // Make room by evicting the oldest evictable event.
                Some(pos) => {
                    self.items.remove(pos);
                    self.dropped += 1;
                }
                // Nothing to evict and the event is durable: keep both.
                None if event.is_durable() => {
                    self.forced += 1;
                }
                // Nothing to evict: the queue holds durable events only, so the
                // new intermediate revision is the one to drop.
                None => {
                    self.dropped += 1;
                    return true;
                }
            }
            self.items.push_back(event);
            return true;
        }

        self.items.push_back(event);
        false
    }

    fn pop(&mut self) -> Option<Arc<TranslationEvent>> {
        self.items.pop_front()
    }
}

/// A subscriber of the bus.
pub struct Subscriber {
    id: String,
    queue: Arc<Mutex<Queue>>,
    notify: Arc<Notify>,
}

impl Subscriber {
    /// Identifier this subscriber was created with.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Number of events evicted for this subscriber so far.
    ///
    /// A non-zero value means the subscriber is slower than the translation
    /// stream; the sink is expected to report it.
    pub fn dropped(&self) -> u64 {
        self.queue.lock().expect("bus queue poisoned").dropped
    }

    /// Wait for the next event.
    ///
    /// Returns `None` when the bus is gone and no events are left.
    pub async fn recv(&mut self) -> Option<Arc<TranslationEvent>> {
        loop {
            {
                let mut queue = self.queue.lock().expect("bus queue poisoned");
                if let Some(event) = queue.pop() {
                    queue.lagging = false;
                    return Some(event);
                }
            }
            self.notify.notified().await;
        }
    }

    /// Take the next event without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`TryRecvError::Empty`] when nothing is pending.
    pub fn try_recv(&mut self) -> Result<Arc<TranslationEvent>, TryRecvError> {
        let mut queue = self.queue.lock().expect("bus queue poisoned");
        match queue.pop() {
            Some(event) => {
                queue.lagging = false;
                Ok(event)
            }
            None => Err(TryRecvError::Empty),
        }
    }
}

/// Reason why [`Subscriber::try_recv`] returned nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// No events are pending.
    Empty,
}

impl std::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "нет ожидающих событий"),
        }
    }
}

impl std::error::Error for TryRecvError {}

/// A bus slot: everything the bus needs to reach one subscriber.
struct Slot {
    id: String,
    queue: Arc<Mutex<Queue>>,
    notify: Arc<Notify>,
}

struct BusInner {
    slots: Mutex<Vec<Slot>>,
}

/// The translation event bus.
///
/// Cloning a bus shares the same subscribers; every clone publishes to all of
/// them.
///
/// # Examples
///
/// ```
/// use ai_translator::bus::{EventBus, LineId, LineState, TranslationEvent};
/// use std::time::Instant;
///
/// let bus = EventBus::new();
/// let mut subscriber = bus.subscribe("console", 8);
///
/// bus.publish(TranslationEvent::Status {
///     line: LineId(0),
///     state: LineState::Running,
///     at: Instant::now(),
/// });
///
/// let event = subscriber.try_recv().expect("the event is delivered");
/// assert_eq!(event.line(), LineId(0));
/// ```
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<BusInner>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// Create a bus with no subscribers.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BusInner {
                slots: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Subscribe a receiver holding at most `capacity` pending events.
    pub fn subscribe(&self, id: &str, capacity: usize) -> Subscriber {
        let capacity = capacity.max(1);
        let queue = Arc::new(Mutex::new(Queue::new(capacity)));
        let notify = Arc::new(Notify::new());

        self.inner
            .slots
            .lock()
            .expect("bus slots poisoned")
            .push(Slot {
                id: id.to_string(),
                queue: queue.clone(),
                notify: notify.clone(),
            });

        Subscriber {
            id: id.to_string(),
            queue,
            notify,
        }
    }

    /// Deliver an event to every subscriber.
    ///
    /// This call never waits: it only appends to per-subscriber queues. A
    /// subscriber that is behind loses intermediate events, never final ones.
    pub fn publish(&self, event: TranslationEvent) {
        let event = Arc::new(event);
        let slots = self.inner.slots.lock().expect("bus slots poisoned");

        for slot in slots.iter() {
            let report = {
                let mut queue = slot.queue.lock().expect("bus queue poisoned");
                let full = queue.push(event.clone());
                let first_report = full && !queue.lagging;
                queue.lagging |= full;
                first_report
            };

            if report {
                tracing::warn!(
                    subscriber = %slot.id,
                    "приёмник отстаёт: промежуточные события вытесняются"
                );
            }

            slot.notify.notify_one();
        }
    }

    /// Number of events evicted for the subscriber with this identifier.
    pub fn dropped_for(&self, id: &str) -> u64 {
        self.slot(id)
            .map(|queue| queue.lock().expect("bus queue poisoned").dropped)
            .unwrap_or(0)
    }

    /// Number of durable events accepted over capacity for a subscriber.
    ///
    /// Non-zero means the subscriber is stuck: everything pending was final
    /// text or a state change, so nothing could be evicted.
    pub fn forced_for(&self, id: &str) -> u64 {
        self.slot(id)
            .map(|queue| queue.lock().expect("bus queue poisoned").forced)
            .unwrap_or(0)
    }

    /// Number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.inner.slots.lock().expect("bus slots poisoned").len()
    }

    fn slot(&self, id: &str) -> Option<Arc<Mutex<Queue>>> {
        self.inner
            .slots
            .lock()
            .expect("bus slots poisoned")
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| slot.queue.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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

    fn drain(subscriber: &mut Subscriber) -> Vec<Arc<TranslationEvent>> {
        let mut out = Vec::new();
        while let Ok(event) = subscriber.try_recv() {
            out.push(event);
        }
        out
    }

    #[tokio::test]
    async fn all_subscribers_receive_every_event() {
        let bus = EventBus::new();
        let mut a = bus.subscribe("a", 16);
        let mut b = bus.subscribe("b", 16);

        bus.publish(translated("один", true, 1));

        assert_eq!(a.recv().await.expect("event for a").text(), Some("один"));
        assert_eq!(b.recv().await.expect("event for b").text(), Some("один"));
        assert_eq!(bus.subscriber_count(), 2);
    }

    #[tokio::test]
    async fn slow_subscriber_does_not_block_the_rest() {
        let bus = EventBus::new();
        let mut slow = bus.subscribe("slow", 2);
        let mut fast = bus.subscribe("fast", 64);

        for seq in 1..=10 {
            bus.publish(translated("x", false, seq));
        }

        // The fast subscriber received everything.
        let got = drain(&mut fast);
        assert_eq!(got.len(), 10);
        assert_eq!(bus.dropped_for("fast"), 0);

        // The slow one kept only its capacity and counted the losses.
        let kept = drain(&mut slow);
        assert_eq!(kept.len(), 2);
        assert_eq!(bus.dropped_for("slow"), 8);
    }

    #[tokio::test]
    async fn final_events_are_never_dropped() {
        let bus = EventBus::new();
        let mut slow = bus.subscribe("slow", 1);

        bus.publish(translated("промежуточное", false, 1));
        bus.publish(translated("финал", true, 2));

        let first = slow.recv().await.expect("final event is delivered");
        assert!(first.is_final(), "финальное событие обязано дойти");
        assert_eq!(first.text(), Some("финал"));
        assert_eq!(bus.dropped_for("slow"), 1);
    }

    #[tokio::test]
    async fn durable_events_grow_the_queue_when_nothing_can_be_evicted() {
        let bus = EventBus::new();
        let mut slow = bus.subscribe("slow", 1);

        for seq in 1..=3 {
            bus.publish(translated("финал", true, seq));
        }

        assert_eq!(bus.forced_for("slow"), 2, "вытеснять было нечего");
        let kept = drain(&mut slow);
        assert_eq!(kept.len(), 3, "финальные события сохранены");
        assert_eq!(bus.dropped_for("slow"), 0);
    }

    #[tokio::test]
    async fn intermediate_event_is_dropped_when_only_durable_ones_are_pending() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("s", 1);

        bus.publish(TranslationEvent::Status {
            line: LineId(3),
            state: LineState::Running,
            at: Instant::now(),
        });
        bus.publish(translated("промежуточное", false, 1));

        let kept = drain(&mut subscriber);
        assert_eq!(kept.len(), 1, "очередь не растёт из-за промежуточных");
        assert!(matches!(*kept[0], TranslationEvent::Status { .. }));
        assert_eq!(bus.dropped_for("s"), 1);
        assert_eq!(bus.forced_for("s"), 0);
    }

    #[tokio::test]
    async fn delivery_order_is_preserved() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("s", 2);

        for seq in 1..=5 {
            let text = format!("шаг {seq}");
            bus.publish(translated(&text, seq == 5, seq));
        }

        let seqs: Vec<u64> = drain(&mut subscriber)
            .iter()
            .filter_map(|event| event.seq())
            .collect();
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        assert_eq!(seqs, sorted, "порядок доставки не нарушается");
        assert_eq!(
            *seqs.last().expect("events"),
            5,
            "последнее событие сохранено"
        );
    }

    #[tokio::test]
    async fn subscriber_reports_its_own_losses() {
        let bus = EventBus::new();
        let subscriber = bus.subscribe("s", 1);

        bus.publish(translated("a", false, 1));
        bus.publish(translated("b", false, 2));
        bus.publish(translated("c", false, 3));

        assert_eq!(subscriber.dropped(), 2);
        assert_eq!(subscriber.dropped(), bus.dropped_for("s"));
        assert_eq!(subscriber.id(), "s");
    }

    #[tokio::test]
    async fn publishing_without_subscribers_is_harmless() {
        let bus = EventBus::new();
        bus.publish(translated("никому", true, 1));
        assert_eq!(bus.subscriber_count(), 0);
        assert_eq!(bus.dropped_for("нет такого"), 0);
        assert_eq!(bus.forced_for("нет такого"), 0);
    }

    #[test]
    fn try_recv_reports_empty_queue() {
        let bus = EventBus::new();
        let mut subscriber = bus.subscribe("s", 1);
        assert_eq!(subscriber.try_recv().unwrap_err(), TryRecvError::Empty);
    }
}
