//! Local HTTP server: the embedded web UI, its WebSocket, and optionally MCP over HTTP.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rust_embed::RustEmbed;
use tokio::net::TcpListener;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::controller::{Controller, Event, Source};
use crate::mcp::ConductorMcp;
use crate::protocol::{ClientCommand, ServerMessage};

const CONSOLE_BACKLOG: usize = 2000;

#[derive(RustEmbed)]
#[folder = "ui/dist"]
struct Assets;

fn asset(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => (
            [(
                header::CONTENT_TYPE,
                mime_guess::from_path(path)
                    .first_or_octet_stream()
                    .to_string(),
            )],
            file.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    }
}

/// Browsers send an Origin header; only accept pages served from this machine
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else {
        return true;
    };
    let host = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .unwrap_or(origin);
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(ctrl): State<Controller>,
) -> Response {
    if !origin_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin WebSocket refused").into_response();
    }
    ws.on_upgrade(move |socket| client(socket, ctrl))
}

async fn send(
    tx: &mut (impl SinkExt<Message, Error = axum::Error> + Unpin),
    msg: &ServerMessage,
) -> bool {
    match serde_json::to_string(msg) {
        Ok(json) => tx.send(Message::Text(json.into())).await.is_ok(),
        Err(_) => true,
    }
}

async fn client(socket: WebSocket, ctrl: Controller) {
    let (mut tx, mut rx) = socket.split();
    let mut events = ctrl.subscribe();

    let hello = [
        ServerMessage::Info(ctrl.info()),
        ServerMessage::State(Box::new(ctrl.snapshot())),
        ServerMessage::ConsoleBacklog {
            lines: ctrl.console_recent(CONSOLE_BACKLOG),
        },
    ];
    for msg in &hello {
        if !send(&mut tx, msg).await {
            return;
        }
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                let msg = match event {
                    Ok(Event::State(s)) => ServerMessage::State(Box::new((*s).clone())),
                    Ok(Event::Console(line)) => ServerMessage::Console { line },
                    Ok(Event::ConsoleCleared) => ServerMessage::ConsoleCleared,
                    // Fell behind: resynchronise everything
                    Err(RecvError::Lagged(_)) => {
                        if !send(&mut tx, &ServerMessage::ConsoleBacklog {
                            lines: ctrl.console_recent(CONSOLE_BACKLOG),
                        }).await {
                            return;
                        }
                        ServerMessage::State(Box::new(ctrl.snapshot()))
                    }
                    Err(RecvError::Closed) => return,
                };
                if !send(&mut tx, &msg).await {
                    return;
                }
            }
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let result = serde_json::from_str::<ClientCommand>(&text)
                        .map_err(|e| format!("invalid command: {e}"))
                        .and_then(|cmd| ctrl.apply(cmd, Source::Ui));
                    if let Err(message) = result
                        && !send(&mut tx, &ServerMessage::Error { message }).await
                    {
                        return;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            }
        }
    }
}

fn ui_router(ctrl: Controller) -> Router {
    Router::new()
        .route("/", get(|| async { asset("index.html") }))
        .route("/console", get(|| async { asset("console.html") }))
        .route("/ws", get(ws_handler))
        .route(
            "/api/state",
            get(|State(ctrl): State<Controller>| async move { Json(ctrl.snapshot()) }),
        )
        .route(
            "/{*path}",
            get(|Path(path): Path<String>| async move { asset(&path) }),
        )
        .with_state(ctrl)
}

/// The MCP Streamable HTTP endpoint, mounted at `/mcp`
pub fn mcp_router(ctrl: Controller, cancel: CancellationToken) -> Router {
    // Host validation (loopback only) is on by default. MCP clients don't send an Origin
    // header and no web page should call this endpoint, so reject every request that has one.
    let mut config = StreamableHttpServerConfig::default().enforce_origin_validation();
    config.cancellation_token = cancel;
    let service: StreamableHttpService<ConductorMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(ConductorMcp::new(ctrl.clone())),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    Router::new().nest_service("/mcp", service)
}

async fn serve(listener: TcpListener, router: Router, cancel: CancellationToken) {
    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await;
}

pub struct Servers {
    pub ui_addr: SocketAddr,
    pub mcp_addr: Option<SocketAddr>,
}

impl Servers {
    pub fn ui_url(&self) -> String {
        format!("http://{}", self.ui_addr)
    }

    pub fn mcp_url(&self) -> Option<String> {
        self.mcp_addr.map(|a| format!("http://{a}/mcp"))
    }
}

/// Starts the UI server, and the MCP HTTP server if `mcp_addr` is given
pub async fn start(
    ctrl: Controller,
    ui_addr: SocketAddr,
    mcp_addr: Option<SocketAddr>,
    cancel: CancellationToken,
) -> anyhow::Result<Servers> {
    let ui = TcpListener::bind(ui_addr)
        .await
        .with_context(|| format!("could not listen on {ui_addr}"))?;
    let ui_addr = ui.local_addr()?;
    tokio::spawn(serve(ui, ui_router(ctrl.clone()), cancel.clone()));

    let mcp_addr = match mcp_addr {
        Some(addr) => {
            let listener = TcpListener::bind(addr)
                .await
                .with_context(|| format!("could not listen for MCP on {addr}"))?;
            let addr = listener.local_addr()?;
            tokio::spawn(serve(
                listener,
                mcp_router(ctrl.clone(), cancel.clone()),
                cancel.clone(),
            ));
            Some(addr)
        }
        None => None,
    };

    let servers = Servers { ui_addr, mcp_addr };
    ctrl.set_mcp_url(servers.mcp_url());
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        let check = |origin: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(o) = origin {
                headers.insert(header::ORIGIN, o.parse().unwrap());
            }
            origin_allowed(&headers)
        };
        assert!(check(None));
        assert!(check(Some("http://127.0.0.1:5173")));
        assert!(check(Some("http://localhost:41234")));
        assert!(check(Some("http://[::1]:80")));
        assert!(!check(Some("https://evil.example")));
        assert!(!check(Some("http://127.0.0.1.evil.example:80")));
        assert!(!check(Some("null")));
    }
}
