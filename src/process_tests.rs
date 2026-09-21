use super::*;

#[test]
fn current_process_is_alive() {
    assert!(pid_alive(std::process::id()));
}

#[test]
fn zero_is_dead() {
    assert!(!pid_alive(0));
}

#[test]
fn finds_nearest_named_ancestor_through_wrappers() {
    let processes = [
        "100 90 agent-berth",
        "90 80 /bin/sh",
        "80 70 /opt/Codex CLI/codex",
        "70 60 node",
        "60 1 codex",
    ]
    .into_iter()
    .filter_map(parse_process)
    .collect();
    assert_eq!(find_ancestor(100, "codex", &processes), Some(80));
    assert_eq!(find_ancestor(100, "claude", &processes), None);
    assert_eq!(find_ancestor(100, "agent-berth", &processes), None);
}

#[test]
fn finds_windows_executable_and_handles_cycles() {
    let processes = ["100 90 agent-berth.exe", "90 80 cmd.exe", "80 90 CODEX.EXE"]
        .into_iter()
        .filter_map(parse_process)
        .collect();
    assert_eq!(find_ancestor(100, "codex", &processes), Some(80));
    assert_eq!(find_ancestor(100, "missing", &processes), None);
}

#[test]
fn ancestors_include_self() {
    let pid = std::process::id();
    let chain = ancestors(pid);
    assert_eq!(chain.first(), Some(&pid));
    assert!(chain.contains(&pid));
    assert!(ancestors(0).is_empty());
}

#[test]
fn ancestors_include_parent_process() {
    #[cfg(unix)]
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    #[cfg(windows)]
    let mut child = std::process::Command::new("ping")
        .args(["-n", "5", "127.0.0.1"])
        .spawn()
        .unwrap();
    let parent = std::process::id();
    let chain = ancestors(child.id());
    let _ = child.kill();
    let _ = child.wait();
    assert!(chain.contains(&parent), "{chain:?} missing {parent}");
}
