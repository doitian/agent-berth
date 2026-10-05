use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use interprocess::{
    ConnectWaitMode,
    local_socket::{ConnectOptions, Stream, prelude::*},
};
use serde_json::{Value, json};
use tungstenite::{
    HandshakeError, Message, WebSocket, client::client_with_config, protocol::WebSocketConfig,
};

const TIMEOUT: Duration = Duration::from_secs(5);
static CLIENT_SERIAL: AtomicU64 = AtomicU64::new(0);

pub(super) struct Api {
    socket: WebSocket<Transport>,
    pub server_id: String,
    pending: VecDeque<(Value, usize)>,
    pending_bytes: usize,
    serial: u64,
    last_ping: Instant,
    last_received: Instant,
}

impl Api {
    pub fn connect(home: &Path, cancel: &AtomicBool) -> Result<Self> {
        let lock: Value = serde_json::from_slice(
            &std::fs::read(home.join("paseo.pid")).context("read Paseo daemon endpoint")?,
        )
        .context("invalid Paseo daemon state")?;
        let listen = lock["listen"]
            .as_str()
            .filter(|value| !value.is_empty())
            .context("Paseo daemon has not published an endpoint")?;
        let (transport, url) = Transport::connect(listen)?;
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(1024 * 1024);
        config.max_frame_size = Some(512 * 1024);
        config.write_buffer_size = 0;
        let deadline = Instant::now() + TIMEOUT;
        let mut handshake = client_with_config(url, transport, Some(config));
        let socket = loop {
            check(cancel, deadline)?;
            match handshake {
                Ok((socket, _)) => break socket,
                Err(HandshakeError::Interrupted(mid)) => {
                    thread::sleep(Duration::from_millis(10));
                    handshake = mid.handshake();
                }
                Err(HandshakeError::Failure(error)) => {
                    return Err(error).context("connect to Paseo API");
                }
            }
        };
        let mut api = Self {
            socket,
            server_id: String::new(),
            pending: VecDeque::new(),
            pending_bytes: 0,
            serial: 0,
            last_ping: Instant::now(),
            last_received: Instant::now(),
        };
        let mut hello = json!({
            "type":"hello", "clientId":format!("agent-berth-{}-{}", std::process::id(), CLIENT_SERIAL.fetch_add(1, Ordering::Relaxed)),
            "clientType":"cli", "protocolVersion":1,
            "capabilities":{"hello_rejection":true, "owned_subscriptions":true,
                "explicit_event_subscriptions":true, "selective_agent_timeline":true,
                "all_providers":true, "timeline_replacement_invalidation":true}
        });
        if let Ok(password) = std::env::var("PASEO_PASSWORD")
            && !password.is_empty()
        {
            hello["auth"] = json!({"kind":"password", "password":password});
        } else if let Ok(token) = std::fs::read_to_string(home.join("local-credential")) {
            let token = token.trim();
            ensure!(
                token.len() == 43
                    && token
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
                "invalid Paseo local credential"
            );
            hello["auth"] = json!({"kind":"localCredential", "token":token});
        }
        api.send(hello, cancel)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let envelope = api.receive(cancel, Some(deadline))?;
            if envelope["type"] == "hello.rejected" {
                let reason = match envelope["reason"].as_str() {
                    Some("password_required") => "password required; set PASEO_PASSWORD",
                    Some("incorrect_password") => "authentication rejected",
                    _ => "incompatible protocol",
                };
                bail!("Paseo API: {reason}");
            }
            let message = &envelope["message"];
            if message["type"] == "status" && message["payload"]["status"] == "server_info" {
                api.server_id = message["payload"]["serverId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .context("Paseo API omitted server ID")?
                    .to_string();
                if let Some(expected) = lock["serverId"].as_str() {
                    ensure!(
                        expected == api.server_id,
                        "Paseo API server ID does not match local daemon state"
                    );
                }
                return Ok(api);
            }
        }
    }

    pub fn agent(&mut self, agent_id: &str, cancel: &AtomicBool) -> Result<()> {
        let payload = self.request(
            json!({"type":"fetch_agent_request", "agentId":agent_id}),
            "fetch_agent_response",
            cancel,
        )?;
        ensure!(
            payload["agent"]["id"].as_str() == Some(agent_id),
            "session does not exist in Paseo"
        );
        ensure!(
            payload["agent"]["archivedAt"].as_str().is_none(),
            "Paseo session is archived"
        );
        Ok(())
    }

    pub fn subscribe(&mut self, agent_id: &str, cancel: &AtomicBool) -> Result<Option<String>> {
        let payload = self.request(
            json!({"type":"agent.timeline.set_subscription.request", "agentIds":[agent_id]}),
            "agent.timeline.set_subscription.response",
            cancel,
        )?;
        ensure!(
            payload["agentIds"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == agent_id)),
            "Paseo did not subscribe to the selected session"
        );
        Ok(payload["subscriptionId"].as_str().map(str::to_string))
    }

    pub fn history(&mut self, agent_id: &str, cancel: &AtomicBool) -> Result<Value> {
        self.request(
            json!({"type":"fetch_agent_timeline_request", "agentId":agent_id,
            "direction":"tail", "limit":48, "projection":"projected"}),
            "fetch_agent_timeline_response",
            cancel,
        )
    }

    fn request(
        &mut self,
        mut message: Value,
        response: &str,
        cancel: &AtomicBool,
    ) -> Result<Value> {
        self.serial += 1;
        let request_id = format!("berth-{}", self.serial);
        message["requestId"] = request_id.clone().into();
        self.send(json!({"type":"session", "message":message}), cancel)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let envelope = self.receive(cancel, Some(deadline))?;
            let message = &envelope["message"];
            if envelope["type"] != "session" {
                continue;
            }
            if message["type"] == response && message["payload"]["requestId"] == request_id {
                // Do not surface arbitrary server error strings: they may echo credentials.
                ensure!(
                    message["payload"]["error"].is_null(),
                    "Paseo API request failed ({response})"
                );
                return Ok(message["payload"].clone());
            }
            if matches!(
                message["type"].as_str(),
                Some("agent_stream" | "agent.timeline.replacement")
            ) {
                let size = message.to_string().len();
                ensure!(
                    self.pending.len() < 128 && self.pending_bytes + size <= 2 * 1024 * 1024,
                    "Paseo stream overflow while fetching history"
                );
                self.pending_bytes += size;
                self.pending.push_back((message.clone(), size));
            }
        }
    }

    pub fn next(&mut self, cancel: &AtomicBool) -> Result<Value> {
        if let Some((message, size)) = self.pending.pop_front() {
            self.pending_bytes -= size;
            return Ok(message);
        }
        loop {
            let envelope = self.receive(cancel, None)?;
            if envelope["type"] == "session" {
                return Ok(envelope["message"].clone());
            }
        }
    }

    fn send(&mut self, value: Value, cancel: &AtomicBool) -> Result<()> {
        match self.socket.write(Message::Text(value.to_string().into())) {
            Ok(()) => {}
            Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error).context("write Paseo API message"),
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            check(cancel, deadline)?;
            match self.socket.flush() {
                Ok(()) => return Ok(()),
                Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error).context("flush Paseo API message"),
            }
        }
    }

    fn receive(&mut self, cancel: &AtomicBool, deadline: Option<Instant>) -> Result<Value> {
        loop {
            ensure!(!cancel.load(Ordering::Relaxed), "Paseo stream canceled");
            if let Some(deadline) = deadline {
                check(cancel, deadline)?;
            }
            if self.last_ping.elapsed() >= Duration::from_secs(15) {
                self.send(json!({"type":"ping"}), cancel)?;
                self.last_ping = Instant::now();
            }
            ensure!(
                self.last_received.elapsed() < Duration::from_secs(45),
                "Paseo API stopped responding"
            );
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    self.last_received = Instant::now();
                    return serde_json::from_str(&text).context("invalid Paseo API message");
                }
                Ok(Message::Ping(_)) => {
                    self.last_received = Instant::now();
                    self.socket.flush().ok();
                }
                Ok(Message::Close(_)) => bail!("Paseo API disconnected"),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error).context("read Paseo API message"),
            }
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}

fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Paseo stream canceled");
    ensure!(Instant::now() < deadline, "Paseo API timed out");
    Ok(())
}

enum Transport {
    Tcp(TcpStream),
    Local(Stream),
}

impl Transport {
    fn connect(listen: &str) -> Result<(Self, String)> {
        if let Some(path) = listen.strip_prefix("unix://") {
            let stream = ConnectOptions::new()
                .name(Path::new(path).to_fs_name::<interprocess::local_socket::GenericFilePath>()?)
                .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
                .nonblocking_stream(true)
                .connect_sync()
                .context("connect to Paseo Unix socket")?;
            return Ok((Self::Local(stream), "ws://localhost/ws".into()));
        }
        if listen.starts_with("pipe://") || listen.starts_with(r"\\.\pipe\") {
            let name = listen.strip_prefix("pipe://").unwrap_or(listen);
            let name = name.strip_prefix(r"\\.\pipe\").unwrap_or(name);
            ensure!(cfg!(windows), "Paseo named pipes require Windows");
            let stream = ConnectOptions::new()
                .name(name.to_ns_name::<interprocess::local_socket::GenericNamespaced>()?)
                .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
                .nonblocking_stream(true)
                .connect_sync()
                .context("connect to Paseo named pipe")?;
            return Ok((Self::Local(stream), "ws://localhost/ws".into()));
        }
        let address = listen.strip_prefix("tcp://").unwrap_or(listen);
        let (host, port) = address
            .rsplit_once(':')
            .context("invalid Paseo TCP endpoint")?;
        let port: u16 = port.parse().context("invalid Paseo port")?;
        let host = host.trim_matches(['[', ']']);
        let ip: IpAddr = if host == "localhost" {
            "127.0.0.1".parse().unwrap()
        } else {
            host.parse()
                .context("Paseo endpoint must be a local IP address")?
        };
        ensure!(
            ip.is_loopback() || ip.is_unspecified(),
            "Paseo endpoint must be local; refusing to send local credentials remotely"
        );
        let ip = if ip.is_unspecified() {
            if ip.is_ipv4() { "127.0.0.1" } else { "::1" }
                .parse()
                .unwrap()
        } else {
            ip
        };
        let address = SocketAddr::new(ip, port);
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .context("connect to Paseo daemon")?;
        stream.set_nonblocking(true)?;
        Ok((Self::Tcp(stream), format!("ws://{address}/ws")))
    }
}

#[cfg(windows)]
fn pipe_readable(stream: &Stream) -> io::Result<()> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;
    let Stream::NamedPipe(pipe) = stream;
    let mut available = 0;
    // PIPE_NOWAIT reads with no data become EOF in interprocess; probe first.
    let result = unsafe {
        PeekNamedPipe(
            pipe.as_handle().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    if available == 0 {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    Ok(())
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(bytes),
            Self::Local(stream) => {
                #[cfg(windows)]
                if !bytes.is_empty() {
                    pipe_readable(stream)?;
                }
                stream.read(bytes)
            }
        }
    }
}
impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(bytes),
            Self::Local(stream) => {
                let size = stream.write(bytes)?;
                if size == 0 && !bytes.is_empty() {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                Ok(size)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            Self::Local(stream) => stream.flush(),
        }
    }
}
