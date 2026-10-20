//! Application configuration and its validation.
//!
//! The configuration is a TOML document describing the translation core and the
//! sinks. It is parsed in full before anything is started, so a typo in a sink
//! type or a missing environment variable is reported as a diagnostic instead
//! of a failure in the middle of a live translation.

use std::path::PathBuf;

use serde::Deserialize;

/// Root configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Translation core parameters.
    pub core: CoreConfig,
    /// Sinks.
    #[serde(default)]
    pub sink: Sinks,
}

/// Section `[core]`.
#[derive(Debug, Clone, Deserialize)]
pub struct CoreConfig {
    /// Provider identifier, for example `openai/translation`.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Source language (`auto` lets the provider detect it).
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    /// Target language.
    pub target_lang: String,
    /// Capture device (`default` is the system default device).
    #[serde(default = "default_device")]
    pub input_device: String,
    /// Playback device of the translated speech: `default` is the system
    /// default output, `none` plays nothing locally (the voice sinks still get
    /// the audio), anything else is the name of an output device as printed by
    /// the `devices` command.
    #[serde(default = "default_device")]
    pub output_device: String,
    /// Provider synthesis voice (used by the realtime provider only).
    #[serde(default)]
    pub voice: Option<String>,
    /// Source speech recognition model (realtime provider only; the synchronous
    /// translation provider performs recognition itself).
    #[serde(default)]
    pub transcription_model: Option<String>,
}

/// Section `[sink]` with the arrays `[[sink.text]]` and `[[sink.voice]]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Sinks {
    /// Text sinks.
    #[serde(default)]
    pub text: Vec<TextSinkConfig>,
    /// Voice sinks.
    #[serde(default)]
    pub voice: Vec<VoiceSinkConfig>,
}

/// Configuration of a text sink. The type name comes from the `type` field.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TextSinkConfig {
    /// Print the translation in a terminal.
    Console {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
    },
    /// Local WebSocket server serving the full-screen display page.
    Websocket {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Listen address.
        #[serde(default = "default_bind")]
        bind: String,
        /// Translation font size, in percent of the screen width.
        #[serde(default = "default_line_size_vw")]
        line_size_vw: f64,
        /// Lower bound the font shrinks to when a phrase does not fit.
        #[serde(default = "default_min_line_size_vw")]
        min_line_size_vw: f64,
        /// Poster image shown on the big screen while nothing is being
        /// translated: a path to a PNG, JPEG, GIF, WebP, AVIF or SVG file.
        #[serde(default)]
        image_path: Option<std::path::PathBuf>,
        /// Text above the poster image.
        #[serde(default)]
        image_text_top: Option<String>,
        /// Text below the poster image.
        #[serde(default)]
        image_text_bottom: Option<String>,
        /// Seconds without a new translation before the poster appears.
        ///
        /// Zero shows the poster as soon as the screen has nothing newer to
        /// display, including the very start of the run.
        #[serde(default)]
        image_show_timeout: u64,
    },
    /// Write the transcript and the translation to a file.
    Transcript {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Path to the file.
        path: PathBuf,
        /// Record format.
        #[serde(default)]
        format: TranscriptFormat,
        /// Write the source text next to the translation.
        #[serde(default)]
        bilingual: bool,
    },
    /// Send the translation to a Telegram chat through a bot.
    Telegram {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Channel to publish into: `@name` or a numeric id. The bot must be an
        /// administrator there with the right to post.
        chat_id: String,
        /// Name of the environment variable holding the bot token.
        #[serde(default = "default_telegram_token_env")]
        token_env: String,
        /// Post the source text above the translation.
        #[serde(default)]
        bilingual: bool,
        /// Send without a notification sound.
        #[serde(default)]
        silent: bool,
        /// Forum topic identifier, when the chat is a forum.
        #[serde(default)]
        message_thread_id: Option<i64>,
        /// How many new messages the sink may post per minute; 0 removes the limit.
        #[serde(default = "default_telegram_max_sends")]
        max_sends_per_minute: u32,
        /// Shortest time between two edits of one message, in milliseconds;
        /// 0 sends every revision.
        #[serde(default = "default_telegram_edit_interval_ms")]
        edit_interval_ms: u64,
    },
    /// Send the translation over HTTP POST using a body template.
    Webhook {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Endpoint address.
        url: String,
        /// Request body template with the substitutions `{text}`,
        /// `{source_text}`, `{is_final}`, `{seq}`, `{target_lang}`.
        body_template: String,
        /// Name of the environment variable holding the token (optional).
        #[serde(default)]
        token_env: Option<String>,
    },
}

impl TextSinkConfig {
    /// Whether the sink is enabled.
    pub fn enabled(&self) -> bool {
        match self {
            TextSinkConfig::Console { enabled }
            | TextSinkConfig::Websocket { enabled, .. }
            | TextSinkConfig::Transcript { enabled, .. }
            | TextSinkConfig::Telegram { enabled, .. }
            | TextSinkConfig::Webhook { enabled, .. } => *enabled,
        }
    }

    /// Sink type name as written in the configuration.
    pub fn kind(&self) -> &'static str {
        match self {
            TextSinkConfig::Console { .. } => "console",
            TextSinkConfig::Websocket { .. } => "websocket",
            TextSinkConfig::Transcript { .. } => "transcript",
            TextSinkConfig::Telegram { .. } => "telegram",
            TextSinkConfig::Webhook { .. } => "webhook",
        }
    }
}

/// Transcript file format.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptFormat {
    /// Plain text.
    #[default]
    Txt,
    /// SubRip subtitles.
    Srt,
    /// WebVTT subtitles.
    Vtt,
    /// Event log.
    Jsonl,
}

/// Configuration of a voice sink.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceSinkConfig {
    /// Play the translation into a virtual audio device.
    VirtualSink {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Device name.
        device: String,
        /// Monitoring volume of the original sound.
        #[serde(default)]
        monitor_original: f32,
    },
    /// Broadcast the translated speech into a Telegram voice chat, as a user
    /// account: a bot cannot join a group call, so this needs `api_id`,
    /// `api_hash` and a session of a real account.
    TelegramCall {
        /// Whether the sink is enabled.
        #[serde(default = "yes")]
        enabled: bool,
        /// Group whose voice chat carries the broadcast: a number or `@name`.
        chat_id: String,
        /// Name of the environment variable holding the `api_id` from my.telegram.org.
        #[serde(default = "default_telegram_api_id_env")]
        api_id_env: String,
        /// Name of the environment variable holding the `api_hash`.
        #[serde(default = "default_telegram_api_hash_env")]
        api_hash_env: String,
        /// File with the saved account session, written by `telegram-login`.
        #[serde(default = "default_telegram_session_file")]
        session_file: String,
        /// Start the voice chat when the group has none active.
        #[serde(default = "yes")]
        create_if_missing: bool,
    },
}

impl VoiceSinkConfig {
    /// Whether the sink is enabled.
    pub fn enabled(&self) -> bool {
        match self {
            VoiceSinkConfig::VirtualSink { enabled, .. } => *enabled,
            VoiceSinkConfig::TelegramCall { enabled, .. } => *enabled,
        }
    }

    /// Sink type name as written in the configuration.
    pub fn kind(&self) -> &'static str {
        match self {
            VoiceSinkConfig::VirtualSink { .. } => "virtual_sink",
            VoiceSinkConfig::TelegramCall { .. } => "telegram_call",
        }
    }
}

/// A configuration problem found before anything is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    /// Not a single sink is enabled.
    NoEnabledSink,
    /// An environment variable is not set.
    MissingEnv(String),
    /// A value is not usable.
    Invalid {
        /// Field that holds the value.
        field: String,
        /// What is wrong with it.
        reason: String,
    },
}

impl std::fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigProblem::NoEnabledSink => write!(f, "ни один приёмник не включён"),
            ConfigProblem::MissingEnv(name) => {
                write!(f, "не задана переменная окружения {name}")
            }
            ConfigProblem::Invalid { field, reason } => write!(f, "поле {field}: {reason}"),
        }
    }
}

impl std::error::Error for ConfigProblem {}

impl Config {
    /// Read the configuration from a file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or does not parse.
    pub fn load(path: &std::path::Path) -> std::io::Result<Config> {
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Notes that do not stop the run but are worth saying out loud.
    ///
    /// A screen bound to every interface is a working setup for a hall, and a
    /// questionable one everywhere else: whoever reaches the port reads every
    /// translation and the settings. The user asked for it explicitly, so this is
    /// a warning and not a refusal.
    pub fn warnings(&self) -> Vec<String> {
        let mut notes = Vec::new();

        for sink in &self.sink.text {
            let TextSinkConfig::Websocket {
                enabled: true,
                bind,
                ..
            } = sink
            else {
                continue;
            };

            if !is_loopback(bind) {
                notes.push(format!(
                    "экран слушает {bind}: перевод и настройки увидит любой, кто дотянется до порта"
                ));
            }
        }

        notes
    }

    /// Check the configuration without starting any translation.
    ///
    /// Disabled sinks are not checked: a sink that is switched off is not
    /// expected to be configured.
    pub fn validate(&self) -> Vec<ConfigProblem> {
        let mut problems = Vec::new();
        let mut enabled = 0usize;

        if self.core.target_lang.trim().is_empty() {
            problems.push(ConfigProblem::Invalid {
                field: "core.target_lang".into(),
                reason: "пустое значение".into(),
            });
        }
        if self.core.source_lang.trim().is_empty() {
            problems.push(ConfigProblem::Invalid {
                field: "core.source_lang".into(),
                reason: "пустое значение".into(),
            });
        }
        if self.core.input_device.trim().is_empty() {
            problems.push(ConfigProblem::Invalid {
                field: "core.input_device".into(),
                reason: "пустое значение".into(),
            });
        }
        if self.core.output_device.trim().is_empty() {
            problems.push(ConfigProblem::Invalid {
                field: "core.output_device".into(),
                reason: "пустое значение".into(),
            });
        }

        for sink in &self.sink.text {
            if !sink.enabled() {
                continue;
            }
            enabled += 1;

            match sink {
                TextSinkConfig::Console { .. } => {}
                TextSinkConfig::Websocket {
                    bind,
                    line_size_vw,
                    min_line_size_vw,
                    ..
                } => {
                    if bind.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.websocket.bind".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                    if *line_size_vw <= 0.0 || !line_size_vw.is_finite() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.websocket.line_size_vw".into(),
                            reason: "размер должен быть положительным числом".into(),
                        });
                    }
                    if *min_line_size_vw <= 0.0 || !min_line_size_vw.is_finite() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.websocket.min_line_size_vw".into(),
                            reason: "размер должен быть положительным числом".into(),
                        });
                    } else if *min_line_size_vw > *line_size_vw {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.websocket.min_line_size_vw".into(),
                            reason: "минимальный размер больше основного".into(),
                        });
                    }
                }
                TextSinkConfig::Transcript { path, .. } => {
                    if path.as_os_str().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.transcript.path".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                }
                TextSinkConfig::Telegram {
                    chat_id, token_env, ..
                } => {
                    if chat_id.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.telegram.chat_id".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                    if token_env.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.telegram.token_env".into(),
                            reason: "пустое значение".into(),
                        });
                    } else if std::env::var(token_env).is_err() {
                        problems.push(ConfigProblem::MissingEnv(token_env.clone()));
                    }
                }
                TextSinkConfig::Webhook {
                    url,
                    body_template,
                    token_env,
                    ..
                } => {
                    if !url.starts_with("http") {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.webhook.url".into(),
                            reason: "должен начинаться с http".into(),
                        });
                    }
                    if body_template.is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.text.webhook.body_template".into(),
                            reason: "пустой шаблон".into(),
                        });
                    }
                    if let Some(name) = token_env
                        && std::env::var(name).is_err()
                    {
                        problems.push(ConfigProblem::MissingEnv(name.clone()));
                    }
                }
            }
        }

        for sink in &self.sink.voice {
            if !sink.enabled() {
                continue;
            }
            enabled += 1;

            match sink {
                VoiceSinkConfig::VirtualSink { device, .. } => {
                    if device.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.voice.virtual_sink.device".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                }
                VoiceSinkConfig::TelegramCall {
                    chat_id,
                    api_id_env,
                    api_hash_env,
                    session_file,
                    ..
                } => {
                    if !telegram_call_supported() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.voice.telegram_call".into(),
                            reason: "эта сборка не умеет транслировать в голосовой чат: \
                                     платформа или архитектура не поддерживается"
                                .into(),
                        });
                        continue;
                    }

                    if chat_id.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.voice.telegram_call.chat_id".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                    if session_file.trim().is_empty() {
                        problems.push(ConfigProblem::Invalid {
                            field: "sink.voice.telegram_call.session_file".into(),
                            reason: "пустое значение".into(),
                        });
                    }
                    for name in [api_id_env, api_hash_env] {
                        if name.trim().is_empty() {
                            problems.push(ConfigProblem::Invalid {
                                field: "sink.voice.telegram_call".into(),
                                reason: "пустое имя переменной окружения".into(),
                            });
                        } else if std::env::var(name).is_err() {
                            problems.push(ConfigProblem::MissingEnv(name.clone()));
                        }
                    }
                }
            }
        }

        if enabled == 0 {
            problems.push(ConfigProblem::NoEnabledSink);
        }

        problems
    }

    /// Number of enabled sinks.
    pub fn enabled_sinks(&self) -> usize {
        self.sink.text.iter().filter(|s| s.enabled()).count()
            + self.sink.voice.iter().filter(|s| s.enabled()).count()
    }
}

fn yes() -> bool {
    true
}

/// Whether this build carries the live broadcast into a Telegram voice chat.
///
/// The feature is off by default and on only for the targets that ship the
/// native media engine: Linux x86_64, Linux arm64 and macOS arm64. A build
/// without it still parses the sink in the configuration and then refuses it
/// with this explanation, so a copy of the configuration on an unsupported
/// machine says why instead of silently dropping the translation.
pub const fn telegram_call_supported() -> bool {
    cfg!(feature = "telegram-call")
}

fn default_provider() -> String {
    "openai/translation".to_string()
}

fn default_source_lang() -> String {
    "auto".to_string()
}

fn default_device() -> String {
    "default".to_string()
}

fn default_telegram_token_env() -> String {
    "TELEGRAM_BOT_TOKEN".to_string()
}

/// Upper bound Telegram itself applies to new messages in one chat.
fn default_telegram_max_sends() -> u32 {
    20
}

/// One edit per second: enough for the phrase to grow visibly, mild for Telegram.
fn default_telegram_edit_interval_ms() -> u64 {
    1000
}

/// `api_id` of the Telegram application, from my.telegram.org.
fn default_telegram_api_id_env() -> String {
    "TELEGRAM_API_ID".to_string()
}

/// `api_hash` of the same application.
fn default_telegram_api_hash_env() -> String {
    "TELEGRAM_API_HASH".to_string()
}

/// Where the account session is kept once `telegram-login` has signed in.
fn default_telegram_session_file() -> String {
    let base = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.local/state")
    });

    format!("{base}/ai-translator/telegram-user.session")
}

/// Only this machine by default: the screen shows everything that is said, and a
/// hall that needs it on the network says so in its own configuration.
fn default_bind() -> String {
    "127.0.0.1:8642".to_string()
}

/// Whether an address listens on this machine only.
fn is_loopback(bind: &str) -> bool {
    let host = match bind.rsplit_once(':') {
        Some((host, _port)) => host.trim_matches(['[', ']']),
        None => bind,
    };
    matches!(host, "localhost" | "::1" | "0:0:0:0:0:0:0:1") || host.starts_with("127.")
}

/// Translation font size, in percent of the screen width.
fn default_line_size_vw() -> f64 {
    4.5
}

/// Lower bound the font shrinks to when a phrase does not fit.
fn default_min_line_size_vw() -> f64 {
    2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_poster_comes_from_the_configuration() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
            enabled = true
            image_path = "/tmp/stand.png"
            image_text_top = "MTL Fest"
            image_text_bottom = "english translation"
            image_show_timeout = 7
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Websocket {
                image_path,
                image_text_top,
                image_text_bottom,
                image_show_timeout,
                ..
            } => {
                assert_eq!(
                    image_path.as_deref(),
                    Some(std::path::Path::new("/tmp/stand.png"))
                );
                assert_eq!(image_text_top.as_deref(), Some("MTL Fest"));
                assert_eq!(image_text_bottom.as_deref(), Some("english translation"));
                assert_eq!(*image_show_timeout, 7);
            }
            other => panic!("не тот приёмник: {other:?}"),
        }
    }

    #[test]
    fn the_screen_poster_is_off_unless_configured() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
            enabled = true
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Websocket {
                image_path,
                image_text_top,
                image_text_bottom,
                image_show_timeout,
                ..
            } => {
                assert!(image_path.is_none());
                assert!(image_text_top.is_none());
                assert!(image_text_bottom.is_none());
                assert_eq!(*image_show_timeout, 0);
            }
            other => panic!("не тот приёмник: {other:?}"),
        }
    }

    #[test]
    fn playback_device_accepts_none_but_not_an_empty_name() {
        let mut cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            output_device = "none"
            [[sink.text]]
            type = "console"
            enabled = true
        "#,
        )
        .unwrap();

        assert_eq!(cfg.core.output_device, "none");
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());

        cfg.core.output_device = "   ".to_string();
        let problems = cfg.validate();

        assert!(
            problems
                .iter()
                .any(|problem| problem.to_string().contains("core.output_device")),
            "{problems:?}"
        );
    }

    #[test]
    fn parses_minimal_config() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "console"
            enabled = true
        "#,
        )
        .unwrap();

        assert_eq!(cfg.core.provider, "openai/translation");
        assert_eq!(cfg.core.source_lang, "auto");
        assert_eq!(cfg.core.input_device, "default");
        assert_eq!(cfg.core.output_device, "default");
        assert_eq!(cfg.sink.text.len(), 1);
        assert_eq!(cfg.enabled_sinks(), 1);
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
    }

    #[test]
    fn unknown_sink_type_is_reported_with_its_name() {
        let err = toml::from_str::<Config>(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "telegramm"
            enabled = true
        "#,
        )
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("telegramm"),
            "ошибка должна называть неизвестный тип: {err}"
        );
    }

    #[test]
    fn missing_env_var_is_reported() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "webhook"
            enabled = true
            url = "http://127.0.0.1:9/hook"
            body_template = "{\"text\":\"{text}\"}"
            token_env = "AI_TRANSLATOR_DEFINITELY_MISSING"
        "#,
        )
        .unwrap();

        let problems = cfg.validate();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0]
                .to_string()
                .contains("AI_TRANSLATOR_DEFINITELY_MISSING"),
            "{problems:?}"
        );
    }

    #[test]
    fn disabled_sinks_are_not_validated() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "console"
            enabled = true
            [[sink.text]]
            type = "webhook"
            enabled = false
            url = "not-a-url"
            body_template = ""
            token_env = "AI_TRANSLATOR_DEFINITELY_MISSING"
        "#,
        )
        .unwrap();

        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());
        assert_eq!(cfg.enabled_sinks(), 1);
    }

    #[test]
    fn a_non_positive_display_size_is_reported() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
            enabled = true
            line_size_vw = 0
            min_line_size_vw = 5
        "#,
        )
        .unwrap();

        let problems = cfg.validate();
        let text = problems
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(text.contains("line_size_vw"), "{text}");
        assert!(text.contains("min_line_size_vw"), "{text}");
        assert!(text.contains("больше основного"), "{text}");
    }

    #[test]
    fn config_without_enabled_sinks_is_reported() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "console"
            enabled = false
        "#,
        )
        .unwrap();

        assert_eq!(cfg.validate(), vec![ConfigProblem::NoEnabledSink]);
    }

    #[test]
    fn empty_target_language_is_reported() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = ""
            [[sink.text]]
            type = "console"
        "#,
        )
        .unwrap();

        let problems = cfg.validate();
        assert!(
            problems
                .iter()
                .any(|p| p.to_string().contains("core.target_lang")),
            "{problems:?}"
        );
    }

    #[test]
    fn console_sink_is_enabled_by_default() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "console"
        "#,
        )
        .unwrap();

        assert!(cfg.sink.text[0].enabled());
        assert_eq!(cfg.sink.text[0].kind(), "console");
    }

    #[test]
    fn transcript_format_defaults_to_txt() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "transcript"
            path = "/tmp/transcript.srt"
            format = "srt"
            bilingual = true
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Transcript {
                format, bilingual, ..
            } => {
                assert_eq!(*format, TranscriptFormat::Srt);
                assert!(*bilingual);
            }
            other => panic!("ожидался приёмник транскрипта, получено {other:?}"),
        }
    }

    #[test]
    fn websocket_bind_has_a_default() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Websocket {
                bind,
                line_size_vw,
                min_line_size_vw,
                ..
            } => {
                assert_eq!(bind, "127.0.0.1:8642", "экран открыт только себе");
                assert!(cfg.warnings().is_empty(), "тишина, пока не просили сеть");
                assert_eq!(*line_size_vw, 4.5, "размер по умолчанию");
                assert_eq!(*min_line_size_vw, 2.0, "предел ужимания по умолчанию");
            }
            other => panic!("ожидался приёмник websocket, получено {other:?}"),
        }
    }

    /// A hall needs the screen on the network; that is allowed, but said out loud.
    #[test]
    fn a_screen_on_every_interface_is_reported_as_a_warning() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
            bind = "0.0.0.0:8642"
        "#,
        )
        .unwrap();

        let warnings = cfg.warnings();
        assert_eq!(warnings.len(), 1, "одно предупреждение: {warnings:?}");
        assert!(warnings[0].contains("0.0.0.0:8642"), "{warnings:?}");
        assert!(cfg.validate().is_empty(), "и всё же это не отказ");
    }

    /// Loopback in its several spellings is not worth a word.
    #[test]
    fn loopback_addresses_are_not_warned_about() {
        for bind in [
            "127.0.0.1:8642",
            "localhost:8642",
            "[::1]:8642",
            "127.0.0.5:9000",
        ] {
            assert!(is_loopback(bind), "{bind} — этот же компьютер");
        }
        for bind in [
            "0.0.0.0:8642",
            "[::]:8642",
            "192.168.1.10:8642",
            "10.0.0.2:8642",
        ] {
            assert!(!is_loopback(bind), "{bind} — сеть");
        }
    }

    /// A disabled screen is not bound at all, so it cannot be exposed.
    #[test]
    fn a_disabled_screen_is_not_warned_about() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "websocket"
            enabled = false
            bind = "0.0.0.0:8642"
            [[sink.text]]
            type = "console"
        "#,
        )
        .unwrap();

        assert!(cfg.warnings().is_empty(), "{:?}", cfg.warnings());
    }

    #[test]
    fn voice_sink_is_parsed() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.voice]]
            type = "virtual_sink"
            device = "ai-translator"
            monitor_original = 0.2
        "#,
        )
        .unwrap();

        assert_eq!(cfg.sink.voice.len(), 1);
        assert_eq!(cfg.enabled_sinks(), 1);
        assert_eq!(cfg.sink.voice[0].kind(), "virtual_sink");
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn config_is_loaded_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai-translator.toml");
        std::fs::write(
            &path,
            "[core]\ntarget_lang = \"ru\"\n[[sink.text]]\ntype = \"console\"\n",
        )
        .unwrap();

        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.core.target_lang, "ru");
        assert_eq!(cfg.enabled_sinks(), 1);
    }

    #[test]
    fn broken_config_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "[core]\ntarget_lang = \n").unwrap();

        let err = Config::load(&path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_config_file_is_reported() {
        let err =
            Config::load(std::path::Path::new("/nonexistent/ai-translator.toml")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn telegram_sink_is_parsed_with_its_defaults() {
        // PATH есть в любом окружении: сам токен здесь не нужен, проверяется
        // разбор и умолчания.
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "telegram"
            chat_id = "@translations"
            token_env = "PATH"
        "#,
        )
        .unwrap();

        assert_eq!(cfg.sink.text[0].kind(), "telegram");
        assert!(cfg.validate().is_empty(), "{:?}", cfg.validate());

        match &cfg.sink.text[0] {
            TextSinkConfig::Telegram {
                token_env,
                bilingual,
                silent,
                message_thread_id,
                ..
            } => {
                assert_eq!(token_env, "PATH");
                assert!(!bilingual, "исходный текст по умолчанию не дублируется");
                assert!(!silent, "по умолчанию сообщение со звуком");
                assert_eq!(message_thread_id, &None);
            }
            other => panic!("ожидался приёмник Telegram, получено {other:?}"),
        }
    }

    #[test]
    fn telegram_token_variable_defaults_to_the_bot_token_name() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "telegram"
            chat_id = "@translations"
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Telegram { token_env, .. } => {
                assert_eq!(token_env, "TELEGRAM_BOT_TOKEN");
            }
            other => panic!("ожидался приёмник Telegram, получено {other:?}"),
        }
    }

    #[test]
    fn telegram_without_its_token_variable_is_reported() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "telegram"
            chat_id = "@translations"
            token_env = "AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN"
        "#,
        )
        .unwrap();

        let text = cfg
            .validate()
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            text.contains("AI_TRANSLATOR_DEFINITELY_MISSING_TOKEN"),
            "{text}"
        );
    }

    #[test]
    fn telegram_limits_have_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"
            [[sink.text]]
            type = "telegram"
            chat_id = "@channel"
            token_env = "PATH"
        "#,
        )
        .unwrap();

        match &cfg.sink.text[0] {
            TextSinkConfig::Telegram {
                max_sends_per_minute,
                edit_interval_ms,
                ..
            } => {
                assert_eq!(*max_sends_per_minute, 20, "двадцать сообщений в минуту");
                assert_eq!(*edit_interval_ms, 1000, "одна правка в секунду");
            }
            other => panic!("ожидался приёмник Telegram, получено {other:?}"),
        }
    }

    #[test]
    fn the_all_example_parses_and_matches_the_schema() {
        let cfg: Config = toml::from_str(include_str!("../examples/config.all.toml")).unwrap();

        // Файл-справочник обязан содержать каждый тип приёмника: переименование
        // поля уронит этот тест вместе с примером.
        assert_eq!(
            cfg.sink.text.len(),
            5,
            "console, websocket, transcript, telegram и webhook"
        );
        assert_eq!(cfg.sink.voice.len(), 2, "virtual_sink и telegram_call");
        let kinds: Vec<&str> = cfg.sink.voice.iter().map(|sink| sink.kind()).collect();
        assert_eq!(kinds, ["virtual_sink", "telegram_call"]);
        assert!(
            cfg.validate().is_empty(),
            "пример должен проходить проверку: {:?}",
            cfg.validate()
        );
    }

    #[test]
    fn telegram_call_sink_is_parsed_with_its_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"

            [[sink.voice]]
            type = "telegram_call"
            chat_id = "-1001234567890"
            "#,
        )
        .expect("конфигурация разбирается");

        let VoiceSinkConfig::TelegramCall {
            enabled,
            chat_id,
            api_id_env,
            api_hash_env,
            session_file,
            create_if_missing,
        } = &cfg.sink.voice[0]
        else {
            panic!("ожидался приёмник telegram_call");
        };

        assert!(enabled, "приёмник включён, если не сказано иное");
        assert_eq!(chat_id, "-1001234567890");
        assert_eq!(api_id_env, "TELEGRAM_API_ID");
        assert_eq!(api_hash_env, "TELEGRAM_API_HASH");
        assert!(
            session_file.ends_with("ai-translator/telegram-user.session"),
            "сессия лежит в каталоге состояния: {session_file}"
        );
        assert!(*create_if_missing, "звонок создаётся, если его нет");
    }

    #[cfg(not(feature = "telegram-call"))]
    #[test]
    fn a_build_without_the_call_feature_refuses_the_sink() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"

            [[sink.voice]]
            type = "telegram_call"
            chat_id = "-1001234567890"
            "#,
        )
        .expect("конфигурация разбирается: отказ — дело проверки, а не разбора");

        let problems = cfg.validate();
        assert!(
            problems.iter().any(|problem| matches!(
                problem,
                ConfigProblem::Invalid { field, reason }
                    if field == "sink.voice.telegram_call"
                        && reason.contains("не умеет транслировать")
            )),
            "сборка без трансляции обязана объяснить отказ: {problems:?}"
        );
    }

    #[cfg(feature = "telegram-call")]
    #[test]
    fn a_build_with_the_call_feature_asks_for_the_account() {
        let cfg: Config = toml::from_str(
            r#"
            [core]
            target_lang = "ru"

            [[sink.voice]]
            type = "telegram_call"
            chat_id = "-1001234567890"
            api_id_env = "AI_TRANSLATOR_MISSING_API_ID"
            "#,
        )
        .expect("конфигурация разбирается");

        let problems = cfg.validate();
        assert!(
            problems.iter().any(|problem| matches!(
                problem,
                ConfigProblem::MissingEnv(name) if name == "AI_TRANSLATOR_MISSING_API_ID"
            )),
            "нужен api_id: {problems:?}"
        );
    }
}
