//! Sources of speech and text.

/// Audio source wired to the transono synchronous translation provider.
pub mod audio;
/// Scripted source replaying a fixed event sequence for tests and offline runs.
pub mod script;
/// Text source feeding the bus directly, without the speech provider.
pub mod text;
