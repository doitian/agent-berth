use super::*;
use crate::status::AgentStatus;
use crate::store::SessionKind;

fn listed() -> ListedSession {
    ListedSession {
        provider: "claude".into(),
        session_id: "abc".into(),
        status: AgentStatus::Running,
        source: Source::Cli,
        cwd: None,
        cmdline: Vec::new(),
        pid: Some(1),
        created_ms: 0,
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        transcript_path: None,
        exited: false,
        title: None,
        pane_pid: None,
        front: false,
        tabbed: false,
        memo: None,
        turns_started: 0,
        turns_ended: 0,
        delegator: None,
        hidden: false,
    }
}

fn pane(id: &str, pid: u32, path: &str) -> Pane {
    Pane {
        id: id.into(),
        pid,
        session: "project".into(),
        window: "0".into(),
        window_name: "shell".into(),
        path: path.into(),
    }
}

#[test]
fn pane_matches_by_pid_in_ancestor_chain() {
    let panes = [pane("%1", 10, "/a"), pane("%2", 20, "/b")];
    assert_eq!(pane_for_pid(&panes, &[99, 20, 5]).unwrap().id, "%2");
    assert!(pane_for_pid(&panes, &[99, 98]).is_none());
}

#[test]
fn same_path_folds_separators_and_trailing_slashes() {
    assert!(same_path("C:\\work\\proj", "C:/work/proj"));
    assert!(same_path("/work/proj", "/work/proj/"));
    assert!(!same_path("C:\\work\\proj", "C:\\work\\other"));
}

#[cfg(windows)]
#[test]
fn same_path_folds_case_on_windows() {
    assert!(same_path("C:\\Work\\Proj", "c:/work/PROJ"));
}

#[test]
fn client_binary_uses_the_cmdline_executable_name() {
    let mut session = listed();
    assert_eq!(client_binary(&session), None);
    session.cmdline = vec!["opencode".into(), "--session".into(), "x".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode"));
    session.cmdline = vec!["/usr/local/bin/opencode".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode"));
}

#[cfg(windows)]
#[test]
fn client_binary_strips_windows_directories() {
    let mut session = listed();
    session.cmdline = vec!["C:\\tools\\opencode.exe".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode.exe"));
}

fn plugin(pid: u32, cmdline: Vec<String>) -> ListedSession {
    let mut session = listed();
    session.provider = "opencode".into();
    session.kind = SessionKind::Plugin;
    session.pid = Some(pid);
    session.cwd = Some("/work".into());
    session.cmdline = cmdline;
    session
}

#[test]
fn closed_tab_is_never_located_through_the_shared_server() {
    let me = std::process::id();
    let panes = [pane("%1", me, "/work")];
    // The server descends from the pane of the TUI that started it.
    let mut session = plugin(me, Vec::new());
    assert!(resolve_session_pane(&panes, &session, 0).is_some());
    session.tabbed = true;
    assert!(resolve_session_pane(&panes, &session, 0).is_none());
    assert!(running_pane(&session, &panes, 0).is_none());

    session.pane_pid = Some(me);
    session.front = true;
    let resolved = resolve_session_pane(&panes, &session, 0).unwrap();
    assert_eq!(resolved.pane.id, "%1");
    assert!(resolved.own_content);
}

#[cfg(target_os = "linux")]
#[test]
fn ended_plugin_session_has_no_pane() {
    let me = std::process::id();
    let comm = std::fs::read_to_string("/proc/self/comm").unwrap();
    let panes = [pane("%1", me, "/work")];
    // The pane runs the agent's binary in the session's directory, so only
    // the ended process keeps the pane from being attributed to it.
    let ended = plugin(u32::MAX, vec![comm.trim().into()]);
    assert!(resolve_session_pane(&panes, &ended, 0).is_some());
    assert!(running_pane(&ended, &panes, 0).is_none());
}
