use super::*;
use crate::paths::Context as AppContext;
use tempfile::tempdir;

#[test]
fn reports_missing_hooks_for_installed_config() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
    std::fs::create_dir_all(&ctx.claude_config_dir).unwrap();
    let check = provider_check(&ctx, ProviderKind::Claude);
    assert_eq!(check.level, Level::Error);
    assert!(check.detail.contains("hooks missing"));
}

#[test]
fn reports_ok_after_install() {
    let root = tempdir().unwrap();
    let bin = root.path().join("agent-berth");
    std::fs::write(&bin, []).unwrap();
    let ctx = AppContext::for_test(root.path(), &bin);
    ProviderKind::Claude.install(&ctx).unwrap();
    let check = provider_check(&ctx, ProviderKind::Claude);
    assert_eq!(check.level, Level::Ok);
}

#[test]
fn fzf_check_matches_path() {
    let check = fzf_check();
    assert_eq!(check.name, "fzf");
    let expected = if crate::paths::on_path("fzf") {
        Level::Ok
    } else {
        Level::Warn
    };
    assert_eq!(check.level, expected);
}

#[test]
fn skips_agents_without_binary_or_config() {
    let root = tempdir().unwrap();
    let ctx = AppContext::for_test(root.path(), &root.path().join("agent-berth"));
    let check = provider_check(&ctx, ProviderKind::Grok);
    if ProviderKind::Grok.is_installed(&ctx) {
        assert_eq!(check.level, Level::Error);
    } else {
        assert_eq!(check.level, Level::Skip);
    }
}

#[test]
fn reports_outdated_hooks_when_config_predates_this_build() {
    let root = tempdir().unwrap();
    let bin = root.path().join("agent-berth");
    std::fs::write(&bin, []).unwrap();
    let ctx = AppContext::for_test(root.path(), &bin);
    let dest = ProviderKind::Claude.install(&ctx).unwrap();

    // The shape older builds installed: a backgrounded Stop hook.
    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&dest).unwrap()).unwrap();
    data["hooks"]["Stop"][0]["hooks"][0]["async"] = serde_json::Value::Bool(true);
    std::fs::write(&dest, serde_json::to_string_pretty(&data).unwrap()).unwrap();

    let check = provider_check(&ctx, ProviderKind::Claude);
    assert_eq!(check.level, Level::Warn);
    assert!(check.detail.contains("out of date"), "{}", check.detail);

    ProviderKind::Claude.install(&ctx).unwrap();
    assert_eq!(provider_check(&ctx, ProviderKind::Claude).level, Level::Ok);
}

#[test]
fn keeps_unrelated_hooks_out_of_the_outdated_check() {
    let root = tempdir().unwrap();
    let bin = root.path().join("agent-berth");
    std::fs::write(&bin, []).unwrap();
    let ctx = AppContext::for_test(root.path(), &bin);
    let dest = ProviderKind::Claude.install(&ctx).unwrap();

    let mut data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&dest).unwrap()).unwrap();
    data["hooks"]["Stop"][0]["hooks"][0]["async"] = serde_json::Value::Bool(true);
    let ours = data["hooks"]["Stop"].as_array().unwrap()[0].clone();
    data["hooks"]["Stop"] = serde_json::json!([
        {"hooks": [{"type": "command", "command": "echo mine"}]},
        ours,
    ]);
    data["env"] = serde_json::json!({"FOO": "bar"});
    std::fs::write(&dest, serde_json::to_string_pretty(&data).unwrap()).unwrap();

    ProviderKind::Claude.install(&ctx).unwrap();
    assert_eq!(provider_check(&ctx, ProviderKind::Claude).level, Level::Ok);
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&dest).unwrap()).unwrap();
    assert_eq!(data["env"]["FOO"], "bar");
    let stop = data["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "echo mine");
    assert_eq!(stop[1]["hooks"][0]["async"], serde_json::Value::Bool(false));
}
