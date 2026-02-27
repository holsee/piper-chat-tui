//! Centralized color theme for the TUI.
//!
//! Defines a `Theme` struct with named color slots for every semantic role used
//! across the UI. Two palettes are provided — dark (default) and light — and a
//! runtime toggle switches between them with Ctrl+T.

use ratatui::style::Color;

/// Which palette is currently active.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Dark,
    Light,
}

/// A complete color palette for the TUI.
///
/// Every color used by the UI is looked up here — no hardcoded `Color::*`
/// constants elsewhere in the codebase. This makes it trivial to swap palettes
/// at runtime.
pub struct Theme {
    pub mode: ThemeMode,

    // ── Background ─────────────────────────────────────────────────────
    pub bg: Color,

    // ── Surfaces / borders ───────────────────────────────────────────────
    pub border: Color,
    pub border_focused: Color,
    pub title: Color,

    // ── Text ─────────────────────────────────────────────────────────────
    pub text: Color,
    pub text_dim: Color,
    pub text_muted: Color,

    // ── Accents ──────────────────────────────────────────────────────────
    pub accent: Color,
    pub accent_bg: Color,
    pub accent_on_bg: Color,

    // ── Nicknames / peers ────────────────────────────────────────────────
    pub nickname: Color,
    pub peer_name: Color,

    // ── Semantic: ticket ─────────────────────────────────────────────────
    pub ticket_label: Color,
    pub ticket_value: Color,

    // ── Semantic: connection types ───────────────────────────────────────
    pub conn_direct: Color,
    pub conn_relay: Color,
    pub conn_unknown: Color,

    // ── Transfer states ──────────────────────────────────────────────────
    pub transfer_pending: Color,
    pub transfer_progress: Color,
    pub transfer_complete: Color,
    pub transfer_failed: Color,
    pub transfer_sharing: Color,

    // ── Status / hints ───────────────────────────────────────────────────
    pub error: Color,
    pub hint_key: Color,
    pub hint_text: Color,

    // ── File picker ──────────────────────────────────────────────────────
    pub picker_highlight_file_fg: Color,
    pub picker_highlight_file_bg: Color,
    pub picker_highlight_dir_fg: Color,
    pub picker_highlight_dir_bg: Color,

    // ── Input ────────────────────────────────────────────────────────────
    pub input_prompt: Color,
    pub cursor_blink: Color,
}

impl Theme {
    /// Dark theme — dark grey background (terminal default), purple accent.
    pub fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,

            bg: Color::Rgb(25, 20, 35),

            border: Color::Rgb(100, 80, 140),
            border_focused: Color::Rgb(180, 130, 255),
            title: Color::Rgb(180, 130, 255),

            text: Color::Rgb(220, 220, 220),
            text_dim: Color::Rgb(120, 115, 130),
            text_muted: Color::Rgb(100, 100, 110),

            accent: Color::Rgb(180, 130, 255),
            accent_bg: Color::Rgb(180, 130, 255),
            accent_on_bg: Color::Rgb(20, 15, 30),

            nickname: Color::Rgb(200, 160, 255),
            peer_name: Color::Rgb(170, 140, 220),

            ticket_label: Color::Rgb(220, 180, 100),
            ticket_value: Color::Rgb(220, 220, 220),

            conn_direct: Color::Rgb(100, 220, 100),
            conn_relay: Color::Rgb(220, 180, 100),
            conn_unknown: Color::Rgb(100, 100, 110),

            transfer_pending: Color::Rgb(220, 180, 100),
            transfer_progress: Color::Rgb(100, 220, 100),
            transfer_complete: Color::Rgb(100, 220, 100),
            transfer_failed: Color::Rgb(255, 100, 100),
            transfer_sharing: Color::Rgb(140, 120, 220),

            error: Color::Rgb(255, 100, 100),
            hint_key: Color::Rgb(140, 200, 140),
            hint_text: Color::Rgb(120, 115, 130),

            picker_highlight_file_fg: Color::Rgb(20, 15, 30),
            picker_highlight_file_bg: Color::Rgb(180, 130, 255),
            picker_highlight_dir_fg: Color::Rgb(20, 15, 30),
            picker_highlight_dir_bg: Color::Rgb(220, 180, 100),

            input_prompt: Color::Rgb(180, 130, 255),
            cursor_blink: Color::Rgb(100, 100, 110),
        }
    }

    /// Light theme — off-white feel (terminal handles actual bg), deeper purples.
    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,

            bg: Color::Rgb(240, 236, 245),

            border: Color::Rgb(180, 160, 200),
            border_focused: Color::Rgb(120, 60, 200),
            title: Color::Rgb(120, 60, 200),

            text: Color::Rgb(50, 50, 60),
            text_dim: Color::Rgb(110, 100, 120),
            text_muted: Color::Rgb(140, 130, 150),

            accent: Color::Rgb(120, 60, 200),
            accent_bg: Color::Rgb(120, 60, 200),
            accent_on_bg: Color::Rgb(255, 255, 255),

            nickname: Color::Rgb(100, 40, 180),
            peer_name: Color::Rgb(90, 50, 160),

            ticket_label: Color::Rgb(160, 100, 20),
            ticket_value: Color::Rgb(50, 50, 60),

            conn_direct: Color::Rgb(30, 140, 30),
            conn_relay: Color::Rgb(160, 100, 20),
            conn_unknown: Color::Rgb(140, 130, 150),

            transfer_pending: Color::Rgb(160, 100, 20),
            transfer_progress: Color::Rgb(30, 140, 30),
            transfer_complete: Color::Rgb(30, 140, 30),
            transfer_failed: Color::Rgb(200, 40, 40),
            transfer_sharing: Color::Rgb(100, 60, 180),

            error: Color::Rgb(200, 40, 40),
            hint_key: Color::Rgb(30, 140, 30),
            hint_text: Color::Rgb(140, 130, 150),

            picker_highlight_file_fg: Color::Rgb(255, 255, 255),
            picker_highlight_file_bg: Color::Rgb(120, 60, 200),
            picker_highlight_dir_fg: Color::Rgb(255, 255, 255),
            picker_highlight_dir_bg: Color::Rgb(160, 100, 20),

            input_prompt: Color::Rgb(120, 60, 200),
            cursor_blink: Color::Rgb(140, 130, 150),
        }
    }

    /// Toggle between dark and light palettes.
    pub fn toggle(&mut self) {
        *self = match self.mode {
            ThemeMode::Dark => Self::light(),
            ThemeMode::Light => Self::dark(),
        };
    }
}
