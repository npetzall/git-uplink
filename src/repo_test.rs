use super::{
    UPSTREAM_REF, ensure_upstream_ref, file_history, has_ref, new_patch_id, patch_substance,
    random_hex, show_at,
};
use crate::git::git_ok;

#[test]
fn ensure_upstream_ref_tolerates_a_missing_branch_but_not_a_broken_origin() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("origin.git");
    let repo = dir.path().join("work");
    std::fs::create_dir_all(&repo).unwrap();
    git_ok(
        dir.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    )
    .unwrap();
    git_ok(&repo, &["init", "-q"]).unwrap();
    git_ok(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]).unwrap();
    git_ok(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();

    ensure_upstream_ref(&repo).unwrap();
    assert!(!has_ref(&repo, UPSTREAM_REF).unwrap());

    let missing = dir.path().join("missing.git");
    git_ok(
        &repo,
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    )
    .unwrap();
    assert!(ensure_upstream_ref(&repo).is_err());
}

#[test]
fn option_like_refs_are_not_parsed_as_git_options() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git_ok(repo, &["init", "-q"]).unwrap();
    std::fs::create_dir_all(repo.join(".uplink")).unwrap();
    std::fs::write(repo.join(".uplink/queue.json"), "{}\n").unwrap();
    git_ok(repo, &["add", "."]).unwrap();
    git_ok(repo, &["commit", "-q", "-m", "init"]).unwrap();
    // Without --end-of-options, git show would write the output here.
    std::fs::create_dir_all(repo.join("out:.uplink")).unwrap();

    assert!(show_at(repo, "--output=out", ".uplink/queue.json").is_err());
    assert!(!repo.join("out:.uplink/queue.json").exists());
    assert!(
        file_history(repo, "--output=log.txt", ".uplink/queue.json")
            .unwrap()
            .is_empty()
    );
    assert!(!repo.join("log.txt").exists());
    assert_eq!(show_at(repo, "HEAD", ".uplink/queue.json").unwrap(), "{}");
}

const SHA_A: &str = "d075809c4498552bd4e080010af34999967d68f7";
const SHA_B: &str = "5529448d17e1f2eb9c08b4396ed7b22b55ccb83d";

fn sample(sha: &str, date: &str, subject: &str, body_date: &str, hunk: &str) -> String {
    format!(
        "From {sha} Mon Sep 17 00:00:00 2001\n\
From: Uplink Bot <uplink@company.example>\n\
Date: {date}\n\
Subject: [PATCH] {subject}\n\
\n\
{subject}\n\
{body_date}\n\
\n\
---\n\
 notes.md | 1 +\n\
 1 file changed, 1 insertion(+)\n\
\n\
diff --git a/notes.md b/notes.md\n\
--- a/notes.md\n\
+++ b/notes.md\n\
@@ -0,0 +1 @@\n\
+{hunk}\n"
    )
}

#[test]
fn sha_and_date_header_do_not_change_patch_substance() {
    let stored = sample(
        SHA_A,
        "Tue, 22 Sep 2026 22:52:44 +0200",
        "Uplink tooling",
        "Date: kept in the message",
        "tooling",
    );
    let replayed = sample(
        SHA_B,
        "Tue, 22 Sep 2026 21:20:41 +0000",
        "Uplink tooling",
        "Date: kept in the message",
        "tooling",
    );
    assert_eq!(patch_substance(&stored), patch_substance(&replayed));
}

#[test]
fn hunk_change_is_a_different_patch() {
    let stored = sample(SHA_A, "Tue, 22 Sep 2026 22:52:44 +0200", "Note", "", "one");
    let updated = sample(SHA_B, "Tue, 22 Sep 2026 21:20:41 +0000", "Note", "", "two");
    assert_ne!(patch_substance(&stored), patch_substance(&updated));
}

#[test]
fn subject_change_is_a_different_patch() {
    let stored = sample(SHA_A, "Tue, 22 Sep 2026 22:52:44 +0200", "Note", "", "one");
    let updated = sample(SHA_B, "Tue, 22 Sep 2026 21:20:41 +0000", "Other", "", "one");
    assert_ne!(patch_substance(&stored), patch_substance(&updated));
}

#[test]
fn date_line_in_the_message_body_is_a_change() {
    let stored = sample(
        SHA_A,
        "Tue, 22 Sep 2026 22:52:44 +0200",
        "Note",
        "Date: kept in the message",
        "one",
    );
    let updated = sample(
        SHA_A,
        "Tue, 22 Sep 2026 22:52:44 +0200",
        "Note",
        "Date: rewritten in the message",
        "one",
    );
    assert_ne!(patch_substance(&stored), patch_substance(&updated));
}

#[test]
fn random_hex_has_the_requested_length_and_alphabet() {
    for len in [1, 8, 10, 32] {
        let hex = random_hex(len);
        assert_eq!(hex.len(), len);
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }
    assert_ne!(random_hex(32), random_hex(32));
}

#[test]
fn new_patch_id_is_upl_and_ten_hex_digits() {
    let id = new_patch_id();
    assert!(id.starts_with("upl_"), "{id}");
    assert_eq!(id.len(), 14, "{id}");
    let header = format!("Uplink-Depends-On: {id}");
    assert_eq!(crate::assess::parse_depends_on(&header), vec![id]);
}
