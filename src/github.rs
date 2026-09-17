use regex::Regex;

pub fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let re = Regex::new(r"github\.com[:/](.+?)/(.+?)(?:\.git)?/?$").ok()?;
    let caps = re.captures(url)?;
    Some((
        caps[1].to_string(),
        caps[2].trim_end_matches(".git").to_string(),
    ))
}

pub fn parse_pull_request_url(url: &str) -> Option<u64> {
    let re = Regex::new(r"/pulls?/(\d+)(?:/|$|\?)").ok()?;
    re.captures(url)?.get(1)?.as_str().parse().ok()
}

pub fn parse_issue_url(url: &str) -> Option<u64> {
    let re = Regex::new(r"/issues/(\d+)(?:/|$|\?)").ok()?;
    re.captures(url)?.get(1)?.as_str().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
