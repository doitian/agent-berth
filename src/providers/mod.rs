use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::paths::{self, Context as AppContext};
use crate::status::AgentSession;

mod claude;
mod codex;
mod grok;
mod opencode;
mod pi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    Codex,
    Grok,
    Opencode,
    Pi,
}

impl ProviderKind {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Grok,
        Self::Opencode,
        Self::Pi,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn is_installed(self, ctx: &AppContext) -> bool {
        paths::on_path(self.name()) || config_exists(self, ctx)
    }

    pub fn install(self, ctx: &AppContext) -> Result<std::path::PathBuf> {
        match self {
            Self::Claude => claude::install(ctx),
            Self::Codex => codex::install(ctx),
            Self::Grok => grok::install(ctx),
            Self::Opencode => opencode::install(ctx),
            Self::Pi => pi::install(ctx),
        }
    }

    pub fn uninstall(self, ctx: &AppContext) -> Result<()> {
        match self {
            Self::Claude => claude::uninstall(ctx),
            Self::Codex => codex::uninstall(ctx),
            Self::Grok => grok::uninstall(ctx),
            Self::Opencode => opencode::uninstall(ctx),
            Self::Pi => pi::uninstall(ctx),
        }
    }

    pub fn hook_path(self, ctx: &AppContext) -> PathBuf {
        match self {
            Self::Claude => ctx.claude_config_dir.join("settings.json"),
            Self::Codex => ctx.codex_home.join("hooks.json"),
            Self::Grok => ctx.grok_home.join("hooks").join("agent-bridge.json"),
            Self::Opencode => ctx.opencode_plugin_dir().join("agent-bridge.js"),
            Self::Pi => ctx.pi_dir.join("extensions").join("agent-bridge.ts"),
        }
    }

    pub fn hooks_installed(self, ctx: &AppContext) -> bool {
        let Ok(text) = fs::read_to_string(self.hook_path(ctx)) else {
            return false;
        };
        text.contains(&format!("notify --provider {}", self.name()))
    }

    pub fn hooks_stale(self, ctx: &AppContext) -> bool {
        if !self.hooks_installed(ctx) {
            return false;
        }
        let Ok(text) = fs::read_to_string(self.hook_path(ctx)) else {
            return false;
        };
        let bin = ctx.bridge_bin.display().to_string();
        let escaped = bin.replace('\\', "\\\\");
        !(text.contains(&bin) || text.contains(&escaped))
    }
}

fn config_exists(kind: ProviderKind, ctx: &AppContext) -> bool {
    match kind {
        ProviderKind::Claude => ctx.claude_config_dir.is_dir(),
        ProviderKind::Codex => ctx.codex_home.is_dir(),
        ProviderKind::Grok => ctx.grok_home.is_dir(),
        ProviderKind::Opencode => ctx
            .opencode_plugin_dir()
            .parent()
            .is_some_and(|p| p.is_dir()),
        ProviderKind::Pi => ctx.pi_dir.is_dir(),
    }
}

pub fn discover_claude(
    ctx: &AppContext,
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
) -> bool {
    claude::discover(ctx, sessions)
}

pub fn drop_archived_codex(
    ctx: &AppContext,
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
) -> bool {
    codex::drop_archived(ctx, sessions)
}

pub fn apply_hook(
    provider: &str,
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
    payload: &Value,
) {
    match provider {
        "claude" => claude::apply_hook(sessions, payload),
        "codex" => codex::apply_hook(sessions, payload),
        "grok" => grok::apply_hook(sessions, payload),
        _ => {}
    }
}

pub fn resume_cmd(provider: &str, session_id: &str) -> Vec<String> {
    match provider {
        "claude" => vec!["claude".into(), "--resume".into(), session_id.into()],
        "codex" => vec!["codex".into(), "resume".into(), session_id.into()],
        "grok" => vec!["grok".into(), "--resume".into(), session_id.into()],
        "opencode" => vec!["opencode".into(), "--session".into(), session_id.into()],
        "pi" => vec!["pi".into(), "--session".into(), session_id.into()],
        other => vec![other.into(), "--resume".into(), session_id.into()],
    }
}

pub fn load_object(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("invalid JSON {}", path.display()))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

pub fn save_object(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    if value.as_object().is_some_and(|obj| obj.is_empty()) {
        let _ = fs::remove_file(path);
        return Ok(());
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

pub fn command_handler(ctx: &AppContext, provider: &str, timeout: u32, async_hook: bool) -> Value {
    let mut handler = json!({
        "type": "command",
        "command": ctx.notify_command(provider),
        "timeout": timeout,
    });
    if async_hook {
        handler["async"] = Value::Bool(true);
    }
    handler
}

pub fn handler_is_ours(handler: &Value, provider: &str) -> bool {
    if handler.get("type").and_then(Value::as_str) != Some("command") {
        return false;
    }
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(&format!("notify --provider {provider}")))
}

pub fn group_is_ours(group: &Value, provider: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks
                .iter()
                .any(|handler| handler_is_ours(handler, provider))
        })
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
