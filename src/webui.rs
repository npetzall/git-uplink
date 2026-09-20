use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as PathParam, Query, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::ops::{QueueCounts, StateStatus, refresh_from_origin, state_status_at, summarize_queue};
use crate::queue::{get_patch, patch_path, require_path_component};
use crate::repo::{
    COMPANY_REMOTE, FileRevision, file_history, has_ref, queue_at, rev_parse, show_at, state_branch,
};
use crate::types::{LastSync, Patch, QUEUE_PATH, QueueState};

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    repo: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueSource {
    Checkout,
    Remote,
}

impl QueueSource {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("remote") => Self::Remote,
            _ => Self::Checkout,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Checkout => "checkout",
            Self::Remote => "remote",
        }
    }
}

#[derive(Deserialize)]
struct StatusQuery {
    source: Option<String>,
    fetch: Option<String>,
}

#[derive(Deserialize)]
struct PatchQuery {
    source: Option<String>,
}

#[derive(Deserialize)]
struct FileQuery {
    source: Option<String>,
    sha: Option<String>,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    present: bool,
    cwd: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    company_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    counts: Option<QueueCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tooling: Option<Patch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream: Option<Vec<Patch>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    internal: Option<Vec<Patch>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sync: Option<LastSync>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<StateStatus>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchResponse {
    present: bool,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch: Option<Patch>,
    revisions: Vec<FileRevision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patch_file: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileResponse {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<String>,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: String,
}

pub fn has_embedded_index() -> bool {
    Assets::get("index.html").is_some()
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/refresh", post(refresh))
        .route("/api/patches/{id}", get(patch_detail))
        .route("/api/file", get(file_at))
        .fallback(static_file)
        .with_state(state)
}

pub async fn serve(repo: PathBuf, addr: SocketAddr, open_browser: bool) -> std::io::Result<()> {
    let app = router(Arc::new(AppState { repo: repo.clone() }));

    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let local = format!("http://127.0.0.1:{}", bound.port());
    println!("git uplink web-ui listening on http://{bound}");
    println!("Open {local}");
    if open_browser {
        launch_browser(&local);
    }
    axum::serve(listener, app).await
}

fn launch_browser(url: &str) {
    let commands: [(&str, Vec<&str>); 3] = [
        ("xdg-open", vec![url]),
        ("open", vec![url]),
        ("gio", vec!["open", url]),
    ];
    for (program, args) in commands {
        if std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

fn trusted_repo(path: &Path) -> crate::error::Result<PathBuf> {
    let s = path
        .to_str()
        .ok_or_else(|| crate::error::Error::msg("repo path is not utf-8"))?;
    if s.contains("..") {
        return Err(crate::error::Error::msg("invalid repo path"));
    }
    Ok(PathBuf::from(s))
}

fn wants_fetch(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None => true,
        Some("0") | Some("false") | Some("no") => false,
        Some(_) => true,
    }
}

fn missing_status(cwd: String, source: QueueSource, error: String) -> StatusResponse {
    StatusResponse {
        present: false,
        cwd,
        source: source.as_str().into(),
        error: Some(error),
        company_head: None,
        upstream_head: None,
        counts: None,
        tooling: None,
        upstream: None,
        internal: None,
        last_sync: None,
        state: None,
    }
}

fn source_ref(repo: &Path, source: QueueSource) -> String {
    let branch = state_branch(repo);
    match source {
        QueueSource::Checkout => branch,
        QueueSource::Remote => format!("{COMPANY_REMOTE}/{branch}"),
    }
}

fn maybe_fetch(repo: &Path, fetch: bool) {
    if !fetch {
        return;
    }
    if refresh_from_origin(repo).is_err() {
        let _ = state_status_at(repo, true);
    }
}

fn queue_for_source(repo: &Path, source: QueueSource) -> crate::error::Result<QueueState> {
    match source {
        QueueSource::Checkout => {
            crate::repo::ensure_state_worktree(repo)?;
            crate::ops::read_queue(repo)
        }
        QueueSource::Remote => queue_at(repo, &source_ref(repo, source)),
    }
}

fn optional_rev(repo: &Path, git_ref: &str) -> Option<String> {
    if has_ref(repo, git_ref).ok()? {
        rev_parse(repo, git_ref).ok()
    } else {
        None
    }
}

fn company_head(repo: &Path, queue: &QueueState, source: QueueSource) -> Option<String> {
    let branch = &queue.config.internal_branch;
    let git_ref = match source {
        QueueSource::Checkout => branch.clone(),
        QueueSource::Remote => format!("{COMPANY_REMOTE}/{branch}"),
    };
    optional_rev(repo, &git_ref)
}

fn upstream_head(repo: &Path, source: QueueSource) -> Option<String> {
    let git_ref = match source {
        QueueSource::Checkout => "uplink/upstream".to_string(),
        QueueSource::Remote => format!("{COMPANY_REMOTE}/uplink/upstream"),
    };
    optional_rev(repo, &git_ref)
}

fn is_uplink_path(path: &str) -> bool {
    if path.contains("..") {
        return false;
    }
    let path = path.trim_start_matches('/');
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return false;
    }
    path == ".uplink" || path.starts_with(".uplink/")
}

fn read_worktree_uplink_file(repo: &Path, path: &str) -> crate::error::Result<String> {
    let uplink_root = repo.join(".uplink");
    let file_path = repo.join(path);
    let file_path = file_path
        .canonicalize()
        .map_err(|err| crate::error::Error::msg(format!("{path}: {err}")))?;
    let uplink_root = uplink_root
        .canonicalize()
        .map_err(|err| crate::error::Error::msg(format!("{path}: {err}")))?;
    if !file_path.starts_with(&uplink_root) {
        return Err(crate::error::Error::msg("path must be under .uplink/"));
    }
    std::fs::read_to_string(file_path)
        .map_err(|err| crate::error::Error::msg(format!("{path}: {err}")))
}

fn read_uplink_file(
    repo: &Path,
    source: QueueSource,
    sha: Option<&str>,
    path: &str,
) -> crate::error::Result<String> {
    if !is_uplink_path(path) {
        return Err(crate::error::Error::msg("path must be under .uplink/"));
    }
    let use_worktree = match sha {
        Some("worktree") => true,
        None if source == QueueSource::Checkout => true,
        _ => false,
    };
    if use_worktree {
        return read_worktree_uplink_file(repo, path);
    }
    let git_ref = match sha {
        Some(sha) => sha.to_string(),
        None => source_ref(repo, source),
    };
    show_at(repo, &git_ref, path)
}

fn patch_revisions(
    repo: &Path,
    source: QueueSource,
    id: &str,
    uncommitted: &[String],
) -> Vec<FileRevision> {
    let path = patch_path(id)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let git_ref = source_ref(repo, source);
    let mut revisions = file_history(repo, &git_ref, &path).unwrap_or_default();
    if source == QueueSource::Checkout && uncommitted.iter().any(|p| p == &path) {
        revisions.insert(
            0,
            FileRevision {
                sha: "worktree".into(),
                at: String::new(),
                subject: "uncommitted".into(),
            },
        );
    }
    revisions
}

fn build_status(repo: &Path, source: QueueSource, fetch: bool) -> StatusResponse {
    let cwd = repo.display().to_string();
    maybe_fetch(repo, fetch);
    if source == QueueSource::Checkout {
        let _ = crate::repo::ensure_state_worktree(repo);
    }
    let queue = match queue_for_source(repo, source) {
        Ok(queue) => queue,
        Err(err) => {
            return missing_status(cwd, source, err.to_string());
        }
    };
    if source == QueueSource::Checkout && !repo.join(QUEUE_PATH).is_file() {
        return missing_status(cwd, source, format!("no {QUEUE_PATH} in this directory"));
    }
    let state = state_status_at(repo, false).ok();
    let counts = summarize_queue(&queue);
    StatusResponse {
        present: true,
        cwd,
        source: source.as_str().into(),
        error: None,
        company_head: company_head(repo, &queue, source),
        upstream_head: upstream_head(repo, source),
        counts: Some(counts),
        tooling: queue.tooling.clone(),
        upstream: Some(queue.upstream.clone()),
        internal: Some(queue.internal.clone()),
        last_sync: queue.last_sync.clone(),
        state,
    }
}

fn build_patch(repo: &Path, id: &str, source: QueueSource) -> PatchResponse {
    let queue = match queue_for_source(repo, source) {
        Ok(queue) => queue,
        Err(err) => {
            return PatchResponse {
                present: false,
                source: source.as_str().into(),
                error: Some(err.to_string()),
                layer: None,
                patch: None,
                revisions: Vec::new(),
                patch_file: None,
            };
        }
    };
    let patch = match get_patch(&queue, id) {
        Ok(patch) => patch.clone(),
        Err(err) => {
            return PatchResponse {
                present: false,
                source: source.as_str().into(),
                error: Some(err.to_string()),
                layer: None,
                patch: None,
                revisions: Vec::new(),
                patch_file: None,
            };
        }
    };
    let uncommitted = state_status_at(repo, false)
        .map(|s| s.uncommitted)
        .unwrap_or_default();
    let revisions = patch_revisions(repo, source, id, &uncommitted);
    let path = match patch_path(id) {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(err) => {
            return PatchResponse {
                present: false,
                source: source.as_str().into(),
                error: Some(err.to_string()),
                layer: None,
                patch: None,
                revisions: Vec::new(),
                patch_file: None,
            };
        }
    };
    let default_sha = revisions.first().map(|r| r.sha.as_str());
    let patch_file = read_uplink_file(repo, source, default_sha, &path).ok();
    PatchResponse {
        present: true,
        source: source.as_str().into(),
        error: None,
        layer: Some(crate::queue::layer_label(&queue, id).to_string()),
        patch: Some(patch),
        revisions,
        patch_file,
    }
}

async fn status(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StatusQuery>,
) -> Json<StatusResponse> {
    let source = QueueSource::parse(query.source.as_deref());
    let fetch = wants_fetch(query.fetch.as_deref());
    let repo = match trusted_repo(&state.repo) {
        Ok(repo) => repo,
        Err(err) => {
            return Json(missing_status(
                state.repo.display().to_string(),
                source,
                err.to_string(),
            ));
        }
    };
    let cwd = repo.display().to_string();
    Json(
        tokio::task::spawn_blocking(move || build_status(&repo, source, fetch))
            .await
            .unwrap_or_else(|err| missing_status(cwd, source, format!("status worker: {err}"))),
    )
}

async fn refresh(State(state): State<Arc<AppState>>) -> Response {
    let repo = match trusted_repo(&state.repo) {
        Ok(repo) => repo,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    match tokio::task::spawn_blocking(move || refresh_from_origin(&repo)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: err.to_string(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: format!("refresh worker: {err}"),
            }),
        )
            .into_response(),
    }
}

fn missing_patch(source: QueueSource, error: String) -> PatchResponse {
    PatchResponse {
        present: false,
        source: source.as_str().into(),
        error: Some(error),
        layer: None,
        patch: None,
        revisions: Vec::new(),
        patch_file: None,
    }
}

async fn patch_detail(
    State(state): State<Arc<AppState>>,
    PathParam(id): PathParam<String>,
    Query(query): Query<PatchQuery>,
) -> Response {
    let source = QueueSource::parse(query.source.as_deref());
    if let Err(err) = require_path_component(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(missing_patch(source, err.to_string())),
        )
            .into_response();
    }
    let repo = match trusted_repo(&state.repo) {
        Ok(repo) => repo,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(missing_patch(source, err.to_string())),
            )
                .into_response();
        }
    };
    match tokio::task::spawn_blocking(move || build_patch(&repo, &id, source)).await {
        Ok(body) => {
            let status = if body.present {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            (status, Json(body)).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(missing_patch(source, format!("patch worker: {err}"))),
        )
            .into_response(),
    }
}

async fn file_at(State(state): State<Arc<AppState>>, Query(query): Query<FileQuery>) -> Response {
    let repo = match trusted_repo(&state.repo) {
        Ok(repo) => repo,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    let source = QueueSource::parse(query.source.as_deref());
    let sha = query.sha.clone();
    let path = query.path.clone();
    match tokio::task::spawn_blocking(move || {
        read_uplink_file(&repo, source, sha.as_deref(), &path)
    })
    .await
    {
        Ok(Ok(content)) => Json(FileResponse {
            path: query.path,
            sha: query.sha,
            content,
        })
        .into_response(),
        Ok(Err(err)) => (
            StatusCode::NOT_FOUND,
            Json(ErrorBody {
                error: err.to_string(),
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: format!("file worker: {err}"),
            }),
        )
            .into_response(),
    }
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(path) {
        return file_response(path, file.data.as_ref());
    }
    if let Some(file) = Assets::get("index.html") {
        return file_response("index.html", file.data.as_ref());
    }
    (StatusCode::INTERNAL_SERVER_ERROR, "embedded UI is missing").into_response()
}

fn file_response(path: &str, bytes: &[u8]) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(bytes.to_vec()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::util::ServiceExt;

    fn test_app(repo: PathBuf) -> Router {
        router(Arc::new(AppState { repo }))
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

    #[tokio::test]
    async fn patch_detail_route_captures_id() {
        let app = test_app(PathBuf::from("/tmp"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/patches/not-a-real-patch")
                    .body(Body::empty())
                    .unwrap(),
            )
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
            .oneshot(
                Request::builder()
                    .uri("/api/patches/..%2Fetc")
                    .body(Body::empty())
                    .unwrap(),
            )
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
    async fn file_at_rejects_repo_root_and_serves_uplink() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::create_dir_all(repo.join(".uplink")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "SECRET_ROOT_FILE\n").unwrap();
        std::fs::write(repo.join(".uplink/queue.json"), "{\"ok\":true}\n").unwrap();
        let app = test_app(repo.to_path_buf());

        for uri in [
            "/api/file?path=../Cargo.toml",
            "/api/file?path=.uplink/../Cargo.toml",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_ne!(response.status(), StatusCode::OK, "{uri}");
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8_lossy(&body);
            assert!(
                !text.contains("SECRET_ROOT_FILE"),
                "{uri} leaked repo-root content: {text}"
            );
        }

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/file?path=.uplink/queue.json")
                    .body(Body::empty())
                    .unwrap(),
            )
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
}
