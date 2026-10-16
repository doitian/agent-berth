use super::*;

#[test]
fn window_is_named_after_the_program() {
    assert_eq!(window_name("claude"), "claude");
    assert_eq!(window_name("/usr/local/bin/codex"), "codex");
    assert_eq!(window_name("opencode.exe"), "opencode");
    assert_eq!(window_name(&"x".repeat(40)), "x".repeat(24));
}
