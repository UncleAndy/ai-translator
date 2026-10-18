//! Sink supervisor: starting, isolating and restarting sinks.
//!
//! Every sink gets its own bus subscriber and its own task, so a sink that
//! fails or hangs cannot affect the others: the bus already drops intermediate
//! events for a slow subscriber instead of blocking the core, and a failing
//! sink only ends its own task. The supervisor watches that task, records the
//! state and restarts the sink according to the policy — calling the sink
//! factory again, because a sink that returned an error is not reusable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::bus::{EventBus, Subscriber, TranslationEvent};
use crate::sink::{Result, TextSink, VoiceSink};

/// Pending events a sink may fall behind by before intermediate ones are dropped.
const DEFAULT_CAPACITY: usize = 64;

/// Restart policy of a failed sink.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    /// Maximum number of restarts.
    pub attempts: u32,
    /// Delay before a restart.
    pub delay: Duration,
}

impl RestartPolicy {
    /// A limited number of attempts with a delay.
    pub fn limited(attempts: u32, delay: Duration) -> Self {
        Self { attempts, delay }
    }

    /// Never restart a failed sink.
    pub fn none() -> Self {
        Self {
            attempts: 0,
            delay: Duration::ZERO,
        }
    }
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::limited(2, Duration::from_secs(1))
    }
}

/// Sink state, observable from outside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkState {
    /// The sink is running.
    Running,
    /// The sink failed and will be restarted.
    Restarting {
        /// Number of the restart being attempted.
        attempt: u32,
    },
    /// The sink failed for good.
    Error(String),
    /// The sink stopped in an orderly way.
    Stopped,
    /// The sink is not known to the supervisor.
    Unknown,
}

impl SinkState {
    /// The sink failed for good.
    pub fn is_error(&self) -> bool {
        matches!(self, SinkState::Error(_))
    }

    /// The sink is running.
    pub fn is_running(&self) -> bool {
        matches!(self, SinkState::Running)
    }

    /// The sink is not running and will not be started again.
    pub fn is_finished(&self) -> bool {
        matches!(self, SinkState::Error(_) | SinkState::Stopped)
    }
}

/// A sink together with everything needed to feed it.
enum SinkKind {
    /// A sink delivering text.
    Text(Box<dyn TextSink>),
    /// A sink delivering speech.
    Voice(Box<dyn VoiceSink>),
}

impl SinkKind {
    /// Hand one event to the sink.
    async fn feed(&mut self, event: &TranslationEvent) -> Result<()> {
        match self {
            SinkKind::Text(sink) => sink.on_event(event).await,
            // Голосовому приёмнику нужен только звук перевода.
            SinkKind::Voice(sink) => match event {
                TranslationEvent::TranslatedAudio { line, audio, .. } => {
                    sink.on_audio(*line, audio).await
                }
                _ => Ok(()),
            },
        }
    }

    /// Shut the sink down.
    async fn shutdown(self) -> Result<()> {
        match self {
            SinkKind::Text(sink) => sink.shutdown().await,
            SinkKind::Voice(sink) => sink.shutdown().await,
        }
    }
}

/// Builds a fresh sink instance, used on start and on every restart.
type SinkFactory = Arc<dyn Fn() -> SinkKind + Send + Sync>;

/// What the supervisor knows about one sink.
struct SinkHandle {
    state: Arc<Mutex<SinkState>>,
    starts: Arc<AtomicU32>,
    task: Option<JoinHandle<()>>,
}

/// Runs the enabled sinks and keeps them alive.
pub struct Supervisor {
    bus: EventBus,
    policy: RestartPolicy,
    capacity: usize,
    sinks: Mutex<HashMap<String, SinkHandle>>,
    stop_tx: watch::Sender<bool>,
}

impl Supervisor {
    /// Create a supervisor for the given bus.
    pub fn new(bus: EventBus, policy: RestartPolicy) -> Self {
        let (stop_tx, _) = watch::channel(false);

        Self {
            bus,
            policy,
            capacity: DEFAULT_CAPACITY,
            sinks: Mutex::new(HashMap::new()),
            stop_tx,
        }
    }

    /// Queue capacity of each sink subscriber.
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity.max(1);
        self
    }

    /// Start a text sink built by `factory`.
    ///
    /// The factory is called on start and on every restart, so it must produce
    /// a fresh sink each time.
    pub fn spawn<F>(&self, id: &str, factory: F)
    where
        F: Fn() -> Box<dyn TextSink> + Send + Sync + 'static,
    {
        self.spawn_kind(id, Arc::new(move || SinkKind::Text(factory())));
    }

    /// Start a voice sink built by `factory`.
    pub fn spawn_voice<F>(&self, id: &str, factory: F)
    where
        F: Fn() -> Box<dyn VoiceSink> + Send + Sync + 'static,
    {
        self.spawn_kind(id, Arc::new(move || SinkKind::Voice(factory())));
    }

    /// Deliver an event to every sink.
    pub fn publish(&self, event: TranslationEvent) {
        self.bus.publish(event);
    }

    /// State of a sink.
    pub fn state_of(&self, id: &str) -> SinkState {
        let sinks = self.sinks.lock().expect("список приёмников повреждён");
        let Some(handle) = sinks.get(id) else {
            return SinkState::Unknown;
        };

        handle
            .state
            .lock()
            .expect("состояние приёмника повреждено")
            .clone()
    }

    /// How many times the factory of a sink was called.
    pub fn starts_of(&self, id: &str) -> u32 {
        self.sinks
            .lock()
            .expect("список приёмников повреждён")
            .get(id)
            .map(|handle| handle.starts.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// States of all sinks.
    pub fn states(&self) -> Vec<(String, SinkState)> {
        let ids: Vec<String> = self
            .sinks
            .lock()
            .expect("список приёмников повреждён")
            .keys()
            .cloned()
            .collect();

        ids.into_iter()
            .map(|id| {
                let state = self.state_of(&id);
                (id, state)
            })
            .collect()
    }

    /// Stop every sink and wait until they are done.
    pub async fn shutdown(&mut self) {
        let _ = self.stop_tx.send(true);

        let tasks: Vec<JoinHandle<()>> = self
            .sinks
            .lock()
            .expect("список приёмников повреждён")
            .values_mut()
            .filter_map(|handle| handle.task.take())
            .collect();

        for task in tasks {
            let _ = task.await;
        }
    }

    /// Create the subscriber and the task of one sink.
    fn spawn_kind(&self, id: &str, factory: SinkFactory) {
        let subscriber = self.bus.subscribe(id, self.capacity);
        let state = Arc::new(Mutex::new(SinkState::Unknown));
        let starts = Arc::new(AtomicU32::new(0));
        let stop = self.stop_tx.subscribe();

        let task = tokio::spawn(run_sink(
            id.to_string(),
            factory,
            subscriber,
            self.policy,
            state.clone(),
            starts.clone(),
            stop,
        ));

        self.sinks
            .lock()
            .expect("список приёмников повреждён")
            .insert(
                id.to_string(),
                SinkHandle {
                    state,
                    starts,
                    task: Some(task),
                },
            );
    }
}

/// Feed one sink until it fails for good or is stopped.
async fn run_sink(
    id: String,
    factory: SinkFactory,
    mut subscriber: Subscriber,
    policy: RestartPolicy,
    state: Arc<Mutex<SinkState>>,
    starts: Arc<AtomicU32>,
    mut stop: watch::Receiver<bool>,
) {
    let mut attempt = 0;

    loop {
        let mut sink = factory();
        starts.fetch_add(1, Ordering::Relaxed);
        *state.lock().expect("состояние приёмника повреждено") = SinkState::Running;

        // Хотя бы одно обработанное событие означает, что приёмник ожил: тогда
        // счётчик попыток считает только подряд идущие отказы.
        let mut handled = false;

        let outcome = loop {
            tokio::select! {
                _ = stop.changed() => break Ok(()),
                event = subscriber.recv() => match event {
                    Some(event) => match sink.feed(&event).await {
                        Ok(()) => handled = true,
                        Err(error) => break Err(error.to_string()),
                    },
                    None => break Ok(()),
                },
            }
        };

        let _ = sink.shutdown().await;

        match outcome {
            Ok(()) => {
                *state.lock().expect("состояние приёмника повреждено") = SinkState::Stopped;
                return;
            }
            Err(reason) => {
                if handled {
                    tracing::warn!(
                        sink = %id,
                        "приёмник отказал после успешной работы, счётчик попыток сброшен: {reason}"
                    );
                    attempt = 0;
                }

                if attempt >= policy.attempts {
                    tracing::error!(sink = %id, "приёмник отказал окончательно: {reason}");
                    *state.lock().expect("состояние приёмника повреждено") =
                        SinkState::Error(reason);
                    return;
                }

                tracing::warn!(sink = %id, "приёмник перезапускается: {reason}");
                attempt = restart(&id, &state, policy, attempt).await;
            }
        }
    }
}

/// Wait out the restart delay, reporting the attempt in progress.
async fn restart(
    id: &str,
    state: &Arc<Mutex<SinkState>>,
    policy: RestartPolicy,
    attempt: u32,
) -> u32 {
    let attempt = attempt + 1;
    *state.lock().expect("состояние приёмника повреждено") = SinkState::Restarting { attempt };
    tracing::debug!(sink = %id, attempt, "перезапуск приёмника");
    tokio::time::sleep(policy.delay).await;
    attempt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId};
    use crate::sink::SinkError;
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;
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

    /// A sink that always fails.
    struct AlwaysFails;

    #[async_trait]
    impl TextSink for AlwaysFails {
        fn id(&self) -> &str {
            "always-fails"
        }

        async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
            Err(SinkError::Unavailable("нет связи".into()))
        }

        async fn shutdown(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    /// A sink that counts the events it received.
    #[derive(Clone)]
    struct CountingSink {
        id: String,
        seen: Arc<AtomicUsize>,
    }

    impl CountingSink {
        fn new(id: &str, seen: Arc<AtomicUsize>) -> Self {
            Self {
                id: id.to_string(),
                seen,
            }
        }
    }

    #[async_trait]
    impl TextSink for CountingSink {
        fn id(&self) -> &str {
            &self.id
        }

        async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
            self.seen.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn shutdown(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    /// A sink that fails on the listed calls, counting from zero.
    struct ScriptedSink {
        calls: Arc<AtomicUsize>,
        fail_on: Vec<usize>,
    }

    #[async_trait]
    impl TextSink for ScriptedSink {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);

            if self.fail_on.contains(&call) {
                Err(SinkError::Unavailable(format!("сбой на вызове {call}")))
            } else {
                Ok(())
            }
        }

        async fn shutdown(self: Box<Self>) -> Result<()> {
            Ok(())
        }
    }

    /// Wait until the state of a sink satisfies the predicate.
    async fn wait_until(
        supervisor: &Supervisor,
        id: &str,
        predicate: impl Fn(&SinkState) -> bool,
    ) -> SinkState {
        let deadline = Instant::now() + Duration::from_secs(3);

        loop {
            let state = supervisor.state_of(id);
            if predicate(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "состояние приёмника {id} не наступило: {state:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn failing_sink_does_not_stop_the_others() {
        let bus = EventBus::new();
        let mut healthy = bus.subscribe("healthy", 64);
        let supervisor = Supervisor::new(
            bus.clone(),
            RestartPolicy::limited(2, Duration::from_millis(10)),
        );

        let seen = Arc::new(AtomicUsize::new(0));
        supervisor.spawn("always-fails", || Box::new(AlwaysFails));
        supervisor.spawn("healthy", {
            let seen = seen.clone();
            move || Box::new(CountingSink::new("healthy", seen.clone()))
        });

        // Падающий приёмник получает событие на каждый перезапуск: без нового
        // события он просто ждёт и не отказывает снова.
        for seq in 1..=4 {
            supervisor.publish(translated("привет", true, seq));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let event = tokio::time::timeout(Duration::from_secs(1), healthy.recv())
            .await
            .expect("событие дошло до независимого подписчика")
            .expect("событие получено");
        assert_eq!(event.text(), Some("привет"));

        let state = wait_until(&supervisor, "always-fails", SinkState::is_error).await;
        assert!(matches!(state, SinkState::Error(_)), "{state:?}");
        assert_eq!(
            supervisor.state_of("healthy"),
            SinkState::Running,
            "отказ одного приёмника не тронул другой"
        );
        assert_eq!(
            seen.load(Ordering::Relaxed),
            4,
            "исправный приёмник получил все события"
        );
    }

    #[tokio::test]
    async fn restart_is_limited_and_the_factory_is_called_again() {
        let bus = EventBus::new();
        let supervisor = Supervisor::new(bus, RestartPolicy::limited(2, Duration::from_millis(10)));
        supervisor.spawn("always-fails", || Box::new(AlwaysFails));

        for seq in 1..=4 {
            supervisor.publish(translated("привет", true, seq));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        wait_until(&supervisor, "always-fails", SinkState::is_error).await;

        assert_eq!(
            supervisor.starts_of("always-fails"),
            3,
            "первый запуск и две попытки перезапуска"
        );
        assert_eq!(
            supervisor.state_of("always-fails"),
            SinkState::Error("внешний сервис недоступен: нет связи".into())
        );
    }

    #[tokio::test]
    async fn attempt_counter_resets_after_a_successful_event() {
        let bus = EventBus::new();
        let supervisor = Supervisor::new(bus, RestartPolicy::limited(1, Duration::from_millis(10)));

        // Приёмник падает на первом и третьем событии, между ними работает.
        let calls = Arc::new(AtomicUsize::new(0));
        supervisor.spawn("scripted", {
            let calls = calls.clone();
            move || {
                Box::new(ScriptedSink {
                    calls: calls.clone(),
                    fail_on: vec![0, 2],
                })
            }
        });

        for seq in 1..=4 {
            supervisor.publish(translated("привет", true, seq));
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        assert_eq!(
            supervisor.starts_of("scripted"),
            3,
            "две попытки израсходованы по одному отказу каждая"
        );
        assert_eq!(
            supervisor.state_of("scripted"),
            SinkState::Running,
            "после успешного события счётчик отказов сброшен"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn policy_without_attempts_fails_at_once() {
        let bus = EventBus::new();
        let supervisor = Supervisor::new(bus, RestartPolicy::none());
        supervisor.spawn("always-fails", || Box::new(AlwaysFails));

        supervisor.publish(translated("привет", true, 1));
        wait_until(&supervisor, "always-fails", SinkState::is_error).await;

        assert_eq!(supervisor.starts_of("always-fails"), 1);
    }

    #[tokio::test]
    async fn shutdown_stops_the_sinks_and_keeps_their_state() {
        let bus = EventBus::new();
        let mut supervisor = Supervisor::new(bus, RestartPolicy::none());
        let seen = Arc::new(AtomicUsize::new(0));
        supervisor.spawn("counter", {
            let seen = seen.clone();
            move || Box::new(CountingSink::new("counter", seen.clone()))
        });

        wait_until(&supervisor, "counter", SinkState::is_running).await;
        supervisor.publish(translated("привет", true, 1));
        supervisor.shutdown().await;

        assert_eq!(supervisor.state_of("counter"), SinkState::Stopped);
        assert!(supervisor.state_of("counter").is_finished());

        let received = seen.load(Ordering::Relaxed);
        supervisor.publish(translated("после остановки", true, 2));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            seen.load(Ordering::Relaxed),
            received,
            "остановленный приёмник события не получает"
        );
    }

    #[tokio::test]
    async fn unknown_sink_is_reported_as_unknown() {
        let bus = EventBus::new();
        let supervisor = Supervisor::new(bus, RestartPolicy::none());

        assert_eq!(supervisor.state_of("нет такого"), SinkState::Unknown);
        assert_eq!(supervisor.starts_of("нет такого"), 0);
        assert!(supervisor.states().is_empty());
    }

    #[tokio::test]
    async fn states_list_every_sink() {
        let bus = EventBus::new();
        let mut supervisor = Supervisor::new(bus, RestartPolicy::none());
        supervisor.spawn("counter", || {
            Box::new(CountingSink::new("counter", Arc::new(AtomicUsize::new(0))))
        });

        wait_until(&supervisor, "counter", SinkState::is_running).await;

        let states = supervisor.states();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].0, "counter");
        assert!(states[0].1.is_running());

        supervisor.shutdown().await;
    }
}
