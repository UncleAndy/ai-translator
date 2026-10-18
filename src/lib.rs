//! ai-translator: synchronous speech and text translation with parallel
//! delivery of the result to independent sinks.
//!
//! The translation core produces a normalized stream of translation events;
//! delivery is performed by isolated sink modules that can be enabled and
//! disabled individually.

#![warn(missing_docs)]

/// Application assembly: sources → core → bus → sinks.
pub mod app;
/// Translation event bus and the event model.
pub mod bus;
/// Application configuration and its validation.
pub mod config;
/// Glue between transono sessions and the translation event bus.
pub mod core_glue;
/// Result sinks: text and voice.
pub mod sink;
/// Sources of speech and text.
pub mod source;
/// Sink supervisor: starting, isolating and restarting sinks.
pub mod supervisor;
/// Sign-in of the account that broadcasts into a Telegram voice chat.
#[cfg(feature = "telegram-call")]
pub mod telegram_account;
/// Optional speech synthesis (text → audio).
pub mod tts;
