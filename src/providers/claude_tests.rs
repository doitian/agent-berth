use super::*;

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
            status: AgentStatus::Working,
            source: Source::Desktop,
            cwd: Some("/work".into()),
            ..AgentSession::default()
        },
    );
    sessions.insert(
        "cli-keep".into(),
        AgentSession {
            status: AgentStatus::Working,
            source: Source::Cli,
            ..AgentSession::default()
        },
    );
    apply_agents(&ctx, &mut sessions, &[], &HashSet::new());
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
    apply_agents(&ctx, &mut sessions, &rows, &desktop_ids);
    assert_eq!(
        sessions.keys().map(String::as_str).collect::<Vec<_>>(),
        ["conversation", "sdk", "terminal"]
    );
    assert_eq!(sessions["conversation"].source, Source::Desktop);

    // The next refresh, after the helper exits, must keep the same rows.
    rows.retain(|row| row["sessionId"] != "temporary-query");
    apply_agents(&ctx, &mut sessions, &rows, &desktop_ids);
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
    apply_agents(&ctx, &mut sessions, &rows, &HashSet::new());
    assert_eq!(sessions.len(), 4);
}
