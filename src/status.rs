use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentEventKind {
    SessionStart,
    PromptSubmit,
    ToolStart,
    ToolComplete,
    PermissionRequest,
    QuestionAsked,
    Notification,
    Stop,
    SessionEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    #[default]
    Idle,
    Working,
    Waiting,
    Done,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Done => "done",
        }
    }

    pub fn is_busy(self) -> bool {
        matches!(self, Self::Working | Self::Waiting)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Cli,
    Desktop,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Desktop => "desktop",
        }
    }

    pub fn from_label(value: &str) -> Self {
        if value.to_ascii_lowercase().contains("desktop") {
            Self::Desktop
        } else {
            Self::Cli
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub session_id: String,
    pub kind: AgentEventKind,
    pub source: Source,
    pub pid: Option<u32>,
    pub parent_id: Option<String>,
    pub background_running: bool,
}

impl AgentEvent {
    pub fn new(session_id: impl Into<String>, kind: AgentEventKind) -> Self {
        Self {
            session_id: session_id.into(),
            kind,
            source: Source::Cli,
            pid: None,
            parent_id: None,
            background_running: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    #[serde(default)]
    pub status: AgentStatus,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub background_only: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub cmdline: Vec<String>,
    #[serde(default)]
    pub last_report_ms: u64,
    #[serde(default)]
    pub exited: bool,
    #[serde(default)]
    pub discovered: bool,
    #[serde(default)]
    pub title: Option<String>,
}

impl Default for AgentSession {
    fn default() -> Self {
        Self {
            status: AgentStatus::Idle,
            source: Source::Cli,
            pid: None,
            parent_id: None,
            background_only: false,
            cwd: None,
            cmdline: Vec::new(),
            last_report_ms: 0,
            exited: false,
            discovered: false,
            title: None,
        }
    }
}

impl AgentSession {
    pub fn apply(&mut self, event: &AgentEvent) {
        self.source = event.source;
        if event.pid.is_some() {
            self.pid = event.pid;
        }
        if event.parent_id.is_some() {
            self.parent_id = event.parent_id.clone();
        }
        match event.kind {
            AgentEventKind::PromptSubmit => {
                self.status = AgentStatus::Working;
                self.background_only = false;
            }
            AgentEventKind::ToolStart => {
                if self.status == AgentStatus::Idle {
                    self.status = AgentStatus::Working;
                }
                if self.status == AgentStatus::Working {
                    self.background_only = false;
                }
            }
            AgentEventKind::ToolComplete => {
                if self.status == AgentStatus::Waiting {
                    self.status = AgentStatus::Working;
                }
                if self.status == AgentStatus::Working {
                    self.background_only = false;
                }
            }
            AgentEventKind::PermissionRequest | AgentEventKind::QuestionAsked => {
                self.status = AgentStatus::Waiting;
            }
            AgentEventKind::Notification => {
                if self.status == AgentStatus::Working {
                    self.status = AgentStatus::Waiting;
                }
            }
            AgentEventKind::Stop => {
                self.status = if event.background_running {
                    AgentStatus::Working
                } else {
                    AgentStatus::Done
                };
                self.background_only = event.background_running;
            }
            AgentEventKind::SessionStart | AgentEventKind::SessionEnd => {}
        }
    }
}

pub fn prune(
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
    keep: Option<&str>,
    source: Option<Source>,
    pid: Option<u32>,
) {
    sessions.retain(|sid, item| {
        if Some(sid.as_str()) == keep || item.status.is_busy() {
            return true;
        }
        if source.is_some() && Some(item.source) != source {
            return true;
        }
        item.pid != pid
    });
}

pub fn apply_event(
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
    event: AgentEvent,
) {
    let sid = event.session_id.clone();
    let kind = event.kind;
    let existing_pid = sessions.get(&sid).and_then(|item| item.pid);
    let pid = event.pid.or(existing_pid);
    if kind == AgentEventKind::SessionEnd {
        sessions.remove(&sid);
        sessions.retain(|_, child| child.parent_id.as_deref() != Some(sid.as_str()));
        return;
    }
    if kind == AgentEventKind::SessionStart {
        prune(sessions, Some(&sid), Some(event.source), pid);
        return;
    }
    if !sessions.contains_key(&sid) {
        if matches!(
            kind,
            AgentEventKind::ToolComplete | AgentEventKind::Notification
        ) {
            return;
        }
        if kind == AgentEventKind::Stop && !event.background_running {
            return;
        }
        sessions.insert(sid.clone(), AgentSession::default());
    }
    let parent_id;
    let source;
    let session_pid;
    {
        let item = sessions.get_mut(&sid).expect("session inserted");
        item.apply(&event);
        parent_id = item.parent_id.clone();
        source = item.source;
        session_pid = item.pid;
    }
    if parent_id.is_some() {
        if sessions
            .get(&sid)
            .is_some_and(|item| item.status == AgentStatus::Done)
        {
            sessions.remove(&sid);
        }
    } else if matches!(kind, AgentEventKind::PromptSubmit | AgentEventKind::Stop) {
        prune(sessions, Some(&sid), Some(source), session_pid);
    }
}

pub fn session_key(payload: &serde_json::Value) -> Option<String> {
    let sid = string_field(payload, &["session_id", "sessionId"]);
    let agent = string_field(payload, &["agent_id", "agentId"]);
    match (sid, agent) {
        (Some(sid), Some(agent)) => Some(format!("{sid}:{agent}")),
        (None, Some(agent)) => Some(agent.to_string()),
        (Some(sid), None) => Some(sid.to_string()),
        (None, None) => None,
    }
}

pub fn string_field<'a>(payload: &'a serde_json::Value, names: &[&str]) -> Option<&'a str> {
    let obj = payload.as_object()?;
    for name in names {
        match obj.get(*name) {
            Some(serde_json::Value::String(value)) if !value.is_empty() => return Some(value),
            _ => {}
        }
    }
    None
}

pub fn u32_field(payload: &serde_json::Value, names: &[&str]) -> Option<u32> {
    let obj = payload.as_object()?;
    for name in names {
        match obj.get(*name) {
            Some(serde_json::Value::Number(n)) => {
                if let Some(v) = n.as_u64() {
                    return u32::try_from(v).ok();
                }
            }
            Some(serde_json::Value::String(s)) => {
                if let Ok(v) = s.parse() {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn title_field(payload: &serde_json::Value) -> Option<String> {
    string_field(
        payload,
        &[
            "title",
            "name",
            "thread_name",
            "session_name",
            "conversation_title",
            "display_name",
        ],
    )
    .map(str::to_string)
}

pub fn cwd_field(payload: &serde_json::Value) -> Option<String> {
    string_field(
        payload,
        &[
            "cwd",
            "workdir",
            "workDir",
            "working_directory",
            "work_dir",
            "directory",
        ],
    )
    .map(str::to_string)
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
