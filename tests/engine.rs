use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use git_uplink::{
    AddPatchOpts, ApprovalReceipt, ConflictError, DEFAULT_CUTOFF, Error, GitOpts,
    IncomingPreflight, InitOpts, MergeVia, Patch, QueueConfig, QueueState, Result, STATE_BRANCH,
    accept_upstream, add_patch, approve_patch, configure_repo, drop_patch, format_approval_receipt,
    format_approver_packet, format_contribution_packet, from_upstream_report_paths, git, git_ok,
    init, init_repo, mark_merged, parse_depends_on, preflight_incoming_change, rebuild,
    record_conflict_issue, record_pull_request, report_paths, resolve_conflict, status_snapshot,
    strip_html_comments, submit_patch, summarize_queue, sync, write_queue,
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
    assert!(!stored.contains("companyBranch"));
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
fn queue_config_reads_legacy_company_branch_alias() {
    let raw = r#"{
      "version": 1,
      "config": {
        "upstreamRemote": "upstream",
        "upstreamBranch": "main",
        "contribRemote": "contrib",
        "companyBranch": "release",
        "trailerKey": "Uplink-Patch-Id"
      },
      "patches": []
    }"#;
    let queue: QueueState = serde_json::from_str(raw).unwrap();
    assert_eq!(queue.config.internal_branch, "release");
    let out = serde_json::to_string(&queue).unwrap();
    assert!(out.contains("internalBranch"));
    assert!(!out.contains("companyBranch"));
}

#[test]
fn add_records_the_patch_on_state_without_moving_main() {
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
    land_on_main(company, &head_sha);
    let main_before = git_ok(company, &["rev-parse", "main"]).unwrap();
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
    assert_eq!(
        git_ok(company, &["rev-parse", "main"]).unwrap(),
        main_before
    );
    assert!(!tree_has_uplink(company, "main"));
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
    assert_eq!(status_snapshot(company).unwrap().queue.patches.len(), 0);
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
    assert_eq!(internal.intent, "internal-only");
    assert_eq!(log_patch.depends_on, vec![hash_patch.id]);
    assert_eq!(snapshot.queue.patches.len(), 3);
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
        .patches
        .iter()
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
        .patches
        .iter()
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
    assert_eq!(result.queue.patches[0].status, "queued");
    accept_upstream(company).unwrap();
    let snapshot = status_snapshot(company).unwrap();
    assert_eq!(snapshot.queue.patches[0].status, "merged");
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
    let conflicted = queued
        .patches
        .iter()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
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
    assert_eq!(snapshot.queue.patches[0].status, "queued");
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
    let conflicted = queued
        .patches
        .iter()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
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
        .patches
        .iter()
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
    let conflicted = again.patches.iter().find(|p| p.id == ttl_patch.id).unwrap();
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
        .patches
        .iter()
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
    let asha_conflicted = queued.patches.iter().find(|p| p.id == asha.id).unwrap();
    assert_eq!(asha_conflicted.status, "conflict");
    assert_eq!(
        queued
            .patches
            .iter()
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
        .patches
        .iter()
        .find(|p| p.id == asha.id)
        .unwrap();
    let ben_after = snapshot
        .queue
        .patches
        .iter()
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
            .patches
            .iter()
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
    assert_eq!(snapshot.queue.patches[0].status, "dropped");
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
    assert_eq!(status_snapshot(company).unwrap().queue.patches.len(), 1);
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
        status_snapshot(company).unwrap().queue.patches[0].status,
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
    assert_eq!(snapshot.queue.patches.len(), 2);
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
    let origin = create_bare_from(company);
    git(
        company,
        &["remote", "add", "origin", origin.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["push", "--quiet", "-u", "origin", "main"],
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

    let clone_company = |origin: &Path, upstream: &Path| {
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
    };

    let (asha_keep, asha) = clone_company(&origin, &upstream);
    let (ben_keep, ben) = clone_company(&origin, &upstream);

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
                push_remote: Some("origin".into()),
                internal_pr_number: Some(201),
                ..Default::default()
            },
        )
    });
    let h2 = thread::spawn(move || {
        add_patch(
            &ben_t,
            AddPatchOpts {
                title: "Notes from Ben".into(),
                from_ref: Some(from_b),
                head_ref: Some(sha_b),
                push_remote: Some("origin".into()),
                internal_pr_number: Some(202),
                ..Default::default()
            },
        )
    });
    h1.join().unwrap().unwrap();
    h2.join().unwrap().unwrap();

    let (_integrated_keep, integrated) = clone_company(&origin, &upstream);
    let snapshot = status_snapshot(&integrated).unwrap();
    let mut titles: Vec<_> = snapshot
        .queue
        .patches
        .iter()
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
    assert_eq!(status_snapshot(company).unwrap().queue.patches.len(), 1);
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
    assert_eq!(snapshot.queue.patches[0].status, "approved");
    assert!(
        snapshot.queue.patches[0]
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
    assert!(company_msg.contains("wip: ignore this git log"));
    assert!(!company_msg.contains("Replace SHA-1 in the default hasher."));
    assert!(!company_msg.contains(&format!("Uplink-Patch-Id: {}", patch.id)));

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
    assert!(company_msg.contains("WIP second"));
    assert!(!company_msg.contains("Use SHA-256 for tokens"));
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
    assert_eq!(status_snapshot(company).unwrap().queue.patches.len(), 0);
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
    assert_eq!(after_submit.queue.patches[0].status, "approved");
    assert!(
        after_submit.queue.patches[0]
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
    let conflicted = queued
        .patches
        .iter()
        .find(|p| p.id == ttl_patch.id)
        .unwrap();
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
}
