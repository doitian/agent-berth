use super::*;
use std::path::Path;

#[test]
fn names_follow_tmux_up() {
    assert_eq!(session_name(Path::new("/work/agent-berth")), "agent-berth");
    assert_eq!(session_name(Path::new("/work/.dotfiles")), "dotfiles");
    assert_eq!(session_name(Path::new("/work/foo.bar:baz")), "foo_bar_baz");
}

#[test]
fn parses_listed_panes() {
    let pane = Pane::parse("%3\t4321\tproject\t0\tclaude\t/work/project").unwrap();
    assert_eq!(pane.id, "%3");
    assert_eq!(pane.pid, 4321);
    assert_eq!(pane.session, "project");
    assert_eq!(pane.window, "0");
    assert_eq!(pane.window_name, "claude");
    assert_eq!(pane.path, "/work/project");
}

#[test]
fn ignores_malformed_panes() {
    assert_eq!(Pane::parse(""), None);
    assert_eq!(Pane::parse("\tnotapid\ts\t0\tw\t/p"), None);
}

#[test]
fn preview_command_quotes_arguments() {
    assert_eq!(quote_arg("tmux"), "tmux");
    assert_eq!(quote_arg("a b"), "\"a b\"");
}
