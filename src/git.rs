use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

const BOT_NAME: &str = "Uplink Bot";
const BOT_EMAIL: &str = "uplink@company.example";

const IDENTITY_CONFIG: &[(&str, &str)] = &[
    ("user.name", BOT_NAME),
    ("user.email", BOT_EMAIL),
    ("commit.gpgsign", "false"),
    ("tag.gpgsign", "false"),
    ("push.gpgsign", "false"),
    ("advice.detachedHead", "false"),
];

const NETWORK_COMMANDS: &[&str] = &["fetch", "push", "ls-remote", "clone", "pull"];

const FLAGS_TAKING_VALUE: &[&str] = &[
    "-o",
    "--push-option",
    "--upload-pack",
    "--receive-pack",
    "--exec",
    "--recurse-submodules",
    "--jobs",
    "-j",
    "--depth",
    "--shallow-since",
    "--shallow-exclude",
    "--negotiation-tip",
    "--deepen",
    "--server-option",
    "--refmap",
    "--filter",
    "--keep",
    "-c",
    "-C",
];

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

#[derive(Default, Clone)]
pub struct GitOpts<'a> {
    pub allow_fail: bool,
    pub input: Option<&'a [u8]>,
    pub extra_env: Vec<(String, String)>,
}

#[derive(Default, Debug)]
struct Transport {
    remote_url: Option<String>,
    extra_header: Option<String>,
    /// `scheme://host[:port]` for blanking `http.<origin>/.extraheader` (Actions checkout).
    http_origin: Option<String>,
    ssh_command: Option<String>,
    isolate_gitconfig: bool,
}

fn strip_nl(s: String) -> String {
    s.strip_suffix('\n').unwrap_or(&s).to_string()
}

fn env_lookup(opts: &GitOpts<'_>, key: &str) -> Option<String> {
    if let Some((_, value)) = opts.extra_env.iter().rev().find(|(k, _)| k == key) {
        return if value.is_empty() {
            None
        } else {
            Some(value.clone())
        };
    }
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn network_remote_index(args: &[&str]) -> Option<usize> {
    let cmd = args.first()?;
    if !NETWORK_COMMANDS.contains(cmd) {
        return None;
    }
    let mut i = 1;
    while i < args.len() {
        let arg = args[i];
        if arg == "--" {
            return (i + 1 < args.len()).then_some(i + 1);
        }
        if arg.starts_with('-') {
            if arg.contains('=') {
                i += 1;
                continue;
            }
            if FLAGS_TAKING_VALUE.contains(&arg) {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        return Some(i);
    }
    None
}

fn is_explicit_url(spec: &str) -> bool {
    spec.contains("://") || spec.starts_with("git@")
}

fn is_local_transport(url: &str) -> bool {
    let url = url.trim();
    if url.starts_with("file://") {
        return true;
    }
    if url.contains("://") {
        return false;
    }
    if url.contains('@') {
        return false;
    }
    true
}

fn ssh_to_https(url: &str) -> Option<String> {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        let path = path.trim_start_matches('/');
        return Some(format!("https://{host}/{path}"));
    }
    let rest = url.strip_prefix("ssh://")?;
    let rest = rest.strip_prefix("git@").unwrap_or(rest);
    let (hostport, path) = rest.split_once('/')?;
    let host = match hostport.rsplit_once(':') {
        Some((name, port)) if port == "22" => name,
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            return Some(format!("https://{name}:{port}/{path}"));
        }
        _ => hostport,
    };
    Some(format!("https://{host}/{path}"))
}

/// Origin used as the `http.<url>.*` subsection: `scheme://host` or `scheme://host:port`.
/// Default ports 80/443 are omitted so the key matches actions/checkout (`https://github.com/`).
fn http_origin(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|part| !part.is_empty())?;
    let hostport = authority.rsplit('@').next()?;
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').filter(|p| !p.is_empty());
        (format!("[{host}]"), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
                (host.to_string(), Some(port))
            }
            _ => (hostport.to_string(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let drop_default_port = matches!(
        (scheme, port),
        ("http", Some("80")) | ("https", Some("443"))
    );
    match port.filter(|_| !drop_default_port) {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(BASE64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_ALPHABET[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// GitHub git smart-HTTP wants Basic `x-access-token`, not Bearer (REST API).
fn git_http_extra_header(token: &str) -> String {
    format!(
        "Authorization: Basic {}",
        base64_encode(format!("x-access-token:{token}").as_bytes())
    )
}

fn resolve_remote_url(cwd: &Path, spec: &str, opts: &GitOpts<'_>) -> Result<String> {
    if is_explicit_url(spec) || spec.starts_with("file://") {
        return Ok(spec.to_string());
    }
    if spec.starts_with('/') || spec.starts_with('.') {
        return Ok(spec.to_string());
    }
    let looked_up = git_inner(
        cwd,
        &["remote", "get-url", spec],
        GitOpts {
            allow_fail: true,
            extra_env: opts.extra_env.clone(),
            ..GitOpts::default()
        },
        false,
    )?;
    if looked_up.code == 0 && !looked_up.stdout.is_empty() {
        Ok(looked_up.stdout)
    } else {
        Ok(spec.to_string())
    }
}

const NETWORK_AUTH_HELP: &str = "Network git needs UPLINK_INTERNAL_KEY or UPLINK_INTERNAL_TOKEN (origin), UPLINK_CONTRIB_KEY or UPLINK_CONTRIB_TOKEN (contrib), or UPLINK_UPSTREAM_KEY or UPLINK_UPSTREAM_TOKEN (upstream). Keys must be passwordless. git-uplink does not use the operator SSH agent, GITHUB_TOKEN, or a commit signing key.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthRole {
    Internal,
    Contrib,
    Upstream,
}

impl AuthRole {
    fn key_env(self) -> &'static str {
        match self {
            Self::Internal => "UPLINK_INTERNAL_KEY",
            Self::Contrib => "UPLINK_CONTRIB_KEY",
            Self::Upstream => "UPLINK_UPSTREAM_KEY",
        }
    }

    fn token_env(self) -> &'static str {
        match self {
            Self::Internal => "UPLINK_INTERNAL_TOKEN",
            Self::Contrib => "UPLINK_CONTRIB_TOKEN",
            Self::Upstream => "UPLINK_UPSTREAM_TOKEN",
        }
    }

    fn remote_label(self) -> &'static str {
        match self {
            Self::Internal => "origin",
            Self::Contrib => "contrib",
            Self::Upstream => "upstream",
        }
    }

    fn help(self) -> String {
        format!(
            "Network git to {} needs {} or {}. Keys must be passwordless. git-uplink does not use the operator SSH agent, GITHUB_TOKEN, or a commit signing key.",
            self.remote_label(),
            self.key_env(),
            self.token_env()
        )
    }
}

fn normalize_remote_url(url: &str) -> String {
    let url = ssh_to_https(url).unwrap_or_else(|| url.trim().to_string());
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase()
}

fn urls_match(a: &str, b: &str) -> bool {
    normalize_remote_url(a) == normalize_remote_url(b)
}

fn remote_get_url(cwd: &Path, name: &str, opts: &GitOpts<'_>) -> Option<String> {
    let looked_up = git_inner(
        cwd,
        &["remote", "get-url", name],
        GitOpts {
            allow_fail: true,
            extra_env: opts.extra_env.clone(),
            ..GitOpts::default()
        },
        false,
    )
    .ok()?;
    if looked_up.code == 0 && !looked_up.stdout.is_empty() {
        Some(looked_up.stdout)
    } else {
        None
    }
}

fn named_auth_role(spec: &str) -> Option<AuthRole> {
    match spec {
        "origin" => Some(AuthRole::Internal),
        "contrib" => Some(AuthRole::Contrib),
        "upstream" => Some(AuthRole::Upstream),
        _ => None,
    }
}

fn auth_role_for(cwd: &Path, spec: &str, url: &str, opts: &GitOpts<'_>) -> Result<AuthRole> {
    if let Some(role) = named_auth_role(spec) {
        return Ok(role);
    }
    for (name, role) in [
        ("origin", AuthRole::Internal),
        ("upstream", AuthRole::Upstream),
        ("contrib", AuthRole::Contrib),
    ] {
        if let Some(remote_url) = remote_get_url(cwd, name, opts) {
            if urls_match(&remote_url, url) {
                return Ok(role);
            }
        }
    }
    if !is_explicit_url(spec)
        && !spec.starts_with('/')
        && !spec.starts_with('.')
        && spec != "origin"
        && spec != "upstream"
        && remote_get_url(cwd, spec, opts).is_some()
    {
        return Ok(AuthRole::Contrib);
    }
    Err(Error::msg(NETWORK_AUTH_HELP))
}

fn ssh_command_for_key(key: &str) -> String {
    format!(
        "ssh -o BatchMode=yes -o IdentitiesOnly=yes -i {}",
        shell_quote(key)
    )
}

fn transport_from_role(url: &str, role: AuthRole, opts: &GitOpts<'_>) -> Result<Transport> {
    if let Some(key) = env_lookup(opts, role.key_env()) {
        return Ok(Transport {
            ssh_command: Some(ssh_command_for_key(&key)),
            isolate_gitconfig: true,
            ..Transport::default()
        });
    }
    if let Some(token) = env_lookup(opts, role.token_env()) {
        let remote_url = ssh_to_https(url).unwrap_or_else(|| url.to_string());
        return Ok(Transport {
            http_origin: http_origin(&remote_url),
            remote_url: Some(remote_url),
            extra_header: Some(git_http_extra_header(&token)),
            ssh_command: None,
            isolate_gitconfig: true,
        });
    }
    Err(Error::msg(role.help()))
}

fn transport_for(cwd: &Path, args: &[&str], opts: &GitOpts<'_>) -> Result<Transport> {
    let Some(index) = network_remote_index(args) else {
        return Ok(Transport::default());
    };
    let spec = args[index];
    let url = resolve_remote_url(cwd, spec, opts)?;
    if is_local_transport(&url) {
        return Ok(Transport::default());
    }
    let role = auth_role_for(cwd, spec, &url, opts)?;
    transport_from_role(&url, role, opts)
}

pub fn git(cwd: &Path, args: &[&str], opts: GitOpts<'_>) -> Result<GitResult> {
    git_inner(cwd, args, opts, true)
}

fn git_inner(
    cwd: &Path,
    args: &[&str],
    opts: GitOpts<'_>,
    isolate_transport: bool,
) -> Result<GitResult> {
    let transport = if isolate_transport {
        transport_for(cwd, args, &opts)?
    } else {
        Transport::default()
    };

    let mut child_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if let (Some(index), Some(url)) = (network_remote_index(args), transport.remote_url.as_ref()) {
        child_args[index] = url.clone();
    }

    let mut cmd = Command::new("git");
    for (key, value) in IDENTITY_CONFIG {
        cmd.arg("-c").arg(format!("{key}={value}"));
    }
    if let Some(header) = &transport.extra_header {
        cmd.arg("-c").arg("credential.helper=");
        // actions/checkout persist-credentials writes http.<origin>/.extraheader
        // (GITHUB_TOKEN). Empty `-c` overrides that multi-value; a following `-c`
        // on the same key supplies UPLINK_INTERNAL_TOKEN / UPLINK_CONTRIB_TOKEN /
        // UPLINK_UPSTREAM_TOKEN. Generic http.extraHeader is shadowed once the
        // URL-specific key exists.
        if let Some(origin) = &transport.http_origin {
            cmd.arg("-c").arg(format!("http.{origin}/.extraheader="));
            cmd.arg("-c")
                .arg(format!("http.{origin}/.extraheader={header}"));
        } else {
            cmd.arg("-c").arg(format!("http.extraHeader={header}"));
        }
    }
    if transport.isolate_gitconfig {
        cmd.arg("-c").arg("safe.directory=*");
    }
    cmd.current_dir(cwd)
        .args(&child_args)
        .env("GIT_AUTHOR_NAME", BOT_NAME)
        .env("GIT_AUTHOR_EMAIL", BOT_EMAIL)
        .env("GIT_COMMITTER_NAME", BOT_NAME)
        .env("GIT_COMMITTER_EMAIL", BOT_EMAIL)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &opts.extra_env {
        if value.is_empty() {
            cmd.env_remove(key);
        } else {
            cmd.env(key, value);
        }
    }
    cmd.env_remove("GIT_SSH").env_remove("GIT_SSH_COMMAND");
    if transport.isolate_gitconfig {
        cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
    }
    if let Some(ssh_command) = &transport.ssh_command {
        cmd.env("GIT_SSH_COMMAND", ssh_command);
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

/// Identity, signing, and detached-HEAD advice are process-scoped in [`git`].
/// This stays for callers and tests; it does not write those values into the repo.
pub fn configure_repo(_cwd: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AuthRole, GitOpts, git_http_extra_header, http_origin, is_local_transport, named_auth_role,
        network_remote_index, ssh_to_https, transport_from_role, urls_match,
    };

    #[test]
    fn git_http_header_is_basic_x_access_token() {
        let header = git_http_extra_header("test-token");
        assert_eq!(
            header,
            "Authorization: Basic eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg=="
        );
        assert!(header.ends_with("=="), "standard base64 must keep padding");
        assert!(!header.contains("Bearer"));
        assert!(!header.contains('\n'));

        let padded = git_http_extra_header("a");
        assert_eq!(padded, "Authorization: Basic eC1hY2Nlc3MtdG9rZW46YQ==");
        assert!(!padded.contains('\n'));
    }

    #[test]
    fn network_remote_index_skips_flags() {
        assert_eq!(
            network_remote_index(&[
                "fetch",
                "--quiet",
                "--prune",
                "origin",
                "+refs/heads/main:refs/remotes/origin/main"
            ]),
            Some(3)
        );
        assert_eq!(
            network_remote_index(&[
                "push",
                "--force-with-lease=refs/heads/main:abc",
                "origin",
                "HEAD:refs/heads/main",
            ]),
            Some(2)
        );
        assert_eq!(
            network_remote_index(&["clone", "--quiet", "/tmp/repo.git", "/tmp/dest"]),
            Some(2)
        );
        assert_eq!(network_remote_index(&["commit", "-m", "msg"]), None);
        assert_eq!(network_remote_index(&["remote", "get-url", "origin"]), None);
    }

    #[test]
    fn ssh_urls_rewrite_to_https() {
        assert_eq!(
            ssh_to_https("git@github.com:acme/app.git").as_deref(),
            Some("https://github.com/acme/app.git")
        );
        assert_eq!(
            ssh_to_https("ssh://git@github.example.com/acme/app.git").as_deref(),
            Some("https://github.example.com/acme/app.git")
        );
        assert_eq!(
            ssh_to_https("ssh://git@github.com:22/acme/app.git").as_deref(),
            Some("https://github.com/acme/app.git")
        );
        assert_eq!(ssh_to_https("https://github.com/acme/app.git"), None);
    }

    #[test]
    fn http_origin_matches_checkout_extraheader_subsection() {
        assert_eq!(
            http_origin("https://github.com/acme/app.git").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            http_origin("https://github.example.com/acme/app.git").as_deref(),
            Some("https://github.example.com")
        );
        assert_eq!(
            http_origin("http://127.0.0.1:12345/repo.git").as_deref(),
            Some("http://127.0.0.1:12345")
        );
        assert_eq!(
            http_origin("https://github.com:443/acme/app.git").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            http_origin("http://example.com:80/x").as_deref(),
            Some("http://example.com")
        );
        assert_eq!(
            http_origin("https://github.com:8443/acme/app.git").as_deref(),
            Some("https://github.com:8443")
        );
        assert_eq!(
            http_origin("https://x-access-token:tok@github.com/acme/app.git").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            http_origin(&ssh_to_https("git@github.com:acme/app.git").unwrap()).as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            http_origin(&ssh_to_https("ssh://git@github.example.com/acme/app.git").unwrap())
                .as_deref(),
            Some("https://github.example.com")
        );
        assert_eq!(http_origin("file:///tmp/bare.git"), None);
        assert_eq!(http_origin("git@github.com:acme/app.git"), None);
    }

    #[test]
    fn file_and_path_remotes_are_local() {
        assert!(is_local_transport("/tmp/bare.git"));
        assert!(is_local_transport("file:///tmp/bare.git"));
        assert!(is_local_transport("../bare.git"));
        assert!(!is_local_transport("git@github.com:acme/app.git"));
        assert!(!is_local_transport(
            "ssh://git@example.invalid/org/repo.git"
        ));
        assert!(!is_local_transport("https://github.com/acme/app.git"));
    }

    #[test]
    fn named_remotes_map_to_auth_roles() {
        assert_eq!(named_auth_role("origin"), Some(AuthRole::Internal));
        assert_eq!(named_auth_role("contrib"), Some(AuthRole::Contrib));
        assert_eq!(named_auth_role("upstream"), Some(AuthRole::Upstream));
        assert_eq!(named_auth_role("other"), None);
    }

    #[test]
    fn ssh_and_https_remote_urls_match() {
        assert!(urls_match(
            "git@github.com:acme/app.git",
            "https://github.com/acme/app"
        ));
        assert!(!urls_match(
            "https://github.com/acme/app.git",
            "https://github.com/acme/other.git"
        ));
    }

    #[test]
    fn key_wins_over_token_for_role() {
        let opts = GitOpts {
            extra_env: vec![
                ("UPLINK_INTERNAL_KEY".into(), "/tmp/id_uplink".into()),
                ("UPLINK_INTERNAL_TOKEN".into(), "pat-token".into()),
            ],
            ..GitOpts::default()
        };
        let transport =
            transport_from_role("git@github.com:acme/app.git", AuthRole::Internal, &opts).unwrap();
        assert!(
            transport
                .ssh_command
                .as_deref()
                .is_some_and(|cmd| cmd.contains("/tmp/id_uplink") && cmd.contains("BatchMode=yes")),
            "key should drive GIT_SSH_COMMAND: {:?}",
            transport.ssh_command
        );
        assert!(transport.extra_header.is_none());
        assert!(transport.remote_url.is_none());
    }

    #[test]
    fn token_rewrites_ssh_origin_to_https() {
        let opts = GitOpts {
            extra_env: vec![("UPLINK_INTERNAL_TOKEN".into(), "pat-token".into())],
            ..GitOpts::default()
        };
        let transport =
            transport_from_role("git@github.com:acme/app.git", AuthRole::Internal, &opts).unwrap();
        assert_eq!(
            transport.remote_url.as_deref(),
            Some("https://github.com/acme/app.git")
        );
        assert!(
            transport
                .extra_header
                .as_deref()
                .is_some_and(|h| h.starts_with("Authorization: Basic "))
        );
        assert!(transport.ssh_command.is_none());
    }

    #[test]
    fn missing_role_creds_name_that_role() {
        let opts = GitOpts::default();
        let err = transport_from_role("https://github.com/acme/app.git", AuthRole::Upstream, &opts)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("UPLINK_UPSTREAM_KEY"));
        assert!(message.contains("UPLINK_UPSTREAM_TOKEN"));
        assert!(!message.contains("UPLINK_CONTRIB_TOKEN"));
    }
}
