//! Modal file picker overlay using `ratatui-explorer`.
//!
//! Presents a centered card overlay where the user navigates their filesystem
//! and selects a file to share. Uses the same `Clear` pattern as `welcome.rs`
//! to draw on top of the existing chat UI.
//!
//! ## Modal overlay pattern
//!
//! The file picker is a **modal overlay** — it temporarily takes over keyboard
//! focus while the underlying chat UI remains visible (but inactive). This is
//! implemented by:
//! 1. Storing the picker as `Option<FilePicker>` in `App` — `None` means closed.
//! 2. Setting `AppMode::FilePicker` to route all key events to the picker.
//! 3. Rendering the picker *last* in `ui()`, so it draws on top of everything.
//! 4. Using `Clear` widget to erase the area behind the overlay.

// `anyhow::Result` — convenient error type for functions that can fail.
// The `?` operator works with anyhow to convert any `std::error::Error` automatically.
use anyhow::Result;
// `crossterm::event::Event` — the full terminal event enum (key, mouse, resize).
// The ratatui-explorer widget expects this type for its `handle()` method.
use crossterm::event::Event;
// Ratatui types:
// - `Alignment`: text alignment (Left, Center, Right) — used for the title.
// - `Rect`: a rectangle (x, y, width, height) — all positioning in ratatui uses `Rect`.
// - `Style` / `Color`: styling primitives for colors and text attributes.
// - `Block` / `Borders`: bordered container widget — wraps the explorer widget.
// - `Clear`: a special widget that erases (fills with spaces) a rectangular area.
//   Used for overlays to prevent the underlying UI from showing through.
use ratatui::{
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Block, Borders, Clear},
};
// `ratatui_explorer` provides a ready-made filesystem browser widget:
// - `FileExplorer`: the main widget — handles directory traversal, file listing,
//   and keyboard navigation (up/down to select, left to go to parent, right/enter
//   to enter a directory or select a file).
// - `Theme`: builder API for customizing the explorer's appearance (border style,
//   highlight colors, etc.). Uses the builder pattern: chain `.with_*()` methods
//   to configure, then pass to `FileExplorer::with_theme()`.
use ratatui_explorer::{FileExplorer, Theme as ExplorerTheme};
use std::path::PathBuf;

use crate::theme::Theme;

// ── Types ────────────────────────────────────────────────────────────────────

/// The result of processing a key event in the file picker.
///
/// This three-variant enum cleanly separates the three possible outcomes of a
/// key press, letting the caller (in `main.rs`) handle each case with `match`.
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
/// destroyed when they select a file or press Esc. This **create-on-demand,
/// destroy-on-close** pattern keeps the picker stateless between uses — each
/// opening starts fresh from the current working directory.
pub struct FilePicker {
    /// The underlying filesystem explorer widget from `ratatui-explorer`.
    explorer: FileExplorer,
}

impl FilePicker {
    /// Create a new file picker starting at the current working directory.
    ///
    /// `Result<Self>` because `FileExplorer::with_theme()` can fail if the
    /// current directory is unreadable. The `?` operator propagates any error
    /// to the caller, which displays it as a system message.
    pub fn new(theme: &Theme) -> Result<Self> {
        let explorer_theme = ExplorerTheme::default()
            .with_block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(theme.bg))
                    .border_style(Style::default().fg(theme.border_focused))
                    .title(" Select File (Enter=select, Esc=cancel) ")
                    .title_alignment(Alignment::Center)
                    .title_style(Style::default().fg(theme.title)),
            )
            .with_highlight_item_style(
                Style::default()
                    .fg(theme.picker_highlight_file_fg)
                    .bg(theme.picker_highlight_file_bg),
            )
            .with_highlight_dir_style(
                Style::default()
                    .fg(theme.picker_highlight_dir_fg)
                    .bg(theme.picker_highlight_dir_bg),
            )
            .add_default_title();

        let explorer = FileExplorer::with_theme(explorer_theme)?;
        Ok(Self { explorer })
    }

    /// Handle a crossterm event. Returns the picker result.
    ///
    /// - Enter on a file → `Selected(path)`
    /// - Esc → `Cancelled`
    /// - Everything else is delegated to the explorer for navigation.
    ///
    /// We check for Esc/Enter *before* delegating to avoid the explorer
    /// consuming these keys for its own navigation (e.g. Enter on a directory
    /// means "enter the directory", not "select it").
    pub fn handle(&mut self, event: &Event) -> Result<FilePickerResult> {
        // `if let Event::Key(key) = event` is a *refutable pattern match* —
        // it only enters the block if `event` is the `Key` variant.
        if let Event::Key(key) = event {
            // Filter out non-Press events (Windows sends Release events too).
            if key.kind != crossterm::event::KeyEventKind::Press {
                return Ok(FilePickerResult::Browsing);
            }
            match key.code {
                crossterm::event::KeyCode::Esc => return Ok(FilePickerResult::Cancelled),
                crossterm::event::KeyCode::Enter => {
                    // `.current()` returns the currently highlighted `DirEntry`.
                    // `.is_file()` checks the filesystem entry type — returns
                    // `true` for regular files, `false` for directories/symlinks.
                    let current = self.explorer.current();
                    if current.is_file() {
                        // `.path()` returns a reference to the entry's `PathBuf`.
                        // `.clone()` creates an owned copy to return to the caller.
                        return Ok(FilePickerResult::Selected(current.path().clone()));
                    }
                    // If it's a directory, fall through to let the explorer
                    // handle Enter as "navigate into this directory".
                }
                _ => {}
            }
        }

        // Delegate all other events to the explorer for navigation
        // (arrow keys, typing to filter, etc.).
        self.explorer.handle(event)?;
        Ok(FilePickerResult::Browsing)
    }

    /// Render the file picker as a centered overlay on top of the existing UI.
    ///
    /// This demonstrates **Rect arithmetic** for centered card layout:
    /// 1. Calculate card dimensions as a percentage of the terminal size.
    /// 2. Clamp to reasonable min/max bounds with `.max()` and `.min()`.
    /// 3. Center by computing offsets with `saturating_sub()` / 2.
    pub fn render(&self, f: &mut ratatui::Frame) {
        let area = f.area();

        // Centered card: 70% width, 70% height, clamped to reasonable bounds.
        // Integer arithmetic: `area.width * 70 / 100` avoids floating-point.
        // `.max(40)` ensures a minimum width for readability.
        // `.min(area.width)` ensures we don't exceed the terminal size.
        let card_w = (area.width * 70 / 100).max(40).min(area.width);
        let card_h = (area.height * 70 / 100).max(10).min(area.height);
        // `saturating_sub` prevents underflow when the terminal is very small —
        // returns 0 instead of wrapping around (which would be a huge number for u16).
        let x = area.width.saturating_sub(card_w) / 2;
        let y = area.height.saturating_sub(card_h) / 2;
        let card = Rect::new(x, y, card_w, card_h);

        // `Clear` erases the card area (fills with spaces) so the underlying
        // chat UI doesn't show through the overlay. Without this, the explorer
        // widget would be drawn on top of the existing characters, creating a
        // visual mess.
        f.render_widget(Clear, card);
        // `.widget()` returns a ratatui `Widget` that can be rendered into a `Rect`.
        // The `&` borrow is needed because `widget()` returns a reference-based type.
        f.render_widget(&self.explorer.widget(), card);
    }
}
