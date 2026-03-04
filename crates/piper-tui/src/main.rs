//! piper-chat — P2P terminal chat TUI, powered by piper-core session API.

mod app;
mod filepicker;
mod net;
mod theme;
mod transfer;
mod ui;
mod welcome;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event as TermEvent, EventStream, KeyCode,
        KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh_tickets::Ticket;
use n0_future::StreamExt;
use tokio::time::{Duration, interval};

use app::{App, AppMode, ClickAction};
use filepicker::FilePickerResult;
use piper_core::protocol::{ChatTicket, ConnType, PeerInfo};
use piper_core::session::{SessionCommand, SessionConfig, SessionEvent, start_session};
use piper_core::transfer::TransferState;
use piper_core::util::format_file_size;
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

    // Start the session
    let mut handle = start_session(SessionConfig {
        nickname: nickname.clone(),
        ticket,
    })
    .await?;

    // ── Terminal setup ───────────────────────────────────────────────────

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let mut app = App::new();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(50));

    // ── Event loop ───────────────────────────────────────────────────────

    loop {
        terminal.draw(|f| ui::ui(f, &mut app))?;

        tokio::select! {
            // ── Keyboard / mouse input ──────────────────────────────────
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = &ev {
                    if key.kind != KeyEventKind::Press { continue; }

                    match app.mode {
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
                                KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    app.theme.toggle();
                                }
                                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    copy_ticket_to_clipboard(&mut app);
                                }
                                KeyCode::Enter => {
                                    let text: String = app.input.drain(..).collect();
                                    app.cursor_pos = 0;
                                    if text.trim() == "/help" {
                                        show_help(&mut app);
                                    } else if text.trim() == "/send" {
                                        app.pending_send_target = None;
                                        app.open_file_picker();
                                    } else if text.trim().starts_with("/sendto ") {
                                        let target_name = text.trim().strip_prefix("/sendto ").unwrap().trim().to_string();
                                        if target_name.is_empty() {
                                            app.system("usage: /sendto <nickname>");
                                        } else if app.peers.values().any(|p| p.name == target_name) {
                                            app.pending_send_target = Some(target_name);
                                            app.open_file_picker();
                                        } else {
                                            app.system(format!("unknown peer: {target_name}"));
                                        }
                                    } else if !text.is_empty() {
                                        let _ = handle.cmd_tx.send(SessionCommand::SendChat { text }).await;
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

                        AppMode::FilePicker => {
                            let key_event = TermEvent::Key(*key);
                            if let Some(picker) = &mut app.file_picker {
                                match picker.handle(&key_event)? {
                                    FilePickerResult::Selected(path) => {
                                        let send_target = app.pending_send_target.take();
                                        app.close_file_picker();
                                        let _ = handle.cmd_tx.send(SessionCommand::ShareFile {
                                            path,
                                            target: send_target,
                                        }).await;
                                    }
                                    FilePickerResult::Cancelled => {
                                        app.pending_send_target = None;
                                        app.close_file_picker();
                                    }
                                    FilePickerResult::Browsing => {}
                                }
                            }
                        }

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
                                                let hash = entry.offer.hash;
                                                let _ = handle.cmd_tx.send(SessionCommand::StartDownload { hash }).await;
                                            }
                                            TransferState::Complete(path) => {
                                                let dir = path.parent().unwrap_or(std::path::Path::new("."));
                                                let _ = open::that(dir);
                                            }
                                            TransferState::Sharing => {
                                                let hash = entry.offer.hash;
                                                let _ = handle.cmd_tx.send(SessionCommand::UnshareFile { hash }).await;
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

                // ── Mouse events ────────────────────────────────────────
                if let Some(Ok(TermEvent::Mouse(mouse))) = &ev {
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            handle_mouse_click(&mut app, mouse.column, mouse.row, &handle.cmd_tx).await;
                        }
                        MouseEventKind::ScrollUp => {
                            app.scroll_offset = app.scroll_offset.saturating_add(3);
                        }
                        MouseEventKind::ScrollDown => {
                            app.scroll_offset = app.scroll_offset.saturating_sub(3);
                        }
                        _ => {}
                    }
                }
            }

            // ── Session events ──────────────────────────────────────────
            event = handle.event_rx.recv() => {
                match event {
                    Some(SessionEvent::Ready { our_id, ticket_str }) => {
                        app.peers.insert(our_id, PeerInfo {
                            name: format!("{nickname} (you)"),
                            conn_type: ConnType::You,
                        });
                        app.ticket(ticket_str.clone());
                        app.ticket_str = Some(ticket_str);
                        app.system("share the ticket above with others to join");
                        app.system("type /help for commands | waiting for peers...");
                    }
                    Some(SessionEvent::PeerJoined { endpoint_id, nickname: name }) => {
                        app.peers.insert(endpoint_id, PeerInfo {
                            name: name.clone(),
                            conn_type: ConnType::Unknown,
                        });
                        app.system(format!("{name} joined"));
                    }
                    Some(SessionEvent::PeerLeft { nickname: name, endpoint_id }) => {
                        app.peers.remove(&endpoint_id);
                        app.system(format!("{name} left"));
                    }
                    Some(SessionEvent::ChatReceived { nickname: name, text, timestamp_ms, .. }) => {
                        app.chat(name, text, timestamp_ms);
                    }
                    Some(SessionEvent::ChatSent { nickname: name, text, timestamp_ms, .. }) => {
                        app.chat(name, text, timestamp_ms);
                    }
                    Some(SessionEvent::FileOffered { offer, target, .. }) => {
                        let target_label = target
                            .as_ref()
                            .map(|_| " (with you)".to_string())
                            .unwrap_or_default();
                        let name = offer.sender_nickname.clone();
                        let filename = offer.filename.clone();
                        let size = offer.size;
                        app.transfers.add_offer(offer);
                        app.system(format!(
                            "{name} shared{target_label}: {filename} ({})",
                            format_file_size(size)
                        ));
                    }
                    Some(SessionEvent::FileRetracted { nickname: name, hash, filename }) => {
                        // Session already retracted from its internal TransferManager.
                        // We need to retract from our local TUI TransferManager too.
                        let fname = filename.or_else(|| app.transfers.retract(&hash));
                        if let Some(fname) = fname {
                            app.system(format!("{name} unshared: {fname}"));
                        } else {
                            app.transfers.retract(&hash);
                        }
                    }
                    Some(SessionEvent::FileShared { offer, target }) => {
                        let target_label = target
                            .as_ref()
                            .map(|t| format!(" (to {t})"))
                            .unwrap_or_default();
                        let filename = offer.filename.clone();
                        app.transfers.add_sent(offer);
                        app.system(format!("sharing{target_label}: {filename}"));
                    }
                    Some(SessionEvent::FileShareFailed { error }) => {
                        app.system(format!("failed to share file: {error}"));
                    }
                    Some(SessionEvent::TransferProgress { hash, bytes_received, total_bytes }) => {
                        app.transfers.update_progress(&hash, bytes_received, total_bytes);
                    }
                    Some(SessionEvent::TransferComplete { hash, filename, path }) => {
                        app.transfers.complete_download(&hash, path);
                        app.system(format!("download complete: {filename}"));
                    }
                    Some(SessionEvent::TransferFailed { hash, filename, error }) => {
                        app.transfers.fail_download(&hash, error.clone());
                        app.system(format!("download failed: {filename} — {error}"));
                    }
                    Some(SessionEvent::ConnTypeChanged { endpoint_id, conn_type }) => {
                        if let Some(peer) = app.peers.get_mut(&endpoint_id) {
                            peer.conn_type = conn_type;
                        }
                    }
                    Some(SessionEvent::HistorySynced { entries, merged_count }) => {
                        let mut historical: Vec<app::ChatLine> = Vec::new();
                        for entry in &entries {
                            match &entry.kind {
                                piper_core::protocol::HistoryEntryKind::Chat { nickname: nick, text } => {
                                    historical.push(app::ChatLine::Chat {
                                        nickname: nick.clone(),
                                        text: text.clone(),
                                        timestamp_ms: entry.timestamp_ms,
                                    });
                                }
                                piper_core::protocol::HistoryEntryKind::FileOffer {
                                    nickname: nick,
                                    filename,
                                    size,
                                    ..
                                } => {
                                    historical.push(app::ChatLine::System(format!(
                                        "{nick} shared: {filename} ({})",
                                        format_file_size(*size)
                                    )));
                                }
                                piper_core::protocol::HistoryEntryKind::FileRetract { .. } => {}
                                piper_core::protocol::HistoryEntryKind::System(text) => {
                                    historical.push(app::ChatLine::System(text.clone()));
                                }
                            }
                        }
                        historical.append(&mut app.messages);
                        app.messages = historical;
                        app.system(format!("history sync complete: {merged_count} new messages"));
                    }
                    Some(SessionEvent::System { message }) => {
                        app.system(message);
                    }
                    Some(SessionEvent::Disconnected { reason }) => {
                        app.system(reason);
                        app.should_quit = true;
                    }
                    None => {
                        app.should_quit = true;
                    }
                }
            }

            // ── UI tick ─────────────────────────────────────────────────
            _ = tick.tick() => {}
        }

        if app.should_quit {
            let _ = handle.cmd_tx.send(SessionCommand::Quit).await;
            break;
        }
    }

    // ── Restore terminal ─────────────────────────────────────────────────
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;

    handle.join().await;

    Ok(())
}

// ── Help ─────────────────────────────────────────────────────────────────────

fn show_help(app: &mut App) {
    app.system("── Commands ──────────────────────────────");
    app.system("  /help           Show this help");
    app.system("  /send           Open file picker to share a file");
    app.system("  /sendto <name>  Send a file to a specific peer");
    app.system("── Keys (chat) ───────────────────────────");
    app.system("  Enter        Send message");
    app.system("  Ctrl+F       Open file picker");
    app.system("  Ctrl+T       Toggle dark/light theme");
    app.system("  Ctrl+Y       Copy invite ticket to clipboard");
    app.system("  Tab          Focus file pane (when visible)");
    app.system("  Esc          Quit");
    app.system("── Keys (file pane) ──────────────────────");
    app.system("  Up/Down      Select entry");
    app.system("  Enter        Download / open folder / unshare");
    app.system("  Tab/Esc      Return to chat");
    app.system("── Keys (file picker) ────────────────────");
    app.system("  Up/Down      Navigate files");
    app.system("  Left/Right   Parent / enter directory");
    app.system("  Enter        Select file to share");
    app.system("  Esc          Cancel");
    app.system("── Mouse ─────────────────────────────────");
    app.system("  Click        Focus pane / trigger action");
    app.system("  Scroll       Scroll messages up/down");
    app.system("──────────────────────────────────────────");
}

// ── Mouse handling ───────────────────────────────────────────────────────────

async fn handle_mouse_click(
    app: &mut App,
    col: u16,
    row: u16,
    cmd_tx: &tokio::sync::mpsc::Sender<SessionCommand>,
) {
    for region in app.click_regions.iter().rev() {
        if col >= region.rect.x
            && col < region.rect.x + region.rect.width
            && row >= region.rect.y
            && row < region.rect.y + region.rect.height
        {
            match &region.action {
                ClickAction::FocusChat => {
                    app.focus_chat();
                }
                ClickAction::FocusFilePane => {
                    app.focus_file_pane();
                }
                ClickAction::CopyTicket => {
                    copy_ticket_to_clipboard(app);
                }
                ClickAction::DownloadTransfer(hash) => {
                    let hash = *hash;
                    let _ = cmd_tx.send(SessionCommand::StartDownload { hash }).await;
                }
                ClickAction::OpenTransfer(hash) => {
                    if let Some(entry) = app
                        .transfers
                        .entries
                        .iter()
                        .find(|e| e.offer.hash == *hash)
                        && let TransferState::Complete(path) = &entry.state
                    {
                        let dir = path.parent().unwrap_or(std::path::Path::new("."));
                        let _ = open::that(dir);
                    }
                }
                ClickAction::UnshareTransfer(hash) => {
                    let hash = *hash;
                    let _ = cmd_tx.send(SessionCommand::UnshareFile { hash }).await;
                }
            }
            break;
        }
    }
}

// ── Clipboard helpers ────────────────────────────────────────────────────────

fn copy_ticket_to_clipboard(app: &mut App) {
    use base64::Engine;
    if let Some(ref ticket) = app.ticket_str {
        let b64 = base64::engine::general_purpose::STANDARD.encode(ticket.as_bytes());
        let osc = format!("\x1b]52;c;{b64}\x07");
        let _ = std::io::Write::write_all(&mut std::io::stdout(), osc.as_bytes());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        app.copy_feedback_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
    }
}
