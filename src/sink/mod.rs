//! Result sinks: text and voice.

/// Sink registry: building a sink from its configuration entry.
pub mod registry;
/// Text sinks: delivering the translated text.
pub mod text;
/// Voice sinks: delivering the translated speech.
pub mod voice;
