use sage_core::{AgentBuilder, SessionOptions};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const SERVER_TIMEOUT: Duration = Duration::from_secs(10);
const INVOCATION_TIMEOUT: Duration = Duration::from_secs(20);
const RESPONSE_BODY: &str = r#"{"id":"chatcmpl-mock","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"mocked response"},"finish_reason":"stop"}]}"#;

struct ReceivedRequest {
    head: String,
    body: Vec<u8>,
}

struct MockServer {
    origin: String,
    received: Receiver<Result<ReceivedRequest, String>>,
    thread: JoinHandle<()>,
}

impl MockServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock server");
        listener
            .set_nonblocking(true)
            .expect("make mock listener nonblocking");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (sender, received) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let deadline = Instant::now() + SERVER_TIMEOUT;
            let result = loop {
                match listener.accept() {
                    Ok((stream, _)) => break serve(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            break Err("mock server timed out waiting for a request".to_owned());
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => break Err(format!("mock server accept failed: {error}")),
                }
            };
            let _ = sender.send(result);
        });

        Self {
            origin,
            received,
            thread,
        }
    }

    fn finish(self) -> ReceivedRequest {
        let request = self
            .received
            .recv_timeout(SERVER_TIMEOUT)
            .expect("mock server did not report a request")
            .expect("mock server failed while serving the request");
        self.thread.join().expect("mock server thread panicked");
        request
    }
}

#[tokio::test]
async fn completes_through_the_sage_host_against_an_allowed_local_server() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let component = build_openai_component(&workspace);
    let mock = MockServer::start();
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("sage.toml");
    write_openai_config(&config_path, &component, &mock.origin);

    let builder = AgentBuilder::load(&config_path).unwrap();
    builder.approve_plugin("openai").await.unwrap();
    let agent = builder.start().await.unwrap();
    assert!(agent.plugin_errors().next().is_none());
    let session = agent.session(SessionOptions::new("openai")).unwrap();
    let completion = tokio::time::timeout(INVOCATION_TIMEOUT, session.send("hello"))
        .await
        .expect("provider invocation timed out")
        .expect("provider invocation failed");
    let received = mock.finish();

    assert_eq!(completion, "mocked response");
    assert!(
        received
            .head
            .starts_with("POST /v1/chat/completions HTTP/1.1\r\n")
    );
    assert!(
        received
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer mock-key\r\n")
    );
    let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
    assert_eq!(body["model"], "mock-model");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "hello");
    assert_eq!(body["stream"], false);

    tokio::task::spawn_blocking(move || drop((session, agent)))
        .await
        .unwrap();
}

#[tokio::test]
async fn refuses_an_expanded_egress_manifest_until_reapproved() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let component = build_openai_component(&workspace);
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("sage.toml");
    write_openai_config(&config_path, &component, "http://127.0.0.1:41001");

    AgentBuilder::load(&config_path)
        .unwrap()
        .approve_plugin("openai")
        .await
        .unwrap();
    write_openai_config(&config_path, &component, "http://127.0.0.1:41002");
    let agent = AgentBuilder::load(&config_path)
        .unwrap()
        .start()
        .await
        .unwrap();
    let errors = agent.plugin_errors().collect::<Vec<_>>();

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].0, "openai");
    assert!(
        errors[0].1.contains("expanded its permission manifest"),
        "{}",
        errors[0].1
    );
    assert!(
        errors[0].1.contains("sage grants review openai"),
        "{}",
        errors[0].1
    );
    assert_eq!(
        agent.session(SessionOptions::new("openai")).err().unwrap(),
        errors[0].1
    );

    tokio::task::spawn_blocking(move || drop(agent))
        .await
        .unwrap();
}

fn write_openai_config(path: &Path, component: &Path, origin: &str) {
    std::fs::write(
        path,
        format!(
            r#"
[plugins.openai]
component = {component:?}

[plugins.openai.settings]
base-url = "{origin}/v1"
egress-origin = "{origin}"
model = "mock-model"
api-key = "mock-key"
"#,
            component = component.display().to_string(),
        ),
    )
    .unwrap();
}

fn build_openai_component(workspace: &Path) -> PathBuf {
    static COMPONENT: OnceLock<PathBuf> = OnceLock::new();
    COMPONENT
        .get_or_init(|| build_openai_component_once(workspace))
        .clone()
}

fn build_openai_component_once(workspace: &Path) -> PathBuf {
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace)
        .args([
            "build",
            "-p",
            "sage-openai-compatible",
            "--target",
            "wasm32-wasip2",
        ])
        .output()
        .expect("run cargo to build the OpenAI-compatible component");
    assert!(
        output.status.success(),
        "failed to build OpenAI-compatible component:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target) if Path::new(&target).is_absolute() => PathBuf::from(target),
        Some(target) => workspace.join(target),
        None => workspace.join("target"),
    };
    let component = target.join("wasm32-wasip2/debug/sage_openai_compatible.wasm");
    assert!(
        component.is_file(),
        "OpenAI-compatible component was not built at `{}`",
        component.display()
    );
    component
}

fn serve(mut stream: TcpStream) -> Result<ReceivedRequest, String> {
    stream
        .set_read_timeout(Some(SERVER_TIMEOUT))
        .map_err(|error| format!("failed to set mock read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(SERVER_TIMEOUT))
        .map_err(|error| format!("failed to set mock write timeout: {error}"))?;

    let mut request = Vec::new();
    let head_end = loop {
        let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            read_more(
                &mut stream,
                &mut request,
                "client closed before sending complete HTTP headers",
            )?;
            continue;
        };
        break head_end;
    };

    let body_start = head_end + 4;
    let head = std::str::from_utf8(&request[..head_end])
        .map_err(|error| format!("mock request headers were not UTF-8: {error}"))?;
    let content_length = header_values(head, "content-length")
        .next()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("invalid mock request Content-Length: {error}"))
        })
        .transpose()?;
    let is_chunked = header_values(head, "transfer-encoding")
        .flat_map(|value| value.split(','))
        .any(|encoding| encoding.trim().eq_ignore_ascii_case("chunked"));
    let body = if is_chunked {
        read_chunked_body(&mut stream, &mut request, body_start)?
    } else if let Some(content_length) = content_length {
        while request.len() < body_start + content_length {
            read_more(
                &mut stream,
                &mut request,
                "client closed before sending the complete request body",
            )?;
        }
        request[body_start..body_start + content_length].to_vec()
    } else {
        return Err("mock request used unsupported HTTP body framing".to_owned());
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        RESPONSE_BODY.len(),
        RESPONSE_BODY
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("failed to write mock response: {error}"))?;

    Ok(ReceivedRequest {
        head: String::from_utf8(request[..head_end].to_vec())
            .map_err(|error| format!("mock request headers were not UTF-8: {error}"))?,
        body,
    })
}

fn header_values<'a>(head: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(move |(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

fn read_chunked_body(
    stream: &mut TcpStream,
    request: &mut Vec<u8>,
    mut cursor: usize,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let line_end = loop {
            if let Some(offset) = request[cursor..]
                .windows(2)
                .position(|window| window == b"\r\n")
            {
                break cursor + offset;
            }
            read_more(
                stream,
                request,
                "client closed before sending a complete chunk header",
            )?;
        };
        let size = std::str::from_utf8(&request[cursor..line_end])
            .map_err(|error| format!("mock request chunk size was not UTF-8: {error}"))?
            .split(';')
            .next()
            .unwrap()
            .trim();
        let size = usize::from_str_radix(size, 16)
            .map_err(|error| format!("invalid mock request chunk size: {error}"))?;
        cursor = line_end + 2;

        if size == 0 {
            while request.len() < cursor + 2 {
                read_more(
                    stream,
                    request,
                    "client closed before terminating the chunked request body",
                )?;
            }
            if request[cursor..].starts_with(b"\r\n") {
                return Ok(body);
            }
            loop {
                if request[cursor..]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    return Ok(body);
                }
                read_more(
                    stream,
                    request,
                    "client closed before sending complete chunked trailers",
                )?;
            }
        }

        while request.len() < cursor + size + 2 {
            read_more(
                stream,
                request,
                "client closed before sending complete chunk data",
            )?;
        }
        body.extend_from_slice(&request[cursor..cursor + size]);
        cursor += size;
        if request[cursor..cursor + 2] != *b"\r\n" {
            return Err("mock request chunk omitted its terminating CRLF".to_owned());
        }
        cursor += 2;
    }
}

fn read_more(
    stream: &mut TcpStream,
    request: &mut Vec<u8>,
    closed_message: &str,
) -> Result<(), String> {
    let mut chunk = [0; 4096];
    let read = stream
        .read(&mut chunk)
        .map_err(|error| format!("failed to read mock request: {error}"))?;
    if read == 0 {
        return Err(closed_message.to_owned());
    }
    request.extend_from_slice(&chunk[..read]);
    Ok(())
}
