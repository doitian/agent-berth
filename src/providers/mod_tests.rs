use super::*;
use crate::paths::Context;
use crate::status::AgentStatus;
use tempfile::tempdir;

fn emit(
    provider: &str,
    sessions: &mut std::collections::BTreeMap<String, AgentSession>,
    name: &str,
) {
    apply_hook(
        provider,
        sessions,
        &serde_json::json!({"session_id":"s","hook_event_name":name}),
    );
}

#[test]
fn hook_providers_share_turn_lifecycle() {
    for provider in ["claude", "codex", "grok"] {
        let mut sessions = std::collections::BTreeMap::new();
        emit(provider, &mut sessions, "SessionStart");
        emit(provider, &mut sessions, "PostToolUse");
        assert!(sessions.is_empty(), "{provider}");
        emit(provider, &mut sessions, "UserPromptSubmit");
        emit(provider, &mut sessions, "PermissionRequest");
        emit(provider, &mut sessions, "PreToolUse");
        assert_eq!(sessions["s"].status, AgentStatus::Waiting, "{provider}");
        emit(provider, &mut sessions, "PostToolUse");
        assert_eq!(sessions["s"].status, AgentStatus::Running, "{provider}");
        emit(provider, &mut sessions, "Stop");
        emit(provider, &mut sessions, "PostToolUse");
        emit(provider, &mut sessions, "PreToolUse");
        emit(provider, &mut sessions, "Stop");
        assert_eq!(sessions["s"].status, AgentStatus::Done, "{provider}");
        emit(provider, &mut sessions, "UserPromptSubmit");
        assert_eq!(sessions["s"].status, AgentStatus::Running, "{provider}");
        emit(provider, &mut sessions, "SessionEnd");
        emit(provider, &mut sessions, "PostToolUse");
        assert!(sessions.is_empty(), "{provider}");
    }
}

#[test]
fn claude_and_codex_track_child_sessions() {
    for provider in ["claude", "codex"] {
        let mut sessions = std::collections::BTreeMap::new();
        apply_hook(
            provider,
            &mut sessions,
            &serde_json::json!({"session_id":"parent","hook_event_name":"UserPromptSubmit"}),
        );
        apply_hook(
            provider,
            &mut sessions,
            &serde_json::json!({
                "session_id":"parent",
                "agent_id":"child",
                "hook_event_name":"PermissionRequest"
            }),
        );
        assert_eq!(
            sessions["parent"].status,
            AgentStatus::Running,
            "{provider}"
        );
        assert_eq!(
            sessions["parent:child"].status,
            AgentStatus::Waiting,
            "{provider}"
        );
        apply_hook(
            provider,
            &mut sessions,
            &serde_json::json!({
                "session_id":"parent",
                "agent_id":"child",
                "hook_event_name":"Stop"
            }),
        );
        assert_eq!(sessions.keys().cloned().collect::<Vec<_>>(), ["parent"]);
    }
}

#[test]
fn codex_subagent_stop_without_id_leaves_parent() {
    let mut sessions = std::collections::BTreeMap::new();
    apply_hook(
        "codex",
        &mut sessions,
        &serde_json::json!({"session_id":"s","hook_event_name":"UserPromptSubmit"}),
    );
    apply_hook(
        "codex",
        &mut sessions,
        &serde_json::json!({"session_id":"s","hook_event_name":"SubagentStop"}),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);
}

#[test]
fn grok_elicitation_is_waiting_and_ignores_subagents() {
    let mut sessions = std::collections::BTreeMap::new();
    apply_hook(
        "grok",
        &mut sessions,
        &serde_json::json!({
            "sessionId":"s",
            "hookEventName":"notification",
            "notificationType":"elicitation_dialog"
        }),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Waiting);
    apply_hook(
        "grok",
        &mut sessions,
        &serde_json::json!({
            "sessionId":"child",
            "hookEventName":"UserPromptSubmit",
            "subagentType":"explore"
        }),
    );
    assert!(!sessions.contains_key("child"));
}

#[test]
fn claude_desktop_entrypoint_and_background_stop() {
    let mut sessions = std::collections::BTreeMap::new();
    apply_hook(
        "claude",
        &mut sessions,
        &serde_json::json!({
            "session_id":"s",
            "hook_event_name":"UserPromptSubmit",
            "entrypoint":"claude-desktop"
        }),
    );
    assert_eq!(sessions["s"].source, crate::status::Source::Desktop);
    apply_hook(
        "claude",
        &mut sessions,
        &serde_json::json!({
            "session_id":"s",
            "hook_event_name":"Stop",
            "background_tasks":[{"status":"busy"}]
        }),
    );
    assert_eq!(sessions["s"].status, AgentStatus::Running);
}

#[test]
fn install_and_uninstall_are_idempotent() {
    let root = tempdir().unwrap();
    let ctx = Context::for_test(root.path(), &root.path().join("agent-berth"));
    for provider in ProviderKind::ALL {
        let path = provider.install(&ctx).unwrap();
        assert!(path.exists(), "{}", provider.name());
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("notify --provider") || text.contains("agent-berth"),
            "{}",
            provider.name()
        );
        provider.install(&ctx).unwrap();
        provider.uninstall(&ctx).unwrap();
        provider.uninstall(&ctx).unwrap();
    }
    let claude =
        fs::read_to_string(ctx.claude_config_dir.join("settings.json")).unwrap_or_default();
    assert!(!claude.contains("notify --provider claude"));
    assert!(!ctx.opencode_plugin_dir().join("agent-berth.js").exists());
    assert!(!ctx.opencode_plugin_dir().join("agent-berth-v2.js").exists());
    assert!(
        !ctx.pi_dir
            .join("extensions")
            .join("agent-berth.ts")
            .exists()
    );
    assert!(
        !ctx.grok_home
            .join("hooks")
            .join("agent-berth.json")
            .exists()
    );
}

#[test]
fn resume_commands_match_providers() {
    assert_eq!(resume_cmd("claude", "abc"), ["claude", "--resume", "abc"]);
    assert_eq!(resume_cmd("codex", "abc"), ["codex", "resume", "abc"]);
    assert_eq!(resume_cmd("grok", "abc"), ["grok", "--resume", "abc"]);
    assert_eq!(
        resume_cmd("opencode", "abc"),
        ["opencode", "--session", "abc"]
    );
    assert_eq!(resume_cmd("pi", "abc"), ["pi", "--session", "abc"]);
}

#[test]
fn freshly_installed_hooks_are_never_outdated() {
    let root = tempdir().unwrap();
    let bin = root.path().join("agent-berth");
    fs::write(&bin, []).unwrap();
    let ctx = Context::for_test(root.path(), &bin);
    for provider in ProviderKind::ALL {
        provider.install(&ctx).unwrap();
        assert!(
            !provider.hooks_outdated(&ctx),
            "{} reported outdated right after install",
            provider.name()
        );
    }
}
