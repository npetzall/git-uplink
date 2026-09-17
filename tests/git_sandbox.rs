use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use git_uplink::{GitOpts, QueueConfig, configure_repo, git, git_ok, init_repo};
use tempfile::TempDir;

fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn write(repo: &Path, file: &str, contents: &str) {
    let full = repo.join(file);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(full, contents).unwrap();
}

fn blank_network_creds() -> Vec<(String, String)> {
    [
        "UPLINK_INTERNAL_TOKEN",
        "UPLINK_INTERNAL_KEY",
        "UPLINK_CONTRIB_TOKEN",
        "UPLINK_CONTRIB_KEY",
        "UPLINK_UPSTREAM_TOKEN",
        "UPLINK_UPSTREAM_KEY",
        "UPLINK_GITHUB_TOKEN",
        "GITHUB_TOKEN",
        "UPLINK_SSH_KEY",
        "UPLINK_SSH_COMMAND",
    ]
    .into_iter()
    .map(|key| (key.into(), String::new()))
    .collect()
}

fn no_operator_creds() -> GitOpts<'static> {
    GitOpts {
        extra_env: blank_network_creds(),
        ..GitOpts::default()
    }
}

fn with_creds(pairs: &[(&str, &str)]) -> GitOpts<'static> {
    let mut extra_env = blank_network_creds();
    for (key, value) in pairs {
        extra_env.push(((*key).into(), (*value).into()));
    }
    GitOpts {
        extra_env,
        allow_fail: true,
        ..GitOpts::default()
    }
}

#[test]
fn commits_stay_unsigned_when_global_gpgsign_is_true() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();

    let global = keep.path().join("hostile.gitconfig");
    fs::write(
        &global,
        "[user]\n\
         \tname = Operator\n\
         \temail = operator@example.com\n\
         \tsigningkey = /nonexistent/uplink-test-signing-key\n\
         [commit]\n\
         \tgpgsign = true\n\
         [gpg]\n\
         \tformat = openpgp\n",
    )
    .unwrap();

    let extra_env = vec![
        (
            "GIT_CONFIG_GLOBAL".into(),
            global.to_str().unwrap().to_string(),
        ),
        ("GIT_CONFIG_SYSTEM".into(), "/dev/null".into()),
    ];
    write(repo, "README.md", "sandbox\n");
    git(
        repo,
        &["add", "README.md"],
        GitOpts {
            extra_env: extra_env.clone(),
            ..GitOpts::default()
        },
    )
    .unwrap();
    git(
        repo,
        &["commit", "-m", "unsigned bot commit"],
        GitOpts {
            extra_env,
            ..GitOpts::default()
        },
    )
    .unwrap();

    let body = git_ok(repo, &["cat-file", "-p", "HEAD"]).unwrap();
    assert!(
        !body.contains("gpgsig"),
        "git-uplink must not sign with the operator key: {body}"
    );
    assert!(
        body.contains("Uplink Bot"),
        "bot identity should win over global user.name: {body}"
    );
    assert!(
        !body.contains("Operator"),
        "operator name leaked into the commit: {body}"
    );
}

#[test]
fn configure_repo_does_not_write_identity_into_local_config() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();
    configure_repo(repo).unwrap();
    init_repo(repo, QueueConfig::default()).unwrap();

    let name = git(
        repo,
        &["config", "--local", "--get", "user.name"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap();
    assert_ne!(
        name.stdout, "Uplink Bot",
        "init must not persist bot identity in the product repo"
    );
    assert_eq!(name.code, 1);

    let gpgsign = git(
        repo,
        &["config", "--local", "--get", "commit.gpgsign"],
        GitOpts {
            allow_fail: true,
            ..GitOpts::default()
        },
    )
    .unwrap();
    assert_eq!(gpgsign.code, 1);
}

#[test]
fn file_remotes_fetch_and_push_without_tokens() {
    let keep = temp_dir();
    let working = keep.path().join("work");
    let bare = keep.path().join("bare.git");
    fs::create_dir_all(&working).unwrap();

    git(&working, &["init", "-b", "main"], GitOpts::default()).unwrap();
    write(&working, "a.txt", "one\n");
    git(&working, &["add", "a.txt"], GitOpts::default()).unwrap();
    git(&working, &["commit", "-m", "one"], GitOpts::default()).unwrap();

    git(
        keep.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            working.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
        no_operator_creds(),
    )
    .unwrap();

    git(
        &working,
        &["remote", "add", "origin", bare.to_str().unwrap()],
        GitOpts::default(),
    )
    .unwrap();
    git(
        &working,
        &["push", "origin", "HEAD:main"],
        no_operator_creds(),
    )
    .unwrap();
    git(
        &working,
        &["fetch", "--quiet", "origin"],
        no_operator_creds(),
    )
    .unwrap();
}

#[test]
fn ssh_remote_without_token_or_key_fails_closed() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();

    let err = git(
        repo,
        &["fetch", "ssh://git@example.invalid/org/repo.git"],
        no_operator_creds(),
    )
    .expect_err("must not invoke ssh with the operator agent");
    let message = err.to_string();
    assert!(
        message.contains("UPLINK_INTERNAL_KEY")
            || message.contains("UPLINK_CONTRIB_KEY")
            || message.contains("UPLINK_UPSTREAM_KEY")
            || message.contains("UPLINK_INTERNAL_TOKEN"),
        "unexpected error: {message}"
    );
    assert!(
        !message.contains("Permission denied")
            && !message.contains("Could not resolve hostname")
            && !message.contains("Connection refused"),
        "ssh ran instead of failing closed: {message}"
    );
}

fn spawn_header_capture() -> (u16, mpsc::Receiver<String>, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let port = listener.local_addr().expect("local_addr").port();
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stop_thread.load(Ordering::Relaxed) && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    loop {
                        match stream.read(&mut tmp) {
                            Ok(0) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&tmp[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let req = String::from_utf8_lossy(&buf);
                    for line in req.split(['\r', '\n']) {
                        let Some((name, value)) = line.split_once(':') else {
                            continue;
                        };
                        if name.eq_ignore_ascii_case("authorization") {
                            let _ = tx.send(value.trim().to_string());
                        }
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    });
    (port, rx, stop)
}

#[test]
fn checkout_url_extraheader_does_not_override_uplink_token() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();

    let (port, rx, stop) = spawn_header_capture();
    let origin = format!("http://127.0.0.1:{port}");
    let remote = format!("{origin}/repo.git");
    git(
        repo,
        &["remote", "add", "origin", &remote],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &[
            "config",
            "--local",
            &format!("http.{origin}/.extraheader"),
            "AUTHORIZATION: basic BOT",
        ],
        GitOpts::default(),
    )
    .unwrap();

    let fetch = git(
        repo,
        &["fetch", "origin"],
        with_creds(&[("UPLINK_INTERNAL_TOKEN", "pat-token")]),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    let auths: Vec<String> = rx.try_iter().collect();
    assert!(
        !auths.is_empty(),
        "git fetch never sent Authorization to the test server (code {} stdout {:?} stderr {:?})",
        fetch.code,
        fetch.stdout,
        fetch.stderr
    );
    assert!(
        auths
            .iter()
            .all(|h| !h.to_ascii_uppercase().contains("BOT")),
        "checkout extraheader leaked: {auths:?}"
    );
    // x-access-token:pat-token (no padding; 24 bytes)
    let expected_basic = "basic eC1hY2Nlc3MtdG9rZW46cGF0LXRva2Vu";
    for header in &auths {
        assert!(
            header.eq_ignore_ascii_case(expected_basic),
            "unexpected Authorization {header:?}; all: {auths:?}"
        );
    }
}

fn expected_basic(token: &str) -> String {
    let header = format!("x-access-token:{token}");
    let b64 = {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let input = header.as_bytes();
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
            out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(ALPHABET[(n & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    };
    format!("basic {b64}")
}

fn drain_auths(rx: &mpsc::Receiver<String>) -> Vec<String> {
    thread::sleep(Duration::from_millis(200));
    rx.try_iter().collect()
}

#[test]
fn remotes_use_matching_role_tokens() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();

    let (port, rx, stop) = spawn_header_capture();
    let host = format!("http://127.0.0.1:{port}");
    git(
        repo,
        &["remote", "add", "origin", &format!("{host}/internal.git")],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &["remote", "add", "contrib", &format!("{host}/contrib.git")],
        GitOpts::default(),
    )
    .unwrap();
    git(
        repo,
        &["remote", "add", "upstream", &format!("{host}/upstream.git")],
        GitOpts::default(),
    )
    .unwrap();

    let creds = with_creds(&[
        ("UPLINK_INTERNAL_TOKEN", "internal-token"),
        ("UPLINK_CONTRIB_TOKEN", "contrib-token"),
        ("UPLINK_UPSTREAM_TOKEN", "upstream-token"),
    ]);

    git(repo, &["fetch", "origin"], creds.clone()).unwrap();
    let origin_auths = drain_auths(&rx);
    git(repo, &["fetch", "contrib"], creds.clone()).unwrap();
    let contrib_auths = drain_auths(&rx);
    git(repo, &["fetch", "upstream"], creds).unwrap();
    let upstream_auths = drain_auths(&rx);
    stop.store(true, Ordering::Relaxed);

    assert!(
        !origin_auths.is_empty() && !contrib_auths.is_empty() && !upstream_auths.is_empty(),
        "missing Authorization headers origin={origin_auths:?} contrib={contrib_auths:?} upstream={upstream_auths:?}"
    );
    let internal = expected_basic("internal-token");
    let contrib = expected_basic("contrib-token");
    let upstream = expected_basic("upstream-token");
    for header in &origin_auths {
        assert!(
            header.eq_ignore_ascii_case(&internal),
            "origin sent {header:?}, expected internal"
        );
        assert!(!header.eq_ignore_ascii_case(&contrib));
        assert!(!header.eq_ignore_ascii_case(&upstream));
    }
    for header in &contrib_auths {
        assert!(
            header.eq_ignore_ascii_case(&contrib),
            "contrib sent {header:?}, expected contrib"
        );
        assert!(!header.eq_ignore_ascii_case(&internal));
    }
    for header in &upstream_auths {
        assert!(
            header.eq_ignore_ascii_case(&upstream),
            "upstream sent {header:?}, expected upstream"
        );
        assert!(!header.eq_ignore_ascii_case(&contrib));
        assert!(!header.eq_ignore_ascii_case(&internal));
    }
}

#[test]
fn origin_does_not_fall_back_to_contrib_token() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();
    git(
        repo,
        &[
            "remote",
            "add",
            "origin",
            "ssh://git@example.invalid/org/repo.git",
        ],
        GitOpts::default(),
    )
    .unwrap();

    let err = git(
        repo,
        &["fetch", "origin"],
        GitOpts {
            extra_env: {
                let mut env = blank_network_creds();
                env.push(("UPLINK_CONTRIB_TOKEN".into(), "contrib-token".into()));
                env
            },
            ..GitOpts::default()
        },
    )
    .expect_err("origin must not use contrib creds");
    let message = err.to_string();
    assert!(
        message.contains("UPLINK_INTERNAL_KEY") && message.contains("UPLINK_INTERNAL_TOKEN"),
        "unexpected error: {message}"
    );
}

#[test]
fn key_wins_over_token_does_not_send_https_token() {
    let keep = temp_dir();
    let repo = keep.path();
    git(repo, &["init", "-b", "main"], GitOpts::default()).unwrap();

    let (port, rx, stop) = spawn_header_capture();
    let remote = format!("http://127.0.0.1:{port}/repo.git");
    git(
        repo,
        &["remote", "add", "origin", &remote],
        GitOpts::default(),
    )
    .unwrap();

    let key = keep.path().join("id_uplink");
    fs::write(&key, "not-a-real-key\n").unwrap();

    git(
        repo,
        &["fetch", "origin"],
        with_creds(&[
            ("UPLINK_INTERNAL_KEY", key.to_str().unwrap()),
            ("UPLINK_INTERNAL_TOKEN", "pat-token"),
        ]),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    let auths: Vec<String> = rx.try_iter().collect();
    let expected = expected_basic("pat-token");
    assert!(
        auths.iter().all(|h| !h.eq_ignore_ascii_case(&expected)),
        "TOKEN was sent even though KEY was set: {auths:?}"
    );
}
