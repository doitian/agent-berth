use super::*;
use crate::status::{AgentStatus, Source};
use crate::store::SessionKind;

fn session(provider: &str, title: &str, cwd: &str) -> ListedSession {
    ListedSession {
        provider: provider.into(),
        session_id: "session-1".into(),
        status: AgentStatus::Running,
        source: Source::Cli,
        cwd: Some(cwd.into()),
        cmdline: Vec::new(),
        pid: None,
        created_ms: 0,
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        transcript_path: None,
        paseo_agent_id: None,
        paseo_host_changed_ms: None,
        exited: false,
        title: Some(title.into()),
        pane_pid: None,
        front: false,
        tabbed: false,
        memo: None,
        turns_started: 0,
        turns_ended: 0,
        delegator: None,
    }
}

#[test]
fn filters_all_terms_case_insensitively() {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let sessions = [
        session("claude", "Fix the parser", &cwd),
        session("codex", "Unrelated work", &cwd),
    ];
    let matched = filter(&sessions, &["CLAUDE".into(), "parser".into()]);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].provider, "claude");
}

#[test]
fn single_by_id_takes_the_exact_id_before_matching_patterns() {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let target = session("claude", "Fix the parser", &cwd);
    let decoy = session("codex", &format!("Notes on {}", target.id()), &cwd);
    let sessions = [target.clone(), decoy];
    // As a pattern the id matches both, which would need the fzf picker.
    assert_eq!(filter(&sessions, &[target.id()]).len(), 2);
    let picked = single_by_id(&sessions, &target.id()).unwrap().unwrap();
    assert_eq!(picked.provider, "claude");
    let picked = single_by_id(&sessions, "PARSER").unwrap().unwrap();
    assert_eq!(picked.provider, "claude");
    assert!(single_by_id(&sessions, "nothing").is_err());
}

#[test]
fn filters_nothing_when_patterns_are_empty() {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let sessions = [session("claude", "Fix", &cwd)];
    assert_eq!(filter(&sessions, &[]).len(), 1);
}

#[test]
fn filters_match_memos_and_lines_keep_titles_searchable() {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let mut claude = session("claude", "Fix the parser", &cwd);
    claude.memo = Some("investigate flaky CI".into());
    let sessions = [claude, session("codex", "Unrelated work", &cwd)];
    let matched = filter(&sessions, &["flaky".into()]);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].provider, "claude");

    // The picker row shows the memo with the detected title appended, so an
    // fzf query seeded from patterns that matched the title still matches.
    let line = display_line(&matched[0]);
    assert!(
        line.contains("investigate flaky CI (Fix the parser)"),
        "{line}"
    );
    assert!(display_line(&sessions[1]).contains("Unrelated work"));
}

#[test]
fn multi_enables_fzf_multi_select() {
    let multi = fzf_args(&["fix".into()], true);
    assert!(multi.iter().any(|arg| arg == "--multi"), "{multi:?}");
    assert!(
        multi.windows(2).any(|pair| pair == ["-q", "fix"]),
        "{multi:?}"
    );

    let single = fzf_args(&[], false);
    assert!(single.iter().any(|arg| arg == "+m"), "{single:?}");
    assert!(!single.iter().any(|arg| arg == "-q"), "{single:?}");
}
