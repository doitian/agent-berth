use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::db;
use crate::git;
use crate::paths::Context as AppContext;
use crate::process;
use crate::status::Source;
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
    let candidates = collect(&sessions, session)?;
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
    if !crate::paths::on_path("fzf") {
        bail!("fzf is required to select a session");
    }
    let lines: Vec<String> = candidates.iter().map(Candidate::line).collect();
    let Some(selected) = select(&lines, query.as_deref(), preview)? else {
        return Ok(());
    };
    let pane_id = selected.split('\t').next().unwrap_or("");
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.pane.id == pane_id)
        .context("selected pane is no longer active")?;
    tmux::attach_pane(&candidate.pane)
}

fn collect(sessions: &[ListedSession], session: bool) -> Result<Vec<Candidate>> {
    let panes = tmux::list_panes(!session)?;
    let mut ordered: Vec<&ListedSession> = sessions.iter().collect();
    ordered.sort_by_key(|session| std::cmp::Reverse(session.last_report_ms));
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for session in ordered {
        if session.source != Source::Cli {
            continue;
        }
        let Some(pid) = session.pid else {
            continue;
        };
        if !process::pid_alive(pid) {
            continue;
        }
        let chain = process::ancestors(pid);
        let Some(pane) = pane_for_pid(&panes, &chain) else {
            continue;
        };
        if !seen.insert(pane.id.clone()) {
            continue;
        }
        candidates.push(Candidate::new(pane.clone(), session.clone()));
    }
    candidates.sort_by(|a, b| a.pane.id.cmp(&b.pane.id));
    Ok(candidates)
}

fn pane_for_pid<'a>(panes: &'a [Pane], chain: &[u32]) -> Option<&'a Pane> {
    panes.iter().find(|pane| chain.contains(&pane.pid))
}

fn select(lines: &[String], query: Option<&str>, preview: bool) -> Result<Option<String>> {
    let mut command = Command::new("fzf");
    let preview_command = tmux::preview_command();
    let preview_window = if preview { "up:80%" } else { "up:80%:hidden" };
    command.args([
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "+m",
        "-0",
        "--ansi",
        "--preview",
        preview_command.as_str(),
        "--preview-window",
        preview_window,
        "--bind",
        "ctrl-t:toggle-preview",
    ]);
    if let Some(query) = query {
        command.arg("-q").arg(query);
        command.arg("-1");
    }
    let mut child = command
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

    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.pane.id,
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

#[cfg(test)]
#[path = "attach_tests.rs"]
mod tests;
