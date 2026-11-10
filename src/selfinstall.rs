//! Versioned installs of the binary itself under `~/.uplink`.
//!
//! Each version is kept as `bin/git-uplink-X.Y.Z`, and `bin/git-uplink` is a
//! relative symlink to the active one. Downloads use `curl` and are checked
//! the way the forge pack's install action checks them: the sha256 against
//! the release's `SHA256SUMS`, the cosign bundle against the signer identity
//! in `config.toml`, and the version the binary reports against the tag.

use std::env;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const DEFAULT_REPO: &str = "npetzall/git-uplink";
const OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
const LINK: &str = "git-uplink";
const PREFIX: &str = "git-uplink-";
const AVAILABLE: usize = 10;
const COSIGN_HINT: &str = "https://docs.sigstore.dev/cosign/system_config/installation/";

/// Where installs live and where releases are fetched from.
#[derive(Debug, Clone)]
pub struct SelfEnv {
    pub home: PathBuf,
    pub releases_base: String,
    pub api_base: String,
}

impl SelfEnv {
    /// `UPLINK_HOME` or `~/.uplink`. `UPLINK_RELEASES_BASE` and
    /// `UPLINK_API_BASE` replace the GitHub hosts, for tests.
    pub fn from_env() -> Result<Self> {
        let home = match env::var_os("UPLINK_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => env::home_dir()
                .ok_or_else(|| Error::msg("cannot find the home directory; set UPLINK_HOME"))?
                .join(".uplink"),
        };
        let base = |name: &str, default: &str| {
            env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
                .trim_end_matches('/')
                .to_string()
        };
        Ok(Self {
            home,
            releases_base: base("UPLINK_RELEASES_BASE", "https://github.com"),
            api_base: base("UPLINK_API_BASE", "https://api.github.com"),
        })
    }

    fn bin(&self) -> PathBuf {
        self.home.join("bin")
    }
}

/// The repository releases come from, and the identity that must have signed them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfConfig {
    pub repo: String,
    pub signer: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    repo: Option<String>,
    signer: Option<String>,
}

/// The release workflow identity of `repo`.
pub fn default_signer(repo: &str) -> String {
    format!("https://github.com/{repo}/.github/workflows/release.yml@refs/heads/main")
}

impl SelfConfig {
    /// Parses `config.toml`; missing keys fall back to the defaults.
    pub fn parse(text: &str) -> Result<Self> {
        let file: ConfigFile =
            toml::from_str(text).map_err(|e| Error::msg(format!("config.toml: {e}")))?;
        let repo = file.repo.unwrap_or_else(|| DEFAULT_REPO.to_string());
        if !valid_repo(&repo) {
            return Err(Error::msg(format!(
                "config.toml: repo must be owner/name, got {repo:?}"
            )));
        }
        let signer = file.signer.unwrap_or_else(|| default_signer(&repo));
        if !signer.starts_with("https://") {
            return Err(Error::msg(format!(
                "config.toml: signer must be an https:// workflow identity, got {signer:?}"
            )));
        }
        Ok(Self { repo, signer })
    }

    fn load(env: &SelfEnv) -> Result<Self> {
        match fs::read_to_string(env.home.join("config.toml")) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == ErrorKind::NotFound => Self::parse(""),
            Err(e) => Err(e.into()),
        }
    }
}

fn valid_repo(repo: &str) -> bool {
    let ok = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((owner, name)) if ok(owner) && ok(name))
}

/// `X.Y.Z` of `X.Y.Z` or `vX.Y.Z`; anything else is an error.
pub fn normalize_version(input: &str) -> Result<String> {
    let version = input.strip_prefix('v').unwrap_or(input);
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        Ok(version.to_string())
    } else {
        Err(Error::msg(format!(
            "not a version: {input:?} (expected X.Y.Z or vX.Y.Z)"
        )))
    }
}

fn version_key(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split('.').map(|p| p.parse().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// The release asset target of the platform this binary runs on.
pub fn host_target() -> Result<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        (os, arch) => Err(Error::msg(format!(
            "no release build for {os}/{arch}; install git-uplink manually"
        ))),
    }
}

/// The hash `SHA256SUMS` lists for `asset`.
pub fn expected_sha256(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        (name == asset).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Whether `git-uplink version` output is for `version`.
pub fn reports_version(output: &str, version: &str) -> bool {
    output
        .trim_start()
        .strip_prefix("git-uplink ")
        .and_then(|rest| rest.strip_prefix(version))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\n']))
}

/// Installed versions, newest first.
pub fn installed(env: &SelfEnv) -> Result<Vec<String>> {
    let mut versions = Vec::new();
    match fs::read_dir(env.bin()) {
        Ok(entries) => {
            for entry in entries {
                let name = entry?.file_name();
                if let Some(v) = name.to_str().and_then(|n| n.strip_prefix(PREFIX))
                    && normalize_version(v).is_ok_and(|n| n == v)
                {
                    versions.push(v.to_string());
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    versions.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
    Ok(versions)
}

/// The version `bin/git-uplink` points at.
pub fn active(env: &SelfEnv) -> Option<String> {
    let target = fs::read_link(env.bin().join(LINK)).ok()?;
    let name = target.file_name()?.to_str()?;
    Some(name.strip_prefix(PREFIX)?.to_string())
}

#[cfg(unix)]
fn switch_link(env: &SelfEnv, version: &str) -> Result<()> {
    let bin = env.bin();
    let link = bin.join(LINK);
    if let Ok(meta) = fs::symlink_metadata(&link)
        && !meta.file_type().is_symlink()
    {
        return Err(Error::msg(format!(
            "{} is not a symlink; move it away and retry",
            link.display()
        )));
    }
    let tmp = bin.join(format!(".{LINK}.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(format!("{PREFIX}{version}"), &tmp)?;
    fs::rename(&tmp, &link).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn switch_link(_env: &SelfEnv, _version: &str) -> Result<()> {
    Err(Error::msg(
        "versioned installs need symlinks; install git-uplink manually on this platform",
    ))
}

fn tool_missing(tool: &str, hint: &str) -> Error {
    Error::msg(format!(
        "{tool} is required but was not found on PATH; {hint}"
    ))
}

fn curl_proto(url: &str) -> &'static str {
    if url.starts_with("file://") {
        "=file"
    } else {
        "=https"
    }
}

/// Runs curl and returns its stdout.
fn curl(args: &[&str], url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            curl_proto(url),
            "--proto-redir",
            "=https",
        ])
        .args(args)
        .arg(url)
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => tool_missing("curl", "install it with your package manager"),
            _ => e.into(),
        })?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "curl {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let dest = dest.to_str().ok_or_else(|| Error::msg("non-UTF-8 path"))?;
    curl(&["-o", dest], url).map(drop)
}

/// The version `latest` points at, from the release page redirect.
fn resolve_latest(env: &SelfEnv, cfg: &SelfConfig) -> Result<String> {
    let url = format!("{}/{}/releases/latest", env.releases_base, cfg.repo);
    let effective = curl(&["-I", "-o", "/dev/null", "-w", "%{url_effective}"], &url)?;
    let tag = effective
        .trim()
        .rsplit_once("/tag/")
        .map(|(_, tag)| tag)
        .ok_or_else(|| Error::msg(format!("{} has no latest release", cfg.repo)))?;
    normalize_version(tag)
}

/// Removes the download directory however the install ends.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fetch_and_verify(env: &SelfEnv, cfg: &SelfConfig, version: &str) -> Result<()> {
    let target = host_target()?;
    let asset = format!("{PREFIX}{target}");
    let tmp = TempDir(
        env.home
            .join("tmp")
            .join(format!("v{version}-{}", std::process::id())),
    );
    let _ = fs::remove_dir_all(&tmp.0);
    fs::create_dir_all(&tmp.0)?;
    let base = format!(
        "{}/{}/releases/download/v{version}",
        env.releases_base, cfg.repo
    );
    eprintln!("Downloading v{version} ({target}) from {}", cfg.repo);
    let file = tmp.0.join(&asset);
    let bundle = tmp.0.join(format!("{asset}.sigstore.json"));
    let sums = tmp.0.join("SHA256SUMS");
    download(&format!("{base}/{asset}"), &file)?;
    download(&format!("{base}/{asset}.sigstore.json"), &bundle)?;
    download(&format!("{base}/SHA256SUMS"), &sums)?;

    let expected = expected_sha256(&fs::read_to_string(&sums)?, &asset)
        .ok_or_else(|| Error::msg(format!("SHA256SUMS of v{version} does not list {asset}")))?;
    let actual = sha256_file(&file)?;
    if actual != expected {
        return Err(Error::msg(format!(
            "sha256 mismatch for {asset}: SHA256SUMS lists {expected}, got {actual}"
        )));
    }
    eprintln!("sha256 matches SHA256SUMS");

    let output = Command::new("cosign")
        .arg("verify-blob")
        .arg(&file)
        .arg("--bundle")
        .arg(&bundle)
        .args(["--certificate-identity", &cfg.signer])
        .args(["--certificate-oidc-issuer", OIDC_ISSUER])
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => tool_missing("cosign", &format!("see {COSIGN_HINT}")),
            _ => e.into(),
        })?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "cosign could not verify {asset} as signed by {}: {}",
            cfg.signer,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    eprintln!("cosign signature by {}", cfg.signer);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))?;
    }
    let output = Command::new(&file).arg("version").output()?;
    let reported = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !reports_version(&reported, version) {
        return Err(Error::msg(format!(
            "the v{version} download reports {:?}, not version {version}",
            reported.trim()
        )));
    }

    fs::create_dir_all(env.bin())?;
    fs::rename(&file, env.bin().join(format!("{PREFIX}{version}")))?;
    Ok(())
}

fn path_hint(env: &SelfEnv) -> Option<String> {
    let bin = env.bin();
    let on_path = env::var_os("PATH").is_some_and(|p| env::split_paths(&p).any(|d| d == bin));
    (!on_path).then(|| {
        format!(
            "{} is not on PATH; see https://npetzall.github.io/git-uplink/install",
            bin.display()
        )
    })
}

/// `self-update`: installs `version` (default latest) and makes it active.
pub fn self_update(env: &SelfEnv, version: Option<&str>, no_switch: bool) -> Result<String> {
    let cfg = SelfConfig::load(env)?;
    let version = match version {
        None | Some("latest") => resolve_latest(env, &cfg)?,
        Some(v) => normalize_version(v)?,
    };
    let fresh = !installed(env)?.contains(&version);
    if fresh {
        fetch_and_verify(env, &cfg, &version)?;
    }
    let mut out = if fresh {
        format!("Installed {version}")
    } else {
        format!("{version} is already installed")
    };
    if !no_switch {
        switch_link(env, &version)?;
        out.push_str(&format!("; active version is now {version}"));
    }
    if let Some(hint) = path_hint(env) {
        eprintln!("{hint}");
    }
    Ok(out)
}

/// `self-use`: makes an installed version active.
pub fn self_use(env: &SelfEnv, version: &str) -> Result<String> {
    let version = normalize_version(version)?;
    let versions = installed(env)?;
    if !versions.contains(&version) {
        let have = if versions.is_empty() {
            "none".to_string()
        } else {
            versions.join(", ")
        };
        return Err(Error::msg(format!(
            "{version} is not installed (installed: {have}); run `git uplink self-update {version}`"
        )));
    }
    switch_link(env, &version)?;
    Ok(format!("Active version is now {version}"))
}

/// `self-list`: installed versions, the active one marked with `*`.
pub fn self_list(env: &SelfEnv) -> Result<String> {
    let cfg = SelfConfig::load(env)?;
    let versions = installed(env)?;
    let current = active(env);
    let mut out = format!("Repo:   {}\nSigner: {}\n", cfg.repo, cfg.signer);
    if versions.is_empty() {
        out.push_str(&format!(
            "No versions installed in {}; run `git uplink self-update`.",
            env.bin().display()
        ));
    }
    for v in &versions {
        let mark = if current.as_ref() == Some(v) {
            '*'
        } else {
            ' '
        };
        out.push_str(&format!("{mark} {v}\n"));
    }
    Ok(out.trim_end().to_string())
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// `self-available`: the latest published releases of the configured repo.
pub fn self_available(env: &SelfEnv) -> Result<String> {
    let cfg = SelfConfig::load(env)?;
    let url = format!("{}/repos/{}/releases", env.api_base, cfg.repo);
    let body = curl(&["-H", "Accept: application/vnd.github+json"], &url)?;
    let releases: Vec<Release> = serde_json::from_str(&body)?;
    let versions = installed(env)?;
    let current = active(env);
    let mut out = format!("Releases of {}:\n", cfg.repo);
    let shown: Vec<_> = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .take(AVAILABLE)
        .collect();
    if shown.is_empty() {
        out.push_str("none published");
    }
    for r in shown {
        let version = r.tag_name.strip_prefix('v').unwrap_or(&r.tag_name);
        let date = r
            .published_at
            .as_deref()
            .map_or("", |d| d.get(..10).unwrap_or(d));
        let (mark, state) = if current.as_deref() == Some(version) {
            ('*', "active")
        } else if versions.iter().any(|v| v == version) {
            (' ', "installed")
        } else {
            (' ', "")
        };
        let line = format!("{mark} {:<10} {date:<10} {state}", r.tag_name);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

/// `self-check`: whether a newer version than the active one is installed
/// or released.
pub fn self_check(env: &SelfEnv) -> Result<String> {
    let cfg = SelfConfig::load(env)?;
    let url = format!("{}/repos/{}/releases/latest", env.api_base, cfg.repo);
    let body = curl(&["-H", "Accept: application/vnd.github+json"], &url)?;
    let latest: Release = serde_json::from_str(&body)?;
    let latest = normalize_version(&latest.tag_name)?;
    let newest_installed = installed(env)?.into_iter().next();
    let Some(current) = active(env) else {
        return Ok(format!(
            "No version is active; run `git uplink self-update` to install {latest}."
        ));
    };
    let newer = |v: &str| version_key(v) > version_key(&current);
    // A release that is already installed is reported as installed only.
    let release_is_new = newer(&latest)
        && newest_installed
            .as_deref()
            .is_none_or(|v| version_key(&latest) > version_key(v));
    let mut lines = Vec::new();
    if let Some(v) = newest_installed.as_deref().filter(|v| newer(v)) {
        lines.push(format!(
            "Newer installed: {v} (active {current}); run `git uplink self-use {v}`"
        ));
    }
    if release_is_new {
        lines.push(format!(
            "Newer release: {latest} (active {current}); run `git uplink self-update`"
        ));
    }
    if lines.is_empty() {
        lines.push(format!("{current} is active and the latest release."));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_overrides() {
        let cfg = SelfConfig::parse("").unwrap();
        assert_eq!(cfg.repo, DEFAULT_REPO);
        assert_eq!(cfg.signer, default_signer(DEFAULT_REPO));

        let cfg = SelfConfig::parse("repo = \"acme/git-uplink\"\n").unwrap();
        assert_eq!(
            cfg.signer,
            "https://github.com/acme/git-uplink/.github/workflows/release.yml@refs/heads/main"
        );

        let cfg =
            SelfConfig::parse("repo = \"acme/x\"\nsigner = \"https://example.com/id\"\n").unwrap();
        assert_eq!(cfg.signer, "https://example.com/id");
    }

    #[test]
    fn config_rejects_bad_values() {
        assert!(SelfConfig::parse("repo = \"acme\"").is_err());
        assert!(SelfConfig::parse("repo = \"acme/../x\"").is_err());
        assert!(SelfConfig::parse("repo = \"a/b?c\"").is_err());
        assert!(SelfConfig::parse("signer = \"http://x\"").is_err());
        assert!(SelfConfig::parse("unknown = 1").is_err());
    }

    #[test]
    fn versions_normalize() {
        assert_eq!(normalize_version("v1.2.3").unwrap(), "1.2.3");
        assert_eq!(normalize_version("1.2.3").unwrap(), "1.2.3");
        for bad in ["1.2", "v1.2.3-rc1", "latest", "1..3", "a.b.c", ""] {
            assert!(normalize_version(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sums_lookup() {
        let sums =
            "AAAA  git-uplink-x86_64-unknown-linux-musl\nbbbb *git-uplink-aarch64-apple-darwin\n";
        assert_eq!(
            expected_sha256(sums, "git-uplink-x86_64-unknown-linux-musl").as_deref(),
            Some("aaaa")
        );
        assert_eq!(
            expected_sha256(sums, "git-uplink-aarch64-apple-darwin").as_deref(),
            Some("bbbb")
        );
        assert_eq!(expected_sha256(sums, "git-uplink-aarch64"), None);
    }

    #[test]
    fn version_output_match() {
        assert!(reports_version("git-uplink 0.5.0 (abc1234)\n", "0.5.0"));
        assert!(reports_version("git-uplink 0.5.0\n", "0.5.0"));
        assert!(!reports_version("git-uplink 0.5.01 (abc)", "0.5.0"));
        assert!(!reports_version("git-uplink 0.4.0 (abc)", "0.5.0"));
        assert!(!reports_version("other 0.5.0", "0.5.0"));
    }

    #[cfg(unix)]
    #[test]
    fn switch_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let env = SelfEnv {
            home: dir.path().to_path_buf(),
            releases_base: String::new(),
            api_base: String::new(),
        };
        fs::create_dir_all(env.bin()).unwrap();
        for v in ["0.9.0", "0.10.0", "0.2.1"] {
            fs::write(env.bin().join(format!("{PREFIX}{v}")), "").unwrap();
        }
        fs::write(env.bin().join("git-uplink-junk"), "").unwrap();
        assert_eq!(installed(&env).unwrap(), ["0.10.0", "0.9.0", "0.2.1"]);
        assert_eq!(active(&env), None);
        switch_link(&env, "0.9.0").unwrap();
        assert_eq!(active(&env).as_deref(), Some("0.9.0"));
        switch_link(&env, "0.10.0").unwrap();
        assert_eq!(
            fs::read_link(env.bin().join(LINK)).unwrap(),
            PathBuf::from("git-uplink-0.10.0")
        );
    }
}
