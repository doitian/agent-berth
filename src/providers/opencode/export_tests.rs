use super::*;
use crate::transcript::tests_support::rendered;
use serde_json::json;

#[test]
fn renders_opencode_messages_and_skips_reasoning() {
    let export = json!({
        "info": {"id": "s1"},
        "messages": [
            {"type": "user", "text": "Hello"},
            {"type": "system", "text": "tools changed"},
            {"type": "assistant", "content": [
                {"type": "reasoning", "text": "private"},
                {"type": "text", "text": "Hi there"},
                {"type": "tool", "name": "bash", "state": {
                    "status": "completed",
                    "input": {"command": "ls"},
                    "content": [{"type": "text", "text": "file.txt"}]
                }}
            ]}
        ]
    });
    let text = rendered(&conversation(&export));
    assert!(text.contains("You: Hello"), "{text}");
    assert!(text.contains("Assistant: Hi there"), "{text}");
    assert!(text.contains("Tool: bash"), "{text}");
    assert!(text.contains(r#"Input: {"command":"ls"}"#), "{text}");
    assert!(text.contains("Result: file.txt"), "{text}");
    assert!(!text.contains("private"), "{text}");
    assert!(!text.contains("tools changed"), "{text}");
}

#[test]
fn renders_opencode_tool_output_fallback_and_strips_controls() {
    let export = json!({
        "messages": [
            {"type": "assistant", "content": [
                {"type": "tool", "name": "exec", "state": {"output": "ok\x1b[31m"}}
            ]}
        ]
    });
    let text = rendered(&conversation(&export));
    assert!(text.contains("Result: ok"), "{text}");
    assert!(!text.contains('\x1b'), "{text:?}");
}

#[test]
fn renders_no_messages_as_empty() {
    assert_eq!(rendered(&conversation(&json!({"messages": []}))), "");
    assert_eq!(rendered(&conversation(&json!({}))), "");
}
