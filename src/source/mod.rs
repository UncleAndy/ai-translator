//! Sources of speech and text.

use crate::bus::{LangPair, Language};
use crate::config::CoreConfig;

/// Audio source wired to the transono synchronous translation provider.
pub mod audio;
/// Scripted source replaying a fixed event sequence for tests and offline runs.
pub mod script;
/// Text source feeding the bus directly, without the speech provider.
pub mod text;

/// Language pair of a line, taken from the configuration.
///
/// The source language is reported as [`Language::Auto`] when the configuration
/// asks for automatic detection.
pub fn lang_pair(cfg: &CoreConfig) -> LangPair {
    let source = if cfg.source_lang == "auto" {
        Language::Auto
    } else {
        Language::code(&cfg.source_lang)
    };

    LangPair {
        source: Some(source),
        target: Language::code(&cfg.target_lang),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CoreConfig {
        CoreConfig {
            provider: "openai/translation".to_string(),
            source_lang: "auto".to_string(),
            target_lang: "ru".to_string(),
            input_device: "default".to_string(),
            voice: None,
            transcription_model: None,
        }
    }

    #[test]
    fn language_pair_follows_the_configuration() {
        let pair = lang_pair(&config());
        assert_eq!(pair.source, Some(Language::Auto));
        assert_eq!(pair.target, Language::code("ru"));

        let mut cfg = config();
        cfg.source_lang = "en".to_string();
        assert_eq!(lang_pair(&cfg).source, Some(Language::code("en")));
    }
}
