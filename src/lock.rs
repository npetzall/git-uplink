use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

thread_local! {
    static LOCK_OWNER: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Releases the queue lock on drop, including when the locked closure panics.
struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        LOCK_OWNER.with(|owner| *owner.borrow_mut() = None);
        let _ = fs::remove_file(&self.path);
    }
}

/// A lock file without a readable PID is only treated as abandoned after this long.
const UNREADABLE_LOCK_MAX_AGE: Duration = Duration::from_secs(10 * 60);

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // `ps -p` also sees processes owned by other users, unlike `kill -0`.
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_or(true, |status| status.success())
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

fn lock_is_stale(lock_path: &Path, contents: &str) -> bool {
    match contents
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse::<u32>().ok())
    {
        Some(pid) => !process_alive(pid),
        None => fs::metadata(lock_path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > UNREADABLE_LOCK_MAX_AGE),
    }
}

/// Removes a lock left by a process that no longer runs. Re-reads the file
/// first so a lock another waiter just took over is left alone.
fn remove_if_stale(lock_path: &Path) {
    let Ok(contents) = fs::read_to_string(lock_path) else {
        return;
    };
    if lock_is_stale(lock_path, &contents)
        && fs::read_to_string(lock_path).is_ok_and(|now| now == contents)
    {
        let _ = fs::remove_file(lock_path);
    }
}

pub fn with_queue_lock<T>(repo: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    if LOCK_OWNER.with(|owner| owner.borrow().as_ref() == Some(&key)) {
        return f();
    }

    fs::create_dir_all(repo.join(".git"))?;
    let lock_path = repo.join(".git/uplink.lock");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut next_stale_check = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _guard = LockGuard {
                    path: lock_path.clone(),
                };
                let _ = writeln!(file, "{} {}", std::process::id(), crate::queue::now_iso());
                LOCK_OWNER.with(|owner| *owner.borrow_mut() = Some(key.clone()));
                return f();
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() >= next_stale_check {
                    remove_if_stale(&lock_path);
                    next_stale_check = Instant::now() + Duration::from_secs(1);
                }
                if Instant::now() > deadline {
                    return Err(Error::msg(
                        "Timed out waiting for the Uplink queue lock. Another import or sync is still running.",
                    ));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Push rejections caused by the remote ref moving concurrently. Auth, hook,
/// and other push failures are not retried.
const PUSH_RACE_REJECTIONS: &[&str] = &[
    "stale info",
    "non-fast-forward",
    "fetch first",
    "cannot lock ref",
];

pub fn is_push_lease_rejected(err: &Error) -> bool {
    let Error::Git(git_err) = err else {
        return false;
    };
    if git_err.args.first().map(String::as_str) != Some("push") {
        return false;
    }
    let stderr = git_err.result.stderr.to_lowercase();
    PUSH_RACE_REJECTIONS
        .iter()
        .any(|reason| stderr.contains(reason))
}

#[cfg(test)]
mod tests {
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
}
