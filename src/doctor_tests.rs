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
