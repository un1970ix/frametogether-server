# FrameTogether Server

A lightweight WebSocket server for synchronizing video playback across multiple browsers. Built with Rust for performance and reliability.

This server communicates with the [FrameTogether browser extension](https://github.com/un1970ix/frametogether-extension).

You can self-host FrameTogether to run your own instance, or use the default public instance hosted by the author at [frametogether.19702038.xyz](https://frametogether.19702038.xyz). The default instance holds client addresses only in RAM for rate limiting, does not persist or share them, and does not write them to its logs at the default log level. The author is not liable for any data handling or security issues on self-hosted or third-party instances.

**Current Status:** Built specifically for MUBI video synchronization. Future versions may support additional platforms.

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

## Upgrading to 0.2

Room IDs are now 10 characters instead of 6, configurable via `ROOM_ID_LENGTH` between 6 and 64. Update the extension alongside the server, or set `ROOM_ID_LENGTH=6` during a transition.

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
| `RATE_LIMIT_PER_SECOND` | 30 | Messages per second per client address. |
| `MAX_CONNECTIONS` | 10000 | Maximum concurrent connections. |
| `MAX_CONNECTIONS_PER_IP` | 20 | Maximum concurrent connections per address. |
| `MAX_PARTICIPANTS_PER_ROOM` | 32 | Maximum participants in one room. At most 256. |
| `IDLE_TIMEOUT_SECONDS` | 90 | Disconnect a client that sends nothing for this long. |
| `MAX_MESSAGE_BYTES` | 8192 | Largest accepted client message. |
| `ROOM_ID_LENGTH` | 10 | Length of generated room IDs, between 6 and 64. |
| `TRUSTED_PROXIES` | *EMPTY* | Addresses or CIDR ranges whose forwarding headers are believed. See below. |
| `ALLOWED_ORIGINS` | *EMPTY* | Extra `Origin` values allowed to open a socket. See below. |
| `RUST_LOG` | info | Logging level options are debug, info, warn, or error. |

### Running Behind a Reverse Proxy

`TRUSTED_PROXIES` is empty by default, which means **forwarding headers are ignored entirely** and the real TCP peer address is used for rate limiting. This is the safe default: if headers were trusted unconditionally, any client could pick its own rate limit bucket just by setting `X-Forwarded-For`.

When you do put the server behind a proxy, set it to the proxy's address so `X-Forwarded-For` is honoured:

```bash
TRUSTED_PROXIES=127.0.0.1,10.0.0.0/8
```

Your proxy **must** overwrite `X-Forwarded-For` rather than append to a client-supplied value. The nginx configuration below does this correctly.

### Origin Checks

Browsers always attach an `Origin` header to a WebSocket handshake. Extension origins such as `chrome-extension://`, `moz-extension://` and `safari-web-extension://` are always allowed, and so are clients that send no `Origin` at all, which means anything that is not a browser. Any other web origin is rejected unless it is listed in `ALLOWED_ORIGINS`. This stops an arbitrary website from opening sockets on its visitors' behalf. Setting `ALLOWED_ORIGINS=*` disables the check.

## Protocol

The server uses WebSocket with JSON messages.

### Client to Server

**Create Room**
```json
{"type": "Create"}
```

**Join Room**
```json
{"type": "Join", "room_id": "kWwlQxFUDX"}
```

**Update Video State:** may only be sent by the host.
```json
{"type": "State", "time": 120.5, "paused": false}
```

**Leave Room**
```json
{"type": "Leave"}
```

**Heartbeat:** keeps the connection alive.
```json
{"type": "Heartbeat"}
```

### Server to Client

**Room Created**
```json
{
  "type": "RoomCreated",
  "room_id": "kWwlQxFUDX"
}
```

**Room Joined**
```json
{
  "type": "RoomJoined",
  "room_id": "kWwlQxFUDX",
  "is_host": false,
  "your_name": "Eager Otter",
  "participants": [
    {"name": "Elegant Swan", "is_host": true},
    {"name": "Happy Eagle", "is_host": false}
  ]
}
```

`participants` lists everyone already in the room and excludes you. It is empty for the first user.

Creating a room already puts you in it, so the server sends `RoomCreated` **and** `RoomJoined`. Clients must not follow up with a redundant `Join` for the room they just created.

**Video Sync:** is sent to every participant except the host that triggered it, and once to a client immediately after it joins so it can catch up.
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

**Host Changed:** is sent when the host leaves and another participant is promoted.
```json
{
  "type": "HostChanged",
  "user_name": "Clever Fox"
}
```

**Error**
```json
{
  "type": "Error",
  "message": "Room not found."
}
```

### Error Messages

| Message | Cause |
|---------|-------|
| `Room not found.` | No room exists with that ID. |
| `Room is full.` | The room already holds `MAX_PARTICIPANTS_PER_ROOM` participants. |
| `Too many rooms.` | The server already holds `MAX_ROOMS` rooms. |
| `Not authorized.` | A `State` message from a client that is not the host of its room. |
| `Invalid request.` | A malformed field, such as a `time` that is negative, not finite, or beyond `MAX_PLAYBACK_SECONDS`, or a room ID that is empty or longer than 64 characters. |
| `Invalid message format.` | The payload is not valid JSON, has an unknown `type`, or carries a field the message does not define. |
| `Rate limit exceeded.` | More than `RATE_LIMIT_PER_SECOND` messages from one client address. |
| `Message too large.` | The payload exceeds `MAX_MESSAGE_BYTES`. |

Messages are validated strictly, so an unknown field causes the whole message to be rejected rather than ignored.

## Architecture

The server maintains all state in memory:
- No database required.
- Rooms are automatically cleaned up when empty.
- Connection IDs are incremental and never reset.
- Each connection gets a unique animal name per room. Just for fun, I may add functionality later.

### Security

- Rate limiting keyed on the verified TCP peer address. Forwarding headers are ignored unless the peer is a configured trusted proxy, so a client cannot choose its own bucket.
- `Origin` checks on the WebSocket handshake, which block cross-site WebSocket hijacking.
- Schema-validated messages: unknown fields are rejected, room IDs are length-bounded, and non-finite or out-of-range playback times are refused rather than fanned out.
- Bounded everything: concurrent rooms, connections overall and per address, participants per room, message size, per-client send queue, and an idle timeout. Rate limiter state is evicted on a timer rather than kept forever.
- A client that stops reading is disconnected instead of being allowed to queue messages without limit.
- Runs as a non-root user in Docker, read-only, with all capabilities dropped.
- Memory-safe Rust, with overflow checks kept on in release builds.

Client addresses are never written to logs above `debug` level.

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
cargo clippy --all-targets -- -D warnings
```

Run the tests:
```bash
cargo test
```

Audit dependencies:
```bash
cargo audit
```

## Deployment

The included `compose.yml` provides a production-ready setup:
- Automatic restart on failure.
- Health checks via `/health` endpoint.
- Runs as unprivileged user.

`compose.yml` binds to `127.0.0.1` so the server is not directly reachable from the network. Place it behind a reverse proxy such as nginx or Caddy for:
- SSL and TLS termination.
- Additional rate limiting.
- Load balancing multiple instances.

Remember to set `TRUSTED_PROXIES` to your proxy's address, otherwise every client will appear to come from the proxy and share one rate limit bucket.

### Reverse Proxy Configuration

For nginx:
```nginx
location /sync {
    proxy_pass http://localhost:47642;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header X-Real-IP $remote_addr;
    # Overwrite rather than append: $proxy_add_x_forwarded_for would preserve a
    # client-supplied value and let callers forge their own address.
    proxy_set_header X-Forwarded-For $remote_addr;
}
```

## License

See the [LICENSE](LICENSE) file for details.

## Contributing

Pull requests are welcome. Please run `cargo fmt` and `cargo clippy` before submitting.
