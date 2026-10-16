use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::resume;
use crate::tmux;

/// Start `cmd` in a new window of the current directory's tmux session, the
/// same session `resume` uses for agents that worked here, creating it when
/// missing, and print the new pane's id.
pub fn run(cmd: &[String]) -> Result<()> {
    let Some(program) = cmd.first() else {
        bail!("provide the agent command to run");
    };
    if !tmux::available() {
        bail!("tmux is required to start an agent");
    }
    let cwd = std::env::current_dir().context("read the current directory")?;
    let session = tmux::ensure_session(&cwd)?;
    let window = window_name(program);
    let pane_id = tmux::spawn_window(&session, &window, &cwd, cmd)?;
    // Only the pane id goes to stdout, so scripts can capture it.
    eprintln!(
        "started {} in tmux {session} window {window}",
        cmd.join(" ")
    );
    println!("{pane_id}");
    resume::attach_if_interactive(&session)
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
