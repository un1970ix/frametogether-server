/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use anyhow::{Context, Result, bail};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::Duration,
};

const EXTENSION_SCHEMES: &[&str] = &[
    "chrome-extension://",
    "moz-extension://",
    "safari-web-extension://",
];

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub max_rooms: usize,
    pub rate_limit_per_second: u32,
    pub max_connections: usize,
    pub max_connections_per_ip: usize,
    pub max_participants_per_room: usize,
    pub room_id_length: usize,
    pub idle_timeout: Duration,
    pub max_message_bytes: usize,
    pub trusted_proxies: TrustedProxies,
    pub allowed_origins: AllowedOrigins,
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(raw) => raw
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid {key}: {e}")),
        Err(_) => Ok(default),
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();

        let addr = std::env::var("ADDR")
            .or_else(|_| std::env::var("PORT").map(|p| format!("0.0.0.0:{}", p.trim())))
            .unwrap_or_else(|_| "0.0.0.0:47642".to_string())
            .parse()
            .context("invalid ADDR/PORT")?;

        let cfg = Config {
            addr,
            max_rooms: env_parse("MAX_ROOMS", 1000)?,
            rate_limit_per_second: env_parse("RATE_LIMIT_PER_SECOND", 30)?,
            max_connections: env_parse("MAX_CONNECTIONS", 10_000)?,
            max_connections_per_ip: env_parse("MAX_CONNECTIONS_PER_IP", 20)?,
            max_participants_per_room: env_parse("MAX_PARTICIPANTS_PER_ROOM", 32)?,
            room_id_length: env_parse("ROOM_ID_LENGTH", 10)?,
            idle_timeout: Duration::from_secs(env_parse("IDLE_TIMEOUT_SECONDS", 90)?),
            max_message_bytes: env_parse("MAX_MESSAGE_BYTES", 8 * 1024)?,
            trusted_proxies: TrustedProxies::parse(
                &std::env::var("TRUSTED_PROXIES").unwrap_or_default(),
            )?,
            allowed_origins: AllowedOrigins::parse(
                &std::env::var("ALLOWED_ORIGINS").unwrap_or_default(),
            )?,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let limits = [
            ("RATE_LIMIT_PER_SECOND", self.rate_limit_per_second as usize),
            ("MAX_ROOMS", self.max_rooms),
            ("MAX_CONNECTIONS", self.max_connections),
            ("MAX_CONNECTIONS_PER_IP", self.max_connections_per_ip),
            ("MAX_PARTICIPANTS_PER_ROOM", self.max_participants_per_room),
        ];
        for (name, value) in limits {
            if value == 0 {
                bail!("{name} must be >= 1");
            }
        }
        if self.room_id_length < 6 {
            bail!("ROOM_ID_LENGTH must be >= 6");
        }
        if self.max_message_bytes < 64 {
            bail!("MAX_MESSAGE_BYTES must be >= 64");
        }
        if self.idle_timeout.as_secs() < 5 {
            bail!("IDLE_TIMEOUT_SECONDS must be >= 5");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<(IpAddr, u8)>,
}

impl TrustedProxies {
    pub fn parse(raw: &str) -> Result<Self> {
        let nets = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_cidr)
            .collect::<Result<_>>()?;
        Ok(Self { nets })
    }

    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    pub fn contains(&self, addr: &IpAddr) -> bool {
        self.nets
            .iter()
            .any(|(net, prefix)| prefix_match(net, addr, *prefix))
    }
}

fn parse_cidr(entry: &str) -> Result<(IpAddr, u8)> {
    let (ip_part, prefix_part) = match entry.split_once('/') {
        Some((ip, prefix)) => (ip, Some(prefix)),
        None => (entry, None),
    };
    let ip: IpAddr = ip_part
        .parse()
        .with_context(|| format!("invalid TRUSTED_PROXIES entry: {entry}"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix_part {
        Some(p) => p
            .parse()
            .with_context(|| format!("invalid prefix in TRUSTED_PROXIES: {entry}"))?,
        None => max,
    };
    if prefix > max {
        bail!("prefix /{prefix} too large for {ip_part}");
    }
    Ok((ip, prefix))
}

fn prefix_match(net: &IpAddr, addr: &IpAddr, prefix: u8) -> bool {
    match (net, addr) {
        (IpAddr::V4(a), IpAddr::V4(b)) => bits_match(&a.octets(), &b.octets(), prefix),
        (IpAddr::V6(a), IpAddr::V6(b)) => bits_match(&a.octets(), &b.octets(), prefix),
        _ => false,
    }
}

fn bits_match(a: &[u8], b: &[u8], prefix: u8) -> bool {
    let whole = usize::from(prefix / 8);
    if a[..whole] != b[..whole] {
        return false;
    }
    let rest = prefix % 8;
    rest == 0 || (a[whole] ^ b[whole]) >> (8 - rest) == 0
}

#[derive(Debug, Clone)]
pub struct AllowedOrigins {
    exact: Vec<String>,
    allow_any: bool,
}

impl AllowedOrigins {
    pub fn parse(raw: &str) -> Result<Self> {
        let mut exact = Vec::new();
        let mut allow_any = false;
        for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match entry {
                "*" => allow_any = true,
                _ => exact.push(normalize_origin(entry)),
            }
        }
        Ok(Self { exact, allow_any })
    }

    pub fn allows_any(&self) -> bool {
        self.allow_any
    }

    pub fn allows(&self, origin: Option<&str>) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        if self.allow_any {
            return true;
        }
        let origin = normalize_origin(origin);
        EXTENSION_SCHEMES.iter().any(|s| origin.starts_with(s)) || self.exact.contains(&origin)
    }
}

fn normalize_origin(origin: &str) -> String {
    origin.trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn unconfigured_trusted_proxies_trusts_nothing() {
        let t = TrustedProxies::parse("").unwrap();
        assert!(t.is_empty());
        assert!(!t.contains(&ip("127.0.0.1")));
    }

    #[test]
    fn trusted_proxies_match_exact_and_cidr() {
        let t = TrustedProxies::parse("127.0.0.1, 10.0.0.0/8, 172.16.0.0/12").unwrap();
        for good in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.254"] {
            assert!(t.contains(&ip(good)), "{good} should match");
        }
        for bad in ["172.32.0.1", "11.0.0.1", "192.168.1.1", "::1"] {
            assert!(!t.contains(&ip(bad)), "{bad} should not match");
        }
    }

    #[test]
    fn trusted_proxies_support_ipv6() {
        let t = TrustedProxies::parse("::1, fd00::/8").unwrap();
        assert!(t.contains(&ip("::1")));
        assert!(t.contains(&ip("fd00::1234")));
        assert!(!t.contains(&ip("fe80::1")));
        assert!(!t.contains(&ip("127.0.0.1")));
    }

    #[test]
    fn malformed_trusted_proxies_are_rejected() {
        assert!(TrustedProxies::parse("not-an-ip").is_err());
        assert!(TrustedProxies::parse("10.0.0.0/99").is_err());
    }

    #[test]
    fn origins_default_to_extensions_only() {
        let o = AllowedOrigins::parse("").unwrap();
        assert!(o.allows(None));
        assert!(o.allows(Some("chrome-extension://abcdef")));
        assert!(o.allows(Some("moz-extension://abcdef")));
        assert!(!o.allows(Some("https://evil.example")));
        assert!(!o.allows(Some("https://mubi.com")));
    }

    #[test]
    fn origins_allowlist_is_honoured_and_case_insensitive() {
        let o = AllowedOrigins::parse("https://watch.example.com/").unwrap();
        assert!(o.allows(Some("https://watch.example.com")));
        assert!(o.allows(Some("https://WATCH.example.com/")));
        assert!(!o.allows(Some("https://watch.example.com.evil.net")));
        assert!(!o.allows(Some("http://watch.example.com")));
    }

    #[test]
    fn wildcard_origin_allows_everything() {
        let o = AllowedOrigins::parse("*").unwrap();
        assert!(o.allows_any());
        assert!(o.allows(Some("https://evil.example")));
    }

    fn base() -> Config {
        Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            max_rooms: 10,
            rate_limit_per_second: 30,
            max_connections: 10,
            max_connections_per_ip: 2,
            max_participants_per_room: 4,
            room_id_length: 10,
            idle_timeout: Duration::from_secs(90),
            max_message_bytes: 8192,
            trusted_proxies: TrustedProxies::parse("").unwrap(),
            allowed_origins: AllowedOrigins::parse("").unwrap(),
        }
    }

    #[test]
    fn zero_rate_limit_is_rejected_instead_of_panicking_at_runtime() {
        let mut c = base();
        c.rate_limit_per_second = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn short_room_ids_are_rejected() {
        let mut c = base();
        c.room_id_length = 4;
        assert!(c.validate().is_err());
        c.room_id_length = 6;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn zero_limits_are_rejected() {
        let mutations: [fn(&mut Config); 4] = [
            |c| c.max_rooms = 0,
            |c| c.max_connections = 0,
            |c| c.max_connections_per_ip = 0,
            |c| c.max_participants_per_room = 0,
        ];
        for mutate in mutations {
            let mut c = base();
            mutate(&mut c);
            assert!(c.validate().is_err());
        }
    }
}
