//! Transcript sink: writing the translation to TXT, SRT, VTT or JSONL.
//!
//! The file is opened when the sink is created and kept open; every block is
//! written and flushed at once, so a transcript survives a crash and the last
//! phrase is never left half-written. Times are counted from the moment the
//! sink started, because the translation event carries a monotonic instant, not
//! a wall clock.
//!
//! File writes are blocking and short (a subtitle block is a few hundred
//! bytes), so the sink writes directly instead of paying for a thread pool.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::bus::TranslationEvent;
use crate::config::TranscriptFormat;
use crate::sink::{Result, SinkError, TextSink};

/// Shortest block written for a phrase that arrived in a single event.
const MIN_BLOCK: Duration = Duration::from_millis(500);

/// Header of a WebVTT file.
const VTT_HEADER: &str = "WEBVTT\n\n";

/// Format an SRT timestamp: `HH:MM:SS,mmm`.
pub fn format_srt_timestamp(t: Duration) -> String {
    let millis = t.subsec_millis();
    let seconds = t.as_secs();
    format!(
        "{:02}:{:02}:{:02},{:03}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
        millis
    )
}

/// Format a WebVTT timestamp: `HH:MM:SS.mmm`.
pub fn format_vtt_timestamp(t: Duration) -> String {
    let millis = t.subsec_millis();
    let seconds = t.as_secs();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60,
        millis
    )
}

/// Format one SubRip block, including its number.
pub fn format_srt_block(index: u64, start: Duration, end: Duration, text: &str) -> String {
    format!(
        "{index}\n{} --> {}\n{text}\n\n",
        format_srt_timestamp(start),
        format_srt_timestamp(end)
    )
}

/// Format one WebVTT block (WebVTT blocks carry no number).
pub fn format_vtt_block(start: Duration, end: Duration, text: &str) -> String {
    format!(
        "{} --> {}\n{text}\n\n",
        format_vtt_timestamp(start),
        format_vtt_timestamp(end)
    )
}

/// One record of a JSONL transcript.
#[derive(Debug, serde::Serialize)]
struct JsonlRecord<'a> {
    /// Milliseconds since the sink started.
    t_ms: u64,
    /// Line the event belongs to.
    line: u16,
    /// Kind of the record.
    kind: &'static str,
    /// Text of the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    /// Monotonic event number within the line.
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<u64>,
    /// The phrase is complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    is_final: Option<bool>,
    /// Machine-readable state name.
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
}

/// Writes the transcript and the translation to a file.
#[derive(Debug)]
pub struct TranscriptSink {
    id: String,
    path: PathBuf,
    format: TranscriptFormat,
    bilingual: bool,
    file: std::fs::File,
    started: Instant,
    phrase_start: Option<Duration>,
    blocks: u64,
    last_source: String,
}

impl TranscriptSink {
    /// Create a sink writing to `path` in the given format.
    ///
    /// The file is created (an existing file is truncated), so a new run starts
    /// a new transcript.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is empty or the file cannot be created.
    pub fn new(id: &str, path: &Path, format: TranscriptFormat, bilingual: bool) -> Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(SinkError::Protocol(
                "пустой путь к файлу транскрипта".into(),
            ));
        }

        let mut file = std::fs::File::create(path)?;
        if format == TranscriptFormat::Vtt {
            file.write_all(VTT_HEADER.as_bytes())?;
            file.flush()?;
        }

        Ok(Self {
            id: id.to_string(),
            path: path.to_path_buf(),
            format,
            bilingual,
            file,
            started: Instant::now(),
            phrase_start: None,
            blocks: 0,
            last_source: String::new(),
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

    /// Number of written blocks (phrases or records).
    pub fn blocks(&self) -> u64 {
        self.blocks
    }

    /// Time of an event counted from the moment the sink started.
    fn elapsed(&self, at: Instant) -> Duration {
        at.saturating_duration_since(self.started)
    }

    /// Text of a block: the source text above the translation in bilingual mode.
    fn block_text(&self, text: &str) -> String {
        if self.bilingual && !self.last_source.is_empty() {
            format!("{}\n{text}", self.last_source)
        } else {
            text.to_string()
        }
    }

    /// Write a finished phrase.
    fn write_phrase(&mut self, start: Duration, end: Duration, text: &str) -> Result<()> {
        let end = if end > start { end } else { start + MIN_BLOCK };
        let text = self.block_text(text);

        let block = match self.format {
            TranscriptFormat::Srt => format_srt_block(self.blocks + 1, start, end, &text),
            TranscriptFormat::Vtt => format_vtt_block(start, end, &text),
            TranscriptFormat::Txt => format!("{text}\n"),
            // В JSONL фразы пишутся событийными записями, а не блоками.
            TranscriptFormat::Jsonl => return Ok(()),
        };

        self.file.write_all(block.as_bytes())?;
        self.file.flush()?;
        self.blocks += 1;
        Ok(())
    }

    /// Write one JSONL record.
    fn write_record(&mut self, record: &JsonlRecord<'_>) -> Result<()> {
        let line = serde_json::to_string(record).expect("запись транскрипта сериализуется");
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        self.blocks += 1;
        Ok(())
    }
}

#[async_trait]
impl TextSink for TranscriptSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()> {
        match ev {
            TranslationEvent::SourceText { text, at, line, .. } => {
                self.last_source = text.clone();
                if self.format == TranscriptFormat::Jsonl {
                    let record = JsonlRecord {
                        t_ms: self.elapsed(*at).as_millis() as u64,
                        line: line.0,
                        kind: "source",
                        text: Some(text),
                        seq: None,
                        is_final: None,
                        state: None,
                    };
                    self.write_record(&record)?;
                }
            }

            TranslationEvent::Translated {
                line,
                text,
                is_final,
                seq,
                at,
                ..
            } => {
                if self.format == TranscriptFormat::Jsonl {
                    let record = JsonlRecord {
                        t_ms: self.elapsed(*at).as_millis() as u64,
                        line: line.0,
                        kind: "translated",
                        text: Some(text),
                        seq: Some(*seq),
                        is_final: Some(*is_final),
                        state: None,
                    };
                    self.write_record(&record)?;
                } else {
                    if text.is_empty() {
                        return Ok(());
                    }

                    let now = self.elapsed(*at);
                    let start = *self.phrase_start.get_or_insert(now);

                    if *is_final {
                        self.phrase_start = None;
                        self.write_phrase(start, now, text)?;
                    }
                }
            }

            TranslationEvent::Status {
                line, state, at, ..
            } => {
                if self.format == TranscriptFormat::Jsonl {
                    let record = JsonlRecord {
                        t_ms: self.elapsed(*at).as_millis() as u64,
                        line: line.0,
                        kind: "status",
                        text: None,
                        seq: None,
                        is_final: None,
                        state: Some(state.as_str()),
                    };
                    self.write_record(&record)?;
                }
            }

            TranslationEvent::TranslatedAudio { .. } => {}
        }

        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        let mut file = self.file;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId, LineState};

    fn langs() -> LangPair {
        LangPair {
            source: None,
            target: Language::code("ru"),
        }
    }

    fn translated(text: &str, is_final: bool, seq: u64) -> TranslationEvent {
        TranslationEvent::Translated {
            line: LineId(0),
            langs: langs(),
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final,
            seq,
            at: Instant::now(),
        }
    }

    fn source_text(text: &str) -> TranslationEvent {
        TranslationEvent::SourceText {
            line: LineId(0),
            text: text.to_string(),
            delta: Some(text.to_string()),
            is_final: true,
            at: Instant::now(),
        }
    }

    #[test]
    fn srt_block_has_correct_timestamps() {
        let out = format_srt_block(
            1,
            Duration::from_millis(1500),
            Duration::from_millis(4250),
            "привет мир",
        );
        assert_eq!(out, "1\n00:00:01,500 --> 00:00:04,250\nпривет мир\n\n");
    }

    #[test]
    fn vtt_block_uses_dot_separator() {
        let out = format_vtt_block(
            Duration::from_millis(1500),
            Duration::from_millis(4250),
            "привет мир",
        );
        assert!(out.starts_with("00:00:01.500 --> 00:00:04.250"));
        assert!(out.ends_with("привет мир\n\n"));
    }

    #[test]
    fn timestamps_pass_an_hour() {
        let t = Duration::from_secs(3725) + Duration::from_millis(7);
        assert_eq!(format_srt_timestamp(t), "01:02:05,007");
        assert_eq!(format_vtt_timestamp(t), "01:02:05.007");
    }

    #[test]
    fn rejects_empty_path() {
        let err =
            TranscriptSink::new("file", Path::new(""), TranscriptFormat::Txt, false).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn rejects_unusable_path() {
        let err = TranscriptSink::new(
            "file",
            Path::new("/nonexistent-directory/transcript.txt"),
            TranscriptFormat::Txt,
            false,
        )
        .unwrap_err();
        assert!(matches!(err, SinkError::Io(_)), "{err:?}");
    }

    #[tokio::test]
    async fn srt_file_ends_with_a_complete_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.srt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Srt, false).unwrap();

        sink.on_event(&translated("привет", true, 1)).await.unwrap();
        sink.on_event(&translated("мир", true, 2)).await.unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.trim_end().ends_with("мир"),
            "последний блок дописан: {text:?}"
        );
        assert!(text.contains(" --> "), "метки времени на месте: {text:?}");
        assert_eq!(text.matches(" --> ").count(), 2, "два блока: {text:?}");
    }

    #[tokio::test]
    async fn txt_file_has_one_line_per_phrase() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Txt, false).unwrap();

        sink.on_event(&translated("при", false, 1)).await.unwrap();
        sink.on_event(&translated("привет", false, 2))
            .await
            .unwrap();
        sink.on_event(&translated("привет мир", true, 3))
            .await
            .unwrap();
        sink.on_event(&translated("вторая фраза", true, 4))
            .await
            .unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text, "привет мир\nвторая фраза\n",
            "ревизии не размножаются"
        );
    }

    #[tokio::test]
    async fn vtt_file_starts_with_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.vtt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Vtt, false).unwrap();

        sink.on_event(&translated("привет мир", true, 1))
            .await
            .unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("WEBVTT\n\n"), "{text:?}");
        assert!(text.contains(" --> "), "{text:?}");
        assert!(
            !text.contains("\n1\n"),
            "в WebVTT блоки без номеров: {text:?}"
        );
    }

    #[tokio::test]
    async fn jsonl_file_has_one_record_per_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.jsonl");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Jsonl, false).unwrap();

        sink.on_event(&source_text("hello")).await.unwrap();
        sink.on_event(&translated("привет", false, 1))
            .await
            .unwrap();
        sink.on_event(&translated("привет мир", true, 2))
            .await
            .unwrap();
        sink.on_event(&TranslationEvent::Status {
            line: LineId(0),
            state: LineState::Running,
            at: Instant::now(),
        })
        .await
        .unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let records: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("запись JSON"))
            .collect();

        assert_eq!(records.len(), 4);
        assert_eq!(records[0]["kind"], "source");
        assert_eq!(records[0]["text"], "hello");
        assert_eq!(records[1]["kind"], "translated");
        assert_eq!(records[1]["is_final"], false);
        assert_eq!(records[2]["seq"], 2);
        assert_eq!(records[3]["state"], "running");
        assert!(records[0]["t_ms"].is_u64(), "время записано: {text:?}");
    }

    #[tokio::test]
    async fn bilingual_writes_source_above_translation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.srt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Srt, true).unwrap();

        sink.on_event(&source_text("hello world")).await.unwrap();
        sink.on_event(&translated("привет мир", true, 1))
            .await
            .unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hello world\nпривет мир"), "{text:?}");
    }

    #[tokio::test]
    async fn single_event_phrase_gets_a_visible_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.srt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Srt, false).unwrap();

        sink.on_event(&translated("одним куском", true, 1))
            .await
            .unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let line = text
            .lines()
            .find(|line| line.contains(" --> "))
            .expect("блок с метками времени");

        let (start, end) = line.split_once(" --> ").expect("обе метки");
        assert_ne!(start, end, "блок нулевой длины недопустим: {text:?}");
    }

    #[tokio::test]
    async fn empty_text_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        let mut sink = TranscriptSink::new("t", &path, TranscriptFormat::Txt, false).unwrap();

        sink.on_event(&translated("", true, 1)).await.unwrap();
        Box::new(sink).shutdown().await.unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.is_empty(), "пустая фраза не пишется: {text:?}");
    }
}
