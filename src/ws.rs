/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use crate::{
    server::SyncServer,
    types::{ClientMessage, ServerMessage},
};
use axum::{
    extract::{State, ws::WebSocketUpgrade},
    http::{HeaderMap, header},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::net::IpAddr;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(server): State<SyncServer>,
    headers: HeaderMap,
) -> Response {
    let client_ip = get_real_ip(&headers);
    ws.on_upgrade(move |socket| handle_socket(socket, server, client_ip))
}

fn get_real_ip(headers: &HeaderMap) -> IpAddr {
    let header_names = [
        "cf-connecting-ip",
        "x-real-ip",
        "x-forwarded-for",
        "x-client-ip",
        "true-client-ip",
        "forwarded",
    ];

    for header_name in &header_names {
        if let Some(header_value) = headers.get(*header_name) {
            if let Ok(header_str) = header_value.to_str() {
                let ip_str = if header_name == &"x-forwarded-for" {
                    header_str.split(',').next().unwrap_or(header_str).trim()
                } else if header_name == &"forwarded" {
                    parse_forwarded_for(header_str).unwrap_or(header_str)
                } else {
                    header_str.trim()
                };

                if let Ok(ip) = ip_str.parse::<IpAddr>() {
                    debug!("Got real IP {} from header {}", ip, header_name);
                    return ip;
                }
            }
        }
    }

    if let Some(host_header) = headers.get(header::HOST) {
        if let Ok(host) = host_header.to_str() {
            if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
                debug!("Local connection detected");
                return "127.0.0.1".parse().unwrap();
            }
        }
    }

    debug!("No proxy headers found, using default IP");
    "127.0.0.1".parse().unwrap()
}

fn parse_forwarded_for(forwarded: &str) -> Option<&str> {
    for part in forwarded.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("for=") {
            let value = value.trim_matches('"');
            if let Some(ip) = value.split(':').next() {
                return Some(ip);
            }
            return Some(value);
        }
    }
    None
}

async fn handle_socket(
    socket: axum::extract::ws::WebSocket,
    server: SyncServer,
    client_ip: IpAddr,
) {
    let conn_id = server.next_connection_id();
    let (tx, mut rx) = mpsc::unbounded_channel();

    server.add_connection(conn_id, client_ip, tx);

    let (mut sender, mut receiver) = socket.split();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if sender
                    .send(axum::extract::ws::Message::Text(text.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    });

    while let Some(msg) = receiver.next().await {
        if let Ok(msg) = msg {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    if server.check_rate_limit(&client_ip).is_err() {
                        if let Some(conn) = server.connections.get(&conn_id) {
                            let _ = conn.tx.send(ServerMessage::Error {
                                message: "Rate limit exceeded".to_string(),
                            });
                        }
                        continue;
                    }

                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(client_msg) => {
                            if let Err(e) = server.handle_message(conn_id, client_msg).await {
                                if let Some(conn) = server.connections.get(&conn_id) {
                                    let _ = conn.tx.send(ServerMessage::Error {
                                        message: e.to_string(),
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to parse message: {}", e);
                            if let Some(conn) = server.connections.get(&conn_id) {
                                let _ = conn.tx.send(ServerMessage::Error {
                                    message: "Invalid message format".to_string(),
                                });
                            }
                        }
                    }
                }
                axum::extract::ws::Message::Close(_) => break,
                _ => {}
            }
        } else {
            break;
        }
    }

    send_task.abort();
    server.remove_connection(conn_id);
    info!("WebSocket {} disconnected", conn_id);
}
