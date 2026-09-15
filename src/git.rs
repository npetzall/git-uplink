use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct GitResult {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Debug)]
pub struct GitError {
    pub args: Vec<String>,
    pub result: GitResult,
    message: String,
}

impl GitError {
    fn new(args: &[&str], result: GitResult) -> Self {
        let message = format!(
            "git {} failed ({}): {}",
            args.join(" "),
            result.code,
            if result.stderr.is_empty() {
                &result.stdout
            } else {
                &result.stderr
            }
        );
        Self {
            args: args.iter().map(|s| s.to_string()).collect(),
            result,
            message,
        }
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

#[derive(Default)]
pub struct GitOpts<'a> {
    pub allow_fail: bool,
    pub input: Option<&'a [u8]>,
    pub extra_env: Vec<(String, String)>,
}

fn strip_nl(s: String) -> String {
    s.strip_suffix('\n').unwrap_or(&s).to_string()
}

pub fn git(cwd: &Path, args: &[&str], opts: GitOpts<'_>) -> Result<GitResult> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Uplink Bot")
        .env("GIT_AUTHOR_EMAIL", "uplink@company.example")
        .env("GIT_COMMITTER_NAME", "Uplink Bot")
        .env("GIT_COMMITTER_EMAIL", "uplink@company.example")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &opts.extra_env {
        cmd.env(k, v);
    }
    if opts.input.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd.spawn().map_err(Error::from)?;
    if let Some(input) = opts.input {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input)?;
        }
    }
    let output = child.wait_with_output()?;
    let result = GitResult {
        stdout: strip_nl(String::from_utf8_lossy(&output.stdout).into_owned()),
        stderr: strip_nl(String::from_utf8_lossy(&output.stderr).into_owned()),
        code: output.status.code().unwrap_or(1),
    };
    if result.code != 0 && !opts.allow_fail {
        return Err(Error::Git(GitError::new(args, result)));
    }
    Ok(result)
}

pub fn git_ok(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(git(cwd, args, GitOpts::default())?.stdout)
}

pub fn configure_repo(cwd: &Path) -> Result<()> {
    git(
        cwd,
        &["config", "user.name", "Uplink Bot"],
        GitOpts::default(),
    )?;
    git(
        cwd,
        &["config", "user.email", "uplink@company.example"],
        GitOpts::default(),
    )?;
    git(
        cwd,
        &["config", "commit.gpgsign", "false"],
        GitOpts::default(),
    )?;
    git(
        cwd,
        &["config", "advice.detachedHead", "false"],
        GitOpts::default(),
    )?;
    Ok(())
}
