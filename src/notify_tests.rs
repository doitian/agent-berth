use super::*;
use crate::paths::Context;
use tempfile::tempdir;

#[test]
fn paseo_identity_comes_from_the_hook_environment_unless_explicit() {
    for environment in [Some("agent-123"), None, Some("")] {
        let mut payload = serde_json::json!({});
        enrich_paseo_agent(&mut payload, environment);
        assert_eq!(
            payload["paseo_agent_id"].as_str(),
            environment.filter(|id| !id.is_empty())
        );
        let mut payload = serde_json::json!({"paseo_agent_id":"explicit"});
        enrich_paseo_agent(&mut payload, environment);
        assert_eq!(payload["paseo_agent_id"], "explicit");
    }
    let mut payload = Value::Null;
    enrich_paseo_agent(&mut payload, Some("agent-123"));
    assert!(payload.is_null());
}

#[test]
fn delegator_comes_from_the_hook_environment_unless_explicit_or_empty() {
    let mut payload = serde_json::json!({});
    enrich_delegator(&mut payload, Some("boss12"));
    assert_eq!(payload["delegator"], "boss12");
    let mut payload = serde_json::json!({"delegator":"explicit"});
    enrich_delegator(&mut payload, Some("boss12"));
    assert_eq!(payload["delegator"], "explicit");
    for environment in [None, Some("")] {
        let mut payload = serde_json::json!({});
        enrich_delegator(&mut payload, environment);
        assert!(payload.get("delegator").is_none());
    }
    let mut payload = Value::Null;
    enrich_delegator(&mut payload, Some("boss12"));
    assert!(payload.is_null());
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
