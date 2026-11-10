//! `self-*` commands against a temporary `UPLINK_HOME`, `file://` releases
//! and a fake `cosign` on `PATH`.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn fake_binary(version: &str) -> String {
    format!("#!/bin/sh\necho 'git-uplink {version} (abc1234)'\n")
}

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in ["home/bin", "fakebin", "releases", "api"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(
            root.join("home/config.toml"),
            "repo = \"acme/git-uplink\"\n",
        )
        .unwrap();
        Self { _dir: dir, root }
    }

    fn home_bin(&self) -> PathBuf {
        self.root.join("home/bin")
    }

    fn installed(&self, version: &str) {
        write_exec(
            &self.home_bin().join(format!("git-uplink-{version}")),
            &fake_binary(version),
        );
    }

    fn cosign(&self, ok: bool) {
        let code = if ok { 0 } else { 1 };
        write_exec(
            &self.root.join("fakebin/cosign"),
            &format!("#!/bin/sh\necho 'cosign says {code}' >&2\nexit {code}\n"),
        );
    }

    /// Publishes `body` as the asset of `tag`, with SHA256SUMS listing `sum`
    /// (the real hash when `None`).
    fn release(&self, tag: &str, body: &str, sum: Option<&str>) {
        let dir = self
            .root
            .join("releases/acme/git-uplink/releases/download")
            .join(tag);
        fs::create_dir_all(&dir).unwrap();
        let asset = format!("git-uplink-{}", target());
        fs::write(dir.join(&asset), body).unwrap();
        fs::write(dir.join(format!("{asset}.sigstore.json")), "{}").unwrap();
        let hash: String = Sha256::digest(body.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            dir.join("SHA256SUMS"),
            format!("{}  {asset}\n", sum.unwrap_or(&hash)),
        )
        .unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Command::new(env!("CARGO_BIN_EXE_git-uplink"))
            .args(args)
            .current_dir(&self.root)
            .env("UPLINK_HOME", self.root.join("home"))
            .env(
                "UPLINK_RELEASES_BASE",
                format!("file://{}", self.root.join("releases").display()),
            )
            .env(
                "UPLINK_API_BASE",
                format!("file://{}", self.root.join("api").display()),
            )
            .env("PATH", path)
            .output()
            .unwrap()
    }

    fn active(&self) -> Option<PathBuf> {
        fs::read_link(self.home_bin().join("git-uplink")).ok()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn list_marks_active_and_use_switches() {
    let sb = Sandbox::new();
    sb.installed("0.4.0");
    sb.installed("0.5.0");

    let out = sb.run(&["self-use", "v0.4.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(sb.active(), Some(PathBuf::from("git-uplink-0.4.0")));

    let out = sb.run(&["self-list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "Repo:   acme/git-uplink\n\
         Signer: https://github.com/acme/git-uplink/.github/workflows/release.yml@refs/heads/main\n  \
         0.5.0\n\
         * 0.4.0\n"
    );
}

#[test]
fn use_of_missing_version_fails() {
    let sb = Sandbox::new();
    sb.installed("0.4.0");
    let out = sb.run(&["self-use", "0.9.9"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("0.9.9 is not installed (installed: 0.4.0)"),
        "{}",
        stderr(&out)
    );
    assert_eq!(sb.active(), None);
}

#[test]
fn update_to_installed_version_only_switches() {
    let sb = Sandbox::new();
    sb.installed("0.4.0");
    let out = sb.run(&["self-update", "0.4.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("0.4.0 is already installed"));
    assert_eq!(sb.active(), Some(PathBuf::from("git-uplink-0.4.0")));
}

#[test]
fn update_downloads_verifies_and_switches() {
    let sb = Sandbox::new();
    sb.cosign(true);
    sb.release("v0.6.0", &fake_binary("0.6.0"), None);
    let out = sb.run(&["self-update", "v0.6.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(sb.home_bin().join("git-uplink-0.6.0").is_file());
    assert_eq!(sb.active(), Some(PathBuf::from("git-uplink-0.6.0")));
    assert!(!sb.root.join("home/tmp/v0.6.0").exists());
}

#[test]
fn update_no_switch_keeps_active() {
    let sb = Sandbox::new();
    sb.cosign(true);
    sb.installed("0.4.0");
    assert!(sb.run(&["self-use", "0.4.0"]).status.success());
    sb.release("v0.6.0", &fake_binary("0.6.0"), None);
    let out = sb.run(&["self-update", "0.6.0", "--no-switch"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(sb.home_bin().join("git-uplink-0.6.0").is_file());
    assert_eq!(sb.active(), Some(PathBuf::from("git-uplink-0.4.0")));
}

#[test]
fn update_rejects_sha_mismatch() {
    let sb = Sandbox::new();
    sb.cosign(true);
    sb.release("v0.6.0", &fake_binary("0.6.0"), Some(&"0".repeat(64)));
    let out = sb.run(&["self-update", "0.6.0"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("sha256 mismatch"), "{}", stderr(&out));
    assert!(!sb.home_bin().join("git-uplink-0.6.0").exists());
    assert_eq!(sb.active(), None);
}

#[test]
fn update_rejects_bad_signature() {
    let sb = Sandbox::new();
    sb.cosign(false);
    sb.release("v0.6.0", &fake_binary("0.6.0"), None);
    let out = sb.run(&["self-update", "0.6.0"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cosign could not verify"),
        "{}",
        stderr(&out)
    );
    assert!(!sb.home_bin().join("git-uplink-0.6.0").exists());
}

#[test]
fn update_rejects_binary_of_another_version() {
    let sb = Sandbox::new();
    sb.cosign(true);
    sb.release("v0.6.0", &fake_binary("0.5.0"), None);
    let out = sb.run(&["self-update", "0.6.0"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not version 0.6.0"),
        "{}",
        stderr(&out)
    );
    assert!(!sb.home_bin().join("git-uplink-0.6.0").exists());
}

#[test]
fn available_lists_published_releases() {
    let sb = Sandbox::new();
    sb.installed("0.5.0");
    sb.installed("0.4.0");
    assert!(sb.run(&["self-use", "0.5.0"]).status.success());
    let mut releases = vec![
        r#"{"tag_name":"v1.0.0","published_at":"2026-11-05T10:00:00Z","draft":true,"prerelease":false}"#.to_string(),
        r#"{"tag_name":"v0.11.1","published_at":"2026-11-04T10:00:00Z","draft":false,"prerelease":true}"#.to_string(),
    ];
    for minor in (0..12).rev() {
        releases.push(format!(
            r#"{{"tag_name":"v0.{minor}.0","published_at":"2026-10-{:02}T10:00:00Z","draft":false,"prerelease":false}}"#,
            minor + 1
        ));
    }
    let dir = sb.root.join("api/repos/acme/git-uplink");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("releases"), format!("[{}]", releases.join(","))).unwrap();

    let out = sb.run(&["self-available"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Releases of acme/git-uplink:");
    assert_eq!(lines.len(), 11, "{text}");
    assert!(lines[1].starts_with("  v0.11.0"), "{text}");
    assert!(
        !text.contains("v1.0.0") && !text.contains("v0.11.1"),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("* v0.5.0") && l.ends_with("active")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("  v0.4.0") && l.ends_with("installed")),
        "{text}"
    );
    assert!(!text.contains("v0.1.0"), "{text}");
}

fn latest_release(sb: &Sandbox, tag: &str) {
    let dir = sb.root.join("api/repos/acme/git-uplink/releases");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("latest"),
        format!(r#"{{"tag_name":"{tag}","published_at":"2026-11-01T00:00:00Z"}}"#),
    )
    .unwrap();
}

fn check(sb: &Sandbox) -> String {
    let out = sb.run(&["self-check"]);
    assert!(out.status.success(), "{}", stderr(&out));
    stdout(&out)
}

#[test]
fn check_up_to_date() {
    let sb = Sandbox::new();
    sb.installed("0.5.0");
    sb.installed("0.4.0");
    assert!(sb.run(&["self-use", "0.5.0"]).status.success());
    latest_release(&sb, "v0.5.0");
    assert_eq!(check(&sb), "0.5.0 is active and the latest release.\n");
}

#[test]
fn check_newer_installed_and_released() {
    let sb = Sandbox::new();
    sb.installed("0.10.0");
    sb.installed("0.9.0");
    assert!(sb.run(&["self-use", "0.9.0"]).status.success());

    latest_release(&sb, "v0.10.0");
    assert_eq!(
        check(&sb),
        "Newer installed: 0.10.0 (active 0.9.0); run `git uplink self-use 0.10.0`\n"
    );

    latest_release(&sb, "v0.11.0");
    assert_eq!(
        check(&sb),
        "Newer installed: 0.10.0 (active 0.9.0); run `git uplink self-use 0.10.0`\n\
         Newer release: 0.11.0 (active 0.9.0); run `git uplink self-update`\n"
    );

    assert!(sb.run(&["self-use", "0.10.0"]).status.success());
    assert_eq!(
        check(&sb),
        "Newer release: 0.11.0 (active 0.10.0); run `git uplink self-update`\n"
    );
}

#[test]
fn check_nothing_active() {
    let sb = Sandbox::new();
    latest_release(&sb, "v0.5.0");
    assert_eq!(
        check(&sb),
        "No version is active; run `git uplink self-update` to install 0.5.0.\n"
    );
}
