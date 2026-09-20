use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::{command_handler, load_object, save_object};
use crate::paths::Context;
use crate::status::{
    AgentEvent, AgentEventKind, AgentSession, apply_event, string_field, u32_field,
};

const ASK_EVENTS: &[&str] = &["PermissionRequest"];
const IDLE_EVENTS: &[&str] = &["Stop", "StopFailure", "StopCancelled"];
const ASK_NOTIFICATIONS: &[&str] = &["permission_prompt", "elicitation_dialog"];
const IDLE_NOTIFICATIONS: &[&str] = &["idle_prompt"];
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "StopFailure",
    "StopCancelled",
    "Notification",
    "SubagentStop",
    "SessionEnd",
];

pub fn apply_hook(sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
    if string_field(event, &["subagentType", "subagent_type"]).is_some() {
        return;
    }
    let Some(sid) = string_field(event, &["session_id", "sessionId"]) else {
        return;
    };
    let Some(raw_name) = string_field(event, &["hook_event_name", "hookEventName"]) else {
        return;
    };
    let name = pascal(raw_name);
    if name.is_empty() {
        return;
    }
    let notify = string_field(event, &["notification_type", "notificationType"]).unwrap_or("");
    let kind = if ASK_EVENTS.contains(&name.as_str())
        || (name == "Notification" && ASK_NOTIFICATIONS.contains(&notify))
    {
        Some(AgentEventKind::PermissionRequest)
    } else if IDLE_EVENTS.contains(&name.as_str())
        || (name == "Notification" && IDLE_NOTIFICATIONS.contains(&notify))
    {
        Some(AgentEventKind::Stop)
    } else {
        match name.as_str() {
            "SessionStart" => Some(AgentEventKind::SessionStart),
            "UserPromptSubmit" => Some(AgentEventKind::PromptSubmit),
            "PreToolUse" => Some(AgentEventKind::ToolStart),
            "PostToolUse" => Some(AgentEventKind::ToolComplete),
            "SessionEnd" => Some(AgentEventKind::SessionEnd),
            _ => None,
        }
    };
    let Some(kind) = kind else {
        return;
    };
    apply_event(
        sessions,
        AgentEvent {
            session_id: sid.to_string(),
            kind,
            source: crate::status::Source::Cli,
            pid: u32_field(event, &["pid"]),
            parent_id: None,
            background_running: false,
        },
    );
}

fn pascal(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    if name.contains('_') || name == name.to_ascii_lowercase() {
        name.replace('-', "_")
            .split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect()
    } else {
        name.to_string()
    }
}

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.grok_home.join("hooks").join("agent-berth.json");
    let handler = command_handler(ctx, "grok", 5, true);
    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        hooks.insert((*event).into(), json!([{"hooks": [handler.clone()]}]));
    }
    save_object(&dest, &json!({"hooks": hooks}))?;
    Ok(dest)
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    let dest = ctx.grok_home.join("hooks").join("agent-berth.json");
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }
    let _ = load_object(&dest);
    Ok(())
}
