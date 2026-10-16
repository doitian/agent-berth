//! Sessions running in tmux panes. Every session outside Paseo falls back to
//! this hub, which reaches it only when a pane runs it.
use std::sync::OnceLock;

use anyhow::{Context as _, Result, anyhow, bail};

use super::{Hub, Preview};
use crate::paths::Context as AppContext;
use crate::process;
use crate::status::Source;
use crate::store::ListedSession;
use crate::tmux::{self, Pane};
use crate::transcript::Conversation;

#[derive(Debug)]
pub struct Tmux {
    panes: Panes,
}

#[derive(Debug)]
enum Panes {
    /// The caller already resolved the session's pane.
    Resolved(Option<Located>),
    /// Listed on first use, so a hub that is never asked for a pane runs no
    /// tmux commands. Errors are kept as text to report on every use.
    Lazy(OnceLock<Result<Vec<Pane>, String>>),
}

#[derive(Debug, Clone)]
struct Located {
    pane: Pane,
    /// The pane shows this session's own output rather than hosting it as a
    /// background tab.
    own_content: bool,
}

impl Tmux {
    pub fn new(pane: Option<Pane>, own_content: bool) -> Self {
        Self {
            panes: Panes::Resolved(pane.map(|pane| Located { pane, own_content })),
        }
    }

    /// Finds the session's pane among all tmux panes when first needed.
    pub fn lookup() -> Self {
        Self {
            panes: Panes::Lazy(OnceLock::new()),
        }
    }

    fn locate(&self, session: &ListedSession) -> Result<Option<Located>> {
        let panes = match &self.panes {
            Panes::Resolved(located) => return Ok(located.clone()),
            Panes::Lazy(panes) => {
                panes.get_or_init(|| tmux::list_panes(true).map_err(|error| format!("{error:#}")))
            }
        };
        let panes = panes.as_ref().map_err(|error| anyhow!("{error}"))?;
        Ok(
            resolve_session_pane(panes, session, std::process::id()).map(|resolved| Located {
                pane: resolved.pane.clone(),
                own_content: resolved.own_content,
            }),
        )
    }

    fn pane(&self, session: &ListedSession) -> Result<Option<Pane>> {
        Ok(self.locate(session)?.map(|located| located.pane))
    }

    fn own_pane(&self, session: &ListedSession) -> Result<Option<Pane>> {
        Ok(self
            .locate(session)?
            .filter(|located| located.own_content)
            .map(|located| located.pane))
    }
}

impl Hub for Tmux {
    fn hosts(&self, session: &ListedSession) -> bool {
        self.pane(session).is_ok_and(|pane| pane.is_some())
    }

    fn focus(&self, _ctx: &AppContext, session: &ListedSession) -> Result<()> {
        let pane = self
            .pane(session)?
            .context("selected session is not in a tmux pane")?;
        tmux::attach_pane(&pane)
    }

    fn focus_in_foreground(&self, session: &ListedSession) -> bool {
        self.hosts(session)
    }

    fn send(&self, session: &ListedSession, message: &str) -> Result<()> {
        // Skip the tmux calls when there is no live process to type into. The
        // resolver checks liveness again, but a clear error is better here.
        if !session.pid.is_some_and(process::pid_alive)
            && !session.pane_pid.is_some_and(process::pid_alive)
        {
            bail!(
                "{} {} has no command to receive a message and is not running in a tmux pane",
                session.provider,
                session.session_id
            );
        }
        // Only the pane showing this session's own output may receive the
        // text, never a pane that hosts it as a background tab.
        let pane = self.own_pane(session)?.with_context(|| {
            format!(
                "{} {} is not the active session in a tmux pane",
                session.provider, session.session_id
            )
        })?;
        if tmux::pane_in_mode(&pane.id)? {
            bail!(
                "tmux pane {} is in copy mode; exit it before sending",
                pane.id
            );
        }
        tmux::send_message(&pane.id, message)
    }

    /// Best-effort transcript from the pane, including scrollback.
    fn transcript(&self, session: &ListedSession) -> Result<Conversation> {
        let pane = self
            .pane(session)
            .context("tmux capture")?
            .context("no tmux pane found")?;
        let text = tmux::capture_pane_transcript(&pane.id).context("tmux capture")?;
        Ok(Conversation::Text {
            text,
            origin: format!("tmux pane {}", pane.id),
        })
    }

    fn preview(&self, _ctx: &AppContext, session: &ListedSession) -> Option<Preview> {
        self.own_pane(session).ok().flatten().map(Preview::Pane)
    }
}

/// Pane a live session runs in. Paseo and desktop sessions are not in tmux,
/// and a session whose process ended no longer runs in any pane, even one
/// where another session of the same agent now runs.
pub(crate) fn running_pane(session: &ListedSession, panes: &[Pane], me: u32) -> Option<Pane> {
    live_session_pane(session, panes, me).map(|resolved| resolved.pane.clone())
}

/// Pane currently showing the session: it runs there, or is the front tab of
/// the process hosting it. This is the pane `send` would type into.
pub(crate) fn showing_pane<'a>(
    session: &ListedSession,
    panes: &'a [Pane],
    me: u32,
) -> Option<&'a Pane> {
    live_session_pane(session, panes, me)
        .filter(|resolved| resolved.own_content)
        .map(|resolved| resolved.pane)
}

fn live_session_pane<'a>(
    session: &ListedSession,
    panes: &'a [Pane],
    me: u32,
) -> Option<SessionPane<'a>> {
    let alive = |pid: Option<u32>| pid.is_some_and(process::pid_alive);
    if !alive(session.pid) && !alive(session.pane_pid) {
        return None;
    }
    resolve_session_pane(panes, session, me)
}

fn pane_for_pid<'a>(panes: &'a [Pane], chain: &[u32]) -> Option<&'a Pane> {
    panes.iter().find(|pane| chain.contains(&pane.pid))
}

pub(crate) struct SessionPane<'a> {
    pub pane: &'a Pane,
    /// The pane shows this session's own output: the session's process lives
    /// in the pane, or the process hosting it reports it as the front tab.
    pub own_content: bool,
}

/// Pane running the session, resolved in order of reliability: the process a
/// plugin reported as hosting its tab, the session's own process, and
/// finally any pane running the client binary in the session's directory.
pub(crate) fn resolve_session_pane<'a>(
    panes: &'a [Pane],
    session: &ListedSession,
    me: u32,
) -> Option<SessionPane<'a>> {
    if session.source != Source::Cli || session.paseo_agent_id.is_some() {
        return None;
    }
    // A shared reporter process can itself descend from a tmux pane while
    // hosting sessions from several TUIs, so a hosting report is the only
    // sound pane attribution — and the pane shows this session's own output
    // only when the host reports it as the front tab.
    if let Some(pid) = session.pane_pid.filter(|pid| process::pid_alive(*pid)) {
        let chain = process::ancestors(pid);
        if let Some(pane) = pane_for_pid(panes, &chain) {
            return Some(SessionPane {
                pane,
                own_content: session.front,
            });
        }
    }
    // A closed tab keeps the shared server's pid, which may descend from
    // another TUI's pane, and the server runs the same binary as every pane.
    if session.tabbed {
        return None;
    }
    if let Some(pid) = session.pid.filter(|pid| process::pid_alive(*pid)) {
        let chain = process::ancestors(pid);
        if let Some(pane) = pane_for_pid(panes, &chain) {
            return Some(SessionPane {
                pane,
                own_content: true,
            });
        }
    }
    let pane = pane_running_client(panes, session, me)?;
    Some(SessionPane {
        pane,
        own_content: false,
    })
}

/// OpenCode 2 hosts every TUI session in a shared server process, so the pid
/// its plugin reports is never the pane process or its descendant. Fall back
/// to the pane running the client binary; when the session's working
/// directory is known, only a pane at that directory may match.
fn pane_running_client<'a>(
    panes: &'a [Pane],
    session: &ListedSession,
    me: u32,
) -> Option<&'a Pane> {
    let binary = client_binary(session)?;
    let matches: Vec<&Pane> = panes
        .iter()
        .filter(|pane| process::tree_contains_named(pane.pid, &binary, me))
        .collect();
    match session.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
        Some(cwd) => matches
            .iter()
            .copied()
            .find(|pane| same_path(&pane.path, cwd)),
        None => match matches.as_slice() {
            [only] => Some(*only),
            _ => None,
        },
    }
}

fn client_binary(session: &ListedSession) -> Option<String> {
    let first = session.cmdline.first()?;
    let name = std::path::Path::new(first)
        .file_name()
        .and_then(|name| name.to_str())?;
    Some(name.to_string())
}

fn same_path(left: &str, right: &str) -> bool {
    let fold = |value: &str| {
        let text = value.replace('\\', "/");
        let text = text.trim_end_matches('/').to_string();
        if cfg!(windows) {
            text.to_ascii_lowercase()
        } else {
            text
        }
    };
    fold(left) == fold(right)
}

#[cfg(test)]
#[path = "tmux_tests.rs"]
mod tests;
