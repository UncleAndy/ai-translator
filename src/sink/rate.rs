//! Sliding window over the messages a sink posted in the last minute.
//!
//! Telegram counts every new message against a per-chat limit: about twenty a
//! minute, and a burst above that is refused. Both Telegram sinks share the same
//! arithmetic, so it lives here rather than in either of them.
//!
//! The window holds the times of the messages already posted: a send is allowed
//! while fewer than `limit` of them are younger than [`SEND_WINDOW`], otherwise it
//! waits for the oldest one to fall out.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The window the send limit counts over.
pub const SEND_WINDOW: Duration = Duration::from_secs(60);

/// Decision of [`SendWindow::plan`].
#[derive(Debug, PartialEq, Eq)]
pub enum Slot {
    /// Send now; the slot is taken.
    Now,
    /// Wait this long, then ask again.
    Wait(Duration),
}

/// Sliding window over the messages posted in the last minute.
pub struct SendWindow {
    limit: usize,
    window: Duration,
    slots: Mutex<VecDeque<Instant>>,
}

impl SendWindow {
    /// A window over the standard minute, allowing `limit` messages in it.
    pub fn new(limit: u32) -> Self {
        Self::with_window(limit, SEND_WINDOW)
    }

    /// A window of a custom length, for tests that must not wait a minute.
    pub fn with_window(limit: u32, window: Duration) -> Self {
        Self {
            limit: limit as usize,
            window,
            slots: Mutex::new(VecDeque::new()),
        }
    }

    /// Take a slot if one is free at `now`.
    pub fn plan(&self, now: Instant) -> Slot {
        if self.limit == 0 {
            return Slot::Now;
        }

        let mut slots = match self.slots.lock() {
            Ok(slots) => slots,
            // A poisoned window is not worth failing a translation over: let the
            // message through and let Telegram decide.
            Err(poisoned) => poisoned.into_inner(),
        };

        while let Some(oldest) = slots.front() {
            if now.saturating_duration_since(*oldest) >= self.window {
                slots.pop_front();
            } else {
                break;
            }
        }

        if slots.len() < self.limit {
            slots.push_back(now);
            return Slot::Now;
        }

        let oldest = slots.front().copied().unwrap_or(now);
        Slot::Wait(
            self.window
                .saturating_sub(now.saturating_duration_since(oldest)),
        )
    }

    /// Wait until a slot is free.
    pub async fn acquire(&self) {
        let mut announced = false;

        loop {
            match self.plan(Instant::now()) {
                Slot::Now => return,
                Slot::Wait(wait) => {
                    if !announced {
                        tracing::debug!(
                            "предел {} сообщений в минуту: ждём {} мс",
                            self.limit,
                            wait.as_millis()
                        );
                        announced = true;
                    }
                    tokio::time::sleep(wait.max(Duration::from_millis(10))).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_lets_the_first_messages_through_and_then_waits() {
        let window = SendWindow::new(20);
        let start = Instant::now();

        for i in 0..20 {
            let now = start + Duration::from_millis(i * 100);
            assert_eq!(window.plan(now), Slot::Now, "сообщение {i} проходит сразу");
        }

        match window.plan(start + Duration::from_secs(2)) {
            Slot::Wait(wait) => assert!(
                wait > Duration::from_secs(50) && wait <= SEND_WINDOW,
                "двадцать первое сообщение ждёт почти всю минуту: {wait:?}"
            ),
            other => panic!("двадцать первое сообщение должно ждать: {other:?}"),
        }
    }

    #[test]
    fn a_slot_frees_up_when_the_oldest_message_leaves_the_minute() {
        let window = SendWindow::new(2);
        let start = Instant::now();

        assert_eq!(window.plan(start), Slot::Now);
        assert_eq!(window.plan(start + Duration::from_secs(1)), Slot::Now);
        assert!(
            matches!(window.plan(start + Duration::from_secs(2)), Slot::Wait(_)),
            "третье сообщение ждёт"
        );
        assert_eq!(
            window.plan(start + SEND_WINDOW + Duration::from_secs(1)),
            Slot::Now,
            "через минуту место освободилось"
        );
    }

    #[test]
    fn a_zero_limit_does_not_hold_anything_back() {
        let window = SendWindow::new(0);
        let start = Instant::now();

        for i in 0..100 {
            assert_eq!(window.plan(start + Duration::from_millis(i)), Slot::Now);
        }
    }

    #[tokio::test]
    async fn acquire_returns_at_once_while_slots_are_free() {
        let window = SendWindow::with_window(2, Duration::from_millis(50));
        let started = Instant::now();

        window.acquire().await;
        window.acquire().await;

        assert!(
            started.elapsed() < Duration::from_millis(40),
            "два свободных места не должны ждать: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn acquire_waits_for_the_window_to_free_a_slot() {
        let window = SendWindow::with_window(1, Duration::from_millis(200));

        window.acquire().await;
        let started = Instant::now();
        window.acquire().await;

        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "второе место ждало окно: {:?}",
            started.elapsed()
        );
    }
}
