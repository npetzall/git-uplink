use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path as PathParam, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

use crate::ops::{
    QueueCounts, StateStatus, approval_stale, refresh_from_origin, stale_approvals,
    state_status_at, summarize_queue,
};
use crate::queue::{get_patch, patch_path, require_path_component};
use crate::repo::{
    COMPANY_REMOTE, FileRevision, file_history, has_ref, queue_at, rev_parse, show_at,
};
use crate::types::{LastSync, Patch, QUEUE_PATH, QueueState, STATE_BRANCH};

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
    /// Approved or submitted patches that changed since their last approval.
    #[serde(skip_serializing_if = "Option::is_none")]
    stale_approvals: Option<Vec<String>>,
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
    /// The newest approval does not cover the patch as it is now.
    approval_stale: bool,
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

/// JSON `{"error": …}` response with a status code.
struct ApiError {
    status: StatusCode,
    error: String,
}

impl ApiError {
    fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
        }
    }

    fn bad_request(err: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_REQUEST, err.to_string())
    }

    fn not_found(err: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::NOT_FOUND, err.to_string())
    }

    fn internal(err: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorBody { error: self.error })).into_response()
    }
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
        .layer(middleware::from_fn(loopback_only))
        .with_state(state)
}

/// Host part of `host[:port]` or `[v6]:port`.
fn authority_host(authority: &str) -> &str {
    if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority.split(':').next().unwrap_or(authority)
    }
}

fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host == "[::1]" || host.eq_ignore_ascii_case("localhost")
}

/// Blocks DNS rebinding (foreign `Host`) and cross-site requests (foreign `Origin`).
fn is_local_request(headers: &HeaderMap) -> bool {
    let host_ok = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| is_loopback_host(authority_host(host)));
    let origin_ok = match headers.get(header::ORIGIN) {
        None => true,
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|origin| origin.strip_prefix("http://"))
            .is_some_and(|rest| is_loopback_host(authority_host(rest))),
    };
    host_ok && origin_ok
}

async fn loopback_only(request: Request, next: Next) -> Response {
    if !is_local_request(request.headers()) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "web-ui only serves requests from 127.0.0.1",
        )
        .into_response();
    }
    next.run(request).await
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

/// Fetching from origin is opt-in: a plain `GET /api/status` never touches the network.
fn wants_fetch(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
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
        stale_approvals: None,
    }
}

fn source_ref(source: QueueSource) -> String {
    match source {
        QueueSource::Checkout => STATE_BRANCH.to_string(),
        QueueSource::Remote => format!("{COMPANY_REMOTE}/{STATE_BRANCH}"),
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
        QueueSource::Remote => queue_at(repo, &source_ref(source)),
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

/// `worktree` or a full commit id (SHA-1 or SHA-256), as listed in `revisions`.
fn is_valid_sha(sha: &str) -> bool {
    sha == "worktree"
        || (matches!(sha.len(), 40 | 64)
            && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
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
    if sha.is_some_and(|sha| !is_valid_sha(sha)) {
        return Err(crate::error::Error::msg("invalid sha"));
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
        None => source_ref(source),
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
    let git_ref = source_ref(source);
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
        stale_approvals: Some(stale_approvals(repo, &queue)),
    }
}

fn build_patch(repo: &Path, id: &str, source: QueueSource) -> PatchResponse {
    let queue = match queue_for_source(repo, source) {
        Ok(queue) => queue,
        Err(err) => {
            return missing_patch(source, err.to_string());
        }
    };
    let patch = match get_patch(&queue, id) {
        Ok(patch) => patch.clone(),
        Err(err) => {
            return missing_patch(source, err.to_string());
        }
    };
    let uncommitted = state_status_at(repo, false)
        .map(|s| s.uncommitted)
        .unwrap_or_default();
    let revisions = patch_revisions(repo, source, id, &uncommitted);
    let path = match patch_path(id) {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(err) => {
            return missing_patch(source, err.to_string());
        }
    };
    let default_sha = revisions.first().map(|r| r.sha.as_str());
    let patch_file = read_uplink_file(repo, source, default_sha, &path).ok();
    PatchResponse {
        present: true,
        source: source.as_str().into(),
        error: None,
        layer: Some(crate::queue::layer_label(&queue, id).to_string()),
        approval_stale: approval_stale(repo, &queue, &patch),
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
    let repo = state.repo.clone();
    let cwd = repo.display().to_string();
    Json(
        tokio::task::spawn_blocking(move || build_status(&repo, source, fetch))
            .await
            .unwrap_or_else(|err| missing_status(cwd, source, format!("status worker: {err}"))),
    )
}

async fn refresh(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    let repo = state.repo.clone();
    let result = tokio::task::spawn_blocking(move || refresh_from_origin(&repo))
        .await
        .map_err(|err| ApiError::internal(format!("refresh worker: {err}")))?
        .map_err(ApiError::bad_request)?;
    Ok(Json(result).into_response())
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
        approval_stale: false,
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
    let repo = state.repo.clone();
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

async fn file_at(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileResponse>, ApiError> {
    let repo = state.repo.clone();
    if query.sha.as_deref().is_some_and(|sha| !is_valid_sha(sha)) {
        return Err(ApiError::bad_request("invalid sha"));
    }
    let source = QueueSource::parse(query.source.as_deref());
    let sha = query.sha.clone();
    let path = query.path.clone();
    let content =
        tokio::task::spawn_blocking(move || read_uplink_file(&repo, source, sha.as_deref(), &path))
            .await
            .map_err(|err| ApiError::internal(format!("file worker: {err}")))?
            .map_err(ApiError::not_found)?;
    Ok(Json(FileResponse {
        path: query.path,
        sha: query.sha,
        content,
    }))
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

    #[test]
    fn status_fetch_is_opt_in() {
        for value in [None, Some(""), Some("0"), Some("false"), Some("bogus")] {
            assert!(!wants_fetch(value), "{value:?}");
        }
        for value in ["1", "true", "yes", " 1 "] {
            assert!(wants_fetch(Some(value)), "{value}");
        }
    }

    #[tokio::test]
    async fn foreign_host_is_forbidden() {
        let app = test_app(PathBuf::from("/tmp"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status?fetch=0")
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
        assert!(is_valid_sha("worktree"));
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
        std::fs::create_dir_all(repo.join(".uplink")).unwrap();
        std::fs::write(repo.join(".uplink/queue.json"), "{}\n").unwrap();
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
}
