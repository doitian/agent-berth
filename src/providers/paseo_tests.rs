use super::*;
use serde_json::json;
use std::time::Instant;

#[path = "../../tests/support/paseo.rs"]
mod mock;
use mock::{MockPaseo, Options};

fn session(provider: &str, sid: &str) -> ListedSession {
    serde_json::from_value(json!({
        "provider":provider, "session_id":sid, "status":"running", "source":"cli",
        "cwd":"/work", "cmdline":[], "pid":null, "created_ms":1,
        "last_report_ms":2, "kind":"hook"
    }))
    .unwrap()
}

fn context(root: &Path) -> Context {
    Context::for_test(root, &root.join("berth"))
}

fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "Paseo test timed out");
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn attributes_only_existing_sessions_without_changing_monitoring_or_resume() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let dir = ctx.paseo_home.join("agents/project");
    std::fs::create_dir_all(&dir).unwrap();
    for (id, sid, extra) in [
        ("paseo-one", "native", json!({})),
        ("paseo-unknown", "unknown", json!({})),
        ("paseo-archived", "archived", json!({"archivedAt":"today"})),
        ("paseo-internal", "internal", json!({"internal":true})),
        ("paseo-closed", "closed", json!({"lastStatus":"closed"})),
    ] {
        let mut record = json!({"id":id, "provider":"claude", "persistence":{"sessionId":sid}});
        record
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        std::fs::write(dir.join(format!("{id}.json")), record.to_string()).unwrap();
    }
    let mut sessions = vec![
        session("claude", "native"),
        session("codex", "native"),
        session("claude", "archived"),
        session("claude", "internal"),
        session("claude", "closed"),
    ];
    let mut expected = serde_json::to_value(&sessions).unwrap();
    expected[0]["paseo_agent_id"] = "paseo-one".into();
    attribute(&ctx, &mut sessions);
    assert_eq!(serde_json::to_value(&sessions).unwrap(), expected);
    sessions[0].paseo_agent_id = Some("explicit-hook-id".into());
    attribute(&ctx, &mut sessions);
    assert_eq!(
        sessions[0].paseo_agent_id.as_deref(),
        Some("explicit-hook-id")
    );
}

#[test]
fn routing_switches_between_native_hosts_and_newer_paseo_imports() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let dir = ctx.paseo_home.join("agents/project");
    std::fs::create_dir_all(&dir).unwrap();
    let write_record = |id: &str, created: &str| {
        std::fs::write(
            dir.join(format!("{id}.json")),
            json!({
                "id":id, "provider":"pi", "createdAt":created, "lastStatus":"idle",
                "runtimeInfo":{"sessionId":"native"}
            })
            .to_string(),
        )
        .unwrap();
    };
    write_record("old-paseo", "2026-01-01T00:00:00Z");
    let mut native = session("pi", "native");
    native.paseo_host_changed_ms = parse_timestamp("2026-01-02T00:00:00Z");
    let mut sessions = vec![native.clone()];
    attribute(&ctx, &mut sessions);
    assert!(sessions[0].paseo_agent_id.is_none());
    write_record("new-paseo", "2026-01-03T00:00:00Z");
    attribute(&ctx, &mut sessions);
    assert_eq!(sessions[0].paseo_agent_id.as_deref(), Some("new-paseo"));
    native.paseo_host_changed_ms = parse_timestamp("2026-01-04T00:00:00Z");
    sessions = vec![native];
    attribute(&ctx, &mut sessions);
    assert!(sessions[0].paseo_agent_id.is_none());
    assert_eq!(
        parse_timestamp("2026-01-03T01:00:00+01:00"),
        parse_timestamp("2026-01-03T00:00:00Z")
    );
}

#[test]
fn api_stream_merges_chunks_deduplicates_history_and_closes_on_drop() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let mock = MockPaseo::new(&ctx.paseo_home, Options::default());
    let mut stream = Stream::new("paseo-agent", &ctx.paseo_home);
    wait(|| stream.preview().contains("Hello streamed"));
    let content = stream.preview();
    assert_eq!(content.matches("Hello").count(), 1);
    assert!(!content.contains("WRONG"));
    drop(stream);
    wait(|| mock.closed.load(Ordering::Relaxed) == 1);
    let frames = mock.received.lock().unwrap();
    let hello = frames
        .iter()
        .find(|frame| frame["type"] == "hello")
        .unwrap();
    assert_eq!(hello["protocolVersion"], 1);
    assert_eq!(hello["auth"]["kind"], "localCredential");
    assert!(
        frames
            .iter()
            .any(|frame| frame["message"]["type"] == "agent.timeline.set_subscription.request")
    );
}

#[test]
fn replacement_refetches_history_and_discards_the_previous_epoch() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let _mock = MockPaseo::new(
        &ctx.paseo_home,
        Options {
            replace: true,
            ..Options::default()
        },
    );
    let mut stream = Stream::new("paseo-agent", &ctx.paseo_home);
    wait(|| stream.preview().contains("replacement history"));
    assert!(!stream.preview().contains("Hello"));
}

#[test]
fn stream_reconnects_and_refreshes_history_with_a_new_client_identity() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let mock = MockPaseo::new(
        &ctx.paseo_home,
        Options {
            disconnect_once: true,
            ..Options::default()
        },
    );
    let mut stream = Stream::new("paseo-agent", &ctx.paseo_home);
    // Connections are counted on accept, before the reconnect's hello arrives.
    let hellos = || {
        mock.received
            .lock()
            .unwrap()
            .iter()
            .filter(|frame| frame["type"] == "hello")
            .count()
    };
    wait(|| {
        mock.connections.load(Ordering::Relaxed) >= 2
            && hellos() >= 2
            && stream.preview().contains("Hello streamed")
    });
    let frames = mock.received.lock().unwrap();
    let clients: Vec<_> = frames
        .iter()
        .filter(|frame| frame["type"] == "hello")
        .map(|frame| &frame["clientId"])
        .collect();
    assert_ne!(clients[0], clients[1]);
    assert_eq!(stream.preview().matches("Hello").count(), 1);
}

#[test]
fn focus_verifies_the_agent_and_uses_the_api_server_identity_in_a_deep_link() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    let mock = MockPaseo::new(
        &ctx.paseo_home,
        Options {
            server_id: "server /name".into(),
            ..Options::default()
        },
    );
    assert_eq!(
        focus_url(&ctx, "paseo-agent").unwrap(),
        "paseo://h/server%20%2Fname/agent/paseo-agent"
    );
    let frames = mock.received.lock().unwrap();
    assert!(
        frames
            .iter()
            .any(|frame| frame["message"]["type"] == "fetch_agent_request")
    );
    assert!(
        !frames
            .iter()
            .any(|frame| frame["message"]["type"] == "agent.timeline.set_subscription.request")
    );
}

#[test]
fn focus_connects_to_the_published_local_socket_endpoint() {
    use interprocess::local_socket::{ListenerOptions, prelude::*};
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    std::fs::create_dir_all(&ctx.paseo_home).unwrap();
    #[cfg(windows)]
    let (name, listen) = {
        let name = format!(
            "paseo-test-{}-{}",
            std::process::id(),
            root.path().file_name().unwrap().to_string_lossy()
        );
        (
            name.clone()
                .to_ns_name::<interprocess::local_socket::GenericNamespaced>()
                .unwrap(),
            format!("pipe://{name}"),
        )
    };
    #[cfg(unix)]
    let (name, listen) = {
        let path = root.path().join("paseo.sock");
        (
            path.clone()
                .to_fs_name::<interprocess::local_socket::GenericFilePath>()
                .unwrap(),
            format!("unix://{}", path.display()),
        )
    };
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    std::fs::write(
        ctx.paseo_home.join("paseo.pid"),
        json!({"listen":listen,"serverId":"socket-server"}).to_string(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        let stream = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let hello: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(hello["type"], "hello");
        ws.send(tungstenite::Message::Text(
            json!({"type":"session","message":{"type":"status",
            "payload":{"status":"server_info","serverId":"socket-server"}}})
            .to_string()
            .into(),
        ))
        .unwrap();
        let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        let request = &request["message"];
        assert_eq!(request["type"], "fetch_agent_request");
        ws.send(tungstenite::Message::Text(json!({"type":"session","message":{"type":"fetch_agent_response",
            "payload":{"requestId":request["requestId"],"agent":{"id":"paseo-agent"},"error":null}}}).to_string().into())).unwrap();
        let _ = ws.read();
    });
    let result = focus_url(&ctx, "paseo-agent");
    assert_eq!(result.unwrap(), "paseo://h/socket-server/agent/paseo-agent");
    server.join().unwrap();
}

#[test]
fn missing_sessions_and_authentication_errors_are_visible_without_secrets() {
    for (options, expected) in [
        (
            Options {
                missing: true,
                ..Options::default()
            },
            "does not exist",
        ),
        (
            Options {
                reject_auth: true,
                ..Options::default()
            },
            "authentication rejected",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        let _mock = MockPaseo::new(&ctx.paseo_home, options);
        let error = focus_url(&ctx, "paseo-agent").unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains(&"A".repeat(43)));
    }
}

#[test]
fn invalid_ids_and_nonlocal_endpoints_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let ctx = context(root.path());
    for id in ["", "-option", "a;b", "x\ny", "$(cmd)", "'quote'"] {
        assert!(!valid_id(id));
        assert!(
            focus_url(&ctx, id)
                .unwrap_err()
                .to_string()
                .contains("invalid Paseo")
        );
    }
    std::fs::create_dir_all(&ctx.paseo_home).unwrap();
    std::fs::write(
        ctx.paseo_home.join("paseo.pid"),
        json!({"listen":"192.0.2.1:6767"}).to_string(),
    )
    .unwrap();
    let error = focus_url(&ctx, "valid-id").unwrap_err().to_string();
    assert!(
        error.contains("refusing to send local credentials remotely"),
        "{error}"
    );
}

#[test]
fn timeline_is_bounded_and_omits_reasoning_and_terminal_controls() {
    let mut timeline = timeline::Timeline::default();
    timeline.history(&json!({"epoch":"e", "entries":[]}));
    for seq in 0..60 {
        timeline.event(&json!({"seq":seq, "event":{"type":"timeline", "item":{
            "type":"user_message", "text":format!("\u{1b}\u{0}\u{7}\u{9b}{}", "é".repeat(10_000))
        }}}));
    }
    timeline.event(&json!({"seq":61, "event":{"type":"timeline", "item":{"type":"reasoning", "text":"hidden reasoning"}}}));
    let content = timeline.render();
    assert!(content.len() < 70 * 1024);
    assert!(content.chars().all(|ch| !ch.is_control() || ch == '\n'));
    assert!(!content.contains("hidden reasoning"));
}

#[test]
fn growing_tool_updates_stay_within_the_total_limit() {
    let mut timeline = timeline::Timeline::default();
    timeline.history(&json!({"epoch":"e", "entries":[]}));
    for call in 0..48 {
        timeline.event(&json!({"event":{"type":"timeline", "item":{
            "type":"tool_call", "callId":format!("call-{call}"), "name":"t"
        }}}));
    }
    for call in 0..48 {
        timeline.event(&json!({"event":{"type":"timeline", "item":{
            "type":"tool_call", "callId":format!("call-{call}"), "name":"x".repeat(8192)
        }}}));
    }
    assert!(timeline.render().len() < 70 * 1024);
}

#[test]
fn failed_and_canceled_turns_are_labeled_by_their_event() {
    let mut timeline = timeline::Timeline::default();
    timeline.event(&json!({"event":{"type":"turn_failed"}}));
    timeline.event(&json!({"event":{"type":"turn_canceled"}}));
    let content = timeline.render();
    assert!(content.contains("Error: Turn failed"));
    assert!(content.contains("Error: Turn canceled"));
}
