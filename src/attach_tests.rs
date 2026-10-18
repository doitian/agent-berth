use super::*;
use crate::status::AgentStatus;
use crate::status::Source;
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
        paseo_agent_id: None,
        paseo_host_changed_ms: None,
        exited: false,
        title: title.map(str::to_string),
        pane_pid: None,
        front: false,
        tabbed: false,
        memo: None,
        turns_started: 0,
        turns_ended: 0,
        delegator: None,
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
    let session = listed(Some(&cwd), Some("Fix attach"));
    let id = session.id();
    let candidate = Candidate::new(pane("%1", 1, &cwd), session);
    assert_eq!(
        candidate.line(),
        format!("%1\t{id}\tclaude\trunning\tFix attach\tproject\tmain\tabc")
    );
}

#[test]
fn line_shows_memo_but_keeps_title_searchable() {
    let root = repo("project", "main");
    let project = root.path().join("project");
    let cwd = project.display().to_string();
    let mut session = listed(Some(&cwd), Some("Fix attach"));
    session.memo = Some("my note".into());
    let candidate = Candidate::new(pane("%1", 1, &cwd), session);
    let line = candidate.line();
    let fields: Vec<_> = line.split('\t').collect();
    assert_eq!(fields[4], "my note (Fix attach)");
    assert_eq!(fields.len(), 8);
}

#[test]
fn line_falls_back_to_pane_path_and_dashes() {
    let root = repo("project", "topic");
    let project = root.path().join("project");
    let cwd = project.display().to_string();
    let session = listed(None, None);
    let id = session.id();
    let candidate = Candidate::new(pane("%2", 1, &cwd), session);
    assert_eq!(
        candidate.line(),
        format!("%2\t{id}\tclaude\trunning\t-\tproject\ttopic\tabc")
    );
}

#[test]
fn missing_branch_is_dashed() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().display().to_string();
    let candidate = Candidate::new(pane("%3", 1, &cwd), listed(Some(&cwd), Some("Task")));
    let line = candidate.line();
    let fields: Vec<_> = line.split('\t').collect();
    assert_eq!(fields[6], "-");
}

#[test]
fn direct_matches_one_candidate_by_id() {
    let mut first = listed(Some("/a"), Some("A"));
    first.session_id = "one".into();
    let second = listed(Some("/b"), Some("B"));
    let first_id = first.id();
    let candidates = vec![
        Candidate::new(pane("%1", 1, "/a"), first),
        Candidate::new(pane("%2", 2, "/b"), second),
    ];
    assert_eq!(direct(&candidates, Some(&first_id)).unwrap().pane.id, "%1");
    assert!(direct(&candidates, Some(&first_id.to_uppercase())).is_some());
    assert!(direct(&candidates, Some("missing")).is_none());
    assert!(direct(&candidates, None).is_none());
}

#[test]
fn only_tmux_cli_sessions_are_candidates() {
    let me = std::process::id();
    let panes = [pane("%1", me, "/work")];
    let mut native = listed(Some("/work"), Some("Native"));
    native.pid = Some(me);
    let mut paseo = native.clone();
    paseo.session_id = "paseo".into();
    paseo.paseo_agent_id = Some("paseo-agent".into());
    let mut desktop = native.clone();
    desktop.session_id = "desktop".into();
    desktop.source = Source::Desktop;

    let candidates = collect(&[paseo.clone(), desktop.clone(), native], &panes);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].session.session_id, "abc");
    assert!(collect(&[paseo, desktop], &panes).is_empty());
    assert!(!fzf_args(None, true).iter().any(|arg| arg.contains("paseo")));
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
