use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{Listener, ListenerOptions, Stream};

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
        .context("failed to write to agent-berth IPC channel")?;
    stream
        .flush()
        .context("failed to flush agent-berth IPC channel")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read agent-berth IPC response")?;
    if line.trim().is_empty() {
        bail!("agent-berth server closed the IPC channel");
    }
    serde_json::from_str(&line).context("invalid agent-berth IPC response")
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

pub fn list_all(ctx: &AppContext) -> Result<Vec<crate::store::ListedSession>> {
    match send(ctx, &Request::ListAll)? {
        Response::Ok { sessions, .. } => Ok(sessions.unwrap_or_default()),
        Response::Error { message } => bail!("{message}"),
    }
}

pub fn stats(ctx: &AppContext) -> Result<Vec<crate::store::ProviderStats>> {
    match send(ctx, &Request::Stats)? {
        Response::Ok { stats, .. } => Ok(stats.unwrap_or_default()),
        Response::Error { message } => bail!("{message}"),
    }
}

pub fn remove(ctx: &AppContext, provider: &str, session_id: &str) -> Result<()> {
    let request = Request::Remove {
        provider: provider.to_string(),
        session_id: session_id.to_string(),
    };
    match send(ctx, &request)? {
        Response::Ok { .. } => Ok(()),
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
            .to_fs_name::<GenericFilePath>()
            .map_err(|err| anyhow::anyhow!("socket name: {err}"))?;
        ListenerOptions::new()
            .name(name)
            .create_sync()
            .context("bind unix socket")
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;

        let raw = ctx.pipe_name();
        let name = raw
            .to_ns_name::<GenericNamespaced>()
            .map_err(|err| anyhow::anyhow!("named pipe name: {err}"))?;
        ListenerOptions::new()
            .name(name)
            .security_descriptor(pipe_security_descriptor()?)
            .create_sync()
            .context("bind named pipe")
    }
}

#[cfg(windows)]
fn pipe_security_descriptor()
-> Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    let sid = current_user_sid().context("current user SID")?;
    let sddl = format!("D:(A;;GA;;;SY)(A;;GA;;;S-1-5-32-544)(A;;GA;;;{sid})");
    let wide = widestring::U16CString::from_str(&sddl)
        .map_err(|err| anyhow::anyhow!("encode pipe security descriptor: {err}"))?;
    SecurityDescriptor::deserialize(wide.as_ucstr()).context("named pipe security descriptor")
}

#[cfg(windows)]
fn current_user_sid() -> Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error()).context("OpenProcessToken");
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buffer = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("GetTokenInformation");
        }
        let user = std::ptr::read_unaligned(buffer.as_ptr() as *const TOKEN_USER);
        let mut sid_text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid_text) == 0 {
            return Err(std::io::Error::last_os_error()).context("ConvertSidToStringSidW");
        }
        let mut end = 0;
        while *sid_text.add(end) != 0 {
            end += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, end));
        LocalFree(sid_text as HLOCAL);
        Ok(sid)
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
        Ok(Err(err)) => Err(err).context("failed to connect to agent-berth IPC channel"),
        Err(_) => bail!("timed out connecting to agent-berth IPC channel"),
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
        .to_fs_name::<GenericFilePath>()
        .map_err(|err| anyhow::anyhow!("socket name: {err}"))?;
    Stream::connect(name).context("connect unix socket")
}
