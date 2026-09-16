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

fn no_operator_creds() -> GitOpts<'static> {
    GitOpts {
        extra_env: vec![
            ("UPLINK_GITHUB_TOKEN".into(), String::new()),
            ("GITHUB_TOKEN".into(), String::new()),
            ("UPLINK_SSH_KEY".into(), String::new()),
            ("UPLINK_SSH_COMMAND".into(), String::new()),
        ],
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
        message.contains("UPLINK_GITHUB_TOKEN") || message.contains("UPLINK_SSH_KEY"),
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
        &["fetch", &remote],
        GitOpts {
            allow_fail: true,
            extra_env: vec![
                ("UPLINK_GITHUB_TOKEN".into(), "pat-token".into()),
                ("GITHUB_TOKEN".into(), String::new()),
                ("UPLINK_SSH_KEY".into(), String::new()),
                ("UPLINK_SSH_COMMAND".into(), String::new()),
            ],
            ..GitOpts::default()
        },
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
