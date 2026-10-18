use super::*;

#[test]
fn status_deserializes_legacy_working() {
    let status: AgentStatus = serde_json::from_str("\"working\"").unwrap();
    assert_eq!(status, AgentStatus::Running);
    let session: AgentSession = serde_json::from_str("{\"status\":\"working\"}").unwrap();
    assert_eq!(session.status, AgentStatus::Running);
}

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
    assert_eq!(sessions["s"].status, AgentStatus::Running);
    emit(&mut sessions, AgentEventKind::Stop, "s");
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    emit(&mut sessions, AgentEventKind::ToolStart, "s");
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Running);
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
fn stop_with_background_work_stays_running() {
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
    assert_eq!(sessions["s"].status, AgentStatus::Running);
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

#[test]
fn tool_start_resumes_a_session_marked_done() {
    let mut sessions = std::collections::BTreeMap::from([(
        "s".to_string(),
        AgentSession {
            status: AgentStatus::Done,
            ..AgentSession::default()
        },
    )]);
    apply_event(
        &mut sessions,
        AgentEvent::new("s", AgentEventKind::ToolStart),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);
}

fn turns(sessions: &std::collections::BTreeMap<String, AgentSession>, sid: &str) -> (u64, u64) {
    (sessions[sid].turns_started, sessions[sid].turns_ended)
}

#[test]
fn turn_counters_follow_hook_edges() {
    let mut sessions = std::collections::BTreeMap::new();
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
    assert_eq!(turns(&sessions, "s"), (1, 0));
    // Moves between busy levels and tool events are the same turn.
    emit(&mut sessions, AgentEventKind::PermissionRequest, "s");
    emit(&mut sessions, AgentEventKind::ToolStart, "s");
    emit(&mut sessions, AgentEventKind::ToolComplete, "s");
    emit(&mut sessions, AgentEventKind::Notification, "s");
    assert_eq!(turns(&sessions, "s"), (1, 0));
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(turns(&sessions, "s"), (1, 1));
    // A Stop while already done ends a turn whose start was missed.
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(turns(&sessions, "s"), (1, 2));
    // A tool starting in a done session is the next turn.
    emit(&mut sessions, AgentEventKind::ToolStart, "s");
    assert_eq!(turns(&sessions, "s"), (2, 2));
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(turns(&sessions, "s"), (2, 3));
    emit(&mut sessions, AgentEventKind::QuestionAsked, "s");
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
    assert_eq!(turns(&sessions, "s"), (3, 3));
}

#[test]
fn stop_with_background_work_does_not_end_the_turn() {
    let mut sessions = std::collections::BTreeMap::new();
    emit(&mut sessions, AgentEventKind::PromptSubmit, "s");
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
    assert_eq!(turns(&sessions, "s"), (1, 0));
    emit(&mut sessions, AgentEventKind::Stop, "s");
    assert_eq!(turns(&sessions, "s"), (1, 1));
}

#[test]
fn observed_status_counts_turn_edges() {
    let mut session = AgentSession::default();
    session.observe(AgentStatus::Running);
    assert_eq!((session.turns_started, session.turns_ended), (1, 0));
    session.observe(AgentStatus::Waiting);
    assert_eq!((session.turns_started, session.turns_ended), (1, 0));
    session.observe(AgentStatus::Done);
    assert_eq!((session.turns_started, session.turns_ended), (1, 1));
    session.observe(AgentStatus::Idle);
    assert_eq!((session.turns_started, session.turns_ended), (1, 1));
    assert_eq!(session.status, AgentStatus::Idle);
}

#[test]
fn turn_counters_round_trip_and_default_to_zero() {
    let session = AgentSession {
        turns_started: 3,
        turns_ended: 2,
        ..AgentSession::default()
    };
    let json = serde_json::to_string(&session).unwrap();
    assert_eq!(
        serde_json::from_str::<AgentSession>(&json).unwrap(),
        session
    );
    let legacy: AgentSession = serde_json::from_str("{\"status\":\"done\"}").unwrap();
    assert_eq!((legacy.turns_started, legacy.turns_ended), (0, 0));
}
