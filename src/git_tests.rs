use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn reads_branch_from_git_directory() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".git/HEAD"), "ref: refs/heads/main\n");
    assert_eq!(branch(root.path()).as_deref(), Some("main"));
}

#[test]
fn finds_repository_from_subdirectory() {
    let root = tempfile::tempdir().unwrap();
    write(
        &root.path().join(".git/HEAD"),
        "ref: refs/heads/feature/x\n",
    );
    let nested = root.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(branch(&nested).as_deref(), Some("feature/x"));
}

#[test]
fn reads_branch_from_worktree_gitdir_file() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("worktree");
    let git_dir = root.path().join("repo/.git/worktrees/worktree");
    write(&git_dir.join("HEAD"), "ref: refs/heads/topic\n");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", git_dir.display()),
    );
    assert_eq!(branch(&worktree).as_deref(), Some("topic"));
}

#[test]
fn detached_head_uses_short_hash() {
    let root = tempfile::tempdir().unwrap();
    write(
        &root.path().join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    );
    assert_eq!(branch(root.path()).as_deref(), Some("01234567"));
}

#[test]
fn missing_repository_has_no_branch() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(branch(root.path()), None);
}

#[test]
fn parses_github_remote_urls() {
    assert_eq!(
        parse_github_url("git@github.com:owner/repo.git").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(
        parse_github_url("https://github.com/owner/repo.git").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(
        parse_github_url("ssh://git@github.com/owner/repo").as_deref(),
        Some("owner/repo")
    );
    assert_eq!(parse_github_url("https://gitlab.com/owner/repo"), None);
    assert_eq!(parse_github_url("https://github.com/owner"), None);
}

#[test]
fn parses_porcelain_status() {
    let text = "# branch.head main\n# branch.ab +2 -1\n# stash 1\n1 M. N... 100644 100644 100644 abc abc a.rs\n1 .M N... 100644 100644 100644 abc abc b.rs\n? new.rs\nu UU N... 100644 100644 100644 100644 abc abc abc c.rs\n";
    assert_eq!(
        parse_porcelain(text),
        RepoStatus {
            conflicted: true,
            stashed: true,
            modified: true,
            staged: true,
            untracked: true,
            tracking: Some((2, 1)),
            ..RepoStatus::default()
        }
    );
}

#[test]
fn porcelain_distinguishes_change_kinds() {
    for (record, expected) in [
        (
            "1 M. N... 100644 100644 100644 abc abc staged.rs",
            RepoStatus {
                staged: true,
                ..RepoStatus::default()
            },
        ),
        (
            "1 .M N... 100644 100644 100644 abc abc modified.rs",
            RepoStatus {
                modified: true,
                ..RepoStatus::default()
            },
        ),
        (
            "1 MM N... 100644 100644 100644 abc abc both.rs",
            RepoStatus {
                staged: true,
                modified: true,
                ..RepoStatus::default()
            },
        ),
        (
            "1 .D N... 100644 100644 000000 abc abc deleted.rs",
            RepoStatus {
                deleted: true,
                ..RepoStatus::default()
            },
        ),
        (
            "2 R. N... 100644 100644 100644 abc abc R100 new.rs\told.rs",
            RepoStatus {
                renamed: true,
                staged: true,
                ..RepoStatus::default()
            },
        ),
        (
            "u UU N... 100644 100644 100644 100644 abc abc abc conflict.rs",
            RepoStatus {
                conflicted: true,
                ..RepoStatus::default()
            },
        ),
        (
            "? untracked.rs",
            RepoStatus {
                untracked: true,
                ..RepoStatus::default()
            },
        ),
        (
            "1 .M S..U 160000 160000 160000 abc abc submodule",
            RepoStatus {
                modified: true,
                ..RepoStatus::default()
            },
        ),
    ] {
        assert_eq!(parse_porcelain(record), expected, "{record}");
    }
}

#[test]
fn porcelain_tracks_divergence_without_treating_headers_as_changes() {
    for (header, tracking) in [
        ("# branch.ab +0 -0", Some((0, 0))),
        ("# branch.ab +2 -0", Some((2, 0))),
        ("# branch.ab +0 -1", Some((0, 1))),
        ("# branch.ab +2 -1", Some((2, 1))),
        ("# branch.head main\n! ignored.rs\n# stash 0", None),
        ("# branch.ab invalid", None),
    ] {
        assert_eq!(
            parse_porcelain(header),
            RepoStatus {
                tracking,
                ..RepoStatus::default()
            }
        );
    }
}

#[test]
fn repo_info_reads_real_repository() {
    let root = tempfile::tempdir().unwrap();
    let Ok(init) = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(root.path())
        .output()
    else {
        return;
    };
    if !init.status.success() {
        return;
    }
    std::fs::write(root.path().join("new.rs"), "fn main() {}\n").unwrap();
    std::process::Command::new("git")
        .args(["remote", "add", "origin", "git@github.com:owner/repo.git"])
        .current_dir(root.path())
        .output()
        .unwrap();
    let info = repo_info(root.path()).unwrap();
    assert_eq!(info.branch, "main");
    assert_eq!(
        info.status,
        RepoStatus {
            untracked: true,
            ..RepoStatus::default()
        }
    );
    assert_eq!(info.github.as_deref(), Some("owner/repo"));
}

#[test]
fn repo_info_is_none_outside_repository() {
    let root = tempfile::tempdir().unwrap();
    assert!(repo_info(root.path()).is_none());
}
