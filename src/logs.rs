use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::attach;
use crate::db;
use crate::paths::Context as AppContext;
use crate::pick;
use crate::store::ListedSession;
use crate::tmux;
use crate::transcript;

pub fn run(ctx: &AppContext, selector: &str, tail: Option<usize>) -> Result<()> {
    let sessions = db::query_stored(ctx)?;
    let Some(session) = pick::single(&sessions, selector)? else {
        return Ok(());
    };
    let full = transcript_for(ctx, &session)?;
    let text = match tail {
        Some(lines) => tail_lines(&full, lines),
        None => full,
    };
    if !text.is_empty() {
        println!("{text}");
    }
    Ok(())
}

/// Best available transcript, in order: OpenCode's CLI export, a local file
/// transcript, then the session's tmux pane. The first source that yields text
/// wins; otherwise the error reports every attempt.
fn transcript_for(ctx: &AppContext, session: &ListedSession) -> Result<String> {
    let mut errors: Vec<String> = Vec::new();
    if session.provider == "opencode" {
        match opencode_export(&session.session_id) {
            Ok(text) => return Ok(text),
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    if transcript::supports(&session.provider) {
        match transcript::full(&source(ctx, session)) {
            Ok(text) => return Ok(text),
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    match pane_transcript(session) {
        Ok(Some(text)) => return Ok(text),
        Ok(None) => errors.push("no tmux pane found".into()),
        Err(error) => errors.push(format!("tmux capture: {error:#}")),
    }
    bail!(
        "no transcript for {} {}: {}",
        session.provider,
        session.session_id,
        errors.join("; ")
    )
}

/// Full session JSON from OpenCode's own CLI. OpenCode keeps sessions in a
/// service-owned database rather than a log file, so `session export` is its
/// only stable, plain-text transcript source.
///
/// The export is captured through a file, not a pipe: OpenCode's CLI exits
/// before its stdout pipe drains, which truncates large sessions, while a file
/// redirect is complete.
fn opencode_export(session_id: &str) -> Result<String> {
    let binary =
        crate::paths::find_executable("opencode").unwrap_or_else(|| PathBuf::from("opencode"));
    let path = export_path();
    let result = capture_export(&binary, session_id, &path).and_then(|()| {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let export: Value =
            serde_json::from_slice(&bytes).context("parse opencode session export")?;
        Ok(render_opencode(&export))
    });
    let _ = std::fs::remove_file(&path);
    result
}

fn capture_export(binary: &Path, session_id: &str, path: &Path) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let child = Command::new(binary)
        .args(["session", "export", session_id])
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()
        .context("run opencode session export")?;
    let output = child
        .wait_with_output()
        .context("wait for opencode session export")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    bail!(
        "opencode session export failed{}",
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    );
}

fn export_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "agent-berth-opencode-{}-{seq}.json",
        std::process::id()
    ))
}

/// Render OpenCode's exported messages with the same labels as the file
/// transcripts, skipping reasoning and system records.
fn render_opencode(export: &Value) -> String {
    let Some(messages) = export["messages"].as_array() else {
        return String::new();
    };
    let mut entries: Vec<String> = Vec::new();
    for message in messages {
        let mut entry = String::new();
        match message["type"].as_str() {
            Some("user") => {
                if let Some(text) = message["text"].as_str() {
                    push_line(&mut entry, "You", text);
                }
            }
            Some("assistant") => {
                let Some(parts) = message["content"].as_array() else {
                    continue;
                };
                for part in parts {
                    match part["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = part["text"].as_str() {
                                push_line(&mut entry, "Assistant", text);
                            }
                        }
                        Some("tool") => {
                            push_line(&mut entry, "Tool", part["name"].as_str().unwrap_or("call"));
                            let state = &part["state"];
                            if !state["input"].is_null() {
                                push_line(&mut entry, "Input", &state["input"].to_string());
                            }
                            if let Some(result) = tool_result(state) {
                                push_line(&mut entry, "Result", &result);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => continue,
        }
        if !entry.is_empty() {
            entries.push(entry);
        }
    }
    entries.join("\n\n")
}

fn tool_result(state: &Value) -> Option<String> {
    if let Some(text) = state["output"].as_str().filter(|text| !text.is_empty()) {
        return Some(text.to_string());
    }
    let parts = state["content"].as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn push_line(entry: &mut String, label: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    if !entry.is_empty() {
        entry.push('\n');
    }
    entry.push_str(label);
    entry.push_str(": ");
    entry.extend(
        text.chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t')),
    );
}

fn source(ctx: &AppContext, session: &ListedSession) -> transcript::Source {
    transcript::Source {
        provider: session.provider.clone(),
        session_id: session.session_id.clone(),
        path: session.transcript_path.clone(),
        root: transcript_root(ctx, &session.provider),
    }
}

/// Local transcript root for a provider, mirroring the TUI's discovery roots.
fn transcript_root(ctx: &AppContext, provider: &str) -> Option<PathBuf> {
    match provider {
        "claude" => Some(ctx.claude_config_dir.join("projects")),
        "codex" => Some(ctx.codex_home.join("sessions")),
        "pi" => Some(ctx.pi_dir.join("sessions")),
        _ => None,
    }
}

/// Best-effort transcript from the session's tmux pane, including scrollback.
/// Only for sessions that resolve to a pane; otherwise `None` so the caller
/// reports why the other sources were unavailable.
fn pane_transcript(session: &ListedSession) -> Result<Option<String>> {
    if !tmux::available() {
        return Ok(None);
    }
    let panes = tmux::list_panes(true)?;
    let Some(resolved) = attach::resolve_session_pane(&panes, session, std::process::id()) else {
        return Ok(None);
    };
    Ok(Some(tmux::capture_pane_transcript(&resolved.pane.id)?))
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
