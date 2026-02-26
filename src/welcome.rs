//! Interactive welcome screen for room setup.
//!
//! This module implements a form-based TUI dialog that collects the user's
//! nickname and room mode (create or join) before entering the chat. It runs
//! its own event loop and returns a `WelcomeResult` to the caller.
//!
//! Internally it follows a simple state machine pattern: a `WelcomeState`
//! struct holds all form data, and key events transition between fields
//! or trigger validation.

use anyhow::Result;
use crossterm::{
    // `Event as TermEvent` is a *type alias import* — it renames crossterm's
    // `Event` to `TermEvent` to avoid collision with other `Event` types
    // (like `GossipEvent` in main.rs). The `as` keyword works at the import
    // level for renaming.
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    // `execute!` is a macro that writes crossterm commands to a writer (stdout).
    // Macros in Rust are invoked with `!` and can generate arbitrary code at
    // compile time.
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh_tickets::Ticket;
// `StreamExt` is an *extension trait* — it adds `.next()` to async streams.
// In Rust, you must import extension traits to use their methods. This is the
// "extension trait pattern": define extra methods in a separate trait so you
// can add functionality to types you don't own.
use n0_future::StreamExt;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tokio::time::{Duration, interval};

// `crate::net` refers to the `net` module declared at the crate root.
// We only need `ChatTicket` for ticket validation in the join flow.
use crate::net::ChatTicket;

// ── Welcome screen state ────────────────────────────────────────────────────
//
// These types are *private* to this module (no `pub` keyword). Only
// `WelcomeResult` and `run_welcome_screen()` are exported. This encapsulation
// means the rest of the codebase doesn't depend on internal form details.

/// Which form field currently has keyboard focus.
///
/// `#[derive(PartialEq)]` generates the `==` and `!=` operators. Rust doesn't
/// provide equality comparison by default — you must opt in, which prevents
/// accidental comparisons of types where equality isn't meaningful (e.g.
/// closures, file handles).
/// `#[derive(Debug)]` is needed for `assert_eq!` in tests — the macro prints
/// both values on failure, which requires the `Debug` trait (Rust's `{:?}` format).
#[derive(Debug, PartialEq)]
enum WelcomeField {
    Name,
    Mode,
    Ticket,
}

/// Whether the user is creating a new room or joining an existing one.
///
/// `#[derive(Clone, Copy)]` makes this type *copyable*. Rust distinguishes
/// "move" semantics (default, ownership transfers) from "copy" semantics
/// (bitwise copy, original remains valid). Small types like enums with no
/// heap data are good candidates for `Copy`. Without `Copy`, assigning
/// `let b = a;` would *move* `a`, making it unusable afterward.
#[derive(Debug, PartialEq, Clone, Copy)]
enum RoomMode {
    Create,
    Join,
}

/// All mutable state for the welcome form.
///
/// This is a "plain old struct" — no generics, no lifetimes, fully owned data.
/// Each field is a concrete type. Keeping all form state in one struct makes
/// it easy to pass around by `&mut` reference.
struct WelcomeState {
    field: WelcomeField,
    name: String,
    name_cursor: usize,
    mode: RoomMode,
    ticket: String,
    ticket_cursor: usize,
    /// `Option<String>` is Rust's null-safe alternative — it's either
    /// `Some(value)` or `None`. No null pointer exceptions possible.
    error: Option<String>,
    should_quit: bool,
}

impl WelcomeState {
    fn new() -> Self {
        Self {
            field: WelcomeField::Name,
            name: String::new(),
            name_cursor: 0,
            mode: RoomMode::Create,
            ticket: String::new(),
            ticket_cursor: 0,
            error: None,
            should_quit: false,
        }
    }

    /// Cycle focus to the next form field.
    ///
    /// `match` in Rust is *exhaustive* — the compiler requires you to handle
    /// every possible variant. This prevents bugs where you add a new field
    /// but forget to update the navigation logic.
    fn next_field(&mut self) {
        self.field = match self.field {
            WelcomeField::Name => WelcomeField::Mode,
            WelcomeField::Mode => {
                if self.mode == RoomMode::Join {
                    WelcomeField::Ticket
                } else {
                    // In Create mode, skip Ticket (it's not relevant) and wrap
                    WelcomeField::Name
                }
            }
            WelcomeField::Ticket => WelcomeField::Name,
        };
    }

    /// Cycle focus to the previous form field.
    fn prev_field(&mut self) {
        self.field = match self.field {
            WelcomeField::Name => {
                if self.mode == RoomMode::Join {
                    WelcomeField::Ticket
                } else {
                    WelcomeField::Mode
                }
            }
            WelcomeField::Mode => WelcomeField::Name,
            WelcomeField::Ticket => WelcomeField::Mode,
        };
    }
}

/// The result returned by the welcome screen to the caller.
///
/// This is `pub` because `main.rs` needs to match on it to determine the
/// chosen mode and extract the user's nickname/ticket.
///
/// Struct variants (like `Create { nickname }`) are a concise way to bundle
/// related data — no need for separate result types per mode.
pub enum WelcomeResult {
    Create { nickname: String },
    Join { nickname: String, ticket: String },
}

// ── UI rendering ────────────────────────────────────────────────────────────
//
// This function builds the welcome dialog as a centered "card" widget.
// It's called every frame (50ms) and does not mutate state — only reads it.

/// Render the welcome form into a terminal frame.
///
/// `&WelcomeState` is an immutable borrow. The function can read all fields
/// but cannot modify any of them. This is enforced at compile time.
fn ui_welcome(f: &mut ratatui::Frame, state: &WelcomeState) {
    let area = f.area();

    // Centered card: 52 wide, 14 tall
    let card_w: u16 = 52;
    let card_h: u16 = 14;
    // `saturating_sub` prevents underflow — returns 0 if the subtraction
    // would go negative. This is safer than `wrapping_sub` (which wraps to
    // u16::MAX) or plain `-` (which panics in debug mode on underflow).
    let x = area.width.saturating_sub(card_w) / 2;
    let y = area.height.saturating_sub(card_h) / 2;
    let card = Rect::new(x, y, card_w.min(area.width), card_h.min(area.height));

    // `Clear` erases the card area so we get a clean background
    f.render_widget(Clear, card);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" piper-chat ")
        .title_alignment(Alignment::Center);
    f.render_widget(block, card);

    let inner = Rect::new(
        card.x + 2,
        card.y + 1,
        card.width.saturating_sub(4),
        card.height.saturating_sub(2),
    );

    // Build up the form line by line. `Vec<Line>` is a dynamically-sized
    // array (growable, heap-allocated). We push lines as we go.
    let mut lines: Vec<Line> = Vec::new();

    // Subtitle
    lines.push(Line::from(Span::styled(
        "P2P terminal chat over iroh gossip",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));

    // ── Name field ───────────────────────────────────────────────────────

    // Highlight the active field's label with cyan/bold; others are plain white.
    let name_style = if state.field == WelcomeField::Name {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let name_label = if state.field == WelcomeField::Name {
        "> Name: "
    } else {
        "  Name: "
    };
    // A `Line` made of multiple `Span`s — each Span has its own style.
    // This is how ratatui does inline styling (like HTML <span> tags).
    lines.push(Line::from(vec![
        Span::styled(name_label, name_style),
        Span::styled(&state.name, Style::default().fg(Color::White)),
        if state.field == WelcomeField::Name {
            Span::styled("_", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(""));

    // ── Mode field ───────────────────────────────────────────────────────

    let mode_style = if state.field == WelcomeField::Mode {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let mode_label = if state.field == WelcomeField::Mode {
        "> Mode: "
    } else {
        "  Mode: "
    };
    // Destructuring a tuple: `let (a, b) = expr;` binds both values at once.
    // The selected mode gets a highlighted style (black text on cyan bg).
    let (create_style, join_style) = match state.mode {
        RoomMode::Create => (
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::DarkGray),
        ),
        RoomMode::Join => (
            Style::default().fg(Color::DarkGray),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(mode_label, mode_style),
        Span::styled(" Create ", create_style),
        Span::raw("  "),
        Span::styled(" Join ", join_style),
    ]));
    lines.push(Line::from(""));

    // ── Ticket field ─────────────────────────────────────────────────────

    // The ticket field is only active in Join mode; otherwise it's grayed out.
    let ticket_active = state.mode == RoomMode::Join;
    let ticket_style = if !ticket_active {
        Style::default().fg(Color::DarkGray)
    } else if state.field == WelcomeField::Ticket {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let ticket_label = if state.field == WelcomeField::Ticket {
        "> Ticket: "
    } else {
        "  Ticket: "
    };

    // Tickets are long base32 strings — show a scrolling window of 30 chars.
    // `String` slicing with `[start..end]` works on byte indices; this is safe
    // because base32 is pure ASCII.
    let ticket_display: String = if state.ticket.len() > 30 {
        let start = state.ticket_cursor.saturating_sub(15);
        let end = (start + 30).min(state.ticket.len());
        let start = end.saturating_sub(30);
        format!("{}...", &state.ticket[start..end])
    } else {
        state.ticket.clone()
    };

    lines.push(Line::from(vec![
        Span::styled(ticket_label, ticket_style),
        Span::styled(
            &ticket_display,
            if ticket_active {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        if state.field == WelcomeField::Ticket && ticket_active {
            Span::styled("_", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(""));

    // ── Error or hint line ───────────────────────────────────────────────

    // `if let Some(err) = &state.error` is Rust's *refutable pattern match* —
    // it tries to match `Some(...)` and binds the inner value, or falls through
    // to `else`. This is more concise than `match` when you only care about
    // one variant.
    if let Some(err) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                "  Enter",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to start  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" next field  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ]));
    }

    let widget = Paragraph::new(lines);
    f.render_widget(widget, inner);

    // Position the terminal cursor in the active text field so the user sees
    // where their typing will appear.
    match state.field {
        WelcomeField::Name => {
            f.set_cursor_position((inner.x + 8 + state.name_cursor as u16, inner.y + 2));
        }
        WelcomeField::Ticket if state.mode == RoomMode::Join => {
            let display_cursor = if state.ticket.len() > 30 {
                let start = state.ticket_cursor.saturating_sub(15);
                (state.ticket_cursor - start).min(30) as u16
            } else {
                state.ticket_cursor as u16
            };
            f.set_cursor_position((inner.x + 10 + display_cursor, inner.y + 6));
        }
        // `_ => {}` is the catch-all arm — handles Mode field and Ticket
        // field when not in Join mode (no visible cursor needed).
        _ => {}
    }
}

// ── Key handling ────────────────────────────────────────────────────────────
//
// Key events are dispatched first to global actions (Esc, Tab, Enter), then
// to field-specific handlers. This two-level dispatch keeps the code organized.

/// Handle a key press in the welcome form.
///
/// `&mut WelcomeState` is a mutable borrow — this function can modify any
/// field of the state struct. Rust's borrow checker ensures no other code
/// can access the state while this function holds the `&mut` reference.
fn handle_welcome_key(state: &mut WelcomeState, key: crossterm::event::KeyEvent) {
    // Clear any previous error on new input
    state.error = None;

    match key.code {
        KeyCode::Esc => state.should_quit = true,
        KeyCode::Tab => {
            // `.contains()` checks a bitflag — KeyModifiers is a bitfield,
            // not an enum, so multiple modifiers can be active simultaneously.
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                state.prev_field();
            } else {
                state.next_field();
            }
        }
        KeyCode::Down => state.next_field(),
        KeyCode::Up => state.prev_field(),
        KeyCode::BackTab => state.prev_field(),
        KeyCode::Enter => {
            // Validate the form before allowing submission.
            // `.trim()` returns a `&str` slice without leading/trailing whitespace.
            // `.to_string()` converts it to an owned `String`.
            let name = state.name.trim().to_string();
            if name.is_empty() {
                state.error = Some("Name cannot be empty".into());
                return;
            }
            if state.mode == RoomMode::Join && state.ticket.trim().is_empty() {
                state.error = Some("Ticket is required to join".into());
                return;
            }
            // Fully-qualified trait method syntax: `<Type as Trait>::method()`
            // This is needed because `deserialize` is a method on the `Ticket`
            // trait, and Rust needs to know which trait implementation to call.
            // Also known as "turbofish" or UFCS (Universal Function Call Syntax).
            if state.mode == RoomMode::Join
                && <ChatTicket as Ticket>::deserialize(state.ticket.trim()).is_err()
            {
                state.error = Some("Invalid ticket format".into());
            }
            // If no error was set, the caller (run_welcome_screen) will detect
            // Enter + no error and break out of the event loop.
        }
        _ => {
            // Dispatch to the currently focused field's handler.
            // `match` on `state.field` routes input to the right place.
            match state.field {
                WelcomeField::Name => {
                    handle_text_input(&mut state.name, &mut state.name_cursor, key);
                }
                WelcomeField::Mode => {
                    // The `|` in match arms means "or" — matches any of the listed patterns.
                    match key.code {
                        KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Char('h')
                        | KeyCode::Char('l') => {
                            // Toggle between Create and Join
                            state.mode = match state.mode {
                                RoomMode::Create => RoomMode::Join,
                                RoomMode::Join => RoomMode::Create,
                            };
                            // If switching away from Join, move focus off the Ticket field
                            if state.mode == RoomMode::Create
                                && state.field == WelcomeField::Ticket
                            {
                                state.field = WelcomeField::Mode;
                            }
                        }
                        _ => {}
                    }
                }
                WelcomeField::Ticket => {
                    if state.mode == RoomMode::Join {
                        handle_text_input(&mut state.ticket, &mut state.ticket_cursor, key);
                    }
                }
            }
        }
    }
}

/// Handle text input for a single-line text field.
///
/// This function is *generic over which field it operates on* by accepting
/// separate `&mut String` and `&mut usize` references. This avoids duplicating
/// the insert/delete/cursor logic for the Name and Ticket fields.
///
/// `&mut String` lets us insert and remove characters in-place.
/// `&mut usize` lets us update the cursor position.
fn handle_text_input(text: &mut String, cursor: &mut usize, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            // `String::insert` inserts a char at a byte index. For ASCII input
            // byte index == char index, so this is safe.
            text.insert(*cursor, c);
            *cursor += 1;
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                *cursor -= 1;
                // `String::remove` removes the char at the given byte index and
                // shifts all subsequent bytes left. O(n) but fine for short inputs.
                text.remove(*cursor);
            }
        }
        KeyCode::Left => {
            // `saturating_sub` clamps at 0 instead of panicking on underflow.
            *cursor = cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            if *cursor < text.len() {
                *cursor += 1;
            }
        }
        _ => {}
    }
}

// ── Public entry point ──────────────────────────────────────────────────────

/// Run the interactive welcome screen and return the user's choice.
///
/// This is an `async fn` — it returns a `Future` that must be `.await`ed.
/// The `async` keyword lets us use `tokio::select!` and `.await` inside.
///
/// Returns `Ok(Some(result))` if the user submitted the form, `Ok(None)` if
/// they pressed Esc to quit, or `Err(...)` on terminal I/O errors.
///
/// `Option<WelcomeResult>` nested inside `Result` is a common Rust pattern:
/// `Result` handles errors, `Option` handles "no value" — they compose cleanly.
pub async fn run_welcome_screen() -> Result<Option<WelcomeResult>> {
    // Enable raw mode: keys are delivered immediately (no line buffering) and
    // aren't echoed. `?` propagates any error to the caller.
    enable_raw_mode()?;
    // `execute!` writes the `EnterAlternateScreen` command to stdout, which
    // switches to the alternate screen buffer (preserving the original terminal
    // contents for when we leave).
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let mut state = WelcomeState::new();
    // `EventStream` is an async stream of terminal events (keys, mouse, resize).
    let mut events = EventStream::new();
    // `interval` creates an async timer that ticks every 50ms — used to drive
    // UI redraws even when no input arrives (e.g. for animations or clock updates).
    let mut tick = interval(Duration::from_millis(50));

    // `loop` with `break value` is Rust's "loop that returns a value" pattern.
    // The `break None` / `break Some(...)` at various points all produce
    // `Option<WelcomeResult>` which is bound to `result`.
    let result = loop {
        // Draw the current frame. The closure `|f| ui_welcome(f, &state)`
        // captures `&state` by reference — closures in Rust automatically
        // borrow their environment.
        terminal.draw(|f| ui_welcome(f, &state))?;

        // `tokio::select!` waits for the *first* of multiple async operations
        // to complete, then executes the corresponding branch. Other branches
        // are cancelled. This is how we multiplex keyboard input and timer ticks
        // without threads.
        tokio::select! {
            ev = events.next() => {
                // Nested pattern match: `Some(Ok(TermEvent::Key(key)))` unwraps
                // three layers at once — the Option from the stream, the Result
                // from event reading, and the Event variant.
                if let Some(Ok(TermEvent::Key(key))) = ev {
                    // Filter out key release/repeat events (Windows sends both
                    // press and release events).
                    if key.kind != KeyEventKind::Press { continue; }

                    handle_welcome_key(&mut state, key);

                    if state.should_quit {
                        break None;
                    }

                    // Check if Enter was pressed and validation passed
                    if key.code == KeyCode::Enter && state.error.is_none() {
                        let nickname = state.name.trim().to_string();
                        break match state.mode {
                            RoomMode::Create => Some(WelcomeResult::Create { nickname }),
                            RoomMode::Join => Some(WelcomeResult::Join {
                                nickname,
                                ticket: state.ticket.trim().to_string(),
                            }),
                        };
                    }
                }
            }
            // The tick branch just triggers a redraw (the `terminal.draw()`
            // call at the top of the loop).
            _ = tick.tick() => {}
        }
    };

    // Restore the terminal to its original state before returning.
    // This runs even on early `break` — Rust's control flow ensures cleanup.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    Ok(result)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    /// Helper to create a simple key press event with no modifiers.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Helper to create a key press event with specific modifiers.
    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    // ── WelcomeState navigation tests ────────────────────────────────────

    #[test]
    fn initial_state() {
        let state = WelcomeState::new();
        assert_eq!(state.field, WelcomeField::Name);
        assert_eq!(state.mode, RoomMode::Create);
        assert!(state.name.is_empty());
        assert!(state.ticket.is_empty());
        assert!(state.error.is_none());
        assert!(!state.should_quit);
    }

    #[test]
    fn next_field_in_create_mode() {
        let mut state = WelcomeState::new();
        // Create mode: Name → Mode → Name (skips Ticket)
        assert_eq!(state.field, WelcomeField::Name);
        state.next_field();
        assert_eq!(state.field, WelcomeField::Mode);
        state.next_field();
        assert_eq!(state.field, WelcomeField::Name); // wraps around
    }

    #[test]
    fn next_field_in_join_mode() {
        let mut state = WelcomeState::new();
        state.mode = RoomMode::Join;
        // Join mode: Name → Mode → Ticket → Name
        assert_eq!(state.field, WelcomeField::Name);
        state.next_field();
        assert_eq!(state.field, WelcomeField::Mode);
        state.next_field();
        assert_eq!(state.field, WelcomeField::Ticket);
        state.next_field();
        assert_eq!(state.field, WelcomeField::Name); // wraps around
    }

    #[test]
    fn prev_field_in_create_mode() {
        let mut state = WelcomeState::new();
        // Create mode: Name → Mode → Name (backwards)
        assert_eq!(state.field, WelcomeField::Name);
        state.prev_field();
        assert_eq!(state.field, WelcomeField::Mode); // wraps around
        state.prev_field();
        assert_eq!(state.field, WelcomeField::Name);
    }

    #[test]
    fn prev_field_in_join_mode() {
        let mut state = WelcomeState::new();
        state.mode = RoomMode::Join;
        // Join mode backwards: Name → Ticket → Mode → Name
        assert_eq!(state.field, WelcomeField::Name);
        state.prev_field();
        assert_eq!(state.field, WelcomeField::Ticket);
        state.prev_field();
        assert_eq!(state.field, WelcomeField::Mode);
        state.prev_field();
        assert_eq!(state.field, WelcomeField::Name);
    }

    // ── handle_text_input tests ──────────────────────────────────────────

    #[test]
    fn text_input_char_insertion() {
        let mut text = String::new();
        let mut cursor = 0;
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Char('a')));
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Char('b')));
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Char('c')));
        assert_eq!(text, "abc");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn text_input_backspace() {
        let mut text = "hello".to_string();
        let mut cursor = 5;
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Backspace));
        assert_eq!(text, "hell");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn text_input_backspace_at_start() {
        let mut text = "hello".to_string();
        let mut cursor = 0;
        // Backspace at position 0 should be a no-op
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Backspace));
        assert_eq!(text, "hello");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn text_input_cursor_movement() {
        let mut text = "abc".to_string();
        let mut cursor = 3;
        // Move left twice
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Left));
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Left));
        assert_eq!(cursor, 1);
        // Move right once
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Right));
        assert_eq!(cursor, 2);
    }

    #[test]
    fn text_input_cursor_clamped() {
        let mut text = "ab".to_string();
        let mut cursor = 0;
        // Left at position 0 stays at 0
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Left));
        assert_eq!(cursor, 0);
        // Right past end stays at end
        cursor = 2;
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Right));
        assert_eq!(cursor, 2);
    }

    #[test]
    fn text_input_insert_in_middle() {
        let mut text = "ac".to_string();
        let mut cursor = 1;
        handle_text_input(&mut text, &mut cursor, key(KeyCode::Char('b')));
        assert_eq!(text, "abc");
        assert_eq!(cursor, 2);
    }

    // ── handle_welcome_key tests ─────────────────────────────────────────

    #[test]
    fn esc_sets_should_quit() {
        let mut state = WelcomeState::new();
        handle_welcome_key(&mut state, key(KeyCode::Esc));
        assert!(state.should_quit);
    }

    #[test]
    fn tab_advances_field() {
        let mut state = WelcomeState::new();
        handle_welcome_key(&mut state, key(KeyCode::Tab));
        assert_eq!(state.field, WelcomeField::Mode);
    }

    #[test]
    fn shift_tab_goes_back() {
        let mut state = WelcomeState::new();
        state.field = WelcomeField::Mode;
        handle_welcome_key(&mut state, key_with(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(state.field, WelcomeField::Name);
    }

    #[test]
    fn enter_with_empty_name_sets_error() {
        let mut state = WelcomeState::new();
        handle_welcome_key(&mut state, key(KeyCode::Enter));
        assert!(state.error.is_some());
        assert!(state.error.unwrap().contains("Name"));
    }

    #[test]
    fn enter_join_without_ticket_sets_error() {
        let mut state = WelcomeState::new();
        state.name = "Alice".into();
        state.name_cursor = 5;
        state.mode = RoomMode::Join;
        handle_welcome_key(&mut state, key(KeyCode::Enter));
        assert!(state.error.is_some());
        assert!(state.error.unwrap().contains("Ticket"));
    }

    #[test]
    fn enter_join_with_invalid_ticket_sets_error() {
        let mut state = WelcomeState::new();
        state.name = "Alice".into();
        state.name_cursor = 5;
        state.mode = RoomMode::Join;
        state.ticket = "not-a-valid-ticket".into();
        state.ticket_cursor = 18;
        handle_welcome_key(&mut state, key(KeyCode::Enter));
        assert!(state.error.is_some());
        assert!(state.error.unwrap().contains("Invalid"));
    }

    #[test]
    fn enter_create_with_name_passes_validation() {
        let mut state = WelcomeState::new();
        state.name = "Alice".into();
        state.name_cursor = 5;
        state.mode = RoomMode::Create;
        handle_welcome_key(&mut state, key(KeyCode::Enter));
        // No error means validation passed
        assert!(state.error.is_none());
    }

    #[test]
    fn typing_in_name_field() {
        let mut state = WelcomeState::new();
        assert_eq!(state.field, WelcomeField::Name);
        handle_welcome_key(&mut state, key(KeyCode::Char('A')));
        handle_welcome_key(&mut state, key(KeyCode::Char('l')));
        assert_eq!(state.name, "Al");
        assert_eq!(state.name_cursor, 2);
    }

    #[test]
    fn mode_toggle_with_arrow_keys() {
        let mut state = WelcomeState::new();
        state.field = WelcomeField::Mode;
        assert_eq!(state.mode, RoomMode::Create);
        handle_welcome_key(&mut state, key(KeyCode::Right));
        assert_eq!(state.mode, RoomMode::Join);
        handle_welcome_key(&mut state, key(KeyCode::Left));
        assert_eq!(state.mode, RoomMode::Create);
    }

    #[test]
    fn key_press_clears_previous_error() {
        let mut state = WelcomeState::new();
        state.error = Some("old error".into());
        handle_welcome_key(&mut state, key(KeyCode::Char('a')));
        // Any key press clears the error
        assert!(state.error.is_none());
    }
}
