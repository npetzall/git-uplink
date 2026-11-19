use super::*;
use axum::body::to_bytes;
use axum::http::Request;
use tower::util::ServiceExt;

fn test_app(repo: PathBuf) -> Router {
    router(Arc::new(AppState { repo }))
}

fn local_request(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::HOST, "127.0.0.1:43721")
        .body(Body::empty())
        .unwrap()
}

#[test]
fn local_request_requires_loopback_host_and_origin() {
    let headers = |pairs: &[(header::HeaderName, &str)]| {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), value.parse().unwrap());
        }
        map
    };
    assert!(is_local_request(&headers(&[(
        header::HOST,
        "127.0.0.1:43721"
    )])));
    assert!(is_local_request(&headers(&[(
        header::HOST,
        "localhost:43721"
    )])));
    assert!(is_local_request(&headers(&[(header::HOST, "[::1]:43721")])));
    assert!(is_local_request(&headers(&[
        (header::HOST, "127.0.0.1:43721"),
        (header::ORIGIN, "http://127.0.0.1:43721"),
    ])));
    assert!(!is_local_request(&headers(&[])));
    assert!(!is_local_request(&headers(&[(
        header::HOST,
        "evil.example"
    )])));
    assert!(!is_local_request(&headers(&[(
        header::HOST,
        "127.0.0.1.evil.example:43721"
    )])));
    assert!(!is_local_request(&headers(&[
        (header::HOST, "127.0.0.1:43721"),
        (header::ORIGIN, "http://evil.example"),
    ])));
    assert!(!is_local_request(&headers(&[
        (header::HOST, "127.0.0.1:43721"),
        (header::ORIGIN, "null"),
    ])));
}

#[tokio::test]
async fn foreign_host_is_forbidden() {
    let app = test_app(PathBuf::from("/tmp"));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .header(header::HOST, "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_refresh_is_forbidden() {
    let app = test_app(PathBuf::from("/tmp"));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/refresh")
                .header(header::HOST, "127.0.0.1:43721")
                .header(header::ORIGIN, "http://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// A repo whose `uplink/state` branch has `files` committed, with
/// `origin/uplink/state` at the same commit.
fn state_repo(repo: &Path, files: &[(&str, &str)]) {
    use crate::git::git_ok;
    git_ok(repo, &["init", "-q", "-b", STATE_BRANCH]).unwrap();
    for (path, content) in files {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    git_ok(repo, &["add", "-A"]).unwrap();
    git_ok(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "state",
        ],
    )
    .unwrap();
    git_ok(
        repo,
        &["update-ref", "refs/remotes/origin/uplink/state", "HEAD"],
    )
    .unwrap();
}

fn queue_json(upstream: serde_json::Value) -> String {
    serde_json::json!({
        "version": 1,
        "config": crate::types::QueueConfig::default(),
        "upstream": upstream,
        "internal": [],
    })
    .to_string()
}

fn queued(id: &str, deps: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "title": format!("title {id}"),
        "status": "queued",
        "dependsOn": deps,
        "createdAt": "t",
        "updatedAt": "t",
        "patchIdStable": null,
        "source": {},
        "events": []
    })
}

async fn json_body(response: Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn is_uplink_path_rejects_traversal() {
    assert!(is_uplink_path(".uplink/queue.json"));
    assert!(is_uplink_path(".uplink/patches/upl_abcdefghij.patch"));
    assert!(!is_uplink_path("../Cargo.toml"));
    assert!(!is_uplink_path(".uplink/../Cargo.toml"));
    assert!(!is_uplink_path("Cargo.toml"));
    assert!(!is_uplink_path("/etc/passwd"));
}

#[test]
fn is_valid_sha_accepts_only_full_ids() {
    assert!(!is_valid_sha("worktree"));
    assert!(is_valid_sha(&"a".repeat(40)));
    assert!(is_valid_sha(&"0".repeat(64)));
    assert!(!is_valid_sha("--output=x"));
    assert!(!is_valid_sha("HEAD"));
    assert!(!is_valid_sha(&"A".repeat(40)));
    assert!(!is_valid_sha(&"a".repeat(39)));
}

#[tokio::test]
async fn repo_path_with_double_dot_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("release..v2");
    std::fs::create_dir_all(&repo).unwrap();
    state_repo(&repo, &[(".uplink/queue.json", "{}\n")]);
    let response = test_app(repo)
        .oneshot(local_request("/api/file?path=.uplink/queue.json"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn file_at_rejects_option_like_sha() {
    let dir = tempfile::tempdir().unwrap();
    let app = test_app(dir.path().to_path_buf());
    let response = app
        .oneshot(local_request(
            "/api/file?path=.uplink/queue.json&sha=--output=x",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!dir.path().join("x:.uplink").exists());
}

#[tokio::test]
async fn patch_detail_route_captures_id() {
    let app = test_app(PathBuf::from("/tmp"));
    let response = app
        .oneshot(local_request("/api/patches/not-a-real-patch"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON from the patch handler, got {content_type:?}"
    );
    let body = json_body(response).await;
    assert_eq!(body["present"], false);
}

#[tokio::test]
async fn patch_detail_rejects_parent_segments() {
    let app = test_app(PathBuf::from("/tmp"));
    let response = app
        .oneshot(local_request("/api/patches/..%2Fetc"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["present"], false);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("invalid path component")),
        "expected path-component error, got {body}"
    );
}

#[tokio::test]
async fn patch_diff_rejects_bad_ids_and_shas() {
    let app = test_app(PathBuf::from("/tmp"));
    let sha = "a".repeat(40);
    for uri in [
        format!("/api/patches/upl_abcdefghij/diff?from=--output=x&to={sha}"),
        format!("/api/patches/upl_abcdefghij/diff?from={sha}&to=HEAD"),
        format!("/api/patches/..%2Fetc/diff?from={sha}&to={sha}"),
        format!("/api/patches/upl_abcdefghij/diff?from={sha}"),
    ] {
        let response = app.clone().oneshot(local_request(&uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn patch_diff_compares_two_revisions() {
    use crate::git::git_ok;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let file = repo.join(".uplink/patches/upl_abcdefghij.patch");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    git_ok(repo, &["init", "-q"]).unwrap();
    let commit = |files: &[(&str, &str)]| {
        let mut patch = String::from("Subject: [PATCH] x\n\n---\n");
        for (path, lines) in files {
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\n@@ -1 +1 @@\n{lines}"
            ));
        }
        std::fs::write(&file, patch).unwrap();
        git_ok(repo, &["add", "."]).unwrap();
        git_ok(repo, &["commit", "-q", "-m", "patch"]).unwrap();
        git_ok(repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    };
    let first = commit(&[
        ("keep.rs", "-a\n+b\n"),
        ("edit.rs", "-a\n+b\n"),
        ("dropped.rs", "-a\n+b\n"),
    ]);
    let second = commit(&[
        ("keep.rs", "-a\n+b\n"),
        ("edit.rs", "-a\n+c\n"),
        ("picked.rs", "-a\n+b\n"),
    ]);
    let app = test_app(repo.to_path_buf());

    let uri = format!("/api/patches/upl_abcdefghij/diff?from={first}&to={second}");
    let response = app.clone().oneshot(local_request(&uri)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let states: Vec<(&str, &str)> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap(),
                file["state"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            ("keep.rs", "unchanged"),
            ("edit.rs", "changed"),
            ("picked.rs", "added"),
            ("dropped.rs", "removed"),
        ]
    );
    assert_eq!(body["files"][1]["diff"], " -a\n-+b\n++c\n");
    assert_eq!(body["files"][2]["lines"], "-a\n+b\n");

    // The uncommitted patch file is not a revision.
    let uri = format!("/api/patches/upl_abcdefghij/diff?from={second}&to=worktree");
    let response = app.clone().oneshot(local_request(&uri)).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let missing = "b".repeat(40);
    let uri = format!("/api/patches/upl_abcdefghij/diff?from={missing}&to={second}");
    let response = app.oneshot(local_request(&uri)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn file_at_rejects_repo_root_and_serves_uplink() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    state_repo(
        repo,
        &[
            ("Cargo.toml", "SECRET_ROOT_FILE\n"),
            (".uplink/queue.json", "{\"ok\":true}\n"),
        ],
    );
    let app = test_app(repo.to_path_buf());

    for uri in [
        "/api/file?path=../Cargo.toml",
        "/api/file?path=.uplink/../Cargo.toml",
    ] {
        let response = app.clone().oneshot(local_request(uri)).await.unwrap();
        assert_ne!(response.status(), StatusCode::OK, "{uri}");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("SECRET_ROOT_FILE"),
            "{uri} leaked repo-root content: {text}"
        );
    }

    let response = app
        .oneshot(local_request("/api/file?path=.uplink/queue.json"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["path"], ".uplink/queue.json");
    assert!(
        body["content"]
            .as_str()
            .is_some_and(|c| c.contains("\"ok\":true")),
        "expected queue.json body, got {body}"
    );
}

#[tokio::test]
async fn status_reads_origin_not_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    state_repo(
        repo,
        &[(
            ".uplink/queue.json",
            &queue_json(serde_json::json!([queued("upl_a", &[])])),
        )],
    );
    std::fs::write(
        repo.join(".uplink/queue.json"),
        queue_json(serde_json::json!([queued("upl_uncommitted", &[])])),
    )
    .unwrap();
    let app = test_app(repo.to_path_buf());

    let response = app
        .clone()
        .oneshot(local_request("/api/status"))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(body["present"], true, "{body}");
    assert_eq!(body["stateRef"], "origin/uplink/state");
    assert_eq!(body["upstream"][0]["id"], "upl_a");
    assert_eq!(body["readyToSubmit"], serde_json::json!(["upl_a"]));

    let response = app
        .oneshot(local_request("/api/file?path=.uplink/queue.json"))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert!(
        !body["content"]
            .as_str()
            .unwrap()
            .contains("upl_uncommitted"),
        "{body}"
    );
}

#[tokio::test]
async fn status_without_a_state_branch_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    crate::git::git_ok(dir.path(), &["init", "-q"]).unwrap();
    let response = test_app(dir.path().to_path_buf())
        .oneshot(local_request("/api/submit-stream"))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(body["present"], false, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("uplink/state")),
        "{body}"
    );
}

#[tokio::test]
async fn submit_stream_lists_layers() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let upstream = serde_json::json!([
        queued("upl_c", &["upl_a", "upl_b"]),
        queued("upl_a", &[]),
        queued("upl_b", &["upl_gone"]),
    ]);
    state_repo(repo, &[(".uplink/queue.json", &queue_json(upstream))]);
    let response = test_app(repo.to_path_buf())
        .oneshot(local_request("/api/submit-stream"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        stream_ids(&body),
        vec![vec!["upl_a", "upl_b"], vec!["upl_c"]]
    );
    assert_eq!(
        body["layers"][0][1]["missing"],
        serde_json::json!(["upl_gone"])
    );
    assert_eq!(body["layers"][1][0]["title"], "title upl_c");
    assert_eq!(body["layers"][1][0]["stale"], false);
}

fn stream_ids(body: &serde_json::Value) -> Vec<Vec<String>> {
    body["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| {
            column
                .as_array()
                .unwrap()
                .iter()
                .map(|node| node["id"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn local_state_commits_are_not_shown() {
    use crate::git::git_ok;

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let origin = queue_json(serde_json::json!([
        queued("upl_a", &[]),
        queued("upl_b", &["upl_a"]),
    ]));
    state_repo(repo, &[(".uplink/queue.json", &origin)]);
    let mut merged = queued("upl_a", &[]);
    merged["status"] = "merged".into();
    let local = queue_json(serde_json::json!([merged, queued("upl_b", &[])]));
    std::fs::write(repo.join(".uplink/queue.json"), local).unwrap();
    git_ok(
        repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-am",
            "local",
        ],
    )
    .unwrap();
    let app = test_app(repo.to_path_buf());
    let get = |uri: &'static str| {
        let app = app.clone();
        async move { json_body(app.oneshot(local_request(uri)).await.unwrap()).await }
    };

    assert_eq!(
        get("/api/status").await["readyToSubmit"],
        serde_json::json!(["upl_a"])
    );
    assert_eq!(
        stream_ids(&get("/api/submit-stream").await),
        vec![vec!["upl_a"], vec!["upl_b"]]
    );
    assert_eq!(get("/api/patches/upl_a").await["patch"]["status"], "queued");
}

#[test]
fn content_type_follows_the_extension() {
    assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
    assert_eq!(
        content_type("assets/index-abc.js"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(content_type("assets/INDEX.CSS"), "text/css; charset=utf-8");
    assert_eq!(content_type("favicon.svg"), "image/svg+xml");
    assert_eq!(content_type("no-extension"), "application/octet-stream");
}

#[tokio::test]
async fn static_files_are_served_with_their_type_and_unknown_paths_get_index() {
    let app = test_app(PathBuf::from("/tmp"));
    let header_of = |response: &Response| {
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    for (path, _) in crate::embedded::WEB {
        let response = app
            .clone()
            .oneshot(local_request(&format!("/{path}")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(header_of(&response), content_type(path), "{path}");
    }
    let response = app
        .oneshot(local_request("/some/client/route"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response), "text/html; charset=utf-8");
}
