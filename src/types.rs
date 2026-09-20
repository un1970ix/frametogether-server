use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

pub type ConnectionId = u64;
pub type RoomId = String;

pub const MAX_ROOM_ID_LEN: usize = 64;
pub const MAX_PLAYBACK_SECONDS: f64 = 86_400.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ClientMessage {
    Create,
    Join { room_id: RoomId },
    Leave,
    State { time: f64, paused: bool },
    Heartbeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    RoomCreated {
        room_id: RoomId,
    },
    RoomJoined {
        room_id: RoomId,
        is_host: bool,
        your_name: String,
        participants: Vec<Participant>,
    },
    Sync {
        time: f64,
        paused: bool,
    },
    UserJoined {
        user: Participant,
    },
    UserLeft {
        user_name: String,
    },
    HostChanged {
        user_name: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    pub name: String,
    pub is_host: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VideoState {
    pub time: f64,
    pub paused: bool,
}

#[derive(Debug)]
pub struct Connection {
    pub tx: mpsc::Sender<ServerMessage>,
    pub room_id: Option<RoomId>,
    pub name: Option<String>,
    pub shutdown: Arc<Notify>,
}

#[derive(Debug)]
pub struct Room {
    pub host: ConnectionId,
    pub connections: Vec<ConnectionId>,
    pub state: VideoState,
    pub used_names: HashSet<String>,
}

impl Room {
    pub fn new(host: ConnectionId) -> Self {
        Self {
            host,
            connections: vec![host],
            state: VideoState {
                time: 0.0,
                paused: true,
            },
            used_names: HashSet::new(),
        }
    }
}
