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
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub branch: String,
    pub status: RepoStatus,
    /// `owner/repo` when a remote points at GitHub.
    pub github: Option<String>,
}

pub fn repo_info(cwd: &Path) -> Option<RepoInfo> {
    find_git_dir(cwd)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain=v2", "--branch", "--show-stash"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let status = parse_porcelain(&String::from_utf8_lossy(&output.stdout));
    let branch = branch(cwd).unwrap_or_else(|| "-".into());
    let github = github_remote(cwd);
    Some(RepoInfo {
        branch,
        status,
        github,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    pub conflicted: bool,
    pub stashed: bool,
    pub deleted: bool,
    pub renamed: bool,
    pub modified: bool,
    pub staged: bool,
    pub untracked: bool,
    pub tracking: Option<(u32, u32)>,
}

fn parse_porcelain(text: &str) -> RepoStatus {
    let mut state = RepoStatus::default();
    for line in text.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            if let (Some(ahead), Some(behind)) = (parts.next(), parts.next())
                && let (Some(ahead), Some(behind)) =
                    (ahead.strip_prefix('+'), behind.strip_prefix('-'))
                && let (Ok(ahead), Ok(behind)) = (ahead.parse(), behind.parse())
            {
                state.tracking = Some((ahead, behind));
            }
        } else if let Some(count) = line.strip_prefix("# stash ") {
            state.stashed = count.parse::<u32>().is_ok_and(|count| count > 0);
        } else if line.starts_with("u ") {
            state.conflicted = true;
        } else if line.starts_with("? ") {
            state.untracked = true;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let Some(xy) = line.split_whitespace().nth(1) else {
                continue;
            };
            let [index, worktree] = xy.as_bytes() else {
                continue;
            };
            state.staged |= *index != b'.';
            state.modified |= matches!(worktree, b'M' | b'T');
            state.deleted |= xy.contains('D');
            state.renamed |= xy.contains('R');
        }
    }
    state
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
