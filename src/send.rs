use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::attach;
use crate::db;
use crate::paths::Context as AppContext;
use crate::process;
use crate::providers;
use crate::store::ListedSession;
use crate::tmux;

/// Send a message to the session named by its stable `id`. The message is the
/// remaining arguments joined with spaces; when none are given, it is read
/// from stdin.
///
/// The provider's own CLI is used when it has a command that reaches the
/// running session. Otherwise the message is typed into the tmux pane that
/// runs the session, which is the only way to reach an interactive client
/// without a dedicated send command.
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

    if let Some(cmd) = providers::send_cmd(&session.provider, &session.session_id, message) {
        return send_via_cli(session, &cmd);
    }
    send_via_pane(session, message)
}

fn send_via_cli(session: &ListedSession, cmd: &[String]) -> Result<()> {
    let (program, args) = cmd.split_first().context("empty send command")?;
    let mut command = Command::new(program);
    command.args(args);
    // A deleted working directory would mask the real error from the CLI.
    if let Some(cwd) = session.cwd.as_deref().filter(|cwd| Path::new(cwd).is_dir()) {
        command.current_dir(cwd);
    }
    let status = command.status().with_context(|| format!("run {program}"))?;
    if !status.success() {
        bail!("{} failed ({status})", cmd.join(" "));
    }
    println!("sent to {} {}", session.provider, session.session_id);
    Ok(())
}

fn send_via_pane(session: &ListedSession, message: &str) -> Result<()> {
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
    let panes = tmux::list_panes(true)?;
    // Only the pane showing this session's own output may receive the text.
    // `own_content` rejects the client-binary fallback and a background tab in
    // a pane that hosts several sessions.
    let resolved = attach::resolve_session_pane(&panes, session, std::process::id())
        .filter(|resolved| resolved.own_content)
        .with_context(|| {
            format!(
                "{} {} is not the active session in a tmux pane",
                session.provider, session.session_id
            )
        })?;
    if tmux::pane_in_mode(&resolved.pane.id)? {
        bail!(
            "tmux pane {} is in copy mode; exit it before sending",
            resolved.pane.id
        );
    }
    tmux::send_message(&resolved.pane.id, message)?;
    println!("sent to {} {}", session.provider, session.session_id);
    Ok(())
}
