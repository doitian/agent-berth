use super::*;
use crate::status::AgentSession;
use crate::store::Store;

fn session(provider: &str, id: &str, created_ms: u64, last_report_ms: u64) -> ListedSession {
    let mut store = Store::default();
    store.hooks.entry(provider.into()).or_default().insert(
        id.into(),
        AgentSession {
            created_ms,
            last_report_ms,
            title: Some("Fix login flow".into()),
            cwd: Some("/missing-project".into()),
            ..Default::default()
        },
    );
    store.stored().pop().unwrap()
}

#[test]
fn searches_all_providers_and_inactive_sessions_newest_first() {
    let sessions = vec![
        session("claude", "old", 10, 30),
        session("pi", "new", 20, 20),
        session("codex", "middle", 15, 15),
    ];
    let results = filter(sessions, &[], None, 100);
    assert_eq!(
        results
            .iter()
            .map(|s| s.provider.as_str())
            .collect::<Vec<_>>(),
        ["pi", "codex", "claude"]
    );
    assert!(results.iter().all(|s| !s.is_active(100)));
}

#[test]
fn creation_window_uses_creation_not_activity_and_includes_boundary() {
    let results = filter(
        vec![
            session("claude", "old-but-active", 1, 10_000),
            session("pi", "boundary", 7_000, 8_000),
            session("codex", "recent", 9_000, 9_000),
            session("grok", "future", 10_001, 10_001),
        ],
        &[],
        Some(Duration::from_secs(3)),
        10_000,
    );
    assert_eq!(
        results
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["recent", "boundary"]
    );
}

#[test]
fn keywords_are_case_insensitive_and_all_must_match() {
    let sessions = vec![
        session("claude", "abc", 10, 10),
        session("pi", "xyz", 20, 20),
    ];
    let patterns = vec![
        "CLAUDE Login".into(),
        "abc".into(),
        "missing-project".into(),
    ];
    let results = filter(sessions.clone(), &patterns, None, 100);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, "abc");
    assert!(filter(sessions, &["not-found".into()], None, 100).is_empty());
}

#[test]
fn creation_window_can_exceed_timestamp_range() {
    let results = filter(
        vec![session("pi", "s", 10, 10)],
        &[],
        Some(Duration::MAX),
        100,
    );
    assert_eq!(results.len(), 1);
}

#[test]
fn stored_sessions_include_closed_tabs_but_not_explicit_removals() {
    let mut store = Store::default();
    store
        .update(
            "opencode",
            serde_json::json!({
                "id": "server", "status": {"closed": "idle", "removed": "idle"}
            }),
        )
        .unwrap();
    store
        .hosted
        .entry("opencode".into())
        .or_default()
        .insert("closed".into());
    store.mark_removed("opencode", "removed");
    assert!(store.listed().is_empty());
    let results = store.stored();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, "closed");
}
