use super::*;

#[cfg(target_os = "linux")]
#[test]
fn niri_focus_matches_app_id_and_prefers_recent_claude_window() {
    let mut windows = vec![
        json!({"id": 1, "app_id": "kitty", "title": "Claude", "is_focused": true}),
        json!({"id": 2, "app_id": "com.anthropic.Claude", "focus_timestamp": {"secs": 10, "nanos": 1}}),
        json!({"id": 3, "app_id": "com.anthropic.Claude", "focus_timestamp": {"secs": 10, "nanos": 2}}),
        json!({"app_id": "com.anthropic.Claude", "is_focused": true}),
    ];
    assert_eq!(niri_claude_window(&windows), Some(3));
    windows[1]["is_focused"] = json!(true);
    assert_eq!(niri_claude_window(&windows), Some(2));
    assert_eq!(niri_claude_window(&windows[..1]), None);
    assert_eq!(niri_claude_window(&[]), None);
}

#[test]
fn desktop_focus_resolves_cli_and_local_ids_from_nested_records() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let dir = ctx
        .xdg_config_home
        .join("Claude/claude-code-sessions/account/org");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.json"), "{").unwrap();
    std::fs::write(
        dir.join("session.json"),
        json!({"cliSessionId": "cli-uuid", "sessionId": "local_desktop-uuid"}).to_string(),
    )
    .unwrap();
    for id in ["cli-uuid", "local_desktop-uuid"] {
        assert_eq!(
            desktop_url(&ctx, id).unwrap(),
            "claude://code/continue?session=local_desktop-uuid"
        );
    }
    assert!(desktop_url(&ctx, "unknown").is_err());
}

#[test]
fn desktop_focus_rejects_archived_missing_and_invalid_local_ids() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let dir = ctx.xdg_config_home.join("Claude/claude-code-sessions");
    std::fs::create_dir_all(&dir).unwrap();
    for record in [
        json!({"cliSessionId": "cli-uuid", "sessionId": "local_archived", "isArchived": true}),
        json!({"cliSessionId": "cli-uuid"}),
        json!({"cliSessionId": "cli-uuid", "sessionId": "local_"}),
        json!({"cliSessionId": "cli-uuid", "sessionId": "local_a&session=last"}),
        json!({"cliSessionId": "cli-uuid", "sessionId": "last"}),
    ] {
        std::fs::write(dir.join("session.json"), record.to_string()).unwrap();
        assert!(desktop_url(&ctx, "cli-uuid").is_err(), "{record}");
    }
}

#[test]
fn drop_archived_removes_matching_ids() {
    let mut sessions = BTreeMap::new();
    sessions.insert("keep".into(), AgentSession::default());
    sessions.insert("gone".into(), AgentSession::default());
    let archived = HashSet::from(["gone".to_string()]);
    assert!(drop_archived(&mut sessions, &archived));
    assert!(sessions.contains_key("keep"));
    assert!(!sessions.contains_key("gone"));
}

#[test]
fn agents_list_drops_desktop_sessions_that_are_gone() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::new();
    sessions.insert(
        "probe-manual-001".into(),
        AgentSession {
            status: AgentStatus::Running,
            source: Source::Desktop,
            cwd: Some("/work".into()),
            ..AgentSession::default()
        },
    );
    sessions.insert(
        "cli-keep".into(),
        AgentSession {
            status: AgentStatus::Running,
            source: Source::Cli,
            ..AgentSession::default()
        },
    );
    apply_agents(&ctx, &mut sessions, &[], &HashSet::new(), now_ms());
    assert!(sessions.contains_key("cli-keep"));
    assert!(!sessions.contains_key("probe-manual-001"));
}

fn write_registration(ctx: &Context, registration: Value) {
    let dir = ctx.claude_config_dir.join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let pid = registration["pid"].as_u64().unwrap();
    std::fs::write(dir.join(format!("{pid}.json")), registration.to_string()).unwrap();
}

#[test]
fn opening_desktop_session_does_not_list_temporary_query() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut rows = Vec::new();
    for (pid, sid, entrypoint, host) in [
        (
            1,
            "conversation",
            "claude-desktop",
            Some("local_conversation"),
        ),
        (2, "temporary-query", "claude-desktop", None),
        (3, "terminal", "cli", None),
        (4, "sdk", "sdk-ts", None),
    ] {
        write_registration(
            &ctx,
            json!({
                "pid": pid, "sessionId": sid, "entrypoint": entrypoint,
                "hostSessionId": host, "kind": "interactive"
            }),
        );
        // `claude agents --json` omits entrypoint and hostSessionId.
        rows.push(json!({
            "pid": pid, "sessionId": sid, "cwd": "/work",
            "kind": "interactive", "status": "idle"
        }));
    }
    let mut sessions = BTreeMap::new();
    let desktop_ids = HashSet::from(["conversation".to_string()]);
    apply_agents(&ctx, &mut sessions, &rows, &desktop_ids, now_ms());
    assert_eq!(
        sessions.keys().map(String::as_str).collect::<Vec<_>>(),
        ["conversation", "sdk", "terminal"]
    );
    assert_eq!(sessions["conversation"].source, Source::Desktop);

    // The next refresh, after the helper exits, must keep the same rows.
    rows.retain(|row| row["sessionId"] != "temporary-query");
    apply_agents(&ctx, &mut sessions, &rows, &desktop_ids, now_ms());
    assert_eq!(sessions.len(), 3);
}

#[test]
fn agents_list_removes_previously_discovered_desktop_helper() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    write_registration(
        &ctx,
        json!({"pid": 2, "sessionId": "helper", "entrypoint": "claude-desktop"}),
    );
    let mut sessions = BTreeMap::from([(
        "helper".into(),
        AgentSession {
            discovered: true,
            pid: Some(2),
            ..AgentSession::default()
        },
    )]);
    apply_agents(
        &ctx,
        &mut sessions,
        &[json!({"pid": 2, "sessionId": "helper", "status": "idle"})],
        &HashSet::new(),
        now_ms(),
    );
    assert!(sessions.is_empty());
}

#[test]
fn desktop_discovery_keeps_conversations_and_background_agents() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut rows = Vec::new();
    for (pid, sid, host, kind) in [
        (1, "legacy-conversation", None, "interactive"),
        (2, "new-conversation", Some("local_new"), "interactive"),
        (3, "background-agent", None, "bg"),
    ] {
        write_registration(
            &ctx,
            json!({
                "pid": pid, "sessionId": sid, "entrypoint": "claude-desktop",
                "hostSessionId": host, "kind": kind
            }),
        );
        rows.push(json!({
            "pid": pid, "sessionId": sid,
            "kind": if kind == "bg" { "background" } else { kind },
            "status": "idle"
        }));
    }
    let mut sessions = BTreeMap::new();
    // Older registrations have no host ID; new conversations may not yet
    // have reached the desktop index. Neither should be treated as a helper.
    apply_agents(
        &ctx,
        &mut sessions,
        &rows,
        &HashSet::from(["legacy-conversation".into()]),
        now_ms(),
    );
    assert_eq!(sessions.len(), 3);
}

#[test]
fn discovery_keeps_sessions_without_a_matching_readable_registration() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    write_registration(
        &ctx,
        json!({"pid": 1, "sessionId": "old-session", "entrypoint": "claude-desktop"}),
    );
    let dir = ctx.claude_config_dir.join("sessions");
    std::fs::write(dir.join("2.json"), "{").unwrap();
    std::fs::write(
        dir.join("3.json"),
        json!({"pid": 99, "sessionId": "s3", "entrypoint": "claude-desktop"}).to_string(),
    )
    .unwrap();
    let rows = (1..=4)
        .map(|pid| json!({"pid": pid, "sessionId": format!("s{pid}"), "status": "idle"}))
        .collect::<Vec<_>>();
    let mut sessions = BTreeMap::new();
    apply_agents(&ctx, &mut sessions, &rows, &HashSet::new(), now_ms());
    assert_eq!(sessions.len(), 4);
}

#[test]
fn discovery_does_not_revive_a_finished_hook_session() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([
        (
            "stopped".into(),
            AgentSession {
                status: AgentStatus::Done,
                hooked: true,
                pid: Some(1),
                ..AgentSession::default()
            },
        ),
        (
            "waiting".into(),
            AgentSession {
                status: AgentStatus::Waiting,
                hooked: true,
                pid: Some(2),
                ..AgentSession::default()
            },
        ),
    ]);
    // `claude agents --json` still reports the session as busy while the Stop
    // hook runs, and reports idle once the turn is over.
    let busy = vec![
        json!({"pid": 1, "sessionId": "stopped", "status": "busy"}),
        json!({"pid": 2, "sessionId": "waiting", "status": "busy"}),
    ];
    apply_agents(&ctx, &mut sessions, &busy, &HashSet::new(), now_ms());
    assert_eq!(sessions["stopped"].status, AgentStatus::Done);
    assert_eq!(sessions["waiting"].status, AgentStatus::Waiting);

    let idle = busy
        .iter()
        .map(|row| {
            let mut row = row.clone();
            row["status"] = json!("idle");
            row
        })
        .collect::<Vec<_>>();
    apply_agents(&ctx, &mut sessions, &idle, &HashSet::new(), now_ms());
    assert_eq!(sessions["stopped"].status, AgentStatus::Done);
    assert_eq!(sessions["waiting"].status, AgentStatus::Waiting);
}

#[test]
fn idle_agents_row_ends_a_run_whose_stop_hook_never_arrived() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([
        (
            "interrupted".into(),
            AgentSession {
                status: AgentStatus::Running,
                hooked: true,
                pid: Some(1),
                ..AgentSession::default()
            },
        ),
        (
            "background".into(),
            AgentSession {
                status: AgentStatus::Running,
                background_only: true,
                hooked: true,
                pid: Some(2),
                ..AgentSession::default()
            },
        ),
        (
            "discovered".into(),
            AgentSession {
                status: AgentStatus::Running,
                discovered: true,
                pid: Some(3),
                ..AgentSession::default()
            },
        ),
    ]);
    let rows = [
        json!({"pid": 1, "sessionId": "interrupted", "status": "idle"}),
        json!({"pid": 2, "sessionId": "background", "status": "idle"}),
        json!({"pid": 3, "sessionId": "discovered", "status": "idle"}),
    ];
    apply_agents(&ctx, &mut sessions, &rows, &HashSet::new(), now_ms());
    for sid in ["interrupted", "background", "discovered"] {
        assert_eq!(sessions[sid].status, AgentStatus::Done, "{sid}");
    }
    assert!(!sessions["background"].background_only);
}

#[test]
fn discovery_still_drives_sessions_that_never_reported_a_hook() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([(
        "discovered".into(),
        AgentSession {
            status: AgentStatus::Done,
            discovered: true,
            pid: Some(1),
            ..AgentSession::default()
        },
    )]);
    apply_agents(
        &ctx,
        &mut sessions,
        &[json!({"pid": 1, "sessionId": "discovered", "status": "busy"})],
        &HashSet::new(),
        now_ms(),
    );
    assert_eq!(sessions["discovered"].status, AgentStatus::Running);
}

#[test]
fn discovery_fills_in_a_status_hooks_never_reported() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([(
        "idle".into(),
        AgentSession {
            hooked: true,
            pid: Some(1),
            ..AgentSession::default()
        },
    )]);
    apply_agents(
        &ctx,
        &mut sessions,
        &[json!({"pid": 1, "sessionId": "idle", "status": "busy"})],
        &HashSet::new(),
        now_ms(),
    );
    assert_eq!(sessions["idle"].status, AgentStatus::Running);
}

#[test]
fn stale_agents_row_does_not_end_a_run_it_predates() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let mut sessions = BTreeMap::from([(
        "running".into(),
        AgentSession {
            status: AgentStatus::Running,
            hooked: true,
            pid: Some(1),
            last_report_ms: 2_000,
            ..AgentSession::default()
        },
    )]);
    let rows = [json!({"pid": 1, "sessionId": "running", "status": "idle"})];
    // The cached snapshot predates the prompt hook that started this run.
    apply_agents(&ctx, &mut sessions, &rows, &HashSet::new(), 1_000);
    assert_eq!(sessions["running"].status, AgentStatus::Running);
    apply_agents(&ctx, &mut sessions, &rows, &HashSet::new(), 3_000);
    assert_eq!(sessions["running"].status, AgentStatus::Done);
}

#[test]
fn turn_terminal_hooks_are_installed_synchronously() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    let dest = install(&ctx).unwrap();
    let data: Value = serde_json::from_slice(&std::fs::read(&dest).unwrap()).unwrap();
    for event in HOOK_EVENTS {
        let handler = data["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .find(|group| group_is_ours(group, "claude"))
            .map(|group| group["hooks"][0].clone())
            .unwrap_or_else(|| panic!("{event} handler missing"));
        let expected = !SYNC_HOOK_EVENTS.contains(event);
        assert_eq!(handler["async"], Value::Bool(expected), "{event}");
    }
}
