use super::*;

#[test]
fn request_response_roundtrip() {
    let notify = Request::Notify {
        provider: "claude".into(),
        payload: serde_json::json!({"session_id":"s"}),
    };
    let text = serde_json::to_string(&notify).unwrap();
    let back: Request = serde_json::from_str(&text).unwrap();
    assert!(matches!(back, Request::Notify { provider, .. } if provider == "claude"));

    let list = Request::List {
        resumable: true,
        idle_ms: Some(1200),
    };
    let text = serde_json::to_string(&list).unwrap();
    assert!(text.contains("list"));
    let back: Request = serde_json::from_str(&text).unwrap();
    assert!(matches!(
        back,
        Request::List {
            resumable: true,
            idle_ms: Some(1200)
        }
    ));

    let all = serde_json::to_string(&Request::ListAll).unwrap();
    assert!(all.contains("list_all"));
    assert!(matches!(
        serde_json::from_str(&all).unwrap(),
        Request::ListAll
    ));

    let remove = Request::Remove {
        provider: "claude".into(),
        session_id: "s".into(),
    };
    let text = serde_json::to_string(&remove).unwrap();
    assert!(matches!(
        serde_json::from_str(&text).unwrap(),
        Request::Remove { provider, session_id }
            if provider == "claude" && session_id == "s"
    ));

    let ok = serde_json::to_string(&Response::ok()).unwrap();
    let parsed: Response = serde_json::from_str(&ok).unwrap();
    assert!(matches!(parsed, Response::Ok { sessions: None, .. }));
}
