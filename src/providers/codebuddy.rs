use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::{
    Focus, HookEvents, Integration, Messaging, Provider, Transcript, TranscriptLog,
    command_handler, group_is_ours, load_object, log_preview, log_transcript, save_object,
};
use crate::hubs::{Hub, Preview};
use crate::paths::Context;
use crate::status::{
    AgentEvent, AgentEventKind, AgentSession, Source, apply_event, session_key, string_field,
    u32_field,
};
use crate::store::ListedSession;
use crate::transcript::{
    Conversation, Entry, Record, content, context, first_text, prompt, said, tool_result,
};

/// User rows CodeBuddy writes itself: command output, slash-command markers,
/// and the context blocks it injects at the start of a turn.
const INJECTED_PREFIXES: &[&str] = &[
    "<system-reminder",
    "<local-command-caveat>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<command-name>",
    "<command-message>",
];

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SessionEnd",
];

/// Notification types that mean the agent is blocked on the user. The idle
/// reminder is not one: it fires after the turn already finished, and treating
/// it as a stop would count the same turn end twice.
const ASK_NOTIFICATIONS: &[&str] = &["permission_prompt"];

pub struct CodeBuddy;

impl Provider for CodeBuddy {
    fn name(&self) -> &'static str {
        "codebuddy"
    }

    fn logo(&self) -> &'static str {
        "⋈"
    }

    fn resume_cmd(&self, session_id: &str) -> Vec<String> {
        vec!["codebuddy".into(), "--resume".into(), session_id.into()]
    }

    fn session_env(&self) -> Option<&'static str> {
        Some("CODEBUDDY_SESSION_ID")
    }

    fn hooks(&self) -> Option<&dyn HookEvents> {
        Some(self)
    }

    fn log(&self) -> Option<&dyn TranscriptLog> {
        Some(self)
    }
}

impl HookEvents for CodeBuddy {
    fn apply_hook(&self, sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
        apply_hook(sessions, event);
    }

    fn enrich(&self, payload: &mut Value) {
        if !payload.is_object()
            || u32_field(payload, &["pid"])
                .filter(|&pid| pid != 0)
                .is_some()
        {
            return;
        }
        // The hook payload has no pid. Walk the hook process tree instead; the
        // native build keeps its own name, while the JS build runs under node
        // and Node labels that process "node-MainThread" in comm.
        if let Some(pid) =
            crate::process::ancestor_named_any(&["codebuddy", "cbc", "node-MainThread", "node"])
        {
            payload["pid"] = pid.into();
        }
    }
}

fn apply_hook(sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
    // Subagent hooks carry an agent id and a transcript path CodeBuddy never
    // writes, so tracking them leaves sessions that can never finish or
    // stream. Only the parent session is tracked.
    if string_field(event, &["agent_id", "agentId"]).is_some() {
        return;
    }
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
        "Stop" => Some(AgentEventKind::Stop),
        "Notification"
            if ASK_NOTIFICATIONS.contains(
                &string_field(event, &["notification_type", "notificationType"]).unwrap_or(""),
            ) =>
        {
            Some(AgentEventKind::PermissionRequest)
        }
        _ => None,
    };
    let Some(kind) = kind else {
        return;
    };
    apply_event(
        sessions,
        AgentEvent {
            session_id: sid,
            kind,
            source: Source::Cli,
            pid: u32_field(event, &["pid"]),
            parent_id: None,
            background_running: false,
        },
    );
}

impl Focus for CodeBuddy {}

impl Messaging for CodeBuddy {}

impl Transcript for CodeBuddy {
    fn transcript(
        &self,
        ctx: &Context,
        hub: &dyn Hub,
        session: &ListedSession,
    ) -> Result<Conversation> {
        log_transcript(self, ctx, hub, session)
    }

    fn preview(&self, ctx: &Context, hub: &dyn Hub, session: &ListedSession) -> Option<Preview> {
        log_preview(self, ctx, hub, session)
    }
}

impl TranscriptLog for CodeBuddy {
    fn root(&self, ctx: &Context) -> PathBuf {
        ctx.codebuddy_config_dir.join("projects")
    }

    fn is_session_file(&self, file_name: &str, session_id: &str) -> bool {
        file_name == format!("{session_id}.jsonl")
    }

    fn is_session_record(&self, row: &Value, session_id: &str) -> bool {
        row["sessionId"].as_str() == Some(session_id)
    }

    fn parse(&self, row: &Value) -> Option<Record> {
        match row["type"].as_str()? {
            "message" => {
                let message_text = &row["content"];
                match row["role"].as_str()? {
                    "user" => {
                        let text = first_text(message_text);
                        // Injected rows are marked skipRun; older logs only
                        // have the text prefix to go by.
                        let injected = row["providerData"]["skipRun"] == true
                            || INJECTED_PREFIXES
                                .iter()
                                .any(|prefix| text.starts_with(prefix));
                        let entries =
                            content(message_text, if injected { context } else { prompt });
                        let starts_turn = entries
                            .iter()
                            .any(|entry| matches!(entry, Entry::Prompt { .. }));
                        Some(Record {
                            entries,
                            starts_turn,
                            ends_turn: false,
                        })
                    }
                    "assistant" => {
                        // The client also writes harness markers such as
                        // "Interrupted by user" as assistant rows with skipRun.
                        let injected = row["providerData"]["skipRun"] == true;
                        Some(Record {
                            entries: content(message_text, if injected { context } else { said }),
                            ..Record::default()
                        })
                    }
                    _ => None,
                }
            }
            "function_call" => Some(Record {
                entries: vec![Entry::ToolCall {
                    name: row["name"].as_str().unwrap_or("call").to_string(),
                    input: row["arguments"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| row["arguments"].to_string()),
                }],
                ..Record::default()
            }),
            "function_call_result" => {
                let output = &row["output"];
                let entries = match output["text"].as_str() {
                    Some(text) => vec![Entry::ToolResult {
                        text: text.to_string(),
                    }],
                    None => tool_result(output),
                };
                Some(Record {
                    entries,
                    ..Record::default()
                })
            }
            // Written once the agent finishes answering a prompt.
            "turn-metrics" => Some(Record {
                ends_turn: true,
                ..Record::default()
            }),
            _ => None,
        }
    }

    fn record_id<'a>(&self, row: &'a Value) -> Option<&'a str> {
        row["id"].as_str()
    }
}

impl Integration for CodeBuddy {
    fn install(&self, ctx: &Context) -> Result<PathBuf> {
        let dest = self.hook_path(ctx);
        let mut data = load_object(&dest)?;
        plan(ctx, &mut data);
        save_object(&dest, &data)?;
        Ok(dest)
    }

    fn uninstall(&self, ctx: &Context) -> Result<()> {
        let dest = self.hook_path(ctx);
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
            groups.retain(|group| !group_is_ours(group, self.name()));
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

    fn hook_path(&self, ctx: &Context) -> PathBuf {
        ctx.codebuddy_config_dir.join("settings.json")
    }

    fn outdated(&self, ctx: &Context) -> bool {
        let Ok(current) = load_object(&self.hook_path(ctx)) else {
            return false;
        };
        let mut planned = current.clone();
        plan(ctx, &mut planned);
        planned != current
    }

    fn config_exists(&self, ctx: &Context) -> bool {
        ctx.codebuddy_config_dir.is_dir()
    }
}

fn plan(ctx: &Context, data: &mut Value) {
    if !data.is_object() {
        *data = json!({});
    }
    let hooks = data
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let handler = command_handler(ctx, CodeBuddy.name(), 5, false);
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
                    .filter(|group| !group_is_ours(group, CodeBuddy.name()))
                    .cloned(),
            );
        }
        let mut group = json!({"hooks": [handler.clone()]});
        if *event == "Notification" {
            group["matcher"] = Value::String(matcher.clone());
        }
        kept.push(group);
        *groups = Value::Array(kept);
    }
}

#[cfg(test)]
#[path = "codebuddy_tests.rs"]
mod tests;
