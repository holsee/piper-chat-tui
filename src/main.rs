//! piper-chat — P2P terminal chat over iroh gossip.
//!
//! This is the crate root. It declares the module tree, defines the CLI, and
//! runs the main event loop that ties networking, input, and rendering together.
//!
//! ## Module structure
//!
//! - `net`        — Wire protocol, tickets, and connection tracking
//! - `welcome`    — Interactive welcome screen (room setup form)
//! - `chat`       — Chat UI state (`App`) and rendering (`ui()`)
//! - `transfer`   — File transfer state machine and file share pane
//! - `filepicker` — Modal file picker overlay

// ── Module declarations ─────────────────────────────────────────────────────
// `mod` declarations tell Rust to look for a file named `<name>.rs` (or
// `<name>/mod.rs`) in the `src/` directory and include it as a child module.
// Modules form a tree rooted at `main.rs` (for binaries) or `lib.rs` (for libraries).
mod chat;
mod filepicker;
mod net;
mod transfer;
mod welcome;

// ── Imports ─────────────────────────────────────────────────────────────────

// `PathBuf` is an owned, heap-allocated filesystem path. It's the `String`
// equivalent for paths — `Path` (a borrowed slice) is to `PathBuf` what
// `&str` is to `String`. Use `PathBuf` when you need to store or modify a path.
use std::path::PathBuf;

// `anyhow::Result` is a type alias for `Result<T, anyhow::Error>`. It lets
// you use `?` to propagate errors of any type that implements `std::error::Error`,
// without defining custom error enums for a small application.
use anyhow::Result;
// `clap::Parser` is a derive macro that generates a CLI argument parser from
// struct/enum definitions. It reads `#[arg(...)]` and `#[command(...)]` attributes
// to configure flags, subcommands, help text, etc.
use clap::Parser;
// Crossterm provides cross-platform terminal control:
// - `Event`/`EventStream`: async stream of keyboard, mouse, and resize events
// - `KeyCode`/`KeyEventKind`/`KeyModifiers`: key event details
// - `execute!`: writes terminal commands (like switching to alternate screen)
// - `enable_raw_mode`/`disable_raw_mode`: toggles between cooked mode (line-buffered,
//   with echo) and raw mode (immediate key delivery, no echo)
// - `EnterAlternateScreen`/`LeaveAlternateScreen`: uses the terminal's alternate
//   buffer so the original scrollback is preserved when the app exits
use crossterm::{
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
// `iroh_blobs` — content-addressed blob storage and streaming transfers:
// - `FsStore`: persists blobs to disk using the `redb` embedded database
// - `BlobsProtocol`: protocol handler that serves blobs to connecting peers
// - `Hash`: a BLAKE3 content hash — the universal identifier for blob content
// - `HashAndFormat`: combines a `Hash` with a format flag (raw bytes vs hash-seq)
// - `ALPN`: the Application-Layer Protocol Negotiation identifier for the blobs
//   protocol (tells QUIC which protocol handler should receive a connection)
use iroh_blobs::{store::fs::FsStore, BlobsProtocol, Hash, HashAndFormat, ALPN as BLOBS_ALPN};
// `iroh_gossip` — pub-sub messaging over iroh connections:
// - `GossipEvent`: events from a topic subscription (NeighborUp/Down, Received, etc.)
// - `Gossip`: the gossip protocol instance — manages subscriptions and message routing
// - `GOSSIP_ALPN`: the ALPN identifier for the gossip protocol
use iroh_gossip::{
    api::Event as GossipEvent,
    net::{Gossip, GOSSIP_ALPN},
};
// `Ticket` trait from iroh — provides `serialize()`/`deserialize()` for base32
// encoding. We use fully-qualified syntax `<ChatTicket as Ticket>::serialize()`
// because the method name could be ambiguous.
use iroh_tickets::Ticket;
// `StreamExt` is an *extension trait* — it adds the `.next()` method to async
// streams. In Rust, you must `use` an extension trait to call its methods,
// even though the trait isn't named explicitly at the call site.
use n0_future::StreamExt;
// `tokio::time` provides async-aware timers:
// - `Duration`: a span of time (e.g. 50ms)
// - `interval`: creates a recurring timer that yields on each tick
use tokio::time::{Duration, interval};

// Imports from our own crate modules — `use chat::App` brings `chat::App`
// into scope so we can write `App` instead of `chat::App`.
use chat::{ui, App, AppMode};
use filepicker::FilePickerResult;
use net::{ChatTicket, ConnTracker, ConnType, Message, PeerInfo};
use transfer::{FileOffer, TransferEvent, TransferState};
use welcome::{run_welcome_screen, WelcomeResult};

// ── CLI ──────────────────────────────────────────────────────────────────────

/// The top-level CLI struct. `#[derive(Parser)]` generates the argument parser.
///
/// `#[command(...)]` sets the binary name and description shown in `--help`.
///
/// The `command` field is `Option<Command>` — if no subcommand is provided
/// (the user just runs `piper-chat` with no args), it's `None`, and we fall
/// through to the interactive welcome screen.
#[derive(Parser)]
#[command(name = "piper-chat", about = "P2P terminal chat over iroh gossip")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands for the CLI. `#[derive(clap::Subcommand)]` generates the
/// subcommand parser. Each variant becomes a subcommand name (lowercase).
///
/// `#[arg(short, long)]` makes the field available as both `-n` and `--name`.
/// `///` doc comments above fields become the help text shown by `--help`.
#[derive(clap::Subcommand)]
enum Command {
    /// Create a new chat room
    Create {
        /// Your display name
        #[arg(short, long)]
        name: String,
    },
    /// Join an existing chat room
    Join {
        /// Your display name
        #[arg(short, long)]
        name: String,
        /// Ticket string from the room creator
        ticket: String,
    },
}

// ── Main ─────────────────────────────────────────────────────────────────────

/// `#[tokio::main]` is a procedural macro that transforms `async fn main()` into:
/// ```ignore
/// fn main() {
///     tokio::runtime::Builder::new_multi_thread()
///         .enable_all()
///         .build()
///         .unwrap()
///         .block_on(async { /* your async main body */ })
/// }
/// ```
/// This is necessary because Rust's `main()` must be synchronous — the macro
/// creates the tokio runtime and blocks on the async entry point.
#[tokio::main]
async fn main() -> Result<()> {
    // `Cli::parse()` reads `std::env::args()`, parses them according to the
    // `#[derive(Parser)]` attributes, and returns a `Cli` instance. If the
    // arguments are invalid, it prints an error and exits automatically.
    let cli = Cli::parse();

    // Determine the nickname and ticket based on the subcommand.
    // `match` on `Option<Command>` handles all three cases: Create, Join, or
    // no subcommand (interactive welcome screen).
    let (nickname, ticket) = match cli.command {
        Some(Command::Create { name }) => (name, ChatTicket::new_random()),
        Some(Command::Join { name, ticket }) => {
            // Fully-qualified trait method call: `<ChatTicket as Ticket>::deserialize()`
            // This syntax is needed when a type could implement multiple traits with
            // the same method name. Here it calls the `Ticket` trait's `deserialize`
            // which parses a base32 string back into a `ChatTicket`.
            let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
            (name, t)
        }
        // `None` — no subcommand provided, launch the interactive welcome screen.
        // The nested `match` handles the welcome screen's three outcomes:
        // create, join, or quit (user pressed Esc).
        None => match run_welcome_screen().await? {
            Some(WelcomeResult::Create { nickname }) => (nickname, ChatTicket::new_random()),
            Some(WelcomeResult::Join { nickname, ticket }) => {
                let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
                (nickname, t)
            }
            // User quit the welcome screen — exit cleanly.
            None => return Ok(()),
        },
    };

    // ── Networking ───────────────────────────────────────────────────────────

    // `ConnTracker` uses `Arc<RwLock<HashMap>>` internally for thread-safe
    // connection state tracking (see net.rs for details).
    let conn_tracker = ConnTracker::new();

    // Build the iroh endpoint using the builder pattern. The endpoint is our
    // network identity — it generates a keypair, listens for QUIC connections,
    // and manages hole-punching and relay fallback.
    //
    // `.alpns()` registers the Application-Layer Protocol Negotiation identifiers.
    // ALPN is a TLS extension that lets the client tell the server which protocol
    // it wants to speak. By registering both GOSSIP_ALPN and BLOBS_ALPN, our
    // endpoint can handle both gossip messages and blob transfers over the same
    // QUIC connection.
    //
    // `.hooks()` installs our connection tracker hook, which records connection
    // info after each QUIC handshake completes.
    //
    // `.bind()` is async — it binds a UDP socket and starts the endpoint.
    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![GOSSIP_ALPN.to_vec(), BLOBS_ALPN.to_vec()])
        .hooks(conn_tracker.hook())
        .bind()
        .await?;

    // Set up the blob store at a per-instance directory keyed by endpoint ID.
    // This avoids `redb` lock contention when multiple peers run on one machine.
    //
    // `dirs::data_dir()` returns an `Option<PathBuf>` — the platform's standard
    // data directory. `unwrap_or_else(|| ...)` provides a fallback (current dir)
    // if the platform doesn't have a data directory.
    //
    // `.join()` appends path segments using the platform's path separator.
    // `endpoint.id().fmt_short()` returns a short hex prefix for readability.
    let blob_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piper-chat")
        .join("blobs")
        .join(endpoint.id().fmt_short().to_string());
    // `FsStore::load()` opens (or creates) the redb database at the given path.
    // It's async because it may need to perform I/O to initialize the database.
    let blob_store = FsStore::load(&blob_dir).await?;

    // `Gossip::builder().spawn()` creates the gossip protocol instance and starts
    // its background task. It takes a clone of the endpoint because it needs to
    // open connections to peers for gossip message exchange.
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // Create the blobs protocol handler so peers can download blobs from us.
    // `BlobsProtocol` wraps the store and serves blob data over QUIC when a
    // peer connects with the BLOBS_ALPN identifier.
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);

    // The Router multiplexes multiple protocols over a single endpoint.
    // `.accept(ALPN, handler)` registers a protocol handler for a given ALPN.
    // When an incoming connection arrives, the router inspects the ALPN and
    // dispatches to the matching handler. `.spawn()` starts the router's
    // background accept loop.
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(BLOBS_ALPN, blobs_protocol)
        .spawn();

    // Build the ticket string to share with others. We clone the original
    // ticket and insert our own endpoint ID, so peers who receive the ticket
    // can bootstrap by connecting to us.
    let mut our_ticket = ticket.clone();
    our_ticket.bootstrap.insert(endpoint.id());
    let ticket_str = <ChatTicket as Ticket>::serialize(&our_ticket);

    // Subscribe to the gossip topic. `bootstrap` is the list of peers to
    // initially connect to (from the ticket). `subscribe()` returns a
    // `TopicHandle` which we `.split()` into a sender (for broadcasting)
    // and a receiver (an async stream of gossip events).
    let bootstrap: Vec<_> = ticket.bootstrap.iter().cloned().collect();
    let topic = gossip.subscribe(ticket.topic_id, bootstrap).await?;
    let (sender, mut receiver) = topic.split();

    // ── File transfer setup ─────────────────────────────────────────────────

    // Download directory for received files.
    let download_dir = PathBuf::from("./piper-files");
    // `tokio::fs::create_dir_all` is the async version of `std::fs::create_dir_all`.
    // It creates the directory and all missing parent directories. Using the tokio
    // version avoids blocking the async runtime on filesystem I/O.
    tokio::fs::create_dir_all(&download_dir).await?;
    // `canonicalize()` resolves the path to an absolute path, following symlinks.
    // This ensures the path is unambiguous regardless of later working directory changes.
    // Note: this is a `std::path::PathBuf` method (synchronous) — acceptable here
    // because it's a single metadata lookup, not a long-running operation.
    let download_dir = download_dir.canonicalize()?;

    // `tokio::sync::mpsc::channel` creates a bounded multi-producer, single-consumer
    // channel. Background download tasks (producers) send `TransferEvent`s to the
    // main event loop (consumer). The capacity of 64 provides backpressure — if the
    // main loop falls behind, senders will wait rather than using unbounded memory.
    let (transfer_tx, mut transfer_rx) = tokio::sync::mpsc::channel::<TransferEvent>(64);

    // ── Terminal setup ───────────────────────────────────────────────────────

    // `enable_raw_mode()` puts the terminal into raw mode:
    // - Keys are delivered immediately (no line buffering / waiting for Enter)
    // - Input is not echoed to the screen
    // - Special key combos (Ctrl+C, Ctrl+Z) are not intercepted by the terminal
    // This gives us full control over input handling and screen rendering.
    enable_raw_mode()?;
    // `execute!` is a crossterm macro that writes terminal commands to a writer.
    // `EnterAlternateScreen` switches to the terminal's alternate screen buffer,
    // preserving the user's original scrollback. When we `LeaveAlternateScreen`
    // later, the original terminal content is restored — the chat UI disappears.
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    // Create a ratatui `Terminal` backed by crossterm. The terminal manages a
    // double-buffer: widgets draw to a back buffer, then `draw()` diffs it against
    // the front buffer and emits only the changed cells — minimizing terminal I/O.
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    // `endpoint.id()` returns our `EndpointId` — a 32-byte Ed25519 public key
    // that uniquely identifies this node on the network.
    let our_id = endpoint.id();
    let mut app = App::new();
    // Add ourselves to the peers map with "(you)" suffix for the display name.
    app.peers.insert(
        our_id,
        PeerInfo {
            name: format!("{nickname} (you)"),
            conn_type: ConnType::Unknown,
        },
    );
    app.ticket(ticket_str);
    app.system("share the ticket above with others to join");
    app.system("type /help for commands | waiting for peers...");

    // `EventStream::new()` creates an async stream of crossterm terminal events.
    // It uses the "event-stream" feature we enabled in Cargo.toml, which wraps
    // crossterm's blocking `read()` in a tokio-compatible async stream.
    let mut events = EventStream::new();
    // `interval()` creates an async timer that yields at a fixed rate (50ms).
    // We use this to drive periodic UI redraws and connection type polling.
    let mut tick = interval(Duration::from_millis(50));

    // ── Event loop ───────────────────────────────────────────────────────────
    //
    // `tokio::select!` multiplexes multiple async operations into a single loop.
    // On each iteration, it races all branches and runs whichever completes first.
    // The other branches are *cancelled* (their futures are dropped). This is
    // Rust's cooperative concurrency model — no threads, no locks, just futures.

    loop {
        // `terminal.draw()` takes a closure that receives a `Frame` — a mutable
        // drawing surface for one frame. The closure builds the UI by placing
        // widgets at specific `Rect` positions. After the closure returns,
        // ratatui diffs the new buffer against the previous frame and emits
        // only the terminal escape sequences needed to update changed cells.
        terminal.draw(|f| ui(f, &app))?;

        tokio::select! {
            // ── Branch 1: Keyboard input ─────────────────────────────────
            // `events.next()` yields the next terminal event from the async stream.
            // The result is `Option<Result<Event>>` — None means the stream ended.
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = &ev {
                    // On Windows, crossterm sends both Press and Release events.
                    // We only care about Press events to avoid double-handling.
                    if key.kind != KeyEventKind::Press { continue; }

                    match app.mode {
                        // ── Chat mode ────────────────────────────────────
                        AppMode::Chat => {
                            match key.code {
                                KeyCode::Esc => app.should_quit = true,
                                KeyCode::Tab => {
                                    if app.transfers.has_entries() {
                                        app.focus_file_pane();
                                    }
                                }
                                // `key.modifiers.contains(KeyModifiers::CONTROL)` checks
                                // if the Ctrl key is held. `KeyModifiers` is a bitfield,
                                // so `.contains()` tests a specific bit flag.
                                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    app.open_file_picker();
                                }
                                KeyCode::Enter => {
                                    // `drain(..)` removes all characters from the String
                                    // and returns them as an iterator. `.collect()` gathers
                                    // them back into a new String. This efficiently moves
                                    // the input content out while leaving `app.input` empty.
                                    let text: String = app.input.drain(..).collect();
                                    app.cursor_pos = 0;
                                    if text.trim() == "/help" {
                                        show_help(&mut app);
                                    } else if text.trim() == "/send" {
                                        app.open_file_picker();
                                    } else if !text.is_empty() {
                                        let msg = Message::Chat {
                                            nickname: nickname.clone(),
                                            text: text.clone(),
                                        };
                                        // `postcard::to_stdvec()` serializes the message
                                        // into a compact binary `Vec<u8>`.
                                        let encoded = postcard::to_stdvec(&msg)?;
                                        // `.into()` converts `Vec<u8>` into `Bytes` — a
                                        // reference-counted byte buffer. `Bytes::from(Vec)`
                                        // is zero-copy: it takes ownership of the Vec's
                                        // allocation without copying. The `sender.broadcast()`
                                        // method expects `Bytes` because it may need to
                                        // clone the data for multiple recipients cheaply.
                                        sender.broadcast(encoded.into()).await?;
                                        app.chat(nickname.clone(), text);
                                    }
                                }
                                KeyCode::Backspace => {
                                    if app.cursor_pos > 0 {
                                        app.cursor_pos -= 1;
                                        app.input.remove(app.cursor_pos);
                                    }
                                }
                                KeyCode::Left => {
                                    // `saturating_sub(1)` subtracts 1 but clamps at 0
                                    // instead of panicking on unsigned underflow.
                                    app.cursor_pos = app.cursor_pos.saturating_sub(1);
                                }
                                KeyCode::Right => {
                                    if app.cursor_pos < app.input.len() {
                                        app.cursor_pos += 1;
                                    }
                                }
                                KeyCode::Char(c) => {
                                    // `String::insert()` inserts a character at a byte
                                    // index, shifting subsequent bytes right. O(n) but
                                    // fine for short chat input.
                                    app.input.insert(app.cursor_pos, c);
                                    app.cursor_pos += 1;
                                }
                                _ => {}
                            }
                        }

                        // ── File picker mode ─────────────────────────────
                        AppMode::FilePicker => {
                            // Reconstruct the `TermEvent` wrapper to pass to the
                            // ratatui-explorer widget, which expects a full `Event`.
                            let key_event = TermEvent::Key(*key);
                            if let Some(picker) = &mut app.file_picker {
                                match picker.handle(&key_event)? {
                                    FilePickerResult::Selected(path) => {
                                        app.close_file_picker();
                                        match share_file(
                                            &blob_store,
                                            &sender,
                                            &nickname,
                                            our_id,
                                            &path,
                                        ).await {
                                            Ok((hash, filename, size)) => {
                                                let offer = FileOffer {
                                                    sender_nickname: "You".to_string(),
                                                    sender_id: our_id,
                                                    filename: filename.clone(),
                                                    size,
                                                    hash,
                                                };
                                                app.transfers.add_sent(offer);
                                                app.system(format!("sharing: {filename}"));
                                            }
                                            Err(e) => {
                                                app.system(format!("failed to share file: {e}"));
                                            }
                                        }
                                    }
                                    FilePickerResult::Cancelled => {
                                        app.close_file_picker();
                                    }
                                    FilePickerResult::Browsing => {}
                                }
                            }
                        }

                        // ── File pane mode ───────────────────────────────
                        AppMode::FilePane => {
                            match key.code {
                                KeyCode::Tab | KeyCode::Esc => {
                                    app.focus_chat();
                                }
                                KeyCode::Up => {
                                    app.transfers.select_prev();
                                }
                                KeyCode::Down => {
                                    app.transfers.select_next();
                                }
                                KeyCode::Enter => {
                                    if let Some(entry) = app.transfers.selected_entry() {
                                        match &entry.state {
                                            TransferState::Pending => {
                                                let offer = entry.offer.clone();
                                                let hash = offer.hash;
                                                app.transfers.start_download(&hash);
                                                spawn_download(
                                                    &blob_store,
                                                    &endpoint,
                                                    offer,
                                                    download_dir.clone(),
                                                    transfer_tx.clone(),
                                                );
                                            }
                                            TransferState::Complete(path) => {
                                                // Open the folder containing the downloaded file.
                                                // `path.parent()` returns `Option<&Path>` — the
                                                // directory portion of the path. `unwrap_or()` falls
                                                // back to the download dir if the path has no parent.
                                                let dir = path.parent().unwrap_or(&download_dir);
                                                // `open::that()` opens the path with the OS default
                                                // handler — on Windows this launches Explorer, on
                                                // macOS it uses Finder, on Linux it uses xdg-open.
                                                // `let _ = ` discards the Result — we don't care if
                                                // the open fails (e.g. no GUI available).
                                                let _ = open::that(dir);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // ── Branch 2: Gossip network events ──────────────────────────
            // `receiver.try_next()` yields the next gossip event. The result is
            // `Result<Option<GossipEvent>>` — Ok(None) means the stream ended.
            msg = receiver.try_next() => {
                match msg {
                    Ok(Some(GossipEvent::Received(msg))) => {
                        // Deserialize the binary payload back into a `Message` enum.
                        // `postcard::from_bytes()` returns `Result<Message>` — if
                        // the bytes don't match any variant, we silently ignore them
                        // (forward compatibility with future message types).
                        match postcard::from_bytes(&msg.content) {
                            Ok(Message::Join { nickname: name, endpoint_id }) => {
                                app.system(format!("{name} joined"));
                                app.peers.insert(endpoint_id, PeerInfo {
                                    name,
                                    conn_type: ConnType::Unknown,
                                });
                            }
                            Ok(Message::Chat { nickname, text }) => {
                                app.chat(nickname, text);
                            }
                            Ok(Message::FileOffer { nickname: name, endpoint_id, filename, size, hash }) => {
                                // `Hash::from_bytes()` reconstructs the BLAKE3 hash
                                // from the raw [u8; 32] bytes sent over the wire.
                                let blob_hash = Hash::from_bytes(hash);
                                let offer = FileOffer {
                                    sender_nickname: name.clone(),
                                    sender_id: endpoint_id,
                                    filename: filename.clone(),
                                    size,
                                    hash: blob_hash,
                                };
                                app.transfers.add_offer(offer);
                                app.system(format!(
                                    "{name} shared: {filename} ({})",
                                    transfer::format_file_size(size)
                                ));
                            }
                            Err(_) => {}
                        }
                    }
                    // `NeighborUp` fires when a new peer joins the gossip topic.
                    // We add them to the peers map and broadcast our Join message
                    // so they learn our display name.
                    Ok(Some(GossipEvent::NeighborUp(id))) => {
                        app.peers.insert(id, PeerInfo {
                            name: id.fmt_short().to_string(),
                            conn_type: ConnType::Unknown,
                        });
                        app.system(format!("peer connected: {}", id.fmt_short()));
                        let join = Message::Join {
                            nickname: nickname.clone(),
                            endpoint_id: our_id,
                        };
                        let encoded = postcard::to_stdvec(&join)?;
                        sender.broadcast(encoded.into()).await?;
                    }
                    // `NeighborDown` fires when a peer disconnects from the topic.
                    // `.remove()` returns `Option<V>` — the value if the key existed.
                    // `.map(|p| p.name)` extracts the name from the PeerInfo.
                    // `.unwrap_or_else()` provides a fallback if the peer wasn't in our map.
                    Ok(Some(GossipEvent::NeighborDown(id))) => {
                        let name = app.peers.remove(&id)
                            .map(|p| p.name)
                            .unwrap_or_else(|| id.fmt_short().to_string());
                        app.system(format!("{name} left"));
                    }
                    // `Lagged` means we fell behind on processing gossip events and
                    // some messages were dropped. This happens if the event loop is
                    // too slow to keep up with incoming traffic.
                    Ok(Some(GossipEvent::Lagged)) => {
                        app.system("warning: gossip stream lagged");
                    }
                    Ok(None) => {
                        app.system("gossip stream closed");
                        app.should_quit = true;
                    }
                    Err(e) => {
                        app.system(format!("gossip error: {e}"));
                    }
                }
            }

            // ── Branch 3: Transfer events from background tasks ──────────
            // `transfer_rx.recv()` yields the next event from the mpsc channel.
            // `Some(event)` pattern: `recv()` returns `Option<T>` — None means
            // all senders have been dropped (no more background tasks).
            Some(event) = transfer_rx.recv() => {
                match event {
                    TransferEvent::Progress { hash, bytes_received, total_bytes } => {
                        app.transfers.update_progress(&hash, bytes_received, total_bytes);
                    }
                    TransferEvent::Complete { hash, filename, path } => {
                        app.transfers.complete_download(&hash, path);
                        app.system(format!("download complete: {filename}"));
                    }
                    TransferEvent::Failed { hash, filename, error } => {
                        app.transfers.fail_download(&hash, error.clone());
                        app.system(format!("download failed: {filename} — {error}"));
                    }
                }
            }

            // ── Branch 4: UI tick (50ms) ─────────────────────────────────
            // The tick branch fires every 50ms. We use it to poll connection
            // types — iroh may upgrade connections from relay to direct (via
            // UDP hole-punching) at any time, so we check periodically.
            _ = tick.tick() => {
                for (id, peer) in &mut app.peers {
                    if *id != our_id {
                        peer.conn_type = conn_tracker.conn_type(id);
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // ── Restore terminal ─────────────────────────────────────────────────────
    // These cleanup calls mirror the setup — we disable raw mode and leave the
    // alternate screen to restore the user's original terminal state.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    // ── Shutdown ─────────────────────────────────────────────────────────────
    // `router.shutdown()` gracefully stops accepting new connections and waits
    // for in-flight protocol handlers to finish. `endpoint.close()` shuts down
    // the QUIC endpoint and all its connections.
    router.shutdown().await?;
    endpoint.close().await;

    Ok(())
}

// ── Help ─────────────────────────────────────────────────────────────────────

/// Display help text as system messages.
fn show_help(app: &mut App) {
    app.system("── Commands ──────────────────────────────");
    app.system("  /help        Show this help");
    app.system("  /send        Open file picker to share a file");
    app.system("── Keys (chat) ───────────────────────────");
    app.system("  Enter        Send message");
    app.system("  Ctrl+F       Open file picker");
    app.system("  Tab          Focus file pane (when visible)");
    app.system("  Esc          Quit");
    app.system("── Keys (file pane) ──────────────────────");
    app.system("  Up/Down      Select file");
    app.system("  Enter        Download selected / open folder");
    app.system("  Tab/Esc      Return to chat");
    app.system("── Keys (file picker) ────────────────────");
    app.system("  Up/Down      Navigate files");
    app.system("  Left/Right   Parent / enter directory");
    app.system("  Enter        Select file to share");
    app.system("  Esc          Cancel");
    app.system("──────────────────────────────────────────");
}

// ── File sharing helpers ─────────────────────────────────────────────────────

/// Import a file into the blob store and broadcast a `FileOffer` over gossip.
///
/// Returns `(hash, filename, size)` on success.
///
/// This function demonstrates several Rust patterns:
/// - `&FsStore` / `&GossipSender`: borrowed references (we don't need ownership)
/// - `&str` for `nickname`: a borrowed string slice (cheaper than `&String`)
/// - `&std::path::Path` for `path`: a borrowed path slice (accepts both `&Path` and `&PathBuf`)
/// - `Result<(Hash, String, u64)>`: returns a tuple wrapped in Result for error propagation
async fn share_file(
    store: &FsStore,
    sender: &iroh_gossip::api::GossipSender,
    nickname: &str,
    endpoint_id: iroh::EndpointId,
    path: &std::path::Path,
) -> Result<(Hash, String, u64)> {
    // `file_name()` returns `Option<&OsStr>` — the last component of the path.
    // `to_string_lossy()` converts `OsStr` to a `Cow<str>`, replacing invalid
    // Unicode with the replacement character. `.to_string()` converts to owned.
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    // `tokio::fs::metadata()` is the async version of `std::fs::metadata()`.
    // `.len()` returns the file size in bytes.
    let size = tokio::fs::metadata(path).await?.len();

    // Import the file into the blob store. `add_path` returns an `AddProgress`
    // that implements `IntoFuture` — awaiting it gives us a `TagInfo` with the
    // BLAKE3 hash. The blob store computes the hash as it reads the file,
    // verifying integrity. The file content is now addressable by hash.
    let tag_info = store.blobs().add_path(path).await?;
    let hash = tag_info.hash;

    // Broadcast the file offer to all peers via gossip.
    // `*hash.as_bytes()` dereferences the `&[u8; 32]` to get the owned `[u8; 32]`
    // array. We send raw bytes instead of the `Hash` type because `Hash` doesn't
    // derive `Serialize` for postcard — raw bytes are more portable.
    let msg = Message::FileOffer {
        nickname: nickname.to_string(),
        endpoint_id,
        filename: filename.clone(),
        size,
        hash: *hash.as_bytes(),
    };
    let encoded = postcard::to_stdvec(&msg)?;
    sender.broadcast(encoded.into()).await?;

    Ok((hash, filename, size))
}

/// Spawn a background task that downloads a blob from a remote peer and exports
/// it to the download directory. Progress/completion/failure is reported via
/// the `tx` channel.
///
/// `tokio::spawn()` launches a new asynchronous task — like a lightweight green
/// thread. The task runs concurrently with the main event loop. We use this for
/// downloads because they're long-running and shouldn't block the UI.
///
/// The function takes owned/cloned values (not references) because `tokio::spawn`
/// requires the future to be `'static` — it can't borrow from the caller's stack
/// since it runs independently. We clone `store` and `endpoint` (both are cheap
/// Arc-based clones) to satisfy this requirement.
fn spawn_download(
    store: &FsStore,
    endpoint: &iroh::Endpoint,
    offer: FileOffer,
    download_dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<TransferEvent>,
) {
    // Clone `store` and `endpoint` so the spawned future owns its data.
    // These types use `Arc` internally, so cloning is O(1) — it just
    // increments a reference count, not deep-copying the data.
    let store = store.clone();
    let endpoint = endpoint.clone();

    // `tokio::spawn` takes a future and returns a `JoinHandle`. We don't
    // store the handle — this is a "fire-and-forget" pattern. The task will
    // run until completion (or until the runtime shuts down).
    // The `async move` block takes ownership of all captured variables
    // (`store`, `endpoint`, `offer`, etc.) via the `move` keyword.
    tokio::spawn(async move {
        let hash = offer.hash;
        let filename = offer.filename.clone();
        let target = download_dir.join(&filename);

        // Connect to the sender's endpoint for the blobs protocol.
        // `endpoint.connect()` establishes a QUIC connection to the given
        // peer, using BLOBS_ALPN to indicate we want to speak the blobs protocol.
        let conn = match endpoint.connect(offer.sender_id, BLOBS_ALPN).await {
            Ok(conn) => conn,
            Err(e) => {
                // `let _ = tx.send(...)` discards the send result. The channel
                // might be closed if the main loop has already exited — that's
                // fine, we just silently drop the error notification.
                let _ = tx
                    .send(TransferEvent::Failed {
                        hash,
                        filename,
                        error: format!("connect: {e}"),
                    })
                    .await;
                return;
            }
        };

        // Fetch the blob using iroh-blobs' verified streaming download.
        // `HashAndFormat::raw(hash)` specifies we want a raw blob (not a hash
        // sequence / collection). The "raw" format means the hash directly
        // corresponds to the file content, verified chunk-by-chunk during download.
        // `.stream()` returns an async stream of `GetProgressItem` events.
        let content = HashAndFormat::raw(hash);
        let mut progress_stream = store.remote().fetch(conn, content).stream();

        // Consume the progress stream. Each item is either a progress update,
        // completion notification, or error.
        while let Some(item) = progress_stream.next().await {
            match item {
                iroh_blobs::api::remote::GetProgressItem::Progress(bytes) => {
                    let _ = tx
                        .send(TransferEvent::Progress {
                            hash,
                            bytes_received: bytes,
                            total_bytes: offer.size,
                        })
                        .await;
                }
                iroh_blobs::api::remote::GetProgressItem::Done(_stats) => {
                    // Blob downloaded into store — read it out and write to disk.
                    // We use `get_bytes()` instead of `export()` because export
                    // requires the entry to be in `Complete` state, which may not
                    // be the case immediately after a fetch finishes.
                    //
                    // `get_bytes()` returns `Bytes` — a cheaply-clonable byte buffer.
                    match store.blobs().get_bytes(hash).await {
                        Ok(data) => {
                            // `tokio::fs::write()` is the async version of `std::fs::write()`.
                            // It creates the file (or truncates if it exists) and writes
                            // all bytes atomically.
                            match tokio::fs::write(&target, &data).await {
                                Ok(_) => {
                                    let _ = tx
                                        .send(TransferEvent::Complete {
                                            hash,
                                            filename: filename.clone(),
                                            path: target.clone(),
                                        })
                                        .await;
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(TransferEvent::Failed {
                                            hash,
                                            filename: filename.clone(),
                                            error: format!("write file: {e}"),
                                        })
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(TransferEvent::Failed {
                                    hash,
                                    filename: filename.clone(),
                                    error: format!("read blob: {e}"),
                                })
                                .await;
                        }
                    }
                    return;
                }
                iroh_blobs::api::remote::GetProgressItem::Error(e) => {
                    let _ = tx
                        .send(TransferEvent::Failed {
                            hash,
                            filename: filename.clone(),
                            error: format!("download: {e}"),
                        })
                        .await;
                    return;
                }
            }
        }
    });
}
