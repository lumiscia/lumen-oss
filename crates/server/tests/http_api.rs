#![cfg(feature = "cli")]

//! Drives the real `lumen-server` binary over HTTP. Everything here stops before the GPU except the
//! `*_with_gpu` tests, which skip like the engine's GPU tests when no adapter is available.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

use lumen_ffmpeg::{
    CpuVideoFrame, DecodeMode, DecodedVideoFrame, InputContext, MuxedEncoder, PixelFormat,
    VideoDecoder, VideoDecoderConfig, VideoEncoderConfig, YuvVideoFrame,
};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde_json::{Value, json};
use tungstenite::{
    Message, WebSocket, client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream,
};

const TOKEN: &str = "test-token";
const TIMEOUT: Duration = Duration::from_secs(120);
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xef, 0xa0, 0xf0,
    0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd, 0x40, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Server {
    base: String,
    child: Child,
    http: Client,
    log: Arc<Mutex<String>>,
    media_dir: Arc<tempfile::TempDir>,
}

impl Server {
    fn start(args: &[&str]) -> Self {
        Self::start_with(args, |_| {})
    }

    fn start_with(args: &[&str], configure: impl FnOnce(&mut Command)) -> Self {
        let media_dir = Arc::new(tempfile::tempdir().expect("media directory"));
        Self::start_in(media_dir, args, configure)
    }

    /// Stops this server and starts another on the same media directory.
    fn restart(self, args: &[&str]) -> Self {
        let media_dir = Arc::clone(&self.media_dir);
        drop(self);
        Self::start_in(media_dir, args, |_| {})
    }

    fn start_in(
        media_dir: Arc<tempfile::TempDir>,
        args: &[&str],
        configure: impl FnOnce(&mut Command),
    ) -> Self {
        let mut command = server_command();
        command
            .args(["--bind", "127.0.0.1:0", "--token", TOKEN])
            .args(args)
            .env("LUMEN_SERVER_MEDIA_DIR", media_dir.path())
            .env("RUST_LOG", "lumen_server=info")
            .env("NO_COLOR", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        configure(&mut command);
        let mut child = command.spawn().expect("spawn lumen-server");
        let mut stderr = BufReader::new(child.stderr.take().expect("server stderr"));
        let address = loop {
            let mut line = String::new();
            let read = stderr.read_line(&mut line).expect("read server stderr");
            assert_ne!(read, 0, "lumen-server exited before listening");
            eprint!("{line}");
            if let Some((_, address)) = line.split_once("listening address=") {
                break address.trim().to_string();
            }
        };
        // The server blocks on a full pipe, and printing keeps its log with a failing test's output.
        let log = Arc::new(Mutex::new(String::new()));
        let written = Arc::clone(&log);
        thread::spawn(move || {
            for line in stderr.lines().map_while(Result::ok) {
                eprintln!("{line}");
                let mut written = written.lock().unwrap();
                written.push_str(&line);
                written.push('\n');
            }
        });
        Self {
            base: format!("http://{address}"),
            child,
            http: Client::new(),
            log,
            media_dir,
        }
    }

    fn get(&self, path: &str) -> Response {
        self.http
            .get(format!("{}{path}", self.base))
            .bearer_auth(TOKEN)
            .send()
            .expect("GET request")
    }

    fn post(&self, body: &Value) -> Response {
        self.http
            .post(format!("{}/renders", self.base))
            .bearer_auth(TOKEN)
            .json(body)
            .send()
            .expect("POST /renders")
    }

    fn post_to(&self, path: &str, body: &Value) -> Response {
        self.http
            .post(format!("{}{path}", self.base))
            .bearer_auth(TOKEN)
            .json(body)
            .send()
            .expect("POST request")
    }

    fn put_bytes(&self, path: &str, bytes: &[u8]) -> Response {
        self.http
            .put(format!("{}{path}", self.base))
            .bearer_auth(TOKEN)
            .body(bytes.to_vec())
            .send()
            .expect("PUT request")
    }

    fn check(&self, body: &Value) -> Value {
        let response = self.post_to("/renders/check", body);
        assert_eq!(response.status(), StatusCode::OK);
        response.json().expect("check body")
    }

    fn submit(&self, body: &Value) -> String {
        let response = self.post(body);
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: Value = response.json().expect("create response");
        body["render"]["id"]
            .as_str()
            .expect("render id")
            .to_string()
    }

    fn progress(&self, id: &str) -> Value {
        let body: Value = self
            .get(&format!("/renders/{id}/progress"))
            .json()
            .expect("progress response");
        body["progress"].clone()
    }

    fn wait_for_state(&self, id: &str, accept: &[&str]) -> Value {
        let started = Instant::now();
        loop {
            let progress = self.progress(id);
            if accept.contains(&progress["state"].as_str().unwrap_or_default()) {
                return progress;
            }
            assert!(
                started.elapsed() < TIMEOUT,
                "render {id} never reached {accept:?}: {progress}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumen-server"));
    for name in [
        "LUMEN_FONT_DIR",
        "LUMEN_QUALITY_CONFIG",
        "LUMEN_SERVER_ALLOW_MEDIA_HOSTS",
        "LUMEN_SERVER_BIND",
        "LUMEN_SERVER_MEDIA_DIR",
        "LUMEN_SERVER_MEDIA_MAX_BYTES",
        "LUMEN_SERVER_MEDIA_TTL",
        "LUMEN_SERVER_TOKEN",
    ] {
        command.env_remove(name);
    }
    command
}

/// A grayscale baseline JPEG of flat blocks, big enough that FFmpeg's probe picks `image2` for a `.jpg`.
fn normal_sized_jpeg() -> Vec<u8> {
    let (width, height) = (1280_u16, 720_u16);
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x43, 0x00];
    jpeg.extend([1; 64]);
    jpeg.extend([0xff, 0xc0, 0x00, 0x0b, 0x08]);
    jpeg.extend(height.to_be_bytes());
    jpeg.extend(width.to_be_bytes());
    jpeg.extend([0x01, 0x01, 0x11, 0x00]);
    // DC and AC tables holding one 1-bit code, for symbol 0.
    for class in [0x00, 0x10] {
        jpeg.extend([0xff, 0xc4, 0x00, 0x14, class, 0x01]);
        jpeg.extend([0; 16]);
    }
    jpeg.extend([0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]);
    // Each block is "no DC change" and "end of block": two zero bits.
    let blocks = usize::from(width.div_ceil(8)) * usize::from(height.div_ceil(8));
    jpeg.resize(jpeg.len() + (blocks * 2).div_ceil(8), 0);
    jpeg.extend([0xff, 0xd9]);
    jpeg
}

/// Serves `/image.png`, `/photo.jpg`, a redirect to the literal loopback address, `/held`, which
/// answers only once the test releases it, and `/stream.m3u8` with the given playlist.
struct MediaServer {
    port: u16,
    requests: Arc<AtomicUsize>,
    release: Sender<()>,
}

impl MediaServer {
    fn start() -> Self {
        Self::start_with_playlist(String::new())
    }

    fn start_with_playlist(playlist: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind media server");
        let port = listener.local_addr().expect("media server addr").port();
        let requests = Arc::new(AtomicUsize::new(0));
        let (release, released) = channel();
        let released = Arc::new(Mutex::new(released));
        let playlist = Arc::new(playlist);
        let request_count = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                request_count.fetch_add(1, Ordering::SeqCst);
                let released = Arc::clone(&released);
                let playlist = Arc::clone(&playlist);
                thread::spawn(move || serve_media(stream, port, &released, &playlist));
            }
        });
        Self {
            port,
            requests,
            release,
        }
    }

    fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }
}

fn serve_media(mut stream: TcpStream, port: u16, released: &Mutex<Receiver<()>>, playlist: &str) {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let path = request.split_whitespace().nth(1).unwrap_or_default();
    if path.starts_with("/redirect") {
        let _ = write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/image.png\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    if path.starts_with("/held") {
        let _ = released.lock().expect("release lock").recv();
    }
    let photo;
    let body = if path.starts_with("/stream.m3u8") {
        playlist.as_bytes()
    } else if path.starts_with("/photo.jpg") {
        photo = normal_sized_jpeg();
        &photo
    } else if path.starts_with("/corrupt") {
        b"not media"
    } else {
        TINY_PNG
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(body);
}

fn solid_composition() -> Value {
    json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 16, "height": 16 } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    })
}

fn image_composition(source: &str) -> Value {
    media_composition("image", source)
}

fn media_composition(kind: &str, source: &str) -> Value {
    json!({
        "timeline": { "fps": 24, "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 1, "type": "media_in", "params": { "kind": kind, "source": source } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    })
}

fn error_code(response: Response) -> String {
    let body: Value = response.json().expect("error body");
    body["code"].as_str().unwrap_or_default().to_string()
}

#[test]
fn auth_runs_before_parsing_on_every_route_but_health() {
    let server = Server::start(&[]);
    let url = |path: &str| format!("{}{path}", server.base);

    for (path, token) in ["/renders", "/renders/check"]
        .into_iter()
        .flat_map(|path| [(path, None), (path, Some("wrong"))])
    {
        let mut malformed = server
            .http
            .post(url(path))
            .header("content-type", "application/json")
            .body("{");
        if let Some(token) = token {
            malformed = malformed.bearer_auth(token);
        }
        assert_eq!(
            malformed.send().unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    for path in [
        "/renders/x",
        "/renders/x/progress",
        "/renders/x/socket",
        "/renders/x/artifact",
    ] {
        assert_eq!(
            server.http.get(url(path)).send().unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }

    assert_eq!(
        server.http.get(url("/health")).send().unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(server.get("/renders/x").status(), StatusCode::NOT_FOUND);
}

#[test]
fn refuses_a_non_loopback_bind_without_a_token() {
    for args in [
        &["--bind", "0.0.0.0:0"][..],
        &["--bind", "0.0.0.0:0", "--token", ""],
        &["--bind", "[::]:0"],
    ] {
        let output = server_command()
            .args(args)
            .output()
            .expect("run lumen-server");
        assert!(
            !output.status.success(),
            "{args:?} should exit with an error"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("without a token"), "{args:?}: {stderr}");
    }
}

#[test]
fn errors_answer_with_the_code_and_message_the_sdk_reads() {
    let server = Server::start(&[]);
    let body = |response: Response| response.json::<Value>().expect("error body");
    let bytes = |payload: Vec<u8>| {
        server
            .http
            .post(format!("{}/renders", server.base))
            .bearer_auth(TOKEN)
            .header("content-type", "application/json")
            .body(payload)
            .send()
            .expect("request")
    };

    assert_eq!(
        body(
            server
                .http
                .get(format!("{}/renders/nope", server.base))
                .bearer_auth(TOKEN)
                .send()
                .expect("request")
        ),
        json!({ "code": "render_not_found", "error": "render was not found" })
    );

    let malformed = bytes(b"{".to_vec());
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(malformed), "invalid_request_body");

    let too_large = bytes(vec![b' '; 2 * 1024 * 1024 + 1]);
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body(too_large),
        json!({
            "code": "request_too_large",
            "error": "request body exceeds the 2097152 byte limit",
        })
    );
}

#[test]
fn a_failed_render_keeps_its_code_on_every_read() {
    let server = Server::start(&[]);
    let id = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": "http://127.0.0.1:1/image.png" },
    }));
    let progress = server.wait_for_state(&id, &["failed"]);
    assert_eq!(progress["errorCode"], "media_download_forbidden_host");
    assert_eq!(progress["retryable"], false);

    let read = |path: String| {
        server
            .http
            .get(format!("{}{path}", server.base))
            .bearer_auth(TOKEN)
            .send()
            .expect("request")
            .json::<Value>()
            .expect("render body")
    };
    let render = read(format!("/renders/{id}"));
    let expected = json!({
        "code": "media_download_forbidden_host",
        "message": progress["error"],
        "retryable": false,
    });
    assert_eq!(render["render"]["error"], expected);
    assert_eq!(
        read(format!("/renders/{id}/progress"))["render"]["error"],
        expected
    );

    let artifact = server.get(&format!("/renders/{id}/artifact"));
    assert_eq!(artifact.status(), StatusCode::CONFLICT);
    assert_eq!(
        artifact.json::<Value>().expect("artifact error"),
        json!({ "code": "media_download_forbidden_host", "error": progress["error"] })
    );
}

#[test]
fn a_render_has_no_artifact_before_it_succeeds() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let held = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": media.url("localhost", "/held") },
    }));
    server.wait_for_state(&held, &["processing"]);
    let waiting = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": "http://127.0.0.1/forbidden.png" },
    }));

    for (id, status) in [(&held, "processing"), (&waiting, "queued")] {
        let artifact = server.get(&format!("/renders/{id}/artifact"));
        assert_eq!(artifact.status(), StatusCode::CONFLICT, "{status}");
        assert_eq!(error_code(artifact), "artifact_not_ready", "{status}");
        let render: Value = server
            .get(&format!("/renders/{id}"))
            .json()
            .expect("render body");
        assert_eq!(render["render"]["status"], status);
        assert_eq!(render["render"]["outputUrl"], Value::Null, "{status}");
    }
}

fn open_socket(server: &Server, id: &str) -> WebSocket<MaybeTlsStream<TcpStream>> {
    let url = server.base.replacen("http", "ws", 1);
    let mut request = format!("{url}/renders/{id}/socket")
        .into_client_request()
        .expect("socket request");
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
    );
    let (socket, _) = tungstenite::connect(request).expect("open render socket");
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream
            .set_read_timeout(Some(TIMEOUT))
            .expect("socket read timeout");
    }
    socket
}

/// Reads events until the server closes the socket.
fn socket_events(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Vec<Value> {
    let mut events = Vec::new();
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => events.push(serde_json::from_str(&text).expect("event")),
            Ok(Message::Close(_))
            | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return events;
            }
            Ok(_) => {}
            Err(error) => panic!("socket failed after {events:?}: {error}"),
        }
    }
}

#[test]
fn sockets_send_the_current_status_then_changes_and_close_when_the_render_ends() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let id = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": media.url("localhost", "/held") },
    }));
    server.wait_for_state(&id, &["processing"]);

    let mut socket = open_socket(&server, &id);
    let Message::Text(first) = socket.read().expect("current status") else {
        panic!("the current status should be a text frame");
    };
    let first: Value = serde_json::from_str(&first).expect("event");
    assert_eq!(first["type"], "render.started", "{first}");
    assert_eq!(first["renderId"], id.as_str());

    media.release.send(()).expect("release held download");
    let events = socket_events(&mut socket);

    let last = events.last().unwrap();
    assert!(
        ["render.completed", "render.failed"].contains(&last["type"].as_str().unwrap()),
        "{events:?}"
    );
    assert!(
        events[..events.len() - 1]
            .iter()
            .all(|event| event["type"] == "render.progress"),
        "{events:?}"
    );
    assert!(events.iter().all(|event| event["renderId"] == id.as_str()));

    let progress = server.progress(&id);
    if last["type"] == "render.completed" {
        assert_eq!(last["url"], format!("/renders/{id}/artifact"));
        assert_eq!(progress["artifactUrl"], last["url"]);
    } else {
        assert_eq!(last["error"]["code"], progress["errorCode"]);
    }

    let late = socket_events(&mut open_socket(&server, &id));
    assert_eq!(
        late.as_slice(),
        std::slice::from_ref(last),
        "a late subscriber gets the ended status"
    );
}

#[test]
fn errors_axum_answers_itself_carry_the_same_body() {
    let server = Server::start(&[]);
    for (path, status, code) in [
        ("/missing", StatusCode::NOT_FOUND, "route_not_found"),
        (
            "/renders",
            StatusCode::METHOD_NOT_ALLOWED,
            "method_not_allowed",
        ),
        (
            "/renders/%FF/artifact",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "/renders/nope/socket",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
    ] {
        let response = server.get(path);
        assert_eq!(response.status(), status, "{path}");
        if status == StatusCode::METHOD_NOT_ALLOWED {
            assert_eq!(response.headers()["allow"], "POST", "{path}");
        }
        let body: Value = response.json().expect("error body");
        assert_eq!(body["code"], code, "{path}");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "{path}: {body}"
        );
    }
}

fn codes_and_paths(diagnostics: &Value) -> Vec<(String, String)> {
    diagnostics
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|diagnostic| {
            (
                diagnostic["code"].as_str().unwrap().to_string(),
                diagnostic["path"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn checks_say_what_is_wrong_and_invalid_renders_carry_the_same_diagnostics() {
    let server = Server::start(&[]);
    let check = |body: &Value| -> Value {
        let response = server
            .http
            .post(format!("{}/renders/check", server.base))
            .bearer_auth(TOKEN)
            .json(body)
            .send()
            .expect("POST /renders/check");
        assert_eq!(response.status(), StatusCode::OK);
        response.json().expect("check body")
    };

    let mut invalid = solid_composition();
    // The trailing space is part of the key, so it stays in the pointer.
    invalid["nodes"][0]["params"]["paintt "] = json!("#fff");
    invalid["nodes"][1]["type"] = json!("sparkle");
    let checked = check(&json!({ "composition": invalid }));
    assert_eq!(checked["ok"], json!(false));
    let expected = [
        ("node.unknown_param", "/nodes/0/params/paintt "),
        ("node.unknown_type", "/nodes/1/type"),
    ]
    .map(|(code, path)| (code.to_string(), path.to_string()));
    assert_eq!(codes_and_paths(&checked["diagnostics"]), expected);
    assert_eq!(checked["diagnostics"][0]["severity"], json!("error"));
    assert_eq!(checked["diagnostics"][0]["node"], json!(1));

    let rejected = server.post(&json!({ "composition": invalid }));
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let rejected: Value = rejected.json().expect("error body");
    assert_eq!(rejected["code"], json!("invalid_project_payload"));
    assert_eq!(codes_and_paths(&rejected["diagnostics"]), expected);
    assert_eq!(rejected["diagnostics"], checked["diagnostics"]);

    let mut text = solid_composition();
    text["nodes"][0] = json!({ "id": 1, "type": "text", "params": { "content": [
        { "text": "a", "font_family": "No Such Family 272" },
        { "text": "b", "font_family": "brand.ttf" }
    ] } });
    let checked = check(&json!({
        "composition": text,
        "media": { "brand.ttf": "https://example.com/brand.ttf" },
        "quality": "ignored by a check",
    }));
    assert_eq!(checked["ok"], json!(true));
    assert_eq!(
        codes_and_paths(&checked["diagnostics"]),
        [(
            "font.fallback".to_string(),
            "/nodes/0/params/content/0/font_family".to_string()
        )]
    );
    assert_eq!(checked["diagnostics"][0]["severity"], json!("warning"));
}

#[test]
fn rejects_bad_requests_before_downloading_anything() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let hero = media.url("localhost", "/image.png");

    let invalid_composition = server.post(&json!({
        "composition": { "timeline": "nope" },
        "media": { "hero": hero },
    }));
    assert_eq!(
        invalid_composition.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(error_code(invalid_composition), "invalid_project_payload");

    let invalid_alias = server.post(&json!({
        "composition": solid_composition(),
        "media": { "..": hero },
    }));
    assert_eq!(invalid_alias.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(invalid_alias), "invalid_media_alias");

    let webhook = server.post(&json!({
        "composition": solid_composition(),
        "media": { "hero": hero },
        "webhookUrl": "https://example.com/hook",
    }));
    assert_eq!(webhook.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(webhook), "webhooks_unsupported");

    assert_eq!(
        server
            .post(&json!({ "project": solid_composition() }))
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(media.requests.load(Ordering::SeqCst), 0);
}

fn invalid_quality_message(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = response.json().expect("error body");
    assert_eq!(body["code"], "invalid_quality", "{body}");
    body["error"].as_str().unwrap_or_default().to_string()
}

#[test]
fn rejects_quality_this_host_cannot_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("quality.json");
    fs::write(
        &config,
        json!({
            "codecs": { "h264": { "encoders": ["libx264"] } },
            "presets": { "archive": { "encoders": { "libx264": { "crf": 16 } } } }
        })
        .to_string(),
    )
    .unwrap();
    let server = Server::start_with(&[], |command| {
        command.env("LUMEN_QUALITY_CONFIG", &config);
    });
    let post = |quality: Value, codec: Value, render_settings: Value| {
        let mut composition = solid_composition();
        composition["render_settings"] = render_settings;
        server.post(&json!({ "composition": composition, "quality": quality, "codec": codec }))
    };
    let canvas = json!({ "width": 16, "height": 16 });

    let unknown = invalid_quality_message(post(json!("cinema"), Value::Null, canvas.clone()));
    assert!(
        unknown.contains("`cinema`") && unknown.contains("draft, standard, high, lossless"),
        "{unknown}"
    );
    let no_encoder = invalid_quality_message(post(json!("lossless"), json!("av1"), canvas.clone()));
    assert!(no_encoder.contains("av1"), "{no_encoder}");
    let from_composition = invalid_quality_message(post(
        Value::Null,
        Value::Null,
        json!({ "width": 16, "height": 16, "quality": "cinema" }),
    ));
    assert!(from_composition.contains("`cinema`"), "{from_composition}");
    let composition_codec = invalid_quality_message(post(
        json!("lossless"),
        Value::Null,
        json!({ "width": 16, "height": 16, "codec": "av1" }),
    ));
    assert!(composition_codec.contains("av1"), "{composition_codec}");
    let bad_codec = invalid_quality_message(post(json!("high"), json!("vp9"), canvas.clone()));
    assert!(bad_codec.contains("vp9"), "{bad_codec}");

    assert_eq!(
        post(json!("archive"), json!("h264"), canvas.clone()).status(),
        StatusCode::ACCEPTED
    );
    // The request's preset wins over the composition's.
    assert_eq!(
        post(
            json!("archive"),
            Value::Null,
            json!({ "width": 16, "height": 16, "quality": "cinema" })
        )
        .status(),
        StatusCode::ACCEPTED
    );
}

#[test]
fn rejects_quality_the_pinned_encoder_cannot_serve() {
    let server = Server::start_with(&[], |command| {
        command.env("LUMEN_VIDEO_ENCODER", "libx264");
    });
    let post = |body: Value| {
        let mut body = body;
        body["composition"] = solid_composition();
        server.post(&body)
    };
    let codec = invalid_quality_message(post(json!({ "codec": "hevc" })));
    assert!(
        codec.contains("libx264") && codec.contains("hevc"),
        "{codec}"
    );
    assert_eq!(
        post(json!({ "quality": "lossless", "codec": "h264" })).status(),
        StatusCode::ACCEPTED
    );

    let vulkan = Server::start_with(&[], |command| {
        command.env("LUMEN_VIDEO_ENCODER", "h264_vulkan");
    });
    let lossless = invalid_quality_message(vulkan.post(&json!({
        "composition": solid_composition(),
        "quality": "lossless",
    })));
    assert!(lossless.contains("h264_vulkan"), "{lossless}");
}

#[test]
fn an_invalid_quality_config_stops_the_server_starting() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("quality.json");
    fs::write(
        &config,
        json!({ "presets": { "x": { "encoders": { "libx264": { "options": { "not_an_option": "1" } } } } } })
            .to_string(),
    )
    .unwrap();
    let output = server_command()
        .args(["--bind", "127.0.0.1:0"])
        .env("LUMEN_QUALITY_CONFIG", &config)
        .output()
        .expect("run lumen-server");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not_an_option"), "{stderr}");
}

#[test]
fn clients_cannot_choose_render_ids() {
    let server = Server::start(&[]);
    let body = json!({
        "job_id": "mine",
        "composition": solid_composition(),
        "media": { "hero": "http://127.0.0.1/forbidden.png" },
    });
    let first = server.submit(&body);
    let second = server.submit(&body);
    assert_ne!(first, second);
    assert!(first.starts_with("r_") && first != "mine", "{first}");
}

#[test]
fn renders_stay_queued_until_a_slot_frees() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let held = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": media.url("localhost", "/held") },
    }));
    server.wait_for_state(&held, &["processing"]);
    let started = Instant::now();
    while media.requests.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < TIMEOUT, "held download never started");
        thread::sleep(Duration::from_millis(20));
    }

    let waiting = server.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": "http://127.0.0.1/forbidden.png" },
    }));
    thread::sleep(Duration::from_millis(200));
    assert_eq!(server.progress(&waiting)["state"], "queued");

    media.release.send(()).expect("release held download");
    server.wait_for_state(&waiting, &["failed"]);
}

#[test]
fn oldest_finished_render_is_evicted() {
    let server = Server::start(&["--retained-renders", "1"]);
    let body = json!({
        "composition": solid_composition(),
        "media": { "hero": "http://127.0.0.1/forbidden.png" },
    });
    let oldest = server.submit(&body);
    server.wait_for_state(&oldest, &["failed"]);
    let newest = server.submit(&body);
    server.wait_for_state(&newest, &["failed"]);

    assert_eq!(
        server.get(&format!("/renders/{newest}")).status(),
        StatusCode::OK
    );
    for path in ["", "/progress", "/artifact"] {
        assert_eq!(
            server.get(&format!("/renders/{oldest}{path}")).status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let socket = server
        .http
        .get(format!("{}/renders/{oldest}/socket", server.base))
        .bearer_auth(TOKEN)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .expect("socket request");
    assert_eq!(socket.status(), StatusCode::NOT_FOUND);
}

#[test]
fn media_hosts_must_be_public_on_every_connection() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let failure = |url: String| {
        let id = server.submit(&json!({
            "composition": solid_composition(),
            "media": { "hero": url },
        }));
        let progress = server.wait_for_state(&id, &["failed", "succeeded"]);
        progress["error"].as_str().unwrap_or_default().to_string()
    };

    let literal = failure(media.url("127.0.0.1", "/image.png?signature=secret"));
    assert!(
        literal.starts_with("media 'hero' (127.0.0.1): host has no public address"),
        "{literal}"
    );

    let mapped = failure("http://[::ffff:169.254.169.254]/latest?signature=secret".to_string());
    assert!(mapped.contains("host has no public address"), "{mapped}");

    let redirected = failure(media.url("localhost", "/redirect?signature=secret"));
    assert!(
        redirected.contains("host has no public address"),
        "{redirected}"
    );

    for error in [literal, mapped, redirected] {
        assert!(!error.contains("secret"), "{error}");
    }
    assert_eq!(
        media.requests.load(Ordering::SeqCst),
        1,
        "only the allowed host is contacted"
    );

    let unlisted = Server::start(&[]);
    let id = unlisted.submit(&json!({
        "composition": solid_composition(),
        "media": { "hero": media.url("localhost", "/image.png") },
    }));
    let progress = unlisted.wait_for_state(&id, &["failed", "succeeded"]);
    let resolved = progress["error"].as_str().unwrap_or_default();
    assert!(
        resolved.contains("host has no public address"),
        "{resolved}"
    );
    assert_eq!(
        media.requests.load(Ordering::SeqCst),
        1,
        "the resolver refuses before connecting"
    );
}

/// Returns `None` when the machine has no usable GPU adapter.
fn render_outcome(server: &Server, body: &Value) -> Option<(String, Value)> {
    let id = server.submit(body);
    let progress = server.wait_for_state(&id, &["succeeded", "failed"]);
    let error = progress["error"].as_str().unwrap_or_default();
    if error.contains("failed to create GPU renderer") {
        eprintln!("skipping GPU-backed lumen-server test: {error}");
        return None;
    }
    Some((id, progress))
}

#[test]
fn corrupt_media_errors_do_not_reveal_where_it_is_staged_with_gpu() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let temp = std::env::temp_dir().canonicalize().expect("temp directory");
    for (kind, alias) in [("image", "hero.png"), ("video", "clip.mp4")] {
        let Some((_, progress)) = render_outcome(
            &server,
            &json!({
                "composition": media_composition(kind, alias),
                "media": { alias: media.url("localhost", "/corrupt") },
            }),
        ) else {
            return;
        };
        assert_eq!(progress["state"], "failed", "{alias}: {progress}");
        let error = progress["error"].as_str().unwrap_or_default();
        assert!(error.contains(alias), "{alias}: {error}");
        assert!(
            !error.contains(&*temp.to_string_lossy()),
            "{alias}: {error}"
        );
    }

    let Some((_, progress)) = render_outcome(
        &server,
        &json!({ "composition": media_composition("image", ".") }),
    ) else {
        return;
    };
    assert_eq!(progress["state"], "failed", "{progress}");
    let error = progress["error"].as_str().unwrap_or_default();
    assert!(!error.contains(&*temp.to_string_lossy()), "{error}");
}

#[test]
fn renders_read_only_manifest_media_with_gpu() {
    let fixtures = tempfile::tempdir().expect("fixture directory");
    let secret = fixtures.path().join("secret.png");
    std::fs::write(&secret, TINY_PNG).expect("write fixture");
    let media = MediaServer::start();
    let server = serving_from(fixtures.path(), &["--allow-media-host", "localhost"]);

    // Both fallbacks a render could use are pointed at the fixture: the working directory and the
    // font directory, so a source resolving through either one would render.
    for source in [
        "secret.png".to_string(),
        format!("file://{}", secret.display()),
        secret.display().to_string(),
    ] {
        let Some((_, progress)) = render_outcome(
            &server,
            &json!({ "composition": image_composition(&source) }),
        ) else {
            return;
        };
        assert_eq!(progress["state"], "failed", "{source} must not render");
        let error = progress["error"].as_str().unwrap_or_default();
        assert!(error.contains(&source), "{source}: {error}");
    }

    let Some((id, progress)) = render_outcome(
        &server,
        &json!({
            "composition": image_composition("hero"),
            "media": { "hero": media.url("localhost", "/image.png") },
        }),
    ) else {
        return;
    };
    assert_eq!(progress["state"], "succeeded", "{progress}");
    let artifact = server.get(&format!("/renders/{id}/artifact"));
    assert_eq!(artifact.status(), StatusCode::OK);
    assert!(!artifact.bytes().expect("artifact bytes").is_empty());
}

/// This size is below what hardware H.264 encoders accept, and software Vulkan has none, so the
/// default encoder falls back wherever this runs.
#[test]
fn the_default_encoder_falls_back_to_a_correct_video_with_gpu() {
    let server = Server::start_with(&[], |command| {
        command.env_remove("LUMEN_VIDEO_ENCODER");
    });
    let Some((id, progress)) =
        render_outcome(&server, &json!({ "composition": solid_composition() }))
    else {
        return;
    };
    assert_eq!(progress["state"], "succeeded", "{progress}");
    let artifact = server.get(&format!("/renders/{id}/artifact"));
    assert_eq!(artifact.status(), StatusCode::OK);

    // The render can finish before the thread reading the server's stderr has caught up.
    let started = Instant::now();
    let log = loop {
        let log = server.log.lock().unwrap().clone();
        if log.contains("opened video encoder") || started.elapsed() > Duration::from_secs(10) {
            break log;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let lines = |text: &str| {
        log.lines()
            .filter(|line| line.contains(text))
            .collect::<Vec<_>>()
    };
    let selected = lines("selected video encoder");
    assert!(
        selected.len() == 1 && selected[0].contains("h264_vulkan"),
        "{log}"
    );
    let warnings = lines("h264_vulkan is unavailable, encoding with libx264");
    assert!(
        warnings.len() == 1 && warnings[0].contains(id.as_str()),
        "{log}"
    );
    let opened = lines("opened video encoder");
    assert!(opened.len() == 1 && opened[0].contains("libx264"), "{log}");

    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("output.mp4");
    std::fs::write(&path, artifact.bytes().expect("artifact bytes")).expect("write artifact");
    let frames = decode_yuv(&path);
    assert_eq!(frames.len(), 1);
    // The encoder writes red (255, 0, 0) as BT.709 limited-range Y'CbCr.
    const RED: [u16; 3] = [63, 102, 240];
    let sample = frames[0].sample(8, 8);
    assert!(
        sample
            .iter()
            .zip(RED)
            .all(|(got, want)| got.abs_diff(want) <= 3),
        "{sample:?}"
    );
}

/// Decodes the first video stream of `path` to the YUV planes it stores, which sidesteps any colour
/// conversion.
fn decode_yuv(path: &Path) -> Vec<YuvVideoFrame> {
    let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
    let stream_index = input
        .best_video_stream()
        .expect("video stream")
        .stream_index;
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index,
            mode: DecodeMode::Cpu,
        },
    )
    .expect("open decoder");
    let mut frames = Vec::new();
    let mut take = |decoder: &mut VideoDecoder| {
        while let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
            match frame {
                DecodedVideoFrame::Yuv(frame) => frames.push(frame),
                DecodedVideoFrame::Rgba { .. } => panic!("the encoder writes a YUV stream"),
            }
        }
    };
    while let Some(packet) = input.read_packet().expect("read packet") {
        decoder.send_packet(&packet).expect("send packet");
        take(&mut decoder);
    }
    decoder.send_eof().expect("send eof");
    take(&mut decoder);
    frames
}

#[test]
fn renders_stills_read_as_video_with_gpu() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    for (alias, path) in [("photo.jpg", "/photo.jpg"), ("image.png", "/image.png")] {
        let Some((_, progress)) = render_outcome(
            &server,
            &json!({
                "composition": media_composition("video", alias),
                "media": { alias: media.url("localhost", path) },
            }),
        ) else {
            return;
        };
        assert_eq!(progress["state"], "succeeded", "{alias}: {progress}");
    }
}

#[test]
fn staged_playlists_cannot_reach_files_outside_the_job_with_gpu() {
    let fixtures = tempfile::tempdir().expect("fixture directory");
    let segment = fixtures.path().join("outside.ts");
    if !write_video(&segment) {
        eprintln!("no H.264 encoder; skipping lumen-server playlist containment test");
        return;
    }
    let media = MediaServer::start_with_playlist(format!(
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\n{}\n#EXT-X-ENDLIST\n",
        segment.display()
    ));
    let server = Server::start(&["--allow-media-host", "localhost"]);

    let Some((id, progress)) = render_outcome(
        &server,
        &json!({
            "composition": media_composition("video", "stream.m3u8"),
            "media": { "stream.m3u8": media.url("localhost", "/stream.m3u8") },
        }),
    ) else {
        return;
    };
    assert_eq!(progress["state"], "failed", "{progress}");
    assert_eq!(
        server.get(&format!("/renders/{id}/artifact")).status(),
        StatusCode::CONFLICT
    );
}

fn write_video(path: &Path) -> bool {
    let config = VideoEncoderConfig::cpu_rgba(32, 24, 24, "libx264");
    let Ok(mut encoder) = MuxedEncoder::create(path.to_string_lossy().to_string(), config) else {
        return false;
    };
    for pts in 0..6 {
        encoder
            .write_video_frame(&CpuVideoFrame {
                width: 32,
                height: 24,
                stride: 32 * 4,
                pixel_format: PixelFormat::Rgba8,
                pts: Some(pts),
                data: vec![128; 32 * 24 * 4],
            })
            .expect("write frame");
    }
    encoder.finish().expect("finish encode");
    true
}

fn serving_from(directory: &Path, args: &[&str]) -> Server {
    Server::start_with(args, |command| {
        command
            .current_dir(directory)
            .env("LUMEN_FONT_DIR", directory);
    })
}

const MISSING_MEDIA_ID: &str = "m_00000000000000000000000000000000";

fn roboto() -> Vec<u8> {
    fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../text/assets/roboto/Roboto-Regular.ttf"
    ))
    .expect("read Roboto")
}

fn create_upload(server: &Server, file_name: &str, content_type: &str, size: usize) -> Value {
    let created = server.post_to(
        "/media",
        &json!({ "contentType": content_type, "fileName": file_name, "sizeBytes": size }),
    );
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = created.json().expect("create body");
    assert_eq!(created["media"]["status"], "pending", "{created}");
    created
}

/// Creates media and sends `bytes` in one body, returning the media the upload answers with.
fn upload(server: &Server, file_name: &str, content_type: &str, bytes: &[u8]) -> Value {
    let created = create_upload(server, file_name, content_type, bytes.len());
    let uploaded = server.put_bytes(created["uploadUrl"].as_str().expect("upload url"), bytes);
    assert_eq!(uploaded.status(), StatusCode::OK);
    let uploaded: Value = uploaded.json().expect("upload body");
    uploaded["media"].clone()
}

fn media_id(media: &Value) -> String {
    media["id"].as_str().expect("media id").to_string()
}

fn asset_source_error(code: &str) -> [(String, String); 1] {
    [(code.to_string(), "/nodes/0/params/source".to_string())]
}

#[test]
fn media_uploads_are_probed_once_their_bytes_are_complete() {
    let server = Server::start(&[]);

    // The probe decides the kind, whatever the name and declared type say.
    let image = upload(&server, "pixel.bin", "application/octet-stream", TINY_PNG);
    assert_eq!(image["status"], "validated", "{image}");
    assert_eq!(image["kind"], "image");
    assert_eq!(image["metadata"], json!({ "width": 1, "height": 1 }));
    let id = media_id(&image);
    let read: Value = server
        .get(&format!("/media/{id}"))
        .json()
        .expect("media body");
    assert_eq!(read["media"], image);
    let content = server.get(&format!("/media/{id}/content"));
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(
        content.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(content.bytes().expect("content").as_ref(), TINY_PNG);

    let font = upload(&server, "brand.png", "image/png", &roboto());
    assert_eq!(font["kind"], "font", "{font}");
    assert_eq!(font["metadata"]["families"], json!(["Roboto"]));

    let fox = include_bytes!("../../engine/tests/fixtures/Fox.glb");
    let model = upload(&server, "fox.glb", "model/gltf-binary", fox);
    assert_eq!(model["kind"], "model", "{model}");
    let truncated = upload(&server, "fox.glb", "model/gltf-binary", &fox[..4096]);
    assert_eq!(truncated["status"], "rejected", "{truncated}");
    assert!(
        truncated["rejectionReason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("the model could not be read")),
        "{truncated}"
    );

    let sky = upload(
        &server,
        "sky.hdr",
        "image/vnd.radiance",
        b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n\x80\x80\x80\x81\x80\x80\x80\x81",
    );
    assert_eq!(sky["kind"], "environment", "{sky}");
    assert_eq!(sky["metadata"], json!({ "width": 2, "height": 1 }));

    let fake = upload(&server, "fake.png", "image/png", b"not media");
    assert_eq!(fake["status"], "rejected", "{fake}");
    assert!(fake["rejectionReason"].is_string(), "{fake}");
    let content = server.get(&format!("/media/{}/content", media_id(&fake)));
    assert_eq!(content.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(content), "media_not_validated");

    // Media bodies aren't held to the JSON body limit.
    let large = upload(
        &server,
        "large.bin",
        "application/octet-stream",
        &vec![0; 3 << 20],
    );
    assert_eq!(large["status"], "rejected", "{large}");
}

#[test]
fn media_waiting_for_its_content_is_capped_until_one_is_checked() {
    let server = Server::start(&["--media-max-pending", "1"]);
    let created = create_upload(&server, "pixel.png", "image/png", TINY_PNG.len());
    let refused = server.post_to(
        "/media",
        &json!({ "contentType": "image/png", "fileName": "next.png", "sizeBytes": 1 }),
    );
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(refused), "media_upload_limit");

    let uploaded = server.put_bytes(created["uploadUrl"].as_str().expect("upload url"), TINY_PNG);
    assert_eq!(uploaded.status(), StatusCode::OK);
    create_upload(&server, "next.png", "image/png", 1);
}

#[test]
fn media_multipart_uploads_join_the_listed_parts_in_order() {
    let server = Server::start(&[]);
    let jpeg = normal_sized_jpeg();
    let created = create_upload(&server, "photo.jpg", "image/jpeg", jpeg.len());
    let id = media_id(&created["media"]);
    let started = server.post_to(&format!("/media/{id}/multipart"), &json!({}));
    assert_eq!(started.status(), StatusCode::CREATED);
    let started: Value = started.json().expect("multipart body");
    let upload_id = started["uploadId"].as_str().expect("upload id").to_string();

    let part = |number: u32, bytes: &[u8]| -> Value {
        let response = server.put_bytes(
            &format!("/media/{id}/multipart/{upload_id}/parts/{number}"),
            bytes,
        );
        assert_eq!(response.status(), StatusCode::OK);
        let part: Value = response.json().expect("part body");
        assert_eq!(part["partNumber"], number);
        json!({ "etag": part["etag"], "partNumber": number })
    };
    let complete = |parts: Value| {
        server.post_to(
            &format!("/media/{id}/multipart/{upload_id}/complete"),
            &json!({ "parts": parts }),
        )
    };
    let (first, second) = jpeg.split_at(jpeg.len() / 2);
    // A part sent again replaces the earlier one, and parts are joined in the order listed.
    let stale = part(1, b"stale");
    let first = part(1, first);
    let second = part(2, second);

    let refused = complete(json!([stale, second]));
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error_code(refused), "invalid_upload_parts");

    let completed = complete(json!([first, second]));
    assert_eq!(completed.status(), StatusCode::OK);
    let completed: Value = completed.json().expect("complete body");
    assert_eq!(completed["media"]["status"], "validated", "{completed}");
    assert_eq!(completed["media"]["kind"], "image");
    assert_eq!(
        completed["media"]["metadata"],
        json!({ "width": 1280, "height": 720 })
    );
}

#[test]
fn media_url_imports_download_once_through_the_media_host_checks() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);

    let imported = server.post_to(
        "/media/url",
        &json!({ "url": media.url("localhost", "/photo.jpg") }),
    );
    assert_eq!(imported.status(), StatusCode::CREATED);
    let imported: Value = imported.json().expect("import body");
    let imported = &imported["media"];
    assert_eq!(imported["status"], "validated", "{imported}");
    assert_eq!(imported["sourceType"], "url");
    assert_eq!(imported["fileName"], "photo.jpg");
    assert_eq!(
        imported["metadata"],
        json!({ "width": 1280, "height": 720 })
    );
    assert_eq!(media.requests.load(Ordering::SeqCst), 1);

    // A private address is refused like an unlisted host: a request this server won't serve.
    let refused = server.post_to(
        "/media/url",
        &json!({ "url": media.url("127.0.0.1", "/image.png") }),
    );
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refused: Value = refused.json().expect("error body");
    assert_eq!(refused["code"], "media_download_forbidden_host");
    let error = refused["error"].as_str().unwrap_or_default();
    assert!(error.contains("host has no public address"), "{refused}");
    // So is a name the resolver finds only private addresses for.
    let unlisted = Server::start(&[]);
    let resolved = unlisted.post_to(
        "/media/url",
        &json!({ "url": media.url("localhost", "/image.png") }),
    );
    assert_eq!(resolved.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(resolved), "media_download_failed");
    assert_eq!(
        media.requests.load(Ordering::SeqCst),
        1,
        "the refused hosts are never contacted"
    );
    let kept = fs::read_dir(server.media_dir.path())
        .expect("media directory")
        .count();
    assert_eq!(kept, 1, "a refused import leaves nothing behind");
}

#[test]
fn renders_and_checks_resolve_asset_ids() {
    let server = Server::start(&[]);
    let image = media_id(&upload(&server, "pixel.png", "image/png", TINY_PNG));
    let font = media_id(&upload(&server, "Roboto.ttf", "font/ttf", &roboto()));
    let pending =
        media_id(&create_upload(&server, "later.png", "image/png", TINY_PNG.len())["media"]);
    let rejected = media_id(&upload(&server, "fake.png", "image/png", b"not media"));

    let mut text = solid_composition();
    text["nodes"][0] = json!({ "id": 1, "type": "text", "params": { "content": [
        { "text": "a", "font_family": format!("asset:{font}") }
    ] } });
    for composition in [image_composition(&format!("asset:{image}")), text] {
        let checked = server.check(&json!({ "composition": composition }));
        assert_eq!(checked, json!({ "ok": true, "diagnostics": [] }));
    }
    server.submit(&json!({ "composition": image_composition(&format!("asset:{image}")) }));

    for (composition, code) in [
        (
            image_composition(&format!("asset:{MISSING_MEDIA_ID}")),
            "asset.not_found",
        ),
        (
            image_composition(&format!("asset:{pending}")),
            "asset.not_found",
        ),
        (
            image_composition(&format!("asset:{rejected}")),
            "asset.not_found",
        ),
        (
            media_composition("video", &format!("asset:{image}")),
            "asset.wrong_kind",
        ),
    ] {
        let body = json!({ "composition": composition });
        let checked = server.check(&body);
        assert_eq!(checked["ok"], false, "{checked}");
        assert_eq!(
            codes_and_paths(&checked["diagnostics"]),
            asset_source_error(code)
        );
        let refused = server.post(&body);
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let refused: Value = refused.json().expect("error body");
        assert_eq!(refused["code"], "invalid_project_payload");
        assert_eq!(refused["diagnostics"], checked["diagnostics"]);
    }
}

#[test]
fn media_expires_after_its_ttl_except_while_a_render_holds_it() {
    let media = MediaServer::start();
    let server = Server::start(&["--media-ttl", "2s", "--allow-media-host", "localhost"]);
    let held = media_id(&upload(&server, "held.png", "image/png", TINY_PNG));
    let free = media_id(&upload(&server, "free.png", "image/png", TINY_PNG));
    // Staging waits on `/held`, so the render holds its asset until the test releases it.
    let render = server.submit(&json!({
        "composition": image_composition(&format!("asset:{held}")),
        "media": { "gate": media.url("localhost", "/held") },
    }));
    let dir = |id: &str| server.media_dir.path().join(id);
    let wait_until_swept = |id: &str| {
        let started = Instant::now();
        while dir(id).exists() {
            assert!(started.elapsed() < TIMEOUT, "{id} was never swept");
            thread::sleep(Duration::from_millis(50));
        }
    };

    // `free` expires after `held`, so the sweep that removed it passed over `held`.
    wait_until_swept(&free);
    assert!(dir(&held).exists(), "a held asset outlives its expiry");
    let expired = server.get(&format!("/media/{held}"));
    assert_eq!(expired.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(expired), "media_not_found");
    let checked =
        server.check(&json!({ "composition": image_composition(&format!("asset:{held}")) }));
    assert_eq!(
        codes_and_paths(&checked["diagnostics"]),
        asset_source_error("asset.not_found")
    );

    media.release.send(()).expect("release staging");
    server.wait_for_state(&render, &["failed", "succeeded"]);
    wait_until_swept(&held);
}

/// Bytes held by the temporary files directly in `dir`.
fn temp_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|items| {
            items
                .flatten()
                .filter(|item| item.file_name().to_string_lossy().starts_with(".tmp-"))
                .filter_map(|item| item.metadata().ok())
                .map(|metadata| metadata.len())
                .sum()
        })
        .unwrap_or_default()
}

fn bytes_under(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|items| {
            items
                .flatten()
                .map(|item| match item.metadata() {
                    Ok(metadata) if metadata.is_dir() => bytes_under(&item.path()),
                    Ok(metadata) => metadata.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or_default()
}

/// Sends `head` and `body` on a connection of its own, which the caller drops to disconnect.
fn send_raw(server: &Server, head: &str, body: &[u8]) -> TcpStream {
    let mut stream =
        TcpStream::connect(server.base.trim_start_matches("http://")).expect("connect to server");
    write!(
        stream,
        "{head}\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
    )
    .expect("write request head");
    stream.write_all(body).expect("write request body");
    stream
}

fn wait_until(what: &str, deadline: Duration, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < deadline, "{what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn media_uploads_never_hold_more_than_their_size_on_disk() {
    let server = Server::start(&["--media-max-bytes", "1024"]);
    let created = create_upload(&server, "parts.bin", "application/octet-stream", 1024);
    let id = media_id(&created["media"]);
    let started: Value = server
        .post_to(&format!("/media/{id}/multipart"), &json!({}))
        .json()
        .expect("multipart body");
    let upload_id = started["uploadId"].as_str().expect("upload id");
    let part = |number: u32, size: usize| {
        server.put_bytes(
            &format!("/media/{id}/multipart/{upload_id}/parts/{number}"),
            &vec![0; size],
        )
    };

    assert_eq!(part(1, 1024).status(), StatusCode::OK);
    // Sending part 1 again would hold both copies until the new one replaced the old.
    for (number, size) in [(2, 1), (3, 1024), (1, 1024)] {
        let refused = part(number, size);
        assert_eq!(
            refused.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "part {number}"
        );
        assert_eq!(error_code(refused), "media_too_large");
    }
    assert_eq!(
        bytes_under(&server.media_dir.path().join(&id).join("parts")),
        1024
    );

    // A single upload in flight counts too, and stops counting once its client goes away.
    let single =
        media_id(&create_upload(&server, "one.bin", "application/octet-stream", 1024)["media"]);
    let content = format!("/media/{single}/content");
    let entry = server.media_dir.path().join(&single);
    let partial = send_raw(
        &server,
        &format!("PUT {content} HTTP/1.1\r\nContent-Length: 1024"),
        &[0; 1000],
    );
    wait_until("the partial upload never arrived", TIMEOUT, || {
        temp_bytes(&entry) >= 1000
    });
    let refused = server.put_bytes(&content, &[0; 1024]);
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    drop(partial);
    wait_until("a dropped upload kept its bytes counted", TIMEOUT, || {
        let response = server.put_bytes(&content, &[0; 1024]);
        assert_ne!(response.status(), StatusCode::CONFLICT);
        response.status() == StatusCode::OK
    });
    assert_eq!(temp_bytes(&entry), 0);
}

#[test]
fn media_url_imports_cancelled_mid_download_leave_nothing_behind() {
    let media = MediaServer::start();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let body = json!({ "url": media.url("localhost", "/held") }).to_string();
    let request = send_raw(
        &server,
        &format!(
            "POST /media/url HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}",
            body.len()
        ),
        body.as_bytes(),
    );
    let stored = || {
        fs::read_dir(server.media_dir.path())
            .expect("media directory")
            .count()
    };
    wait_until("the import never started", TIMEOUT, || {
        media.requests.load(Ordering::SeqCst) == 1 && stored() == 1
    });

    drop(request);
    wait_until(
        "a cancelled import left its directory behind",
        Duration::from_secs(10),
        || stored() == 0,
    );
    let _ = media.release.send(());
}

#[test]
fn deleted_media_a_render_holds_stays_deleted_after_a_restart() {
    let media = MediaServer::start();
    let args = ["--allow-media-host", "localhost"];
    let server = Server::start(&args);
    let held = media_id(&upload(&server, "held.png", "image/png", TINY_PNG));
    // Staging waits on `/held`, so the render holds its asset until the server stops.
    server.submit(&json!({
        "composition": image_composition(&format!("asset:{held}")),
        "media": { "gate": media.url("localhost", "/held") },
    }));

    let deleted = server
        .http
        .delete(format!("{}/media/{held}", server.base))
        .bearer_auth(TOKEN)
        .send()
        .expect("DELETE media");
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        server.get(&format!("/media/{held}")).status(),
        StatusCode::NOT_FOUND
    );
    let dir = server.media_dir.path().join(&held);
    assert!(dir.join("content").exists(), "the render still reads it");

    let server = server.restart(&args);
    let read = server.get(&format!("/media/{held}"));
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(read), "media_not_found");
    assert!(!dir.exists());
    let _ = media.release.send(());
}

#[test]
fn media_url_import_failures_are_told_apart_by_cause_not_message() {
    let closed = TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("free port")
        .port();
    let server = Server::start(&["--allow-media-host", "localhost"]);
    let response = server.post_to(
        "/media/url",
        &json!({
            "url": format!("http://localhost:{closed}/clip.mp4"),
            "fileName": "host has no public address; allow it with --allow-media-host if this is intended",
        }),
    );
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "a refused connection is transient, whatever the file is called"
    );
    assert_eq!(error_code(response), "media_download_failed");
}
