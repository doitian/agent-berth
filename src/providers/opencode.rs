use std::path::PathBuf;

use anyhow::Result;

use super::write_text;
use crate::paths::Context;

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.opencode_plugin_dir().join("agent-berth.js");
    write_text(&dest, &plugin_source(ctx))?;
    Ok(dest)
}

pub fn uninstall(ctx: &Context) -> Result<()> {
    let dest = ctx.opencode_plugin_dir().join("agent-berth.js");
    if dest.exists() {
        std::fs::remove_file(dest)?;
    }
    Ok(())
}

fn plugin_source(ctx: &Context) -> String {
    include_str!("hooks/opencode.js").replace(
        "__AGENT_BERTH_BIN__",
        &serde_json::to_string(&ctx.berth_bin.display().to_string()).unwrap(),
    )
}
