/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use anyhow::Result;
use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tracing::info;

mod config;
mod error;
mod names;
mod server;
mod types;
mod ws;

use crate::config::Config;
use crate::server::SyncServer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("frametogether=debug,tower_http=debug")
        .init();

    let config = Config::load()?;
    info!("Starting FrameTogether server on {}", config.addr);

    let server = SyncServer::new(config.clone());

    let app = Router::new()
        .route("/sync", get(ws::websocket_handler))
        .route("/health", get(health))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(server);

    let listener = TcpListener::bind(&config.addr).await?;
    info!("FrameTogether server listening on {}", config.addr);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "OK"
}
