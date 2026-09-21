use super::*;
use crate::store::SessionKind;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn listed(provider: &str, session_id: &str, title: Option<&str>) -> ListedSession {
    ListedSession {
        provider: provider.into(),
        session_id: session_id.into(),
        status: AgentStatus::Working,
        source: Source::Cli,
        cwd: Some("/tmp/project".into()),
        cmdline: Vec::new(),
        pid: Some(1),
        created_ms: 0,
        last_report_ms: 0,
        kind: SessionKind::Hook,
        parent_id: None,
        exited: false,
        title: title.map(str::to_string),
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn char_key(ch: char) -> KeyEvent {
    key(KeyCode::Char(ch))
}

fn shift_char_key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::SHIFT)
}

fn app_with_sessions() -> App {
    let mut app = App::new();
    app.sessions = vec![
        listed("claude", "abc", Some("Fix attach")),
        listed("codex", "def", Some("Write docs")),
        listed("pi", "ghi", None),
    ];
    app
}

#[test]
fn details_color_status_and_branch() {
    for status in [
        AgentStatus::Working,
        AgentStatus::Waiting,
        AgentStatus::Idle,
        AgentStatus::Done,
    ] {
        let mut app = app_with_sessions();
        app.sessions[0].status = status;
        app.branch = Some("main".into());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| render_details(frame, &app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(10, 3)].symbol(), &status.as_str()[..1]);
        assert_eq!(buffer[(10, 3)].fg, status_style(status).fg.unwrap());
        assert_eq!(buffer[(10, 7)].symbol(), "m");
        assert_eq!(buffer[(10, 7)].fg, latte::GREEN);
    }
}

#[test]
fn details_color_git_status_symbols_independently() {
    let mut app = app_with_sessions();
    app.git = Some(git::RepoInfo {
        branch: "main".into(),
        status: git::RepoStatus {
            conflicted: true,
            stashed: true,
            deleted: true,
            renamed: true,
            modified: true,
            staged: true,
            untracked: true,
            tracking: Some((2, 1)),
        },
        github: None,
    });
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| render_details(frame, &app, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    for (offset, symbol, color, bold) in [
        (0, "m", latte::GREEN, false),
        (5, "[", latte::SURFACE2, true),
        (6, "!", latte::RED, false),
        (7, "$", latte::RED, true),
        (8, "✘", latte::RED, true),
        (9, "»", latte::YELLOW, true),
        (10, "!", latte::YELLOW, true),
        (11, "+", latte::GREEN, true),
        (12, "?", latte::RED, true),
        (13, "↕", latte::RED, true),
        (14, "]", latte::SURFACE2, true),
    ] {
        let cell = &buffer[(10 + offset, 7)];
        assert_eq!(cell.symbol(), symbol);
        assert_eq!(cell.fg, color, "{symbol}");
        assert_eq!(cell.modifier.contains(Modifier::BOLD), bold, "{symbol}");
    }
}

#[test]
fn branch_tracking_symbols_match_starship_overrides() {
    for (tracking, expected) in [
        (None, "main"),
        (Some((0, 0)), "main [≡]"),
        (Some((2, 0)), "main [↑]"),
        (Some((0, 1)), "main [↓]"),
        (Some((2, 1)), "main [↕]"),
    ] {
        let status = git::RepoStatus {
            tracking,
            ..git::RepoStatus::default()
        };
        let spans = branch_spans("main", Some(&status));
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, expected);
    }
    let spans = branch_spans("feature/!+", None);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content, "feature/!+");
}

#[test]
fn details_show_combined_branch_status_with_branch_only_fallback() {
    let mut app = app_with_sessions();
    app.branch = Some("main".into());
    app.git = Some(git::RepoInfo {
        branch: "main".into(),
        status: git::RepoStatus {
            modified: true,
            tracking: Some((2, 1)),
            ..git::RepoStatus::default()
        },
        github: Some("owner/repo".into()),
    });
    for expected in ["branch   main [!↕]", "branch   main", "branch   -"] {
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_details(frame, &app, frame.area()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains(expected), "{text}");
        assert_eq!(
            text.matches("main").count(),
            usize::from(app.branch.is_some())
        );
        assert!(!text.contains("git      "), "{text}");
        if app.git.is_some() {
            assert!(text.contains("repo     owner/repo"), "{text}");
            app.git = None;
        } else {
            app.branch = None;
        }
    }
}

#[test]
fn starts_on_active_view() {
    let app = App::new();
    assert_eq!(app.view, View::Active);
    assert_eq!(app.sort, Sort::Created);
    assert_eq!(app.input, Input::Normal);
    assert!(!app.idle_enabled);
}

#[test]
fn jk_navigates_and_clamps() {
    let mut app = app_with_sessions();
    assert_eq!(app.selected, 0);
    app.handle_key(char_key('j'));
    assert_eq!(app.selected, 1);
    app.handle_key(char_key('k'));
    assert_eq!(app.selected, 0);
    app.handle_key(char_key('k'));
    assert_eq!(app.selected, 0);
    app.handle_key(char_key('j'));
    app.handle_key(char_key('j'));
    app.handle_key(char_key('j'));
    assert_eq!(app.selected, 2);
}

#[test]
fn ga_gr_switch_views() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('g'));
    assert_eq!(app.pending, Some(Pending::G));
    app.handle_key(char_key('r'));
    assert_eq!(app.view, View::Resumable);
    assert!(app.dirty);
    app.dirty = false;
    app.handle_key(char_key('g'));
    app.handle_key(char_key('a'));
    assert_eq!(app.view, View::Active);
    assert!(app.dirty);
}

#[test]
fn pending_g_is_cancelled_by_other_keys() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('g'));
    app.handle_key(char_key('x'));
    assert_eq!(app.pending, None);
    assert_eq!(app.view, View::Active);
}

#[test]
fn space_gg_opens_lazygit_in_session_cwd() {
    let mut app = app_with_sessions();
    app.handle_key(char_key(' '));
    assert_eq!(app.pending, Some(Pending::Space));
    app.handle_key(char_key('g'));
    assert_eq!(app.pending, Some(Pending::SpaceG));
    match app.handle_key(char_key('g')) {
        Effect::Lazygit(cwd) => assert_eq!(cwd, PathBuf::from("/tmp/project")),
        _ => panic!("expected lazygit effect"),
    }
    assert_eq!(app.pending, None);
}

#[test]
fn space_g_followed_by_other_key_cancels() {
    let mut app = app_with_sessions();
    app.handle_key(char_key(' '));
    app.handle_key(char_key('g'));
    assert!(matches!(app.handle_key(char_key('x')), Effect::None));
    assert_eq!(app.pending, None);
}

#[test]
fn space_gg_without_cwd_shows_message() {
    let mut app = app_with_sessions();
    app.sessions[0].cwd = None;
    app.handle_key(char_key(' '));
    app.handle_key(char_key('g'));
    assert!(matches!(app.handle_key(char_key('g')), Effect::None));
    assert!(app.message.is_some());
}

#[test]
fn br_browses_repo_and_bp_browses_pr() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('b'));
    assert_eq!(app.pending, Some(Pending::B));
    match app.handle_key(char_key('r')) {
        Effect::Browse(cwd, Browse::Repo) => assert_eq!(cwd, PathBuf::from("/tmp/project")),
        _ => panic!("expected repo browse effect"),
    }
    app.handle_key(char_key('b'));
    match app.handle_key(char_key('p')) {
        Effect::Browse(cwd, Browse::Pr) => assert_eq!(cwd, PathBuf::from("/tmp/project")),
        _ => panic!("expected pr browse effect"),
    }
    assert_eq!(app.pending, None);
}

#[test]
fn b_followed_by_other_key_cancels() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('b'));
    assert!(matches!(app.handle_key(char_key('x')), Effect::None));
    assert_eq!(app.pending, None);
}

#[test]
fn slash_opens_filter_and_narrows_list() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('/'));
    assert_eq!(app.input, Input::Filter);
    for ch in "codex".chars() {
        app.handle_key(char_key(ch));
    }
    assert_eq!(app.filter, "codex");
    let filtered = app.filtered();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].session_id, "def");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.input, Input::Normal);
    assert_eq!(app.filter, "codex");
}

#[test]
fn esc_in_filter_clears_and_exits() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('/'));
    app.handle_key(char_key('x'));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.input, Input::Normal);
    assert!(app.filter.is_empty());
    assert_eq!(app.filtered().len(), 3);
}

#[test]
fn esc_clears_filter_before_quitting() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('/'));
    app.handle_key(char_key('x'));
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.handle_key(key(KeyCode::Esc)), Effect::None));
    assert!(app.filter.is_empty());
    assert!(matches!(app.handle_key(key(KeyCode::Esc)), Effect::Quit));
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut app = app_with_sessions();
    assert!(matches!(app.handle_key(char_key('q')), Effect::Quit));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(matches!(app.handle_key(ctrl_c), Effect::Quit));
}

#[test]
fn idle_toggle_requires_resumable_view() {
    let mut app = app_with_sessions();
    app.handle_key(shift_char_key('I'));
    assert_eq!(app.input, Input::Normal);
    assert!(!app.idle_enabled);
    assert!(app.message.is_some());
}

#[test]
fn idle_toggle_prefills_current_duration() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.handle_key(shift_char_key('I'));
    assert_eq!(app.input, Input::Idle);
    assert_eq!(app.idle_input, "20m");
}

#[test]
fn idle_enter_applies_and_marks_dirty() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.handle_key(shift_char_key('I'));
    app.idle_input = "1h".into();
    app.handle_key(key(KeyCode::Enter));
    assert!(app.idle_enabled);
    assert_eq!(app.idle, Duration::from_secs(3600));
    assert_eq!(app.input, Input::Normal);
    assert!(app.dirty);
}

#[test]
fn idle_enter_rejects_invalid_duration() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.handle_key(shift_char_key('I'));
    app.idle_input = "bogus".into();
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.idle_enabled);
    assert_eq!(app.input, Input::Idle);
    assert!(app.message.is_some());
}

#[test]
fn idle_esc_cancels_without_enabling() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.handle_key(shift_char_key('I'));
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.input, Input::Normal);
    assert!(!app.idle_enabled);
}

#[test]
fn idle_toggle_off_when_enabled() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.idle_enabled = true;
    app.handle_key(shift_char_key('I'));
    assert!(!app.idle_enabled);
    assert!(app.dirty);
    assert_eq!(app.input, Input::Normal);
}

#[test]
fn equals_requires_a_preview() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('='));
    assert!(!app.preview_maximized);
    assert!(app.message.is_some());
}

#[test]
fn attach_without_pane_shows_message() {
    let mut app = app_with_sessions();
    assert!(matches!(app.handle_key(char_key('a')), Effect::None));
    assert!(app.message.is_some());
}

#[test]
fn resume_requires_resumable_view() {
    let mut app = app_with_sessions();
    assert!(matches!(app.handle_key(char_key('r')), Effect::None));
    assert!(app.message.is_some());
}

#[test]
fn resume_returns_selected_session() {
    let mut app = app_with_sessions();
    app.set_view(View::Resumable);
    app.handle_key(char_key('j'));
    match app.handle_key(char_key('r')) {
        Effect::Resume(session) => assert_eq!(session.session_id, "def"),
        _ => panic!("expected resume effect"),
    }
}

#[test]
fn d_starts_confirmation_without_removing() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('j'));
    assert!(matches!(app.handle_key(char_key('d')), Effect::None));
    assert_eq!(app.input, Input::Confirm);
    assert_eq!(app.confirm_session.as_ref().unwrap().session_id, "def");
}

#[test]
fn confirm_yes_removes_the_prompted_session() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('j'));
    app.handle_key(char_key('d'));
    match app.handle_key(char_key('y')) {
        Effect::Remove(session) => assert_eq!(session.session_id, "def"),
        _ => panic!("expected remove effect"),
    }
    assert_eq!(app.input, Input::Normal);
    assert!(app.confirm_session.is_none());
}

#[test]
fn confirmation_target_is_fixed_at_prompt() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('j'));
    app.handle_key(char_key('d'));
    app.sessions.reverse();
    match app.handle_key(char_key('y')) {
        Effect::Remove(session) => assert_eq!(session.session_id, "def"),
        _ => panic!("expected remove effect"),
    }
}

#[test]
fn confirm_cancel_keys_keep_the_session() {
    for cancel in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('N')] {
        let mut app = app_with_sessions();
        app.handle_key(char_key('d'));
        assert!(matches!(app.handle_key(key(cancel)), Effect::None));
        assert_eq!(app.input, Input::Normal);
        assert!(app.confirm_session.is_none());
    }
}

#[test]
fn d_without_sessions_shows_message() {
    let mut app = App::new();
    assert!(matches!(app.handle_key(char_key('d')), Effect::None));
    assert_eq!(app.input, Input::Normal);
    assert!(app.message.is_some());
}

#[test]
fn sort_sessions_orders_newest_first_with_stable_ties() {
    let mut a = listed("claude", "a", None);
    a.created_ms = 100;
    let mut b = listed("codex", "b", None);
    b.created_ms = 300;
    let mut c = listed("pi", "c", None);
    c.created_ms = 200;
    let mut d = listed("claude", "d", None);
    d.created_ms = 200;
    let mut sessions = vec![a, b, c, d];
    sort_sessions(&mut sessions, Sort::Created);
    let order: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(order, ["b", "d", "c", "a"]);
}

#[test]
fn sort_keys_reorder_sessions_and_preserve_selection() {
    for (key, sort, expected) in [
        ('t', Sort::Created, ["b", "d", "c", "a"]),
        ('r', Sort::Activity, ["c", "b", "d", "a"]),
        ('a', Sort::Provider, ["d", "a", "b", "c"]),
        ('s', Sort::Status, ["b", "d", "a", "c"]),
        ('d', Sort::Directory, ["d", "c", "a", "b"]),
    ] {
        let mut app = App::new();
        for (provider, id, created, activity, status, cwd) in [
            ("claude", "a", 100, 100, AgentStatus::Waiting, Some("/z")),
            ("codex", "b", 300, 200, AgentStatus::Done, None),
            ("pi", "c", 200, 300, AgentStatus::Working, Some("/a")),
            ("claude", "d", 200, 100, AgentStatus::Idle, Some("/a")),
        ] {
            let mut session = listed(provider, id, None);
            session.created_ms = created;
            session.last_report_ms = activity;
            session.status = status;
            session.cwd = cwd.map(str::to_string);
            app.sessions.push(session);
        }
        app.selected = 2;
        app.handle_key(char_key('s'));
        assert_eq!(app.pending, Some(Pending::S));
        assert!(matches!(app.handle_key(char_key(key)), Effect::None));
        assert_eq!(app.pending, None);
        assert_eq!(app.input, Input::Normal);
        assert_eq!(app.sort, sort);
        let order: Vec<&str> = app.sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(order, expected, "sort key: s{key}");
        assert_eq!(app.selected_session().unwrap().session_id, "c");
    }
}

#[test]
fn sort_prefix_can_be_cancelled_and_filter_input_stays_literal() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('s'));
    assert!(matches!(app.handle_key(key(KeyCode::Esc)), Effect::None));
    assert_eq!(app.pending, None);
    assert_eq!(app.sort, Sort::Created);
    app.handle_key(char_key('/'));
    app.handle_key(char_key('s'));
    app.handle_key(char_key('r'));
    assert_eq!(app.filter, "sr");
    assert_eq!(app.sort, Sort::Created);
}

#[test]
fn refresh_uses_current_sort_after_mode_change_and_preserves_filtered_selection() {
    let mut app = app_with_sessions();
    for session in &mut app.sessions {
        session.cwd = None;
    }
    app.filter = "docs".into();
    app.refresh_generation = 1;
    let mut sessions = app.sessions.clone();
    sessions[2].last_report_ms = 300;
    sessions[1].last_report_ms = 200;
    app.handle_key(char_key('s'));
    app.handle_key(char_key('r'));
    app.set_view(View::Resumable);
    app.handle_fetched(Fetched::Refresh {
        generation: 1,
        result: Ok((sessions, Vec::new())),
    });
    assert_eq!(app.sort, Sort::Activity);
    assert_eq!(app.sessions[0].session_id, "ghi");
    assert_eq!(app.selected_session().unwrap().session_id, "def");
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(char_key('s'));
    app.handle_key(char_key('t'));
    assert_eq!(app.sort, Sort::Created);
    assert_eq!(app.sessions[0].session_id, "abc");
}

#[test]
fn restore_selection_follows_session_across_reorder() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('j'));
    app.handle_key(char_key('j'));
    assert_eq!(app.selected, 2);
    let keep = app
        .selected_session()
        .map(|s| (s.provider.clone(), s.session_id.clone()));
    app.sessions.reverse();
    app.restore_selection(keep);
    assert_eq!(app.selected, 0);
    assert_eq!(app.selected_session().unwrap().session_id, "ghi");
}

#[test]
fn restore_selection_clamps_when_session_is_gone() {
    let mut app = app_with_sessions();
    app.handle_key(char_key('j'));
    app.handle_key(char_key('j'));
    let keep = app
        .selected_session()
        .map(|s| (s.provider.clone(), s.session_id.clone()));
    app.sessions.pop();
    app.restore_selection(keep);
    assert_eq!(app.selected, 1);
    assert_eq!(app.selected_session().unwrap().session_id, "def");
}

#[test]
fn fetched_results_are_cached_and_applied() {
    let mut app = App::new();
    app.git_cwd = Some("/tmp/project".into());
    app.git_generation = 1;
    app.handle_fetched(Fetched::Git {
        generation: 1,
        cwd: "/tmp/project".into(),
        branch: Some("main".into()),
        info: None,
    });
    assert_eq!(app.branch.as_deref(), Some("main"));
    assert_eq!(
        app.gits
            .get("/tmp/project")
            .and_then(|cached| cached.branch.as_deref()),
        Some("main")
    );
}

#[test]
fn fetched_refresh_preserves_selection_after_navigation_and_reorder() {
    let mut app = app_with_sessions();
    for session in &mut app.sessions {
        session.cwd = None;
    }
    app.refresh_generation = 1;
    let mut sessions = app.sessions.clone();
    sessions[2].created_ms = 100;
    sessions.reverse();
    app.handle_key(char_key('j'));
    app.handle_key(char_key('j'));
    app.handle_fetched(Fetched::Refresh {
        generation: 1,
        result: Ok((sessions, Vec::new())),
    });
    assert_eq!(app.selected, 0);
    assert_eq!(app.selected_session().unwrap().session_id, "ghi");
}

#[test]
fn stale_fetches_are_cached_but_not_applied() {
    let mut app = App::new();
    app.git_cwd = Some("/tmp/project".into());
    app.git_generation = 2;
    app.handle_fetched(Fetched::Git {
        generation: 1,
        cwd: "/tmp/project".into(),
        branch: Some("main".into()),
        info: None,
    });
    assert_eq!(app.branch, None);
    assert!(app.gits.contains_key("/tmp/project"));
}

#[test]
fn reselecting_restores_cached_git_details() {
    let mut app = app_with_sessions();
    app.sessions[1].cwd = Some("/tmp/other".into());
    for (cwd, branch) in [("/tmp/project", "main"), ("/tmp/other", "dev")] {
        app.gits.insert(
            cwd.into(),
            CachedGit {
                branch: Some(branch.into()),
                info: None,
            },
        );
    }
    app.handle_key(char_key('j'));
    assert_eq!(app.branch.as_deref(), Some("dev"));
    app.handle_key(char_key('k'));
    assert_eq!(app.branch.as_deref(), Some("main"));
}

fn preview_pane(id: &str) -> Pane {
    Pane {
        id: id.into(),
        pid: 1,
        session: "test".into(),
        window: "0".into(),
        window_name: "agent".into(),
        path: "/tmp/project".into(),
    }
}

fn start_preview(app: &mut App) -> (String, u64) {
    let now = app.preview_pending_since.unwrap() + PREVIEW_DEBOUNCE;
    match app.next_preview_fetch(now) {
        Some(Fetch::Preview {
            pane_id,
            generation,
        }) => (pane_id, generation),
        _ => panic!("expected preview fetch"),
    }
}

fn finish_preview(app: &mut App, request: (String, u64), content: Option<&str>) {
    app.handle_fetched(Fetched::Preview {
        pane_id: request.0,
        generation: request.1,
        content: content.map(str::to_string),
    });
}

#[test]
fn preview_navigation_is_debounced_to_latest_selection() {
    let mut app = App::new();
    app.select_preview(Some(preview_pane("%1")));
    let since = app.preview_pending_since.unwrap();
    assert!(app.next_preview_fetch(since).is_none());
    app.select_preview(Some(preview_pane("%2")));
    let since = app.preview_pending_since.unwrap();
    assert!(
        app.next_preview_fetch(since + PREVIEW_DEBOUNCE - Duration::from_millis(1))
            .is_none()
    );
    assert_eq!(start_preview(&mut app).0, "%2");
    assert!(app.preview_pending_since.is_none());
}

#[test]
fn preview_serializes_fetches_and_preserves_latest_pending_selection() {
    let mut app = App::new();
    app.select_preview(Some(preview_pane("%1")));
    let first = start_preview(&mut app);
    app.select_preview(Some(preview_pane("%2")));
    app.select_preview(Some(preview_pane("%3")));
    let since = app.preview_pending_since.unwrap();
    assert!(app.next_preview_fetch(since + REFRESH).is_none());
    assert_eq!(app.preview_pending_since, Some(since));
    app.needs_redraw = false;
    finish_preview(&mut app, first, Some("first pane"));
    assert!(!app.needs_redraw);
    assert!(app.preview.is_none());
    assert_eq!(
        app.previews.get("%1").map(String::as_str),
        Some("first pane")
    );
    assert_eq!(start_preview(&mut app).0, "%3");
}

#[test]
fn preview_reselection_keeps_cache_until_changed_content_arrives() {
    let mut app = App::new();
    app.select_preview(Some(preview_pane("%1")));
    let first = start_preview(&mut app);
    finish_preview(&mut app, first, Some("cached"));
    app.select_preview(Some(preview_pane("%2")));
    app.select_preview(Some(preview_pane("%1")));
    assert_eq!(app.preview.as_deref(), Some("cached"));
    let second = start_preview(&mut app);
    assert_eq!(app.preview.as_deref(), Some("cached"));
    app.needs_redraw = false;
    finish_preview(&mut app, second, Some("cached"));
    assert!(!app.needs_redraw);
    app.preview_pending_since = Some(Instant::now());
    let third = start_preview(&mut app);
    finish_preview(&mut app, third, Some("updated"));
    assert!(app.needs_redraw);
    assert_eq!(app.preview.as_deref(), Some("updated"));
}

#[test]
fn failed_preview_preserves_cache_and_allows_retry() {
    let mut app = App::new();
    app.previews.insert("%1".into(), "cached".into());
    app.select_preview(Some(preview_pane("%1")));
    let request = start_preview(&mut app);
    app.needs_redraw = false;
    finish_preview(&mut app, request, None);
    assert!(app.preview_in_flight.is_none());
    assert_eq!(app.preview.as_deref(), Some("cached"));
    assert!(!app.needs_redraw);
    app.preview_pending_since = Some(Instant::now());
    assert_eq!(start_preview(&mut app).0, "%1");
}

#[test]
fn outdated_preview_completion_cannot_release_current_fetch() {
    let mut app = App::new();
    app.select_preview(Some(preview_pane("%1")));
    let first = start_preview(&mut app);
    finish_preview(&mut app, first.clone(), Some("first"));
    app.preview_pending_since = Some(Instant::now());
    let second = start_preview(&mut app);
    app.needs_redraw = false;
    finish_preview(&mut app, first, Some("outdated"));
    assert_eq!(app.preview_in_flight, Some(second.1));
    assert_eq!(app.preview.as_deref(), Some("first"));
    assert!(!app.needs_redraw);
}

#[test]
fn deselecting_preview_clears_pending_work_and_ignores_completion() {
    let mut app = App::new();
    app.select_preview(Some(preview_pane("%1")));
    let request = start_preview(&mut app);
    app.select_preview(None);
    app.needs_redraw = false;
    finish_preview(&mut app, request, Some("old pane"));
    assert!(app.preview.is_none());
    assert!(app.preview_pending_since.is_none());
    assert!(app.preview_in_flight.is_none());
    assert!(!app.needs_redraw);
    assert!(app.next_preview_fetch(Instant::now() + REFRESH).is_none());
}

#[test]
fn logos_cover_known_providers() {
    for provider in ["claude", "codex", "grok", "opencode", "pi"] {
        assert_ne!(logo(provider), logo("unknown"), "{provider}");
    }
}
