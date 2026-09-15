use std::io::Write;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct GithubPr {
    pub number: u64,
    pub url: String,
    pub merged: bool,
    pub merge_commit_sha: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GithubConfig {
    pub token: String,
    pub api_url: Option<String>,
    pub upstream_owner: String,
    pub upstream_repo: String,
    pub contrib_owner: String,
    pub contrib_repo: String,
}

fn api(config: &GithubConfig) -> String {
    config
        .api_url
        .clone()
        .unwrap_or_else(|| "https://api.github.com".into())
}

fn request_json<T: serde::de::DeserializeOwned>(
    config: &GithubConfig,
    method: &str,
    pathname: &str,
    body: Option<&serde_json::Value>,
) -> Result<T> {
    let url = format!("{}{pathname}", api(config));
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",
        "-X",
        method,
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        &format!("Authorization: Bearer {}", config.token),
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        "-w",
        "\n%{http_code}",
    ]);
    if body.is_some() {
        cmd.args([
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ]);
    }
    cmd.arg(&url).stdout(Stdio::piped()).stderr(Stdio::piped());
    if body.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn()?;
    if let Some(body) = body {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(body.to_string().as_bytes())?;
        }
    }
    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (payload, status) = stdout.rsplit_once('\n').unwrap_or((&stdout, "000"));
    let status: u16 = status.trim().parse().unwrap_or(0);
    if !output.status.success() || !(200..300).contains(&status) {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Error::msg(format!(
            "GitHub API {method} {pathname} failed ({status}): {} {err}",
            payload.trim()
        )));
    }
    serde_json::from_str(payload).map_err(|e| Error::msg(e.to_string()))
}

#[derive(Deserialize)]
struct PullJson {
    number: u64,
    html_url: String,
    merged: bool,
    merge_commit_sha: Option<String>,
}

pub fn create_upstream_pull_request(
    config: &GithubConfig,
    title: &str,
    body: &str,
    head_branch: &str,
    base_branch: &str,
) -> Result<GithubPr> {
    let payload = serde_json::json!({
        "title": title,
        "body": body,
        "head": format!("{}:{head_branch}", config.contrib_owner),
        "base": base_branch,
    });
    let created: PullJson = request_json(
        config,
        "POST",
        &format!(
            "/repos/{}/{}/pulls",
            config.upstream_owner, config.upstream_repo
        ),
        Some(&payload),
    )?;
    Ok(GithubPr {
        number: created.number,
        url: created.html_url,
        merged: created.merged,
        merge_commit_sha: created.merge_commit_sha,
    })
}

pub fn get_pull_request(config: &GithubConfig, number: u64) -> Result<GithubPr> {
    let pr: PullJson = request_json(
        config,
        "GET",
        &format!(
            "/repos/{}/{}/pulls/{number}",
            config.upstream_owner, config.upstream_repo
        ),
        None,
    )?;
    Ok(GithubPr {
        number: pr.number,
        url: pr.html_url,
        merged: pr.merged,
        merge_commit_sha: pr.merge_commit_sha,
    })
}

pub fn comment_on_issue(
    token: &str,
    api_url: Option<&str>,
    owner: &str,
    repo: &str,
    issue_number: u64,
    body: &str,
) -> Result<()> {
    let config = GithubConfig {
        token: token.into(),
        api_url: api_url.map(str::to_string),
        upstream_owner: owner.into(),
        upstream_repo: repo.into(),
        contrib_owner: owner.into(),
        contrib_repo: repo.into(),
    };
    let payload = serde_json::json!({ "body": body });
    request_json::<serde_json::Value>(
        &config,
        "POST",
        &format!("/repos/{owner}/{repo}/issues/{issue_number}/comments"),
        Some(&payload),
    )?;
    Ok(())
}

pub fn parse_github_repo(url: &str) -> Option<(String, String)> {
    let re = regex::Regex::new(r"github\.com[:/](.+?)/(.+?)(?:\.git)?$").ok()?;
    let caps = re.captures(url)?;
    Some((
        caps[1].to_string(),
        caps[2].trim_end_matches(".git").to_string(),
    ))
}
