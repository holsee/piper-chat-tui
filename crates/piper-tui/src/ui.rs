//! Chat UI rendering.

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, AppMode, ChatLine, ClickAction, ClickRegion};
use crate::transfer;
use piper_core::protocol::ConnType;

/// Format a unix timestamp (ms) as `HH:MM` UTC.
fn format_timestamp(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) as i64;
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    format!("{hours:02}:{minutes:02}")
}

/// Render the chat UI into a terminal frame.
pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    app.click_regions.clear();
    let bg_block = Block::default().style(Style::default().bg(app.theme.bg));
    f.render_widget(bg_block, f.area());

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
    let top = Layout::horizontal([Constraint::Min(1), Constraint::Length(24)]).split(rows[0]);

    // ── Messages pane ────────────────────────────────────────────────────

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

    let visible = top[0].height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible) as u16;
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

    app.click_regions.push(ClickRegion {
        rect: top[0],
        action: ClickAction::FocusChat,
    });

    // ── Peers pane ───────────────────────────────────────────────────────

    let show_copy_btn = app.ticket_str.is_some();
    let btn_height = if show_copy_btn { 3 } else { 0 };
    let peers_split = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(btn_height),
    ])
    .split(top[1]);

    let mut sorted_peers: Vec<_> = app.peers.values().collect();
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

    // ── Input pane ───────────────────────────────────────────────────────

    let input_row = if app.transfers.has_entries() { 2 } else { 1 };
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

    app.click_regions.push(ClickRegion {
        rect: rows[input_row],
        action: ClickAction::FocusChat,
    });

    f.set_cursor_position((
        rows[input_row].x + 2 + app.cursor_pos as u16,
        rows[input_row].y + 1,
    ));

    // ── File share pane ──────────────────────────────────────────────────

    if app.transfers.has_entries() {
        let focused = matches!(app.mode, AppMode::FilePane);
        transfer::render_file_pane(f, rows[1], &app.transfers, focused, theme);

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

        app.click_regions.push(ClickRegion {
            rect: rows[1],
            action: ClickAction::FocusFilePane,
        });
    }

    // ── File picker overlay ──────────────────────────────────────────────

    if let Some(picker) = &app.file_picker {
        picker.render(f);
    }
}
