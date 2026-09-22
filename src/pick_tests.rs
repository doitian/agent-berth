use super::*;
use crate::status::{AgentStatus, Source};
use crate::store::SessionKind;

fn session(provider: &str, title: &str, cwd: &str) -> ListedSession {
    ListedSession {
        provider: provider.into(),
        session_id: "session-1".into(),
        status: AgentStatus::Working,
        source: Source::Cli,
        cwd: Some(cwd.into()),
        cmdline: Vec::new(),
        pid: None,
        created_ms: 0,
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        transcript_path: None,
        exited: false,
        title: Some(title.into()),
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
fn filters_nothing_when_patterns_are_empty() {
    let cwd = std::env::current_dir().unwrap().display().to_string();
    let sessions = [session("claude", "Fix", &cwd)];
    assert_eq!(filter(&sessions, &[]).len(), 1);
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
