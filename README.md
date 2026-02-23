# piper-chat

A minimal peer-to-peer terminal chat app built on [iroh](https://iroh.computer/) gossip.

```
+------------------------------------------+
| piper-chat                               |
|------------------------------------------|
| [system] peer joined: abc123             |
| Alice: Hello!                            |
| Bob: Hey there!                          |
|                                          |
|------------------------------------------|
| > type here|                             |
+------------------------------------------+
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

## How It Works

Peers connect directly over QUIC using iroh's gossip protocol. No server, no accounts.

1. **Create** generates a random gossip topic and prints a ticket (topic ID + your endpoint)
2. **Join** uses that ticket to bootstrap into the gossip swarm
3. Messages are broadcast to all subscribed peers via `iroh-gossip`
4. The TUI renders incoming messages and peer join/leave events in real time

## Dependencies

Built with [iroh](https://github.com/n0-computer/iroh) 0.96, [ratatui](https://github.com/ratatui/ratatui) 0.29, and [crossterm](https://github.com/crossterm-rs/crossterm) 0.28.
