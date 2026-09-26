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
use crate::store::SessionKind;
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
    let me = std::process::id();
    let mut ordered: Vec<&ListedSession> = sessions.iter().collect();
    ordered.sort_by_key(|session| std::cmp::Reverse(session.last_report_ms));
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for session in ordered {
        if session.source != Source::Cli {
            continue;
        }
        if session.kind == SessionKind::Hook && !session.pid.is_some_and(process::pid_alive) {
            continue;
        }
        let Some(resolved) = resolve_session_pane(&panes, session, me) else {
            continue;
        };
        if !seen.insert(resolved.pane.id.clone()) {
            continue;
        }
        candidates.push(Candidate::new(resolved.pane.clone(), session.clone()));
    }
    candidates.sort_by(|a, b| a.pane.id.cmp(&b.pane.id));
    Ok(candidates)
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
    if session.source != Source::Cli {
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

fn select(lines: &[String], query: Option<&str>, preview: bool) -> Result<Option<String>> {
    let mut command = Command::new("fzf");
    command.args(fzf_args(query, preview));
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
