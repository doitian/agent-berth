use super::*;

fn session(source: &str, kind: &str, pid: u32) -> ListedSession {
    serde_json::from_value(serde_json::json!({
        "provider":"claude", "session_id":"s", "status":"running", "source":source,
        "cwd":"/work", "cmdline":["claude"], "pid":pid, "created_ms":1,
        "last_report_ms":2, "kind":kind
    }))
    .unwrap()
}

fn pane(pid: u32) -> Pane {
    Pane {
        id: "%3".into(),
        pid,
        session: "dev".into(),
        window: "1".into(),
        window_name: "claude".into(),
        path: "/work".into(),
    }
}

#[test]
fn json_names_the_pane_a_live_cli_session_runs_in() {
    let me = std::process::id();
    let values = json_values(&[session("cli", "hook", me)], &[pane(me)], 0).unwrap();
    assert_eq!(values[0]["tmux_pane"]["id"], "%3");
    assert_eq!(values[0]["tmux_pane"]["session"], "dev");
    assert_eq!(values[0]["tmux_pane"]["window_name"], "claude");
    assert_eq!(values[0]["id"], session("cli", "hook", me).id());
}

#[test]
fn json_omits_the_pane_for_desktop_and_ended_sessions() {
    let me = std::process::id();
    let mut paseo = session("cli", "hook", me);
    paseo.paseo_agent_id = Some("agent".into());
    // The pane still runs the client binary in the same directory, so only
    // the ended process keeps it from being attributed to this session.
    let ended = session("cli", "hook", u32::MAX);
    for session in [session("desktop", "hook", me), paseo, ended] {
        let values = json_values(&[session], &[pane(me)], 0).unwrap();
        assert!(values[0].get("tmux_pane").is_none(), "{}", values[0]);
    }
}
