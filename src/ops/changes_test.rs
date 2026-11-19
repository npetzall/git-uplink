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
