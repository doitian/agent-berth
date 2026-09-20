use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::git;
use crate::paths;
use crate::store::ListedSession;

pub fn single(sessions: &[ListedSession], pattern: &str) -> Result<Option<ListedSession>> {
    let candidates = filter(sessions, &[pattern.to_string()]);
    if candidates.is_empty() {
        bail!("no session matches {pattern:?}");
    }
    if candidates.len() == 1 {
        return Ok(candidates.into_iter().next());
    }
    Ok(select(&candidates, &[pattern.to_string()], false)?
        .into_iter()
        .next())
}

pub fn multi(sessions: &[ListedSession], patterns: &[String]) -> Result<Vec<ListedSession>> {
    let candidates = filter(sessions, patterns);
    if candidates.is_empty() {
        bail!("no sessions match");
    }
    select(&candidates, patterns, true)
}

fn filter(sessions: &[ListedSession], patterns: &[String]) -> Vec<ListedSession> {
    let terms: Vec<String> = patterns
        .iter()
        .flat_map(|pattern| pattern.split_whitespace())
        .map(str::to_ascii_lowercase)
        .collect();
    sessions
        .iter()
        .filter(|session| {
            let haystack = search_text(session);
            terms.iter().all(|term| haystack.contains(term.as_str()))
        })
        .cloned()
        .collect()
}

fn search_text(session: &ListedSession) -> String {
    let cwd = session.cwd.clone().unwrap_or_default();
    let branch = git::branch(Path::new(&cwd)).unwrap_or_default();
    let folder = folder_name(&cwd).unwrap_or_default();
    format!(
        "{} {} {} {} {} {} {}",
        session.provider,
        session.session_id,
        session.status.as_str(),
        session.title.as_deref().unwrap_or(""),
        cwd,
        folder,
        branch,
    )
    .to_ascii_lowercase()
}

fn select(
    sessions: &[ListedSession],
    patterns: &[String],
    multi: bool,
) -> Result<Vec<ListedSession>> {
    if !paths::on_path("fzf") {
        bail!("fzf is required to select a session");
    }
    let lines: Vec<String> = sessions
        .iter()
        .enumerate()
        .map(|(index, session)| format!("{index}\t{}", display_line(session)))
        .collect();
    let selected = run_fzf(&lines, patterns, multi)?;
    let mut out = Vec::new();
    for line in selected {
        let index: usize = line
            .split('\t')
            .next()
            .unwrap_or_default()
            .parse()
            .with_context(|| format!("invalid session selection {line:?}"))?;
        if let Some(session) = sessions.get(index) {
            out.push(session.clone());
        }
    }
    Ok(out)
}

fn display_line(session: &ListedSession) -> String {
    let cwd = session.cwd.clone().unwrap_or_default();
    let branch = git::branch(Path::new(&cwd)).unwrap_or_else(|| "-".into());
    let folder = folder_name(&cwd).unwrap_or_else(|| "-".into());
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        session.provider,
        session.status.as_str(),
        session.session_id,
        session.title.as_deref().unwrap_or("-"),
        folder,
        branch,
    )
}

fn folder_name(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn run_fzf(lines: &[String], patterns: &[String], multi: bool) -> Result<Vec<String>> {
    let mut command = Command::new("fzf");
    command.args(fzf_args(patterns, multi));
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
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

fn fzf_args(patterns: &[String], multi: bool) -> Vec<String> {
    let mut args: Vec<String> = ["--delimiter", "\t", "--with-nth", "2..", "--ansi"]
        .into_iter()
        .map(str::to_string)
        .collect();
    args.push(if multi { "--multi".into() } else { "+m".into() });
    let query = patterns.join(" ");
    if !query.is_empty() {
        args.push("-q".into());
        args.push(query);
    }
    args
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
