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
        paseo_agent_id: None,
        paseo_host_changed_ms: None,
        exited: false,
        title: None,
        pane_pid: None,
        front: false,
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
