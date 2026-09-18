use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use crate::error::{Error, PreflightError, Result};
use crate::git::{GitOpts, git};
use crate::prepare::{depends_on_from_message, export_commit_message};
use crate::queue::{active_patches, get_patch, patch_path, read_queue};
use crate::repo::{ensure_revs, ensure_upstream_ref, has_ref, rev_parse, write_product_patch};
use crate::types::{Patch, QueueState};

fn run_shell(command: &str, cwd: &Path) -> (i32, String) {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .output();
    match output {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.code().unwrap_or(1), text.trim().to_string())
        }
        Err(err) => (1, err.to_string()),
    }
}

fn apply_abs(dir: &Path, patch_abs: &Path, message: &str) -> Result<&'static str> {
    let apply = git(
        dir,
        &[
            "apply",
            "--3way",
            "--index",
            patch_abs.to_str().unwrap_or(""),
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if apply.code != 0 {
        return Ok("conflict");
    }
    let staged = git(
        dir,
        &["diff", "--cached", "--quiet"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )?;
    if staged.code == 0 {
        return Ok("empty");
    }
    git(dir, &["commit", "-m", message], GitOpts::default())?;
    Ok("applied")
}

fn with_upstream_worktree<T>(repo: &Path, f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    ensure_upstream_ref(repo)?;
    if !has_ref(repo, "uplink/upstream")? {
        return Err(Error::msg(
            "No uplink/upstream ref; cannot preflight an export tree.",
        ));
    }
    let dir = std::env::temp_dir().join(format!("uplink-export-{}", Uuid::new_v4()));
    git(
        repo,
        &[
            "worktree",
            "add",
            "--detach",
            "--quiet",
            dir.to_str().unwrap_or(""),
            "uplink/upstream",
        ],
        GitOpts::default(),
    )?;
    let result = f(&dir);
    let _ = git(
        repo,
        &["worktree", "remove", "--force", dir.to_str().unwrap_or("")],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    );
    let _ = fs::remove_dir_all(&dir);
    result
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

fn dep_patch_abs(repo: &Path, id: &str) -> PathBuf {
    repo.join(patch_path(id))
}

fn apply_deps(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    dep_ids: &[String],
) -> Result<&'static str> {
    for id in dep_ids {
        let dep = get_patch(queue, id)?;
        let result = apply_abs(
            dir,
            &dep_patch_abs(repo, id),
            &format!("{} (dep)", dep.title),
        )?;
        if result == "conflict" {
            return Ok("conflict");
        }
    }
    Ok("ok")
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
    let candidates: Vec<String> = active_patches(queue)
        .into_iter()
        .filter(|p| p.intent == "upstream" && p.status != "conflict")
        .map(|p| p.id.clone())
        .collect();
    with_upstream_worktree(repo, |dir| {
        let alone = apply_abs(dir, candidate_abs, candidate_message)?;
        if alone != "conflict" {
            return Ok(Vec::new());
        }
        for id in &candidates {
            reset_export(dir, repo)?;
            if apply_deps(dir, repo, queue, &[id.clone()])? == "conflict" {
                continue;
            }
            if apply_abs(dir, candidate_abs, candidate_message)? != "conflict" {
                return Ok(vec![id.clone()]);
            }
        }
        let mut prefix = Vec::new();
        for id in &candidates {
            prefix.push(id.clone());
            reset_export(dir, repo)?;
            if apply_deps(dir, repo, queue, &prefix)? == "conflict" {
                continue;
            }
            if apply_abs(dir, candidate_abs, candidate_message)? != "conflict" {
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
    if patch.intent == "internal-only" {
        return Ok(());
    }
    let command = match command_override {
        Some(None) => None,
        Some(Some(cmd)) => Some(cmd),
        None => preflight_command_for(queue),
    };

    with_upstream_worktree(repo, |dir| {
        let deps = apply_deps(dir, repo, queue, &patch.depends_on)?;
        if deps == "conflict" {
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
        if applied == "conflict" {
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
    let candidates: Vec<String> = active_patches(queue)
        .into_iter()
        .filter(|item| {
            item.intent == "upstream"
                && item.status != "conflict"
                && item.id != patch.id
                && !patch.depends_on.contains(&item.id)
        })
        .map(|item| item.id.clone())
        .collect();
    let mut prefix = patch.depends_on.clone();
    for id in candidates {
        prefix.push(id);
        reset_export(dir, repo)?;
        if apply_deps(dir, repo, queue, &prefix)? == "conflict" {
            continue;
        }
        if apply_abs(dir, candidate_abs, &patch.title)? == "conflict" {
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
    assert_export_preflight(repo, queue, &patch, &repo.join(patch_path(id)), None)
}

pub struct IncomingPreflight {
    pub title: String,
    pub from_ref: String,
    pub head_ref: String,
    pub depends_on: Vec<String>,
    pub message: Option<String>,
    pub preflight_command: Option<String>,
}

pub fn preflight_incoming_change(repo: &Path, opts: IncomingPreflight) -> Result<()> {
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
        intent: "upstream".into(),
        status: "queued".into(),
        depends_on,
        created_at: String::new(),
        updated_at: String::new(),
        patch_id_stable: None,
        commit_message: String::new(),
        source: Default::default(),
        prepare: None,
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        events: Vec::new(),
    };
    let message = export_commit_message(&patch);
    let shas = ensure_revs(repo, &[&opts.from_ref, &opts.head_ref])?;
    write_product_patch(repo, &id, &shas[0], &message, &shas[1])?;
    let candidate_abs = repo.join(patch_path(&id));
    let result = assert_export_preflight(
        repo,
        &queue,
        &patch,
        &candidate_abs,
        opts.preflight_command.map(Some),
    );
    let _ = fs::remove_file(&candidate_abs);
    result
}
