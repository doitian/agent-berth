use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
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

// Claude can exit as soon as a turn-terminal hook fires: `claude -p` shuts down
// within milliseconds of Stop, and a backgrounded hook process dies with it.
// Run these synchronously so the report lands before the client goes away.
const SYNC_HOOK_EVENTS: &[&str] = &["Stop", "StopFailure", "SessionEnd"];

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
                    .is_some_and(|_| agent_kind(task) == AgentStatus::Running)
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
    if let Some((rows, observed_ms)) = list_agents() {
        apply_agents(ctx, sessions, &rows, &index.live, observed_ms);
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

#[derive(Default)]
struct AgentsCache {
    rows: Option<Vec<Value>>,
    fetched_ms: u64,
    refreshing: bool,
}

impl AgentsCache {
    fn snapshot(&self) -> Option<(Vec<Value>, u64)> {
        self.rows.clone().map(|rows| (rows, self.fetched_ms))
    }

    fn store(&mut self, fetched: Option<(Vec<Value>, u64)>) {
        if let Some((rows, fetched_ms)) = fetched {
            self.rows = Some(rows);
            self.fetched_ms = fetched_ms;
        }
    }
}

static AGENTS_CACHE: OnceLock<Mutex<AgentsCache>> = OnceLock::new();

// Stale-while-revalidate: serve the last `claude agents --json` result and
// refresh it in a background thread, so callers never pay the subprocess
// startup cost. The first call fetches synchronously. Rows come with the time
// they were observed, since a cached row describes a session as it was.
pub fn list_agents() -> Option<(Vec<Value>, u64)> {
    let cache = AGENTS_CACHE.get_or_init(|| Mutex::new(AgentsCache::default()));
    let Ok(mut guard) = cache.lock() else {
        return fetch_agents();
    };
    if guard.refreshing {
        return guard.snapshot();
    }
    if guard.rows.is_none() {
        let fetched = fetch_agents();
        guard.store(fetched.clone());
        return fetched;
    }
    guard.refreshing = true;
    thread::spawn(|| {
        let fetched = fetch_agents();
        if let Some(cache) = AGENTS_CACHE.get()
            && let Ok(mut guard) = cache.lock()
        {
            guard.store(fetched);
            guard.refreshing = false;
        }
    });
    guard.snapshot()
}

fn fetch_agents() -> Option<(Vec<Value>, u64)> {
    // Stamp the fetch before it starts: anything the server learns while the
    // subprocess runs is newer than the snapshot it returns.
    let observed_ms = now_ms();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(claude_agents_output());
    });
    let output = rx.recv_timeout(Duration::from_secs(2)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let data: Value = serde_json::from_slice(&output.stdout).ok()?;
    Some((data.as_array().cloned()?, observed_ms))
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
    local_ids: BTreeMap<String, String>,
}

fn desktop_index(ctx: &Context) -> DesktopIndex {
    let mut index = DesktopIndex {
        live: HashSet::new(),
        archived: HashSet::new(),
        local_ids: BTreeMap::new(),
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

#[derive(Clone)]
struct DesktopEntry {
    ids: Vec<(String, bool)>,
    local_id: Option<String>,
}

// Session records only change when Claude Desktop rewrites them, so key the
// parse results by (mtime, len) and skip re-reading unchanged files.
type DesktopCache = HashMap<PathBuf, (SystemTime, u64, DesktopEntry)>;

static DESKTOP_CACHE: OnceLock<Mutex<DesktopCache>> = OnceLock::new();

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
        let Some(parsed) = parse_desktop_file(&path) else {
            continue;
        };
        for (id, archived) in &parsed.ids {
            if *archived {
                index.archived.insert(id.clone());
            } else {
                index.live.insert(id.clone());
                if let Some(local_id) = &parsed.local_id {
                    index.local_ids.insert(id.clone(), local_id.clone());
                }
            }
        }
    }
}

fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let cache = DESKTOP_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some((cached_mtime, cached_len, entry)) = guard.get(path)
        && *cached_mtime == mtime
        && *cached_len == meta.len()
    {
        return Some(entry.clone());
    }
    let text = std::fs::read_to_string(path).ok()?;
    let data = serde_json::from_str::<Value>(&text).ok()?;
    let archived = data
        .get("isArchived")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let local_id = string_field(&data, &["sessionId"])
        .filter(|id| valid_desktop_id(id))
        .map(str::to_string);
    let mut ids = Vec::new();
    for key in ["cliSessionId", "sessionId"] {
        if let Some(id) = data
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            ids.push((id.to_string(), archived));
        }
    }
    let entry = DesktopEntry { ids, local_id };
    if let Ok(mut guard) = cache.lock() {
        guard.insert(path.to_path_buf(), (mtime, meta.len(), entry.clone()));
    }
    Some(entry)
}

fn valid_desktop_id(id: &str) -> bool {
    id.strip_prefix("local_").is_some_and(|suffix| {
        (1..=64).contains(&suffix.len())
            && suffix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    })
}

fn desktop_url(ctx: &Context, session_id: &str) -> Result<String> {
    let index = desktop_index(ctx);
    let local_id = index
        .local_ids
        .get(session_id)
        .filter(|_| !index.archived.contains(session_id))
        .context("no matching active Claude Desktop session found")?;
    Ok(format!("claude://code/continue?session={local_id}"))
}

pub fn focus_desktop(ctx: &Context, session_id: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("NIRI_SOCKET").is_some() {
        let _ = raise_niri_window();
    }
    let url = desktop_url(ctx, session_id)?;
    super::open_desktop_url(&url)
}

#[cfg(target_os = "linux")]
fn raise_niri_window() -> Result<()> {
    let output = Command::new("niri")
        .args(["msg", "--json", "windows"])
        .stdin(Stdio::null())
        .output()
        .context("list niri windows")?;
    if !output.status.success() {
        anyhow::bail!(
            "list niri windows: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let windows: Vec<Value> =
        serde_json::from_slice(&output.stdout).context("parse niri windows")?;
    let id = niri_claude_window(&windows).context("no Claude Desktop window found")?;
    let output = Command::new("niri")
        .args(["msg", "action", "focus-window", "--id", &id.to_string()])
        .stdin(Stdio::null())
        .output()
        .context("focus niri window")?;
    if !output.status.success() {
        anyhow::bail!(
            "focus niri window: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn niri_claude_window(windows: &[Value]) -> Option<u64> {
    windows
        .iter()
        .filter(|window| {
            window["app_id"] == "com.anthropic.Claude" && window["id"].as_u64().is_some()
        })
        // Prefer the most recently used Claude window when more than one is open.
        .max_by_key(|window| {
            (
                window["is_focused"].as_bool().unwrap_or(false),
                window["focus_timestamp"]["secs"].as_u64().unwrap_or(0),
                window["focus_timestamp"]["nanos"].as_u64().unwrap_or(0),
            )
        })
        .and_then(|window| window["id"].as_u64())
}

pub fn apply_agents(
    ctx: &Context,
    sessions: &mut BTreeMap<String, AgentSession>,
    rows: &[Value],
    desktop_ids: &HashSet<String>,
    observed_ms: u64,
) {
    let now = now_ms();
    let mut live = HashSet::new();
    for row in rows {
        let Some(sid) = string_field(row, &["sessionId", "id"]).map(str::to_string) else {
            continue;
        };
        if is_desktop_helper(ctx, row, &sid, desktop_ids) {
            continue;
        }
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
        let idle_row = matches!(string_field(row, &["status", "state"]), Some("idle"));
        if let Some(existing) = sessions.get_mut(&sid) {
            // `claude agents --json` still reports a session as busy while its
            // Stop hook runs, so discovery must not raise a hook-tracked session
            // back to running; it only fills in a status hooks never reported. An
            // idle row still ends a run, covering a Stop hook that never arrived.
            if status == AgentStatus::Idle || existing.hooked {
                if pid.is_some() {
                    existing.pid = pid;
                }
                // A cached row can predate the hook that started this run, so
                // only one observed since the last report can end it.
                if idle_row
                    && existing.status == AgentStatus::Running
                    && observed_ms > existing.last_report_ms
                {
                    existing.status = AgentStatus::Done;
                    existing.background_only = false;
                } else if status != AgentStatus::Idle && existing.status == AgentStatus::Idle {
                    existing.status = status;
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

fn is_desktop_helper(ctx: &Context, row: &Value, sid: &str, desktop_ids: &HashSet<String>) -> bool {
    if desktop_ids.contains(sid) || string_field(row, &["kind"]) == Some("background") {
        return false;
    }
    let Some(pid) = u32_field(row, &["pid"]) else {
        return false;
    };
    // `claude agents --json` omits the entrypoint and host session identity.
    // Desktop also registers temporary config queries, which have no host
    // session. Consult the matching registration to avoid listing those helpers.
    let path = ctx
        .claude_config_dir
        .join("sessions")
        .join(format!("{pid}.json"));
    let Some(registration) = std::fs::read(&path)
        .ok()
        .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
    else {
        return false;
    };
    u32_field(&registration, &["pid"]) == Some(pid)
        && string_field(&registration, &["sessionId"]) == Some(sid)
        && source_for(&registration) == Source::Desktop
        && string_field(&registration, &["hostSessionId"]).is_none()
        && string_field(&registration, &["kind"]) != Some("bg")
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
        return AgentStatus::Running;
    }
    AgentStatus::Idle
}

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.claude_config_dir.join("settings.json");
    let mut data = load_object(&dest)?;
    plan(ctx, &mut data);
    save_object(&dest, &data)?;
    Ok(dest)
}

pub fn outdated(ctx: &Context) -> bool {
    let Ok(current) = load_object(&ctx.claude_config_dir.join("settings.json")) else {
        return false;
    };
    let mut planned = current.clone();
    plan(ctx, &mut planned);
    planned != current
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
        if SYNC_HOOK_EVENTS.contains(event) {
            group["hooks"][0]["async"] = Value::Bool(false);
        }
        kept.push(group);
        *groups = Value::Array(kept);
    }
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
