//! Modal file picker overlay using `ratatui-explorer`.
//!
//! Presents a centered card overlay where the user navigates their filesystem
//! and selects a file to share. Uses the same `Clear` pattern as `welcome.rs`
//! to draw on top of the existing chat UI.

use anyhow::Result;
use crossterm::event::Event;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear},
};
use ratatui_explorer::{FileExplorer, Theme};
use std::path::PathBuf;

// ── Types ────────────────────────────────────────────────────────────────────

/// The result of processing a key event in the file picker.
pub enum FilePickerResult {
    /// User selected a file at this path.
    Selected(PathBuf),
    /// User cancelled (Esc).
    Cancelled,
    /// Still browsing — no action taken yet.
    Browsing,
}

// ── FilePicker ───────────────────────────────────────────────────────────────

/// A modal file picker that wraps `ratatui_explorer::FileExplorer`.
///
/// Created on demand when the user presses Ctrl+F or types `/send`, and
/// destroyed when they select a file or press Esc.
pub struct FilePicker {
    explorer: FileExplorer,
}

impl FilePicker {
    /// Create a new file picker starting at the current working directory.
    pub fn new() -> Result<Self> {
        let theme = Theme::default()
            .with_block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" Select File (Enter=select, Esc=cancel) ")
                    .title_alignment(Alignment::Center),
            )
            .with_highlight_item_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan),
            )
            .with_highlight_dir_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow),
            )
            .add_default_title();

        let explorer = FileExplorer::with_theme(theme)?;
        Ok(Self { explorer })
    }

    /// Handle a crossterm event. Returns the picker result.
    ///
    /// - Enter on a file → `Selected(path)`
    /// - Esc → `Cancelled`
    /// - Everything else is delegated to the explorer for navigation.
    pub fn handle(&mut self, event: &Event) -> Result<FilePickerResult> {
        // Check for Esc or Enter before delegating to the explorer
        if let Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return Ok(FilePickerResult::Browsing);
            }
            match key.code {
                crossterm::event::KeyCode::Esc => return Ok(FilePickerResult::Cancelled),
                crossterm::event::KeyCode::Enter => {
                    let current = self.explorer.current();
                    if current.is_file() {
                        return Ok(FilePickerResult::Selected(current.path().clone()));
                    }
                    // If it's a directory, let the explorer handle it (navigate into)
                }
                _ => {}
            }
        }

        self.explorer.handle(event)?;
        Ok(FilePickerResult::Browsing)
    }

    /// Render the file picker as a centered overlay on top of the existing UI.
    pub fn render(&self, f: &mut ratatui::Frame) {
        let area = f.area();

        // Centered card: 70% width, 70% height, clamped to reasonable bounds
        let card_w = (area.width * 70 / 100).max(40).min(area.width);
        let card_h = (area.height * 70 / 100).max(10).min(area.height);
        let x = area.width.saturating_sub(card_w) / 2;
        let y = area.height.saturating_sub(card_h) / 2;
        let card = Rect::new(x, y, card_w, card_h);

        // Clear the area behind the overlay
        f.render_widget(Clear, card);
        // Render the explorer widget into the card area
        f.render_widget(&self.explorer.widget(), card);
    }
}
