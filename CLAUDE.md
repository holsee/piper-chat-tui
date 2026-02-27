# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build
cargo run                                # interactive welcome screen (TUI form)
cargo run -- create --name Alice         # create a new chat room, prints a ticket
cargo run -- join --name Bob <ticket>    # join with the ticket string
cargo test                               # run unit tests (net, chat, transfer, welcome modules)
```

## Architecture

P2P terminal chat over iroh gossip + iroh-blobs for file transfer. Dual-licensed MIT/Apache-2.0.

### Module structure

- `main.rs` — CLI parsing (clap), networking setup, and the main `tokio::select!` event loop
- `net.rs` — Wire protocol (`Message` enum serialized with postcard), `ChatTicket` (base32 via `iroh_tickets::Ticket` trait), `ConnTracker` (thread-safe connection type tracking via `Arc<RwLock<HashMap>>`)
- `chat.rs` — `App` struct (all TUI state) and `ui()` rendering function (immediate-mode ratatui)
- `welcome.rs` — Standalone welcome screen with its own event loop; returns `WelcomeResult` to main
- `transfer.rs` — `TransferManager` state machine (`Pending → Downloading → Complete/Failed`) and file pane rendering. Background downloads send `TransferEvent`s via mpsc channel
- `filepicker.rs` — Modal file picker overlay wrapping `ratatui-explorer::FileExplorer`
- `theme.rs` — Centralized `Theme` struct with dark/light palettes, toggled at runtime with Ctrl+T. All color references go through `Theme` — no hardcoded `Color::*` elsewhere

### Event loop (main.rs)

The main `tokio::select!` merges four async sources:
1. Keyboard input — `crossterm::EventStream`
2. Gossip events — `NeighborUp`/`NeighborDown`/`Received` from `iroh_gossip`
3. Transfer events — progress/complete/failed from background download tasks via `mpsc`
4. UI tick — 50ms interval for ratatui redraws + connection type polling

### Networking flow

`Endpoint` → `Gossip` + `BlobsProtocol` → `Router` (multiplexes GOSSIP_ALPN + BLOBS_ALPN) → subscribe to topic → split into sender/receiver. QUIC transport provides identity (no message signing). Blob store uses `FsStore` (redb) keyed by endpoint ID to avoid lock contention across instances.

### Wire protocol

`Message` enum: `Join { nickname, endpoint_id }`, `Chat { nickname, text }`, `FileOffer { nickname, endpoint_id, filename, size, hash }` — serialized with postcard.

### Key TUI patterns

- Immediate-mode rendering: `App` is mutated then `ui()` rebuilds every frame
- `AppMode` enum routes keyboard focus between Chat, FilePicker, and FilePane
- Modal overlays (file picker, welcome) use `Clear` widget + render-last for z-ordering
- Connection type indicators (`[direct]`/`[relay]`/`[?]`) polled from `ConnTracker` every tick

### Keyboard controls

| Key | Context | Action |
|-----|---------|--------|
| Enter | Chat | Send message |
| Esc | Chat | Quit |
| Ctrl+F | Chat | Open file picker |
| Ctrl+T | Any | Toggle dark/light theme |
| Tab/Shift+Tab | Chat | Cycle focus (chat ↔ file pane) |
| Up/Down | File pane | Navigate entries |
| Enter | File pane | Download pending / open completed |
