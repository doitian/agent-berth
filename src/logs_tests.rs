use super::*;
use serde_json::json;

#[test]
fn tail_keeps_the_last_lines() {
    assert_eq!(tail_lines("a\nb\nc", 2), "b\nc");
    assert_eq!(tail_lines("a\nb\nc", 5), "a\nb\nc");
    assert_eq!(tail_lines("a\nb\nc", 0), "");
}

#[test]
fn tail_ignores_a_trailing_newline() {
    assert_eq!(tail_lines("a\nb\nc\n", 1), "c");
}

#[test]
fn tail_of_empty_text_is_empty() {
    assert_eq!(tail_lines("", 3), "");
}

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
    let text = render_opencode(&export);
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
    let text = render_opencode(&export);
    assert!(text.contains("Result: ok"), "{text}");
    assert!(!text.contains('\x1b'), "{text:?}");
}

#[test]
fn renders_no_messages_as_empty() {
    assert_eq!(render_opencode(&json!({"messages": []})), "");
    assert_eq!(render_opencode(&json!({})), "");
}

#[test]
fn transcript_root_maps_local_providers_only() {
    let ctx = crate::paths::Context::for_test(
        std::path::Path::new("/root"),
        std::path::Path::new("/bin/agent-berth"),
    );
    assert_eq!(
        transcript_root(&ctx, "claude"),
        Some(ctx.claude_config_dir.join("projects"))
    );
    assert_eq!(
        transcript_root(&ctx, "codex"),
        Some(ctx.codex_home.join("sessions"))
    );
    assert_eq!(
        transcript_root(&ctx, "pi"),
        Some(ctx.pi_dir.join("sessions"))
    );
    assert_eq!(transcript_root(&ctx, "opencode"), None);
}
