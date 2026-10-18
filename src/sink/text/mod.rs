//! Text sinks: delivering the translated text.

/// Console sink: watching the translation in a terminal.
pub mod console;
/// Transcript sink: writing the translation to TXT, SRT, VTT or JSONL.
pub mod transcript;
/// Webhook sink: sending the translation over HTTP POST.
pub mod webhook;
/// Local WebSocket server sink serving the full-screen display page.
pub mod websocket;
