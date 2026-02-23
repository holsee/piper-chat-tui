# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build
cargo run -- create --name Alice        # create a new chat room, prints a ticket
cargo run -- join --name Bob <ticket>    # join with the ticket string
```

No tests or linter configured — this is a single-file example app.

## Architecture

Single-crate, single-file (`src/main.rs`, ~315 lines) P2P terminal chat over iroh gossip.

**Event loop** uses `tokio::select!` to merge three async streams:
1. Keyboard input via `crossterm::EventStream`
2. Gossip network events (`NeighborUp`/`NeighborDown`/`Received`) from `iroh_gossip`
3. UI tick (50ms interval) for ratatui redraws

**Networking flow:** Endpoint → Gossip → Router → subscribe to topic → split into sender/receiver. No message signing — QUIC transport provides identity. Tickets are `TopicId + BTreeSet<EndpointId>`, base32-serialized via `iroh_tickets::Ticket`.

**Wire protocol:** `Message::Chat { nickname, text }` serialized with postcard.

**TUI:** Two-pane ratatui layout (messages + input). System messages styled dim/italic, nicknames cyan/bold. Auto-scrolls to bottom.
