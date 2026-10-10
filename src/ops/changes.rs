use std::collections::BTreeSet;

use super::*;

/// The commit a rebuilt branch is compared against.
#[derive(Debug, Clone)]
pub struct ChangeBase {
    /// How to name it in a git command.
    pub rev: String,
    pub sha: String,
}

impl ChangeBase {
    pub(super) fn of(repo: &Path, rev: &str) -> Result<Self> {
        Ok(Self {
            rev: rev.to_string(),
            sha: rev_parse(repo, rev)?,
        })
    }

    /// Origin's company branch, which a push of the rebuilt one replaces.
    /// A clone that does not know it compares against its own company
    /// branch as it is now.
    pub(super) fn of_company(repo: &Path, company_branch: &str) -> Result<Option<Self>> {
        let tracking = format!("{COMPANY_REMOTE}/{company_branch}");
        if has_ref(repo, &tracking)? {
            return Self::of(repo, &tracking).map(Some);
        }
        if has_ref(repo, company_branch)? {
            return Ok(Some(Self::of(repo, company_branch)?.pinned(repo)?));
        }
        Ok(None)
    }

    /// Named by its commit, for a base whose ref is about to move.
    pub(super) fn pinned(mut self, repo: &Path) -> Result<Self> {
        self.rev = git_ok(repo, &["rev-parse", "--short", &self.sha])?;
        Ok(self)
    }
}

/// The files a rebuilt branch changes against [`ChangeBase`], split into the
/// ones the tooling patch owns and the rest.
#[derive(Debug, Clone)]
pub struct MainChanges {
    /// The base, as named in a git command.
    pub base: String,
    pub tooling: Vec<String>,
    /// Status letter of `git diff --name-status` and path.
    pub product: Vec<(char, String)>,
}

/// A queued patch that changes files of the tooling pack.
#[derive(Debug, Clone)]
pub struct ToolingOverride {
    pub id: String,
    pub title: String,
    pub files: Vec<String>,
}

/// The paths a patch (format-patch text) touches, both names of a rename.
pub(super) fn patch_paths(patch: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    // The message ends at `---`; a `diff --git` line quoted in it is no file.
    for line in patch.lines().skip_while(|line| *line != "---") {
        let Some(names) = line.strip_prefix("diff --git a/") else {
            continue;
        };
        if let Some((old, new)) = names.rsplit_once(" b/") {
            paths.insert(old.to_string());
            paths.insert(new.to_string());
        }
    }
    paths
}

/// The paths the stored patch `id` touches.
pub(super) fn stored_patch_paths(repo: &Path, id: &str) -> Result<BTreeSet<String>> {
    let text = fs::read_to_string(repo.join(patch_path(id)?)).unwrap_or_default();
    Ok(patch_paths(&text))
}

/// The paths the tooling patch owns: the ones its stored patch touches.
pub(super) fn tooling_paths(repo: &Path, queue: &QueueState) -> Result<BTreeSet<String>> {
    match &queue.tooling {
        Some(patch) => stored_patch_paths(repo, &patch.id),
        None => Ok(BTreeSet::new()),
    }
}

/// The paths the tooling commit under `rev` touches. That is the pack `rev`
/// was built with, which can hold files a newer pack dropped.
fn tooling_commit_paths(repo: &Path, queue: &QueueState, rev: &str) -> Result<BTreeSet<String>> {
    let Some(tooling) = &queue.tooling else {
        return Ok(BTreeSet::new());
    };
    if !has_ref(repo, UPSTREAM_REF)? {
        return Ok(BTreeSet::new());
    }
    let log = git_ok(
        repo,
        &[
            "log",
            "--first-parent",
            "--format=%H %(trailers:key=Uplink-Patch-Id,valueonly,separator=%x2C)",
            rev,
            "--not",
            UPSTREAM_REF,
        ],
    )?;
    let commit = log.lines().find_map(|line| {
        let (sha, ids) = line.split_once(' ')?;
        (ids.trim() == tooling.id).then_some(sha)
    });
    let Some(commit) = commit else {
        return Ok(BTreeSet::new());
    };
    let listing = git_ok(
        repo,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            commit,
        ],
    )?;
    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// What `new_rev` changes against `base`. A file of the tooling pack, the
/// one `new_rev` or the one `base` was built with, is tooling; every other
/// file is product code.
pub(super) fn main_changes(
    repo: &Path,
    queue: &QueueState,
    base: &ChangeBase,
    new_rev: &str,
) -> Result<MainChanges> {
    let mut owned = tooling_paths(repo, queue)?;
    owned.extend(tooling_commit_paths(repo, queue, &base.sha)?);
    let listing = git_ok(
        repo,
        &[
            "diff",
            "--name-status",
            "--no-renames",
            "-z",
            &base.sha,
            new_rev,
        ],
    )?;
    let mut changes = MainChanges {
        base: base.rev.clone(),
        tooling: Vec::new(),
        product: Vec::new(),
    };
    let mut fields = listing.split('\0');
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        if owned.contains(path) {
            changes.tooling.push(path.to_string());
        } else {
            let status = status.chars().next().unwrap_or('M');
            changes.product.push((status, path.to_string()));
        }
    }
    Ok(changes)
}

/// How many product files [`format_product_changes`] lists before it sums up.
const LISTED_FILES: usize = 20;

pub fn count_files(count: usize) -> String {
    if count == 1 {
        "1 file".into()
    } else {
        format!("{count} files")
    }
}

fn shell_word(path: &str) -> String {
    let plain = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c));
    if plain && !path.is_empty() {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// The product code `new_rev` changes, with the command that shows it.
/// `unexpected` words it as a warning, for a command that should change
/// tooling only.
pub fn format_product_changes(changes: &MainChanges, new_rev: &str, unexpected: bool) -> String {
    let base = &changes.base;
    let product = &changes.product;
    if product.is_empty() {
        return format!("Product code: unchanged against {base}");
    }
    let count = count_files(product.len());
    let mut out = if unexpected {
        format!(
            "warning: product code changed in {count} outside the tooling pack, against {base}:"
        )
    } else {
        format!("Product code: {count} changed against {base}:")
    };
    for (status, path) in product.iter().take(LISTED_FILES) {
        let _ = write!(out, "\n  {status} {path}");
    }
    if product.len() > LISTED_FILES {
        let _ = write!(
            out,
            "\n  … and {} more\nInspect with: git diff --stat {base} {new_rev}",
            product.len() - LISTED_FILES
        );
    } else {
        let _ = write!(out, "\nInspect with: git diff {base} {new_rev} --");
        for (_, path) in product {
            let _ = write!(out, " {}", shell_word(path));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_paths_skips_the_message_and_keeps_both_rename_names() {
        let patch = "From abc Mon Sep 17 00:00:00 2001\n\
Subject: [PATCH] x\n\n\
diff --git a/quoted.txt b/quoted.txt is in the message\n\
---\n \
a | 2 +-\n\n\
diff --git a/src/a.rs b/src/a.rs\n\
--- a/src/a.rs\n\
+++ b/src/a.rs\n\
@@ -1 +1 @@\n\
-a\n\
+b\n\
diff --git a/old name.md b/new name.md\n\
rename from old name.md\n\
rename to new name.md\n";
        let paths: Vec<String> = patch_paths(patch).into_iter().collect();
        assert_eq!(paths, ["new name.md", "old name.md", "src/a.rs"]);
    }

    fn changes(product: &[(char, &str)]) -> MainChanges {
        MainChanges {
            base: "origin/main".into(),
            tooling: Vec::new(),
            product: product
                .iter()
                .map(|(status, path)| (*status, path.to_string()))
                .collect(),
        }
    }

    #[test]
    fn product_changes_unchanged_is_one_line() {
        assert_eq!(
            format_product_changes(&changes(&[]), "main", true),
            "Product code: unchanged against origin/main"
        );
    }

    #[test]
    fn product_changes_list_files_and_the_command_for_them() {
        let changed = changes(&[('M', "src/a.rs"), ('D', "my notes.md")]);
        assert_eq!(
            format_product_changes(&changed, "main", true),
            "warning: product code changed in 2 files outside the tooling pack, against origin/main:\n  \
M src/a.rs\n  \
D my notes.md\n\
Inspect with: git diff origin/main main -- src/a.rs 'my notes.md'"
        );
        assert!(
            format_product_changes(&changed, "main", false)
                .starts_with("Product code: 2 files changed against origin/main:\n")
        );
    }

    #[test]
    fn product_changes_sum_up_a_long_list() {
        let paths: Vec<String> = (0..25).map(|n| format!("f{n}")).collect();
        let product: Vec<(char, &str)> = paths.iter().map(|p| ('A', p.as_str())).collect();
        let text = format_product_changes(&changes(&product), "main", false);
        assert!(
            text.ends_with("  … and 5 more\nInspect with: git diff --stat origin/main main"),
            "{text}"
        );
    }
}
