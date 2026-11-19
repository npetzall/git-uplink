use crate::pattern::{github_owner_repo, number_after};

pub fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let (owner, name) = github_owner_repo(url)?;
    Some((owner.to_string(), name.trim_end_matches(".git").to_string()))
}

pub fn parse_pull_request_url(url: &str) -> Option<u64> {
    number_after(url, &["/pulls/", "/pull/"])?.parse().ok()
}

pub fn parse_issue_url(url: &str) -> Option<u64> {
    number_after(url, &["/issues/"])?.parse().ok()
}

#[cfg(test)]
#[path = "github_test.rs"]
mod tests;
