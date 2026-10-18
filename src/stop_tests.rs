use super::*;

fn session(provider: &str, sid: &str, pid: u32) -> ListedSession {
    serde_json::from_value(serde_json::json!({
        "provider":provider, "session_id":sid, "status":"running", "source":"cli",
        "cwd":"/work", "cmdline":[provider], "pid":pid, "created_ms":1,
        "last_report_ms":2, "kind":"hook"
    }))
    .unwrap()
}

/// A plugin session hosted as a tab by the process `pane_pid`.
fn tab(sid: &str, pane_pid: u32, front: bool) -> ListedSession {
    let mut session = session("opencode", sid, u32::MAX);
    session.kind = crate::store::SessionKind::Plugin;
    session.pane_pid = Some(pane_pid);
    session.front = front;
    session.tabbed = true;
    session
}

fn pane(id: &str, pid: u32) -> Pane {
    Pane {
        id: id.into(),
        pid,
        session: "project".into(),
        window: "0".into(),
        window_name: "agent".into(),
        path: "/work".into(),
    }
}

fn error(result: Result<&Pane>) -> String {
    result
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn kills_the_pane_running_the_session_alone() {
    let me = std::process::id();
    let panes = [pane("%1", me), pane("%2", 1)];
    let own = session("claude", "s", me);
    let other = session("codex", "elsewhere", 1);
    let sessions = [own.clone(), other];
    assert_eq!(target(&own, &sessions, &panes, 0, false).unwrap().id, "%1");
}

#[test]
fn refuses_sessions_outside_a_pane_with_the_reason() {
    let me = std::process::id();
    let panes = [pane("%1", me)];
    let alone = |session: &ListedSession| {
        error(target(
            session,
            std::slice::from_ref(session),
            &panes,
            0,
            false,
        ))
    };
    assert!(alone(&session("claude", "s", u32::MAX)).contains("not showing"));
    let mut exited = session("claude", "s", me);
    exited.exited = true;
    assert!(alone(&exited).contains("already exited"));
    let mut paseo = session("claude", "s", me);
    paseo.paseo_agent_id = Some("agent".into());
    assert!(alone(&paseo).contains("Paseo"));
    let mut desktop = session("claude", "s", me);
    desktop.source = Source::Desktop;
    assert!(alone(&desktop).contains("desktop"));
}

#[test]
fn refuses_a_pane_hosting_other_sessions_unless_forced() {
    let me = std::process::id();
    let panes = [pane("%1", me)];
    let front = tab("front", me, true);
    let background = tab("background", me, false);
    let sessions = [front.clone(), background.clone()];
    let refusal = error(target(&front, &sessions, &panes, 0, false));
    assert!(
        refusal.contains(&format!("opencode {}", background.id())),
        "{refusal}"
    );
    assert!(refusal.contains("--force"), "{refusal}");
    assert_eq!(target(&front, &sessions, &panes, 0, true).unwrap().id, "%1");
    // A background tab is not what the pane shows.
    assert!(error(target(&background, &sessions, &panes, 0, true)).contains("not showing"));
}

#[test]
fn a_sessions_own_subagents_do_not_block_it() {
    let me = std::process::id();
    let panes = [pane("%1", me)];
    let parent = session("claude", "s", me);
    let mut child = session("claude", "s:agent", me);
    child.parent_id = Some("s".into());
    let sessions = [parent.clone(), child];
    assert_eq!(
        target(&parent, &sessions, &panes, 0, false).unwrap().id,
        "%1"
    );
}
