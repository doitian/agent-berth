use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::{Value, json};

use super::{command_handler, group_is_ours, load_object, save_object};
use crate::paths::Context;
use crate::status::{
    AgentEvent, AgentEventKind, AgentSession, AgentStatus, Source, apply_event, session_key,
    string_field, u32_field,
};

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Notification",
    "Elicitation",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

const ASK_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "agent_needs_input",
    "elicitation_dialog",
    "elicitation_url_dialog",
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
        "PermissionRequest" => Some(AgentEventKind::PermissionRequest),
        "Elicitation" => Some(AgentEventKind::QuestionAsked),
        "Notification"
            if ASK_NOTIFICATIONS.contains(
                &string_field(event, &["notification_type", "notificationType"]).unwrap_or(""),
            ) =>
        {
            Some(AgentEventKind::PermissionRequest)
        }
        "Stop" | "StopFailure" => Some(AgentEventKind::Stop),
        _ => None,
    };
    let Some(kind) = kind else {
        return;
    };
    let background_running = kind == AgentEventKind::Stop && background_running(event);
    let parent_id = string_field(event, &["agent_id", "agentId"]).map(|_| {
        string_field(event, &["session_id", "sessionId"])
            .unwrap_or("")
            .to_string()
    });
    apply_event(
        sessions,
        AgentEvent {
            session_id: sid,
            kind,
            source: source_for(event),
            pid: u32_field(event, &["pid"]),
            parent_id,
            background_running,
        },
    );
}

fn source_for(event: &Value) -> Source {
    if let Some(entry) = string_field(event, &["entrypoint"])
        && entry.to_ascii_lowercase().contains("desktop")
    {
        return Source::Desktop;
    }
    Source::Cli
}

fn background_running(event: &Value) -> bool {
    event
        .get("background_tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| {
            tasks.iter().any(|task| {
                task.as_object()
                    .is_some_and(|_| agent_kind(task) == AgentStatus::Working)
            })
        })
}

fn is_present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(false) => false,
        Value::String(text) if text.is_empty() => false,
        _ => true,
    }
}

pub fn discover(ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
    let index = desktop_index(ctx);
    if let Some(rows) = list_agents() {
        apply_agents(sessions, &rows, &index.live);
    }
    drop_archived(sessions, &index.archived)
}

pub fn drop_archived(
    sessions: &mut BTreeMap<String, AgentSession>,
    archived: &HashSet<String>,
) -> bool {
    let before = sessions.len();
    sessions.retain(|sid, _| !archived.contains(sid));
    sessions.len() != before
}

pub fn list_agents() -> Option<Vec<Value>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(claude_agents_output());
    });
    let output = rx.recv_timeout(Duration::from_secs(2)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let data: Value = serde_json::from_slice(&output.stdout).ok()?;
    data.as_array().cloned()
}

fn claude_agents_output() -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("claude");
    cmd.args(["agents", "--json"])
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.output()
}

struct DesktopIndex {
    live: HashSet<String>,
    archived: HashSet<String>,
}

fn desktop_index(ctx: &Context) -> DesktopIndex {
    let mut index = DesktopIndex {
        live: HashSet::new(),
        archived: HashSet::new(),
    };
    for root in desktop_session_dirs(ctx) {
        collect_desktop_ids(&root, &mut index);
    }
    index
}

fn desktop_session_dirs(ctx: &Context) -> Vec<PathBuf> {
    if let Ok(override_dir) = std::env::var("CLAUDE_DESKTOP_SESSIONS")
        && !override_dir.is_empty()
    {
        return vec![PathBuf::from(override_dir)];
    }
    let mut dirs = vec![
        ctx.xdg_config_home
            .join("Claude")
            .join("claude-code-sessions"),
    ];
    if let Some(appdata) = &ctx.appdata {
        dirs.push(appdata.join("Claude").join("claude-code-sessions"));
    }
    dirs.push(
        ctx.home
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude-code-sessions"),
    );
    dirs
}

fn collect_desktop_ids(root: &Path, index: &mut DesktopIndex) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_ids(&path, index);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let archived = data
            .get("isArchived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        for key in ["cliSessionId", "sessionId"] {
            let Some(id) = data
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            if archived {
                index.archived.insert(id.to_string());
            } else {
                index.live.insert(id.to_string());
            }
        }
    }
}

pub fn apply_agents(
    sessions: &mut BTreeMap<String, AgentSession>,
    rows: &[Value],
    desktop_ids: &HashSet<String>,
) {
    let now = now_ms();
    let mut live = HashSet::new();
    for row in rows {
        let Some(sid) = string_field(row, &["sessionId", "id"]).map(str::to_string) else {
            continue;
        };
        let state = string_field(row, &["state"]).unwrap_or("");
        if matches!(state, "done" | "failed" | "stopped") {
            apply_event(sessions, AgentEvent::new(sid, AgentEventKind::SessionEnd));
            continue;
        }
        live.insert(sid.clone());
        let status = agent_kind(row);
        let pid = u32_field(row, &["pid"]);
        let cwd = string_field(row, &["cwd"]).map(str::to_string);
        let desktop = desktop_ids.contains(&sid);
        if let Some(existing) = sessions.get_mut(&sid)
            && status == AgentStatus::Idle
        {
            if pid.is_some() {
                existing.pid = pid;
            }
            if existing.background_only
                && existing.status != AgentStatus::Waiting
                && matches!(string_field(row, &["status", "state"]), Some("idle"))
            {
                existing.status = AgentStatus::Done;
                existing.background_only = false;
            }
            if desktop {
                existing.source = Source::Desktop;
            }
            if let Some(cwd) = cwd {
                existing.cwd = Some(cwd);
            }
            if let Some(title) = string_field(row, &["name", "title"]).map(str::to_string) {
                existing.title = Some(title);
            }
            continue;
        }
        let item = sessions.entry(sid.clone()).or_insert_with(|| AgentSession {
            discovered: true,
            created_ms: crate::store::now_ms(),
            ..AgentSession::default()
        });
        item.status = status;
        if pid.is_some() {
            item.pid = pid;
        }
        if let Some(cwd) = cwd {
            item.cwd = Some(cwd);
        }
        if desktop {
            item.source = Source::Desktop;
        }
        if let Some(title) = string_field(row, &["name", "title"]).map(str::to_string) {
            item.title = Some(title);
        }
        if item.cmdline.is_empty() {
            item.cmdline = super::resume_cmd("claude", &sid);
        }
        if item.discovered || item.last_report_ms == 0 {
            item.last_report_ms = now;
        }
    }
    sessions.retain(|sid, session| {
        live.contains(sid) || (!session.discovered && session.source != Source::Desktop)
    });
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn agent_kind(row: &Value) -> AgentStatus {
    let waiting = row.get("waitingFor");
    let status = string_field(row, &["status"]).unwrap_or("");
    let state = string_field(row, &["state"]).unwrap_or("");
    if waiting.is_some_and(is_present) || status == "waiting" || state == "blocked" {
        return AgentStatus::Waiting;
    }
    if matches!(state, "working" | "running" | "busy")
        || matches!(status, "busy" | "running" | "working")
    {
        return AgentStatus::Working;
    }
    AgentStatus::Idle
}

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.claude_config_dir.join("settings.json");
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
    let handler = command_handler(ctx, "claude", 5, true);
    let matcher = ASK_NOTIFICATIONS.join("|");
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
                    .filter(|group| !group_is_ours(group, "claude"))
                    .cloned(),
            );
        }
        let mut group = json!({"hooks": [handler.clone()]});
        if *event == "Notification" {
            group["matcher"] = Value::String(matcher.clone());
        }
        if *event == "SessionEnd" {
            group["hooks"][0]["async"] = Value::Bool(false);
        }
        kept.push(group);
        *groups = Value::Array(kept);
    }
    save_object(&dest, &data)?;
    Ok(dest)
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    let dest = ctx.claude_config_dir.join("settings.json");
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
        groups.retain(|group| !group_is_ours(group, "claude"));
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
#[path = "claude_tests.rs"]
mod tests;
