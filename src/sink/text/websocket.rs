//! Local WebSocket server sink serving the full-screen display page.
//!
//! The sink runs an HTTP server: `GET /` returns the big-screen page and
//! `GET /ws` upgrades to a WebSocket. Every connected screen receives every
//! translation event, so several displays can be driven at once. The server
//! listens on its own socket, so a display works independently of the other
//! sinks.

use std::net::SocketAddr;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Html;
use axum::routing::get;
use tokio::sync::{broadcast, watch};

use crate::bus::TranslationEvent;
use crate::sink::{Result, SinkError, TextSink};

/// Capacity of the broadcast channel to the screens.
const BROADCAST_CAPACITY: usize = 64;

/// The big-screen page, embedded into the binary.
const PAGE: &str = include_str!("../../../web/index.html");

/// A message sent to the big-screen page.
///
/// The wire format is described in the header of `web/index.html`: a
/// translation message carries `text`/`is_final`/`seq`, a state message carries
/// `state`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum BroadcastMessage {
    /// A fragment of the translated text.
    Translation {
        /// Line the event belongs to.
        line: u16,
        /// Accumulated text of the phrase.
        text: String,
        /// Recognized source text, for the bilingual mode.
        source_text: Option<String>,
        /// The phrase is complete.
        is_final: bool,
        /// Monotonic event number within the line.
        seq: u64,
    },
    /// A line state change.
    State {
        /// Line the event belongs to.
        line: u16,
        /// Machine-readable state name.
        state: &'static str,
    },
}

impl BroadcastMessage {
    /// Build a message from a translation event.
    ///
    /// Returns `None` for events the page does not display: source text is
    /// attached to the translation message instead, and the translated audio
    /// goes to voice sinks.
    pub fn from_event(ev: &TranslationEvent, source_text: Option<&str>) -> Option<Self> {
        match ev {
            TranslationEvent::Translated {
                line,
                text,
                is_final,
                seq,
                ..
            } => Some(BroadcastMessage::Translation {
                line: line.0,
                text: text.clone(),
                source_text: source_text.map(str::to_string),
                is_final: *is_final,
                seq: *seq,
            }),
            TranslationEvent::Status { line, state, .. } => Some(BroadcastMessage::State {
                line: line.0,
                state: state.as_str(),
            }),
            TranslationEvent::SourceText { .. } | TranslationEvent::TranslatedAudio { .. } => None,
        }
    }

    /// Serialize the message for the page.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("сообщение перевода сериализуется")
    }
}

/// Serves the translation to clients of the full-screen page.
#[derive(Debug)]
pub struct WebsocketSink {
    id: String,
    addr: SocketAddr,
    tx: broadcast::Sender<String>,
    /// Listening socket, until the server task takes it over.
    listener: Option<std::net::TcpListener>,
    stop_tx: watch::Sender<bool>,
    last_source: String,
    sent: u64,
}

impl WebsocketSink {
    /// Create a sink listening on `bind`.
    ///
    /// The address is occupied immediately, so an unusable address is reported
    /// here instead of at the first event. When a Tokio runtime is already
    /// running, the server starts at once; otherwise it starts on the first
    /// event.
    ///
    /// # Errors
    ///
    /// Returns an error when the address is not a `host:port` socket address or
    /// cannot be occupied.
    pub fn new(id: &str, bind: &str) -> Result<Self> {
        let requested: SocketAddr = bind.parse().map_err(|e| {
            SinkError::Protocol(format!("некорректный адрес прослушивания {bind}: {e}"))
        })?;

        let listener = std::net::TcpListener::bind(requested).map_err(|e| {
            SinkError::Unavailable(format!("не удалось занять адрес {requested}: {e}"))
        })?;
        listener.set_nonblocking(true)?;

        let addr = listener.local_addr()?;
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (stop_tx, _) = watch::channel(false);

        let mut sink = Self {
            id: id.to_string(),
            addr,
            tx,
            listener: Some(listener),
            stop_tx,
            last_source: String::new(),
            sent: 0,
        };
        sink.start_if_possible();

        Ok(sink)
    }

    /// Address the server actually listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Number of messages published to the screens.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Last recognized source text, attached to translation messages.
    pub fn last_source(&self) -> &str {
        &self.last_source
    }

    /// Start the server task when a runtime is available.
    fn start_if_possible(&mut self) {
        let Some(listener) = self.listener.take() else {
            return;
        };

        match tokio::runtime::Handle::try_current() {
            Ok(_) => spawn_server(listener, self.tx.clone(), self.stop_tx.subscribe()),
            Err(_) => self.listener = Some(listener),
        }
    }
}

#[async_trait]
impl TextSink for WebsocketSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn on_event(&mut self, ev: &TranslationEvent) -> Result<()> {
        if let TranslationEvent::SourceText { text, .. } = ev {
            self.last_source = text.clone();
        }

        self.start_if_possible();

        let source = if self.last_source.is_empty() {
            None
        } else {
            Some(self.last_source.as_str())
        };

        if let Some(message) = BroadcastMessage::from_event(ev, source) {
            self.sent += 1;
            // Отсутствие подключённых экранов — не ошибка.
            let _ = self.tx.send(message.to_json());
        }

        Ok(())
    }

    async fn shutdown(self: Box<Self>) -> Result<()> {
        let _ = self.stop_tx.send(true);
        Ok(())
    }
}

/// Routes of the display server.
fn router(tx: broadcast::Sender<String>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/ws", get(upgrade))
        .with_state(tx)
}

/// Serve the embedded page.
async fn page() -> Html<&'static str> {
    Html(PAGE)
}

/// Upgrade a screen connection.
async fn upgrade(
    ws: WebSocketUpgrade,
    State(tx): State<broadcast::Sender<String>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| serve_client(socket, tx))
}

/// Feed one connected screen until it goes away.
async fn serve_client(mut socket: WebSocket, tx: broadcast::Sender<String>) {
    let mut rx = tx.subscribe();

    loop {
        tokio::select! {
            message = rx.recv() => match message {
                Ok(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                // Экран отстал: показываем свежие сообщения, а не очередь.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}

/// Run the display server until it is stopped.
fn spawn_server(
    listener: std::net::TcpListener,
    tx: broadcast::Sender<String>,
    mut stop: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!("экран перевода: слушающий сокет не принят tokio: {error}");
                return;
            }
        };

        let shutdown = async move {
            while stop.changed().await.is_ok() {
                if *stop.borrow() {
                    break;
                }
            }
        };

        if let Err(error) = axum::serve(listener, router(tx))
            .with_graceful_shutdown(shutdown)
            .await
        {
            tracing::error!("экран перевода: сервер остановлен с ошибкой: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{LangPair, Language, LineId, LineState};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn translated(text: &str, is_final: bool, seq: u64) -> TranslationEvent {
        TranslationEvent::Translated {
            line: LineId(0),
            langs: LangPair {
                source: None,
                target: Language::code("ru"),
            },
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
    fn rejects_unusable_listen_address() {
        let err = WebsocketSink::new("screen", "не-адрес").unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn occupies_the_requested_address() {
        let sink = WebsocketSink::new("screen", "127.0.0.1:0").expect("адрес занят");
        assert!(sink.addr().ip().is_loopback());
        assert_ne!(sink.addr().port(), 0, "порт назначает система");
        assert_eq!(sink.id(), "screen");
    }

    #[test]
    fn message_json_contains_required_fields() {
        let msg = BroadcastMessage::from_event(&translated("привет", true, 5), None)
            .expect("сообщение перевода");
        let v: serde_json::Value = serde_json::from_str(&msg.to_json()).expect("разбор JSON");

        assert_eq!(v["text"], "привет");
        assert_eq!(v["is_final"], true);
        assert_eq!(v["seq"], 5);
        assert_eq!(v["line"], 0);
        assert!(v.get("source_text").is_some(), "поле присутствует: {v}");
    }

    #[test]
    fn state_message_carries_the_state_name() {
        let event = TranslationEvent::Status {
            line: LineId(2),
            state: LineState::Failed,
            at: Instant::now(),
        };
        let msg = BroadcastMessage::from_event(&event, None).expect("сообщение состояния");

        assert_eq!(msg.to_json(), r#"{"line":2,"state":"failed"}"#);
    }

    #[test]
    fn source_and_audio_events_are_not_broadcast() {
        assert!(BroadcastMessage::from_event(&source_text("hello"), None).is_none());

        let audio = TranslationEvent::TranslatedAudio {
            line: LineId(0),
            audio: test_audio(),
            at: Instant::now(),
        };
        assert!(BroadcastMessage::from_event(&audio, None).is_none());
    }

    fn test_audio() -> transono::audio::Audio {
        use transono::audio::{Audio, PcmAudio};
        use transono::providers::openai::translation::OpenAITranslationConfig;

        let spec = OpenAITranslationConfig::default()
            .audio_format()
            .spec()
            .clone();
        Audio::from_pcm(&PcmAudio::new(spec, 240)).expect("тестовый фрагмент")
    }

    /// Read whatever arrives, until the deadline.
    async fn read_some(stream: &mut TcpStream, buffer: &mut Vec<u8>) {
        let mut chunk = [0u8; 4096];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .expect("таймаут чтения")
            .expect("чтение из сокета");
        buffer.extend_from_slice(&chunk[..read]);
    }

    /// Connect a raw WebSocket client and complete the handshake.
    ///
    /// Returns the stream together with the bytes that arrived after the
    /// handshake headers.
    async fn connect_screen(addr: SocketAddr) -> (TcpStream, Vec<u8>) {
        let mut stream = TcpStream::connect(addr)
            .await
            .expect("подключение к экрану");
        let request = format!(
            "GET /ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("рукопожатие отправлено");

        let mut buffer = Vec::new();
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            read_some(&mut stream, &mut buffer).await;
        }

        let headers_end = buffer
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("конец заголовков")
            + 4;
        let headers = String::from_utf8_lossy(&buffer[..headers_end]).to_string();
        assert!(
            headers.starts_with("HTTP/1.1 101"),
            "ожидалось переключение протокола, получено: {headers}"
        );

        // Даём серверу подписаться на рассылку до первого события.
        tokio::time::sleep(Duration::from_millis(200)).await;

        (stream, buffer[headers_end..].to_vec())
    }

    /// Read one text frame payload, starting from the bytes already read.
    async fn read_frame(stream: &mut TcpStream, mut buffer: Vec<u8>) -> String {
        while buffer.len() < 2 {
            read_some(stream, &mut buffer).await;
        }
        assert_eq!(buffer[0], 0x81, "ожидался текстовый кадр");

        let length = (buffer[1] & 0x7f) as usize;
        assert!(length < 126, "тестовое сообщение короткое");

        while buffer.len() < 2 + length {
            read_some(stream, &mut buffer).await;
        }

        String::from_utf8(buffer[2..2 + length].to_vec()).expect("кадр в UTF-8")
    }

    #[tokio::test]
    async fn serves_the_page_over_http() {
        let sink = WebsocketSink::new("screen", "127.0.0.1:0").expect("адрес занят");
        let addr = sink.addr();

        let mut stream = TcpStream::connect(addr)
            .await
            .expect("подключение к серверу");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("запрос отправлен");

        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .await
            .expect("ответ прочитан");

        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("Экран перевода"), "страница отдана целиком");
        assert!(answer.contains("ws"), "страница содержит подключение");
    }

    #[tokio::test]
    async fn broadcasts_translation_to_a_connected_screen() {
        let mut sink = WebsocketSink::new("screen", "127.0.0.1:0").expect("адрес занят");
        let (mut screen, rest) = connect_screen(sink.addr()).await;

        sink.on_event(&translated("привет мир", true, 7))
            .await
            .expect("событие принято");

        let payload = read_frame(&mut screen, rest).await;
        let v: serde_json::Value = serde_json::from_str(&payload).expect("разбор JSON");

        assert_eq!(v["text"], "привет мир");
        assert_eq!(v["is_final"], true);
        assert_eq!(v["seq"], 7);
        assert_eq!(sink.sent(), 1);
    }

    #[tokio::test]
    async fn source_text_is_attached_to_the_translation() {
        let mut sink = WebsocketSink::new("screen", "127.0.0.1:0").expect("адрес занят");
        let (mut screen, rest) = connect_screen(sink.addr()).await;

        sink.on_event(&source_text("hello world"))
            .await
            .expect("исходный текст принят");
        sink.on_event(&translated("привет мир", false, 3))
            .await
            .expect("событие принято");

        let payload = read_frame(&mut screen, rest).await;
        let v: serde_json::Value = serde_json::from_str(&payload).expect("разбор JSON");

        assert_eq!(v["source_text"], "hello world");
        assert_eq!(v["is_final"], false);
        assert_eq!(sink.last_source(), "hello world");
    }

    #[tokio::test]
    async fn shutdown_stops_the_server() {
        let sink = WebsocketSink::new("screen", "127.0.0.1:0").expect("адрес занят");
        let addr = sink.addr();

        Box::new(sink).shutdown().await.expect("остановка принята");
        tokio::time::sleep(Duration::from_millis(200)).await;

        let refused = TcpStream::connect(addr).await;
        assert!(refused.is_err(), "сервер обязан освободить адрес");
    }
}
