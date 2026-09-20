use super::*;
use crate::paths::Context;
use tempfile::tempdir;

#[test]
fn ipc_failure_is_success() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-bridge"));
    send_payload(
        &ctx,
        "claude".into(),
        serde_json::json!({"session_id":"s","hook_event_name":"Stop"}),
    )
    .unwrap();
}
