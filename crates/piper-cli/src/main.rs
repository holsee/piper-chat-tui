//! piper-cli — headless P2P chat client (stdin/stdout).

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};

use piper_core::protocol::{ChatTicket, HistoryEntryKind};
use piper_core::session::{SessionCommand, SessionConfig, SessionEvent, start_session};
use piper_core::util::format_file_size;

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "piper-cli", about = "Headless P2P chat over iroh gossip")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
        Command::Create { name } => (name, ChatTicket::new_random()),
        Command::Join { name, ticket } => {
            let parsed = <ChatTicket as iroh_tickets::Ticket>::deserialize(&ticket)
                .map_err(|e| anyhow::anyhow!("invalid ticket: {e}"))?;
            (name, parsed)
        }
    };

    let handle = start_session(SessionConfig {
        nickname,
        ticket,
    })
    .await?;

    let cmd_tx = handle.cmd_tx.clone();

    // Spawn stdin reader
    let stdin_tx = cmd_tx.clone();
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if line == "/quit" || line == "/q" {
                let _ = stdin_tx.send(SessionCommand::Quit).await;
                break;
            }
            if let Some(path) = line.strip_prefix("/send ") {
                let path = path.trim().to_string();
                let _ = stdin_tx
                    .send(SessionCommand::ShareFile {
                        path: PathBuf::from(path),
                        target: None,
                    })
                    .await;
                continue;
            }
            let _ = stdin_tx
                .send(SessionCommand::SendChat { text: line })
                .await;
        }
    });

    // Event loop
    let mut handle = handle;
    while let Some(event) = handle.event_rx.recv().await {
        match event {
            SessionEvent::Ready { ticket_str, .. } => {
                println!("[system] Session ready. Ticket: {ticket_str}");
            }
            SessionEvent::PeerJoined { nickname, .. } => {
                println!("[system] {nickname} joined");
            }
            SessionEvent::PeerLeft { nickname, .. } => {
                println!("[system] {nickname} left");
            }
            SessionEvent::ChatReceived {
                nickname, text, ..
            } => {
                println!("{nickname}: {text}");
            }
            SessionEvent::ChatSent {
                nickname, text, ..
            } => {
                println!("{nickname}: {text}");
            }
            SessionEvent::FileOffered { offer, target, .. } => {
                let size = format_file_size(offer.size);
                let target_str = target
                    .map(|t| format!(" (to {t})"))
                    .unwrap_or_default();
                println!(
                    "[system] {} shared {} ({size}){target_str}",
                    offer.sender_nickname, offer.filename
                );
            }
            SessionEvent::FileRetracted {
                nickname,
                filename,
                ..
            } => {
                let name = filename.as_deref().unwrap_or("unknown");
                println!("[system] {nickname} unshared {name}");
            }
            SessionEvent::FileShared { offer, target } => {
                let target_str = target
                    .map(|t| format!(" (to {t})"))
                    .unwrap_or_default();
                println!("[system] Sharing {}{target_str}", offer.filename);
            }
            SessionEvent::FileShareFailed { error } => {
                println!("[error] Failed to share file: {error}");
            }
            SessionEvent::TransferProgress {
                bytes_received,
                total_bytes,
                ..
            } => {
                let pct = if total_bytes > 0 {
                    (bytes_received * 100) / total_bytes
                } else {
                    0
                };
                eprint!("\r[download] {pct}%");
            }
            SessionEvent::TransferComplete { filename, path, .. } => {
                eprintln!();
                println!("[system] Downloaded {filename} → {}", path.display());
            }
            SessionEvent::TransferFailed {
                filename, error, ..
            } => {
                eprintln!();
                println!("[error] Download of {filename} failed: {error}");
            }
            SessionEvent::ConnTypeChanged { .. } => {}
            SessionEvent::HistorySynced {
                merged_count,
                entries,
                ..
            } => {
                if merged_count > 0 {
                    println!("[system] Synced {merged_count} history entries");
                    for entry in &entries {
                        match &entry.kind {
                            HistoryEntryKind::Chat { nickname, text } => {
                                println!("  {nickname}: {text}");
                            }
                            HistoryEntryKind::FileOffer {
                                nickname,
                                filename,
                                size,
                                ..
                            } => {
                                let size = format_file_size(*size);
                                println!("  {nickname} shared {filename} ({size})");
                            }
                            _ => {}
                        }
                    }
                }
            }
            SessionEvent::System { message } => {
                println!("[system] {message}");
            }
            SessionEvent::Disconnected { reason } => {
                println!("[system] Disconnected: {reason}");
                break;
            }
        }
    }

    handle.join().await;
    Ok(())
}
