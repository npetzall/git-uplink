use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use git_uplink::{
    AddPatchOpts, AdoptGroup, AmendMessage, ApprovalReceipt, CheckStatus, ConflictError,
    DEFAULT_CUTOFF, Error, Forge, GitOpts, HooksPushAction, IncomingPreflight, InitOpts, MergeVia,
    Patch, PatchIntent, PatchStatus, PendingMerge, ProgressMode, PushOpts, QueueConfig, QueueState,
    RebuildOpts, Result, STATE_BRANCH, Settings, SettingsFlags, StepOutcome, SyncOpts,
    TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE, TransferDirection, accept_upstream,
    accept_upstream_at, add_patch, amend_patch, approve_patch, approve_patch_reviewed, doctor,
    drop_patch, extras_dir, format_approval_receipt, format_approver_packet,
    format_contribution_packet, format_contribution_packet_with_extras, format_step_line,
    from_upstream_report_paths, git, git_ok, init, init_repo, load_extra_markdown, mark_merged,
    parse_depends_on, preflight_incoming_change, push_queue, rebuild, rebuild_with,
    record_gated_pr, record_pull_request, refresh_from_origin, report_paths, reset_from_origin,
    resolve_conflict, review_token, status_snapshot, store_patch_extras, stored_extras_fresh,
    strip_html_comments, submit_patch, summarize_queue, sync, sync_with, transfer_patch,
    write_queue,
};
use tempfile::TempDir;

const TOKENS: &str = r#"export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 3600;
}
"#;

fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn write(repo: &Path, file: &str, contents: &str) {
    let full = repo.join(file);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(full, contents).unwrap();
}

fn commit_all(repo: &Path, message: &str) {
    git(repo, &["add", "-A"], GitOpts::default()).unwrap();
    git(repo, &["commit", "-m", message], GitOpts::default()).unwrap();
}

fn commit_contribution_packet(repo: &Path, patch: &Patch) {
    let packet = format_approver_packet(patch);
    let (_, prepare_path, _) = report_paths(&patch.id).unwrap();
    write(repo, &prepare_path, &packet);
    git_uplink::commit_queue(repo, &format!("uplink: contribution packet {}", patch.id)).unwrap();
}

fn sync_apply(repo: &Path) -> QueueState {
    let result = sync(repo).unwrap();
    if result.needs_approval {
        accept_upstream(repo).unwrap().queue
    } else {
        result.queue
    }
}

fn hash_pr_message() -> String {
    format!(
        "Use SHA-256 for tokens\n\n\
<!-- Visible while writing the PR; stripped on import. -->\n\
Replace SHA-1 in the default hasher.\n\n\
{DEFAULT_CUTOFF}\n\n\
Ticket: PROJ-9999\n\
Uplink-Export-Author: Jane Public <jane@users.noreply.github.com>\n"
    )
}

fn create_bare_from(working: &Path) -> PathBuf {
    let bare = temp_dir();
    let path = bare.path().to_path_buf();
    std::mem::forget(bare);
    git(&path, &["init", "--bare", "-b", "main"], GitOpts::default()).unwrap();
    git(
        working,
        &["remote", "add", "tmp-bare", path.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        working,
        &["push", "tmp-bare", "HEAD:main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        working,
        &["remote", "remove", "tmp-bare"],
        GitOpts::default(),
    )
    .unwrap();
    path
}

struct World {
    _upstream_keep: TempDir,
    _company_keep: TempDir,
    upstream: PathBuf,
    company: PathBuf,
}

fn setup_uninitialized() -> World {
    let upstream_keep = temp_dir();
    let upstream = upstream_keep.path().to_path_buf();
    git(&upstream, &["init", "-b", "main"], GitOpts::default()).unwrap();
    write(&upstream, "src/tokens.js", TOKENS);
    write(&upstream, "README.md", "tokenkit\n");
    commit_all(&upstream, "initial tokens");

    let contrib_bare = create_bare_from(&upstream);

    let company_keep = temp_dir();
    let company = company_keep.path().to_path_buf();
    git(&company, &["init", "-b", "main"], GitOpts::default()).unwrap();
    git(
        &company,
        &["remote", "add", "upstream", upstream.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &company,
        &["remote", "add", "contrib", contrib_bare.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &company,
        &[
            "fetch",
            "--quiet",
            "upstream",
            "+refs/heads/main:refs/remotes/upstream/main",
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &company,
        &["checkout", "-b", "main", "upstream/main"],
        GitOpts::default(),
    )
    .unwrap();

    World {
        _upstream_keep: upstream_keep,
        _company_keep: company_keep,
        upstream,
        company,
    }
}

fn setup_world() -> World {
    let world = setup_uninitialized();
    init_repo(&world.company, QueueConfig::default()).unwrap();
    world
}

fn remote_get_url(repo: &Path, name: &str) -> String {
    git_ok(repo, &["remote", "get-url", name]).unwrap()
}

fn keep_dir() -> PathBuf {
    let dir = temp_dir();
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    path
}

/// Commit `uplink.toml` with `edit` applied on `uplink/hooks`, creating the
/// branch in worlds that have none.
fn set_settings(repo: &Path, edit: impl FnOnce(&mut Settings)) {
    let mut settings = Settings::default();
    edit(&mut settings);
    commit_hooks_file(repo, "uplink/hooks", "uplink.toml", &settings.render());
}

/// Makes `body` the commands of `preflight.sh` on `uplink/hooks`.
fn set_preflight_script(repo: &Path, body: &str) {
    commit_hooks_file(repo, "uplink/hooks", "preflight.sh", &format!("{body}\n"));
}

/// Commits `content` as `path` on top of `branch`, creating it when missing.
fn commit_hooks_file(repo: &Path, branch: &str, path: &str, content: &str) {
    let blob = git(
        repo,
        &["hash-object", "-w", "--stdin"],
        GitOpts {
            input: Some(content.as_bytes()),
            ..GitOpts::default()
        },
    )
    .unwrap()
    .stdout;
    let index = keep_dir().join("index");
    let with_index = || GitOpts {
        extra_env: vec![(
            "GIT_INDEX_FILE".into(),
            index.to_string_lossy().into_owned(),
        )],
        ..GitOpts::default()
    };
    let branch_ref = format!("refs/heads/{branch}");
    let parent = has_git_ref(repo, &branch_ref).then(|| rev_of(repo, branch));
    match &parent {
        Some(parent) => git(repo, &["read-tree", parent], with_index()).unwrap(),
        None => git(repo, &["read-tree", "--empty"], with_index()).unwrap(),
    };
    git(
        repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},{path}"),
        ],
        with_index(),
    )
    .unwrap();
    let tree = git(repo, &["write-tree"], with_index()).unwrap().stdout;
    let mut args = vec!["commit-tree", tree.as_str(), "-m", "hooks"];
    if let Some(parent) = &parent {
        args.extend_from_slice(&["-p", parent]);
    }
    let commit = git_ok(repo, &args).unwrap();
    git(
        repo,
        &["update-ref", &branch_ref, &commit],
        GitOpts::default(),
    )
    .unwrap();
}

fn tree_has_uplink(repo: &Path, git_ref: &str) -> bool {
    git(
        repo,
        &["cat-file", "-e", &format!("{git_ref}:.uplink")],
        GitOpts::allow_fail(),
    )
    .unwrap()
    .code
        == 0
}

fn has_git_ref(repo: &Path, git_ref: &str) -> bool {
    let result = git(
        repo,
        &["rev-parse", "--verify", "--quiet", git_ref],
        GitOpts::allow_fail(),
    )
    .unwrap();
    result.code == 0 && !result.stdout.is_empty()
}

fn has_git_object(repo: &Path, git_ref: &str) -> bool {
    git(repo, &["cat-file", "-e", git_ref], GitOpts::allow_fail())
        .unwrap()
        .code
        == 0
}

fn land_on_main(repo: &Path, head_sha: &str) {
    git(
        repo,
        &["checkout", "-f", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let ff = git(
        repo,
        &["merge", "--ff-only", "--quiet", head_sha],
        GitOpts::allow_fail(),
    )
    .unwrap();
    if ff.code != 0 {
        git(
            repo,
            &["merge", "--no-edit", "--quiet", head_sha],
            GitOpts::default(),
        )
        .unwrap();
    }
}

fn add_landed_patch(repo: &Path, mut opts: AddPatchOpts) -> Result<Patch> {
    let from = opts.from_ref.clone().unwrap_or_else(|| "main".into());
    let from_sha = git_ok(repo, &["rev-parse", &from]).unwrap();
    let head_sha = git_ok(
        repo,
        &["rev-parse", opts.head_ref.as_deref().unwrap_or("HEAD")],
    )
    .unwrap();
    land_on_main(repo, &head_sha);
    opts.from_ref = Some(from_sha);
    opts.head_ref = Some(head_sha);
    add_patch(repo, opts)
}

fn work_branch_of(patch: &Patch) -> String {
    patch
        .conflict
        .as_ref()
        .and_then(|c| c.work_branch.clone())
        .unwrap_or_else(|| format!("{}-work", patch.conflict.as_ref().unwrap().branch))
}

#[test]
fn a_stored_state_branch_setting_is_ignored() {
    let world = setup_world();
    let company = &world.company;
    let path = company.join(".uplink/queue.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    raw["config"]["stateBranch"] = "custom/state".into();
    fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

    git_uplink::read_queue(company).unwrap();
    let before = rev_of(company, STATE_BRANCH);
    git_uplink::commit_queue(company, "uplink: legacy config").unwrap();
    assert_ne!(rev_of(company, STATE_BRANCH), before);
    assert!(!has_git_ref(company, "custom/state"));
}

fn rev_of(repo: &Path, git_ref: &str) -> String {
    git_ok(repo, &["rev-parse", git_ref]).unwrap()
}

#[test]
fn init_puts_uplink_on_the_orphan_state_branch_not_main() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{STATE_BRANCH}"),
        ],
        GitOpts::default(),
    )
    .unwrap();
    assert!(company.join(".uplink/queue.json").is_file());
    assert!(!tree_has_uplink(company, "main"));
    let stored = git_ok(
        company,
        &["show", &format!("{STATE_BRANCH}:.uplink/queue.json")],
    )
    .unwrap();
    assert!(stored.contains("\"version\": 1"));
}

fn init_with_recorded_urls(world: &World) -> QueueState {
    init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            ..Default::default()
        },
    )
    .unwrap()
    .queue
}

fn publish_origin(company: &Path) -> PathBuf {
    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &[
            "push",
            "--quiet",
            "origin",
            "main",
            "uplink/state",
            "uplink/upstream",
        ],
        GitOpts::default(),
    )
    .unwrap();
    origin
}

fn clone_company_from(origin: &Path, upstream: &Path) -> (TempDir, PathBuf) {
    let dir_keep = temp_dir();
    let dir = dir_keep.path().to_path_buf();
    git(
        Path::new("/tmp"),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            dir.to_str().unwrap(),
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &dir,
        &[
            "fetch",
            "--quiet",
            "origin",
            "uplink/upstream:uplink/upstream",
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &dir,
        &["fetch", "--quiet", "origin", "uplink/state:uplink/state"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &dir,
        &["remote", "add", "upstream", upstream.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    (dir_keep, dir)
}

const PUSH_MARKER: &str = "UPLINK_TEST_PUSHED";

fn git_dir_of(repo: &Path) -> PathBuf {
    let dot_git = repo.join(".git");
    if dot_git.is_dir() {
        dot_git
    } else {
        repo.to_path_buf()
    }
}

fn remote_refs(repo: &Path) -> String {
    git_ok(repo, &["for-each-ref", "--format=%(refname) %(objectname)"]).unwrap()
}

/// Fails any push into `repo` and leaves a marker behind, so a push is caught
/// even when the caller swallows the rejection.
fn arm_push_tripwire(repo: &Path) -> PathBuf {
    let git_dir = git_dir_of(repo);
    let marker = git_dir.join(PUSH_MARKER);
    let hooks = git_dir.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-receive");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    marker
}

/// Snapshot of remotes that must not receive a push.
struct PushGuard {
    remotes: Vec<(PathBuf, PathBuf, String)>,
}

impl PushGuard {
    fn arm(repos: &[&Path]) -> Self {
        let remotes = repos
            .iter()
            .map(|repo| {
                let marker = arm_push_tripwire(repo);
                (repo.to_path_buf(), marker, remote_refs(repo))
            })
            .collect();
        Self { remotes }
    }

    fn assert_untouched(&self) {
        for (repo, marker, refs) in &self.remotes {
            assert!(!marker.exists(), "push attempted to {}", repo.display());
            assert_eq!(
                &remote_refs(repo),
                refs,
                "refs changed on {}",
                repo.display()
            );
        }
    }
}

fn add_origin(company: &Path) -> PathBuf {
    let origin = create_bare_from(company);
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    origin
}

fn contrib_of(world: &World) -> PathBuf {
    PathBuf::from(remote_get_url(&world.company, "contrib"))
}

#[test]
fn push_tripwire_detects_a_push() {
    let world = setup_uninitialized();
    let origin = add_origin(&world.company);
    init_with_recorded_urls(&world);
    let guard = PushGuard::arm(&[&origin]);
    let pushed = rebuild_with(
        &world.company,
        RebuildOpts {
            push: true,
            push_remote: Some("origin".into()),
            ..Default::default()
        },
    );
    assert!(pushed.is_err(), "tripwire should reject the push");
    assert!(
        std::panic::catch_unwind(|| guard.assert_untouched()).is_err(),
        "guard should report the push"
    );
}

#[test]
fn init_first_run_does_not_push() {
    let world = setup_uninitialized();
    let origin = add_origin(&world.company);
    let contrib = contrib_of(&world);
    let guard = PushGuard::arm(&[&origin, &contrib, &world.upstream]);
    init_with_recorded_urls(&world);
    guard.assert_untouched();
}

#[test]
fn init_adopt_does_not_push() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let origin = add_origin(&world.company);
    let contrib = contrib_of(&world);
    let guard = PushGuard::arm(&[&origin, &contrib, &world.upstream]);
    let queue = init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    assert_eq!(queue.all_patches().count(), 3);
    guard.assert_untouched();
}

#[test]
fn init_on_existing_queue_does_not_push() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let contrib = contrib_of(&world);
    let new_contrib = create_bare_from(&world.upstream);
    let guard = PushGuard::arm(&[&origin, &contrib, &new_contrib, &world.upstream]);
    let queue = init(
        &world.company,
        InitOpts {
            contrib_url: Some(new_contrib.to_str().unwrap().into()),
            ..Default::default()
        },
    )
    .unwrap()
    .queue;
    assert_eq!(
        queue.config.contrib_url.as_deref(),
        Some(new_contrib.to_str().unwrap())
    );
    guard.assert_untouched();
}

#[test]
fn init_without_args_hydrate_does_not_push() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let contrib = contrib_of(&world);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    let guard = PushGuard::arm(&[&origin, &contrib, &world.upstream]);
    init(&clone, InitOpts::default()).unwrap();
    assert!(has_git_ref(&clone, "uplink/upstream"));
    guard.assert_untouched();
}

#[test]
fn init_upgrade_with_tooling_change_does_not_push() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    make_tooling_stale(&world.company, &queue.patch_refs()[0].id);
    let origin = publish_origin(&world.company);
    let contrib = contrib_of(&world);
    let guard = PushGuard::arm(&[&origin, &contrib, &world.upstream]);
    let upgraded = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(upgraded.tooling_changed);
    guard.assert_untouched();
}

#[test]
fn init_cli_does_not_push() {
    let world = setup_uninitialized();
    let origin = add_origin(&world.company);
    let contrib = contrib_of(&world);
    let guard = PushGuard::arm(&[&origin, &contrib, &world.upstream]);
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args([
            "init",
            "--upstream",
            world.upstream.to_str().unwrap(),
            "--contrib",
            contrib.to_str().unwrap(),
            "--forge",
            "github",
        ])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    guard.assert_untouched();
}

#[test]
fn init_records_remote_urls_and_internal_branch() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    assert_eq!(
        queue.config.upstream_url.as_deref(),
        Some(world.upstream.to_str().unwrap())
    );
    assert_eq!(
        queue.config.contrib_url.as_deref(),
        Some(remote_get_url(&world.company, "contrib").as_str())
    );
    assert_eq!(queue.config.internal_branch, "main");
    assert_eq!(
        remote_get_url(&world.company, "upstream"),
        world.upstream.to_str().unwrap()
    );
    let stored = git_ok(
        &world.company,
        &["show", &format!("{STATE_BRANCH}:.uplink/queue.json")],
    )
    .unwrap();
    assert!(stored.contains("\"internalBranch\": \"main\""));
    assert!(stored.contains("\"upstreamUrl\""));
    assert!(stored.contains("\"contribUrl\""));
    assert!(stored.contains("\"forge\": \"github\""));
    assert!(!stored.contains("companyBranch"));
    assert_eq!(queue.config.forge, Some(Forge::Github));
    assert_eq!(queue.all_patches().count(), 1);
    assert_eq!(
        queue.patch_refs()[0].kind.as_deref(),
        Some(TOOLING_PATCH_KIND)
    );
    assert!(queue.is_tooling(&queue.patch_refs()[0].id));
    assert_eq!(queue.patch_refs()[0].title, TOOLING_PATCH_TITLE);
    assert!(
        world
            .company
            .join(".github/workflows/uplink-pr.yml")
            .is_file()
    );
    assert!(
        world
            .company
            .join(".github/pull_request_template.md")
            .is_file()
    );
    assert!(
        world
            .company
            .join(".github/actions/install-git-uplink/action.yml")
            .is_file()
    );
    assert!(!tree_has_uplink(&world.company, "main"));
}

#[test]
fn init_without_args_hydrates_remotes_from_origin_state() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    let remotes = git_ok(&clone, &["remote"]).unwrap();
    assert!(!remotes.split('\n').any(|r| r == "upstream"));
    assert!(!remotes.split('\n').any(|r| r == "contrib"));

    init(&clone, InitOpts::default()).unwrap();
    assert_eq!(
        remote_get_url(&clone, "upstream"),
        world.upstream.to_str().unwrap()
    );
    assert_eq!(
        remote_get_url(&clone, "contrib"),
        remote_get_url(&world.company, "contrib")
    );
    assert!(has_git_ref(&clone, "uplink/upstream"));
    assert!(clone.join(".uplink/queue.json").is_file());
}

#[test]
fn init_without_args_materializes_main_without_leaving_detach() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    git(
        &clone,
        &["checkout", "--quiet", "--detach"],
        GitOpts::default(),
    )
    .unwrap();
    git(&clone, &["branch", "-D", "main"], GitOpts::default()).unwrap();
    let head_before = git_ok(&clone, &["rev-parse", "HEAD"]).unwrap();
    let origin_main = git_ok(&clone, &["rev-parse", "origin/main"]).unwrap();

    init(&clone, InitOpts::default()).unwrap();

    assert_eq!(git_ok(&clone, &["rev-parse", "main"]).unwrap(), origin_main);
    assert_eq!(
        git_ok(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "HEAD"
    );
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]).unwrap(), head_before);
}

#[test]
fn init_without_args_leaves_checked_out_main_alone() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    write(&clone, "local.txt", "not on origin\n");
    commit_all(&clone, "local only");
    let main_before = git_ok(&clone, &["rev-parse", "main"]).unwrap();

    init(&clone, InitOpts::default()).unwrap();

    assert_eq!(git_ok(&clone, &["rev-parse", "main"]).unwrap(), main_before);
    assert_eq!(
        git_ok(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "main"
    );
    assert_eq!(git_ok(&clone, &["rev-parse", "HEAD"]).unwrap(), main_before);
    assert!(clone.join("local.txt").is_file());
}

#[test]
fn init_without_args_fails_when_company_branch_is_missing_on_origin() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    git(
        &origin,
        &["update-ref", "-d", "refs/heads/main"],
        GitOpts::default(),
    )
    .unwrap();

    let err = init(&clone, InitOpts::default()).unwrap_err().to_string();
    assert!(err.contains("Could not fetch origin main"), "{err}");
}

#[test]
fn init_without_args_cli_is_quiet_and_hydrates_remotes() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");

    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .arg("init")
        .current_dir(&clone)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        remote_get_url(&clone, "upstream"),
        world.upstream.to_str().unwrap()
    );
    assert_eq!(
        remote_get_url(&clone, "contrib"),
        remote_get_url(&world.company, "contrib")
    );
}

#[test]
fn init_without_args_json_prints_queue_config() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");

    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(["init", "--json"])
        .current_dir(&clone)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let config: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        config["upstreamUrl"].as_str(),
        Some(world.upstream.to_str().unwrap())
    );
    assert_eq!(
        config["contribUrl"].as_str(),
        Some(remote_get_url(&world.company, "contrib").as_str())
    );
    assert!(config.get("version").is_none(), "{stdout}");
}

#[test]
fn init_without_args_fails_when_state_is_missing() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();
    let missing_origin = init(repo, InitOpts::default()).unwrap_err().to_string();
    assert!(
        missing_origin.contains("origin remote is missing"),
        "{missing_origin}"
    );

    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    let missing_state = init(repo, InitOpts::default()).unwrap_err().to_string();
    assert!(missing_state.contains("not initialized"), "{missing_state}");
}

#[test]
fn reset_from_origin_matches_moved_refs_and_discards_local_work() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");

    git(
        &world.company,
        &["checkout", "--quiet", "--detach", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "UPSTREAM.md", "moved upstream\n");
    commit_all(&world.company, "move uplink/upstream");
    git(
        &world.company,
        &["branch", "-f", "uplink/upstream", "HEAD"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["checkout", "-f", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "MAIN.md", "moved main\n");
    commit_all(&world.company, "move main");
    write(&world.company, ".uplink/reports/note.md", "moved state\n");
    git_uplink::commit_queue(&world.company, "uplink: move state").unwrap();
    git(
        &world.company,
        &[
            "push",
            "--quiet",
            "origin",
            "main",
            "uplink/state",
            "uplink/upstream",
        ],
        GitOpts::default(),
    )
    .unwrap();
    let origin_main = git_ok(&world.company, &["rev-parse", "main"]).unwrap();
    let origin_state = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    let origin_upstream = git_ok(&world.company, &["rev-parse", "uplink/upstream"]).unwrap();
    let origin_queue = git_ok(
        &world.company,
        &["show", &format!("{STATE_BRANCH}:.uplink/queue.json")],
    )
    .unwrap();

    git(
        &clone,
        &["fetch", "--quiet", "origin", "uplink/state:uplink/state"],
        GitOpts::default(),
    )
    .unwrap();
    write(&clone, ".uplink/local-only.md", "stale state\n");
    git_uplink::commit_queue(&clone, "uplink: local only").unwrap();
    git(&clone, &["checkout", "-b", "feat/wip"], GitOpts::default()).unwrap();
    write(&clone, "README.md", "dirty working tree\n");

    let result = reset_from_origin(&clone).unwrap();
    assert_eq!(result.internal_branch, "main");
    assert_eq!(result.internal_sha, origin_main);
    assert_eq!(result.state_sha, origin_state);
    assert_eq!(result.upstream_sha, origin_upstream);
    assert_eq!(
        git_ok(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "main"
    );
    assert_eq!(git_ok(&clone, &["rev-parse", "main"]).unwrap(), origin_main);
    assert_eq!(
        git_ok(&clone, &["rev-parse", STATE_BRANCH]).unwrap(),
        origin_state
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "uplink/upstream"]).unwrap(),
        origin_upstream
    );
    assert!(clone.join("MAIN.md").is_file());
    assert_ne!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "dirty working tree\n"
    );
    assert!(!clone.join(".uplink/local-only.md").is_file());
    assert!(clone.join(".uplink/queue.json").is_file());
    assert_eq!(
        git_ok(
            &clone,
            &["show", &format!("{STATE_BRANCH}:.uplink/queue.json")]
        )
        .unwrap(),
        origin_queue
    );
}

#[test]
fn reset_from_origin_fails_without_origin_or_state() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();
    let missing_origin = reset_from_origin(repo).unwrap_err().to_string();
    assert!(
        missing_origin.contains("origin remote is missing"),
        "{missing_origin}"
    );

    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    let missing_state = reset_from_origin(repo).unwrap_err().to_string();
    assert!(missing_state.contains("not initialized"), "{missing_state}");
}

#[test]
fn refresh_from_origin_updates_tracking_without_moving_local() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    git(
        &clone,
        &["fetch", "--quiet", "origin", "uplink/state:uplink/state"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &clone,
        &[
            "fetch",
            "--quiet",
            "origin",
            "uplink/upstream:uplink/upstream",
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &clone,
        &[
            "restore",
            "--source",
            STATE_BRANCH,
            "--worktree",
            "--",
            ".uplink",
        ],
        GitOpts::default(),
    )
    .unwrap();
    write(&clone, ".uplink/local-only.md", "stale state\n");
    git_uplink::commit_queue(&clone, "uplink: local only").unwrap();
    git(&clone, &["checkout", "-b", "feat/wip"], GitOpts::default()).unwrap();
    write(&clone, "README.md", "dirty working tree\n");

    let local_main = git_ok(&clone, &["rev-parse", "main"]).unwrap();
    let local_state = git_ok(&clone, &["rev-parse", STATE_BRANCH]).unwrap();
    let local_upstream = git_ok(&clone, &["rev-parse", "uplink/upstream"]).unwrap();

    git(
        &world.company,
        &["checkout", "--quiet", "--detach", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "UPSTREAM.md", "moved upstream\n");
    commit_all(&world.company, "move uplink/upstream");
    git(
        &world.company,
        &["branch", "-f", "uplink/upstream", "HEAD"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["checkout", "-f", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "MAIN.md", "moved main\n");
    commit_all(&world.company, "move main");
    write(&world.company, ".uplink/reports/note.md", "moved state\n");
    git_uplink::commit_queue(&world.company, "uplink: move state").unwrap();
    git(
        &world.company,
        &[
            "push",
            "--quiet",
            "origin",
            "main",
            "uplink/state",
            "uplink/upstream",
        ],
        GitOpts::default(),
    )
    .unwrap();
    let origin_main = git_ok(&world.company, &["rev-parse", "main"]).unwrap();
    let origin_state = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    let origin_upstream = git_ok(&world.company, &["rev-parse", "uplink/upstream"]).unwrap();
    assert_ne!(local_state, origin_state);
    assert_ne!(local_main, origin_main);
    assert_ne!(local_upstream, origin_upstream);

    let result = refresh_from_origin(&clone).unwrap();
    assert_eq!(result.internal_branch, "main");
    assert_eq!(result.internal_sha, origin_main);
    assert_eq!(result.state_sha, origin_state);
    assert_eq!(result.upstream_sha, origin_upstream);
    assert_eq!(
        git_ok(&clone, &["rev-parse", "origin/main"]).unwrap(),
        origin_main
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "origin/uplink/state"]).unwrap(),
        origin_state
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "origin/uplink/upstream"]).unwrap(),
        origin_upstream
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "feat/wip"
    );
    assert_eq!(git_ok(&clone, &["rev-parse", "main"]).unwrap(), local_main);
    assert_eq!(
        git_ok(&clone, &["rev-parse", STATE_BRANCH]).unwrap(),
        local_state
    );
    assert_eq!(
        git_ok(&clone, &["rev-parse", "uplink/upstream"]).unwrap(),
        local_upstream
    );
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "dirty working tree\n"
    );
    assert!(clone.join(".uplink/local-only.md").is_file());
    assert!(!clone.join("MAIN.md").is_file());
}

#[test]
fn refresh_from_origin_fails_without_origin_or_state() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();
    let missing_origin = refresh_from_origin(repo).unwrap_err().to_string();
    assert!(
        missing_origin.contains("origin remote is missing"),
        "{missing_origin}"
    );

    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    let missing_state = refresh_from_origin(repo).unwrap_err().to_string();
    assert!(missing_state.contains("not initialized"), "{missing_state}");
}

#[test]
fn queue_at_remote_differs_from_local_after_add() {
    let world = setup_world();
    let company = &world.company;
    let origin = publish_origin(company);
    let (_keep, clone) = clone_company_from(&origin, &world.upstream);

    git(
        &clone,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&clone, "README.md", "local only\n");
    commit_all(&clone, "readme");
    let patch = add_landed_patch(
        &clone,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let local = git_uplink::queue_at(&clone, STATE_BRANCH).unwrap();
    let remote = git_uplink::queue_at(&clone, "origin/uplink/state").unwrap();
    assert!(local.all_patches().any(|p| p.id == patch.id));
    assert!(!remote.all_patches().any(|p| p.id == patch.id));
}

#[test]
fn file_history_lists_patch_revisions() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "README.md", "first\n");
    commit_all(company, "readme");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let path = format!(".uplink/patches/{}.patch", patch.id);
    let first = git_uplink::file_history(company, STATE_BRANCH, &path).unwrap();
    assert!(!first.is_empty(), "{first:?}");

    let mut body = fs::read_to_string(company.join(&path)).unwrap();
    body.push_str("+extra\n");
    write(company, &path, &body);
    git_uplink::commit_queue(company, "uplink: revise patch").unwrap();
    let history = git_uplink::file_history(company, STATE_BRANCH, &path).unwrap();
    assert_eq!(history.len(), first.len() + 1, "{history:?}");
    assert_eq!(history[0].subject, "uplink: revise patch");
    assert_ne!(history[0].sha, first[0].sha);
}

#[test]
fn init_with_matching_args_is_a_noop_on_the_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    init_with_recorded_urls(&world);
    let after = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_eq!(before, after);
}

#[test]
fn init_rejects_remote_or_branch_renames_on_an_existing_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let err = init(
        &world.company,
        InitOpts {
            upstream_remote_name: Some("public".into()),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("upstreamRemote"), "{err}");
    assert!(err.contains("stored \"upstream\""), "{err}");
    assert!(err.contains("requested \"public\""), "{err}");

    let err = init(
        &world.company,
        InitOpts {
            internal_branch: Some("trunk".into()),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("internalBranch"), "{err}");
}

#[test]
fn init_updates_recorded_urls_on_an_existing_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let new_upstream = keep_dir();
    git(
        &new_upstream,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let queue = init(
        &world.company,
        InitOpts {
            upstream_url: Some(new_upstream.to_str().unwrap().into()),
            ..Default::default()
        },
    )
    .unwrap()
    .queue;
    assert_eq!(
        queue.config.upstream_url.as_deref(),
        Some(new_upstream.to_str().unwrap())
    );
    assert_eq!(
        remote_get_url(&world.company, "upstream"),
        new_upstream.to_str().unwrap()
    );
}

#[test]
fn init_requires_forge_when_creating_a_queue() {
    let world = setup_uninitialized();
    let err = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("--forge"), "{err}");
}

#[test]
fn init_example_github_includes_install_action_and_shared_pr_template() {
    let world = setup_uninitialized();
    let queue = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::TryItOnGithub),
            ..Default::default()
        },
    )
    .unwrap()
    .queue;
    assert_eq!(queue.config.forge, Some(Forge::TryItOnGithub));
    assert!(
        world
            .company
            .join(".github/actions/install-git-uplink/action.yml")
            .is_file()
    );
    let pack_template =
        fs::read_to_string("templates/github/.github/pull_request_template.md").unwrap();
    let installed =
        fs::read_to_string(world.company.join(".github/pull_request_template.md")).unwrap();
    assert_eq!(installed, pack_template);
}

#[test]
fn init_without_args_does_not_rewrite_workflows() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    fs::remove_dir_all(clone.join(".github")).unwrap();
    assert!(!clone.join(".github/workflows/uplink-pr.yml").is_file());
    init(&clone, InitOpts::default()).unwrap();
    assert!(!clone.join(".github/workflows/uplink-pr.yml").is_file());
}

#[test]
fn former_forge_names_still_work() {
    let world = setup_uninitialized();
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args([
            "init",
            "--upstream",
            world.upstream.to_str().unwrap(),
            "--contrib",
            &remote_get_url(&world.company, "contrib"),
            "--forge",
            "ghec",
        ])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let company = &world.company;

    // A queue written by an older binary names the forge `ghec`.
    let path = company.join(".uplink/queue.json");
    let stored = fs::read_to_string(&path).unwrap();
    assert!(stored.contains("\"forge\": \"github\""), "{stored}");
    fs::write(
        &path,
        stored.replace("\"forge\": \"github\"", "\"forge\": \"ghec\""),
    )
    .unwrap();
    git_uplink::commit_queue(company, "uplink: old forge name").unwrap();

    let upgraded = init(
        company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upgraded.queue.config.forge, Some(Forge::Github));
    let example: Forge = serde_json::from_str("\"example-github\"").unwrap();
    assert_eq!(example, Forge::TryItOnGithub);
}

#[test]
fn init_rejects_forge_renames_on_an_existing_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let err = init(
        &world.company,
        InitOpts {
            forge: Some(Forge::TryItOnGithub),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("forge"), "{err}");
    assert!(err.contains("\"github\""), "{err}");
    assert!(err.contains("try-it-on-github"), "{err}");
}

/// Replace the stored tooling patch with one that writes a stale workflow, so
/// the next `init --upgrade` sees a changed pack and rebuilds.
fn make_tooling_stale(company: &Path, id: &str) {
    let original = git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
    git(
        company,
        &["checkout", "--quiet", "--detach", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, ".github/workflows/uplink-pr.yml", "stale\n");
    git(company, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        company,
        &["commit", "-m", "stale tooling"],
        GitOpts::default(),
    )
    .unwrap();
    let stale = git_ok(company, &["format-patch", "--full-index", "-1", "--stdout"]).unwrap();
    git(
        company,
        &["checkout", "-f", "--quiet", &original],
        GitOpts::default(),
    )
    .unwrap();
    fs::write(company.join(format!(".uplink/patches/{id}.patch")), stale).unwrap();
    let mut queue = git_uplink::read_queue(company).unwrap();
    queue.tooling.as_mut().unwrap().patch_id_stable = Some("stale".into());
    git_uplink::write_queue(company, &queue).unwrap();
    git_uplink::commit_queue(company, "uplink: stale tooling patch").unwrap();
}

#[test]
fn init_upgrade_refreshes_the_same_tooling_patch() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let id = queue.patch_refs()[0].id.clone();
    assert_eq!(
        queue.patch_refs()[0].kind.as_deref(),
        Some(TOOLING_PATCH_KIND)
    );
    make_tooling_stale(&world.company, &id);

    let upgraded = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upgraded.all_patches().count(), 1);
    assert_eq!(upgraded.patch_refs()[0].id, id);
    assert_eq!(
        upgraded.patch_refs()[0].kind.as_deref(),
        Some(TOOLING_PATCH_KIND)
    );
    assert_ne!(
        upgraded.patch_refs()[0].patch_id_stable.as_deref(),
        Some("stale")
    );
    let prepare =
        fs::read_to_string(world.company.join(".github/workflows/uplink-pr.yml")).unwrap();
    assert!(prepare.contains("name: Uplink PR checks"), "{prepare}");
    assert!(
        prepare.contains("name: Uplink upstream assess"),
        "{prepare}"
    );
    assert!(
        prepare.contains("GH_REPO: ${{ github.repository }}"),
        "{prepare}"
    );
    assert!(prepare.contains("git uplink preflight"), "{prepare}");
    assert!(!prepare.trim().eq("stale"));
    let patch_file = fs::read_to_string(tooling_patch_path(&world.company, &id)).unwrap();
    assert!(
        patch_file.contains(&format!("Uplink-Patch-Id: {id}")),
        "{patch_file}"
    );
    assert!(
        !upgraded.patch_refs()[0]
            .commit_message
            .contains("Uplink-Patch-Id"),
        "{}",
        upgraded.patch_refs()[0].commit_message
    );
}

#[test]
fn init_upgrade_is_a_noop_when_the_pack_matches() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let id = queue.patch_refs()[0].id.clone();
    let stable = queue.patch_refs()[0].patch_id_stable.clone();
    let before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    let upgraded = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    let after = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_eq!(before, after);
    assert_eq!(upgraded.patch_refs()[0].id, id);
    assert_eq!(upgraded.patch_refs()[0].patch_id_stable, stable);
}

#[test]
fn init_upgrade_cli_reports_already_up_to_date() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(["init", "--upgrade"])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Uplink init: ready\nalready up-to-date\nPublish hooks: git uplink push\n"
    );
    let after = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_eq!(before, after);
}

fn run_version(args: &[&str]) -> String {
    let dir = temp_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(args)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn version_subcommand_prints_version_outside_a_repo() {
    let stdout = run_version(&["version"]);
    let prefix = format!("git-uplink {} (", env!("CARGO_PKG_VERSION"));
    assert!(stdout.starts_with(&prefix), "{stdout}");
    assert!(stdout.ends_with(")\n"), "{stdout}");
}

#[test]
fn version_flag_matches_subcommand() {
    assert_eq!(run_version(&["--version"]), run_version(&["version"]));
    assert_eq!(run_version(&["-V"]), run_version(&["version"]));
}

#[test]
fn init_upgrade_cli_reports_a_tooling_refresh() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let id = queue.patch_refs()[0].id.clone();
    fs::write(tooling_patch_path(&world.company, &id), "stale\n").unwrap();
    git_uplink::commit_queue(&world.company, "uplink: stale tooling patch").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(["init", "--upgrade"])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Uplink init: ready\n\
tooling has been updated\n\
Company main was rebuilt locally. Nothing was pushed.\n\
Inspect with: git diff origin/main main\n\
Publish state: git uplink push\n\
Publish main: git uplink rebuild --push\n\
Publish hooks: git uplink push\n"
    );
}

#[test]
fn init_stamps_tooling_patch_id_and_rebuild_leaves_it() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let patch = &queue.patch_refs()[0];
    let path = tooling_patch_path(&world.company, &patch.id);
    let before = fs::read_to_string(&path).unwrap();
    let trailer = format!("Uplink-Patch-Id: {}", patch.id);
    assert_eq!(before.matches(&trailer).count(), 1, "{before}");
    assert!(
        !patch.commit_message.contains("Uplink-Patch-Id"),
        "{}",
        patch.commit_message
    );
    rebuild(&world.company).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn init_upgrade_stamps_a_missing_tooling_patch_id_once() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let id = queue.patch_refs()[0].id.clone();
    let path = tooling_patch_path(&world.company, &id);
    let stripped = strip_patch_id_trailer(&fs::read_to_string(&path).unwrap());
    assert!(!stripped.contains("Uplink-Patch-Id:"));
    fs::write(&path, &stripped).unwrap();
    git_uplink::commit_queue(&world.company, "uplink: drop tooling trailer").unwrap();

    let upgraded = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upgraded.patch_refs()[0].id, id);
    let stamped = fs::read_to_string(&path).unwrap();
    assert_eq!(
        stamped.matches(&format!("Uplink-Patch-Id: {id}")).count(),
        1,
        "{stamped}"
    );
    let before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    let again = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    let after = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_eq!(before, after);
    assert_eq!(again.patch_refs()[0].id, id);
    assert_eq!(fs::read_to_string(&path).unwrap(), stamped);
}

#[test]
fn init_upgrade_ignores_tooling_from_and_date_headers() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let id = queue.patch_refs()[0].id.clone();
    let path = tooling_patch_path(&world.company, &id);
    let rewritten = rewrite_mbox_from_and_date(&fs::read_to_string(&path).unwrap());
    assert_ne!(rewritten, fs::read_to_string(&path).unwrap());
    fs::write(&path, &rewritten).unwrap();
    git_uplink::commit_queue(&world.company, "uplink: rewrite tooling mbox headers").unwrap();
    let before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();

    let upgraded = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    let after = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_eq!(before, after);
    assert_eq!(upgraded.patch_refs()[0].id, id);
    assert_eq!(fs::read_to_string(&path).unwrap(), rewritten);
}

fn tooling_patch_path(repo: &Path, id: &str) -> PathBuf {
    repo.join(format!(".uplink/patches/{id}.patch"))
}

fn strip_patch_id_trailer(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with("Uplink-Patch-Id:") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn rewrite_mbox_from_and_date(text: &str) -> String {
    let mut out = String::new();
    let mut headers = true;
    let mut first = true;
    for line in text.lines() {
        if first {
            first = false;
            out.push_str(
                "From 0123456789abcdef0123456789abcdef01234567 Mon Sep 17 00:00:00 2001\n",
            );
            continue;
        }
        if headers {
            if line.is_empty() {
                headers = false;
                out.push('\n');
                continue;
            }
            if line.starts_with("Date:") {
                out.push_str("Date: Thu, 1 Jan 1970 00:00:00 +0000\n");
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[test]
fn init_upgrade_before_init_errors() {
    let world = setup_uninitialized();
    let err = init(
        &world.company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not initialized"), "{err}");
}

fn rev(repo: &Path) -> String {
    git_ok(repo, &["rev-parse", "HEAD"]).unwrap()
}

fn adopt_group(commits: &[&str], title: &str, intent: PatchIntent) -> AdoptGroup {
    AdoptGroup {
        commits: commits.iter().map(|s| s.to_string()).collect(),
        title: title.into(),
        intent,
        message: None,
    }
}

fn init_adopt(world: &World, groups: Vec<AdoptGroup>) -> QueueState {
    init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            adopt_groups: Some(groups),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap()
    .queue
}

fn three_linear_ahead(company: &Path) -> [String; 3] {
    write(company, "src/metrics.js", "export const n = 1;\n");
    commit_all(company, "Add metrics collector");
    let a = rev(company);
    write(company, "src/metrics.js", "export const n = 2;\n");
    commit_all(company, "Wire collector into server");
    let b = rev(company);
    write(company, "src/dash.js", "export const dash = true;\n");
    commit_all(company, "Vendor grafana dashboards");
    let c = rev(company);
    [a, b, c]
}

#[test]
fn init_adopts_linear_history_without_moving_main() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let main_before = rev(&world.company);
    let queue = init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    assert_eq!(rev(&world.company), main_before);
    assert_eq!(queue.all_patches().count(), 3);
    assert_eq!(
        queue.patch_refs()[0].kind.as_deref(),
        Some(TOOLING_PATCH_KIND)
    );
    assert_eq!(queue.patch_refs()[1].title, "Metrics");
    assert!(queue.is_upstream(&queue.patch_refs()[1].id));
    assert_eq!(queue.patch_refs()[2].title, "Dashboards");
    assert!(queue.is_internal(&queue.patch_refs()[2].id));
    assert!(
        queue.patch_refs()[1]
            .source
            .note
            .as_deref()
            .unwrap()
            .starts_with("adopted from ")
    );
    assert!(queue.last_sync.is_none());
    assert!(!has_git_ref(&world.company, "uplink/adopt-from"));
    assert!(
        !world
            .company
            .join(".github/workflows/uplink-pr.yml")
            .is_file()
    );
}

#[test]
fn rebuild_preview_branch_leaves_main_and_queue_alone() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let original = rev(&world.company);
    init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    let queue_before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    rebuild_with(
        &world.company,
        RebuildOpts {
            branch: Some("uplink/preview/verify".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rev(&world.company), original);
    assert_eq!(
        git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap(),
        queue_before
    );
    assert!(has_git_ref(&world.company, "uplink/preview/verify"));
    let diff = git(
        &world.company,
        &[
            "diff",
            "--quiet",
            original.as_str(),
            "uplink/preview/verify",
            "--",
            ".",
            ":!.github",
        ],
        GitOpts::allow_fail(),
    )
    .unwrap();
    assert_eq!(diff.code, 0, "{}", diff.stderr);
    git(
        &world.company,
        &[
            "cat-file",
            "-e",
            "uplink/preview/verify:.github/workflows/uplink-pr.yml",
        ],
        GitOpts::default(),
    )
    .unwrap();
}

#[test]
fn rebuild_preview_refuses_other_branches_and_push() {
    let world = setup_world();
    let company = &world.company;
    git(company, &["branch", "develop"], GitOpts::default()).unwrap();
    let develop = rev_of(company, "develop");

    let err = rebuild_with(
        company,
        RebuildOpts {
            branch: Some("develop".into()),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("uplink/preview/<name>"), "{err}");
    assert_eq!(rev_of(company, "develop"), develop);

    let err = rebuild_with(
        company,
        RebuildOpts {
            branch: Some("uplink/preview/x".into()),
            push: true,
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("not pushing preview"), "{err}");
    assert!(!has_git_ref(company, "uplink/preview/x"));
}

#[test]
fn rebuild_leaves_stored_patch_bytes_unchanged() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let path = company.join(format!(".uplink/patches/{}.patch", patch.id));
    let before = fs::read(&path).unwrap();
    rebuild(company).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn rebuild_after_adopt_replays_onto_main() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let original = rev(&world.company);
    init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    rebuild(&world.company).unwrap();
    assert_ne!(rev(&world.company), original);
    assert!(
        world
            .company
            .join(".github/workflows/uplink-pr.yml")
            .is_file()
    );
    assert_eq!(
        fs::read_to_string(world.company.join("src/metrics.js")).unwrap(),
        "export const n = 2;\n"
    );
    assert_eq!(
        fs::read_to_string(world.company.join("src/dash.js")).unwrap(),
        "export const dash = true;\n"
    );
}

#[test]
fn init_adopts_each_merge_commit_as_a_patch() {
    let world = setup_uninitialized();
    git(
        &world.company,
        &["checkout", "-b", "feat/one"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "src/one.js", "export const one = 1;\n");
    commit_all(&world.company, "one feature");
    git(&world.company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        &world.company,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Merge pull request #12 from feat/one",
            "feat/one",
        ],
        GitOpts::default(),
    )
    .unwrap();
    let m1 = rev(&world.company);
    git(
        &world.company,
        &["checkout", "-b", "feat/two"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "src/two.js", "export const two = 2;\n");
    commit_all(&world.company, "two feature");
    git(&world.company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        &world.company,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Merge pull request #14 from feat/two",
            "feat/two",
        ],
        GitOpts::default(),
    )
    .unwrap();
    let m2 = rev(&world.company);
    let side = git_ok(&world.company, &["rev-parse", "feat/one"]).unwrap();
    let side_err = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            adopt_groups: Some(vec![adopt_group(&[&side], "Side", PatchIntent::Upstream)]),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(side_err.contains("first-parent"), "{side_err}");
    let queue = init_adopt(
        &world,
        vec![
            adopt_group(&[&m1], "One", PatchIntent::Upstream),
            adopt_group(&[&m2], "Two", PatchIntent::Upstream),
        ],
    );
    assert_eq!(queue.all_patches().count(), 3);
    assert_eq!(queue.patch_refs()[1].title, "One");
    assert_eq!(queue.patch_refs()[2].title, "Two");
    assert_eq!(
        queue.patch_refs()[2].depends_on,
        vec![queue.patch_refs()[1].id.clone()]
    );
}

#[test]
fn init_adopts_mixed_merge_then_direct_commit() {
    let world = setup_uninitialized();
    git(
        &world.company,
        &["checkout", "-b", "feat/one"],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "src/one.js", "export const one = 1;\n");
    commit_all(&world.company, "one feature");
    git(&world.company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        &world.company,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Merge pull request #12 from feat/one",
            "feat/one",
        ],
        GitOpts::default(),
    )
    .unwrap();
    let merge = rev(&world.company);
    write(&world.company, "src/hot.js", "export const hot = true;\n");
    commit_all(&world.company, "hotfix on main");
    let direct = rev(&world.company);
    let queue = init_adopt(
        &world,
        vec![
            adopt_group(&[&merge], "One", PatchIntent::Upstream),
            adopt_group(&[&direct], "Hotfix", PatchIntent::Upstream),
        ],
    );
    assert_eq!(queue.all_patches().count(), 3);
    assert_eq!(queue.patch_refs()[1].title, "One");
    assert_eq!(queue.patch_refs()[2].title, "Hotfix");
}

#[test]
fn init_skips_empty_first_parent_merge_in() {
    let world = setup_uninitialized();
    git(
        &world.company,
        &["checkout", "-b", "empty-side"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["commit", "--allow-empty", "-m", "empty side"],
        GitOpts::default(),
    )
    .unwrap();
    git(&world.company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        &world.company,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Merge empty side",
            "empty-side",
        ],
        GitOpts::default(),
    )
    .unwrap();
    write(&world.company, "src/real.js", "export const real = 1;\n");
    commit_all(&world.company, "real product change");
    let real = rev(&world.company);
    let queue = init_adopt(
        &world,
        vec![adopt_group(&[&real], "Real", PatchIntent::Upstream)],
    );
    assert_eq!(queue.all_patches().count(), 2);
    assert_eq!(queue.patch_refs()[1].title, "Real");
}

#[test]
fn init_rejects_incomplete_and_noncontiguous_adopt_groups() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let main_before = rev(&world.company);
    let missing = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            adopt_groups: Some(vec![adopt_group(
                &[&a, &b],
                "Partial",
                PatchIntent::Upstream,
            )]),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(missing.contains("not assigned"), "{missing}");
    assert_eq!(rev(&world.company), main_before);

    let split = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            adopt_groups: Some(vec![
                adopt_group(&[&a, &c], "Split", PatchIntent::Upstream),
                adopt_group(&[&b], "Mid", PatchIntent::Upstream),
            ]),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        split.contains("contiguous") || split.contains("interleave"),
        "{split}"
    );
    assert_eq!(rev(&world.company), main_before);
}

#[test]
fn init_rejects_history_that_is_ahead_and_behind() {
    let world = setup_uninitialized();
    write(&world.company, "src/private.js", "export const p = 1;\n");
    commit_all(&world.company, "private work");
    let main_before = rev(&world.company);
    write(
        &world.upstream,
        "src/tokens.js",
        "export function hash() {}\n",
    );
    commit_all(&world.upstream, "upstream moved");
    let err = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            adopt_groups: Some(vec![adopt_group(&["HEAD"], "Nope", PatchIntent::Upstream)]),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("fast-forward"), "{err}");
    assert_eq!(rev(&world.company), main_before);
}

#[test]
fn init_ahead_without_groups_fails_closed() {
    let world = setup_uninitialized();
    three_linear_ahead(&world.company);
    let err = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Github),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("--adopt-groups"), "{err}");
}

#[test]
fn rebuild_push_publishes_state_and_main() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    rebuild_with(
        &world.company,
        RebuildOpts {
            push: true,
            push_remote: Some("origin".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let remote_main = git_ok(&origin, &["rev-parse", "main"]).unwrap();
    let local_main = rev(&world.company);
    assert_eq!(remote_main, local_main);
    git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap();
}

#[test]
fn rebuild_push_replaces_remote_main_that_moved() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", PatchIntent::Upstream),
            adopt_group(&[&c], "Dashboards", PatchIntent::InternalOnly),
        ],
    );
    let origin = keep_dir();
    git(
        &origin,
        &["init", "--bare", "-b", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    rebuild_with(
        &world.company,
        RebuildOpts {
            push: true,
            push_remote: Some("origin".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let replay = rev(&world.company);
    git(
        &world.company,
        &["commit", "--allow-empty", "-m", "stray merge"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &world.company,
        &["push", "--quiet", "origin", "HEAD:main"],
        GitOpts::default(),
    )
    .unwrap();
    let moved = git_ok(&origin, &["rev-parse", "main"]).unwrap();
    assert_ne!(moved, replay);
    git(
        &world.company,
        &["reset", "--hard", "--quiet", &replay],
        GitOpts::default(),
    )
    .unwrap();
    rebuild_with(
        &world.company,
        RebuildOpts {
            push: true,
            push_remote: Some("origin".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let published = rev(&world.company);
    assert_eq!(git_ok(&origin, &["rev-parse", "main"]).unwrap(), published);
    assert_ne!(published, moved);
}

#[test]
fn add_records_the_patch_and_rebuilds_upstream_under_internal() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let internal = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let main_after_internal = git_ok(company, &["rev-parse", "main"]).unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    land_on_main(company, &head_sha);
    let main_before_upstream = git_ok(company, &["rev-parse", "main"]).unwrap();
    let patch = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main^".into()),
            head_ref: Some(head_sha),
            ..Default::default()
        },
    )
    .unwrap();
    let main_after = git_ok(company, &["rev-parse", "main"]).unwrap();
    assert_ne!(main_after, main_before_upstream);
    assert_ne!(main_after, main_after_internal);
    assert!(!tree_has_uplink(company, "main"));
    let snapshot = status_snapshot(company).unwrap();
    assert!(snapshot.queue.is_internal(&internal.id));
    assert!(snapshot.queue.is_upstream(&patch.id));
    assert!(
        snapshot
            .product_files
            .get("NOTES.md")
            .unwrap()
            .contains("internal-notes")
    );
    assert!(
        snapshot
            .product_files
            .get("src/tokens.js")
            .unwrap()
            .contains("sha256")
    );
    let stored = git_ok(
        company,
        &[
            "show",
            &format!("{STATE_BRANCH}:.uplink/patches/{}.patch", patch.id),
        ],
    )
    .unwrap();
    assert!(stored.contains("sha256"));
}

#[test]
fn add_refuses_when_the_change_is_not_on_main() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("not on company main yet"), "{err}");
    assert!(status_snapshot(company).unwrap().queue.upstream.is_empty());
}

#[test]
fn add_from_explicit_range_does_not_require_company_main() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let from = git_ok(company, &["rev-parse", "main"]).unwrap();
    let head = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let main_before = git_ok(company, &["rev-parse", "main"]).unwrap();
    let not_on_main = git(
        company,
        &["merge-base", "--is-ancestor", &head, "main"],
        GitOpts::allow_fail(),
    )
    .unwrap();
    assert_ne!(not_on_main.code, 0);
    let patch = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some(from),
            head_ref: Some(head),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        git_ok(company, &["rev-parse", "main"]).unwrap(),
        main_before
    );
    assert!(
        status_snapshot(company)
            .unwrap()
            .queue
            .is_upstream(&patch.id)
    );
}

#[test]
fn internal_only_add_from_explicit_range_does_not_rebuild_main() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let from = git_ok(company, &["rev-parse", "main"]).unwrap();
    let head = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let main_before = git_ok(company, &["rev-parse", "main"]).unwrap();
    let patch = add_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some(from),
            head_ref: Some(head),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        git_ok(company, &["rev-parse", "main"]).unwrap(),
        main_before
    );
    assert!(
        status_snapshot(company)
            .unwrap()
            .queue
            .is_internal(&patch.id)
    );
}

#[test]
fn add_and_preflight_materialize_uplink_upstream_from_origin() {
    let world = setup_world();
    let company = &world.company;
    let origin = create_bare_from(company);
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["push", "--quiet", "origin", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["push", "--quiet", "origin", "uplink/state"],
        GitOpts::default(),
    )
    .unwrap();

    let clone_keep = temp_dir();
    let clone = clone_keep.path().to_path_buf();
    git(
        Path::new("/tmp"),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
        GitOpts::default(),
    )
    .unwrap();
    let _ = git(
        &clone,
        &["branch", "-D", "uplink/upstream"],
        GitOpts::allow_fail(),
    );
    assert!(
        !has_git_ref(&clone, "uplink/upstream"),
        "clone should not have a local uplink/upstream (Actions checkout of main)"
    );
    assert!(
        has_git_ref(&clone, "origin/uplink/upstream"),
        "clone should have origin/uplink/upstream as a tracking ref"
    );

    git(&clone, &["checkout", "-b", "feat/hash"], GitOpts::default()).unwrap();
    write(
        &clone,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(&clone, "use sha256");
    let from = git_ok(&clone, &["rev-parse", "main"]).unwrap();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]).unwrap();

    preflight_incoming_change(
        &clone,
        IncomingPreflight {
            title: "Use SHA-256 for tokens".into(),
            from_ref: from.clone(),
            head_ref: head.clone(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap();
    assert!(
        has_git_ref(&clone, "uplink/upstream"),
        "preflight should create local uplink/upstream from origin"
    );

    git(
        &clone,
        &["branch", "-D", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &clone,
        &["update-ref", "-d", "refs/remotes/origin/uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    assert!(!has_git_ref(&clone, "uplink/upstream"));
    assert!(!has_git_ref(&clone, "origin/uplink/upstream"));

    let patch = add_landed_patch(
        &clone,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(patch.title, "Use SHA-256 for tokens");
    assert!(
        has_git_ref(&clone, "uplink/upstream"),
        "add should fetch uplink/upstream from origin when the tracking ref is missing"
    );
}

#[test]
fn preflight_fetches_missing_from_and_head_from_origin() {
    let world = setup_world();
    let company = &world.company;
    let origin = create_bare_from(company);
    git(
        &origin,
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["push", "--quiet", "origin", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["push", "--quiet", "origin", "uplink/state"],
        GitOpts::default(),
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let from = git_ok(company, &["rev-parse", "main"]).unwrap();
    let head = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["push", "--quiet", "origin", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();

    let clone_keep = temp_dir();
    let clone = clone_keep.path().to_path_buf();
    let origin_url = format!("file://{}", origin.display());
    git(
        Path::new("/tmp"),
        &[
            "clone",
            "--quiet",
            "--single-branch",
            "--branch",
            "main",
            &origin_url,
            clone.to_str().unwrap(),
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &clone,
        &[
            "fetch",
            "--quiet",
            "origin",
            "+refs/heads/uplink/state:refs/heads/uplink/state",
        ],
        GitOpts::default(),
    )
    .unwrap();
    assert!(
        !has_git_object(&clone, &head),
        "single-branch clone of main should not have the PR head SHA"
    );

    preflight_incoming_change(
        &clone,
        IncomingPreflight {
            title: "Use SHA-256 for tokens".into(),
            from_ref: from,
            head_ref: head.clone(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .expect("preflight should fetch missing --from/--head from origin");
    assert!(
        has_git_object(&clone, &head),
        "preflight should fetch the missing head SHA from origin"
    );
}

#[test]
fn preflight_missing_revs_without_origin_fails_clearly() {
    let world = setup_world();
    let err = preflight_incoming_change(
        &world.company,
        IncomingPreflight {
            title: "missing".into(),
            from_ref: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into(),
            head_ref: "cafebabecafebabecafebabecafebabecafebabe".into(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("origin is not configured"),
        "expected a missing-origin error, got {msg}"
    );
}

#[test]
fn preflight_missing_revs_not_on_origin_fails_clearly() {
    let world = setup_world();
    let company = &world.company;
    let origin = create_bare_from(company);
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    let err = preflight_incoming_change(
        company,
        IncomingPreflight {
            title: "missing".into(),
            from_ref: "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".into(),
            head_ref: "cafebabecafebabecafebabecafebabecafebabe".into(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not in this clone"),
        "expected a missing-revision error, got {msg}"
    );
}

#[test]
fn sync_skips_rebuilding_main_when_upstream_is_unchanged() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    sync(company).unwrap();
    let main_after_first = git_ok(company, &["rev-parse", "main"]).unwrap();
    sync(company).unwrap();
    let main_after_second = git_ok(company, &["rev-parse", "main"]).unwrap();
    assert_eq!(main_after_first, main_after_second);
    assert!(!tree_has_uplink(company, "main"));
}

#[test]
fn approve_receipt_records_the_state_branch_commit() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let sha = git_uplink::patch_state_commit(company, &patch.id).unwrap();
    let receipt = format_approval_receipt(ApprovalReceipt {
        patch_id: &patch.id,
        environment: "to-upstream",
        actor: "dispatcher",
        run_url: "https://github.example/run/1",
        sha: &sha,
        reviewed: None,
        at: Some("2026-09-14T00:00:00.000Z".into()),
    });
    assert!(receipt.contains(&format!("`{sha}`")));
    assert!(receipt.contains("Queue commit"));
    assert!(!tree_has_uplink(company, "main"));
}

#[test]
fn rebuilds_company_main_with_stacked_patches_including_internal_only() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/logs"],
        GitOpts::default(),
    )
    .unwrap();
    let current = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &current.replace(
            "return sha256(value);",
            "console.log(\"hash\");\n  return sha256(value);",
        ),
    );
    commit_all(company, "add log");
    let log_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Log token hashes".into(),
            from_ref: Some("main".into()),
            depends_on: vec![hash_patch.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/telemetry"],
        GitOpts::default(),
    )
    .unwrap();
    let with_logs = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &with_logs.replace(
            "console.log(\"hash\");",
            "console.log(\"hash\");\n  companyTelemetry();",
        ),
    );
    commit_all(company, "internal telemetry");
    let internal = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Vendor telemetry".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let snapshot = status_snapshot(company).unwrap();
    let tokens = snapshot.product_files.get("src/tokens.js").unwrap();
    assert!(tokens.contains("sha256"));
    assert!(tokens.contains("console.log(\"hash\")"));
    assert!(tokens.contains("companyTelemetry()"));
    assert!(snapshot.queue.is_internal(&internal.id));
    assert_eq!(log_patch.depends_on, vec![hash_patch.id]);
    assert_eq!(snapshot.queue.all_patches().count(), 3);
}

#[test]
fn drops_a_merged_patch_so_a_later_upstream_fix_is_not_reverted() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    let hashed = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &hashed.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    git(upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        upstream,
        &[
            "commit",
            "-m",
            &format!(
                "Use SHA-256 for tokens\n\nUplink-Patch-Id: {}\n",
                hash_patch.id
            ),
        ],
        GitOpts::default(),
    )
    .unwrap();
    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return saltedSha256(value);"),
    );
    commit_all(upstream, "follow-up: salt the hash");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let inspected = sync(company).unwrap();
    assert!(inspected.needs_approval, "salt follow-up is foreign");
    assert!(inspected.flowed_back.iter().any(|id| id == &hash_patch.id));
    accept_upstream(company).unwrap();

    let snapshot = status_snapshot(company).unwrap();
    let merged = snapshot
        .queue
        .all_patches()
        .find(|p| p.id == hash_patch.id)
        .unwrap();
    assert_eq!(merged.status, PatchStatus::Merged);
    assert_eq!(merged.merged.as_ref().unwrap().via, MergeVia::Trailer);
    let tokens = snapshot.product_files.get("src/tokens.js").unwrap();
    assert!(tokens.contains("saltedSha256"));
    assert!(!tokens.contains("return sha256(value)"));
    assert!(tokens.contains("return 7200;"));
}

#[test]
fn sync_applies_flowed_back_commits_without_approval() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    git(upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        upstream,
        &[
            "commit",
            "-m",
            &format!(
                "Use SHA-256 for tokens\n\nUplink-Patch-Id: {}\n",
                hash_patch.id
            ),
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let result = sync(company).unwrap();
    assert!(!result.needs_approval);
    assert_eq!(result.flowed_back, vec![hash_patch.id.clone()]);
    assert!(result.queue.pending_upstream.is_none());
    let merged = result
        .queue
        .all_patches()
        .find(|p| p.id == hash_patch.id)
        .unwrap();
    assert_eq!(merged.status, PatchStatus::Merged);
    let upstream_tokens = git_ok(company, &["show", "uplink/upstream:src/tokens.js"]).unwrap();
    assert!(upstream_tokens.contains("sha256"));
}

#[test]
fn sync_holds_foreign_commits_until_accept_upstream() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    let before = git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap();

    write(upstream, "CHANGELOG.md", "upstream 1.2\n");
    commit_all(upstream, "document 1.2");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let result = sync(company).unwrap();
    assert!(result.needs_approval);
    assert!(!result.foreign_commits.is_empty());
    assert_eq!(
        git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap(),
        before
    );
    let incoming = company.join(from_upstream_report_paths().1);
    let packet = fs::read_to_string(&incoming).unwrap();
    assert!(packet.contains("from-upstream"));
    assert!(packet.contains("document 1.2"));
    assert_eq!(
        result.queue.pending_upstream.as_ref().unwrap().sha,
        result.pending_sha.as_deref().unwrap()
    );

    let applied = accept_upstream(company).unwrap();
    assert!(!applied.needs_approval);
    assert!(applied.queue.pending_upstream.is_none());
    let after = git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap();
    assert_ne!(after, before);
    let changelog = git_ok(company, &["show", "uplink/upstream:CHANGELOG.md"]).unwrap();
    assert!(changelog.contains("upstream 1.2"));
}

fn world_with_hash_patch() -> (World, Patch) {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    (world, hash_patch)
}

fn commit_with_trailer(repo: &Path, contents: &str, id: &str) -> String {
    write(repo, "src/tokens.js", contents);
    git(repo, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        repo,
        &[
            "commit",
            "-m",
            &format!("Use SHA-256 for tokens\n\nUplink-Patch-Id: {id}\n"),
        ],
        GitOpts::default(),
    )
    .unwrap();
    rev(repo)
}

#[test]
fn sync_sends_a_trailer_with_a_different_diff_to_review() {
    let (world, hash_patch) = world_with_hash_patch();
    let altered = commit_with_trailer(
        &world.upstream,
        &TOKENS.replace("return sha1(value);", "return sha512(value);"),
        &hash_patch.id,
    );

    let result = sync(&world.company).unwrap();
    assert!(result.needs_approval);
    assert!(result.flowed_back.is_empty(), "{:?}", result.flowed_back);
    assert_eq!(result.foreign_commits, [altered]);
    assert_eq!(result.queue.patch_refs()[0].status, PatchStatus::Queued);
}

fn patch_status(repo: &Path, id: &str) -> PatchStatus {
    let queue = git_uplink::read_queue(repo).unwrap();
    queue.all_patches().find(|p| p.id == id).unwrap().status
}

fn merged_pr(patch: &Patch, sha: &str) -> SyncOpts {
    SyncOpts {
        merged_prs: vec![(patch.id.clone(), sha.to_string())],
    }
}

/// A world whose hash patch has a recorded public PR.
fn world_with_submitted_hash_patch() -> (World, Patch) {
    let (world, hash_patch) = world_with_hash_patch();
    approve_patch(&world.company, &hash_patch.id).unwrap();
    record_pull_request(
        &world.company,
        &hash_patch.id,
        7,
        "https://github.com/acme/app/pull/7",
        &format!("uplink/{}", hash_patch.id),
        None,
    )
    .unwrap();
    (world, hash_patch)
}

#[test]
fn accepting_a_trailer_with_a_different_diff_does_not_merge_the_patch() {
    let (world, hash_patch) = world_with_hash_patch();
    let altered = commit_with_trailer(
        &world.upstream,
        &TOKENS.replace("return sha1(value);", "return sha512(value);"),
        &hash_patch.id,
    );

    let result = sync(&world.company).unwrap();
    assert!(result.needs_approval);
    assert!(result.merges.is_empty(), "{:?}", result.merges);
    assert_eq!(result.claims.len(), 1);
    assert_eq!(result.claims[0].sha, altered);
    assert_eq!(result.claims[0].id, hash_patch.id);
    let packet = result.report.unwrap();
    assert!(
        packet.contains(&format!("does **not** mark `{}` merged", hash_patch.id)),
        "{packet}"
    );

    accept_upstream(&world.company).unwrap();
    // Both change the same line, so the patch is replayed into a conflict.
    assert_eq!(
        patch_status(&world.company, &hash_patch.id),
        PatchStatus::Conflict
    );
}

#[test]
fn a_patch_id_mentioned_in_an_unrelated_commit_does_not_merge_the_patch() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    write(&world.upstream, "CHANGELOG.md", "notes\n");
    git(&world.upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        &world.upstream,
        &[
            "commit",
            "-m",
            &format!(
                "docs: changelog\n\nsee Uplink-Patch-Id: {} in the fork\n",
                hash_patch.id
            ),
        ],
        GitOpts::default(),
    )
    .unwrap();

    let result = sync(company).unwrap();
    assert!(result.needs_approval);
    assert!(result.claims.is_empty(), "{:?}", result.claims);
    accept_upstream(company).unwrap();

    assert_eq!(patch_status(company, &hash_patch.id), PatchStatus::Queued);
    let tokens = git_ok(company, &["show", "main:src/tokens.js"]).unwrap();
    assert!(tokens.contains("sha256"), "{tokens}");
}

#[test]
fn sync_applies_a_merge_commit_of_only_our_patch_without_approval() {
    let (world, hash_patch) = world_with_hash_patch();
    let upstream = &world.upstream;
    git(upstream, &["checkout", "-b", "pr"], GitOpts::default()).unwrap();
    commit_with_trailer(
        upstream,
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
        &hash_patch.id,
    );
    git(upstream, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        upstream,
        &["merge", "--no-ff", "--quiet", "-m", "Merge pr", "pr"],
        GitOpts::default(),
    )
    .unwrap();

    let result = sync(&world.company).unwrap();
    assert!(!result.needs_approval);
    assert_eq!(result.flowed_back, std::slice::from_ref(&hash_patch.id));
    assert_eq!(
        patch_status(&world.company, &hash_patch.id),
        PatchStatus::Merged
    );
}

#[test]
fn sync_sends_a_merge_commit_that_changes_more_to_review() {
    let (world, hash_patch) = world_with_hash_patch();
    let upstream = &world.upstream;
    git(upstream, &["checkout", "-b", "pr"], GitOpts::default()).unwrap();
    commit_with_trailer(
        upstream,
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
        &hash_patch.id,
    );
    git(upstream, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        upstream,
        &["merge", "--no-ff", "--no-commit", "--quiet", "pr"],
        GitOpts::default(),
    )
    .unwrap();
    write(upstream, "EVIL.md", "added in the merge commit\n");
    git(upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        upstream,
        &["commit", "--quiet", "-m", "Merge pr"],
        GitOpts::default(),
    )
    .unwrap();
    let merge = rev(upstream);

    let result = sync(&world.company).unwrap();
    assert!(result.needs_approval);
    assert_eq!(result.flowed_back, std::slice::from_ref(&hash_patch.id));
    assert_eq!(result.foreign_commits, [merge]);
    let packet = result.report.unwrap();
    assert!(packet.contains("added in the merge commit"), "{packet}");
    assert!(
        !packet.contains("+  return sha256(value);"),
        "our own patch is not up for review\n{packet}"
    );
    assert_eq!(
        patch_status(&world.company, &hash_patch.id),
        PatchStatus::Queued,
        "merged only at promotion"
    );
}

#[test]
fn a_merged_pr_with_maintainer_extras_merges_after_review_of_the_extras() {
    let (world, hash_patch) = world_with_submitted_hash_patch();
    let company = &world.company;
    let upstream = &world.upstream;
    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    write(upstream, "NOTES.md", "maintainer note\n");
    commit_all(upstream, "Use SHA-256 for tokens (#7)");
    let squash = rev(upstream);

    let result = sync_with(company, merged_pr(&hash_patch, &squash)).unwrap();
    assert!(result.needs_approval);
    assert_eq!(
        result.merges,
        [PendingMerge {
            id: hash_patch.id.clone(),
            sha: squash.clone(),
            via: MergeVia::Pr,
            modified: false,
        }]
    );
    let packet = result.report.unwrap();
    assert!(packet.contains("maintainer note"), "{packet}");
    assert!(!packet.contains("+  return sha256(value);"), "{packet}");
    assert!(!packet.contains("Modified by the maintainer"), "{packet}");
    assert_eq!(
        patch_status(company, &hash_patch.id),
        PatchStatus::Submitted
    );

    let applied = accept_upstream_at(company, Some(&squash)).unwrap();
    let merged = applied
        .queue
        .all_patches()
        .find(|p| p.id == hash_patch.id)
        .unwrap();
    assert_eq!(merged.status, PatchStatus::Merged);
    let record = merged.merged.as_ref().unwrap();
    assert_eq!(record.via, MergeVia::Pr);
    assert_eq!(record.upstream_sha.as_deref(), Some(squash.as_str()));
}

#[test]
fn a_merged_pr_the_maintainer_modified_merges_only_on_approval() {
    let (world, hash_patch) = world_with_submitted_hash_patch();
    let company = &world.company;
    let upstream = &world.upstream;
    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha512(value);"),
    );
    commit_all(upstream, "Use SHA-512 for tokens (#7)");
    let squash = rev(upstream);

    let result = sync_with(company, merged_pr(&hash_patch, &squash)).unwrap();
    assert!(result.needs_approval);
    assert_eq!(result.merges.len(), 1);
    assert!(result.merges[0].modified);
    let packet = result.report.unwrap();
    assert!(packet.contains("Modified by the maintainer"), "{packet}");
    // Only the maintainer's change to our patch is left to review.
    assert!(packet.contains("-  return sha256(value);"), "{packet}");
    assert!(packet.contains("+  return sha512(value);"), "{packet}");
    // Until approval the patch is still carried on company main.
    assert_eq!(
        patch_status(company, &hash_patch.id),
        PatchStatus::Submitted
    );
    let tokens = git_ok(company, &["show", "main:src/tokens.js"]).unwrap();
    assert!(tokens.contains("sha256"), "{tokens}");

    accept_upstream(company).unwrap();
    assert_eq!(patch_status(company, &hash_patch.id), PatchStatus::Merged);
    let tokens = git_ok(company, &["show", "main:src/tokens.js"]).unwrap();
    assert!(
        tokens.contains("sha512") && !tokens.contains("sha256"),
        "{tokens}"
    );
}

#[test]
fn a_merged_pr_report_outside_the_range_or_the_queue_is_ignored() {
    let (world, hash_patch) = world_with_submitted_hash_patch();
    let company = &world.company;
    let old = git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap();
    write(&world.upstream, "CHANGELOG.md", "upstream 1.2\n");
    commit_all(&world.upstream, "document 1.2");

    let opts = SyncOpts {
        merged_prs: vec![
            (hash_patch.id.clone(), old),
            (hash_patch.id.clone(), "--output=x".into()),
            ("upl_0000000000".into(), rev(&world.upstream)),
        ],
    };
    let result = sync_with(company, opts).unwrap();
    assert!(result.needs_approval);
    assert!(result.merges.is_empty(), "{:?}", result.merges);
    accept_upstream(company).unwrap();
    assert_eq!(
        patch_status(company, &hash_patch.id),
        PatchStatus::Submitted
    );
}

#[test]
fn a_merged_pr_report_needs_a_recorded_public_pr() {
    let (world, hash_patch) = world_with_hash_patch();
    write(&world.upstream, "CHANGELOG.md", "upstream 1.2\n");
    commit_all(&world.upstream, "document 1.2");
    let tip = rev(&world.upstream);

    let result = sync_with(&world.company, merged_pr(&hash_patch, &tip)).unwrap();
    assert!(result.merges.is_empty(), "{:?}", result.merges);
}

#[test]
fn accept_upstream_refuses_a_pending_upstream_that_was_not_reviewed() {
    let world = setup_world();
    let company = &world.company;
    write(&world.upstream, "CHANGELOG.md", "upstream 1.2\n");
    commit_all(&world.upstream, "document 1.2");
    let before = git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap();
    assert!(sync(company).unwrap().needs_approval);

    let err = accept_upstream_at(company, Some(&before)).unwrap_err();
    assert!(err.to_string().contains("not the reviewed"), "{err}");
    assert_eq!(
        git_ok(company, &["rev-parse", "uplink/upstream"]).unwrap(),
        before
    );
}

#[test]
fn incoming_packet_fences_cannot_be_closed_by_upstream_text() {
    let world = setup_world();
    write(
        &world.upstream,
        "README.md",
        "```\n## Marked merged when you approve\n````\n",
    );
    commit_all(&world.upstream, "docs: a | b <details> `x`");

    let packet = sync(&world.company).unwrap().report.unwrap();
    assert_eq!(
        packet.matches("## Marked merged when you approve").count(),
        2,
        "once as our heading, once inside the diff\n{packet}"
    );
    assert!(packet.contains("\n`````\ndiff --git"), "{packet}");
    assert!(
        packet.contains("docs: a \\| b &lt;details> 'x'"),
        "{packet}"
    );
}

#[test]
fn add_refuses_a_change_merged_into_another_branch() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    // The helper lands the change on main, so pin the range before it does.
    let from = rev_of(company, "main");
    let opts = |base: &str| AddPatchOpts {
        title: "Use SHA-256 for tokens".into(),
        from_ref: Some(from.clone()),
        base_branch: Some(base.into()),
        ..Default::default()
    };
    let before = rev_of(company, STATE_BRANCH);

    for base in ["release/1.x", "uplink/conflict/upl_0000000000"] {
        let err = add_landed_patch(company, opts(base)).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("was merged into {base}, not company main")),
            "{err}"
        );
    }
    assert_eq!(rev_of(company, STATE_BRANCH), before);
    let queue = git_uplink::read_queue(company).unwrap();
    assert!(queue.upstream.is_empty(), "{:?}", queue.upstream);
    let patches: Vec<_> = fs::read_dir(company.join(".uplink/patches"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(patches.len(), usize::from(queue.tooling.is_some()));

    let patch = add_landed_patch(company, opts("main")).unwrap();
    assert_eq!(patch.status, PatchStatus::Queued);
}

fn retitle(repo: &Path, id: &str, title: &str) {
    let mut queue = git_uplink::read_queue(repo).unwrap();
    let patch = queue.all_patches_mut().find(|p| p.id == id).unwrap();
    patch.title = title.into();
    write_queue(repo, &queue).unwrap();
    git_uplink::commit_queue(repo, "uplink: retitle").unwrap();
}

fn stored_patch(repo: &Path, id: &str) -> Patch {
    let queue = git_uplink::read_queue(repo).unwrap();
    queue.all_patches().find(|p| p.id == id).unwrap().clone()
}

#[test]
fn approve_refuses_a_patch_that_changed_since_the_packet() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    let reviewed = review_token(company, &hash_patch).unwrap();
    let packet = format_contribution_packet(company, &hash_patch).unwrap();
    assert!(
        packet.contains(&format!("| Review token | `{reviewed}` |")),
        "{packet}"
    );

    // The public PR title is part of what leaves the company.
    retitle(company, &hash_patch.id, "Use SHA-256 everywhere");
    let err =
        approve_patch_reviewed(company, &hash_patch.id, None, None, Some(&reviewed)).unwrap_err();
    assert!(
        err.to_string()
            .contains("changed since the packet was reviewed"),
        "{err}"
    );
    let after = stored_patch(company, &hash_patch.id);
    assert_eq!(after.status, PatchStatus::Queued);
    assert!(after.approvals.is_empty());

    let current = review_token(company, &after).unwrap();
    assert_ne!(current, reviewed);
    let approved =
        approve_patch_reviewed(company, &hash_patch.id, None, None, Some(&current)).unwrap();
    assert_eq!(approved.status, PatchStatus::Approved);
    assert_eq!(
        approved.last_approval().unwrap().reviewed.as_deref(),
        Some(current.as_str())
    );
}

#[test]
fn report_writes_the_token_that_approve_takes_back() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_git-uplink"))
            .args(args)
            .current_dir(company)
            .output()
            .unwrap()
    };
    let id = hash_patch.id.as_str();

    let report = cli(&["report", id]);
    assert!(report.status.success(), "{report:?}");
    let token_file = company.join(format!(".uplink/reports/{id}/review-token"));
    let token = fs::read_to_string(&token_file).unwrap().trim().to_string();
    assert_eq!(token, review_token(company, &hash_patch).unwrap());
    let committed = git_ok(
        company,
        &[
            "show",
            &format!("uplink/state:.uplink/reports/{id}/review-token"),
        ],
    )
    .unwrap();
    assert_eq!(committed.trim(), token);

    retitle(company, id, "Use SHA-256 everywhere");
    let stale = cli(&["approve", id, "--reviewed", &token]);
    assert!(!stale.status.success(), "{stale:?}");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("changed since the packet was reviewed"),
        "{stale:?}"
    );
    assert!(
        !company
            .join(format!(".uplink/reports/{id}/approval.md"))
            .exists(),
        "a refused approval leaves no receipt"
    );
    assert_eq!(stored_patch(company, id).status, PatchStatus::Queued);

    let report = cli(&["report", id]);
    assert!(report.status.success(), "{report:?}");
    let token = fs::read_to_string(&token_file).unwrap().trim().to_string();
    let approved = cli(&["approve", id, "--reviewed", &token]);
    assert!(approved.status.success(), "{approved:?}");
    let receipt = String::from_utf8_lossy(&approved.stdout).into_owned();
    assert!(
        receipt.contains(&format!("| Review token | `{token}` |")),
        "{receipt}"
    );
    let patch = stored_patch(company, id);
    assert_eq!(patch.status, PatchStatus::Approved);
    assert_eq!(
        patch.last_approval().unwrap().reviewed.as_deref(),
        Some(token.as_str())
    );
}

#[test]
fn a_public_pr_is_recorded_only_after_approval() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    let url = "https://github.com/acme/app/pull/7";
    let branch = format!("uplink/{}", hash_patch.id);
    let record = || record_pull_request(company, &hash_patch.id, 7, url, &branch, None);

    let err = record().unwrap_err();
    assert!(
        err.to_string()
            .contains("is queued; record a public PR only after approve and submit"),
        "{err}"
    );
    let untouched = stored_patch(company, &hash_patch.id);
    assert_eq!(untouched.status, PatchStatus::Queued);
    assert!(untouched.upstream.is_none());

    approve_patch(company, &hash_patch.id).unwrap();
    assert_eq!(record().unwrap().status, PatchStatus::Submitted);
    let events = stored_patch(company, &hash_patch.id).events.len();
    assert_eq!(record().unwrap().status, PatchStatus::Submitted);
    assert_eq!(stored_patch(company, &hash_patch.id).events.len(), events);
}

#[test]
fn status_lists_patches_whose_approval_no_longer_covers_them() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    let stale = |repo: &Path| status_snapshot(repo).unwrap().stale_approvals;

    // Not approved yet: nothing to be stale.
    assert!(stale(company).is_empty());
    approve_patch(company, &hash_patch.id).unwrap();
    assert!(stale(company).is_empty());

    retitle(company, &hash_patch.id, "Use SHA-256 everywhere");
    assert_eq!(stale(company), std::slice::from_ref(&hash_patch.id));
    let snapshot = status_snapshot(company).unwrap();
    let report = serde_json::to_value(git_uplink::status_report(&snapshot)).unwrap();
    assert_eq!(report["staleApprovals"][0], hash_patch.id.as_str());
    let table = git_uplink::format_status_table(&snapshot);
    assert!(
        table.contains(&format!(
            "needs approval before submit (content changed since the last approval): {}",
            hash_patch.id
        )),
        "{table}"
    );

    approve_patch(company, &hash_patch.id).unwrap();
    assert!(stale(company).is_empty());
    let table = git_uplink::format_status_table(&status_snapshot(company).unwrap());
    assert!(!table.contains("needs approval before submit"), "{table}");
}

#[test]
fn review_token_ignores_the_rest_of_the_queue() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    let before = review_token(company, &hash_patch).unwrap();

    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "notes\n");
    commit_all(company, "notes");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Add notes".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let after = stored_patch(company, &hash_patch.id);
    assert_eq!(review_token(company, &after).unwrap(), before);
    approve_patch_reviewed(company, &hash_patch.id, None, None, Some(&before)).unwrap();
}

#[test]
fn submit_refuses_content_that_changed_after_approval() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    approve_patch(company, &hash_patch.id).unwrap();

    retitle(company, &hash_patch.id, "Use SHA-256 everywhere");
    let err = submit_patch(company, &hash_patch.id, false).unwrap_err();
    assert!(
        err.to_string().contains("changed since it was approved"),
        "{err}"
    );

    // A new approval of the current content is recorded, not skipped.
    let again = approve_patch(company, &hash_patch.id).unwrap();
    assert_eq!(again.status, PatchStatus::Approved);
    assert_eq!(again.approvals.len(), 2);
    assert_eq!(again.approvals[1].kind, "refresh");
    let noop = approve_patch(company, &hash_patch.id).unwrap();
    assert_eq!(noop.approvals.len(), 2);
    submit_patch(company, &hash_patch.id, false).unwrap();
}

#[test]
fn an_approval_without_a_token_covers_the_same_patch_id_only() {
    let (world, hash_patch) = world_with_hash_patch();
    let company = &world.company;
    approve_patch(company, &hash_patch.id).unwrap();
    let rewrite_approval = |stable: Option<&str>| {
        let mut queue = git_uplink::read_queue(company).unwrap();
        let patch = queue
            .all_patches_mut()
            .find(|p| p.id == hash_patch.id)
            .unwrap();
        let approval = patch.approvals.last_mut().unwrap();
        approval.reviewed = None;
        if let Some(stable) = stable {
            approval.patch_id_stable = Some(stable.into());
        }
        write_queue(company, &queue).unwrap();
        git_uplink::commit_queue(company, "uplink: legacy approval").unwrap();
    };

    rewrite_approval(Some("0000000000000000000000000000000000000000"));
    let err = submit_patch(company, &hash_patch.id, false).unwrap_err();
    assert!(
        err.to_string().contains("changed since it was approved"),
        "{err}"
    );

    let stable = stored_patch(company, &hash_patch.id)
        .patch_id_stable
        .unwrap();
    rewrite_approval(Some(&stable));
    submit_patch(company, &hash_patch.id, false).unwrap();
}

#[test]
fn sync_detects_merged_patches_by_a_custom_trailer_key() {
    let world = setup_world();
    let company = &world.company;
    let mut queue = git_uplink::read_queue(company).unwrap();
    queue.config.trailer_key = "Company-Patch".into();
    write_queue(company, &queue).unwrap();
    git_uplink::commit_queue(company, "uplink: custom trailer").unwrap();

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();

    let upstream = &world.upstream;
    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    git(upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        upstream,
        &[
            "commit",
            "-m",
            &format!("Use SHA-256 for tokens\n\nCompany-Patch: {}\n", patch.id),
        ],
        GitOpts::default(),
    )
    .unwrap();

    let queue = sync_apply(company);
    let merged = queue.all_patches().find(|p| p.id == patch.id).unwrap();
    assert_eq!(merged.status, PatchStatus::Merged);
    assert_eq!(merged.merged.as_ref().unwrap().via, MergeVia::Trailer);
}

#[test]
fn drop_and_merged_refuse_patches_they_do_not_apply_to() {
    let world = setup_uninitialized();
    let queue = init_with_recorded_urls(&world);
    let company = &world.company;
    let tooling = queue.tooling.as_ref().unwrap().id.clone();
    let before = rev_of(company, STATE_BRANCH);
    let err = drop_patch(company, &tooling, "oops")
        .unwrap_err()
        .to_string();
    assert!(err.contains("tooling patch"), "{err}");
    assert_eq!(rev_of(company, STATE_BRANCH), before);

    let internal = add_internal_notes(company);
    let err = mark_merged(company, &internal.id, MergeVia::Manual, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not in the upstream queue"), "{err}");

    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let upstream = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop_patch(company, &upstream.id, "not needed").unwrap();
    let err = mark_merged(company, &upstream.id, MergeVia::Manual, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("already dropped"), "{err}");
    let err = drop_patch(company, &upstream.id, "again")
        .unwrap_err()
        .to_string();
    assert!(err.contains("already dropped"), "{err}");
}

#[test]
fn mark_merged_commits_the_queue() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    mark_merged(company, &patch.id, MergeVia::Manual, None).unwrap();
    assert_eq!(
        git_ok(company, &["log", "-1", "--format=%s", STATE_BRANCH]).unwrap(),
        format!("uplink: merged {}", patch.id)
    );
    let committed = git_uplink::queue_at(company, STATE_BRANCH).unwrap();
    let recorded = committed.all_patches().find(|p| p.id == patch.id).unwrap();
    assert_eq!(recorded.status, PatchStatus::Merged);
}

#[test]
fn sync_detects_several_patches_merged_by_patch_id() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    let hashed = TOKENS.replace("return sha1(value);", "return sha256(value);");
    let mut ids = Vec::new();
    for (branch, file, contents, title) in [
        (
            "feat/hash",
            "src/tokens.js",
            hashed.as_str(),
            "Use SHA-256 for tokens",
        ),
        ("feat/readme", "README.md", "tokenkit v2\n", "Describe v2"),
    ] {
        git(
            company,
            &["checkout", "--quiet", "main"],
            GitOpts::default(),
        )
        .unwrap();
        git(company, &["checkout", "-b", branch], GitOpts::default()).unwrap();
        write(company, file, contents);
        commit_all(company, title);
        let patch = add_landed_patch(
            company,
            AddPatchOpts {
                title: title.into(),
                from_ref: Some("main".into()),
                ..Default::default()
            },
        )
        .unwrap();
        ids.push(patch.id);
        write(upstream, file, contents);
        commit_all(upstream, &format!("{title} (squashed)"));
    }
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();

    let queue = sync_apply(company);
    for id in &ids {
        let patch = queue.all_patches().find(|p| &p.id == id).unwrap();
        assert_eq!(patch.status, PatchStatus::Merged, "{id}");
        assert_eq!(
            patch.merged.as_ref().unwrap().via,
            MergeVia::PatchId,
            "{id}"
        );
    }
    let message = queue.last_sync.unwrap().message.unwrap();
    assert_eq!(message, format!("Marked merged: {}", ids.join(", ")));
}

#[test]
fn sync_mixed_flow_back_and_foreign_waits_for_approval() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    git(upstream, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        upstream,
        &[
            "commit",
            "-m",
            &format!(
                "Use SHA-256 for tokens\n\nUplink-Patch-Id: {}\n",
                hash_patch.id
            ),
        ],
        GitOpts::default(),
    )
    .unwrap();
    write(upstream, "CHANGELOG.md", "also a release note\n");
    commit_all(upstream, "release notes");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let result = sync(company).unwrap();
    assert!(result.needs_approval);
    assert!(result.flowed_back.iter().any(|id| id == &hash_patch.id));
    assert_eq!(result.queue.patch_refs()[0].status, PatchStatus::Queued);
    accept_upstream(company).unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patch_refs()[0].status, PatchStatus::Merged);
    let changelog = snapshot.product_files.get("CHANGELOG.md").unwrap();
    assert!(changelog.contains("release note"));
}

#[test]
fn command_output_does_not_write_github_step_summary() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let summary_file = company.join("step-summary.txt");
    fs::write(&summary_file, "").unwrap();
    let bin = env!("CARGO_BIN_EXE_git-uplink");
    let run = |args: &[&str]| {
        let output = Command::new(bin)
            .args(args)
            .current_dir(company)
            .env("GITHUB_STEP_SUMMARY", &summary_file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        let written = fs::read_to_string(&summary_file).unwrap();
        assert!(
            written.is_empty(),
            "{} wrote the step summary file:\n{written}",
            args.join(" ")
        );
        output
    };

    let report = run(&["report", &patch.id]);
    let packet = String::from_utf8(report.stdout).unwrap();
    assert!(packet.contains("# Contribution packet"), "{packet}");

    let approved = run(&["approve", &patch.id]);
    let receipt = String::from_utf8(approved.stdout).unwrap();
    assert!(receipt.contains("environment approval"), "{receipt}");
    assert!(
        String::from_utf8_lossy(&approved.stderr).contains("approved"),
        "{}",
        String::from_utf8_lossy(&approved.stderr)
    );

    write(upstream, "CHANGELOG.md", "upstream note\n");
    commit_all(upstream, "release notes");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();

    let sync_out = run(&["sync"]);
    let sync_json: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&sync_out.stdout).unwrap().trim())
            .expect("sync stdout is json");
    let summary = sync_json["summary"].as_str().unwrap_or("");
    assert!(summary.contains("Incoming upstream"), "{sync_json}");

    let accepted = run(&["accept-upstream"]);
    let accepted_json: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(&accepted.stdout).unwrap().trim())
            .expect("accept-upstream stdout is json");
    let receipt = accepted_json["summary"].as_str().unwrap_or("");
    assert!(receipt.contains("environment approval"), "{accepted_json}");
}

#[test]
fn stops_on_a_sync_conflict_and_amends_the_same_patch_when_resolved() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let ttl_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 1800;"),
    );
    commit_all(upstream, "shorten default ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_git-uplink");
    let sync_out = Command::new(bin)
        .args(["sync"])
        .current_dir(company)
        .output()
        .unwrap();
    assert_eq!(
        sync_out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&sync_out.stderr)
    );
    let after_sync = git_uplink::read_queue(company).unwrap();
    let queued = if after_sync.pending_upstream.is_some() {
        let acc = Command::new(bin)
            .args(["accept-upstream"])
            .current_dir(company)
            .output()
            .unwrap();
        assert_eq!(
            acc.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&acc.stderr)
        );
        git_uplink::read_queue(company).unwrap()
    } else {
        after_sync
    };
    let conflicted = queued.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, PatchStatus::Conflict);
    let conflict_branch = conflicted
        .conflict
        .as_ref()
        .map(|c| c.branch.as_str())
        .unwrap();
    assert_eq!(conflict_branch, format!("uplink/conflict/{}", ttl_patch.id));
    let work_branch = work_branch_of(conflicted);
    assert_eq!(
        work_branch,
        format!("uplink/conflict/{}-work", ttl_patch.id)
    );

    let on_main = git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
    assert_eq!(on_main, "main");
    let main_tokens = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    assert!(main_tokens.contains("return 7200;"), "{main_tokens}");
    assert!(!main_tokens.contains("<<<<<<"), "{main_tokens}");

    git(
        company,
        &["checkout", "--quiet", &work_branch],
        GitOpts::default(),
    )
    .unwrap();
    let tokens = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    assert!(
        tokens.contains("<<<<<<") || tokens.contains("1800") || tokens.contains("7200"),
        "{tokens}"
    );
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();
    resolve_conflict(company, &ttl_patch.id).unwrap();

    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patch_refs()[0].status, PatchStatus::Queued);
    let tokens = snapshot.product_files.get("src/tokens.js").unwrap();
    assert!(tokens.contains("return 7200;"));
    assert!(!tokens.contains("return 1800;"));
}

#[test]
fn resolve_refuses_a_resolution_that_fails_the_assessment() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let ttl_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(ttl_patch.assess.as_ref().unwrap().ok);

    set_settings(company, |s| s.redact_keywords = vec!["AcmeCorp".into()]);

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 1800;"),
    );
    commit_all(upstream, "shorten default ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let queued = sync_apply(company);
    let conflicted = queued.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, PatchStatus::Conflict);
    let conflict_branch = conflicted.conflict.as_ref().unwrap().branch.clone();
    git(
        company,
        &["checkout", "--quiet", &conflict_branch],
        GitOpts::default(),
    )
    .unwrap();
    // The resolution adds company text the original change did not have.
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200; // AcmeCorp SLA"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();
    let before = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    let err = resolve_conflict(company, &ttl_patch.id).unwrap_err();
    assert!(
        err.to_string().contains("Upstream assessment failed"),
        "{err}"
    );
    assert!(err.to_string().contains("affiliation-leak"), "{err}");
    // Refused: the branch and the staged resolution are as they were, and
    // the patch is still in conflict.
    assert_eq!(git_ok(company, &["rev-parse", "HEAD"]).unwrap(), before);
    let staged = git_ok(company, &["diff", "--cached", "--name-only"]).unwrap();
    assert_eq!(staged, "src/tokens.js");
    let queue = status_snapshot(company).unwrap().queue;
    let still = queue.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(still.status, PatchStatus::Conflict);

    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();
    resolve_conflict(company, &ttl_patch.id).unwrap();
    let queue = status_snapshot(company).unwrap().queue;
    let resolved = queue.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(resolved.status, PatchStatus::Queued);
    assert!(resolved.assess.as_ref().unwrap().ok);
}

#[test]
fn submitted_conflict_resolve_requires_delta_approval_and_keeps_the_pr() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let ttl_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    commit_contribution_packet(company, &ttl_patch);
    let first = approve_patch(company, &ttl_patch.id).unwrap();
    assert_eq!(first.approvals.len(), 1);
    assert_eq!(first.approvals[0].kind, "initial");
    let still = approve_patch(company, &ttl_patch.id).unwrap();
    assert_eq!(still.approvals.len(), 1);
    let submitted = submit_patch(company, &ttl_patch.id, true).unwrap();
    record_pull_request(
        company,
        &ttl_patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &submitted.branch,
        None,
    )
    .unwrap();
    let noop = approve_patch(company, &ttl_patch.id).unwrap();
    assert_eq!(noop.status, PatchStatus::Submitted);
    assert_eq!(noop.approvals.len(), 1);

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 1800;"),
    );
    commit_all(upstream, "shorten default ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let queued = sync_apply(company);
    let conflicted = queued.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, PatchStatus::Conflict);
    let conflict_branch = conflicted.conflict.as_ref().unwrap().branch.clone();
    // A patch in conflict blocks every later rebuild. Only resolve, drop or
    // an upstream merge moves it on; a replay never puts it back to submitted.
    let err = rebuild(company).unwrap_err();
    assert!(err.to_string().contains("blocked on conflict"), "{err}");
    assert_eq!(patch_status(company, &ttl_patch.id), PatchStatus::Conflict);
    let err = record_pull_request(
        company,
        &ttl_patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &submitted.branch,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("is conflict"), "{err}");
    git(
        company,
        &["checkout", "--quiet", &conflict_branch],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();
    resolve_conflict(company, &ttl_patch.id).unwrap();

    let after_resolve = status_snapshot(company).unwrap();
    let amended = after_resolve
        .queue
        .all_patches()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
    assert_eq!(amended.status, PatchStatus::Amended);
    assert_eq!(summarize_queue(&after_resolve.queue).amended, 1);
    let fork_after_resolve =
        git_ok(company, &["rev-parse", &format!("uplink/{}", ttl_patch.id)]).unwrap();
    assert_eq!(fork_after_resolve, submitted.sha);
    // Recording the PR again must not skip the delta approval.
    let err = record_pull_request(
        company,
        &ttl_patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &submitted.branch,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("is amended"), "{err}");
    assert_eq!(patch_status(company, &ttl_patch.id), PatchStatus::Amended);
    let err = submit_patch(company, &ttl_patch.id, true).unwrap_err();
    assert!(err.to_string().contains("must be approved"), "{}", err);

    let packet = format_contribution_packet(company, amended).unwrap();
    assert!(packet.contains(&format!("Delta packet — {}", ttl_patch.id)));
    assert!(packet.contains("already IP-approved"));
    assert!(packet.contains("Already approved (initial)"));
    assert!(packet.contains("## Upstream commit message"));
    assert!(!packet.contains("### Company main"));
    assert!(!packet.contains("Queue status"));
    assert!(packet.contains("## Upstream Assessment: ✅"), "{packet}");
    assert!(packet.contains(&amended.approvals[0].sha));
    assert!(packet.contains("Uplink-Patch-Id"));

    let (_, prepare_path, _) = report_paths(&ttl_patch.id).unwrap();
    write(company, &prepare_path, &packet);
    git_uplink::commit_queue(
        company,
        &format!("uplink: contribution packet {}", ttl_patch.id),
    )
    .unwrap();

    let second = approve_patch(company, &ttl_patch.id).unwrap();
    assert_eq!(second.status, PatchStatus::Approved);
    assert_eq!(second.approvals.len(), 2);
    assert_eq!(second.approvals[1].kind, "delta");
    assert_ne!(second.approvals[0].sha, second.approvals[1].sha);

    let resubmitted = submit_patch(company, &ttl_patch.id, true).unwrap();
    let recorded = record_pull_request(
        company,
        &ttl_patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &resubmitted.branch,
        None,
    )
    .unwrap();
    assert_eq!(recorded.status, PatchStatus::Submitted);
    assert_eq!(recorded.upstream.as_ref().unwrap().pr_number, Some(99));
    assert_eq!(
        recorded.upstream.as_ref().unwrap().pr_url.as_deref(),
        Some("https://github.com/upstream/tokenkit/pull/99")
    );
    let exported = git_ok(
        company,
        &["show", &format!("{}:src/tokens.js", resubmitted.branch)],
    )
    .unwrap();
    assert!(exported.contains("return 7200;"));
    assert!(!exported.contains("return 1800;"));

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 900;"),
    );
    commit_all(upstream, "even shorter ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let again = sync_apply(company);
    let conflicted = again.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, PatchStatus::Conflict);
    let conflict_branch = conflicted.conflict.as_ref().unwrap().branch.clone();
    git(
        company,
        &["checkout", "--quiet", &conflict_branch],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();
    resolve_conflict(company, &ttl_patch.id).unwrap();
    let third = status_snapshot(company).unwrap();
    let amended = third
        .queue
        .all_patches()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
    assert_eq!(amended.status, PatchStatus::Amended);
    let packet = format_contribution_packet(company, amended).unwrap();
    assert!(packet.contains("Already approved (initial)"));
    assert!(packet.contains("Already approved (delta)"));
    assert!(packet.contains(&amended.approvals[1].sha));
}

#[test]
fn resolving_asha_records_a_follow_on_conflict_on_ben() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;

    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let asha = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    let with_ttl = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &with_ttl.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let ben = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS
            .replace("return sha1(value);", "return saltedSha256(value);")
            .replace("return 3600;", "return 1800;"),
    );
    commit_all(upstream, "salt the hash and shorten ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    sync_apply(company);

    let queued = git_uplink::read_queue(company).unwrap();
    let asha_conflicted = queued.all_patches().find(|p| p.id == asha.id).unwrap();
    assert_eq!(asha_conflicted.status, PatchStatus::Conflict);
    assert_eq!(
        queued
            .all_patches()
            .find(|p| p.id == ben.id)
            .unwrap()
            .status,
        PatchStatus::Queued
    );
    let asha_branch = asha_conflicted
        .conflict
        .as_ref()
        .map(|c| c.branch.as_str())
        .unwrap()
        .to_string();

    git(
        company,
        &["checkout", "--quiet", &asha_branch],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS
            .replace("return sha1(value);", "return saltedSha256(value);")
            .replace("return 3600;", "return 7200;"),
    );
    git(company, &["add", "src/tokens.js"], GitOpts::default()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(["resolve", &asha.id])
        .current_dir(company)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "expected resolve to succeed after amending asha, got {:?}\n{stderr}",
        output.status.code()
    );
    assert!(
        stderr.contains(&ben.id),
        "follow-on conflict should name ben, stderr:\n{stderr}"
    );

    let snapshot = status_snapshot(company).unwrap();
    let asha_after = snapshot
        .queue
        .all_patches()
        .find(|p| p.id == asha.id)
        .unwrap();
    let ben_after = snapshot
        .queue
        .all_patches()
        .find(|p| p.id == ben.id)
        .unwrap();
    assert_ne!(asha_after.status, PatchStatus::Conflict);
    assert_eq!(ben_after.status, PatchStatus::Conflict);
    let ben_branch = ben_after
        .conflict
        .as_ref()
        .map(|c| c.branch.as_str())
        .unwrap();
    assert_eq!(ben_branch, format!("uplink/conflict/{}", ben.id));

    let head = git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
    assert_eq!(head, "main");
    git(
        company,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{ben_branch}"),
        ],
        GitOpts::default(),
    )
    .unwrap();
}

#[test]
fn refuses_to_submit_internal_only_patches_and_exports_approved_ones() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/telemetry"],
        GitOpts::default(),
    )
    .unwrap();
    let hashed = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &format!("{hashed}\nexport const vendor = true;\n"),
    );
    commit_all(company, "vendor flag");
    let internal = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Vendor flag".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let err = approve_patch(company, &internal.id).unwrap_err();
    assert!(err.to_string().contains("internal-only"));
    approve_patch(company, &hash_patch.id).unwrap();
    let submitted = submit_patch(company, &hash_patch.id, true).unwrap();
    record_pull_request(
        company,
        &hash_patch.id,
        42,
        "https://github.com/upstream/tokenkit/pull/42",
        &submitted.branch,
        None,
    )
    .unwrap();
    assert_eq!(submitted.branch, format!("uplink/{}", hash_patch.id));
    let exported = git_ok(
        company,
        &["show", &format!("{}:src/tokens.js", submitted.branch)],
    )
    .unwrap();
    assert!(exported.contains("sha256"));
    assert!(!exported.contains("vendor"));

    mark_merged(company, &hash_patch.id, MergeVia::Pr, Some(&submitted.sha)).unwrap();
    rebuild(company).unwrap();
    let after = status_snapshot(company).unwrap();
    assert_eq!(
        after
            .queue
            .all_patches()
            .find(|p| p.id == hash_patch.id)
            .unwrap()
            .status,
        PatchStatus::Merged
    );
    assert!(
        after
            .product_files
            .get("src/tokens.js")
            .unwrap()
            .contains("vendor")
    );
}

#[test]
fn can_drop_an_internal_only_patch_from_the_company_build() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/telemetry"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &format!("{TOKENS}\nexport const vendor = true;\n"),
    );
    commit_all(company, "vendor flag");
    let internal = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Vendor flag".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop_patch(company, &internal.id, "no longer needed").unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert!(
        !snapshot
            .product_files
            .get("src/tokens.js")
            .unwrap()
            .contains("vendor")
    );
    assert_eq!(snapshot.queue.patch_refs()[0].status, PatchStatus::Dropped);
}

#[test]
fn imports_as_queued_not_contribution_approved() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(88),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(patch.status, PatchStatus::Queued);
    let err = submit_patch(company, &patch.id, true).unwrap_err();
    assert!(err.to_string().contains("must be approved"));
    let again = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(88),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(again.id, patch.id);
    assert_eq!(
        status_snapshot(company)
            .unwrap()
            .queue
            .all_patches()
            .count(),
        1
    );
}

#[test]
fn merge_then_import_stays_queued_and_approvable() {
    let world = setup_world();
    let company = &world.company;
    let from_sha = git_ok(company, &["rev-parse", "main"]).unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        company,
        &["merge", "--ff-only", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();

    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some(from_sha),
            head_ref: Some(head_sha),
            internal_pr_number: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(patch.status, PatchStatus::Queued);
    let upstream_tokens = git_ok(company, &["show", "uplink/upstream:src/tokens.js"]).unwrap();
    assert!(upstream_tokens.contains("return sha1(value);"));
    assert!(!upstream_tokens.contains("return sha256(value);"));
    approve_patch(company, &patch.id).unwrap();
    assert_eq!(
        status_snapshot(company).unwrap().queue.patch_refs()[0].status,
        PatchStatus::Approved
    );
}

#[test]
fn import_marks_merged_when_already_on_upstream() {
    let world = setup_world();
    let company = &world.company;
    let from_sha = git_ok(company, &["rev-parse", "main"]).unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["branch", "-f", "uplink/upstream", &head_sha],
        GitOpts::default(),
    )
    .unwrap();
    git(company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        company,
        &["merge", "--ff-only", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    let main_before = git_ok(company, &["rev-parse", "main"]).unwrap();

    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some(from_sha),
            head_ref: Some(head_sha),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(patch.status, PatchStatus::Merged);
    assert_eq!(
        git_ok(company, &["rev-parse", "main"]).unwrap(),
        main_before
    );
}

#[test]
fn serializes_two_adds_in_one_checkout_so_both_patches_survive() {
    let world = setup_world();
    let company = world.company.clone();
    let main_sha = git_ok(&company, &["rev-parse", "main"]).unwrap();

    git(
        &company,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&company, "README.md", "from-asha\n");
    commit_all(&company, "readme from asha");
    let sha_a = git_ok(&company, &["rev-parse", "HEAD"]).unwrap();

    git(
        &company,
        &["checkout", "-B", "feat/notes", &main_sha],
        GitOpts::default(),
    )
    .unwrap();
    write(&company, "NOTES.md", "from-ben\n");
    commit_all(&company, "notes from ben");
    let sha_b = git_ok(&company, &["rev-parse", "HEAD"]).unwrap();

    land_on_main(&company, &sha_a);
    land_on_main(&company, &sha_b);

    let handle_a = {
        let company_a = company.clone();
        let main_sha = main_sha.clone();
        let sha_a = sha_a.clone();
        thread::spawn(move || {
            add_patch(
                &company_a,
                AddPatchOpts {
                    title: "Readme from Asha".into(),
                    from_ref: Some(main_sha),
                    head_ref: Some(sha_a),
                    internal_pr_number: Some(101),
                    ..Default::default()
                },
            )
        })
    };
    let handle_b = {
        let company_b = company.clone();
        thread::spawn(move || {
            add_patch(
                &company_b,
                AddPatchOpts {
                    title: "Notes from Ben".into(),
                    from_ref: Some(main_sha),
                    head_ref: Some(sha_b),
                    internal_pr_number: Some(102),
                    ..Default::default()
                },
            )
        })
    };
    handle_a.join().unwrap().unwrap();
    handle_b.join().unwrap().unwrap();

    let snapshot = status_snapshot(&company).unwrap();
    assert_eq!(snapshot.queue.all_patches().count(), 2);
    assert!(
        snapshot
            .product_files
            .get("README.md")
            .unwrap()
            .contains("from-asha")
    );
    assert!(
        snapshot
            .product_files
            .get("NOTES.md")
            .unwrap()
            .contains("from-ben")
    );
}

#[test]
fn retries_concurrent_adds_from_two_clones_against_a_shared_origin() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);

    let (asha_keep, asha) = clone_company_from(&origin, &upstream);
    let (ben_keep, ben) = clone_company_from(&origin, &upstream);

    git(
        &asha,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&asha, "README.md", "from-asha\n");
    commit_all(&asha, "readme from asha");
    let from_sha = git_ok(&asha, &["rev-parse", "main"]).unwrap();
    let sha_a = git_ok(&asha, &["rev-parse", "HEAD"]).unwrap();

    git(&ben, &["checkout", "-b", "feat/notes"], GitOpts::default()).unwrap();
    write(&ben, "NOTES.md", "from-ben\n");
    commit_all(&ben, "notes from ben");
    let sha_b = git_ok(&ben, &["rev-parse", "HEAD"]).unwrap();

    land_on_main(&asha, &sha_a);
    git(
        &asha,
        &["push", "--quiet", "origin", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &[
            "fetch",
            "--quiet",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["checkout", "-f", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["reset", "--hard", "--quiet", "origin/main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["merge", "--no-edit", "--quiet", &sha_b],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["push", "--quiet", "origin", "main"],
        GitOpts::default(),
    )
    .unwrap();

    let asha_t = asha.clone();
    let ben_t = ben.clone();
    let from_a = from_sha.clone();
    let from_b = from_sha;
    let h1 = thread::spawn(move || {
        add_patch(
            &asha_t,
            AddPatchOpts {
                title: "Readme from Asha".into(),
                from_ref: Some(from_a),
                head_ref: Some(sha_a),
                internal_pr_number: Some(201),
                ..Default::default()
            },
        )?;
        push_queue(
            &asha_t,
            PushOpts {
                push_remote: Some("origin".into()),
            },
        )
        .map(|_| ())
    });
    let h2 = thread::spawn(move || {
        add_patch(
            &ben_t,
            AddPatchOpts {
                title: "Notes from Ben".into(),
                from_ref: Some(from_b),
                head_ref: Some(sha_b),
                internal_pr_number: Some(202),
                ..Default::default()
            },
        )?;
        push_queue(
            &ben_t,
            PushOpts {
                push_remote: Some("origin".into()),
            },
        )
        .map(|_| ())
    });
    h1.join().unwrap().unwrap();
    h2.join().unwrap().unwrap();

    let (_integrated_keep, integrated) = clone_company_from(&origin, &upstream);
    let snapshot = status_snapshot(&integrated).unwrap();
    let mut titles: Vec<_> = snapshot
        .queue
        .all_patches()
        .map(|p| p.title.clone())
        .collect();
    titles.sort();
    assert_eq!(titles, vec!["Notes from Ben", "Readme from Asha"]);
    assert!(
        snapshot
            .product_files
            .get("README.md")
            .unwrap()
            .contains("from-asha")
    );
    assert!(
        snapshot
            .product_files
            .get("NOTES.md")
            .unwrap()
            .contains("from-ben")
    );
    drop((asha_keep, ben_keep));
}

#[test]
fn push_publishes_a_local_add() {
    let world = setup_world();
    let company = &world.company;
    let origin = publish_origin(company);
    git(
        company,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "README.md", "from-local\n");
    commit_all(company, "readme");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let local = git_ok(company, &["rev-parse", STATE_BRANCH]).unwrap();
    let remote_before = git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_ne!(local, remote_before);
    let result = push_queue(
        company,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(result.action, "pushed");
    assert_eq!(
        git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap(),
        local
    );
}

#[test]
fn status_reports_ahead_after_local_add() {
    let world = setup_world();
    let company = &world.company;
    let _origin = publish_origin(company);
    git(
        company,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "README.md", "from-local\n");
    commit_all(company, "readme");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.state.ahead, Some(1));
    assert_eq!(snapshot.state.behind, Some(0));
    assert!(
        snapshot.state.uncommitted.is_empty(),
        "{:?}",
        snapshot.state.uncommitted
    );
    assert!(snapshot.state.remote.is_some());
}

#[test]
fn status_reports_behind_when_origin_moved() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);
    let (_ahead_keep, ahead) = clone_company_from(&origin, &upstream);
    let (_behind_keep, behind) = clone_company_from(&origin, &upstream);

    git(
        &ahead,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&ahead, "README.md", "from-ahead\n");
    commit_all(&ahead, "readme");
    add_landed_patch(
        &ahead,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    push_queue(
        &ahead,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();

    let local_before = git_ok(&behind, &["rev-parse", STATE_BRANCH]).unwrap();
    let snapshot = status_snapshot(&behind).unwrap();
    assert_eq!(
        git_ok(&behind, &["rev-parse", STATE_BRANCH]).unwrap(),
        local_before
    );
    assert_eq!(snapshot.state.ahead, Some(0));
    assert_eq!(snapshot.state.behind, Some(1));
    assert!(
        snapshot.state.uncommitted.is_empty(),
        "{:?}",
        snapshot.state.uncommitted
    );
}

#[test]
fn status_reports_uncommitted_queue() {
    let world = setup_world();
    let company = &world.company;
    let _origin = publish_origin(company);
    let path = company.join(".uplink/queue.json");
    let raw = fs::read_to_string(&path).unwrap();
    fs::write(&path, raw.replace("\"version\": 1", "\"version\": 1 ")).unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert!(
        snapshot
            .state
            .uncommitted
            .iter()
            .any(|path| path == ".uplink/queue.json"),
        "{:?}",
        snapshot.state.uncommitted
    );
    assert_eq!(snapshot.state.ahead, Some(0));
    assert_eq!(snapshot.state.behind, Some(0));
}

#[test]
fn status_cli_table_or_json() {
    let world = setup_world();
    let company = &world.company;
    let _origin = publish_origin(company);
    let bin = env!("CARGO_BIN_EXE_git-uplink");
    let table = Command::new(bin)
        .arg("status")
        .current_dir(company)
        .output()
        .unwrap();
    assert!(table.status.success(), "{:?}", table);
    let text = String::from_utf8_lossy(&table.stdout);
    assert!(!text.trim_start().starts_with('{'), "{text}");
    assert!(text.contains("uplink/state"), "{text}");
    assert!(text.contains("up to date"), "{text}");
    assert!(text.contains("id"), "{text}");

    let json = Command::new(bin)
        .args(["status", "--json"])
        .current_dir(company)
        .output()
        .unwrap();
    assert!(json.status.success(), "{:?}", json);
    let text = String::from_utf8_lossy(&json.stdout);
    let value: serde_json::Value = serde_json::from_str(text.trim()).expect(&text);
    assert!(value.get("state").is_some(), "{text}");
    assert!(value.get("upstream").is_some(), "{text}");
    assert!(value.get("internal").is_some(), "{text}");
    assert!(value.get("counts").is_some(), "{text}");
    assert_eq!(value["state"]["ahead"], 0);
    assert_eq!(value["state"]["behind"], 0);
}

#[test]
fn push_appends_local_only_patches_when_origin_moved() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);
    let (asha_keep, asha) = clone_company_from(&origin, &upstream);
    let (ben_keep, ben) = clone_company_from(&origin, &upstream);

    git(
        &asha,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&asha, "README.md", "from-asha\n");
    commit_all(&asha, "readme from asha");
    add_landed_patch(
        &asha,
        AddPatchOpts {
            title: "Readme from Asha".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(201),
            ..Default::default()
        },
    )
    .unwrap();

    git(&ben, &["checkout", "-b", "feat/notes"], GitOpts::default()).unwrap();
    write(&ben, "NOTES.md", "from-ben\n");
    commit_all(&ben, "notes from ben");
    add_landed_patch(
        &ben,
        AddPatchOpts {
            title: "Notes from Ben".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(202),
            ..Default::default()
        },
    )
    .unwrap();
    push_queue(
        &ben,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();

    let result = push_queue(
        &asha,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(result.action, "restacked");

    let (_integrated_keep, integrated) = clone_company_from(&origin, &upstream);
    let patch_titles: Vec<_> = status_snapshot(&integrated)
        .unwrap()
        .queue
        .all_patches()
        .filter(|p| p.kind.is_none())
        .map(|p| p.title.clone())
        .collect();
    assert_eq!(patch_titles, ["Notes from Ben", "Readme from Asha"]);
    drop((asha_keep, ben_keep));
}

fn world_with_shared_patch() -> (World, PathBuf, Patch) {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "README.md", "shared\n");
    commit_all(company, "shared readme");
    let shared = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Shared readme".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(301),
            ..Default::default()
        },
    )
    .unwrap();
    let origin = publish_origin(company);
    (world, origin, shared)
}

fn add_notes_patch(repo: &Path) {
    git(repo, &["checkout", "-b", "feat/notes"], GitOpts::default()).unwrap();
    write(repo, "NOTES.md", "from-ben\n");
    commit_all(repo, "notes from ben");
    add_landed_patch(
        repo,
        AddPatchOpts {
            title: "Notes from Ben".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(302),
            ..Default::default()
        },
    )
    .unwrap();
}

fn push_origin(repo: &Path) -> Result<git_uplink::PushResult> {
    push_queue(
        repo,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
}

#[test]
fn push_keeps_a_local_drop_of_a_patch_that_is_also_on_origin() {
    let (world, origin, shared) = world_with_shared_patch();
    let (asha_keep, asha) = clone_company_from(&origin, &world.upstream);
    let (ben_keep, ben) = clone_company_from(&origin, &world.upstream);

    drop_patch(&asha, &shared.id, "no longer needed").unwrap();
    add_notes_patch(&ben);
    push_origin(&ben).unwrap();

    let result = push_origin(&asha).unwrap();
    assert_eq!(result.action, "restacked");

    let (_integrated_keep, integrated) = clone_company_from(&origin, &world.upstream);
    let queue = status_snapshot(&integrated).unwrap().queue;
    let shared_now = queue.all_patches().find(|p| p.id == shared.id).unwrap();
    assert_eq!(shared_now.status, PatchStatus::Dropped);
    assert!(queue.all_patches().any(|p| p.title == "Notes from Ben"));
    drop((asha_keep, ben_keep));
}

#[test]
fn push_refuses_when_origin_changed_the_same_patch() {
    let (world, origin, shared) = world_with_shared_patch();
    let (asha_keep, asha) = clone_company_from(&origin, &world.upstream);
    let (ben_keep, ben) = clone_company_from(&origin, &world.upstream);

    drop_patch(&asha, &shared.id, "no longer needed").unwrap();
    approve_patch(&ben, &shared.id).unwrap();
    push_origin(&ben).unwrap();

    let err = push_origin(&asha).unwrap_err().to_string();
    assert!(err.contains("changed both locally and on origin"), "{err}");
    drop((asha_keep, ben_keep));
}

#[test]
fn rebuild_push_leaves_origin_main_when_state_push_is_rejected() {
    let world = setup_world();
    let origin = publish_origin(&world.company);
    let (asha_keep, asha) = clone_company_from(&origin, &world.upstream);
    let (ben_keep, ben) = clone_company_from(&origin, &world.upstream);
    git(&asha, &["checkout", "--quiet", "main"], GitOpts::default()).unwrap();

    add_notes_patch(&ben);
    push_origin(&ben).unwrap();
    let origin_main = rev_of(&origin, "main");

    git(
        &asha,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&asha, "README.md", "from-asha\n");
    commit_all(&asha, "readme from asha");
    add_landed_patch(
        &asha,
        AddPatchOpts {
            title: "Readme from Asha".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(401),
            ..Default::default()
        },
    )
    .unwrap();

    let err = rebuild_with(
        &asha,
        RebuildOpts {
            push: true,
            ..Default::default()
        },
    );
    assert!(err.is_err(), "state push should be rejected");
    assert_eq!(rev_of(&origin, "main"), origin_main);
    drop((asha_keep, ben_keep));
}

#[test]
fn push_fast_forwards_when_local_is_behind() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);
    let (_ahead_keep, ahead) = clone_company_from(&origin, &upstream);
    let (_behind_keep, behind) = clone_company_from(&origin, &upstream);

    git(
        &ahead,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&ahead, "README.md", "from-ahead\n");
    commit_all(&ahead, "readme");
    add_landed_patch(
        &ahead,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    push_queue(
        &ahead,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    let origin_sha = git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap();
    let behind_before = git_ok(&behind, &["rev-parse", STATE_BRANCH]).unwrap();
    assert_ne!(behind_before, origin_sha);

    let result = push_queue(
        &behind,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(result.action, "fast-forwarded");
    assert_eq!(
        git_ok(&behind, &["rev-parse", STATE_BRANCH]).unwrap(),
        origin_sha
    );
    assert_eq!(
        git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap(),
        origin_sha
    );
}

#[test]
fn push_is_a_no_op_when_tips_match() {
    let world = setup_world();
    let company = &world.company;
    publish_origin(company);
    let sha = git_ok(company, &["rev-parse", STATE_BRANCH]).unwrap();
    let result = push_queue(
        company,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(result.action, "up-to-date");
    assert_eq!(result.sha, sha);
    assert_eq!(git_ok(company, &["rev-parse", STATE_BRANCH]).unwrap(), sha);
}

#[test]
fn push_skips_a_local_patch_whose_internal_pr_is_already_on_origin() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);
    let (_a_keep, asha) = clone_company_from(&origin, &upstream);
    let (_b_keep, ben) = clone_company_from(&origin, &upstream);

    git(
        &asha,
        &["checkout", "-b", "feat/readme"],
        GitOpts::default(),
    )
    .unwrap();
    write(&asha, "README.md", "from-pr\n");
    commit_all(&asha, "readme");
    add_landed_patch(
        &asha,
        AddPatchOpts {
            title: "Readme".into(),
            from_ref: Some("main".into()),
            internal_pr_number: Some(99),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        &asha,
        &["push", "--quiet", "origin", "main"],
        GitOpts::default(),
    )
    .unwrap();
    push_queue(
        &asha,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();

    git(
        &ben,
        &[
            "fetch",
            "--quiet",
            "origin",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["checkout", "-f", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &ben,
        &["reset", "--hard", "--quiet", "origin/main"],
        GitOpts::default(),
    )
    .unwrap();
    add_patch(
        &ben,
        AddPatchOpts {
            title: "Readme again".into(),
            from_ref: Some("main^".into()),
            head_ref: Some("HEAD".into()),
            internal_pr_number: Some(99),
            ..Default::default()
        },
    )
    .unwrap();
    let origin_before = git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap();
    let result = push_queue(
        &ben,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();
    assert_eq!(result.action, "fast-forwarded");
    assert_eq!(
        git_ok(&origin, &["rev-parse", STATE_BRANCH]).unwrap(),
        origin_before
    );
    let titles: Vec<_> = status_snapshot(&ben)
        .unwrap()
        .queue
        .all_patches()
        .filter(|p| p.kind.is_none())
        .map(|p| p.title.clone())
        .collect();
    assert_eq!(titles, vec!["Readme"]);
}

#[test]
fn refuses_import_when_a_stacked_change_does_not_declare_depends_on() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/logs"],
        GitOpts::default(),
    )
    .unwrap();
    let hashed = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &hashed.replace(
            "return sha256(value);",
            "console.log(\"hash\");\n  return sha256(value);",
        ),
    );
    commit_all(company, "add log");

    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Log token hashes".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    match err {
        Error::Preflight(pre) => {
            assert_eq!(pre.suggested_depends_on, vec![hash_patch.id.clone()]);
            assert!(pre.to_string().contains("--depends-on"));
        }
        other => panic!("expected preflight, got {other}"),
    }
    assert_eq!(
        status_snapshot(company)
            .unwrap()
            .queue
            .all_patches()
            .count(),
        1
    );
}

#[test]
fn refuses_import_when_export_build_fails_without_the_used_patches() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/check"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/check-hash.js",
        "if (hash(\"x\") !== \"ok\") throw new Error(\"need hash\");\n",
    );
    commit_all(company, "add checker");

    set_preflight_script(company, "grep -q sha256 src/tokens.js");
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Add hash checker".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    match err {
        Error::Preflight(pre) => {
            assert_eq!(pre.stage, "command");
            assert_eq!(pre.suggested_depends_on, vec![hash_patch.id.clone()]);
        }
        other => panic!("expected preflight, got {other}"),
    }

    let imported = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Add hash checker".into(),
            from_ref: Some("main".into()),
            depends_on: vec![hash_patch.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(imported.depends_on, vec![hash_patch.id]);
    assert_eq!(imported.status, PatchStatus::Queued);
}

#[test]
fn records_depends_on_from_commit_message_trailers() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &fs::read_to_string(company.join("src/tokens.js"))
            .unwrap()
            .replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "extend ttl");
    let ttl_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend token TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/logs"],
        GitOpts::default(),
    )
    .unwrap();
    let hashed = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &hashed.replace(
            "return sha256(value);",
            "console.log(\"hash\");\n  return sha256(value);",
        ),
    );
    commit_all(company, "add log");

    let imported = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Log token hashes".into(),
            message: Some(format!(
                "Log token hashes\n\n\
Public rationale.\n\n\
<!--\nUplink-Depends-On: upl_deadbeef00\n-->\n\n\
See also {} in the queue.\n\n\
{DEFAULT_CUTOFF}\n\n\
Uplink-Depends-On: {}\n\
Uplink-Depends-On: {}\n",
                ttl_patch.id, hash_patch.id, ttl_patch.id
            )),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        imported.depends_on,
        vec![hash_patch.id.clone(), ttl_patch.id.clone()]
    );
}

#[test]
fn incoming_preflight_reads_depends_on_from_the_message() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/logs"],
        GitOpts::default(),
    )
    .unwrap();
    let hashed = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &hashed.replace(
            "return sha256(value);",
            "console.log(\"hash\");\n  return sha256(value);",
        ),
    );
    commit_all(company, "add log");
    let head = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    let from = git_ok(company, &["rev-parse", "main"]).unwrap();

    preflight_incoming_change(
        company,
        IncomingPreflight {
            title: "Log token hashes".into(),
            from_ref: from,
            head_ref: head,
            depends_on: Vec::new(),
            message: Some(format!(
                "Log token hashes\n\nUplink-Depends-On: {}\n",
                hash_patch.id
            )),
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap();
}

#[test]
fn does_not_submit_or_push_when_export_tests_fail() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(company, &hash_patch.id).unwrap();

    // The failing script lives on this repo's uplink/hooks only.
    set_preflight_script(company, "exit 1");
    let err = submit_patch(company, &hash_patch.id, true);
    assert!(matches!(err, Err(Error::Preflight(_))));

    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patch_refs()[0].status, PatchStatus::Approved);
    assert!(
        snapshot.queue.patch_refs()[0]
            .upstream
            .as_ref()
            .and_then(|u| u.pr_number)
            .is_none()
    );
}

#[test]
fn user_git_config_and_hooks_do_not_change_the_imported_patch() {
    let world = setup_world();
    let company = &world.company;
    for (key, value) in [
        ("color.ui", "always"),
        ("color.diff", "always"),
        ("diff.noprefix", "true"),
        ("diff.external", "false"),
        ("format.signOff", "true"),
    ] {
        git(company, &["config", key, value], GitOpts::default()).unwrap();
    }
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hook = company.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let body = fs::read_to_string(tooling_patch_path(company, &patch.id)).unwrap();
    assert!(!body.contains('\u{1b}'), "{body}");
    assert!(body.contains("--- a/src/tokens.js"), "{body}");
    assert!(body.contains("+++ b/src/tokens.js"), "{body}");
    assert!(!body.contains("Signed-off-by"), "{body}");
}

#[test]
fn preflight_command_does_not_see_uplink_credentials() {
    let world = setup_world();
    let company = &world.company;
    set_preflight_script(
        company,
        r#"test -z "$UPLINK_CONTRIB_TOKEN$UPLINK_INTERNAL_KEY$GITHUB_TOKEN""#,
    );

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");

    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args([
            "preflight",
            "--from",
            "main",
            "--head",
            "HEAD",
            "--title",
            "Use SHA-256 for tokens",
        ])
        .current_dir(company)
        .env("UPLINK_CONTRIB_TOKEN", "contrib-secret")
        .env("UPLINK_INTERNAL_KEY", "internal-secret")
        .env("GITHUB_TOKEN", "github-secret")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn strips_the_internal_commit_section_and_adds_a_co_author_trailer() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "seed-internal\n");
    commit_all(company, "seed internal");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Seed internal".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "wip: ignore this git log");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            message: Some(hash_pr_message()),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let prepare = patch.assess.as_ref().unwrap();
    assert!(prepare.ok);
    assert!(prepare.cutoff_found);
    assert_eq!(
        prepare.co_author.as_deref(),
        Some("Jane Public <jane@users.noreply.github.com>")
    );
    assert!(!patch.commit_message.contains("Visible while writing"));
    assert!(!patch.commit_message.contains("wip: ignore this git log"));
    assert!(patch.commit_message.contains("PROJ-9999"));
    assert!(patch.commit_message.contains(DEFAULT_CUTOFF));

    let company_log = git_ok(company, &["log", "--format=%B", "main"]).unwrap();
    assert!(company_log.contains("Use SHA-256 for tokens"));
    assert!(company_log.contains(&format!("Uplink-Patch-Id: {}", patch.id)));
    assert!(!company_log.contains("wip: ignore this git log"));

    let stored =
        fs::read_to_string(company.join(format!(".uplink/patches/{}.patch", patch.id))).unwrap();
    assert!(stored.contains("Replace SHA-1 in the default hasher."));
    assert!(stored.contains("PROJ-9999"));
    assert!(!stored.contains("Visible while writing"));

    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id, true).unwrap();
    let contrib_msg = git_ok(company, &["log", "-1", "--format=%B", &submitted.branch]).unwrap();
    assert!(contrib_msg.contains("Replace SHA-1 in the default hasher."));
    assert!(!contrib_msg.contains("PROJ-9999"));
    assert!(!contrib_msg.contains(DEFAULT_CUTOFF));
    assert!(!contrib_msg.contains("Uplink-Export-Author"));
    assert!(contrib_msg.ends_with(&format!(
        "\n\nUplink-Patch-Id: {}\nCo-Authored-By: Jane Public <jane@users.noreply.github.com>\n",
        patch.id
    )), "{contrib_msg:?}");
    let trailers = git_ok(
        company,
        &[
            "log",
            "-1",
            "--format=%(trailers:key=Co-Authored-By,valueonly)",
            &submitted.branch,
        ],
    )
    .unwrap();
    assert_eq!(
        trailers.trim(),
        "Jane Public <jane@users.noreply.github.com>"
    );
}

#[test]
fn exports_no_co_author_trailer_without_an_export_author_header() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            message: Some("Use SHA-256 for tokens\n\nReplace SHA-1.\n".into()),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let assess = patch.assess.as_ref().unwrap();
    assert_eq!(assess.co_author, None);
    let check = assess.checks.iter().find(|c| c.id == "co-author").unwrap();
    assert_eq!(check.status, CheckStatus::Skip);

    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id, true).unwrap();
    let contrib_msg = git_ok(company, &["log", "-1", "--format=%B", &submitted.branch]).unwrap();
    assert!(!contrib_msg.contains("Co-Authored-By"), "{contrib_msg}");
}

#[test]
fn refuses_approve_and_submit_until_an_upstream_dependency_is_merged() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let first = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/flag"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "FLAG.md", "flag\n");
    commit_all(company, "add flag");
    let second = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Add flag".into(),
            from_ref: Some("main".into()),
            depends_on: vec![first.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();

    approve_patch(company, &first.id).unwrap();
    submit_patch(company, &first.id, true).unwrap();
    let err = approve_patch(company, &second.id).unwrap_err();
    assert!(err.to_string().contains("not merged upstream"), "{err}");
    let err = submit_patch(company, &second.id, true).unwrap_err();
    assert!(err.to_string().contains("not merged upstream"), "{err}");

    mark_merged(company, &first.id, MergeVia::Manual, None).unwrap();
    approve_patch(company, &second.id).unwrap();
    let submitted = submit_patch(company, &second.id, true).unwrap();
    let parent = git_ok(company, &["rev-parse", &format!("{}^", submitted.branch)]).unwrap();
    assert_eq!(parent.trim(), rev_of(company, "uplink/upstream"));
}

#[test]
fn submit_pushes_to_contrib_only_with_push() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(company, &patch.id).unwrap();
    let contrib = PathBuf::from(git_ok(company, &["remote", "get-url", "contrib"]).unwrap());
    let branch = format!("uplink/{}", patch.id);

    let local = submit_patch(company, &patch.id, false).unwrap();
    assert!(!local.pushed);
    assert!(!has_git_ref(&contrib, &branch), "contrib must be untouched");
    assert_eq!(local.base, rev_of(company, "uplink/upstream"));
    assert_eq!(rev_of(company, &format!("{}^", local.sha)), local.base);
    assert_eq!(
        local.tree,
        rev_of(company, &format!("{}^{{tree}}", local.sha))
    );
    assert!(
        local
            .message
            .contains(&format!("Uplink-Patch-Id: {}", patch.id)),
        "{}",
        local.message
    );

    let pushed = submit_patch(company, &patch.id, true).unwrap();
    assert!(pushed.pushed);
    assert_eq!(rev_of(&contrib, &branch), pushed.sha);

    git(
        company,
        &["remote", "remove", "contrib"],
        GitOpts::default(),
    )
    .unwrap();
    let err = submit_patch(company, &patch.id, true).unwrap_err();
    assert!(err.to_string().contains("--push needs"), "{err}");
}

#[test]
fn submit_cli_prints_the_contrib_commit_for_the_forge() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(company, &patch.id).unwrap();
    let contrib = PathBuf::from(git_ok(company, &["remote", "get-url", "contrib"]).unwrap());
    let branch = format!("uplink/{}", patch.id);
    let submit = |extra: &[&str]| -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
            .args(["submit", &patch.id])
            .args(extra)
            .current_dir(company)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let value = submit(&[]);
    assert_eq!(value["pushed"], false);
    assert!(!has_git_ref(&contrib, &branch), "contrib must be untouched");
    let commit = &value["contribCommit"];
    assert_eq!(commit["branch"], branch.as_str());
    let local = commit["localSha"].as_str().unwrap();
    assert_eq!(value["sha"], local);
    assert_eq!(
        commit["baseSha"],
        rev_of(company, "uplink/upstream").as_str()
    );
    assert_eq!(
        commit["treeSha"],
        rev_of(company, &format!("{local}^{{tree}}")).as_str()
    );
    let message =
        fs::read_to_string(company.join(commit["messageFile"].as_str().unwrap())).unwrap();
    assert!(message.starts_with("Use SHA-256 for tokens\n"), "{message}");
    assert!(
        message.contains(&format!("Uplink-Patch-Id: {}", patch.id)),
        "{message}"
    );

    let value = submit(&["--push"]);
    assert_eq!(value["pushed"], true);
    assert!(value.get("contribCommit").is_none(), "{value}");
    assert_eq!(rev_of(&contrib, &branch), value["sha"].as_str().unwrap());
}

#[test]
fn reads_a_queue_with_legacy_export_author_fields() {
    let world = setup_world();
    let company = &world.company;
    let path = company.join(".uplink/queue.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    value["config"]["exportAuthorName"] = "Uplink Contributor".into();
    value["config"]["exportAuthorEmail"] = "uplink@users.noreply.github.com".into();
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    let queue = git_uplink::read_queue(company).unwrap();
    let written = serde_json::to_string(&queue).unwrap();
    assert!(!written.contains("exportAuthor"), "{written}");
}

#[test]
fn does_not_squash_git_commit_messages_on_import() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "seed-internal\n");
    commit_all(company, "seed internal");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Seed internal".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "WIP first");
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);\n"),
    );
    commit_all(company, "WIP second");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let company_log = git_ok(company, &["log", "--format=%B", "main"]).unwrap();
    assert!(company_log.contains("Use SHA-256 for tokens"));
    assert!(!company_log.contains("WIP second"));
    assert_eq!(patch.commit_message, "Use SHA-256 for tokens");
}

#[test]
fn strips_html_comments_from_the_stored_message() {
    let raw = "Subject\n\n<!-- keep this out -->\n\nBody\n\n<!--\nmultiline\n-->\n";
    assert_eq!(strip_html_comments(raw), "Subject\n\nBody");
    assert_eq!(
        strip_html_comments("# Heading\n\n<!-- x -->\nbody"),
        "# Heading\n\nbody"
    );
}

#[test]
fn parses_uplink_depends_on_from_the_commit_message() {
    let a = "upl_aaaaaaaaaa";
    let b = "upl_bbbbbbbbbb";
    let mentioned = "upl_cccccccccc";
    let message = format!(
        "Subject\n\n\
See also {mentioned} in a paragraph.\n\n\
<!--\nUplink-Depends-On: upl_deadbeef00\nUplink-Depends-On: {mentioned}\n-->\n\n\
{DEFAULT_CUTOFF}\n\n\
Uplink-Depends-On: {a}\n\
Uplink-Depends-On: {b}\n\
Uplink-Depends-On: upl_…\n\
Uplink-Depends-On: upl_asha\n"
    );
    assert_eq!(parse_depends_on(&message), vec![a, b]);
    assert_eq!(
        parse_depends_on(&format!("Uplink-Depends-On: {a}, {b}")),
        vec![a, b]
    );
    assert!(parse_depends_on("depends on upl_aaaaaaaaaa").is_empty());
}

#[test]
fn refuses_import_when_the_export_diff_names_the_company() {
    let world = setup_world();
    let company = &world.company;
    set_settings(company, |s| s.redact_keywords = vec!["AcmeCorp".into()]);

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    write(
        company,
        "src/tokens.test.js",
        "test(\"AcmeCorp hasher\", () => {});",
    );
    commit_all(company, "use sha256");

    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Assess(_)));
    assert_eq!(
        status_snapshot(company)
            .unwrap()
            .queue
            .all_patches()
            .count(),
        0
    );
}

#[test]
fn formats_a_contribution_packet_and_keeps_reports_across_rebuild() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            message: Some(format!(
                "Use SHA-256 for tokens\n\nSHA-1 is weak.\n\n{DEFAULT_CUTOFF}\n\nTicket: PROJ-9\n"
            )),
            from_ref: Some("main".into()),
            internal_pr_number: Some(44),
            ..Default::default()
        },
    )
    .unwrap();

    let packet = format_approver_packet(&patch);
    assert!(
        packet.starts_with(&format!(
            "# Contribution packet — {}\n\n| Field | Value |",
            patch.id
        )),
        "{packet}"
    );
    assert!(packet.contains("#44"));
    assert!(!packet.contains("Review this packet"), "{packet}");
    assert!(!packet.contains("| Queue"), "{packet}");
    assert!(packet.contains("## Upstream commit message"));
    assert!(packet.contains("SHA-1 is weak."));
    assert!(packet.contains(&format!("Uplink-Patch-Id: {}", patch.id)));
    assert!(
        !packet.contains("PROJ-9"),
        "the company commit message must not be in the packet\n{packet}"
    );
    assert!(packet.contains("## Upstream Assessment: ✅"), "{packet}");
    assert!(!packet.contains("### Public title"), "{packet}");
    assert!(
        packet.contains("| Check | Description | Result |"),
        "{packet}"
    );

    let (_, prepare_path, approval_path) = report_paths(&patch.id).unwrap();
    write(company, &prepare_path, &packet);
    write(
        company,
        &approval_path,
        &format_approval_receipt(ApprovalReceipt {
            patch_id: &patch.id,
            environment: "to-upstream",
            actor: "dispatcher",
            run_url: "https://github.example/acme/product/actions/runs/9",
            sha: "abc123",
            reviewed: None,
            at: Some("2026-09-14T00:00:00.000Z".into()),
        }),
    );
    git_uplink::commit_queue(
        company,
        &format!("uplink: contribution packet {}", patch.id),
    )
    .unwrap();

    rebuild(company).unwrap();
    let kept = fs::read_to_string(company.join(&prepare_path)).unwrap();
    let receipt = fs::read_to_string(company.join(&approval_path)).unwrap();
    assert!(kept.contains(&patch.id));
    assert!(receipt.contains("authoritative approval event"));
    assert!(receipt.contains("`to-upstream`"));
    assert!(receipt.contains("dispatcher"));
    let tracked = git_ok(company, &["show", &format!("uplink/state:{prepare_path}")]).unwrap();
    assert!(tracked.contains("Use SHA-256 for tokens"));
    let on_main = git(
        company,
        &["cat-file", "-e", "main:.uplink"],
        GitOpts::allow_fail(),
    )
    .unwrap();
    assert_ne!(on_main.code, 0, ".uplink must not live on main");
}

#[test]
fn report_extra_dir_prepends_markdown_and_rejects_dotdot_names() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    let extras = temp_dir();
    write(
        extras.path(),
        "10-legal.md",
        "## Legal\n\nCleared for export.\n",
    );
    write(
        extras.path(),
        "20-license.md",
        "## License\n\nScan is clean.\n",
    );
    write(extras.path(), ".hidden.md", "must not appear");
    write(extras.path(), "notes.txt", "ignored");

    let packet =
        format_contribution_packet_with_extras(company, &patch, Some(extras.path())).unwrap();
    assert!(
        packet.starts_with("## Legal"),
        "extras must lead the packet:\n{packet}"
    );
    let legal = packet.find("## Legal").unwrap();
    let license = packet.find("## License").unwrap();
    let heading = packet.find("# Contribution packet").unwrap();
    assert!(legal < license);
    assert!(license < heading);
    assert!(!packet.contains("must not appear"));
    assert!(!packet.contains("ignored"));

    let without = format_contribution_packet(company, &patch).unwrap();
    assert!(without.starts_with("# Contribution packet"));

    let (_, assessment_path, _) = report_paths(&patch.id).unwrap();
    assert!(assessment_path.ends_with("assessment.md"));

    let bad = temp_dir();
    write(bad.path(), "foo..md", "nope");
    let err = load_extra_markdown(bad.path()).unwrap_err();
    assert!(err.to_string().contains("invalid path component"), "{err}");
}

#[test]
fn stored_extras_lead_the_packet_until_the_patch_changes() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let extras = temp_dir();
    write(
        extras.path(),
        "10-company.md",
        "## Company\n\nFrom the PR.\n",
    );
    let run = "https://github.example/acme/product/actions/runs/7";
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            extra_dir: Some(extras.path().to_path_buf()),
            extra_source: Some(run.into()),
            ..Default::default()
        },
    )
    .unwrap();

    let stored = patch.extras.as_ref().expect("extras recorded");
    assert_eq!(stored.source.as_deref(), Some(run));
    assert_eq!(
        Some(&stored.patch_id_stable),
        patch.patch_id_stable.as_ref()
    );
    assert!(stored_extras_fresh(&patch));
    let dir = extras_dir(&patch.id).unwrap();
    let tracked = git_ok(
        company,
        &["show", &format!("{STATE_BRANCH}:{dir}/10-company.md")],
    )
    .unwrap();
    assert!(tracked.contains("From the PR."));

    let packet = format_contribution_packet(company, &patch).unwrap();
    assert!(packet.starts_with("## Company"), "{packet}");

    let mut changed = patch.clone();
    changed.patch_id_stable = Some("0000000000000000000000000000000000000000".into());
    assert!(!stored_extras_fresh(&changed));
    let packet = format_contribution_packet(company, &changed).unwrap();
    assert!(packet.starts_with("# Contribution packet"), "{packet}");

    let mut amended = patch.clone();
    amended.status = PatchStatus::Amended;
    assert!(!stored_extras_fresh(&amended));

    let rerun = temp_dir();
    write(rerun.path(), "20-rerun.md", "## Rerun\n");
    let refreshed = store_patch_extras(company, &patch.id, rerun.path(), None).unwrap();
    assert!(stored_extras_fresh(&refreshed));
    assert!(!company.join(&dir).join("10-company.md").exists());
    let packet = format_contribution_packet(company, &refreshed).unwrap();
    assert!(packet.starts_with("## Rerun"), "{packet}");
    assert!(!packet.contains("From the PR."));
}

#[test]
fn add_rejects_extras_for_internal_only_patches() {
    let world = setup_world();
    let company = &world.company;
    let extras = temp_dir();
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Internal".into(),
            internal_only: true,
            extra_dir: Some(extras.path().to_path_buf()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("--extra-dir"), "{err}");
}

#[test]
fn rust_sources_have_no_prepare_identifiers() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let needles = [
        "PrepareReport",
        "PrepareCheck",
        "PrepareError",
        "Commands::Prepare",
        "Error::Prepare",
        "git uplink prepare",
        "uplink-prepare",
        "prepare.md",
        "mod prepare",
    ];
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for needle in needles {
            assert!(
                !text.contains(needle),
                "{} still contains {needle}",
                path.display()
            );
        }
    }
}

#[test]
fn conflict_error_is_an_error() {
    let error = ConflictError::new("blocked", "upl_1", vec!["src/tokens.js".into()]);
    assert_eq!(error.patch_id, "upl_1");
    let _err: &dyn std::error::Error = &error;
}

#[test]
fn submit_does_not_commit_queue_until_submitted() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(company, &patch.id).unwrap();
    let exported = submit_patch(company, &patch.id, true).unwrap();
    let after_submit = status_snapshot(company).unwrap();
    assert_eq!(
        after_submit.queue.patch_refs()[0].status,
        PatchStatus::Approved
    );
    assert!(
        after_submit.queue.patch_refs()[0]
            .upstream
            .as_ref()
            .and_then(|u| u.pr_number)
            .is_none()
    );

    let url = "https://github.com/upstream/tokenkit/pull/7";
    let recorded = record_pull_request(company, &patch.id, 7, url, &exported.branch, None).unwrap();
    assert_eq!(recorded.status, PatchStatus::Submitted);
    assert_eq!(recorded.upstream.as_ref().unwrap().pr_number, Some(7));
    assert_eq!(
        recorded.upstream.as_ref().unwrap().pr_url.as_deref(),
        Some(url)
    );

    let again = record_pull_request(company, &patch.id, 7, url, &exported.branch, None).unwrap();
    assert_eq!(again.status, PatchStatus::Submitted);
    let submitted_events = again
        .events
        .iter()
        .filter(|e| e.kind == "submitted")
        .count();
    assert_eq!(submitted_events, 1);

    let err = record_pull_request(
        company,
        &patch.id,
        8,
        "https://github.com/upstream/tokenkit/pull/8",
        &exported.branch,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("will not retarget"), "{err}");
}

#[test]
fn submitted_push_replays_pr_fields_when_origin_state_moved() {
    let world = setup_world();
    let company = &world.company;
    let upstream = world.upstream.clone();
    let origin = publish_origin(company);
    let (_asha_keep, asha) = clone_company_from(&origin, &upstream);
    let (_ben_keep, ben) = clone_company_from(&origin, &upstream);

    git(&asha, &["checkout", "-b", "feat/hash"], GitOpts::default()).unwrap();
    write(
        &asha,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(&asha, "use sha256");
    let asha_patch = add_landed_patch(
        &asha,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(&asha, &asha_patch.id).unwrap();
    push_queue(
        &asha,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();

    reset_from_origin(&ben).unwrap();
    git(&ben, &["checkout", "-b", "feat/notes"], GitOpts::default()).unwrap();
    write(&ben, "NOTES.md", "from-ben\n");
    commit_all(&ben, "notes from ben");
    add_landed_patch(
        &ben,
        AddPatchOpts {
            title: "Notes from Ben".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    push_queue(
        &ben,
        PushOpts {
            push_remote: Some("origin".into()),
        },
    )
    .unwrap();

    let url = "https://github.com/upstream/tokenkit/pull/7";
    record_pull_request(&asha, &asha_patch.id, 7, url, "uplink/asha", Some("origin")).unwrap();

    let (_check_keep, check) = clone_company_from(&origin, &upstream);
    let queue = status_snapshot(&check).unwrap().queue;
    let recorded = queue.all_patches().find(|p| p.id == asha_patch.id).unwrap();
    assert_eq!(recorded.status, PatchStatus::Submitted);
    assert_eq!(recorded.upstream.as_ref().unwrap().pr_number, Some(7));
    assert_eq!(
        recorded.upstream.as_ref().unwrap().pr_url.as_deref(),
        Some(url)
    );
    assert!(
        queue.all_patches().any(|p| p.title == "Notes from Ben"),
        "origin must keep Ben's import that moved uplink/state"
    );
}

#[test]
fn gated_records_the_conflict_pr_on_the_patch() {
    let world = setup_world();
    let company = &world.company;
    let upstream = &world.upstream;
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    let ttl_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    write(
        upstream,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 1800;"),
    );
    commit_all(upstream, "shorten default ttl");
    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let queued = sync_apply(company);
    let conflicted = queued.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, PatchStatus::Conflict);

    let url = "https://github.com/acme/product/pull/12";
    let recorded = record_gated_pr(company, &ttl_patch.id, 12, url, None).unwrap();
    assert_eq!(recorded.conflict.as_ref().unwrap().pr_number, Some(12));
    assert_eq!(
        recorded.conflict.as_ref().unwrap().pr_url.as_deref(),
        Some(url)
    );
    let again = record_gated_pr(company, &ttl_patch.id, 12, url, None).unwrap();
    assert_eq!(again.conflict.as_ref().unwrap().pr_number, Some(12));
    let err = record_gated_pr(
        company,
        &ttl_patch.id,
        13,
        "https://github.com/acme/product/pull/13",
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("will not retarget"), "{err}");
}

#[test]
fn add_internal_only_does_not_rebuild_main() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    land_on_main(company, &head_sha);
    let main_after_land = git_ok(company, &["rev-parse", "main"]).unwrap();
    let patch = add_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main^".into()),
            head_ref: Some(head_sha),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        git_ok(company, &["rev-parse", "main"]).unwrap(),
        main_after_land
    );
    assert!(
        status_snapshot(company)
            .unwrap()
            .queue
            .is_internal(&patch.id)
    );
}

#[test]
fn rebuild_applies_earlier_internal_after_later_upstream() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-first\n");
    commit_all(company, "internal notes");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.upstream.len(), 1);
    assert_eq!(snapshot.queue.internal.len(), 1);
    assert!(
        snapshot
            .product_files
            .get("NOTES.md")
            .unwrap()
            .contains("internal-first")
    );
    assert!(
        snapshot
            .product_files
            .get("src/tokens.js")
            .unwrap()
            .contains("sha256")
    );
}

#[test]
fn upstream_add_refuses_depends_on_internal_and_internal_may_depend_on_upstream() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let internal = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let from_sha = git_ok(company, &["rev-parse", "main"]).unwrap();
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    land_on_main(company, &head_sha);
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some(from_sha.clone()),
            head_ref: Some(head_sha.clone()),
            depends_on: vec![internal.id.clone()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("cannot depend on internal-only"),
        "{err}"
    );

    let upstream = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some(from_sha),
            head_ref: Some(head_sha),
            ..Default::default()
        },
    )
    .unwrap();
    git(
        company,
        &["checkout", "-b", "feat/flag"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "FLAG.md", "stacked-on-upstream\n");
    commit_all(company, "internal flag");
    let stacked = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal flag".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            depends_on: vec![upstream.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(stacked.depends_on, vec![upstream.id]);
}

#[test]
fn rebuild_empty_apply_merges_upstream_only() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let head_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["branch", "-f", "uplink/upstream", &head_sha],
        GitOpts::default(),
    )
    .unwrap();
    git(company, &["checkout", "main"], GitOpts::default()).unwrap();
    git(
        company,
        &["merge", "--ff-only", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    let upstream = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main^".into()),
            head_ref: Some(head_sha.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(upstream.status, PatchStatus::Merged);

    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let notes_sha = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    git(
        company,
        &["branch", "-f", "uplink/upstream", &notes_sha],
        GitOpts::default(),
    )
    .unwrap();
    land_on_main(company, &notes_sha);
    let internal = add_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main^".into()),
            head_ref: Some(notes_sha),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(internal.status, PatchStatus::Queued);
    rebuild(company).unwrap();
    let after = status_snapshot(company).unwrap();
    assert_eq!(
        after
            .queue
            .all_patches()
            .find(|p| p.id == internal.id)
            .unwrap()
            .status,
        PatchStatus::Queued
    );
    assert_eq!(
        after
            .queue
            .all_patches()
            .find(|p| p.id == upstream.id)
            .unwrap()
            .status,
        PatchStatus::Merged
    );
}

#[test]
fn incoming_preflight_skips_internal_only() {
    let world = setup_world();
    preflight_incoming_change(
        &world.company,
        IncomingPreflight {
            title: "Vendor telemetry".into(),
            from_ref: "does-not-exist".into(),
            head_ref: "also-missing".into(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: true,
        },
    )
    .unwrap();
}

#[test]
fn incoming_preflight_fails_when_candidate_needs_internal() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/internal-hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return companySha(value);"),
    );
    commit_all(company, "company hasher");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Company hasher".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(company, &["checkout", "-b", "feat/log"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return companySha(value); // logged"),
    );
    commit_all(company, "log company hasher");
    let from = git_ok(company, &["rev-parse", "main"]).unwrap();
    let head = git_ok(company, &["rev-parse", "HEAD"]).unwrap();
    let err = preflight_incoming_change(
        company,
        IncomingPreflight {
            title: "Log company hasher".into(),
            from_ref: from,
            head_ref: head,
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("internal omitted") || text.contains("does not apply"),
        "{text}"
    );
}

#[test]
fn git_uplink_binary_is_named_for_git_subcommand() {
    let status = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(text.contains("\"name\":\"git-uplink\""));
}

#[test]
fn git_uplink_help_includes_web_ui() {
    let bin = env!("CARGO_BIN_EXE_git-uplink");
    let output = Command::new(bin).arg("-h").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("push"),
        "expected push subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("web-ui"),
        "expected web-ui subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("submitted"),
        "expected submitted subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("gated"),
        "expected gated subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("transfer"),
        "expected transfer subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("reset"),
        "expected reset subcommand in help, got:\n{text}"
    );
    assert!(
        text.contains("refresh"),
        "expected refresh subcommand in help, got:\n{text}"
    );
}

fn add_internal_notes(company: &Path) -> Patch {
    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap()
}

fn ref_exists(repo: &Path, name: &str) -> bool {
    git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
        GitOpts::allow_fail(),
    )
    .unwrap()
    .code
        == 0
}

#[test]
fn assess_warns_about_binary_files_in_the_export() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/logo"],
        GitOpts::default(),
    )
    .unwrap();
    fs::write(company.join("logo.bin"), b"\x00\x01AcmeCorp\x00\xff").unwrap();
    commit_all(company, "add logo");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Add logo".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let assess = patch.assess.as_ref().unwrap();
    let check = assess
        .checks
        .iter()
        .find(|c| c.id == "binary-files")
        .expect("binary-files check");
    assert_eq!(check.status, CheckStatus::Warn);
    assert!(check.detail.contains("logo.bin"), "{}", check.detail);
    assert!(assess.ok);
}

#[test]
fn assess_fails_an_export_author_at_an_internal_domain() {
    let world = setup_world();
    let company = &world.company;
    set_settings(company, |s| {
        s.internal_email_domains = vec!["acme.com".into()]
    });

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let err = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            message: Some(format!(
                "Use SHA-256 for tokens\n\nReplace SHA-1.\n\n{DEFAULT_CUTOFF}\n\n\
Uplink-Export-Author: Jane <jane@acme.com>\n"
            )),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Assess(_)), "{err}");
    assert!(err.to_string().contains("@acme.com"), "{err}");
}

#[test]
fn transfer_to_upstream_moves_immediately_when_apply_and_preflight_pass() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    let result = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(result.transferred, "{result:?}");
    assert!(!result.gated);
    let queue = git_uplink::read_queue(company).unwrap();
    assert!(queue.is_upstream(&patch.id));
    assert!(!queue.is_internal(&patch.id));
    assert!(!ref_exists(
        company,
        &format!("uplink/transfer-to-upstream/{}", patch.id)
    ));
    let moved = queue.all_patches().find(|p| p.id == patch.id).unwrap();
    let leak = moved
        .assess
        .as_ref()
        .unwrap()
        .checks
        .iter()
        .find(|c| c.id == "affiliation-leak")
        .unwrap();
    assert_ne!(
        leak.status,
        CheckStatus::Skip,
        "to-upstream must re-assess; leak was {}",
        leak.status
    );
}

#[test]
fn transfer_to_upstream_gates_on_assess_failure_without_writing_queue() {
    let world = setup_world();
    let company = &world.company;
    set_settings(company, |s| s.redact_keywords = vec!["AcmeCorp".into()]);

    git(
        company,
        &["checkout", "-b", "feat/secret"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.test.js",
        "test(\"AcmeCorp hasher\", () => {});",
    );
    commit_all(company, "vendor secret");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Vendor secret".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let imported = git_uplink::read_queue(company).unwrap();
    assert!(imported.is_internal(&patch.id));
    let leak = imported
        .all_patches()
        .find(|p| p.id == patch.id)
        .unwrap()
        .assess
        .as_ref()
        .unwrap()
        .checks
        .iter()
        .find(|c| c.id == "affiliation-leak")
        .unwrap();
    assert_eq!(leak.status, CheckStatus::Skip, "{leak:?}");

    let result = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(result.gated, "{result:?}");
    assert!(!result.transferred);
    assert!(
        result
            .message
            .as_deref()
            .unwrap_or("")
            .contains("Upstream assessment failed"),
        "{result:?}"
    );
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_internal(&patch.id));
    assert!(!after.is_upstream(&patch.id));
}

#[test]
fn transfer_to_upstream_gates_on_preflight_failure_without_writing_queue() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    set_preflight_script(company, "exit 1");

    let result = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(result.gated, "{result:?}");
    assert!(!result.transferred);
    let base = format!("uplink/transfer-to-upstream/{}", patch.id);
    let work = format!("{base}-work");
    assert_eq!(result.base_branch.as_deref(), Some(base.as_str()));
    assert_eq!(result.work_branch.as_deref(), Some(work.as_str()));
    assert!(ref_exists(company, &base));
    assert!(ref_exists(company, &work));

    git(company, &["checkout", "--quiet", &work], GitOpts::default()).unwrap();
    let notes = fs::read_to_string(company.join("NOTES.md")).unwrap();
    assert!(notes.contains("internal-notes"), "{notes}");
    assert!(!notes.contains("<<<<<<"), "{notes}");

    git(
        company,
        &["checkout", "--quiet", "main"],
        GitOpts::default(),
    )
    .unwrap();
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_internal(&patch.id));
    assert!(!after.is_upstream(&patch.id));
    assert_eq!(
        after
            .all_patches()
            .find(|p| p.id == patch.id)
            .unwrap()
            .status,
        PatchStatus::Queued
    );
}

#[test]
fn transfer_abort_deletes_branches_and_leaves_the_source_queue() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    set_preflight_script(company, "exit 1");
    let result = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(result.gated);
    let base = result.base_branch.unwrap();
    let work = result.work_branch.unwrap();
    git(company, &["branch", "-D", &base, &work], GitOpts::default()).unwrap();
    assert!(!ref_exists(company, &base));
    assert!(!ref_exists(company, &work));
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_internal(&patch.id));
    let again = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(again.gated);
    assert!(ref_exists(
        company,
        &format!("uplink/transfer-to-upstream/{}", patch.id)
    ));
}

#[test]
fn transfer_complete_applies_work_and_moves_the_patch() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    set_preflight_script(company, "grep -q ready NOTES.md");
    let result = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap();
    assert!(result.gated, "{result:?}");
    let work = result.work_branch.unwrap();
    git(company, &["checkout", "--quiet", &work], GitOpts::default()).unwrap();
    write(company, "NOTES.md", "internal-notes\nready\n");
    git(company, &["add", "NOTES.md"], GitOpts::default()).unwrap();
    git(
        company,
        &["commit", "-m", "make preflight pass"],
        GitOpts::default(),
    )
    .unwrap();
    let done = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, true).unwrap();
    assert!(done.transferred, "{done:?}");
    assert!(!done.gated);
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_upstream(&patch.id));
    let leak = after
        .all_patches()
        .find(|p| p.id == patch.id)
        .unwrap()
        .assess
        .as_ref()
        .unwrap()
        .checks
        .iter()
        .find(|c| c.id == "affiliation-leak")
        .unwrap();
    assert_ne!(leak.status, CheckStatus::Skip, "{leak:?}");
}

#[test]
fn transfer_refuses_the_wrong_source_queue() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let err = transfer_patch(company, &patch.id, TransferDirection::ToUpstream, false).unwrap_err();
    assert!(
        err.to_string().contains("not in the internal queue"),
        "{err}"
    );
}

#[test]
fn transfer_to_internal_of_submitted_clears_upstream() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id, true).unwrap();
    record_pull_request(
        company,
        &patch.id,
        44,
        "https://github.com/upstream/tokenkit/pull/44",
        &submitted.branch,
        None,
    )
    .unwrap();
    let result = transfer_patch(company, &patch.id, TransferDirection::ToInternal, false).unwrap();
    assert!(result.transferred, "{result:?}");
    assert_eq!(
        result.pr_close_url.as_deref(),
        Some("https://github.com/upstream/tokenkit/pull/44")
    );
    assert_eq!(
        result.pr_close_branch.as_deref(),
        Some(submitted.branch.as_str())
    );
    let after = git_uplink::read_queue(company).unwrap();
    let moved = after.all_patches().find(|p| p.id == patch.id).unwrap();
    assert!(after.is_internal(&patch.id));
    assert_eq!(moved.status, PatchStatus::Queued);
    assert!(moved.upstream.is_none());
    assert!(moved.approvals.is_empty());
}

#[test]
fn transfer_to_internal_refuses_while_upstream_dependents_exist() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/logs"],
        GitOpts::default(),
    )
    .unwrap();
    let current = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    write(
        company,
        "src/tokens.js",
        &current.replace(
            "return sha256(value);",
            "console.log(\"hash\");\n  return sha256(value);",
        ),
    );
    commit_all(company, "add log");
    let log_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Log token hashes".into(),
            from_ref: Some("main".into()),
            depends_on: vec![hash_patch.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();

    let err = transfer_patch(
        company,
        &hash_patch.id,
        TransferDirection::ToInternal,
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("dependents on the upstream queue"),
        "{err}"
    );
    assert!(err.to_string().contains(&log_patch.id), "{err}");
    assert!(err.to_string().contains("--to-internal first"), "{err}");

    let moved_dep =
        transfer_patch(company, &log_patch.id, TransferDirection::ToInternal, false).unwrap();
    assert!(moved_dep.transferred, "{moved_dep:?}");

    let moved = transfer_patch(
        company,
        &hash_patch.id,
        TransferDirection::ToInternal,
        false,
    )
    .unwrap();
    assert!(moved.transferred, "{moved:?}");
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_internal(&hash_patch.id));
    assert!(after.is_internal(&log_patch.id));
}

#[test]
fn transfer_to_upstream_allows_internal_dependents() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let hash_patch = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();

    git(
        company,
        &["checkout", "-b", "feat/notes"],
        GitOpts::default(),
    )
    .unwrap();
    write(company, "NOTES.md", "internal-notes\n");
    commit_all(company, "internal notes");
    let notes = add_landed_patch(
        company,
        AddPatchOpts {
            title: "Internal notes".into(),
            internal_only: true,
            from_ref: Some("main".into()),
            depends_on: vec![hash_patch.id.clone()],
            ..Default::default()
        },
    )
    .unwrap();

    let result = transfer_patch(
        company,
        &hash_patch.id,
        TransferDirection::ToUpstream,
        false,
    )
    .unwrap();
    assert!(result.transferred, "{result:?}");
    let after = git_uplink::read_queue(company).unwrap();
    assert!(after.is_upstream(&hash_patch.id));
    assert!(after.is_internal(&notes.id));
}

fn has_local_ref(repo: &Path, git_ref: &str) -> bool {
    git(
        repo,
        &["rev-parse", "--verify", "--quiet", git_ref],
        GitOpts::allow_fail(),
    )
    .map(|result| result.code == 0)
    .unwrap_or(false)
}

#[test]
fn init_resumes_after_partial_upstream_seed_failure() {
    let world = setup_uninitialized();
    let contrib = remote_get_url(&world.company, "contrib");
    let partial = init(
        &world.company,
        InitOpts {
            upstream_url: Some("https://example.invalid/repo.git".into()),
            contrib_url: Some(contrib.clone()),
            forge: Some(Forge::Github),
            ..Default::default()
        },
    );
    assert!(partial.is_err(), "expected upstream fetch to fail");
    assert!(has_local_ref(&world.company, STATE_BRANCH));
    assert!(!has_local_ref(&world.company, "uplink/upstream"));

    let result = init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(contrib),
            forge: Some(Forge::Github),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.report.ok, "{:?}", result.report.checks);
    assert!(has_local_ref(&world.company, "uplink/upstream"));
    assert!(result.queue.tooling.is_some());
}

#[test]
fn doctor_passes_on_initialized_local_world() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let report = doctor(&world.company, ProgressMode::Disabled).unwrap();
    assert!(report.ok, "{:?}", report.checks);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id == "initialized" && check.status == CheckStatus::Pass)
    );
}

#[test]
fn doctor_fails_when_queue_is_missing() {
    let world = setup_uninitialized();
    let report = doctor(&world.company, ProgressMode::Disabled).unwrap();
    assert!(!report.ok);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id == "initialized" && check.status == CheckStatus::Fail)
    );
}

fn hooks_check(repo: &Path) -> git_uplink::DoctorReport {
    doctor(repo, ProgressMode::Disabled).unwrap()
}

fn hooks_status(report: &git_uplink::DoctorReport) -> (CheckStatus, String) {
    let check = report
        .checks
        .iter()
        .find(|check| check.id == "hooks-branch")
        .expect("hooks-branch check");
    (check.status, check.detail.clone())
}

fn hooks_paths(repo: &Path) -> Vec<String> {
    git_ok(repo, &["ls-tree", "-r", "--name-only", "uplink/hooks"])
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn init_creates_the_hooks_branch_locally_as_an_orphan() {
    let world = setup_uninitialized();
    let origin = add_origin(&world.company);
    init_with_recorded_urls(&world);
    let company = &world.company;
    assert!(has_git_ref(company, "refs/heads/uplink/hooks"));
    assert!(!has_git_ref(company, "uplink/hooks^"), "must be an orphan");
    assert_eq!(
        hooks_paths(company),
        [
            ".github/actions/uplink-toolchain-hook/action.yml",
            ".github/workflows/uplink-assessment-hook-example.yml",
            "assessment-hook.md",
            "preflight.sh",
            "toolchain-hook.md",
            "uplink.toml",
        ]
    );
    assert!(!has_git_object(company, "main:uplink.toml"));
    assert!(!has_git_object(company, "main:toolchain-hook.md"));
    assert!(!has_git_object(
        company,
        "main:.github/uplink-assessment-hook.md"
    ));
    assert!(has_git_object(
        company,
        "main:.github/actions/uplink-toolchain-hook/action.yml"
    ));
    assert!(!has_git_ref(&origin, "refs/heads/uplink/hooks"));
}

/// Commit `edit` on `uplink/hooks` through a throwaway worktree.
fn edit_hooks(company: &Path, message: &str, edit: impl FnOnce(&Path)) -> String {
    let dir = keep_dir().join("hooks");
    git(
        company,
        &[
            "worktree",
            "add",
            "--quiet",
            dir.to_str().unwrap(),
            "uplink/hooks",
        ],
        GitOpts::default(),
    )
    .unwrap();
    edit(&dir);
    commit_all(&dir, message);
    git(
        company,
        &["worktree", "remove", "--force", dir.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    rev_of(company, "uplink/hooks")
}

#[test]
fn init_without_upgrade_never_changes_an_existing_hooks_branch() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let edited = edit_hooks(company, "drop the guide", |dir| {
        fs::remove_file(dir.join("toolchain-hook.md")).unwrap();
    });
    init(
        company,
        InitOpts {
            forge: Some(Forge::Github),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rev_of(company, "uplink/hooks"), edited);
}

#[test]
fn init_upgrade_adds_missing_hook_files_without_overwriting() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let edited = edit_hooks(company, "company edits", |dir| {
        fs::remove_file(dir.join("toolchain-hook.md")).unwrap();
        fs::remove_file(dir.join(".github/actions/uplink-toolchain-hook/action.yml")).unwrap();
        write(dir, "assessment-hook.md", "# Our own notes\n");
    });
    let result = init(
        company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rev_of(company, "uplink/hooks^"), edited);
    assert!(has_git_object(company, "uplink/hooks:toolchain-hook.md"));
    assert!(has_git_object(
        company,
        "uplink/hooks:.github/actions/uplink-toolchain-hook/action.yml"
    ));
    assert_eq!(
        git_ok(company, &["show", "uplink/hooks:assessment-hook.md"]).unwrap(),
        "# Our own notes"
    );
    let step = result
        .report
        .checks
        .iter()
        .find(|c| c.id == "hooks-branch")
        .unwrap();
    assert!(step.detail.contains("toolchain-hook.md"), "{}", step.detail);
    assert!(step.detail.contains("git uplink push"), "{}", step.detail);

    let again = rev_of(company, "uplink/hooks");
    init(
        company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        rev_of(company, "uplink/hooks"),
        again,
        "nothing left to add"
    );
}

#[test]
fn uplink_push_publishes_the_hooks_branch() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let origin = publish_origin(company);
    let result = push_queue(company, PushOpts::default()).unwrap();
    assert_eq!(result.hooks, HooksPushAction::Pushed);
    assert_eq!(
        rev_of(&origin, "refs/heads/uplink/hooks"),
        rev_of(company, "uplink/hooks")
    );
    let again = push_queue(company, PushOpts::default()).unwrap();
    assert_eq!(again.hooks, HooksPushAction::UpToDate);

    let ahead = edit_hooks(company, "tune the toolchain", |dir| {
        write(dir, "toolchain-hook.md", "# Tuned\n");
    });
    let pushed = push_queue(company, PushOpts::default()).unwrap();
    assert_eq!(pushed.hooks, HooksPushAction::Pushed);
    assert_eq!(rev_of(&origin, "refs/heads/uplink/hooks"), ahead);
}

#[test]
fn uplink_push_never_force_pushes_the_hooks_branch() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let origin = publish_origin(company);
    push_queue(company, PushOpts::default()).unwrap();
    let published = rev_of(&origin, "refs/heads/uplink/hooks");
    let root = rev_of(company, "uplink/hooks");
    let empty_tree = git_ok(company, &["hash-object", "-t", "tree", "/dev/null"]).unwrap();
    let other = git_ok(
        company,
        &["commit-tree", &empty_tree, "-p", &root, "-m", "local"],
    )
    .unwrap();
    git(
        company,
        &[
            "push",
            "--quiet",
            "origin",
            &format!("{other}:refs/heads/uplink/hooks"),
        ],
        GitOpts::default(),
    )
    .unwrap();
    let local = edit_hooks(company, "diverge", |dir| {
        write(dir, "toolchain-hook.md", "# Mine\n");
    });
    let result = push_queue(company, PushOpts::default()).unwrap();
    assert_eq!(result.hooks, HooksPushAction::Diverged);
    assert_eq!(rev_of(&origin, "refs/heads/uplink/hooks"), other);
    assert_eq!(rev_of(company, "uplink/hooks"), local);
    assert_ne!(published, other);
}

#[test]
fn init_upgrade_creates_a_missing_hooks_branch() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    git(
        company,
        &["branch", "-D", "uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    let result = init(
        company,
        InitOpts {
            upgrade: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(has_git_ref(company, "refs/heads/uplink/hooks"));
    assert!(
        result
            .report
            .checks
            .iter()
            .any(|c| c.id == "hooks-branch" && c.detail.contains("git uplink push")),
        "{:?}",
        result.report.checks
    );
}

#[test]
fn init_without_args_fetches_the_hooks_branch_from_origin() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let origin = publish_origin(&world.company);
    git(
        &world.company,
        &["push", "--quiet", "origin", "uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    let clone_parent = keep_dir();
    git(
        &clone_parent,
        &["clone", "--quiet", origin.to_str().unwrap(), "product"],
        GitOpts::default(),
    )
    .unwrap();
    let clone = clone_parent.join("product");
    init(&clone, InitOpts::default()).unwrap();
    assert_eq!(
        rev_of(&clone, "refs/heads/uplink/hooks"),
        rev_of(&world.company, "uplink/hooks")
    );
}

#[test]
fn doctor_tells_you_to_push_the_hooks_branch() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    publish_origin(company);

    let (status, detail) = hooks_status(&hooks_check(company));
    assert_eq!(status, CheckStatus::Fail, "{detail}");
    assert!(detail.contains("git uplink push"), "{detail}");

    git(
        company,
        &["push", "--quiet", "origin", "uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    let (status, detail) = hooks_status(&hooks_check(company));
    assert_eq!(status, CheckStatus::Pass, "{detail}");

    let tree = git_ok(company, &["rev-parse", "uplink/hooks^{tree}"]).unwrap();
    let ahead = git_ok(
        company,
        &["commit-tree", &tree, "-p", "uplink/hooks", "-m", "tweak"],
    )
    .unwrap();
    git(
        company,
        &["update-ref", "refs/heads/uplink/hooks", &ahead],
        GitOpts::default(),
    )
    .unwrap();
    let (status, detail) = hooks_status(&hooks_check(company));
    assert_eq!(status, CheckStatus::Warn, "{detail}");
    assert!(detail.contains("git uplink push"), "{detail}");
}

#[test]
fn doctor_fails_when_the_hooks_branch_lacks_the_toolchain_hook() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let empty_tree = git_ok(company, &["hash-object", "-t", "tree", "/dev/null"]).unwrap();
    let emptied = git_ok(
        company,
        &[
            "commit-tree",
            &empty_tree,
            "-p",
            "uplink/hooks",
            "-m",
            "drop",
        ],
    )
    .unwrap();
    git(
        company,
        &["update-ref", "refs/heads/uplink/hooks", &emptied],
        GitOpts::default(),
    )
    .unwrap();
    let (status, detail) = hooks_status(&hooks_check(company));
    assert_eq!(status, CheckStatus::Fail, "{detail}");
    assert!(
        detail.contains(git_uplink::TOOLCHAIN_ACTION_PATH),
        "{detail}"
    );
}

#[test]
fn doctor_fails_when_the_hooks_branch_is_missing() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    git(
        &world.company,
        &["branch", "-D", "uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    let report = hooks_check(&world.company);
    assert!(!report.ok);
    let (status, detail) = hooks_status(&report);
    assert_eq!(status, CheckStatus::Fail, "{detail}");
    assert!(detail.contains("git uplink init --upgrade"), "{detail}");
}

fn hooks_file(repo: &Path, path: &str) -> String {
    git_ok(repo, &["show", &format!("uplink/hooks:{path}")]).unwrap()
}

fn hooks_toml(repo: &Path) -> String {
    git_ok(repo, &["show", "uplink/hooks:uplink.toml"]).unwrap()
}

fn init_opts_with_settings(world: &World, settings: SettingsFlags) -> InitOpts {
    InitOpts {
        upstream_url: Some(world.upstream.to_str().unwrap().into()),
        contrib_url: Some(remote_get_url(&world.company, "contrib")),
        forge: Some(Forge::Github),
        settings,
        ..Default::default()
    }
}

fn hooks_step_detail(result: &git_uplink::InitResult) -> String {
    result
        .report
        .checks
        .iter()
        .find(|c| c.id == "hooks-branch")
        .expect("hooks-branch step")
        .detail
        .clone()
}

#[test]
fn init_writes_the_answers_to_the_hooks_branch() {
    let world = setup_uninitialized();
    let result = init(
        &world.company,
        init_opts_with_settings(
            &world,
            SettingsFlags {
                preflight: Some("npm ci && npm test".into()),
                redact_keywords: Some(vec!["AcmeCorp,companyTelemetry".into()]),
                internal_domains: Some(vec!["acme.example".into()]),
            },
        ),
    )
    .unwrap();
    let script = hooks_file(&world.company, "preflight.sh");
    assert!(script.starts_with("#!/bin/sh\n"), "{script}");
    assert!(script.ends_with("\nnpm ci && npm test"), "{script}");
    let text = hooks_toml(&world.company);
    assert!(!text.contains("preflight"), "{text}");
    assert!(
        text.contains("redact_keywords = [\"AcmeCorp\", \"companyTelemetry\"]"),
        "{text}"
    );
    assert!(
        text.contains("internal_email_domains = [\"acme.example\"]"),
        "{text}"
    );
    assert!(!hooks_step_detail(&result).contains("Left empty"));

    let settings = git_uplink::read_queue(&world.company).unwrap().settings;
    assert_eq!(settings.redact_keywords, ["AcmeCorp", "companyTelemetry"]);
    assert_eq!(settings.internal_email_domains, ["acme.example"]);
    assert!(settings.problem.is_none());

    // The settings live on uplink/hooks only, never in queue.json.
    let stored = fs::read_to_string(world.company.join(".uplink/queue.json")).unwrap();
    for key in ["preflightCommand", "redactKeywords", "internalEmailDomains"] {
        assert!(!stored.contains(key), "{stored}");
    }
}

#[test]
fn init_without_answers_writes_empty_settings_and_says_so() {
    let world = setup_uninitialized();
    let result = init(
        &world.company,
        init_opts_with_settings(&world, SettingsFlags::default()),
    )
    .unwrap();
    let text = hooks_toml(&world.company);
    assert!(text.contains("redact_keywords = []"), "{text}");
    // The stub has the contract and no command.
    let script = hooks_file(&world.company, "preflight.sh");
    assert!(script.trim_end().ends_with("\nset -eu"), "{script}");
    let detail = hooks_step_detail(&result);
    assert!(
        detail.contains("Left empty in uplink.toml: redact_keywords, internal_email_domains"),
        "{detail}"
    );
    assert!(result.report.ok, "{:?}", result.report.checks);
}

#[test]
fn init_upgrade_adds_a_missing_uplink_toml_from_flags_and_the_old_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    edit_hooks(company, "drop settings", |dir| {
        fs::remove_file(dir.join("uplink.toml")).unwrap();
    });
    // An older binary kept these in queue.json.
    let path = company.join(".uplink/queue.json");
    let stored = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        stored.replacen(
            "\"config\": {",
            "\"config\": {\n    \"redactKeywords\": [\"LegacyCo\"],",
            1,
        ),
    )
    .unwrap();
    git_uplink::commit_queue(company, "uplink: old settings").unwrap();
    assert!(
        git_uplink::read_queue(company)
            .unwrap()
            .settings
            .problem
            .is_some()
    );

    let result = init(
        company,
        InitOpts {
            upgrade: true,
            settings: SettingsFlags {
                preflight: Some("make check".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let text = hooks_toml(company);
    assert!(text.contains("redact_keywords = [\"LegacyCo\"]"), "{text}");
    // preflight.sh was already there, so the flag seeds nothing.
    assert!(!hooks_file(company, "preflight.sh").contains("make check"));
    let detail = hooks_step_detail(&result);
    assert!(detail.contains("uplink.toml"), "{detail}");
    assert!(
        detail.contains("Left empty in uplink.toml: internal_email_domains"),
        "{detail}"
    );
    assert_eq!(result.queue.settings.redact_keywords, ["LegacyCo"]);
}

#[test]
fn init_upgrade_appends_missing_settings_and_keeps_the_rest() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let ours = "# ours\nredact_keywords = [\"Mine\"]\n";
    edit_hooks(company, "partial settings", |dir| {
        write(dir, "uplink.toml", ours);
    });
    let upgrade = |settings: SettingsFlags| {
        init(
            company,
            InitOpts {
                upgrade: true,
                settings,
                ..Default::default()
            },
        )
        .unwrap()
    };
    upgrade(SettingsFlags {
        preflight: None,
        redact_keywords: Some(vec!["Ignored".into()]),
        internal_domains: None,
    });
    let text = format!("{}\n", hooks_toml(company));
    assert!(text.starts_with(ours), "{text}");
    assert!(
        !text.contains("Ignored"),
        "existing values are never changed\n{text}"
    );
    assert!(text.contains("internal_email_domains = []"), "{text}");

    let tip = rev_of(company, "uplink/hooks");
    upgrade(SettingsFlags {
        preflight: Some("something else".into()),
        ..Default::default()
    });
    assert_eq!(rev_of(company, "uplink/hooks"), tip, "nothing left to add");
}

#[test]
fn init_upgrade_adds_a_missing_preflight_script_and_never_rewrites_it() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    edit_hooks(company, "drop the script", |dir| {
        fs::remove_file(dir.join("preflight.sh")).unwrap();
    });
    // An older binary kept the command in queue.json.
    let path = company.join(".uplink/queue.json");
    let stored = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        stored.replacen(
            "\"config\": {",
            "\"config\": {\n    \"preflightCommand\": \"make legacy\",",
            1,
        ),
    )
    .unwrap();
    git_uplink::commit_queue(company, "uplink: old command").unwrap();

    let upgrade = |preflight: Option<&str>| {
        init(
            company,
            InitOpts {
                upgrade: true,
                settings: SettingsFlags {
                    preflight: preflight.map(str::to_string),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
    };
    let result = upgrade(None);
    assert!(
        hooks_step_detail(&result).contains("preflight.sh"),
        "{}",
        hooks_step_detail(&result)
    );
    let script = hooks_file(company, "preflight.sh");
    assert!(script.ends_with("\nmake legacy"), "{script}");

    let tip = rev_of(company, "uplink/hooks");
    upgrade(Some("make other"));
    assert_eq!(rev_of(company, "uplink/hooks"), tip, "the script is kept");
}

#[test]
fn a_forge_queue_without_uplink_toml_fails_closed() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    publish_origin(company);
    edit_hooks(company, "drop settings", |dir| {
        fs::remove_file(dir.join("uplink.toml")).unwrap();
    });

    let (status, detail) = hooks_status(&hooks_check(company));
    assert_eq!(status, CheckStatus::Fail, "{detail}");
    assert!(detail.contains("uplink.toml"), "{detail}");
    assert!(detail.contains("git uplink init --upgrade"), "{detail}");

    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, Error::Assess(_)), "{text}");
    assert!(text.contains("uplink.toml is missing"), "{text}");
}

#[test]
fn preflight_command_only_runs_preflight_sh_from_uplink_hooks() {
    let world = setup_world();
    let company = &world.company;
    let run = |old_variable: &str| {
        Command::new(env!("CARGO_BIN_EXE_git-uplink"))
            .args(["preflight", "--command-only"])
            .env("UPLINK_PREFLIGHT", old_variable)
            .current_dir(company)
            .output()
            .unwrap()
    };
    // No preflight.sh: nothing to run, and the old variable is not read.
    let output = run("exit 9");
    assert!(output.status.success(), "{output:?}");

    set_preflight_script(company, "test -f src/tokens.js");
    let output = run("exit 9");
    assert!(output.status.success(), "{output:?}");

    set_preflight_script(company, "exit 3");
    let output = run("true");
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        format!("{stdout}{stderr}").contains("exit 3"),
        "{stdout}\n{stderr}"
    );
}

fn run_command_only(dir: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args(["preflight", "--command-only"])
        .args(extra)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn preflight_sh_starts_in_the_checkout_root_from_a_subdirectory() {
    let world = setup_world();
    let company = &world.company;
    // Passes only in the directory that holds src/, and prints where it ran.
    set_preflight_script(company, "pwd\ntest -f src/tokens.js && exit 4");
    let output = run_command_only(&company.join("src"), &[]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("exit 4"), "{text}");
    let root = git_ok(company, &["rev-parse", "--show-toplevel"]).unwrap();
    assert!(text.lines().any(|line| line == root), "{text}");
}

#[test]
fn preflight_sh_runs_in_the_export_tree_and_reaches_its_siblings() {
    let world = setup_world();
    let company = &world.company;
    commit_hooks_file(company, "uplink/hooks", "lib/check.sh", "pwd\nexit 6\n");
    set_preflight_script(company, r#"sh "$(dirname "$0")/lib/check.sh""#);
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");

    let err = preflight_incoming_change(
        company,
        IncomingPreflight {
            title: "Use SHA-256 for tokens".into(),
            from_ref: "main".into(),
            head_ref: "HEAD".into(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap_err();
    let Error::Preflight(pre) = err else {
        panic!("expected preflight, got {err}");
    };
    assert_eq!(pre.stage, "command");
    let message = pre.to_string();
    assert!(message.contains("preflight.sh, exit 6"), "{message}");
    // The sibling printed the working directory: the export tree, not the
    // checkout and not the hooks checkout.
    assert!(message.contains("uplink-export-"), "{message}");
    let worktrees = git_ok(company, &["worktree", "list", "--porcelain"]).unwrap();
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
}

#[test]
fn preflight_hooks_flag_reads_the_script_from_another_revision() {
    let world = setup_world();
    let company = &world.company;
    set_preflight_script(company, "exit 0");
    git(
        company,
        &["branch", "hooks-change", "uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    commit_hooks_file(company, "hooks-change", "preflight.sh", "exit 7\n");

    let output = run_command_only(company, &[]);
    assert!(output.status.success(), "{output:?}");
    let output = run_command_only(company, &["--hooks", "hooks-change"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("exit 7"),
        "{output:?}"
    );
    let output = run_command_only(company, &["--hooks", "no-such-branch"]);
    assert!(!output.status.success(), "{output:?}");
}

#[test]
fn preflight_without_a_hooks_branch_fails_closed_for_a_forge_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let company = &world.company;
    let output = run_command_only(company, &[]);
    assert!(output.status.success(), "the stub passes\n{output:?}");

    git(
        company,
        &["update-ref", "-d", "refs/heads/uplink/hooks"],
        GitOpts::default(),
    )
    .unwrap();
    let output = run_command_only(company, &[]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("uplink/hooks is missing"),
        "{output:?}"
    );
}

#[test]
fn init_cli_prints_how_to_publish_the_hooks_branch() {
    let world = setup_uninitialized();
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args([
            "init",
            "--upstream",
            world.upstream.to_str().unwrap(),
            "--contrib",
            &remote_get_url(&world.company, "contrib"),
            "--forge",
            "github",
        ])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Publish hooks: git uplink push"),
        "{stdout}"
    );
}

#[test]
fn init_prints_summary_not_full_queue_json() {
    let world = setup_uninitialized();
    let output = Command::new(env!("CARGO_BIN_EXE_git-uplink"))
        .args([
            "init",
            "--upstream",
            world.upstream.to_str().unwrap(),
            "--contrib",
            &remote_get_url(&world.company, "contrib"),
            "--forge",
            "github",
        ])
        .current_dir(&world.company)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Uplink init: ready"), "{stdout}");
    assert!(!stdout.contains("\"version\""), "{stdout}");
}

#[test]
fn format_step_line_plain_uses_tags() {
    let line = format_step_line(
        false,
        "Seed uplink/upstream",
        &StepOutcome::pass("fetched main"),
    );
    assert!(line.starts_with("[ok  ]"));
}

#[test]
fn preflight_keeps_uncommitted_work_and_rebuild_refuses_it() {
    let world = setup_world();
    let company = &world.company;
    git(
        company,
        &["checkout", "-b", "feat/hash"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return sha1(value);", "return sha256(value);"),
    );
    commit_all(company, "use sha256");
    write(company, "README.md", "MY UNSAVED WORK\n");

    preflight_incoming_change(
        company,
        IncomingPreflight {
            title: "Use SHA-256 for tokens".into(),
            from_ref: "main".into(),
            head_ref: "HEAD".into(),
            depends_on: Vec::new(),
            message: None,
            hooks_ref: None,
            internal_only: false,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(company.join("README.md")).unwrap(),
        "MY UNSAVED WORK\n"
    );

    let err = rebuild(company).unwrap_err().to_string();
    assert!(err.contains("Uncommitted changes"), "{err}");
    assert_eq!(
        fs::read_to_string(company.join("README.md")).unwrap(),
        "MY UNSAVED WORK\n"
    );
    assert_eq!(
        git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "feat/hash"
    );
}

fn add_ttl_patch(company: &Path) -> Patch {
    git(company, &["checkout", "-b", "feat/ttl"], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 7200;"),
    );
    commit_all(company, "longer ttl");
    add_landed_patch(
        company,
        AddPatchOpts {
            title: "Extend TTL".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap()
}

/// Commits `file = contents` on the amend work branch and squash-merges it
/// into the base, as the gated PR would. Leaves HEAD on the base.
fn amend_on_work_and_squash(company: &Path, id: &str, file: &str, contents: &str) {
    let base = format!("uplink/amend/{id}");
    let work = format!("{base}-work");
    git(company, &["checkout", "--quiet", &work], GitOpts::default()).unwrap();
    write(company, file, contents);
    commit_all(company, "address review");
    git(company, &["checkout", "--quiet", &base], GitOpts::default()).unwrap();
    git(company, &["merge", "--squash", &work], GitOpts::default()).unwrap();
    git(company, &["commit", "-m", "Amend (#7)"], GitOpts::default()).unwrap();
}

fn patch_of(company: &Path, id: &str) -> Patch {
    git_uplink::read_queue(company)
        .unwrap()
        .all_patches()
        .find(|p| p.id == id)
        .unwrap()
        .clone()
}

#[test]
fn amend_start_cuts_the_patch_and_a_seeded_work_branch_without_touching_the_queue() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    let state_before = rev_of(company, STATE_BRANCH);
    let result = amend_patch(company, &patch.id, false, None).unwrap();
    assert!(!result.completed);
    let base = result.base_branch.unwrap();
    let work = result.work_branch.unwrap();
    assert_eq!(base, format!("uplink/amend/{}", patch.id));
    assert_eq!(work, format!("{base}-work"));
    assert_eq!(
        git_ok(company, &["show", &format!("{base}:NOTES.md")]).unwrap(),
        "internal-notes"
    );
    assert_eq!(rev_of(company, &format!("{work}^")), rev_of(company, &base));
    assert_eq!(
        rev_of(company, &format!("{work}^{{tree}}")),
        rev_of(company, &format!("{base}^{{tree}}"))
    );
    assert_eq!(
        rev_of(company, &format!("{base}^")),
        result.onto.unwrap(),
        "base is onto plus the patch"
    );
    assert_eq!(rev_of(company, STATE_BRANCH), state_before);
    assert_eq!(
        git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "main"
    );
}

#[test]
fn amend_refuses_dropped_patches_and_a_message_without_complete() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    let early = amend_patch(
        company,
        &patch.id,
        false,
        Some(AmendMessage {
            title: "x".into(),
            message: None,
        }),
    )
    .unwrap_err();
    assert!(early.to_string().contains("--complete"), "{early}");
    drop_patch(company, &patch.id, "not needed").unwrap();
    let err = amend_patch(company, &patch.id, false, None).unwrap_err();
    assert!(err.to_string().contains("dropped"), "{err}");
}

#[test]
fn amend_complete_refreshes_an_internal_patch_and_rebuilds_main() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    amend_patch(company, &patch.id, false, None).unwrap();
    amend_on_work_and_squash(company, &patch.id, "NOTES.md", "internal-notes\nreviewed\n");
    let done = amend_patch(company, &patch.id, true, None).unwrap();
    assert!(done.completed && done.changed, "{done:?}");
    let after = patch_of(company, &patch.id);
    assert_eq!(after.status, PatchStatus::Queued);
    assert_eq!(after.title, patch.title);
    assert_ne!(after.patch_id_stable, patch.patch_id_stable);
    assert!(after.events.iter().any(|e| e.kind == "amended"));
    assert_eq!(
        git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "main"
    );
    assert_eq!(
        git_ok(company, &["show", "main:NOTES.md"]).unwrap(),
        "internal-notes\nreviewed"
    );
    let stored = git_ok(
        company,
        &[
            "show",
            &format!("{STATE_BRANCH}:.uplink/patches/{}.patch", patch.id),
        ],
    )
    .unwrap();
    assert!(stored.contains("+reviewed"), "{stored}");
    assert_eq!(
        git_ok(company, &["log", "-1", "--format=%s", "main"]).unwrap(),
        "Internal notes"
    );
}

#[test]
fn amend_complete_of_a_submitted_patch_takes_the_pr_message_and_is_amended() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_ttl_patch(company);
    commit_contribution_packet(company, &patch);
    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id, true).unwrap();
    record_pull_request(
        company,
        &patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &submitted.branch,
        None,
    )
    .unwrap();

    amend_patch(company, &patch.id, false, None).unwrap();
    // A merge commit instead of a squash: the patch commit stays first-parent.
    let base = format!("uplink/amend/{}", patch.id);
    let work = format!("{base}-work");
    git(company, &["checkout", "--quiet", &work], GitOpts::default()).unwrap();
    write(
        company,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 5400;"),
    );
    commit_all(company, "maintainer asked for 5400");
    git(company, &["checkout", "--quiet", &base], GitOpts::default()).unwrap();
    git(
        company,
        &["merge", "--no-ff", "--no-edit", &work],
        GitOpts::default(),
    )
    .unwrap();

    let done = amend_patch(
        company,
        &patch.id,
        true,
        Some(AmendMessage {
            title: "Extend TTL to 90 minutes".into(),
            message: Some(
                "Extend TTL to 90 minutes\n\n<!-- instructions -->\nPer review.\n".into(),
            ),
        }),
    )
    .unwrap();
    assert!(done.changed);
    let after = patch_of(company, &patch.id);
    assert_eq!(after.status, PatchStatus::Amended);
    assert_eq!(after.title, "Extend TTL to 90 minutes");
    assert!(after.commit_message.contains("Per review."), "{after:?}");
    assert!(!after.commit_message.contains("instructions"));
    assert!(after.assess.as_ref().unwrap().ok);
    assert!(
        git_ok(company, &["show", "main:src/tokens.js"])
            .unwrap()
            .contains("return 5400;")
    );
    let delta = approve_patch(company, &patch.id).unwrap();
    assert_eq!(delta.approvals.last().unwrap().kind, "delta");
}

#[test]
fn amend_complete_refuses_a_leaking_message_and_keeps_the_branch() {
    let world = setup_world();
    let company = &world.company;
    set_settings(company, |s| s.redact_keywords = vec!["AcmeCorp".into()]);
    let patch = add_ttl_patch(company);
    amend_patch(company, &patch.id, false, None).unwrap();
    amend_on_work_and_squash(
        company,
        &patch.id,
        "src/tokens.js",
        &TOKENS.replace("return 3600;", "return 5400;"),
    );
    let base = format!("uplink/amend/{}", patch.id);
    let before = rev_of(company, &base);
    let err = amend_patch(
        company,
        &patch.id,
        true,
        Some(AmendMessage {
            title: "AcmeCorp TTL".into(),
            message: None,
        }),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Assess(_)), "{err}");
    assert_eq!(rev_of(company, &base), before);
    assert_eq!(patch_of(company, &patch.id).title, patch.title);
}

#[test]
fn amend_complete_without_changes_leaves_the_queue() {
    let world = setup_world();
    let company = &world.company;
    let patch = add_internal_notes(company);
    amend_patch(company, &patch.id, false, None).unwrap();
    let base = format!("uplink/amend/{}", patch.id);
    git(company, &["checkout", "--quiet", &base], GitOpts::default()).unwrap();
    git(
        company,
        &["merge", "--no-ff", "--no-edit", &format!("{base}-work")],
        GitOpts::default(),
    )
    .unwrap();
    let state_before = rev_of(company, STATE_BRANCH);
    let done = amend_patch(company, &patch.id, true, None).unwrap();
    assert!(done.completed && !done.changed, "{done:?}");
    assert_eq!(rev_of(company, STATE_BRANCH), state_before);
    assert_eq!(
        git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
        "main"
    );
}
