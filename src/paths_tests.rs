use super::*;
use std::path::Path;

#[test]
fn quote_path_wraps_spaces() {
    assert_eq!(quote_path(Path::new("agent-bridge")), "agent-bridge");
    assert_eq!(
        quote_path(Path::new("/opt/my tools/agent-bridge")),
        "\"/opt/my tools/agent-bridge\""
    );
}

#[test]
fn notify_command_includes_provider() {
    let root = Path::new("/tmp/ab");
    let ctx = Context::for_test(root, Path::new("/bin/agent-bridge"));
    assert_eq!(
        ctx.notify_command("claude"),
        "/bin/agent-bridge notify --provider claude"
    );
}
