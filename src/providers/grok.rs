use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::{
    Focus, HookEvents, Integration, Messaging, Provider, Transcript, command_handler, load_object,
    save_object,
};
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

pub struct Grok;

impl Provider for Grok {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn logo(&self) -> &'static str {
        "𝕏"
    }

    fn resume_cmd(&self, session_id: &str) -> Vec<String> {
        vec!["grok".into(), "--resume".into(), session_id.into()]
    }

    fn hooks(&self) -> Option<&dyn HookEvents> {
        Some(self)
    }
}

impl Focus for Grok {}

impl Messaging for Grok {}

impl Transcript for Grok {}

impl HookEvents for Grok {
    fn apply_hook(&self, sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
        apply_hook(sessions, event);
    }
}

fn apply_hook(sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
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

impl Integration for Grok {
    fn install(&self, ctx: &Context) -> Result<PathBuf> {
        let dest = self.hook_path(ctx);
        save_object(&dest, &plan(ctx))?;
        Ok(dest)
    }

    fn uninstall(&self, ctx: &Context) -> Result<()> {
        let dest = self.hook_path(ctx);
        if dest.exists() {
            std::fs::remove_file(&dest)?;
        }
        Ok(())
    }

    fn hook_path(&self, ctx: &Context) -> PathBuf {
        ctx.grok_home.join("hooks").join("agent-berth.json")
    }

    fn outdated(&self, ctx: &Context) -> bool {
        load_object(&self.hook_path(ctx)).is_ok_and(|current| current != plan(ctx))
    }

    fn config_exists(&self, ctx: &Context) -> bool {
        ctx.grok_home.is_dir()
    }
}

fn plan(ctx: &Context) -> Value {
    let handler = command_handler(ctx, Grok.name(), 5, true);
    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        hooks.insert((*event).into(), json!([{"hooks": [handler.clone()]}]));
    }
    json!({"hooks": hooks})
}
