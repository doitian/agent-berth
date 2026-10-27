use super::tests_support::rendered;
use super::*;
use serde_json::json;
use std::io::Write;

fn source(path: &Path, provider: &str) -> Source {
    Source {
        provider: provider.into(),
        session_id: "s".into(),
        path: Some(path.to_string_lossy().into_owned()),
        root: None,
    }
}

fn message(id: &str, text: &str) -> String {
    format!(
        "{}\n",
        json!({"type":"assistant","uuid":id,"message":{"content":[{"type":"text","text":text}]}})
    )
}

fn append_file(path: &Path, bytes: &[u8]) {
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn follows_complete_records_and_buffers_partial_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, message("1", "First")).unwrap();
    let source = source(&path, "claude");
    let mut reader = Reader::default();
    assert!(reader.preview_log(&source).contains("Assistant: First"));
    let next = message("2", "你好");
    let split = next.find('你').unwrap() + 1;
    append_file(&path, &next.as_bytes()[..split]);
    assert!(!reader.preview_log(&source).contains("你好"));
    let old_offset = reader.offset;
    append_file(&path, &next.as_bytes()[split..]);
    let output = reader.preview_log(&source);
    assert!(output.contains("Assistant: 你好"));
    assert_eq!(output.matches("Assistant: First").count(), 1);
    assert!(reader.offset > old_offset);
    assert_eq!(reader.preview_log(&source), output);
}

/// `text` in one raw zstd block, framed the way DeepSeek Harness appends each
/// write to its log.
fn zstd_frame(text: &str) -> Vec<u8> {
    let len = text.len();
    assert!(len < 256, "a one-byte content size");
    let block = (len << 3) | 1;
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, len as u8];
    frame.extend_from_slice(&block.to_le_bytes()[..3]);
    frame.extend_from_slice(text.as_bytes());
    frame
}

#[test]
fn follows_compressed_frames_and_waits_for_one_being_written() {
    let row = |id: &str, text: &str| {
        format!(
            "{}\n",
            json!({"type":"assistant/message","data":{"message":{"id":id,"content":[{"type":"text","text":text}]}}})
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.v4.jsonl.zstd");
    std::fs::write(&path, zstd_frame(&row("m1", "First"))).unwrap();
    let source = source(&path, "dsh");
    let mut reader = Reader::default();
    assert!(reader.preview_log(&source).contains("First"));

    let next = zstd_frame(&row("m2", "Second"));
    append_file(&path, &next[..10]);
    let output = reader.preview_log(&source);
    assert!(!output.contains("Second"));
    assert!(!output.contains("malformed"));
    let old_offset = reader.offset;

    append_file(&path, &next[10..]);
    let output = reader.preview_log(&source);
    assert!(output.contains("Second"));
    assert_eq!(output.matches("First").count(), 1);
    assert!(reader.offset > old_offset);
}

#[test]
fn replaces_duplicate_claude_records_and_skips_malformed_and_unknown_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, message("1", "Draft")).unwrap();
    let mut reader = Reader::default();
    let source = source(&path, "claude");
    reader.preview_log(&source);
    append_file(
        &path,
        format!(
            "broken\n{{\"type\":\"future\"}}\n{}{}",
            message("1", "Final"),
            message("1", "Final")
        )
        .as_bytes(),
    );
    let output = reader.preview_log(&source);
    assert!(!output.contains("Draft"));
    assert_eq!(output.matches("Assistant: Final").count(), 1);
    assert!(output.contains("malformed"));
}

#[test]
fn parses_claude_tools_images_and_omits_thinking() {
    let row = json!({"type":"assistant","message":{"content":[
        {"type":"thinking","thinking":"private reasoning"},
        {"type":"text","text":"Checking"},
        {"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}
    ]}});
    let output = parse(log("claude").unwrap(), &row).unwrap();
    assert!(output.contains("Assistant: Checking"));
    assert!(output.contains("Tool: Read"));
    assert!(output.contains("src/main.rs"));
    assert!(!output.contains("private reasoning"));
    let row = json!({"type":"user","message":{"content":[
        {"type":"text","text":"Look"}, {"type":"image","source":{"data":"base64"}},
        {"type":"tool_result","content":[{"type":"text","text":"file contents"}]}
    ]}});
    let output = parse(log("claude").unwrap(), &row).unwrap();
    assert!(output.contains("You: Look"));
    assert!(output.contains("You: [image]"));
    assert!(output.contains("Result: file contents"));
    assert!(!output.contains("base64"));
}

#[test]
fn codex_uses_response_items_without_event_mirrors_or_instructions() {
    assert_eq!(parse(log("codex").unwrap(), &json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}})).as_deref(), Some("You: Hello"));
    assert_eq!(parse(log("codex").unwrap(), &json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi"}]}})).as_deref(), Some("Assistant: Hi"));
    assert!(
        parse(
            log("codex").unwrap(),
            &json!({"type":"event_msg","payload":{"type":"agent_message","message":"Hi"}})
        )
        .is_none()
    );
    assert!(parse(log("codex").unwrap(), &json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"instructions"}]}})).is_none());
    for kind in ["function_call", "custom_tool_call"] {
        let text = parse(
            log("codex").unwrap(),
            &json!({"type":"response_item","payload":{"type":kind,"name":"exec","arguments":"ls"}}),
        )
        .unwrap();
        assert!(text.contains("Tool: exec"));
        assert!(text.contains("Input: ls"));
    }
    for kind in ["function_call_output", "custom_tool_call_output"] {
        assert_eq!(
            parse(
                log("codex").unwrap(),
                &json!({"type":"response_item","payload":{"type":kind,"output":"ok"}})
            )
            .as_deref(),
            Some("Result: ok")
        );
    }
}

#[test]
fn parses_pi_message_entries_and_skips_other_roles_and_entries() {
    assert_eq!(
        parse(
            log("pi").unwrap(),
            &json!({"type":"message","id":"a1","parentId":null,
                "message":{"role":"user","content":"Hello","timestamp":1}})
        )
        .as_deref(),
        Some("You: Hello")
    );
    let row = json!({"type":"message","id":"a2","parentId":"a1","message":{
        "role":"assistant",
        "content":[
            {"type":"thinking","thinking":"private reasoning"},
            {"type":"text","text":"Checking"},
            {"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}
        ],
        "stopReason":"toolUse"
    }});
    let output = parse(log("pi").unwrap(), &row).unwrap();
    assert!(output.contains("Assistant: Checking"));
    assert!(output.contains("Tool: bash"));
    assert!(output.contains("\"command\":\"ls\""));
    assert!(!output.contains("private reasoning"));
    let row = json!({"type":"message","id":"a3","parentId":"a2","message":{
        "role":"toolResult","toolCallId":"call_1","toolName":"bash",
        "content":[{"type":"text","text":"output"},{"type":"image","data":"base64"}],
        "isError":false
    }});
    assert_eq!(
        parse(log("pi").unwrap(), &row).as_deref(),
        Some("Result: output")
    );
    assert!(
        parse(
            log("pi").unwrap(),
            &json!({"type":"session","version":3,"id":"s"})
        )
        .is_none()
    );
    assert!(
        parse(
            log("pi").unwrap(),
            &json!({"type":"model_change","id":"m","modelId":"m"})
        )
        .is_none()
    );
    assert!(
        parse(
            log("pi").unwrap(),
            &json!({"type":"message","id":"a4","message":{"role":"system","content":"",
                "sections":{"preamble":"prompt text"}}})
        )
        .is_none()
    );
}

#[test]
fn resets_after_truncate_regrow_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, message("1", "old")).unwrap();
    let source = source(&path, "claude");
    let mut reader = Reader::default();
    reader.preview_log(&source);
    std::fs::write(&path, message("2", "replacement longer than old")).unwrap();
    let text = reader.preview_log(&source);
    assert!(text.contains("replacement longer"));
    assert!(!text.contains("Assistant: old"));
    std::fs::write(&path, "").unwrap();
    assert!(
        reader
            .preview_log(&source)
            .contains("No conversation messages")
    );
    std::fs::remove_file(&path).unwrap();
    assert!(reader.preview_log(&source).contains("file is missing"));
    std::fs::write(&path, message("3", "new file")).unwrap();
    assert!(reader.preview_log(&source).contains("Assistant: new file"));
}

#[test]
fn skips_oversized_records_and_bounds_recent_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut file = File::create(&path).unwrap();
    file.write_all(&vec![b'x'; READ_LIMIT as usize + 100])
        .unwrap();
    writeln!(file).unwrap();
    for i in 0..100 {
        file.write_all(message(&i.to_string(), &format!("message-{i}")).as_bytes())
            .unwrap();
    }
    let mut reader = Reader::default();
    let source = source(&path, "claude");
    let text = reader.preview_log(&source);
    assert!(text.contains("Assistant: message-99"));
    assert!(!text.contains("Assistant: message-0\n"));
    assert_eq!(reader.entries.len(), ENTRY_LIMIT);
    assert!(reader.pending.len() <= RECORD_LIMIT);
    // An oversized record arriving over several polls is discarded through its
    // newline; the next valid record must still be consumed.
    append_file(&path, &vec![b'x'; RECORD_LIMIT]);
    reader.preview_log(&source);
    append_file(&path, b"x");
    reader.preview_log(&source);
    assert!(reader.pending.is_empty());
    append_file(
        &path,
        format!("\n{}", message("last", "Recovered")).as_bytes(),
    );
    assert!(reader.preview_log(&source).contains("Assistant: Recovered"));
}

#[test]
fn bounds_text_and_removes_terminal_controls() {
    let raw = format!(
        "\x1b]52;c;clipboard\x07\u{009b}31m\r{}",
        "界".repeat(TEXT_LIMIT * 2)
    );
    let output = parse(
        log("claude").unwrap(),
        &json!({"type":"user","message":{"content":raw}}),
    )
    .unwrap();
    assert!(output.chars().count() <= TEXT_LIMIT);
    assert!(!output.chars().any(char::is_control));
    assert!(output.ends_with('…'));
}

#[test]
fn discovers_only_transcripts_with_matching_session_identity() {
    let dir = tempfile::tempdir().unwrap();
    for provider in ["claude", "codebuddy", "codex", "pi"] {
        let path = dir.path().join(match provider {
            "claude" | "codebuddy" => "s.jsonl",
            "codex" => "rollout-date-s.jsonl",
            _ => "2026-09-26T12-00-00_s.jsonl",
        });
        let source = Source {
            provider: provider.into(),
            session_id: "s".into(),
            path: None,
            root: Some(dir.path().into()),
        };
        std::fs::write(&path, "{\"sessionId\":\"other\"}\n").unwrap();
        assert!(locate(&source).is_none());
        let header = match provider {
            "claude" | "codebuddy" => json!({"sessionId":"s"}),
            "codex" => json!({"type":"session_meta","payload":{"id":"s"}}),
            _ => json!({"type":"session","version":3,"id":"s"}),
        };
        std::fs::write(&path, format!("{header}\n")).unwrap();
        assert_eq!(locate(&source), Some(path));
        let mut child = source.clone();
        child.session_id = "s:child".into();
        assert!(locate(&child).is_none());
    }
}

#[test]
fn unavailable_and_read_errors_are_distinct_and_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = source(dir.path(), "claude");
    let mut reader = Reader::default();
    assert!(
        reader
            .preview_log(&source)
            .contains("Unable to read transcript")
    );
    source.path = None;
    assert!(
        reader
            .preview_log(&source)
            .contains("waiting for a session hook")
    );
    source.path = Some(dir.path().join("missing").to_string_lossy().into_owned());
    assert!(reader.preview_log(&source).contains("file is missing"));
}

#[test]
fn supports_covers_exactly_the_parseable_providers() {
    assert!(supports("claude"));
    assert!(supports("codebuddy"));
    assert!(supports("codex"));
    assert!(supports("pi"));
    assert!(supports("dsh"));
    for provider in ["grok", "opencode", "unknown"] {
        assert!(!supports(provider), "{provider}");
    }
}

#[test]
fn full_reads_every_entry_without_truncating_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut body = String::new();
    for i in 0..ENTRY_LIMIT + 5 {
        body.push_str(&message(&i.to_string(), &format!("message-{i}")));
    }
    let long = "界".repeat(TEXT_LIMIT * 2);
    body.push_str(&message("long", &long));
    std::fs::write(&path, body).unwrap();

    let text = rendered(&full(&source(&path, "claude")).unwrap());
    assert!(text.contains("Assistant: message-0"));
    assert!(text.contains(&format!("Assistant: message-{}", ENTRY_LIMIT + 4)));
    assert!(text.contains(&long));
    assert!(!text.contains('…'));
}

#[test]
fn full_keeps_the_last_revision_of_a_claude_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(
        &path,
        format!("{}{}", message("1", "Draft"), message("1", "Final")),
    )
    .unwrap();
    assert_eq!(
        rendered(&full(&source(&path, "claude")).unwrap()),
        "Assistant: Final"
    );
}

#[test]
fn full_errors_without_a_readable_transcript() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        full(&source(dir.path(), "opencode"))
            .unwrap_err()
            .to_string()
            .contains("no local transcript")
    );
    let mut missing = source(dir.path(), "claude");
    missing.path = Some(
        dir.path()
            .join("missing.jsonl")
            .to_string_lossy()
            .into_owned(),
    );
    assert!(full(&missing).is_err());
    let mut unlocated = source(dir.path(), "claude");
    unlocated.path = None;
    unlocated.root = None;
    assert!(
        full(&unlocated)
            .unwrap_err()
            .to_string()
            .contains("no transcript file found")
    );
}

fn claude_turns(rows: &[Value]) -> Vec<Turn> {
    let log = log("claude").unwrap();
    turns(rows.iter().filter_map(|row| log.parse(row)))
}

fn user(content: Value) -> Value {
    json!({"type":"user","message":{"role":"user","content":content}})
}

fn assistant(content: Value, stop: &str) -> Value {
    json!({"type":"assistant","message":{"content":content,"stop_reason":stop}})
}

#[test]
fn claude_groups_a_prompt_and_its_work_into_one_complete_turn() {
    let turns = claude_turns(&[
        user(json!("<command-message>review</command-message>")),
        json!({"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"Skill body"}]}}),
        assistant(
            json!([{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]),
            "tool_use",
        ),
        user(json!([{"type":"tool_result","content":"src"}])),
        assistant(json!([{"type":"text","text":"All good."}]), "end_turn"),
        json!({"type":"system","subtype":"turn_duration"}),
        user(json!("next request")),
    ]);
    assert_eq!(turns.len(), 2);
    assert!(turns[0].complete);
    assert!(!turns[1].complete);
    let kinds: Vec<&str> = turns[0]
        .entries()
        .map(|entry| match entry {
            Entry::Prompt { .. } => "prompt",
            Entry::Context { .. } => "context",
            Entry::Text { .. } => "text",
            Entry::ToolCall { .. } => "call",
            Entry::ToolResult { .. } => "result",
        })
        .collect();
    assert_eq!(kinds, ["prompt", "context", "call", "result", "text"]);
}

#[test]
fn a_prompt_without_a_reply_joins_the_next_turn() {
    let turns = claude_turns(&[
        user(json!("<command-name>/model</command-name>")),
        json!({"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>Caveat"}}),
        user(json!(
            "<local-command-stdout>Set model</local-command-stdout>"
        )),
        user(json!("real task")),
        assistant(json!([{"type":"text","text":"Done."}]), "end_turn"),
        user(json!("[Request interrupted by user]")),
    ]);
    assert_eq!(turns.len(), 1);
    let prompts = turns[0]
        .entries()
        .filter(|entry| matches!(entry, Entry::Prompt { .. }))
        .count();
    assert_eq!(prompts, 2);
}

#[test]
fn a_task_notification_starts_a_turn_as_injected_context() {
    let turns = claude_turns(&[
        user(json!("task")),
        assistant(json!([{"type":"text","text":"Started it."}]), "end_turn"),
        user(json!("<task-notification>finished</task-notification>")),
        assistant(json!([{"type":"text","text":"It finished."}]), "end_turn"),
    ]);
    assert_eq!(turns.len(), 2);
    assert!(matches!(
        turns[1].entries().next(),
        Some(Entry::Context { .. })
    ));
}

#[test]
fn a_start_after_a_finished_turn_opens_a_new_one_even_without_output() {
    let marker = |starts_turn, ends_turn| Record {
        entries: Vec::new(),
        starts_turn,
        ends_turn,
    };
    let turns = turns([
        marker(true, false),
        marker(false, true),
        marker(true, false),
        marker(false, true),
    ]);
    assert_eq!(turns.len(), 2);
    assert!(turns.iter().all(|turn| turn.complete));
}
