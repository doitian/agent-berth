use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Full session JSON from OpenCode's own CLI. OpenCode keeps sessions in a
/// service-owned database rather than a log file, so `session export` is its
/// only stable, plain-text transcript source.
///
/// The export is captured through a file, not a pipe: OpenCode's CLI exits
/// before its stdout pipe drains, which truncates large sessions, while a file
/// redirect is complete.
pub(super) fn export(session_id: &str) -> Result<String> {
    let binary =
        crate::paths::find_executable("opencode").unwrap_or_else(|| PathBuf::from("opencode"));
    let path = export_path();
    let result = capture_export(&binary, session_id, &path).and_then(|()| {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let export: Value =
            serde_json::from_slice(&bytes).context("parse opencode session export")?;
        Ok(render(&export))
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
fn render(export: &Value) -> String {
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

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
