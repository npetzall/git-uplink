use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
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

pub fn with_queue_lock<T>(repo: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let key = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    if LOCK_OWNER.with(|owner| owner.borrow().as_ref() == Some(&key)) {
        return f();
    }

    fs::create_dir_all(repo.join(".git"))?;
    let lock_path = repo.join(".git/uplink.lock");
    let deadline = Instant::now() + Duration::from_secs(60);
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

pub fn is_push_lease_rejected(err: &Error) -> bool {
    let text = err.to_string().to_lowercase();
    text.contains("stale info")
        || text.contains("failed to push some refs")
        || text.contains("lease")
        || text.contains("non-fast-forward")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
