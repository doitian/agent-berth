use std::time::Duration;

use anyhow::Result;

use crate::paths::Context;
use crate::store::{ListedSession, now_ms};
use crate::{db, list, pick};

pub fn run(
    ctx: &Context,
    patterns: &[String],
    created_in: Option<Duration>,
    json: bool,
) -> Result<()> {
    let sessions = filter(db::query_stored(ctx)?, patterns, created_in, now_ms());
    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
    } else if sessions.is_empty() {
        println!("No matching sessions.");
    } else {
        list::print_table(&sessions, true)?;
    }
    Ok(())
}

fn filter(
    mut sessions: Vec<ListedSession>,
    patterns: &[String],
    created_in: Option<Duration>,
    now: u64,
) -> Vec<ListedSession> {
    if let Some(window) = created_in {
        let cutoff = u128::from(now).saturating_sub(window.as_millis()) as u64;
        sessions.retain(|session| session.created_ms >= cutoff && session.created_ms <= now);
    }
    let mut sessions = pick::filter(&sessions, patterns);
    sessions.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| (&a.provider, &a.session_id).cmp(&(&b.provider, &b.session_id)))
    });
    sessions
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
