//! File transfer state machine and file share pane rendering.
//!
//! This module tracks all file offers (sent and received) and renders the
//! horizontal "files" pane between the messages area and the input bar.
//!
//! ## Key concepts
//!
//! - **State machine**: each transfer follows a `Pending → Downloading → Complete/Failed`
//!   lifecycle, encoded as the `TransferState` enum.
//! - **Event-driven updates**: background download tasks communicate via an `mpsc`
//!   channel, sending `TransferEvent`s that the main loop applies to the state machine.
//! - **Content-addressed storage**: files are identified by their BLAKE3 hash — the
//!   `Hash` type from `iroh_blobs`. Two files with the same content have the same hash.

// `EndpointId` is a 32-byte Ed25519 public key identifying a peer on the network.
// We store it in `FileOffer` so the receiver knows which peer to connect to for
// downloading the blob.
use iroh::EndpointId;
// `Hash` is a BLAKE3 content hash from iroh-blobs — a 32-byte digest that uniquely
// identifies blob content. BLAKE3 is extremely fast (faster than SHA-256) and
// produces a 256-bit hash. It's used as the primary key for looking up transfers.
use iroh_blobs::Hash;
// Ratatui types for building the file pane widget:
// - `Rect`: a rectangle defined by (x, y, width, height) — all widget positioning
//   in ratatui is done with `Rect` values.
// - `Style`: combines foreground color, background color, and modifiers (bold, italic, etc.)
// - `Color`: terminal colors — both named (Red, Cyan) and 256-color palette.
// - `Modifier`: text attributes like BOLD, ITALIC — combined with bitwise OR.
// - `Line`: a single horizontal row of styled text, made up of `Span`s.
// - `Span`: a run of text with a single style — the atomic unit of styled text in ratatui.
// - `Block` / `Borders`: a bordered container widget with optional title.
// - `Paragraph`: a multi-line text widget that renders a `Vec<Line>`.
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
// `PathBuf` is an owned filesystem path (the `String` of paths).
// Used in `TransferState::Complete` to store where the downloaded file was saved.
use std::path::PathBuf;

// ── Types ────────────────────────────────────────────────────────────────────

/// A file offer broadcast over gossip. Contains the metadata needed for a
/// receiver to decide whether to download and to initiate the blob fetch.
///
/// `#[derive(Debug, Clone)]`:
/// - `Debug` enables `{:?}` formatting — required for `assert!` macros in tests.
/// - `Clone` generates a `.clone()` method that deep-copies all fields. We need
///   this because the offer is stored in `TransferEntry` but also passed to
///   `spawn_download()` which needs its own owned copy (the spawned task must be
///   `'static` — it can't borrow from the caller).
#[derive(Debug, Clone)]
pub struct FileOffer {
    pub sender_nickname: String,
    /// The peer to connect to for downloading the blob.
    pub sender_id: EndpointId,
    /// The original filename (e.g. "photo.png") — used for display and saving.
    pub filename: String,
    /// File size in bytes — used for progress calculation and display.
    pub size: u64,
    /// The BLAKE3 content hash — the unique identifier for this blob.
    /// Used to match progress/completion events back to the right transfer.
    pub hash: Hash,
}

/// The lifecycle state of a single file transfer.
///
/// This enum implements a **state machine pattern**: each variant represents a
/// distinct phase of the transfer lifecycle. Transitions between states are
/// explicit method calls on `TransferManager` (e.g. `start_download()`,
/// `complete_download()`). Rust enums are perfect for state machines because:
/// 1. Each state can carry different data (struct variants).
/// 2. `match` forces you to handle every state (exhaustiveness checking).
/// 3. Invalid states are unrepresentable — you can't have "downloading" without
///    a byte count, or "complete" without a file path.
#[derive(Debug)]
pub enum TransferState {
    /// Offer received but download not yet started.
    Pending,
    /// Download in progress — tracks bytes received so far.
    /// Both fields are `u64` because files can exceed 4 GB (`u32::MAX`).
    Downloading {
        bytes_received: u64,
        total_bytes: u64,
    },
    /// Download completed — the file is available at `path`.
    Complete(PathBuf),
    /// Download failed with an error message.
    Failed(String),
    /// We are the sender — the file is being shared to peers.
    Sharing,
}

/// Events sent from background download tasks back to the main event loop
/// via an `mpsc` (multi-producer, single-consumer) channel.
///
/// This is the **channel event pattern**: background tasks can't directly mutate
/// the UI state (it lives on the main task), so they send events through a channel.
/// The main event loop receives these events in its `tokio::select!` and applies
/// them to the `TransferManager`. This decouples the download logic from the UI logic.
#[derive(Debug)]
pub enum TransferEvent {
    /// Periodic progress update — sent during download.
    Progress {
        hash: Hash,
        bytes_received: u64,
        total_bytes: u64,
    },
    /// Download completed successfully — file is saved to `path`.
    Complete {
        hash: Hash,
        filename: String,
        path: PathBuf,
    },
    /// Download failed with an error description.
    Failed {
        hash: Hash,
        filename: String,
        error: String,
    },
}

/// A single entry in the file share pane — an offer paired with its state.
#[derive(Debug)]
pub struct TransferEntry {
    pub offer: FileOffer,
    pub state: TransferState,
}

// ── TransferManager ──────────────────────────────────────────────────────────

/// Manages the list of file transfers (both sent and received) and tracks
/// which entry is currently selected when the file pane has focus.
///
/// This struct owns a `Vec<TransferEntry>` — a growable, heap-allocated array.
/// Entries are appended with `push()` and never removed (completed/failed entries
/// stay visible in the UI so the user can see their transfer history).
#[derive(Debug)]
pub struct TransferManager {
    pub entries: Vec<TransferEntry>,
    /// Index of the currently highlighted entry in the file pane.
    /// Used for keyboard navigation (Up/Down) and action dispatch (Enter).
    pub selected_index: usize,
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected_index: 0,
        }
    }

    /// Add an incoming file offer from a remote peer.
    pub fn add_offer(&mut self, offer: FileOffer) {
        self.entries.push(TransferEntry {
            offer,
            state: TransferState::Pending,
        });
    }

    /// Add an entry for a file we are sharing (sender's view).
    pub fn add_sent(&mut self, offer: FileOffer) {
        self.entries.push(TransferEntry {
            offer,
            state: TransferState::Sharing,
        });
    }

    /// Mark a transfer as downloading by matching on the BLAKE3 hash.
    ///
    /// `iter_mut()` returns an iterator of `&mut TransferEntry` — mutable
    /// references to each element. `.find()` returns the first element matching
    /// the predicate, wrapped in `Option<&mut TransferEntry>`. `if let Some(entry)`
    /// unwraps it for mutation.
    pub fn start_download(&mut self, hash: &Hash) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Downloading {
                bytes_received: 0,
                total_bytes: entry.offer.size,
            };
        }
    }

    /// Update download progress for a transfer identified by hash.
    pub fn update_progress(&mut self, hash: &Hash, bytes_received: u64, total_bytes: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Downloading {
                bytes_received,
                total_bytes,
            };
        }
    }

    /// Mark a transfer as complete with the path to the downloaded file.
    pub fn complete_download(&mut self, hash: &Hash, path: PathBuf) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Complete(path);
        }
    }

    /// Mark a transfer as failed with an error message.
    pub fn fail_download(&mut self, hash: &Hash, error: String) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Failed(error);
        }
    }

    /// Get a reference to the currently selected entry (if any).
    ///
    /// `Vec::get()` returns `Option<&T>` — it's the bounds-checked alternative
    /// to indexing with `[]`. If `selected_index` is out of bounds, it returns
    /// `None` instead of panicking.
    pub fn selected_entry(&self) -> Option<&TransferEntry> {
        self.entries.get(self.selected_index)
    }

    /// Whether there are any entries to display.
    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Move selection to the next entry (wrapping around).
    ///
    /// `(self.selected_index + 1) % self.entries.len()` uses the modulo operator
    /// for **wrap-around arithmetic**: when the index reaches the end of the list,
    /// it wraps back to 0. This is a common pattern for cyclic navigation.
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.entries.len();
        }
    }

    /// Move selection to the previous entry (wrapping around).
    ///
    /// We can't just subtract 1 because `usize` is unsigned — subtracting from 0
    /// would panic (in debug) or wrap to `usize::MAX` (in release). Instead, we
    /// check for 0 explicitly and jump to the last element.
    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.entries.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }
}

// ── Rendering ────────────────────────────────────────────────────────────────

/// Format a byte count as a human-readable file size string.
///
/// Uses `const` items for the size thresholds. `const` in Rust means the value
/// is computed at compile time and inlined wherever it's used — zero runtime cost.
/// The `as f64` casts perform integer-to-float conversion for the division.
pub fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Render the file share pane into the given area.
///
/// Shows a bordered block titled "files" with one line per transfer entry.
/// The border color is cyan when the pane is focused, default otherwise.
/// The selected row gets a `>` prefix and bold styling when focused.
///
/// This function demonstrates ratatui's **composition model**:
/// - `Block` provides the border and title
/// - `Paragraph` renders multi-line styled text inside the block
/// - Each `Line` is built from a `Vec<Span>` — each span has its own style
/// - `f.render_widget()` places the composed widget at a specific `Rect`
pub fn render_file_pane(
    f: &mut ratatui::Frame,
    area: Rect,
    manager: &TransferManager,
    focused: bool,
) {
    let border_color = if focused { Color::Cyan } else { Color::White };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title("files");

    // Build one `Line` per transfer entry using iterator chains.
    // `.enumerate()` wraps each element with its index `(i, entry)` — we need
    // the index to determine if this row is currently selected.
    let lines: Vec<Line> = manager
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = focused && i == manager.selected_index;
            let prefix = if is_selected { "> " } else { "  " };
            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };

            let sender = &entry.offer.sender_nickname;
            let filename = &entry.offer.filename;
            let size = format_file_size(entry.offer.size);

            // Build the state indicator span — each transfer state gets a
            // different visual representation.
            let state_span = match &entry.state {
                TransferState::Pending => {
                    Span::styled("[ dl ]", Style::default().fg(Color::Yellow))
                }
                TransferState::Downloading {
                    bytes_received,
                    total_bytes,
                } => {
                    // Calculate download percentage (0–100).
                    let pct = if *total_bytes > 0 {
                        (*bytes_received as f64 / *total_bytes as f64 * 100.0) as u64
                    } else {
                        0
                    };
                    // Build a 6-character progress bar using Unicode block characters:
                    // - U+2588 (█) "full block" for filled portion
                    // - U+2591 (░) "light shade" for empty portion
                    // `.repeat(n)` creates a String of n copies of the character.
                    // `.min(6)` clamps the filled count to prevent overflow.
                    let filled = (pct as usize * 6 / 100).min(6);
                    let empty = 6 - filled;
                    let bar = format!(
                        "[{}{}] {pct}%",
                        "\u{2588}".repeat(filled),
                        "\u{2591}".repeat(empty)
                    );
                    Span::styled(bar, Style::default().fg(Color::Green))
                }
                TransferState::Complete(_) => {
                    Span::styled("[open dir]", Style::default().fg(Color::Green))
                }
                TransferState::Failed(err) => {
                    // Truncate long error messages to keep the UI tidy.
                    // `.chars().take(17).collect()` iterates Unicode characters
                    // (not bytes) and collects the first 17 into a new String.
                    let truncated: String = err.chars().take(17).collect();
                    let msg = if err.len() > 20 {
                        format!("[err: {truncated}...]")
                    } else {
                        format!("[err: {err}]")
                    };
                    Span::styled(msg, Style::default().fg(Color::Red))
                }
                TransferState::Sharing => {
                    Span::styled("[sharing]", Style::default().fg(Color::Blue))
                }
            };

            // Compose the line from multiple spans — each with its own style.
            // `Line::from(Vec<Span>)` concatenates them horizontally.
            Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(format!("{sender}: "), name_style),
                Span::styled(format!("{filename} "), Style::default().fg(Color::White)),
                Span::styled(format!("({size})  "), Style::default().fg(Color::DarkGray)),
                state_span,
            ])
        })
        .collect();

    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a deterministic `Hash` for testing.
    /// `[42u8; 32]` creates a 32-byte array filled with the value 42.
    fn test_hash() -> Hash {
        Hash::from_bytes([42u8; 32])
    }

    /// Helper to create a `FileOffer` with test data.
    /// `EndpointId::from_bytes()` constructs an ID from raw bytes — useful for
    /// tests where we don't need a real cryptographic keypair.
    fn test_offer(nickname: &str) -> FileOffer {
        FileOffer {
            sender_nickname: nickname.to_string(),
            sender_id: EndpointId::from_bytes(&[1u8; 32]).unwrap(),
            filename: "test.txt".to_string(),
            size: 1024,
            hash: test_hash(),
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let m = TransferManager::new();
        assert!(!m.has_entries());
        assert_eq!(m.selected_index, 0);
        assert!(m.selected_entry().is_none());
    }

    #[test]
    fn add_offer_and_sent() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        assert!(m.has_entries());
        assert_eq!(m.entries.len(), 1);
        // `matches!` is a macro that returns `true` if the value matches the
        // given pattern. It's more concise than a `match` block when you only
        // need a boolean check.
        assert!(matches!(m.entries[0].state, TransferState::Pending));

        m.add_sent(test_offer("You"));
        assert_eq!(m.entries.len(), 2);
        assert!(matches!(m.entries[1].state, TransferState::Sharing));
    }

    #[test]
    fn start_download_transitions_state() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.start_download(&hash);
        assert!(matches!(
            m.entries[0].state,
            TransferState::Downloading {
                bytes_received: 0,
                total_bytes: 1024,
            }
        ));
    }

    #[test]
    fn update_progress() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.start_download(&hash);
        m.update_progress(&hash, 512, 1024);
        assert!(matches!(
            m.entries[0].state,
            TransferState::Downloading {
                bytes_received: 512,
                total_bytes: 1024,
            }
        ));
    }

    #[test]
    fn complete_download() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.complete_download(&hash, PathBuf::from("/tmp/test.txt"));
        match &m.entries[0].state {
            TransferState::Complete(p) => assert_eq!(p, &PathBuf::from("/tmp/test.txt")),
            _ => panic!("expected Complete state"),
        }
    }

    #[test]
    fn fail_download() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.fail_download(&hash, "network error".into());
        match &m.entries[0].state {
            TransferState::Failed(e) => assert_eq!(e, "network error"),
            _ => panic!("expected Failed state"),
        }
    }

    #[test]
    fn select_next_and_prev() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        let mut offer2 = test_offer("Bob");
        offer2.hash = Hash::from_bytes([99u8; 32]);
        m.add_offer(offer2);

        assert_eq!(m.selected_index, 0);
        m.select_next();
        assert_eq!(m.selected_index, 1);
        m.select_next();
        assert_eq!(m.selected_index, 0); // wraps
        m.select_prev();
        assert_eq!(m.selected_index, 1); // wraps backward
        m.select_prev();
        assert_eq!(m.selected_index, 0);
    }

    #[test]
    fn select_on_empty_is_noop() {
        let mut m = TransferManager::new();
        m.select_next();
        assert_eq!(m.selected_index, 0);
        m.select_prev();
        assert_eq!(m.selected_index, 0);
    }

    #[test]
    fn format_file_size_units() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(1048576), "1.0 MB");
        assert_eq!(format_file_size(1073741824), "1.0 GB");
    }
}
