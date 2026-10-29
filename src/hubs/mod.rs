//! Hosts that run agent sessions and can reach them without the agent's help.
//! A hub supplies the default behaviour of each session feature; a provider
//! decides whether to delegate to it or use its own native support.
use std::fmt::Debug;

use anyhow::Result;

use crate::paths::Context as AppContext;
use crate::status::Source;
use crate::store::ListedSession;
use crate::tmux::Pane;
use crate::transcript::{self, Conversation};

pub(crate) mod tmux;

pub use tmux::Tmux;

pub trait Hub: Debug + Send + Sync {
    /// The hub runs this session, so it can act on it.
    fn hosts(&self, session: &ListedSession) -> bool;

    fn focus(&self, ctx: &AppContext, session: &ListedSession) -> Result<()>;

    /// Focusing drives this terminal's tmux client, so a full-screen caller
    /// must run it on its own thread and may need to release the terminal.
    fn focus_in_foreground(&self, _session: &ListedSession) -> bool {
        false
    }

    fn send(&self, session: &ListedSession, message: &str) -> Result<()>;

    /// The whole conversation, as the hub can read it.
    fn transcript(&self, session: &ListedSession) -> Result<Conversation>;

    /// Live view of the conversation, or `None` when the hub cannot show it.
    fn preview(&self, ctx: &AppContext, session: &ListedSession) -> Option<Preview>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// The pane shows the session's own output.
    Pane(Pane),
    /// The session's conversation, read from its transcript log.
    Log(transcript::Source),
}

/// Hub hosting `session`: tmux, which `tmux` builds only when needed.
///
/// Only CLI sessions run in a tmux pane, so a desktop session's tmux hub never
/// hosts it and its provider focuses it in the desktop app instead.
pub fn resolve(session: &ListedSession, tmux: impl FnOnce() -> Tmux) -> Box<dyn Hub> {
    if session.source == Source::Cli {
        Box::new(tmux())
    } else {
        Box::new(Tmux::new(None, false))
    }
}
