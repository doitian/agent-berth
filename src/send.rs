use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::db;
use crate::hubs::{self, Tmux};
use crate::paths::Context as AppContext;
use crate::pick;
use crate::providers;
use crate::self_cmd::{self, Identity};
use crate::status::AgentStatus;
use crate::wait::{self, Outcome};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Deliver even to a session waiting on a permission prompt or a
    /// question, where typed text and Enter would answer the dialog.
    pub force: bool,
    /// Leave out the line naming the sending agent.
    pub raw: bool,
    /// After delivering, wait for the turn to end with this timeout.
    pub wait: Option<Duration>,
}

/// Send a message to the session named by its stable `id`. The message is the
/// remaining arguments joined with spaces; when none are given, it is read
/// from stdin.
///
/// The provider's own CLI is used when it has a command that reaches the
/// running session. Otherwise the session's hub delivers it, such as by
/// typing into the tmux pane that runs the session.
///
/// With `wait`, the session's `turns_ended` is read before delivery and the
/// call then behaves like `wait --after` that count, so a turn that ends
/// before the first poll still counts. The "sent to" line moves to stderr
/// then, leaving stdout to the outcome word.
pub fn run(
    ctx: &AppContext,
    id: &str,
    message: &[String],
    options: Options,
) -> Result<Option<Outcome>> {
    let message = read_message(message)?;
    let message = if options.raw {
        message
    } else {
        with_header(&message, self_cmd::identity(ctx).as_ref())
    };
    let sessions = db::query_sessions(ctx, false, None)?;
    let session = pick::by_id(&sessions, id)?;
    if session.status == AgentStatus::Waiting && !options.force {
        bail!(
            "{} {} is waiting on a permission prompt or a question; answer it with `agent-berth attach {}`, or pass --force to send anyway",
            session.provider,
            session.session_id,
            session.id()
        );
    }
    let baseline = session.turns_ended;

    let hub = hubs::resolve(session, Tmux::lookup);
    providers::send(hub.as_ref(), session, &message)?;
    let sent = format!(
        "sent to {} {} (turns_ended {baseline})",
        session.provider, session.session_id
    );
    let Some(timeout) = options.wait else {
        println!("{sent}");
        return Ok(None);
    };
    eprintln!("{sent}");
    wait::wait_for(ctx, session, baseline, timeout).map(Some)
}

/// The message from the arguments, or from stdin when there are none.
pub fn read_message(args: &[String]) -> Result<String> {
    let message = if args.is_empty() {
        std::io::read_to_string(std::io::stdin()).context("read message from stdin")?
    } else {
        args.join(" ")
    };
    let message = message.trim_end_matches(['\r', '\n']);
    if message.trim().is_empty() {
        bail!("message is empty");
    }
    Ok(message.to_string())
}

/// The message headed by a line naming the sending agent, so the receiver
/// knows who to `reply` to. A sender without an identity, such as a human in
/// a shell pane, leaves the message as it is.
fn with_header(message: &str, sender: Option<&Identity>) -> String {
    match sender {
        Some(sender) => format!(
            "[agent-berth message from {} ({})]\n\n{message}",
            sender.id(),
            sender.provider
        ),
        None => message.to_string(),
    }
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
