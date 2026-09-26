use super::*;
use crate::status::AgentStatus;
use crate::store::SessionKind;

fn listed(cwd: Option<&str>, title: Option<&str>) -> ListedSession {
    ListedSession {
        provider: "claude".into(),
        session_id: "abc".into(),
        status: AgentStatus::Running,
        source: Source::Cli,
        cwd: cwd.map(str::to_string),
        cmdline: Vec::new(),
        pid: Some(1),
        created_ms: 0,
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        transcript_path: None,
        exited: false,
        title: title.map(str::to_string),
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

fn repo(name: &str, branch: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join(name);
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(
        project.join(".git/HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
    root
}

#[test]
fn line_exposes_title_folder_and_branch() {
    let root = repo("project", "main");
    let project = root.path().join("project");
    let cwd = project.display().to_string();
    let candidate = Candidate::new(pane("%1", 1, &cwd), listed(Some(&cwd), Some("Fix attach")));
    assert_eq!(
        candidate.line(),
        "%1\tclaude\trunning\tFix attach\tproject\tmain\tabc"
    );
}

#[test]
fn line_falls_back_to_pane_path_and_dashes() {
    let root = repo("project", "topic");
    let project = root.path().join("project");
    let cwd = project.display().to_string();
    let candidate = Candidate::new(pane("%2", 1, &cwd), listed(None, None));
    assert_eq!(
        candidate.line(),
        "%2\tclaude\trunning\t-\tproject\ttopic\tabc"
    );
}

#[test]
fn missing_branch_is_dashed() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().display().to_string();
    let candidate = Candidate::new(pane("%3", 1, &cwd), listed(Some(&cwd), Some("Task")));
    let line = candidate.line();
    let fields: Vec<_> = line.split('\t').collect();
    assert_eq!(fields[5], "-");
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
    let mut session = listed(None, None);
    assert_eq!(client_binary(&session), None);
    session.cmdline = vec!["opencode".into(), "--session".into(), "x".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode"));
    session.cmdline = vec!["/usr/local/bin/opencode".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode"));
}

#[cfg(windows)]
#[test]
fn client_binary_strips_windows_directories() {
    let mut session = listed(None, None);
    session.cmdline = vec!["C:\\tools\\opencode.exe".into()];
    assert_eq!(client_binary(&session).as_deref(), Some("opencode.exe"));
}

#[test]
fn fzf_never_auto_selects() {
    let args = fzf_args(Some("query"), true);
    assert!(
        !args.iter().any(|arg| arg == "-1" || arg == "-0"),
        "{args:?}"
    );
    assert!(
        args.windows(2).any(|pair| pair == ["-q", "query"]),
        "{args:?}"
    );
}

#[test]
fn fzf_hides_preview_by_default() {
    assert!(
        fzf_args(None, false)
            .iter()
            .any(|arg| arg == "up:80%:hidden")
    );
    assert!(fzf_args(None, true).iter().any(|arg| arg == "up:80%"));
}

#[test]
#[cfg(windows)]
fn fzf_uses_powershell_for_preview() {
    assert!(
        fzf_args(None, true)
            .windows(2)
            .any(|pair| pair == ["--with-shell", tmux::PREVIEW_SHELL])
    );
}
