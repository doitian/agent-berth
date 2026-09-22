use super::*;
use crate::paths::Context as AppContext;
use crate::status::AgentStatus;
use tempfile::tempdir;

#[test]
fn hooks_preserve_known_source_and_children_inherit_the_parent_source() {
    let mut sessions = BTreeMap::new();
    for (originator, expected) in [
        ("Codex Desktop", Source::Desktop),
        ("codex_work_desktop", Source::Desktop),
        ("codex-tui", Source::Cli),
        ("codex_exec", Source::Cli),
    ] {
        apply_hook(
            &mut sessions,
            &json!({
                "session_id":"s", "hook_event_name":"UserPromptSubmit",
                "originator":originator
            }),
        );
        assert_eq!(sessions["s"].source, expected);
        apply_hook(
            &mut sessions,
            &json!({"session_id":"s", "hook_event_name":"PreToolUse"}),
        );
        assert_eq!(sessions["s"].source, expected);
        apply_hook(
            &mut sessions,
            &json!({"session_id":"s", "agent_id":"child", "hook_event_name":"PreToolUse"}),
        );
        assert_eq!(sessions["s:child"].source, expected);
        apply_hook(
            &mut sessions,
            &json!({"session_id":"s", "agent_id":"child", "hook_event_name":"SubagentStop"}),
        );
    }
}

#[test]
fn automatic_approval_review_does_not_wait_for_user_input() {
    let root = tempdir().unwrap();
    let transcript = root.path().join("rollout.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({"type":"turn_context","payload":{
                "turn_id":"turn", "approvals_reviewer":"auto_review"
            }})
        ),
    )
    .unwrap();
    let mut sessions = BTreeMap::new();
    for event in ["UserPromptSubmit", "PreToolUse", "PermissionRequest"] {
        apply_hook(
            &mut sessions,
            &json!({
                "session_id":"s", "hook_event_name":event,
                "turn_id":"turn", "transcript_path":transcript
            }),
        );
        assert_eq!(sessions["s"].status, AgentStatus::Running, "{event}");
    }
}

#[test]
fn automatic_approval_review_recovers_a_persisted_waiting_session() {
    let root = tempdir().unwrap();
    let transcript = root.path().join("rollout.jsonl");
    std::fs::write(
        &transcript,
        "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn\",\"approvals_reviewer\":\"auto_review\"}}\n",
    )
    .unwrap();
    let mut sessions: BTreeMap<String, AgentSession> = serde_json::from_value(json!({
        "s": {"status":"waiting", "background_only":true, "title":"Keep this title"}
    }))
    .unwrap();
    apply_hook(
        &mut sessions,
        &json!({
            "session_id":"s", "hook_event_name":"PermissionRequest",
            "turn_id":"turn", "transcript_path":transcript
        }),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);
    assert!(!sessions["s"].background_only);
    assert_eq!(sessions["s"].title.as_deref(), Some("Keep this title"));
}

#[test]
fn permission_requests_wait_unless_active_turn_confirms_auto_review() {
    let root = tempdir().unwrap();
    let transcript = root.path().join("rollout.jsonl");
    for contexts in [
        vec![],
        vec![json!({"turn_id":"turn", "approvals_reviewer":"user"})],
        vec![json!({"turn_id":"other", "approvals_reviewer":"auto_review"})],
        vec![json!({"turn_id":"turn"})],
        vec![json!({"turn_id":"turn", "approvals_reviewer":"unknown"})],
        vec![
            json!({"turn_id":"turn", "approvals_reviewer":"auto_review"}),
            json!({"turn_id":"turn", "approvals_reviewer":"user"}),
        ],
    ] {
        let text: String = contexts
            .iter()
            .map(|context| format!("{}\n", json!({"type":"turn_context","payload":context})))
            .collect();
        std::fs::write(&transcript, text).unwrap();
        let mut sessions = BTreeMap::new();
        apply_hook(
            &mut sessions,
            &json!({
                "session_id":"s", "hook_event_name":"PermissionRequest",
                "turn_id":"turn", "transcript_path":transcript
            }),
        );
        assert_eq!(sessions["s"].status, AgentStatus::Waiting, "{contexts:?}");
    }
}

#[test]
fn unavailable_approval_context_preserves_permission_waiting() {
    let root = tempdir().unwrap();
    let transcript = root.path().join("missing.jsonl");
    let mut event = json!({
        "session_id":"s", "hook_event_name":"PermissionRequest",
        "turn_id":"turn", "transcript_path":transcript
    });
    for contents in [
        None,
        Some("not json\n{\"type\":\"turn_context\",\"payload\":"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&transcript, contents).unwrap();
        }
        let mut sessions = BTreeMap::new();
        apply_hook(&mut sessions, &event);
        assert_eq!(sessions["s"].status, AgentStatus::Waiting);
    }
    std::fs::write(
        &transcript,
        "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"turn\",\"approvals_reviewer\":\"auto_review\"}}\n",
    )
    .unwrap();
    event.as_object_mut().unwrap().remove("turn_id");
    let mut sessions = BTreeMap::new();
    apply_hook(&mut sessions, &event);
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
}

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
            status: AgentStatus::Running,
            ..AgentSession::default()
        },
    );
    sessions.insert(
        "cli-keep".into(),
        AgentSession {
            source: Source::Cli,
            status: AgentStatus::Running,
            ..AgentSession::default()
        },
    );
    assert!(drop_archived(&ctx, &mut sessions));
    assert!(sessions.contains_key("cli-keep"));
    assert!(!sessions.contains_key("test-desktop"));
}
