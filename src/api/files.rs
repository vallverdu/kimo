//! `/projects/{id}/…` for files and git (owner only): the window's right panel and the
//! phone's review. Every call names the project and, for a worktree, its folder (`root`).

use super::{ApiError, ApiResult, App};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

type Q = Query<HashMap<String, String>>;

/// Run blocking file and git work off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(ApiError::from)
}

fn root(q: &HashMap<String, String>) -> Option<String> {
    q.get("root").cloned().filter(|r| !r.is_empty())
}

pub(super) async fn list(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let dir = q.get("dir").cloned().unwrap_or_default();
    let list = blocking(move || m.list_files(&id, root(&q).as_deref(), &dir)).await?;
    Ok(Json(json!(list)))
}

pub(super) async fn find(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let query = q.get("q").cloned().unwrap_or_default();
    let contents = q.get("contents").is_some_and(|v| v == "1" || v == "true");
    let found = blocking(move || m.find_files(&id, root(&q).as_deref(), &query, contents)).await?;
    Ok(Json(json!(found)))
}

pub(super) async fn read(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let path = q.get("path").cloned().unwrap_or_default();
    let file = blocking(move || m.read_file(&id, root(&q).as_deref(), &path)).await?;
    Ok(Json(json!(file)))
}

#[derive(Deserialize)]
pub(super) struct WriteBody {
    #[serde(default)]
    root: Option<String>,
    path: String,
    text: String,
    /// When the client read it (unix ms): a newer copy on disk is not overwritten.
    #[serde(default)]
    modified: Option<u64>,
}

pub(super) async fn write(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<WriteBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let result = tokio::task::spawn_blocking(move || {
        m.write_file(&id, b.root.as_deref(), &b.path, &b.text, b.modified)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match result {
        Ok(file) => Ok(Json(json!(file))),
        // Someone (an agent) saved it meanwhile: 409, so clients can offer to reload.
        Err(e) if e.to_string().contains("changed on disk") => {
            Err(ApiError(StatusCode::CONFLICT, e.to_string()))
        }
        Err(e) => Err(e.into()),
    }
}

pub(super) async fn status(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let state = blocking(move || m.git_status(&id, root(&q).as_deref())).await?;
    Ok(Json(json!(state)))
}

pub(super) async fn diff(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let path = q.get("path").cloned().unwrap_or_default();
    let staged = q.get("staged").is_some_and(|v| v == "1" || v == "true");
    let diff = blocking(move || m.git_diff(&id, root(&q).as_deref(), &path, staged)).await?;
    Ok(Json(json!({ "diff": diff })))
}

pub(super) async fn log(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Q,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(30);
    let commits = blocking(move || m.git_log(&id, root(&q).as_deref(), limit)).await?;
    Ok(Json(json!(commits)))
}

#[derive(Deserialize)]
pub(super) struct PathsBody {
    #[serde(default)]
    root: Option<String>,
    /// None or empty: everything.
    #[serde(default)]
    paths: Vec<String>,
}

pub(super) async fn stage(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<PathsBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    blocking(move || m.git_stage(&id, b.root.as_deref(), &b.paths, true)).await?;
    Ok(Json(json!({ "ok": true })))
}

pub(super) async fn unstage(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<PathsBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    blocking(move || m.git_stage(&id, b.root.as_deref(), &b.paths, false)).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(super) struct CommitBody {
    #[serde(default)]
    root: Option<String>,
    message: String,
}

pub(super) async fn commit(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<CommitBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let c = blocking(move || m.git_commit(&id, b.root.as_deref(), &b.message)).await?;
    Ok(Json(json!(c)))
}

#[derive(Deserialize)]
pub(super) struct RootBody {
    #[serde(default)]
    root: Option<String>,
}

pub(super) async fn push(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<RootBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    blocking(move || m.git_push(&id, b.root.as_deref())).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(super) struct PrBody {
    #[serde(default)]
    root: Option<String>,
    title: String,
    #[serde(default)]
    body: String,
}

pub(super) async fn pull_request(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<PrBody>,
) -> ApiResult<serde_json::Value> {
    let m = app.manager.clone();
    let url =
        blocking(move || m.git_pull_request(&id, b.root.as_deref(), &b.title, &b.body)).await?;
    Ok(Json(json!({ "url": url })))
}
