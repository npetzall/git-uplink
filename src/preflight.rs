use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::assess::{depends_on_from_message, export_commit_message};
use crate::error::{Error, PreflightError, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::hooks::{PREFLIGHT_SCRIPT_PATH, hooks_source};
use crate::queue::{
    active_upstream, apply_order_upstream_layer, get_patch, patch_path, read_queue,
};
use crate::repo::{
    TempWorktree, ensure_revs, ensure_upstream_ref, has_ref, path_exists_at, rev_parse,
    write_product_patch,
};
use crate::types::{ApplyOutcome, Patch, PatchStatus, QueueState};

/// Stage of a [`PreflightError`]: `preflight.sh` exited non-zero.
const STAGE_COMMAND: &str = "command";
/// Stage of a [`PreflightError`]: the supplied result is for another tree.
pub const STAGE_STALE: &str = "stale";

/// Credentials a forge job can hold. They are dropped from the environment
/// of `preflight.sh`, and a CI job that holds one does not run the script at
/// all: only that keeps them from product code.
fn is_credential_env(name: &str) -> bool {
    matches!(
        name,
        "GITHUB_TOKEN"
            | "GH_TOKEN"
            | "GH_ENTERPRISE_TOKEN"
            | "GITHUB_ENTERPRISE_TOKEN"
            | "ACTIONS_RUNTIME_TOKEN"
            | "ACTIONS_ID_TOKEN_REQUEST_TOKEN"
    ) || (name.starts_with("UPLINK_") && (name.ends_with("_TOKEN") || name.ends_with("_KEY")))
}

/// What `preflight.sh` said about one tree. `preflight --json` and
/// `--preflight-only` print it in a job without credentials;
/// `--preflight-result` reads it back, so the command that holds the
/// credentials never runs product code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreflightReport {
    pub ok: bool,
    /// Names the tree under test and the hooks it was tested with. Absent
    /// when no tree was built.
    pub token: Option<String>,
    pub stage: Option<String>,
    pub message: Option<String>,
    pub suggested_depends_on: Vec<String>,
    pub output: Option<String>,
}

/// A job output holds 1 MiB; keep a report well under it.
const REPORT_TEXT_LIMIT: usize = 48 * 1024;

/// `text` with its middle cut out when it is longer than the limit. The
/// start says what failed and the end is where a build reports its error.
fn clip(text: &str) -> String {
    if text.len() <= REPORT_TEXT_LIMIT {
        return text.to_string();
    }
    let mut head = REPORT_TEXT_LIMIT / 8;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - (REPORT_TEXT_LIMIT - REPORT_TEXT_LIMIT / 8);
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}\n[... output cut ...]\n{}", &text[..head], &text[tail..])
}

impl PreflightReport {
    /// A pass or a preflight failure is a report. Any other error is one
    /// too, without a token: the command that reads it fails the same way
    /// before it needs the script, or refuses the report as stale.
    pub fn of(result: Result<Option<String>>) -> Self {
        match result {
            Ok(token) => Self {
                ok: true,
                token,
                ..Self::default()
            },
            Err(Error::Preflight(err)) => Self {
                ok: false,
                token: err.token.clone(),
                stage: Some(err.stage.to_string()),
                message: Some(clip(&err.to_string())),
                suggested_depends_on: err.suggested_depends_on.clone(),
                output: err.output.as_deref().map(clip),
            },
            Err(err) => Self {
                ok: false,
                stage: Some("error".into()),
                message: Some(clip(&err.to_string())),
                ..Self::default()
            },
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path).map_err(|err| {
            Error::msg(format!(
                "could not read the preflight result {}: {err}",
                path.display()
            ))
        })?;
        serde_json::from_str(&raw).map_err(|err| {
            Error::msg(format!(
                "{} is not a preflight result (expected the output of git uplink preflight --json): {err}",
                path.display()
            ))
        })
    }

    /// The verdict this report holds for the tree `token` names.
    fn replay(&self, token: &str) -> Result<()> {
        if self.token.as_deref() != Some(token) {
            let why = match (&self.token, &self.message) {
                (None, Some(message)) => format!(" It reports: {message}"),
                _ => String::new(),
            };
            return Err(Error::Preflight(
                PreflightError::new(
                    format!(
                        "The preflight result is not for the tree this command builds (the queue, uplink/upstream or uplink/hooks changed, or preflight did not get that far). Run preflight again and pass its result.{why}"
                    ),
                    Vec::new(),
                    STAGE_STALE,
                    None,
                )
                .with_token(token),
            ));
        }
        // A token is set once the tree is built, so a failure that is not the
        // script's came after the script passed (the probe checks more than
        // some commands do). The command makes its own checks.
        if self.ok || self.stage.as_deref() != Some(STAGE_COMMAND) {
            return Ok(());
        }
        Err(Error::Preflight(
            PreflightError::new(
                self.message
                    .clone()
                    .unwrap_or_else(|| format!("Preflight failed ({PREFLIGHT_SCRIPT_PATH}).")),
                self.suggested_depends_on.clone(),
                STAGE_COMMAND,
                self.output.clone(),
            )
            .with_token(token),
        ))
    }
}

/// Where a command gets the verdict of `preflight.sh` on the tree it builds.
#[derive(Debug, Clone, Default)]
pub enum ScriptVerdict {
    /// Run the script in this process.
    #[default]
    Run,
    /// Take it from a run made elsewhere. The script is not run here.
    Reported(PreflightReport),
}

/// The `uplink/hooks` commit preflight reads its script from. `None` when
/// there is no script to run. `hooks_ref` stands in for `uplink/hooks`, to
/// try a change to the script before it lands there.
fn script_source(
    repo: &Path,
    queue: &QueueState,
    hooks_ref: Option<&str>,
) -> Result<Option<String>> {
    let sha = match hooks_ref {
        Some(rev) => ensure_revs(repo, &[rev])?.remove(0),
        None => match hooks_source(repo)? {
            Some(sha) => sha,
            // Never pass because there was nothing to run.
            None if queue.config.forge.is_some() => {
                return Err(Error::msg(
                    "uplink/hooks is missing, so preflight has no preflight.sh to run; \
run git uplink init --upgrade to create it, then git uplink push",
                ));
            }
            None => return Ok(None),
        },
    };
    Ok(path_exists_at(repo, &sha, PREFLIGHT_SCRIPT_PATH)?.then_some(sha))
}

/// Names what a verdict is for: the tree under test at `dir`'s `HEAD`, and
/// the tree of the hooks commit the script (and the files beside it) come
/// from. Not a secret; it only ties a result to its input.
fn preflight_token(dir: &Path, source: Option<&str>) -> Result<String> {
    let tree = rev_parse(dir, "HEAD^{tree}")?;
    let hooks = match source {
        Some(sha) => rev_parse(dir, &format!("{sha}^{{tree}}"))?,
        None => "none".to_string(),
    };
    let content = format!("uplink preflight 1\n{tree}\n{hooks}\n");
    Ok(git(
        dir,
        &["hash-object", "--stdin"],
        GitOpts {
            input: Some(content.as_bytes()),
            ..GitOpts::default()
        },
    )?
    .stdout)
}

/// True on a forge runner, where a job's credentials are not the user's own.
fn on_ci_runner(lookup: impl Fn(&str) -> Option<String>) -> bool {
    ["CI", "GITHUB_ACTIONS"]
        .iter()
        .any(|name| lookup(name).is_some_and(|value| !value.is_empty() && value != "false"))
}

fn credentials_held(names: impl IntoIterator<Item = (String, String)>) -> Vec<String> {
    let mut held: Vec<String> = names
        .into_iter()
        .filter(|(name, value)| is_credential_env(name) && !value.is_empty())
        .map(|(name, _)| name)
        .collect();
    held.sort();
    held
}

/// Refuses to run `preflight.sh` in a CI job that holds credentials.
/// Dropping them from the script's environment is not enough there: product
/// code can read them from the parent process, from the runner, or from
/// `.git/config`. On a developer's machine the credentials are the
/// developer's own, and so is the code.
pub fn refuse_script_with_credentials(
    repo: &Path,
    queue: &QueueState,
    hooks_ref: Option<&str>,
) -> Result<()> {
    if queue.config.forge.is_none() || !on_ci_runner(|name| env::var(name).ok()) {
        return Ok(());
    }
    let held = credentials_held(env::vars_os().filter_map(|(name, value)| {
        Some((
            name.into_string().ok()?,
            value.to_string_lossy().into_owned(),
        ))
    }));
    if held.is_empty() || script_source(repo, queue, hooks_ref)?.is_none() {
        return Ok(());
    }
    Err(Error::msg(format!(
        "Not running {PREFLIGHT_SCRIPT_PATH}: this CI job holds {}. The script builds and runs \
product code, which could read them. Run git uplink preflight --json (or the command with \
--preflight-only) in a job without credentials, and pass its output here with --preflight-result.",
        held.join(", ")
    )))
}

/// `preflight.sh` from `uplink/hooks`, checked out with the rest of that
/// commit so the script can reach the files next to it.
struct PreflightScript<'a> {
    path: PathBuf,
    // Dropped with the script, which removes the checkout.
    _hooks: TempWorktree<'a>,
}

impl<'a> PreflightScript<'a> {
    /// Checks out `source`, the commit [`script_source`] found.
    fn materialise(repo: &'a Path, source: &str) -> Result<Self> {
        let hooks = TempWorktree::add(repo, "uplink-hooks", source)?;
        Ok(Self {
            path: hooks.dir.join(PREFLIGHT_SCRIPT_PATH),
            _hooks: hooks,
        })
    }

    /// Runs the script with `cwd`, the root of the tree under test, as its
    /// working directory.
    fn run(&self, cwd: &Path) -> (i32, String) {
        let mut cmd = Command::new("sh");
        cmd.arg(&self.path).current_dir(cwd);
        for (name, _) in env::vars_os() {
            if name.to_str().is_some_and(is_credential_env) {
                cmd.env_remove(name);
            }
        }
        // A checkout with persist-credentials leaves its token in
        // .git/config, which the trees under test share. Blank it for git
        // run by the script. The file itself stays readable: only a job
        // that checked out without credentials closes that.
        let mut blank = vec!["credential.helper".to_string()];
        if let Ok(listed) = git(
            cwd,
            &[
                "config",
                "--name-only",
                "--get-regexp",
                r"^http\..*\.extraheader$",
            ],
            GitOpts::allow_fail(),
        ) {
            blank.extend(listed.stdout.lines().map(str::to_string));
        }
        cmd.env("GIT_CONFIG_COUNT", blank.len().to_string());
        for (i, key) in blank.iter().enumerate() {
            cmd.env(format!("GIT_CONFIG_KEY_{i}"), key)
                .env(format!("GIT_CONFIG_VALUE_{i}"), "");
        }
        match cmd.output() {
            Ok(output) => {
                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                text.push_str(&String::from_utf8_lossy(&output.stderr));
                (output.status.code().unwrap_or(1), text.trim().to_string())
            }
            Err(err) => (1, err.to_string()),
        }
    }
}

pub(crate) fn apply_abs(dir: &Path, patch_abs: &Path, message: &str) -> Result<ApplyOutcome> {
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
    hooks_ref: Option<&str>,
) -> Result<()> {
    export_preflight(
        repo,
        queue,
        patch,
        candidate_abs,
        hooks_ref,
        &ScriptVerdict::Run,
    )
    .map(|_| ())
}

/// Applies `patch` with its dependencies onto public upstream and gets the
/// verdict of `preflight.sh` on that tree. Returns the token of what was
/// tested; `None` for a patch that never goes upstream.
pub fn export_preflight(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    candidate_abs: &Path,
    hooks_ref: Option<&str>,
    verdict: &ScriptVerdict,
) -> Result<Option<String>> {
    if queue.is_internal(&patch.id) || queue.is_tooling(&patch.id) {
        return Ok(None);
    }
    let source = script_source(repo, queue, hooks_ref)?;
    let script = match (verdict, &source) {
        (ScriptVerdict::Run, Some(sha)) => Some(PreflightScript::materialise(repo, sha)?),
        _ => None,
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

        let token = preflight_token(dir, source.as_deref())?;
        if let ScriptVerdict::Reported(report) = verdict {
            report.replay(&token)?;
            return Ok(Some(token));
        }
        let Some(script) = &script else {
            return Ok(Some(token));
        };
        let (code, output) = script.run(dir);
        if code == 0 {
            return Ok(Some(token));
        }
        let suggested = suggest_command_deps(repo, queue, candidate_abs, patch, script, dir)?;
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
        Err(Error::Preflight(
            PreflightError::new(
                format!(
                    "Export preflight failed on public upstream{extra} ({PREFLIGHT_SCRIPT_PATH}, exit {code}). No upstream PR should be opened until this passes.{}{output_suffix}",
                    format_suggestion(&suggested)
                ),
                suggested,
                STAGE_COMMAND,
                if output.is_empty() {
                    None
                } else {
                    Some(output)
                },
            )
            .with_token(token),
        ))
    })
}

fn suggest_command_deps(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    patch: &Patch,
    script: &PreflightScript,
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
        let (code, _) = script.run(dir);
        if code == 0 {
            return Ok(prefix);
        }
    }
    Ok(Vec::new())
}

pub fn preflight_existing_patch(
    repo: &Path,
    queue: &QueueState,
    id: &str,
    hooks_ref: Option<&str>,
) -> Result<()> {
    existing_patch_preflight(repo, queue, id, hooks_ref).map(|_| ())
}

/// [`preflight_existing_patch`], returning the token of what was tested.
pub fn existing_patch_preflight(
    repo: &Path,
    queue: &QueueState,
    id: &str,
    hooks_ref: Option<&str>,
) -> Result<Option<String>> {
    let patch = get_patch(queue, id)?.clone();
    export_preflight(
        repo,
        queue,
        &patch,
        &repo.join(patch_path(id)?),
        hooks_ref,
        &ScriptVerdict::Run,
    )
}

pub struct IncomingPreflight {
    pub title: String,
    pub from_ref: String,
    pub head_ref: String,
    pub depends_on: Vec<String>,
    pub message: Option<String>,
    /// Read `preflight.sh` from this revision instead of `uplink/hooks`.
    pub hooks_ref: Option<String>,
    pub internal_only: bool,
}

/// Runs `preflight.sh` on the checkout `cwd` is in, from its root, with
/// nothing applied.
pub fn run_preflight_command_in(
    queue: &QueueState,
    cwd: &Path,
    hooks_ref: Option<&str>,
) -> Result<()> {
    command_preflight(queue, cwd, hooks_ref, &ScriptVerdict::Run).map(|_| ())
}

/// Gets the verdict of `preflight.sh` on the commit checked out at `cwd`,
/// with nothing applied. Returns the token of what was tested.
pub fn command_preflight(
    queue: &QueueState,
    cwd: &Path,
    hooks_ref: Option<&str>,
    verdict: &ScriptVerdict,
) -> Result<Option<String>> {
    let source = script_source(cwd, queue, hooks_ref)?;
    let token = preflight_token(cwd, source.as_deref())?;
    if let ScriptVerdict::Reported(report) = verdict {
        report.replay(&token)?;
        return Ok(Some(token));
    }
    let Some(source) = &source else {
        return Ok(Some(token));
    };
    let script = PreflightScript::materialise(cwd, source)?;
    let root = git_ok(cwd, &["rev-parse", "--show-toplevel"])?;
    let (code, output) = script.run(Path::new(&root));
    if code == 0 {
        return Ok(Some(token));
    }
    let output_suffix = if output.is_empty() {
        String::new()
    } else {
        format!("\n\n{output}")
    };
    Err(Error::Preflight(
        PreflightError::new(
            format!("Preflight failed ({PREFLIGHT_SCRIPT_PATH}, exit {code}).{output_suffix}"),
            Vec::new(),
            STAGE_COMMAND,
            if output.is_empty() {
                None
            } else {
                Some(output)
            },
        )
        .with_token(token),
    ))
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
    incoming_change_preflight(repo, opts).map(|_| ())
}

/// [`preflight_incoming_change`], returning the token of what was tested.
/// `None` for an internal-only change, which has no export tree.
pub fn incoming_change_preflight(repo: &Path, opts: IncomingPreflight) -> Result<Option<String>> {
    if opts.internal_only {
        return Ok(None);
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
        let token = export_preflight(
            repo,
            &queue,
            &patch,
            &candidate_abs,
            opts.hooks_ref.as_deref(),
            &ScriptVerdict::Run,
        )?;
        // Keep the token: the script passed on that tree, whatever follows.
        match assert_upstream_layer_preflight(repo, &queue, &candidate_abs, &opts.title) {
            Err(Error::Preflight(err)) => Err(Error::Preflight(match &token {
                Some(token) => err.with_token(token),
                None => err,
            })),
            other => other.map(|_| token),
        }
    })();
    let _ = fs::remove_file(&candidate_abs);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "GH_ENTERPRISE_TOKEN",
            "ACTIONS_RUNTIME_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "UPLINK_INTERNAL_TOKEN",
            "UPLINK_UPSTREAM_TOKEN",
            "UPLINK_CONTRIB_TOKEN",
            "UPLINK_INTERNAL_KEY",
            "UPLINK_CONTRIB_KEY",
        ] {
            assert!(is_credential_env(name), "{name}");
        }
        for name in ["UPLINK_SRC", "UPLINK_VERSION", "PATH", "HOME", "MY_TOKEN"] {
            assert!(!is_credential_env(name), "{name}");
        }
    }

    #[test]
    fn credentials_matter_only_on_a_runner_and_only_when_set() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            }
        };
        assert!(on_ci_runner(env(&[("GITHUB_ACTIONS", "true")])));
        assert!(on_ci_runner(env(&[("CI", "1")])));
        assert!(!on_ci_runner(env(&[("CI", "false")])));
        assert!(!on_ci_runner(env(&[("CI", "")])));
        assert!(!on_ci_runner(env(&[])));

        let held = credentials_held([
            ("UPLINK_CONTRIB_TOKEN".to_string(), "x".to_string()),
            ("GH_TOKEN".to_string(), "y".to_string()),
            ("GITHUB_TOKEN".to_string(), String::new()),
            ("PATH".to_string(), "/bin".to_string()),
        ]);
        assert_eq!(held, ["GH_TOKEN", "UPLINK_CONTRIB_TOKEN"]);
    }

    fn repo_with_hooks() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git_ok(repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        git_ok(repo, &["add", "a.txt"]).unwrap();
        git_ok(repo, &["commit", "-q", "-m", "init"]).unwrap();
        dir
    }

    fn commit_file(repo: &std::path::Path, name: &str, body: &str) -> String {
        std::fs::write(repo.join(name), body).unwrap();
        git_ok(repo, &["add", name]).unwrap();
        git_ok(repo, &["commit", "-q", "-m", name]).unwrap();
        git_ok(repo, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn token_follows_the_tree_and_the_hooks_and_nothing_else() {
        let dir = repo_with_hooks();
        let repo = dir.path();
        let first = git_ok(repo, &["rev-parse", "HEAD"]).unwrap();
        let base = preflight_token(repo, None).unwrap();
        assert_eq!(base, preflight_token(repo, None).unwrap());
        assert_ne!(base, preflight_token(repo, Some(&first)).unwrap());

        // Same tree under another commit: same token.
        git_ok(repo, &["commit", "-q", "--allow-empty", "-m", "again"]).unwrap();
        assert_eq!(base, preflight_token(repo, None).unwrap());

        let second = commit_file(repo, "b.txt", "two\n");
        let moved = preflight_token(repo, Some(&first)).unwrap();
        assert_ne!(base, preflight_token(repo, None).unwrap());
        assert_ne!(moved, preflight_token(repo, Some(&second)).unwrap());
    }

    #[test]
    fn a_report_is_replayed_only_for_its_own_tree() {
        let pass = PreflightReport::of(Ok(Some("abc".into())));
        assert!(pass.ok);
        pass.replay("abc").unwrap();
        let Error::Preflight(stale) = pass.replay("def").unwrap_err() else {
            panic!("expected a preflight error");
        };
        assert_eq!(stale.stage, STAGE_STALE);

        let failure = PreflightError::new(
            "tests failed",
            vec!["upl_1".into()],
            STAGE_COMMAND,
            Some("boom".into()),
        )
        .with_token("abc");
        let fail = PreflightReport::of(Err(Error::Preflight(failure)));
        assert!(!fail.ok);
        let round: PreflightReport =
            serde_json::from_str(&serde_json::to_string(&fail).unwrap()).unwrap();
        assert_eq!(round, fail);
        let Error::Preflight(replayed) = round.replay("abc").unwrap_err() else {
            panic!("expected a preflight error");
        };
        assert_eq!(replayed.stage, STAGE_COMMAND);
        assert_eq!(replayed.to_string(), "tests failed");
        assert_eq!(replayed.suggested_depends_on, ["upl_1"]);
        assert_eq!(replayed.output.as_deref(), Some("boom"));

        // The script passed; what failed after it is the command's to check.
        let layer = PreflightError::new("layer does not apply", Vec::new(), "apply", None)
            .with_token("abc");
        let after = PreflightReport::of(Err(Error::Preflight(layer)));
        assert!(!after.ok);
        after.replay("abc").unwrap();
        assert!(after.replay("def").is_err());

        // A report that never reached the script covers no tree.
        let other = PreflightReport::of(Err(Error::msg("queue is blocked")));
        assert!(!other.ok && other.token.is_none());
        let err = other.replay("abc").unwrap_err().to_string();
        assert!(err.contains("queue is blocked"), "{err}");
        assert!(PreflightReport::default().replay("abc").is_err());
    }

    #[test]
    fn long_output_is_cut_in_the_middle() {
        let text = format!("HEAD{}TAIL", "x".repeat(REPORT_TEXT_LIMIT * 2));
        let cut = clip(&text);
        assert!(cut.len() < REPORT_TEXT_LIMIT + 64, "{}", cut.len());
        assert!(cut.starts_with("HEAD") && cut.ends_with("TAIL"));
        assert!(cut.contains("[... output cut ...]"));
        assert_eq!(clip("short"), "short");
    }
}
