//! # piper-chat — A peer-to-peer terminal chat application
//!
//! This is a single-file TUI (terminal user interface) chat app built on top of
//! the [iroh](https://docs.rs/iroh) networking stack. Two or more users can chat
//! directly over the internet without any central server.
//!
//! ## How it works at a high level
//!
//! 1. **One user creates a room** (`cargo run -- create --name Alice`), which
//!    prints a "ticket" string — a compact token encoding the chat topic and
//!    the creator's network identity.
//! 2. **Other users join** (`cargo run -- join --name Bob <ticket>`) by pasting
//!    that ticket string.
//! 3. Under the hood, [iroh-gossip] connects the peers using QUIC (a modern
//!    transport protocol). Messages are broadcast to all peers in the topic via
//!    a gossip protocol — each peer forwards messages it receives to its
//!    neighbors.
//!
//! ## Architecture overview
//!
//! The program is structured as an **async event loop** (powered by [tokio])
//! that merges three independent streams of events using `tokio::select!`:
//!
//! | Stream              | Source                  | What it produces                     |
//! |---------------------|-------------------------|--------------------------------------|
//! | Keyboard input      | `crossterm::EventStream`| Key presses from the user            |
//! | Gossip network      | `iroh_gossip`           | Messages from peers, join/leave events |
//! | UI tick (50 ms)     | `tokio::time::interval` | Periodic signal to redraw the screen |
//!
//! ## Key Rust concepts used
//!
//! - **`?` operator**: Propagates errors up to the caller. `foo()?` is shorthand
//!   for "if `foo()` returns an error, return that error from this function too."
//! - **`impl Into<String>`**: A generic parameter that accepts anything convertible
//!   to a `String` (string literals, `String` values, `format!()` output, etc.).
//! - **`Arc<RwLock<T>>`**: Shared ownership (`Arc`) of data protected by a
//!   read-write lock (`RwLock`), allowing multiple threads to read concurrently
//!   but only one to write at a time.
//! - **`tokio::select!`**: Waits on multiple async operations simultaneously and
//!   runs the handler for whichever completes first.
//! - **Trait implementations**: Types implement "traits" (similar to interfaces)
//!   to gain capabilities — e.g., `Serialize`/`Deserialize` for encoding,
//!   `Ticket` for the ticket format, `EndpointHooks` for connection callbacks.

// ─────────────────────────────────────────────────────────────────────────────
// Imports — grouped by purpose with explanations of what each crate provides.
// ─────────────────────────────────────────────────────────────────────────────

// Standard library: collections, synchronization primitives
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

// `anyhow::Result` is a convenience type that can hold any error. It saves us
// from defining custom error types for this small program.
use anyhow::Result;

// `clap` parses command-line arguments. The `derive` feature lets us define the
// CLI structure as a Rust enum and clap generates the parser automatically.
use clap::Parser;

// `crossterm` provides cross-platform terminal manipulation: raw mode (character-
// by-character input), alternate screen (so we don't clobber the user's terminal
// history), and an async event stream for keyboard input.
use crossterm::{
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

// `iroh` is the networking layer. An "endpoint" is our identity on the network,
// identified by an `EndpointId` (a public key). `EndpointHooks` lets us observe
// connection events (like completed handshakes).
use iroh::endpoint::{AfterHandshakeOutcome, ConnectionInfo, EndpointHooks};
use iroh::EndpointId;

// `iroh_gossip` implements a gossip protocol on top of iroh. Peers subscribe to
// "topics" and any message broadcast to a topic is forwarded to all subscribers.
// `GOSSIP_ALPN` is the protocol identifier used during QUIC handshakes.
use iroh_gossip::{
    api::Event as GossipEvent,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};

// `iroh_tickets` provides a standard way to serialize/deserialize "tickets" —
// compact tokens that encode connection information for sharing out-of-band
// (e.g., copy-pasted in a chat message or email).
use iroh_tickets::Ticket;

// `n0_future::StreamExt` extends async streams with combinators like `.next()`.
// This is similar to `futures::StreamExt` but from the n0 (iroh) ecosystem.
use n0_future::StreamExt;

// `ratatui` is the TUI (terminal UI) framework. It uses an "immediate mode"
// rendering model: every frame, we build the entire UI from scratch and ratatui
// figures out what changed on screen. This is simple but effective for small UIs.
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

// `serde` provides serialization/deserialization. Adding `#[derive(Serialize,
// Deserialize)]` to a struct/enum lets it be encoded to/from formats like JSON,
// postcard (a compact binary format), etc.
use serde::{Deserialize, Serialize};

// `tokio` is the async runtime. `interval` creates a repeating timer, and
// `Duration` represents a span of time.
use tokio::time::{Duration, interval};

// ═════════════════════════════════════════════════════════════════════════════
// CLI — Command-line argument parsing
// ═════════════════════════════════════════════════════════════════════════════

/// The two modes of operation, parsed from command-line arguments by `clap`.
///
/// Usage:
/// ```text
/// piper-chat create --name Alice          # Creates a new room, prints a ticket
/// piper-chat join --name Bob <ticket>     # Joins an existing room using a ticket
/// ```
///
/// The `#[derive(Parser)]` attribute tells `clap` to generate argument parsing
/// code automatically from this enum's structure. Each variant becomes a
/// subcommand.
#[derive(Parser)]
#[command(name = "piper-chat")]
enum Cli {
    /// Create a new chat room
    Create {
        /// Your display name in the chat
        #[arg(short, long)]
        name: String,
    },
    /// Join an existing chat room
    Join {
        /// Your display name in the chat
        #[arg(short, long)]
        name: String,
        /// Ticket string from the room creator (a base32-encoded blob)
        ticket: String,
    },
}

// ═════════════════════════════════════════════════════════════════════════════
// Wire protocol — The messages sent between peers over the network
// ═════════════════════════════════════════════════════════════════════════════

/// A message that travels over the network between peers.
///
/// Serialized using [postcard](https://docs.rs/postcard), a compact binary
/// format designed for embedded/no_std use. It's much smaller than JSON, which
/// matters for real-time chat.
///
/// `#[derive(Serialize, Deserialize)]` automatically implements the conversion
/// to/from bytes — we just call `postcard::to_stdvec(&msg)` to encode and
/// `postcard::from_bytes(&bytes)` to decode.
#[derive(Serialize, Deserialize)]
enum Message {
    /// Sent when a peer first connects, so others learn their chosen nickname.
    /// Without this, peers would only know each other by cryptographic ID.
    Join {
        nickname: String,
        endpoint_id: EndpointId,
    },
    /// A regular chat message containing the sender's nickname and their text.
    Chat { nickname: String, text: String },
}

// ═════════════════════════════════════════════════════════════════════════════
// Ticket — A shareable token that lets new peers join the chat room
// ═════════════════════════════════════════════════════════════════════════════

/// A "ticket" encodes everything a new peer needs to join a chat room:
/// - `topic_id`: Identifies *which* chat room to join (a random 32-byte ID).
/// - `bootstrap`: A set of peers already in the room that the new peer can
///   connect to initially. Once connected, gossip discovers other peers.
///
/// The `Ticket` trait (from `iroh_tickets`) provides `serialize()`/`deserialize()`
/// methods that encode this as a human-friendly base32 string prefixed with the
/// ticket kind (e.g., `chat...`).
///
/// `BTreeSet` is used instead of `HashSet` because it produces deterministic
/// (sorted) serialization output — the same set of peers always produces the
/// exact same ticket string.
#[derive(Serialize, Deserialize, Clone)]
struct ChatTicket {
    topic_id: TopicId,
    bootstrap: BTreeSet<EndpointId>,
}

impl ChatTicket {
    /// Creates a ticket for a brand-new chat room with a random topic ID
    /// and no bootstrap peers yet (the creator will add themselves before
    /// sharing the ticket).
    fn new_random() -> Self {
        Self {
            topic_id: TopicId::from_bytes(rand::random()),
            bootstrap: BTreeSet::new(),
        }
    }
}

/// Implementing the `Ticket` trait lets `iroh_tickets` handle the base32
/// encoding/decoding for us. We just need to define how to convert our
/// `ChatTicket` to/from raw bytes (using postcard).
impl Ticket for ChatTicket {
    /// A short string prefix baked into the serialized ticket so that different
    /// ticket types (chat, file-transfer, etc.) can be distinguished.
    const KIND: &'static str = "chat";

    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap()
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        // The `?` here converts a postcard decode error into a `ParseError`.
        Ok(postcard::from_bytes(bytes)?)
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Connection tracking — Monitors whether peers are connected directly or via relay
// ═════════════════════════════════════════════════════════════════════════════

/// Describes how we're connected to a peer.
///
/// Iroh can connect peers in two ways:
/// - **Direct**: A QUIC connection straight between the two machines (fastest).
/// - **Relay**: Traffic bounces through an iroh relay server (works when both
///   peers are behind NAT/firewalls that prevent direct connections).
enum ConnectionKind {
    /// We haven't determined the connection type yet.
    Unknown,
    /// Connected directly via IP (lowest latency).
    Direct,
    /// Connected through an iroh relay server (higher latency, but always works).
    Relay,
}

/// Display information about a connected peer, shown in the "peers" sidebar.
struct PeerInfo {
    /// The peer's display name (their nickname, or a truncated ID if unknown).
    name: String,
    /// How we're connected to this peer (direct, relay, or unknown).
    conn_type: ConnectionKind,
}

/// Tracks connection metadata for all peers we've completed a QUIC handshake with.
///
/// This uses `Arc<RwLock<HashMap<...>>>` — a common Rust pattern for sharing
/// mutable state across async tasks and threads:
/// - `Arc` (Atomic Reference Count): Allows multiple owners of the same data.
///   When cloned, it increments a counter; when dropped, it decrements. The data
///   is freed when the count reaches zero.
/// - `RwLock`: Protects the `HashMap` so multiple readers can access it
///   simultaneously, but writers get exclusive access.
///
/// The tracker itself lives in `main()`, while a cloned `Arc` handle is given
/// to the `EndpointHooks` callback (see `ConnectionTrackerHook` below).
#[derive(Debug)]
struct ConnectionTracker(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

impl ConnectionTracker {
    fn new() -> Self {
        // `Arc::default()` creates an Arc wrapping a new, empty HashMap.
        Self(Arc::default())
    }

    /// Creates a hook (callback handler) that shares the same underlying data.
    /// The hook is passed to the iroh endpoint so it can record connection info
    /// whenever a new peer completes a QUIC handshake.
    fn create_hook(&self) -> ConnectionTrackerHook {
        // `.clone()` on an `Arc` is cheap — it just increments the reference
        // count. Both the tracker and the hook now point to the same HashMap.
        ConnectionTrackerHook(self.0.clone())
    }

    /// Determines how we're connected to a specific peer by checking the
    /// connection metadata recorded by our hook.
    fn connection_kind(&self, peer_id: &EndpointId) -> ConnectionKind {
        let connections = self.0.read().unwrap();
        // `.and_then()` chains Option operations: if the peer exists in our map,
        // try to get their selected network path.
        match connections.get(peer_id).and_then(|conn| conn.selected_path()) {
            Some(path) if path.is_ip() => ConnectionKind::Direct,
            Some(_) => ConnectionKind::Relay,
            None => ConnectionKind::Unknown,
        }
    }
}

/// The hook half of the connection tracker. This implements `EndpointHooks`,
/// which iroh calls whenever a connection event occurs.
///
/// It's a separate type from `ConnectionTracker` because iroh takes ownership
/// of the hooks object, but we still need to read the data from our main loop.
/// Both types share the same `Arc<RwLock<HashMap<...>>>` underneath.
#[derive(Debug)]
struct ConnectionTrackerHook(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

impl EndpointHooks for ConnectionTrackerHook {
    /// Called by iroh after a QUIC handshake completes with a peer.
    /// We record the connection info so we can later check if the connection
    /// is direct (IP) or relayed.
    ///
    /// The lifetime annotations (`'a`) tell the Rust compiler that the returned
    /// future borrows from both `self` and `conn` for the same lifetime. This is
    /// required by the trait definition.
    fn after_handshake<'a>(
        &'a self,
        conn: &'a ConnectionInfo,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        self.0.write().unwrap().insert(conn.remote_id(), conn.clone());
        // Always accept the connection. In a more complex app, you could reject
        // unknown peers here.
        async { AfterHandshakeOutcome::accept() }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// App state — All the data the TUI needs to render and respond to input
// ═════════════════════════════════════════════════════════════════════════════

/// A single line in the chat message history. The UI renders these differently:
/// system messages are dim/italic, chat messages show a colored nickname.
enum ChatLine {
    /// An informational message from the system (e.g., "peer connected", "peer left").
    System(String),
    /// A chat message from a user.
    Chat { nickname: String, text: String },
}

/// The complete application state. Everything the UI needs lives here.
///
/// This is a common pattern in TUI apps: a single struct holds all state, and
/// the render function takes an immutable reference (`&App`) to draw the UI.
/// The event loop holds a mutable reference (`&mut App`) to update state.
struct App {
    /// All chat messages and system notifications, in chronological order.
    messages: Vec<ChatLine>,
    /// The current text the user is typing (before they press Enter).
    input: String,
    /// The cursor position within `input` (0 = before the first character).
    /// Tracked separately from `input.len()` to support left/right arrow keys.
    cursor_pos: usize,
    /// Set to `true` when the user presses Esc or the gossip stream closes.
    /// The event loop checks this after each iteration to know when to exit.
    should_quit: bool,
    /// Currently known peers, keyed by their endpoint ID.
    /// `BTreeMap` keeps peers sorted by ID for consistent display order.
    peers: BTreeMap<EndpointId, PeerInfo>,
}

impl App {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            should_quit: false,
            peers: BTreeMap::new(),
        }
    }

    /// Adds a system notification to the message history.
    ///
    /// `impl Into<String>` is a generic parameter — this function accepts
    /// anything that can be converted into a `String`: string literals (`&str`),
    /// owned `String` values, or `format!()` output. This avoids forcing callers
    /// to call `.to_string()` at every call site.
    fn push_system_message(&mut self, message: impl Into<String>) {
        self.messages.push(ChatLine::System(message.into()));
    }

    /// Adds a chat message to the message history.
    fn push_chat_message(&mut self, nickname: String, text: String) {
        self.messages.push(ChatLine::Chat { nickname, text });
    }

    // ── Keyboard input handling ─────────────────────────────────────────────

    /// Handles a key press event from the terminal.
    ///
    /// Returns `Some(text)` when the user presses Enter on a non-empty input
    /// (meaning we need to broadcast a chat message), or `None` otherwise.
    ///
    /// This is extracted from the event loop so the `main()` function reads
    /// more clearly. The `Option<String>` return type uses Rust's standard
    /// "optional value" enum: `Some(value)` when there's data, `None` when
    /// there isn't. The caller uses `if let Some(text) = ...` to handle both
    /// cases.
    fn handle_key_press(&mut self, key_code: KeyCode) -> Option<String> {
        match key_code {
            KeyCode::Esc => {
                self.should_quit = true;
                None
            }
            KeyCode::Enter => {
                // `drain(..)` removes all characters from the string and returns
                // them as an iterator. `.collect()` gathers them into a new String.
                // This is an efficient way to "take" the contents of a String.
                let text: String = self.input.drain(..).collect();
                self.cursor_pos = 0;
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
                None
            }
            KeyCode::Left => {
                // `saturating_sub(1)` subtracts 1 but clamps at 0 instead of
                // wrapping around (which would panic for unsigned integers).
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
                None
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
                None
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
                None
            }
            _ => None,
        }
    }

    // ── Gossip event handling ───────────────────────────────────────────────

    /// Handles an incoming gossip message (a chat message or a join
    /// announcement from another peer).
    fn handle_gossip_message(&mut self, raw_bytes: &[u8]) {
        // Attempt to decode the raw bytes into our `Message` enum.
        // If decoding fails (e.g., corrupted data), we silently ignore it.
        match postcard::from_bytes::<Message>(raw_bytes) {
            Ok(Message::Join {
                nickname,
                endpoint_id,
            }) => {
                self.push_system_message(format!("{nickname} joined"));
                self.peers.insert(
                    endpoint_id,
                    PeerInfo {
                        name: nickname,
                        conn_type: ConnectionKind::Unknown,
                    },
                );
            }
            Ok(Message::Chat { nickname, text }) => {
                self.push_chat_message(nickname, text);
            }
            Err(_) => {
                // Ignore malformed messages — could be from a different
                // protocol version or corrupted in transit.
            }
        }
    }

    /// Called when a new peer connects to the gossip topic (before they send
    /// their Join message, so we only know their cryptographic ID).
    fn handle_peer_connected(&mut self, peer_id: EndpointId) {
        // `fmt_short()` returns a truncated hex representation of the ID,
        // e.g., "a1b2c3d4" instead of the full 64-character key.
        self.peers.insert(
            peer_id,
            PeerInfo {
                name: peer_id.fmt_short().to_string(),
                conn_type: ConnectionKind::Unknown,
            },
        );
        self.push_system_message(format!("peer connected: {}", peer_id.fmt_short()));
    }

    /// Called when a peer disconnects from the gossip topic.
    fn handle_peer_disconnected(&mut self, peer_id: EndpointId) {
        // Remove the peer and use their name in the notification. If the peer
        // wasn't in our map (shouldn't happen), fall back to their short ID.
        let display_name = self
            .peers
            .remove(&peer_id)
            .map(|peer| peer.name)
            .unwrap_or_else(|| peer_id.fmt_short().to_string());
        self.push_system_message(format!("{display_name} left"));
    }

    /// Refreshes the connection type (direct/relay) for all remote peers.
    fn refresh_connection_types(
        &mut self,
        our_id: EndpointId,
        conn_tracker: &ConnectionTracker,
    ) {
        for (peer_id, peer) in &mut self.peers {
            // Skip ourselves — we don't have a network connection to ourself.
            if *peer_id != our_id {
                peer.conn_type = conn_tracker.connection_kind(peer_id);
            }
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// UI rendering — Draws the terminal interface using ratatui
// ═════════════════════════════════════════════════════════════════════════════

/// Renders the entire terminal UI for one frame.
///
/// Ratatui uses "immediate mode" rendering: every frame, this function builds
/// the complete UI from the current `App` state, and ratatui diffs it against
/// what's currently on screen to minimize terminal writes.
///
/// ## Layout
///
/// ```text
/// ┌─── piper-chat ──────────────┐┌─── peers ───┐
/// │ [system] waiting for peers...││ [direct] Bob │
/// │ Alice: hello!                ││              │
/// │ Bob: hi there!               ││              │
/// ├──────────────────────────────┤├──────────────┤
/// │ > typing here_               │               │
/// └──────────────────────────────┘               │
/// ```
fn render_ui(frame: &mut ratatui::Frame, app: &App) {
    // Split the terminal into two rows: messages area (flexible) and input bar (3 lines tall).
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(frame.area());

    // Split the top row into two columns: messages (flexible) and peers sidebar (24 chars wide).
    let top_panes =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(24)]).split(rows[0]);

    // ── Messages pane (top-left) ────────────────────────────────────────────

    let message_lines: Vec<Line> = app
        .messages
        .iter()
        .map(|msg| match msg {
            // System messages: dim and italic to visually separate them from chat.
            ChatLine::System(text) => Line::from(Span::styled(
                format!("[system] {text}"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
            // Chat messages: bold cyan nickname followed by the message text.
            ChatLine::Chat { nickname, text } => Line::from(vec![
                Span::styled(
                    nickname.as_str(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(": {text}")),
            ]),
        })
        .collect();

    // Auto-scroll: calculate how many lines to skip so the newest messages
    // are always visible at the bottom of the pane.
    let visible_line_count = top_panes[0].height.saturating_sub(2) as usize; // -2 for the border
    let scroll_offset = message_lines.len().saturating_sub(visible_line_count) as u16;

    let messages_widget = Paragraph::new(message_lines)
        .scroll((scroll_offset, 0))
        .block(Block::default().borders(Borders::ALL).title("piper-chat"));
    frame.render_widget(messages_widget, top_panes[0]);

    // ── Peers pane (top-right) ──────────────────────────────────────────────

    let peer_lines: Vec<Line> = app
        .peers
        .values()
        .map(|peer| {
            // Show connection type as a colored tag next to each peer's name.
            let (tag, tag_color) = match peer.conn_type {
                ConnectionKind::Direct => ("[direct]", Color::Green),
                ConnectionKind::Relay => ("[relay]", Color::Yellow),
                ConnectionKind::Unknown => ("[?]", Color::DarkGray),
            };
            Line::from(vec![
                Span::styled(format!("{tag} "), Style::default().fg(tag_color)),
                Span::styled(peer.name.as_str(), Style::default().fg(Color::Green)),
            ])
        })
        .collect();

    let peers_widget = Paragraph::new(peer_lines)
        .block(Block::default().borders(Borders::ALL).title("peers"));
    frame.render_widget(peers_widget, top_panes[1]);

    // ── Input pane (bottom, full width) ─────────────────────────────────────

    let input_widget = Paragraph::new(format!("> {}", app.input))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(input_widget, rows[1]);

    // Position the blinking cursor inside the input box.
    // +2 accounts for the border (1) and the "> " prompt (1 more character).
    frame.set_cursor_position((rows[1].x + 2 + app.cursor_pos as u16, rows[1].y + 1));
}

// ═════════════════════════════════════════════════════════════════════════════
// Main — Program entry point: sets up networking, terminal, and runs the event loop
// ═════════════════════════════════════════════════════════════════════════════

/// The `#[tokio::main]` attribute sets up a multi-threaded async runtime.
/// This lets us use `.await` in main and run multiple async tasks concurrently.
#[tokio::main]
async fn main() -> Result<()> {
    // ── Parse CLI arguments ─────────────────────────────────────────────────

    let cli = Cli::parse();

    // Extract the user's nickname and the chat ticket from the CLI arguments.
    // For `create`: generate a fresh random ticket.
    // For `join`: deserialize the ticket string the user pasted.
    let (nickname, ticket) = match &cli {
        Cli::Create { name } => (name.clone(), ChatTicket::new_random()),
        Cli::Join { name, ticket } => {
            // The turbofish syntax `<ChatTicket as Ticket>` tells Rust which
            // trait's `deserialize` method to call. This is needed because
            // multiple traits might define methods with the same name.
            let parsed_ticket = <ChatTicket as Ticket>::deserialize(ticket)?;
            (name.clone(), parsed_ticket)
        }
    };

    // ── Set up networking ───────────────────────────────────────────────────
    //
    // The networking stack has three layers:
    // 1. Endpoint: Our identity on the network (generates a keypair, manages
    //    QUIC connections to other peers).
    // 2. Gossip: A pub/sub protocol built on top of the endpoint. Peers
    //    subscribe to "topics" and messages broadcast to a topic reach all
    //    subscribers.
    // 3. Router: Dispatches incoming connections to the right protocol handler
    //    based on the ALPN (Application-Layer Protocol Negotiation) identifier.

    let conn_tracker = ConnectionTracker::new();

    let endpoint = iroh::Endpoint::builder()
        // ALPN tells connecting peers which protocol we speak. This is standard
        // TLS/QUIC behavior — it's how a single port can serve multiple protocols.
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        // Register our connection tracker so it gets notified of new connections.
        .hooks(conn_tracker.create_hook())
        .bind()
        .await?;

    // `spawn` starts the gossip protocol running in the background.
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // The router listens for incoming connections and routes them to the gossip
    // handler when the ALPN matches.
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // ── Build and display the shareable ticket ──────────────────────────────

    // Add our own endpoint ID to the ticket's bootstrap set so that peers who
    // join using this ticket will connect to us first.
    let mut shareable_ticket = ticket.clone();
    shareable_ticket.bootstrap.insert(endpoint.id());
    let ticket_string = <ChatTicket as Ticket>::serialize(&shareable_ticket);

    // Print the ticket before the TUI takes over the screen. The user can
    // copy-paste this string and share it with others.
    println!("Ticket (share with others to join):\n\n{ticket_string}\n");
    println!("Press ENTER to start chat...");
    let _ = std::io::stdin().read_line(&mut String::new());

    // ── Subscribe to the gossip topic ───────────────────────────────────────

    // Convert the bootstrap set to a Vec (the API expects this format).
    let bootstrap_peers: Vec<_> = ticket.bootstrap.iter().cloned().collect();

    // Subscribe to our chat topic. If we're joining, the bootstrap peers help
    // us discover other participants. If we're creating, the bootstrap list is
    // empty and we wait for others to connect to us.
    let topic = gossip
        .subscribe(ticket.topic_id, bootstrap_peers)
        .await?;

    // Split the topic handle into a sender (for broadcasting our messages)
    // and a receiver (for receiving messages from other peers).
    let (sender, mut receiver) = topic.split();

    // ── Set up the terminal for TUI rendering ───────────────────────────────

    // "Raw mode" disables line buffering and echo — we get each keypress
    // individually instead of waiting for Enter.
    enable_raw_mode()?;

    // "Alternate screen" switches to a separate terminal buffer, so when we
    // exit, the user's previous terminal content is restored.
    execute!(std::io::stdout(), EnterAlternateScreen)?;

    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    // ── Initialize application state ────────────────────────────────────────

    let our_id = endpoint.id();
    let mut app = App::new();

    // Add ourselves to the peer list so we appear in the sidebar.
    app.peers.insert(
        our_id,
        PeerInfo {
            name: format!("{nickname} (you)"),
            conn_type: ConnectionKind::Unknown,
        },
    );
    app.push_system_message(format!("ticket: {ticket_string}"));
    app.push_system_message("waiting for peers...");

    // Create the async event streams we'll multiplex in the event loop.
    let mut keyboard_events = EventStream::new();
    let mut ui_tick = interval(Duration::from_millis(50));

    // ── Event loop ──────────────────────────────────────────────────────────
    //
    // This is the heart of the program. `tokio::select!` waits on multiple
    // async operations simultaneously and runs whichever branch completes
    // first. The other branches are cancelled and retried on the next
    // iteration.
    //
    // Think of it like a "whoever raises their hand first" system:
    //   - Did the user press a key? → Handle the keypress.
    //   - Did a gossip message arrive? → Process the network event.
    //   - Did the 50ms timer tick? → Refresh connection types and redraw.

    loop {
        // Redraw the UI at the start of every iteration.
        terminal.draw(|frame| render_ui(frame, &app))?;

        tokio::select! {
            // ── Branch 1: Keyboard input from the user ──────────────────────
            keyboard_event = keyboard_events.next() => {
                // Unwrap the nested Option<Result<Event>>: the stream yields
                // `Some(Ok(event))` for valid keypresses.
                if let Some(Ok(TermEvent::Key(key))) = keyboard_event {
                    // Ignore key release/repeat events — we only care about presses.
                    if key.kind != KeyEventKind::Press { continue; }

                    // If the user pressed Enter on a non-empty input, broadcast it.
                    if let Some(text) = app.handle_key_press(key.code) {
                        let message = Message::Chat {
                            nickname: nickname.clone(),
                            text: text.clone(),
                        };
                        // Serialize to bytes and broadcast to all peers in the topic.
                        // `.into()` converts `Vec<u8>` into the `Bytes` type gossip expects.
                        let encoded = postcard::to_stdvec(&message)?;
                        sender.broadcast(encoded.into()).await?;
                        // Also add to our own message list (gossip doesn't echo back).
                        app.push_chat_message(nickname.clone(), text);
                    }
                }
            }

            // ── Branch 2: Network events from the gossip protocol ───────────
            gossip_event = receiver.try_next() => {
                match gossip_event {
                    // A peer sent us a message (chat or join announcement).
                    Ok(Some(GossipEvent::Received(msg))) => {
                        app.handle_gossip_message(&msg.content);
                    }
                    // A new peer joined the gossip topic.
                    Ok(Some(GossipEvent::NeighborUp(peer_id))) => {
                        app.handle_peer_connected(peer_id);
                        // Announce ourselves so the new peer learns our nickname.
                        let join_announcement = Message::Join {
                            nickname: nickname.clone(),
                            endpoint_id: our_id,
                        };
                        let encoded = postcard::to_stdvec(&join_announcement)?;
                        sender.broadcast(encoded.into()).await?;
                    }
                    // A peer left the gossip topic.
                    Ok(Some(GossipEvent::NeighborDown(peer_id))) => {
                        app.handle_peer_disconnected(peer_id);
                    }
                    // We fell behind processing messages — some were dropped.
                    Ok(Some(GossipEvent::Lagged)) => {
                        app.push_system_message("warning: gossip stream lagged");
                    }
                    // The gossip stream ended (shouldn't happen in normal operation).
                    Ok(None) => {
                        app.push_system_message("gossip stream closed");
                        app.should_quit = true;
                    }
                    // An error occurred reading from the gossip stream.
                    Err(e) => {
                        app.push_system_message(format!("gossip error: {e}"));
                    }
                }
            }

            // ── Branch 3: Periodic UI tick (every 50ms) ─────────────────────
            _ = ui_tick.tick() => {
                // Periodically refresh connection types for all peers. Connections
                // may upgrade from relay to direct over time as NAT traversal
                // succeeds, so we poll for changes.
                app.refresh_connection_types(our_id, &conn_tracker);
            }
        }

        if app.should_quit {
            break;
        }
    }

    // ── Restore terminal to normal mode ─────────────────────────────────────

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    // ── Graceful shutdown ───────────────────────────────────────────────────
    //
    // Shut down the protocol router first (stops accepting new connections),
    // then close the endpoint (terminates existing connections).

    router.shutdown().await?;
    endpoint.close().await;

    Ok(())
}
