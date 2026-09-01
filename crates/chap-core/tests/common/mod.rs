use chap_core::{SessionEvent, SessionEventKind, ToolError};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    thread::JoinHandle,
    time::Duration,
};

pub const SERVER_TIMEOUT: Duration = Duration::from_secs(60);
pub const INVOCATION_TIMEOUT: Duration = Duration::from_secs(20);

pub struct ToolRequest<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub arguments: Value,
}

pub struct ReceivedRequest {
    pub body: Vec<u8>,
}

pub struct MockServer {
    pub origin: String,
    received: Receiver<Result<Vec<ReceivedRequest>, String>>,
    thread: JoinHandle<()>,
}

impl MockServer {
    pub fn start(tool_requests: &[ToolRequest<'_>]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock server");
        listener
            .set_nonblocking(true)
            .expect("make mock listener nonblocking");
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let responses = vec![tool_call_response(tool_requests), final_response()];
        let (sender, received) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let result = responses
                .into_iter()
                .map(|response| {
                    let stream = accept(&listener)?;
                    serve(stream, &response)
                })
                .collect();
            let _ = sender.send(result);
        });

        Self {
            origin,
            received,
            thread,
        }
    }

    pub fn finish(self) -> Vec<ReceivedRequest> {
        let requests = self
            .received
            .recv_timeout(SERVER_TIMEOUT)
            .expect("mock server did not report its requests")
            .expect("mock server failed while serving requests");
        self.thread.join().expect("mock server thread panicked");
        requests
    }
}

pub async fn receive_until_complete(events: &mut chap_core::SessionEvents) -> Vec<SessionEvent> {
    tokio::time::timeout(INVOCATION_TIMEOUT, async {
        let mut observed = Vec::new();
        loop {
            let event = events.recv().await.unwrap().unwrap();
            let complete = matches!(event.kind, SessionEventKind::RunCompleted { .. });
            observed.push(event);
            if complete {
                return observed;
            }
        }
    })
    .await
    .expect("agent loop events timed out")
}

pub fn tool_result<'a>(
    events: &'a [SessionEvent],
    expected_call_id: &str,
) -> &'a Result<String, ToolError> {
    events
        .iter()
        .find_map(|event| match &event.kind {
            SessionEventKind::ToolFinished {
                call_id, result, ..
            } if call_id == expected_call_id => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing tool result for `{expected_call_id}`"))
}

pub fn provider_tool_output(requests: &[ReceivedRequest], call_id: &str) -> String {
    let body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["tool_call_id"] == call_id)
        .and_then(|message| message["content"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("provider did not receive tool result for `{call_id}`"))
}

pub fn build_component(workspace: &Path, package: &str, wasm_file_name: &str) -> PathBuf {
    let output = Command::new(env!("CARGO"))
        .current_dir(workspace)
        .args(["build", "-p", package, "--target", "wasm32-wasip2"])
        .output()
        .unwrap_or_else(|error| panic!("run cargo to build the `{package}` component: {error}"));
    assert!(
        output.status.success(),
        "failed to build the `{package}` component:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(target) if Path::new(&target).is_absolute() => PathBuf::from(target),
        Some(target) => workspace.join(target),
        None => workspace.join("target"),
    };
    let component = target.join("wasm32-wasip2/debug").join(wasm_file_name);
    assert!(
        component.is_file(),
        "`{package}` component was not built at `{}`",
        component.display()
    );
    component
}

pub fn build_openai_component(workspace: &Path) -> PathBuf {
    build_component(
        workspace,
        "chap-openai-compatible",
        "chap_openai_compatible.wasm",
    )
}

pub fn tool_call_response(tool_requests: &[ToolRequest<'_>]) -> String {
    let tool_calls = tool_requests
        .iter()
        .map(|request| {
            json!({
                "id": request.id,
                "type": "function",
                "function": {
                    "name": request.name,
                    "arguments": request.arguments.to_string(),
                },
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": "chatcmpl-tools",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": tool_calls,
            },
            "finish_reason": "tool_calls",
        }],
    })
    .to_string()
}

pub fn final_response() -> String {
    json!({
        "id": "chatcmpl-final",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "done",
            },
            "finish_reason": "stop",
        }],
    })
    .to_string()
}

// Waits unboundedly: agent startup between bind and request includes
// admission and wasmtime compilation, which loaded runners stretch past any
// fixed deadline. The invocation timeout bounds the no-request failure path.
pub fn accept(listener: &TcpListener) -> Result<TcpStream, String> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets inherit O_NONBLOCK from the listener on macOS.
                stream.set_nonblocking(false).map_err(|error| {
                    format!("failed to make the mock connection blocking: {error}")
                })?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("mock server accept failed: {error}")),
        }
    }
}

pub fn serve(mut stream: TcpStream, response_body: &str) -> Result<ReceivedRequest, String> {
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
        response_body.len(),
        response_body
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| format!("failed to write mock response: {error}"))?;

    Ok(ReceivedRequest { body })
}

pub fn header_values<'a>(head: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .filter(move |(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

pub fn read_chunked_body(
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

pub fn read_more(
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
