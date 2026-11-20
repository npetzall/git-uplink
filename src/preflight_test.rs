use super::*;

#[test]
fn pre_image_hunks_reads_old_ranges_of_existing_files() {
    let diff = "\
diff --git a/src/a.js b/src/a.js
index 1..2 100644
--- a/src/a.js
+++ b/src/a.js
@@ -3,7 +3,8 @@ fn
 x
@@ -20 +21,2 @@
-y
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+z
diff --git a/my file b/my file
--- a/my file\t
+++ b/my file\t
@@ -1,2 +1,2 @@
";
    assert_eq!(
        pre_image_hunks(diff).unwrap(),
        vec![
            ("src/a.js".to_string(), 3, 7),
            ("src/a.js".to_string(), 20, 1),
            ("my file".to_string(), 1, 2),
        ]
    );
    assert!(pre_image_hunks("--- \"a/caf\\303\\251\"\n@@ -1 +1 @@\n").is_none());
}

#[test]
fn blame_commits_reads_only_header_lines() {
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let porcelain = format!(
        "{a} 1 1 2\nauthor x\nsummary {b} looks like a sha\n\tline one\n{a} 2 2\n\t{b} 1 1\n{b} 9 3 1\nboundary\n\tline three\n"
    );
    let shas: Vec<&str> = blame_commits(&porcelain).collect();
    assert_eq!(shas, vec![a.as_str(), a.as_str(), b.as_str()]);
}

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

fn queued(id: &str, deps: &[&str]) -> Patch {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "commitMessage": id,
        "status": "queued",
        "dependsOn": deps,
        "createdAt": "t",
        "updatedAt": "t",
        "source": {},
        "events": []
    }))
    .unwrap()
}

/// Commits `from` → `to` on a branch named `to`, with `edit` applied to
/// `f.txt`, and writes the diff from `from` to `out`.
fn commit_patch(repo: &Path, from: &str, to: &str, edit: (&str, &str), out: &Path) {
    git_ok(repo, &["checkout", "-q", "-b", to, from]).unwrap();
    let text = fs::read_to_string(repo.join("f.txt")).unwrap();
    assert!(text.contains(edit.0), "{to}: {}", edit.0);
    fs::write(repo.join("f.txt"), text.replace(edit.0, edit.1)).unwrap();
    git_ok(repo, &["commit", "-q", "-am", to]).unwrap();
    let diff = git_ok(repo, &["diff", "--full-index", from, to]).unwrap();
    fs::write(out, format!("{diff}\n")).unwrap();
}

/// Queued `upl_a` changes line 2 and `upl_b` line 18 of a 20-line file,
/// out of each other's context. Each candidate is blamed on its own.
#[test]
fn blamed_depends_on_names_the_patches_whose_lines_a_hunk_touches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let out = tempfile::tempdir().unwrap();
    git_ok(repo, &["init", "-q", "-b", "main"]).unwrap();
    git_ok(repo, &["config", "user.name", "t"]).unwrap();
    git_ok(repo, &["config", "user.email", "t@example.com"]).unwrap();
    git_ok(repo, &["config", "commit.gpgsign", "false"]).unwrap();
    let lines: String = (1..=20).map(|i| format!("l{i}\n")).collect();
    fs::write(repo.join("f.txt"), lines).unwrap();
    git_ok(repo, &["add", "f.txt"]).unwrap();
    git_ok(repo, &["commit", "-q", "-m", "init"]).unwrap();
    git_ok(repo, &["branch", "uplink/upstream"]).unwrap();

    let patch = |name: &str| out.path().join(format!("queued-{name}.patch"));
    let cand = |name: &str| out.path().join(format!("{name}.patch"));
    commit_patch(repo, "main", "upl_a", ("l2\n", "a2\n"), &patch("upl_a"));
    commit_patch(repo, "upl_a", "upl_b", ("l18\n", "b18\n"), &patch("upl_b"));
    // On A's line, on B's line, on both, and on upstream's lines only.
    commit_patch(repo, "upl_b", "on_a", ("a2\n", "c2\n"), &cand("on_a"));
    commit_patch(repo, "upl_b", "on_b", ("b18\n", "c18\n"), &cand("on_b"));
    commit_patch(repo, "on_a", "on_ab", ("b18\n", "c18\n"), &cand("on_ab"));
    let both = git_ok(repo, &["diff", "--full-index", "upl_b", "on_ab"]).unwrap();
    fs::write(cand("on_ab"), format!("{both}\n")).unwrap();
    commit_patch(
        repo,
        "upl_b",
        "on_upstream",
        ("l10\n", "c10\n"),
        &cand("on_upstream"),
    );

    let mut state = StateTree::at(repo, None).unwrap();
    for id in ["upl_a", "upl_b"] {
        state
            .write_patch(id, fs::read_to_string(patch(id)).unwrap())
            .unwrap();
    }
    let blamed = |queue: &QueueState, name: &str| {
        let candidates = vec!["upl_a".to_string(), "upl_b".to_string()];
        let candidate = fs::read_to_string(cand(name)).unwrap();
        with_upstream_worktree(repo, |dir| {
            blamed_depends_on(dir, &state, queue, &[], &candidates, &candidate, name)
        })
        .unwrap()
    };

    let mut independent = QueueState::empty(crate::types::QueueConfig::default());
    independent.upstream.push(queued("upl_a", &[]));
    independent.upstream.push(queued("upl_b", &[]));
    assert_eq!(blamed(&independent, "on_a"), Some(vec!["upl_a".into()]));
    assert_eq!(blamed(&independent, "on_b"), Some(vec!["upl_b".into()]));
    assert_eq!(
        blamed(&independent, "on_ab"),
        Some(vec!["upl_a".into(), "upl_b".into()])
    );
    assert_eq!(blamed(&independent, "on_upstream"), Some(Vec::new()));

    // With B depending on A, B alone brings A in.
    let mut chained = QueueState::empty(crate::types::QueueConfig::default());
    chained.upstream.push(queued("upl_a", &[]));
    chained.upstream.push(queued("upl_b", &["upl_a"]));
    assert_eq!(blamed(&chained, "on_ab"), Some(vec!["upl_b".into()]));
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
fn runner_command_file_names() {
    for name in [
        "GITHUB_ENV",
        "GITHUB_OUTPUT",
        "GITHUB_PATH",
        "GITHUB_STATE",
        "GITHUB_STEP_SUMMARY",
    ] {
        assert!(is_runner_command_file(name), "{name}");
    }
    for name in ["GITHUB_WORKSPACE", "GITHUB_SHA", "PATH", "RUNNER_TEMP"] {
        assert!(!is_runner_command_file(name), "{name}");
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
    let base = preflight_token(repo, "HEAD", None).unwrap();
    assert_eq!(base, preflight_token(repo, "HEAD", None).unwrap());
    assert_ne!(base, preflight_token(repo, "HEAD", Some(&first)).unwrap());

    // Same tree under another commit: same token.
    git_ok(repo, &["commit", "-q", "--allow-empty", "-m", "again"]).unwrap();
    assert_eq!(base, preflight_token(repo, "HEAD", None).unwrap());

    let second = commit_file(repo, "b.txt", "two\n");
    let moved = preflight_token(repo, "HEAD", Some(&first)).unwrap();
    assert_ne!(base, preflight_token(repo, "HEAD", None).unwrap());
    assert_ne!(moved, preflight_token(repo, "HEAD", Some(&second)).unwrap());
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
    assert_eq!(replayed.summary(), "tests failed");
    assert_eq!(replayed.to_string(), "tests failed\n\nboom");
    assert_eq!(replayed.suggested_depends_on, ["upl_1"]);
    assert_eq!(replayed.output.as_deref(), Some("boom"));

    // The script passed; what failed after it is the command's to check.
    let layer =
        PreflightError::new("layer does not apply", Vec::new(), "apply", None).with_token("abc");
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
fn script_output_is_shown_and_captured_in_order() {
    let dir = repo_with_hooks();
    let repo = dir.path();
    commit_file(
        repo,
        PREFLIGHT_SCRIPT_PATH,
        "echo one\necho two >&2\necho three\nexit 3\n",
    );
    let head = git_ok(repo, &["rev-parse", "HEAD"]).unwrap();
    let script = PreflightScript::materialise(repo, &head).unwrap();

    let mut shown = Vec::new();
    let (code, text) = script.run_to(repo, &mut shown);
    assert_eq!(code, 3);
    assert_eq!(text, "one\ntwo\nthree");
    assert_eq!(String::from_utf8(shown).unwrap(), "one\ntwo\nthree\n");

    // Off until a command sets it.
    assert_eq!(script.run(repo), (3, "one\ntwo\nthree".to_string()));
    assert_eq!(script.run_quiet(repo).0, 3);
}

#[test]
fn tested_lists_upstream_then_what_is_applied() {
    let dir = repo_with_hooks();
    let repo = dir.path();
    // No uplink/upstream: only HEAD can be named.
    let head = git_ok(repo, &["rev-parse", "--short", "HEAD"]).unwrap();
    assert_eq!(command_tested(repo), [format!("HEAD@{head}")]);

    git_ok(repo, &["branch", "uplink/upstream"]).unwrap();
    assert_eq!(command_tested(repo), [format!("uplink/upstream@{head}")]);
    commit_file(repo, "b.txt", "two\n");
    commit_file(repo, "c.txt", "three\n");
    let listed = command_tested(repo);
    assert_eq!(listed.len(), 3, "{listed:?}");
    assert_eq!(listed[0], format!("uplink/upstream@{head}"));
    assert!(listed[1].ends_with(" b.txt"), "{listed:?}");
    assert!(listed[2].ends_with(" c.txt"), "{listed:?}");

    for i in 0..TESTED_COMMITS_LIMIT {
        let message = format!("empty {i}");
        git_ok(repo, &["commit", "-q", "--allow-empty", "-m", &message]).unwrap();
    }
    let listed = command_tested(repo);
    assert_eq!(listed.len(), TESTED_COMMITS_LIMIT + 2, "{listed:?}");
    assert_eq!(listed[1], "… 2 earlier commits");
    assert!(listed.last().unwrap().ends_with(" empty 49"), "{listed:?}");

    let patch = |id: &str, status: PatchStatus| Patch {
        id: id.into(),
        status,
        depends_on: Vec::new(),
        created_at: String::new(),
        updated_at: String::new(),
        patch_id_stable: None,
        commit_message: format!("title of {id}"),
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
    let mut queue = QueueState::empty(Default::default());
    queue.push_patch(patch("upl_aaaaaaaaaa", PatchStatus::Queued), false);
    queue.push_patch(patch("upl_bbbbbbbbbb", PatchStatus::Merged), false);
    let deps = ["upl_aaaaaaaaaa".to_string(), "upl_bbbbbbbbbb".to_string()];
    assert_eq!(
        export_tested(repo, &queue, &deps, "the change"),
        [
            format!("uplink/upstream@{head}"),
            "`upl_aaaaaaaaaa` title of upl_aaaaaaaaaa".to_string(),
            "the change".to_string(),
        ]
    );
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

#[test]
fn the_report_keeps_the_end_of_the_output() {
    assert_eq!(output_tail("one\ntwo\n"), "one\ntwo");
    let many: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
    let tail = output_tail(&many);
    assert!(
        tail.starts_with("[... 9980 earlier lines in the job log ...]\nline 9980\n"),
        "{tail}"
    );
    assert!(tail.ends_with("line 9999"), "{tail}");
    let wide = format!("{}ÅEND", "é".repeat(1024 * 1024));
    let tail = output_tail(&wide);
    assert!(tail.len() <= REPORT_TEXT_LIMIT + 32, "{}", tail.len());
    assert!(tail.ends_with("ÅEND"), "{tail}");

    // Small whatever the script printed, with the output once.
    let noise: String = (0..100_000)
        .map(|i| format!("\u{1b}[31m\"{i}\"\u{1b}[0m\n"))
        .collect();
    let failure = || {
        PreflightReport::of(Err(Error::Preflight(PreflightError::new(
            "tests failed",
            Vec::new(),
            STAGE_COMMAND,
            Some(noise.clone()),
        ))))
    };
    let report = failure();
    assert_eq!(report.message.as_deref(), Some("tests failed"));
    let rebuild = PreflightReport::of_rebuild(RebuildReport {
        upstream: Some(failure()),
        built: Some(failure()),
        first_bad: None,
    });
    for report in [report, rebuild] {
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.len() < 16 * 1024, "{}", json.len());
    }
}
