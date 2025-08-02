use anyhow::Result;
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_addr")]
    pub addr: SocketAddr,

    #[serde(default = "default_max_rooms")]
    pub max_rooms: usize,

    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_second: u32,
}

impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Config {
            addr: std::env::var("ADDR")
                .or_else(|_| std::env::var("PORT").map(|p| format!("0.0.0.0:{p}")))
                .unwrap_or_else(|_| "0.0.0.0:47642".to_string())
                .parse()?,
            max_rooms: std::env::var("MAX_ROOMS")
                .unwrap_or_else(|_| "1000".to_string())
                .parse()?,
            rate_limit_per_second: std::env::var("RATE_LIMIT_PER_SECOND")
                .unwrap_or_else(|_| "100".to_string())
                .parse()?,
        })
    }
}

fn default_addr() -> SocketAddr {
    "0.0.0.0:47642".parse().unwrap()
}

fn default_max_rooms() -> usize {
    1000
}

fn default_rate_limit() -> u32 {
    100
}
