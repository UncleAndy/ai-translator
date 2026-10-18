//! Glue between transono sessions and the translation event bus.

/// Adapter from provider session events to normalized translation events.
pub mod adapt;
/// Translation line wiring over transono: devices, provider session, bridge.
pub mod line;
