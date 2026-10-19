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

/// Display settings handed to the page.
///
/// The page must not change the font size while a translation runs, so the size
/// comes from the configuration: it is fixed for the whole session and only
/// shrinks (down to [`ScreenSettings::min_line_size_vw`]) when a long phrase
/// would not fit the screen otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenSettings {
    /// Translation font size, in percent of the screen width.
    pub line_size_vw: f64,
    /// Lower bound the font shrinks to when a phrase does not fit.
    pub min_line_size_vw: f64,
    /// Poster shown while nothing is translated; `None` without an image.
    pub poster: Option<Poster>,
}

/// The page the screen shows when there is nothing to translate.
///
/// A picture in the middle with a line of text above and below it: the screen
/// keeps something on it during the pauses instead of a black rectangle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Poster {
    /// Text above the image.
    pub text_top: String,
    /// Text below the image.
    pub text_bottom: String,
    /// Seconds without a new translation before the poster appears; zero shows
    /// it as soon as the screen has nothing newer to display.
    pub timeout_secs: u64,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            line_size_vw: 4.5,
            min_line_size_vw: 2.0,
            poster: None,
        }
    }
}

impl ScreenSettings {
    /// JavaScript handing the settings to the page.
    ///
    /// The page reads `window.TRANSLATOR_SETTINGS`; when the script is not
    /// served (the page is opened as a file), the page falls back to the same
    /// values.
    pub fn to_script(&self) -> String {
        let mut settings = serde_json::json!({
            "line_size_vw": self.line_size_vw,
            "min_line_size_vw": self.min_line_size_vw,
        });

        if let Some(poster) = &self.poster {
            settings["poster"] = serde_json::json!({
                "url": POSTER_URL,
                "text_top": poster.text_top,
                "text_bottom": poster.text_bottom,
                "timeout": poster.timeout_secs,
            });
        }

        format!("window.TRANSLATOR_SETTINGS = {settings};\n")
    }
}

/// Path the page asks the poster image from.
const POSTER_URL: &str = "/poster";

/// Shared state of the display server.
#[derive(Clone)]
struct ScreenState {
    /// Translations broadcast to the connected screens.
    tx: broadcast::Sender<String>,
    /// Settings the page asks for.
    settings: ScreenSettings,
    /// Poster image, read on every request.
    image: Option<std::path::PathBuf>,
}

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
    settings: ScreenSettings,
    /// Poster image read from disk on every request, so it can be replaced
    /// while the screen is up.
    image: Option<std::path::PathBuf>,
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
    pub fn new(
        id: &str,
        bind: &str,
        settings: ScreenSettings,
        image: Option<std::path::PathBuf>,
    ) -> Result<Self> {
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
            settings,
            image,
            listener: Some(listener),
            stop_tx,
            last_source: String::new(),
            sent: 0,
        };
        sink.start_if_possible();

        Ok(sink)
    }

    /// Display settings handed to the page.
    pub fn settings(&self) -> ScreenSettings {
        self.settings.clone()
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
            Ok(_) => spawn_server(
                listener,
                self.tx.clone(),
                self.settings.clone(),
                self.image.clone(),
                self.stop_tx.subscribe(),
            ),
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
fn router(state: ScreenState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/settings.js", get(settings_script))
        .route("/poster", get(poster_image))
        .route("/ws", get(upgrade))
        .with_state(state)
}

/// Serve the embedded page.
async fn page() -> Html<&'static str> {
    Html(PAGE)
}

/// Serve the poster image the page shows while nothing is translated.
///
/// The file is read on every request, so the picture can be replaced while the
/// screen is up; a missing or unreadable file answers with a plain error and the
/// page keeps the black background.
async fn poster_image(State(state): State<ScreenState>) -> axum::response::Response {
    use axum::response::IntoResponse;

    let Some(path) = state.image else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            "картинка не задана в конфигурации".to_string(),
        )
            .into_response();
    };

    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (axum::http::header::CONTENT_TYPE, image_content_type(&path)),
                (axum::http::header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(error) => {
            tracing::warn!(
                "экран перевода: картинка {} не прочитана: {error}",
                path.display()
            );
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                [(
                    axum::http::header::CONTENT_TYPE,
                    "text/plain; charset=utf-8",
                )],
                format!("картинка не прочитана: {error}"),
            )
                .into_response()
        }
    }
}

/// Content type of the poster image, taken from its extension.
fn image_content_type(path: &std::path::Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Serve the display settings as a script the page loads.
async fn settings_script(State(state): State<ScreenState>) -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        state.settings.to_script(),
    )
}

/// Upgrade a screen connection.
async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<ScreenState>,
) -> impl axum::response::IntoResponse {
    let tx = state.tx.clone();
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
    settings: ScreenSettings,
    image: Option<std::path::PathBuf>,
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

        if let Err(error) = axum::serve(
            listener,
            router(ScreenState {
                tx,
                settings,
                image,
            }),
        )
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
        let err =
            WebsocketSink::new("screen", "не-адрес", ScreenSettings::default(), None).unwrap_err();
        assert!(matches!(err, SinkError::Protocol(_)), "{err:?}");
    }

    #[test]
    fn occupies_the_requested_address() {
        let sink = WebsocketSink::new("screen", "127.0.0.1:0", ScreenSettings::default(), None)
            .expect("адрес занят");
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
        let sink = WebsocketSink::new("screen", "127.0.0.1:0", ScreenSettings::default(), None)
            .expect("адрес занят");
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
        assert!(
            answer.contains("/settings.js"),
            "страница спрашивает настройки показа"
        );
    }

    #[tokio::test]
    async fn serves_the_display_settings_from_the_configuration() {
        let settings = ScreenSettings {
            line_size_vw: 7.5,
            min_line_size_vw: 3.0,
            poster: None,
        };
        let sink =
            WebsocketSink::new("screen", "127.0.0.1:0", settings, None).expect("адрес занят");
        let addr = sink.addr();

        let mut stream = TcpStream::connect(addr)
            .await
            .expect("подключение к серверу");
        stream
            .write_all(b"GET /settings.js HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("запрос отправлен");

        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .await
            .expect("ответ прочитан");

        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(
            answer.contains("javascript"),
            "скрипт отдаётся с типом JavaScript: {answer}"
        );
        assert!(
            answer.contains("\"line_size_vw\":7.5"),
            "размер из настройки: {answer}"
        );
        assert!(
            answer.contains("\"min_line_size_vw\":3"),
            "предел ужимания из настройки: {answer}"
        );
    }

    #[tokio::test]
    async fn serves_the_poster_from_the_configured_image() {
        let dir = std::env::temp_dir().join(format!("ait-poster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("каталог");
        let image = dir.join("stand.png");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\n-not-really-a-png").expect("картинка");

        let settings = ScreenSettings {
            line_size_vw: 4.5,
            min_line_size_vw: 2.0,
            poster: Some(Poster {
                text_top: "MTL Fest".to_string(),
                text_bottom: "english translation".to_string(),
                timeout_secs: 5,
            }),
        };
        let sink = WebsocketSink::new("screen", "127.0.0.1:0", settings, Some(image.clone()))
            .expect("адрес занят");
        let addr = sink.addr();

        let script = http_get(addr, "/settings.js").await;
        assert!(
            script.contains("\"poster\""),
            "заставка объявлена в настройках: {script}"
        );
        assert!(
            script.contains("\"url\":\"/poster\""),
            "адрес картинки: {script}"
        );
        assert!(script.contains("\"text_top\":\"MTL Fest\""), "{script}");
        assert!(
            script.contains("\"text_bottom\":\"english translation\""),
            "{script}"
        );
        assert!(script.contains("\"timeout\":5"), "{script}");

        let answer = http_get(addr, "/poster").await;
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("image/png"), "тип из расширения: {answer}");
        assert!(
            answer.contains("no-store"),
            "картинку не кэшируем: {answer}"
        );
        assert!(
            answer.contains("-not-really-a-png"),
            "тело картинки: {answer}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn the_poster_is_absent_without_an_image() {
        let script = ScreenSettings::default().to_script();

        assert!(
            !script.contains("poster"),
            "без картинки заставки нет: {script}"
        );
        assert!(script.contains("\"line_size_vw\":4.5"), "{script}");
    }

    #[test]
    fn the_image_type_follows_the_extension() {
        use std::path::Path;

        assert_eq!(image_content_type(Path::new("a.PNG")), "image/png");
        assert_eq!(image_content_type(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(image_content_type(Path::new("a.webp")), "image/webp");
        assert_eq!(
            image_content_type(Path::new("a.unknown")),
            "application/octet-stream"
        );
    }

    /// GET one path and read the whole answer as text.
    ///
    /// The poster test asks for a binary image, so the body is decoded lossily:
    /// the assertions only look at the headers and at the readable part.
    async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
        let mut stream = TcpStream::connect(addr).await.expect("подключение");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("запрос отправлен");

        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .await
            .expect("ответ прочитан");
        String::from_utf8_lossy(&answer).into_owned()
    }

    #[tokio::test]
    async fn broadcasts_translation_to_a_connected_screen() {
        let mut sink = WebsocketSink::new("screen", "127.0.0.1:0", ScreenSettings::default(), None)
            .expect("адрес занят");
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
        let mut sink = WebsocketSink::new("screen", "127.0.0.1:0", ScreenSettings::default(), None)
            .expect("адрес занят");
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
        let sink = WebsocketSink::new("screen", "127.0.0.1:0", ScreenSettings::default(), None)
            .expect("адрес занят");
        let addr = sink.addr();

        Box::new(sink).shutdown().await.expect("остановка принята");
        tokio::time::sleep(Duration::from_millis(200)).await;

        let refused = TcpStream::connect(addr).await;
        assert!(refused.is_err(), "сервер обязан освободить адрес");
    }
}
