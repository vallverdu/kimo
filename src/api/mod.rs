//! HTTP + WebSocket API and the embedded PWA. The API is also the surface for custom tools.

use crate::core::lifecycle::Action;
use crate::core::models::Provider;
use crate::services::sessions::{Manager, NewSession};
use crate::services::stats;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

#[derive(rust_embed::Embed)]
#[folder = "web/"]
struct Web;

#[derive(Clone)]
struct App {
    manager: Arc<Manager>,
    token: Option<Arc<str>>,
}

/// Errors become `{ "error": "..." }` with a plain message the UI can show.
struct ApiError(StatusCode, String);

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::warn!("{e:#}");
        ApiError(StatusCode::BAD_REQUEST, format!("{e:#}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("unknown {what}"))
}

pub fn router(manager: Arc<Manager>, token: Option<String>) -> Router {
    let app = App {
        manager,
        token: token.map(Into::into),
    };
    let api = Router::new()
        .route("/status", get(status))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/action", post(session_action))
        .route("/sessions/{id}/account", post(session_account))
        .route("/sessions/{id}/input", post(session_input))
        .route("/sessions/{id}/term", get(terminal))
        .route("/accounts", get(list_accounts).post(create_account))
        .route("/accounts/{id}", axum::routing::delete(delete_account))
        .route("/accounts/{id}/login", post(login_account))
        .route("/accounts/{id}/move", post(move_account))
        .layer(middleware::from_fn_with_state(app.clone(), auth));
    Router::new()
        .nest("/api", api)
        .fallback(static_file)
        .with_state(app)
}

/// Token from `Authorization: Bearer`, or `?token=` (browsers cannot set websocket headers).
async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    let Some(expected) = &app.token else {
        return next.run(req).await;
    };
    let header_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let query_token = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")));
    if header_token.or(query_token) == Some(expected) {
        next.run(req).await
    } else {
        ApiError(StatusCode::UNAUTHORIZED, "missing or wrong token".into()).into_response()
    }
}

async fn status(State(app): State<App>) -> Json<serde_json::Value> {
    let sessions = app.manager.sessions();
    let (mem_total, mem_available) = stats::system_memory();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "kimo_rss_bytes": stats::self_rss_bytes(),
        "mem_total_bytes": mem_total,
        "mem_available_bytes": mem_available,
        "sessions": sessions.len(),
        "agents_rss_bytes": sessions.iter().map(|s| s.usage.rss_bytes).sum::<u64>(),
    }))
}

async fn list_sessions(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.sessions()))
}

async fn get_session(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.manager
        .session(&id)
        .map(|s| Json(json!(s)))
        .ok_or_else(|| not_found("session"))
}

#[derive(Deserialize)]
struct CreateSession {
    #[serde(flatten)]
    new: NewSession,
    #[serde(default = "yes")]
    start: bool,
}

fn yes() -> bool {
    true
}

async fn create_session(
    State(app): State<App>,
    Json(body): Json<CreateSession>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.create(body.new, body.start)?)))
}

async fn delete_session(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.manager.delete(&id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ActionBody {
    action: Action,
}

async fn session_action(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<ActionBody>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.act(&id, body.action).await?)))
}

#[derive(Deserialize)]
struct AccountBody {
    /// Account id; null or "default" means the provider's default login.
    account: Option<String>,
}

fn account_id(id: Option<String>) -> Option<String> {
    id.filter(|a| !a.is_empty() && a != "default")
}

async fn session_account(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<AccountBody>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(
        app.manager
            .set_account(&id, account_id(body.account))
            .await?
    )))
}

#[derive(Deserialize)]
struct InputBody {
    text: String,
}

/// Type into a session without a terminal (quick replies from a notification, scripts…).
async fn session_input(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<InputBody>,
) -> ApiResult<serde_json::Value> {
    app.manager
        .session(&id)
        .ok_or_else(|| not_found("session"))?;
    app.manager.write(&id, body.text.into_bytes());
    Ok(Json(json!({ "ok": true })))
}

async fn list_accounts(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.accounts()))
}

#[derive(Deserialize)]
struct NewAccount {
    name: String,
    provider: Provider,
}

async fn create_account(
    State(app): State<App>,
    Json(body): Json<NewAccount>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app
        .manager
        .add_account(&body.name, body.provider)?)))
}

async fn delete_account(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.manager.delete_account(&id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn login_account(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.login_session(&id)?)))
}

#[derive(Deserialize)]
struct MoveBody {
    to: Option<String>,
}

/// Move every session of account `id` ("default" = default login) to another account.
async fn move_account(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<MoveBody>,
) -> ApiResult<serde_json::Value> {
    let from = account_id(Some(id));
    let moved = app
        .manager
        .move_sessions(from.as_deref(), account_id(body.to))
        .await?;
    Ok(Json(json!({ "moved": moved })))
}

/// Terminal websocket: binary frames carry raw bytes both ways; text frames carry control
/// messages such as `{"type":"resize","cols":80,"rows":24}`.
async fn terminal(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(_q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let attachment = app
        .manager
        .attach(&id)
        .ok_or_else(|| not_found("session"))?;
    Ok(ws.on_upgrade(move |socket| pump(socket, app.manager, id, attachment)))
}

async fn pump(
    socket: WebSocket,
    manager: Arc<Manager>,
    id: String,
    attachment: crate::services::sessions::Attachment,
) {
    let (mut tx, mut rx) = socket.split();
    let mut output = attachment.output;
    if tx
        .send(Message::Binary(attachment.replay.into()))
        .await
        .is_err()
    {
        return;
    }
    let send = async {
        loop {
            match output.recv().await {
                Ok(bytes) => {
                    if tx.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    };
    let receive = async {
        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                Message::Binary(data) => manager.write(&id, data.to_vec()),
                Message::Text(text) => {
                    if let Ok(ctl) = serde_json::from_str::<serde_json::Value>(&text) {
                        match ctl["type"].as_str() {
                            Some("resize") => manager.resize(
                                &id,
                                ctl["cols"].as_u64().unwrap_or(0) as u16,
                                ctl["rows"].as_u64().unwrap_or(0) as u16,
                            ),
                            Some("input") => manager
                                .write(&id, ctl["data"].as_str().unwrap_or("").as_bytes().to_vec()),
                            _ => {}
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    tokio::select! {
        _ = send => {}
        _ = receive => {}
    }
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Web::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let cache = if path == "index.html" || path == "sw.js" {
                "no-cache"
            } else {
                "public, max-age=3600"
            };
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
