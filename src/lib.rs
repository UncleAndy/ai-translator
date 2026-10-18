//! ai-translator: synchronous speech and text translation with parallel
//! delivery of the result to independent sinks.
//!
//! The translation core produces a normalized stream of translation events;
//! delivery is performed by isolated sink modules that can be enabled and
//! disabled individually.

#![warn(missing_docs)]

/// Translation event bus and the event model.
pub mod bus;
/// Application configuration and its validation.
pub mod config;
/// Result sinks: text and voice.
pub mod sink;
