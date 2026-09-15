use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

thread_local! {
    static LOCK_OWNER: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
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
                let _ = writeln!(file, "{} {}", std::process::id(), crate::queue::now_iso());
                LOCK_OWNER.with(|owner| *owner.borrow_mut() = Some(key.clone()));
                let result = f();
                LOCK_OWNER.with(|owner| *owner.borrow_mut() = None);
                let _ = fs::remove_file(&lock_path);
                return result;
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
