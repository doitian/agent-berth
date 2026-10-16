use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::db;
use crate::git;
use crate::hubs::tmux::running_pane;
use crate::paths::Context as AppContext;
use crate::store::ListedSession;
use crate::tmux::{self, Pane};

pub fn run(
    ctx: &AppContext,
    query: Option<String>,
    preview: bool,
    session: bool,
    dry_run: bool,
) -> Result<()> {
    let sessions = db::query_sessions(ctx, false, None)?;
    let candidates = collect(&sessions, &tmux::list_panes(!session)?);
    if candidates.is_empty() {
        println!("No active agents in tmux panes.");
        return Ok(());
    }
    if dry_run {
        for candidate in &candidates {
            println!("{}", candidate.line());
        }
        return Ok(());
    }
    // A query that names exactly one candidate by its stable ID skips the
    // picker, so `attach <id>` works without a selection step.
    let selected = match direct(&candidates, query.as_deref()) {
        Some(candidate) => candidate.line(),
        None => {
            if !crate::paths::on_path("fzf") {
                bail!("fzf is required to select a session");
            }
            let lines: Vec<String> = candidates.iter().map(Candidate::line).collect();
            match select(&lines, query.as_deref(), preview)? {
                Some(selected) => selected,
                None => return Ok(()),
            }
        }
    };
    let target_id = selected.split('\t').next().unwrap_or("");
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.id() == target_id)
        .context("selected session is no longer active")?;
    // Resolve only the picked session again: another session reporting from
    // the same pane while fzf was open would displace it from `collect`.
    let sessions = db::query_sessions(ctx, false, None)?;
    let current = sessions
        .iter()
        .find(|current| {
            current.provider == candidate.session.provider
                && current.session_id == candidate.session.session_id
        })
        .context("selected session is no longer active")?;
    let panes = tmux::list_panes(!session)?;
    let pane = running_pane(current, &panes, std::process::id())
        .context("selected session is no longer active")?;
    tmux::attach_pane(&pane)
}

fn collect(sessions: &[ListedSession], panes: &[Pane]) -> Vec<Candidate> {
    let me = std::process::id();
    let mut ordered: Vec<&ListedSession> = sessions.iter().collect();
    ordered.sort_by_key(|session| std::cmp::Reverse(session.last_report_ms));
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for item in ordered {
        let Some(pane) = running_pane(item, panes, me) else {
            continue;
        };
        if seen.insert(pane.id.clone()) {
            candidates.push(Candidate::new(pane, item.clone()));
        }
    }
    candidates.sort_by(|left, right| left.id().cmp(right.id()));
    candidates
}

/// Candidate named by the exact stable session ID in `query`, when exactly one
/// matches. Any other query (including multi-word fzf queries) returns `None`
/// so the picker handles it.
fn direct<'a>(candidates: &'a [Candidate], query: Option<&str>) -> Option<&'a Candidate> {
    let query = query?.trim();
    if query.is_empty() {
        return None;
    }
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.session.id().eq_ignore_ascii_case(query));
    let only = matches.next()?;
    matches.next().is_none().then_some(only)
}

fn select(lines: &[String], query: Option<&str>, preview: bool) -> Result<Option<String>> {
    let mut child = Command::new("fzf")
        .args(fzf_args(query, preview))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("run fzf")?;
    if let Some(mut stdin) = child.stdin.take() {
        for line in lines {
            writeln!(stdin, "{line}")?;
        }
    }
    let output = child.wait_with_output().context("wait for fzf")?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().next().map(str::to_string))
}

struct Candidate {
    pane: Pane,
    session: ListedSession,
    cwd: String,
    branch: Option<String>,
}

impl Candidate {
    fn new(pane: Pane, session: ListedSession) -> Self {
        let cwd = session
            .cwd
            .clone()
            .filter(|cwd| !cwd.is_empty())
            .unwrap_or_else(|| pane.path.clone());
        let branch = git::branch(Path::new(&cwd));
        Self {
            pane,
            session,
            cwd,
            branch,
        }
    }

    fn id(&self) -> &str {
        &self.pane.id
    }

    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.id(),
            self.session.id(),
            self.session.provider,
            self.session.status.as_str(),
            self.session.title.as_deref().unwrap_or("-"),
            folder_name(&self.cwd).unwrap_or_else(|| "-".into()),
            self.branch.as_deref().unwrap_or("-"),
            self.session.session_id,
        )
    }
}

fn folder_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn fzf_args(query: Option<&str>, preview: bool) -> Vec<String> {
    let preview_command = tmux::preview_command();
    let preview_window = if preview { "up:80%" } else { "up:80%:hidden" };
    let mut args: Vec<String> = [
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "+m",
        "--ansi",
        "--preview",
        preview_command.as_str(),
        "--preview-window",
        preview_window,
        "--bind",
        "ctrl-t:toggle-preview",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    #[cfg(windows)]
    args.extend(["--with-shell".into(), tmux::PREVIEW_SHELL.into()]);
    if let Some(query) = query {
        args.push("-q".into());
        args.push(query.into());
    }
    args
}

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
