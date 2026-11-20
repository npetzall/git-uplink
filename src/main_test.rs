use clap::Parser;

use super::*;

#[test]
fn web_ui_has_no_bind_option() {
    assert!(Cli::try_parse_from(["git-uplink", "web-ui", "--bind", "0.0.0.0"]).is_err());
    assert!(Cli::try_parse_from(["git-uplink", "web-ui", "--port", "1", "--no-open"]).is_ok());
}

type Parsed = (Option<Option<PathBuf>>, bool, bool);

fn adopt_parses(extra: &[&str]) -> Option<Parsed> {
    let mut argv = vec!["git-uplink", "adopt", "--branch", "main-fork"];
    argv.extend(extra);
    match Cli::try_parse_from(argv).ok()?.command {
        Commands::Adopt {
            groups,
            apply,
            write_groups,
            ..
        } => Some((groups, apply, write_groups)),
        _ => None,
    }
}

#[test]
fn adopt_groups_flags() {
    assert_eq!(adopt_parses(&[]), Some((None, false, false)));
    // No path: the groups file of the branch.
    assert_eq!(
        adopt_parses(&["--groups"]),
        Some((Some(None), false, false))
    );
    assert_eq!(
        adopt_parses(&["--groups", "g.json", "--apply"]),
        Some((Some(Some(PathBuf::from("g.json"))), true, false))
    );
    assert_eq!(adopt_parses(&["--write-groups"]), Some((None, false, true)));
    assert_eq!(adopt_parses(&["--write-groups", "g.json"]), None);
    assert_eq!(adopt_parses(&["--write-groups", "--groups"]), None);
    assert_eq!(adopt_parses(&["--apply"]), None);
}

#[test]
fn init_adopt_groups_flags() {
    let ok = |args: &[&str]| {
        let mut argv = vec!["git-uplink", "init"];
        argv.extend(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(ok(&["--adopt-groups"]));
    assert!(ok(&["--adopt-groups", "g.json", "--apply"]));
    assert!(ok(&["--write-groups"]));
    assert!(!ok(&["--apply"]));
    assert!(!ok(&["--write-groups", "--adopt-groups"]));
}

fn merged_via(args: &[&str]) -> Option<MergeVia> {
    let argv = ["git-uplink", "merged", "upl_x"].iter().chain(args);
    match Cli::try_parse_from(argv).ok()?.command {
        Commands::Merged { via, .. } => Some(via.into()),
        _ => None,
    }
}

#[test]
fn write_markdown_file_reports_errors() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write_markdown_file(repo, Path::new("out/report.md"), "# hi").unwrap();
    assert_eq!(
        fs::read_to_string(repo.join("out/report.md")).unwrap(),
        "# hi\n"
    );
    // A regular file where a directory is needed cannot be written through.
    let err = write_markdown_file(repo, Path::new("out/report.md/x.md"), "x").unwrap_err();
    assert!(err.to_string().contains("could not write"), "{err}");
}

#[test]
fn gated_pr_body_is_the_message_without_its_subject() {
    let patch: Patch = serde_json::from_value(serde_json::json!({
            "id": "upl_x",
            "commitMessage": "Use SHA-256\n\nWhy.\n\n----- Uplink: internal below this line -----\nPROJ-1",
            "status": "queued",
            "dependsOn": [],
            "createdAt": "t",
            "updatedAt": "t",
            "source": {},
            "events": [],
        }))
        .unwrap();
    assert_eq!(
        patch_body(&patch),
        "Why.\n\n----- Uplink: internal below this line -----\nPROJ-1\n"
    );
}

#[test]
fn gated_pr_body_names_every_dependency_once() {
    let patch: Patch = serde_json::from_value(serde_json::json!({
        "id": "upl_x",
        "commitMessage": "Use SHA-256\n\nWhy.\nUplink-Depends-On: upl_0123456789",
        "status": "queued",
        "dependsOn": ["upl_0123456789", "upl_abcdefabcd"],
        "createdAt": "t",
        "updatedAt": "t",
        "source": {},
        "events": [],
    }))
    .unwrap();
    assert_eq!(
        patch_body(&patch),
        "Why.\nUplink-Depends-On: upl_0123456789\n\nUplink-Depends-On: upl_abcdefabcd\n"
    );
}

#[test]
fn conflict_json_keeps_camel_case_keys() {
    let patch: Patch = serde_json::from_value(serde_json::json!({
        "id": "upl_x",
        "commitMessage": "x",
        "status": "conflict",
        "dependsOn": [],
        "createdAt": "t",
        "updatedAt": "t",
        "source": {},
        "events": [],
        "conflict": {
            "branch": "uplink/conflict/upl_x",
            "workBranch": "uplink/conflict/upl_x-work",
            "files": [],
            "message": "m",
            "onto": "abc"
        }
    }))
    .unwrap();
    assert_eq!(
        conflict_json(&patch),
        serde_json::json!({
            "id": "upl_x",
            "branch": "uplink/conflict/upl_x",
            "workBranch": "uplink/conflict/upl_x-work",
            "onto": "abc",
        })
    );
}

#[test]
fn pr_number_prefers_flag_then_url() {
    let url = "https://github.com/acme/app/pull/42";
    assert_eq!(pr_number(Some(7), url).unwrap(), 7);
    assert_eq!(pr_number(None, url).unwrap(), 42);
    let err = pr_number(None, "https://github.com/acme/app").unwrap_err();
    assert!(
        err.to_string()
            .contains("could not parse pull request number")
    );
}

#[test]
fn transfer_requires_exactly_one_direction() {
    let parse = |args: &[&str]| {
        let argv = ["git-uplink", "transfer", "upl_x"].iter().chain(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(!parse(&[]));
    assert!(!parse(&["--complete"]));
    assert!(!parse(&["--to-upstream", "--to-internal"]));
    assert!(parse(&["--to-upstream"]));
    assert!(parse(&["--to-internal", "--complete"]));
}

#[test]
fn amend_message_flags_need_complete() {
    let parse = |args: &[&str]| {
        let argv = ["git-uplink", "amend", "upl_x"].iter().chain(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(parse(&[]));
    assert!(parse(&["--complete"]));
    assert!(parse(&["--complete", "--message-file", "m.txt"]));
    assert!(!parse(&["--message-file", "m.txt"]));
    assert!(!parse(&["--complete", "--title", "T"]));
    assert!(!parse(&[
        "--complete",
        "--message",
        "m",
        "--message-file",
        "f"
    ]));
}

#[test]
fn resolve_and_transfer_take_the_merged_pr_message() {
    let parse = |cmd: &[&str], args: &[&str]| {
        let argv = ["git-uplink"].iter().chain(cmd).chain(args);
        Cli::try_parse_from(argv).is_ok()
    };
    let resolve = ["resolve", "upl_x"];
    assert!(parse(&resolve, &["--message-file", "m.txt"]));
    assert!(!parse(&resolve, &["--title", "T"]));
    let transfer = ["transfer", "upl_x", "--to-upstream"];
    assert!(parse(&transfer, &["--complete", "--message-file", "m.txt"]));
    assert!(!parse(&transfer, &["--message-file", "m.txt"]));
}

#[test]
fn add_needs_a_message() {
    let parse = |args: &[&str]| {
        let argv = ["git-uplink", "add"].iter().chain(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(!parse(&[]));
    assert!(!parse(&["--title", "T"]));
    assert!(parse(&["--message", "Subject"]));
    assert!(parse(&["--message-file", "m.txt"]));
}

#[test]
fn assess_patch_accepts_a_message_override() {
    let parse = |args: &[&str]| {
        let argv = ["git-uplink", "assess", "--patch", "upl_x"]
            .iter()
            .chain(args);
        Cli::try_parse_from(argv).is_ok()
    };
    assert!(parse(&["--message-file", "m.txt"]));
    assert!(!parse(&["--internal-only"]));
}

#[test]
fn merged_via_is_parsed_by_clap() {
    assert_eq!(
        merged_via(&["--via", "stable-patch-id"]),
        Some(MergeVia::StablePatchId)
    );
    assert_eq!(
        merged_via(&["--via", "empty-rebase"]),
        Some(MergeVia::EmptyRebase)
    );
    assert_eq!(merged_via(&[]), Some(MergeVia::Manual));
    assert_eq!(merged_via(&["--via", "bogus"]), None);
}

#[test]
fn paths_under_the_legacy_folder_are_found_however_written() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    for path in [
        ".uplink/x.md",
        "./.uplink/x.md",
        "docs/../.uplink/x.md",
        ".uplink",
    ] {
        assert!(under_legacy_folder(repo, Path::new(path)), "{path}");
    }
    assert!(under_legacy_folder(repo, &repo.join(".uplink/x.md")));
    for path in ["out/x.md", ".uplinks/x.md", "docs/.uplink/x.md"] {
        assert!(!under_legacy_folder(repo, Path::new(path)), "{path}");
    }
    assert!(!under_legacy_folder(repo, Path::new("/tmp/.uplink/x.md")));
}
