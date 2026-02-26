//! Chat application state and terminal UI rendering.
//!
//! This module owns the `App` struct (the "model" in an MVC sense) and the
//! `ui()` function that renders it into a ratatui terminal frame. The event
//! loop in `main.rs` mutates `App` then calls `ui()` on each tick.

// `BTreeMap` is an ordered map backed by a B-tree. Unlike `HashMap`, it keeps
// keys sorted — so the peers panel always displays peers in a consistent
// (deterministic) order based on their `EndpointId`.
use std::collections::BTreeMap;

// `EndpointId` is a 32-byte public key that uniquely identifies each iroh node.
use iroh::EndpointId;
// Ratatui types for building terminal UIs:
// - `Layout` / `Constraint`: split the terminal into regions (vertical/horizontal)
// - `Style` / `Color` / `Modifier`: text styling (foreground, bold, italic, etc.)
// - `Line` / `Span`: styled text primitives — a `Line` is a row of `Span`s
// - `Block` / `Borders` / `Paragraph`: widget types for bordered text panels
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

// Import types from our sibling modules.
use crate::filepicker::FilePicker;
use crate::net::{ConnType, PeerInfo};
use crate::transfer::{self, TransferManager};

// ── App state ────────────────────────────────────────────────────────────────
//
// The `App` struct is the single source of truth for the chat session.
// It follows the "immediate mode" UI pattern: mutate state → redraw everything.

/// Which UI element currently has keyboard focus.
pub enum AppMode {
    /// Normal chat input mode.
    Chat,
    /// The modal file picker overlay is open.
    FilePicker,
    /// The file share pane has focus (navigate with Up/Down, Enter to act).
    FilePane,
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
    Chat { nickname: String, text: String },
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
    pub file_picker: Option<FilePicker>,
    /// All file transfer entries (sent and received).
    pub transfers: TransferManager,
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
        }
    }

    /// Open the modal file picker overlay.
    pub fn open_file_picker(&mut self) {
        if let Ok(picker) = FilePicker::new() {
            self.file_picker = Some(picker);
            self.mode = AppMode::FilePicker;
        }
    }

    /// Close the file picker overlay and return to chat mode.
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

    /// Append a chat message to the message log.
    pub fn chat(&mut self, nickname: String, text: String) {
        self.messages.push(ChatLine::Chat { nickname, text });
    }
}

// ── UI ───────────────────────────────────────────────────────────────────────
//
// Ratatui uses an "immediate mode" rendering model: every frame, we build up
// the entire UI from scratch based on current state. No retained widget tree,
// no diffing — just draw what the state says. This is simple and fast for TUIs.

/// Render the chat UI into a terminal frame.
///
/// `&App` is an immutable borrow — the UI function only *reads* the state,
/// it never modifies it. This is enforced at compile time: you literally cannot
/// mutate through a `&` reference. This is a key Rust safety guarantee.
///
/// `ratatui::Frame` is a mutable drawing surface for one frame. It provides
/// `render_widget()` to place widgets at specific screen rectangles, and
/// `set_cursor_position()` to show the blinking cursor.
pub fn ui(f: &mut ratatui::Frame, app: &App) {
    // Build the vertical layout — conditionally include the file pane row when
    // there are active offers/transfers.
    let rows = if app.transfers.has_entries() {
        let file_pane_height = (app.transfers.entries.len() as u16 + 2).min(8);
        Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(file_pane_height),
            Constraint::Length(3),
        ])
        .split(f.area())
    } else {
        Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area())
    };
    // Split the top row into left (messages, flexible) and right (peers, 24 cols).
    let top = Layout::horizontal([Constraint::Min(1), Constraint::Length(24)]).split(rows[0]);

    // ── Messages pane (top left) ─────────────────────────────────────────

    // Transform each `ChatLine` into a styled ratatui `Line` using iterators.
    //
    // `.iter()` borrows each element; `.map()` transforms it; `.collect()`
    // gathers results into a `Vec<Line>`. This is Rust's iterator chain
    // pattern — lazy evaluation, zero allocation overhead (the compiler fuses
    // the iterator chain into a single loop).
    let lines: Vec<Line> = app
        .messages
        .iter()
        .map(|msg| match msg {
            // Pattern matching on enum variants with destructuring:
            // `ChatLine::System(text)` binds the inner String to `text`.
            ChatLine::System(text) => Line::from(Span::styled(
                format!("[system] {text}"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            )),
            ChatLine::Ticket(ticket) => Line::from(vec![
                Span::styled(
                    "Ticket: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(ticket.as_str(), Style::default().fg(Color::White)),
            ]),
            // Struct variant destructuring: `{ nickname, text }` pulls out
            // named fields directly into local variables.
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
    // are always visible. `saturating_sub` returns 0 instead of underflowing.
    let visible = top[0].height.saturating_sub(2) as usize;
    let scroll = lines.len().saturating_sub(visible) as u16;

    let messages_widget = Paragraph::new(lines)
        .scroll((scroll, 0))
        .block(Block::default().borders(Borders::ALL).title("piper-chat"));
    f.render_widget(messages_widget, top[0]);

    // ── Peers pane (top right) ───────────────────────────────────────────

    // `.values()` iterates only over the `PeerInfo` values in the BTreeMap,
    // skipping the keys. The `match` on `peer.conn_type` maps each connection
    // type to a display tag and color.
    let peer_lines: Vec<Line> = app
        .peers
        .values()
        .map(|peer| {
            let (tag, tag_color) = match peer.conn_type {
                ConnType::Direct => ("[direct]", Color::Green),
                ConnType::Relay => ("[relay]", Color::Yellow),
                ConnType::Unknown => ("[?]", Color::DarkGray),
            };
            Line::from(vec![
                Span::styled(format!("{tag} "), Style::default().fg(tag_color)),
                Span::styled(peer.name.as_str(), Style::default().fg(Color::Green)),
            ])
        })
        .collect();
    let peers_widget = Paragraph::new(peer_lines)
        .block(Block::default().borders(Borders::ALL).title("peers"));
    f.render_widget(peers_widget, top[1]);

    // ── Input pane (bottom, full width) ──────────────────────────────────

    // The input row index depends on whether the file pane is visible.
    let input_row = if app.transfers.has_entries() { 2 } else { 1 };
    let input_border_color = if matches!(app.mode, AppMode::Chat) {
        Color::Cyan
    } else {
        Color::White
    };
    let input_widget = Paragraph::new(format!("> {}", app.input)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(input_border_color)),
    );
    f.render_widget(input_widget, rows[input_row]);

    // Place the terminal cursor at the user's typing position.
    f.set_cursor_position((
        rows[input_row].x + 2 + app.cursor_pos as u16,
        rows[input_row].y + 1,
    ));

    // ── File share pane (between messages and input) ─────────────────

    if app.transfers.has_entries() {
        let focused = matches!(app.mode, AppMode::FilePane);
        transfer::render_file_pane(f, rows[1], &app.transfers, focused);
    }

    // ── File picker overlay (on top of everything) ───────────────────

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
        app.chat("Alice".into(), "hey there".into());
        assert_eq!(app.messages.len(), 1);
        assert!(
            matches!(&app.messages[0], ChatLine::Chat { nickname, text } if nickname == "Alice" && text == "hey there")
        );
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
        app.chat("Bob".into(), "second".into());
        app.ticket("third");
        assert_eq!(app.messages.len(), 3);
        assert!(matches!(&app.messages[0], ChatLine::System(_)));
        assert!(matches!(&app.messages[1], ChatLine::Chat { .. }));
        assert!(matches!(&app.messages[2], ChatLine::Ticket(_)));
    }
}
