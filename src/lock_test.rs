use super::*;
use crate::git::{GitError, GitResult};

fn git_failure(args: &[&str], stderr: &str) -> Error {
    Error::Git(GitError::new(
        args,
        GitResult {
            stdout: String::new(),
            stderr: stderr.into(),
            code: 1,
        },
    ))
}

#[test]
fn only_push_race_rejections_are_retried() {
    let push = ["push", "origin", "refs/heads/uplink/state"];
    assert!(is_push_lease_rejected(&git_failure(
        &push,
        " ! [rejected] uplink/state -> uplink/state (stale info)\nerror: failed to push some refs",
    )));
    assert!(is_push_lease_rejected(&git_failure(
        &push,
        " ! [rejected] uplink/state -> uplink/state (fetch first)\nerror: failed to push some refs",
    )));
    assert!(is_push_lease_rejected(&git_failure(
        &push,
        "remote: fatal error in commit_refs\n ! [remote rejected] uplink/state -> uplink/state (failure)\nerror: failed to push some refs",
    )));
    assert!(!is_push_lease_rejected(&git_failure(
        &push,
        " ! [remote rejected] uplink/state -> uplink/state (failure)\nerror: failed to push some refs",
    )));
    assert!(!is_push_lease_rejected(&git_failure(
        &push,
        "remote: Permission denied\nerror: failed to push some refs",
    )));
    assert!(!is_push_lease_rejected(&git_failure(
        &push,
        " ! [remote rejected] uplink/state -> uplink/state (pre-receive hook declined)\nerror: failed to push some refs",
    )));
    assert!(!is_push_lease_rejected(&git_failure(
        &["fetch", "origin"],
        "non-fast-forward",
    )));
    assert!(!is_push_lease_rejected(&Error::msg("lease stale info")));
}

#[test]
fn lock_is_released_when_closure_panics() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let panicked =
        std::panic::catch_unwind(|| with_queue_lock(repo, || -> Result<()> { panic!("boom") }));
    assert!(panicked.is_err());
    assert!(!repo.join(".git/uplink.lock").exists());
    assert!(LOCK_OWNER.with(|owner| owner.borrow().is_none()));
    with_queue_lock(repo, || Ok(())).unwrap();
}

#[cfg(unix)]
#[test]
fn lock_from_dead_process_is_taken_over() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let mut child = Command::new("true").spawn().unwrap();
    let dead_pid = child.id();
    child.wait().unwrap();
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(
        repo.join(".git/uplink.lock"),
        format!("{dead_pid} 2026-01-01T00:00:00.000Z\n"),
    )
    .unwrap();

    let started = Instant::now();
    with_queue_lock(repo, || Ok(())).unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!repo.join(".git/uplink.lock").exists());
}

#[test]
fn lock_from_live_process_is_not_stale() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("uplink.lock");
    let contents = format!("{} 2026-01-01T00:00:00.000Z\n", std::process::id());
    fs::write(&lock, &contents).unwrap();
    assert!(!lock_is_stale(&lock, &contents));
    // A fresh lock whose PID is not written yet is not stale either.
    fs::write(&lock, "").unwrap();
    assert!(!lock_is_stale(&lock, ""));
}
