//! End-to-end pipeline tests: source → bus → sinks, without network.
//!
//! These tests go through the whole chain the application assembles — the bus,
//! the supervisor, the registry and the sinks — and check the result where it
//! can be observed: in the transcript file on disk and in the state of the
//! sinks. Nothing here needs a microphone, a provider or an API key.

use std::time::{Duration, Instant};

use ai_translator::app::App;
use ai_translator::bus::{EventBus, LangPair, Language, LineId, TranslationEvent};
use ai_translator::config::{Config, CoreConfig, Sinks, TextSinkConfig, TranscriptFormat};
use ai_translator::sink::text::console::ConsoleSink;
use ai_translator::sink::text::transcript::TranscriptSink;
use ai_translator::source::script::ScriptSource;
use ai_translator::supervisor::{RestartPolicy, SinkState, Supervisor};

fn langs() -> LangPair {
    LangPair {
        source: Some(Language::Auto),
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

fn core() -> CoreConfig {
    CoreConfig {
        provider: "openai/translation".to_string(),
        source_lang: "auto".to_string(),
        target_lang: "ru".to_string(),
        input_device: "default".to_string(),
        output_device: "default".to_string(),
        voice: None,
        transcription_model: None,
        session_reconnect_minutes: 55,
    }
}

fn config(sinks: Vec<TextSinkConfig>) -> Config {
    Config {
        core: core(),
        sink: Sinks {
            text: sinks,
            voice: Vec::new(),
        },
    }
}

/// The whole chain, assembled by hand: a script source feeds two sinks and the
/// translation lands in the file.
#[tokio::test]
async fn a_script_source_reaches_two_sinks() {
    let dir = tempfile::tempdir().expect("временный каталог");
    let path = dir.path().join("out.srt");

    let bus = EventBus::new();
    let mut supervisor = Supervisor::new(
        bus.clone(),
        RestartPolicy::limited(1, Duration::from_millis(5)),
    );

    supervisor.spawn("console", || Ok(Box::new(ConsoleSink::new("console"))));

    let file = path.clone();
    supervisor.spawn("file", move || {
        Ok(Box::new(
            TranscriptSink::new("file", &file, TranscriptFormat::Srt, true)
                .expect("файл транскрипта"),
        ))
    });

    let source = ScriptSource::new(vec![
        translated("Привет,", false, 1),
        translated("Привет, это тест.", true, 2),
    ])
    .with_delay(Duration::ZERO);

    assert_eq!(source.run(&bus).await, 2, "оба события опубликованы");

    supervisor.shutdown().await;

    let text = std::fs::read_to_string(&path).expect("транскрипт создан");
    assert!(
        text.contains("Привет, это тест."),
        "файловый приёмник получил событие: {text:?}"
    );
    assert!(
        text.contains("00:00:00,000 -->"),
        "разметка SRT на месте: {text:?}"
    );
}

/// The same chain through the assembled application: the text source, the bus
/// and the registry, as the `script` command runs it.
#[tokio::test]
async fn the_application_run_writes_a_transcript() {
    let dir = tempfile::tempdir().expect("временный каталог");
    let path = dir.path().join("script.jsonl");

    let cfg = config(vec![
        TextSinkConfig::Console { enabled: true },
        TextSinkConfig::Transcript {
            enabled: true,
            path: path.clone(),
            format: TranscriptFormat::Jsonl,
            bilingual: true,
        },
    ]);

    let app = App::build(&cfg).expect("приложение собрано");
    assert_eq!(
        app.sink_ids(),
        vec!["console".to_string(), "transcript".to_string()]
    );

    let published = app.publish_text(&cfg, "Первое слово. Второе слово.").await;
    assert_eq!(published, 4, "по два события на фразу");

    app.shutdown().await.expect("остановлено");

    let text = std::fs::read_to_string(&path).expect("транскрипт создан");
    assert_eq!(
        text.lines().count(),
        4,
        "каждое событие — строка JSONL: {text:?}"
    );
    assert!(
        text.contains("\"text\":\"Второе слово.\""),
        "последняя фраза дописана: {text:?}"
    );
}

/// A sink that fails does not take the others down with it, even when the whole
/// application is assembled: the webhook sink is a stage 2 placeholder and
/// always answers "unavailable", while the console keeps working.
#[tokio::test]
async fn the_application_keeps_a_healthy_sink_when_another_fails() {
    let cfg = config(vec![
        TextSinkConfig::Console { enabled: true },
        TextSinkConfig::Webhook {
            enabled: true,
            url: "http://127.0.0.1:9/hook".to_string(),
            body_template: "{\"text\": \"{text}\"}".to_string(),
            token_env: None,
        },
    ]);

    let app = App::build_with_policy(&cfg, RestartPolicy::limited(2, Duration::from_millis(20)))
        .expect("приложение собрано");
    assert_eq!(app.publish_text(&cfg, "Один два три.").await, 3);

    // Приёмник вебхука отказывает на каждом событии: он должен исчерпать
    // попытки перезапуска и уйти в ошибку, не тронув консоль.
    tokio::time::sleep(Duration::from_millis(1600)).await;

    let console = app.sink_state("console");
    let webhook = app.sink_state("webhook");

    assert!(
        console.is_running(),
        "исправный приёмник работает: {console:?}"
    );
    assert!(
        webhook.is_error(),
        "сломанный приёмник назван ошибкой, а не молчит: {webhook:?}"
    );
    assert!(
        matches!(webhook, SinkState::Error(ref reason) if reason.contains("не реализован")),
        "причина отказа сохранена: {webhook:?}"
    );

    app.shutdown().await.expect("остановлено");
}
