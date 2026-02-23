# piper-chat

A minimal peer-to-peer terminal chat app built on [iroh](https://iroh.computer/) gossip.

```
+------------------------------------------+------------------+
| piper-chat                               | peers            |
|------------------------------------------|------------------|
| [system] peer joined: abc123             | [?]     Alice    |
| Alice: Hello!                            | [direct] Bob     |
| Bob: Hey there!                          |                  |
|                                          |                  |
|------------------------------------------|                  |
| > type here|                             |                  |
+------------------------------------------+------------------+
```

## Quick Start

```bash
cargo build
```

**Terminal 1** — create a room:

```bash
cargo run -- create --name Alice
```

Copy the printed ticket string, then press Enter to start chatting.

**Terminal 2** — join the room:

```bash
cargo run -- join --name Bob <ticket>
```

Press Enter to start chatting. Messages appear on both sides in real time.

## Keyboard Controls

| Key       | Action              |
|-----------|---------------------|
| Enter     | Send message        |
| Esc       | Quit                |
| Backspace | Delete character    |
| Left/Right| Move cursor         |

## Features

- Peer-to-peer chat over QUIC using iroh gossip — no server, no accounts
- **Connection type indicators** — each peer in the sidebar shows how you're connected:
  - `[direct]` (green) — direct IP connection, lowest latency
  - `[relay]` (yellow) — relayed through an iroh relay server
  - `[?]` (gray) — connection type not yet determined
- Connection status updates in real time as NAT traversal completes and paths upgrade from relay to direct

## How It Works

1. **Create** generates a random gossip topic and prints a ticket (topic ID + your endpoint)
2. **Join** uses that ticket to bootstrap into the gossip swarm
3. Messages are broadcast to all subscribed peers via `iroh-gossip`
4. The TUI renders incoming messages and peer join/leave events in real time
5. An `EndpointHooks` handler tracks each peer's `ConnectionInfo`, and the UI polls `selected_path()` every 50ms to show whether the connection is direct or relayed

## Dependencies

Built with [iroh](https://github.com/n0-computer/iroh) 0.96, [ratatui](https://github.com/ratatui/ratatui) 0.29, and [crossterm](https://github.com/crossterm-rs/crossterm) 0.28.
