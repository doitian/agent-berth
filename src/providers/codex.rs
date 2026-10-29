use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use super::{
    Discovery, Focus, HookEvents, Integration, Messaging, Provider, Transcript, TranscriptLog,
    command_handler, focus_or_desktop, group_is_ours, load_object, log_preview, log_transcript,
    run_send_cmd, save_object,
};
use crate::hubs::{Hub, Preview};
use crate::paths::Context;
use crate::status::{
    AgentEvent, AgentEventKind, AgentSession, Source, apply_event, session_key, string_field,
    u32_field,
};
use crate::store::ListedSession;
use crate::transcript::{
    Conversation, Entry, Record, content, context, first_text, prompt, said, thinking, tool_result,
};

/// User messages that Codex adds itself: the environment, instruction files,
/// skill bodies, and the history it hands a reviewer.
const INJECTED_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "<skill>",
    "# AGENTS.md instructions",
    "The following is the Codex agent history",
];

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

pub struct Codex;

impl Provider for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn logo(&self) -> &'static str {
        "⌘"
    }

    fn resume_cmd(&self, session_id: &str) -> Vec<String> {
        vec!["codex".into(), "resume".into(), session_id.into()]
    }

    fn hooks(&self) -> Option<&dyn HookEvents> {
        Some(self)
    }

    fn discovery(&self) -> Option<&dyn Discovery> {
        Some(self)
    }

    fn log(&self) -> Option<&dyn TranscriptLog> {
        Some(self)
    }
}

impl HookEvents for Codex {
    fn apply_hook(&self, sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
        apply_hook(sessions, event);
    }

    fn enrich(&self, payload: &mut Value) {
        if !payload.is_object() {
            return;
        }
        enrich_originator(
            payload,
            std::env::var("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
                .ok()
                .as_deref(),
        );
        if u32_field(payload, &["pid"])
            .filter(|&pid| pid != 0)
            .is_none()
            && let Some(pid) = crate::process::ancestor_named("codex")
        {
            payload["pid"] = pid.into();
        }
    }
}

fn enrich_originator(payload: &mut Value, originator: Option<&str>) {
    // Capture this in the hook process: the long-lived berth server has a
    // different environment, and rollout metadata predates client switches.
    if string_field(payload, &["originator"]).is_none() {
        payload["originator"] = originator
            .filter(|value| !value.is_empty())
            .unwrap_or("codex-cli")
            .into();
    }
}

impl Discovery for Codex {
    fn discover(&self, ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
        discover(ctx, sessions)
    }
}

impl Focus for Codex {
    fn focus(&self, ctx: &Context, hub: &dyn Hub, session: &ListedSession) -> Result<()> {
        focus_or_desktop(ctx, hub, session, || {
            super::open_desktop_url(&desktop_url(&session.session_id)?)
        })
    }
}

impl Messaging for Codex {
    // `codex queue` hands the message to the shared local app-server that the
    // interactive session is attached to.
    fn send(&self, _hub: &dyn Hub, session: &ListedSession, message: &str) -> Result<()> {
        run_send_cmd(
            session,
            &[
                "codex".into(),
                "queue".into(),
                "--thread".into(),
                session.session_id.clone(),
                "--message".into(),
                message.into(),
            ],
        )
    }
}

impl Transcript for Codex {
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

impl TranscriptLog for Codex {
    fn root(&self, ctx: &Context) -> PathBuf {
        ctx.codex_home.join("sessions")
    }

    fn is_session_file(&self, file_name: &str, session_id: &str) -> bool {
        file_name.starts_with("rollout-") && file_name.ends_with(&format!("-{session_id}.jsonl"))
    }

    fn is_session_record(&self, row: &Value, session_id: &str) -> bool {
        row["type"] == "session_meta"
            && row["payload"]["id"]
                .as_str()
                .or_else(|| row["payload"]["session_id"].as_str())
                == Some(session_id)
    }

    fn parse(&self, row: &Value) -> Option<Record> {
        let item = &row["payload"];
        let marker = |starts_turn, ends_turn| Record {
            entries: Vec::new(),
            starts_turn,
            ends_turn,
        };
        match row["type"].as_str()? {
            "event_msg" => match item["type"].as_str()? {
                "task_started" => Some(marker(true, false)),
                "task_complete" | "turn_aborted" => Some(marker(false, true)),
                _ => None,
            },
            "response_item" => {
                let entries = match item["type"].as_str()? {
                    "message" => match item["role"].as_str()? {
                        "user" => {
                            let text = first_text(&item["content"]);
                            let injected = INJECTED_PREFIXES
                                .iter()
                                .any(|prefix| text.starts_with(prefix));
                            content(&item["content"], if injected { context } else { prompt })
                        }
                        "assistant" => content(&item["content"], said),
                        _ => return None,
                    },
                    "function_call" | "custom_tool_call" => vec![Entry::ToolCall {
                        name: item["name"].as_str().unwrap_or("call").to_string(),
                        input: item["arguments"]
                            .as_str()
                            .or_else(|| item["input"].as_str())
                            .unwrap_or("")
                            .to_string(),
                    }],
                    "function_call_output" | "custom_tool_call_output" => {
                        tool_result(&item["output"])
                    }
                    "reasoning" => thinking(item).into_iter().collect(),
                    _ => return None,
                };
                let starts_turn = entries
                    .iter()
                    .any(|entry| matches!(entry, Entry::Prompt { .. }));
                Some(Record {
                    entries,
                    starts_turn,
                    ends_turn: false,
                })
            }
            _ => None,
        }
    }
}

fn apply_hook(sessions: &mut BTreeMap<String, AgentSession>, event: &Value) {
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

fn discover(ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
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

fn drop_archived(ctx: &Context, sessions: &mut BTreeMap<String, AgentSession>) -> bool {
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

impl Integration for Codex {
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
        ctx.codex_home.join("hooks.json")
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
        ctx.codex_home.is_dir()
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
    // Codex ignores async hooks in `exec` mode, so install synchronous hooks.
    let handler = command_handler(ctx, Codex.name(), 3, false);
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
                    .filter(|group| !group_is_ours(group, Codex.name()))
                    .cloned(),
            );
        }
        let group = json!({"hooks": [handler.clone()]});
        kept.push(group);
        *groups = Value::Array(kept);
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
