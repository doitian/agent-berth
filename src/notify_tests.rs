use super::*;
use crate::paths::Context;
use tempfile::tempdir;

#[test]
fn codex_originator_comes_from_the_hook_environment_unless_explicit() {
    for (environment, expected) in [
        (Some("Codex Desktop"), "Codex Desktop"),
        (Some("codex_work_desktop"), "codex_work_desktop"),
        (Some("codex-tui"), "codex-tui"),
        (None, "codex-cli"),
        (Some(""), "codex-cli"),
    ] {
        let mut payload = serde_json::json!({});
        enrich_codex_originator(&mut payload, environment);
        assert_eq!(payload["originator"], expected);
        let mut payload = serde_json::json!({"originator":"explicit"});
        enrich_codex_originator(&mut payload, environment);
        assert_eq!(payload["originator"], "explicit");
    }
}

#[test]
fn ipc_failure_is_success() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    send_payload(
        &ctx,
        "claude".into(),
        serde_json::json!({"session_id":"s","hook_event_name":"Stop"}),
    )
    .unwrap();
}
