use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

#[derive(Clone)]
pub struct Options {
    pub server_id: String,
    pub missing: bool,
    pub reject_auth: bool,
    pub replace: bool,
    pub disconnect_once: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            server_id: "server-id".into(),
            missing: false,
            reject_auth: false,
            replace: false,
            disconnect_once: false,
        }
    }
}

pub struct MockPaseo {
    pub received: Arc<Mutex<Vec<Value>>>,
    pub closed: Arc<AtomicUsize>,
    pub connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MockPaseo {
    pub fn new(home: &Path, options: Options) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(
            home.join("paseo.pid"),
            json!({
                "listen":listener.local_addr().unwrap().to_string(), "serverId":options.server_id,
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(home.join("local-credential"), "A".repeat(43)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (received, closed, connections, stop) = (
                received.clone(),
                closed.clone(),
                connections.clone(),
                stop.clone(),
            );
            thread::spawn(move || {
                let mut workers = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let index = connections.fetch_add(1, Ordering::Relaxed);
                            let (received, closed, stop, options) = (
                                received.clone(),
                                closed.clone(),
                                stop.clone(),
                                options.clone(),
                            );
                            workers.push(thread::spawn(move || {
                                serve(stream, index, &options, &received, &stop);
                                closed.fetch_add(1, Ordering::Relaxed);
                            }));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
                for worker in workers {
                    let _ = worker.join();
                }
            })
        };
        Self {
            received,
            closed,
            connections,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for MockPaseo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve(
    stream: TcpStream,
    index: usize,
    options: &Options,
    received: &Mutex<Vec<Value>>,
    stop: &AtomicBool,
) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    ws.get_mut()
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let mut history_count = 0;
    while !stop.load(Ordering::Relaxed) {
        let frame = match ws.read() {
            Ok(Message::Text(text)) => serde_json::from_str::<Value>(&text).unwrap(),
            Ok(Message::Close(_)) => {
                let _ = ws.flush();
                return;
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => return,
        };
        received.lock().unwrap().push(frame.clone());
        if frame["type"] == "hello" {
            if options.reject_auth {
                send(
                    &mut ws,
                    json!({"type":"hello.rejected", "reason":"incorrect_password", "accepts":["password"]}),
                );
                return;
            }
            assert_eq!(
                frame["auth"],
                json!({"kind":"localCredential", "token":"A".repeat(43)})
            );
            session(
                &mut ws,
                "status",
                json!({"status":"server_info", "serverId":options.server_id}),
            );
            continue;
        }
        if frame["type"] == "ping" {
            send(&mut ws, json!({"type":"pong"}));
            continue;
        }
        let message = &frame["message"];
        let request = message["requestId"].clone();
        match message["type"].as_str() {
            Some("fetch_agent_request") => {
                let agent = if options.missing {
                    Value::Null
                } else {
                    json!({"id":message["agentId"]})
                };
                session(
                    &mut ws,
                    "fetch_agent_response",
                    json!({"requestId":request, "agent":agent, "error":null}),
                );
            }
            Some("agent.timeline.set_subscription.request") => {
                assert_eq!(message["agentIds"], json!(["paseo-agent"]));
                session(
                    &mut ws,
                    "agent.timeline.set_subscription.response",
                    json!({
                        "requestId":request, "agentIds":message["agentIds"], "subscriptionId":"sub-1"
                    }),
                );
                event(&mut ws, 1, "Hello", "paseo-agent", "sub-1");
            }
            Some("fetch_agent_timeline_request") => {
                assert_eq!(message["limit"], 48);
                let replacement = history_count > 0 && options.replace;
                let epoch = if replacement { "epoch-2" } else { "epoch-1" };
                let content = if replacement {
                    "replacement history"
                } else {
                    "Hello"
                };
                session(
                    &mut ws,
                    "fetch_agent_timeline_response",
                    json!({
                        "requestId":request, "agentId":"paseo-agent", "epoch":epoch, "error":null,
                        "entries":[{"seqStart":1,"seqEnd":1,"turnId":"turn-1", "item":{
                            "type":"assistant_message", "messageId":"message-1", "text":content
                        }}]
                    }),
                );
                if history_count == 0 {
                    event(&mut ws, 2, " streamed", "paseo-agent", "sub-1");
                    event(&mut ws, 3, "WRONG AGENT", "other-agent", "sub-1");
                    event(&mut ws, 4, "WRONG SUBSCRIPTION", "paseo-agent", "other-sub");
                    if options.replace {
                        session(
                            &mut ws,
                            "agent.timeline.replacement",
                            json!({
                                "agentId":"paseo-agent", "subscriptionId":"sub-1", "epoch":"epoch-2"
                            }),
                        );
                    }
                    if options.disconnect_once && index == 0 {
                        let _ = ws.close(None);
                        return;
                    }
                }
                history_count += 1;
            }
            _ => {}
        }
    }
}

fn event(ws: &mut WebSocket<TcpStream>, seq: u64, text: &str, agent: &str, subscription: &str) {
    session(
        ws,
        "agent_stream",
        json!({"agentId":agent,"subscriptionId":subscription,"epoch":"epoch-1","seq":seq,
            "event":{"type":"timeline","turnId":"turn-1","item":{"type":"assistant_message","messageId":"message-1","text":text}}
        }),
    );
}
fn session(ws: &mut WebSocket<TcpStream>, kind: &str, payload: Value) {
    send(
        ws,
        json!({"type":"session","message":{"type":kind,"payload":payload}}),
    );
}
fn send(ws: &mut WebSocket<TcpStream>, message: Value) {
    let _ = ws.send(Message::Text(message.to_string().into()));
}
