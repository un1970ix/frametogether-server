/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use anyhow::Result;
use axum::{Router, http::Method, routing::get};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod config;
mod error;
mod names;
mod server;
mod types;
mod ws;

use crate::config::Config;
use crate::server::SyncServer;

const DEFAULT_LOG_FILTER: &str = "frametogether_server=info,tower_http=warn";

#[tokio::main]
async fn main() -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config::load()?;

    if config.trusted_proxies.is_empty() {
        info!(
            "TRUSTED_PROXIES is empty; using the TCP peer address and ignoring forwarding headers"
        );
    }
    if config.allowed_origins.allows_any() {
        warn!("ALLOWED_ORIGINS=* allows any website to open a socket on a visitor's behalf");
    }

    let server = SyncServer::new(config.clone());

    let app = Router::new()
        .route("/sync", get(ws::websocket_handler))
        .route("/health", get(health))
        .layer(
            CorsLayer::new()
                .allow_methods([Method::GET])
                .allow_origin(Any),
        )
        .with_state(server);

    let listener = TcpListener::bind(&config.addr).await?;
    info!("FrameTogether server listening on {}", config.addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("shutdown complete");
    Ok(())
}

async fn health() -> &'static str {
    "OK"
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("shutdown signal received");
}
