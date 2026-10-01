use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use crate::assess::{depends_on_from_message, export_commit_message};
use crate::error::{Error, PreflightError, Result};
use crate::git::{GitOpts, git, git_succeeds};
use crate::queue::{
    active_upstream, apply_order_upstream_layer, get_patch, patch_path, read_queue,
};
use crate::repo::{
    TempWorktree, ensure_revs, ensure_upstream_ref, has_ref, rev_parse, write_product_patch,
};
use crate::types::{ApplyOutcome, Patch, PatchStatus, QueueState};

/// Credentials the import, transfer and submit workflows put in the same step
/// as preflight. The product's build and tests must not be able to read them.
fn is_credential_env(name: &str) -> bool {
    name == "GITHUB_TOKEN"
        || name == "GH_TOKEN"
        || (name.starts_with("UPLINK_") && (name.ends_with("_TOKEN") || name.ends_with("_KEY")))
}

fn run_shell(command: &str, cwd: &Path) -> (i32, String) {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    for (name, _) in env::vars_os() {
        if name.to_str().is_some_and(is_credential_env) {
            cmd.env_remove(name);
        }
    }
    let output = cmd.output();
    match output {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.code().unwrap_or(1), text.trim().to_string())
        }
        Err(err) => (1, err.to_string()),
    }
}

fn apply_abs(dir: &Path, patch_abs: &Path, message: &str) -> Result<ApplyOutcome> {
    if !git_succeeds(
        dir,
        &[
            "apply",
            "--3way",
            "--index",
            patch_abs.to_str().unwrap_or(""),
        ],
    )? {
        return Ok(ApplyOutcome::Conflict);
    }
    if git_succeeds(dir, &["diff", "--cached", "--quiet"])? {
        return Ok(ApplyOutcome::Empty);
    }
    git(dir, &["commit", "-m", message], GitOpts::default())?;
    Ok(ApplyOutcome::Applied)
}

fn with_upstream_worktree<T>(repo: &Path, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    ensure_upstream_ref(repo)?;
    if !has_ref(repo, "uplink/upstream")? {
        return Err(Error::msg(
            "No uplink/upstream ref; cannot preflight an export tree.",
        ));
    }
    // Dropped even if `f` panics, so the worktree is always removed.
    let worktree = TempWorktree::add(repo, "uplink-export", "uplink/upstream")?;
    f(&worktree.dir)
}

fn reset_export(dir: &Path, repo: &Path) -> Result<()> {
    let sha = rev_parse(repo, "uplink/upstream")?;
    git(
        dir,
        &["reset", "--hard", "--quiet", &sha],
        GitOpts::default(),
    )?;
    git(dir, &["clean", "-fdq"], GitOpts::default())?;
    Ok(())
}

fn dep_patch_abs(repo: &Path, id: &str) -> Result<PathBuf> {
    Ok(repo.join(patch_path(id)?))
}

fn apply_deps(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    dep_ids: &[String],
) -> Result<ApplyOutcome> {
    for id in dep_ids {
        let dep = get_patch(queue, id)?;
        let result = apply_abs(
            dir,
            &dep_patch_abs(repo, id)?,
            &format!("{} (dep)", dep.title),
        )?;
        if result == ApplyOutcome::Conflict {
            return Ok(ApplyOutcome::Conflict);
        }
    }
    Ok(ApplyOutcome::Applied)
}

pub fn preflight_command_for(queue: &QueueState) -> Option<String> {
    if let Ok(env_cmd) = env::var("UPLINK_PREFLIGHT") {
        let trimmed = env_cmd.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    queue
        .config
        .preflight_command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn format_suggestion(ids: &[String]) -> String {
    if ids.is_empty() {
        " Record --depends-on for every queued patch this change actually uses, or rewrite it so it stands on public main.".into()
    } else {
        format!(
            " Add this before import:\n  --depends-on {}",
            ids.join(" --depends-on ")
        )
    }
}

pub fn suggest_depends_on(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    candidate_message: &str,
) -> Result<Vec<String>> {
    let candidates: Vec<String> = active_upstream(queue)
        .into_iter()
        .map(|p| p.id.clone())
        .collect();
    with_upstream_worktree(repo, |dir| {
        let alone = apply_abs(dir, candidate_abs, candidate_message)?;
        if alone != ApplyOutcome::Conflict {
            return Ok(Vec::new());
        }
        for id in &candidates {
            reset_export(dir, repo)?;
            if apply_deps(dir, repo, queue, std::slice::from_ref(id))? == ApplyOutcome::Conflict {
                continue;
            }
            if apply_abs(dir, candidate_abs, candidate_message)? != ApplyOutcome::Conflict {
                return Ok(vec![id.clone()]);
            }
        }
        let mut prefix = Vec::new();
        for id in &candidates {
            prefix.push(id.clone());
            reset_export(dir, repo)?;
            if apply_deps(dir, repo, queue, &prefix)? == ApplyOutcome::Conflict {
                continue;
            }
            if apply_abs(dir, candidate_abs, candidate_message)? != ApplyOutcome::Conflict {
                return Ok(prefix);
            }
        }
        Ok(Vec::new())
    })
}

pub fn assert_export_preflight(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    candidate_abs: &Path,
    command_override: Option<Option<String>>,
) -> Result<()> {
    if queue.is_internal(&patch.id) || queue.is_tooling(&patch.id) {
        return Ok(());
    }
    let command = match command_override {
        Some(None) => None,
        Some(Some(cmd)) => Some(cmd),
        None => preflight_command_for(queue),
    };

    with_upstream_worktree(repo, |dir| {
        let deps = apply_deps(dir, repo, queue, &patch.depends_on)?;
        if deps == ApplyOutcome::Conflict {
            let suggested = suggest_depends_on(repo, queue, candidate_abs, &patch.title)?;
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Declared depends-on [{}] do not apply onto public upstream before \"{}\".{}",
                    if patch.depends_on.is_empty() {
                        "(none)".into()
                    } else {
                        patch.depends_on.join(", ")
                    },
                    patch.title,
                    format_suggestion(&suggested)
                ),
                suggested,
                "apply",
                None,
            )));
        }

        let applied = apply_abs(dir, candidate_abs, &patch.title)?;
        if applied == ApplyOutcome::Conflict {
            let suggested = suggest_depends_on(repo, queue, candidate_abs, &patch.title)?;
            let extra = if patch.depends_on.is_empty() {
                " alone".to_string()
            } else {
                format!(" plus {}", patch.depends_on.join(", "))
            };
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Patch \"{}\" does not apply onto public upstream{extra}. Company main is not a valid export base.{}",
                    patch.title,
                    format_suggestion(&suggested)
                ),
                suggested,
                "apply",
                None,
            )));
        }

        let Some(command) = command else {
            return Ok(());
        };
        let (code, output) = run_shell(&command, dir);
        if code == 0 {
            return Ok(());
        }
        let suggested = suggest_command_deps(repo, queue, candidate_abs, patch, &command, dir)?;
        let extra = if patch.depends_on.is_empty() {
            String::new()
        } else {
            format!(" + {}", patch.depends_on.join(", "))
        };
        let output_suffix = if output.is_empty() {
            String::new()
        } else {
            format!("\n\n{output}")
        };
        Err(Error::Preflight(PreflightError::new(
            format!(
                "Export preflight failed on public upstream{extra} ({command}, exit {code}). No upstream PR should be opened until this passes.{}{output_suffix}",
                format_suggestion(&suggested)
            ),
            suggested,
            "command",
            if output.is_empty() {
                None
            } else {
                Some(output)
            },
        )))
    })
}

fn suggest_command_deps(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    patch: &Patch,
    command: &str,
    dir: &Path,
) -> Result<Vec<String>> {
    let candidates: Vec<String> = active_upstream(queue)
        .into_iter()
        .filter(|item| item.id != patch.id && !patch.depends_on.contains(&item.id))
        .map(|item| item.id.clone())
        .collect();
    let mut prefix = patch.depends_on.clone();
    for id in candidates {
        prefix.push(id);
        reset_export(dir, repo)?;
        if apply_deps(dir, repo, queue, &prefix)? == ApplyOutcome::Conflict {
            continue;
        }
        if apply_abs(dir, candidate_abs, &patch.title)? == ApplyOutcome::Conflict {
            continue;
        }
        let (code, _) = run_shell(command, dir);
        if code == 0 {
            return Ok(prefix);
        }
    }
    Ok(Vec::new())
}

pub fn preflight_existing_patch(repo: &Path, queue: &QueueState, id: &str) -> Result<()> {
    let patch = get_patch(queue, id)?.clone();
    assert_export_preflight(repo, queue, &patch, &repo.join(patch_path(id)?), None)
}

pub struct IncomingPreflight {
    pub title: String,
    pub from_ref: String,
    pub head_ref: String,
    pub depends_on: Vec<String>,
    pub message: Option<String>,
    pub preflight_command: Option<String>,
    pub internal_only: bool,
}

pub fn run_preflight_command_in(queue: &QueueState, cwd: &Path) -> Result<()> {
    let Some(command) = preflight_command_for(queue) else {
        return Ok(());
    };
    let (code, output) = run_shell(&command, cwd);
    if code == 0 {
        return Ok(());
    }
    let output_suffix = if output.is_empty() {
        String::new()
    } else {
        format!("\n\n{output}")
    };
    Err(Error::Preflight(PreflightError::new(
        format!("Preflight failed ({command}, exit {code}).{output_suffix}"),
        Vec::new(),
        "command",
        if output.is_empty() {
            None
        } else {
            Some(output)
        },
    )))
}

pub fn assert_upstream_layer_applies(repo: &Path, queue: &QueueState) -> Result<()> {
    with_upstream_worktree(repo, |dir| apply_upstream_layer(dir, repo, queue, None))
}

/// Applies tooling + active upstream patches in order onto `dir`. `before`
/// names the candidate that will follow, for the error message.
fn apply_upstream_layer(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    before: Option<&str>,
) -> Result<()> {
    for patch in apply_order_upstream_layer(queue)? {
        if patch.status == PatchStatus::Conflict {
            return Err(Error::msg(format!(
                "Queue is blocked on conflict in {}; cannot preflight the upstream layer.",
                patch.id
            )));
        }
        let result = apply_abs(dir, &dep_patch_abs(repo, &patch.id)?, &patch.title)?;
        if result == ApplyOutcome::Conflict {
            let before = before
                .map(|title| format!(" before \"{title}\""))
                .unwrap_or_default();
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Queued patch \"{}\" does not apply onto tooling + upstream{before}.",
                    patch.title
                ),
                Vec::new(),
                "apply",
                None,
            )));
        }
    }
    Ok(())
}

pub fn assert_upstream_layer_preflight(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    title: &str,
) -> Result<()> {
    with_upstream_worktree(repo, |dir| {
        apply_upstream_layer(dir, repo, queue, Some(title))?;
        let applied = apply_abs(dir, candidate_abs, title)?;
        if applied == ApplyOutcome::Conflict {
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Patch \"{title}\" does not apply onto tooling + queued upstream (internal omitted). \
Rewrite it so it does not need internal changes, promote the internal patch upstream with depends-on, \
or label the PR uplink:internal-only."
                ),
                Vec::new(),
                "apply",
                None,
            )));
        }
        Ok(())
    })
}

pub fn preflight_incoming_change(repo: &Path, opts: IncomingPreflight) -> Result<()> {
    if opts.internal_only {
        return Ok(());
    }
    let queue = read_queue(repo)?;
    let id = format!(
        "upl_preflight_{}",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    let depends_on =
        depends_on_from_message(opts.message.as_deref().unwrap_or(""), &opts.depends_on);
    let patch = Patch {
        id: id.clone(),
        title: opts.title.clone(),
        status: PatchStatus::Queued,
        depends_on,
        created_at: String::new(),
        updated_at: String::new(),
        patch_id_stable: None,
        commit_message: String::new(),
        source: Default::default(),
        assess: None,
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        extras: None,
        events: Vec::new(),
        kind: None,
    };
    let message = export_commit_message(&patch);
    let shas = ensure_revs(repo, &[&opts.from_ref, &opts.head_ref])?;
    write_product_patch(repo, &id, &shas[0], &message, &shas[1])?;
    let candidate_abs = repo.join(patch_path(&id)?);
    let result = (|| {
        assert_export_preflight(
            repo,
            &queue,
            &patch,
            &candidate_abs,
            opts.preflight_command.map(Some),
        )?;
        assert_upstream_layer_preflight(repo, &queue, &candidate_abs, &opts.title)
    })();
    let _ = fs::remove_file(&candidate_abs);
    result
}

#[cfg(test)]
mod tests {
    use super::{is_credential_env, with_upstream_worktree};
    use crate::git::git_ok;

    #[test]
    fn upstream_worktree_is_removed_when_the_closure_panics() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).unwrap();
        git_ok(repo, &["commit", "-q", "--allow-empty", "-m", "init"]).unwrap();
        git_ok(repo, &["branch", "uplink/upstream"]).unwrap();

        let panicked = std::panic::catch_unwind(|| {
            let _ = with_upstream_worktree(repo, |_| -> crate::error::Result<()> {
                panic!("preflight closure failed")
            });
        });
        assert!(panicked.is_err());
        let worktrees = git_ok(repo, &["worktree", "list", "--porcelain"]).unwrap();
        assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
    }

    #[test]
    fn credential_env_names() {
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "UPLINK_INTERNAL_TOKEN",
            "UPLINK_UPSTREAM_TOKEN",
            "UPLINK_CONTRIB_TOKEN",
            "UPLINK_INTERNAL_KEY",
            "UPLINK_CONTRIB_KEY",
        ] {
            assert!(is_credential_env(name), "{name}");
        }
        for name in [
            "UPLINK_PREFLIGHT",
            "UPLINK_EXPORT_AUTHOR",
            "PATH",
            "HOME",
            "MY_TOKEN",
        ] {
            assert!(!is_credential_env(name), "{name}");
        }
    }
}
