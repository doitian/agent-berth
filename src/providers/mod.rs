use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::hubs::{Hub, Preview};
use crate::paths::{self, Context as AppContext};
use crate::status::{AgentSession, Source};
use crate::store::ListedSession;
use crate::transcript::{self, Conversation, Feed, Record};

mod claude;
mod codex;
mod grok;
mod opencode;
mod pi;

/// Every supported agent. Adding a provider means implementing [`Provider`]
/// and its supertraits, overriding the hub defaults it supports natively, and
/// listing it here.
static ALL: [&dyn Provider; 5] = [
    &claude::Claude,
    &codex::Codex,
    &grok::Grok,
    &opencode::Opencode,
    &pi::Pi,
];

pub fn all() -> &'static [&'static dyn Provider] {
    &ALL
}

pub fn lookup(name: &str) -> Option<&'static dyn Provider> {
    ALL.iter().copied().find(|provider| provider.name() == name)
}

/// A supported coding agent. Features with a hub fallback are supertraits
/// whose default methods delegate to the session's hub; a provider overrides
/// them when it has native support. Provider-only features are optional and
/// return `None` when the provider lacks them.
pub trait Provider: Integration + Focus + Messaging + Transcript + Sync {
    fn name(&self) -> &'static str;

    fn logo(&self) -> &'static str;

    fn resume_cmd(&self, session_id: &str) -> Vec<String>;

    fn normalize_title(&self, title: &str) -> String {
        title.to_string()
    }

    fn hooks(&self) -> Option<&dyn HookEvents> {
        None
    }

    fn discovery(&self) -> Option<&dyn Discovery> {
        None
    }

    fn log(&self) -> Option<&dyn TranscriptLog> {
        None
    }
}

/// The hooks or plugin that report the agent's status to agent-berth.
pub trait Integration {
    /// Writes the integration and returns the file whose contents identify it.
    fn install(&self, ctx: &AppContext) -> Result<PathBuf>;

    fn uninstall(&self, ctx: &AppContext) -> Result<()>;

    fn hook_path(&self, ctx: &AppContext) -> PathBuf;

    /// True when re-running `install` would rewrite the config, so the on-disk
    /// integration predates the one this build ships.
    fn outdated(&self, ctx: &AppContext) -> bool;

    /// The agent keeps a config directory, so it counts as installed even
    /// when its binary is not on PATH.
    fn config_exists(&self, ctx: &AppContext) -> bool;

    /// Explains why the hooks are missing when it is not simply that `setup`
    /// never ran.
    fn install_problem(&self, _ctx: &AppContext) -> Option<String> {
        None
    }
}

pub trait Focus {
    fn focus(&self, ctx: &AppContext, hub: &dyn Hub, session: &ListedSession) -> Result<()> {
        hub.focus(ctx, session)
    }
}

pub trait Messaging {
    fn send(&self, hub: &dyn Hub, session: &ListedSession, message: &str) -> Result<()> {
        hub.send(session, message)
    }
}

/// The session's conversation: whole for `logs`, live for the TUI.
pub trait Transcript {
    fn transcript(
        &self,
        _ctx: &AppContext,
        hub: &dyn Hub,
        session: &ListedSession,
    ) -> Result<Conversation> {
        hub.transcript(session)
    }

    fn preview(&self, ctx: &AppContext, hub: &dyn Hub, session: &ListedSession) -> Option<Preview> {
        hub.preview(ctx, session)
    }
}

/// Status tracking from hook payloads that the agent pipes to `notify`.
pub trait HookEvents {
    fn apply_hook(&self, sessions: &mut BTreeMap<String, AgentSession>, payload: &Value);

    /// Adds details only the hook process can see, such as its environment,
    /// before the payload leaves for the long-lived server.
    fn enrich(&self, _payload: &mut Value) {}
}

/// Status and metadata the agent keeps outside its hooks.
pub trait Discovery {
    /// Returns whether any session changed.
    fn discover(&self, ctx: &AppContext, sessions: &mut BTreeMap<String, AgentSession>) -> bool;
}

/// JSONL conversation logs the agent writes to disk.
pub trait TranscriptLog {
    /// Directory searched for sessions whose hooks never reported a path.
    fn root(&self, ctx: &AppContext) -> PathBuf;

    fn is_session_file(&self, file_name: &str, session_id: &str) -> bool;

    /// The record proves the file belongs to the session, not to a parent or
    /// sibling with a similar name.
    fn is_session_record(&self, row: &Value, session_id: &str) -> bool;

    /// The conversation entries and turn markers in one log row, or `None` for
    /// rows that are neither.
    fn parse(&self, row: &Value) -> Option<Record>;

    /// Identifies revisions of the same record. Without one, the whole record
    /// is its identity.
    fn record_id<'a>(&self, _row: &'a Value) -> Option<&'a str> {
        None
    }
}

impl dyn Provider {
    pub fn is_installed(&self, ctx: &AppContext) -> bool {
        paths::on_path(self.name()) || self.config_exists(ctx)
    }

    pub fn hooks_installed(&self, ctx: &AppContext) -> bool {
        let Ok(text) = fs::read_to_string(self.hook_path(ctx)) else {
            return false;
        };
        // Shell hooks spell out `notify --provider <name>`, while JS/TS plugins
        // pass it as an argv array; compare with separators stripped.
        let compact: String = text
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect();
        compact.contains(&format!("notifyprovider{}", self.name()))
    }

    pub fn hooks_outdated(&self, ctx: &AppContext) -> bool {
        self.hooks_installed(ctx) && self.outdated(ctx)
    }

    pub fn hooks_stale(&self, ctx: &AppContext) -> bool {
        if !self.hooks_installed(ctx) {
            return false;
        }
        let Ok(text) = fs::read_to_string(self.hook_path(ctx)) else {
            return false;
        };
        let bin = ctx.berth_bin.display().to_string();
        let variants = [
            bin.clone(),
            bin.replace('\\', "\\\\"),
            bin.replace('\\', "/"),
        ];
        !variants.iter().any(|variant| text.contains(variant))
    }
}

// Sessions of a provider this build no longer knows get the hub's behaviour.

pub fn focus(ctx: &AppContext, hub: &dyn Hub, session: &ListedSession) -> Result<()> {
    match lookup(&session.provider) {
        Some(provider) => provider.focus(ctx, hub, session),
        None => hub.focus(ctx, session),
    }
}

pub fn send(hub: &dyn Hub, session: &ListedSession, message: &str) -> Result<()> {
    match lookup(&session.provider) {
        Some(provider) => provider.send(hub, session, message),
        None => hub.send(session, message),
    }
}

pub fn transcript(
    ctx: &AppContext,
    hub: &dyn Hub,
    session: &ListedSession,
) -> Result<Conversation> {
    match lookup(&session.provider) {
        Some(provider) => provider.transcript(ctx, hub, session),
        None => hub.transcript(session),
    }
}

pub fn preview(ctx: &AppContext, hub: &dyn Hub, session: &ListedSession) -> Option<Preview> {
    match lookup(&session.provider) {
        Some(provider) => provider.preview(ctx, hub, session),
        None => hub.preview(ctx, session),
    }
}

/// Focuses a desktop session in its app only when the hub does not host it,
/// so a Paseo route still wins over the app that first opened the session.
fn focus_or_desktop(
    ctx: &AppContext,
    hub: &dyn Hub,
    session: &ListedSession,
    desktop: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if session.source == Source::Desktop && !hub.hosts(session) {
        desktop()
    } else {
        hub.focus(ctx, session)
    }
}

/// Runs a provider CLI command that delivers a message to the session.
fn run_send_cmd(session: &ListedSession, cmd: &[String]) -> Result<()> {
    let (program, args) = cmd.split_first().context("empty send command")?;
    let mut command = Command::new(program);
    command.args(args);
    // A deleted working directory would mask the real error from the CLI.
    if let Some(cwd) = session.cwd.as_deref().filter(|cwd| Path::new(cwd).is_dir()) {
        command.current_dir(cwd);
    }
    let status = command.status().with_context(|| format!("run {program}"))?;
    if !status.success() {
        anyhow::bail!("{} failed ({status})", cmd.join(" "));
    }
    Ok(())
}

fn log_source(
    log: &dyn TranscriptLog,
    ctx: &AppContext,
    session: &ListedSession,
) -> transcript::Source {
    transcript::Source {
        provider: session.provider.clone(),
        session_id: session.session_id.clone(),
        path: session.transcript_path.clone(),
        root: Some(log.root(ctx)),
    }
}

/// The local log, falling back to the hub when the log is unreadable.
fn log_transcript(
    log: &dyn TranscriptLog,
    ctx: &AppContext,
    hub: &dyn Hub,
    session: &ListedSession,
) -> Result<Conversation> {
    or_hub_transcript(
        transcript::full(&log_source(log, ctx, session)),
        hub,
        session,
    )
}

/// The hub's view when it shows the session's own output, otherwise the
/// local log, which also covers a pane hosting the session in the background.
fn log_preview(
    log: &dyn TranscriptLog,
    ctx: &AppContext,
    hub: &dyn Hub,
    session: &ListedSession,
) -> Option<Preview> {
    hub.preview(ctx, session)
        .or_else(|| Some(Preview::Feed(Feed::Log(log_source(log, ctx, session)))))
}

/// Falls back to the hub's transcript, reporting both failures when it also
/// has none.
fn or_hub_transcript(
    result: Result<Conversation>,
    hub: &dyn Hub,
    session: &ListedSession,
) -> Result<Conversation> {
    result.or_else(|error| {
        hub.transcript(session)
            .map_err(|hub_error| anyhow::anyhow!("{error:#}; {hub_error:#}"))
    })
}

fn open_desktop_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = Command::new("xdg-open");
    let mut child = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("open desktop session")?;
    // Reap the launcher without waiting for the app to handle the deep link.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn apply_hook(provider: &str, sessions: &mut BTreeMap<String, AgentSession>, payload: &Value) {
    if let Some(hooks) = lookup(provider).and_then(|provider| provider.hooks()) {
        hooks.apply_hook(sessions, payload);
    }
}

pub fn normalize_title(provider: &str, title: &str) -> String {
    lookup(provider).map_or_else(
        || title.to_string(),
        |provider| provider.normalize_title(title),
    )
}

/// Resume command for a stored session. Sessions of a provider this build no
/// longer knows fall back to the most common `<name> --resume <id>` form.
pub fn resume_cmd(provider: &str, session_id: &str) -> Vec<String> {
    match lookup(provider) {
        Some(provider) => provider.resume_cmd(session_id),
        None => vec![provider.into(), "--resume".into(), session_id.into()],
    }
}

pub fn logo(provider: &str) -> &'static str {
    lookup(provider).map_or("●", |provider| provider.logo())
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
