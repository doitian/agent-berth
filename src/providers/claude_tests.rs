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
    apply_agents(&mut sessions, &[], &HashSet::new());
    assert!(sessions.contains_key("cli-keep"));
    assert!(!sessions.contains_key("probe-manual-001"));
}
