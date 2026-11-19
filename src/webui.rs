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
    QueueCounts, approval_stale_at, origin_status, ready_to_submit_ids, refresh_from_origin,
    summarize_queue,
};
use crate::queue::{get_patch, patch_path, require_path_component, submit_layers};
use crate::repo::{COMPANY_REMOTE, FileRevision, file_history, has_ref, queue_at, show_at};
use crate::types::{LastSync, Patch, QueueState, STATE_BRANCH};

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    repo: PathBuf,
}

#[derive(Deserialize)]
struct FileQuery {
    sha: Option<String>,
    path: String,
}

#[derive(Deserialize)]
struct DiffQuery {
    from: String,
    to: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    present: bool,
    cwd: String,
    /// `origin/uplink/state`, the only queue the UI reads.
    state_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_sha: Option<String>,
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

fn state_ref() -> String {
    format!("{COMPANY_REMOTE}/{STATE_BRANCH}")
}

fn missing_status(cwd: String, error: String) -> StatusResponse {
    StatusResponse {
        present: false,
        cwd,
        state_ref: state_ref(),
        state_sha: None,
        error: Some(error),
        company_head: None,
        upstream_head: None,
        counts: None,
        tooling: None,
        upstream: None,
        internal: None,
        last_sync: None,
        stale_approvals: None,
        ready_to_submit: None,
    }
}

/// The queue on `origin/uplink/state` as last fetched, read with git. The
/// `.uplink/` folder and local branches are never read, and nothing is fetched.
fn origin_queue(repo: &Path) -> crate::error::Result<QueueState> {
    let git_ref = state_ref();
    if !has_ref(repo, &git_ref)? {
        return Err(crate::error::Error::msg(format!("no {git_ref} branch")));
    }
    queue_at(repo, &git_ref)
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

fn read_uplink_file(repo: &Path, sha: Option<&str>, path: &str) -> crate::error::Result<String> {
    if !is_uplink_path(path) {
        return Err(crate::error::Error::msg("path must be under .uplink/"));
    }
    if sha.is_some_and(|sha| !is_valid_sha(sha)) {
        return Err(crate::error::Error::msg("invalid sha"));
    }
    let git_ref = match sha {
        Some(sha) => sha.to_string(),
        None => state_ref(),
    };
    show_at(repo, &git_ref, path)
}

fn patch_revisions(repo: &Path, id: &str) -> Vec<FileRevision> {
    let path = patch_path(id)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    file_history(repo, &state_ref(), &path).unwrap_or_default()
}

fn build_status(repo: &Path) -> StatusResponse {
    let cwd = repo.display().to_string();
    let status = match origin_status(repo) {
        Ok(status) => status,
        Err(err) => return missing_status(cwd, err.to_string()),
    };
    let queue = &status.queue;
    StatusResponse {
        present: true,
        cwd,
        state_ref: status.state_ref.clone(),
        state_sha: Some(status.state_sha.clone()),
        error: None,
        company_head: status.company_head.clone(),
        upstream_head: status.upstream_head.clone(),
        counts: Some(summarize_queue(queue)),
        tooling: queue.tooling.clone(),
        upstream: Some(queue.upstream.clone()),
        internal: Some(queue.internal.clone()),
        last_sync: queue.last_sync.clone(),
        ready_to_submit: Some(ready_to_submit_ids(queue)),
        stale_approvals: Some(status.stale_approvals),
    }
}

fn build_submit_stream(repo: &Path) -> SubmitStreamResponse {
    let body = |error: Option<String>, layers| SubmitStreamResponse {
        present: true,
        error,
        layers,
    };
    let queue = match origin_queue(repo) {
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
    let git_ref = state_ref();
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

fn build_patch(repo: &Path, id: &str) -> PatchResponse {
    let queue = match origin_queue(repo) {
        Ok(queue) => queue,
        Err(err) => {
            return missing_patch(err.to_string());
        }
    };
    let patch = match get_patch(&queue, id) {
        Ok(patch) => patch.clone(),
        Err(err) => {
            return missing_patch(err.to_string());
        }
    };
    let revisions = patch_revisions(repo, id);
    let path = match patch_path(id) {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(err) => {
            return missing_patch(err.to_string());
        }
    };
    let default_sha = revisions.first().map(|r| r.sha.as_str());
    let patch_file = read_uplink_file(repo, default_sha, &path).ok();
    PatchResponse {
        present: true,
        error: None,
        layer: Some(crate::queue::layer_label(&queue, id).to_string()),
        approval_stale: approval_stale_at(repo, &state_ref(), &queue, &patch),
        patch: Some(patch),
        revisions,
        patch_file,
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let repo = state.repo.clone();
    let cwd = repo.display().to_string();
    Json(
        tokio::task::spawn_blocking(move || build_status(&repo))
            .await
            .unwrap_or_else(|err| missing_status(cwd, format!("status worker: {err}"))),
    )
}

async fn submit_stream(State(state): State<Arc<AppState>>) -> Json<SubmitStreamResponse> {
    let repo = state.repo.clone();
    Json(
        tokio::task::spawn_blocking(move || build_submit_stream(&repo))
            .await
            .unwrap_or_else(|err| SubmitStreamResponse {
                present: false,
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

fn missing_patch(error: String) -> PatchResponse {
    PatchResponse {
        present: false,
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
) -> Response {
    if let Err(err) = require_path_component(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(missing_patch(err.to_string())),
        )
            .into_response();
    }
    let repo = state.repo.clone();
    match tokio::task::spawn_blocking(move || build_patch(&repo, &id)).await {
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
            Json(missing_patch(format!("patch worker: {err}"))),
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
    let repo = state.repo.clone();
    let (from, to) = (query.from.clone(), query.to.clone());
    let files = tokio::task::spawn_blocking(move || {
        let older = read_uplink_file(&repo, Some(&from), &path).map_err(ApiError::not_found)?;
        let newer = read_uplink_file(&repo, Some(&to), &path).map_err(ApiError::not_found)?;
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
    let sha = query.sha.clone();
    let path = query.path.clone();
    let content =
        tokio::task::spawn_blocking(move || read_uplink_file(&repo, sha.as_deref(), &path))
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
#[path = "webui_test.rs"]
mod tests;
