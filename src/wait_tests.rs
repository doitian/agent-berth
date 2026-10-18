use super::*;

/// A hook session whose process is this test, so it counts as active.
fn session(status: &str, turns_ended: u64) -> ListedSession {
    serde_json::from_value(serde_json::json!({
        "provider":"claude", "session_id":"s", "status":status, "source":"cli",
        "cwd":"/work", "cmdline":[], "pid":std::process::id(), "created_ms":1,
        "last_report_ms":2, "kind":"hook", "turns_ended":turns_ended
    }))
    .unwrap()
}

#[test]
fn outcomes_map_to_words_and_distinct_exit_codes() {
    let all = [
        Outcome::Done,
        Outcome::Waiting,
        Outcome::Gone,
        Outcome::Timeout,
    ];
    assert_eq!(
        all.map(Outcome::as_str),
        ["done", "waiting", "gone", "timeout"]
    );
    assert_eq!(all.map(Outcome::exit_code), [0, 2, 3, 4]);
}

#[test]
fn turn_ending_past_the_baseline_is_done() {
    let now = now_ms();
    assert_eq!(
        check(Some(&session("done", 1)), 0, now),
        Some(Outcome::Done)
    );
    assert_eq!(
        check(Some(&session("idle", 3)), 2, now),
        Some(Outcome::Done)
    );
    assert_eq!(check(Some(&session("done", 1)), 1, now), None);
    assert_eq!(check(Some(&session("running", 1)), 1, now), None);
}

#[test]
fn a_busy_session_is_not_done_even_past_the_baseline() {
    let now = now_ms();
    assert_eq!(check(Some(&session("running", 2)), 0, now), None);
    assert_eq!(
        check(Some(&session("waiting", 2)), 0, now),
        Some(Outcome::Waiting)
    );
}

#[test]
fn blocked_session_is_waiting() {
    assert_eq!(
        check(Some(&session("waiting", 0)), 0, now_ms()),
        Some(Outcome::Waiting)
    );
}

#[test]
fn missing_exited_or_inactive_session_is_gone() {
    let now = now_ms();
    assert_eq!(check(None, 0, now), Some(Outcome::Gone));
    let mut exited = session("done", 0);
    exited.exited = true;
    assert_eq!(check(Some(&exited), 0, now), Some(Outcome::Gone));
    let mut dead = session("done", 0);
    dead.pid = Some(u32::MAX);
    assert_eq!(check(Some(&dead), 0, now), Some(Outcome::Gone));
    // Its turn ended before it left, so the output is there to read.
    let mut finished = session("done", 1);
    finished.exited = true;
    assert_eq!(check(Some(&finished), 0, now), Some(Outcome::Done));
}

#[test]
fn stale_plugin_session_is_gone_before_it_is_waiting() {
    let mut plugin = session("waiting", 0);
    plugin.kind = crate::store::SessionKind::Plugin;
    plugin.last_report_ms = 1_000;
    assert_eq!(check(Some(&plugin), 0, 1_500), Some(Outcome::Waiting));
    assert_eq!(
        check(
            Some(&plugin),
            0,
            1_000 + crate::store::PLUGIN_STALE.as_millis() as u64 + 1
        ),
        Some(Outcome::Gone)
    );
}

#[test]
fn reasons_name_the_session_and_counts() {
    let before = session("done", 2);
    let name = format!("claude {}", before.id());
    let timeout = Duration::from_secs(90);
    let after = session("done", 3);
    assert_eq!(
        describe(Outcome::Done, &before, Some(&after), 2, timeout),
        format!("{name} ended a turn (turns_ended 3, was 2)")
    );
    assert_eq!(
        describe(Outcome::Waiting, &before, Some(&after), 2, timeout),
        format!("{name} is waiting on a permission prompt or a question")
    );
    let mut exited = after.clone();
    exited.exited = true;
    assert_eq!(
        describe(Outcome::Gone, &before, Some(&exited), 2, timeout),
        format!("{name} has exited")
    );
    assert_eq!(
        describe(Outcome::Gone, &before, Some(&after), 2, timeout),
        format!("{name} is no longer active")
    );
    assert_eq!(
        describe(Outcome::Gone, &before, None, 2, timeout),
        format!("{name} is no longer stored")
    );
    assert_eq!(
        describe(
            Outcome::Timeout,
            &before,
            Some(&session("running", 2)),
            2,
            timeout
        ),
        format!("{name} did not end a turn within 90s (status running, turns_ended 2, was 2)")
    );
}
