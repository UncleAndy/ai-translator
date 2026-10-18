//! Voice sinks: delivering the translated speech.

/// Live broadcast of the translated speech into a Telegram group call.
#[cfg(feature = "telegram-call")]
pub mod telegram_call;
/// Virtual audio device sink: playing the translated speech into a device.
pub mod virtual_sink;
