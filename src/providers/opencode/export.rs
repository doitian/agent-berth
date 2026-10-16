use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::transcript::{Conversation, Entry, Record, turns};

/// Full session JSON from OpenCode's own CLI. OpenCode keeps sessions in a
/// service-owned database rather than a log file, so `session export` is its
/// only stable, plain-text transcript source.
///
/// The export is captured through a file, not a pipe: OpenCode's CLI exits
/// before its stdout pipe drains, which truncates large sessions, while a file
/// redirect is complete.
pub(super) fn export(session_id: &str) -> Result<Conversation> {
    let binary =
        crate::paths::find_executable("opencode").unwrap_or_else(|| PathBuf::from("opencode"));
    let path = export_path();
    let result = capture_export(&binary, session_id, &path).and_then(|()| {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let export: Value =
            serde_json::from_slice(&bytes).context("parse opencode session export")?;
        Ok(conversation(&export))
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

/// OpenCode's exported messages as transcript records, skipping reasoning and
/// system records. Each user message opens a turn, and `idle` closes it.
fn records(export: &Value) -> Vec<Record> {
    let Some(messages) = export["messages"].as_array() else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for message in messages {
        match message["type"].as_str() {
            Some("user") => records.push(Record {
                entries: message["text"]
                    .as_str()
                    .map(|text| Entry::Prompt { text: text.into() })
                    .into_iter()
                    .collect(),
                starts_turn: true,
                ends_turn: false,
            }),
            Some("assistant") => {
                let Some(parts) = message["content"].as_array() else {
                    continue;
                };
                let mut entries = Vec::new();
                for part in parts {
                    match part["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = part["text"].as_str() {
                                entries.push(Entry::Text { text: text.into() });
                            }
                        }
                        Some("tool") => {
                            let state = &part["state"];
                            entries.push(Entry::ToolCall {
                                name: part["name"].as_str().unwrap_or("call").into(),
                                input: if state["input"].is_null() {
                                    String::new()
                                } else {
                                    state["input"].to_string()
                                },
                            });
                            if let Some(text) = tool_result(state) {
                                entries.push(Entry::ToolResult { text });
                            }
                        }
                        _ => {}
                    }
                }
                records.push(Record {
                    entries,
                    ..Record::default()
                });
            }
            Some("idle") => records.push(Record {
                ends_turn: true,
                ..Record::default()
            }),
            _ => {}
        }
    }
    records
}

fn conversation(export: &Value) -> Conversation {
    Conversation::Turns(turns(records(export)))
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

#[cfg(test)]
#[path = "export_tests.rs"]
mod tests;
