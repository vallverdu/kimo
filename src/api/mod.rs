//! HTTP + WebSocket API and the embedded PWA. The API is also the surface for custom tools.

mod hosts;

use crate::core::access::{self, Caller};
use crate::core::lifecycle::Action;
use crate::core::models::{Provider, Scope};
use crate::services::sessions::{Manager, NewSession};
use crate::services::stats;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Extension, Path, Query, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

#[derive(rust_embed::Embed)]
#[folder = "web/"]
struct Web;

#[derive(Clone)]
struct App {
    manager: Arc<Manager>,
    token: Option<Arc<str>>,
    /// Origins where the admin token may be used from another page (config).
    admin_origins: Arc<[String]>,
    local_owner: bool,
    /// Addresses of this machine: requests from them are the owner (when `local_owner`).
    local_ips: Arc<[IpAddr]>,
    /// This machine's names: only requests addressed to one of them (or to an address of
    /// the machine) are the local owner.
    own_names: Arc<[String]>,
    bound: IpAddr,
}

/// How the API is exposed.
pub struct Options {
    /// Optional owner token for scripts.
    pub token: Option<String>,
    /// Origins where the owner token may be used from another page.
    pub admin_origins: Vec<String>,
    /// Requests from this machine itself are the owner.
    pub local_owner: bool,
    /// Address kimo listens on (for the network page).
    pub bound: IpAddr,
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

pub fn router(manager: Arc<Manager>, options: Options) -> Router {
    let own_names = access::own_names(
        &[manager.host_name().to_string(), crate::config::hostname()],
        &options.admin_origins,
    );
    let app = App {
        manager,
        token: options.token.map(Into::into),
        admin_origins: options.admin_origins.into(),
        local_owner: options.local_owner,
        local_ips: crate::services::network::local_ips().into(),
        own_names: own_names.into(),
        bound: options.bound,
    };
    let api = Router::new()
        .route("/status", get(status))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/action", post(session_action))
        .route("/sessions/{id}/account", post(session_account))
        .route("/sessions/{id}/input", post(session_input))
        .route(
            "/sessions/{id}/files",
            // Photos: base64 makes them a third larger than the 20 MB limit.
            post(session_file).layer(axum::extract::DefaultBodyLimit::max(28 << 20)),
        )
        .route("/sessions/{id}/message", post(session_message))
        .route("/sessions/{id}/reply", get(session_reply))
        .route("/sessions/{id}/screen", get(session_screen))
        .route("/sessions/{id}/term", get(terminal))
        .route("/sessions/{id}/memory", get(session_memory))
        .route("/memory", get(memory))
        .route("/memory/free", post(free_memory))
        .route("/accounts", get(list_accounts).post(create_account))
        .route("/accounts/active", post(active_account))
        .route("/accounts/{id}", axum::routing::delete(delete_account))
        .route("/accounts/{id}/login", post(login_account))
        .route("/accounts/{id}/move", post(move_account))
        .route("/events", get(events))
        .route("/clients", get(list_clients).post(create_client))
        .route("/clients/{id}", axum::routing::delete(delete_client))
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/{id}",
            axum::routing::delete(delete_project).patch(update_project),
        )
        .route(
            "/projects/{id}/worktrees",
            post(add_worktree).delete(remove_worktree),
        )
        .route("/conversations", get(list_conversations))
        .route("/conversations/continue", post(continue_conversation))
        .route("/fs", get(browse).post(make_folder))
        .route("/network", get(network))
        .route("/network/https", post(enable_https))
        .route("/pairing", post(start_pairing))
        .route("/devices", get(list_devices))
        .route("/devices/{id}", axum::routing::delete(delete_device))
        .route("/qr", get(qr))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/agents", get(agents))
        .route("/usage", get(usage))
        .route("/usage/daily", get(daily_usage))
        .route("/update", get(check_update).post(install_update))
        .route("/ports", get(ports))
        .route("/awake", get(awake).post(set_awake))
        .route("/update/restart", post(restart))
        .route(
            "/skills",
            get(list_skills).post(install_skill).delete(remove_skill),
        )
        .route("/skills/kimo", post(install_kimo_skill))
        .route("/peers", get(list_peers).post(add_peer))
        .route("/peers/keys", get(peer_keys))
        .route("/peers/accept", post(accept_peer))
        .route("/peers/{id}", axum::routing::delete(remove_peer))
        .route("/hosts/{host}/{*rest}", axum::routing::any(hosts::proxy))
        .layer(middleware::from_fn_with_state(app.clone(), auth));
    // Pairing is the way in for a new device: it needs a one-time code, not a key.
    let public = Router::new().route("/pair", post(pair));
    Router::new()
        .nest("/api", api.merge(public))
        .fallback(static_file)
        .with_state(app)
}

fn denied(status: StatusCode, message: &str) -> Response {
    ApiError(status, message.into()).into_response()
}

/// Access control for every `/api` call:
/// - the token comes from `Authorization: Bearer` or `?token=` (websockets, EventSource);
/// - it is kimo's own token (admin) or an app key (`/api/clients`);
/// - a request from another page (`Origin` not kimo itself) needs a key listing that origin,
///   so no website can reach kimo, even on localhost without a token;
/// - app keys are limited by their scope.
async fn auth(State(app): State<App>, mut req: Request, next: Next) -> Response {
    // Read what is needed up front: a borrow of the request must not live across an await.
    let (origin, token, local) = {
        let header =
            |name: header::HeaderName| req.headers().get(name).and_then(|v| v.to_str().ok());
        let host = header(header::HOST).unwrap_or_default().to_string();
        let origin = header(header::ORIGIN)
            .filter(|o| !access::is_same_origin(o, &host))
            .map(str::to_string);
        let token = header(header::AUTHORIZATION)
            .and_then(|v| v.strip_prefix("Bearer "))
            .or_else(|| {
                req.uri()
                    .query()
                    .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
            })
            .map(str::to_string);
        // The machine itself, not through a proxy (tailscale serve also connects from localhost).
        let forwarded = [
            "x-forwarded-for",
            "forwarded",
            "x-real-ip",
            "tailscale-user-login",
        ]
        .iter()
        .any(|h| req.headers().contains_key(*h));
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        let local = app.local_owner
            && !forwarded
            && peer.is_some_and(|ip| ip.is_loopback() || app.local_ips.contains(&ip))
            && access::is_own_authority(&host, &app.own_names, &app.local_ips);
        (origin, token, local)
    };

    if req.method() == Method::OPTIONS {
        // CORS preflight: no credentials yet; allow origins any key (or the config) lists.
        let Some(origin) = origin else {
            return StatusCode::NO_CONTENT.into_response();
        };
        let known = access::origin_listed(&origin, &app.admin_origins)
            || app
                .manager
                .clients()
                .iter()
                .any(|c| access::origin_listed(&origin, &c.origins));
        if !known {
            return denied(StatusCode::FORBIDDEN, "origin not allowed");
        }
        let mut res = StatusCode::NO_CONTENT.into_response();
        cors(&mut res, &origin);
        let h = res.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        return res;
    }

    let owner_here = local && origin.is_none();
    let caller = match (&token, &app.token) {
        // kimo's own page (or curl) on the server itself is the owner.
        _ if owner_here => Some(Caller::Admin),
        (Some(t), Some(admin)) if t.as_str() == &**admin => Some(Caller::Admin),
        (Some(t), _) if app.manager.device_by_token(t).is_some() => Some(Caller::Admin),
        (Some(t), _) => app.manager.client_by_token(t).map(|c| Caller::Client {
            scope: c.scope,
            origins: c.origins,
        }),
        (None, _) => None,
    };
    let Some(caller) = caller else {
        return denied(StatusCode::UNAUTHORIZED, "missing or wrong token");
    };
    if let Some(origin) = &origin {
        if !access::cross_origin_allowed(&caller, origin, &app.admin_origins) {
            return denied(
                StatusCode::FORBIDDEN,
                "this key is not allowed from this origin",
            );
        }
    }
    if !access::permits(&caller, req.method().as_str(), req.uri().path()) {
        return denied(StatusCode::FORBIDDEN, "this key cannot do that");
    }
    // The key behind the call: open terminals and streams end when it is revoked.
    req.extensions_mut()
        .insert(Principal(if owner_here { None } else { token.clone() }));
    let mut res = next.run(req).await;
    if let Some(origin) = &origin {
        cors(&mut res, origin);
    }
    res
}

/// The key a call came with (`None`: the machine itself, which nothing revokes).
#[derive(Clone)]
pub(crate) struct Principal(pub(crate) Option<String>);

impl App {
    /// Is this key still one kimo accepts (owner token, paired device, app key)?
    fn key_valid(&self, token: &str) -> bool {
        self.token.as_deref() == Some(token)
            || self.manager.device_by_token(token).is_some()
            || self.manager.client_by_token(token).is_some()
    }

    /// Resolves when the key behind a connection is revoked (never for the machine itself).
    pub(crate) async fn until_revoked(&self, principal: Principal) {
        let Some(token) = principal.0 else {
            return std::future::pending().await;
        };
        let mut revocations = self.manager.revocations();
        loop {
            if !self.key_valid(&token) {
                return;
            }
            if revocations.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}

fn cors(res: &mut Response, origin: &str) {
    if let Ok(v) = HeaderValue::from_str(origin) {
        res.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
        res.headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
}

async fn status(State(app): State<App>) -> Json<serde_json::Value> {
    let sessions = app.manager.sessions();
    let (mem_total, mem_available) = stats::system_memory();
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "api": 1,
        "host": app.manager.host_name(),
        "kimo_rss_bytes": stats::self_rss_bytes(),
        "mem_total_bytes": mem_total,
        "mem_available_bytes": mem_available,
        "sessions": sessions.len(),
        "agents_rss_bytes": sessions.iter().map(|s| s.usage.rss_bytes).sum::<u64>(),
        "memory_pressure": crate::core::memory::pressure(mem_total, mem_available),
        "awake": crate::services::awake::status(app.manager.settings().keep_awake),
    }))
}

/// Sessions of this kimo; with `?all=1` also those of paired kimos (ids `host/id`).
async fn list_sessions(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    if q.get("all").is_some_and(|v| v == "1" || v == "true") {
        let manager = app.manager.clone();
        let all = tokio::task::spawn_blocking(move || manager.all_sessions())
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        return Ok(Json(json!(all)));
    }
    Ok(Json(json!(app.manager.sessions())))
}

fn enc(id: &str) -> String {
    id.replace('%', "%25")
        .replace('/', "%2F")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23")
}

/// Run a call on a paired kimo (for `host/id` sessions) and pass its answer through.
async fn forward(
    peer: crate::core::models::Peer,
    method: &'static str,
    path: String,
    body: Option<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    tokio::task::spawn_blocking(move || {
        Manager::peer_client(&peer)
            .call_timeout(method, &path, body.as_ref(), 60)
            .map_err(|e| (peer.name, e))
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map(Json)
    .map_err(|(host, e)| ApiError(StatusCode::BAD_GATEWAY, format!("{host}: {e}")))
}

/// The peer and path for a `host/id` session, if it is one.
fn remote(app: &App, id: &str, path: &str) -> Option<(crate::core::models::Peer, String)> {
    let (peer, rest) = app.manager.remote(id)?;
    Some((peer, format!("/sessions/{}{path}", enc(&rest))))
}

async fn get_session(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "") {
        return forward(peer, "GET", path, None).await;
    }
    // By id, name or unique id prefix (what people and agents type).
    app.manager
        .resolve(&id)
        .map(|s| Json(json!(s)))
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, e.to_string()))
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
    if let Some((peer, path)) = remote(&app, &id, "") {
        return forward(peer, "DELETE", path, None).await;
    }
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
    if let Some((peer, path)) = remote(&app, &id, "/action") {
        return forward(peer, "POST", path, Some(json!({ "action": body.action }))).await;
    }
    Ok(Json(json!(app.manager.act(&id, body.action).await?)))
}

#[derive(Deserialize)]
struct AccountBody {
    /// Account id; null, "system" or "default" means this computer's own login.
    account: Option<String>,
}

fn account_id(id: Option<String>) -> Option<String> {
    crate::services::sessions::account_key(id)
}

async fn session_account(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<AccountBody>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/account") {
        return forward(peer, "POST", path, Some(json!({ "account": body.account }))).await;
    }
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
#[derive(Deserialize)]
struct FileBody {
    name: String,
    /// The file, base64.
    data: String,
}

/// A file for the agent (a photo from the phone): saved on the session's machine; the answer's
/// `path` goes into the message, and Claude Code and Codex read the image from it.
async fn session_file(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<FileBody>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/files") {
        return forward(
            peer,
            "POST",
            path,
            Some(json!({ "name": body.name, "data": body.data })),
        )
        .await;
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body.data.trim())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "the file is not base64".into()))?;
    let manager = app.manager.clone();
    let path = tokio::task::spawn_blocking(move || manager.save_upload(&id, &body.name, &bytes))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!({ "path": path })))
}

async fn session_input(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<InputBody>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/input") {
        return forward(peer, "POST", path, Some(json!({ "text": body.text }))).await;
    }
    app.manager
        .session(&id)
        .ok_or_else(|| not_found("session"))?;
    app.manager.write(&id, body.text.into_bytes());
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MessageBody {
    text: String,
    /// The sending session (id or name), so the receiver knows who asks and can answer.
    #[serde(default)]
    from: Option<String>,
    /// The sender's name when it runs on another kimo (set by the forwarding kimo).
    #[serde(default)]
    from_name: Option<String>,
}

/// Message another agent: typed into it (as a paste, then Enter), signed with the sender.
async fn session_message(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<MessageBody>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/message") {
        // Over there, a local sender is `<our name there>/<id>` so the answer finds its way back.
        let sender = body
            .from
            .as_deref()
            .and_then(|f| app.manager.resolve(f).ok());
        let from_name = sender
            .as_ref()
            .map(|s| s.spec.name.clone())
            .or(body.from_name.clone());
        let from = body.from.as_deref().map(|f| match app.manager.resolve(f) {
            Ok(s) => format!("{}/{}", peer.self_name, s.spec.id),
            Err(_) => f.to_string(),
        });
        let to = id.clone();
        let mut answer = forward(
            peer,
            "POST",
            path,
            Some(json!({ "text": body.text, "from": from, "from_name": from_name })),
        )
        .await?;
        answer.0["to"] = json!(to);
        return Ok(answer);
    }
    let to = app
        .manager
        .message(
            &id,
            &body.text,
            body.from.as_deref(),
            body.from_name.as_deref(),
        )
        .await?;
    Ok(Json(json!({ "ok": true, "to": to })))
}

/// The agent's last reply (from its log) or its recent screen.
async fn session_reply(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/reply") {
        return forward(peer, "GET", path, None).await;
    }
    let manager = app.manager.clone();
    let reply = tokio::task::spawn_blocking(move || manager.reply(&id))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(reply)))
}

/// What an agent's memory is made of (the agent, its MCP servers, tools it runs).
async fn session_memory(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    if let Some((peer, path)) = remote(&app, &id, "/memory") {
        return forward(peer, "GET", path, None).await;
    }
    if app.manager.session(&id).is_none() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("unknown session: {id}"),
        ));
    }
    let manager = app.manager.clone();
    let parts = tokio::task::spawn_blocking(move || manager.memory_parts(&id))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(parts)))
}

async fn memory(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let report = tokio::task::spawn_blocking(move || manager.memory())
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(report)))
}

/// Stop every idle agent now; their conversations continue when they start again.
async fn free_memory(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let stopped = app.manager.free_idle().await;
    Ok(Json(json!({ "stopped": stopped })))
}

async fn session_screen(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let lines = q
        .get("lines")
        .and_then(|l| l.parse().ok())
        .unwrap_or(40)
        .min(2000);
    if let Some((peer, path)) = remote(&app, &id, &format!("/screen?lines={lines}")) {
        return forward(peer, "GET", path, None).await;
    }
    Ok(Json(json!(app.manager.screen(&id, lines)?)))
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
    if id == crate::services::sessions::SYSTEM {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "this computer's own login is not kept by kimo: sign out in the agent itself".into(),
        ));
    }
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
struct ActiveBody {
    provider: Provider,
    /// Account id; null or "system" = this computer's own login.
    account: Option<String>,
    /// Also move the provider's agents to it now (they continue their conversations).
    #[serde(default, rename = "move")]
    move_agents: bool,
}

/// Choose the subscription new agents of a provider start on.
async fn active_account(
    State(app): State<App>,
    Json(body): Json<ActiveBody>,
) -> ApiResult<serde_json::Value> {
    let moved = app
        .manager
        .set_active_account(body.provider, body.account, body.move_agents)
        .await?;
    Ok(Json(json!({ "moved": moved })))
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

/// Server-sent events: an `update` event with `{sessions, accounts}` whenever they change
/// (status, account, usage every ~2 s while agents run). The first event is the full state.
async fn events(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = app.manager.subscribe();
    let revoked = {
        let app = app.clone();
        Box::pin(async move { app.until_revoked(principal).await })
    };
    let stream = futures_util::stream::unfold(
        (app.manager, rx, String::new(), true),
        |(manager, mut rx, mut last, mut first)| async move {
            loop {
                if !first {
                    rx.changed().await.ok()?;
                    // Let a burst of changes settle into one event.
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    rx.borrow_and_update();
                }
                first = false;
                let body =
                    json!({ "sessions": manager.sessions(), "accounts": manager.accounts() })
                        .to_string();
                if body != last {
                    last = body.clone();
                    let event = Event::default().event("update").data(body);
                    return Some((Ok(event), (manager, rx, last, first)));
                }
            }
        },
    );
    // A revoked key's stream ends.
    let stream = futures_util::StreamExt::take_until(stream, revoked);
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)))
}

/// App keys without their secret (it is shown once, on creation).
async fn list_clients(State(app): State<App>) -> Json<serde_json::Value> {
    let list: Vec<_> = app
        .manager
        .clients()
        .into_iter()
        .map(|c| json!({ "id": c.id, "name": c.name, "origins": c.origins, "scope": c.scope, "token_hint": format!("{}…", &c.token[..9.min(c.token.len())]) }))
        .collect();
    Json(json!(list))
}

#[derive(Deserialize)]
struct NewClient {
    name: String,
    #[serde(default)]
    origins: Vec<String>,
    #[serde(default = "control")]
    scope: Scope,
}

fn control() -> Scope {
    Scope::Control
}

async fn create_client(
    State(app): State<App>,
    Json(body): Json<NewClient>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.add_client(
        &body.name,
        body.origins,
        body.scope
    )?)))
}

async fn delete_client(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.manager.delete_client(&id)?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_projects(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.projects()))
}

async fn create_project(
    State(app): State<App>,
    Json(body): Json<crate::services::projects::NewProject>,
) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    // git init / folder creation touch the disk: keep them off the async workers.
    let created = tokio::task::spawn_blocking(move || manager.create_project(body))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(created)))
}

/// Forget a project and end its terminals and agents (its folder stays; conversations stay
/// in the agents' history).
async fn delete_project(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let sessions = app.manager.sessions_of_project(&id)?;
    app.manager.delete_project(&id)?;
    for s in &sessions {
        app.manager.delete(s).await?;
    }
    Ok(Json(
        json!({ "ok": true, "sessions_removed": sessions.len() }),
    ))
}

async fn update_project(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<crate::services::projects::ProjectChanges>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.update_project(&id, body)?)))
}

async fn add_worktree(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<crate::services::projects::NewWorktree>,
) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let made = tokio::task::spawn_blocking(move || manager.add_worktree(&id, body))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(made)))
}

/// `?path=`: remove a worktree (git refuses when it has uncommitted changes) and the stopped
/// sessions that worked in it.
async fn remove_worktree(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let path = q.get("path").cloned().ok_or_else(|| not_found("path"))?;
    let manager = app.manager.clone();
    let sessions = tokio::task::spawn_blocking(move || manager.remove_worktree(&id, &path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    for s in &sessions {
        app.manager.delete(s).await?;
    }
    Ok(Json(
        json!({ "ok": true, "sessions_removed": sessions.len() }),
    ))
}

/// Listening TCP ports, each with the session that opened it (if one did).
async fn ports(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let list =
        tokio::task::spawn_blocking(move || crate::services::ports::list(&manager.sessions()))
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(list)))
}

async fn awake(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(crate::services::awake::status(
        app.manager.settings().keep_awake
    )))
}

#[derive(Deserialize)]
struct AwakeBody {
    mode: crate::core::awake::KeepAwake,
}

/// Keep the computer awake: `off`, `agents` (while an agent works) or `on`.
async fn set_awake(
    State(app): State<App>,
    Json(body): Json<AwakeBody>,
) -> ApiResult<serde_json::Value> {
    let mut settings = app.manager.settings();
    settings.keep_awake = body.mode;
    app.manager.set_settings(settings)?;
    crate::services::awake::tick(body.mode, false);
    Ok(Json(json!(crate::services::awake::status(body.mode))))
}

/// The latest kimo release (`?force=true` asks GitHub now instead of the 6-hour cache).
async fn check_update(Query(q): Query<HashMap<String, String>>) -> ApiResult<serde_json::Value> {
    let force = q.get("force").is_some_and(|v| v == "true" || v == "1");
    let release = tokio::task::spawn_blocking(move || crate::services::update::check(force))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(release)))
}

/// Install the latest release over this kimo's binary (it runs after a restart).
async fn install_update(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let data = app.manager.cfg.data_dir.clone();
    let version = tokio::task::spawn_blocking(move || crate::services::update::install(&data))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!({ "installed": version })))
}

/// Restart kimo (after an update): running sessions stop and start again, continuing their
/// conversations, once the new kimo is up.
async fn restart(State(app): State<App>) -> ApiResult<serde_json::Value> {
    app.manager.request_restart()?;
    Ok(Json(json!({ "ok": true })))
}

/// Earlier Claude and Codex conversations in a folder (`?path=`), newest first.
async fn list_conversations(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let path = q.get("path").cloned().ok_or_else(|| not_found("path"))?;
    let manager = app.manager.clone();
    let list = tokio::task::spawn_blocking(move || manager.conversations(&path))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
struct ContinueBody {
    agent: crate::core::models::SessionKind,
    cwd: String,
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "yes")]
    start: bool,
}

/// Continue an existing conversation as a kimo session (reused if one is attached already).
async fn continue_conversation(
    State(app): State<App>,
    Json(b): Json<ContinueBody>,
) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let session = tokio::task::spawn_blocking(move || {
        manager.continue_conversation(b.agent, &b.cwd, &b.id, b.name, b.start)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(session)))
}

async fn browse(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app
        .manager
        .browse(q.get("path").map(String::as_str))?)))
}

#[derive(Deserialize)]
struct NewFolder {
    parent: String,
    name: String,
}

/// `{parent, name}`: create a folder (folder pickers); returns its listing.
async fn make_folder(
    State(app): State<App>,
    Json(body): Json<NewFolder>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app
        .manager
        .make_folder(&body.parent, &body.name)?)))
}

async fn network(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let (bound, port) = (app.bound, app.manager.port());
    let info = tokio::task::spawn_blocking(move || crate::services::network::info(bound, port))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(info)))
}

async fn enable_https(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let port = app.manager.port();
    let url =
        tokio::task::spawn_blocking(move || crate::services::network::enable_tailscale_https(port))
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!({ "url": url })))
}

async fn start_pairing(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.start_pairing()))
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    #[serde(default)]
    name: String,
}

/// A new device trades its one-time code for its own key.
async fn pair(State(app): State<App>, Json(body): Json<PairBody>) -> ApiResult<serde_json::Value> {
    let device = app
        .manager
        .pair(&body.code, &body.name)
        .map_err(|e| ApiError(StatusCode::FORBIDDEN, e.to_string()))?;
    Ok(Json(json!(device)))
}

async fn list_devices(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.devices()))
}

async fn delete_device(
    State(app): State<App>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.manager.delete_device(&id)?;
    Ok(Json(json!({ "ok": true })))
}

/// QR code (SVG) for a pairing link or any address.
async fn qr(Query(q): Query<HashMap<String, String>>) -> Result<Response, ApiError> {
    let text = q
        .get("text")
        .filter(|t| !t.is_empty() && t.len() < 1024)
        .ok_or_else(|| not_found("text"))?;
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#232323"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        svg,
    )
        .into_response())
}

async fn get_settings(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.settings()))
}

async fn put_settings(
    State(app): State<App>,
    Json(body): Json<crate::core::models::Settings>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.set_settings(body)?)))
}

/// Usage per subscription (Claude tokens, Codex plan limits) of the agents run through kimo.
async fn usage(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let list = tokio::task::spawn_blocking(move || manager.usage())
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(list)))
}

/// Daily usage of every Claude and Codex conversation on this machine (`?days=`, default 30),
/// by day, project and account.
async fn daily_usage(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let days = q.get("days").and_then(|d| d.parse().ok()).unwrap_or(30);
    let manager = app.manager.clone();
    let report = tokio::task::spawn_blocking(move || manager.daily_usage(days))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(report)))
}

/// Agent CLIs installed on this machine, with versions.
async fn agents() -> ApiResult<serde_json::Value> {
    let list = tokio::task::spawn_blocking(crate::services::skills::detect_agents)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(list)))
}

async fn list_skills(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app
        .manager
        .skills(q.get("project").map(String::as_str))?)))
}

async fn install_skill(
    State(app): State<App>,
    Json(body): Json<crate::services::skills::InstallSkill>,
) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let installed = tokio::task::spawn_blocking(move || manager.install_skill(body))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(installed)))
}

#[derive(Deserialize)]
struct KimoSkillBody {
    #[serde(default = "both_agents")]
    agents: Vec<crate::services::skills::SkillAgent>,
}

fn both_agents() -> Vec<crate::services::skills::SkillAgent> {
    vec![
        crate::services::skills::SkillAgent::Claude,
        crate::services::skills::SkillAgent::Codex,
    ]
}

/// Teach agents to work together: install the bundled `kimo-agents` skill.
async fn install_kimo_skill(
    State(app): State<App>,
    Json(body): Json<KimoSkillBody>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(json!(app.manager.install_kimo_skill(&body.agents)?)))
}

async fn remove_skill(
    State(app): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let path = q.get("path").ok_or_else(|| not_found("path"))?;
    Ok(Json(json!({ "backup": app.manager.remove_skill(path)? })))
}

async fn list_peers(State(app): State<App>) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let peers = tokio::task::spawn_blocking(move || manager.peers())
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!(peers)))
}

/// Peers with their keys, for the owner's own apps (the window connects to every host).
async fn peer_keys(State(app): State<App>) -> Json<serde_json::Value> {
    Json(json!(app.manager.peer_list()))
}

#[derive(Deserialize)]
struct AddPeer {
    url: String,
    code: String,
    /// Where the other kimo reaches this one (default: picked from our addresses).
    #[serde(default)]
    my_url: Option<String>,
}

async fn add_peer(
    State(app): State<App>,
    Json(body): Json<AddPeer>,
) -> ApiResult<serde_json::Value> {
    let manager = app.manager.clone();
    let bound = app.bound;
    let peer = tokio::task::spawn_blocking(move || {
        let my_url = match body.my_url.filter(|u| !u.trim().is_empty()) {
            Some(u) => u,
            None => {
                let info = crate::services::network::info(bound, manager.port());
                let candidates: Vec<(crate::core::network::NetKind, String)> = info
                    .addresses
                    .iter()
                    .filter(|a| !a.https)
                    .map(|a| (a.kind, a.url.clone()))
                    .collect();
                crate::core::network::pick_address(&candidates, &body.url).ok_or_else(|| {
                    anyhow::anyhow!("this kimo only listens on localhost: start it with --bind all")
                })?
            }
        };
        manager.add_peer(&body.url, &body.code, &my_url)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(
        json!({ "id": peer.id, "name": peer.name, "url": peer.url }),
    ))
}

#[derive(Deserialize)]
struct AcceptPeer {
    name: String,
    url: String,
    token: String,
}

/// Called by a kimo that just paired with this one: it registers itself with a key for it.
async fn accept_peer(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
    Json(body): Json<AcceptPeer>,
) -> ApiResult<serde_json::Value> {
    // The device key the peer called with: revoked when it is forgotten here.
    let device = principal
        .0
        .and_then(|t| app.manager.device_by_token(&t))
        .map(|d| d.id);
    let peer = app
        .manager
        .accept_peer(&body.name, &body.url, &body.token, device)?;
    // `accepted_as`: the prefix under which this kimo lists the caller's agents.
    Ok(Json(
        json!({ "host": app.manager.host_name(), "accepted_as": peer.id }),
    ))
}

/// Forget a paired kimo: its key here is revoked (its open terminals close), and it is asked
/// to forget this kimo and revoke our key there. When it cannot be reached, the answer says
/// so: forget this kimo over there too.
async fn remove_peer(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let peer = app
        .manager
        .peer_list()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| not_found("host"))?;
    // `from_peer`: the other kimo is the one asking; it forgets us by itself.
    let remote = if q.contains_key("from_peer") {
        Ok(())
    } else {
        let p = peer.clone();
        tokio::task::spawn_blocking(move || {
            Manager::peer_client(&p)
                .call_timeout(
                    "DELETE",
                    &format!("/peers/{}?from_peer=1", p.self_name),
                    None,
                    8,
                )
                .map(|_| ())
        })
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };
    app.manager.remove_peer(&id)?;
    Ok(Json(match remote {
        Ok(()) => json!({ "ok": true, "remote_forgotten": true }),
        Err(e) => {
            tracing::warn!("{} could not forget this kimo: {e:#}", peer.name);
            json!({
                "ok": true,
                "remote_forgotten": false,
                "note": format!(
                    "{} was not reachable: forget {} in its Devices too, so the key this kimo has there stops working",
                    peer.name,
                    app.manager.host_name()
                ),
            })
        }
    }))
}

/// Terminal websocket: binary frames carry raw bytes both ways; text frames carry control
/// messages such as `{"type":"resize","cols":80,"rows":24}`.
async fn terminal(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Query(_q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let attachment = app
        .manager
        .attach(&id)
        .ok_or_else(|| not_found("session"))?;
    Ok(ws.on_upgrade(move |socket| async move {
        // The terminal closes when the key it was opened with is revoked.
        tokio::select! {
            _ = pump(socket, app.manager.clone(), id, attachment) => {}
            _ = app.until_revoked(principal) => {}
        }
    }))
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

/// kimo's web files. Browsers check with kimo before using a cached copy (a `304` when
/// unchanged), so a phone runs the new web app as soon as kimo is updated.
async fn static_file(uri: Uri, headers: axum::http::HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Web::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let hash = file.metadata.sha256_hash();
            let etag = format!(
                "\"{}\"",
                hash[..12]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let cached = headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(',').any(|t| t.trim() == etag));
            if cached {
                return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
            }
            let cache = "no-cache";
            let mut res = (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::CACHE_CONTROL, cache.to_string()),
                    (header::ETAG, etag),
                ],
                file.data,
            )
                .into_response();
            // The SDK and the terminal library are public: any app may load them.
            if path.starts_with("sdk/") || path.starts_with("vendor/xterm/") {
                res.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
            }
            res
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
