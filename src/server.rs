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
    names::generate_unique_name,
    types::{
        ClientMessage, Connection, ConnectionId, Participant, Room, RoomId, ServerMessage,
        VideoState,
    },
};
use dashmap::DashMap;
use governor::{Quota, RateLimiter};
use nanoid::nanoid;
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

type IpRateLimiter = Arc<
    RateLimiter<
        governor::state::direct::NotKeyed,
        governor::state::InMemoryState,
        governor::clock::DefaultClock,
    >,
>;

#[derive(Clone)]
pub struct SyncServer {
    pub config: Config,
    pub rooms: Arc<DashMap<RoomId, Room>>,
    pub connections: Arc<DashMap<ConnectionId, Connection>>,
    pub rate_limiters: Arc<DashMap<IpAddr, IpRateLimiter>>,
    next_connection_id: Arc<std::sync::atomic::AtomicU64>,
}

impl SyncServer {
    pub fn new(config: Config) -> Self {
        let server = Self {
            config,
            rooms: Arc::new(DashMap::new()),
            connections: Arc::new(DashMap::new()),
            rate_limiters: Arc::new(DashMap::new()),
            next_connection_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        };

        let cleanup_server = server.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                cleanup_server.cleanup_empty_rooms();
            }
        });

        server
    }

    pub fn next_connection_id(&self) -> ConnectionId {
        self.next_connection_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn add_connection(
        &self,
        id: ConnectionId,
        ip: IpAddr,
        tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        self.connections.insert(
            id,
            Connection {
                tx,
                room_id: None,
                name: None,
            },
        );
        info!("Connection {} added from {}", id, ip);
    }

    pub fn remove_connection(&self, id: ConnectionId) {
        if let Some((_, conn)) = self.connections.remove(&id) {
            if let Some(room_id) = conn.room_id {
                self.leave_room(id, &room_id);
            }
            info!("Connection {} removed", id);
        }
    }

    pub fn check_rate_limit(&self, ip: &IpAddr) -> Result<(), AppError> {
        let limiter = self.rate_limiters.entry(*ip).or_insert_with(|| {
            Arc::new(RateLimiter::direct(Quota::per_second(
                self.config.rate_limit_per_second.try_into().unwrap(),
            )))
        });

        match limiter.check() {
            Ok(_) => Ok(()),
            Err(_) => {
                warn!("Rate limit exceeded for {}", ip);
                Err(AppError::RateLimitExceeded)
            }
        }
    }

    pub async fn handle_message(
        &self,
        conn_id: ConnectionId,
        msg: ClientMessage,
    ) -> Result<(), AppError> {
        match msg {
            ClientMessage::Create => self.create_room(conn_id).await,
            ClientMessage::Join { room_id } => self.join_room(conn_id, room_id).await,
            ClientMessage::Leave => self.handle_leave(conn_id).await,
            ClientMessage::State { time, paused } => self.update_state(conn_id, time, paused).await,
            ClientMessage::Heartbeat => Ok(()),
        }
    }

    async fn create_room(&self, conn_id: ConnectionId) -> Result<(), AppError> {
        if self.rooms.len() >= self.config.max_rooms {
            return Err(AppError::TooManyRooms);
        }

        let room_id = nanoid!(6);
        let mut room = Room::new(room_id.clone(), conn_id);

        let name = generate_unique_name(&room.used_names);
        room.used_names.push(name.clone());

        self.rooms.insert(room_id.clone(), room);

        if let Some(mut conn) = self.connections.get_mut(&conn_id) {
            conn.room_id = Some(room_id.clone());
            conn.name = Some(name.clone());
            let _ = conn.tx.send(ServerMessage::RoomCreated {
                room_id: room_id.clone(),
            });
            let _ = conn.tx.send(ServerMessage::RoomJoined {
                room_id: room_id.clone(),
                is_host: true,
                your_name: name,
                participants: vec![],
            });
        }

        info!("Room {} created by connection {}", room_id, conn_id);
        Ok(())
    }

    async fn join_room(&self, conn_id: ConnectionId, room_id: RoomId) -> Result<(), AppError> {
        let mut room = self.rooms.get_mut(&room_id).ok_or(AppError::RoomNotFound)?;

        if !room.connections.contains(&conn_id) {
            room.connections.push(conn_id);
        }

        let name = generate_unique_name(&room.used_names);
        room.used_names.push(name.clone());

        let is_host = room.host == conn_id;
        let current_state = room.state.clone();

        let participants: Vec<Participant> = room
            .connections
            .iter()
            .filter_map(|&id| {
                if id != conn_id {
                    self.connections.get(&id).and_then(|conn| {
                        conn.name.as_ref().map(|n| Participant {
                            name: n.clone(),
                            is_host: room.host == id,
                        })
                    })
                } else {
                    None
                }
            })
            .collect();

        drop(room);

        if let Some(mut conn) = self.connections.get_mut(&conn_id) {
            conn.room_id = Some(room_id.clone());
            conn.name = Some(name.clone());
            let _ = conn.tx.send(ServerMessage::RoomJoined {
                room_id: room_id.clone(),
                is_host,
                your_name: name.clone(),
                participants,
            });
            let _ = conn.tx.send(ServerMessage::Sync {
                time: current_state.time,
                paused: current_state.paused,
            });
        }

        let new_user = Participant { name, is_host };
        self.broadcast_to_room(
            &room_id,
            ServerMessage::UserJoined { user: new_user },
            Some(conn_id),
        )
        .await;

        info!("Connection {} joined room {}", conn_id, room_id);
        Ok(())
    }

    async fn handle_leave(&self, conn_id: ConnectionId) -> Result<(), AppError> {
        if let Some(conn) = self.connections.get(&conn_id) {
            if let Some(room_id) = &conn.room_id {
                self.leave_room(conn_id, room_id);
            }
        }
        Ok(())
    }

    fn leave_room(&self, conn_id: ConnectionId, room_id: &str) {
        let user_name = self
            .connections
            .get(&conn_id)
            .and_then(|conn| conn.name.clone())
            .unwrap_or_default();

        if let Some(mut room) = self.rooms.get_mut(room_id) {
            room.connections.retain(|&id| id != conn_id);

            if room.connections.is_empty() {
                drop(room);
                self.rooms.remove(room_id);
                info!("Room {} removed (empty)", room_id);
            } else if room.host == conn_id && !room.connections.is_empty() {
                room.host = room.connections[0];
                let new_host_id = room.connections[0];
                drop(room);

                if let Some(new_host) = self.connections.get(&new_host_id) {
                    if let Some(new_host_name) = &new_host.name {
                        let _ = new_host.tx.send(ServerMessage::UserJoined {
                            user: Participant {
                                name: new_host_name.clone(),
                                is_host: true,
                            },
                        });
                    }
                }
            } else {
                drop(room);
            }

            let leave_msg = ServerMessage::UserLeft { user_name };
            let room_id_owned = room_id.to_string();
            let self_clone = self.clone();
            tokio::spawn(async move {
                self_clone
                    .broadcast_to_room(&room_id_owned, leave_msg, Some(conn_id))
                    .await;
            });
        }

        if let Some(mut conn) = self.connections.get_mut(&conn_id) {
            conn.room_id = None;
            conn.name = None;
        }
    }

    async fn update_state(
        &self,
        conn_id: ConnectionId,
        time: f64,
        paused: bool,
    ) -> Result<(), AppError> {
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or(AppError::NotAuthorized)?;
        let room_id = conn.room_id.as_ref().ok_or(AppError::NotAuthorized)?;

        let mut room = self.rooms.get_mut(room_id).ok_or(AppError::RoomNotFound)?;

        if room.host != conn_id {
            return Err(AppError::NotAuthorized);
        }

        room.state = VideoState { time, paused };

        let room_id = room_id.clone();
        drop(room);

        self.broadcast_to_room(
            &room_id,
            ServerMessage::Sync { time, paused },
            Some(conn_id),
        )
        .await;

        debug!(
            "State updated in room {}: time={}, paused={}",
            room_id, time, paused
        );
        Ok(())
    }

    async fn broadcast_to_room(
        &self,
        room_id: &str,
        msg: ServerMessage,
        exclude: Option<ConnectionId>,
    ) {
        if let Some(room) = self.rooms.get(room_id) {
            for &conn_id in &room.connections {
                if Some(conn_id) != exclude {
                    if let Some(conn) = self.connections.get(&conn_id) {
                        let _ = conn.tx.send(msg.clone());
                    }
                }
            }
        }
    }

    fn cleanup_empty_rooms(&self) {
        let empty_rooms: Vec<_> = self
            .rooms
            .iter()
            .filter(|r| r.connections.is_empty())
            .map(|r| r.key().clone())
            .collect();

        for room_id in empty_rooms {
            self.rooms.remove(&room_id);
            debug!("Cleaned up empty room {}", room_id);
        }
    }
}
