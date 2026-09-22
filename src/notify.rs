use std::io;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::ipc;
use crate::paths::Context as AppContext;

pub fn run(ctx: &AppContext, provider: String) -> Result<()> {
    let payload: Value = serde_json::from_reader(io::stdin().lock())
        .context("notify payload must be a JSON object on stdin")?;
    send_payload(ctx, provider, payload)
}

pub fn send_payload(ctx: &AppContext, provider: String, mut payload: Value) -> Result<()> {
    if provider == "codex" && payload.is_object() {
        enrich_codex_originator(
            &mut payload,
            std::env::var("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
                .ok()
                .as_deref(),
        );
    }
    if provider == "codex"
        && payload.is_object()
        && crate::status::u32_field(&payload, &["pid"])
            .filter(|&pid| pid != 0)
            .is_none()
        && let Some(pid) = crate::process::ancestor_named("codex")
    {
        payload["pid"] = pid.into();
    }
    match ipc::notify(ctx, provider, payload) {
        Ok(()) => Ok(()),
        Err(_) => Ok(()),
    }
}

fn enrich_codex_originator(payload: &mut Value, originator: Option<&str>) {
    // Capture this in the hook process: the long-lived berth server has a
    // different environment, and rollout metadata predates client switches.
    if crate::status::string_field(payload, &["originator"]).is_none() {
        payload["originator"] = originator
            .filter(|value| !value.is_empty())
            .unwrap_or("codex-cli")
            .into();
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
