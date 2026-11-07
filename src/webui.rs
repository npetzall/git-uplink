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

use crate::assess::{FileDelta, revision_delta};
use crate::ops::{
    QueueCounts, StateStatus, approval_stale_at, ready_to_submit_ids, refresh_from_origin,
    stale_approvals_at, state_status_at, summarize_queue,
};
use crate::queue::{get_patch, patch_path, require_path_component, submit_layers};
use crate::repo::{
    COMPANY_REMOTE, FileRevision, file_history, has_ref, queue_at, rev_parse, show_at,
};
use crate::types::{LastSync, Patch, QueueState, STATE_BRANCH};

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    repo: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueSource {
    /// The local `uplink/state` branch, read with git.
    Local,
    /// `origin/uplink/state` as last fetched.
    Remote,
}

impl QueueSource {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("remote") => Self::Remote,
            _ => Self::Local,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
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

#[derive(Deserialize)]
struct DiffQuery {
    source: Option<String>,
    from: String,
    to: String,
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
    /// Queued upstream patches that submit can take now, in queue order.
    #[serde(skip_serializing_if = "Option::is_none")]
    ready_to_submit: Option<Vec<String>>,
}

/// Active upstream patches by the column they can go upstream in.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitStreamResponse {
    present: bool,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    layers: Vec<Vec<StreamNodeBody>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamNodeBody {
    id: String,
    title: String,
    status: String,
    depends_on: Vec<String>,
    /// Dependencies that are not active upstream patches.
    missing: Vec<String>,
    /// The newest approval does not cover the patch as it is now.
    stale: bool,
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
struct DiffResponse {
    from: String,
    to: String,
    files: Vec<FileDelta>,
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
        .route("/api/submit-stream", get(submit_stream))
        .route("/api/patches/{id}", get(patch_detail))
        .route("/api/patches/{id}/diff", get(patch_diff))
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
        ready_to_submit: None,
    }
}

fn source_ref(source: QueueSource) -> String {
    match source {
        QueueSource::Local => STATE_BRANCH.to_string(),
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

/// The queue on the source branch, read with git. The `.uplink/` folder and
/// its uncommitted edits are never read, and no branch is created or moved.
fn queue_for_source(repo: &Path, source: QueueSource) -> crate::error::Result<QueueState> {
    let git_ref = source_ref(source);
    if !has_ref(repo, &git_ref)? {
        return Err(crate::error::Error::msg(format!("no {git_ref} branch")));
    }
    queue_at(repo, &git_ref)
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
        QueueSource::Local => branch.clone(),
        QueueSource::Remote => format!("{COMPANY_REMOTE}/{branch}"),
    };
    optional_rev(repo, &git_ref)
}

fn upstream_head(repo: &Path, source: QueueSource) -> Option<String> {
    let git_ref = match source {
        QueueSource::Local => "uplink/upstream".to_string(),
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

/// A full commit id (SHA-1 or SHA-256), as listed in `revisions`.
fn is_valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
    let git_ref = match sha {
        Some(sha) => sha.to_string(),
        None => source_ref(source),
    };
    show_at(repo, &git_ref, path)
}

fn patch_revisions(repo: &Path, source: QueueSource, id: &str) -> Vec<FileRevision> {
    let path = patch_path(id)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    file_history(repo, &source_ref(source), &path).unwrap_or_default()
}

fn build_status(repo: &Path, source: QueueSource, fetch: bool) -> StatusResponse {
    let cwd = repo.display().to_string();
    maybe_fetch(repo, fetch);
    let queue = match queue_for_source(repo, source) {
        Ok(queue) => queue,
        Err(err) => {
            return missing_status(cwd, source, err.to_string());
        }
    };
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
        stale_approvals: Some(stale_approvals_at(repo, &source_ref(source), &queue)),
        ready_to_submit: Some(ready_to_submit_ids(&queue)),
    }
}

fn build_submit_stream(repo: &Path, source: QueueSource) -> SubmitStreamResponse {
    let body = |error: Option<String>, layers| SubmitStreamResponse {
        present: true,
        source: source.as_str().into(),
        error,
        layers,
    };
    let queue = match queue_for_source(repo, source) {
        Ok(queue) => queue,
        Err(err) => {
            return SubmitStreamResponse {
                present: false,
                ..body(Some(err.to_string()), Vec::new())
            };
        }
    };
    let layers = match submit_layers(&queue) {
        Ok(layers) => layers,
        Err(err) => return body(Some(err.to_string()), Vec::new()),
    };
    let git_ref = source_ref(source);
    let layers = layers
        .into_iter()
        .map(|column| {
            column
                .into_iter()
                .map(|node| StreamNodeBody {
                    id: node.patch.id.clone(),
                    title: node.patch.subject().to_string(),
                    status: node.patch.status.as_str().into(),
                    depends_on: node.patch.depends_on.clone(),
                    missing: node.missing,
                    stale: approval_stale_at(repo, &git_ref, &queue, node.patch),
                })
                .collect()
        })
        .collect();
    body(None, layers)
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
    let revisions = patch_revisions(repo, source, id);
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
        approval_stale: approval_stale_at(repo, &source_ref(source), &queue, &patch),
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

async fn submit_stream(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PatchQuery>,
) -> Json<SubmitStreamResponse> {
    let source = QueueSource::parse(query.source.as_deref());
    let repo = state.repo.clone();
    Json(
        tokio::task::spawn_blocking(move || build_submit_stream(&repo, source))
            .await
            .unwrap_or_else(|err| SubmitStreamResponse {
                present: false,
                source: source.as_str().into(),
                error: Some(format!("submit stream worker: {err}")),
                layers: Vec::new(),
            }),
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

/// What changed in a patch between two revisions of its patch file.
async fn patch_diff(
    State(state): State<Arc<AppState>>,
    PathParam(id): PathParam<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffResponse>, ApiError> {
    let path = patch_path(&id).map_err(ApiError::bad_request)?;
    let path = path.to_string_lossy().into_owned();
    if !is_valid_sha(&query.from) || !is_valid_sha(&query.to) {
        return Err(ApiError::bad_request("invalid sha"));
    }
    let source = QueueSource::parse(query.source.as_deref());
    let repo = state.repo.clone();
    let (from, to) = (query.from.clone(), query.to.clone());
    let files = tokio::task::spawn_blocking(move || {
        let older =
            read_uplink_file(&repo, source, Some(&from), &path).map_err(ApiError::not_found)?;
        let newer =
            read_uplink_file(&repo, source, Some(&to), &path).map_err(ApiError::not_found)?;
        revision_delta(&repo, &older, &newer).map_err(ApiError::internal)
    })
    .await
    .map_err(|err| ApiError::internal(format!("diff worker: {err}")))??;
    Ok(Json(DiffResponse {
        from: query.from,
        to: query.to,
        files,
    }))
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

    /// A repo whose `uplink/state` branch has `files` committed.
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

        let uri =
            format!("/api/patches/upl_abcdefghij/diff?source=remote&from={first}&to={second}");
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
    async fn local_source_reads_the_state_branch_not_the_folder() {
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
        assert_eq!(body["source"], "local");
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
    async fn local_and_remote_tabs_read_their_own_ref() {
        use crate::git::git_ok;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let local = queue_json(serde_json::json!([
            queued("upl_a", &[]),
            queued("upl_b", &["upl_a"]),
        ]));
        state_repo(repo, &[(".uplink/queue.json", &local)]);
        let mut merged = queued("upl_a", &[]);
        merged["status"] = "merged".into();
        let remote = queue_json(serde_json::json!([merged, queued("upl_b", &[])]));
        std::fs::write(repo.join(".uplink/queue.json"), remote).unwrap();
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-am",
            "remote",
        ];
        git_ok(repo, &commit).unwrap();
        let remote_sha = git_ok(repo, &["rev-parse", "HEAD"]).unwrap();
        git_ok(
            repo,
            &[
                "update-ref",
                "refs/remotes/origin/uplink/state",
                remote_sha.trim(),
            ],
        )
        .unwrap();
        git_ok(repo, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
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
            get("/api/status?source=remote").await["readyToSubmit"],
            serde_json::json!(["upl_b"])
        );
        assert_eq!(
            stream_ids(&get("/api/submit-stream").await),
            vec![vec!["upl_a"], vec!["upl_b"]]
        );
        let remote = get("/api/submit-stream?source=remote").await;
        assert_eq!(remote["source"], "remote");
        assert_eq!(stream_ids(&remote), vec![vec!["upl_b"]]);
    }
}
