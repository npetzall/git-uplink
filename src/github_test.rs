use super::*;

#[test]
fn static_patterns_compile() {
    for re in [&GITHUB_REPO, &PULL_REQUEST, &ISSUE] {
        LazyLock::force(re);
    }
}

#[test]
fn parses_https_and_ssh_github_remotes() {
    assert_eq!(
        parse_github_repo("https://github.com/upstream/tokenkit.git"),
        Some(("upstream".into(), "tokenkit".into()))
    );
    assert_eq!(
        parse_github_repo("git@github.com:contrib/tokenkit.git"),
        Some(("contrib".into(), "tokenkit".into()))
    );
}

#[test]
fn parses_pr_and_issue_urls() {
    assert_eq!(
        parse_pull_request_url("https://github.com/upstream/tokenkit/pull/99"),
        Some(99)
    );
    assert_eq!(
        parse_issue_url("https://github.com/acme/product/issues/12"),
        Some(12)
    );
}
