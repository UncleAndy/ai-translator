//! The media engine of the voice-chat sink has to load and answer.
//!
//! Everything the sink does with Telegram rests on this library: it is the one
//! that builds the WebRTC parameters `phone.joinGroupCall` expects and then
//! carries the audio. The test asks it for those parameters, so a broken link,
//! a missing `libstdc++` or a library built for another architecture fails here
//! instead of in the middle of a live broadcast.
#![cfg(feature = "telegram-call")]

use ntgcalls::NTgCalls;

#[tokio::test]
async fn the_engine_produces_join_parameters() {
    println!("ntgcalls {}", ntgcalls::version());

    let engine = NTgCalls::new();
    let params = engine
        .create_call(-1001234567890)
        .await
        .expect("движок отдаёт параметры входа");

    let value: serde_json::Value = serde_json::from_str(&params).expect("параметры — JSON");

    for key in ["ufrag", "pwd", "fingerprints", "ssrc"] {
        assert!(
            !value[key].is_null(),
            "в параметрах входа нет поля {key}: {params}"
        );
    }

    assert!(
        value["pwd"].as_str().is_some_and(|pwd| pwd.len() >= 16),
        "пароль сессии слишком короткий: {params}"
    );
}
