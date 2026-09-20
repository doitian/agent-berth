use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

const DEFAULT_CONFIG: &str = ".tmux-up.conf";

pub fn session_name(root: &Path) -> String {
    let mut session = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "agent-berth".into());
    if session.starts_with('.') {
        session.remove(0);
    }
    session.replace(['.', ':'], "_")
}

pub fn ensure_session(root: &Path) -> Result<String> {
    if !root.is_dir() {
        bail!("{} does not exist", root.display());
    }
    let session = session_name(root);
    let target = format!("={session}");
    if tmux(["has-session", "-t", &target])?.success() {
        return Ok(session);
    }
    let status = tmux([
        "new",
        "-d",
        "-c",
        &root.display().to_string(),
        "-s",
        &session,
    ])?;
    if !status.success() {
        bail!("tmux new-session failed");
    }
    apply_config(root, &target)?;
    Ok(session)
}

pub fn new_window(session: &str, name: &str, cwd: &Path, cmd: &[String]) -> Result<()> {
    let target = format!("={session}");
    let mut args = vec![
        "new-window".into(),
        "-t".into(),
        target,
        "-n".into(),
        name.into(),
        "-c".into(),
        cwd.display().to_string(),
        "--".into(),
    ];
    args.extend(cmd.iter().cloned());
    let status = tmux(args)?;
    if !status.success() {
        bail!("tmux new-window failed for {name}");
    }
    Ok(())
}

pub fn attach_or_switch(session: &str) -> Result<()> {
    let target = format!("={session}");
    if std::env::var_os("TMUX").is_some() {
        let status = tmux(["switch-client", "-t", &target])?;
        if !status.success() {
            bail!("tmux switch-client failed");
        }
        return Ok(());
    }
    let status = command()
        .args(["attach", "-t", &target])
        .status()
        .context("tmux attach")?;
    if !status.success() {
        bail!("tmux attach failed");
    }
    Ok(())
}

pub fn available() -> bool {
    crate::paths::on_path("tmux")
}

const PANE_FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{session_name}\t#{window_index}\t#{window_name}\t#{pane_current_path}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub pid: u32,
    pub session: String,
    pub window: String,
    pub window_name: String,
    pub path: String,
}

impl Pane {
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split('\t');
        let id = fields.next()?.trim().to_string();
        let pid = fields.next()?.trim().parse().ok()?;
        let session = fields.next()?.to_string();
        let window = fields.next()?.to_string();
        let window_name = fields.next()?.to_string();
        let path = fields.next()?.to_string();
        if id.is_empty() {
            return None;
        }
        Some(Self {
            id,
            pid,
            session,
            window,
            window_name,
            path,
        })
    }
}

pub fn list_panes(all: bool) -> Result<Vec<Pane>> {
    let scope = if all { "-a" } else { "-s" };
    let output = command()
        .args(["list-panes", scope, "-F", PANE_FORMAT])
        .output()
        .context("tmux is not available")?;
    if !output.status.success() {
        bail!("tmux list-panes failed");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().filter_map(Pane::parse).collect())
}

pub fn attach_pane(pane: &Pane) -> Result<()> {
    let session = format!("={}", pane.session);
    let window = format!("{session}:{}", pane.window);
    if std::env::var_os("TMUX").is_some() {
        run(["switch-client", "-t", &session])?;
        run(["select-window", "-t", &window])?;
        run(["select-pane", "-t", &pane.id])?;
        return Ok(());
    }
    run(["select-window", "-t", &window])?;
    run(["select-pane", "-t", &pane.id])?;
    let status = command()
        .args(["attach", "-t", &session])
        .status()
        .context("tmux attach")?;
    if !status.success() {
        bail!("tmux attach failed");
    }
    Ok(())
}

fn run<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    if !tmux(args)?.success() {
        bail!("tmux command failed");
    }
    Ok(())
}

/// fzf preview command for `{1}`, the pane id, including the active socket and config.
pub fn preview_command() -> String {
    let mut parts = vec!["tmux".to_string(), "-u".to_string()];
    if let Some(socket) = crate::paths::env_path("AGENT_BERTH_TMUX_SOCKET") {
        parts.push("-L".into());
        parts.push(quote_arg(&socket.display().to_string()));
    }
    if let Some(config) = crate::paths::env_path("AGENT_BERTH_TMUX_CONFIG") {
        parts.push("-f".into());
        parts.push(quote_arg(&config.display().to_string()));
    }
    parts.push("capture-pane -p -e -t {1}".into());
    parts.join(" ")
}

fn quote_arg(value: &str) -> String {
    if value.chars().any(char::is_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

fn apply_config(root: &Path, target: &str) -> Result<()> {
    let config = root.join(DEFAULT_CONFIG);
    let mut commands = if config.is_file() {
        let mut text = std::fs::read_to_string(&config)?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
        text
    } else {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".into());
        format!("send '{editor}' Enter\nneww -n shell\nselectw -t 1\n\n")
    };
    commands.push_str("detach-client\n");
    let mut child = command()
        .args(["-C", "attach", "-t", target])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .env("TMUX", "")
        .spawn()
        .context("tmux control mode")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(commands.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("tmux control mode failed");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tmux_tests.rs"]
mod tests;

fn tmux<I, S>(args: I) -> Result<std::process::ExitStatus>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    command()
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("tmux is not available")
}

fn command() -> Command {
    let mut command = Command::new("tmux");
    // Force UTF-8 output; a non-UTF-8 locale makes tmux replace the tabs in
    // format output with underscores.
    command.arg("-u");
    if let Some(socket) = crate::paths::env_path("AGENT_BERTH_TMUX_SOCKET") {
        command.arg("-L").arg(socket);
    }
    if let Some(config) = crate::paths::env_path("AGENT_BERTH_TMUX_CONFIG") {
        command.arg("-f").arg(config);
    }
    command
}
