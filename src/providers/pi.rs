use std::path::PathBuf;

use anyhow::Result;

use super::write_text;
use crate::paths::Context;

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.pi_dir.join("extensions").join("agent-berth.ts");
    write_text(&dest, &extension_source(ctx))?;
    Ok(dest)
}

pub fn outdated(ctx: &Context) -> bool {
    let dest = ctx.pi_dir.join("extensions").join("agent-berth.ts");
    std::fs::read_to_string(&dest).is_ok_and(|text| text != extension_source(ctx))
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    let dest = ctx.pi_dir.join("extensions").join("agent-berth.ts");
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    Ok(())
}

fn extension_source(ctx: &Context) -> String {
    include_str!("hooks/pi.ts").replace(
        "__AGENT_BERTH_BIN__",
        &serde_json::to_string(&ctx.berth_bin.display().to_string()).unwrap(),
    )
}

/// Pi expands `/skill:name` into a `<skill name=… location=…>…</skill>` user
/// message, which the plugin reports as the session title. Show the slash
/// command instead of the raw block.
pub fn normalize_title(title: &str) -> String {
    let Some(rest) = title.strip_prefix("<skill name=\"") else {
        return title.to_string();
    };
    let Some(end) = rest.find('"') else {
        return title.to_string();
    };
    let name = &rest[..end];
    if name.is_empty() || !rest[end..].starts_with("\" location=\"") {
        return title.to_string();
    }
    let Some(close) = rest.find("</skill>") else {
        return title.to_string();
    };
    let tail = rest[close + "</skill>".len()..].trim();
    if tail.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {tail}")
    }
}

#[cfg(test)]
#[path = "pi_tests.rs"]
mod tests;
