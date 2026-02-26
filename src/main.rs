//! piper-chat — P2P terminal chat over iroh gossip.
//!
//! This is the crate root. It declares the module tree, defines the CLI, and
//! runs the main event loop that ties networking, input, and rendering together.
//!
//! ## Module structure
//!
//! - `net`     — Wire protocol, tickets, and connection tracking
//! - `welcome` — Interactive welcome screen (room setup form)
//! - `chat`    — Chat UI state (`App`) and rendering (`ui()`)

// ── Module declarations ─────────────────────────────────────────────────────
//
// `mod name;` tells the compiler to look for `src/name.rs` (or `src/name/mod.rs`)
// and include it as a child module. This is Rust's explicit module system —
// files aren't automatically part of the crate; you must declare them.
//
// Modules form a tree rooted at `main.rs` (for binaries) or `lib.rs` (for
// libraries). Items are private by default; `pub` makes them visible to the
// parent module and beyond.
mod chat;
mod net;
mod welcome;

// ── Imports ─────────────────────────────────────────────────────────────────
//
// `use` brings items into scope so we can refer to them by short name.
// Items from external crates (declared in Cargo.toml) are imported by
// crate name. Items from our own modules use `crate::module::item` or,
// since main.rs is the crate root, just `module::item`.

// `anyhow::Result` is `Result<T, anyhow::Error>` — a catch-all error type
// that can hold any error. Great for applications (as opposed to libraries,
// which typically define specific error types).
use anyhow::Result;
// `clap::Parser` is a derive macro that generates a CLI argument parser from
// struct/enum definitions. `#[derive(Parser)]` on a struct auto-implements
// the `Parser` trait, giving it a `.parse()` class method.
use clap::Parser;
use crossterm::{
    // Crossterm events for keyboard input. `EventStream` provides an async
    // stream of terminal events (keys, mouse, resize). We rename `Event` to
    // `TermEvent` to avoid name collision with gossip events.
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh_gossip::{
    // `GossipEvent` variants tell us about network activity: messages received,
    // peers joining/leaving, stream lag, etc.
    api::Event as GossipEvent,
    // `Gossip` is the gossip protocol handle; `GOSSIP_ALPN` is the QUIC
    // Application-Layer Protocol Negotiation string that identifies gossip
    // connections during the TLS handshake.
    net::{Gossip, GOSSIP_ALPN},
};
// The `Ticket` trait provides `serialize()` / `deserialize()` for base32
// encoding. We import the trait to call these methods on `ChatTicket`.
use iroh_tickets::Ticket;
// `StreamExt` adds `.next()` and `.try_next()` to async streams. Without
// this import, `.try_next()` on the gossip receiver wouldn't compile —
// Rust requires extension traits to be in scope to use their methods.
use n0_future::StreamExt;
use tokio::time::{Duration, interval};

// Imports from our own modules. These types are `pub` in their respective
// modules; main.rs consumes them to wire everything together.
use chat::{ui, App};
use net::{ChatTicket, ConnTracker, ConnType, Message, PeerInfo};
use welcome::{run_welcome_screen, WelcomeResult};

// ── CLI ──────────────────────────────────────────────────────────────────────
//
// Clap's derive API turns Rust structs into full-featured CLI parsers.
// `#[derive(Parser)]` generates the argument parsing code at compile time.

/// Top-level CLI structure.
///
/// `#[command(...)]` attributes configure the help text and binary name.
/// `Option<Command>` makes the subcommand optional — when omitted, we launch
/// the interactive welcome screen instead.
#[derive(Parser)]
#[command(name = "piper-chat", about = "P2P terminal chat over iroh gossip")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Subcommands for creating or joining a room from the command line.
///
/// `#[derive(clap::Subcommand)]` generates `create` and `join` subcommands.
/// Doc comments (`///`) become the help text shown by `--help`.
/// `#[arg(short, long)]` makes `-n` and `--name` both work.
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

/// Entry point. `#[tokio::main]` is a macro that sets up the tokio async
/// runtime and wraps `main()` in a `block_on()` call. Without it, `async fn
/// main()` wouldn't work — Rust has no built-in async runtime; you must pick
/// one (tokio, async-std, smol, etc.).
///
/// `-> Result<()>` means main can return errors. If `main` returns `Err(e)`,
/// the process exits with a non-zero code and prints the error. The `?`
/// operator throughout propagates errors to this return type.
#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments. `Cli::parse()` reads `std::env::args()`, validates
    // them against our struct definition, and returns a populated `Cli` or
    // exits with a help/error message.
    let cli = Cli::parse();

    // Determine nickname and ticket from either CLI args or the welcome screen.
    //
    // This `match` demonstrates *destructuring*: `Some(Command::Create { name })`
    // reaches into nested enums/structs and binds inner fields to local variables
    // in one expression. Rust's pattern matching is one of its most powerful features.
    let (nickname, ticket) = match cli.command {
        Some(Command::Create { name }) => (name, ChatTicket::new_random()),
        Some(Command::Join { name, ticket }) => {
            // Fully-qualified syntax to call the `Ticket` trait's `deserialize` method.
            // `?` propagates the error if the ticket string is invalid.
            let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
            (name, t)
        }
        // `None` means no subcommand was given — launch the interactive welcome screen.
        None => {
            match run_welcome_screen().await? {
                Some(WelcomeResult::Create { nickname }) => {
                    (nickname, ChatTicket::new_random())
                }
                Some(WelcomeResult::Join { nickname, ticket }) => {
                    let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
                    (nickname, t)
                }
                None => return Ok(()), // User pressed Esc to quit
            }
        }
    };

    // ── Networking ───────────────────────────────────────────────────────────
    //
    // Set up the iroh networking stack: endpoint → gossip → router.
    // This follows the iroh "protocol stack" pattern where each layer wraps
    // the one below.

    // `ConnTracker` records connection metadata (direct vs relayed) for each peer.
    let conn_tracker = ConnTracker::new();

    // Build an iroh `Endpoint` — this is the QUIC transport layer.
    // `.alpns()` registers which protocols this endpoint speaks.
    // `.hooks()` installs our connection tracker to intercept handshakes.
    // `.bind()` is async — it opens the UDP socket and starts listening.
    // `.await?` suspends until binding completes, propagating errors with `?`.
    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .hooks(conn_tracker.hook())
        .bind()
        .await?;

    // Create the gossip protocol layer on top of the endpoint.
    // `.spawn()` starts a background task that manages gossip state.
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // The router dispatches incoming connections to the right protocol handler
    // based on the ALPN negotiated during the TLS handshake.
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // Build a shareable ticket that includes our own endpoint ID so others
    // can connect to us. `mut` allows us to modify the cloned ticket.
    let mut our_ticket = ticket.clone();
    our_ticket.bootstrap.insert(endpoint.id());
    let ticket_str = <ChatTicket as Ticket>::serialize(&our_ticket);

    // Subscribe to the gossip topic (chat room). `bootstrap` is the list of
    // known peers to connect to initially. `.split()` gives us separate
    // sender and receiver handles — a common pattern for duplex channels.
    let bootstrap: Vec<_> = ticket.bootstrap.iter().cloned().collect();
    let topic = gossip.subscribe(ticket.topic_id, bootstrap).await?;
    let (sender, mut receiver) = topic.split();

    // ── Terminal setup ───────────────────────────────────────────────────────

    // Enable raw mode (no line buffering, no echo) and switch to the alternate
    // screen buffer so the chat UI doesn't clobber the user's scrollback.
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let our_id = endpoint.id();
    let mut app = App::new();
    // Insert ourselves in the peers list so we appear in the sidebar.
    app.peers.insert(
        our_id,
        PeerInfo {
            name: format!("{nickname} (you)"),
            conn_type: ConnType::Unknown,
        },
    );
    app.ticket(ticket_str);
    app.system("share the ticket above with others to join");
    app.system("waiting for peers...");

    // `EventStream` wraps crossterm's synchronous event polling into an async stream.
    let mut events = EventStream::new();
    // A 50ms tick interval drives UI redraws — even when no input arrives,
    // we still update the display (e.g. connection type changes from the tracker).
    let mut tick = interval(Duration::from_millis(50));

    // ── Event loop ───────────────────────────────────────────────────────────
    //
    // This is the heart of the application. `tokio::select!` multiplexes three
    // async event sources into a single loop:
    // 1. Terminal keyboard events (user input)
    // 2. Gossip network events (messages, peer joins/leaves)
    // 3. Timer ticks (UI refresh + connection status polling)
    //
    // Only one branch runs per iteration — whichever future resolves first.

    loop {
        // Redraw the entire UI from current state. This is ratatui's
        // "immediate mode" model — no retained widget tree, just rebuild
        // every frame. The closure borrows `app` immutably for rendering.
        terminal.draw(|f| ui(f, &app))?;

        tokio::select! {
            // ── Branch 1: Keyboard input ─────────────────────────────────
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = ev {
                    // Filter out release/repeat events (Windows sends both)
                    if key.kind != KeyEventKind::Press { continue; }
                    match key.code {
                        KeyCode::Esc => app.should_quit = true,
                        KeyCode::Enter => {
                            // `.drain(..)` empties the string and returns its
                            // contents as an iterator. `.collect()` gathers them
                            // back into a new String. This transfers ownership of
                            // the character data without reallocating.
                            let text: String = app.input.drain(..).collect();
                            app.cursor_pos = 0;
                            if !text.is_empty() {
                                let msg = Message::Chat {
                                    nickname: nickname.clone(),
                                    text: text.clone(),
                                };
                                // Serialize the message with postcard and broadcast
                                // to all peers on the gossip topic.
                                // `.into()` converts `Vec<u8>` to `Bytes` (zero-copy).
                                let encoded = postcard::to_stdvec(&msg)?;
                                sender.broadcast(encoded.into()).await?;
                                // Also display locally (gossip doesn't echo back)
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
                            app.cursor_pos = app.cursor_pos.saturating_sub(1);
                        }
                        KeyCode::Right => {
                            if app.cursor_pos < app.input.len() {
                                app.cursor_pos += 1;
                            }
                        }
                        KeyCode::Char(c) => {
                            app.input.insert(app.cursor_pos, c);
                            app.cursor_pos += 1;
                        }
                        _ => {}
                    }
                }
            }

            // ── Branch 2: Gossip network events ──────────────────────────
            //
            // `.try_next()` returns `Result<Option<Event>>`:
            // - `Ok(Some(event))` — received an event
            // - `Ok(None)`        — stream closed (no more events)
            // - `Err(e)`          — stream error
            msg = receiver.try_next() => {
                match msg {
                    Ok(Some(GossipEvent::Received(msg))) => {
                        // Deserialize the incoming message. `postcard::from_bytes`
                        // returns our `Message` enum — the variant tag is encoded
                        // in the first byte(s).
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
                            // Silently ignore malformed messages
                            Err(_) => {}
                        }
                    }
                    // A new peer connected to the gossip topic
                    Ok(Some(GossipEvent::NeighborUp(id))) => {
                        app.peers.insert(id, PeerInfo {
                            name: id.fmt_short().to_string(),
                            conn_type: ConnType::Unknown,
                        });
                        app.system(format!("peer connected: {}", id.fmt_short()));
                        // Announce ourselves so the new peer learns our display name.
                        // Without this, they'd only see our short endpoint ID.
                        let join = Message::Join {
                            nickname: nickname.clone(),
                            endpoint_id: our_id,
                        };
                        let encoded = postcard::to_stdvec(&join)?;
                        sender.broadcast(encoded.into()).await?;
                    }
                    // A peer disconnected from the gossip topic
                    Ok(Some(GossipEvent::NeighborDown(id))) => {
                        // `.remove()` returns `Option<V>` — we use it to get the
                        // peer's display name for the departure message.
                        // `.unwrap_or_else()` provides a fallback closure that's
                        // only called if the Option is None.
                        let name = app.peers.remove(&id)
                            .map(|p| p.name)
                            .unwrap_or_else(|| id.fmt_short().to_string());
                        app.system(format!("{name} left"));
                    }
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

            // ── Branch 3: UI tick (50ms) ─────────────────────────────────
            //
            // On each tick, poll the connection tracker to update peer connection
            // types (direct vs relayed). This catches changes that happen
            // asynchronously as QUIC path probing discovers direct routes.
            _ = tick.tick() => {
                for (id, peer) in &mut app.peers {
                    // Don't look up our own connection type (we're always local)
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
    //
    // Disable raw mode and leave the alternate screen so the user's original
    // terminal contents reappear. This must happen before process exit.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    // ── Shutdown ─────────────────────────────────────────────────────────────
    //
    // Gracefully shut down the networking stack. `router.shutdown()` stops
    // accepting new connections; `endpoint.close()` closes the QUIC socket.
    // Both are async and may take a moment to drain in-flight data.
    router.shutdown().await?;
    endpoint.close().await;

    Ok(())
}
