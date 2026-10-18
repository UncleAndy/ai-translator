//! Translation event bus: normalized events and their fan-out to sinks.

/// Normalized translation event model shared by the core and the sinks.
pub mod event;

pub use event::*;
