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
fn worktree_label_uses_folder_name_for_plain_directories() {
    let home = Some(Path::new("/home/me"));
    for (path, expected) in [
        ("/home/me/codebase/agent-berth", "agent-berth"),
        ("/home/me/codebase/agent-berth/", "agent-berth"),
        (r"C:\Users\me\codebase\agent-berth", "agent-berth"),
        (r"\\server\share\project", "project"),
        ("project", "project"),
        ("/project", "project"),
        ("/", "/"),
        (r"C:\", "C:"),
        ("", "-"),
    ] {
        assert_eq!(worktree_label_with_home(path, home), expected, "{path}");
    }
}

#[test]
fn worktree_label_normalizes_sibling_worktrees_folder() {
    let home = Some(Path::new("/home/me"));
    assert_eq!(
        worktree_label_with_home("/home/me/codebase/acme-platform.worktrees/feature-x", home),
        "acme-platform (feature-x)"
    );
}

#[test]
fn worktree_label_normalizes_central_worktrees_folder() {
    let home = Some(Path::new("/home/me"));
    assert_eq!(
        worktree_label_with_home("/home/me/.codex/worktrees/ab12/acme-platform", home),
        "acme-platform (ab12)"
    );
    // Without a matching $HOME the folder is treated as nested in a checkout.
    assert_eq!(
        worktree_label_with_home(
            "/home/me/.codex/worktrees/ab12/acme-platform",
            Some(Path::new("/home/other"))
        ),
        "me (acme-platform)"
    );
}

#[test]
fn worktree_label_normalizes_worktrees_inside_main_checkout() {
    let home = Some(Path::new("/home/me"));
    assert_eq!(
        worktree_label_with_home(
            "/home/me/codebase/acme-platform/.claude/worktrees/fix-crash-a1b2c3",
            home
        ),
        "acme-platform (fix-crash-a1b2c3)"
    );
    assert_eq!(
        worktree_label_with_home("/home/me/codebase/acme-platform/worktrees/fix", home),
        "acme-platform (fix)"
    );
}

#[test]
fn current_dir_matches_only_itself() {
    let cwd = std::env::current_dir().unwrap();
    assert!(is_current_dir(&cwd.display().to_string()));
    assert!(!is_current_dir(""));
    let missing = cwd.join("agent-berth-missing-dir");
    assert!(!is_current_dir(&missing.display().to_string()));
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

#[test]
fn find_executable_resolves_launchable_shims() {
    let dir = tempfile::tempdir().unwrap();
    if cfg!(windows) {
        std::fs::write(dir.path().join("probe.cmd"), "").unwrap();
        assert_eq!(
            find_executable_in("probe", dir.path().as_os_str()),
            Some(dir.path().join("probe.cmd"))
        );
        // A real executable wins over the shim.
        std::fs::write(dir.path().join("probe.exe"), "").unwrap();
        assert_eq!(
            find_executable_in("probe", dir.path().as_os_str()),
            Some(dir.path().join("probe.exe"))
        );
        // An extensionless file is not launchable through CreateProcess.
        std::fs::write(dir.path().join("plain"), "").unwrap();
        assert_eq!(find_executable_in("plain", dir.path().as_os_str()), None);
    } else {
        std::fs::write(dir.path().join("probe"), "").unwrap();
        assert_eq!(
            find_executable_in("probe", dir.path().as_os_str()),
            Some(dir.path().join("probe"))
        );
    }
    assert_eq!(find_executable_in("missing", dir.path().as_os_str()), None);
}
