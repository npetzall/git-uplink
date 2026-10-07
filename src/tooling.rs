use std::fs;
use std::path::Path;

use rust_embed::RustEmbed;

use crate::assess::{append_patch_id_trailer, assess_from_message, strip_html_comments};
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok, git_succeeds};
use crate::queue::{
    add_event, patch_path, read_queue as read_queue_file, write_queue as write_queue_file,
};
use crate::repo::{
    TempWorktree, commit_queue, ensure_uplink_dirs, ensure_upstream_ref, has_ref, new_patch_id,
    patch_substance, stable_patch_id_from_contents, stamp,
};
use crate::types::{
    AssessReport, DEFAULT_CUTOFF, Forge, ForgeFamily, Patch, PatchIntent, PatchSource, PatchStatus,
    QueueState, TOOLING_PATCH_KIND, TOOLING_PATCH_TITLE,
};

#[derive(RustEmbed)]
#[folder = "templates/try-it-on-github"]
struct TryItOnGithubPack;

#[derive(RustEmbed)]
#[folder = "templates/github"]
struct GithubFamilyPack;

#[derive(RustEmbed)]
#[folder = "templates/github-hooks"]
struct GithubHooksPack;

/// The action every job installs the binary with, and the placeholder in it
/// that `composed_files` replaces with this binary's version: pack and binary
/// are one release.
const INSTALL_ACTION: &str = ".github/actions/install-git-uplink/action.yml";
const VERSION_PLACEHOLDER: &str = "@GIT_UPLINK_VERSION@";

pub struct ToolingRefresh {
    pub changed: bool,
}

pub fn refresh_tooling_patch(repo: &Path) -> Result<ToolingRefresh> {
    ensure_upstream_ref(repo)?;
    if !has_ref(repo, "uplink/upstream")? {
        return Err(Error::msg(
            "uplink/upstream is missing; cannot install forge tooling until upstream is seeded.",
        ));
    }
    let queue = read_queue_file(repo)?;
    let forge = queue.config.forge.ok_or_else(|| {
        Error::msg(
            "queue.json has no forge. Re-run `git uplink init --upgrade --forge github` \
(or --forge try-it-on-github) to record it.",
        )
    })?;
    let files = composed_files(forge)?;
    if files.is_empty() {
        return Err(Error::msg(format!(
            "embedded forge pack for {forge} is empty"
        )));
    }

    let existing_id = find_tooling_patch(&queue, repo)?;
    let (id, created) = if let Some(id) = existing_id {
        (id, false)
    } else {
        (new_patch_id(), true)
    };
    let message = tooling_commit_message();
    let file_message = append_patch_id_trailer(&strip_html_comments(&message), &id);
    let SynthesizedPatch {
        formatted,
        from_sha,
        head_sha,
    } = synthesize_pack_patch(repo, &files, &file_message)?;

    if !created {
        let patch_rel = patch_path(&id)?;
        let stored = fs::read_to_string(repo.join(&patch_rel)).unwrap_or_default();
        if patch_substance(&stored) == patch_substance(&formatted) {
            return Ok(ToolingRefresh { changed: false });
        }
    }

    let new_stable = stable_patch_id_from_contents(repo, &formatted)?;
    ensure_uplink_dirs(repo)?;
    let mut queue = read_queue_file(repo)?;
    let patch_rel = patch_path(&id)?;
    fs::write(repo.join(&patch_rel), &formatted)?;

    let assess = assess_from_message(
        repo,
        &queue,
        &from_sha,
        &head_sha,
        &message,
        Some(TOOLING_PATCH_TITLE),
        PatchIntent::InternalOnly,
    )?;

    if created {
        queue.tooling = Some(new_tooling_patch(&id, forge, new_stable, assess));
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: add {id} {TOOLING_PATCH_TITLE}"))?;
    } else {
        upgrade_tooling_patch(&mut queue, &id, forge, new_stable, assess);
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: upgrade {id} {TOOLING_PATCH_TITLE}"))?;
    }

    Ok(ToolingRefresh { changed: true })
}

fn new_tooling_patch(id: &str, forge: Forge, stable: String, assess: AssessReport) -> Patch {
    let created_at = stamp();
    let mut patch = Patch {
        id: id.to_string(),
        title: TOOLING_PATCH_TITLE.into(),
        commit_message: assess.commit_message.clone(),
        status: PatchStatus::Queued,
        depends_on: Vec::new(),
        created_at: created_at.clone(),
        updated_at: created_at,
        patch_id_stable: Some(stable),
        source: PatchSource {
            note: Some(format!("forge {forge}")),
            ..Default::default()
        },
        assess: Some(assess),
        upstream: None,
        merged: None,
        conflict: None,
        approvals: Vec::new(),
        extras: None,
        events: Vec::new(),
        kind: Some(TOOLING_PATCH_KIND.into()),
    };
    add_event(
        &mut patch,
        "created",
        format!("Installed {forge} forge pack as internal-only tooling"),
    );
    patch
}

/// Moves patch `id` into the tooling layer if needed and records the refreshed pack.
fn upgrade_tooling_patch(
    queue: &mut QueueState,
    id: &str,
    forge: Forge,
    stable: String,
    assess: AssessReport,
) {
    if queue.tooling.as_ref().map(|p| p.id.as_str()) != Some(id) {
        if let Some(idx) = queue.internal.iter().position(|p| p.id == id) {
            queue.tooling = Some(queue.internal.remove(idx));
        } else if let Some(idx) = queue.upstream.iter().position(|p| p.id == id) {
            queue.tooling = Some(queue.upstream.remove(idx));
        }
    }
    if let Some(patch) = queue.tooling.as_mut() {
        patch.kind = Some(TOOLING_PATCH_KIND.into());
        patch.status = PatchStatus::Queued;
        patch.conflict = None;
        patch.patch_id_stable = Some(stable);
        patch.commit_message = assess.commit_message.clone();
        patch.assess = Some(assess);
        add_event(
            patch,
            "upgraded",
            format!("Refreshed {forge} forge pack from embedded templates"),
        );
    }
}

/// The forge's overlay laid over the family base pack. Both hold repo-relative
/// paths. A path in both comes from the overlay; `Forge::omitted_paths` are
/// dropped.
pub fn composed_files(forge: Forge) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    match forge {
        Forge::Github => {}
        Forge::TryItOnGithub => collect_pack::<TryItOnGithubPack>(&mut files),
    }
    match forge.family() {
        ForgeFamily::Github => collect_pack::<GithubFamilyPack>(&mut files),
    }
    // The sort is stable, so the overlay's copy of a path stays first.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    files.retain(|(path, _)| !forge.omitted_paths().contains(&path.as_str()));
    stamp_version(&mut files)?;
    Ok(files)
}

fn stamp_version(files: &mut [(String, Vec<u8>)]) -> Result<()> {
    let Some((_, data)) = files.iter_mut().find(|(path, _)| path == INSTALL_ACTION) else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(data);
    if !text.contains(VERSION_PLACEHOLDER) {
        return Err(Error::msg(format!(
            "embedded {INSTALL_ACTION} lacks {VERSION_PLACEHOLDER}"
        )));
    }
    *data = text
        .replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION"))
        .into_bytes();
    Ok(())
}

/// Files for the orphan branch `uplink/hooks`, which `init` creates locally.
pub fn hooks_files(forge: Forge) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    match forge.family() {
        ForgeFamily::Github => collect_pack::<GithubHooksPack>(&mut files),
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Pack documentation for operators. It is rendered on the site and must not
/// land in the product repository, where it would replace the product README.
const PACK_README: &str = "README.md";

fn collect_pack<E: RustEmbed>(files: &mut Vec<(String, Vec<u8>)>) {
    for name in E::iter() {
        let rel = name.as_ref().replace('\\', "/");
        if rel == PACK_README {
            continue;
        }
        let Some(file) = E::get(name.as_ref()) else {
            continue;
        };
        files.push((rel, file.data.into_owned()));
    }
}

fn tooling_commit_message() -> String {
    format!(
        "{TOOLING_PATCH_TITLE}\n\n\
Workflows and the GitHub pull request template for this repository.\n\n\
{DEFAULT_CUTOFF}\n\n\
internal-only; not submitted upstream.\n"
    )
}

fn find_tooling_patch(queue: &QueueState, repo: &Path) -> Result<Option<String>> {
    if let Some(patch) = &queue.tooling {
        return Ok(Some(patch.id.clone()));
    }
    if let Some(patch) = queue
        .all_patches()
        .find(|p| p.kind.as_deref() == Some(TOOLING_PATCH_KIND))
    {
        return Ok(Some(patch.id.clone()));
    }
    // Legacy queues kept the tooling patch in internal[] without a kind. Only
    // that patch is migrated; an ordinary patch that edits an uplink workflow
    // is left where it is.
    for patch in queue
        .internal
        .iter()
        .filter(|p| p.title == TOOLING_PATCH_TITLE)
    {
        let Ok(rel) = patch_path(&patch.id) else {
            continue;
        };
        let path = repo.join(rel);
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path)?;
        if contents.contains(".github/workflows/uplink-") {
            return Ok(Some(patch.id.clone()));
        }
    }
    Ok(None)
}

struct SynthesizedPatch {
    formatted: String,
    from_sha: String,
    head_sha: String,
}

fn synthesize_pack_patch(
    repo: &Path,
    files: &[(String, Vec<u8>)],
    message: &str,
) -> Result<SynthesizedPatch> {
    let worktree = TempWorktree::add(repo, "uplink-tooling", "uplink/upstream")?;
    let dir = worktree.dir.as_path();
    let from_sha = git_ok(dir, &["rev-parse", "HEAD"])?;
    for (rel, bytes) in files {
        let dest = dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, bytes)?;
        git(dir, &["add", "-f", "--", rel], GitOpts::default())?;
    }
    if git_succeeds(dir, &["diff", "--cached", "--quiet"])? {
        return Err(Error::msg(
            "forge pack produced no product changes against uplink/upstream",
        ));
    }
    git(dir, &["commit", "-m", message], GitOpts::default())?;
    let head_sha = git_ok(dir, &["rev-parse", "HEAD"])?;
    let formatted = git_ok(dir, &["format-patch", "--full-index", "-1", "--stdout"])?;
    let formatted = if formatted.ends_with('\n') {
        formatted
    } else {
        format!("{formatted}\n")
    };
    Ok(SynthesizedPatch {
        formatted,
        from_sha,
        head_sha,
    })
}

#[cfg(test)]
mod find_tooling_tests {
    use super::*;

    fn patch(id: &str, title: &str) -> Patch {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
            "status": "queued",
            "dependsOn": [],
            "createdAt": "",
            "updatedAt": "",
            "source": {},
            "events": [],
        }))
        .unwrap()
    }

    #[test]
    fn only_the_legacy_tooling_patch_is_found_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        fs::create_dir_all(repo.join(".uplink/patches")).unwrap();
        for id in ["upl_aaaaaaaaaa", "upl_bbbbbbbbbb", "upl_cccccccccc"] {
            fs::write(
                repo.join(format!(".uplink/patches/{id}.patch")),
                "diff --git a/.github/workflows/uplink-pr.yml b/.github/workflows/uplink-pr.yml\n",
            )
            .unwrap();
        }
        let mut queue = QueueState::empty(Default::default());
        queue
            .internal
            .push(patch("upl_aaaaaaaaaa", "Tune PR workflow"));
        queue
            .upstream
            .push(patch("upl_bbbbbbbbbb", TOOLING_PATCH_TITLE));
        assert_eq!(find_tooling_patch(&queue, repo).unwrap(), None);

        queue
            .internal
            .push(patch("upl_cccccccccc", TOOLING_PATCH_TITLE));
        assert_eq!(
            find_tooling_patch(&queue, repo).unwrap().as_deref(),
            Some("upl_cccccccccc")
        );
    }
}

#[cfg(test)]
mod embed_tests {
    use super::*;

    #[test]
    fn github_pack_embeds_workflows_and_shared_pr_template() {
        let files = composed_files(Forge::Github).unwrap();
        let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(
            paths.contains(&".github/workflows/uplink-pr.yml"),
            "{paths:?}"
        );
        assert!(
            !paths.contains(&".github/workflows/uplink-assess.yml"),
            "{paths:?}"
        );
        assert!(
            !paths.contains(&".github/workflows/uplink-preflight.yml"),
            "{paths:?}"
        );
        let pr = files
            .iter()
            .find(|(p, _)| p == ".github/workflows/uplink-pr.yml")
            .unwrap();
        let text = String::from_utf8_lossy(&pr.1);
        assert!(text.contains("name: Uplink upstream assess"), "{text}");
        assert!(text.contains("name: Uplink upstream preflight"), "{text}");
        assert!(text.contains("git uplink assess"), "{text}");
        assert!(text.contains("git uplink preflight"), "{text}");
        // Removing uplink:internal-only must run the checks it skipped, and
        // no job may be skipped for an unrelated label: skipped is a pass.
        assert!(
            text.contains("types: [opened, synchronize, reopened, edited, labeled, unlabeled]"),
            "{text}"
        );
        // The one job that looks at the label is the rebase request, which
        // is not a check.
        for (key, job) in jobs_of(&text) {
            assert!(
                key == "rebase-request"
                    || (!job.contains("github.event.action")
                        && !job.contains("github.event.label")),
                "{key} must run for every label\n{job}"
            );
        }
        assert!(
            paths.contains(&".github/pull_request_template.md"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&".github/actions/install-git-uplink/action.yml"),
            "{paths:?}"
        );
    }

    #[test]
    fn install_action_defaults_to_the_version_of_this_binary() {
        let default = format!("VERSION=\"${{VERSION:-{}}}\"", env!("CARGO_PKG_VERSION"));
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            for (path, data) in &files {
                let text = String::from_utf8_lossy(data);
                assert!(!text.contains(VERSION_PLACEHOLDER), "{forge:?}: {path}");
                if path == INSTALL_ACTION {
                    assert!(text.contains(&default), "{forge:?}: {text}");
                    assert!(
                        text.contains("REPO=\"${REPO:-npetzall/git-uplink}\""),
                        "{forge:?}: {text}"
                    );
                }
            }
        }
    }

    #[test]
    fn github_packs_install_the_contrib_commit_script_their_submit_runs() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
            assert!(
                paths.contains(&".github/uplink/contrib_commit.py"),
                "{forge:?}: {paths:?}"
            );
            assert!(
                paths
                    .iter()
                    .all(|p| !p.contains("__pycache__") && !p.ends_with(".pyc")),
                "{forge:?}: {paths:?}"
            );
            let submit = files
                .iter()
                .find(|(p, _)| p == ".github/workflows/uplink-submit.yml")
                .unwrap();
            let text = String::from_utf8_lossy(&submit.1);
            assert!(
                text.contains("python3 .github/uplink/contrib_commit.py"),
                "{forge:?}"
            );
            assert!(!text.contains("submit \"$PATCH_ID\" --push"), "{forge:?}");
        }
    }

    #[test]
    fn pack_readme_is_not_installed_into_the_product_repo() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            assert!(
                files
                    .iter()
                    .all(|(p, _)| p != PACK_README && p != ".github/README.md"),
                "{forge:?} pack would overwrite the product README"
            );
        }
    }

    #[test]
    fn try_it_on_github_is_the_github_pack_without_the_sync_schedule() {
        let schedule = ".github/workflows/uplink-sync-schedule.yml";
        let mut base = composed_files(Forge::Github).unwrap();
        let text = |path: &str| {
            base.iter()
                .find(|(p, _)| p == path)
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("github pack is missing {path}"))
        };
        let scheduled = text(schedule);
        assert!(scheduled.contains("schedule:"), "{scheduled}");
        assert!(
            scheduled.contains("gh workflow run uplink-sync.yml"),
            "{scheduled}"
        );
        let sync = text(".github/workflows/uplink-sync.yml");
        assert!(sync.contains("workflow_dispatch:"), "{sync}");
        assert!(
            !sync.contains("schedule:"),
            "the schedule lives in its own file so an overlay can drop it\n{sync}"
        );

        // Anything else that differs must be a deliberate overlay file.
        base.retain(|(path, _)| path != schedule);
        assert_eq!(composed_files(Forge::TryItOnGithub).unwrap(), base);
    }

    #[test]
    fn submit_workflows_run_optional_hooks_before_to_upstream() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
            assert!(
                !paths.contains(&".github/uplink-assessment-hook.md"),
                "{forge:?} the guide lives on uplink/hooks now {paths:?}"
            );
            assert!(
                paths.contains(&".github/uplink-hooks-ruleset.json"),
                "{forge:?} {paths:?}"
            );
            let placeholder = files
                .iter()
                .find(|(p, _)| p == ".github/workflows/uplink-assessment-hook.yml")
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing assessment hook placeholder"));
            // GitHub checks dispatch inputs against the file on the default
            // branch, so the placeholder declares the real hook's inputs.
            let trigger = |workflow: &str| {
                let start = workflow.find("\non:\n").expect("on: block");
                let end = workflow.find("\npermissions:").expect("permissions: block");
                workflow[start..end].to_string()
            };
            let example = hooks_files(forge)
                .into_iter()
                .find(|(p, _)| p == ".github/workflows/uplink-assessment-hook-example.yml")
                .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing assessment hook example"));
            assert_eq!(
                trigger(&placeholder),
                trigger(&example),
                "{forge:?} placeholder and hook example must take the same inputs"
            );
            assert!(
                placeholder.contains("if: github.ref == 'refs/heads/main'"),
                "{forge:?} placeholder must not run as the hook\n{placeholder}"
            );
            assert!(
                placeholder.contains("refs/heads/uplink/hooks"),
                "{forge:?}\n{placeholder}"
            );
            let submit = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-submit.yml"))
                .unwrap();
            let text = String::from_utf8_lossy(&submit.1);
            assert!(
                text.contains("uses: $/.github/actions/uplink-assessment-hook"),
                "{forge:?}\n{text}"
            );
            assert!(text.contains("--store-extras"), "{forge:?}\n{text}");
            // Assess is its own step, and the packet stores its result:
            // the one the hook read too.
            assert!(
                text.contains("git uplink assess --patch \"$PATCH_ID\" --package \"$out\""),
                "{forge:?}\n{text}"
            );
            assert_eq!(
                text.matches("--assess-result \"$ASSESSMENT\"").count(),
                3,
                "{forge:?} every report takes the assessment of this run\n{text}"
            );
            // The binary decides whether dependencies are merged and whether
            // a stored hook result is current; the workflow only reads that.
            assert!(
                !text.contains("dependsOn") && !text.contains("patchIdStable"),
                "{forge:?}\n{text}"
            );
            assert!(
                text.contains("jq -r '.storedExtras.fresh == true'")
                    && text.contains("jq -r '.ok == true'"),
                "{forge:?}\n{text}"
            );
            assert!(
                text.contains(
                    "if: steps.assess.outputs.ok == 'true' && steps.assess.outputs.fresh != 'true'\n        uses: $/.github/actions/uplink-assessment-hook"
                ),
                "{forge:?} no hook run for findings or a current stored result\n{text}"
            );
            let action = files
                .iter()
                .find(|(p, _)| p == ".github/actions/uplink-assessment-hook/action.yml")
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing assessment hook action"));
            assert!(
                action.contains("gh workflow run uplink-assessment-hook.yml --ref uplink/hooks"),
                "{action}"
            );
            // The hook is told only which run holds its assessment package.
            assert_eq!(action.matches(" -f \"").count(), 1, "{action}");
            assert!(
                action.contains("-f \"caller_run_id=${caller}\""),
                "{action}"
            );
            assert!(
                example.contains("-n uplink-assessment") && example.contains("actions: read"),
                "{example}"
            );
            assert!(
                action.contains("::warning title=Uplink assessment hook failed::"),
                "{action}"
            );
            assert!(action.contains("00-uplink-hook-failed.md"), "{action}");
            assert!(
                !action.contains("gh run watch \"$run_id\" --exit-status\n"),
                "a failed hook must not fail the caller\n{action}"
            );
            let pr = files
                .iter()
                .find(|(p, _)| p == ".github/workflows/uplink-pr.yml")
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap();
            assert!(
                pr.contains("uses: $/.github/actions/uplink-assessment-hook"),
                "{forge:?}\n{pr}"
            );
            assert!(pr.contains("<!-- uplink:assessment -->"), "{forge:?}");
            assert!(pr.contains("--method PATCH"), "{forge:?}");
            assert!(!pr.contains("fingerprint"), "{forge:?}\n{pr}");
            // Both callers upload the package before they run the hook.
            for (name, workflow) in [("pr", pr.as_str()), ("submit", text.as_ref())] {
                let assess = workflow
                    .find("git uplink assess")
                    .unwrap_or_else(|| panic!("{forge:?} {name} runs no assess"));
                let upload = workflow
                    .find("name: uplink-assessment\n")
                    .unwrap_or_else(|| panic!("{forge:?} {name} uploads no package"));
                let hook = workflow
                    .find("uses: $/.github/actions/uplink-assessment-hook")
                    .unwrap();
                assert!(assess < upload && upload < hook, "{forge:?} {name}");
            }
            assert!(
                !pr.contains("gh pr comment"),
                "{forge:?} PR comments must be updated in place\n{pr}"
            );
            let import = files
                .iter()
                .find(|(p, _)| p == ".github/workflows/uplink-import.yml")
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap();
            assert!(
                !import.contains("--extra-dir") && !import.contains("gh run download"),
                "{forge:?} import must not store a result of the PR checks\n{import}"
            );
            assert!(action.contains("uplink-packet-extra"), "{action}");
            assert!(action.contains("run-id:"), "{action}");
            assert!(action.contains("continue-on-error: true"), "{action}");
            assert!(text.contains("needs: assess"), "{forge:?}");
            assert!(text.contains("needs: [packet, preflight]"), "{forge:?}");
            assert!(!text.contains("finalize:"), "{forge:?}\n{text}");
            // The hook runs in `assess`, which must not hold the queue lock;
            // only `packet` writes uplink/state.
            let assess_job = text.find("\n  assess:").expect("assess job");
            let packet_job = text.find("\n  packet:").expect("packet job");
            let submit_job = text.find("\n  submit:").expect("submit job");
            let lock = text.find("group: uplink-mutate").expect("uplink-mutate");
            assert_eq!(
                text.matches("group: uplink-mutate").count(),
                1,
                "{forge:?}\n{text}"
            );
            assert!(packet_job < lock && lock < submit_job, "{forge:?}\n{text}");
            let hook = text
                .find("uses: $/.github/actions/uplink-assessment-hook")
                .unwrap();
            assert!(assess_job < hook && hook < packet_job, "{forge:?}\n{text}");
            assert_eq!(
                text.matches("printf '%s\\n' \"$packet\" >> \"$GITHUB_STEP_SUMMARY\"")
                    .count(),
                1,
                "{forge:?} the packet is written to the summary once\n{text}"
            );
            assert!(text.contains("assessment.md"), "{forge:?}");
            assert!(
                paths.contains(&".github/workflows/uplink-pr.yml"),
                "{forge:?} {paths:?}"
            );
            assert!(
                !paths.contains(&".github/workflows/uplink-assess.yml"),
                "{forge:?} {paths:?}"
            );
            assert!(
                !paths.contains(&".github/workflows/uplink-preflight.yml"),
                "{forge:?} {paths:?}"
            );
        }
    }

    #[test]
    fn hooks_pack_holds_guides_stub_and_example() {
        let files = hooks_files(Forge::Github);
        let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".github/actions/uplink-toolchain-hook/action.yml",
                ".github/workflows/uplink-assessment-hook-example.yml",
                "assessment-hook.md",
                "preflight.sh",
                "toolchain-hook.md",
            ]
        );
        let text = |path: &str| {
            files
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap()
        };
        let script = text("preflight.sh");
        assert!(script.starts_with("#!/bin/sh\n"), "{script}");
        // `init` appends the seed command, so the stub must end on a new line
        // and run nothing by itself.
        assert!(script.ends_with("\nset -eu\n"), "{script}");
        let stub = text(".github/actions/uplink-toolchain-hook/action.yml");
        assert!(stub.contains("using: composite"), "{stub}");
        assert!(stub.contains("toolchain-hook.md"), "{stub}");
        let example = text(".github/workflows/uplink-assessment-hook-example.yml");
        assert!(example.contains("caller_run_id"), "{example}");
        assert!(example.contains("uplink-packet-extra"), "{example}");
        // The starter checks out the ref it runs on and may run from any branch.
        assert!(
            !example.contains("ref:") && !example.contains("github.ref"),
            "{example}"
        );
        assert!(
            !example.contains("\n  push:") && !example.contains("pull_request"),
            "the example must only run when dispatched\n{example}"
        );
        assert_eq!(hooks_files(Forge::TryItOnGithub), files);
    }

    /// The jobs of a workflow as (key, text).
    fn jobs_of(text: &str) -> Vec<(String, String)> {
        let body = text.split_once("\njobs:\n").expect("jobs:").1;
        let mut jobs: Vec<(String, String)> = Vec::new();
        for line in body.lines() {
            let key = line
                .strip_prefix("  ")
                .filter(|rest| !rest.starts_with([' ', '#']))
                .and_then(|rest| rest.strip_suffix(':'));
            match (key, jobs.last_mut()) {
                (Some(key), _) => jobs.push((key.to_string(), String::new())),
                (None, Some((_, job))) => {
                    job.push_str(line);
                    job.push('\n');
                }
                (None, None) => {}
            }
        }
        jobs
    }

    /// The steps of a job as text, each starting at its `name:` or `uses:`.
    fn steps_of(job: &str) -> Vec<&str> {
        job.split("\n      - ").skip(1).collect()
    }

    #[test]
    fn preflight_sh_runs_only_in_jobs_without_credentials() {
        let hook = "uses: $/.github/actions/uplink-toolchain-hook";
        // (workflow, job that runs preflight.sh, job that records, its command)
        let split = [
            (
                "uplink-import.yml",
                "preflight",
                "import",
                "git uplink add ",
            ),
            (
                "uplink-submit.yml",
                "preflight",
                "submit",
                "git uplink submit ",
            ),
            (
                "uplink-amend.yml",
                "complete-preflight",
                "complete",
                "git uplink amend ",
            ),
            (
                "uplink-transfer.yml",
                "start-preflight",
                "start",
                "git uplink transfer ",
            ),
            (
                "uplink-transfer.yml",
                "complete-preflight",
                "complete",
                "git uplink transfer ",
            ),
            (
                "uplink-resolve.yml",
                "preflight",
                "resolve",
                "git uplink resolve ",
            ),
            (
                "uplink-verify.yml",
                "preflight",
                "verify",
                "git uplink rebuild ",
            ),
            (
                "uplink-sync.yml",
                "accept-preflight",
                "apply",
                "git uplink accept-upstream ",
            ),
        ];
        // Jobs that only check: (workflow, job, step that runs preflight.sh).
        let checks = [
            ("uplink-pr.yml", "preflight", "name: Export preflight"),
            ("uplink-gate.yml", "validate", "name: Validate gated work"),
        ];
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let jobs = |workflow: &str| {
                let text = files
                    .iter()
                    .find(|(p, _)| p.ends_with(workflow))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| panic!("{forge:?} missing {workflow}"));
                jobs_of(&text)
            };
            let job = |workflow: &str, key: &str| {
                jobs(workflow)
                    .into_iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, text)| text)
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} has no job {key}"))
            };
            // The step that runs the script: no secret, the hook right before it.
            let script_step_is_clean = |workflow: &str, key: &str, name: &str| {
                let text = job(workflow, key);
                let steps = steps_of(&text);
                let i = steps
                    .iter()
                    .position(|step| step.contains(name))
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} {key} has no step {name:?}"));
                assert!(
                    steps[i - 1].contains(hook),
                    "{forge:?} {workflow} {key}: the toolchain hook must run right before {name:?}"
                );
                assert!(
                    !steps[i].contains("secrets.") && !steps[i].contains("_TOKEN"),
                    "{forge:?} {workflow} {key}: {name:?} runs preflight.sh and must hold no token\n{}",
                    steps[i]
                );
                assert!(
                    text.contains("persist-credentials: false")
                        && !text.contains("persist-credentials: true"),
                    "{forge:?} {workflow} {key} must not keep credentials in its checkout"
                );
                // The probe of an approved upstream has to fetch it: it may
                // mint the upstream token, read-only, and nothing else.
                let mints = text.matches("create-github-app-token").count();
                let upstream_only = key == "accept-preflight"
                    && mints == 1
                    && text.contains("id: upstream-app")
                    && text.contains("permission-contents: read")
                    && !text.contains("UPLINK_INTERNAL_APP");
                assert!(
                    (mints == 0 || upstream_only) && !text.contains("environment:"),
                    "{forge:?} {workflow} {key} must not mint a token or use an Environment"
                );
            };
            for (workflow, probe, record, command) in split {
                script_step_is_clean(workflow, probe, "name: Run preflight.sh");
                let probing = job(workflow, probe);
                assert!(
                    probing.contains("    permissions:\n      contents: read\n    outputs:")
                        && !probing.contains(": write"),
                    "{forge:?} {workflow} {probe} must be read-only\n{probing}"
                );
                let recording = job(workflow, record);
                assert!(
                    recording.contains(&format!("needs: {probe}"))
                        || recording.contains(&format!(", {probe}]")),
                    "{forge:?} {workflow} {record} must wait for {probe}"
                );
                assert!(
                    !recording.contains(hook),
                    "{forge:?} {workflow} {record} holds write tokens and must not run the toolchain hook"
                );
                assert!(
                    recording.contains(&format!(
                        "PREFLIGHT_REPORT: ${{{{ needs.{probe}.outputs.report }}}}"
                    )),
                    "{forge:?} {workflow} {record} must take the result of {probe}"
                );
                // Every call of the recording command takes the result.
                let calls: Vec<&str> = recording
                    .split(command)
                    .skip(1)
                    .map(|rest| rest.split(")\n").next().unwrap_or(rest))
                    .collect();
                assert!(
                    !calls.is_empty(),
                    "{forge:?} {workflow} {record}: no {command}"
                );
                for call in calls {
                    let call = call.split(" > ").next().unwrap_or(call);
                    assert!(
                        call.contains("--preflight-result \"$RUNNER_TEMP/uplink-preflight.json\""),
                        "{forge:?} {workflow} {record}: `{command}` would run preflight.sh itself\n{call}"
                    );
                }
            }
            for (workflow, key, name) in checks {
                script_step_is_clean(workflow, key, name);
            }
            // No other job runs a command that would run the script.
            for (path, bytes) in &files {
                if !path.starts_with(".github/workflows/") {
                    continue;
                }
                let text = String::from_utf8_lossy(bytes);
                for (key, body) in jobs_of(&text) {
                    let runs_script =
                        body.contains("git uplink preflight") || body.contains("--preflight-only");
                    let known = split
                        .iter()
                        .any(|(w, probe, ..)| path.ends_with(w) && *probe == key)
                        || checks
                            .iter()
                            .any(|(w, job, _)| path.ends_with(w) && *job == key);
                    assert_eq!(
                        runs_script, known,
                        "{forge:?} {path} job {key}: jobs that run preflight.sh are listed in this test"
                    );
                }
            }
            for (path, bytes) in &files {
                let text = String::from_utf8_lossy(bytes);
                for variable in [
                    "UPLINK_PREFLIGHT",
                    "UPLINK_REDACT_KEYWORDS",
                    "UPLINK_INTERNAL_DOMAINS",
                ] {
                    assert!(
                        !text.contains(variable),
                        "{forge:?} {path} still uses {variable}; it lives on uplink/hooks now"
                    );
                }
            }
            let wrapper = files
                .iter()
                .find(|(p, _)| p == ".github/actions/uplink-toolchain-hook/action.yml")
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing toolchain hook wrapper"));
            assert!(wrapper.contains("ref: uplink/hooks"), "{wrapper}");
            assert!(
                wrapper.contains("uses: ./.uplink-hooks/.github/actions/uplink-toolchain-hook"),
                "{wrapper}"
            );
            assert!(
                !wrapper.contains("continue-on-error"),
                "a failed toolchain hook must fail the job\n{wrapper}"
            );
            assert!(wrapper.contains("persist-credentials: false"), "{wrapper}");
        }
    }

    #[test]
    fn sync_passes_merged_prs_and_accepts_only_the_reviewed_upstream() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-sync.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing uplink-sync.yml"));
            let lookup = text
                .find("name: Find merged public pull requests")
                .unwrap_or_else(|| panic!("{forge:?} sync must look up merged PRs\n{text}"));
            let sync = text.find("git uplink sync \"${merged[@]}\"").unwrap();
            assert!(lookup < sync, "{forge:?}\n{text}");
            assert!(
                text.contains("merged+=(--merged-pr \"$item\")"),
                "{forge:?}"
            );
            assert!(
                text.contains("pulls/${number}") && !text.contains(".upstream.prUrl"),
                "{forge:?} the PR is looked up by number on the configured upstream\n{text}"
            );
            assert!(
                text.contains("git uplink accept-upstream --sha \"$PENDING_SHA\""),
                "{forge:?} apply must name the reviewed upstream\n{text}"
            );
            assert!(
                text.contains("PENDING_SHA: ${{ needs.inspect.outputs.pending_sha }}"),
                "{forge:?}\n{text}"
            );
        }
    }

    #[test]
    fn auto_submit_dispatches_what_the_binary_says_is_ready_only_when_the_variable_is_set() {
        let action = "uses: $/.github/actions/uplink-dispatch-submit";
        // (workflow, job, the step that writes the queue)
        let sites = [
            ("uplink-import.yml", "import", "add"),
            ("uplink-sync.yml", "inspect", "sync"),
            ("uplink-sync.yml", "apply", "accept"),
            ("uplink-transfer.yml", "start", "transfer"),
            ("uplink-transfer.yml", "complete", "transfer"),
            ("uplink-resolve.yml", "resolve", "resolve"),
        ];
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = |path: &str| {
                files
                    .iter()
                    .find(|(p, _)| p.ends_with(path))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| panic!("{forge:?} missing {path}"))
            };
            for (workflow, job_key, step_id) in sites {
                let workflow_text = text(workflow);
                let jobs = jobs_of(&workflow_text);
                let (_, job) = jobs
                    .iter()
                    .find(|(key, _)| key == job_key)
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} has no job {job_key}"));
                assert!(
                    job.contains("      actions: write\n"),
                    "{forge:?} {workflow} {job_key} dispatches with the Actions token\n{job}"
                );
                let steps = steps_of(job);
                let dispatch = steps
                    .iter()
                    .position(|step| step.contains(action))
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} {job_key} does not dispatch"));
                let written = steps
                    .iter()
                    .position(|step| step.contains(&format!("id: {step_id}\n")))
                    .unwrap_or_else(|| {
                        panic!("{forge:?} {workflow} {job_key} has no step {step_id}")
                    });
                assert!(
                    written < dispatch,
                    "{forge:?} {workflow} {job_key} dispatches after the queue is pushed\n{job}"
                );
                let last = steps[dispatch];
                assert!(
                    last.contains(&format!(
                        "if: ${{{{ vars.UPLINK_AUTO_SUBMIT == 'true' && steps.{step_id}.outputs.ready_to_submit != '' && steps.{step_id}.outputs.ready_to_submit != '[]' }}}}"
                    )),
                    "{forge:?} {workflow} {job_key} is off unless the variable is true\n{last}"
                );
                assert!(
                    last.contains(&format!(
                        "patch-ids: ${{{{ steps.{step_id}.outputs.ready_to_submit }}}}"
                    )),
                    "{forge:?} {workflow} {job_key}\n{last}"
                );
                let writer = steps
                    .iter()
                    .find(|step| step.contains(&format!("id: {step_id}\n")))
                    .unwrap_or_else(|| {
                        panic!("{forge:?} {workflow} {job_key} has no step {step_id}")
                    });
                let pushed = writer
                    .find("git push origin uplink/state")
                    .or_else(|| writer.find("git uplink add "))
                    .unwrap();
                let read = writer.find(".readyToSubmit // []").unwrap_or_else(|| {
                    panic!("{forge:?} {workflow} {job_key} reads readyToSubmit\n{writer}")
                });
                assert!(pushed < read, "{forge:?} {workflow} {job_key}\n{writer}");
                // The binary decides what is ready; the workflow only reads that.
                assert!(
                    !workflow_text.contains("dependsOn"),
                    "{forge:?} {workflow}\n{workflow_text}"
                );
            }
            let dispatch = text(".github/actions/uplink-dispatch-submit/action.yml");
            assert!(
                dispatch
                    .contains("gh workflow run \"Uplink submit\" --ref main -f \"patch_id=$id\""),
                "{dispatch}"
            );
            assert!(dispatch.contains("gh run cancel \"$run_id\""), "{dispatch}");
            assert!(
                dispatch.contains("PATCH_IDS: ${{ inputs.patch-ids }}")
                    && !dispatch.split("run: |").nth(1).unwrap().contains("${{"),
                "ids reach the script through the environment\n{dispatch}"
            );
        }
    }

    #[test]
    fn import_runs_only_for_pull_requests_merged_into_main() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-import.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing uplink-import.yml"));
            assert!(
                text.contains(
                    "  pull_request:\n    types: [closed]\n    branches:\n      - main\n"
                ),
                "{forge:?} import must be limited to main\n{text}"
            );
            assert!(
                text.contains("BASE_REF: ${{ github.event.pull_request.base.ref }}")
                    && text.contains("--base-branch \"$BASE_REF\""),
                "{forge:?} import must tell add where the PR merged\n{text}"
            );
        }
    }

    #[test]
    fn submit_approves_only_the_packet_of_its_own_run() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-submit.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing uplink-submit.yml"));
            for needle in [
                "review_token: ${{ steps.packet.outputs.review_token }}",
                "REVIEW_TOKEN: ${{ needs.packet.outputs.review_token }}",
                "git uplink approve \"$PATCH_ID\" --reviewed \"$REVIEW_TOKEN\"",
                "/blob/${{ needs.packet.outputs.state_sha }}/.uplink/reports/",
            ] {
                assert!(text.contains(needle), "{forge:?} missing {needle}\n{text}");
            }
            assert!(
                !text.contains("git uplink approve \"$PATCH_ID\")")
                    && !text.contains("/blob/uplink/state/"),
                "{forge:?} approval must not follow the moving branch\n{text}"
            );
        }
    }

    #[test]
    fn abandon_contrib_is_detached_from_uplink_mutate() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-abandon.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing uplink-abandon.yml"));
            assert!(
                text.contains("environment:\n      name: abandon-contrib"),
                "{forge:?} must wait on abandon-contrib\n{text}"
            );
            assert!(
                !text.contains("group: uplink-mutate"),
                "{forge:?} abandon must not take uplink-mutate\n{text}"
            );
            assert!(
                text.contains("workflow_dispatch:"),
                "{forge:?} abandon must be workflow_dispatch\n{text}"
            );
        }
    }

    #[test]
    fn gated_branch_sidecars_load_from_default_branch() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
            assert!(
                paths.contains(&".github/uplink-pack-files-ruleset.json"),
                "{forge:?} {paths:?}"
            );
            for name in [
                "uplink-gate.yml",
                "uplink-resolve.yml",
                "uplink-transfer.yml",
                "uplink-amend.yml",
            ] {
                let text = files
                    .iter()
                    .find(|(p, _)| p.ends_with(name))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| panic!("{forge:?} missing {name}"));
                assert!(
                    text.contains("pull_request_target:"),
                    "{forge:?} {name} must use pull_request_target\n{text}"
                );
                assert!(
                    text.contains("zizmor: ignore[dangerous-triggers]"),
                    "{forge:?} {name} must ignore dangerous-triggers\n{text}"
                );
                assert!(
                    !text.contains("\n  pull_request:\n"),
                    "{forge:?} {name} must not use pull_request\n{text}"
                );
            }
            let gate = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-gate.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap();
            assert!(
                gate.contains("uplink-.*\\.yml"),
                "{forge:?} gate must refuse pack-file diffs\n{gate}"
            );
            // The gate and the ruleset refuse the same paths.
            for path in ["\\.github/uplink/", "'^\\.uplink/'"] {
                assert!(
                    gate.contains(path),
                    "{forge:?} gate must refuse {path}\n{gate}"
                );
            }
            let ruleset = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-pack-files-ruleset.json"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap();
            for path in [
                ".github/workflows/uplink-*.yml",
                ".github/actions/install-git-uplink/**",
                ".github/actions/uplink-*/**",
                ".github/uplink/**",
                ".uplink/**",
            ] {
                assert!(
                    ruleset.contains(&format!("\"{path}\"")),
                    "{forge:?} ruleset must restrict {path}\n{ruleset}"
                );
            }
            assert!(
                gate.contains("contents: read"),
                "{forge:?} gate must be contents: read\n{gate}"
            );
            assert!(
                gate.contains("git uplink assess --patch \"${BASE_REF#uplink/conflict/}\""),
                "{forge:?} gate must assess conflict resolutions\n{gate}"
            );
            assert!(
                gate.contains("\"uplink/amend/**\"")
                    && gate.contains("git uplink assess --patch \"${BASE_REF#uplink/amend/}\""),
                "{forge:?} gate must assess amends\n{gate}"
            );
            assert!(
                gate.contains("is_internal \"${BASE_REF#uplink/conflict/}\"")
                    && gate.contains("is_internal \"${BASE_REF#uplink/amend/}\""),
                "{forge:?} gate must skip assess for internal-only patches\n{gate}"
            );
            // Only the job that writes the preflight comment may write, and
            // it never sees the gated tree.
            for (key, job) in jobs_of(&gate) {
                if key == "preflight-comment" {
                    assert!(
                        !job.contains("pull_request.head") && !job.contains("git uplink"),
                        "{forge:?} gate {key} must not check out or run the gated tree\n{job}"
                    );
                    assert!(
                        job.contains("ref: ${{ github.event.repository.default_branch }}"),
                        "{forge:?} gate {key} must take its action from the default branch\n{job}"
                    );
                } else {
                    assert!(
                        !job.contains(": write"),
                        "{forge:?} gate {key} must not hold a write permission\n{job}"
                    );
                }
            }
        }
    }

    #[test]
    fn preflight_verdict_is_commented_on_every_run() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = |path: &str| {
                files
                    .iter()
                    .find(|(p, _)| p.ends_with(path))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| panic!("{forge:?} missing {path}"))
            };
            let action = text(".github/actions/uplink-preflight-comment/action.yml");
            assert!(action.contains("<!-- uplink:preflight -->"), "{action}");
            // One comment: update it, else create it. No outcome decides.
            assert!(
                action.contains("--method PATCH") && action.contains("--method POST"),
                "{action}"
            );
            assert!(!action.contains("elif"), "{action}");
            assert!(action.contains(".comment // .message"), "{action}");
            for (workflow, needs) in [
                ("uplink-pr.yml", "needs: preflight"),
                ("uplink-gate.yml", "needs: validate"),
            ] {
                let job = jobs_of(&text(workflow))
                    .into_iter()
                    .find(|(key, _)| key == "preflight-comment")
                    .map(|(_, job)| job)
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} has no preflight-comment"));
                assert!(job.contains(needs), "{forge:?} {workflow}\n{job}");
                assert!(
                    job.contains("uses: $/.github/actions/uplink-preflight-comment")
                        && job.contains("pull-requests: write")
                        && job.contains("always()")
                        && job.contains("persist-credentials: false"),
                    "{forge:?} {workflow}\n{job}"
                );
                assert!(
                    !job.contains("uplink-toolchain-hook") && !job.contains("git uplink"),
                    "{forge:?} {workflow}: the job that comments must not run preflight.sh\n{job}"
                );
            }
            // The gate hands over every preflight it runs.
            let gate = text("uplink-gate.yml");
            assert_eq!(
                gate.matches("git uplink preflight").count(),
                1,
                "{forge:?} gate runs preflight in one place, with --json\n{gate}"
            );
            assert!(
                gate.contains("report=$(git uplink preflight --json \"$@\") || true"),
                "{gate}"
            );
        }
    }
    #[test]
    fn a_rebuilt_main_is_told_to_open_pull_requests_and_rebased_on_request() {
        let action = "uses: $/.github/actions/uplink-rebase-comment";
        // Every job that pushes a rebuilt main.
        let sites = [
            ("uplink-import.yml", "import"),
            ("uplink-sync.yml", "inspect"),
            ("uplink-sync.yml", "apply"),
            ("uplink-transfer.yml", "start"),
            ("uplink-transfer.yml", "complete"),
            ("uplink-resolve.yml", "resolve"),
            ("uplink-amend.yml", "complete"),
        ];
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = |path: &str| {
                files
                    .iter()
                    .find(|(p, _)| p.ends_with(path))
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_else(|| panic!("{forge:?} missing {path}"))
            };
            let job = |workflow: &str, key: &str| {
                jobs_of(&text(workflow))
                    .into_iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, job)| job)
                    .unwrap_or_else(|| panic!("{forge:?} {workflow} has no job {key}"))
            };
            for (workflow, key) in sites {
                let job = job(workflow, key);
                assert!(
                    job.contains("git push origin main --force")
                        || job.contains("git push origin HEAD:main --force")
                        || job.contains("git uplink rebuild --push"),
                    "{forge:?} {workflow} {key} pushes main\n{job}"
                );
                let last = *steps_of(&job).last().unwrap();
                // Last, and never the reason the job fails: main is pushed.
                assert!(
                    last.contains(action)
                        && last.contains("continue-on-error: true")
                        && last.contains("token: ${{ secrets.GITHUB_TOKEN }}")
                        && last.contains("auto-rebase: ${{ vars.UPLINK_AUTO_REBASE }}"),
                    "{forge:?} {workflow} {key}\n{last}"
                );
                assert!(
                    job.contains("      pull-requests: write\n")
                        && job.contains("      actions: write\n"),
                    "{forge:?} {workflow} {key}\n{job}"
                );
            }

            let comment = text(".github/actions/uplink-rebase-comment/action.yml");
            assert!(
                comment.contains("<!-- uplink:rebase needs ${fork} -->"),
                "{comment}"
            );
            assert!(comment.contains("<!-- uplink:rebase done -->"), "{comment}");
            // The plan comes from the binary, for a head that is only fetched.
            assert!(
                comment.contains("git uplink rebase --plan --head \"$oid\" --json"),
                "{comment}"
            );
            assert!(
                !comment.contains("git checkout") && !comment.contains("git switch"),
                "{comment}"
            );
            // Dispatch only with the variable, and only for a comment this
            // run wrote: a rebase that stopped is not asked for again.
            assert!(
                comment.contains(
                    "if [ \"$changed\" = \"true\" ] && [ \"$AUTO_REBASE\" = \"true\" ]; then"
                ),
                "{comment}"
            );
            assert_eq!(comment.matches("gh workflow run").count(), 1, "{comment}");

            let hint = job("uplink-pr.yml", "rebase-hint");
            assert!(
                hint.contains(action)
                    && hint.contains("pr-number: ${{ github.event.pull_request.number }}")
                    && hint.contains("persist-credentials: false")
                    && !hint.contains("\n    if:"),
                "{forge:?}\n{hint}"
            );
            let request = job("uplink-pr.yml", "rebase-request");
            assert!(
                request.contains(
                    "if: ${{ github.event.action == 'labeled' && github.event.label.name == 'uplink:rebase' }}"
                ),
                "{forge:?}\n{request}"
            );
            let unlabel = request.find("labels/uplink%3Arebase").unwrap();
            let dispatch = request.find("gh workflow run \"Uplink rebase\"").unwrap();
            assert!(unlabel < dispatch, "{forge:?}\n{request}");
            assert!(!request.contains("secrets.UPLINK"), "{forge:?}\n{request}");

            let rebase = text("uplink-rebase.yml");
            assert!(rebase.contains("name: Uplink rebase\n"), "{rebase}");
            assert!(!rebase.contains("group: uplink-mutate"), "{rebase}");
            // The pull request number is the only input: the commit to
            // rebase from is worked out in the job.
            assert_eq!(
                rebase.matches("inputs.").count(),
                rebase.matches("inputs.pr_number").count(),
                "{rebase}"
            );
            assert!(
                rebase.contains("git uplink rebase --plan --head \"$old\" --json"),
                "{rebase}"
            );
            // The branch's files stay out of the checkout, and a push made
            // since the head was read wins.
            assert!(
                rebase.contains("git worktree add --quiet --detach \"$tree\" \"$old\"")
                    && rebase.contains("-c core.hooksPath=/dev/null"),
                "{rebase}"
            );
            assert!(
                rebase.contains("\"--force-with-lease=refs/heads/${HEAD_REF}:${old}\""),
                "{rebase}"
            );
            assert!(
                !rebase.contains(" --force\n") && !rebase.contains(" --force "),
                "{rebase}"
            );
            assert!(
                rebase.contains("git check-ref-format --branch \"$HEAD_REF\""),
                "{rebase}"
            );
        }
    }

    #[test]
    fn a_verify_that_passes_pushes_nothing() {
        for forge in [Forge::Github, Forge::TryItOnGithub] {
            let files = composed_files(forge).unwrap();
            let text = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-verify.yml"))
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_else(|| panic!("{forge:?} missing uplink-verify.yml"));
            let job = jobs_of(&text)
                .into_iter()
                .find(|(key, _)| key == "verify")
                .map(|(_, job)| job)
                .unwrap_or_else(|| panic!("{forge:?} uplink-verify.yml has no job verify"));
            // It rebuilds main to the tree main has, so main is never pushed,
            // and no open pull request is told that main was rebuilt.
            assert!(
                !job.contains("main --force")
                    && !job.contains("rebuild --push")
                    && !text.contains("uplink-rebase-comment"),
                "{forge:?}\n{job}"
            );
            // The rebuild records origin's main as replaced on uplink/state,
            // which Uplink rebase reads. A pass leaves before any push; only
            // a conflict, which records no rebuild, is pushed.
            let passed = job
                .find("if ! jq -e '.conflict != null' <<<\"$rebuild_json\"")
                .unwrap_or_else(|| panic!("{forge:?} verify does not branch on the conflict"));
            let left = passed + job[passed..].find("exit 0").expect("a pass exits");
            let pushes: Vec<usize> = job.match_indices("git push").map(|(at, _)| at).collect();
            assert!(
                !pushes.is_empty() && pushes.iter().all(|at| *at > left),
                "{forge:?} verify pushes before it knows the rebuild stopped\n{job}"
            );
        }
    }
}
