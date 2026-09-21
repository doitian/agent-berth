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

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
