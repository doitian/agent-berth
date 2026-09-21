use super::*;
use crate::paths::Context as AppContext;
use crate::status::AgentStatus;
use tempfile::tempdir;

#[test]
fn discovers_latest_index_title_without_changing_activity() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.codex_home).unwrap();
    std::fs::write(
        ctx.codex_home.join("session_index.jsonl"),
        concat!(
            "{\"id\":\"s\",\"thread_name\":\"Original title\"}\n",
            "invalid json\n",
            "{\"id\":\"untracked\",\"thread_name\":\"Other session\"}\n",
            "{\"id\":\"s\",\"thread_name\":\"Renamed session\"}\n",
            "{\"id\":\"s\",\"thread_name\":42}\n",
            "{\"id\":\"s\",\"thread_name\":\"\"}\n",
            "{\"id\":\"s\",\"thread_name\":",
        ),
    )
    .unwrap();
    let mut sessions = BTreeMap::from([(
        "s".into(),
        AgentSession {
            status: AgentStatus::Done,
            last_report_ms: 123,
            ..AgentSession::default()
        },
    )]);

    assert!(discover(&ctx, &mut sessions));
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions["s"].title.as_deref(), Some("Renamed session"));
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    assert_eq!(sessions["s"].last_report_ms, 123);
    assert!(!discover(&ctx, &mut sessions));
}

#[test]
fn missing_index_preserves_hook_title() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([(
        "s".into(),
        AgentSession {
            title: Some("Hook title".into()),
            ..AgentSession::default()
        },
    )]);

    assert!(!discover(&ctx, &mut sessions));
    assert_eq!(sessions["s"].title.as_deref(), Some("Hook title"));
}

#[test]
fn drops_sessions_moved_to_archived_sessions() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
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
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
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
