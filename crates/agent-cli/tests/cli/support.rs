use super::*;

pub(super) fn agent_cmd() -> Command {
    Command::cargo_bin("agent").expect("agent binary exists")
}

pub(super) fn read_json(path: impl AsRef<std::path::Path>) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("JSON file exists")).expect("file is JSON")
}

pub(super) fn read_sqlite_trace(store: &std::path::Path, run_id: &str) -> AgentTrace {
    let sqlite_path = camino::Utf8PathBuf::from_path_buf(store.join("runtime.sqlite"))
        .expect("sqlite path is utf8");
    let run_id = RunId(run_id.to_owned());
    tokio::runtime::Runtime::new()
        .expect("tokio runtime starts")
        .block_on(async {
            let sqlite = SqliteStore::open(sqlite_path).await.expect("sqlite opens");
            sqlite
                .read_trace(&run_id)
                .await
                .expect("trace reads")
                .expect("trace exists")
        })
}

pub(super) fn reserve_local_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("port can be reserved");
    listener.local_addr().expect("local addr").port()
}

pub(super) fn spawn_openai_compatible_server() -> (u16, std::thread::JoinHandle<String>) {
    spawn_json_server(
        r#"{"choices":[{"message":{"content":"network answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#,
    )
}

pub(super) fn spawn_anthropic_server() -> (u16, std::thread::JoinHandle<String>) {
    spawn_json_server(
        r#"{"content":[{"type":"text","text":"anthropic answer"}],"stop_reason":"end_turn","usage":{"input_tokens":4,"output_tokens":3}}"#,
    )
}

pub(super) fn spawn_ollama_server() -> (u16, std::thread::JoinHandle<String>) {
    spawn_json_server(
        r#"{"message":{"role":"assistant","content":"local answer"},"done_reason":"stop","prompt_eval_count":6,"eval_count":4}"#,
    )
}

fn spawn_json_server(body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("server binds");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request accepted");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout set");
        let request = read_http_request(&mut stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("response writes");
        request
    });
    (port, handle)
}

pub(super) fn spawn_otlp_trace_collector() -> (u16, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("server binds");
    let port = listener.local_addr().expect("local addr").port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request accepted");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout set");
        let request = read_http_request(&mut stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
        .expect("response writes");
        request
    });
    (port, handle)
}

pub(super) fn spawn_http_tool_source_server() -> (u16, std::thread::JoinHandle<String>) {
    spawn_json_server(
        r#"{"output":{"host":"http-tool-source","tool":"http_echo","input":{"value":64}}}"#,
    )
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).expect("request reads");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if http_request_complete(&bytes) {
            break;
        }
    }
    String::from_utf8(bytes).expect("request is utf8")
}

fn http_request_complete(bytes: &[u8]) -> bool {
    let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    bytes.len() >= header_end + 4 + content_length
}

pub(super) fn wait_for_http_server(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if try_http_json_request(port, "GET", "/healthz", None).is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "HTTP server did not start on port {port}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn wait_for_event_log_contains(store: &std::path::Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = std::fs::read_dir(store.join("traces"))
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".events.jsonl"))
            })
            .any(|path| {
                std::fs::read_to_string(path)
                    .map(|text| text.contains(needle))
                    .unwrap_or(false)
            });
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "event log did not contain {needle}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub(super) fn find_single_run_event_log(store: &std::path::Path) -> std::path::PathBuf {
    std::fs::read_dir(store.join("traces"))
        .expect("trace dir reads")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".events.jsonl"))
        })
        .expect("run event log exists")
}

pub(super) fn http_json_request(port: u16, method: &str, path: &str, body: Option<&str>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_http_json_request(port, method, path, body) {
            Ok(value) => return value,
            Err(err) => {
                assert!(
                    Instant::now() < deadline,
                    "HTTP request {method} {path} did not succeed: {err}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub(super) fn http_json_status_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_http_json_status_request(port, method, path, body) {
            Ok(value) => return value,
            Err(err) => {
                assert!(
                    Instant::now() < deadline,
                    "HTTP request {method} {path} did not complete: {err}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub(super) fn http_text_request(port: u16, method: &str, path: &str, body: Option<&str>) -> String {
    http_text_request_with_headers(port, method, path, body, &[])
}

pub(super) fn http_text_request_with_headers(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match try_http_text_request_with_headers(port, method, path, body, headers) {
            Ok(value) => return value,
            Err(err) => {
                assert!(
                    Instant::now() < deadline,
                    "HTTP request {method} {path} did not succeed: {err}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub(super) fn try_http_text_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<String, String> {
    try_http_text_request_with_headers(port, method, path, body, &[])
}

fn try_http_json_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Value, String> {
    let (status, value) = try_http_json_status_request(port, method, path, body)?;
    if status != 200 {
        return Err(format!("unexpected HTTP status: {status}"));
    }
    Ok(value)
}

fn try_http_json_status_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, Value), String> {
    let response = send_http_request(port, method, path, body, &[])?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("malformed HTTP response: {response}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("malformed HTTP status line: {head}"))?
        .parse::<u16>()
        .map_err(|err| format!("parse status: {err}"))?;
    let value =
        serde_json::from_str(body).map_err(|err| format!("decode JSON: {err}; body: {body}"))?;
    Ok((status, value))
}

fn try_http_text_request_with_headers(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    let response = send_http_request(port, method, path, body, headers)?;
    if !response.starts_with("HTTP/1.1 200") {
        return Err(format!("unexpected HTTP response: {response}"));
    }
    Ok(response)
}

fn send_http_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    let body = body.unwrap_or("");
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|err| format!("connect: {err}"))?;
    let extra_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .map_err(|err| format!("write: {err}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|err| format!("read: {err}"))?;
    Ok(response)
}

pub(super) struct ChildGuard(pub(super) Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
