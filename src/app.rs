//! Application assembly: sources → core → bus → sinks.
//!
//! The order of assembly matters and is fixed: the bus comes first, then the
//! sinks (so that they are subscribed before the first event exists), and only
//! then the source. A sink that cannot be built is reported before anything is
//! started, and a sink that fails while running is restarted by the supervisor
//! without touching the others.

use std::collections::HashMap;
use std::time::Duration;

use crate::bus::EventBus;
use crate::config::Config;
use crate::sink::SinkError;
use crate::sink::registry::{build_text_sink, build_voice_sink};
use crate::source::audio::{AudioSource, SourceError};
use crate::source::text::TextSource;
use crate::supervisor::{RestartPolicy, SinkState, Supervisor};

/// How many events may wait for one sink before the bus starts dropping
/// intermediate revisions.
const SINK_CAPACITY: usize = 256;

/// How long a sink waits before the first restart; the delay doubles after that.
const RESTART_DELAY: Duration = Duration::from_millis(500);

/// Ceiling of the restart delay: half a minute is long enough for a network to
/// come back, and short enough that a broadcast is not lost for a whole session.
const RESTART_MAX_DELAY: Duration = Duration::from_secs(30);

/// How many times a sink is restarted after a failure. Six attempts with the
/// doubling delay span about half a minute of trouble.
const RESTART_ATTEMPTS: u32 = 6;

/// Why the application could not be assembled or run.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The configuration could not be read.
    #[error("конфигурация не прочитана: {0}")]
    Config(std::io::Error),

    /// The configuration is not usable.
    #[error("конфигурация не готова к запуску: {0}")]
    Invalid(String),

    /// A sink could not be built.
    #[error("приёмник «{id}» не собран: {error}")]
    Sink {
        /// Sink identifier.
        id: String,
        /// What went wrong.
        error: SinkError,
    },

    /// The translation core could not be started.
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Assembled application: bus, sinks and, once started, the translation core.
pub struct App {
    bus: EventBus,
    supervisor: Supervisor,
    core: Option<AudioSource>,
}

impl App {
    /// Read a configuration file, reporting a bad file as an application error.
    pub fn load_config(path: &std::path::Path) -> Result<Config, AppError> {
        Config::load(path).map_err(AppError::Config)
    }

    /// Assemble the application from a configuration.
    ///
    /// Every enabled sink is built here and handed to the supervisor as its
    /// first instance: a sink takes a resource when it is built — a listening
    /// socket, an open file — and building it twice would take that resource
    /// twice. A failure to build is reported before anything starts.
    pub fn build(cfg: &Config) -> Result<Self, AppError> {
        Self::build_with_policy(cfg, Self::restart_policy())
    }

    /// The policy the application gives its sinks by default.
    pub fn restart_policy() -> RestartPolicy {
        RestartPolicy::limited(RESTART_ATTEMPTS, RESTART_DELAY).with_max_delay(RESTART_MAX_DELAY)
    }

    /// Assemble the application with an explicit restart policy.
    ///
    /// The default policy spans half a minute of trouble, which is right in
    /// production and far too long for a test: the tests that watch a sink die pass
    /// their own short policy here.
    pub fn build_with_policy(cfg: &Config, policy: RestartPolicy) -> Result<Self, AppError> {
        let problems = cfg.validate();
        if !problems.is_empty() {
            let text = problems
                .iter()
                .map(|problem| problem.to_string())
                .collect::<Vec<_>>()
                .join("; ");

            return Err(AppError::Invalid(text));
        }

        let bus = EventBus::new();
        let supervisor = Supervisor::new(bus.clone(), policy).with_capacity(SINK_CAPACITY);

        let mut ids: HashMap<String, usize> = HashMap::new();

        for sink in cfg.sink.text.iter().filter(|sink| sink.enabled()) {
            let id = next_id(&mut ids, sink.kind());
            let ready = build_text_sink(&id, sink).map_err(|error| AppError::Sink {
                id: id.clone(),
                error,
            })?;

            let sink = sink.clone();
            let factory_id = id.clone();
            supervisor.spawn_ready(&id, ready, move || build_text_sink(&factory_id, &sink));
        }

        for sink in cfg.sink.voice.iter().filter(|sink| sink.enabled()) {
            let id = next_id(&mut ids, sink.kind());
            let ready = build_voice_sink(&id, sink).map_err(|error| AppError::Sink {
                id: id.clone(),
                error,
            })?;

            let sink = sink.clone();
            let factory_id = id.clone();
            supervisor.spawn_voice_ready(&id, ready, move || build_voice_sink(&factory_id, &sink));
        }

        Ok(Self {
            bus,
            supervisor,
            core: None,
        })
    }

    /// State of a sink, or [`SinkState::Unknown`] if there is no such sink.
    pub fn sink_state(&self, id: &str) -> SinkState {
        self.supervisor.state_of(id)
    }

    /// States of all sinks, in a stable order.
    pub fn sink_states(&self) -> Vec<(String, SinkState)> {
        let mut states = self.supervisor.states();
        states.sort_by(|left, right| left.0.cmp(&right.0));

        states
    }

    /// Identifiers of the running sinks, in a stable order.
    pub fn sink_ids(&self) -> Vec<String> {
        self.sink_states().into_iter().map(|(id, _)| id).collect()
    }

    /// Start translation from the microphone.
    pub async fn start_audio(&mut self, cfg: &Config) -> Result<(), AppError> {
        let source = AudioSource::start(&cfg.core, self.bus.clone()).await?;
        self.core = Some(source);

        Ok(())
    }

    /// Feed a text to the bus, bypassing the translation provider.
    ///
    /// Returns the number of events published.
    pub async fn publish_text(&self, cfg: &Config, text: &str) -> usize {
        let source = TextSource::new(
            crate::bus::LineId(0),
            crate::source::lang_pair(&cfg.core),
            text,
        );

        source.run(&self.bus).await
    }

    /// Stop everything: the phrase in progress is written out, connections are
    /// closed, files are saved.
    pub async fn shutdown(mut self) -> Result<(), AppError> {
        if let Some(core) = self.core.take() {
            core.stop().await?;
        }

        self.supervisor.shutdown().await;

        Ok(())
    }
}

/// Sink identifier: the type name, numbered when the type repeats.
fn next_id(ids: &mut HashMap<String, usize>, kind: &str) -> String {
    let count = ids.entry(kind.to_string()).or_insert(0);
    *count += 1;

    if *count == 1 {
        kind.to_string()
    } else {
        format!("{kind}-{count}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, CoreConfig, Sinks, TextSinkConfig};
    use std::path::PathBuf;

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

    fn console(enabled: bool) -> TextSinkConfig {
        TextSinkConfig::Console { enabled }
    }

    #[test]
    fn identifiers_number_repeating_types() {
        let mut ids = HashMap::new();

        assert_eq!(next_id(&mut ids, "console"), "console");
        assert_eq!(next_id(&mut ids, "console"), "console-2");
        assert_eq!(next_id(&mut ids, "transcript"), "transcript");
        assert_eq!(next_id(&mut ids, "console"), "console-3");
    }

    #[test]
    fn broken_configuration_is_refused_before_anything_starts() {
        let cfg = config(vec![TextSinkConfig::Transcript {
            enabled: true,
            path: PathBuf::from("/tmp/тест.jsonl"),
            format: crate::config::TranscriptFormat::Jsonl,
            bilingual: false,
        }]);
        let mut cfg = cfg;
        cfg.core.target_lang = String::new();

        let error = App::build(&cfg).err().expect("сборка отказана");
        assert!(matches!(error, AppError::Invalid(_)), "{error:?}");
        assert!(error.to_string().contains("target_lang"), "{error}");
    }

    #[tokio::test]
    async fn disabled_sinks_are_not_started() {
        let cfg = config(vec![console(true), console(false)]);
        let app = App::build(&cfg).expect("приложение собрано");

        assert_eq!(app.sink_ids(), vec!["console".to_string()]);
    }

    #[tokio::test]
    async fn a_sink_that_cannot_be_built_is_reported_with_its_name() {
        // Адрес заведомо непригоден: сборка приёмника должна сказать об этом
        // до запуска, а не молча ждать первого события.
        let cfg = config(vec![TextSinkConfig::Websocket {
            enabled: true,
            bind: "не адрес".to_string(),
            line_size_vw: 4.5,
            min_line_size_vw: 2.0,
            image_path: None,
            image_text_top: None,
            image_text_bottom: None,
            image_show_timeout: 0,
        }]);

        let error = App::build(&cfg).err().expect("сборка отказана");
        match error {
            AppError::Sink { ref id, .. } => assert_eq!(id, "websocket"),
            other => panic!("неожиданная ошибка: {other:?}"),
        }
    }
}
