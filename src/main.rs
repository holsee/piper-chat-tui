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
mod chat;
mod filepicker;
mod net;
mod transfer;
mod welcome;

// ── Imports ─────────────────────────────────────────────────────────────────

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh_blobs::{store::fs::FsStore, BlobsProtocol, Hash, HashAndFormat, ALPN as BLOBS_ALPN};
use iroh_gossip::{
    api::Event as GossipEvent,
    net::{Gossip, GOSSIP_ALPN},
};
use iroh_tickets::Ticket;
use n0_future::StreamExt;
use tokio::time::{Duration, interval};

use chat::{ui, App, AppMode};
use filepicker::FilePickerResult;
use net::{ChatTicket, ConnTracker, ConnType, Message, PeerInfo};
use transfer::{FileOffer, TransferEvent, TransferState};
use welcome::{run_welcome_screen, WelcomeResult};

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "piper-chat", about = "P2P terminal chat over iroh gossip")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let (nickname, ticket) = match cli.command {
        Some(Command::Create { name }) => (name, ChatTicket::new_random()),
        Some(Command::Join { name, ticket }) => {
            let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
            (name, t)
        }
        None => match run_welcome_screen().await? {
            Some(WelcomeResult::Create { nickname }) => (nickname, ChatTicket::new_random()),
            Some(WelcomeResult::Join { nickname, ticket }) => {
                let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
                (nickname, t)
            }
            None => return Ok(()),
        },
    };

    // ── Networking ───────────────────────────────────────────────────────────

    let conn_tracker = ConnTracker::new();

    // Build the endpoint first — we need its unique ID before creating the
    // blob store so each instance gets its own database (avoids file lock
    // contention when running multiple instances on the same machine).
    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![GOSSIP_ALPN.to_vec(), BLOBS_ALPN.to_vec()])
        .hooks(conn_tracker.hook())
        .bind()
        .await?;

    // Set up the blob store at a per-instance directory keyed by endpoint ID.
    // This avoids redb lock contention when multiple peers run on one machine.
    let blob_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piper-chat")
        .join("blobs")
        .join(endpoint.id().fmt_short().to_string());
    let blob_store = FsStore::load(&blob_dir).await?;

    let gossip = Gossip::builder().spawn(endpoint.clone());

    // Create the blobs protocol handler so peers can download blobs from us.
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);

    // Register both gossip and blobs with the router.
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(BLOBS_ALPN, blobs_protocol)
        .spawn();

    let mut our_ticket = ticket.clone();
    our_ticket.bootstrap.insert(endpoint.id());
    let ticket_str = <ChatTicket as Ticket>::serialize(&our_ticket);

    let bootstrap: Vec<_> = ticket.bootstrap.iter().cloned().collect();
    let topic = gossip.subscribe(ticket.topic_id, bootstrap).await?;
    let (sender, mut receiver) = topic.split();

    // ── File transfer setup ─────────────────────────────────────────────────

    // Download directory for received files.
    let download_dir = PathBuf::from("./piper-files");
    tokio::fs::create_dir_all(&download_dir).await?;
    let download_dir = download_dir.canonicalize()?;

    // Channel for background download tasks to report progress/completion.
    let (transfer_tx, mut transfer_rx) = tokio::sync::mpsc::channel::<TransferEvent>(64);

    // ── Terminal setup ───────────────────────────────────────────────────────

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let our_id = endpoint.id();
    let mut app = App::new();
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

    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(50));

    // ── Event loop ───────────────────────────────────────────────────────────

    loop {
        terminal.draw(|f| ui(f, &app))?;

        tokio::select! {
            // ── Branch 1: Keyboard input ─────────────────────────────────
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = &ev {
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
                                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    app.open_file_picker();
                                }
                                KeyCode::Enter => {
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
                                        let encoded = postcard::to_stdvec(&msg)?;
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

                        // ── File picker mode ─────────────────────────────
                        AppMode::FilePicker => {
                            // Reconstruct the TermEvent to pass to the explorer widget.
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
                                                let dir = path.parent().unwrap_or(&download_dir);
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
            msg = receiver.try_next() => {
                match msg {
                    Ok(Some(GossipEvent::Received(msg))) => {
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
                    Ok(Some(GossipEvent::NeighborDown(id))) => {
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

            // ── Branch 3: Transfer events from background tasks ──────────
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
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    // ── Shutdown ─────────────────────────────────────────────────────────────
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
async fn share_file(
    store: &FsStore,
    sender: &iroh_gossip::api::GossipSender,
    nickname: &str,
    endpoint_id: iroh::EndpointId,
    path: &std::path::Path,
) -> Result<(Hash, String, u64)> {
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    let size = tokio::fs::metadata(path).await?.len();

    // Import the file into the blob store. `add_path` returns an `AddProgress`
    // that implements `IntoFuture` — awaiting it gives us a `TagInfo` with the hash.
    let tag_info = store.blobs().add_path(path).await?;
    let hash = tag_info.hash;

    // Broadcast the file offer to all peers via gossip.
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
fn spawn_download(
    store: &FsStore,
    endpoint: &iroh::Endpoint,
    offer: FileOffer,
    download_dir: PathBuf,
    tx: tokio::sync::mpsc::Sender<TransferEvent>,
) {
    let store = store.clone();
    let endpoint = endpoint.clone();

    tokio::spawn(async move {
        let hash = offer.hash;
        let filename = offer.filename.clone();
        let target = download_dir.join(&filename);

        // Connect to the sender's endpoint for the blobs protocol.
        let conn = match endpoint.connect(offer.sender_id, BLOBS_ALPN).await {
            Ok(conn) => conn,
            Err(e) => {
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
        // `remote().fetch()` returns a `GetProgress` stream.
        let content = HashAndFormat::raw(hash);
        let mut progress_stream = store.remote().fetch(conn, content).stream();

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
                    match store.blobs().get_bytes(hash).await {
                        Ok(data) => {
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
