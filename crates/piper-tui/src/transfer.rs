//! File transfer types (re-exported from piper-core) and file pane rendering.

pub use piper_core::transfer::*;
pub use piper_core::util::format_file_size;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::theme::Theme;

/// Render the file share pane into the given area.
pub fn render_file_pane(
    f: &mut ratatui::Frame,
    area: Rect,
    manager: &TransferManager,
    focused: bool,
    theme: &Theme,
) {
    let border_color = if focused {
        theme.border_focused
    } else {
        theme.border
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.bg))
        .border_style(Style::default().fg(border_color))
        .title("files")
        .title_style(Style::default().fg(theme.title));

    let lines: Vec<Line> = manager
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = focused && i == manager.selected_index;
            let prefix = if is_selected { "> " } else { "  " };
            let name_style = if is_selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent)
            };

            let sender = &entry.offer.sender_nickname;
            let filename = &entry.offer.filename;
            let size = format_file_size(entry.offer.size);

            let state_span = match &entry.state {
                TransferState::Pending => {
                    Span::styled("[ dl ]", Style::default().fg(theme.transfer_pending))
                }
                TransferState::Downloading {
                    bytes_received,
                    total_bytes,
                } => {
                    let pct = if *total_bytes > 0 {
                        (*bytes_received as f64 / *total_bytes as f64 * 100.0) as u64
                    } else {
                        0
                    };
                    let filled = (pct as usize * 6 / 100).min(6);
                    let empty = 6 - filled;
                    let bar = format!(
                        "[{}{}] {pct}%",
                        "\u{2588}".repeat(filled),
                        "\u{2591}".repeat(empty)
                    );
                    Span::styled(bar, Style::default().fg(theme.transfer_progress))
                }
                TransferState::Complete(_) => {
                    Span::styled("[open dir]", Style::default().fg(theme.transfer_complete))
                }
                TransferState::Failed(err) => {
                    let truncated: String = err.chars().take(17).collect();
                    let msg = if err.len() > 20 {
                        format!("[err: {truncated}...]")
                    } else {
                        format!("[err: {err}]")
                    };
                    Span::styled(msg, Style::default().fg(theme.transfer_failed))
                }
                TransferState::Sharing => {
                    Span::styled("[unshare]", Style::default().fg(theme.transfer_sharing))
                }
            };

            Line::from(vec![
                Span::styled(prefix, name_style),
                Span::styled(format!("{sender}: "), name_style),
                Span::styled(format!("{filename} "), Style::default().fg(theme.text)),
                Span::styled(format!("({size})  "), Style::default().fg(theme.text_muted)),
                state_span,
            ])
        })
        .collect();

    let widget = Paragraph::new(lines).block(block);
    f.render_widget(widget, area);
}
