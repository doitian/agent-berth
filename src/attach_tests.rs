use super::*;
use crate::status::AgentStatus;
use crate::store::SessionKind;

fn listed(cwd: Option<&str>, title: Option<&str>) -> ListedSession {
    ListedSession {
        provider: "claude".into(),
        session_id: "abc".into(),
        status: AgentStatus::Working,
        source: Source::Cli,
        cwd: cwd.map(str::to_string),
        cmdline: Vec::new(),
        pid: Some(1),
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        exited: false,
        title: title.map(str::to_string),
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
        "%1\tclaude\tworking\tFix attach\tproject\tmain\tabc"
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
        "%2\tclaude\tworking\t-\tproject\ttopic\tabc"
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
