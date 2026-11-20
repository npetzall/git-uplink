use super::{PACK_GITHUB, PACK_GITHUB_HOOKS, PACK_TRY_IT_ON_GITHUB, WEB, get};
use std::process::Command;

/// The files git tracks under `folder`, relative to it, or `None` outside a
/// work tree or when it tracks none there (`build.rs` then embeds them all).
fn tracked(folder: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--", folder])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let prefix = format!("{folder}/");
    let files: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .split('\0')
        .filter_map(|name| name.strip_prefix(&prefix))
        .map(str::to_string)
        .collect();
    (!files.is_empty()).then_some(files)
}

#[test]
fn packs_embed_only_tracked_files() {
    for (folder, table) in [
        ("templates/github", PACK_GITHUB),
        ("templates/github-hooks", PACK_GITHUB_HOOKS),
        ("templates/try-it-on-github", PACK_TRY_IT_ON_GITHUB),
    ] {
        let Some(files) = tracked(folder) else {
            eprintln!("skipped: {folder} is not tracked in a git work tree");
            continue;
        };
        for (path, _) in table {
            assert!(
                files.iter().any(|file| file == path),
                "{folder}/{path} is embedded but not tracked by git"
            );
        }
    }
}

#[test]
fn tables_are_sorted_and_use_forward_slashes() {
    for table in [WEB, PACK_GITHUB, PACK_GITHUB_HOOKS, PACK_TRY_IT_ON_GITHUB] {
        assert!(!table.is_empty());
        assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(table.iter().all(|(path, _)| !path.contains('\\')));
    }
}

#[test]
fn packs_include_dotfiles() {
    assert!(
        PACK_GITHUB
            .iter()
            .any(|(path, _)| path.starts_with(".github/"))
    );
    assert!(
        PACK_GITHUB_HOOKS
            .iter()
            .any(|(path, _)| path.starts_with(".github/"))
    );
}

#[test]
fn get_finds_present_paths_only() {
    assert!(get(WEB, "index.html").is_some_and(|bytes| !bytes.is_empty()));
    assert!(get(WEB, "missing.html").is_none());
}
