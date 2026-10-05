use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Result, ensure};
use serde_json::Value;

use crate::paths::Context;
use crate::process;
use crate::status::string_field;
use crate::store::ListedSession;

mod api;
mod desktop;
mod timeline;

// The records only provide routing identity; monitoring and resume stay with
// the existing provider hooks and snapshots.
pub fn attribute(ctx: &Context, sessions: &mut [ListedSession]) {
    let mut ids = HashMap::new();
    let cache = RECORDS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Rebuilding the cache from this walk evicts records deleted since the last one.
    let mut visited = HashMap::new();
    collect_ids(
        &ctx.paseo_home.join("agents"),
        &cache,
        &mut visited,
        &mut ids,
    );
    *cache = visited;
    drop(cache);
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

#[derive(Clone)]
struct Route {
    agent_id: String,
    created_ms: Option<u64>,
}

type RouteKey = (String, String);

fn parse_timestamp(value: &str) -> Option<u64> {
    let time =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000_000).ok()
}

type RecordCache = HashMap<PathBuf, (SystemTime, u64, Option<(RouteKey, Route)>)>;
static RECORDS: OnceLock<Mutex<RecordCache>> = OnceLock::new();

fn collect_ids(
    root: &Path,
    cache: &RecordCache,
    visited: &mut RecordCache,
    ids: &mut HashMap<RouteKey, Route>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect_ids(&path, cache, visited, ids);
            continue;
        }
        if !kind.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let record = match cache.get(&path) {
            Some((mtime, len, record)) if *mtime == modified && *len == meta.len() => {
                record.clone()
            }
            _ => read_record(&path),
        };
        if let Some((key, route)) = &record
            && ids.get(key).is_none_or(|old| {
                (route.created_ms, &route.agent_id) > (old.created_ms, &old.agent_id)
            })
        {
            ids.insert(key.clone(), route.clone());
        }
        visited.insert(path, (modified, meta.len(), record));
    }
}

fn read_record(path: &Path) -> Option<(RouteKey, Route)> {
    let row: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if row["internal"] == true
        || string_field(&row, &["archivedAt"]).is_some()
        || row["lastStatus"] == "closed"
    {
        return None;
    }
    let agent_id = string_field(&row, &["id"]).filter(|id| valid_id(id))?;
    let provider = string_field(&row, &["provider"])?;
    let sid = string_field(&row["runtimeInfo"], &["sessionId"])
        .or_else(|| string_field(&row["persistence"], &["sessionId"]))?;
    Some((
        (provider.to_string(), sid.to_string()),
        Route {
            agent_id: agent_id.to_string(),
            created_ms: row["createdAt"].as_str().and_then(parse_timestamp),
        },
    ))
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
    pub fn new(agent_id: &str, home: &Path) -> Self {
        let state = Arc::new(Mutex::new(StreamState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let home = home.to_path_buf();
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
        let event_epoch = payload["epoch"].as_str();
        let current_epoch = || {
            state
                .lock()
                .ok()
                .and_then(|state| state.timeline.epoch.clone())
        };
        let mut epoch = current_epoch();
        if message["type"] == "agent.timeline.replacement"
            || event_epoch.is_some_and(|event| epoch.as_deref().is_some_and(|epoch| epoch != event))
        {
            let history = api.history(agent_id, cancel)?;
            if let Ok(mut state) = state.lock() {
                state.timeline.history(&history);
            }
            epoch = current_epoch();
        }
        // Events buffered while fetching history can predate the epoch it returned.
        if message["type"] == "agent_stream"
            && event_epoch.is_none_or(|event| epoch.as_deref().is_none_or(|epoch| epoch == event))
            && let Ok(mut state) = state.lock()
        {
            if state.timeline.epoch.is_none() {
                state.timeline.epoch = event_epoch.map(str::to_string);
            }
            state.timeline.event(payload);
        }
    }
    Ok(())
}

pub fn preview(ctx: &Context, agent_id: &str) -> Result<()> {
    ensure!(valid_id(agent_id), "invalid Paseo agent ID");
    let mut stream = Stream::new(agent_id, &ctx.paseo_home);
    // fzf on Windows kills only the preview shell, and a failed write never
    // reveals that while the agent is idle, so watch the shell instead.
    let parent = process::ancestors(std::process::id()).get(1).copied();
    let rows = std::env::var("FZF_PREVIEW_LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(40)
        .clamp(1, 200);
    let mut last = String::new();
    let mut stdout = std::io::stdout();
    for tick in 0u64.. {
        if tick % 10 == 0 && parent.is_some_and(|pid| !process::pid_alive(pid)) {
            break;
        }
        let content = stream.preview();
        if content != last {
            let lines: Vec<_> = content.lines().rev().take(rows).collect();
            let content_tail = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
            if write!(stdout, "\x1b[2J\x1b[H{content_tail}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
            last = content;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(test)]
#[path = "paseo_tests.rs"]
mod tests;
