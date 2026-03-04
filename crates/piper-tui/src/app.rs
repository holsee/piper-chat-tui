//! Chat application state (view-model).
//!
//! The `App` struct holds all TUI state. The session (piper-core) owns networking
//! state; App maps SessionEvents to display data.

use std::collections::BTreeMap;
use std::time::Instant;

use iroh::EndpointId;
use ratatui::layout::Rect;

use crate::filepicker::FilePicker;
use crate::theme::Theme;
use piper_core::protocol::PeerInfo;
use piper_core::transfer::TransferManager;

// ── App state ────────────────────────────────────────────────────────────────

/// Which UI element currently has keyboard focus.
pub enum AppMode {
    Chat,
    FilePicker,
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
pub enum ChatLine {
    System(String),
    Ticket(String),
    Chat {
        nickname: String,
        text: String,
        timestamp_ms: u64,
    },
}

/// The main application state for the chat TUI (pure view-model).
pub struct App {
    pub messages: Vec<ChatLine>,
    pub input: String,
    pub cursor_pos: usize,
    pub should_quit: bool,
    pub peers: BTreeMap<EndpointId, PeerInfo>,
    pub mode: AppMode,
    pub file_picker: Option<FilePicker>,
    pub transfers: TransferManager,
    pub theme: Theme,
    pub scroll_offset: u16,
    pub click_regions: Vec<ClickRegion>,
    pub ticket_str: Option<String>,
    pub copy_feedback_until: Option<Instant>,
    pub pending_send_target: Option<String>,
}

impl App {
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
            scroll_offset: 0,
            click_regions: Vec::new(),
            ticket_str: None,
            copy_feedback_until: None,
            pending_send_target: None,
        }
    }

    pub fn open_file_picker(&mut self) {
        if let Ok(picker) = FilePicker::new(&self.theme) {
            self.file_picker = Some(picker);
            self.mode = AppMode::FilePicker;
        }
    }

    pub fn close_file_picker(&mut self) {
        self.file_picker = None;
        self.mode = AppMode::Chat;
    }

    pub fn focus_file_pane(&mut self) {
        self.mode = AppMode::FilePane;
    }

    pub fn focus_chat(&mut self) {
        self.mode = AppMode::Chat;
    }

    pub fn system(&mut self, msg: impl Into<String>) {
        self.messages.push(ChatLine::System(msg.into()));
    }

    pub fn ticket(&mut self, ticket: impl Into<String>) {
        self.messages.push(ChatLine::Ticket(ticket.into()));
    }

    pub fn chat(&mut self, nickname: String, text: String, timestamp_ms: u64) {
        self.messages.push(ChatLine::Chat {
            nickname,
            text,
            timestamp_ms,
        });
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_starts_empty() {
        let app = App::new();
        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
        assert!(!app.should_quit);
        assert!(app.peers.is_empty());
    }

    #[test]
    fn app_system_message() {
        let mut app = App::new();
        app.system("hello");
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(&app.messages[0], ChatLine::System(s) if s == "hello"));
    }

    #[test]
    fn app_ticket_message() {
        let mut app = App::new();
        app.ticket("abc123");
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(&app.messages[0], ChatLine::Ticket(s) if s == "abc123"));
    }

    #[test]
    fn app_chat_message() {
        let mut app = App::new();
        app.chat("Alice".into(), "hey there".into(), 1700000000000);
        assert_eq!(app.messages.len(), 1);
        assert!(
            matches!(&app.messages[0], ChatLine::Chat { nickname, text, .. } if nickname == "Alice" && text == "hey there")
        );
    }

    #[test]
    fn app_system_accepts_string_and_str() {
        let mut app = App::new();
        app.system("a &str");
        app.system(String::from("a String"));
        assert_eq!(app.messages.len(), 2);
    }

    #[test]
    fn app_messages_accumulate_in_order() {
        let mut app = App::new();
        app.system("first");
        app.chat("Bob".into(), "second".into(), 1000);
        app.ticket("third");
        assert_eq!(app.messages.len(), 3);
        assert!(matches!(&app.messages[0], ChatLine::System(_)));
        assert!(matches!(&app.messages[1], ChatLine::Chat { .. }));
        assert!(matches!(&app.messages[2], ChatLine::Ticket(_)));
    }
}
