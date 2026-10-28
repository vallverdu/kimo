//! The coordinator's HTTP API (`/api/…`), for the owner: the kimo on the same machine
//! passes its owner's calls through (`/api/fleet/…` there), and the CLI calls it locally.
//! Requests from the machine itself are the owner (with the same DNS-rebinding guard as
//! kimo's), and so is the token, when one is set.

use crate::core::jobs::{JobSpec, Workflow};
use crate::services::fleet::{Fleet, Hold};
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

#[derive(Clone)]
struct App {
    fleet: Arc<Fleet>,
    token: Option<Arc<str>>,
    names: Arc<[String]>,
    ips: Arc<[IpAddr]>,
}

pub struct ApiError(StatusCode, String);

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let text = format!("{e:#}");
        let status = if text.starts_with("unknown ") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
        ApiError(status, text)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

/// Run a blocking call (machines are reached over the network) off the async threads.
async fn blocking<T: serde::Serialize + Send + 'static>(
    app: &App,
    f: impl FnOnce(&Fleet) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult {
    let fleet = app.fleet.clone();
    let value = tokio::task::spawn_blocking(move || f(&fleet))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(json!(value)))
}

pub fn router(fleet: Arc<Fleet>, token: Option<String>) -> Router {
    let names = kimo::core::access::own_names(
        &[fleet.cfg.host_name.clone(), kimo::config::hostname()],
        &[],
    );
    let app = App {
        fleet,
        token: token.filter(|t| !t.is_empty()).map(Into::into),
        names: names.into(),
        ips: kimo::services::network::local_ips().into(),
    };
    let api = Router::new()
        .route("/status", get(status))
        .route("/reload", post(reload))
        .route("/registry", get(get_registry).put(put_registry))
        .route("/devices", get(devices))
        .route("/devices/{id}", get(device).delete(remove))
        .route("/hardware", get(hardware))
        .route("/discover", get(discover))
        .route("/adopt", post(adopt))
        .route("/adoptions", get(adoptions))
        .route("/adoptions/{id}", get(adoption))
        .route("/ssh-key", get(ssh_key))
        .route("/devices/{id}/wake", post(wake))
        .route("/devices/{id}/shutdown", post(shutdown))
        .route("/devices/{id}/hold", post(hold))
        .route("/devices/{id}/pair", post(pair))
        .route("/devices/{id}/key", delete(forget_key))
        .route("/devices/{id}/samples", get(samples))
        .route("/events", get(events))
        .route("/jobs", get(jobs).post(submit))
        .route("/jobs/{id}", get(job))
        .route("/jobs/{id}/cancel", post(cancel))
        .route("/jobs/{id}/approve", post(approve))
        .route("/jobs/{id}/reject", post(reject))
        .route("/jobs/{id}/log", get(job_log))
        .route("/jobs/{id}/artifacts/{*path}", get(artifact))
        .route("/workflows", get(workflows).post(submit_workflow))
        .route("/workflows/{id}", get(workflow))
        .route("/workflows/{id}/cancel", post(cancel_workflow))
        .layer(middleware::from_fn_with_state(app.clone(), auth));
    Router::new().nest("/api", api).with_state(app)
}

/// The owner only: this machine itself (not through a proxy, not a web page, addressed by
/// one of its own names), or the token.
async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let header = |n: header::HeaderName| headers.get(n).and_then(|v| v.to_str().ok());
    let token_ok = match (&app.token, header(header::AUTHORIZATION)) {
        (Some(t), Some(given)) => given.strip_prefix("Bearer ") == Some(&**t),
        _ => false,
    };
    let host = header(header::HOST).unwrap_or_default();
    let forwarded = ["x-forwarded-for", "forwarded", "x-real-ip"]
        .iter()
        .any(|h| headers.contains_key(*h));
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let local = !forwarded
        && headers.get(header::ORIGIN).is_none()
        && peer.is_some_and(|ip| ip.is_loopback() || app.ips.contains(&ip))
        && kimo::core::access::is_own_authority(host, &app.names, &app.ips);
    if !(token_ok || local) {
        return ApiError(
            StatusCode::UNAUTHORIZED,
            "the fleet coordinator answers only its owner".into(),
        )
        .into_response();
    }
    next.run(req).await
}

async fn status(State(app): State<App>) -> Json<Value> {
    Json(app.fleet.status())
}

async fn reload(State(app): State<App>) -> ApiResult {
    app.fleet.reload()?;
    Ok(Json(
        json!({ "ok": true, "devices": app.fleet.devices().len() }),
    ))
}

async fn get_registry(State(app): State<App>) -> Response {
    let text = std::fs::read_to_string(app.fleet.registry_path()).unwrap_or_default();
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/yaml; charset=utf-8"),
        )],
        text,
    )
        .into_response()
}

/// Replace `devices.yaml` (checked first: a mistake changes nothing).
async fn put_registry(State(app): State<App>, text: String) -> ApiResult {
    crate::core::registry::parse(&text).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let path = app.fleet.registry_path();
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &text).map_err(anyhow::Error::from)?;
    std::fs::rename(&tmp, &path).map_err(anyhow::Error::from)?;
    app.fleet.reload()?;
    Ok(Json(json!({ "ok": true })))
}

async fn devices(State(app): State<App>) -> Json<Value> {
    Json(json!(app.fleet.devices()))
}

async fn device(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!(app.fleet.device(&id)?)))
}

/// Remove a machine added from the Fleet tab (its entry and its key).
async fn remove(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| {
        f.remove(&id)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// The hardware models and roles machines are added with.
async fn hardware() -> Json<Value> {
    Json(json!({
        "models": crate::core::hardware::MODELS,
        "roles": crate::core::hardware::ROLES,
    }))
}

/// Machines on Tailscale and ZeroTier, checked for kimo and SSH (a few seconds).
async fn discover(State(app): State<App>) -> ApiResult {
    blocking(&app, |f| Ok(f.discover())).await
}

/// Add a machine: over SSH (install, service, pairing) or with a code its kimo shows.
async fn adopt(
    State(app): State<App>,
    Json(req): Json<crate::services::fleet::adopt::Adopt>,
) -> Result<Response, ApiError> {
    let fleet = app.fleet.clone();
    let a = tokio::task::spawn_blocking(move || fleet.adopt(req))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok((StatusCode::ACCEPTED, Json(json!(a))).into_response())
}

async fn adoptions(State(app): State<App>) -> Json<Value> {
    Json(json!(app.fleet.adoptions()))
}

async fn adoption(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!(app.fleet.adoption(&id)?)))
}

/// The coordinator's public SSH key, to put on machines it should set up.
async fn ssh_key(State(app): State<App>) -> Json<Value> {
    Json(json!({ "key": app.fleet.ssh_key() }))
}

async fn wake(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.wake(&id)).await
}

async fn shutdown(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.shutdown(&id)).await
}

async fn hold(State(app): State<App>, Path(id): Path<String>, Json(h): Json<Hold>) -> ApiResult {
    blocking(&app, move |f| f.hold(&id, h)).await
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

async fn pair(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(b): Json<PairBody>,
) -> ApiResult {
    blocking(&app, move |f| {
        f.pair(&id, &b.code)?;
        f.device(&id)
    })
    .await
}

async fn forget_key(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| {
        f.forget_key(&id)?;
        f.device(&id)
    })
    .await
}

async fn samples(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    let hours = q.get("hours").and_then(|h| h.parse().ok()).unwrap_or(24.0);
    let list: Vec<Value> = app
        .fleet
        .samples(&id, hours)
        .into_iter()
        .map(|(at, facts)| json!({ "at": at, "facts": facts }))
        .collect();
    Json(json!(list))
}

async fn events(State(app): State<App>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
    Json(json!(app.fleet.events(
        q.get("device").map(String::as_str),
        q.get("job").map(String::as_str),
        limit
    )))
}

async fn jobs(State(app): State<App>) -> Json<Value> {
    Json(json!(app.fleet.jobs()))
}

/// A job (JSON or YAML; JSON is YAML too).
async fn submit(State(app): State<App>, text: String) -> Result<Response, ApiError> {
    let spec: JobSpec = serde_yaml_ng::from_str(&text)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("the job: {e}")))?;
    let job = app.fleet.submit(spec)?;
    Ok((StatusCode::CREATED, Json(json!(job))).into_response())
}

async fn job(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(json!(app.fleet.job(&id)?)))
}

async fn cancel(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.cancel(&id)).await
}

async fn approve(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.approve(&id, true)).await
}

async fn reject(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.approve(&id, false)).await
}

async fn job_log(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    let offset = q.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
    blocking(&app, move |f| f.job_log(&id, offset)).await
}

/// An output a job made, from the artifact store.
async fn artifact(
    State(app): State<App>,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let job = app.fleet.job(&id)?;
    let art = job
        .artifacts
        .get(&path)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("{id} has no output {path}")))?;
    let file = app
        .fleet
        .artifact_path(&art.sha256)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "that output was deleted".into()))?;
    let file = tokio::fs::File::open(file)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Streamed: models can be gigabytes.
    let stream = futures_util::stream::unfold(file, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256 << 10];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    });
    let name = path.rsplit('/').next().unwrap_or("output").replace('"', "");
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("inline; filename=\"{name}\""))
                    .unwrap_or(HeaderValue::from_static("inline")),
            ),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

async fn workflows(State(app): State<App>) -> Json<Value> {
    Json(json!(app.fleet.workflows()))
}

/// A workflow (YAML or JSON).
async fn submit_workflow(State(app): State<App>, text: String) -> Result<Response, ApiError> {
    let w: Workflow = serde_yaml_ng::from_str(&text)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("the workflow: {e}")))?;
    let run = app.fleet.submit_workflow(w)?;
    Ok((StatusCode::CREATED, Json(json!(run))).into_response())
}

async fn workflow(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let (run, jobs) = app.fleet.workflow(&id)?;
    let mut v = json!(run);
    v["jobs"] = json!(jobs);
    Ok(Json(v))
}

async fn cancel_workflow(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    blocking(&app, move |f| f.cancel_workflow(&id)).await
}
