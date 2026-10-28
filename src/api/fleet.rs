//! The fleet coordinator behind this kimo (`--fleet`): `/api/fleet/…` is its API, for the
//! owner only. The phone pairs with this kimo once and reaches every machine's power and
//! jobs through the coordinator, which holds the only keys to them.

use super::{ApiError, App};
use crate::client::Client;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

pub(super) async fn proxy(
    State(app): State<App>,
    Path(rest): Path<String>,
    req: Request,
) -> Result<Response, ApiError> {
    let Some((url, token)) = app.fleet.clone() else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "this kimo has no fleet coordinator (start it with --fleet)".into(),
        ));
    };
    let path = format!(
        "/{rest}{}",
        super::hosts::query_without_token(req.uri().query())
    );
    let (parts, body) = req.into_parts();
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body = axum::body::to_bytes(body, 32 << 20)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?
        .to_vec();
    let method = parts.method.to_string();
    let (status, content_type, bytes) = tokio::task::spawn_blocking(move || {
        Client::new(&url, token.as_deref().map(str::to_string)).raw(
            &method,
            &path,
            body,
            content_type.as_deref(),
            60,
        )
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("fleet coordinator: {e}")))?;
    let mut res = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
        bytes,
    )
        .into_response();
    if let Some(ct) = content_type.and_then(|c| HeaderValue::from_str(&c).ok()) {
        res.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    Ok(res)
}
