//! The voice note must be a format Telegram plays: OGG with Opus inside.
//!
//! The Bot API is strict here — `sendVoice` accepts OGG/Opus and nothing else —
//! so the file this sink builds is checked by an independent player. The check
//! is skipped when `ffprobe` is not installed, and it never silently passes: a
//! present `ffprobe` that dislikes the file fails the test.

use std::process::Command;

use ai_translator::sink::voice::telegram_voice::encode_voice;

#[test]
fn ffprobe_recognises_the_voice_note() {
    let available = Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success());

    if !available {
        eprintln!("ffprobe не найден — проверка формата пропущена");
        return;
    }

    let rate = 24_000u32;
    let frames = rate as usize;
    let mut plane = vec![0.0f32; frames];
    for (index, sample) in plane.iter_mut().enumerate() {
        let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / rate as f32;
        *sample = 0.25 * phase.sin();
    }

    let voice = encode_voice(&[plane], rate).expect("звук кодируется");
    assert_eq!(&voice[..4], b"OggS", "контейнер Ogg");

    let path = std::env::temp_dir().join("ai-translator-voice-check.ogg");
    std::fs::write(&path, &voice).expect("файл записан");

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,channels:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(&path)
        .output()
        .expect("ffprobe запускается");

    let report = String::from_utf8_lossy(&probe.stdout).to_string();
    let _ = std::fs::remove_file(&path);

    assert!(
        probe.status.success(),
        "ffprobe отверг файл: {}",
        String::from_utf8_lossy(&probe.stderr)
    );
    assert!(
        report.contains("codec_name=opus"),
        "внутри должен быть Opus: {report}"
    );
    assert!(
        report.contains("channels=1"),
        "канал один, как у голосового: {report}"
    );

    // Opus always plays at 48 kHz and the container states its own length, so the
    // second is checked by the clock rather than by the rate it was encoded from.
    let seconds: f64 = report
        .lines()
        .find_map(|line| line.strip_prefix("duration="))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("длительность не прочитана: {report}"));
    assert!(
        (0.85..1.15).contains(&seconds),
        "одна секунда речи — одна секунда файла: {seconds}"
    );
}
