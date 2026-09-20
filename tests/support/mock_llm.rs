use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// In-process mock of the Anthropic Messages and OpenAI Chat Completions APIs.
///
/// Every completion request gets an immediate final answer ("OK", stop), so the
/// conversation completes deterministically on the first model response. Use
/// [`MockLlm::completions`] to assert a client actually reached the server and
/// [`MockLlm::requests`] for diagnostics.
pub struct MockLlm {
    url: String,
    completions: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockLlm {
    /// `delay` postpones every completion response. Fast headless runs can
    /// finish before a client's plugin runtime has flushed a status report;
    /// holding the conversation open keeps the client alive long enough for
    /// its hooks to fire.
    pub fn start_with_delay(delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let completions = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let accept_completions = completions.clone();
        let accept_requests = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let completions = accept_completions.clone();
                let requests = accept_requests.clone();
                std::thread::spawn(move || handle(stream, delay, &completions, &requests));
            }
        });
        Self {
            url,
            completions,
            requests,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn completions(&self) -> usize {
        self.completions.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn handle(
    mut stream: TcpStream,
    delay: Duration,
    completions: &AtomicUsize,
    requests: &Mutex<Vec<String>>,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let Some((method, path, headers)) = read_head(&mut reader) else {
        return;
    };
    if headers
        .get("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
        && stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err()
    {
        return;
    }
    let body = if method == "GET" {
        Vec::new()
    } else {
        match read_body(&mut reader, &headers) {
            Some(body) => body,
            None => return,
        }
    };
    requests.lock().unwrap().push(format!("{method} {path}"));
    let path = path.split('?').next().unwrap_or("");
    let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match (method.as_str(), path) {
        ("POST", "/v1/messages") => {
            completions.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            let model = payload["model"].as_str().unwrap_or("mock-model");
            let (content_type, body) = if payload["stream"].as_bool() == Some(true) {
                ("text/event-stream", anthropic_sse(model))
            } else {
                ("application/json", anthropic_message(model).to_string())
            };
            respond(&mut stream, 200, content_type, body);
        }
        ("POST", "/v1/messages/count_tokens") => {
            respond(
                &mut stream,
                200,
                "application/json",
                json!({"input_tokens": 1}).to_string(),
            );
        }
        ("POST", "/v1/chat/completions") => {
            completions.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            let model = payload["model"].as_str().unwrap_or("mock-model");
            let include_usage = payload["stream_options"]["include_usage"].as_bool() == Some(true);
            let (content_type, body) = if payload["stream"].as_bool() == Some(true) {
                ("text/event-stream", openai_sse(model, include_usage))
            } else {
                ("application/json", openai_message(model).to_string())
            };
            respond(&mut stream, 200, content_type, body);
        }
        ("POST", "/v1/responses") => {
            completions.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            let model = payload["model"].as_str().unwrap_or("mock-model");
            let (content_type, body) = if payload["stream"].as_bool() == Some(true) {
                ("text/event-stream", responses_sse(model))
            } else {
                ("application/json", responses_object(model).to_string())
            };
            respond(&mut stream, 200, content_type, body);
        }
        ("GET", "/v1/models") => {
            respond(
                &mut stream,
                200,
                "application/json",
                json!({
                    "object": "list",
                    "data": [{
                        "id": "mock-model", "object": "model", "type": "model",
                        "created": 0, "owned_by": "mock", "display_name": "Mock Model",
                    }],
                    "has_more": false,
                })
                .to_string(),
            );
        }
        _ => {
            respond(
                &mut stream,
                404,
                "application/json",
                json!({"error": {"message": format!("no mock route for {method} {path}"), "type": "not_found_error"}})
                    .to_string(),
            );
        }
    }
}

fn read_head(
    reader: &mut BufReader<TcpStream>,
) -> Option<(String, String, HashMap<String, String>)> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Some((method, path, headers))
}

fn read_body(
    reader: &mut BufReader<TcpStream>,
    headers: &HashMap<String, String>,
) -> Option<Vec<u8>> {
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        let mut body = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            let size = usize::from_str_radix(line.trim().split(';').next()?, 16).ok()?;
            if size == 0 {
                let mut trailer = String::new();
                reader.read_line(&mut trailer).ok()?;
                break;
            }
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            body.extend_from_slice(&chunk[..size]);
        }
        Some(body)
    } else {
        let length: usize = headers.get("content-length")?.parse().ok()?;
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        Some(body)
    }
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: String) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn anthropic_message(model: &str) -> Value {
    json!({
        "id": "msg_mock",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{"type": "text", "text": "OK"}],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": {"input_tokens": 1, "output_tokens": 1},
    })
}

fn anthropic_sse(model: &str) -> String {
    let events = [
        (
            "message_start",
            json!({"type": "message_start", "message": {
                "id": "msg_mock", "type": "message", "role": "assistant", "model": model,
                "content": [], "stop_reason": null, "stop_sequence": null,
                "usage": {"input_tokens": 1, "output_tokens": 0},
            }}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "OK"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 1}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ];
    let mut body = String::new();
    for (event, data) in events {
        body.push_str(&format!("event: {event}\ndata: {data}\n\n"));
    }
    body
}

fn responses_object(model: &str) -> Value {
    json!({
        "id": "resp_mock",
        "object": "response",
        "created_at": 0,
        "status": "completed",
        "model": model,
        "output": [{
            "type": "message",
            "id": "msg_mock",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "OK", "annotations": []}],
        }],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
    })
}

fn responses_sse(model: &str) -> String {
    let events = [
        json!({"type": "response.created", "response": {
            "id": "resp_mock", "object": "response", "created_at": 0,
            "status": "in_progress", "model": model, "output": [],
        }}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {
            "type": "message", "id": "msg_mock", "status": "in_progress",
            "role": "assistant", "content": [],
        }}),
        json!({"type": "response.content_part.added", "item_id": "msg_mock", "output_index": 0, "content_index": 0, "part": {
            "type": "output_text", "text": "", "annotations": [],
        }}),
        json!({"type": "response.output_text.delta", "item_id": "msg_mock", "output_index": 0, "content_index": 0, "delta": "OK"}),
        json!({"type": "response.output_text.done", "item_id": "msg_mock", "output_index": 0, "content_index": 0, "text": "OK"}),
        json!({"type": "response.content_part.done", "item_id": "msg_mock", "output_index": 0, "content_index": 0, "part": {
            "type": "output_text", "text": "OK", "annotations": [],
        }}),
        json!({"type": "response.output_item.done", "output_index": 0, "item": {
            "type": "message", "id": "msg_mock", "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "OK", "annotations": []}],
        }}),
        json!({"type": "response.completed", "response": responses_object(model)}),
    ];
    let mut body = String::new();
    for data in events {
        let event = data["type"].as_str().unwrap();
        body.push_str(&format!("event: {event}\ndata: {data}\n\n"));
    }
    body
}

fn openai_message(model: &str) -> Value {
    json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "OK"},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    })
}

fn openai_sse(model: &str, include_usage: bool) -> String {
    let chunk = |choices: Value| json!({"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 0, "model": model, "choices": choices});
    let mut body = String::new();
    for data in [
        chunk(
            json!([{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]),
        ),
        chunk(json!([{"index": 0, "delta": {"content": "OK"}, "finish_reason": null}])),
        chunk(json!([{"index": 0, "delta": {}, "finish_reason": "stop"}])),
    ] {
        body.push_str(&format!("data: {data}\n\n"));
    }
    if include_usage {
        let mut usage_chunk = chunk(json!([]));
        usage_chunk["usage"] =
            json!({"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2});
        body.push_str(&format!("data: {usage_chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}
