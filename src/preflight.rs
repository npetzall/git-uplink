use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::assess::{depends_on_from_message, export_commit_message, strip_html_comments};
use crate::error::{Error, PreflightError, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::hooks::{PREFLIGHT_SCRIPT_PATH, hooks_source};
use crate::queue::{
    active_upstream, apply_order_active, apply_order_upstream_layer, dependency_closure,
    direct_dependencies, get_patch, patch_path, read_queue,
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

/// Where the output of `preflight.sh` is shown while the script runs. It is
/// captured for the result either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptEcho {
    Off,
    Stdout,
    /// For a command whose stdout is its result.
    Stderr,
}

static SCRIPT_ECHO: AtomicU8 = AtomicU8::new(ScriptEcho::Off as u8);

/// Sets where every later run of `preflight.sh` in this process shows its
/// output. Off until set, so a caller of the library prints nothing.
pub fn set_script_echo(echo: ScriptEcho) {
    SCRIPT_ECHO.store(echo as u8, Ordering::Relaxed);
}

fn script_echo() -> Box<dyn Write> {
    match SCRIPT_ECHO.load(Ordering::Relaxed) {
        x if x == ScriptEcho::Stdout as u8 => Box::new(io::stdout()),
        x if x == ScriptEcho::Stderr as u8 => Box::new(io::stderr()),
        _ => Box::new(io::sink()),
    }
}

/// Tokens of what the script passed on in this process. A command that
/// tests a tree and then rebuilds to the same one runs the script once.
static PASSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note_passed(token: &str) {
    if let Ok(mut passed) = PASSED.lock() {
        passed.push(token.to_string());
    }
}

fn has_passed(token: &str) -> bool {
    PASSED
        .lock()
        .is_ok_and(|passed| passed.iter().any(|known| known == token))
}

/// Copies `from` to `to` as it arrives and returns all of it.
fn tee(mut from: impl Read, to: &mut dyn Write) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match from.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                captured.extend_from_slice(&chunk[..n]);
                // A closed terminal or pipe must not change the verdict.
                let _ = to.write_all(&chunk[..n]);
                let _ = to.flush();
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    captured
}

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

/// The runner's command files. They are dropped from the environment of
/// `preflight.sh` too: through them the script could set `PATH` or
/// `BASH_ENV` for the later steps of its job, or add job outputs.
fn is_runner_command_file(name: &str) -> bool {
    matches!(
        name,
        "GITHUB_ENV" | "GITHUB_OUTPUT" | "GITHUB_PATH" | "GITHUB_STATE" | "GITHUB_STEP_SUMMARY"
    )
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
    /// What failed, without the script's output.
    pub message: Option<String>,
    pub suggested_depends_on: Vec<String>,
    /// The end of what the script printed. All of it is in the log of the
    /// run that ran it.
    pub output: Option<String>,
    /// What the script ran on, one line each: the base, then what was
    /// applied onto it. See [`export_tested`] and [`command_tested`].
    pub tested: Vec<String>,
    /// Review token of the queued patch the result is for. `submit` accepts
    /// a passing result for unchanged content after upstream or hooks moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_token: Option<String>,
    /// What the script said about the rebuild the command ends with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<Box<RebuildReport>>,
}

/// What `preflight.sh` said about a rebuild. Each part is there when the
/// rebuild needs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RebuildReport {
    /// On the upstream the rebuild starts from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<PreflightReport>,
    /// On the rebuilt tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built: Option<PreflightReport>,
    /// The first patch the script fails from, found by `git bisect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_bad: Option<FirstBad>,
}

impl RebuildReport {
    /// False when the script failed on the upstream or on the rebuilt tree.
    pub fn ok(&self) -> bool {
        [&self.upstream, &self.built]
            .into_iter()
            .flatten()
            .all(|part| part.ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstBad {
    pub id: String,
    /// Names the tree with that patch applied, as the rebuild made it.
    pub token: String,
}

/// A workflow passes the report to a step in one environment variable,
/// which Linux caps at 128 KiB. The script's whole output is in the log of
/// the job that ran it; the report keeps what failed and how it ended.
const REPORT_TEXT_LIMIT: usize = 4 * 1024;
const OUTPUT_TAIL_LINES: usize = 20;

/// The last lines of what the script printed, where a build reports its
/// error, within [`REPORT_TEXT_LIMIT`].
fn output_tail(output: &str) -> String {
    let output = output.trim_end();
    let lines: Vec<&str> = output.lines().collect();
    let kept = lines.len().min(OUTPUT_TAIL_LINES);
    let mut tail = lines[lines.len() - kept..].join("\n");
    if tail.len() > REPORT_TEXT_LIMIT {
        let mut start = tail.len() - REPORT_TEXT_LIMIT;
        while !tail.is_char_boundary(start) {
            start += 1;
        }
        tail = format!("[... line cut ...]{}", &tail[start..]);
    }
    let earlier = lines.len() - kept;
    if earlier == 0 {
        return tail;
    }
    format!("[... {earlier} earlier lines in the job log ...]\n{tail}")
}

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
        Self::of_ref(&result)
    }

    pub(crate) fn of_ref(result: &Result<Option<String>>) -> Self {
        match result {
            Ok(token) => Self {
                ok: true,
                token: token.clone(),
                ..Self::default()
            },
            Err(Error::Preflight(err)) => Self {
                ok: false,
                token: err.token.clone(),
                stage: Some(err.stage.to_string()),
                message: Some(clip(err.summary())),
                suggested_depends_on: err.suggested_depends_on.clone(),
                output: err.output.as_deref().map(output_tail),
                ..Self::default()
            },
            Err(err) => Self {
                ok: false,
                stage: Some("error".into()),
                message: Some(clip(&err.to_string())),
                ..Self::default()
            },
        }
    }

    /// The report of a command that only rebuilds. Not a pass when the
    /// upstream it starts from fails: nothing is rebuilt then. A patch the
    /// script fails on is a pass, and the command gates that patch.
    pub(crate) fn of_rebuild(rebuild: RebuildReport) -> Self {
        let failed = rebuild.upstream.as_ref().filter(|upstream| !upstream.ok);
        Self {
            ok: failed.is_none(),
            stage: failed.and_then(|upstream| upstream.stage.clone()),
            message: failed.and_then(|upstream| upstream.message.clone()),
            rebuild: Some(Box::new(rebuild)),
            ..Self::default()
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
        if self.ok {
            note_passed(token);
        }
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

impl ScriptVerdict {
    /// Where the verdict on one part of the rebuild comes from. A report
    /// without that part is for no tree, so it is refused as stale.
    pub(crate) fn rebuild_part(
        &self,
        pick: impl Fn(&RebuildReport) -> Option<&PreflightReport>,
    ) -> Self {
        match self {
            Self::Run => Self::Run,
            Self::Reported(report) => Self::Reported(
                report
                    .rebuild
                    .as_deref()
                    .and_then(pick)
                    .cloned()
                    .unwrap_or_default(),
            ),
        }
    }

    /// The patch a reported bisect blamed.
    pub(crate) fn first_bad(&self) -> Option<&FirstBad> {
        match self {
            Self::Run => None,
            Self::Reported(report) => report.rebuild.as_deref()?.first_bad.as_ref(),
        }
    }
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
run git uplink init --upgrade to create it and answer Yes to publish it",
                ));
            }
            None => return Ok(None),
        },
    };
    Ok(path_exists_at(repo, &sha, PREFLIGHT_SCRIPT_PATH)?.then_some(sha))
}

/// Names what a verdict is for: the tree under test, of `rev` in `dir`, and
/// the tree of the hooks commit the script (and the files beside it) come
/// from. Not a secret; it only ties a result to its input.
fn preflight_token(dir: &Path, rev: &str, source: Option<&str>) -> Result<String> {
    let tree = rev_parse(dir, &format!("{rev}^{{tree}}"))?;
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
    /// working directory, and shows its output where [`set_script_echo`] said.
    fn run(&self, cwd: &Path) -> (i32, String) {
        self.run_to(cwd, &mut *script_echo())
    }

    /// [`Self::run`] without showing the output, for a run that only probes.
    fn run_quiet(&self, cwd: &Path) -> (i32, String) {
        self.run_to(cwd, &mut io::sink())
    }

    fn run_to(&self, cwd: &Path, echo: &mut dyn Write) -> (i32, String) {
        self.shell(cwd, r#"exec sh "$1" 2>&1"#, echo)
    }

    /// Lets `git bisect` run the script from `bad` back to `good` in `cwd`,
    /// a checkout made for it, and returns the first commit it fails on.
    fn bisect(&self, cwd: &Path, good: &str, bad: &str) -> Result<String> {
        git(cwd, &["bisect", "start", bad, good], GitOpts::default())?;
        // Any failure is "bad": 125 would skip the commit, and 128 and up
        // would stop the bisect.
        let (code, output) = self.shell(
            cwd,
            r#"exec git bisect run sh -c 'sh "$1" || exit 1' sh "$1" 2>&1"#,
            &mut *script_echo(),
        );
        let found = git(
            cwd,
            &["rev-parse", "--verify", "--quiet", "refs/bisect/bad"],
            GitOpts::allow_fail(),
        )?;
        if code != 0 || found.code != 0 || found.stdout.is_empty() {
            return Err(Error::msg(format!(
                "git bisect did not find the patch {PREFLIGHT_SCRIPT_PATH} fails on (exit {code}).\n\n{output}"
            )));
        }
        Ok(found.stdout)
    }

    /// Runs `script` with `sh -c` in `cwd`, the path of `preflight.sh` as
    /// its `$1`, without the credentials a job can hold or the runner's
    /// command files.
    fn shell(&self, cwd: &Path, script: &str, echo: &mut dyn Write) -> (i32, String) {
        // One pipe for stdout and stderr keeps their lines in order.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script, "sh"])
            .arg(&self.path)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        for (name, _) in env::vars_os() {
            if name
                .to_str()
                .is_some_and(|name| is_credential_env(name) || is_runner_command_file(name))
            {
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
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => return (1, err.to_string()),
        };
        let captured = child
            .stdout
            .take()
            .map(|out| tee(out, echo))
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&captured).trim().to_string();
        match child.wait() {
            Ok(status) => (status.code().unwrap_or(1), text),
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

/// Applies `dep_ids` and everything they depend on, dependencies first.
fn apply_deps(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    dep_ids: &[String],
) -> Result<ApplyOutcome> {
    for id in &dependency_closure(queue, dep_ids)? {
        let dep = get_patch(queue, id)?;
        // Already in upstream, possibly in another form than its patch file.
        if dep.status == PatchStatus::Merged {
            continue;
        }
        let result = apply_abs(
            dir,
            &dep_patch_abs(repo, id)?,
            &format!("{} (dep)", dep.subject()),
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

/// The queued patch `patch_id` (in the queue already) needs to apply onto
/// public upstream, as [`detect_depends_on`] finds them; it and the patches
/// that depend on it are not candidates. Empty when none is found.
pub fn suggest_depends_on(
    repo: &Path,
    queue: &QueueState,
    patch_id: &str,
    candidate_abs: &Path,
    title: &str,
) -> Result<Vec<String>> {
    let candidates = upstream_candidates(queue, Some(patch_id))?;
    Ok(
        detect_depends_on_in(repo, queue, &[], &candidates, candidate_abs, title, None)?
            .unwrap_or_default(),
    )
}

/// Active upstream patches that may be a dependency: with `exclude`, not
/// that patch nor one that depends on it, which would be a cycle.
fn upstream_candidates(queue: &QueueState, exclude: Option<&str>) -> Result<Vec<String>> {
    let mut candidates = Vec::new();
    for patch in active_upstream(queue) {
        if let Some(exclude) = exclude
            && (patch.id == exclude
                || dependency_closure(queue, &patch.depends_on)?
                    .iter()
                    .any(|id| id == exclude))
        {
            continue;
        }
        candidates.push(patch.id.clone());
    }
    Ok(candidates)
}

/// The active upstream patches `candidate_abs` needs to apply onto public
/// upstream, direct dependencies only: none when it applies alone, else the
/// patches whose lines its hunks touch (see [`blamed_depends_on`]), else
/// the first set found by trying each patch alone, then growing prefixes.
/// Then every entry it applies without is dropped. `None` when no set
/// makes it apply. With `keep_branch`, that branch is left at upstream +
/// the set + the candidate for inspection.
pub(crate) fn detect_depends_on(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    title: &str,
    keep_branch: Option<&str>,
) -> Result<Option<Vec<String>>> {
    let candidates = upstream_candidates(queue, None)?;
    detect_depends_on_in(
        repo,
        queue,
        &[],
        &candidates,
        candidate_abs,
        title,
        keep_branch,
    )
}

/// The active patches an internal-only `candidate_abs` needs to apply on
/// public upstream with the tooling patch: upstream and internal ones, in
/// the order company main applies them. As [`detect_depends_on`] otherwise.
pub(crate) fn detect_internal_depends_on(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    title: &str,
) -> Result<Option<Vec<String>>> {
    let order = apply_order_active(queue)?;
    let tooling = queue.tooling.as_ref().map(|p| p.id.as_str());
    let (base, candidates): (Vec<String>, Vec<String>) = order
        .into_iter()
        .map(|p| p.id)
        .partition(|id| Some(id.as_str()) == tooling);
    detect_depends_on_in(repo, queue, &base, &candidates, candidate_abs, title, None)
}

/// The smallest subset of `candidates` (kept in their order) that makes
/// `candidate_abs` apply on public upstream with `base` applied first.
fn detect_depends_on_in(
    repo: &Path,
    queue: &QueueState,
    base: &[String],
    candidates: &[String],
    candidate_abs: &Path,
    title: &str,
    keep_branch: Option<&str>,
) -> Result<Option<Vec<String>>> {
    with_upstream_worktree(repo, |dir| {
        let applies_with = |deps: &[String]| -> Result<bool> {
            reset_export(dir, repo)?;
            Ok(
                apply_deps(dir, repo, queue, base)? != ApplyOutcome::Conflict
                    && apply_deps(dir, repo, queue, deps)? != ApplyOutcome::Conflict
                    && apply_abs(dir, candidate_abs, title)? != ApplyOutcome::Conflict,
            )
        };
        let mut found = None;
        if applies_with(&[])? {
            found = Some(Vec::new());
        }
        if found.is_none()
            && let Some(blamed) =
                blamed_depends_on(dir, repo, queue, base, candidates, candidate_abs, title)?
            && applies_with(&blamed)?
        {
            found = Some(blamed);
        }
        for id in candidates {
            if found.is_some() {
                break;
            }
            if applies_with(std::slice::from_ref(id))? {
                found = Some(vec![id.clone()]);
            }
        }
        let mut prefix = Vec::new();
        for id in candidates {
            if found.is_some() {
                break;
            }
            prefix.push(id.clone());
            if applies_with(&prefix)? {
                found = Some(prefix.clone());
            }
        }
        let Some(deps) = found else {
            return Ok(None);
        };
        // What applies stays the same; only the ids other ids bring in go.
        let mut deps = direct_dependencies(queue, &deps)?;
        for i in (0..deps.len()).rev() {
            if deps.len() < 2 {
                break;
            }
            let mut fewer = deps.clone();
            fewer.remove(i);
            if applies_with(&fewer)? {
                deps = fewer;
            }
        }
        if let Some(branch) = keep_branch {
            applies_with(&deps)?;
            git(dir, &["branch", "-f", branch, "HEAD"], GitOpts::default())?;
        }
        Ok(Some(deps))
    })
}

/// The patches among `candidates` whose lines `candidate_abs` changes or
/// keeps as context, as direct dependencies. Applies `base` and every
/// candidate, then the candidate on top, and runs `git blame` on the lines
/// before each hunk of the candidate: a line from a queued patch's commit
/// makes that patch a dependency. Finds dependencies on the text only, not
/// on code the candidate calls. `None` when the candidate does not apply
/// on top of them all, or its diff cannot be read; the caller then probes.
fn blamed_depends_on(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    base: &[String],
    candidates: &[String],
    candidate_abs: &Path,
    title: &str,
) -> Result<Option<Vec<String>>> {
    reset_export(dir, repo)?;
    if apply_deps(dir, repo, queue, base)? == ApplyOutcome::Conflict {
        return Ok(None);
    }
    let stack_base = rev_parse(dir, "HEAD")?;
    let mut by_commit = std::collections::HashMap::new();
    for id in &dependency_closure(queue, candidates)? {
        let patch = get_patch(queue, id)?;
        if patch.status == PatchStatus::Merged {
            continue;
        }
        match apply_abs(dir, &dep_patch_abs(repo, id)?, patch.subject())? {
            ApplyOutcome::Conflict => return Ok(None),
            ApplyOutcome::Empty => {}
            ApplyOutcome::Applied => {
                by_commit.insert(rev_parse(dir, "HEAD")?, id.clone());
            }
        }
    }
    let stack_tip = rev_parse(dir, "HEAD")?;
    if apply_abs(dir, candidate_abs, title)? != ApplyOutcome::Applied {
        return Ok(None);
    }
    let diff = git_ok(
        dir,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "-U3",
            &stack_tip,
            "HEAD",
        ],
    )?;
    let Some(hunks) = pre_image_hunks(&diff) else {
        return Ok(None);
    };
    let range = format!("{stack_base}..{stack_tip}");
    let mut found = Vec::new();
    for (path, start, len) in hunks {
        let lines = format!("{start},+{len}");
        let blame = git_ok(
            dir,
            &["blame", "--porcelain", "-L", &lines, &range, "--", &path],
        )?;
        for sha in blame_commits(&blame) {
            if let Some(id) = by_commit.get(sha)
                && !found.contains(id)
            {
                found.push(id.clone());
            }
        }
    }
    // Back in queue order, then only the ones no other one brings in.
    let ordered: Vec<String> = candidates
        .iter()
        .filter(|id| found.contains(id))
        .cloned()
        .chain(found.iter().filter(|id| !candidates.contains(id)).cloned())
        .collect();
    Ok(Some(direct_dependencies(queue, &ordered)?))
}

/// `(path, first line, line count)` of the pre-image of every hunk in a
/// unified `diff`, for files that existed before. `None` for a path git
/// quoted, which this does not unquote.
fn pre_image_hunks(diff: &str) -> Option<Vec<(String, usize, usize)>> {
    let mut hunks = Vec::new();
    let mut path: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("--- ") {
            // Git ends the name with a tab when it has a space.
            let rest = rest.trim_end_matches('\t');
            path = match rest {
                "/dev/null" => None,
                _ if rest.starts_with('"') => return None,
                _ => Some(rest.strip_prefix("a/").unwrap_or(rest).to_string()),
            };
        } else if let Some(rest) = line.strip_prefix("@@ -") {
            let Some(path) = &path else { continue };
            let old = rest.split(' ').next()?;
            let (start, len) = match old.split_once(',') {
                Some((start, len)) => (start.parse().ok()?, len.parse().ok()?),
                None => (old.parse().ok()?, 1),
            };
            if len > 0 {
                hunks.push((path.clone(), start, len));
            }
        }
    }
    Some(hunks)
}

/// The commit of every line in `git blame --porcelain` output.
fn blame_commits(porcelain: &str) -> impl Iterator<Item = &str> {
    porcelain.lines().filter_map(|line| {
        let sha = line.split(' ').next()?;
        let is_header = !line.starts_with('\t')
            && (sha.len() == 40 || sha.len() == 64)
            && sha.bytes().all(|b| b.is_ascii_hexdigit());
        is_header.then_some(sha)
    })
}

/// Commits listed by [`command_tested`] before the rest is counted.
const TESTED_COMMITS_LIMIT: usize = 50;

fn short_sha(repo: &Path, rev: &str) -> Option<String> {
    git_ok(repo, &["rev-parse", "--short", "--verify", "--quiet", rev])
        .ok()
        .filter(|sha| !sha.is_empty())
}

/// What an export preflight runs `preflight.sh` on, one line each: public
/// upstream, each of `depends_on` that is applied onto it, then `subject`,
/// the change itself. For a report; a line that cannot be read is left out.
pub fn export_tested(
    repo: &Path,
    queue: &QueueState,
    depends_on: &[String],
    subject: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(sha) = short_sha(repo, "uplink/upstream") {
        lines.push(format!("uplink/upstream@{sha}"));
    }
    // What `apply_deps` applies: the dependencies of the dependencies too.
    let applied = dependency_closure(queue, depends_on).unwrap_or_else(|_| depends_on.to_vec());
    for id in &applied {
        match get_patch(queue, id) {
            // Not applied: it is in upstream already. See `apply_deps`.
            Ok(dep) if dep.status == PatchStatus::Merged => {}
            Ok(dep) => lines.push(format!("`{}` {}", dep.id, dep.subject())),
            Err(_) => lines.push(format!("`{id}`")),
        }
    }
    lines.push(subject.to_string());
    lines
}

/// What `--command-only` runs `preflight.sh` on, one line each: public
/// upstream, then the commits `HEAD` has on top of it, oldest first. Only
/// `HEAD` when there is no `uplink/upstream`.
pub fn command_tested(cwd: &Path) -> Vec<String> {
    let base = ["uplink/upstream", "origin/uplink/upstream"]
        .into_iter()
        .find_map(|name| short_sha(cwd, name).map(|sha| (name, sha)));
    let Some((name, sha)) = base else {
        return short_sha(cwd, "HEAD")
            .map(|sha| vec![format!("HEAD@{sha}")])
            .unwrap_or_default();
    };
    let mut lines = vec![format!("uplink/upstream@{sha}")];
    let range = format!("{name}..HEAD");
    let log = git_ok(cwd, &["log", "--oneline", "--reverse", &range]).unwrap_or_default();
    let commits: Vec<&str> = log.lines().collect();
    // The newest are the change under test.
    let skipped = commits.len().saturating_sub(TESTED_COMMITS_LIMIT);
    if skipped > 0 {
        lines.push(format!("… {skipped} earlier commits"));
    }
    lines.extend(commits[skipped..].iter().map(|line| line.to_string()));
    lines
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
/// tested; `None` for a patch that never goes upstream. When the script
/// fails, the queued patches it passes with are suggested as dependencies.
pub fn export_preflight(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    candidate_abs: &Path,
    hooks_ref: Option<&str>,
    verdict: &ScriptVerdict,
) -> Result<Option<String>> {
    export_preflight_with(repo, queue, patch, candidate_abs, hooks_ref, verdict, true)
}

/// [`export_preflight`]; without `suggest_deps` a failing script is not run
/// again with queued patches, so nothing is suggested.
pub(crate) fn export_preflight_with(
    repo: &Path,
    queue: &QueueState,
    patch: &Patch,
    candidate_abs: &Path,
    hooks_ref: Option<&str>,
    verdict: &ScriptVerdict,
    suggest_deps: bool,
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
            let suggested =
                suggest_depends_on(repo, queue, &patch.id, candidate_abs, patch.subject())?;
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Declared depends-on [{}] do not apply onto public upstream before \"{}\".{}",
                    if patch.depends_on.is_empty() {
                        "(none)".into()
                    } else {
                        patch.depends_on.join(", ")
                    },
                    patch.subject(),
                    format_suggestion(&suggested)
                ),
                suggested,
                "apply",
                None,
            )));
        }

        let applied = apply_abs(dir, candidate_abs, patch.subject())?;
        if applied == ApplyOutcome::Conflict {
            let suggested =
                suggest_depends_on(repo, queue, &patch.id, candidate_abs, patch.subject())?;
            let extra = if patch.depends_on.is_empty() {
                " alone".to_string()
            } else {
                format!(" plus {}", patch.depends_on.join(", "))
            };
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Patch \"{}\" does not apply onto public upstream{extra}. Company main is not a valid export base.{}",
                    patch.subject(),
                    format_suggestion(&suggested)
                ),
                suggested,
                "apply",
                None,
            )));
        }

        let token = preflight_token(dir, "HEAD", source.as_deref())?;
        if let ScriptVerdict::Reported(report) = verdict {
            report.replay(&token)?;
            return Ok(Some(token));
        }
        let Some(script) = &script else {
            return Ok(Some(token));
        };
        let (code, output) = script.run(dir);
        if code == 0 {
            note_passed(&token);
            return Ok(Some(token));
        }
        let suggested = if suggest_deps {
            suggest_command_deps(repo, queue, candidate_abs, patch, script, dir)?
        } else {
            Vec::new()
        };
        let extra = if patch.depends_on.is_empty() {
            String::new()
        } else {
            format!(" + {}", patch.depends_on.join(", "))
        };
        Err(Error::Preflight(
            PreflightError::new(
                format!(
                    "Export preflight failed on public upstream{extra} ({PREFLIGHT_SCRIPT_PATH}, exit {code}). No upstream PR should be opened until this passes.{}",
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
    let declared = dependency_closure(queue, &patch.depends_on)?;
    let mut candidates: Vec<String> = upstream_candidates(queue, Some(&patch.id))?
        .into_iter()
        .filter(|id| !declared.contains(id))
        .collect();
    // Patches that change the same files are the likely ones: try them first.
    let touched = patch_paths(candidate_abs);
    candidates.sort_by_key(|id| {
        let shares = dep_patch_abs(repo, id)
            .map(|abs| !patch_paths(&abs).is_disjoint(&touched))
            .unwrap_or(false);
        !shares
    });
    let passes_with = |deps: &[String]| -> Result<Option<bool>> {
        reset_export(dir, repo)?;
        if apply_deps(dir, repo, queue, deps)? == ApplyOutcome::Conflict
            || apply_abs(dir, candidate_abs, patch.subject())? == ApplyOutcome::Conflict
        {
            return Ok(None);
        }
        Ok(Some(script.run_quiet(dir).0 == 0))
    };
    let mut prefix = patch.depends_on.clone();
    let mut passed = false;
    for id in candidates {
        prefix.push(id);
        match passes_with(&prefix)? {
            Some(true) => {
                passed = true;
                break;
            }
            Some(false) => {}
            // Leave out a patch that does not apply with the others.
            None => {
                prefix.pop();
            }
        }
    }
    if !passed {
        return Ok(Vec::new());
    }
    // Drop added patches the script passes without; the one that made it pass stays.
    let mut deps = direct_dependencies(queue, &prefix)?;
    let last = prefix.last().cloned();
    for i in (0..deps.len()).rev() {
        if patch.depends_on.contains(&deps[i]) || Some(&deps[i]) == last.as_ref() {
            continue;
        }
        let mut fewer = deps.clone();
        fewer.remove(i);
        if passes_with(&fewer)? == Some(true) {
            deps = fewer;
        }
    }
    Ok(deps)
}

/// Paths a patch file changes, before and after. Empty when it cannot be read.
fn patch_paths(patch_abs: &Path) -> std::collections::HashSet<String> {
    let Ok(text) = fs::read_to_string(patch_abs) else {
        return Default::default();
    };
    text.lines()
        .filter_map(|line| {
            let name = line
                .strip_prefix("--- a/")
                .or_else(|| line.strip_prefix("+++ b/"))?;
            Some(name.trim_end_matches('\t').to_string())
        })
        .collect()
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
    pub from_ref: String,
    pub head_ref: String,
    pub depends_on: Vec<String>,
    /// The commit message of the incoming change.
    pub message: String,
    /// Read `preflight.sh` from this revision instead of `uplink/hooks`.
    pub hooks_ref: Option<String>,
    pub internal_only: bool,
    /// The queued upstream patch the change amends: it takes that patch's
    /// place in the upstream layer instead of following it.
    pub amends: Option<String>,
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
    let token = preflight_token(cwd, "HEAD", source.as_deref())?;
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
        note_passed(&token);
        return Ok(Some(token));
    }
    Err(Error::Preflight(
        PreflightError::new(
            format!("Preflight failed ({PREFLIGHT_SCRIPT_PATH}, exit {code})."),
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

/// The token a verdict on `rev` has.
pub(crate) fn rev_token(repo: &Path, queue: &QueueState, rev: &str) -> Result<String> {
    let source = script_source(repo, queue, None)?;
    preflight_token(repo, rev, source.as_deref())
}

/// Gets the verdict of `preflight.sh` on the commit `rev`, in a checkout
/// made for it. Returns the token of what was tested.
pub(crate) fn rev_preflight(
    repo: &Path,
    queue: &QueueState,
    rev: &str,
    verdict: &ScriptVerdict,
) -> Result<String> {
    let source = script_source(repo, queue, None)?;
    let token = preflight_token(repo, rev, source.as_deref())?;
    if has_passed(&token) {
        return Ok(token);
    }
    if let ScriptVerdict::Reported(report) = verdict {
        report.replay(&token)?;
        return Ok(token);
    }
    let Some(source) = &source else {
        return Ok(token);
    };
    refuse_script_with_credentials(repo, queue, None)?;
    let script = PreflightScript::materialise(repo, source)?;
    let tree = TempWorktree::add(repo, "uplink-rebuild", rev)?;
    let (code, output) = script.run(&tree.dir);
    if code == 0 {
        note_passed(&token);
        return Ok(token);
    }
    Err(Error::Preflight(
        PreflightError::new(
            format!("Preflight failed ({PREFLIGHT_SCRIPT_PATH}, exit {code})."),
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

/// True for a failure of the script itself, not of what came before it.
pub(crate) fn is_script_failure(err: &PreflightError) -> bool {
    err.stage == STAGE_COMMAND
}

/// The failure of the script on an upstream: `what` says which, and what
/// follows from it.
pub(crate) fn upstream_failure(what: &str, err: PreflightError) -> Error {
    let mut failure = PreflightError::new(what, Vec::new(), STAGE_COMMAND, err.output);
    failure.token = err.token;
    Error::Preflight(failure)
}

/// The first commit of `good..bad` that `preflight.sh` fails on, found with
/// `git bisect run`. `good` passes and `bad` fails.
pub(crate) fn bisect_first_bad(
    repo: &Path,
    queue: &QueueState,
    good: &str,
    bad: &str,
) -> Result<String> {
    let Some(source) = script_source(repo, queue, None)? else {
        return Err(Error::msg(format!(
            "No {PREFLIGHT_SCRIPT_PATH} to bisect with."
        )));
    };
    refuse_script_with_credentials(repo, queue, None)?;
    let script = PreflightScript::materialise(repo, &source)?;
    // Its own checkout, so the bisect state goes away with it.
    let tree = TempWorktree::add(repo, "uplink-bisect", bad)?;
    script.bisect(&tree.dir, good, bad)
}

/// Refuses a reported bisect that is not for the rebuild made here.
pub(crate) fn stale_first_bad(token: &str) -> Error {
    Error::Preflight(
        PreflightError::new(
            "The preflight result blames a patch of another rebuild (the queue, uplink/upstream or uplink/hooks changed). Run preflight again and pass its result.",
            Vec::new(),
            STAGE_STALE,
            None,
        )
        .with_token(token),
    )
}

/// Points `branch` at company main as `queue` builds it: public upstream,
/// then every active patch in apply order. Built in a worktree of its own,
/// so the checkout is not touched.
pub(crate) fn build_company_stack(repo: &Path, queue: &QueueState, branch: &str) -> Result<()> {
    with_upstream_worktree(repo, |dir| {
        for patch in apply_order_active(queue)? {
            if patch.status == PatchStatus::Conflict {
                return Err(Error::msg(format!(
                    "Queue is blocked on conflict in {}; cannot build company main.",
                    patch.id
                )));
            }
            let result = apply_abs(dir, &dep_patch_abs(repo, &patch.id)?, patch.subject())?;
            if result == ApplyOutcome::Conflict {
                return Err(Error::msg(format!(
                    "Queued patch {} (\"{}\") does not apply on company main; fix the queue (git uplink rebuild -i) first.",
                    patch.id,
                    patch.subject()
                )));
            }
        }
        git(dir, &["branch", "-f", branch, "HEAD"], GitOpts::default())?;
        Ok(())
    })
}

pub fn assert_upstream_layer_applies(repo: &Path, queue: &QueueState) -> Result<()> {
    with_upstream_worktree(repo, |dir| apply_upstream_layer(dir, repo, queue, None))
}

/// The change preflight tests on the upstream layer, and the queued patch it
/// amends, which it replaces there.
struct LayerCandidate<'a> {
    file: &'a Path,
    title: &'a str,
    amends: Option<&'a str>,
}

/// Applies tooling + active upstream patches in order onto `dir`, then
/// `candidate`, or `candidate` in place of the patch it amends: on top of
/// the old version an amend would apply the same change twice.
fn apply_upstream_layer(
    dir: &Path,
    repo: &Path,
    queue: &QueueState,
    candidate: Option<&LayerCandidate>,
) -> Result<()> {
    let amends = candidate.and_then(|c| c.amends);
    let mut amended = false;
    for patch in apply_order_upstream_layer(queue)? {
        if let Some(candidate) = candidate.filter(|_| amends == Some(patch.id.as_str())) {
            apply_candidate(dir, candidate)?;
            amended = true;
            continue;
        }
        if patch.status == PatchStatus::Conflict {
            return Err(Error::msg(format!(
                "Queue is blocked on conflict in {}; cannot preflight the upstream layer.",
                patch.id
            )));
        }
        let result = apply_abs(dir, &dep_patch_abs(repo, &patch.id)?, patch.subject())?;
        if result == ApplyOutcome::Conflict {
            let before = candidate
                .filter(|_| !amended)
                .map(|c| format!(" before \"{}\"", c.title))
                .unwrap_or_default();
            return Err(Error::Preflight(PreflightError::new(
                format!(
                    "Queued patch \"{}\" does not apply onto tooling + upstream{before}.",
                    patch.subject()
                ),
                Vec::new(),
                "apply",
                None,
            )));
        }
    }
    match (candidate, amends) {
        (Some(_), Some(id)) if !amended => Err(Error::msg(format!(
            "{id} is not an active upstream patch; cannot preflight an amend of it."
        ))),
        (Some(candidate), None) => apply_candidate(dir, candidate),
        _ => Ok(()),
    }
}

fn apply_candidate(dir: &Path, candidate: &LayerCandidate) -> Result<()> {
    let title = candidate.title;
    if apply_abs(dir, candidate.file, title)? == ApplyOutcome::Conflict {
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
}

/// Applies the upstream layer with `candidate_abs` on it, in place of the
/// patch `amends` names, if any.
pub fn assert_upstream_layer_preflight(
    repo: &Path,
    queue: &QueueState,
    candidate_abs: &Path,
    title: &str,
    amends: Option<&str>,
) -> Result<()> {
    let candidate = LayerCandidate {
        file: candidate_abs,
        title,
        amends,
    };
    with_upstream_worktree(repo, |dir| {
        apply_upstream_layer(dir, repo, queue, Some(&candidate))
    })
}

/// The dependencies of an incoming change: its message trailers and
/// `--depends-on`, and for an amend those of the patch it amends, which
/// `amend --complete` keeps.
pub fn incoming_depends_on(queue: &QueueState, opts: &IncomingPreflight) -> Result<Vec<String>> {
    let mut depends_on = depends_on_from_message(&opts.message, &opts.depends_on);
    if let Some(amends) = opts.amends.as_deref() {
        let Some(amended) = queue.upstream.iter().find(|p| p.id == amends) else {
            return Err(Error::msg(format!(
                "{amends} is not an upstream patch; cannot preflight an amend of it."
            )));
        };
        for dep in &amended.depends_on {
            if !depends_on.contains(dep) {
                depends_on.push(dep.clone());
            }
        }
    }
    Ok(depends_on)
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
    let depends_on = incoming_depends_on(&queue, &opts)?;
    let patch = Patch {
        id: id.clone(),
        status: PatchStatus::Queued,
        depends_on,
        created_at: String::new(),
        updated_at: String::new(),
        patch_id_stable: None,
        commit_message: strip_html_comments(&opts.message),
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
        match assert_upstream_layer_preflight(
            repo,
            &queue,
            &candidate_abs,
            patch.subject(),
            opts.amends.as_deref(),
        ) {
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
#[path = "preflight_test.rs"]
mod tests;
