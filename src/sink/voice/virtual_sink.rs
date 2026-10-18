//! Virtual audio device sink: playing the translated speech into a device.
//!
//! The sink is a stub for now: the registry must be able to build it from the
//! configuration, but playback is not written yet. It reports that honestly
//! instead of silently dropping the translated speech.

use async_trait::async_trait;
use transono::audio::Audio;

use crate::bus::LineId;
use crate::sink::{Result, SinkError, VoiceSink};

/// Plays the translated speech into a virtual audio device.
#[derive(Debug)]
pub struct VirtualSink {
    id: String,
    device: String,
    monitor_original: f32,
}

impl VirtualSink {
    /// Create a sink playing into `device`.
    ///
    /// # Errors
    ///
    /// Returns an error when the device name is empty or the monitoring volume
    /// is outside `0.0..=1.0`.
    pub fn new(id: &str, device: &str, monitor_original: f32) -> Result<Self> {
        if device.trim().is_empty() {
            return Err(SinkError::Protocol("пустое имя аудиоустройства".into()));
        }
        if !(0.0..=1.0).contains(&monitor_original) {
            return Err(SinkError::Protocol(format!(
                "громкость мониторинга вне диапазона 0.0..=1.0: {monitor_original}"
            )));
        }

        Ok(Self {
            id: id.to_string(),
            device: device.to_string(),
            monitor_original,
        })
    }

    /// Device name.
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Monitoring volume of the original sound.
    pub fn monitor_original(&self) -> f32 {
        self.monitor_original
    }
}

#[async_trait]
impl VoiceSink for VirtualSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_audio(&mut self, _line: LineId, _audio: &Audio) -> Result<()> {
        Err(SinkError::Unavailable(
            "голосовой приёмник «виртуальное аудиоустройство» ещё не реализован".into(),
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
    fn rejects_empty_device() {
        let err = VirtualSink::new("voice", "  ", 0.0).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn rejects_monitoring_volume_out_of_range() {
        let err = VirtualSink::new("voice", "ai-translator", 1.5).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn keeps_device_settings() {
        let sink = VirtualSink::new("voice", "ai-translator", 0.2).unwrap();
        assert_eq!(sink.device(), "ai-translator");
        assert_eq!(sink.monitor_original(), 0.2);
        assert_eq!(sink.id(), "voice");
    }
}
