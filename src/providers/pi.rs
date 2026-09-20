use std::path::PathBuf;

use anyhow::Result;

use super::write_text;
use crate::paths::Context;

pub fn install(ctx: &Context) -> Result<PathBuf> {
    let dest = ctx.pi_dir.join("extensions").join("agent-berth.ts");
    write_text(&dest, &extension_source(ctx))?;
    Ok(dest)
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
