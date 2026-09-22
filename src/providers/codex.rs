use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use super::{command_handler, group_is_ours, load_object, save_object};
use crate::paths::Context;
use crate::status::{
    AgentEvent, AgentEventKind, AgentSession, Source, apply_event, session_key, string_field,
    u32_field,
};

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
    "SubagentStop",
    "SessionEnd",
];

pub fn apply_hook(sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
    let Some(sid) = session_key(event) else {
        return;
    };
    let Some(name) = string_field(event, &["hook_event_name", "hookEventName"]) else {
        return;
    };
    let kind = match name {
        "SessionStart" => Some(AgentEventKind::SessionStart),
        "UserPromptSubmit" => Some(AgentEventKind::PromptSubmit),
        "PreToolUse" => Some(AgentEventKind::ToolStart),
        "PostToolUse" => Some(AgentEventKind::ToolComplete),
        "SessionEnd" => Some(AgentEventKind::SessionEnd),
        "PermissionRequest" => Some(if uses_auto_review(event) {
            AgentEventKind::PermissionReview
        } else {
            AgentEventKind::PermissionRequest
        }),
        "Stop" | "Interrupt" => Some(AgentEventKind::Stop),
        "SubagentStop" => {
            if string_field(event, &["agent_id", "agentId"]).is_none() {
                return;
            }
            Some(AgentEventKind::SessionEnd)
        }
        _ => None,
    };
    let Some(kind) = kind else {
        return;
    };
    let parent_id = string_field(event, &["agent_id", "agentId"]).map(|_| {
        string_field(event, &["session_id", "sessionId"])
            .unwrap_or("")
            .to_string()
    });
    let existing = sessions
        .get(&sid)
        .or_else(|| parent_id.as_ref().and_then(|id| sessions.get(id)));
    let source = source_for(event, existing);
    apply_event(
        sessions,
        AgentEvent {
            session_id: sid,
            kind,
            source,
            pid: u32_field(event, &["pid"]),
            parent_id,
            background_running: false,
        },
    );
}

fn uses_auto_review(event: &Value) -> bool {
    let (Some(path), Some(turn_id)) = (
        string_field(event, &["transcript_path"]),
        string_field(event, &["turn_id"]),
    ) else {
        return false;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    // PermissionRequest also fires for automatic reviews. The hook omits the
    // reviewer, but Codex records it in the active turn's transcript context.
    let mut auto_review = false;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"turn_context\"") {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if row.get("type").and_then(Value::as_str) != Some("turn_context") {
            continue;
        }
        let Some(context) = row.get("payload") else {
            continue;
        };
        if string_field(context, &["turn_id"]) == Some(turn_id) {
            auto_review = string_field(context, &["approvals_reviewer"]) == Some("auto_review");
        }
    }
    auto_review
}

fn source_for(event: &Value, existing: Option<&AgentSession>) -> Source {
    if let Some(origin) = string_field(event, &["originator"]) {
        return Source::from_label(origin);
    }
    existing.map(|session| session.source).unwrap_or_default()
}

fn desktop_url(session_id: &str) -> Result<String> {
    anyhow::ensure!(
        session_id.len() == 36
            && session_id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }),
        "invalid Codex desktop session ID"
    );
    Ok(format!("codex://threads/{session_id}"))
}

pub fn focus_desktop(session_id: &str) -> Result<()> {
    super::open_desktop_url(&desktop_url(session_id)?)
}

pub fn discover(ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
    let mut changed = drop_archived(ctx, sessions);
    let Ok(index) = std::fs::read_to_string(ctx.codex_home.join("session_index.jsonl")) else {
        return changed;
    };
    let mut seen = HashSet::new();
    // The index is append-only; the last valid name for each session wins.
    for line in index.lines().rev() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(title)) = (
            string_field(&row, &["id"]),
            string_field(&row, &["thread_name"]),
        ) else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        if let Some(session) = sessions.get_mut(id)
            && session.title.as_deref() != Some(title)
        {
            session.title = Some(title.to_string());
            changed = true;
        }
    }
    changed
}

pub fn drop_archived(ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
    let archived = session_ids_in(&ctx.codex_home.join("archived_sessions"));
    let known = {
        let mut ids = session_ids_in(&ctx.codex_home.join("sessions"));
        ids.extend(archived.iter().cloned());
        ids
    };
    let before = sessions.len();
    sessions.retain(|sid, session| {
        if archived.contains(sid) {
            return false;
        }
        session.source != Source::Desktop || known.contains(sid)
    });
    sessions.len() != before
}

fn session_ids_in(root: &Path) -> HashSet<String> {
    let mut ids = HashSet::new();
    collect_session_ids(root, &mut ids);
    ids
}

fn collect_session_ids(root: &Path, ids: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_ids(&path, ids);
            continue;
        }
        if let Some(id) = session_id_from_rollout(&path) {
            ids.insert(id);
        }
    }
}

fn session_id_from_rollout(path: &Path) -> Option<String> {
    let line = std::fs::File::open(path)
        .ok()
        .and_then(|file| BufReader::new(file).lines().next())
        .and_then(Result::ok);
    if let Some(line) = line
        && let Ok(data) = serde_json::from_str::<Value>(&line)
    {
        let payload = if data.get("type").and_then(Value::as_str) == Some("session_meta") {
            data.get("payload").cloned().unwrap_or(data)
        } else {
            data
        };
        if let Some(id) = string_field(&payload, &["session_id", "id"]) {
            return Some(id.to_string());
        }
    }
    let stem = path.file_stem()?.to_str()?;
    let parts: Vec<_> = stem.split('-').collect();
    if parts.len() >= 5 {
        return Some(parts[parts.len() - 5..].join("-"));
    }
    None
}

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.codex_home.join("hooks.json");
    let mut data = load_object(&dest)?;
    if !data.is_object() {
        data = json!({});
    }
    let hooks = data
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    // Codex ignores async hooks in `exec` mode, so install synchronous hooks.
    let handler = command_handler(ctx, "codex", 3, false);
    for event in HOOK_EVENTS {
        let groups = hooks
            .as_object_mut()
            .unwrap()
            .entry(*event)
            .or_insert_with(|| json!([]));
        let mut kept = Vec::new();
        if let Some(items) = groups.as_array() {
            kept.extend(
                items
                    .iter()
                    .filter(|group| !group_is_ours(group, "codex"))
                    .cloned(),
            );
        }
        let group = json!({"hooks": [handler.clone()]});
        kept.push(group);
        *groups = Value::Array(kept);
    }
    save_object(&dest, &data)?;
    Ok(dest)
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    let dest = ctx.codex_home.join("hooks.json");
    if !dest.exists() {
        return Ok(());
    }
    let mut data = load_object(&dest)?;
    let Some(hooks) = data.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
            continue;
        };
        groups.retain(|group| !group_is_ours(group, "codex"));
        if groups.is_empty() {
            hooks.remove(&event);
        }
    }
    if hooks.is_empty() {
        data.as_object_mut().unwrap().remove("hooks");
    }
    save_object(&dest, &data)?;
    Ok(())
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
