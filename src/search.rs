use std::time::Duration;

use anyhow::Result;

use crate::hubs::tmux::showing_pane;
use crate::paths::Context;
use crate::store::{ListedSession, now_ms};
use crate::tmux::{self, Pane};
use crate::{db, list, pick};

pub fn run(
    ctx: &Context,
    patterns: &[String],
    created_in: Option<Duration>,
    tmux_pane: Option<&str>,
    json: bool,
) -> Result<()> {
    let mut sessions = filter(db::query_stored(ctx)?, patterns, created_in, now_ms());
    let panes = match tmux_pane {
        Some(pane_id) => {
            let panes = tmux::list_panes(true)?;
            sessions = showing(sessions, &panes, pane_id.trim(), std::process::id());
            Some(panes)
        }
        None => None,
    };
    if json {
        let json = match &panes {
            Some(panes) => list::to_json_with_panes(&sessions, panes)?,
            None => list::to_json(&sessions)?,
        };
        println!("{json}");
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

/// Sessions that the tmux pane `pane_id` shows right now. A session hosted
/// there only as a background tab, or one that has ended, is left out.
fn showing(
    sessions: Vec<ListedSession>,
    panes: &[Pane],
    pane_id: &str,
    me: u32,
) -> Vec<ListedSession> {
    sessions
        .into_iter()
        .filter(|session| showing_pane(session, panes, me).is_some_and(|pane| pane.id == pane_id))
        .collect()
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
