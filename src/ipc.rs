use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener, ListenerOptions, Stream, ToNsName};

use crate::paths::Context as AppContext;
use crate::protocol::{Request, Response};

#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

pub fn send(ctx: &AppContext, request: &Request) -> Result<Response> {
    send_timeout(ctx, request, DEFAULT_TIMEOUT)
}

pub fn send_timeout(ctx: &AppContext, request: &Request, timeout: Duration) -> Result<Response> {
    let mut stream = connect(ctx, timeout)?;
    let mut payload = serde_json::to_vec(request).context("encode IPC request")?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .context("failed to write to agent-bridge IPC channel")?;
    stream
        .flush()
        .context("failed to flush agent-bridge IPC channel")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read agent-bridge IPC response")?;
    if line.trim().is_empty() {
        bail!("agent-bridge server closed the IPC channel");
    }
    serde_json::from_str(&line).context("invalid agent-bridge IPC response")
}

pub fn ping(ctx: &AppContext) -> Result<()> {
    match send(ctx, &Request::Ping)? {
        Response::Ok { .. } => Ok(()),
        Response::Error { message } => bail!("{message}"),
    }
}

pub fn notify(ctx: &AppContext, provider: String, payload: serde_json::Value) -> Result<()> {
    match send(ctx, &Request::Notify { provider, payload })? {
        Response::Ok { .. } => Ok(()),
        Response::Error { message } => bail!("{message}"),
    }
}

pub fn list(
    ctx: &AppContext,
    resumable: bool,
    idle_ms: Option<u64>,
) -> Result<Vec<crate::store::ListedSession>> {
    match send(ctx, &Request::List { resumable, idle_ms })? {
        Response::Ok { sessions, .. } => Ok(sessions.unwrap_or_default()),
        Response::Error { message } => bail!("{message}"),
    }
}

pub fn bind(ctx: &AppContext) -> Result<Listener> {
    #[cfg(unix)]
    {
        let path = ctx.socket_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&path);
        let name = path
            .to_ns_name::<GenericFilePath>()
            .map_err(|err| anyhow::anyhow!("socket name: {err}"))?;
        ListenerOptions::new()
            .name(name)
            .create_sync()
            .context("bind unix socket")
    }
    #[cfg(windows)]
    {
        let raw = ctx.pipe_name();
        let name = raw
            .to_ns_name::<GenericNamespaced>()
            .map_err(|err| anyhow::anyhow!("named pipe name: {err}"))?;
        ListenerOptions::new()
            .name(name)
            .create_sync()
            .context("bind named pipe")
    }
}

fn connect(ctx: &AppContext, timeout: Duration) -> Result<Stream> {
    #[cfg(windows)]
    let raw = ctx.pipe_name();
    #[cfg(unix)]
    let raw = ctx.socket_path();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = connect_owned(raw);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(err)) => Err(err).context("failed to connect to agent-bridge IPC channel"),
        Err(_) => bail!("timed out connecting to agent-bridge IPC channel"),
    }
}

#[cfg(windows)]
fn connect_owned(raw: String) -> Result<Stream> {
    let name = raw
        .to_ns_name::<GenericNamespaced>()
        .map_err(|err| anyhow::anyhow!("named pipe name: {err}"))?;
    Stream::connect(name).context("connect named pipe")
}

#[cfg(unix)]
fn connect_owned(raw: std::path::PathBuf) -> Result<Stream> {
    let name = raw
        .to_ns_name::<GenericFilePath>()
        .map_err(|err| anyhow::anyhow!("socket name: {err}"))?;
    Stream::connect(name).context("connect unix socket")
}
