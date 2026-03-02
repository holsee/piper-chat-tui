```
 ████ █ ████ ████ ████   ████ █  █  ██  █████
 █  █ █ █  █ █    █  █   █    █  █ █  █   █
 ████ █ ████ ███  ████   █    ████ ████   █
 █    █ █    █    █ █    █    █  █ █  █   █
 █    █ █    ████ █  █   ████ █  █ █  █   █
```

**Peer-to-peer terminal chat over [iroh](https://iroh.computer/) gossip. No server. No accounts. Just QUIC.**

```
┌─ piper-chat ──────────────────────────────────┬─ peers ──────────────┐
│ Ticket: chataaab3j...                         │ [you]  Alice (you)   │
│ [system] share the ticket to join             │ [direct] Bob         │
│ [system] type /help for commands              │ [relay]  Charlie     │
│                                               │                      │
│ 14:32 Bob: hey!                               │                      │
│ 14:32 Alice: hello!                           │                      │
│ 14:33 Charlie shared an image:                │                      │
│    ┌────────────────────────────┐             ├──────────────────────┤
│    │ sunset.jpg (2.4 MB) [jpeg] │             │ Copy Ticket (Ctrl+Y) │
│    │ ↓ download  │  ⏎ open      │             ├──────────────────────┤
│    └────────────────────────────┘             │                      │
├─ files ───────────────────────────────────────┤                      │
│ > Bob: notes.txt (12 KB)            [dl]      │                      │
│   You: photo.png (3.1 MB)      [Sharing]      │                      │
├───────────────────────────────────────────────┤                      │
│ > type here█                                  │                      │
└───────────────────────────────────────────────┴──────────────────────┘
```

---

## Quick Start

```bash
cargo build
```

**Terminal 1** &mdash; create a room:

```bash
cargo run -- create --name Alice
```

Copy the printed ticket, then start chatting.

**Terminal 2** &mdash; join the room:

```bash
cargo run -- join --name Bob <ticket>
```

That's it. Messages flow in real time over QUIC.

Or skip the CLI flags and use the **interactive welcome screen**:

```bash
cargo run
```

```
┌──────────────── piper-chat ─────────────────┐
│    P2P terminal chat over iroh gossip       │
│                                             │
│  Name:   [ Alice          ]                 │
│  Mode:   (x) Create  ( ) Join               │
│                                             │
│         [ Enter ] Start   [ Esc ] Quit      │
└─────────────────────────────────────────────┘
```

---

## Features

### Encrypted P2P Messaging

```
         You                      Peer
          │                          │
          │──── QUIC (iroh gossip) ──│
          │   end-to-end encrypted   │
          │   no central server      │
          │   NAT traversal built-in │
          │                          │
```

- Messages broadcast via iroh gossip over QUIC &mdash; direct UDP when possible, relay fallback when not
- Each peer identified by an Ed25519 keypair &mdash; no accounts, no signup
- Share a base32 ticket string to invite others &mdash; copy with **Ctrl+Y**
- Message deduplication ensures no duplicates even with multiple paths

### File Sharing

```
       Alice                         Bob
         │                             │
         │   FileOffer (BLAKE3 hash)   │
         │ ──────────────────────────> │
         │                             │
         │    iroh-blobs download      │
         │ <────── ████░░ 65% ──────── │
         │                             │
```

- **Broadcast** &mdash; `Ctrl+F` to open file picker, share with all peers
- **Targeted** &mdash; `/sendto <name>` to share with a specific peer only
- **Progress** &mdash; live download bar: `[███░░░] 45%`
- **Media cards** &mdash; images and videos render as inline cards with download/open actions
- **Unshare** &mdash; retract a shared file at any time
- Content-addressed via BLAKE3 &mdash; integrity verified automatically

### Live Connection Status

```
┌─ peers ────────────────┐
│ [you]    Alice (you)   │  ← always first
│ [direct] Bob           │  ← UDP hole-punched
│ [relay]  Charlie       │  ← via relay server
│ [?]      Dave          │  ← resolving...
└────────────────────────┘
```

- Polled live from the iroh endpoint every 50ms
- Watches connections upgrade in real time: `[?]` &rarr; `[relay]` &rarr; `[direct]`
- Color-coded: green (direct), yellow (relay), gray (unknown), purple (you)

### History Sync

- New peers automatically receive chat history from existing peers
- Up to 1000 messages synced as an iroh blob on join
- Synced messages render inline with `(history)` tag

### Dark & Light Themes

Toggle with **Ctrl+T** anywhere &mdash; welcome screen, chat, file picker, all panels.

```
┌─ Dark (default) ─────┐     ┌─ Light ──────────────┐
│  bg:  deep purple    │     │  bg:  off-white      │
│  acc: vivid purple   │     │  acc: deep purple    │
│  txt: light gray     │     │  txt: dark gray      │
└──────────────────────┘     └──────────────────────┘
```

### Mouse Support

- Click message pane, input bar, file entries, media card actions, copy ticket button
- Scroll wheel to browse message history (3 lines per tick)
- Scroll position indicator: `↑ 5/12`

---

## Keyboard Controls

| Key              | Context   | Action                    |
|------------------|-----------|---------------------------|
| **Enter**        | Chat      | Send message              |
| **Esc**          | Chat      | Quit                      |
| **Ctrl+F**       | Chat      | Open file picker          |
| **Ctrl+T**       | Any       | Toggle dark/light theme   |
| **Ctrl+Y**       | Chat      | Copy ticket to clipboard  |
| **Tab**          | Chat      | Focus file pane           |
| **Shift+Tab**    | File pane | Focus chat                |
| **Up/Down**      | File pane | Navigate entries          |
| **Enter**        | File pane | Download / open / unshare |
| **Left/Right**   | Chat      | Move cursor               |
| **Backspace**    | Chat      | Delete character          |

### Slash Commands

| Command            | Action                           |
|--------------------|----------------------------------|
| `/help`            | Show controls reference          |
| `/send`            | Open file picker (broadcast)     |
| `/sendto <name>`   | Open file picker (targeted)      |

---

## Architecture

```
┌───────────────────────────────────────────────────┐
│                     main.rs                       │
│            CLI + tokio::select! loop              │
└──────────┬────────────────┬──────────────┬────────┘
           │                │              │
           v                v              v
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│   chat.rs    │  │    net.rs    │  │ transfer.rs  │
│  App + ui()  │  │   Message    │  │ state mach.  │
│  rendering   │  │  ChatTicket  │  │  file pane   │
└──────┬───────┘  └──────────────┘  └──────────────┘
       │
┌──────┴───────┐  ┌──────────────┐  ┌──────────────┐
│  welcome.rs  │  │ filepicker.rs│  │   theme.rs   │
│  setup form  │  │ modal overlay│  │  dark/light  │
└──────────────┘  └──────────────┘  └──────────────┘
```

The main event loop merges four async sources via `tokio::select!`:

1. **Keyboard/mouse** &mdash; crossterm `EventStream`
2. **Gossip events** &mdash; `NeighborUp` / `NeighborDown` / `Received`
3. **Transfer events** &mdash; progress/complete/failed from background downloads
4. **UI tick** &mdash; 50ms interval for rendering + live connection polling

Networking: `Endpoint` &rarr; `Gossip` + `BlobsProtocol` &rarr; `Router` (multiplexes GOSSIP_ALPN + BLOBS_ALPN). QUIC provides identity. Blob store uses `FsStore` (redb) keyed by endpoint ID.

---

## Dependencies

| Crate | Role |
|-------|------|
| [iroh](https://github.com/n0-computer/iroh) 0.96 | QUIC endpoint, NAT traversal, relay |
| [iroh-gossip](https://crates.io/crates/iroh-gossip) | Pub-sub messaging |
| [iroh-blobs](https://crates.io/crates/iroh-blobs) | Content-addressed file transfer |
| [ratatui](https://github.com/ratatui/ratatui) 0.29 | Terminal UI framework |
| [crossterm](https://github.com/crossterm-rs/crossterm) 0.28 | Terminal input/output |
| [tokio](https://tokio.rs/) | Async runtime |
| [clap](https://github.com/clap-rs/clap) | CLI argument parsing |
| [postcard](https://github.com/jamesmunns/postcard) | Binary serialization |

## License

Dual-licensed under MIT and Apache-2.0.
