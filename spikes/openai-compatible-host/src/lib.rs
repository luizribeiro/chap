use std::error::Error;

use lockgate::{HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

const STARTUP_FUEL: u64 = 1_000_000;
const CALL_FUEL: u64 = 100_000_000;

lockgate::host_bindings!({
    path: "../../plugins/openai-compatible/wit",
    world: "provider-plugin",
});

pub async fn complete(
    wasm: &[u8],
    settings: serde_json::Value,
    request: provider::CompletionRequest,
) -> Result<provider::Completion, Box<dyn Error>> {
    let mut builder = HostBuilder::new(())?;
    let prepared = builder
        .prepare(
            "openai",
            wasm,
            PluginConfig {
                settings: Some(settings),
                ..PluginConfig::default()
            },
        )
        .await?;
    let acceptance = prepared.accept_all();
    let handle = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(STARTUP_FUEL),
        )
        .await?;
    let host = builder.finish();
    let client = host.client::<provider::Role>(&handle)?;
    let completion = client
        .complete(InvocationCtx::bounded(CALL_FUEL), request)
        .await?
        .map_err(std::io::Error::other)?;

    Ok(completion)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use super::*;

    const SERVER_TIMEOUT: Duration = Duration::from_secs(10);
    const HARNESS_TIMEOUT: Duration = Duration::from_secs(20);
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

    fn serve(mut stream: TcpStream) -> Result<ReceivedRequest, String> {
        stream
            .set_read_timeout(Some(SERVER_TIMEOUT))
            .map_err(|error| format!("failed to set mock read timeout: {error}"))?;
        stream
            .set_write_timeout(Some(SERVER_TIMEOUT))
            .map_err(|error| format!("failed to set mock write timeout: {error}"))?;

        let mut request = Vec::new();
        let (head_end, content_length) = loop {
            let mut chunk = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .map_err(|error| format!("failed to read mock request: {error}"))?;
            if read == 0 {
                return Err("client closed before sending complete HTTP headers".to_owned());
            }
            request.extend_from_slice(&chunk[..read]);

            let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head = std::str::from_utf8(&request[..head_end])
                .map_err(|error| format!("mock request headers were not UTF-8: {error}"))?;
            let content_length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .ok_or_else(|| "mock request omitted Content-Length".to_owned())?
                .1
                .trim()
                .parse::<usize>()
                .map_err(|error| format!("invalid mock request Content-Length: {error}"))?;
            break (head_end, content_length);
        };

        let body_start = head_end + 4;
        while request.len() < body_start + content_length {
            let mut chunk = [0; 4096];
            let read = stream
                .read(&mut chunk)
                .map_err(|error| format!("failed to read mock request body: {error}"))?;
            if read == 0 {
                return Err("client closed before sending the complete request body".to_owned());
            }
            request.extend_from_slice(&chunk[..read]);
        }

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
            body: request[body_start..body_start + content_length].to_vec(),
        })
    }

    #[tokio::test]
    async fn completes_against_an_egress_allowed_local_server() {
        let mock = MockServer::start();
        let wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/wasm32-wasip2/debug/sage_openai_compatible.wasm");
        let wasm = std::fs::read(&wasm).unwrap_or_else(|error| {
            panic!(
                "could not read plugin at `{}`: {error}; build it with `cargo build -p sage-openai-compatible --target wasm32-wasip2`",
                wasm.display()
            )
        });
        let settings = serde_json::json!({
            "egress-origin": mock.origin.clone(),
            "base-url": format!("{}/v1", mock.origin),
            "model": "m",
            "api-key": "x",
        });
        let request = provider::CompletionRequest {
            messages: vec![provider::Message::User("hello".to_owned())],
            tools: Vec::new(),
        };

        let completion = tokio::time::timeout(HARNESS_TIMEOUT, complete(&wasm, settings, request))
            .await
            .expect("provider invocation timed out")
            .expect("provider invocation failed");
        let received = mock.finish();

        assert!(
            received
                .head
                .starts_with("POST /v1/chat/completions HTTP/1.1\r\n")
        );
        assert!(
            received
                .head
                .to_ascii_lowercase()
                .contains("authorization: bearer x\r\n")
        );
        let body: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["model"], "m");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hello");
        assert_eq!(body["stream"], false);
        assert!(matches!(
            completion.content.as_slice(),
            [provider::AssistantContent::Text(text)] if text == "mocked response"
        ));
        assert!(matches!(
            completion.finish_reason,
            provider::FinishReason::Stop
        ));
    }
}
