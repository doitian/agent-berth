use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;

use super::write_text;
use crate::paths::Context;

const V1_PLUGIN: &str = "agent-berth.js";
const V2_PLUGIN: &str = "agent-berth-v2";
const V2_ENTRY: &str = "index.js";
const V2_TUI: &str = "tui.js";
const V2_MANIFEST: &str = "package.json";
// Earlier builds installed the v2 plugin as a single file.
const V2_LEGACY_PLUGIN: &str = "agent-berth-v2.js";

pub fn install(ctx: &Context) -> Result<PathBuf> {
    install_for(ctx, opencode_major().unwrap_or(1))
}

pub fn outdated(ctx: &Context) -> bool {
    let major = opencode_major().unwrap_or(1);
    inactive_plugins(major)
        .iter()
        .any(|name| plugin_path(ctx, name).exists())
        || source_differs(ctx, major)
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    remove_plugin(ctx, V1_PLUGIN)?;
    remove_plugin(ctx, V2_PLUGIN)?;
    remove_plugin(ctx, V2_LEGACY_PLUGIN)?;
    Ok(())
}

pub(crate) fn hook_path(ctx: &Context) -> PathBuf {
    if opencode_major().unwrap_or(1) >= 2 {
        plugin_path(ctx, V2_PLUGIN).join(V2_ENTRY)
    } else {
        plugin_path(ctx, V1_PLUGIN)
    }
}

fn install_for(ctx: &Context, major: u32) -> Result<PathBuf> {
    // OpenCode 2 rejects the v1 default-function plugin. Leave that file for
    // v1, and install a separate plugin package when `opencode --version` is
    // 2 or newer. The package holds a server-side reporter plus a TUI part
    // that reports the pane process and its front tab.
    for name in inactive_plugins(major) {
        remove_plugin(ctx, name)?;
    }
    if major >= 2 {
        let dir = plugin_path(ctx, V2_PLUGIN);
        write_text(
            &dir.join(V2_MANIFEST),
            include_str!("hooks/opencode-v2/package.json"),
        )?;
        write_text(&dir.join(V2_ENTRY), &source_file(V2_ENTRY, ctx))?;
        write_text(&dir.join(V2_TUI), &source_file(V2_TUI, ctx))?;
        Ok(dir.join(V2_ENTRY))
    } else {
        let dest = plugin_path(ctx, V1_PLUGIN);
        write_text(&dest, &source_file("opencode.js", ctx))?;
        Ok(dest)
    }
}

/// Plugin files for a version that `setup` must remove, including the single
/// file an earlier build left behind.
fn inactive_plugins(major: u32) -> &'static [&'static str] {
    if major >= 2 {
        &[V1_PLUGIN, V2_LEGACY_PLUGIN]
    } else {
        &[V2_PLUGIN, V2_LEGACY_PLUGIN]
    }
}

fn plugin_for_major(major: u32) -> &'static str {
    if major >= 2 { V2_PLUGIN } else { V1_PLUGIN }
}

fn source_differs(ctx: &Context, major: u32) -> bool {
    if major >= 2 {
        let dir = plugin_path(ctx, V2_PLUGIN);
        file_differs(
            &dir.join(V2_MANIFEST),
            include_str!("hooks/opencode-v2/package.json"),
        ) || file_differs(&dir.join(V2_ENTRY), &source_file(V2_ENTRY, ctx))
            || file_differs(&dir.join(V2_TUI), &source_file(V2_TUI, ctx))
    } else {
        file_differs(
            &plugin_path(ctx, V1_PLUGIN),
            &source_file("opencode.js", ctx),
        )
    }
}

fn file_differs(path: &Path, expected: &str) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => text != expected,
        Err(_) => true,
    }
}

fn remove_plugin(ctx: &Context, name: &str) -> Result<()> {
    let dest = plugin_path(ctx, name);
    if dest.is_dir() {
        std::fs::remove_dir_all(&dest)?;
    } else if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    Ok(())
}

fn plugin_path(ctx: &Context, name: &str) -> PathBuf {
    ctx.opencode_plugin_dir().join(name)
}

fn source_file(entry: &str, ctx: &Context) -> String {
    let template = match entry {
        V2_ENTRY => include_str!("hooks/opencode-v2/index.js"),
        V2_TUI => include_str!("hooks/opencode-v2/tui.js"),
        _ => include_str!("hooks/opencode.js"),
    };
    template.replace(
        "__AGENT_BERTH_BIN__",
        &serde_json::to_string(&ctx.berth_bin.display().to_string()).unwrap(),
    )
}

/// A plugin file exists for a different OpenCode major version than the one
/// on PATH, and no plugin for the detected version is installed.
pub fn version_mismatch(ctx: &Context) -> Option<String> {
    mismatch_detail(ctx, opencode_major()?)
}

fn mismatch_detail(ctx: &Context, major: u32) -> Option<String> {
    let active = if major >= 2 {
        plugin_path(ctx, V2_PLUGIN).join(V2_ENTRY)
    } else {
        plugin_path(ctx, V1_PLUGIN)
    };
    if active.is_file() {
        return None;
    }
    let other = if major >= 2 { 1 } else { 2 };
    let unmatched = plugin_for_major(other);
    if !plugin_path(ctx, unmatched).exists() {
        return None;
    }
    Some(format!(
        "plugin {unmatched} targets opencode {other}, but opencode {major} is installed"
    ))
}

fn opencode_major() -> Option<u32> {
    static CACHE: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *CACHE.get_or_init(detect_opencode_major)
}

fn detect_opencode_major() -> Option<u32> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(opencode_version_output());
    });
    let output = rx.recv_timeout(Duration::from_secs(5)).ok()?.ok()?;
    parse_opencode_major(&String::from_utf8_lossy(&output.stdout))
        .or_else(|| parse_opencode_major(&String::from_utf8_lossy(&output.stderr)))
}

fn opencode_version_output() -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new("opencode");
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.output()
}

fn parse_opencode_major(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            if index + 1 < bytes.len() && bytes[index] == b'.' && bytes[index + 1].is_ascii_digit()
            {
                return std::str::from_utf8(&bytes[start..index]).ok()?.parse().ok();
            }
        } else {
            index += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_opencode_major() {
        assert_eq!(parse_opencode_major("opencode v2.0.18\n"), Some(2));
        assert_eq!(parse_opencode_major("1.18.32"), Some(1));
        assert_eq!(parse_opencode_major("v2.0.0-beta.1"), Some(2));
        assert_eq!(parse_opencode_major("local\n"), None);
        assert_eq!(parse_opencode_major(""), None);
    }

    #[test]
    fn detects_plugin_version_mismatch() {
        let root = tempdir().unwrap();
        let bin = root.path().join("agent-berth");
        let ctx = Context::for_test(root.path(), &bin);
        assert_eq!(mismatch_detail(&ctx, 2), None);

        install_for(&ctx, 1).unwrap();
        let detail = mismatch_detail(&ctx, 2).unwrap();
        assert!(
            detail.contains("agent-berth.js targets opencode 1"),
            "{detail}"
        );
        assert!(detail.contains("opencode 2 is installed"), "{detail}");
        assert_eq!(mismatch_detail(&ctx, 1), None);

        install_for(&ctx, 2).unwrap();
        let detail = mismatch_detail(&ctx, 1).unwrap();
        assert!(
            detail.contains("agent-berth-v2 targets opencode 2"),
            "{detail}"
        );
        assert_eq!(mismatch_detail(&ctx, 2), None);
    }

    #[test]
    fn detects_the_legacy_single_file_plugin() {
        let root = tempdir().unwrap();
        let bin = root.path().join("agent-berth");
        let ctx = Context::for_test(root.path(), &bin);
        std::fs::create_dir_all(ctx.opencode_plugin_dir()).unwrap();
        std::fs::write(plugin_path(&ctx, V2_LEGACY_PLUGIN), "stale").unwrap();
        // The legacy file is not a version mismatch; doctor reports the
        // missing v2 entry instead, and setup replaces the old layout.
        assert_eq!(mismatch_detail(&ctx, 2), None);
        assert!(source_differs(&ctx, 2));

        install_for(&ctx, 2).unwrap();
        assert!(!plugin_path(&ctx, V2_LEGACY_PLUGIN).exists());
        assert!(!source_differs(&ctx, 2));
        assert_eq!(mismatch_detail(&ctx, 2), None);
    }

    #[test]
    fn installs_v1_plugin_or_a_separate_v2_plugin() {
        let root = tempdir().unwrap();
        let bin = root.path().join("agent-berth");
        let ctx = Context::for_test(root.path(), &bin);
        let v1 = install_for(&ctx, 1).unwrap();
        assert_eq!(v1.file_name().unwrap(), "agent-berth.js");
        let v1_text = std::fs::read_to_string(&v1).unwrap();
        assert!(v1_text.contains("export default () =>"));
        assert!(!ctx.opencode_plugin_dir().join(V2_PLUGIN).exists());
        assert!(!ctx.opencode_plugin_dir().join(V2_LEGACY_PLUGIN).exists());

        let v2 = install_for(&ctx, 2).unwrap();
        assert_eq!(v2.file_name().unwrap(), V2_ENTRY);
        let dir = v2.parent().unwrap();
        assert_eq!(dir.file_name().unwrap(), V2_PLUGIN);
        let manifest = std::fs::read_to_string(dir.join(V2_MANIFEST)).unwrap();
        assert!(manifest.contains("\"./tui\": \"./tui.js\""));
        let v2_text = std::fs::read_to_string(&v2).unwrap();
        assert!(v2_text.contains("id: \"agent-berth-v2\""));
        assert!(v2_text.contains("async setup(ctx)"));
        let tui_text = std::fs::read_to_string(dir.join(V2_TUI)).unwrap();
        assert!(tui_text.contains("id: \"agent-berth-v2-tui\""));
        assert!(tui_text.contains("front"));
        assert!(!v1.exists());

        let restored = install_for(&ctx, 1).unwrap();
        assert_eq!(restored.file_name().unwrap(), "agent-berth.js");
        assert!(!v2.exists());
        assert!(!dir.exists());
        let encoded = serde_json::to_string(&bin.display().to_string()).unwrap();
        assert!(
            std::fs::read_to_string(&restored)
                .unwrap()
                .contains(&encoded)
        );
        assert!(v2_text.contains(&encoded));
        assert!(tui_text.contains(&encoded));
    }
}
