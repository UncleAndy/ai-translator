//! Voice sinks: delivering the translated speech.

/// Telegram voice sink: the translated speech goes to a group as voice messages.
pub mod telegram_voice;
/// Virtual audio device sink: playing the translated speech into a device.
pub mod virtual_sink;
