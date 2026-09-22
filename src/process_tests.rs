use super::*;

fn process_map(
    lines: &[&str],
) -> (
    std::collections::HashMap<u32, Process>,
    std::collections::HashMap<u32, Vec<u32>>,
) {
    let processes: std::collections::HashMap<u32, Process> = lines
        .iter()
        .filter_map(|line| parse_process(line))
        .collect();
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for (pid, process) in &processes {
        children.entry(process.parent).or_default().push(*pid);
    }
    (processes, children)
}

#[test]
fn tree_contains_matches_root_and_descendants() {
    let (processes, children) = process_map(&[
        "50 1 sh",
        "100 50 agent-berth",
        "101 100 sleep",
        "60 1 sh",
        "110 60 codex",
    ]);
    // The pane process itself runs the match.
    assert!(tree_contains(100, "agent-berth", 0, &processes, &children));
    // A descendant of the pane runs the match.
    assert!(tree_contains(50, "agent-berth", 0, &processes, &children));
    assert!(!tree_contains(50, "codex", 0, &processes, &children));
    assert!(tree_contains(60, "codex", 0, &processes, &children));
    // The caller's own pid is skipped, so a self-match is ignored.
    assert!(!tree_contains(
        100,
        "agent-berth",
        100,
        &processes,
        &children
    ));
    // But the match still counts when it runs in a sibling of the caller.
    assert!(tree_contains(50, "agent-berth", 101, &processes, &children));
    // A cyclic process table cannot loop forever.
    let (processes, children) = process_map(&["1 2 agent-berth", "2 1 sh"]);
    assert!(tree_contains(2, "agent-berth", 0, &processes, &children));
}

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
