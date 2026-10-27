//! Keeping a provider session alive when the provider ends it.
//!
//! The OpenAI sessions have a lifetime of their own: after about an hour the
//! server closes the session and translation stops, even though the program,
//! its devices and its sinks are fine. Restarting the program helped, which is
//! exactly the diagnosis: nothing in it ever created a second session.
//!
//! This module is that second session. It watches one live session, waits for
//! it to end — or replaces it before the provider's limit, on its own timer —
//! and opens the next one, with a backoff that does not hammer the provider and
//! does not spin on a wrong key.
//!
//! The supervisor knows nothing about audio, TOML or OpenAI: a session is a
//! pair of "tell me when you are done" and "close yourself", which is what the
//! tests use.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// Why the supervisor is opening a new session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectReason {
    /// The session ended by itself: the provider closed it or the transport broke.
    Ended,
    /// The session was replaced before the provider's own limit.
    Proactive,
}

/// Why a new session could not be opened.
#[derive(Debug, Clone)]
pub enum OpenError {
    /// Worth another attempt soon: the network, DNS, a TLS handshake, a device.
    Transient(String),
    /// Credentials or quota. Retrying every second only burns attempts, so the
    /// supervisor waits much longer between them.
    Refused(String),
}

impl OpenError {
    /// Classify an error text from the provider or the transport.
    ///
    /// The provider reports authentication and quota problems as 401, 403 or
    /// 429 in the error message; everything else is treated as transient,
    /// because a wrong guess here costs seconds, while a wrong guess the other
    /// way costs a whole broadcast.
    pub fn classify(message: impl Into<String>) -> Self {
        let message = message.into();
        let lowered = message.to_lowercase();

        let refused = ["401", "403", "429", "insufficient_quota", "invalid api key"]
            .iter()
            .any(|marker| lowered.contains(marker));

        if refused {
            Self::Refused(message)
        } else {
            Self::Transient(message)
        }
    }

    /// The text to show a human.
    pub fn message(&self) -> &str {
        match self {
            Self::Transient(message) | Self::Refused(message) => message,
        }
    }

    /// Whether this error means "the key or the quota is wrong".
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

/// What the supervisor reports while it works.
#[derive(Debug, Clone)]
pub enum ReconnectReport {
    /// The old session is over; a new one is being opened right now.
    SessionEnded {
        /// Why the session was replaced.
        reason: ReconnectReason,
    },
    /// The old session is gone; another attempt will be made.
    Reconnecting {
        /// Why the session was replaced.
        reason: ReconnectReason,
        /// Number of the attempt that failed, starting at one.
        attempt: u32,
        /// How long the supervisor waits before the next one.
        delay: Duration,
        /// What went wrong.
        error: String,
    },
    /// A new session is up and translating again.
    Restored {
        /// Number of the attempt that succeeded.
        attempt: u32,
        /// Time from the end of the old session to the new one.
        after: Duration,
    },
    /// The quick attempts ran out; the supervisor keeps trying slowly.
    GaveUp {
        /// How many attempts were made.
        attempts: u32,
        /// Why the last one failed.
        error: String,
    },
}

/// How the supervisor behaves.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    /// Delay before the second attempt; doubles up to `max_delay`.
    pub first_delay: Duration,
    /// Ceiling of the delay between attempts.
    pub max_delay: Duration,
    /// How many attempts one interruption may take.
    pub attempts: u32,
    /// Delay after a refused session (credentials, quota).
    pub refused_delay: Duration,
    /// Replace the session after this long, before the provider does it;
    /// `None` reconnects only when the session actually ends.
    pub proactive: Option<Duration>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            attempts: 6,
            refused_delay: Duration::from_secs(60),
            proactive: None,
        }
    }
}

/// The most a supervisor may replace a session, after one interruption.
pub const MAX_ATTEMPTS: u32 = 240;

impl ReconnectPolicy {
    /// Milliseconds converted into the policy of one configuration.
    pub fn from_minutes(proactive_minutes: u32) -> Self {
        Self {
            proactive: (proactive_minutes > 0)
                .then(|| Duration::from_secs(u64::from(proactive_minutes) * 60)),
            ..Self::default()
        }
    }
}

/// A session as the supervisor sees it.
///
/// Deliberately minimal: the supervisor must be testable without a provider and
/// without a sound card, so all it needs is a way to learn that the session is
/// over and a way to shut it down.
pub trait LiveSession: Send + 'static {
    /// A receiver that changes when the session ends, however it ends.
    fn finished(&self) -> watch::Receiver<u64>;

    /// Stop the session and let go of everything it holds.
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

/// Everything `supervise` needs to open a session, boxed so that the future can
/// outlive the closure call.
pub type OpenFuture<S> = Pin<Box<dyn Future<Output = Result<S, OpenError>> + Send>>;

/// Watch a live session and replace it when it ends or when it is time to.
///
/// Returns when the session ends and no replacement could be opened, or when
/// `stop` says so — the latter matters for a Ctrl+C during a backoff.
pub async fn supervise<S, O, R>(
    mut open: O,
    first: S,
    mut stop: watch::Receiver<bool>,
    policy: ReconnectPolicy,
    mut report: R,
) where
    S: LiveSession,
    O: FnMut() -> OpenFuture<S>,
    R: FnMut(ReconnectReport),
{
    let mut session = first;

    loop {
        if *stop.borrow() {
            Box::new(session).close().await;
            return;
        }

        let Some(reason) = wait_for_end(session.finished(), policy.proactive, &mut stop).await
        else {
            // The program is stopping: the phrase in progress is flushed by
            // `close`, and no replacement is opened.
            Box::new(session).close().await;
            return;
        };

        tracing::warn!(
            "сессия перевода завершилась ({:?}), переподключаюсь",
            reason
        );
        Box::new(session).close().await;
        report(ReconnectReport::SessionEnded { reason });

        let started = Instant::now();
        let mut delay = policy.first_delay;
        let mut attempt: u32 = 0;
        let mut gave_up = false;

        loop {
            if *stop.borrow() {
                return;
            }

            if attempt > 0 {
                if sleep_or_stop(delay, &mut stop).await {
                    return;
                }
                delay = (delay * 2).min(policy.max_delay);
            }

            attempt += 1;
            if attempt > policy.attempts.min(MAX_ATTEMPTS) && !gave_up {
                // A broadcast is more important than a tidy limit: the quick
                // attempts are over, the slow ones are not. Saying so once lets
                // the screen show the trouble instead of freezing on the last
                // phrase.
                report(ReconnectReport::GaveUp {
                    attempts: attempt - 1,
                    error: "быстрые попытки исчерпаны, повторяю редко".to_string(),
                });
                gave_up = true;
                delay = policy.refused_delay.max(policy.max_delay);
            }

            match open().await {
                Ok(next) => {
                    let after = started.elapsed();
                    tracing::info!(
                        "сессия перевода восстановлена: попытка {attempt}, разрыв {after:?}"
                    );
                    report(ReconnectReport::Restored { attempt, after });
                    session = next;
                    break;
                }
                Err(error) => {
                    let refused = error.is_refused();
                    if refused {
                        delay = policy.refused_delay.max(policy.first_delay);
                    }
                    tracing::warn!(
                        "переподключение не удалось (попытка {attempt}): {}",
                        error.message()
                    );
                    report(ReconnectReport::Reconnecting {
                        reason,
                        attempt,
                        delay,
                        error: error.message().to_string(),
                    });
                }
            }
        }
    }
}

/// Wait for the session to end, for the proactive timer, or for the stop signal.
async fn wait_for_end(
    mut finished: watch::Receiver<u64>,
    proactive: Option<Duration>,
    stop: &mut watch::Receiver<bool>,
) -> Option<ReconnectReason> {
    let timer = async move {
        match proactive {
            Some(duration) => tokio::time::sleep(duration).await,
            // No timer: this branch simply never resolves.
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timer);

    tokio::select! {
        _ = stop.changed() => None,
        // An error means the sender is gone, which also means the session ended.
        _ = finished.changed() => Some(ReconnectReason::Ended),
        _ = &mut timer => Some(ReconnectReason::Proactive),
    }
}

/// Sleep, unless the program is stopping; returns `true` when it is.
async fn sleep_or_stop(delay: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = stop.changed() => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    /// A session that ends when the test says so.
    struct FakeSession {
        finished: watch::Receiver<u64>,
        closed: Arc<AtomicU32>,
    }

    impl LiveSession for FakeSession {
        fn finished(&self) -> watch::Receiver<u64> {
            self.finished.clone()
        }

        fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    /// One opener scripted by the test: each call takes the next outcome.
    #[derive(Clone)]
    struct Script {
        outcomes: Arc<Mutex<Vec<Result<(), OpenError>>>>,
        opened: Arc<AtomicU32>,
        ended: watch::Sender<u64>,
    }

    impl Script {
        fn new(outcomes: Vec<Result<(), OpenError>>) -> Self {
            let (ended, _) = watch::channel(0);
            Self {
                outcomes: Arc::new(Mutex::new(outcomes)),
                opened: Arc::new(AtomicU32::new(0)),
                ended,
            }
        }

        fn closed(&self) -> watch::Receiver<u64> {
            self.ended.subscribe()
        }

        fn end_session(&self) {
            self.ended.send_modify(|n| *n = n.wrapping_add(1));
        }

        /// Move to the next scripted outcome, if any.
        fn next(&self) -> Result<(), OpenError> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            let mut outcomes = self.outcomes.lock().expect("сценарий попыток");
            if outcomes.is_empty() {
                return Ok(());
            }
            outcomes.remove(0)
        }
    }

    fn fast_policy() -> ReconnectPolicy {
        ReconnectPolicy {
            first_delay: Duration::from_millis(5),
            max_delay: Duration::from_millis(20),
            attempts: 6,
            refused_delay: Duration::from_millis(40),
            proactive: None,
        }
    }

    /// Run the supervisor in a task with a fake session factory.
    fn spawn(
        script: Script,
        policy: ReconnectPolicy,
    ) -> (
        watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
        Arc<Mutex<Vec<ReconnectReport>>>,
    ) {
        let (stop_tx, stop_rx) = watch::channel(false);
        let first = FakeSession {
            finished: script.closed(),
            closed: Arc::new(AtomicU32::new(0)),
        };

        let opener = script.clone();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let collected = reports.clone();

        let task = tokio::spawn(async move {
            let open = move || {
                let opener = opener.clone();
                let future: OpenFuture<FakeSession> = Box::pin(async move {
                    let ended = opener.closed();
                    opener.next()?;
                    Ok(FakeSession {
                        finished: ended,
                        closed: Arc::new(AtomicU32::new(0)),
                    })
                });
                future
            };
            let mut reporter = move |report: ReconnectReport| {
                collected.lock().expect("reports").push(report);
            };
            supervise(open, first, stop_rx, policy, &mut reporter).await;
        });

        (stop_tx, task, reports)
    }

    /// Wait until the supervisor has reported something that satisfies `done`.
    ///
    /// Polling the shared list beats sleeping a fixed time: the tests must not
    /// depend on the machine being slow or fast.
    async fn wait_until(
        reports: &Arc<Mutex<Vec<ReconnectReport>>>,
        done: impl Fn(&[ReconnectReport]) -> bool,
    ) -> Vec<ReconnectReport> {
        let deadline = Instant::now() + Duration::from_secs(3);

        loop {
            {
                let collected = reports.lock().expect("reports");
                if done(&collected) {
                    return collected.clone();
                }
            }

            assert!(
                Instant::now() < deadline,
                "не дождались нужного отчёта: {:?}",
                reports.lock().expect("reports")
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn an_ended_session_is_replaced() {
        let script = Script::new(vec![Ok(())]);
        let (stop, task, reports) = spawn(script.clone(), fast_policy());

        tokio::time::sleep(Duration::from_millis(20)).await;
        script.end_session();

        let reports = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { .. }))
        })
        .await;
        let _ = stop.send(true);
        let _ = task.await;

        assert!(
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { attempt: 1, .. })),
            "сессия обязана быть заменена: {reports:?}"
        );
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_before_giving_up() {
        let script = Script::new(vec![
            Err(OpenError::Transient("no route to host".into())),
            Err(OpenError::Transient("tls handshake".into())),
            Ok(()),
        ]);
        let (stop, task, reports) = spawn(script.clone(), fast_policy());

        tokio::time::sleep(Duration::from_millis(20)).await;
        script.end_session();

        let reports = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { .. }))
        })
        .await;
        let _ = stop.send(true);
        let _ = task.await;

        let failures = reports
            .iter()
            .filter(|report| matches!(report, ReconnectReport::Reconnecting { .. }))
            .count();
        assert_eq!(failures, 2, "{reports:?}");
        assert!(
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { attempt: 3, .. })),
            "третья попытка обязана удаться: {reports:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_key_is_not_hammered() {
        let script = Script::new(vec![
            Err(OpenError::Refused("401 invalid api key".into())),
            Ok(()),
        ]);
        let policy = ReconnectPolicy {
            refused_delay: Duration::from_millis(150),
            ..fast_policy()
        };
        let (stop, task, reports) = spawn(script.clone(), policy);

        tokio::time::sleep(Duration::from_millis(20)).await;
        script.end_session();

        // Ждём признания отказа: после него надзиратель обязан замолчать
        // надолго, а не долбиться каждые полсекунды.
        let reports = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Reconnecting { .. }))
        })
        .await;

        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = stop.send(true);
        let _ = task.await;

        assert_eq!(
            script.opened.load(Ordering::SeqCst),
            1,
            "после 401 второй попытки за 100 мс быть не должно"
        );
        assert!(
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Reconnecting { .. })),
            "отказ обязан быть доложен: {reports:?}"
        );
    }

    #[tokio::test]
    async fn stopping_during_a_backoff_returns_at_once() {
        let script = Script::new(vec![Err(OpenError::Transient("down".into()))]);
        let policy = ReconnectPolicy {
            first_delay: Duration::from_secs(30),
            ..fast_policy()
        };
        let (stop, task, reports) = spawn(script.clone(), policy);

        tokio::time::sleep(Duration::from_millis(20)).await;
        script.end_session();

        // Дождаться отказа: значит, надзиратель уже спит в backoff.
        let _ = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Reconnecting { .. }))
        })
        .await;

        let started = Instant::now();
        let _ = stop.send(true);
        let _ = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .expect("остановка не должна ждать backoff");

        assert!(
            started.elapsed() < Duration::from_millis(400),
            "остановка заняла {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn the_proactive_timer_replaces_a_live_session() {
        let script = Script::new(vec![Ok(())]);
        let policy = ReconnectPolicy {
            proactive: Some(Duration::from_millis(30)),
            ..fast_policy()
        };
        let (stop, task, reports) = spawn(script.clone(), policy);

        let reports = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { .. }))
        })
        .await;
        let _ = stop.send(true);
        let _ = task.await;

        assert!(
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::Restored { .. })),
            "таймер обязан заменить сессию, не дожидаясь её конца: {reports:?}"
        );
    }

    #[tokio::test]
    async fn exhausted_attempts_are_reported() {
        let failures: Vec<Result<(), OpenError>> = (0..8)
            .map(|_| Err(OpenError::Transient("down".into())))
            .collect();
        let script = Script::new(failures);
        let (stop, task, reports) = spawn(script.clone(), fast_policy());

        tokio::time::sleep(Duration::from_millis(20)).await;
        script.end_session();

        let reports = wait_until(&reports, |reports| {
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::GaveUp { .. }))
        })
        .await;
        let _ = stop.send(true);
        let _ = task.await;

        assert!(
            reports
                .iter()
                .any(|report| matches!(report, ReconnectReport::GaveUp { .. })),
            "после шести попыток надзиратель обязан сдаться: {reports:?}"
        );
    }

    #[test]
    fn errors_are_classified_by_what_the_provider_says() {
        assert!(OpenError::classify("401 Unauthorized").is_refused());
        assert!(OpenError::classify("429 rate limited").is_refused());
        assert!(OpenError::classify("insufficient_quota").is_refused());
        assert!(!OpenError::classify("connection reset by peer").is_refused());
        assert!(!OpenError::classify("dns error").is_refused());
    }

    #[test]
    fn a_zero_minute_setting_turns_the_proactive_timer_off() {
        assert_eq!(ReconnectPolicy::from_minutes(0).proactive, None);
        assert_eq!(
            ReconnectPolicy::from_minutes(55).proactive,
            Some(Duration::from_secs(55 * 60))
        );
    }
}
