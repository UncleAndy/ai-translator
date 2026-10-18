//! Transcript sink: writing the translation to TXT, SRT, VTT or JSONL.
//!
//! The sink is a stub for now: the registry must be able to build it from the
//! configuration, but the record formats are not written yet. It reports that
//! honestly instead of silently dropping the translation.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::bus::TranslationEvent;
use crate::config::TranscriptFormat;
use crate::sink::{Result, SinkError, TextSink};

/// Writes the transcript and the translation to a file.
#[derive(Debug)]
pub struct TranscriptSink {
    id: String,
    path: PathBuf,
    format: TranscriptFormat,
    bilingual: bool,
}

impl TranscriptSink {
    /// Create a sink writing to `path` in the given format.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is empty.
    pub fn new(id: &str, path: &Path, format: TranscriptFormat, bilingual: bool) -> Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(SinkError::Protocol(
                "пустой путь к файлу транскрипта".into(),
            ));
        }

        Ok(Self {
            id: id.to_string(),
            path: path.to_path_buf(),
            format,
            bilingual,
        })
    }

    /// Path of the transcript file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record format.
    pub fn format(&self) -> TranscriptFormat {
        self.format
    }

    /// Whether the source text is written next to the translation.
    pub fn is_bilingual(&self) -> bool {
        self.bilingual
    }
}

#[async_trait]
impl TextSink for TranscriptSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, _ev: &TranslationEvent) -> Result<()> {
        Err(SinkError::Unavailable(
            "приёмник «файл транскрипта» ещё не реализован".into(),
        ))
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_path() {
        let err =
            TranscriptSink::new("file", Path::new(""), TranscriptFormat::Txt, false).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn keeps_path_and_format() {
        let sink = TranscriptSink::new(
            "file",
            Path::new("/tmp/transcript.srt"),
            TranscriptFormat::Srt,
            true,
        )
        .unwrap();

        assert_eq!(sink.path(), Path::new("/tmp/transcript.srt"));
        assert_eq!(sink.format(), TranscriptFormat::Srt);
        assert!(sink.is_bilingual());
        assert_eq!(sink.id(), "file");
    }
}
