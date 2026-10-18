use super::*;

fn env_of<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    }
}

#[test]
fn env_identity_uses_the_provider_session_variable() {
    let this = from_env(env_of(&[("PI_SESSION_ID", "abc123")])).unwrap();
    assert_eq!(this.provider, "pi");
    assert_eq!(this.session_id, "abc123");
    assert_eq!(
        providers::lookup("pi").unwrap().session_env(),
        Some("PI_SESSION_ID")
    );
}

#[test]
fn env_identity_reads_claude_code_session_id() {
    let this = from_env(env_of(&[(
        "CLAUDE_CODE_SESSION_ID",
        "83844ac0-e963-46b0-9c67-c1b43aacd29c",
    )]))
    .unwrap();
    assert_eq!(this.provider, "claude");
    assert_eq!(this.session_id, "83844ac0-e963-46b0-9c67-c1b43aacd29c");
    assert_eq!(
        providers::lookup("claude").unwrap().session_env(),
        Some("CLAUDE_CODE_SESSION_ID")
    );
}

#[test]
fn env_identity_ignores_missing_and_empty_variables() {
    assert!(from_env(env_of(&[])).is_none());
    assert!(from_env(env_of(&[("TMUX_PANE", "%7")])).is_none());
    assert!(from_env(env_of(&[("PI_SESSION_ID", "")])).is_none());
    assert!(from_env(env_of(&[("CLAUDE_CODE_SESSION_ID", "")])).is_none());
}

fn session(provider: &str, id: &str, pid: u32, created_ms: u64) -> ListedSession {
    serde_json::from_value(serde_json::json!({
        "provider": provider, "session_id": id, "status": "running", "source": "cli",
        "cwd": "/work", "cmdline": [provider], "pid": pid, "created_ms": created_ms,
        "last_report_ms": created_ms, "kind": "hook"
    }))
    .unwrap()
}

fn pane(id: &str, pid: u32) -> Pane {
    Pane {
        id: id.into(),
        pid,
        session: "dev".into(),
        window: "0".into(),
        window_name: "agent".into(),
        path: "/work".into(),
    }
}

#[test]
fn pane_identity_picks_only_the_session_the_pane_shows() {
    let me = std::process::id();
    let panes = [pane("%1", me), pane("%2", 1)];
    let sessions = [
        session("claude", "shown", me, 1),
        session("codex", "dead", 1, 2),
    ];
    let showing = in_pane(&sessions, &panes, "%1");
    assert_eq!(showing.len(), 1);
    assert_eq!(showing[0].provider, "claude");
    assert_eq!(showing[0].session_id, "shown");
    assert!(in_pane(&sessions, &panes, "%9").is_empty());
}

#[test]
fn pane_identity_reports_every_matching_session_newest_first() {
    let me = std::process::id();
    let panes = [pane("%1", me)];
    let sessions = [
        session("claude", "old", me, 1),
        session("claude", "new", me, 2),
    ];
    let showing = in_pane(&sessions, &panes, "%1");
    let ids: Vec<&str> = showing
        .iter()
        .map(|this| this.session_id.as_str())
        .collect();
    assert_eq!(ids, ["new", "old"]);
}
