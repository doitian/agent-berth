use super::*;
use crate::paths::Context as AppContext;
use crate::status::AgentStatus;
use tempfile::tempdir;

#[test]
fn drops_sessions_moved_to_archived_sessions() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-bridge"));
    let dir = ctx.codex_home.join("archived_sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("rollout-2026-09-20T17-50-44-01a0be39-a601-7e20-bcb1-60122ec3c8c6.jsonl"),
        "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"01a0be39-a601-7e20-bcb1-60122ec3c8c6\"}}\n",
    )
    .unwrap();
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "01a0be39-a601-7e20-bcb1-60122ec3c8c6".into(),
        AgentSession::default(),
    );
    sessions.insert("keep".into(), AgentSession::default());
    assert!(drop_archived(&ctx, &mut sessions));
    assert!(sessions.contains_key("keep"));
    assert!(!sessions.contains_key("01a0be39-a601-7e20-bcb1-60122ec3c8c6"));
}

#[test]
fn drops_desktop_sessions_without_session_files() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-bridge"));
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "test-desktop".into(),
        AgentSession {
            source: Source::Desktop,
            status: AgentStatus::Working,
            ..AgentSession::default()
        },
    );
    sessions.insert(
        "cli-keep".into(),
        AgentSession {
            source: Source::Cli,
            status: AgentStatus::Working,
            ..AgentSession::default()
        },
    );
    assert!(drop_archived(&ctx, &mut sessions));
    assert!(sessions.contains_key("cli-keep"));
    assert!(!sessions.contains_key("test-desktop"));
}
