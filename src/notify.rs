use std::io;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::ipc;
use crate::paths::{Context as AppContext, DELEGATOR_ENV};
use crate::providers;

pub fn run(ctx: &AppContext, provider: String) -> Result<()> {
    let payload: Value = serde_json::from_reader(io::stdin().lock())
        .context("notify payload must be a JSON object on stdin")?;
    send_payload(ctx, provider, payload)
}

pub fn send_payload(ctx: &AppContext, provider: String, mut payload: Value) -> Result<()> {
    enrich_delegator(&mut payload, std::env::var(DELEGATOR_ENV).ok().as_deref());
    if let Some(hooks) = providers::lookup(&provider).and_then(|p| p.hooks()) {
        hooks.enrich(&mut payload);
    }
    match ipc::notify(ctx, provider, payload) {
        Ok(()) => Ok(()),
        Err(_) => Ok(()),
    }
}

/// Hooks and plugins run inside the agent's process tree, which inherited the
/// window environment `new` set, so this process sees the delegator the
/// long-lived server cannot.
fn enrich_delegator(payload: &mut Value, delegator: Option<&str>) {
    if let Some(id) = delegator.filter(|id| !id.is_empty())
        && payload.is_object()
        && payload.get("delegator").is_none()
    {
        payload["delegator"] = Value::String(id.to_string());
    }
}

#[cfg(test)]
#[path = "notify_tests.rs"]
mod tests;
