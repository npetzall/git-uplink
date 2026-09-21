use std::fs;
use std::path::Path;

use rust_embed::RustEmbed;

use crate::assess::assess_from_message;
use crate::error::{Error, Result};
use crate::git::{GitOpts, git, git_ok};
use crate::queue::{
    add_event, get_patch, patch_path, read_queue as read_queue_file,
    write_queue as write_queue_file,
};
use crate::repo::{
    commit_queue, ensure_uplink_dirs, ensure_upstream_ref, has_ref, new_patch_id,
    stable_patch_id_from_contents, stamp,
};
use crate::types::{
    DEFAULT_CUTOFF, Forge, ForgeFamily, Patch, PatchSource, QueueState, TOOLING_PATCH_KIND,
    TOOLING_PATCH_TITLE,
};

#[derive(RustEmbed)]
#[folder = "templates/ghec"]
struct GhecPack;

#[derive(RustEmbed)]
#[folder = "templates/example-github"]
struct ExampleGithubPack;

#[derive(RustEmbed)]
#[folder = "templates/github"]
struct GithubFamilyPack;

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
            "queue.json has no forge. Re-run `git uplink init --upgrade --forge ghec` \
(or --forge example-github) to record it.",
        )
    })?;
    let files = composed_files(forge)?;
    if files.is_empty() {
        return Err(Error::msg(format!(
            "embedded forge pack for {forge} is empty"
        )));
    }

    let existing_id = find_tooling_patch(&queue, repo)?;
    let SynthesizedPatch {
        formatted,
        from_sha,
        head_sha,
    } = synthesize_pack_patch(repo, &files)?;
    let new_stable = stable_patch_id_from_contents(repo, &formatted)?;

    if let Some(id) = &existing_id {
        let current = get_patch(&queue, id)?;
        if current.patch_id_stable.as_deref() == Some(new_stable.as_str()) {
            return Ok(ToolingRefresh { changed: false });
        }
    }

    ensure_uplink_dirs(repo)?;
    let mut queue = read_queue_file(repo)?;
    let (id, created) = if let Some(id) = existing_id {
        (id, false)
    } else {
        (new_patch_id(), true)
    };

    let patch_rel = patch_path(&id)?;
    fs::write(repo.join(&patch_rel), &formatted)?;

    let message = tooling_commit_message();
    let assess = assess_from_message(
        repo,
        &queue,
        &from_sha,
        &head_sha,
        &message,
        Some(TOOLING_PATCH_TITLE),
        "internal-only",
    )?;

    if created {
        let created_at = stamp();
        let mut patch = Patch {
            id: id.clone(),
            title: TOOLING_PATCH_TITLE.into(),
            commit_message: String::new(),
            status: "queued".into(),
            depends_on: Vec::new(),
            created_at: created_at.clone(),
            updated_at: created_at,
            patch_id_stable: Some(new_stable.clone()),
            source: PatchSource {
                note: Some(format!("forge {forge}")),
                ..Default::default()
            },
            assess: Some(assess.clone()),
            upstream: None,
            merged: None,
            conflict: None,
            approvals: Vec::new(),
            events: Vec::new(),
            kind: Some(TOOLING_PATCH_KIND.into()),
        };
        patch.commit_message = assess.commit_message.clone();
        add_event(
            &mut patch,
            "created",
            format!("Installed {forge} forge pack as internal-only tooling"),
        );
        queue.tooling = Some(patch);
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: add {id} {TOOLING_PATCH_TITLE}"))?;
    } else {
        if queue.tooling.as_ref().map(|p| p.id.as_str()) != Some(id.as_str()) {
            if let Some(idx) = queue.internal.iter().position(|p| p.id == id) {
                queue.tooling = Some(queue.internal.remove(idx));
            } else if let Some(idx) = queue.upstream.iter().position(|p| p.id == id) {
                queue.tooling = Some(queue.upstream.remove(idx));
            }
        }
        if let Some(patch) = queue.tooling.as_mut() {
            patch.kind = Some(TOOLING_PATCH_KIND.into());
            patch.status = "queued".into();
            patch.conflict = None;
            patch.patch_id_stable = Some(new_stable);
            patch.assess = Some(assess.clone());
            patch.commit_message = assess.commit_message.clone();
            add_event(
                patch,
                "upgraded",
                format!("Refreshed {forge} forge pack from embedded templates"),
            );
        }
        write_queue_file(repo, &queue)?;
        commit_queue(repo, &format!("uplink: upgrade {id} {TOOLING_PATCH_TITLE}"))?;
    }

    Ok(ToolingRefresh { changed: true })
}

pub fn composed_files(forge: Forge) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    match forge {
        Forge::Ghec => collect_pack::<GhecPack>(&mut files),
        Forge::ExampleGithub => collect_pack::<ExampleGithubPack>(&mut files),
    }
    match forge.family() {
        ForgeFamily::Github => {
            for name in GithubFamilyPack::iter() {
                let rel = name.as_ref().replace('\\', "/");
                let Some(file) = GithubFamilyPack::get(name.as_ref()) else {
                    continue;
                };
                files.push((format!(".github/{rel}"), file.data.into_owned()));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    Ok(files)
}

fn collect_pack<E: RustEmbed>(files: &mut Vec<(String, Vec<u8>)>) {
    for name in E::iter() {
        let rel = name.as_ref().replace('\\', "/");
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
    for patch in queue.internal.iter().chain(queue.upstream.iter()) {
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

fn synthesize_pack_patch(repo: &Path, files: &[(String, Vec<u8>)]) -> Result<SynthesizedPatch> {
    let original = git_ok(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let original_sha = git_ok(repo, &["rev-parse", "HEAD"])?;
    let synthesized = (|| -> Result<SynthesizedPatch> {
        git(
            repo,
            &["checkout", "--quiet", "--detach", "uplink/upstream"],
            GitOpts::default(),
        )?;
        let from_sha = git_ok(repo, &["rev-parse", "HEAD"])?;
        for (rel, bytes) in files {
            let dest = repo.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, bytes)?;
            git(repo, &["add", "-f", "--", rel], GitOpts::default())?;
        }
        let staged = git(
            repo,
            &["diff", "--cached", "--quiet"],
            GitOpts {
                allow_fail: true,
                ..GitOpts::default()
            },
        )?;
        if staged.code == 0 {
            return Err(Error::msg(
                "forge pack produced no product changes against uplink/upstream",
            ));
        }
        git(
            repo,
            &["commit", "-m", &tooling_commit_message()],
            GitOpts::default(),
        )?;
        let head_sha = git_ok(repo, &["rev-parse", "HEAD"])?;
        let formatted = git_ok(repo, &["format-patch", "--full-index", "-1", "--stdout"])?;
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
    })();

    if original != "HEAD" {
        git(
            repo,
            &["checkout", "-f", "--quiet", &original],
            GitOpts::default(),
        )?;
    } else {
        git(
            repo,
            &["checkout", "-f", "--quiet", &original_sha],
            GitOpts::default(),
        )?;
    }
    synthesized
}

#[cfg(test)]
mod embed_tests {
    use super::*;

    #[test]
    fn ghec_pack_embeds_workflows_and_shared_pr_template() {
        let files = composed_files(Forge::Ghec).unwrap();
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
        assert!(
            paths.contains(&".github/pull_request_template.md"),
            "{paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("install-git-uplink")),
            "{paths:?}"
        );
    }

    #[test]
    fn example_github_pack_embeds_install_action_and_shared_pr_template() {
        let files = composed_files(Forge::ExampleGithub).unwrap();
        let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert!(
            paths.contains(&".github/actions/install-git-uplink/action.yml"),
            "{paths:?}"
        );
        assert!(
            paths.contains(&".github/pull_request_template.md"),
            "{paths:?}"
        );
        let ghec_pr = composed_files(Forge::Ghec)
            .unwrap()
            .into_iter()
            .find(|(p, _)| p == ".github/pull_request_template.md")
            .unwrap()
            .1;
        let example_pr = files
            .into_iter()
            .find(|(p, _)| p == ".github/pull_request_template.md")
            .unwrap()
            .1;
        assert_eq!(ghec_pr, example_pr);
    }

    #[test]
    fn submit_workflows_run_optional_hooks_before_to_upstream() {
        for forge in [Forge::Ghec, Forge::ExampleGithub] {
            let files = composed_files(forge).unwrap();
            let paths: Vec<_> = files.iter().map(|(p, _)| p.as_str()).collect();
            assert!(
                !paths
                    .iter()
                    .any(|p| p.ends_with("uplink-assessment-hook.yml")),
                "{forge:?} must not embed company assessment hook: {paths:?}"
            );
            let submit = files
                .iter()
                .find(|(p, _)| p.ends_with("uplink-submit.yml"))
                .unwrap();
            let text = String::from_utf8_lossy(&submit.1);
            assert!(
                text.contains("uplink-assessment-hook.yml"),
                "{forge:?}\n{text}"
            );
            assert!(text.contains("uplink-packet-extra"), "{forge:?}");
            assert!(text.contains("run-id:"), "{forge:?}");
            assert!(text.contains("needs: finalize"), "{forge:?}");
            assert!(text.contains("assessment.md"), "{forge:?}");
            assert!(text.contains("continue-on-error: true"), "{forge:?}");
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
}
