use super::*;
use crate::status::{AgentEvent, AgentEventKind, apply_event};

#[test]
fn hook_payload_title_is_listed() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"s",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work",
                "title":"Fix the hooks"
            }),
        )
        .unwrap();
    assert_eq!(store.listed()[0].title.as_deref(), Some("Fix the hooks"));
}

#[test]
fn plugin_payload_title_is_listed() {
    let mut store = Store::default();
    store
        .update(
            "opencode",
            serde_json::json!({
                "id":"p",
                "status":{"s":"busy"},
                "titles":{"s":"Fix the plugin","other":"unused"}
            }),
        )
        .unwrap();
    assert_eq!(store.listed()[0].title.as_deref(), Some("Fix the plugin"));
}

#[test]
fn plugin_and_hook_identities_do_not_collide() {
    let mut store = Store::default();
    store
        .update(
            "codex",
            serde_json::json!({"session_id":"s","hook_event_name":"PermissionRequest","cwd":"/tmp"}),
        )
        .unwrap();
    store
        .update(
            "codex",
            serde_json::json!({"id":"cli","status":{"s":"busy"}}),
        )
        .unwrap();
    let listed = store.listed();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .any(|s| s.kind == SessionKind::Hook && s.status == AgentStatus::Waiting)
    );
    assert!(
        listed
            .iter()
            .any(|s| s.kind == SessionKind::Plugin && s.status == AgentStatus::Working)
    );
}

#[test]
fn busy_session_without_process_is_resumable() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work",
                "pid": 4294967295u64
            }),
        )
        .unwrap();
    let idle = Duration::from_secs(1200);
    let sessions = store.resumable(Some(idle));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "abc");
}

#[test]
fn idle_sessions_need_an_explicit_window() {
    let mut store = Store::default();
    for event in ["UserPromptSubmit", "Stop"] {
        store
            .update(
                "claude",
                serde_json::json!({
                    "session_id":"abc",
                    "hook_event_name":event,
                    "cwd":"/work"
                }),
            )
            .unwrap();
    }
    assert!(store.resumable(None).is_empty());
    assert_eq!(store.resumable(Some(Duration::from_secs(1200))).len(), 1);
}

#[test]
fn session_end_is_not_resumable() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work"
            }),
        )
        .unwrap();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"SessionEnd",
                "cwd":"/work"
            }),
        )
        .unwrap();
    assert!(store.resumable(Some(Duration::from_secs(1200))).is_empty());
}

#[test]
fn apply_event_roundtrip_through_store() {
    let mut sessions = BTreeMap::new();
    apply_event(
        &mut sessions,
        AgentEvent::new("s", AgentEventKind::PromptSubmit),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Working);
}

#[test]
fn idle_session_uses_previous_heartbeat_window() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work"
            }),
        )
        .unwrap();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"Stop",
                "cwd":"/work"
            }),
        )
        .unwrap();
    {
        let session = store
            .hooks
            .get_mut("claude")
            .unwrap()
            .get_mut("abc")
            .unwrap();
        session.last_report_ms = 1_000;
    }
    store.previous_heartbeat_ms = Some(1_000 + 10 * 60 * 1000);
    assert_eq!(store.resumable(Some(Duration::from_secs(20 * 60))).len(), 1);
    store.previous_heartbeat_ms = Some(1_000 + 30 * 60 * 1000);
    assert!(
        store
            .resumable(Some(Duration::from_secs(20 * 60)))
            .is_empty()
    );
}

#[test]
fn desktop_busy_sessions_are_resumable_for_list() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"desk",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work",
                "entrypoint":"claude-desktop"
            }),
        )
        .unwrap();
    let sessions = store.resumable(Some(Duration::from_secs(1200)));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].source, Source::Desktop);
}

#[test]
fn missing_cwd_and_live_pid_are_not_resumable() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"nocwd",
                "hook_event_name":"UserPromptSubmit"
            }),
        )
        .unwrap();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"live",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work",
                "pid": std::process::id()
            }),
        )
        .unwrap();
    assert!(store.resumable(Some(Duration::from_secs(1200))).is_empty());
}

#[test]
fn removed_sessions_are_hidden_until_they_report_again() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"UserPromptSubmit",
                "cwd":"/work"
            }),
        )
        .unwrap();
    store.mark_removed("claude", "abc");
    assert!(store.listed().is_empty());
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id":"abc",
                "hook_event_name":"PermissionRequest",
                "cwd":"/work"
            }),
        )
        .unwrap();
    assert_eq!(store.listed().len(), 1);
}

#[test]
fn prune_drops_stale_sessions_and_their_tombstones() {
    let mut store = Store::default();
    for sid in ["old", "fresh", "live"] {
        store
            .update(
                "claude",
                serde_json::json!({
                    "session_id": sid,
                    "hook_event_name": "UserPromptSubmit",
                    "cwd": "/work"
                }),
            )
            .unwrap();
    }
    {
        let bucket = store.hooks.get_mut("claude").unwrap();
        bucket.get_mut("old").unwrap().last_report_ms = 1_000;
        bucket.get_mut("fresh").unwrap().last_report_ms = 999_999;
        let live = bucket.get_mut("live").unwrap();
        live.last_report_ms = 1_000;
        live.pid = Some(std::process::id());
    }
    store.mark_removed("claude", "old");
    assert!(store.prune(1_000_000, Duration::from_secs(60)));
    let bucket = &store.hooks["claude"];
    assert!(!bucket.contains_key("old"));
    assert!(bucket.contains_key("fresh"));
    assert!(bucket.contains_key("live"));
    assert!(!store.is_removed("claude", "old"));
}

#[test]
fn prune_is_noop_when_sessions_are_recent() {
    let mut store = Store::default();
    store
        .update(
            "claude",
            serde_json::json!({
                "session_id": "abc",
                "hook_event_name": "UserPromptSubmit",
                "cwd": "/work"
            }),
        )
        .unwrap();
    let now = store.hooks["claude"]["abc"].last_report_ms;
    assert!(!store.prune(now, Duration::from_secs(60)));
}

#[test]
fn duplicate_snapshots_deduplicate_by_session() {
    let mut store = Store::default();
    for instance in ["1", "2"] {
        store
            .update(
                "opencode",
                serde_json::json!({
                    "id": instance,
                    "cwd":"/work",
                    "status":{"same":"idle"}
                }),
            )
            .unwrap();
    }
    let listed = store.listed();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id, "same");
}

#[test]
fn plugin_blocking_is_waiting_and_stale_is_inactive() {
    let mut store = Store::default();
    store
        .update(
            "opencode",
            serde_json::json!({
                "id":"a:b",
                "status":{"c":"busy"}
            }),
        )
        .unwrap();
    store
        .update(
            "opencode",
            serde_json::json!({
                "id":"a",
                "status":{"b:c":"idle"},
                "blocking":["b:c"]
            }),
        )
        .unwrap();
    let listed = store.listed();
    assert!(
        listed
            .iter()
            .any(|s| s.session_id == "c" && s.status == AgentStatus::Working)
    );
    assert!(
        listed
            .iter()
            .any(|s| s.session_id == "b:c" && s.status == AgentStatus::Waiting)
    );
    let now = now_ms();
    assert!(listed.iter().all(|s| s.is_active(now)));
    assert!(
        listed
            .iter()
            .all(|s| !s.is_active(now + PLUGIN_STALE.as_millis() as u64 + 1))
    );
}

#[test]
fn rejects_invalid_notify_payloads() {
    let mut store = Store::default();
    assert!(store.update("claude", serde_json::json!([])).is_err());
    assert!(
        store
            .update("opencode", serde_json::json!({"status":{}}))
            .is_err()
    );
    assert!(
        store
            .update(
                "opencode",
                serde_json::json!({"id":"p","status":{"s":"nope"}})
            )
            .is_err()
    );
    assert!(
        store
            .update(
                "opencode",
                serde_json::json!({"id":"p","status":{},"blocking":[1]})
            )
            .is_err()
    );
    assert!(
        store
            .update(
                "opencode",
                serde_json::json!({"id":"p","status":{},"titles":{"s":1}})
            )
            .is_err()
    );
}
