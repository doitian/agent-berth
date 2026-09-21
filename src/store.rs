use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::process::pid_alive;
use crate::providers;
use crate::status::{
    AgentSession, AgentStatus, Source, cwd_field, string_field, title_field, u32_field,
};

pub const PLUGIN_STALE: Duration = Duration::from_secs(5);
pub const SESSION_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone)]
pub enum Change {
    Hooks { provider: String },
    Snapshot { provider: String, instance: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Hook,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSnapshot {
    #[serde(default)]
    pub status: BTreeMap<String, String>,
    #[serde(default)]
    pub blocking: Vec<String>,
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub cmdline: Vec<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub last_report_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub last_heartbeat_ms: u64,
    #[serde(default)]
    pub previous_heartbeat_ms: Option<u64>,
    #[serde(default)]
    pub hooks: BTreeMap<String, BTreeMap<String, AgentSession>>,
    #[serde(default)]
    pub snapshots: BTreeMap<String, BTreeMap<String, PluginSnapshot>>,
    #[serde(default)]
    pub removed: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            last_heartbeat_ms: now_ms(),
            previous_heartbeat_ms: None,
            hooks: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            removed: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListedSession {
    pub provider: String,
    pub session_id: String,
    pub status: AgentStatus,
    pub source: Source,
    pub cwd: Option<String>,
    pub cmdline: Vec<String>,
    pub pid: Option<u32>,
    pub last_report_ms: u64,
    pub kind: SessionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exited: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Store {
    pub fn on_server_start(&mut self) {
        let now = now_ms();
        if self.last_heartbeat_ms > 0 {
            self.previous_heartbeat_ms = Some(self.last_heartbeat_ms);
        }
        self.last_heartbeat_ms = now;
    }

    pub fn heartbeat(&mut self) {
        self.last_heartbeat_ms = now_ms();
    }

    pub fn update(&mut self, provider: &str, payload: Value) -> Result<Change> {
        if !payload.is_object() {
            anyhow::bail!("provide a JSON object");
        }
        if payload.get("hook_event_name").is_some() || payload.get("hookEventName").is_some() {
            let sid = crate::status::session_key(&payload);
            if let Some(sid) = &sid {
                self.clear_removed(provider, sid);
            }
            let bucket = self.hooks.entry(provider.to_string()).or_default();
            providers::apply_hook(provider, bucket, &payload);
            if let Some(sid) = sid {
                if let Some(session) = bucket.get_mut(&sid) {
                    touch_session(session, &payload, provider);
                    if session.exited {
                        bucket.remove(&sid);
                    }
                } else if crate::status::string_field(
                    &payload,
                    &["hook_event_name", "hookEventName"],
                )
                .is_some_and(|name| name.eq_ignore_ascii_case("SessionEnd"))
                {
                    // already removed
                }
            }
            return Ok(Change::Hooks {
                provider: provider.to_string(),
            });
        }
        let instance = payload
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("provide a string id"))?;
        let raw_status = payload
            .get("status")
            .ok_or_else(|| anyhow::anyhow!("status must be an object"))?;
        let obj = raw_status
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("status must be an object"))?;
        let mut status = BTreeMap::new();
        for (sid, kind) in obj {
            let kind = kind
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("status values must be idle, busy, or retry"))?;
            if !matches!(kind, "idle" | "busy" | "retry") {
                anyhow::bail!("status values must be idle, busy, or retry");
            }
            status.insert(sid.clone(), kind.to_string());
        }
        for sid in status.keys() {
            self.clear_removed(provider, sid);
        }
        let blocking = match payload.get("blocking") {
            None => Vec::new(),
            Some(Value::Array(items)) => {
                let mut out = Vec::new();
                for item in items {
                    let sid = item
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("blocking must be an array of strings"))?;
                    out.push(sid.to_string());
                }
                out
            }
            Some(_) => anyhow::bail!("blocking must be an array of strings"),
        };
        let titles = match payload.get("titles") {
            None => BTreeMap::new(),
            Some(Value::Object(items)) => {
                let mut out = BTreeMap::new();
                for (sid, title) in items {
                    let title = title
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("titles must be an object of strings"))?;
                    out.insert(sid.clone(), providers::normalize_title(provider, title));
                }
                out
            }
            Some(_) => anyhow::bail!("titles must be an object of strings"),
        };
        let pid = u32_field(&payload, &["pid"]).or_else(|| instance.parse().ok());
        let snapshot = PluginSnapshot {
            status,
            blocking,
            titles,
            cwd: cwd_field(&payload),
            cmdline: string_list(&payload, "cmdline"),
            pid,
            last_report_ms: now_ms(),
        };
        self.snapshots
            .entry(provider.to_string())
            .or_default()
            .insert(instance.to_string(), snapshot);
        Ok(Change::Snapshot {
            provider: provider.to_string(),
            instance: instance.to_string(),
        })
    }

    pub fn mark_removed(&mut self, provider: &str, session_id: &str) -> u64 {
        let at = now_ms();
        self.removed
            .entry(provider.to_string())
            .or_default()
            .insert(session_id.to_string(), at);
        at
    }

    pub fn clear_removed(&mut self, provider: &str, session_id: &str) {
        if let Some(bucket) = self.removed.get_mut(provider) {
            bucket.remove(session_id);
        }
    }

    pub fn is_removed(&self, provider: &str, session_id: &str) -> bool {
        self.removed
            .get(provider)
            .is_some_and(|bucket| bucket.contains_key(session_id))
    }

    pub fn discover(&mut self, ctx: &crate::paths::Context) -> bool {
        let claude =
            crate::providers::discover_claude(ctx, self.hooks.entry("claude".into()).or_default());
        let codex = crate::providers::drop_archived_codex(
            ctx,
            self.hooks.entry("codex".into()).or_default(),
        );
        claude || codex
    }

    pub fn listed(&self) -> Vec<ListedSession> {
        let mut out = Vec::new();
        for (provider, sessions) in &self.hooks {
            for (sid, session) in sessions {
                let mut cmdline = session.cmdline.clone();
                if cmdline.is_empty() {
                    cmdline = providers::resume_cmd(provider, sid);
                }
                out.push(ListedSession {
                    provider: provider.clone(),
                    session_id: sid.clone(),
                    status: session.status,
                    source: session.source,
                    cwd: session.cwd.clone(),
                    cmdline,
                    pid: session.pid,
                    last_report_ms: session.last_report_ms,
                    kind: SessionKind::Hook,
                    parent_id: session.parent_id.clone(),
                    exited: session.exited,
                    title: session.title.clone(),
                });
            }
        }
        for (provider, instances) in &self.snapshots {
            for snapshot in instances.values() {
                let blocking: std::collections::BTreeSet<_> =
                    snapshot.blocking.iter().cloned().collect();
                for (sid, kind) in &snapshot.status {
                    let status = if blocking.contains(sid) {
                        AgentStatus::Waiting
                    } else if matches!(kind.as_str(), "busy" | "retry") {
                        AgentStatus::Working
                    } else {
                        AgentStatus::Idle
                    };
                    let mut cmdline = snapshot.cmdline.clone();
                    if cmdline.is_empty() {
                        cmdline = providers::resume_cmd(provider, sid);
                    }
                    out.push(ListedSession {
                        provider: provider.clone(),
                        session_id: sid.clone(),
                        status,
                        source: Source::Cli,
                        cwd: snapshot.cwd.clone(),
                        cmdline,
                        pid: snapshot.pid,
                        last_report_ms: snapshot.last_report_ms,
                        kind: SessionKind::Plugin,
                        parent_id: None,
                        exited: false,
                        title: snapshot.titles.get(sid).cloned(),
                    });
                }
            }
        }
        out.retain(|session| !self.is_removed(&session.provider, &session.session_id));
        let mut best: BTreeMap<(String, String, SessionKind), ListedSession> = BTreeMap::new();
        for session in out {
            let key = (
                session.provider.clone(),
                session.session_id.clone(),
                session.kind,
            );
            match best.get(&key) {
                Some(existing) if existing.last_report_ms >= session.last_report_ms => {}
                _ => {
                    best.insert(key, session);
                }
            }
        }
        let mut out: Vec<ListedSession> = best.into_values().collect();
        out.sort_by(|a, b| (&a.provider, &a.session_id).cmp(&(&b.provider, &b.session_id)));
        out
    }

    pub fn active(&self) -> Vec<ListedSession> {
        let now = now_ms();
        self.listed()
            .into_iter()
            .filter(|session| session.is_active(now))
            .collect()
    }

    pub fn resumable(&self, idle: Option<Duration>) -> Vec<ListedSession> {
        let now = now_ms();
        self.listed()
            .into_iter()
            .filter(|session| session.is_resumable(self, now, idle))
            .collect()
    }

    pub fn prune(&mut self, now_ms: u64, retention: Duration) -> bool {
        let cutoff = now_ms.saturating_sub(retention.as_millis() as u64);
        let mut changed = false;

        for bucket in self.hooks.values_mut() {
            let before = bucket.len();
            bucket.retain(|_, session| {
                session.pid.is_some_and(pid_alive) || session.last_report_ms >= cutoff
            });
            changed |= bucket.len() != before;
        }
        self.hooks.retain(|_, bucket| !bucket.is_empty());

        for bucket in self.snapshots.values_mut() {
            let before = bucket.len();
            bucket.retain(|_, snapshot| {
                snapshot.pid.is_some_and(pid_alive) || snapshot.last_report_ms >= cutoff
            });
            changed |= bucket.len() != before;
        }
        self.snapshots.retain(|_, bucket| !bucket.is_empty());

        let mut live: BTreeSet<(String, String)> = BTreeSet::new();
        for (provider, bucket) in &self.hooks {
            for sid in bucket.keys() {
                live.insert((provider.clone(), sid.clone()));
            }
        }
        for (provider, bucket) in &self.snapshots {
            for snapshot in bucket.values() {
                for sid in snapshot.status.keys() {
                    live.insert((provider.clone(), sid.clone()));
                }
            }
        }
        for (provider, bucket) in self.removed.iter_mut() {
            let before = bucket.len();
            bucket.retain(|sid, _| live.contains(&(provider.clone(), sid.clone())));
            changed |= bucket.len() != before;
        }
        self.removed.retain(|_, bucket| !bucket.is_empty());
        changed
    }
}

impl ListedSession {
    pub fn is_active(&self, now_ms: u64) -> bool {
        if self.exited || self.parent_id.is_some() {
            return false;
        }
        match self.kind {
            SessionKind::Plugin => {
                now_ms.saturating_sub(self.last_report_ms) <= PLUGIN_STALE.as_millis() as u64
            }
            SessionKind::Hook => {
                self.status.is_busy()
                    || self.pid.is_some_and(pid_alive)
                    || (self.source == Source::Desktop && !self.exited)
            }
        }
    }

    pub fn process_attached(&self) -> bool {
        self.pid.is_some_and(pid_alive)
    }

    pub fn is_resumable(&self, store: &Store, now_ms: u64, idle: Option<Duration>) -> bool {
        if self.exited || self.parent_id.is_some() {
            return false;
        }
        if self.process_attached() {
            return false;
        }
        if self.cwd.as_ref().is_none_or(|cwd| cwd.is_empty()) {
            return false;
        }
        if self.status.is_busy() {
            return true;
        }
        if !matches!(self.status, AgentStatus::Idle | AgentStatus::Done) {
            return false;
        }
        let Some(idle) = idle else {
            return false;
        };
        idle_age_ms(self, store, now_ms) <= idle.as_millis() as u64
    }
}

fn idle_age_ms(session: &ListedSession, store: &Store, now_ms: u64) -> u64 {
    let anchor = store.previous_heartbeat_ms.unwrap_or(now_ms);
    if session.last_report_ms > anchor {
        now_ms.saturating_sub(session.last_report_ms)
    } else {
        anchor.saturating_sub(session.last_report_ms)
    }
}

fn touch_session(session: &mut AgentSession, payload: &Value, provider: &str) {
    session.last_report_ms = now_ms();
    if let Some(cwd) = cwd_field(payload) {
        session.cwd = Some(cwd);
    }
    if let Some(title) = title_field(payload) {
        session.title = Some(title);
    }
    if let Some(pid) = u32_field(payload, &["pid"]) {
        session.pid = Some(pid);
    }
    if session.cmdline.is_empty()
        && let Some(sid) = crate::status::session_key(payload)
    {
        session.cmdline = providers::resume_cmd(provider, &sid);
    }
    let name = string_field(payload, &["hook_event_name", "hookEventName"]).unwrap_or("");
    if name.eq_ignore_ascii_case("SessionEnd") {
        session.exited = true;
    }
}

fn string_list(payload: &Value, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
