//! Probe: the sink must survive the first chunk of speech.
//!
//! The engine is created on the sink's own thread, and the join needs a session;
//! without a valid one the sink has to answer with an error, not take the process
//! down. Run with the feature enabled.

#![cfg(feature = "telegram-call")]

use ai_translator::bus::LineId;
use ai_translator::sink::VoiceSink;
use ai_translator::sink::voice::telegram_call::{TelegramCallSettings, TelegramCallSink};
use symphonia::core::audio::{AudioSpec, Channels, Position};
use transono::audio::{Audio, PcmAudio};

#[tokio::test(flavor = "multi_thread")]
async fn the_first_chunk_of_speech_does_not_take_the_process_down() {
    let dir = std::env::temp_dir().join("ai-translator-probe");
    std::fs::create_dir_all(&dir).expect("каталог проб");
    let session = dir.join("probe.session");
    std::fs::write(&session, b"").expect("пустой файл сессии");

    let settings = TelegramCallSettings {
        chat_id: "@mtl_fest_en_translate".to_string(),
        api_id: 31_099_668,
        api_hash: "0123456789abcdef0123456789abcdef".to_string(),
        session_file: session,
        create_if_missing: true,
    };

    let mut sink = TelegramCallSink::new("voice-probe", settings).expect("приёмник собран");

    let spec = AudioSpec::new(48_000, Channels::Positioned(Position::FRONT_CENTER));
    let mut pcm = PcmAudio::new(spec, 4_800);
    for (index, sample) in pcm.data.iter_mut().enumerate() {
        *sample = (index as f32 * 0.01).sin() * 0.3;
    }

    let audio = Audio::from_pcm(&pcm).expect("звук собран");
    let answer = sink.on_audio(LineId(1), &audio).await;

    println!("ответ приёмника: {answer:?}");
    assert!(answer.is_err(), "без сессии звонок не начинается");
}
