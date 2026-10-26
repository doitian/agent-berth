use super::*;
use crate::status::AgentStatus;

fn emit(sessions: &mut BTreeMap<String, AgentSession>, event: Value) {
    apply_hook(sessions, &event);
}

#[test]
fn hook_lifecycle_reaches_done() {
    let mut sessions = BTreeMap::new();
    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"UserPromptSubmit"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);

    emit(
        &mut sessions,
        json!({
            "session_id":"s","hook_event_name":"Notification",
            "notification_type":"permission_prompt"
        }),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);

    // A tool call only clears the waiting state once it completes.
    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"PreToolUse"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);

    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"PostToolUse"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);

    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"Stop"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Done);

    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"SessionEnd"}),
    );
    assert!(sessions.is_empty());
}

#[test]
fn idle_and_auth_notifications_are_ignored() {
    let mut sessions = BTreeMap::new();
    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"UserPromptSubmit"}),
    );
    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"Stop"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    assert_eq!(sessions["s"].turns_ended, 1);

    // The idle reminder is not a second turn end, and subagent or auth
    // notifications are not this session's lifecycle.
    for notification in ["idle_prompt", "auth_success"] {
        emit(
            &mut sessions,
            json!({
                "session_id":"s","hook_event_name":"Notification",
                "notification_type":notification
            }),
        );
    }
    emit(
        &mut sessions,
        json!({"session_id":"s","hook_event_name":"SubagentStop"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Done);
    assert_eq!(sessions["s"].turns_ended, 1);
}

#[test]
fn exposes_session_id_to_shell_tools() {
    assert_eq!(CodeBuddy.session_env(), Some("CODEBUDDY_SESSION_ID"));
}

#[test]
fn subagent_hooks_are_not_tracked() {
    let mut sessions = BTreeMap::new();
    // CodeBuddy writes no subagent transcript, so an agent-scoped session
    // would never finish or stream.
    emit(
        &mut sessions,
        json!({
            "session_id":"parent","agent_id":"child",
            "hook_event_name":"UserPromptSubmit"
        }),
    );
    assert!(sessions.is_empty());

    emit(
        &mut sessions,
        json!({"session_id":"parent","hook_event_name":"UserPromptSubmit"}),
    );
    assert_eq!(sessions["parent"].status, AgentStatus::Running);
    for event in ["PreToolUse", "PostToolUse", "Stop"] {
        emit(
            &mut sessions,
            json!({"session_id":"parent","agent_id":"child","hook_event_name":event}),
        );
    }
    assert_eq!(sessions.keys().cloned().collect::<Vec<_>>(), ["parent"]);
    assert_eq!(sessions["parent"].status, AgentStatus::Running);

    // The parent's own SubagentStop must not end its turn either.
    emit(
        &mut sessions,
        json!({"session_id":"parent","hook_event_name":"SubagentStop"}),
    );
    assert_eq!(sessions["parent"].status, AgentStatus::Running);
}

fn parsed(row: Value) -> Record {
    CodeBuddy.parse(&row).unwrap()
}

#[test]
fn transcript_rows_become_entries_and_mark_turn_boundaries() {
    let injected = parsed(json!({
        "id":"1","type":"message","role":"user",
        "content":[{"type":"input_text","text":"<local-command-stdout>ok</local-command-stdout>"}],
        "providerData":{"skipRun":true}
    }));
    assert_eq!(
        injected.entries,
        [Entry::Context {
            text: "<local-command-stdout>ok</local-command-stdout>".into()
        }]
    );
    assert!(!injected.starts_turn);

    let asked = parsed(json!({
        "id":"2","type":"message","role":"user",
        "content":[{"type":"input_text","text":"fix it"}]
    }));
    assert_eq!(
        asked.entries,
        [Entry::Prompt {
            text: "fix it".into()
        }]
    );
    assert!(asked.starts_turn);

    let answered = parsed(json!({
        "id":"3","type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"done"}]
    }));
    assert_eq!(
        answered.entries,
        [Entry::Text {
            text: "done".into()
        }]
    );

    // The client writes an interruption marker as an assistant skipRun row;
    // it must not become the final reply.
    let interrupted = parsed(json!({
        "id":"7","type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"Interrupted by user"}],
        "providerData":{"skipRun":true}
    }));
    assert_eq!(
        interrupted.entries,
        [Entry::Context {
            text: "Interrupted by user".into()
        }]
    );

    let call = parsed(json!({
        "id":"4","type":"function_call","name":"Bash",
        "arguments":"{\"command\":\"ls\"}"
    }));
    assert_eq!(
        call.entries,
        [Entry::ToolCall {
            name: "Bash".into(),
            input: "{\"command\":\"ls\"}".into()
        }]
    );

    let result = parsed(json!({
        "id":"5","type":"function_call_result","name":"Bash",
        "output":{"type":"text","text":"a.rs"}
    }));
    assert_eq!(
        result.entries,
        [Entry::ToolResult {
            text: "a.rs".into()
        }]
    );

    let end = parsed(json!({"id":"6","type":"turn-metrics","durationMs":10}));
    assert!(end.ends_turn);
    assert!(end.entries.is_empty());

    // Rows that are neither conversation nor turn markers are skipped.
    assert!(
        CodeBuddy
            .parse(&json!({"type":"reasoning","content":[]}))
            .is_none()
    );
    assert!(CodeBuddy.parse(&json!({"type":"session-meta"})).is_none());
}

#[test]
fn transcript_rows_are_located_by_session_id_and_file_name() {
    assert!(CodeBuddy.is_session_file("abc.jsonl", "abc"));
    assert!(!CodeBuddy.is_session_file("abc.jsonl", "other"));
    assert!(CodeBuddy.is_session_record(&json!({"sessionId":"abc"}), "abc"));
    assert!(!CodeBuddy.is_session_record(&json!({"sessionId":"other"}), "abc"));
}

#[test]
fn install_merges_settings_and_uninstall_leaves_other_hooks() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.codebuddy_config_dir).unwrap();
    let settings = ctx.codebuddy_config_dir.join("settings.json");
    std::fs::write(
        &settings,
        json!({
            "theme":"dark",
            "hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}
        })
        .to_string(),
    )
    .unwrap();

    let path = CodeBuddy.install(&ctx).unwrap();
    assert_eq!(path, settings);
    let data: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(data["theme"], "dark");
    assert_eq!(data["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        data["hooks"]["Notification"][0]["matcher"],
        "permission_prompt"
    );
    assert!((&CodeBuddy as &dyn Provider).hooks_installed(&ctx));
    assert!(!CodeBuddy.outdated(&ctx));

    CodeBuddy.uninstall(&ctx).unwrap();
    let data: Value = serde_json::from_slice(&std::fs::read(&settings).unwrap()).unwrap();
    assert_eq!(data["theme"], "dark");
    assert_eq!(data["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(data["hooks"]["Stop"][0]["hooks"][0]["command"], "echo keep");
    assert!(data["hooks"].get("Notification").is_none());
    assert!(!(&CodeBuddy as &dyn Provider).hooks_installed(&ctx));
}
