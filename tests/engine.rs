use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use git_uplink::{
    AddPatchOpts, AdoptGroup, ApprovalReceipt, ConflictError, DEFAULT_CUTOFF, Error, Forge,
    GitOpts, IncomingPreflight, InitOpts, MergeVia, Patch, PushOpts, QueueConfig, QueueState,
    RebuildOpts, Result, STATE_BRANCH, TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE, accept_upstream,
    add_patch, approve_patch, configure_repo, drop_patch, format_approval_receipt,
    format_approver_packet, format_contribution_packet, from_upstream_report_paths, git, git_ok,
    init, init_repo, mark_merged, parse_depends_on, preflight_incoming_change, push_queue, rebuild,
    rebuild_with, record_conflict_issue, record_pull_request, refresh_from_origin, report_paths,
    reset_from_origin, resolve_conflict, status_snapshot, strip_html_comments, submit_patch,
    summarize_queue, sync, write_queue,
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
    let (_, prepare_path, _) = report_paths(&patch.id);
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
    configure_repo(&upstream).unwrap();
    write(&upstream, "src/tokens.js", TOKENS);
    write(&upstream, "README.md", "tokenkit\n");
    commit_all(&upstream, "initial tokens");

    let contrib_bare = create_bare_from(&upstream);

    let company_keep = temp_dir();
    let company = company_keep.path().to_path_buf();
    git(&company, &["init", "-b", "main"], GitOpts::default()).unwrap();
    configure_repo(&company).unwrap();
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

fn tree_has_uplink(repo: &Path, git_ref: &str) -> bool {
    git(
        repo,
        &["cat-file", "-e", &format!("{git_ref}:.uplink")],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap()
    .code
        == 0
}

fn has_git_ref(repo: &Path, git_ref: &str) -> bool {
    let result = git(
        repo,
        &["rev-parse", "--verify", "--quiet", git_ref],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap();
    result.code == 0 && !result.stdout.is_empty()
}

fn has_git_object(repo: &Path, git_ref: &str) -> bool {
    git(
        repo,
        &["cat-file", "-e", git_ref],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
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
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
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
            forge: Some(Forge::Ghec),
            ..Default::default()
        },
    )
    .unwrap()
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
    configure_repo(&dir).unwrap();
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
    assert!(stored.contains("\"forge\": \"ghec\""));
    assert!(!stored.contains("companyBranch"));
    assert_eq!(queue.config.forge, Some(Forge::Ghec));
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
            .join(".github/workflows/uplink-prepare.yml")
            .is_file()
    );
    assert!(
        world
            .company
            .join(".github/pull_request_template.md")
            .is_file()
    );
    assert!(
        !world
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
    configure_repo(&clone).unwrap();

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
    configure_repo(&clone).unwrap();
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
    .unwrap();
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
            forge: Some(Forge::ExampleGithub),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(queue.config.forge, Some(Forge::ExampleGithub));
    assert!(
        world
            .company
            .join(".github/actions/install-git-uplink/action.yml")
            .is_file()
    );
    let ghec_template = fs::read_to_string("templates/github/pull_request_template.md").unwrap();
    let installed =
        fs::read_to_string(world.company.join(".github/pull_request_template.md")).unwrap();
    assert_eq!(installed, ghec_template);
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
    assert!(!clone.join(".github/workflows/uplink-prepare.yml").is_file());
    init(&clone, InitOpts::default()).unwrap();
    assert!(!clone.join(".github/workflows/uplink-prepare.yml").is_file());
}

#[test]
fn init_rejects_forge_renames_on_an_existing_queue() {
    let world = setup_uninitialized();
    init_with_recorded_urls(&world);
    let err = init(
        &world.company,
        InitOpts {
            forge: Some(Forge::ExampleGithub),
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("forge"), "{err}");
    assert!(err.contains("ghec"), "{err}");
    assert!(err.contains("example-github"), "{err}");
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

    let original = git_ok(&world.company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
    git(
        &world.company,
        &["checkout", "--quiet", "--detach", "uplink/upstream"],
        GitOpts::default(),
    )
    .unwrap();
    write(
        &world.company,
        ".github/workflows/uplink-prepare.yml",
        "stale\n",
    );
    git(&world.company, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        &world.company,
        &["commit", "-m", "stale tooling"],
        GitOpts::default(),
    )
    .unwrap();
    let stale = git_ok(
        &world.company,
        &["format-patch", "--full-index", "-1", "--stdout"],
    )
    .unwrap();
    git(
        &world.company,
        &["checkout", "-f", "--quiet", &original],
        GitOpts::default(),
    )
    .unwrap();
    fs::write(
        world.company.join(format!(".uplink/patches/{id}.patch")),
        stale,
    )
    .unwrap();
    let mut queue = git_uplink::read_queue(&world.company).unwrap();
    queue.tooling.as_mut().unwrap().patch_id_stable = Some("stale".into());
    git_uplink::write_queue(&world.company, &queue).unwrap();
    git_uplink::commit_queue(&world.company, "uplink: stale tooling patch").unwrap();

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
        fs::read_to_string(world.company.join(".github/workflows/uplink-prepare.yml")).unwrap();
    assert!(
        prepare.contains("name: Uplink prepare for upstream"),
        "{prepare}"
    );
    assert!(!prepare.trim().eq("stale"));
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

fn adopt_group(commits: &[&str], title: &str, intent: &str) -> AdoptGroup {
    AdoptGroup {
        commits: commits.iter().map(|s| s.to_string()).collect(),
        title: title.into(),
        intent: intent.into(),
        message: None,
    }
}

fn init_adopt(world: &World, groups: Vec<AdoptGroup>) -> QueueState {
    init(
        &world.company,
        InitOpts {
            upstream_url: Some(world.upstream.to_str().unwrap().into()),
            contrib_url: Some(remote_get_url(&world.company, "contrib")),
            forge: Some(Forge::Ghec),
            adopt_groups: Some(groups),
            interactive: Some(false),
            ..Default::default()
        },
    )
    .unwrap()
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
            adopt_group(&[&a, &b], "Metrics", "upstream"),
            adopt_group(&[&c], "Dashboards", "internal-only"),
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
            .join(".github/workflows/uplink-prepare.yml")
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
            adopt_group(&[&a, &b], "Metrics", "upstream"),
            adopt_group(&[&c], "Dashboards", "internal-only"),
        ],
    );
    let queue_before = git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap();
    rebuild_with(
        &world.company,
        RebuildOpts {
            branch: Some("uplink/verify".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rev(&world.company), original);
    assert_eq!(
        git_ok(&world.company, &["rev-parse", STATE_BRANCH]).unwrap(),
        queue_before
    );
    assert!(has_git_ref(&world.company, "uplink/verify"));
    let diff = git(
        &world.company,
        &[
            "diff",
            "--quiet",
            original.as_str(),
            "uplink/verify",
            "--",
            ".",
            ":!.github",
        ],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap();
    assert_eq!(diff.code, 0, "{}", diff.stderr);
    git(
        &world.company,
        &[
            "cat-file",
            "-e",
            "uplink/verify:.github/workflows/uplink-prepare.yml",
        ],
        GitOpts::default(),
    )
    .unwrap();
}

#[test]
fn rebuild_after_adopt_replays_onto_main() {
    let world = setup_uninitialized();
    let [a, b, c] = three_linear_ahead(&world.company);
    let original = rev(&world.company);
    init_adopt(
        &world,
        vec![
            adopt_group(&[&a, &b], "Metrics", "upstream"),
            adopt_group(&[&c], "Dashboards", "internal-only"),
        ],
    );
    rebuild(&world.company).unwrap();
    assert_ne!(rev(&world.company), original);
    assert!(
        world
            .company
            .join(".github/workflows/uplink-prepare.yml")
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
            forge: Some(Forge::Ghec),
            adopt_groups: Some(vec![adopt_group(&[&side], "Side", "upstream")]),
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
            adopt_group(&[&m1], "One", "upstream"),
            adopt_group(&[&m2], "Two", "upstream"),
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
            adopt_group(&[&merge], "One", "upstream"),
            adopt_group(&[&direct], "Hotfix", "upstream"),
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
    let queue = init_adopt(&world, vec![adopt_group(&[&real], "Real", "upstream")]);
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
            forge: Some(Forge::Ghec),
            adopt_groups: Some(vec![adopt_group(&[&a, &b], "Partial", "upstream")]),
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
            forge: Some(Forge::Ghec),
            adopt_groups: Some(vec![
                adopt_group(&[&a, &c], "Split", "upstream"),
                adopt_group(&[&b], "Mid", "upstream"),
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
            forge: Some(Forge::Ghec),
            adopt_groups: Some(vec![adopt_group(&["HEAD"], "Nope", "upstream")]),
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
            forge: Some(Forge::Ghec),
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
            adopt_group(&[&a, &b], "Metrics", "upstream"),
            adopt_group(&[&c], "Dashboards", "internal-only"),
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
    configure_repo(&clone).unwrap();
    let _ = git(
        &clone,
        &["branch", "-D", "uplink/upstream"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
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
            preflight_command: None,
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
    configure_repo(&clone).unwrap();
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
            preflight_command: None,
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
            preflight_command: None,
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
            preflight_command: None,
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
    assert_eq!(merged.status, "merged");
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
    assert_eq!(merged.status, "merged");
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
    assert_eq!(result.queue.patch_refs()[0].status, "queued");
    accept_upstream(company).unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patch_refs()[0].status, "merged");
    let changelog = snapshot.product_files.get("CHANGELOG.md").unwrap();
    assert!(changelog.contains("release note"));
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
    let queued = sync_apply(company);
    let conflicted = queued.all_patches().find(|p| p.id == ttl_patch.id).unwrap();
    assert_eq!(conflicted.status, "conflict");
    let conflict_branch = conflicted
        .conflict
        .as_ref()
        .map(|c| c.branch.as_str())
        .unwrap();
    assert_eq!(conflict_branch, format!("uplink/conflict/{}", ttl_patch.id));

    let on_main = git_ok(company, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
    assert_eq!(on_main, "main");
    let main_tokens = fs::read_to_string(company.join("src/tokens.js")).unwrap();
    assert!(main_tokens.contains("return 7200;"), "{main_tokens}");
    assert!(!main_tokens.contains("<<<<<<"), "{main_tokens}");

    git(
        company,
        &["checkout", "--quiet", conflict_branch],
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
    assert_eq!(snapshot.queue.patch_refs()[0].status, "queued");
    let tokens = snapshot.product_files.get("src/tokens.js").unwrap();
    assert!(tokens.contains("return 7200;"));
    assert!(!tokens.contains("return 1800;"));
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
    let submitted = submit_patch(company, &ttl_patch.id).unwrap();
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
    assert_eq!(noop.status, "submitted");
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
    assert_eq!(conflicted.status, "conflict");
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

    let after_resolve = status_snapshot(company).unwrap();
    let amended = after_resolve
        .queue
        .all_patches()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
    assert_eq!(amended.status, "amended");
    assert_eq!(summarize_queue(&after_resolve.queue).amended, 1);
    let fork_after_resolve =
        git_ok(company, &["rev-parse", &format!("uplink/{}", ttl_patch.id)]).unwrap();
    assert_eq!(fork_after_resolve, submitted.sha);
    let err = submit_patch(company, &ttl_patch.id).unwrap_err();
    assert!(err.to_string().contains("must be approved"), "{}", err);

    let packet = format_contribution_packet(company, amended).unwrap();
    assert!(packet.contains(&format!("Delta packet — {}", ttl_patch.id)));
    assert!(packet.contains("already IP-approved"));
    assert!(packet.contains("Already approved (initial)"));
    assert!(packet.contains("### Upstream contrib"));
    assert!(packet.contains(&amended.approvals[0].sha));
    assert!(packet.contains("Uplink-Patch-Id"));

    let (_, prepare_path, _) = report_paths(&ttl_patch.id);
    write(company, &prepare_path, &packet);
    git_uplink::commit_queue(
        company,
        &format!("uplink: contribution packet {}", ttl_patch.id),
    )
    .unwrap();

    let second = approve_patch(company, &ttl_patch.id).unwrap();
    assert_eq!(second.status, "approved");
    assert_eq!(second.approvals.len(), 2);
    assert_eq!(second.approvals[1].kind, "delta");
    assert_ne!(second.approvals[0].sha, second.approvals[1].sha);

    let resubmitted = submit_patch(company, &ttl_patch.id).unwrap();
    let recorded = record_pull_request(
        company,
        &ttl_patch.id,
        99,
        "https://github.com/upstream/tokenkit/pull/99",
        &resubmitted.branch,
        None,
    )
    .unwrap();
    assert_eq!(recorded.status, "submitted");
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
    assert_eq!(conflicted.status, "conflict");
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
    assert_eq!(amended.status, "amended");
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
    assert_eq!(asha_conflicted.status, "conflict");
    assert_eq!(
        queued
            .all_patches()
            .find(|p| p.id == ben.id)
            .unwrap()
            .status,
        "queued"
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
        Some(2),
        "expected resolve exit 2 after amending asha, got {:?}\n{stderr}",
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
    assert_ne!(asha_after.status, "conflict");
    assert_eq!(ben_after.status, "conflict");
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
    let submitted = submit_patch(company, &hash_patch.id).unwrap();
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
        "merged"
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
    assert_eq!(snapshot.queue.patch_refs()[0].status, "dropped");
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
    assert_eq!(patch.status, "queued");
    let err = submit_patch(company, &patch.id).unwrap_err();
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
    assert_eq!(patch.status, "queued");
    let upstream_tokens = git_ok(company, &["show", "uplink/upstream:src/tokens.js"]).unwrap();
    assert!(upstream_tokens.contains("return sha1(value);"));
    assert!(!upstream_tokens.contains("return sha256(value);"));
    approve_patch(company, &patch.id).unwrap();
    assert_eq!(
        status_snapshot(company).unwrap().queue.patch_refs()[0].status,
        "approved"
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
    assert_eq!(patch.status, "merged");
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
    assert_eq!(snapshot.state.ahead, Some(2));
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
    assert_eq!(snapshot.state.behind, Some(2));
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

    let command = "grep -q sha256 src/tokens.js";
    let err = add_patch(
        company,
        AddPatchOpts {
            title: "Add hash checker".into(),
            from_ref: Some("main".into()),
            preflight_command: Some(command.into()),
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
            preflight_command: Some(command.into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(imported.depends_on, vec![hash_patch.id]);
    assert_eq!(imported.status, "queued");
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
            preflight_command: None,
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

    // Isolate the failing command on this repo's queue config. Do not set
    // UPLINK_PREFLIGHT here: cargo test runs cases in parallel and a process-wide
    // env override leaks into other adds.
    let mut queue = git_uplink::read_queue(company).unwrap();
    queue.config.preflight_command = Some("exit 1".into());
    write_queue(company, &queue).unwrap();
    let err = submit_patch(company, &hash_patch.id);
    assert!(matches!(err, Err(Error::Preflight(_))));

    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patch_refs()[0].status, "approved");
    assert!(
        snapshot.queue.patch_refs()[0]
            .upstream
            .as_ref()
            .and_then(|u| u.pr_number)
            .is_none()
    );
}

#[test]
fn strips_the_internal_commit_section_and_rewrites_export_author() {
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
    let prepare = patch.prepare.as_ref().unwrap();
    assert!(prepare.ok);
    assert!(prepare.cutoff_found);
    assert_eq!(prepare.author_email, "jane@users.noreply.github.com");
    assert!(!patch.commit_message.contains("Visible while writing"));
    assert!(!patch.commit_message.contains("wip: ignore this git log"));
    assert!(patch.commit_message.contains("PROJ-9999"));
    assert!(patch.commit_message.contains(DEFAULT_CUTOFF));

    let company_msg = git_ok(company, &["log", "-1", "--format=%B", "main"]).unwrap();
    assert!(company_msg.contains("Use SHA-256 for tokens"));
    assert!(company_msg.contains(&format!("Uplink-Patch-Id: {}", patch.id)));
    assert!(!company_msg.contains("wip: ignore this git log"));

    let stored =
        fs::read_to_string(company.join(format!(".uplink/patches/{}.patch", patch.id))).unwrap();
    assert!(stored.contains("Replace SHA-1 in the default hasher."));
    assert!(stored.contains("PROJ-9999"));
    assert!(!stored.contains("Visible while writing"));

    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id).unwrap();
    let author = git_ok(
        company,
        &["log", "-1", "--format=%an <%ae>", &submitted.branch],
    )
    .unwrap();
    assert_eq!(author, "Jane Public <jane@users.noreply.github.com>");
    let contrib_msg = git_ok(company, &["log", "-1", "--format=%B", &submitted.branch]).unwrap();
    assert!(contrib_msg.contains("Replace SHA-1 in the default hasher."));
    assert!(!contrib_msg.contains("PROJ-9999"));
    assert!(!contrib_msg.contains(DEFAULT_CUTOFF));
    assert!(contrib_msg.contains(&format!("Uplink-Patch-Id: {}", patch.id)));
}

#[test]
fn does_not_squash_git_commit_messages_on_import() {
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
    let company_msg = git_ok(company, &["log", "-1", "--format=%B", "main"]).unwrap();
    assert!(company_msg.contains("Use SHA-256 for tokens"));
    assert!(!company_msg.contains("WIP second"));
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
    let mut queue = git_uplink::read_queue(company).unwrap();
    queue.config.redact_keywords = vec!["AcmeCorp".into()];
    write_queue(company, &queue).unwrap();

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
    assert!(matches!(err, Error::Prepare(_)));
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
            from_ref: Some("main".into()),
            internal_pr_number: Some(44),
            ..Default::default()
        },
    )
    .unwrap();

    let packet = format_approver_packet(&patch);
    assert!(packet.contains(&format!("Contribution packet — {}", patch.id)));
    assert!(packet.contains("**to-upstream** GitHub Environment"));
    assert!(packet.contains("#44"));
    assert!(packet.contains("GITHUB_STEP_SUMMARY"));
    assert!(packet.contains("Commit messages that will be used"));
    assert!(packet.contains("### Company main"));
    assert!(packet.contains("### Upstream contrib"));
    assert!(packet.contains(&format!("Uplink-Patch-Id: {}", patch.id)));

    let (_, prepare_path, approval_path) = report_paths(&patch.id);
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
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap();
    assert_ne!(on_main.code, 0, ".uplink must not live on main");
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
    let exported = submit_patch(company, &patch.id).unwrap();
    let after_submit = status_snapshot(company).unwrap();
    assert_eq!(after_submit.queue.patch_refs()[0].status, "approved");
    assert!(
        after_submit.queue.patch_refs()[0]
            .upstream
            .as_ref()
            .and_then(|u| u.pr_number)
            .is_none()
    );

    let url = "https://github.com/upstream/tokenkit/pull/7";
    let recorded = record_pull_request(company, &patch.id, 7, url, &exported.branch, None).unwrap();
    assert_eq!(recorded.status, "submitted");
    assert_eq!(recorded.upstream.as_ref().unwrap().pr_number, Some(7));
    assert_eq!(
        recorded.upstream.as_ref().unwrap().pr_url.as_deref(),
        Some(url)
    );

    let again = record_pull_request(company, &patch.id, 7, url, &exported.branch, None).unwrap();
    assert_eq!(again.status, "submitted");
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
fn conflicted_records_the_issue_on_the_patch() {
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
    assert_eq!(conflicted.status, "conflict");

    let url = "https://github.com/acme/product/issues/12";
    let recorded = record_conflict_issue(company, &ttl_patch.id, 12, url, None).unwrap();
    assert_eq!(recorded.conflict.as_ref().unwrap().issue_number, Some(12));
    assert_eq!(
        recorded.conflict.as_ref().unwrap().issue_url.as_deref(),
        Some(url)
    );
    let again = record_conflict_issue(company, &ttl_patch.id, 12, url, None).unwrap();
    assert_eq!(again.conflict.as_ref().unwrap().issue_number, Some(12));
    let err = record_conflict_issue(
        company,
        &ttl_patch.id,
        13,
        "https://github.com/acme/product/issues/13",
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
    assert_eq!(upstream.status, "merged");

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
    assert_eq!(internal.status, "queued");
    rebuild(company).unwrap();
    let after = status_snapshot(company).unwrap();
    assert_eq!(
        after
            .queue
            .all_patches()
            .find(|p| p.id == internal.id)
            .unwrap()
            .status,
        "queued"
    );
    assert_eq!(
        after
            .queue
            .all_patches()
            .find(|p| p.id == upstream.id)
            .unwrap()
            .status,
        "merged"
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
            preflight_command: None,
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
            preflight_command: None,
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
        text.contains("conflicted"),
        "expected conflicted subcommand in help, got:\n{text}"
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
