use super::*;

fn session(provider: &str, sid: &str, delegator: Option<&str>) -> ListedSession {
    let mut session: ListedSession = serde_json::from_value(serde_json::json!({
        "provider":provider, "session_id":sid, "status":"running", "source":"cli",
        "cwd":"/work", "cmdline":[provider], "pid":1, "created_ms":1,
        "last_report_ms":2, "kind":"hook"
    }))
    .unwrap();
    session.delegator = delegator.map(str::to_string);
    session
}

#[test]
fn stored_delegator_comes_from_the_callers_own_session() {
    let this = Identity {
        provider: "claude".into(),
        session_id: "child".into(),
    };
    let sessions = [
        session("codex", "child", Some("wrong1")),
        session("claude", "child", Some("boss12")),
    ];
    assert_eq!(stored_delegator(&sessions, &this).unwrap(), "boss12");
}

#[test]
fn missing_session_or_delegator_is_a_clear_error() {
    let this = Identity {
        provider: "claude".into(),
        session_id: "child".into(),
    };
    let error = stored_delegator(&[], &this).unwrap_err().to_string();
    assert!(error.contains("has not reported yet"), "{error}");
    let error = stored_delegator(&[session("claude", "child", None)], &this)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("was not started by `agent-berth new`"),
        "{error}"
    );
}
