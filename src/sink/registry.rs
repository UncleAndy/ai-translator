//! Sink registry: building a sink from its configuration entry.
//!
//! Unknown sink types are rejected earlier, when the configuration is parsed,
//! so the registry only maps the known types onto constructors. It still owns
//! the [`unknown_type`] helper for callers that switch on a type name by hand.

use std::time::Duration;

use super::text::console::ConsoleSink;
use super::text::telegram::{TelegramSink, TelegramSpec};
use super::text::transcript::TranscriptSink;
use super::text::webhook::WebhookSink;
use super::text::websocket::{Poster, ScreenSettings, WebsocketSink};
use super::voice::virtual_sink::VirtualSink;
use super::{Result, SinkError, TextSink, VoiceSink};
use crate::config::{TextSinkConfig, VoiceSinkConfig};

/// Build a text sink from its configuration.
///
/// # Errors
///
/// Returns an error when the sink cannot be created, for example when a listen
/// address or a file path is unusable.
pub fn build_text_sink(id: &str, cfg: &TextSinkConfig) -> Result<Box<dyn TextSink>> {
    match cfg {
        TextSinkConfig::Console { .. } => Ok(Box::new(ConsoleSink::new(id))),
        TextSinkConfig::Websocket {
            bind,
            line_size_vw,
            min_line_size_vw,
            image_path,
            image_text_top,
            image_text_bottom,
            image_show_timeout,
            ..
        } => Ok(Box::new(WebsocketSink::new(
            id,
            bind,
            ScreenSettings {
                line_size_vw: *line_size_vw,
                min_line_size_vw: *min_line_size_vw,
                poster: image_path.as_ref().map(|_| Poster {
                    text_top: image_text_top.clone().unwrap_or_default(),
                    text_bottom: image_text_bottom.clone().unwrap_or_default(),
                    timeout_secs: *image_show_timeout,
                }),
            },
            image_path.clone(),
        )?)),
        TextSinkConfig::Transcript {
            path,
            format,
            bilingual,
            ..
        } => Ok(Box::new(TranscriptSink::new(
            id, path, *format, *bilingual,
        )?)),
        TextSinkConfig::Telegram {
            chat_id,
            token_env,
            bilingual,
            silent,
            message_thread_id,
            max_sends_per_minute,
            edit_interval_ms,
            ..
        } => Ok(Box::new(TelegramSink::from_spec(
            id,
            TelegramSpec {
                chat_id,
                token_env,
                bilingual: *bilingual,
                silent: *silent,
                thread: *message_thread_id,
                max_sends_per_minute: *max_sends_per_minute,
                edit_interval: Duration::from_millis(*edit_interval_ms),
            },
        )?)),
        TextSinkConfig::Webhook {
            url,
            body_template,
            token_env,
            ..
        } => Ok(Box::new(WebhookSink::new(
            id,
            url,
            body_template,
            token_env.as_deref(),
        )?)),
    }
}

/// Build a voice sink from its configuration.
///
/// # Errors
///
/// Returns an error when the sink cannot be created, for example when the
/// device name is unusable.
pub fn build_voice_sink(id: &str, cfg: &VoiceSinkConfig) -> Result<Box<dyn VoiceSink>> {
    match cfg {
        VoiceSinkConfig::VirtualSink {
            device,
            monitor_original,
            ..
        } => Ok(Box::new(VirtualSink::new(id, device, *monitor_original)?)),
        VoiceSinkConfig::TelegramCall {
            chat_id,
            api_id_env,
            api_hash_env,
            session_file,
            create_if_missing,
            ..
        } => {
            if !crate::config::telegram_call_supported() {
                return Err(SinkError::Unavailable(
                    "эта сборка не умеет транслировать в голосовой чат: \
                     платформа или архитектура не поддерживается"
                        .into(),
                ));
            }

            #[cfg(feature = "telegram-call")]
            {
                let credentials = crate::telegram_account::Credentials::from_env(
                    api_id_env,
                    api_hash_env,
                    session_file,
                )
                .map_err(SinkError::Protocol)?;

                let settings = crate::sink::voice::telegram_call::TelegramCallSettings {
                    chat_id: chat_id.clone(),
                    api_id: credentials.api_id,
                    api_hash: credentials.api_hash,
                    session_file: credentials.session_file,
                    create_if_missing: *create_if_missing,
                };

                Ok(Box::new(
                    crate::sink::voice::telegram_call::TelegramCallSink::new(id, settings)?,
                ))
            }

            #[cfg(not(feature = "telegram-call"))]
            {
                let _ = (
                    chat_id,
                    api_id_env,
                    api_hash_env,
                    session_file,
                    create_if_missing,
                );
                Err(SinkError::Unavailable(
                    "эта сборка собрана без функции трансляции в голосовой чат: \
                     нужна сборка с --features telegram-call"
                        .into(),
                ))
            }
        }
    }
}

/// Sink error naming a type the registry does not know.
pub fn unknown_type(kind: &str) -> SinkError {
    SinkError::UnknownType(kind.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TranscriptFormat;

    fn console() -> TextSinkConfig {
        TextSinkConfig::Console { enabled: true }
    }

    fn websocket(bind: &str) -> TextSinkConfig {
        TextSinkConfig::Websocket {
            enabled: true,
            bind: bind.to_string(),
            line_size_vw: 4.5,
            min_line_size_vw: 2.0,
            image_path: None,
            image_text_top: None,
            image_text_bottom: None,
            image_show_timeout: 0,
        }
    }

    fn transcript(path: &str, format: TranscriptFormat) -> TextSinkConfig {
        TextSinkConfig::Transcript {
            enabled: true,
            path: path.into(),
            format,
            bilingual: true,
        }
    }

    fn webhook() -> TextSinkConfig {
        TextSinkConfig::Webhook {
            enabled: true,
            url: "http://127.0.0.1:9/h".into(),
            body_template: "{\"text\":\"{text}\"}".into(),
            token_env: None,
        }
    }

    /// Токен берётся из существующей переменной: сборка приёмника не должна
    /// зависеть от сети, а проверяется только маршрутизация по типу.
    fn telegram() -> TextSinkConfig {
        TextSinkConfig::Telegram {
            enabled: true,
            chat_id: "-1001234567890".into(),
            token_env: "PATH".into(),
            bilingual: false,
            silent: true,
            message_thread_id: None,
            max_sends_per_minute: 20,
            edit_interval_ms: 1000,
        }
    }

    /// Голосовой чат без файла сессии: сборка приёмника обязана сказать, что
    /// сначала нужно войти аккаунтом, а не падать в глубине звонка.
    #[cfg(feature = "telegram-call")]
    #[test]
    fn a_voice_chat_without_a_session_asks_for_the_login() {
        let cfg = VoiceSinkConfig::TelegramCall {
            enabled: true,
            chat_id: "@mtl_fest_en_translate".into(),
            api_id_env: "AI_TRANSLATOR_TEST_APP_ID".into(),
            api_hash_env: "AI_TRANSLATOR_TEST_APP_HASH".into(),
            session_file: "/nonexistent/ai-translator-test.session".into(),
            create_if_missing: true,
        };

        // SAFETY: the test owns these names and reads them right here.
        unsafe {
            std::env::set_var("AI_TRANSLATOR_TEST_APP_ID", "12345");
            std::env::set_var("AI_TRANSLATOR_TEST_APP_HASH", "0123456789abcdef");
        }

        let err = match build_voice_sink("voice-1", &cfg) {
            Ok(_) => panic!("приёмник собрался без файла сессии"),
            Err(error) => error.to_string(),
        };

        assert!(err.contains("telegram-login"), "{err}");
        assert!(err.contains("сессии"), "{err}");
    }

    #[test]
    fn builds_console_sink_by_type_name() {
        let sink = build_text_sink("console-1", &console()).unwrap();
        assert_eq!(sink.id(), "console-1");
    }

    #[test]
    fn builds_every_configured_text_sink() {
        let cases = [
            ("console", console()),
            ("screen", websocket("127.0.0.1:0")),
            (
                "file",
                transcript("/tmp/ai-translator-test.txt", TranscriptFormat::Txt),
            ),
            ("hook", webhook()),
            ("tg", telegram()),
        ];

        for (id, cfg) in cases {
            let sink = build_text_sink(id, &cfg).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(sink.id(), id, "идентификатор приёмника сохраняется");
        }
    }

    #[test]
    fn builds_voice_sink_by_type_name() {
        let cfg = VoiceSinkConfig::VirtualSink {
            enabled: true,
            device: "ai-translator".into(),
            monitor_original: 0.2,
        };
        let sink = build_voice_sink("voice-1", &cfg).unwrap();
        assert_eq!(sink.id(), "voice-1");
    }

    #[test]
    fn unusable_listen_address_is_reported() {
        let err = build_text_sink("screen", &websocket("не-адрес"))
            .err()
            .expect("ожидалась ошибка сборки");
        assert!(
            matches!(err, SinkError::Protocol(_)),
            "ожидалась ошибка протокола, получено {err:?}"
        );
        assert!(err.to_string().contains("не-адрес"), "{err}");
    }

    #[test]
    fn unknown_type_is_named_in_the_error() {
        let err = unknown_type("telegramm");
        assert_eq!(err.to_string(), "неизвестный тип приёмника: telegramm");
    }

    #[tokio::test]
    async fn not_yet_implemented_sinks_report_unavailability() {
        use crate::bus::{LangPair, Language, LineId, TranslationEvent};
        use std::time::Instant;

        let event = TranslationEvent::Translated {
            line: LineId(0),
            langs: LangPair {
                source: None,
                target: Language::code("ru"),
            },
            text: "привет".into(),
            delta: Some("привет".into()),
            is_final: true,
            seq: 1,
            at: Instant::now(),
        };

        let mut sink = build_text_sink("hook", &webhook()).unwrap();
        let err = sink.on_event(&event).await.unwrap_err();
        assert!(
            matches!(err, SinkError::Unavailable(_)),
            "заглушка обязана честно сообщать о неготовности, получено {err:?}"
        );
        sink.shutdown().await.unwrap();
    }
}
