/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use crate::{
    config::TrustedProxies,
    server::{ConnectionSlot, SEND_QUEUE_DEPTH, SyncServer},
    types::{ClientMessage, ServerMessage},
};
use axum::{
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::sync::{Notify, mpsc};
use tracing::{debug, error, trace, warn};

const FORWARDED_HEADERS: &[&str] = &["cf-connecting-ip", "x-real-ip", "true-client-ip"];

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(server): State<SyncServer>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !server.config.allowed_origins.allows(origin) {
        warn!("rejected websocket from disallowed origin {origin:?}");
        return (StatusCode::FORBIDDEN, "Origin not allowed.").into_response();
    }

    let client_ip = client_ip(peer, &headers, &server.config.trusted_proxies);

    let Some(slot) = server.try_acquire_slot(client_ip) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Too many connections.").into_response();
    };

    ws.max_message_size(server.config.max_message_bytes)
        .max_frame_size(server.config.max_message_bytes)
        .on_upgrade(move |socket| handle_socket(socket, server, slot))
}

fn client_ip(peer: SocketAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> IpAddr {
    let peer_ip = peer.ip();
    if trusted.is_empty() || !trusted.contains(&peer_ip) {
        return peer_ip;
    }

    if let Some(value) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(ip) = value
            .rsplit(',')
            .filter_map(|part| part.trim().parse::<IpAddr>().ok())
            .find(|ip| !trusted.contains(ip))
    {
        return ip;
    }

    FORWARDED_HEADERS
        .iter()
        .filter_map(|name| headers.get(*name)?.to_str().ok()?.trim().parse().ok())
        .next()
        .unwrap_or(peer_ip)
}

async fn handle_socket(socket: WebSocket, server: SyncServer, slot: ConnectionSlot) {
    let client_ip = slot.ip();
    let conn_id = server.next_connection_id();
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(SEND_QUEUE_DEPTH);
    let shutdown = Arc::new(Notify::new());

    server.add_connection(conn_id, tx, shutdown.clone());

    let (mut sender, mut receiver) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(text) => {
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(e) => error!("failed to serialize server message: {e}"),
            }
        }
        let _ = sender.close().await;
    });

    let idle_timeout = server.config.idle_timeout;
    let max_bytes = server.config.max_message_bytes;

    loop {
        let incoming = tokio::select! {
            biased;
            _ = shutdown.notified() => break,
            frame = tokio::time::timeout(idle_timeout, receiver.next()) => frame,
        };

        let text = match incoming {
            Err(_) => {
                debug!("connection {conn_id} idle for {idle_timeout:?}; closing");
                break;
            }
            Ok(None) | Ok(Some(Ok(Message::Close(_)))) => break,
            Ok(Some(Err(e))) => {
                trace!("connection {conn_id} read error: {e}");
                break;
            }
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(_))) => continue,
        };

        if text.len() > max_bytes {
            server.send_error(conn_id, "Message too large.");
            continue;
        }
        if server.check_rate_limit(&client_ip).is_err() {
            server.send_error(conn_id, "Rate limit exceeded.");
            continue;
        }

        match serde_json::from_str::<ClientMessage>(&text) {
            Ok(msg) => {
                if let Err(e) = server.handle_message(conn_id, msg) {
                    server.send_error(conn_id, &e.to_string());
                }
            }
            Err(e) => {
                trace!("connection {conn_id} sent unparseable message: {e}");
                server.send_error(conn_id, "Invalid message format.");
            }
        }
    }

    send_task.abort();
    server.remove_connection(conn_id);
    debug!("websocket {conn_id} disconnected");
}
