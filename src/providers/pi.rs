use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

use super::{
    Focus, Integration, Messaging, Provider, Transcript, TranscriptLog, log_preview,
    log_transcript, write_text,
};
use crate::hubs::{Hub, Preview};
use crate::paths::Context;
use crate::store::ListedSession;
use crate::transcript::{Conversation, Record, content, prompt, said, tool_result};

pub struct Pi;

impl Provider for Pi {
    fn name(&self) -> &'static str {
        "pi"
    }

    fn logo(&self) -> &'static str {
        "π"
    }

    fn resume_cmd(&self, session_id: &str) -> Vec<String> {
        vec!["pi".into(), "--session".into(), session_id.into()]
    }

    fn normalize_title(&self, title: &str) -> String {
        normalize_title(title)
    }

    fn session_env(&self) -> Option<&'static str> {
        Some("PI_SESSION_ID")
    }

    fn log(&self) -> Option<&dyn TranscriptLog> {
        Some(self)
    }
}

impl Integration for Pi {
    fn install(&self, ctx: &Context) -> Result<PathBuf> {
        let dest = self.hook_path(ctx);
        write_text(&dest, &extension_source(ctx))?;
        Ok(dest)
    }

    fn uninstall(&self, ctx: &Context) -> Result<()> {
        let dest = self.hook_path(ctx);
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        Ok(())
    }

    fn hook_path(&self, ctx: &Context) -> PathBuf {
        ctx.pi_dir.join("extensions").join("agent-berth.ts")
    }

    fn outdated(&self, ctx: &Context) -> bool {
        std::fs::read_to_string(self.hook_path(ctx)).is_ok_and(|text| text != extension_source(ctx))
    }

    fn config_exists(&self, ctx: &Context) -> bool {
        ctx.pi_dir.is_dir()
    }
}

impl Focus for Pi {}

impl Messaging for Pi {}

impl Transcript for Pi {
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

impl TranscriptLog for Pi {
    fn root(&self, ctx: &Context) -> PathBuf {
        ctx.pi_dir.join("sessions")
    }

    fn is_session_file(&self, file_name: &str, session_id: &str) -> bool {
        file_name.ends_with(&format!("_{session_id}.jsonl"))
    }

    fn is_session_record(&self, row: &Value, session_id: &str) -> bool {
        row["type"] == "session" && row["id"].as_str() == Some(session_id)
    }

    fn parse(&self, row: &Value) -> Option<Record> {
        if row["type"] != "message" {
            return None;
        }
        let message = &row["message"];
        let record = match message["role"].as_str()? {
            "user" => Record {
                entries: content(&message["content"], prompt),
                starts_turn: true,
                ends_turn: false,
            },
            "assistant" => Record {
                entries: content(&message["content"], said),
                starts_turn: false,
                ends_turn: message["stopReason"] == "stop",
            },
            "toolResult" => Record {
                entries: tool_result(&message["content"]),
                ..Record::default()
            },
            _ => return None,
        };
        Some(record)
    }
}

fn extension_source(ctx: &Context) -> String {
    include_str!("hooks/pi.ts").replace(
        "__AGENT_BERTH_BIN__",
        &serde_json::to_string(&ctx.berth_bin.display().to_string()).unwrap(),
    )
}

/// Pi expands `/skill:name` into a `<skill name=… location=…>…</skill>` user
/// message, which the plugin reports as the session title. Show the slash
/// command instead of the raw block.
fn normalize_title(title: &str) -> String {
    let Some(rest) = title.strip_prefix("<skill name=\"") else {
        return title.to_string();
    };
    let Some(end) = rest.find('"') else {
        return title.to_string();
    };
    let name = &rest[..end];
    if name.is_empty() || !rest[end..].starts_with("\" location=\"") {
        return title.to_string();
    }
    let Some(close) = rest.find("</skill>") else {
        return title.to_string();
    };
    let tail = rest[close + "</skill>".len()..].trim();
    if tail.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {tail}")
    }
}

#[cfg(test)]
#[path = "pi_tests.rs"]
mod tests;
