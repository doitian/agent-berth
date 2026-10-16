use anyhow::{Context, Result, bail};

use crate::db;
use crate::hubs::{self, Tmux};
use crate::paths::Context as AppContext;
use crate::providers;
use crate::store::ListedSession;

/// Send a message to the session named by its stable `id`. The message is the
/// remaining arguments joined with spaces; when none are given, it is read
/// from stdin.
///
/// The provider's own CLI is used when it has a command that reaches the
/// running session. Otherwise the session's hub delivers it, such as by
/// typing into the tmux pane that runs the session.
pub fn run(ctx: &AppContext, id: &str, message: &[String]) -> Result<()> {
    let id = id.trim();
    let message = if message.is_empty() {
        std::io::read_to_string(std::io::stdin()).context("read message from stdin")?
    } else {
        message.join(" ")
    };
    let message = message.trim_end_matches(['\r', '\n']);
    if message.trim().is_empty() {
        bail!("message is empty");
    }
    let sessions = db::query_sessions(ctx, false, None)?;
    let matches: Vec<&ListedSession> = sessions
        .iter()
        .filter(|session| session.id().eq_ignore_ascii_case(id))
        .collect();
    let session = match matches.as_slice() {
        [] => bail!("no session with ID {id:?}"),
        [only] => *only,
        _ => {
            let listed: Vec<String> = matches
                .iter()
                .map(|session| format!("{} {}", session.provider, session.session_id))
                .collect();
            bail!("ID {id:?} matches several sessions: {}", listed.join(", "));
        }
    };

    let hub = hubs::resolve(session, Tmux::lookup);
    providers::send(hub.as_ref(), session, message)?;
    println!("sent to {} {}", session.provider, session.session_id);
    Ok(())
}
