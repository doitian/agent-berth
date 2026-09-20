use super::*;
use std::path::Path;

#[test]
fn names_follow_tmux_up() {
    assert_eq!(session_name(Path::new("/work/agent-berth")), "agent-berth");
    assert_eq!(session_name(Path::new("/work/.dotfiles")), "dotfiles");
    assert_eq!(session_name(Path::new("/work/foo.bar:baz")), "foo_bar_baz");
}
