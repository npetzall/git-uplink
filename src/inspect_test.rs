use super::*;
use crate::types::QueueConfig;

#[test]
fn remotes_configured_matches_ssh_and_https_forms() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git_ok(repo, &["init", "-q"]).unwrap();
    git_ok(
        repo,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/acme/app.git",
        ],
    )
    .unwrap();
    let queue = QueueState::empty(QueueConfig {
        upstream_url: Some("git@github.com:acme/app.git".into()),
        ..QueueConfig::default()
    });
    let outcome = check_remotes_configured(repo, &queue);
    assert_eq!(
        outcome.status,
        crate::types::CheckStatus::Pass,
        "{}",
        outcome.detail
    );
}

#[test]
fn relative_path_contrib_needs_no_credentials() {
    let queue = QueueState::empty(QueueConfig {
        contrib_url: Some("../contrib.git".into()),
        ..QueueConfig::default()
    });
    assert_eq!(
        check_credentials_contrib(&queue).status,
        crate::types::CheckStatus::Skip
    );
}
