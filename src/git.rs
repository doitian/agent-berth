use std::path::{Path, PathBuf};
use std::process::Command;

/// Current branch of the repository containing `cwd`, including linked worktrees.
pub fn branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return Some(name.to_string());
    }
    if head.is_empty() {
        return None;
    }
    Some(head.chars().take(8).collect())
}

/// One-line repository summary for the details pane.
pub struct RepoInfo {
    /// Branch plus compact status, e.g. `main +1 ~2 ?3 ↑1 ↓2`.
    pub status: String,
    /// `owner/repo` when a remote points at GitHub.
    pub github: Option<String>,
}

pub fn repo_info(cwd: &Path) -> Option<RepoInfo> {
    find_git_dir(cwd)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let counts = parse_porcelain(&String::from_utf8_lossy(&output.stdout));
    let status = status_line(branch(cwd).as_deref().unwrap_or("-"), &counts);
    let github = github_remote(cwd);
    Some(RepoInfo { status, github })
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    staged: u32,
    modified: u32,
    unmerged: u32,
    untracked: u32,
    ahead: u32,
    behind: u32,
}

fn parse_porcelain(text: &str) -> Counts {
    let mut counts = Counts::default();
    for line in text.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    counts.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    counts.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let mut xy = line[2..].chars();
            if xy.next().is_some_and(|x| x != '.') {
                counts.staged += 1;
            }
            if xy.next().is_some_and(|y| y != '.') {
                counts.modified += 1;
            }
        } else if line.starts_with("u ") {
            counts.unmerged += 1;
        } else if line.starts_with("? ") {
            counts.untracked += 1;
        }
    }
    counts
}

fn status_line(branch: &str, counts: &Counts) -> String {
    let mut parts = vec![branch.to_string()];
    if counts.staged > 0 {
        parts.push(format!("+{}", counts.staged));
    }
    if counts.modified > 0 {
        parts.push(format!("~{}", counts.modified));
    }
    if counts.unmerged > 0 {
        parts.push(format!("!{}", counts.unmerged));
    }
    if counts.untracked > 0 {
        parts.push(format!("?{}", counts.untracked));
    }
    if counts.ahead > 0 {
        parts.push(format!("↑{}", counts.ahead));
    }
    if counts.behind > 0 {
        parts.push(format!("↓{}", counts.behind));
    }
    if parts.len() == 1 {
        parts.push("(clean)".into());
    }
    parts.join(" ")
}

fn github_remote(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["remote", "-v"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut fallback = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(url)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(repo) = parse_github_url(url) else {
            continue;
        };
        if name == "origin" {
            return Some(repo);
        }
        fallback.get_or_insert(repo);
    }
    fallback
}

fn parse_github_url(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.split_once("github.com/").map(|(_, rest)| rest))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd;
    loop {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).ok()?;
            let target = text.trim().strip_prefix("gitdir:")?.trim();
            let target = PathBuf::from(target);
            return Some(if target.is_absolute() {
                target
            } else {
                dir.join(target)
            });
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
