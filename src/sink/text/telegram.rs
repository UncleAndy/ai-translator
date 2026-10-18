//! Telegram sink: posting the translation into a channel through a bot.
//!
//! The translation is synchronous, so the channel shows it as it appears: the
//! first revision of a phrase is posted at once, and every following revision
//! edits that same message instead of posting a new one. When the phrase ends,
//! the message stays as the finished sentence and the next phrase starts a new
//! one.
//!
//! The channel is the one named in the configuration; the bot must be an
//! administrator there with the right to post. A channel is chosen as the
//! destination because Telegram allows a bot to publish into it without the
//! restrictions a group has: no members to subscribe, no privacy mode, no
//! per-chat rate limit that a broadcast would run into.
//!
//! Messages go out as plain text, without a parse mode: a translation is not
//! markup, so nothing in it may be interpreted as formatting and nothing needs
//! escaping. Telegram caps one message at 4096 characters, so a phrase that
//! grows longer than that spills into the next message.
//!
//! Two limits keep the sink inside the rate Telegram allows, both taken from the
//! configuration:
//!
//! * no more than `max_sends_per_minute` new messages a minute: a phrase that
//!   would exceed it waits until the oldest message leaves the minute;
//! * an edit of one message no more often than `edit_interval_ms`, except for the
//!   revision that ends the phrase — that one is always applied. A skipped
//!   revision costs nothing: the accumulated text of the phrase arrives with the
//!   next event, and the final revision carries the whole sentence.
//!
//! Zero in either field removes that limit.
//!
//! The bot token is read once, when the sink is built: a missing variable is
//! reported at start-up instead of failing in the middle of a run. The token is
//! part of the request URL, so every error message passes through
//! [`reqwest::Error::without_url`] and the token never reaches a log line.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::bus::TranslationEvent;
use crate::sink::rate::SendWindow;
use crate::sink::{Result, SinkError, TextSink};

/// Bot API root.
pub const API_BASE: &str = "https://api.telegram.org";

/// Longest text Telegram accepts in one message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 4096;

/// Shortest time between two edits of one message.
pub const DEFAULT_EDIT_INTERVAL: Duration = Duration::from_secs(1);

/// How many new messages may be posted per minute by default.
pub const DEFAULT_SENDS_PER_MINUTE: u32 = 20;

/// How long a rate limit is waited out before the sink gives up.
const RETRY_AFTER_LIMIT_SECS: u64 = 30;

/// Request timeout: a stuck request must not hold the sink forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Reply Telegram gives when an edit would not change anything.
const NOT_MODIFIED: &str = "message is not modified";

/// Split a text into pieces of at most `limit` characters.
///
/// The split prefers whitespace, so a phrase breaks between words; a word that
/// is longer than the limit on its own is cut in the middle, because Telegram
/// refuses anything longer.
///
/// # Examples
///
/// ```
/// use ai_translator::sink::text::telegram::split_message;
///
/// assert_eq!(split_message("привет мир", 20), vec!["привет мир"]);
/// ```
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    let mut pieces = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if current.chars().count() == limit {
            if let Some(cut) = current.rfind(char::is_whitespace)
                && cut > 0
            {
                let tail = current.split_off(cut);
                pieces.push(current.trim().to_string());
                current = tail.trim_start().to_string();
            } else {
                pieces.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
    }

    if !current.trim().is_empty() {
        pieces.push(current.trim().to_string());
    }

    pieces.retain(|piece| !piece.is_empty());
    pieces
}

/// Parameters of the Telegram sink.
pub struct TelegramSettings {
    /// Chat identifier: a number, `@channel`, or the id of a private chat.
    pub chat_id: String,
    /// Bot token.
    pub token: String,
    /// Bot API root: `https://api.telegram.org` unless a compatible server is used.
    pub base: String,
    /// Post the source text above the translation.
    pub bilingual: bool,
    /// Send without a notification sound.
    pub silent: bool,
    /// Forum topic to post into.
    pub thread: Option<i64>,
    /// Shortest time between two edits of one message.
    pub edit_interval: Duration,
    /// How many new messages may be posted per minute; zero means no limit.
    pub max_sends_per_minute: u32,
}

impl TelegramSettings {
    /// Parameters with the default Bot API root and no optional behaviour.
    pub fn new(chat_id: &str, token: &str) -> Self {
        Self {
            chat_id: chat_id.to_string(),
            token: token.to_string(),
            base: API_BASE.to_string(),
            bilingual: false,
            silent: false,
            thread: None,
            edit_interval: DEFAULT_EDIT_INTERVAL,
            max_sends_per_minute: DEFAULT_SENDS_PER_MINUTE,
        }
    }
}

impl std::fmt::Debug for TelegramSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token must not reach a log line.
        f.debug_struct("TelegramSettings")
            .field("chat_id", &self.chat_id)
            .field("base", &self.base)
            .field("bilingual", &self.bilingual)
            .field("silent", &self.silent)
            .field("thread", &self.thread)
            .field("edit_interval", &self.edit_interval)
            .finish_non_exhaustive()
    }
}

/// Everything the sink takes from the configuration.
#[derive(Debug, Clone, Copy)]
pub struct TelegramSpec<'a> {
    /// Channel to publish into: `@name` or a numeric id.
    pub chat_id: &'a str,
    /// Variable holding the bot token.
    pub token_env: &'a str,
    /// Post the source text above the translation.
    pub bilingual: bool,
    /// Send without a notification sound.
    pub silent: bool,
    /// Forum topic, when the channel is tied to a forum.
    pub thread: Option<i64>,
    /// New messages allowed per minute; zero removes the limit.
    pub max_sends_per_minute: u32,
    /// Shortest time between two edits of one message.
    pub edit_interval: Duration,
}

/// A message carrying the phrase that is still being translated.
struct LiveMessage {
    /// Identifier of the message in the chat.
    id: i64,
    /// Text the message holds now.
    text: String,
    /// When it was edited last.
    edited_at: Instant,
}

/// Posts the translation into a Telegram chat, growing one message per phrase.
pub struct TelegramSink {
    id: String,
    settings: TelegramSettings,
    client: reqwest::Client,
    /// Sliding window of the messages posted in the last minute.
    sends: SendWindow,
    live: Option<LiveMessage>,
    last_source: String,
    sent: u64,
    edited: u64,
}

impl std::fmt::Debug for TelegramSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramSink")
            .field("id", &self.id)
            .field("settings", &self.settings)
            .field("sent", &self.sent)
            .field("edited", &self.edited)
            .finish_non_exhaustive()
    }
}

impl TelegramSink {
    /// Create a sink from ready-made parameters.
    ///
    /// # Errors
    ///
    /// Returns an error when the chat or the token is empty, or when the HTTP
    /// client cannot be built.
    pub fn new(id: &str, settings: TelegramSettings) -> Result<Self> {
        if settings.chat_id.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустой идентификатор чата Telegram".into(),
            ));
        }
        if settings.token.trim().is_empty() {
            return Err(SinkError::Protocol("пустой токен бота Telegram".into()));
        }

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| SinkError::Unavailable(format!("не удалось создать HTTP-клиент: {e}")))?;

        let sends = SendWindow::new(settings.max_sends_per_minute);

        Ok(Self {
            id: id.to_string(),
            settings,
            client,
            sends,
            live: None,
            last_source: String::new(),
            sent: 0,
            edited: 0,
        })
    }

    /// Create a sink from the configuration, reading the bot token from
    /// `spec.token_env`.
    ///
    /// # Errors
    ///
    /// Returns an error when the chat or the variable name is empty, when the
    /// variable is not set, or when the sink itself cannot be created.
    pub fn from_spec(id: &str, spec: TelegramSpec<'_>) -> Result<Self> {
        if spec.token_env.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустое имя переменной с токеном бота".into(),
            ));
        }

        let token = std::env::var(spec.token_env).map_err(|_| {
            SinkError::Protocol(format!("не задана переменная окружения {}", spec.token_env))
        })?;

        let mut settings = TelegramSettings::new(spec.chat_id, &token);
        settings.bilingual = spec.bilingual;
        settings.silent = spec.silent;
        settings.thread = spec.thread;
        settings.max_sends_per_minute = spec.max_sends_per_minute;
        settings.edit_interval = spec.edit_interval;

        Self::new(id, settings)
    }

    /// Number of posted messages.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Number of applied edits.
    pub fn edited(&self) -> u64 {
        self.edited
    }

    /// Text of a message: the source above the translation in bilingual mode.
    fn message_text(&self, translation: &str) -> String {
        let translation = translation.trim();
        let source = self.last_source.trim();

        if self.settings.bilingual && !source.is_empty() {
            format!("{source}\n{translation}")
        } else {
            translation.to_string()
        }
    }

    /// Call a Bot API method, waiting out a single rate limit.
    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let url = format!(
            "{}/bot{}/{method}",
            self.settings.base.trim_end_matches('/'),
            self.settings.token
        );

        let mut waited_out = false;

        loop {
            let response = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                // A transport error carries the URL, and the URL carries the
                // token: drop it before the message can reach a log.
                .map_err(|e| {
                    SinkError::Unavailable(format!("Telegram недоступен: {}", e.without_url()))
                })?;

            let status = response.status();
            let payload: Value = response.json().await.unwrap_or(Value::Null);

            if payload["ok"] == json!(true) {
                return Ok(payload["result"].clone());
            }

            let description = payload["description"]
                .as_str()
                .unwrap_or("ответ без описания");

            // A rate limit is worth waiting out once; everything else is a real
            // error and belongs to the supervisor.
            if status.as_u16() == 429
                && !waited_out
                && let Some(wait) = payload["parameters"]["retry_after"].as_u64()
                && wait <= RETRY_AFTER_LIMIT_SECS
            {
                tracing::warn!("Telegram придержал сообщение на {wait} с");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                waited_out = true;
                continue;
            }

            return Err(SinkError::Unavailable(format!(
                "Telegram ответил {status}: {description}"
            )));
        }
    }

    /// Post a message and return its identifier.
    ///
    /// Waits for a free slot when the per-minute limit is used up: Telegram
    /// counts new messages against it and would refuse the phrase otherwise.
    async fn send(&self, text: &str) -> Result<i64> {
        self.sends.acquire().await;

        let mut body = json!({
            "chat_id": self.settings.chat_id,
            "text": text,
            "disable_notification": self.settings.silent,
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(thread) = self.settings.thread {
            body["message_thread_id"] = json!(thread);
        }

        let result = self.call("sendMessage", body).await?;
        result["message_id"]
            .as_i64()
            .ok_or_else(|| SinkError::Protocol("Telegram не вернул номер сообщения".into()))
    }

    /// Replace the text of a message.
    async fn edit(&self, message_id: i64, text: &str) -> Result<()> {
        let mut body = json!({
            "chat_id": self.settings.chat_id,
            "message_id": message_id,
            "text": text,
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(thread) = self.settings.thread {
            // Topics need the thread on edits as well, or the bot refuses.
            body["message_thread_id"] = json!(thread);
        }

        match self.call("editMessageText", body).await {
            Ok(_) => Ok(()),
            // An edit that changes nothing is not a failure: the message already
            // holds exactly this text.
            Err(SinkError::Unavailable(reason)) if reason.contains(NOT_MODIFIED) => Ok(()),
            Err(other) => Err(other),
        }
    }

    /// Put the text of the phrase into the chat, editing the message in place.
    async fn apply(&mut self, text: &str) -> Result<()> {
        let pieces = split_message(text, MAX_MESSAGE_CHARS);
        let Some((first, rest)) = pieces.split_first() else {
            return Ok(());
        };

        match self.live.take() {
            // The phrase is new: the first message starts it.
            None => {
                let id = self.send(first).await?;
                self.sent += 1;
                self.live = Some(LiveMessage {
                    id,
                    text: first.clone(),
                    edited_at: Instant::now(),
                });
            }

            // The phrase is running: its message grows in place.
            Some(mut live) => {
                if live.text != *first {
                    self.edit(live.id, first).await?;
                    self.edited += 1;
                }
                live.text.clone_from(first);
                live.edited_at = Instant::now();
                self.live = Some(live);
            }
        }

        // A phrase longer than one message spills into the next ones; the last
        // of them takes the following revisions.
        for piece in rest {
            let id = self.send(piece).await?;
            self.sent += 1;
            self.live = Some(LiveMessage {
                id,
                text: piece.clone(),
                edited_at: Instant::now(),
            });
        }

        Ok(())
    }
}

#[async_trait]
impl TextSink for TelegramSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()> {
        match ev {
            TranslationEvent::SourceText { text, .. } => {
                self.last_source = text.clone();
            }

            TranslationEvent::Translated { text, is_final, .. } => {
                if text.trim().is_empty() {
                    return Ok(());
                }

                let message = self.message_text(text);

                // The first revision and the last one always reach the chat; the
                // ones between them wait for the throttle, and what they carried
                // arrives with the next revision anyway.
                let due = self.live.is_none()
                    || *is_final
                    || self.live.as_ref().is_some_and(|live| {
                        live.edited_at.elapsed() >= self.settings.edit_interval
                    });

                if due && !message.is_empty() {
                    self.apply(&message).await?;
                }

                if *is_final {
                    // The finished phrase stays as the last message; the next
                    // phrase starts a new one.
                    self.live = None;
                }
            }

            TranslationEvent::Status { .. } | TranslationEvent::TranslatedAudio { .. } => {}
        }

        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, routing::post};

    use super::*;
    use crate::bus::{LangPair, Language, LineId};

    /// Test token: the request paths carry it, and it must stay out of errors.
    const TEST_TOKEN: &str = "123456:TEST-TOKEN";

    /// One request the stand-in received.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        body: Value,
    }

    /// Start a Bot API stand-in, returning its root and the collected requests.
    async fn stand_in(reply: fn(&str) -> (u16, Value)) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let for_send = captured.clone();
        let for_edit = captured.clone();

        let app = Router::new()
            .route(
                "/bot123456:TEST-TOKEN/sendMessage",
                post(move |Json(body): Json<Value>| {
                    let seen = for_send.clone();
                    async move {
                        seen.lock().unwrap().push(Seen {
                            method: "sendMessage".into(),
                            body: body.clone(),
                        });
                        let (code, payload) = reply("sendMessage");
                        (
                            axum::http::StatusCode::from_u16(code).unwrap(),
                            Json(payload),
                        )
                    }
                }),
            )
            .route(
                "/bot123456:TEST-TOKEN/editMessageText",
                post(move |Json(body): Json<Value>| {
                    let seen = for_edit.clone();
                    async move {
                        seen.lock().unwrap().push(Seen {
                            method: "editMessageText".into(),
                            body: body.clone(),
                        });
                        let (code, payload) = reply("editMessageText");
                        (
                            axum::http::StatusCode::from_u16(code).unwrap(),
                            Json(payload),
                        )
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{addr}"), captured)
    }

    /// Normal Bot API answer: a message is created or edited.
    fn ok(method: &str) -> (u16, Value) {
        let result = if method == "sendMessage" {
            json!({ "message_id": 77 })
        } else {
            json!({ "message_id": 77, "text": "ok" })
        };
        (200, json!({ "ok": true, "result": result }))
    }

    fn chat_not_found(_method: &str) -> (u16, Value) {
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: chat not found"}),
        )
    }

    fn not_modified(method: &str) -> (u16, Value) {
        // Only an edit can be "not modified"; the first message is always new.
        if method == "sendMessage" {
            return ok(method);
        }
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: message is not modified"}),
        )
    }

    fn sink(base: &str, interval: Duration) -> TelegramSink {
        let mut settings = TelegramSettings::new("-1001234567890", TEST_TOKEN);
        settings.base = base.to_string();
        settings.edit_interval = interval;
        TelegramSink::new("telegram", settings).unwrap()
    }

    fn translated(text: &str, is_final: bool) -> TranslationEvent {
        TranslationEvent::Translated {
            line: LineId(0),
            langs: LangPair {
                source: None,
                target: Language::code("ru"),
            },
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final,
            seq: 1,
            at: Instant::now(),
        }
    }

    fn source_text(text: &str) -> TranslationEvent {
        TranslationEvent::SourceText {
            line: LineId(0),
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final: true,
            at: Instant::now(),
        }
    }

    fn methods(seen: &Arc<Mutex<Vec<Seen>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .map(|s| s.method.clone())
            .collect()
    }

    fn texts(seen: &Arc<Mutex<Vec<Seen>>>) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .map(|s| s.body["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn short_text_is_one_piece() {
        assert_eq!(split_message("привет мир", 100), vec!["привет мир"]);
    }

    #[test]
    fn text_of_exactly_the_limit_stays_whole() {
        let text = "a".repeat(10);
        assert_eq!(split_message(&text, 10), vec![text]);
    }

    #[test]
    fn pieces_prefer_whitespace() {
        let pieces = split_message("один два три четыре", 10);
        assert_eq!(pieces, vec!["один два", "три четыре"]);
        assert!(pieces.iter().all(|p| p.chars().count() <= 10), "{pieces:?}");
    }

    #[test]
    fn a_word_longer_than_the_limit_is_cut() {
        let pieces = split_message("abcdefghij", 4);
        assert_eq!(pieces, vec!["abcd", "efgh", "ij"]);
    }

    #[tokio::test]
    async fn a_send_waits_for_the_window_to_free_a_slot() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::ZERO);
        // One message per 300 ms: short enough for a test, the same code path.
        sink.sends = SendWindow::with_window(1, Duration::from_millis(300));

        let started = Instant::now();
        sink.on_event(&translated("первая", true)).await.unwrap();
        sink.on_event(&translated("вторая", true)).await.unwrap();
        let elapsed = started.elapsed();

        assert_eq!(methods(&seen).len(), 2, "оба сообщения всё-таки ушли");
        assert!(
            elapsed >= Duration::from_millis(250),
            "второе сообщение ждало окно: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn a_phrase_starts_at_once_and_grows_by_edits() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::ZERO);

        sink.on_event(&translated("При", false)).await.unwrap();
        sink.on_event(&translated("Привет", false)).await.unwrap();
        sink.on_event(&translated("Привет, мир.", true))
            .await
            .unwrap();

        assert_eq!(
            methods(&seen),
            vec!["sendMessage", "editMessageText", "editMessageText"],
            "фраза заводит одно сообщение и дальше только дописывается"
        );
        assert_eq!(
            texts(&seen),
            vec!["При", "Привет", "Привет, мир."],
            "текст сообщения растёт вместе с фразой"
        );
        assert_eq!(sink.sent(), 1, "новое сообщение на фразу одно");
        assert_eq!(sink.edited(), 2, "каждая следующая ревизия правит его");
    }

    #[tokio::test]
    async fn every_phrase_gets_its_own_message() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::ZERO);

        sink.on_event(&translated("Первая", false)).await.unwrap();
        sink.on_event(&translated("Первая фраза.", true))
            .await
            .unwrap();
        sink.on_event(&translated("Вторая", false)).await.unwrap();
        sink.on_event(&translated("Вторая фраза.", true))
            .await
            .unwrap();

        assert_eq!(
            methods(&seen),
            vec![
                "sendMessage",
                "editMessageText",
                "sendMessage",
                "editMessageText"
            ],
            "{:?}",
            texts(&seen)
        );
        assert_eq!(sink.sent(), 2);
    }

    #[tokio::test]
    async fn edits_are_throttled_while_the_last_revision_is_always_applied() {
        let (base, seen) = stand_in(ok).await;
        // An hour between edits: everything but the final revision is skipped.
        let mut sink = sink(&base, Duration::from_secs(3600));

        sink.on_event(&translated("раз", false)).await.unwrap();
        sink.on_event(&translated("раз два", false)).await.unwrap();
        sink.on_event(&translated("раз два три", false))
            .await
            .unwrap();
        sink.on_event(&translated("раз два три!", true))
            .await
            .unwrap();

        assert_eq!(
            methods(&seen),
            vec!["sendMessage", "editMessageText"],
            "промежуточные ревизии не тратят лимит"
        );
        assert_eq!(
            texts(&seen),
            vec!["раз", "раз два три!"],
            "в конце сообщение получает всю фразу"
        );
    }

    #[tokio::test]
    async fn bilingual_puts_the_source_above_the_translation() {
        let (base, seen) = stand_in(ok).await;
        let mut settings = TelegramSettings::new("-100", TEST_TOKEN);
        settings.base = base.clone();
        settings.bilingual = true;
        let mut sink = TelegramSink::new("telegram", settings).unwrap();

        sink.on_event(&source_text("hello world")).await.unwrap();
        sink.on_event(&translated("привет мир", true))
            .await
            .unwrap();

        assert_eq!(texts(&seen), vec!["hello world\nпривет мир"]);
    }

    #[tokio::test]
    async fn a_phrase_longer_than_one_message_spills_into_the_next() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink(&base, Duration::ZERO);

        let text = "слово ".repeat(MAX_MESSAGE_CHARS / 3);
        sink.on_event(&translated(&text, true)).await.unwrap();

        let bodies = texts(&seen);
        assert!(
            bodies.len() > 1,
            "длинная фраза не влезла в одно: {}",
            bodies.len()
        );
        for piece in &bodies {
            assert!(
                piece.chars().count() <= MAX_MESSAGE_CHARS,
                "кусок длиннее предела: {}",
                piece.chars().count()
            );
        }
    }

    #[tokio::test]
    async fn an_unchanged_edit_is_not_a_failure() {
        let (base, _seen) = stand_in(not_modified).await;
        let mut sink = sink(&base, Duration::ZERO);

        // Telegram answers "message is not modified" when the text is already
        // there; for the sink that is a delivered phrase.
        sink.on_event(&translated("при", false)).await.unwrap();
        sink.on_event(&translated("привет", true)).await.unwrap();
    }

    #[tokio::test]
    async fn a_refusal_is_reported_with_the_reason_and_without_the_token() {
        let (base, _seen) = stand_in(chat_not_found).await;
        let mut sink = sink(&base, Duration::ZERO);

        let err = sink
            .on_event(&translated("привет", false))
            .await
            .expect_err("отказ Telegram — ошибка приёмника");

        assert!(matches!(err, SinkError::Unavailable(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("chat not found"), "причина сохранена: {text}");
        assert!(
            !text.contains(TEST_TOKEN),
            "токен не должен попасть в лог: {text}"
        );
    }

    #[test]
    fn an_empty_chat_is_rejected() {
        let err = TelegramSink::new("t", TelegramSettings::new("  ", TEST_TOKEN)).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn a_missing_variable_is_named() {
        let err = TelegramSink::from_spec(
            "t",
            TelegramSpec {
                chat_id: "-100",
                token_env: "AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN",
                bilingual: false,
                silent: false,
                thread: None,
                max_sends_per_minute: DEFAULT_SENDS_PER_MINUTE,
                edit_interval: DEFAULT_EDIT_INTERVAL,
            },
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN"),
            "{err}"
        );
    }
}
