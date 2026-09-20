use super::*;

#[test]
fn current_process_is_alive() {
    assert!(pid_alive(std::process::id()));
}

#[test]
fn zero_is_dead() {
    assert!(!pid_alive(0));
}
