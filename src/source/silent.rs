//! Playback that makes no sound.
//!
//! The translation line always needs somewhere to put the translated speech.
//! With `output_device = "none"` the speech goes here: the line keeps running,
//! the voice sinks still receive the audio, and the speakers stay silent.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::Sink;
use transono::audio::{Audio, AudioFormat, AudioOutput, BoxSink, Endianness, PcmFormat};
use transono::core::error::CoreError;

/// Playback endpoint that accepts every chunk and plays none of them.
#[derive(Debug, Clone, Copy)]
pub struct SilentOutput {
    format: AudioFormat,
}

impl SilentOutput {
    /// Playback format of the synchronous translation provider: PCM, 24 kHz,
    /// mono, little endian. Nothing is written anywhere, but the line configures
    /// its output pipeline from this value, and this is the format the provider
    /// already produces — so the pipeline stays a passthrough.
    pub fn new() -> Self {
        Self {
            format: AudioFormat {
                sample_rate: 24_000,
                channels: 1,
                sample_format: PcmFormat::F32(Endianness::Little),
            },
        }
    }
}

impl Default for SilentOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput for SilentOutput {
    fn sink(&mut self) -> Result<BoxSink<'static, Audio, CoreError>, CoreError> {
        Ok(Box::pin(SilentSink))
    }

    fn start(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    fn format(&self) -> AudioFormat {
        self.format
    }
}

/// Sink that discards everything it is given.
struct SilentSink;

impl Sink<Audio> for SilentSink {
    type Error = CoreError;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), CoreError>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, _item: Audio) -> Result<(), CoreError> {
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), CoreError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), CoreError>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_matches_what_the_provider_produces() {
        let format = SilentOutput::new().format();

        assert_eq!(format.sample_rate, 24_000);
        assert_eq!(format.channels, 1);
        assert_eq!(format.sample_format, PcmFormat::F32(Endianness::Little));
    }

    #[tokio::test]
    async fn everything_written_is_dropped() {
        use futures_util::SinkExt;
        use transono::audio::PcmAudio;
        use transono::providers::openai::translation::OpenAITranslationConfig;

        let spec = OpenAITranslationConfig::default()
            .audio_format()
            .spec()
            .clone();
        let pcm = PcmAudio::new(spec, 240);
        let audio = Audio::from_pcm(&pcm).expect("test chunk");

        let mut output = SilentOutput::new();
        let mut sink = output.sink().expect("sink");

        sink.send(audio).await.expect("send");
        sink.close().await.expect("close");
    }
}
