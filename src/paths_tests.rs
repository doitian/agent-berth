use super::*;
use std::path::Path;

#[test]
fn quote_path_wraps_spaces() {
    if cfg!(windows) {
        assert_eq!(quote_path(Path::new("agent-berth")), "agent-berth");
        assert_eq!(
            quote_path(Path::new("C:\\tools\\agent-berth")),
            "C:/tools/agent-berth"
        );
    } else {
        assert_eq!(quote_path(Path::new("agent-berth")), "agent-berth");
        assert_eq!(
            quote_path(Path::new("/opt/my tools/agent-berth")),
            "\"/opt/my tools/agent-berth\""
        );
    }
}

#[test]
fn notify_command_includes_provider() {
    let root = Path::new("/tmp/ab");
    let ctx = Context::for_test(root, Path::new("/bin/agent-berth"));
    assert_eq!(
        ctx.notify_command("claude"),
        "/bin/agent-berth notify --provider claude"
    );
}
