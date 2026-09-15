use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::Serialize;
use tokio::net::TcpListener;

use crate::ops::{read_queue, status_snapshot, summarize_queue};
use crate::types::QUEUE_PATH;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

#[derive(Clone)]
struct AppState {
    repo: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    present: bool,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    company_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream_head: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    counts: Option<crate::ops::QueueCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    patches: Option<Vec<crate::types::Patch>>,
}

pub fn has_embedded_index() -> bool {
    Assets::get("index.html").is_some()
}

pub async fn serve(repo: PathBuf, addr: SocketAddr, open_browser: bool) -> std::io::Result<()> {
    let state = Arc::new(AppState { repo: repo.clone() });
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/docs/way-of-working.md", get(way_of_working))
        .route("/docs/templates.md", get(templates_readme))
        .fallback(static_file)
        .with_state(state);

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

async fn way_of_working() -> impl IntoResponse {
    markdown(include_str!("../way-of-working.md"))
}

async fn templates_readme() -> impl IntoResponse {
    markdown(include_str!("../templates/README.md"))
}

fn markdown(body: &'static str) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/markdown; charset=utf-8"),
        )],
        body,
    )
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let cwd = state.repo.display().to_string();
    if !state.repo.join(QUEUE_PATH).is_file() {
        return Json(StatusResponse {
            present: false,
            cwd,
            error: Some(format!("no {QUEUE_PATH} in this directory")),
            company_head: None,
            upstream_head: None,
            counts: None,
            patches: None,
        });
    }
    match read_queue(&state.repo).and_then(|_| status_snapshot(&state.repo)) {
        Ok(snapshot) => {
            let counts = summarize_queue(&snapshot.queue);
            Json(StatusResponse {
                present: true,
                cwd,
                error: None,
                company_head: Some(snapshot.company_head),
                upstream_head: snapshot.upstream_head,
                counts: Some(counts),
                patches: Some(snapshot.queue.patches),
            })
        }
        Err(err) => Json(StatusResponse {
            present: false,
            cwd,
            error: Some(err.to_string()),
            company_head: None,
            upstream_head: None,
            counts: None,
            patches: None,
        }),
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
