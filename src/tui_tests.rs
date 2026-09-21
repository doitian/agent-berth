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
fn starts_on_active_view() {
    let app = App::new();
    assert_eq!(app.view, View::Active);
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
    assert!(app.pending_g);
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
    assert!(!app.pending_g);
    assert_eq!(app.view, View::Active);
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
fn logos_cover_known_providers() {
    for provider in ["claude", "codex", "grok", "opencode", "pi"] {
        assert_ne!(logo(provider), logo("unknown"), "{provider}");
    }
}
