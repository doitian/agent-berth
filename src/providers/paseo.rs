use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Result, ensure};
use serde_json::Value;

use crate::paths::{self, Context};
use crate::status::string_field;
use crate::store::ListedSession;

mod api;
mod desktop;
mod timeline;

// The records only provide routing identity; monitoring and resume stay with
// the existing provider hooks and snapshots.
pub fn attribute(ctx: &Context, sessions: &mut [ListedSession]) {
    let mut ids = HashMap::new();
    collect_ids(&ctx.paseo_home.join("agents"), &mut ids);
    for session in sessions {
        let Some(route) = ids.get(&(session.provider.clone(), session.session_id.clone())) else {
            continue;
        };
        let newer_import = route.created_ms.is_some_and(|created| {
            session
                .paseo_host_changed_ms
                .is_some_and(|reported| created > reported)
        });
        let unknown_host =
            session.paseo_host_changed_ms.is_none() && session.paseo_agent_id.is_none();
        if newer_import || unknown_host {
            session.paseo_agent_id = Some(route.agent_id.clone());
        }
    }
}

struct Route {
    agent_id: String,
    created_ms: Option<u64>,
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let time =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000_000).ok()
}

type RecordCache = HashMap<PathBuf, (SystemTime, u64, Option<Value>)>;
static RECORDS: OnceLock<Mutex<RecordCache>> = OnceLock::new();

fn collect_ids(root: &Path, ids: &mut HashMap<(String, String), Route>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect_ids(&path, ids);
            continue;
        }
        if !kind.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(row) = read_record(&path) else {
            continue;
        };
        if row["internal"] == true
            || string_field(&row, &["archivedAt"]).is_some()
            || row["lastStatus"] == "closed"
        {
            continue;
        }
        let (Some(agent_id), Some(provider), Some(sid)) = (
            string_field(&row, &["id"]),
            string_field(&row, &["provider"]),
            string_field(&row["runtimeInfo"], &["sessionId"])
                .or_else(|| string_field(&row["persistence"], &["sessionId"])),
        ) else {
            continue;
        };
        if valid_id(agent_id) {
            let route = Route {
                agent_id: agent_id.to_string(),
                created_ms: row["createdAt"].as_str().and_then(parse_timestamp),
            };
            let key = (provider.to_string(), sid.to_string());
            if ids.get(&key).is_none_or(|old| {
                (route.created_ms, &route.agent_id) > (old.created_ms, &old.agent_id)
            }) {
                ids.insert(key, route);
            }
        }
    }
}

fn read_record(path: &Path) -> Option<Value> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let cache = RECORDS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some((mtime, len, row)) = guard.get(path)
        && *mtime == modified
        && *len == meta.len()
    {
        return row.clone();
    }
    let row = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if let Ok(mut guard) = cache.lock() {
        guard.insert(path.to_path_buf(), (modified, meta.len(), row.clone()));
    }
    row
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn focus(ctx: &Context, agent_id: &str) -> Result<()> {
    desktop::open(ctx, &focus_url(ctx, agent_id)?)
}

fn focus_url(ctx: &Context, agent_id: &str) -> Result<String> {
    ensure!(valid_id(agent_id), "invalid Paseo agent ID");
    let cancel = AtomicBool::new(false);
    let mut api = api::Api::connect(&ctx.paseo_home, &cancel)?;
    api.agent(agent_id, &cancel)?;
    Ok(format!(
        "paseo://h/{}/agent/{}",
        encode_segment(&api.server_id),
        encode_segment(agent_id)
    ))
}

fn encode_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[derive(Default)]
struct StreamState {
    timeline: timeline::Timeline,
    connected: bool,
    error: Option<String>,
}

pub(crate) struct Stream {
    state: Arc<Mutex<StreamState>>,
    cancel: Arc<AtomicBool>,
}

impl Stream {
    pub fn new(agent_id: &str, home: Option<&Path>) -> Self {
        let state = Arc::new(Mutex::new(StreamState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let home = home
            .map(Path::to_path_buf)
            .or_else(|| paths::env_path("PASEO_HOME"))
            .unwrap_or_else(|| paths::home_dir().unwrap_or_default().join(".paseo"));
        let agent_id = agent_id.to_string();
        let worker_state = state.clone();
        let worker_cancel = cancel.clone();
        thread::spawn(move || {
            while !worker_cancel.load(Ordering::Relaxed) {
                let result = follow(&home, &agent_id, &worker_state, &worker_cancel);
                if let Err(error) = result
                    && let Ok(mut state) = worker_state.lock()
                {
                    state.connected = false;
                    state.error = Some(format!(
                        "Unable to stream Paseo session: {error:#}. Retrying…"
                    ));
                }
                for _ in 0..20 {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Self { state, cancel }
    }

    pub fn preview(&mut self) -> String {
        let Ok(state) = self.state.lock() else {
            return "Paseo stream unavailable".into();
        };
        if let Some(error) = &state.error {
            return format!("{}\n{error}", state.timeline.render());
        }
        if !state.connected {
            return "Connecting to Paseo session…".into();
        }
        state.timeline.render()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn follow(
    home: &Path,
    agent_id: &str,
    state: &Mutex<StreamState>,
    cancel: &AtomicBool,
) -> Result<()> {
    ensure!(valid_id(agent_id), "invalid Paseo agent ID");
    let mut api = api::Api::connect(home, cancel)?;
    api.agent(agent_id, cancel)?;
    let subscription = api.subscribe(agent_id, cancel)?;
    let history = api.history(agent_id, cancel)?;
    if let Ok(mut state) = state.lock() {
        state.timeline.history(&history);
        state.connected = true;
        state.error = None;
    }
    while !cancel.load(Ordering::Relaxed) {
        let message = api.next(cancel)?;
        let payload = &message["payload"];
        if payload["agentId"].as_str() != Some(agent_id) {
            continue;
        }
        if let Some(subscription) = &subscription
            && payload["subscriptionId"]
                .as_str()
                .is_some_and(|id| id != subscription)
        {
            continue;
        }
        let replacement = message["type"] == "agent.timeline.replacement";
        let epoch_changed = payload["epoch"].as_str().is_some_and(|epoch| {
            state
                .lock()
                .ok()
                .is_some_and(|state| state.timeline.epoch.as_deref() != Some(epoch))
        });
        if replacement || epoch_changed {
            let history = api.history(agent_id, cancel)?;
            if let Ok(mut state) = state.lock() {
                state.timeline.history(&history);
            }
        }
        if message["type"] == "agent_stream"
            && let Ok(mut state) = state.lock()
        {
            state.timeline.event(payload);
        }
    }
    Ok(())
}

pub fn preview(ctx: &Context, agent_id: &str) -> Result<()> {
    ensure!(valid_id(agent_id), "invalid Paseo agent ID");
    let mut stream = Stream::new(agent_id, Some(&ctx.paseo_home));
    let rows = std::env::var("FZF_PREVIEW_LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40)
        .clamp(1, 200);
    let mut last = String::new();
    loop {
        let content = stream.preview();
        if content != last {
            let lines: Vec<_> = content.lines().rev().take(rows).collect();
            let content_tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
            print!("\x1b[2J\x1b[H{content_tail}");
            std::io::stdout().flush()?;
            last = content;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
#[path = "paseo_tests.rs"]
mod tests;
