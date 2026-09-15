use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use git_uplink::{
    AddPatchOpts, ApprovalReceipt, ConflictError, DEFAULT_CUTOFF, Error, GitOpts, MergeVia,
    QueueConfig, add_patch, approve_patch, configure_repo, drop_patch, format_approval_receipt,
    format_approver_packet, git, git_ok, init_repo, mark_merged, rebuild, report_paths,
    resolve_conflict, status_snapshot, submit_patch, sync, write_queue,
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

fn setup_world() -> World {
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
    init_repo(&company, QueueConfig::default()).unwrap();

    World {
        _upstream_keep: upstream_keep,
        _company_keep: company_keep,
        upstream,
        company,
    }
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
    let hash_patch = add_patch(
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
    let log_patch = add_patch(
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
    let internal = add_patch(
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
    let hash_patch = add_patch(
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
    add_patch(
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
    sync(company).unwrap();

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
    let ttl_patch = add_patch(
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
    let queued = sync(company).unwrap();
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
    assert_ne!(snapshot.queue.patches[0].status, "conflict");
    let tokens = snapshot.product_files.get("src/tokens.js").unwrap();
    assert!(tokens.contains("return 7200;"));
    assert!(!tokens.contains("return 1800;"));
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
    let hash_patch = add_patch(
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
    let internal = add_patch(
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
    let submitted = submit_patch(
        company,
        &hash_patch.id,
        Some((42, "https://github.com/upstream/tokenkit/pull/42".into())),
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
    let internal = add_patch(
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
    let patch = add_patch(
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
    let err = submit_patch(company, &patch.id, None).unwrap_err();
    assert!(err.to_string().contains("must be approved"));
    let again = add_patch(
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

    git(&ben, &["checkout", "-b", "feat/notes"], GitOpts::default()).unwrap();
    write(&ben, "NOTES.md", "from-ben\n");
    commit_all(&ben, "notes from ben");

    let asha_t = asha.clone();
    let ben_t = ben.clone();
    let h1 = thread::spawn(move || {
        add_patch(
            &asha_t,
            AddPatchOpts {
                title: "Readme from Asha".into(),
                from_ref: Some("main".into()),
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
                from_ref: Some("main".into()),
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
    let hash_patch = add_patch(
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
    let hash_patch = add_patch(
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

    let imported = add_patch(
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
    let hash_patch = add_patch(
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
    let err = submit_patch(company, &hash_patch.id, None);
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
    git(company, &["add", "-A"], GitOpts::default()).unwrap();
    git(
        company,
        &[
            "commit",
            "-m",
            &format!(
                "Use SHA-256 for tokens\n\nReplace SHA-1 in the default hasher.\n\n{DEFAULT_CUTOFF}\n\nTicket: PROJ-9999\nUplink-Export-Author: Jane Public <jane@users.noreply.github.com>\n"
            ),
        ],
        GitOpts::default(),
    )
    .unwrap();
    let patch = add_patch(
        company,
        AddPatchOpts {
            title: "Use SHA-256 for tokens".into(),
            from_ref: Some("main".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let prepare = patch.prepare.as_ref().unwrap();
    assert!(prepare.ok);
    assert!(prepare.cutoff_found);
    assert_eq!(prepare.author_email, "jane@users.noreply.github.com");
    let stored =
        fs::read_to_string(company.join(format!(".uplink/patches/{}.patch", patch.id))).unwrap();
    assert!(stored.contains("Replace SHA-1 in the default hasher."));
    assert!(!stored.contains("PROJ-9999"));
    assert!(!stored.contains("Jane Public"));

    approve_patch(company, &patch.id).unwrap();
    let submitted = submit_patch(company, &patch.id, None).unwrap();
    let author = git_ok(
        company,
        &["log", "-1", "--format=%an <%ae>", &submitted.branch],
    )
    .unwrap();
    assert_eq!(author, "Jane Public <jane@users.noreply.github.com>");
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
fn formats_an_oss_environment_packet_and_keeps_reports_across_rebuild() {
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
    let patch = add_patch(
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
    assert!(packet.contains(&format!("OSS contribution packet — {}", patch.id)));
    assert!(packet.contains("**oss** GitHub Environment"));
    assert!(packet.contains("#44"));
    assert!(packet.contains("GITHUB_STEP_SUMMARY"));

    let (_, prepare_path, approval_path) = report_paths(&patch.id);
    write(company, &prepare_path, &packet);
    write(
        company,
        &approval_path,
        &format_approval_receipt(ApprovalReceipt {
            patch_id: &patch.id,
            environment: "oss",
            actor: "dispatcher",
            run_url: "https://github.example/acme/product/actions/runs/9",
            sha: "abc123",
            at: Some("2026-09-14T00:00:00.000Z".into()),
        }),
    );
    git(
        company,
        &["add", "--", ".uplink/reports"],
        GitOpts::default(),
    )
    .unwrap();
    git(
        company,
        &["commit", "-m", &format!("uplink: OSS packet {}", patch.id)],
        GitOpts::default(),
    )
    .unwrap();

    rebuild(company).unwrap();
    let kept = fs::read_to_string(company.join(&prepare_path)).unwrap();
    let receipt = fs::read_to_string(company.join(&approval_path)).unwrap();
    assert!(kept.contains(&patch.id));
    assert!(receipt.contains("authoritative approval event"));
    assert!(receipt.contains("`oss`"));
    assert!(receipt.contains("dispatcher"));
    let tracked = git_ok(company, &["show", &format!("HEAD:{prepare_path}")]).unwrap();
    assert!(tracked.contains("Use SHA-256 for tokens"));
}

#[test]
fn conflict_error_is_an_error() {
    let error = ConflictError::new("blocked", "upl_1", vec!["src/tokens.js".into()]);
    assert_eq!(error.patch_id, "upl_1");
    let _err: &dyn std::error::Error = &error;
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
}
