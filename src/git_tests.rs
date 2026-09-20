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
