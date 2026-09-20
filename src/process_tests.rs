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
