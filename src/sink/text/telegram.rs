//! Telegram sink: posting the translation into chats through a bot.
//!
//! The translation is synchronous, so the chat shows it as it appears: the first
//! revision of a phrase is posted at once, and every following revision edits
//! that same message instead of posting a new one. When the phrase ends, the
//! message stays as the finished sentence and the next phrase starts a new one.
//!
//! Messages go out as plain text, without a parse mode: a translation is not
//! markup, so nothing in it may be interpreted as formatting and nothing needs
//! escaping. Telegram caps one message at 4096 characters, so a phrase that
//! grows longer than that spills into the next message.
//!
//! Edits are throttled: Telegram counts them as messages and limits the rate, so
//! a message is edited at most once per [`DEFAULT_EDIT_INTERVAL`], except for the
//! revision that ends the phrase — that one is always applied. A skipped
//! revision costs nothing: the accumulated text of the phrase arrives with the
//! next event, and the final revision carries the whole sentence.
//!
//! # Who receives the translation
//!
//! In [`Mode::Single`] everything goes to the one chat named in the config. In
//! [`Mode::Broadcast`] the sink serves everyone who can receive it: a person who
//! opened a private chat with the bot and pressed Start, and any group the bot
//! was added to. Telegram has no method that lists the chats of a bot, so the
//! list is built from incoming updates — [`getUpdates`] is polled in the
//! background:
//!
//! * `message` in a private chat — the person wrote to the bot;
//! * `message` in a group or channel — the bot is there and sees the message;
//! * `my_chat_member` — the bot was added to a group, or removed from it. This
//!   update arrives even with privacy mode on, so adding the bot is enough.
//!
//! The list lives in a state file next to the update offset: a restarted process
//! keeps its subscribers and does not replay the updates it already handled.
//! `/start` adds the private chat it came from, `/stop` removes it; in groups the
//! list is changed only by adding or removing the bot itself, so no member can
//! unsubscribe the whole group.
//!
//! Telegram does not let a bot write first: a person has to open the chat and
//! press Start. That is a platform rule, not a limitation of the sink.
//!
//! The bot token is read once, when the sink is built: a missing variable is
//! reported at start-up instead of failing in the middle of a broadcast. The
//! token is part of the request URL, so every error message passes through
//! [`reqwest::Error::without_url`] and the token never reaches a log line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bus::TranslationEvent;
use crate::sink::{Result, SinkError, TextSink};

/// Bot API root.
pub const API_BASE: &str = "https://api.telegram.org";

/// Longest text Telegram accepts in one message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 4096;

/// Shortest time between two edits of one message.
pub const DEFAULT_EDIT_INTERVAL: Duration = Duration::from_millis(1200);

/// How long `getUpdates` waits for an update on the server side.
const POLL_TIMEOUT_SECS: u64 = 20;

/// Pause after a failed poll: the network must not be hammered.
const POLL_RETRY_DELAY: Duration = Duration::from_secs(5);

/// How long a rate limit is waited out before the sink gives up.
const RETRY_AFTER_LIMIT_SECS: u64 = 30;

/// Request timeout, longer than a poll: a stuck request must not hold the sink.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Reply Telegram gives when an edit would not change anything.
const NOT_MODIFIED: &str = "message is not modified";

/// Replies that mean the chat is gone for good: blocked, kicked, deleted.
const GONE_REPLIES: [&str; 6] = [
    "bot was blocked by the user",
    "bot was kicked",
    "chat not found",
    "user is deactivated",
    "PEER_ID_INVALID",
    "bot is not a member",
];

/// Where the translation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// One chat, named in the configuration.
    #[default]
    Single,
    /// Everyone who wrote to the bot, plus every group it was added to.
    Broadcast,
}

/// Kind of a chat, as far as the sink cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatKind {
    /// A person's private chat with the bot.
    Private,
    /// A group or a supergroup.
    Group,
    /// A channel the bot posts to.
    Channel,
}

impl ChatKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "private" => Some(Self::Private),
            "group" | "supergroup" => Some(Self::Group),
            "channel" => Some(Self::Channel),
            _ => None,
        }
    }

    /// Whether `/start` and `/stop` may change this chat.
    fn obeys_commands(self) -> bool {
        self == Self::Private
    }
}

/// A chat that receives the translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscriber {
    /// Chat identifier.
    pub id: i64,
    /// Title for logs: a group name, or a person's name.
    #[serde(default)]
    pub title: String,
    /// Kind of the chat.
    pub kind: ChatKind,
    /// Forum topic, when the configured chat is a topic of a forum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<i64>,
}

/// The subscribers and the position in the update stream, kept on disk.
///
/// Telegram cannot list the chats of a bot, so the list is the only memory of
/// who receives the translation: losing it means everyone has to write again.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// Last handled update; the next poll asks for everything after it.
    #[serde(default)]
    offset: i64,
    /// Chats in the order they subscribed.
    #[serde(default)]
    chats: Vec<Subscriber>,
}

/// The state file and the list in it.
struct Registry {
    path: PathBuf,
    state: State,
}

impl Registry {
    /// Read the state file; a missing file is an empty list, a broken one is an
    /// error, because silently dropping every subscriber is worse than not
    /// starting.
    fn load(path: &Path) -> Result<Self> {
        let state = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| SinkError::Protocol(format!("не читается {}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => {
                return Err(SinkError::Protocol(format!(
                    "не открывается {}: {e}",
                    path.display()
                )));
            }
        };

        Ok(Self {
            path: path.to_path_buf(),
            state,
        })
    }

    /// Write the state through a temporary file, so a crash mid-write leaves the
    /// previous list intact.
    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir).map_err(|e| {
                SinkError::Unavailable(format!("не создаётся {}: {e}", dir.display()))
            })?;
        }

        let body = serde_json::to_string_pretty(&self.state)
            .map_err(|e| SinkError::Protocol(format!("состояние не сериализуется: {e}")))?;

        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, body)
            .map_err(|e| SinkError::Unavailable(format!("не пишется {}: {e}", temp.display())))?;
        std::fs::rename(&temp, &self.path).map_err(|e| {
            SinkError::Unavailable(format!("не заменяется {}: {e}", self.path.display()))
        })
    }

    fn chats(&self) -> &[Subscriber] {
        &self.state.chats
    }

    fn offset(&self) -> i64 {
        self.state.offset
    }

    /// Add a chat, or refresh the title of one that is already there.
    fn add(&mut self, subscriber: Subscriber) -> bool {
        if let Some(existing) = self
            .state
            .chats
            .iter_mut()
            .find(|chat| chat.id == subscriber.id)
        {
            let changed = existing.title != subscriber.title;
            existing.title = subscriber.title;
            return changed;
        }

        self.state.chats.push(subscriber);
        true
    }

    /// Drop a chat; returns whether it was there.
    fn remove(&mut self, id: i64) -> bool {
        let before = self.state.chats.len();
        self.state.chats.retain(|chat| chat.id != id);
        before != self.state.chats.len()
    }

    fn advance(&mut self, offset: i64) {
        self.state.offset = offset;
    }
}

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
    /// Where the translation goes.
    pub mode: Mode,
    /// The chat of [`Mode::Single`], or one more recipient in [`Mode::Broadcast`].
    pub chat_id: Option<String>,
    /// Bot token.
    pub token: String,
    /// Bot API root: `https://api.telegram.org` unless a compatible server is used.
    pub base: String,
    /// State file with the subscriber list.
    pub state_file: PathBuf,
    /// Post the source text above the translation.
    pub bilingual: bool,
    /// Send without a notification sound.
    pub silent: bool,
    /// Forum topic of the configured chat.
    pub thread: Option<i64>,
    /// Shortest time between two edits of one message.
    pub edit_interval: Duration,
}

impl TelegramSettings {
    /// Parameters with the default Bot API root and no optional behaviour.
    pub fn new(chat_id: &str, token: &str) -> Self {
        Self {
            mode: Mode::Single,
            chat_id: Some(chat_id.to_string()),
            token: token.to_string(),
            base: API_BASE.to_string(),
            state_file: default_state_file(),
            bilingual: false,
            silent: false,
            thread: None,
            edit_interval: DEFAULT_EDIT_INTERVAL,
        }
    }
}

impl std::fmt::Debug for TelegramSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token must not reach a log line.
        f.debug_struct("TelegramSettings")
            .field("mode", &self.mode)
            .field("chat_id", &self.chat_id)
            .field("base", &self.base)
            .field("state_file", &self.state_file)
            .field("bilingual", &self.bilingual)
            .field("silent", &self.silent)
            .field("thread", &self.thread)
            .field("edit_interval", &self.edit_interval)
            .finish_non_exhaustive()
    }
}

/// Everything the sink takes from the configuration.
pub struct TelegramSpec<'a> {
    /// Chat of a single-chat sink, or one more recipient in broadcast mode.
    pub chat_id: Option<&'a str>,
    /// Variable holding the bot token.
    pub token_env: &'a str,
    /// State file with the subscriber list.
    pub state_file: &'a str,
    /// Serve every chat that writes to the bot.
    pub broadcast: bool,
    /// Post the source text above the translation.
    pub bilingual: bool,
    /// Send without a notification sound.
    pub silent: bool,
    /// Forum topic of the configured chat.
    pub thread: Option<i64>,
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

/// Posts the translation into Telegram chats, growing one message per phrase.
pub struct TelegramSink {
    id: String,
    settings: TelegramSettings,
    client: reqwest::Client,
    /// Subscriber list shared with the poller.
    registry: Arc<Mutex<Registry>>,
    /// Fixed recipient of [`Mode::Single`]: never dropped from the list.
    fixed: Option<i64>,
    /// The message of the running phrase, per chat.
    live: BTreeMap<i64, LiveMessage>,
    last_source: String,
    sent: u64,
    edited: u64,
    poller: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for TelegramSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramSink")
            .field("id", &self.id)
            .field("settings", &self.settings)
            .field("fixed", &self.fixed)
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
    /// Returns an error when the token is empty, when a single-chat sink has no
    /// chat, when the state file cannot be read, or when the HTTP client cannot
    /// be built.
    pub fn new(id: &str, settings: TelegramSettings) -> Result<Self> {
        if settings.token.trim().is_empty() {
            return Err(SinkError::Protocol("пустой токен бота Telegram".into()));
        }

        let chat_id = settings
            .chat_id
            .as_deref()
            .map(str::trim)
            .filter(|chat| !chat.is_empty());

        if settings.mode == Mode::Single && chat_id.is_none() {
            return Err(SinkError::Protocol(
                "пустой идентификатор чата Telegram".into(),
            ));
        }

        let mut registry = Registry::load(&settings.state_file)?;

        // The configured chat is a recipient in both modes: in a broadcast it is
        // one more address, kept even when Telegram says it is gone.
        let fixed = match chat_id {
            Some(chat) => {
                let id = chat_id_number(chat);
                if let Some(id) = id {
                    registry.add(Subscriber {
                        id,
                        title: "из конфигурации".into(),
                        kind: ChatKind::Private,
                        thread: settings.thread,
                    });
                }
                id
            }
            None => None,
        };

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| SinkError::Unavailable(format!("не удалось создать HTTP-клиент: {e}")))?;

        Ok(Self {
            id: id.to_string(),
            settings,
            client,
            registry: Arc::new(Mutex::new(registry)),
            fixed,
            live: BTreeMap::new(),
            last_source: String::new(),
            sent: 0,
            edited: 0,
            poller: None,
        })
    }

    /// Create a sink from the configuration, reading the bot token from
    /// `spec.token_env`.
    ///
    /// # Errors
    ///
    /// Returns an error when the variable name is empty, when the variable is
    /// not set, or when the sink itself cannot be created.
    pub fn from_spec(id: &str, spec: TelegramSpec<'_>) -> Result<Self> {
        if spec.token_env.trim().is_empty() {
            return Err(SinkError::Protocol(
                "пустое имя переменной с токеном бота".into(),
            ));
        }

        let token = std::env::var(spec.token_env).map_err(|_| {
            SinkError::Protocol(format!("не задана переменная окружения {}", spec.token_env))
        })?;

        let mode = if spec.broadcast {
            Mode::Broadcast
        } else {
            Mode::Single
        };

        let settings = TelegramSettings {
            mode,
            chat_id: spec.chat_id.map(str::to_string),
            token,
            base: API_BASE.to_string(),
            state_file: expand_home(spec.state_file),
            bilingual: spec.bilingual,
            silent: spec.silent,
            thread: spec.thread,
            edit_interval: DEFAULT_EDIT_INTERVAL,
        };

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

    /// Chats the translation goes to right now.
    pub fn subscribers(&self) -> Vec<i64> {
        self.registry
            .lock()
            .map(|registry| registry.chats().iter().map(|chat| chat.id).collect())
            .unwrap_or_default()
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

    /// Start the background poll that learns about new chats.
    fn start_poller(&mut self) {
        if self.settings.mode != Mode::Broadcast || self.poller.is_some() {
            return;
        }

        let client = self.client.clone();
        let token = self.settings.token.clone();
        let base = self.settings.base.clone();
        let registry = Arc::clone(&self.registry);

        self.poller = Some(tokio::spawn(async move {
            poll_loop(client, base, token, registry).await;
        }));
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
    async fn send(&self, chat: i64, thread: Option<i64>, text: &str) -> Result<i64> {
        let mut body = json!({
            "chat_id": chat,
            "text": text,
            "disable_notification": self.settings.silent,
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(thread) = thread {
            body["message_thread_id"] = json!(thread);
        }

        let result = self.call("sendMessage", body).await?;
        result["message_id"]
            .as_i64()
            .ok_or_else(|| SinkError::Protocol("Telegram не вернул номер сообщения".into()))
    }

    /// Replace the text of a message.
    async fn edit(
        &self,
        chat: i64,
        thread: Option<i64>,
        message_id: i64,
        text: &str,
    ) -> Result<()> {
        let mut body = json!({
            "chat_id": chat,
            "message_id": message_id,
            "text": text,
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(thread) = thread {
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

    /// Remember that a chat is gone, unless it is the configured one.
    fn forget(&self, chat: i64) {
        if Some(chat) == self.fixed {
            return;
        }

        if let Ok(mut registry) = self.registry.lock()
            && registry.remove(chat)
        {
            if let Err(e) = registry.save() {
                tracing::warn!("список получателей не сохранён: {e}");
            }
            tracing::info!("чат Telegram больше не получает перевод: {chat}");
        }
    }

    /// Put the text of the phrase into one chat, editing the message in place.
    async fn apply_to(&mut self, chat: i64, thread: Option<i64>, text: &str) -> Result<()> {
        let pieces = split_message(text, MAX_MESSAGE_CHARS);
        let Some((first, rest)) = pieces.split_first() else {
            return Ok(());
        };

        match self.live.remove(&chat) {
            // The phrase is new for this chat: the first message starts it.
            None => {
                let id = self.send(chat, thread, first).await?;
                self.sent += 1;
                self.live.insert(
                    chat,
                    LiveMessage {
                        id,
                        text: first.clone(),
                        edited_at: Instant::now(),
                    },
                );
            }

            // The phrase is running: its message grows in place.
            Some(mut live) => {
                if live.text != *first {
                    self.edit(chat, thread, live.id, first).await?;
                    self.edited += 1;
                }
                live.text.clone_from(first);
                live.edited_at = Instant::now();
                self.live.insert(chat, live);
            }
        }

        // A phrase longer than one message spills into the next ones; the last
        // of them takes the following revisions.
        for piece in rest {
            let id = self.send(chat, thread, piece).await?;
            self.sent += 1;
            self.live.insert(
                chat,
                LiveMessage {
                    id,
                    text: piece.clone(),
                    edited_at: Instant::now(),
                },
            );
        }

        Ok(())
    }

    /// Whether this chat is due for an update of the running phrase.
    fn due(&self, chat: i64, is_final: bool) -> bool {
        if is_final {
            return true;
        }

        match self.live.get(&chat) {
            None => true,
            Some(live) => live.edited_at.elapsed() >= self.settings.edit_interval,
        }
    }

    /// Deliver the text of a revision to every chat that receives the translation.
    async fn broadcast(&mut self, text: &str, is_final: bool) -> Result<()> {
        let recipients: Vec<Subscriber> = match self.registry.lock() {
            Ok(registry) => registry.chats().to_vec(),
            Err(_) => return Err(SinkError::Unavailable("список чатов недоступен".into())),
        };

        let mut failures = Vec::new();

        let mut attempted = 0usize;

        for subscriber in recipients {
            if !self.due(subscriber.id, is_final) {
                continue;
            }

            attempted += 1;
            match self.apply_to(subscriber.id, subscriber.thread, text).await {
                Ok(()) => {}
                Err(e) => {
                    let reason = e.to_string();
                    if GONE_REPLIES.iter().any(|gone| reason.contains(*gone)) {
                        self.forget(subscriber.id);
                    } else {
                        tracing::warn!("чат {} не получил перевод: {reason}", subscriber.id);
                    }
                    failures.push(e);
                }
            }
        }

        // Nobody received anything: that is a sink failure the supervisor must
        // see. A single silent chat among working ones is only a warning.
        if attempted > 0 && failures.len() == attempted {
            return Err(failures.remove(0));
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

                self.start_poller();

                let message = self.message_text(text);
                if message.is_empty() {
                    return Ok(());
                }

                self.broadcast(&message, *is_final).await?;

                if *is_final {
                    // The finished phrase stays as the last message; the next
                    // phrase starts a new one.
                    self.live.clear();
                }
            }

            TranslationEvent::Status { .. } | TranslationEvent::TranslatedAudio { .. } => {}
        }

        Ok(())
    }

    async fn shutdown(mut self: Box<Self>) -> Result<()> {
        if let Some(poller) = self.poller.take() {
            poller.abort();
        }
        Ok(())
    }
}

/// Default state file: under the home directory, next to the rest of the state.
fn default_state_file() -> PathBuf {
    expand_home("~/.local/state/ai-translator/telegram-chats.json")
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let path = path.trim();

    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }

    if path == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }

    PathBuf::from(path)
}

/// Numeric chat identifier, when the configuration holds one instead of `@name`.
fn chat_id_number(raw: &str) -> Option<i64> {
    raw.trim().parse().ok()
}

/// A reply the poller owes to a chat.
struct Reply {
    chat: i64,
    text: &'static str,
}

/// Read the updates into the subscriber list.
///
/// Only two things matter here: who may receive the translation, and what the
/// bot answers to `/start` and `/stop`. Everything else is ignored.
fn absorb(updates: &[Value], registry: &mut Registry) -> Vec<Reply> {
    let mut replies = Vec::new();
    let mut changed = false;
    let mut high_water = registry.offset();

    for update in updates {
        if let Some(id) = update["update_id"].as_i64()
            && id >= high_water
        {
            high_water = id + 1;
        }

        // The bot was added to a group or removed from it. This update arrives
        // even when privacy mode hides ordinary messages.
        if let Some(member) = update.get("my_chat_member") {
            let status = member["new_chat_member"]["status"].as_str().unwrap_or("");
            if let Some(subscriber) = subscriber_of(&member["chat"]) {
                let present = matches!(status, "member" | "administrator" | "creator");
                let left = matches!(status, "left" | "kicked");

                if present {
                    if registry.add(subscriber) {
                        changed = true;
                    }
                } else if left && registry.remove(subscriber.id) {
                    changed = true;
                }
            }
            continue;
        }

        // A message: in a private chat it is the person writing to the bot, in a
        // group it proves the bot is there.
        let message = match update.get("message").or_else(|| update.get("channel_post")) {
            Some(message) => message,
            None => continue,
        };

        let Some(subscriber) = subscriber_of(&message["chat"]) else {
            continue;
        };

        let text = message["text"].as_str().unwrap_or("");
        let command = command_of(text);

        // Commands are honoured in private chats only: in a group any member
        // could otherwise switch the translation off for everyone.
        if subscriber.kind.obeys_commands() {
            match command {
                Some("/start") => {
                    if registry.add(subscriber.clone()) {
                        changed = true;
                    }
                    replies.push(Reply {
                        chat: subscriber.id,
                        text: "Перевод включён для этого чата.",
                    });
                    continue;
                }
                Some("/stop") => {
                    if registry.remove(subscriber.id) {
                        changed = true;
                    }
                    replies.push(Reply {
                        chat: subscriber.id,
                        text: "Перевод выключен для этого чата.",
                    });
                    continue;
                }
                _ => {}
            }
        }

        if registry.add(subscriber) {
            changed = true;
        }
    }

    if high_water > registry.offset() {
        registry.advance(high_water);
        changed = true;
    }

    if changed && let Err(e) = registry.save() {
        tracing::warn!("список получателей не сохранён: {e}");
    }

    replies
}

/// Turn a chat object of an update into a subscriber.
fn subscriber_of(chat: &Value) -> Option<Subscriber> {
    let id = chat["id"].as_i64()?;
    let kind = ChatKind::parse(chat["type"].as_str()?)?;

    let title = chat["title"]
        .as_str()
        .or_else(|| chat["username"].as_str())
        .or_else(|| chat["first_name"].as_str())
        .unwrap_or("без названия")
        .to_string();

    Some(Subscriber {
        id,
        title,
        kind,
        thread: None,
    })
}

/// The command in a message, without the bot name suffix: `/stop@my_bot`.
fn command_of(text: &str) -> Option<&str> {
    let text = text.trim();
    if !text.starts_with('/') {
        return None;
    }

    let command = text.split_whitespace().next()?;
    let command = command.split('@').next()?;

    Some(command)
}

/// Poll `getUpdates` until the task is aborted.
async fn poll_loop(
    client: reqwest::Client,
    base: String,
    token: String,
    registry: Arc<Mutex<Registry>>,
) {
    let url = format!("{}/bot{}/getUpdates", base.trim_end_matches('/'), token);

    loop {
        let offset = match registry.lock() {
            Ok(registry) => registry.offset(),
            Err(_) => return,
        };

        let body = json!({
            "offset": offset,
            "timeout": POLL_TIMEOUT_SECS,
            "allowed_updates": ["message", "my_chat_member", "channel_post"],
        });

        let response = client.post(&url).json(&body).send().await;

        let payload: Value = match response {
            Ok(response) => match response.json().await {
                Ok(payload) => payload,
                Err(e) => {
                    tracing::warn!("ответ Telegram не разобран: {}", e.without_url());
                    tokio::time::sleep(POLL_RETRY_DELAY).await;
                    continue;
                }
            },
            Err(e) => {
                tracing::warn!("опрос Telegram не удался: {}", e.without_url());
                tokio::time::sleep(POLL_RETRY_DELAY).await;
                continue;
            }
        };

        if payload["ok"] != json!(true) {
            let description = payload["description"].as_str().unwrap_or("без описания");
            tracing::warn!("Telegram отказал в опросе: {description}");
            tokio::time::sleep(POLL_RETRY_DELAY).await;
            continue;
        }

        let updates = payload["result"].as_array().cloned().unwrap_or_default();

        let replies = match registry.lock() {
            Ok(mut registry) => absorb(&updates, &mut registry),
            Err(_) => return,
        };

        for reply in replies {
            let body = json!({ "chat_id": reply.chat, "text": reply.text });
            let url = format!("{}/bot{}/sendMessage", base.trim_end_matches('/'), token);
            if let Err(e) = client.post(&url).json(&body).send().await {
                tracing::warn!("ответ на команду не отправлен: {}", e.without_url());
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    type Captured = Arc<Mutex<Vec<Seen>>>;

    /// Bot API stand-in: every method answers through `reply`.
    async fn stand_in(reply: fn(&str, &Value) -> (u16, Value)) -> (String, Captured) {
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));

        let handler = |method: &'static str, captured: Captured| {
            move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(Seen {
                        method: method.into(),
                        body: body.clone(),
                    });
                    let (code, payload) = reply(method, &body);
                    (
                        axum::http::StatusCode::from_u16(code).unwrap(),
                        Json(payload),
                    )
                }
            }
        };

        let app = Router::new()
            .route(
                "/bot123456:TEST-TOKEN/sendMessage",
                post(handler("sendMessage", captured.clone())),
            )
            .route(
                "/bot123456:TEST-TOKEN/editMessageText",
                post(handler("editMessageText", captured.clone())),
            )
            .route(
                "/bot123456:TEST-TOKEN/getUpdates",
                post(handler("getUpdates", captured.clone())),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        (format!("http://{addr}"), captured)
    }

    /// Normal Bot API answer: a message is created or edited, no updates pending.
    fn ok(method: &str, _body: &Value) -> (u16, Value) {
        let result = match method {
            "sendMessage" => json!({ "message_id": 77 }),
            "editMessageText" => json!({ "message_id": 77, "text": "ok" }),
            _ => json!([]),
        };
        (200, json!({ "ok": true, "result": result }))
    }

    fn not_modified(method: &str, body: &Value) -> (u16, Value) {
        if method != "editMessageText" {
            return ok(method, body);
        }
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: message is not modified"}),
        )
    }

    fn chat_not_found(_method: &str, _body: &Value) -> (u16, Value) {
        (
            400,
            json!({"ok": false, "error_code": 400, "description": "Bad Request: chat not found"}),
        )
    }

    /// A chat that refuses everything except one identifier.
    fn only_private(method: &str, body: &Value) -> (u16, Value) {
        if body["chat_id"].as_i64() == Some(555) {
            return ok(method, body);
        }
        (
            403,
            json!({"ok": false, "error_code": 403, "description": "Forbidden: bot was blocked by the user"}),
        )
    }

    fn state_path(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Every sink gets its own file: tests run in parallel and a shared list
        // would leak subscribers from one test into another.
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let dir = std::env::temp_dir().join("ai-translator-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{name}-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn sink_at(base: &str, mode: Mode, chat: Option<&str>, interval: Duration) -> TelegramSink {
        let mut settings = TelegramSettings::new(chat.unwrap_or("-1001234567890"), TEST_TOKEN);
        settings.mode = mode;
        settings.chat_id = chat.map(str::to_string);
        settings.base = base.to_string();
        settings.state_file = state_path("sink");
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

    fn methods(seen: &Captured) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .map(|s| s.method.clone())
            .collect()
    }

    fn texts(seen: &Captured) -> Vec<String> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|s| s.method != "getUpdates")
            .map(|s| s.body["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn chats_reached(seen: &Captured) -> Vec<i64> {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|s| s.method != "getUpdates")
            .filter_map(|s| s.body["chat_id"].as_i64())
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
    async fn a_phrase_starts_at_once_and_grows_by_edits() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);

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
        assert_eq!(sink.sent(), 1);
        assert_eq!(sink.edited(), 2);
    }

    #[tokio::test]
    async fn every_phrase_gets_its_own_message() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);

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
            ]
        );
        assert_eq!(sink.sent(), 2);
    }

    #[tokio::test]
    async fn edits_are_throttled_while_the_last_revision_is_always_applied() {
        let (base, seen) = stand_in(ok).await;
        // An hour between edits: everything but the final revision is skipped.
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::from_secs(3600));

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
        assert_eq!(texts(&seen), vec!["раз", "раз два три!"]);
    }

    #[tokio::test]
    async fn bilingual_puts_the_source_above_the_translation() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);
        sink.settings.bilingual = true;

        sink.on_event(&source_text("hello world")).await.unwrap();
        sink.on_event(&translated("привет мир", true))
            .await
            .unwrap();

        assert_eq!(texts(&seen), vec!["hello world\nпривет мир"]);
    }

    #[tokio::test]
    async fn a_phrase_longer_than_one_message_spills_into_the_next() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);

        let text = "слово ".repeat(MAX_MESSAGE_CHARS / 3);
        sink.on_event(&translated(&text, true)).await.unwrap();

        let bodies = texts(&seen);
        assert!(bodies.len() > 1, "длинная фраза не влезла в одно");
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
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);

        sink.on_event(&translated("при", false)).await.unwrap();
        sink.on_event(&translated("привет", true)).await.unwrap();
    }

    #[tokio::test]
    async fn a_refusal_is_reported_with_the_reason_and_without_the_token() {
        let (base, _seen) = stand_in(chat_not_found).await;
        let mut sink = sink_at(&base, Mode::Single, Some("-100"), Duration::ZERO);

        let err = sink
            .on_event(&translated("привет", false))
            .await
            .expect_err("отказ Telegram — ошибка приёмника");

        let text = err.to_string();
        assert!(text.contains("chat not found"), "причина сохранена: {text}");
        assert!(
            !text.contains(TEST_TOKEN),
            "токен не должен попасть в лог: {text}"
        );
    }

    #[tokio::test]
    async fn a_broadcast_reaches_every_subscriber() {
        let (base, seen) = stand_in(ok).await;
        let mut sink = sink_at(&base, Mode::Broadcast, None, Duration::ZERO);

        // Two people wrote to the bot, one group has it.
        {
            let mut registry = sink.registry.lock().unwrap();
            registry.add(Subscriber {
                id: 111,
                title: "Анна".into(),
                kind: ChatKind::Private,
                thread: None,
            });
            registry.add(Subscriber {
                id: -222,
                title: "Рабочая группа".into(),
                kind: ChatKind::Group,
                thread: None,
            });
        }

        sink.on_event(&translated("привет", false)).await.unwrap();
        sink.on_event(&translated("привет всем", true))
            .await
            .unwrap();

        let reached = chats_reached(&seen);
        assert!(reached.contains(&111), "личный чат: {reached:?}");
        assert!(reached.contains(&-222), "группа: {reached:?}");
        assert_eq!(sink.sent(), 2, "по сообщению в каждый чат");
        assert_eq!(sink.edited(), 2, "и по правке в каждый");
    }

    #[tokio::test]
    async fn a_chat_that_started_refusing_is_dropped_the_rest_still_get_the_text() {
        let (base, seen) = stand_in(only_private).await;
        let mut sink = sink_at(&base, Mode::Broadcast, None, Duration::ZERO);

        {
            let mut registry = sink.registry.lock().unwrap();
            registry.add(Subscriber {
                id: 555,
                title: "Свой".into(),
                kind: ChatKind::Private,
                thread: None,
            });
            registry.add(Subscriber {
                id: 666,
                title: "Заблокировал".into(),
                kind: ChatKind::Private,
                thread: None,
            });
        }

        // The blocked chat must not stop the delivery to the other one.
        sink.on_event(&translated("привет", false)).await.unwrap();

        assert!(
            chats_reached(&seen).contains(&555),
            "{:?}",
            chats_reached(&seen)
        );
        assert_eq!(
            sink.subscribers(),
            vec![555],
            "заблокировавший бота чат снят со списка"
        );
    }

    #[tokio::test]
    async fn the_poller_learns_a_chat_that_wrote_to_the_bot() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let polls = Arc::new(AtomicUsize::new(0));

        let handler = |method: &'static str, captured: Captured, polls: Arc<AtomicUsize>| {
            move |Json(body): Json<Value>| {
                let captured = captured.clone();
                let polls = polls.clone();
                async move {
                    captured.lock().unwrap().push(Seen {
                        method: method.into(),
                        body: body.clone(),
                    });

                    let (code, payload) = if method == "getUpdates" {
                        // The person wrote to the bot once; the next poll is empty.
                        let first = polls.fetch_add(1, Ordering::SeqCst) == 0;
                        let result = if first {
                            json!([{
                                "update_id": 1,
                                "message": {
                                    "chat": { "id": 111, "type": "private", "first_name": "Анна" },
                                    "text": "/start"
                                }
                            }])
                        } else {
                            json!([])
                        };
                        (200, json!({ "ok": true, "result": result }))
                    } else if method == "sendMessage" {
                        (200, json!({ "ok": true, "result": { "message_id": 5 } }))
                    } else {
                        (200, json!({ "ok": true, "result": true }))
                    };

                    (
                        axum::http::StatusCode::from_u16(code).unwrap(),
                        Json(payload),
                    )
                }
            }
        };

        let app = Router::new()
            .route(
                "/bot123456:TEST-TOKEN/getUpdates",
                post(handler("getUpdates", captured.clone(), polls.clone())),
            )
            .route(
                "/bot123456:TEST-TOKEN/sendMessage",
                post(handler("sendMessage", captured.clone(), polls.clone())),
            )
            .route(
                "/bot123456:TEST-TOKEN/editMessageText",
                post(handler("editMessageText", captured.clone(), polls)),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let mut sink = sink_at(
            &format!("http://{addr}"),
            Mode::Broadcast,
            None,
            Duration::ZERO,
        );

        // The first translation starts the poller.
        sink.on_event(&translated("привет", true)).await.unwrap();

        for _ in 0..100 {
            if sink.subscribers() == vec![111] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            sink.subscribers(),
            vec![111],
            "человек, написавший боту, попал в список"
        );

        let replies: Vec<String> = captured
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == "sendMessage")
            .map(|s| s.body["text"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            replies.iter().any(|text| text.contains("включён")),
            "бот ответил на /start: {replies:?}"
        );
    }

    fn update(id: i64, key: &str, chat: Value, extra: Value) -> Value {
        let mut value = json!({ "update_id": id });
        value[key] = json!({ "chat": chat });
        if let Some(extra) = extra.as_object() {
            for (name, field) in extra {
                value[key][name] = field.clone();
            }
        }
        value
    }

    fn private_chat(id: i64, name: &str) -> Value {
        json!({ "id": id, "type": "private", "first_name": name })
    }

    fn group_chat(id: i64, title: &str) -> Value {
        json!({ "id": id, "type": "supergroup", "title": title })
    }

    #[test]
    fn a_person_who_wrote_to_the_bot_subscribes() {
        let path = state_path("absorb-person");
        let mut registry = Registry::load(&path).unwrap();

        let updates = vec![update(
            10,
            "message",
            private_chat(111, "Анна"),
            json!({ "text": "привет" }),
        )];

        absorb(&updates, &mut registry);

        assert_eq!(registry.chats().len(), 1);
        assert_eq!(registry.chats()[0].id, 111);
        assert_eq!(registry.offset(), 11, "позиция в потоке запомнена");

        // The list survives a restart.
        let reloaded = Registry::load(&path).unwrap();
        assert_eq!(reloaded.chats()[0].id, 111);
        assert_eq!(reloaded.offset(), 11);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adding_the_bot_to_a_group_subscribes_the_group() {
        let path = state_path("absorb-group");
        let mut registry = Registry::load(&path).unwrap();

        let mut member = update(1, "my_chat_member", group_chat(-222, "Рабочая"), json!({}));
        member["my_chat_member"]["new_chat_member"] = json!({ "status": "member" });

        let replies = absorb(&[member], &mut registry);

        assert!(replies.is_empty(), "в группе бот молчит");
        assert_eq!(registry.chats().len(), 1);
        assert_eq!(registry.chats()[0].id, -222);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn removing_the_bot_from_a_group_unsubscribes_the_group() {
        let path = state_path("absorb-kicked");
        let mut registry = Registry::load(&path).unwrap();

        let mut joined = update(1, "my_chat_member", group_chat(-222, "Рабочая"), json!({}));
        joined["my_chat_member"]["new_chat_member"] = json!({ "status": "member" });
        absorb(&[joined], &mut registry);

        let mut left = update(2, "my_chat_member", group_chat(-222, "Рабочая"), json!({}));
        left["my_chat_member"]["new_chat_member"] = json!({ "status": "kicked" });
        absorb(&[left], &mut registry);

        assert!(registry.chats().is_empty(), "{:?}", registry.chats());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn start_and_stop_work_in_a_private_chat() {
        let path = state_path("absorb-commands");
        let mut registry = Registry::load(&path).unwrap();

        let start = update(
            1,
            "message",
            private_chat(111, "Анна"),
            json!({ "text": "/start" }),
        );
        let replies = absorb(&[start], &mut registry);
        assert_eq!(registry.chats().len(), 1);
        assert_eq!(replies.len(), 1, "человеку отвечают");

        let stop = update(
            2,
            "message",
            private_chat(111, "Анна"),
            json!({ "text": "/stop" }),
        );
        absorb(&[stop], &mut registry);
        assert!(registry.chats().is_empty(), "{:?}", registry.chats());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_command_with_the_bot_name_is_understood() {
        assert_eq!(command_of("/stop@my_bot"), Some("/stop"));
        assert_eq!(command_of("  /start  "), Some("/start"));
        assert_eq!(command_of("я не команда"), None);
    }

    #[test]
    fn a_command_in_a_group_does_not_change_the_list() {
        let path = state_path("absorb-group-command");
        let mut registry = Registry::load(&path).unwrap();

        let mut joined = update(1, "my_chat_member", group_chat(-222, "Рабочая"), json!({}));
        joined["my_chat_member"]["new_chat_member"] = json!({ "status": "member" });
        absorb(&[joined], &mut registry);

        let stop = update(
            2,
            "message",
            group_chat(-222, "Рабочая"),
            json!({ "text": "/stop" }),
        );
        absorb(&[stop], &mut registry);

        assert_eq!(
            registry.chats().len(),
            1,
            "в группе список меняет только добавление или удаление бота"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_broken_state_file_is_an_error_not_an_empty_list() {
        let path = state_path("absorb-broken");
        std::fs::write(&path, "{ это не json").unwrap();

        let err = match Registry::load(&path) {
            Err(e) => e,
            Ok(_) => panic!("сломанный файл состояния должен быть ошибкой"),
        };
        assert!(err.to_string().contains("не читается"), "{err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_single_mode_sink_needs_a_chat() {
        let err = TelegramSink::new("t", TelegramSettings::new("  ", TEST_TOKEN)).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn a_missing_variable_is_named() {
        let err = TelegramSink::from_spec(
            "t",
            TelegramSpec {
                chat_id: None,
                token_env: "AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN",
                state_file: "~/.local/state/ai-translator/telegram-chats.json",
                broadcast: true,
                bilingual: false,
                silent: false,
                thread: None,
            },
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN"),
            "{err}"
        );
    }

    #[test]
    fn a_home_relative_state_file_is_expanded() {
        let expanded = expand_home("~/state/chats.json");
        assert!(expanded.is_absolute(), "{expanded:?}");
        assert!(expanded.ends_with("state/chats.json"), "{expanded:?}");
    }
}
