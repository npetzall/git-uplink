use std::path::Path;

use super::{StateTree, read_blob_at};
use crate::git::git_ok;
use crate::types::{QueueConfig, QueueState, STATE_BRANCH};

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q"]).unwrap();
    dir
}

fn state_file(repo: &Path, path: &str) -> String {
    git_ok(repo, &["show", &format!("{STATE_BRANCH}:{path}")]).unwrap()
}

#[test]
fn commits_staged_files_on_the_branch_and_nothing_in_the_work_tree() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    assert!(state.base().is_none());
    state.write(".uplink/a.txt", "one\n");
    state.write(".uplink/b/c.txt", "two\n");
    assert!(state.commit_to_branch("first").unwrap());
    assert!(!state.is_dirty());

    assert_eq!(state_file(repo, ".uplink/a.txt"), "one");
    assert_eq!(state_file(repo, ".uplink/b/c.txt"), "two");
    assert!(!repo.join(".uplink").exists());
    assert_eq!(
        state.base(),
        Some(git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap().as_str())
    );
}

#[test]
fn reads_staged_changes_over_the_base_and_keeps_newlines() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/a.txt", "base\n");
    state.write(".uplink/gone.txt", "x");
    state.commit_to_branch("base").unwrap();

    let mut state = StateTree::head(repo).unwrap();
    assert_eq!(
        state.read(".uplink/a.txt").unwrap().as_deref(),
        Some("base\n")
    );
    state.write(".uplink/a.txt", "staged\n");
    state.remove(".uplink/gone.txt");
    assert_eq!(
        state.read(".uplink/a.txt").unwrap().as_deref(),
        Some("staged\n")
    );
    assert_eq!(state.read(".uplink/gone.txt").unwrap(), None);
    assert_eq!(state.read(".uplink/missing.txt").unwrap(), None);
    assert_eq!(state.list(".uplink").unwrap(), vec![".uplink/a.txt"]);
}

#[test]
fn an_unchanged_tree_makes_no_commit() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/a.txt", "same\n");
    state.commit_to_branch("first").unwrap();
    let before = git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap();

    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/a.txt", "same\n");
    assert!(!state.commit_to_branch("again").unwrap());
    assert_eq!(git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap(), before);
}

#[test]
fn remove_dir_removes_every_file_under_it() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/reports/x/a.md", "a");
    state.write(".uplink/reports/x/extras/b.md", "b");
    state.write(".uplink/reports/y/c.md", "c");
    state.commit_to_branch("reports").unwrap();

    let mut state = StateTree::head(repo).unwrap();
    state.remove_dir(".uplink/reports/x").unwrap();
    state.commit_to_branch("drop x").unwrap();
    let listed = git_ok(repo, &["ls-tree", "-r", "--name-only", STATE_BRANCH]).unwrap();
    assert_eq!(listed, ".uplink/reports/y/c.md");
}

#[test]
fn refuses_to_move_a_branch_that_moved_since_it_was_read() {
    let dir = repo();
    let repo = dir.path();
    let mut first = StateTree::head(repo).unwrap();
    first.write(".uplink/a.txt", "a");
    first.commit_to_branch("first").unwrap();

    let mut stale = StateTree::head(repo).unwrap();
    let mut other = StateTree::head(repo).unwrap();
    other.write(".uplink/b.txt", "b");
    other.commit_to_branch("other").unwrap();
    let moved = git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap();

    stale.write(".uplink/c.txt", "c");
    assert!(stale.commit_to_branch("stale").is_err());
    assert_eq!(git_ok(repo, &["rev-parse", STATE_BRANCH]).unwrap(), moved);
}

#[test]
fn copy_from_takes_files_from_another_commit_and_removes_missing_ones() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/a.txt", "old");
    state.write(".uplink/b.txt", "kept");
    state.commit_to_branch("old").unwrap();
    let old = state.base().unwrap().to_string();
    state.write(".uplink/a.txt", "new");
    state.remove(".uplink/b.txt");
    state.write(".uplink/c.txt", "only new");
    state.commit_to_branch("new").unwrap();

    let mut state = StateTree::head(repo).unwrap();
    state
        .copy_from(&old, &[".uplink/a.txt".into(), ".uplink/c.txt".into()])
        .unwrap();
    assert_eq!(state.read(".uplink/a.txt").unwrap().as_deref(), Some("old"));
    assert_eq!(state.read(".uplink/c.txt").unwrap(), None);
}

#[test]
fn reads_only_regular_files() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    state.write(".uplink/a.txt", "a");
    state.commit_to_branch("first").unwrap();
    let blob = git_ok(
        repo,
        &["rev-parse", &format!("{STATE_BRANCH}:.uplink/a.txt")],
    )
    .unwrap();
    let listing = format!("120000 blob {blob}\tlink\n");
    let tree = crate::git::git(
        repo,
        &["mktree"],
        crate::git::GitOpts {
            input: Some(listing.as_bytes()),
            ..Default::default()
        },
    )
    .unwrap()
    .stdout;
    let commit = git_ok(repo, &["commit-tree", &tree, "-m", "link"]).unwrap();
    assert!(read_blob_at(repo, &commit, "link").is_err());
    assert_eq!(read_blob_at(repo, &commit, "missing").unwrap(), None);
}

#[test]
fn queue_round_trips_and_a_missing_queue_says_how_to_initialize() {
    let dir = repo();
    let repo = dir.path();
    let mut state = StateTree::head(repo).unwrap();
    let err = state.queue().unwrap_err().to_string();
    assert!(err.contains("git uplink init"), "{err}");
    state
        .write_queue(&QueueState::empty(QueueConfig::default()))
        .unwrap();
    state.commit_to_branch("init").unwrap();
    let state = StateTree::head(repo).unwrap();
    assert!(state.has_queue().unwrap());
    assert!(state.queue().unwrap().upstream.is_empty());
}
