//! Chat application state and terminal UI rendering.
//!
//! This module owns the `App` struct (the "model" in an MVC sense) and the
//! `ui()` function that renders it into a ratatui terminal frame. The event
//! loop in `main.rs` mutates `App` then calls `ui()` on each tick.

// `BTreeMap` is an ordered map backed by a B-tree. Unlike `HashMap`, it keeps
// keys sorted — so the peers panel always displays peers in a consistent
// (deterministic) order based on their `EndpointId`.
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use crate::net::{HistoryEntry, HistoryEntryKind, MessageId};

// `EndpointId` is a 32-byte public key that uniquely identifies each iroh node.
use iroh::EndpointId;
// Ratatui types for building terminal UIs:
// - `Layout` / `Constraint`: split the terminal into regions (vertical/horizontal)
// - `Style` / `Color` / `Modifier`: text styling (foreground, bold, italic, etc.)
// - `Line` / `Span`: styled text primitives — a `Line` is a row of `Span`s
// - `Block` / `Borders` / `Paragraph`: widget types for bordered text panels
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

// Import types from our sibling modules.
// `crate::` refers to the crate root (main.rs) — from there, Rust resolves the
// module path. `FilePicker` is the modal overlay widget, `ConnType`/`PeerInfo`
// are network types, and `TransferManager` manages file transfer state.
use ratatui::layout::Rect;

use crate::filepicker::FilePicker;
use crate::net::{ConnType, PeerInfo};
use crate::theme::Theme;
use crate::transfer::{self, TransferManager};

// ── App state ────────────────────────────────────────────────────────────────
//
// The `App` struct is the single source of truth for the chat session.
// It follows the "immediate mode" UI pattern: mutate state → redraw everything.

/// Which UI element currently has keyboard focus.
///
/// This enum implements a **focus management pattern**: the current mode
/// determines which widget receives keyboard input. `main.rs` matches on
/// `app.mode` to dispatch key events to the correct handler. This is simpler
/// than a focus stack or tree because we only have three focusable areas.
pub enum AppMode {
    /// Normal chat input mode.
    Chat,
    /// The modal file picker overlay is open.
    FilePicker,
    /// The file share pane has focus (navigate with Up/Down, Enter to act).
    FilePane,
}

/// A clickable region tracked by `ui()` for mouse interaction.
pub struct ClickRegion {
    pub rect: Rect,
    pub action: ClickAction,
}

/// Action triggered when the user clicks a `ClickRegion`.
pub enum ClickAction {
    FocusChat,
    FocusFilePane,
    CopyTicket,
    DownloadTransfer(iroh_blobs::Hash),
    OpenTransfer(iroh_blobs::Hash),
    UnshareTransfer(iroh_blobs::Hash),
}

/// A single line in the chat message log.
///
/// This enum demonstrates Rust's *algebraic data types*. Each variant can hold
/// different data:
/// - `System(String)` is a *tuple variant* — it wraps a single unnamed value.
/// - `Chat { nickname, text }` is a *struct variant* — it has named fields.
///
/// Pattern matching on this enum (in `ui()`) forces you to handle all variants
/// at compile time — the compiler won't let you forget one (exhaustiveness checking).
pub enum ChatLine {
    /// System notification (e.g. "peer connected", "waiting for peers...")
    System(String),
    /// The room's shareable ticket string, displayed prominently
    Ticket(String),
    /// A chat message from a peer, with their display name
    Chat {
        nickname: String,
        text: String,
        timestamp_ms: u64,
    },
}

/// The main application state for the chat session.
///
/// All fields are `pub` because `main.rs` reads and writes them directly
/// (e.g. `app.input.drain(..)`, `app.peers.insert(...)`). In a larger app
/// you'd use getter/setter methods for encapsulation, but for a small TUI app
/// direct field access is simpler and more idiomatic.
///
/// `BTreeMap<EndpointId, PeerInfo>` maps each peer's cryptographic ID to their
/// display info. We use `BTreeMap` (not `HashMap`) so the peers sidebar renders
/// in a stable order — `BTreeMap` iterates keys in sorted order.
pub struct App {
    /// All chat messages and system notifications, in chronological order.
    pub messages: Vec<ChatLine>,
    /// The current text being typed by the user (not yet sent).
    pub input: String,
    /// Cursor position within `input`, measured in bytes (safe because we only
    /// insert ASCII-range characters one at a time from keyboard input).
    pub cursor_pos: usize,
    /// Set to `true` when the user presses Esc — the event loop checks this
    /// after each iteration and breaks if true.
    pub should_quit: bool,
    /// Connected peers keyed by their endpoint ID.
    pub peers: BTreeMap<EndpointId, PeerInfo>,
    /// Which UI element currently has keyboard focus.
    pub mode: AppMode,
    /// The modal file picker (present only while the overlay is open).
    /// `Option<FilePicker>` is Rust's null-safe pattern — `None` means the
    /// picker is closed, `Some(picker)` means it's open. No null pointers.
    pub file_picker: Option<FilePicker>,
    /// All file transfer entries (sent and received).
    pub transfers: TransferManager,
    /// The active color theme (dark or light), toggled with Ctrl+T.
    pub theme: Theme,
    /// Serializable history log for sync with new peers.
    pub history: Vec<HistoryEntry>,
    /// O(1) dedup set of message IDs already seen.
    pub seen_ids: HashSet<MessageId>,
    /// Whether we have already received a history sync from another peer.
    pub history_synced: bool,
    /// Scroll offset for the messages pane (0 = auto-scroll to bottom).
    pub scroll_offset: u16,
    /// Clickable regions populated each frame by `ui()`.
    pub click_regions: Vec<ClickRegion>,
    /// The room's ticket string, stored for clipboard copy.
    pub ticket_str: Option<String>,
    /// When set, the copy button shows "Copied!" until this instant.
    pub copy_feedback_until: Option<Instant>,
    /// When set, the next file picker selection will send a targeted offer
    /// to this nickname instead of broadcasting to all peers.
    pub pending_send_target: Option<String>,
}

/// The `impl` block contains methods associated with the `App` type.
///
/// In Rust, methods are defined in `impl` blocks rather than inside the struct
/// definition. This separates data layout from behavior. You can have multiple
/// `impl` blocks for the same type (useful for organizing code or conditional
/// compilation).
impl App {
    /// Create a new empty application state.
    ///
    /// `Vec::new()`, `String::new()`, and `BTreeMap::new()` all allocate nothing
    /// until the first element is added — Rust collections are lazy about
    /// allocation.
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            should_quit: false,
            peers: BTreeMap::new(),
            mode: AppMode::Chat,
            file_picker: None,
            transfers: TransferManager::new(),
            theme: Theme::dark(),
            history: Vec::new(),
            seen_ids: HashSet::new(),
            history_synced: false,
            scroll_offset: 0,
            click_regions: Vec::new(),
            ticket_str: None,
            copy_feedback_until: None,
            pending_send_target: None,
        }
    }

    /// Open the modal file picker overlay.
    ///
    /// `if let Ok(picker) = FilePicker::new()` is a *refutable pattern* — it
    /// tries to construct the picker and only sets it if construction succeeded.
    /// If the current directory is unreadable, the picker silently fails to open
    /// (a more robust app would show an error message).
    pub fn open_file_picker(&mut self) {
        if let Ok(picker) = FilePicker::new(&self.theme) {
            self.file_picker = Some(picker);
            self.mode = AppMode::FilePicker;
        }
    }

    /// Close the file picker overlay and return to chat mode.
    ///
    /// Setting `file_picker` to `None` drops the `FilePicker` value — Rust's
    /// deterministic destruction (RAII) ensures any resources it holds are freed.
    pub fn close_file_picker(&mut self) {
        self.file_picker = None;
        self.mode = AppMode::Chat;
    }

    /// Move focus to the file share pane.
    pub fn focus_file_pane(&mut self) {
        self.mode = AppMode::FilePane;
    }

    /// Return focus to chat input.
    pub fn focus_chat(&mut self) {
        self.mode = AppMode::Chat;
    }

    /// Append a system notification to the message log.
    ///
    /// `impl Into<String>` is a *trait bound* on the parameter — it means
    /// "accept any type that can be converted into a String". This lets callers
    /// pass `&str`, `String`, `Cow<str>`, etc. without explicit conversion.
    /// The `.into()` call performs the conversion.
    ///
    /// `&mut self` means this method borrows `self` mutably — only one mutable
    /// reference can exist at a time (Rust's core borrow-checking rule).
    pub fn system(&mut self, msg: impl Into<String>) {
        self.messages.push(ChatLine::System(msg.into()));
    }

    /// Append a ticket display line to the message log.
    pub fn ticket(&mut self, ticket: impl Into<String>) {
        self.messages.push(ChatLine::Ticket(ticket.into()));
    }

    /// Append a chat message to the message log and history.
    pub fn chat(
        &mut self,
        nickname: String,
        text: String,
        message_id: MessageId,
        timestamp_ms: u64,
    ) {
        self.seen_ids.insert(message_id);
        self.messages.push(ChatLine::Chat {
            nickname: nickname.clone(),
            text: text.clone(),
            timestamp_ms,
        });
        self.push_history(HistoryEntry {
            message_id,
            timestamp_ms,
            kind: HistoryEntryKind::Chat { nickname, text },
        });
    }


    /// Push a history entry, capping at 1000 entries.
    pub fn push_history(&mut self, entry: HistoryEntry) {
        self.history.push(entry);
        if self.history.len() > 1000 {
            self.history.remove(0);
        }
    }
}

// ── UI ───────────────────────────────────────────────────────────────────────
//
// Ratatui uses an "immediate mode" rendering model: every frame, we build up
// the entire UI from scratch based on current state. No retained widget tree,
// no diffing — just draw what the state says. This is simple and fast for TUIs.

/// Format a unix timestamp (ms) as `HH:MM` UTC.
fn format_timestamp(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) as i64;
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    format!("{hours:02}:{minutes:02}")
}

/// Render the chat UI into a terminal frame.
///
/// Takes `&mut App` because it rebuilds `click_regions` each frame.
///
/// `ratatui::Frame` is a mutable drawing surface for one frame. It provides
/// `render_widget()` to place widgets at specific screen rectangles, and
/// `set_cursor_position()` to show the blinking cursor.
pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    // Clear click regions — they're rebuilt every frame.
    app.click_regions.clear();
    // Paint the full-screen background so the theme bg covers the terminal area.
    let bg_block = Block::default().style(Style::default().bg(app.theme.bg));
    f.render_widget(bg_block, f.area());

    // Build the vertical layout — conditionally include the file pane row when
    // there are active offers/transfers. This demonstrates ratatui's `Layout`
    // system: you specify constraints (Min, Length, Percentage) and the layout
    // engine computes the actual pixel dimensions. `split()` returns a `Vec<Rect>`.
    let rows = if app.transfers.has_entries() {
        // Dynamic height: number of entries + 2 for the border, capped at 8.
        let file_pane_height = (app.transfers.entries.len() as u16 + 2).min(8);
        Layout::vertical([
            Constraint::Min(1),                    // Messages pane (fills remaining space)
            Constraint::Length(file_pane_height),   // File pane (fixed height)
            Constraint::Length(3),                  // Input bar (3 rows: border + text + border)
        ])
        .split(f.area())
    } else {
        // No file transfers — just messages and input.
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area())
    };
    // Split the top row into left (messages, flexible) and right (peers, 24 cols).
    // `Layout::horizontal` works the same as vertical but splits left-to-right.
    let top = Layout::horizontal([Constraint::Min(1), Constraint::Length(24)]).split(rows[0]);

    // ── Messages pane (top left) ─────────────────────────────────────────

    // Transform each `ChatLine` into a styled ratatui `Line` using iterators.
    //
    // `.iter()` borrows each element; `.map()` transforms it; `.collect()`
    // gathers results into a `Vec<Line>`. This is Rust's iterator chain
    // pattern — lazy evaluation, zero allocation overhead (the compiler fuses
    // the iterator chain into a single loop).
    let theme = &app.theme;
    let mut lines: Vec<Line> = Vec::new();
    for msg in &app.messages {
        match msg {
            ChatLine::System(text) => {
                lines.push(Line::from(Span::styled(
                    format!("[system] {text}"),
                    Style::default()
                        .fg(theme.text_dim)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
            ChatLine::Ticket(ticket) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        "Ticket: ",
                        Style::default()
                            .fg(theme.ticket_label)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(ticket.as_str(), Style::default().fg(theme.ticket_value)),
                ]));
            }
            ChatLine::Chat {
                nickname,
                text,
                timestamp_ms,
            } => {
                let ts = format_timestamp(*timestamp_ms);
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{ts} "),
                        Style::default().fg(theme.timestamp),
                    ),
                    Span::styled(
                        nickname.as_str(),
                        Style::default()
                            .fg(theme.nickname)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(": {text}"), Style::default().fg(theme.text)),
                ]));
            }
        }
    }

    // Auto-scroll: calculate how many lines to skip so the newest messages
    // are always visible. `saturating_sub` returns 0 instead of underflowing.
    // `scroll_offset` allows manual scrollback via mouse wheel.
    let visible = top[0].height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible) as u16;
    // Clamp scroll_offset so it can't exceed actual content overflow.
    // Without this, scrolling up past the top accumulates "dead" offset
    // that makes scrolling back down feel unresponsive.
    app.scroll_offset = app.scroll_offset.min(max_scroll);
    let scroll = max_scroll - app.scroll_offset;

    let mut msg_block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.bg))
        .border_style(Style::default().fg(theme.border))
        .title("piper-chat")
        .title_style(Style::default().fg(theme.title));
    if app.scroll_offset > 0 {
        msg_block = msg_block.title_bottom(
            Line::from(Span::styled(
                format!(" ↑ {}/{} ", app.scroll_offset, max_scroll),
                Style::default().fg(theme.accent),
            ))
            .alignment(Alignment::Right),
        );
    }
    let messages_widget = Paragraph::new(lines)
        .scroll((scroll, 0))
        .block(msg_block);
    f.render_widget(messages_widget, top[0]);

    // Register click region for messages pane → focus chat (lower priority).
    app.click_regions.push(ClickRegion {
        rect: top[0],
        action: ClickAction::FocusChat,
    });

    // ── Peers pane (top right) ───────────────────────────────────────────

    // Split the peers area: peer list on top, copy-ticket button on bottom.
    let show_copy_btn = app.ticket_str.is_some();
    let btn_height = if show_copy_btn { 3 } else { 0 };
    let peers_split = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(btn_height),
    ])
    .split(top[1]);

    // `.values()` iterates only over the `PeerInfo` values in the BTreeMap,
    // skipping the keys. The `match` on `peer.conn_type` maps each connection
    // type to a display tag and color.
    // Sort peers so the local user (ConnType::You) appears first, then
    // all other peers in their existing BTreeMap order.
    let mut sorted_peers: Vec<&PeerInfo> = app.peers.values().collect();
    sorted_peers.sort_by_key(|p| !matches!(p.conn_type, ConnType::You));
    let peer_lines: Vec<Line> = sorted_peers
        .iter()
        .map(|peer| {
            let (tag, tag_color) = match peer.conn_type {
                ConnType::Direct => ("[direct]", theme.conn_direct),
                ConnType::Relay => ("[relay]", theme.conn_relay),
                ConnType::Unknown => ("[?]", theme.conn_unknown),
                ConnType::You => ("[you]", theme.conn_you),
            };
            Line::from(vec![
                Span::styled(format!("{tag} "), Style::default().fg(tag_color)),
                Span::styled(peer.name.as_str(), Style::default().fg(theme.peer_name)),
            ])
        })
        .collect();
    let peers_widget = Paragraph::new(peer_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg))
            .border_style(Style::default().fg(theme.border))
            .title("peers")
            .title_style(Style::default().fg(theme.title)),
    );
    f.render_widget(peers_widget, peers_split[0]);

    // Render the copy-ticket button below the peer list.
    if show_copy_btn {
        let is_feedback = app
            .copy_feedback_until
            .is_some_and(|t| t > Instant::now());
        let (label, style) = if is_feedback {
            (
                " Copied! ",
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                " Copy Ticket (Ctrl+Y) ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
        };
        let btn = Paragraph::new(Line::from(Span::styled(label, style)))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.bg))
                    .border_style(Style::default().fg(theme.accent)),
            );
        f.render_widget(btn, peers_split[1]);

        app.click_regions.push(ClickRegion {
            rect: peers_split[1],
            action: ClickAction::CopyTicket,
        });
    }

    // ── Input pane (bottom, full width) ──────────────────────────────────

    // The input row index depends on whether the file pane is visible.
    // With file pane: rows = [messages, files, input] → input is index 2.
    // Without:        rows = [messages, input]        → input is index 1.
    let input_row = if app.transfers.has_entries() { 2 } else { 1 };
    // `matches!(app.mode, AppMode::Chat)` is a macro that returns `true` if
    // the expression matches the pattern. It's more concise than a `match`
    // block when you just need a boolean. The input border is cyan when
    // focused (Chat mode) and white otherwise, providing a visual focus indicator.
    let input_border_color = if matches!(app.mode, AppMode::Chat) {
        theme.border_focused
    } else {
        theme.border
    };
    let input_widget = Paragraph::new(Line::from(vec![
        Span::styled("> ", Style::default().fg(theme.input_prompt)),
        Span::styled(&app.input, Style::default().fg(theme.text)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.bg))
            .border_style(Style::default().fg(input_border_color)),
    );
    f.render_widget(input_widget, rows[input_row]);

    // Register click region for input bar → focus chat.
    app.click_regions.push(ClickRegion {
        rect: rows[input_row],
        action: ClickAction::FocusChat,
    });

    // Place the terminal cursor at the user's typing position.
    // `x + 2` accounts for the border (1) and the "> " prompt prefix (1 for ">").
    // Wait — actually it's: border(1) + ">" (1) + space is included in the +2.
    // `y + 1` accounts for the top border.
    f.set_cursor_position((
        rows[input_row].x + 2 + app.cursor_pos as u16,
        rows[input_row].y + 1,
    ));

    // ── File share pane (between messages and input) ─────────────────

    if app.transfers.has_entries() {
        let focused = matches!(app.mode, AppMode::FilePane);
        transfer::render_file_pane(f, rows[1], &app.transfers, focused, theme);

        // Register per-entry click regions for download/open actions.
        // Inner area is the pane area minus the 1-cell border on each side.
        let inner = Rect {
            x: rows[1].x + 1,
            y: rows[1].y + 1,
            width: rows[1].width.saturating_sub(2),
            height: rows[1].height.saturating_sub(2),
        };
        for (i, entry) in app.transfers.entries.iter().enumerate() {
            let row_y = inner.y + i as u16;
            if row_y >= inner.y + inner.height {
                break;
            }
            let entry_rect = Rect {
                x: inner.x,
                y: row_y,
                width: inner.width,
                height: 1,
            };
            let action = match &entry.state {
                transfer::TransferState::Pending => {
                    Some(ClickAction::DownloadTransfer(entry.offer.hash))
                }
                transfer::TransferState::Complete(_) => {
                    Some(ClickAction::OpenTransfer(entry.offer.hash))
                }
                transfer::TransferState::Sharing => {
                    Some(ClickAction::UnshareTransfer(entry.offer.hash))
                }
                _ => None,
            };
            if let Some(action) = action {
                app.click_regions.push(ClickRegion {
                    rect: entry_rect,
                    action,
                });
            }
        }

        // Register click region for file pane → focus file pane (lower priority).
        app.click_regions.push(ClickRegion {
            rect: rows[1],
            action: ClickAction::FocusFilePane,
        });
    }

    // ── File picker overlay (on top of everything) ───────────────────

    // `if let Some(picker) = &app.file_picker` unwraps the Option — if the
    // file picker is open (`Some`), we render it on top of everything else.
    // Because this is rendered *last*, it visually overlays the chat UI.
    if let Some(picker) = &app.file_picker {
        picker.render(f);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `App::new()` starts with empty state.
    #[test]
    fn app_starts_empty() {
        let app = App::new();
        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
        assert!(!app.should_quit);
        assert!(app.peers.is_empty());
    }

    /// Test the `system()` helper pushes a `ChatLine::System`.
    #[test]
    fn app_system_message() {
        let mut app = App::new();
        app.system("hello");
        assert_eq!(app.messages.len(), 1);
        // Use `matches!` macro for concise enum variant checking.
        assert!(matches!(&app.messages[0], ChatLine::System(s) if s == "hello"));
    }

    /// Test the `ticket()` helper pushes a `ChatLine::Ticket`.
    #[test]
    fn app_ticket_message() {
        let mut app = App::new();
        app.ticket("abc123");
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(&app.messages[0], ChatLine::Ticket(s) if s == "abc123"));
    }

    /// Test the `chat()` helper pushes a `ChatLine::Chat`.
    #[test]
    fn app_chat_message() {
        let mut app = App::new();
        let mid = crate::net::new_message_id();
        app.chat("Alice".into(), "hey there".into(), mid, 1700000000000);
        assert_eq!(app.messages.len(), 1);
        assert!(
            matches!(&app.messages[0], ChatLine::Chat { nickname, text, .. } if nickname == "Alice" && text == "hey there")
        );
        // Also check it was recorded in history.
        assert_eq!(app.history.len(), 1);
        assert!(app.seen_ids.contains(&mid));
    }

    /// Verify that `system()` accepts both `&str` and `String` (via `impl Into<String>`).
    #[test]
    fn app_system_accepts_string_and_str() {
        let mut app = App::new();
        app.system("a &str");
        app.system(String::from("a String"));
        assert_eq!(app.messages.len(), 2);
    }

    /// Test that multiple message types accumulate in order.
    #[test]
    fn app_messages_accumulate_in_order() {
        let mut app = App::new();
        app.system("first");
        app.chat(
            "Bob".into(),
            "second".into(),
            crate::net::new_message_id(),
            1000,
        );
        app.ticket("third");
        assert_eq!(app.messages.len(), 3);
        assert!(matches!(&app.messages[0], ChatLine::System(_)));
        assert!(matches!(&app.messages[1], ChatLine::Chat { .. }));
        assert!(matches!(&app.messages[2], ChatLine::Ticket(_)));
    }
}
