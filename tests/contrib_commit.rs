//! Runs the forge pack's `contrib_commit.py` against a fake Git Database API
//! backed by a real bare repository, so tree and commit shas are git's own.
#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tempfile::TempDir;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/github/uplink/contrib_commit.py"
);
const TOKEN: &str = "test-token";

fn run_git(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Vec<u8> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn git");
    if let Some(input) = stdin {
        use std::io::Write as _;
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn git_str(dir: &Path, args: &[&str]) -> String {
    String::from_utf8(run_git(dir, args, None))
        .unwrap()
        .trim()
        .to_string()
}

fn try_git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn decode_base64(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("bad base64 byte {c}"),
        }
    };
    let clean: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::new();
    for chunk in clean.chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let mut n = 0u32;
        for &c in chunk {
            n = (n << 6) | if c == b'=' { 0 } else { value(c) };
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    out
}

#[derive(Clone)]
struct Fake {
    bare: PathBuf,
    requests: Arc<Mutex<Vec<(String, String, Value)>>>,
}

impl Fake {
    fn record(&self, method: &str, path: String, body: Value) {
        self.requests
            .lock()
            .unwrap()
            .push((method.into(), path, body));
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {TOKEN}"))
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "bad token").into_response()
}

async fn get_commit(
    State(fake): State<Fake>,
    headers: HeaderMap,
    UrlPath((_, _, sha)): UrlPath<(String, String, String)>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("GET", format!("commits/{sha}"), Value::Null);
    if try_git(
        &fake.bare,
        &["cat-file", "-e", &format!("{sha}^{{commit}}")],
    ) {
        Json(json!({ "sha": sha })).into_response()
    } else {
        (StatusCode::NOT_FOUND, "Not Found").into_response()
    }
}

async fn post_blob(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("POST", "blobs".into(), body.clone());
    assert_eq!(body["encoding"], "base64");
    let content = decode_base64(body["content"].as_str().unwrap());
    let sha = String::from_utf8(run_git(
        &fake.bare,
        &["hash-object", "-w", "--stdin"],
        Some(&content),
    ))
    .unwrap();
    (StatusCode::CREATED, Json(json!({ "sha": sha.trim() }))).into_response()
}

async fn post_tree(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("POST", "trees".into(), body.clone());
    // A scratch index and empty work tree; update-index refuses a bare repo.
    let scratch = tempfile::tempdir().unwrap();
    let index_path = scratch.path().join("index").to_str().unwrap().to_string();
    let with_index = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(&fake.bare)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_DIR", &fake.bare)
            .env("GIT_WORK_TREE", scratch.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    with_index(&["read-tree", body["base_tree"].as_str().unwrap()]);
    for entry in body["tree"].as_array().unwrap() {
        let path = entry["path"].as_str().unwrap();
        match entry["sha"].as_str() {
            None => {
                with_index(&["update-index", "--force-remove", path]);
            }
            Some(sha) => {
                let mode = entry["mode"].as_str().unwrap();
                with_index(&[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("{mode},{sha},{path}"),
                ]);
            }
        }
    }
    let sha = with_index(&["write-tree"]);
    (StatusCode::CREATED, Json(json!({ "sha": sha.trim() }))).into_response()
}

async fn post_commit(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("POST", "commits".into(), body.clone());
    let mut args = vec![
        "commit-tree".to_string(),
        body["tree"].as_str().unwrap().into(),
    ];
    for parent in body["parents"].as_array().unwrap() {
        args.push("-p".into());
        args.push(parent.as_str().unwrap().into());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let message = body["message"].as_str().unwrap().as_bytes().to_vec();
    let sha = String::from_utf8(run_git(&fake.bare, &args, Some(&message))).unwrap();
    (
        StatusCode::CREATED,
        Json(json!({
            "sha": sha.trim(),
            "verification": { "verified": false, "reason": "unsigned" },
        })),
    )
        .into_response()
}

async fn post_ref(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("POST", "refs".into(), body.clone());
    let name = body["ref"].as_str().unwrap();
    if try_git(&fake.bare, &["rev-parse", "--verify", "--quiet", name]) {
        return (StatusCode::UNPROCESSABLE_ENTITY, "Reference already exists").into_response();
    }
    run_git(
        &fake.bare,
        &["update-ref", name, body["sha"].as_str().unwrap()],
        None,
    );
    (StatusCode::CREATED, Json(json!({ "ref": name }))).into_response()
}

async fn patch_ref(
    State(fake): State<Fake>,
    headers: HeaderMap,
    UrlPath((_, _, name)): UrlPath<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("PATCH", format!("refs/{name}"), body.clone());
    let full = format!("refs/{name}");
    if !try_git(&fake.bare, &["rev-parse", "--verify", "--quiet", &full]) {
        return (StatusCode::UNPROCESSABLE_ENTITY, "Reference does not exist").into_response();
    }
    assert_eq!(body["force"], true);
    run_git(
        &fake.bare,
        &["update-ref", &full, body["sha"].as_str().unwrap()],
        None,
    );
    Json(json!({ "ref": full })).into_response()
}

async fn delete_ref(
    State(fake): State<Fake>,
    headers: HeaderMap,
    UrlPath((_, _, name)): UrlPath<(String, String, String)>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    fake.record("DELETE", format!("refs/{name}"), Value::Null);
    run_git(
        &fake.bare,
        &["update-ref", "-d", &format!("refs/{name}")],
        None,
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Serves the fake API on a background runtime; returns its base URL.
fn serve(fake: Fake) -> String {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let app = Router::new()
                .route("/repos/{o}/{r}/git/commits/{sha}", get(get_commit))
                .route("/repos/{o}/{r}/git/blobs", post(post_blob))
                .route("/repos/{o}/{r}/git/trees", post(post_tree))
                .route("/repos/{o}/{r}/git/commits", post(post_commit))
                .route("/repos/{o}/{r}/git/refs", post(post_ref))
                .route(
                    "/repos/{o}/{r}/git/refs/{*name}",
                    patch(patch_ref).delete(delete_ref),
                )
                .with_state(fake);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

struct World {
    _keep: TempDir,
    local: PathBuf,
    bare: PathBuf,
    base: String,
    export: String,
    message_file: String,
}

/// A local repo with an upstream base and an export commit that modifies,
/// adds an executable, adds a symlink, deletes, and creates a nested file.
fn world() -> World {
    let keep = tempfile::tempdir().unwrap();
    let local = keep.path().join("local");
    fs::create_dir_all(&local).unwrap();
    run_git(&local, &["init", "-q", "-b", "main"], None);
    fs::write(local.join("README.md"), "tokenkit\n").unwrap();
    fs::write(local.join("old.txt"), "remove me\n").unwrap();
    fs::create_dir_all(local.join("src")).unwrap();
    fs::write(local.join("src/tokens.js"), "sha1\n").unwrap();
    fs::write(local.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
    run_git(&local, &["add", "-A"], None);
    run_git(&local, &["commit", "-q", "-m", "upstream base"], None);
    let base = git_str(&local, &["rev-parse", "HEAD"]);

    fs::write(local.join("src/tokens.js"), "sha256\n").unwrap();
    fs::remove_file(local.join("old.txt")).unwrap();
    fs::write(local.join("bin-tool"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(local.join("bin-tool"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(local.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/tokens.js", local.join("tokens-link")).unwrap();
    fs::create_dir_all(local.join("docs/deep")).unwrap();
    fs::write(
        local.join("docs/deep/notes.md"),
        [0xffu8, 0x00, 0x10, b'\n'],
    )
    .unwrap();
    run_git(&local, &["add", "-A"], None);
    let message = "Use SHA-256 for tokens\n\nReplace SHA-1.\n\nUplink-Patch-Id: upl_0123456789\nCo-Authored-By: Jane Public <jane@users.noreply.github.com>\n";
    run_git(
        &local,
        &["commit", "-q", "-F", "-"],
        Some(message.as_bytes()),
    );
    let export = git_str(&local, &["rev-parse", "HEAD"]);
    let message_file = ".uplink/reports/upl_0123456789/commit-message.txt";
    fs::create_dir_all(local.join(".uplink/reports/upl_0123456789")).unwrap();
    fs::write(local.join(message_file), message).unwrap();

    let bare = keep.path().join("contrib.git");
    run_git(
        keep.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
        None,
    );
    World {
        _keep: keep,
        local,
        bare,
        base,
        export,
        message_file: message_file.into(),
    }
}

fn run_script(world: &World, api: &str, token: &str) -> Output {
    let tree = git_str(
        &world.local,
        &["rev-parse", &format!("{}^{{tree}}", world.export)],
    );
    let commit = json!({
        "branch": "uplink/upl_0123456789",
        "baseSha": world.base,
        "localSha": world.export,
        "treeSha": tree,
        "messageFile": world.message_file,
    });
    Command::new("python3")
        .arg(SCRIPT)
        .current_dir(&world.local)
        .env("CONTRIB_COMMIT", commit.to_string())
        .env("CONTRIB_REPO", "acme-contrib/tokenkit")
        .env("UPLINK_CONTRIB_TOKEN", token)
        .env("GITHUB_API_URL", api)
        .env("CONTRIB_GIT_URL", world.bare.to_str().unwrap())
        .env_remove("GITHUB_STEP_SUMMARY")
        .output()
        .expect("python3 must be installed to test the forge pack script")
}

fn ls_tree(dir: &Path, rev: &str) -> String {
    git_str(dir, &["ls-tree", "-r", "--full-tree", rev])
}

#[test]
fn recreates_the_export_commit_on_contrib_with_modes_and_no_author() {
    let world = world();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let api = serve(Fake {
        bare: world.bare.clone(),
        requests: requests.clone(),
    });

    let out = run_script(&world, &api, TOKEN);
    assert!(out.status.success(), "{out:?}");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["verified"], false);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("::warning::"),
        "{out:?}"
    );

    let sha = result["sha"].as_str().unwrap();
    let branch = "refs/heads/uplink/upl_0123456789";
    assert_eq!(git_str(&world.bare, &["rev-parse", branch]), sha);
    assert_eq!(
        git_str(&world.bare, &["rev-parse", &format!("{sha}^")]),
        world.base
    );
    assert_eq!(
        git_str(&world.bare, &["rev-parse", &format!("{sha}^{{tree}}")]),
        git_str(
            &world.local,
            &["rev-parse", &format!("{}^{{tree}}", world.export)]
        )
    );
    // Modes, the symlink, the deletion, and binary content all match.
    let fetched = ls_tree(&world.bare, sha);
    assert_eq!(fetched, ls_tree(&world.local, &world.export));
    assert!(fetched.contains("100755 blob"), "{fetched}");
    assert!(fetched.contains("120000 blob"), "{fetched}");
    assert!(!fetched.contains("old.txt"), "{fetched}");
    assert_eq!(
        git_str(&world.bare, &["log", "-1", "--format=%B", sha]),
        git_str(&world.local, &["log", "-1", "--format=%B", &world.export])
    );
    // The base was pushed to a scratch branch, which is gone afterwards.
    assert!(!try_git(
        &world.bare,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/uplink-base/upl_0123456789"
        ]
    ));

    let requests = requests.lock().unwrap();
    let commit = requests
        .iter()
        .find(|(m, p, _)| m == "POST" && p == "commits")
        .map(|(_, _, body)| body)
        .unwrap();
    let keys: Vec<&str> = commit
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 3, "{commit}");
    for key in ["author", "committer", "signature"] {
        assert!(commit.get(key).is_none(), "{commit}");
    }
    let blobs = requests.iter().filter(|(_, p, _)| p == "blobs").count();
    assert_eq!(blobs, 5, "modified, two executables, symlink, nested file");
    assert!(
        requests
            .iter()
            .any(|(m, p, _)| m == "DELETE" && p == "refs/heads/uplink-base/upl_0123456789")
    );
}

#[test]
fn moves_an_existing_contrib_branch_without_pushing_the_base() {
    let world = world();
    // Contrib already has the base and an older PR branch.
    run_git(
        &world.local,
        &[
            "push",
            "-q",
            world.bare.to_str().unwrap(),
            &format!("{}:refs/heads/uplink/upl_0123456789", world.base),
        ],
        None,
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let api = serve(Fake {
        bare: world.bare.clone(),
        requests: requests.clone(),
    });

    let out = run_script(&world, &api, TOKEN);
    assert!(out.status.success(), "{out:?}");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    let sha = result["sha"].as_str().unwrap();
    assert_eq!(
        git_str(
            &world.bare,
            &["rev-parse", "refs/heads/uplink/upl_0123456789"]
        ),
        sha
    );

    let requests = requests.lock().unwrap();
    let methods: HashMap<String, usize> =
        requests.iter().fold(HashMap::new(), |mut acc, (m, p, _)| {
            *acc.entry(format!("{m} {p}")).or_default() += 1;
            acc
        });
    assert_eq!(
        methods.get("PATCH refs/heads/uplink/upl_0123456789"),
        Some(&1)
    );
    assert!(
        !requests.iter().any(|(m, _, _)| m == "DELETE"),
        "{methods:?}"
    );
    assert!(!requests.iter().any(|(m, p, _)| m == "POST" && p == "refs"));
}

#[test]
fn fails_without_touching_the_branch_when_the_token_is_rejected() {
    let world = world();
    run_git(
        &world.local,
        &[
            "push",
            "-q",
            world.bare.to_str().unwrap(),
            &format!("{}:refs/heads/uplink/upl_0123456789", world.base),
        ],
        None,
    );
    let api = serve(Fake {
        bare: world.bare.clone(),
        requests: Arc::new(Mutex::new(Vec::new())),
    });

    let out = run_script(&world, &api, "wrong-token");
    assert!(!out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("401"),
        "{out:?}"
    );
    assert_eq!(
        git_str(
            &world.bare,
            &["rev-parse", "refs/heads/uplink/upl_0123456789"]
        ),
        world.base
    );
}

#[test]
fn refuses_a_plain_http_api_url_that_is_not_loopback() {
    let world = world();
    let out = run_script(&world, "http://api.example.test", TOKEN);
    assert!(!out.status.success(), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("GITHUB_API_URL must be https"),
        "{out:?}"
    );
}
