//! Message the session that delegated to the calling agent, without the
//! caller having to know its id.
use anyhow::{Context as _, Result};

use crate::db;
use crate::paths::{Context as AppContext, DELEGATOR_ENV};
use crate::self_cmd::{self, Identity};
use crate::send;
use crate::store::ListedSession;
use crate::wait::Outcome;

/// Sends the message to the caller's delegator exactly as `send` would.
pub fn run(
    ctx: &AppContext,
    message: &[String],
    options: send::Options,
) -> Result<Option<Outcome>> {
    let target = delegator(ctx)?;
    send::run(ctx, &target, message, options)
}

/// The caller's delegator: the window environment `new` set, or else the
/// delegator the caller's own session reported.
fn delegator(ctx: &AppContext) -> Result<String> {
    if let Some(id) = std::env::var(DELEGATOR_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
    {
        return Ok(id.trim().to_string());
    }
    let this = self_cmd::identity(ctx).with_context(|| {
        format!("no delegator: {DELEGATOR_ENV} is unset and this is not a tracked agent session")
    })?;
    stored_delegator(&db::query_stored(ctx)?, &this)
}

fn stored_delegator(sessions: &[ListedSession], this: &Identity) -> Result<String> {
    let session = sessions
        .iter()
        .find(|session| session.provider == this.provider && session.session_id == this.session_id)
        .with_context(|| {
            format!(
                "no delegator: {DELEGATOR_ENV} is unset and {} {} has not reported yet",
                this.provider,
                this.id()
            )
        })?;
    session.delegator.clone().with_context(|| {
        format!(
            "no delegator: {DELEGATOR_ENV} is unset and {} {} was not started by `agent-berth new`",
            this.provider,
            this.id()
        )
    })
}

#[cfg(test)]
#[path = "reply_tests.rs"]
mod tests;
