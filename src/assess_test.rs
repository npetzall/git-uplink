use super::*;

fn report(ok: bool, checks: Vec<AssessCheck>) -> AssessReport {
    AssessReport {
        at: "2026-10-01T00:00:00.000Z".into(),
        ok,
        public_message: "Use SHA-256\n\nPublic reason.".into(),
        co_author: None,
        original_author: None,
        original_email: None,
        cutoff_found: true,
        checks,
    }
}

fn check(id: &str, status: CheckStatus, detail: &str) -> AssessCheck {
    AssessCheck {
        id: id.into(),
        status,
        detail: detail.into(),
    }
}

const TTL_PATCH: &str = "From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Asha <asha@example.com>
Subject: [PATCH] Extend TTL

diff --git a/README.md b/README.md is quoted in the message
---
 src/tokens.js | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/src/tokens.js b/src/tokens.js
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/src/tokens.js
+++ b/src/tokens.js
@@ -1,3 +1,3 @@
 function ttl() {
-  return 3600;
+  return 7200;
 }
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333
GIT binary patch
literal 4
Lc${NkU|;|M00aO5

literal 0
HcmV?d00001

--
2.50.0
";

fn authored_patch(message: &str, authors: &[&str]) -> Patch {
    let mut patch: Patch = serde_json::from_value(serde_json::json!({
        "id": "upl_0123456789",
        "commitMessage": message,
        "status": "queued",
        "dependsOn": [],
        "createdAt": "2026-10-01T00:00:00.000Z",
        "updatedAt": "2026-10-01T00:00:00.000Z",
        "patchIdStable": null,
        "source": {},
        "events": [],
    }))
    .unwrap();
    patch.source.authors = authors.iter().map(|a| a.to_string()).collect();
    patch
}

fn empty_queue() -> QueueState {
    QueueState::empty(crate::types::QueueConfig::default())
}

#[test]
fn original_author_is_the_stored_one_unless_a_bot_recorded_it() {
    let mut patch = authored_patch("x", &["Asha <asha@corp.example>"]);
    assert_eq!(
        original_author_of(&patch),
        (Some("Asha".into()), Some("asha@corp.example".into()))
    );
    let report = |name: &str, email: &str| AssessReport {
        at: String::new(),
        ok: true,
        public_message: String::new(),
        co_author: None,
        original_author: Some(name.into()),
        original_email: Some(email.into()),
        cutoff_found: false,
        checks: Vec::new(),
    };
    patch.assess = Some(report("Cleo", "cleo@corp.example"));
    assert_eq!(original_author_of(&patch).0.as_deref(), Some("Cleo"));
    patch.assess = Some(report("Uplink Bot", crate::git::BOT_EMAIL));
    assert_eq!(original_author_of(&patch).0.as_deref(), Some("Asha"));
    patch.source.authors.clear();
    assert_eq!(original_author_of(&patch).0.as_deref(), Some("Uplink Bot"));
    patch.assess = None;
    assert_eq!(original_author_of(&patch), (None, None));
}

#[test]
fn only_the_text_before_the_first_cutoff_is_public() {
    let message = format!(
        "Use SHA-256\n\nPublic reason.\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1\n\n{DEFAULT_CUTOFF}\nSecret: PROJ-2\n"
    );
    let (public, internal) = split_internal_message(&message, DEFAULT_CUTOFF);
    assert_eq!(public, "Use SHA-256\n\nPublic reason.");
    assert!(internal.contains("PROJ-1"), "{internal}");
    assert!(internal.contains(DEFAULT_CUTOFF), "{internal}");
    assert!(internal.contains("PROJ-2"), "{internal}");

    let report = assess_export(&empty_queue(), &message, (None, None), None);
    assert_eq!(report.public_message, "Use SHA-256\n\nPublic reason.");
    let mut patch = authored_patch(&message, &[]);
    patch.assess = Some(report);
    let exported = export_commit_message(&patch);
    assert!(!exported.contains("PROJ"), "{exported}");
    assert!(!exported.contains(DEFAULT_CUTOFF), "{exported}");
}

#[test]
fn the_subject_is_the_first_line_once_comments_are_stripped() {
    assert_eq!(
        message_subject("<!-- template -->\nUse SHA-256\n\nWhy.", DEFAULT_CUTOFF).unwrap(),
        "Use SHA-256"
    );
    assert!(message_subject("  \n", DEFAULT_CUTOFF).is_err());
    assert!(message_subject(&format!("{DEFAULT_CUTOFF}\nTicket: PROJ-1"), DEFAULT_CUTOFF).is_err());
}

#[test]
fn public_message_drops_depends_on_and_the_internal_part() {
    let message = format!(
        "Use SHA-256\n\nWhy.\nUplink-Depends-On: upl_0123456789\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1"
    );
    let report = assess_export(&empty_queue(), &message, (None, None), None);
    assert_eq!(report.public_message, "Use SHA-256\n\nWhy.");
    let mut patch = authored_patch(&message, &[]);
    patch.assess = Some(report);
    assert_eq!(
        public_subject_and_body(&patch),
        ("Use SHA-256".to_string(), "Why.".to_string())
    );
    assert!(upstream_pr_body(&patch).starts_with("Why.\n\n"));
}

#[test]
fn a_cutoff_inside_an_html_comment_does_not_split() {
    let message = format!(
        "Subject\n\n<!-- {DEFAULT_CUTOFF} -->\nStill public.\n\n{DEFAULT_CUTOFF}\nInternal.\n"
    );
    let (public, internal) = split_internal_message(&message, DEFAULT_CUTOFF);
    assert_eq!(public, "Subject\n\nStill public.");
    assert_eq!(internal, "Internal.");
}

#[test]
fn company_message_without_authors_is_unchanged() {
    let patch = authored_patch("Use SHA-256\n\nPublic reason.", &[]);
    assert_eq!(
        company_commit_message(&empty_queue(), &patch),
        "Use SHA-256\n\nPublic reason.\n\nUplink-Patch-Id: upl_0123456789\n"
    );
}

#[test]
fn company_message_adds_the_cutoff_before_internal_authors() {
    let patch = authored_patch(
        "Use SHA-256\n\nPublic reason.",
        &["Asha <asha@corp.example>", "Ben <ben@corp.example>"],
    );
    assert_eq!(
        company_commit_message(&empty_queue(), &patch),
        format!(
            "Use SHA-256\n\nPublic reason.\n\n{DEFAULT_CUTOFF}\n\n\
Uplink-Patch-Id: upl_0123456789\n\
Co-Authored-By: Asha <asha@corp.example>\n\
Co-Authored-By: Ben <ben@corp.example>\n"
        )
    );
}

#[test]
fn company_message_keeps_an_existing_cutoff_and_skips_named_co_authors() {
    let message = format!(
        "Use SHA-256\n\n{DEFAULT_CUTOFF}\nTicket: PROJ-1\nco-authored-by: ASHA <Asha@Corp.Example>"
    );
    let patch = authored_patch(
        &message,
        &["Asha <asha@corp.example>", "Ben <ben@corp.example>"],
    );
    let company = company_commit_message(&empty_queue(), &patch);
    assert_eq!(company.matches(DEFAULT_CUTOFF).count(), 1, "{company}");
    assert!(!company.contains("Co-Authored-By: Asha"), "{company}");
    assert!(
        company
            .ends_with("Uplink-Patch-Id: upl_0123456789\nCo-Authored-By: Ben <ben@corp.example>\n"),
        "{company}"
    );
    let (public, _) = split_internal_message(&company, DEFAULT_CUTOFF);
    assert!(!public.contains("Ben"), "{public}");
}

#[test]
fn export_message_never_names_internal_authors() {
    let message = "Use SHA-256\n\nPublic reason.";
    let mut patch = authored_patch(message, &["Asha <asha@corp.example>"]);
    patch.assess = Some(assess_export(&empty_queue(), message, (None, None), None));
    let exported = export_commit_message(&patch);
    assert!(!exported.contains("asha"), "{exported}");
}

#[test]
fn merge_authors_adds_each_email_once_and_keeps_order() {
    let mut authors = vec!["Asha <asha@corp.example>".to_string()];
    merge_authors(
        &mut authors,
        [
            "A. Sha <ASHA@corp.example>".to_string(),
            "Ben <ben@corp.example>".to_string(),
            "not a person".to_string(),
            "Ben B <ben@corp.example>".to_string(),
        ],
    );
    assert_eq!(
        authors,
        ["Asha <asha@corp.example>", "Ben <ben@corp.example>"]
    );
}

#[test]
fn bots_are_not_authors() {
    assert!(is_bot("Uplink Bot", crate::git::BOT_EMAIL));
    assert!(is_bot(
        "github-actions[bot]",
        "41898282+github-actions[bot]@users.noreply.github.com"
    ));
    assert!(!is_bot("Asha", "asha@corp.example"));
}

#[test]
fn changed_lines_is_what_the_patch_adds_and_removes() {
    assert_eq!(
        changed_lines(TTL_PATCH),
        "diff --git a/README.md b/README.md is quoted in the message\n\
diff --git a/src/tokens.js b/src/tokens.js\n\
-  return 3600;\n\
+  return 7200;\n\
diff --git a/logo.png b/logo.png\n\
new file mode 100644\n\
binary 3333333333333333333333333333333333333333\n"
    );
}

#[test]
fn changed_lines_ignores_context_and_position() {
    // Replayed after upstream changed the lines around the patch's own.
    let replayed = TTL_PATCH
            .replace(" function ttl() {", " function defaultTtl() {")
            .replace("@@ -1,3 +1,3 @@", "@@ -40,3 +41,3 @@ header")
            .replace("index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222", "index 4444444444444444444444444444444444444444..5555555555555555555555555555555555555555");
    assert_ne!(replayed, TTL_PATCH);
    assert_eq!(changed_lines(&replayed), changed_lines(TTL_PATCH));
}

#[test]
fn changed_lines_changes_with_what_the_patch_adds_or_removes() {
    let base = changed_lines(TTL_PATCH);
    let changes = [
        ("+  return 7200;", "+  return 7200; // AcmeCorp SLA"),
        // Kept its own line over an upstream change of the same line.
        ("-  return 3600;", "-  return 1800;"),
        (
            "+  return 7200;",
            "+  return 7200;\n\\ No newline at end of file",
        ),
        ("b/src/tokens.js", "b/src/token.js"),
        (
            "..3333333333333333333333333333333333333333",
            "..6666666666666666666666666666666666666666",
        ),
        ("new file mode 100644", "new file mode 100755"),
    ];
    for (from, to) in changes {
        let changed = TTL_PATCH.replace(from, to);
        assert_ne!(changed, TTL_PATCH, "{from}");
        assert_ne!(changed_lines(&changed), base, "{from}");
    }
    // A second added line in the same hunk.
    let more = TTL_PATCH
        .replace("@@ -1,3 +1,3 @@", "@@ -1,3 +1,4 @@")
        .replace("+  return 7200;", "+  return 7200;\n+  // extra");
    assert!(changed_lines(&more).contains("+  // extra\n"));
}

#[test]
fn export_diff_is_the_diffs_without_the_mail_and_binary_content() {
    let patch = TTL_PATCH.replace(
        "Subject: [PATCH] Extend TTL\n",
        "Subject: [PATCH] Extend TTL\n\nPublic reason.\n\nTicket: PROJ-1 for AcmeCorp\n",
    );
    let export = export_diff(&patch);
    // The company message and the author's address are not exported.
    assert!(!export.diff.contains("AcmeCorp"), "{}", export.diff);
    assert!(!export.diff.contains("asha@example.com"), "{}", export.diff);
    assert!(export.diff.contains("-  return 3600;\n+  return 7200;\n"));
    assert!(export.diff.contains(" function ttl() {\n"));
    // A binary file is named, and its encoded content is not scanned.
    assert_eq!(export.binaries, ["logo.png"]);
    assert!(export.diff.contains("GIT binary patch\n"));
    assert!(!export.diff.contains("Lc${NkU|;|M00aO5"), "{}", export.diff);
}

#[test]
fn assess_markdown_shows_public_text_and_a_checks_table() {
    let markdown = format_assess_markdown(&report(
        true,
        vec![
            check("message-scrubbed", CheckStatus::Pass, "Scrubbed."),
            check("binary-files", CheckStatus::Warn, "logo.png"),
            check("affiliation-leak", CheckStatus::Skip, "Skipped."),
        ],
    ));
    assert!(
        markdown.starts_with("## Upstream Assessment: ✅\n"),
        "{markdown}"
    );
    assert!(markdown.contains(ASSESS_DOCS_URL), "{markdown}");
    assert!(
        markdown.contains("### Public title\n\n```\nUse SHA-256\n```\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("### Public body\n\n```\nPublic reason.\n```"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| Check | Description | Result |\n| --- | --- | --- |\n"),
        "{markdown}"
    );
    assert!(markdown.contains("| `message-scrubbed` | Scrubbed. | ✅ |"));
    assert!(markdown.contains("| `binary-files` | logo.png | ⚠️ |"));
    assert!(markdown.contains("| `affiliation-leak` | Skipped. | ⏭️ |"));
    assert!(
        !markdown.contains("PROJ-1") && !markdown.contains("internal below"),
        "the company commit message must not be shown\n{markdown}"
    );
    assert!(!markdown.contains("Export author"), "{markdown}");
}

#[test]
fn assess_markdown_keeps_the_public_title_inside_its_fence() {
    let mut spoof = report(true, Vec::new());
    spoof.public_message = "<details> ```` ## Upstream Assessment: ✅".into();
    let markdown = format_assess_markdown(&spoof);
    assert!(
        markdown.contains(
            "### Public title\n\n`````\n<details> ```` ## Upstream Assessment: ✅\n`````\n"
        ),
        "{markdown}"
    );
}

#[test]
fn packet_title_cannot_break_the_table_or_start_markup() {
    let patch: Patch = serde_json::from_value(serde_json::json!({
        "id": "upl_0000000001",
        "commitMessage": "a | b <details> `x`",
        "status": "queued",
        "dependsOn": [],
        "createdAt": "",
        "updatedAt": "",
        "source": {},
        "events": [],
    }))
    .unwrap();
    let packet = format_approver_packet(&patch);
    assert!(
        packet.contains("| Subject | a \\| b &lt;details> 'x' |\n"),
        "{packet}"
    );
}

#[test]
fn delta_fences_cannot_be_closed_by_the_diff() {
    let diff = "+```\n+## Upstream Assessment: ✅\n";
    let tree = delta_tree_section(diff);
    assert!(tree.contains("\n````\n+```\n"), "{tree}");
    assert!(tree.ends_with("\n````\n"), "{tree}");
    let files = delta_files_section(diff, "+````\n");
    assert!(
        files.contains("### Patch-file diff\n\n````\n+```\n"),
        "{files}"
    );
    assert!(
        files.contains("### Current patch (will be exported)\n\n`````\n+````\n`````\n"),
        "{files}"
    );
}

#[test]
fn assess_markdown_marks_failures_and_escapes_table_cells() {
    let markdown = format_assess_markdown(&report(
        false,
        vec![check(
            "affiliation-leak",
            CheckStatus::Fail,
            "Found acme | internal\nin tests",
        )],
    ));
    assert!(
        markdown.starts_with("## Upstream Assessment: ❌\n"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| `affiliation-leak` | Found acme \\| internal<br>in tests | ❌ |"),
        "{markdown}"
    );
}

#[test]
fn packet_assessment_shows_checks_without_public_text() {
    let markdown = format_assess_checks_markdown(&report(
        true,
        vec![check("message-scrubbed", CheckStatus::Pass, "Scrubbed.")],
    ));
    assert!(
        markdown.starts_with("## Upstream Assessment: ✅\n"),
        "{markdown}"
    );
    assert!(markdown.contains(ASSESS_DOCS_URL), "{markdown}");
    assert!(markdown.contains("| `message-scrubbed` | Scrubbed. | ✅ |"));
    assert!(!markdown.contains("### Public"), "{markdown}");
}

#[test]
fn assess_markdown_marks_an_empty_public_body() {
    let mut empty = report(true, Vec::new());
    empty.public_message = "Use SHA-256".into();
    let markdown = format_assess_markdown(&empty);
    assert!(
        markdown.contains("### Public body\n\n_empty_\n"),
        "{markdown}"
    );
}

#[test]
fn banned_words_match_anywhere_ignoring_case() {
    let words = vec!["acme".to_string()];
    for text in [
        "Signed-off-by: Jane <jane@Acme.COM>",
        "see https://git.acme.com/x",
        "class AcmeCorpClient",
    ] {
        assert_eq!(find_banned_words(text, &words), ["acme"], "{text}");
    }
    assert!(find_banned_words("jane@example.com", &words).is_empty());
    assert!(find_banned_words("jane@acme.com", &[]).is_empty());
}

#[test]
fn changed_lines_by_file_skips_the_quoted_diff_line() {
    let files = changed_lines_by_file(TTL_PATCH);
    let headers: Vec<&str> = files.iter().map(|(header, _)| header.as_str()).collect();
    assert_eq!(
        headers,
        [
            "diff --git a/src/tokens.js b/src/tokens.js",
            "diff --git a/logo.png b/logo.png"
        ]
    );
    assert_eq!(files[0].1, "-  return 3600;\n+  return 7200;\n");
}

/// A patch touching each `(path, body)`; `body` is the text under the `diff --git` line.
fn patch_of(files: &[(&str, &str)]) -> String {
    let mut patch = String::from("Subject: [PATCH] x\n\n---\n");
    for (path, body) in files {
        patch.push_str(&format!("diff --git a/{path} b/{path}\n{body}"));
    }
    patch
}

const KEEP: &str = "--- a/keep.rs\n+++ b/keep.rs\n@@ -1 +1 @@\n-old\n+new\n";
const SHARED: &str = "@@ -1,3 +1,3 @@\n fn main() {\n-    one();\n+    two();\n }\n";

#[test]
fn revision_delta_compares_file_by_file() {
    let dir = tempfile::tempdir().unwrap();
    let older = patch_of(&[
        ("keep.rs", KEEP),
        ("edit.rs", "@@ -1 +1 @@\n-a\n+b\n"),
        ("dropped.rs", SHARED),
    ]);
    let newer = patch_of(&[
        ("keep.rs", KEEP),
        ("edit.rs", "@@ -1 +1,2 @@\n-a\n+b\n+c\n"),
        ("picked.rs", SHARED),
    ]);

    let files = revision_delta(dir.path(), &older, &newer).unwrap();
    let states: Vec<(&str, DeltaState)> = files
        .iter()
        .map(|file| (file.path.as_str(), file.state))
        .collect();
    assert_eq!(
        states,
        [
            ("keep.rs", DeltaState::Unchanged),
            ("edit.rs", DeltaState::Changed),
            ("picked.rs", DeltaState::Added),
            ("dropped.rs", DeltaState::Removed),
        ]
    );
    assert_eq!(files[0].diff, None);
    assert_eq!(files[0].lines, None);
    assert_eq!(files[1].diff.as_deref(), Some(" -a\n +b\n++c\n"));
    // Same lines in the dropped and the picked-up file: each is listed whole.
    let shared = Some("-    one();\n+    two();\n");
    assert_eq!(files[2].lines.as_deref(), shared);
    assert_eq!(files[3].lines.as_deref(), shared);

    let same = revision_delta(dir.path(), &newer, &newer).unwrap();
    assert!(same.iter().all(|file| file.state == DeltaState::Unchanged));
}

#[test]
fn revision_delta_shows_a_file_that_becomes_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let older = patch_of(&[("gone.rs", "@@ -1,2 +1 @@\n one\n-two\n")]);
    let newer = patch_of(&[(
        "gone.rs",
        "deleted file mode 100644\n@@ -1,2 +0,0 @@\n-one\n-two\n",
    )]);

    let files = revision_delta(dir.path(), &older, &newer).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].state, DeltaState::Changed);
    assert_eq!(
        files[0].diff.as_deref(),
        Some("+deleted file mode 100644\n+-one\n -two\n")
    );
}

#[test]
fn tree_diff_patches_compares_applied_trees_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git_ok(repo, &["init", "-q"]).unwrap();
    fs::write(repo.join("rate.txt"), "1800\n").unwrap();
    git_ok(repo, &["add", "."]).unwrap();
    git_ok(repo, &["commit", "-q", "-m", "base"]).unwrap();
    let patch_to = |value: &str| {
        fs::write(repo.join("rate.txt"), format!("{value}\n")).unwrap();
        let diff = git_ok(repo, &["diff"]).unwrap() + "\n";
        git_ok(repo, &["checkout", "--", "rate.txt"]).unwrap();
        diff
    };
    let approved = patch_to("3600");
    let current = patch_to("7200");

    let tree = tree_diff_patches(repo, &approved, &current).expect("tree diff");
    assert!(tree.contains("-3600") && tree.contains("+7200"), "{tree}");
    assert_eq!(tree_diff_patches(repo, &approved, &approved), None);
    assert_eq!(tree_diff_patches(repo, "not a patch\n", &current), None);

    let worktrees = git_ok(repo, &["worktree", "list", "--porcelain"]).unwrap();
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
}

#[test]
fn static_patterns_compile() {
    for re in [
        &HTML_COMMENT,
        &BLANK_LINES,
        &DEPENDS_ON_HEADER,
        &PATCH_ID,
        &PERSON,
        &EXPORT_AUTHOR_HEADER,
        &TICKET,
    ] {
        std::sync::LazyLock::force(re);
    }
}

#[test]
fn ip_review_shows_what_leaves_and_hides_how_the_message_was_written() {
    let passed = report(
        true,
        vec![
            check("dependencies", CheckStatus::Skip, "None."),
            check("message-scrubbed", CheckStatus::Pass, "Scrubbed."),
            check("cutoff-used", CheckStatus::Pass, "Cutoff found."),
            check("co-author", CheckStatus::Skip, "No Uplink-Export-Author."),
            check("affiliation-leak", CheckStatus::Pass, "No banned words."),
        ],
    );
    let review = format_ip_review(&passed);
    assert!(review.starts_with("## IP review: ✅\n"), "{review}");
    assert!(
        review.contains(
            "- ✅ **Banned words:** none in the title, message, file paths or added lines."
        ),
        "{review}"
    );
    for hidden in [
        "dependencies",
        "Scrubbed",
        "Cutoff",
        "Co-author",
        "Uplink-Export-Author",
    ] {
        assert!(!review.contains(hidden), "{hidden}\n{review}");
    }

    let mut flagged = report(
        false,
        vec![
            check("dependencies", CheckStatus::Fail, "upl_1 (queued)"),
            check(
                "cutoff-used",
                CheckStatus::Warn,
                "public message contains PROJ-1",
            ),
            check("affiliation-leak", CheckStatus::Fail, "banned words: acme"),
            check("binary-files", CheckStatus::Warn, "logo.png"),
        ],
    );
    flagged.co_author = Some("Jane Doe <jane@example.com>".into());
    let review = format_ip_review(&flagged);
    assert!(review.starts_with("## IP review: ❌\n"), "{review}");
    assert!(
        review.contains("- ❌ **Banned words:** banned words: acme"),
        "{review}"
    );
    assert!(
        review.contains("- ⚠️ **Binary files:** logo.png"),
        "{review}"
    );
    assert!(
        review.contains("- ⚠️ **Tickets in the public message:** public message contains PROJ-1"),
        "{review}"
    );
    assert!(
        review.contains("- ❌ **Blocking, `dependencies`:** upl_1 (queued)"),
        "{review}"
    );
    assert!(
        review.contains("- **Co-author:** Jane Doe &lt;jane@example.com>"),
        "{review}"
    );
}

#[test]
fn additions_keep_new_line_numbers_and_drop_removals() {
    let diff = "\
diff --git a/src/a.js b/src/a.js
index 1111111..2222222 100644
--- a/src/a.js
+++ b/src/a.js
@@ -2 +2 @@ fn
-  old();
+  new();
@@ -9,0 +10,2 @@
+one
+```
@@ -20,2 +21,0 @@
-gone
-gone too
diff --git a/src/b.js b/src/b.js
deleted file mode 100644
index 3333333..0000000
--- a/src/b.js
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..4444444
--- /dev/null
+++ b/new.txt
@@ -0,0 +1 @@
+hello
diff --git a/logo.png b/logo.png
index 5555555..6666666 100644
Binary files a/logo.png and b/logo.png differ
";
    let files = parse_additions(diff);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/a.js", "src/b.js", "new.txt", "logo.png"]);
    assert_eq!(
        files[0].blocks,
        [
            (2, vec!["  new();".to_string()]),
            (10, vec!["one".to_string(), "```".to_string()]),
        ]
    );
    assert!(files[1].blocks.is_empty() && files[1].removes);
    assert_eq!(files[2].note.as_deref(), Some("new file"));
    assert!(files[3].binary);

    let section = format_additions(&files, "Note.");
    assert!(
        section.contains("Line 2\n\n```diff\n+  new();\n```"),
        "{section}"
    );
    assert!(
        section.contains("Lines 10–11\n\n````diff\n+one\n+```\n````"),
        "{section}"
    );
    assert!(
        !section.contains("old();") && !section.contains("gone"),
        "{section}"
    );
    assert!(
        section.contains("Only removes lines (not shown): `src/b.js`."),
        "{section}"
    );
    assert!(section.contains("_new file_"), "{section}");
    assert!(section.contains("Binary file"), "{section}");
}

#[test]
fn additions_of_a_patch_file_count_context_lines() {
    let patch = "\
From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
Subject: [PATCH] Quote diff --git a/x b/x in the message

diff --git a/x b/x
---
 src/a.js | 2 +-
diff --git a/src/a.js b/src/a.js
--- a/src/a.js
+++ b/src/a.js
@@ -4,3 +4,4 @@
 keep
-old
+new
+more
 keep
--
2.45.0
";
    let files = parse_additions(patch);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(
        files[0].blocks,
        [(5, vec!["new".to_string(), "more".to_string()])]
    );
}

#[test]
fn message_table_escapes_html_and_keeps_one_line_per_cell() {
    let patch: Patch = serde_json::from_value(serde_json::json!({
        "id": "upl_0000000001",
        "status": "queued",
        "dependsOn": [],
        "createdAt": "",
        "updatedAt": "",
        "source": {},
        "events": [],
        "commitMessage": "Fix <b> & co\n\n</pre></td> body",
    }))
    .unwrap();
    let table = format_message_table(&patch);
    assert!(
        table.contains("<pre>Fix &lt;b&gt; &amp; co<br><br>&lt;/pre&gt;&lt;/td&gt; body</pre>"),
        "{table}"
    );
    let inside = table.split("<table>").nth(1).unwrap();
    assert!(
        !inside.contains("\n\n"),
        "no blank line inside the table\n{table}"
    );
}

#[test]
fn the_scan_reads_added_lines_and_kept_paths_only() {
    let diff = "\
diff --git a/src/a.js b/src/a.js
--- a/src/a.js
+++ b/src/a.js
@@ -1,3 +1,3 @@
 // context mentions acme-context
-// removed mentions acme-removed
+// added mentions acme-added
 end
diff --git a/old.txt b/acme-renamed.txt
similarity index 100%
rename from old.txt
rename to acme-renamed.txt
diff --git a/acme-deleted.txt b/acme-deleted.txt
deleted file mode 100644
--- a/acme-deleted.txt
+++ /dev/null
@@ -1 +0,0 @@
-acme-gone
";
    let surface = export_surface_lines(diff);
    assert!(surface.contains("acme-added"), "{surface}");
    assert!(surface.contains("acme-renamed.txt"), "{surface}");
    assert!(surface.contains("src/a.js"), "{surface}");
    for kept_out in [
        "acme-context",
        "acme-removed",
        "acme-deleted",
        "acme-gone",
        "old.txt",
    ] {
        assert!(!surface.contains(kept_out), "{kept_out}\n{surface}");
    }
}
