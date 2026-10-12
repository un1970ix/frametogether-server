/**
 * Copyright (C) 2025 Bedir Ekim
 * This file is part of FrameTogether Server.
 * FrameTogether Server is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
 * FrameTogether Server is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details.
 * You should have received a copy of the GNU Affero General Public License along with FrameTogether Server. If not, see <https://www.gnu.org/licenses/>.
**/
use crate::{
    config::Config,
    error::AppError,
    names::{generate_room_id, generate_unique_name},
    types::{
        ClientMessage, Connection, ConnectionId, MAX_PLAYBACK_SECONDS, MAX_ROOM_ID_LEN,
        Participant, Room, RoomId, ServerMessage, VideoState,
    },
};
use dashmap::{DashMap, mapref::entry::Entry};
use governor::{RateLimiter, clock::DefaultClock, state::keyed::DefaultKeyedStateStore};
use std::{
    net::IpAddr,
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

type IpRateLimiter = RateLimiter<IpAddr, DefaultKeyedStateStore<IpAddr>, DefaultClock>;

pub const SEND_QUEUE_DEPTH: usize = 64;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const ROOM_ID_ATTEMPTS: usize = 8;

pub struct ConnectionSlot {
    server: SyncServer,
    ip: IpAddr,
}

impl ConnectionSlot {
    pub fn ip(&self) -> IpAddr {
        self.ip
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.server.release_slot(self.ip);
    }
}

#[derive(Clone)]
pub struct SyncServer {
    pub config: Config,
    rooms: Arc<DashMap<RoomId, Room>>,
    connections: Arc<DashMap<ConnectionId, Connection>>,
    rate_limiter: Arc<IpRateLimiter>,
    ip_connections: Arc<DashMap<IpAddr, usize>>,
    connection_count: Arc<AtomicUsize>,
    next_connection_id: Arc<AtomicU64>,
}

impl SyncServer {
    pub fn new(config: Config) -> Self {
        let quota = governor::Quota::per_second(
            NonZeroU32::new(config.rate_limit_per_second).unwrap_or(NonZeroU32::MIN),
        );

        let server = Self {
            config,
            rooms: Arc::new(DashMap::new()),
            connections: Arc::new(DashMap::new()),
            rate_limiter: Arc::new(RateLimiter::keyed(quota)),
            ip_connections: Arc::new(DashMap::new()),
            connection_count: Arc::new(AtomicUsize::new(0)),
            next_connection_id: Arc::new(AtomicU64::new(1)),
        };

        let janitor = server.clone();
        tokio::spawn(async move {
            let start = tokio::time::Instant::now() + CLEANUP_INTERVAL;
            let mut interval = tokio::time::interval_at(start, CLEANUP_INTERVAL);
            loop {
                interval.tick().await;
                janitor.cleanup();
            }
        });

        server
    }

    fn peer(&self, id: ConnectionId) -> Option<(mpsc::Sender<ServerMessage>, Arc<Notify>)> {
        self.connections
            .get(&id)
            .map(|c| (c.tx.clone(), c.shutdown.clone()))
    }

    fn send_to(&self, id: ConnectionId, msg: ServerMessage) {
        let Some((tx, shutdown)) = self.peer(id) else {
            return;
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(msg) {
            warn!("connection {id} is not draining its queue; disconnecting");
            shutdown.notify_one();
        }
    }

    pub fn send_error(&self, id: ConnectionId, message: &str) {
        self.send_to(
            id,
            ServerMessage::Error {
                message: message.to_string(),
            },
        );
    }

    fn members(&self, room_id: &str) -> Vec<ConnectionId> {
        self.rooms
            .get(room_id)
            .map(|r| r.connections.clone())
            .unwrap_or_default()
    }

    fn broadcast(&self, room_id: &str, msg: ServerMessage, exclude: Option<ConnectionId>) {
        for id in self.members(room_id) {
            if Some(id) != exclude {
                self.send_to(id, msg.clone());
            }
        }
    }

    fn name_of(&self, id: ConnectionId) -> Option<String> {
        self.connections.get(&id).and_then(|c| c.name.clone())
    }

    fn room_of(&self, id: ConnectionId) -> Option<RoomId> {
        self.connections.get(&id).and_then(|c| c.room_id.clone())
    }

    pub fn next_connection_id(&self) -> ConnectionId {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn try_acquire_slot(&self, ip: IpAddr) -> Option<ConnectionSlot> {
        let accepted = self
            .connection_count
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < self.config.max_connections).then_some(n + 1)
            });
        if accepted.is_err() {
            warn!(
                "global connection limit ({}) reached",
                self.config.max_connections
            );
            return None;
        }

        let mut entry = self.ip_connections.entry(ip).or_insert(0);
        if *entry >= self.config.max_connections_per_ip {
            drop(entry);
            self.connection_count.fetch_sub(1, Ordering::SeqCst);
            warn!("per-IP connection limit reached");
            return None;
        }
        *entry += 1;
        drop(entry);

        Some(ConnectionSlot {
            server: self.clone(),
            ip,
        })
    }

    fn release_slot(&self, ip: IpAddr) {
        self.connection_count.fetch_sub(1, Ordering::SeqCst);
        if let Some(mut entry) = self.ip_connections.get_mut(&ip) {
            *entry = entry.saturating_sub(1);
        }
        self.ip_connections.remove_if(&ip, |_, &n| n == 0);
    }

    pub fn add_connection(
        &self,
        id: ConnectionId,
        tx: mpsc::Sender<ServerMessage>,
        shutdown: Arc<Notify>,
    ) {
        self.connections.insert(
            id,
            Connection {
                tx,
                room_id: None,
                name: None,
                shutdown,
            },
        );
        debug!("connection {id} added");
    }

    pub fn remove_connection(&self, id: ConnectionId) {
        self.leave_current_room(id);
        if self.connections.remove(&id).is_some() {
            debug!("connection {id} removed");
        }
    }

    pub fn check_rate_limit(&self, ip: &IpAddr) -> Result<(), AppError> {
        self.rate_limiter
            .check_key(ip)
            .map_err(|_| AppError::RateLimitExceeded)
    }

    pub fn handle_message(
        &self,
        conn_id: ConnectionId,
        msg: ClientMessage,
    ) -> Result<(), AppError> {
        match msg {
            ClientMessage::Create {} => self.create_room(conn_id),
            ClientMessage::Join { room_id } => self.join_room(conn_id, room_id),
            ClientMessage::Leave {} => {
                self.leave_current_room(conn_id);
                Ok(())
            }
            ClientMessage::State { time, paused } => self.update_state(conn_id, time, paused),
            ClientMessage::Heartbeat {} => Ok(()),
        }
    }

    fn create_room(&self, conn_id: ConnectionId) -> Result<(), AppError> {
        self.leave_current_room(conn_id);

        if self.rooms.len() >= self.config.max_rooms {
            return Err(AppError::TooManyRooms);
        }

        let (room_id, name) = self.insert_room(conn_id).ok_or(AppError::TooManyRooms)?;

        if !self.attach(conn_id, &room_id, &name) {
            self.rooms.remove(&room_id);
            return Err(AppError::NotAuthorized);
        }

        self.send_to(
            conn_id,
            ServerMessage::RoomCreated {
                room_id: room_id.clone(),
            },
        );
        self.send_to(
            conn_id,
            ServerMessage::RoomJoined {
                room_id: room_id.clone(),
                is_host: true,
                your_name: name,
                participants: Vec::new(),
            },
        );

        info!("room {room_id} created");
        Ok(())
    }

    fn insert_room(&self, host: ConnectionId) -> Option<(RoomId, String)> {
        for _ in 0..ROOM_ID_ATTEMPTS {
            let candidate = generate_room_id(self.config.room_id_length);
            if let Entry::Vacant(slot) = self.rooms.entry(candidate.clone()) {
                let mut room = Room::new(host);
                let name = generate_unique_name(&room.used_names);
                room.used_names.insert(name.clone());
                slot.insert(room);
                return Some((candidate, name));
            }
        }
        None
    }

    fn attach(&self, conn_id: ConnectionId, room_id: &str, name: &str) -> bool {
        match self.connections.get_mut(&conn_id) {
            Some(mut conn) => {
                conn.room_id = Some(room_id.to_string());
                conn.name = Some(name.to_string());
                true
            }
            None => false,
        }
    }

    fn join_room(&self, conn_id: ConnectionId, room_id: RoomId) -> Result<(), AppError> {
        if room_id.is_empty() || room_id.len() > MAX_ROOM_ID_LEN {
            return Err(AppError::InvalidRequest);
        }
        if self.room_of(conn_id).as_deref() == Some(room_id.as_str()) {
            return Ok(());
        }
        self.leave_current_room(conn_id);

        let (name, is_host, state, host_id, members) = {
            let Some(mut room) = self.rooms.get_mut(&room_id) else {
                return Err(AppError::RoomNotFound);
            };
            if room.connections.len() >= self.config.max_participants_per_room {
                return Err(AppError::RoomFull);
            }
            if !room.connections.contains(&conn_id) {
                room.connections.push(conn_id);
            }
            let name = generate_unique_name(&room.used_names);
            room.used_names.insert(name.clone());
            (
                name,
                room.host == conn_id,
                room.state,
                room.host,
                room.connections.clone(),
            )
        };

        let participants = members
            .into_iter()
            .filter(|&id| id != conn_id)
            .filter_map(|id| {
                self.name_of(id).map(|name| Participant {
                    name,
                    is_host: id == host_id,
                })
            })
            .collect();

        if !self.attach(conn_id, &room_id, &name) {
            return Err(AppError::NotAuthorized);
        }

        self.send_to(
            conn_id,
            ServerMessage::RoomJoined {
                room_id: room_id.clone(),
                is_host,
                your_name: name.clone(),
                participants,
            },
        );
        self.send_to(
            conn_id,
            ServerMessage::Sync {
                time: state.time,
                paused: state.paused,
            },
        );
        self.broadcast(
            &room_id,
            ServerMessage::UserJoined {
                user: Participant { name, is_host },
            },
            Some(conn_id),
        );

        info!("connection joined room {room_id}");
        Ok(())
    }

    fn detach(&self, conn_id: ConnectionId) -> Option<(RoomId, String)> {
        let mut conn = self.connections.get_mut(&conn_id)?;
        let room_id = conn.room_id.take()?;
        Some((room_id, conn.name.take().unwrap_or_default()))
    }

    fn leave_current_room(&self, conn_id: ConnectionId) {
        let Some((room_id, name)) = self.detach(conn_id) else {
            return;
        };

        let mut new_host = None;
        {
            let Some(mut room) = self.rooms.get_mut(&room_id) else {
                return;
            };
            room.connections.retain(|&id| id != conn_id);
            room.used_names.remove(&name);
            if room.host == conn_id
                && let Some(&next) = room.connections.first()
            {
                room.host = next;
                new_host = Some(next);
            }
        }

        if self
            .rooms
            .remove_if(&room_id, |_, r| r.connections.is_empty())
            .is_some()
        {
            info!("room {room_id} removed (empty)");
            return;
        }

        if !name.is_empty() {
            self.broadcast(
                &room_id,
                ServerMessage::UserLeft { user_name: name },
                Some(conn_id),
            );
        }
        if let Some(host_name) = new_host.and_then(|id| self.name_of(id)) {
            self.broadcast(
                &room_id,
                ServerMessage::HostChanged {
                    user_name: host_name,
                },
                None,
            );
        }
    }

    fn update_state(&self, conn_id: ConnectionId, time: f64, paused: bool) -> Result<(), AppError> {
        if !time.is_finite() || !(0.0..=MAX_PLAYBACK_SECONDS).contains(&time) {
            return Err(AppError::InvalidRequest);
        }
        let room_id = self.room_of(conn_id).ok_or(AppError::NotAuthorized)?;

        {
            let Some(mut room) = self.rooms.get_mut(&room_id) else {
                return Err(AppError::RoomNotFound);
            };
            if room.host != conn_id {
                return Err(AppError::NotAuthorized);
            }
            room.state = VideoState { time, paused };
        }

        self.broadcast(
            &room_id,
            ServerMessage::Sync { time, paused },
            Some(conn_id),
        );
        Ok(())
    }

    fn stale_members(&self, room_id: &str) -> Vec<ConnectionId> {
        let members = self.members(room_id);
        members
            .into_iter()
            .filter(|id| !self.connections.contains_key(id))
            .collect()
    }

    fn cleanup(&self) {
        let room_ids: Vec<RoomId> = self.rooms.iter().map(|r| r.key().clone()).collect();
        for room_id in room_ids {
            let stale = self.stale_members(&room_id);
            if !stale.is_empty() {
                warn!("room {room_id} referenced {} dead connections", stale.len());
                if let Some(mut room) = self.rooms.get_mut(&room_id) {
                    room.connections.retain(|id| !stale.contains(id));
                    if stale.contains(&room.host)
                        && let Some(&next) = room.connections.first()
                    {
                        room.host = next;
                    }
                }
            }
            self.rooms
                .remove_if(&room_id, |_, r| r.connections.is_empty());
        }

        self.rate_limiter.retain_recent();

        debug!(
            rooms = self.rooms.len(),
            connections = self.connection_count.load(Ordering::Relaxed),
            limiter_keys = self.rate_limiter.len(),
            "cleanup"
        );
    }

    #[cfg(test)]
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    #[cfg(test)]
    pub fn used_name_count(&self, room_id: &str) -> usize {
        self.rooms.get(room_id).map_or(0, |r| r.used_names.len())
    }

    #[cfg(test)]
    pub fn host_of(&self, room_id: &str) -> Option<ConnectionId> {
        self.rooms.get(room_id).map(|r| r.host)
    }

    #[cfg(test)]
    pub fn limiter_keys(&self) -> usize {
        self.rate_limiter.len()
    }

    #[cfg(test)]
    pub fn run_cleanup(&self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AllowedOrigins, TrustedProxies};
    use std::net::Ipv4Addr;

    fn config() -> Config {
        Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            max_rooms: 10,
            rate_limit_per_second: 1000,
            max_connections: 100,
            max_connections_per_ip: 100,
            max_participants_per_room: 4,
            room_id_length: 10,
            idle_timeout: Duration::from_secs(60),
            max_message_bytes: 8192,
            trusted_proxies: TrustedProxies::parse("").unwrap(),
            allowed_origins: AllowedOrigins::parse("").unwrap(),
        }
    }

    fn connect(s: &SyncServer) -> (ConnectionId, mpsc::Receiver<ServerMessage>) {
        let id = s.next_connection_id();
        let (tx, rx) = mpsc::channel(64);
        s.add_connection(id, tx, Arc::new(Notify::new()));
        (id, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leave_completes_and_frees_the_room() {
        let s = SyncServer::new(config());
        let (a, _rx) = connect(&s);
        s.handle_message(a, ClientMessage::Create {}).unwrap();
        assert_eq!(s.room_count(), 1);

        s.handle_message(a, ClientMessage::Leave {}).unwrap();
        assert_eq!(s.room_count(), 0);
        assert_eq!(s.room_of(a), None);

        s.handle_message(a, ClientMessage::Create {}).unwrap();
        assert_eq!(s.room_count(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn repeated_create_does_not_leak_rooms() {
        let s = SyncServer::new(config());
        let (a, _rx) = connect(&s);
        for _ in 0..20 {
            s.handle_message(a, ClientMessage::Create {}).unwrap();
        }
        assert_eq!(s.room_count(), 1);

        s.remove_connection(a);
        assert_eq!(s.room_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn joining_another_room_releases_the_previous_one() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();
        let first = s.room_of(host).unwrap();

        let (guest, _g) = connect(&s);
        s.handle_message(guest, ClientMessage::Create {}).unwrap();
        let second = s.room_of(guest).unwrap();

        s.handle_message(
            guest,
            ClientMessage::Join {
                room_id: first.clone(),
            },
        )
        .unwrap();
        assert_eq!(s.room_of(guest), Some(first));
        assert_eq!(s.room_count(), 1);
        assert!(s.host_of(&second).is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejoining_same_room_does_not_grow_name_table() {
        let s = SyncServer::new(config());
        let (a, _rx) = connect(&s);
        s.handle_message(a, ClientMessage::Create {}).unwrap();
        let room = s.room_of(a).unwrap();

        for _ in 0..500 {
            s.handle_message(
                a,
                ClientMessage::Join {
                    room_id: room.clone(),
                },
            )
            .unwrap();
        }
        assert_eq!(s.used_name_count(&room), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn names_are_returned_on_leave() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();
        let room = s.room_of(host).unwrap();

        for _ in 0..50 {
            let (guest, _g) = connect(&s);
            s.handle_message(
                guest,
                ClientMessage::Join {
                    room_id: room.clone(),
                },
            )
            .unwrap();
            s.remove_connection(guest);
        }
        assert_eq!(s.used_name_count(&room), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn room_capacity_is_enforced() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();
        let room = s.room_of(host).unwrap();

        for _ in 0..3 {
            let (g, _r) = connect(&s);
            s.handle_message(
                g,
                ClientMessage::Join {
                    room_id: room.clone(),
                },
            )
            .unwrap();
        }
        let (overflow, _o) = connect(&s);
        assert!(matches!(
            s.handle_message(overflow, ClientMessage::Join { room_id: room }),
            Err(AppError::RoomFull)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_the_host_may_drive_playback() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();
        let room = s.room_of(host).unwrap();
        let (guest, _g) = connect(&s);
        s.handle_message(guest, ClientMessage::Join { room_id: room })
            .unwrap();

        assert!(
            s.handle_message(
                host,
                ClientMessage::State {
                    time: 10.0,
                    paused: false
                }
            )
            .is_ok()
        );
        assert!(matches!(
            s.handle_message(
                guest,
                ClientMessage::State {
                    time: 99.0,
                    paused: true
                }
            ),
            Err(AppError::NotAuthorized)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nonsense_playback_times_are_rejected() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();

        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 1e18] {
            assert!(
                matches!(
                    s.handle_message(
                        host,
                        ClientMessage::State {
                            time: bad,
                            paused: false
                        }
                    ),
                    Err(AppError::InvalidRequest)
                ),
                "time {bad} should be rejected"
            );
        }
        assert!(
            s.handle_message(
                host,
                ClientMessage::State {
                    time: 120.5,
                    paused: false
                }
            )
            .is_ok()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_is_handed_over_when_the_host_leaves() {
        let s = SyncServer::new(config());
        let (host, _h) = connect(&s);
        s.handle_message(host, ClientMessage::Create {}).unwrap();
        let room = s.room_of(host).unwrap();
        let (guest, _g) = connect(&s);
        s.handle_message(
            guest,
            ClientMessage::Join {
                room_id: room.clone(),
            },
        )
        .unwrap();

        s.remove_connection(host);
        assert_eq!(s.host_of(&room), Some(guest));
        assert!(
            s.handle_message(
                guest,
                ClientMessage::State {
                    time: 5.0,
                    paused: false
                }
            )
            .is_ok()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_and_oversized_room_ids_are_rejected() {
        let s = SyncServer::new(config());
        let (a, _rx) = connect(&s);
        assert!(matches!(
            s.handle_message(
                a,
                ClientMessage::Join {
                    room_id: "nope12".into()
                }
            ),
            Err(AppError::RoomNotFound)
        ));
        assert!(matches!(
            s.handle_message(
                a,
                ClientMessage::Join {
                    room_id: "x".repeat(5000)
                }
            ),
            Err(AppError::InvalidRequest)
        ));
        assert!(matches!(
            s.handle_message(
                a,
                ClientMessage::Join {
                    room_id: String::new()
                }
            ),
            Err(AppError::InvalidRequest)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rate_limiter_state_is_evicted() {
        let mut cfg = config();
        cfg.rate_limit_per_second = 1000;
        let s = SyncServer::new(cfg);

        for i in 0..5000u32 {
            let _ = s.check_rate_limit(&IpAddr::V4(Ipv4Addr::from(i.to_be_bytes())));
        }
        assert_eq!(s.limiter_keys(), 5000);

        tokio::time::sleep(Duration::from_millis(200)).await;
        s.run_cleanup();
        assert_eq!(s.limiter_keys(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn per_ip_connection_slots_are_released() {
        let s = SyncServer::new(config());
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..50 {
            let slot = s.try_acquire_slot(ip).expect("slot available");
            let id = s.next_connection_id();
            let (tx, _rx) = mpsc::channel(8);
            s.add_connection(id, tx, Arc::new(Notify::new()));
            s.remove_connection(id);
            drop(slot);
        }
        assert_eq!(s.ip_connections.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connection_limits_are_enforced() {
        let mut cfg = config();
        cfg.max_connections = 3;
        cfg.max_connections_per_ip = 2;
        let s = SyncServer::new(cfg);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        let s1 = s.try_acquire_slot(a).expect("first");
        let _s2 = s.try_acquire_slot(a).expect("second");
        assert!(s.try_acquire_slot(a).is_none());
        let _s3 = s.try_acquire_slot(b).expect("other IP");
        assert!(s.try_acquire_slot(b).is_none());

        drop(s1);
        assert!(s.try_acquire_slot(a).is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abandoned_handshake_releases_its_slot() {
        let mut cfg = config();
        cfg.max_connections_per_ip = 2;
        let s = SyncServer::new(cfg);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));

        for _ in 0..100 {
            drop(s.try_acquire_slot(ip).expect("slot must be available"));
        }
        assert!(s.try_acquire_slot(ip).is_some());
        assert_eq!(s.ip_connections.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn max_rooms_is_enforced_but_recovers() {
        let mut cfg = config();
        cfg.max_rooms = 3;
        let s = SyncServer::new(cfg);

        let mut conns = Vec::new();
        for _ in 0..3 {
            let (c, rx) = connect(&s);
            s.handle_message(c, ClientMessage::Create {}).unwrap();
            conns.push((c, rx));
        }
        let (extra, _rx) = connect(&s);
        assert!(matches!(
            s.handle_message(extra, ClientMessage::Create {}),
            Err(AppError::TooManyRooms)
        ));

        s.remove_connection(conns.pop().unwrap().0);
        assert!(s.handle_message(extra, ClientMessage::Create {}).is_ok());
    }
}
