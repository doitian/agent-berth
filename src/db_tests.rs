use super::*;
use crate::paths::Context;
use tempfile::tempdir;

#[test]
fn heartbeat_does_not_rewrite_sessions() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.state_dir).unwrap();
    let db = open(&ctx).unwrap();
    let mut store = Store::default();
    let change = store
        .update(
            "claude",
            serde_json::json!({
                "session_id": "abc",
                "hook_event_name": "UserPromptSubmit",
                "cwd": "/work"
            }),
        )
        .unwrap();
    persist_change(&db, &store, &change).unwrap();
    store.heartbeat();
    persist_heartbeats(&db, &store).unwrap();
    drop(db);

    let loaded = load_from_path(&ctx).unwrap();
    assert_eq!(loaded.hooks["claude"]["abc"].cwd.as_deref(), Some("/work"));
    assert_eq!(loaded.last_heartbeat_ms, store.last_heartbeat_ms);
}

#[test]
fn session_end_removes_only_that_hook() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.state_dir).unwrap();
    let db = open(&ctx).unwrap();
    let mut store = Store::default();
    for sid in ["keep", "drop"] {
        let change = store
            .update(
                "claude",
                serde_json::json!({
                    "session_id": sid,
                    "hook_event_name": "UserPromptSubmit",
                    "cwd": "/work"
                }),
            )
            .unwrap();
        persist_change(&db, &store, &change).unwrap();
    }
    let change = store
        .update(
            "claude",
            serde_json::json!({"session_id":"drop","hook_event_name":"SessionEnd"}),
        )
        .unwrap();
    persist_change(&db, &store, &change).unwrap();
    drop(db);

    let loaded = load_from_path(&ctx).unwrap();
    assert!(loaded.hooks["claude"].contains_key("keep"));
    assert!(!loaded.hooks["claude"].contains_key("drop"));
}

#[test]
fn persists_plugin_snapshot_and_heartbeats() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.state_dir).unwrap();
    let db = open(&ctx).unwrap();
    let mut store = Store {
        last_heartbeat_ms: 42,
        ..Default::default()
    };
    let change = store
        .update(
            "opencode",
            serde_json::json!({
                "id":"99",
                "cwd":"/proj",
                "status":{"sess":"busy"},
                "blocking":["sess"]
            }),
        )
        .unwrap();
    persist_change(&db, &store, &change).unwrap();
    store.on_server_start();
    persist_heartbeats(&db, &store).unwrap();
    drop(db);

    let loaded = load_from_path(&ctx).unwrap();
    assert_eq!(loaded.previous_heartbeat_ms, Some(42));
    let snap = &loaded.snapshots["opencode"]["99"];
    assert_eq!(snap.status["sess"], "busy");
    assert_eq!(snap.blocking, ["sess"]);
    assert_eq!(snap.cwd.as_deref(), Some("/proj"));
}

#[test]
fn persist_all_drops_pruned_entries() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.state_dir).unwrap();
    let db = open(&ctx).unwrap();
    let mut store = Store::default();
    store
        .update(
            "opencode",
            serde_json::json!({"id":"1","status":{"s":"busy"}}),
        )
        .unwrap();
    persist_all(&db, &store).unwrap();
    store.snapshots.clear();
    store.mark_removed("opencode", "s");
    persist_all(&db, &store).unwrap();
    drop(db);

    let loaded = load_from_path(&ctx).unwrap();
    assert!(loaded.snapshots.is_empty());
    assert!(loaded.is_removed("opencode", "s"));
}

#[test]
fn migrates_legacy_json_once() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.state_dir).unwrap();
    let mut store = Store::default();
    store
        .update(
            "pi",
            serde_json::json!({
                "id":"1",
                "status":{"s":"idle"},
                "cwd":"/old"
            }),
        )
        .unwrap();
    std::fs::write(
        ctx.json_legacy_path(),
        serde_json::to_vec_pretty(&store).unwrap(),
    )
    .unwrap();
    let db = open(&ctx).unwrap();
    drop(db);
    assert!(!ctx.json_legacy_path().exists());
    let loaded = load_from_path(&ctx).unwrap();
    assert_eq!(loaded.snapshots["pi"]["1"].cwd.as_deref(), Some("/old"));
}
