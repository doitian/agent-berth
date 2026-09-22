use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{ListedSession, ProviderStats};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Notify {
        provider: String,
        payload: Value,
    },
    List {
        resumable: bool,
        #[serde(default)]
        idle_ms: Option<u64>,
    },
    ListAll,
    Stats,
    Remove {
        provider: String,
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sessions: Option<Vec<ListedSession>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stats: Option<Vec<ProviderStats>>,
    },
    Error {
        message: String,
    },
}

impl Response {
    pub fn ok() -> Self {
        Self::Ok {
            message: None,
            sessions: None,
            stats: None,
        }
    }

    pub fn sessions(sessions: Vec<ListedSession>) -> Self {
        Self::Ok {
            message: None,
            sessions: Some(sessions),
            stats: None,
        }
    }

    pub fn stats(stats: Vec<ProviderStats>) -> Self {
        Self::Ok {
            message: None,
            sessions: None,
            stats: Some(stats),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
