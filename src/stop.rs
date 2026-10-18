//! End a delegated agent by killing the tmux pane that shows it. The session
//! stays stored and searchable, and its transcript stays readable.
use anyhow::{Result, bail};

use crate::db;
use crate::hubs::tmux::{running_pane, showing_pane};
use crate::paths::Context as AppContext;
use crate::pick;
use crate::status::Source;
use crate::store::ListedSession;
use crate::tmux::{self, Pane};

pub fn run(ctx: &AppContext, id: &str, force: bool, dry_run: bool) -> Result<()> {
    let sessions = db::query_stored(ctx)?;
    let session = pick::by_id(&sessions, id)?;
    let panes = tmux::list_panes(true)?;
    let pane = target(session, &sessions, &panes, std::process::id(), force)?;
    let what = format!(
        "tmux pane {} showing {} {}",
        pane.id,
        session.provider,
        session.id()
    );
    if dry_run {
        println!("would kill {what}");
        return Ok(());
    }
    tmux::kill_pane(&pane.id)?;
    println!("killed {what}");
    Ok(())
}

/// The pane to kill for the session, or why it must not be: the session
/// shows in no pane, or the pane also carries other sessions that would end
/// with it and `force` was not given.
fn target<'a>(
    session: &ListedSession,
    sessions: &[ListedSession],
    panes: &'a [Pane],
    me: u32,
    force: bool,
) -> Result<&'a Pane> {
    let shown = (!session.exited)
        .then(|| showing_pane(session, panes, me))
        .flatten();
    let Some(pane) = shown else {
        bail!("{}", no_pane(session));
    };
    let others: Vec<String> = sessions
        .iter()
        .filter(|other| {
            (other.provider != session.provider || other.session_id != session.session_id)
                && other.parent_id.is_none()
                && running_pane(other, panes, me).is_some_and(|theirs| theirs.id == pane.id)
        })
        .map(|other| format!("{} {}", other.provider, other.id()))
        .collect();
    if !others.is_empty() && !force {
        bail!(
            "tmux pane {} also hosts {}; pass --force to end them too",
            pane.id,
            others.join(", ")
        );
    }
    Ok(pane)
}

fn no_pane(session: &ListedSession) -> String {
    let name = format!("{} {}", session.provider, session.id());
    if session.exited {
        format!("{name} has already exited")
    } else if session.paseo_agent_id.is_some() {
        format!("{name} runs in Paseo, not in a tmux pane")
    } else if session.source == Source::Desktop {
        format!("{name} is a desktop session, not in a tmux pane")
    } else {
        format!("{name} is not showing in any tmux pane")
    }
}

#[cfg(test)]
#[path = "stop_tests.rs"]
mod tests;
