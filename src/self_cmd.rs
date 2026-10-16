//! The calling agent's own identity: its stable session id, derived from the
//! provider's shell-tool environment when it exposes one, or from the
//! enclosing tmux pane otherwise.
use anyhow::{Context as _, Result, bail};

use crate::hubs::tmux::showing_pane;
use crate::paths::Context;
use crate::store::{ListedSession, display_id};
use crate::tmux::{self, Pane};
use crate::{db, list, providers};

struct SelfSession {
    provider: String,
    session_id: String,
}

pub fn run(ctx: &Context, json: bool) -> Result<()> {
    let this = match from_env(env_value) {
        Some(this) => this,
        None => from_current_pane(ctx)?,
    };
    if !json {
        println!("{}", display_id(&this.provider, &this.session_id));
        return Ok(());
    }
    let sessions = db::query_stored(ctx)?;
    let session = sessions
        .iter()
        .find(|session| session.provider == this.provider && session.session_id == this.session_id)
        .with_context(|| {
            format!(
                "{} session {} has not reported to agent-berth yet",
                this.provider, this.session_id
            )
        })?;
    let panes = if tmux::available() {
        tmux::list_panes(true).unwrap_or_default()
    } else {
        Vec::new()
    };
    println!("{}", list::to_json_object(session, &panes)?);
    Ok(())
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Provider and native session id when the agent exposes them to its shell
/// tool, such as Pi's `PI_SESSION_ID`.
fn from_env(get: impl Fn(&str) -> Option<String>) -> Option<SelfSession> {
    providers::all().iter().find_map(|provider| {
        let session_id = get(provider.session_env()?).filter(|id| !id.is_empty())?;
        Some(SelfSession {
            provider: provider.name().to_string(),
            session_id,
        })
    })
}

/// Identity of the session the current tmux pane shows right now.
fn from_current_pane(ctx: &Context) -> Result<SelfSession> {
    let pane_id = env_value("TMUX_PANE").context(
        "not inside a known agent session: no provider session id in the environment and $TMUX_PANE is unset",
    )?;
    let sessions = db::query_stored(ctx)?;
    let panes = tmux::list_panes(true)?;
    let mut showing = in_pane(&sessions, &panes, pane_id.trim());
    match showing.len() {
        0 => bail!("no agent session is running in tmux pane {pane_id}"),
        1 => Ok(showing.remove(0)),
        _ => bail!(
            "tmux pane {pane_id} shows several sessions; pick one with `agent-berth search --tmux-pane {pane_id}`"
        ),
    }
}

/// Sessions the pane shows now, newest first, as provider/id pairs.
fn in_pane(sessions: &[ListedSession], panes: &[Pane], pane_id: &str) -> Vec<SelfSession> {
    let me = std::process::id();
    let mut showing: Vec<&ListedSession> = sessions
        .iter()
        .filter(|session| showing_pane(session, panes, me).is_some_and(|pane| pane.id == pane_id))
        .collect();
    showing.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    showing
        .into_iter()
        .map(|session| SelfSession {
            provider: session.provider.clone(),
            session_id: session.session_id.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "self_cmd_tests.rs"]
mod tests;
