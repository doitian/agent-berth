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
    assert!(reader.preview(&source).contains("Assistant: First"));
    let next = message("2", "你好");
    let split = next.find('你').unwrap() + 1;
    append_file(&path, &next.as_bytes()[..split]);
    assert!(!reader.preview(&source).contains("你好"));
    let old_offset = reader.offset;
    append_file(&path, &next.as_bytes()[split..]);
    let output = reader.preview(&source);
    assert!(output.contains("Assistant: 你好"));
    assert_eq!(output.matches("Assistant: First").count(), 1);
    assert!(reader.offset > old_offset);
    assert_eq!(reader.preview(&source), output);
}

#[test]
fn replaces_duplicate_claude_records_and_skips_malformed_and_unknown_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, message("1", "Draft")).unwrap();
    let mut reader = Reader::default();
    let source = source(&path, "claude");
    reader.preview(&source);
    append_file(
        &path,
        format!(
            "broken\n{{\"type\":\"future\"}}\n{}{}",
            message("1", "Final"),
            message("1", "Final")
        )
        .as_bytes(),
    );
    let output = reader.preview(&source);
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
    let output = parse("claude", &row).unwrap();
    assert!(output.contains("Assistant: Checking"));
    assert!(output.contains("Tool: Read"));
    assert!(output.contains("src/main.rs"));
    assert!(!output.contains("private reasoning"));
    let row = json!({"type":"user","message":{"content":[
        {"type":"text","text":"Look"}, {"type":"image","source":{"data":"base64"}},
        {"type":"tool_result","content":[{"type":"text","text":"file contents"}]}
    ]}});
    let output = parse("claude", &row).unwrap();
    assert!(output.contains("You: Look"));
    assert!(output.contains("You: [image]"));
    assert!(output.contains("Result: file contents"));
    assert!(!output.contains("base64"));
}

#[test]
fn codex_uses_response_items_without_event_mirrors_or_instructions() {
    assert_eq!(parse("codex", &json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}})).as_deref(), Some("You: Hello"));
    assert_eq!(parse("codex", &json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi"}]}})).as_deref(), Some("Assistant: Hi"));
    assert!(
        parse(
            "codex",
            &json!({"type":"event_msg","payload":{"type":"agent_message","message":"Hi"}})
        )
        .is_none()
    );
    assert!(parse("codex", &json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"instructions"}]}})).is_none());
    for kind in ["function_call", "custom_tool_call"] {
        let text = parse(
            "codex",
            &json!({"type":"response_item","payload":{"type":kind,"name":"exec","arguments":"ls"}}),
        )
        .unwrap();
        assert!(text.contains("Tool: exec"));
        assert!(text.contains("Input: ls"));
    }
    for kind in ["function_call_output", "custom_tool_call_output"] {
        assert_eq!(
            parse(
                "codex",
                &json!({"type":"response_item","payload":{"type":kind,"output":"ok"}})
            )
            .as_deref(),
            Some("Result: ok")
        );
    }
}

#[test]
fn resets_after_truncate_regrow_and_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, message("1", "old")).unwrap();
    let source = source(&path, "claude");
    let mut reader = Reader::default();
    reader.preview(&source);
    std::fs::write(&path, message("2", "replacement longer than old")).unwrap();
    let text = reader.preview(&source);
    assert!(text.contains("replacement longer"));
    assert!(!text.contains("Assistant: old"));
    std::fs::write(&path, "").unwrap();
    assert!(reader.preview(&source).contains("No conversation messages"));
    std::fs::remove_file(&path).unwrap();
    assert!(reader.preview(&source).contains("file is missing"));
    std::fs::write(&path, message("3", "new file")).unwrap();
    assert!(reader.preview(&source).contains("Assistant: new file"));
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
    let text = reader.preview(&source);
    assert!(text.contains("Assistant: message-99"));
    assert!(!text.contains("Assistant: message-0\n"));
    assert_eq!(reader.entries.len(), ENTRY_LIMIT);
    assert!(reader.pending.len() <= RECORD_LIMIT);
    // An oversized record arriving over several polls is discarded through its
    // newline; the next valid record must still be consumed.
    append_file(&path, &vec![b'x'; RECORD_LIMIT]);
    reader.preview(&source);
    append_file(&path, b"x");
    reader.preview(&source);
    assert!(reader.pending.is_empty());
    append_file(
        &path,
        format!("\n{}", message("last", "Recovered")).as_bytes(),
    );
    assert!(reader.preview(&source).contains("Assistant: Recovered"));
}

#[test]
fn bounds_text_and_removes_terminal_controls() {
    let raw = format!(
        "\x1b]52;c;clipboard\x07\u{009b}31m\r{}",
        "界".repeat(TEXT_LIMIT * 2)
    );
    let output = parse("claude", &json!({"type":"user","message":{"content":raw}})).unwrap();
    assert!(output.chars().count() <= TEXT_LIMIT);
    assert!(!output.chars().any(char::is_control));
    assert!(output.ends_with('…'));
}

#[test]
fn discovers_only_transcripts_with_matching_session_identity() {
    let dir = tempfile::tempdir().unwrap();
    for provider in ["claude", "codex"] {
        let path = dir.path().join(if provider == "claude" {
            "s.jsonl"
        } else {
            "rollout-date-s.jsonl"
        });
        let source = Source {
            provider: provider.into(),
            session_id: "s".into(),
            path: None,
            root: Some(dir.path().into()),
        };
        std::fs::write(&path, "{\"sessionId\":\"other\"}\n").unwrap();
        assert!(locate(&source).is_none());
        let header = if provider == "claude" {
            json!({"sessionId":"s"})
        } else {
            json!({"type":"session_meta","payload":{"id":"s"}})
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
            .preview(&source)
            .contains("Unable to read transcript")
    );
    source.path = None;
    assert!(
        reader
            .preview(&source)
            .contains("waiting for a session hook")
    );
    source.path = Some(dir.path().join("missing").to_string_lossy().into_owned());
    assert!(reader.preview(&source).contains("file is missing"));
}

#[test]
fn supports_covers_exactly_the_parseable_providers() {
    assert!(supports("claude"));
    assert!(supports("codex"));
    for provider in ["grok", "opencode", "pi", "unknown"] {
        assert!(!supports(provider), "{provider}");
    }
}
