# FrameTogether Server

A lightweight WebSocket server for synchronizing video playback across multiple browsers. Built with Rust for performance and reliability.

This server communicates with the [FrameTogether browser extension](https://github.com/un1970ix/frametogether-extension).

You can self-host FrameTogether to run your own instance, or use the default public instance hosted by the author at [frametogether.19702038.xyz](https://frametogether.19702038.xyz). The default instance temporarily stores IP addresses only in RAM for rate limiting and does not persist or share them. The author is not liable for any data handling or security issues on self-hosted or third-party instances.

**Current Status**: Built specifically for MUBI video synchronization. Future versions may support additional platforms.

FrameTogether is an independent project and is not affiliated with, endorsed by, or connected to MUBI or its affiliates in any way.

## Features

- Real-time video synchronization.
- Anonymous rooms with auto-generated user names.
- Host-based control so first user controls playback.
- Automatic cleanup of empty rooms.
- Rate limiting and connection management.
- Zero configuration defaults.

## Planned Features

- Prometheus metrics endpoint for monitoring.
- Additional video platform support.

## Quick Start

### Using Docker

Clone the repository:
```bash
git clone git@github.com:un1970ix/frametogether-server.git
cd frametogether-server
```

Start the server:
```bash
docker compose up -d
```

The server will start on port 47642. To use a different port:

```bash
PORT=8080 docker compose up -d
```

### Building from Source

Clone the repository:
```bash
git clone git@github.com:un1970ix/frametogether-server.git
cd frametogether-server
```

Requirements:
- Rust 1.88 or later.

```bash
cargo build --release
./target/release/frametogether-server
```

## Configuration

Configuration via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` or `ADDR` | 47642 | Server port or full address. |
| `MAX_ROOMS` | 1000 | Maximum concurrent rooms. |
| `RATE_LIMIT_PER_SECOND` | 30 | Requests per second per IP. |
| `RUST_LOG` | info | Logging level options are debug, info, warn, or error. |

## Protocol

The server uses WebSocket with JSON messages.

### Client to Server

**Create Room**
```json
{"type": "Create"}
```

**Join Room**
```json
{"type": "Join", "room_id": "abc123"}
```

**Update Video State** (Host only.)
```json
{"type": "State", "time": 120.5, "paused": false}
```

**Leave Room**
```json
{"type": "Leave"}
```

**Heartbeat** (Keeps connection alive.)
```json
{"type": "Heartbeat"}
```

### Server to Client

**Room Created**
```json
{
  "type": "RoomCreated",
  "room_id": "abc123"
}
```

**Room Joined**
```json
{
  "type": "RoomJoined",
  "room_id": "abc123",
  "is_host": true,
  "your_name": "Happy Panda",
  "participants": []
}
```

**Video Sync** (Sent to non-hosts.)
```json
{
  "type": "Sync",
  "time": 120.5,
  "paused": false
}
```

**User Joined**
```json
{
  "type": "UserJoined",
  "user": {"name": "Clever Fox", "is_host": false}
}
```

**User Left**
```json
{
  "type": "UserLeft",
  "user_name": "Clever Fox"
}
```

**Error**
```json
{
  "type": "Error",
  "message": "Room not found"
}
```

## Architecture

The server maintains all state in memory:
- No database required.
- Rooms are automatically cleaned up when empty.
- Connection IDs are incremental and never reset.
- Each connection gets a unique animal name per room. (Just for fun, I may add functionality later.)

### Security

- Rate limiting per IP address.
- Input validation on all messages.
- Runs as non-root user in Docker.
- Memory-safe Rust prevents common vulnerabilities.
- Maximum room limit prevents resource exhaustion.
- Real IP detection works behind proxies.

## Development

Run with debug logging:
```bash
RUST_LOG=debug cargo run
```

Format code:
```bash
cargo fmt
```

Check for issues:
```bash
cargo clippy
```

## Deployment

The included `compose.yml` provides a production-ready setup:
- Automatic restart on failure.
- Health checks via `/health` endpoint.
- Runs as unprivileged user.

For production, place behind a reverse proxy (Such as nginx or Caddy.) for:
- SSL/TLS termination.
- Additional rate limiting.
- Load balancing multiple instances.

### Reverse Proxy Configuration

For nginx:
```nginx
location /sync {
    proxy_pass http://localhost:47642;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
}
```

## License

See the [LICENSE](LICENSE) file for details.

## Contributing

Pull requests are welcome. Please run `cargo fmt` and `cargo clippy` before submitting.
