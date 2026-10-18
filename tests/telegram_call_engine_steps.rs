//! Probe: each engine call of the sink, one by one.
//!
//! The join is a chain of engine calls, and a crash anywhere in it kills the
//! process without a word. This walks the chain step by step so the chain itself
//! can be blamed for a specific step, without Telegram and without a call.

#![cfg(feature = "telegram-call")]

use ntgcalls::{
    AudioDescription, FrameData, MediaDescription, MediaSource, NTgCalls, StreamDevice, StreamMode,
    VideoRotation,
};

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_takes_the_sources_of_the_sink() {
    let engine = NTgCalls::new();
    let call_id = -1_000_000_000_123i64;

    let local = engine
        .create_call(call_id)
        .await
        .expect("параметры звонка");
    println!("create_call: ok, {} символов параметров", local.len());

    let sources = MediaDescription {
        microphone: Some(AudioDescription {
            media_source: MediaSource::External,
            sample_rate: 48_000,
            channel_count: 1,
            input: String::new(),
            keep_open: true,
        }),
        speaker: None,
        camera: None,
        screen: None,
    };

    println!("--- set_stream_sources ---");
    let answer = engine
        .set_stream_sources(call_id, StreamMode::Capture, &sources)
        .await;
    println!("set_stream_sources: {answer:?}");

    println!("--- send_external_frame ---");
    let frame = vec![0u8; 960 * 2];
    let answer = engine
        .send_external_frame(
            call_id,
            StreamDevice::Microphone,
            &frame,
            &FrameData {
                absolute_capture_timestamp_ms: 0,
                rotation: VideoRotation::VideoRotation0,
                width: 0,
                height: 0,
            },
        )
        .await;
    println!("send_external_frame: {answer:?}");

    println!("--- stop ---");
    let answer = engine.stop(call_id).await;
    println!("stop: {answer:?}");
}
