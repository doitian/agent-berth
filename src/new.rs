use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::db;
use crate::duration::format_duration;
use crate::paths::{Context as AppContext, DELEGATOR_ENV};
use crate::resume;
use crate::self_cmd;
use crate::tmux;

const POLL: Duration = Duration::from_millis(500);

/// Whose stable id the new agent gets as its delegator, to `reply` to.
pub enum Delegator {
    /// The agent running `new`, when it is a tracked session.
    Caller,
    Named(String),
    Unset,
}

impl Delegator {
    fn resolve(self, ctx: &AppContext) -> Option<String> {
        match self {
            Self::Caller => self_cmd::identity(ctx).map(|this| this.id()),
            Self::Named(id) => Some(id),
            Self::Unset => None,
        }
    }
}

/// Start `cmd` in a new window of the current directory's tmux session, the
/// same session `resume` uses for agents that worked here, creating it when
/// missing, and print the new pane's id. With `wait`, print the stable id of
/// the session the pane reports instead.
///
/// Returns the process exit code: 4 when the wait times out.
pub fn run(
    ctx: &AppContext,
    cmd: &[String],
    wait: Option<Duration>,
    delegator: Delegator,
) -> Result<i32> {
    let Some(program) = cmd.first() else {
        bail!("provide the agent command to run");
    };
    if !tmux::available() {
        bail!("tmux is required to start an agent");
    }
    let cwd = std::env::current_dir().context("read the current directory")?;
    let session = tmux::ensure_session(&cwd)?;
    let window = window_name(program);
    let env: Vec<(String, String)> = delegator
        .resolve(ctx)
        .into_iter()
        .map(|id| (DELEGATOR_ENV.to_string(), id))
        .collect();
    let pane_id = tmux::spawn_window(&session, &window, &cwd, cmd, &env)?;
    // Only the pane or session id goes to stdout, so scripts can capture it.
    eprintln!(
        "started {} in tmux {session} window {window} (pane {pane_id})",
        cmd.join(" ")
    );
    let Some(timeout) = wait else {
        println!("{pane_id}");
        resume::attach_if_interactive(&session)?;
        return Ok(0);
    };
    let Some(id) = await_report(ctx, &pane_id, timeout)? else {
        eprintln!(
            "the agent in {pane_id} has not reported within {}; it may still be starting, or run `agent-berth doctor`",
            format_duration(timeout)
        );
        return Ok(4);
    };
    println!("{id}");
    resume::attach_if_interactive(&session)?;
    Ok(0)
}

/// Stable id of the session the pane shows once its first report arrives, or
/// `None` when `timeout` passes first. The pane closing before any report
/// means the command failed to start.
fn await_report(ctx: &AppContext, pane_id: &str, timeout: Duration) -> Result<Option<String>> {
    let deadline = Instant::now() + timeout;
    loop {
        let panes = tmux::list_panes(true)?;
        if !panes.iter().any(|pane| pane.id == pane_id) {
            bail!("the agent in {pane_id} exited before reporting; run `agent-berth doctor`");
        }
        let sessions = db::query_stored(ctx)?;
        if let Some(this) = self_cmd::in_pane(&sessions, &panes, pane_id).first() {
            return Ok(Some(this.id()));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(POLL.min(deadline - now));
    }
}

fn window_name(program: &str) -> String {
    let name = Path::new(program)
        .file_stem()
        .map_or_else(|| program.into(), |stem| stem.to_string_lossy());
    name.chars().take(24).collect()
}

#[cfg(test)]
#[path = "new_tests.rs"]
mod tests;
