use std::sync::LazyLock;

use regex::Regex;

static GITHUB_REPO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com[:/](.+?)/(.+?)(?:\.git)?/?$").expect("github repo regex")
});
static PULL_REQUEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/pulls?/(\d+)(?:/|$|\?)").expect("pull request regex"));
static ISSUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/issues/(\d+)(?:/|$|\?)").expect("issue regex"));

pub fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let caps = GITHUB_REPO.captures(url)?;
    Some((
        caps[1].to_string(),
        caps[2].trim_end_matches(".git").to_string(),
    ))
}

pub fn parse_pull_request_url(url: &str) -> Option<u64> {
    PULL_REQUEST.captures(url)?.get(1)?.as_str().parse().ok()
}

pub fn parse_issue_url(url: &str) -> Option<u64> {
    ISSUE.captures(url)?.get(1)?.as_str().parse().ok()
}

#[cfg(test)]
#[path = "github_test.rs"]
mod tests;
