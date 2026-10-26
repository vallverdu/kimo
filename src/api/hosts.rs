//! Paired kimos reached through this one: `/api/hosts/{host}/…` is the same API on another
//! machine (calls, the live event stream and terminals), with this kimo's key there. Phones
//! paired with one kimo use every kimo it is paired with, without pairing again.

use super::{ApiError, App, Principal};
use crate::core::models::Peer;
use crate::services::sessions::Manager;
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, FromRequestParts, Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;

fn peer_of(app: &App, host: &str) -> Result<Peer, ApiError> {
    app.manager
        .peer_list()
        .into_iter()
        .find(|p| p.id == host || p.name == host)
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                format!("no paired kimo named {host}"),
            )
        })
}

/// The query without this kimo's key (the call to the peer carries the peer's key).
fn query_without_token(query: Option<&str>) -> String {
    let kept: Vec<&str> = query
        .unwrap_or_default()
        .split('&')
        .filter(|kv| !kv.is_empty() && !kv.starts_with("token="))
        .collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("?{}", kept.join("&"))
    }
}

pub(super) async fn proxy(
    State(app): State<App>,
    Extension(principal): Extension<Principal>,
    Path((host, rest)): Path<(String, String)>,
    req: Request,
) -> Result<Response, ApiError> {
    let peer = peer_of(&app, &host)?;
    let path = format!("/{rest}{}", query_without_token(req.uri().query()));
    let (mut parts, body) = req.into_parts();
    let upgrade = parts
        .headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if upgrade {
        let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
            .await
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
        let url = Manager::peer_client(&peer).ws_url(&path);
        // Like a local terminal, it closes when the caller's key is revoked here.
        return Ok(ws.on_upgrade(move |socket| async move {
            tokio::select! {
                _ = bridge(socket, url) => {}
                _ = app.until_revoked(principal) => {}
            }
        }));
    }
    if parts.method == axum::http::Method::GET && rest == "events" {
        return Ok(events(peer, app, principal));
    }
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
    let name = peer.name.clone();
    let (status, content_type, bytes) = tokio::task::spawn_blocking(move || {
        Manager::peer_client(&peer).raw(&method, &path, body, content_type.as_deref(), 120)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{name}: {e}")))?;
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

/// The peer's event stream, passed through as it arrives (until the caller's key is revoked).
fn events(peer: Peer, app: App, principal: Principal) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    std::thread::Builder::new()
        .name("kimo-host-events".into())
        .spawn(move || {
            use std::io::Read;
            let Ok(mut body) = Manager::peer_client(&peer).stream("/events") else {
                return;
            };
            let mut buf = vec![0u8; 16 << 10];
            while let Ok(n) = body.read(&mut buf) {
                if n == 0 || tx.blocking_send(Bytes::copy_from_slice(&buf[..n])).is_err() {
                    break;
                }
            }
        })
        .ok();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx))
    });
    let revoked = Box::pin(async move { app.until_revoked(principal).await });
    let stream = futures_util::StreamExt::take_until(stream, revoked);
    let mut res = Body::from_stream(stream).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

/// A terminal on the peer: messages both ways until either side closes.
async fn bridge(socket: WebSocket, url: String) {
    let (to_client, mut from_peer) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let (to_peer, from_client) = std::sync::mpsc::channel::<Message>();
    std::thread::Builder::new()
        .name("kimo-host-term".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            use tungstenite::{stream::MaybeTlsStream, Message as M};
            let Ok((mut ws, _)) = tungstenite::connect(&url) else {
                let _ = to_client.send(Message::Close(None));
                return;
            };
            if let MaybeTlsStream::Plain(s) = ws.get_mut() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(25)));
            }
            loop {
                match ws.read() {
                    Ok(M::Binary(b)) => {
                        if to_client.send(Message::Binary(b)).is_err() {
                            break;
                        }
                    }
                    Ok(M::Text(t)) => {
                        if to_client.send(Message::Text(t.as_str().into())).is_err() {
                            break;
                        }
                    }
                    Ok(M::Close(_)) => break,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
                loop {
                    match from_client.try_recv() {
                        Ok(Message::Binary(b)) => {
                            if ws.send(M::Binary(b)).is_err() {
                                return;
                            }
                        }
                        Ok(Message::Text(t)) => {
                            if ws.send(M::Text(t.as_str().into())).is_err() {
                                return;
                            }
                        }
                        Ok(Message::Close(_))
                        | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let _ = ws.close(None);
                            return;
                        }
                        Ok(_) => {}
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    }
                }
            }
            let _ = to_client.send(Message::Close(None));
        })
        .ok();
    let (mut tx, mut rx) = socket.split();
    let send = async {
        while let Some(msg) = from_peer.recv().await {
            let close = matches!(msg, Message::Close(_));
            if tx.send(msg).await.is_err() || close {
                break;
            }
        }
    };
    let receive = async {
        while let Some(Ok(msg)) = rx.next().await {
            let close = matches!(msg, Message::Close(_));
            if to_peer.send(msg).is_err() || close {
                break;
            }
        }
    };
    tokio::select! {
        _ = send => {}
        _ = receive => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_key_is_not_passed_on() {
        assert_eq!(super::query_without_token(None), "");
        assert_eq!(super::query_without_token(Some("token=abc")), "");
        assert_eq!(
            super::query_without_token(Some("days=7&token=abc&x=1")),
            "?days=7&x=1"
        );
    }
}
