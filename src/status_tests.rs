use super::*;

fn emit(
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
    name: AgentEventKind,
    sid: &str,
) {
    apply_event(sessions, AgentEvent::new(sid, name));
}

#[test]
fn turn_waits_resumes_and_ignores_late_tool_events() {
    let mut sessions = std::collections::BTreeMap::new();
    emit(&mut sessions, AgentEventKind::SessionStart, "s");
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    assert!(sessions.is_empty());
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
    emit(&mut sessions, AgentEventKind::PermissionRequest, "s");
    emit(&mut sessions, AgentEventKind::ToolStart, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Working);
    emit(&mut sessions, AgentEventKind::Stop, "s");
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    emit(&mut sessions, AgentEventKind::ToolStart, "s");
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Working);
    emit(&mut sessions, AgentEventKind::SessionEnd, "s");
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    assert!(sessions.is_empty());
}

#[test]
fn notifications_do_not_revive_finished_turns() {
    let mut sessions = std::collections::BTreeMap::new();
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
    emit(&mut sessions, AgentEventKind::Notification, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
    emit(&mut sessions, AgentEventKind::Stop, "s");
    emit(&mut sessions, AgentEventKind::Notification, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    emit(&mut sessions, AgentEventKind::QuestionAsked, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
}

#[test]
fn idle_pruning_is_scoped_to_source_and_process() {
    let mut sessions = std::collections::BTreeMap::new();
    for (sid, source, pid) in [
        ("first", Source::Cli, 1),
        ("second", Source::Cli, 2),
        ("desktop", Source::Desktop, 1),
    ] {
        apply_event(
            &mut sessions,
            AgentEvent {
                session_id: sid.into(),
                kind: AgentEventKind::PromptSubmit,
                source,
                pid: Some(pid),
                parent_id: None,
                background_running: false,
            },
        );
        apply_event(
            &mut sessions,
            AgentEvent {
                session_id: sid.into(),
                kind: AgentEventKind::Stop,
                source,
                pid: Some(pid),
                parent_id: None,
                background_running: false,
            },
        );
    }
    apply_event(
        &mut sessions,
        AgentEvent {
            session_id: "new".into(),
            kind: AgentEventKind::SessionStart,
            source: Source::Cli,
            pid: Some(1),
            parent_id: None,
            background_running: false,
        },
    );
    assert_eq!(
        sessions.keys().cloned().collect::<Vec<_>>(),
        vec!["desktop".to_string(), "second".to_string()]
    );
}

#[test]
fn session_key_combines_agent_id() {
    assert_eq!(
        session_key(&serde_json::json!({"session_id":"parent","agent_id":"child"})).as_deref(),
        Some("parent:child"),
    );
    assert_eq!(
        session_key(&serde_json::json!({"agent_id":"child"})).as_deref(),
        Some("child"),
    );
    assert_eq!(
        session_key(&serde_json::json!({"session_id":"s"})).as_deref(),
        Some("s")
    );
    assert_eq!(session_key(&serde_json::json!({})), None);
}

#[test]
fn cwd_and_pid_fields_accept_aliases() {
    let payload = serde_json::json!({"workDir":"/tmp/proj","pid":"42"});
    assert_eq!(cwd_field(&payload).as_deref(), Some("/tmp/proj"));
    assert_eq!(u32_field(&payload, &["pid"]), Some(42));
}

#[test]
fn title_field_reads_common_names() {
    assert_eq!(
        title_field(&serde_json::json!({"name":"Claude Desktop hooks compatibility"})).as_deref(),
        Some("Claude Desktop hooks compatibility"),
    );
    assert_eq!(
        title_field(&serde_json::json!({"thread_name":"fix the build"})).as_deref(),
        Some("fix the build"),
    );
}

#[test]
fn stop_with_background_work_stays_working() {
    let mut sessions = std::collections::BTreeMap::new();
    apply_event(
        &mut sessions,
        AgentEvent::new("s", AgentEventKind::PromptSubmit),
    );
    apply_event(
        &mut sessions,
        AgentEvent {
            session_id: "s".into(),
            kind: AgentEventKind::Stop,
            source: Source::Cli,
            pid: None,
            parent_id: None,
            background_running: true,
        },
    );
    assert_eq!(sessions["s"].status, AgentStatus::Working);
    assert!(sessions["s"].background_only);
}

#[test]
fn child_done_is_removed() {
    let mut sessions = std::collections::BTreeMap::new();
    apply_event(
        &mut sessions,
        AgentEvent {
            session_id: "child".into(),
            kind: AgentEventKind::PromptSubmit,
            source: Source::Cli,
            pid: None,
            parent_id: Some("parent".into()),
            background_running: false,
        },
    );
    apply_event(
        &mut sessions,
        AgentEvent {
            session_id: "child".into(),
            kind: AgentEventKind::Stop,
            source: Source::Cli,
            pid: None,
            parent_id: Some("parent".into()),
            background_running: false,
        },
    );
    assert!(sessions.is_empty());
}
