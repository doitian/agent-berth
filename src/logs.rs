use anyhow::{Result, anyhow};

use crate::db;
use crate::hubs::{self, Tmux};
use crate::paths::Context as AppContext;
use crate::pick;
use crate::providers;
use crate::store::ListedSession;
use crate::transcript::Conversation;

pub fn run(ctx: &AppContext, selector: &str, tail: Option<usize>) -> Result<()> {
    let sessions = db::query_stored(ctx)?;
    let Some(session) = pick::single(&sessions, selector)? else {
        return Ok(());
    };
    let full = transcript_for(ctx, &session)?.render();
    let text = match tail {
        Some(lines) => tail_lines(&full, lines),
        None => full,
    };
    if !text.is_empty() {
        println!("{text}");
    }
    Ok(())
}

/// Best available transcript: the provider's own source when it has one,
/// otherwise the session's hub, such as its tmux pane.
fn transcript_for(ctx: &AppContext, session: &ListedSession) -> Result<Conversation> {
    let hub = hubs::resolve(session, Tmux::lookup);
    providers::transcript(ctx, hub.as_ref(), session).map_err(|error| {
        anyhow!(
            "no transcript for {} {}: {error:#}",
            session.provider,
            session.session_id
        )
    })
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
