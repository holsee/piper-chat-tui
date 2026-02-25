use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use iroh::endpoint::{AfterHandshakeOutcome, ConnectionInfo, EndpointHooks};
use iroh::EndpointId;
use iroh_gossip::{
    api::Event as GossipEvent,
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use iroh_tickets::Ticket;
use n0_future::StreamExt;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, interval};

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "piper-chat", about = "P2P terminal chat over iroh gossip")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Create a new chat room
    Create {
        /// Your display name
        #[arg(short, long)]
        name: String,
    },
    /// Join an existing chat room
    Join {
        /// Your display name
        #[arg(short, long)]
        name: String,
        /// Ticket string from the room creator
        ticket: String,
    },
}

// ── Wire protocol ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
enum Message {
    Join {
        nickname: String,
        endpoint_id: EndpointId,
    },
    Chat {
        nickname: String,
        text: String,
    },
}

// ── Ticket ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
struct ChatTicket {
    topic_id: TopicId,
    bootstrap: BTreeSet<EndpointId>,
}

impl ChatTicket {
    fn new_random() -> Self {
        Self {
            topic_id: TopicId::from_bytes(rand::random()),
            bootstrap: BTreeSet::new(),
        }
    }
}

impl Ticket for ChatTicket {
    const KIND: &'static str = "chat";

    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap()
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

// ── Connection tracking ─────────────────────────────────────────────────

enum ConnType {
    Unknown,
    Direct,
    Relay,
}

struct PeerInfo {
    name: String,
    conn_type: ConnType,
}

#[derive(Debug)]
struct ConnTracker(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

impl ConnTracker {
    fn new() -> Self {
        Self(Arc::default())
    }

    fn hook(&self) -> ConnTrackerHook {
        ConnTrackerHook(self.0.clone())
    }

    fn conn_type(&self, id: &EndpointId) -> ConnType {
        let map = self.0.read().unwrap();
        match map.get(id).and_then(|c| c.selected_path()) {
            Some(p) if p.is_ip() => ConnType::Direct,
            Some(_) => ConnType::Relay,
            None => ConnType::Unknown,
        }
    }
}

#[derive(Debug)]
struct ConnTrackerHook(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

impl EndpointHooks for ConnTrackerHook {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a ConnectionInfo,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        self.0.write().unwrap().insert(conn.remote_id(), conn.clone());
        async { AfterHandshakeOutcome::accept() }
    }
}

// ── Welcome screen ──────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum WelcomeField {
    Name,
    Mode,
    Ticket,
}

#[derive(PartialEq, Clone, Copy)]
enum RoomMode {
    Create,
    Join,
}

struct WelcomeState {
    field: WelcomeField,
    name: String,
    name_cursor: usize,
    mode: RoomMode,
    ticket: String,
    ticket_cursor: usize,
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

    fn next_field(&mut self) {
        self.field = match self.field {
            WelcomeField::Name => WelcomeField::Mode,
            WelcomeField::Mode => {
                if self.mode == RoomMode::Join {
                    WelcomeField::Ticket
                } else {
                    WelcomeField::Name
                }
            }
            WelcomeField::Ticket => WelcomeField::Name,
        };
    }

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

enum WelcomeResult {
    Create { nickname: String },
    Join { nickname: String, ticket: String },
}

fn ui_welcome(f: &mut ratatui::Frame, state: &WelcomeState) {
    let area = f.area();

    // Centered card: 50 wide, 14 tall
    let card_w: u16 = 52;
    let card_h: u16 = 14;
    let x = area.width.saturating_sub(card_w) / 2;
    let y = area.height.saturating_sub(card_h) / 2;
    let card = Rect::new(x, y, card_w.min(area.width), card_h.min(area.height));

    // Clear background and draw border
    f.render_widget(Clear, card);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" piper-chat ")
        .title_alignment(Alignment::Center);
    f.render_widget(block, card);

    let inner = Rect::new(card.x + 2, card.y + 1, card.width.saturating_sub(4), card.height.saturating_sub(2));

    let mut lines: Vec<Line> = Vec::new();

    // Subtitle
    lines.push(Line::from(Span::styled(
        "P2P terminal chat over iroh gossip",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));

    // Name field
    let name_style = if state.field == WelcomeField::Name {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let name_label = if state.field == WelcomeField::Name { "> Name: " } else { "  Name: " };
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

    // Mode field
    let mode_style = if state.field == WelcomeField::Mode {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let mode_label = if state.field == WelcomeField::Mode { "> Mode: " } else { "  Mode: " };
    let (create_style, join_style) = match state.mode {
        RoomMode::Create => (
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::DarkGray),
        ),
        RoomMode::Join => (
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(mode_label, mode_style),
        Span::styled(" Create ", create_style),
        Span::raw("  "),
        Span::styled(" Join ", join_style),
    ]));
    lines.push(Line::from(""));

    // Ticket field
    let ticket_active = state.mode == RoomMode::Join;
    let ticket_style = if !ticket_active {
        Style::default().fg(Color::DarkGray)
    } else if state.field == WelcomeField::Ticket {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let ticket_label = if state.field == WelcomeField::Ticket { "> Ticket: " } else { "  Ticket: " };

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
            if ticket_active { Style::default().fg(Color::White) } else { Style::default().fg(Color::DarkGray) },
        ),
        if state.field == WelcomeField::Ticket && ticket_active {
            Span::styled("_", Style::default().fg(Color::DarkGray))
        } else {
            Span::raw("")
        },
    ]));
    lines.push(Line::from(""));

    // Error or hint line
    if let Some(err) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                "  Enter",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to start  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Tab",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" next field  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Esc",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(Color::DarkGray)),
        ]));
    }

    let widget = Paragraph::new(lines);
    f.render_widget(widget, inner);

    // Position cursor in the active text field
    match state.field {
        WelcomeField::Name => {
            f.set_cursor_position((
                inner.x + 8 + state.name_cursor as u16,
                inner.y + 2,
            ));
        }
        WelcomeField::Ticket if state.mode == RoomMode::Join => {
            let display_cursor = if state.ticket.len() > 30 {
                let start = state.ticket_cursor.saturating_sub(15);
                (state.ticket_cursor - start).min(30) as u16
            } else {
                state.ticket_cursor as u16
            };
            f.set_cursor_position((
                inner.x + 10 + display_cursor,
                inner.y + 6,
            ));
        }
        _ => {}
    }
}

fn handle_welcome_key(state: &mut WelcomeState, key: crossterm::event::KeyEvent) {
    state.error = None;

    match key.code {
        KeyCode::Esc => state.should_quit = true,
        KeyCode::Tab => {
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
            let name = state.name.trim().to_string();
            if name.is_empty() {
                state.error = Some("Name cannot be empty".into());
                return;
            }
            if state.mode == RoomMode::Join && state.ticket.trim().is_empty() {
                state.error = Some("Ticket is required to join".into());
                return;
            }
            if state.mode == RoomMode::Join {
                if <ChatTicket as Ticket>::deserialize(state.ticket.trim()).is_err() {
                    state.error = Some("Invalid ticket format".into());
                    return;
                }
            }
            // Valid — the main loop will read state and proceed
        }
        _ => {
            // Dispatch to active field
            match state.field {
                WelcomeField::Name => handle_text_input(&mut state.name, &mut state.name_cursor, key),
                WelcomeField::Mode => {
                    match key.code {
                        KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                            state.mode = match state.mode {
                                RoomMode::Create => RoomMode::Join,
                                RoomMode::Join => RoomMode::Create,
                            };
                            // If switching away from Join, move off Ticket field
                            if state.mode == RoomMode::Create && state.field == WelcomeField::Ticket {
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

fn handle_text_input(text: &mut String, cursor: &mut usize, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Char(c) => {
            text.insert(*cursor, c);
            *cursor += 1;
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                *cursor -= 1;
                text.remove(*cursor);
            }
        }
        KeyCode::Left => {
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

async fn run_welcome_screen() -> Result<Option<WelcomeResult>> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let mut state = WelcomeState::new();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(50));

    let result = loop {
        terminal.draw(|f| ui_welcome(f, &state))?;

        tokio::select! {
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = ev {
                    if key.kind != KeyEventKind::Press { continue; }

                    handle_welcome_key(&mut state, key);

                    if state.should_quit {
                        break None;
                    }

                    // Enter was pressed and validation passed (no error set)
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
            _ = tick.tick() => {}
        }
    };

    // Restore terminal before returning
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    Ok(result)
}

// ── App state ────────────────────────────────────────────────────────────────

enum ChatLine {
    System(String),
    Ticket(String),
    Chat { nickname: String, text: String },
}

struct App {
    messages: Vec<ChatLine>,
    input: String,
    cursor_pos: usize,
    should_quit: bool,
    peers: BTreeMap<EndpointId, PeerInfo>,
}

impl App {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            should_quit: false,
            peers: BTreeMap::new(),
        }
    }

    fn system(&mut self, msg: impl Into<String>) {
        self.messages.push(ChatLine::System(msg.into()));
    }

    fn ticket(&mut self, ticket: impl Into<String>) {
        self.messages.push(ChatLine::Ticket(ticket.into()));
    }

    fn chat(&mut self, nickname: String, text: String) {
        self.messages.push(ChatLine::Chat { nickname, text });
    }
}

// ── UI ───────────────────────────────────────────────────────────────────────

fn ui(f: &mut ratatui::Frame, app: &App) {
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area());
    let top = Layout::horizontal([Constraint::Min(1), Constraint::Length(24)]).split(rows[0]);

    // Messages pane (top left)
    let lines: Vec<Line> = app
        .messages
        .iter()
        .map(|msg| match msg {
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
                Span::styled(
                    ticket.as_str(),
                    Style::default().fg(Color::White),
                ),
            ]),
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

    let visible = top[0].height.saturating_sub(2) as usize;
    let scroll = lines.len().saturating_sub(visible) as u16;

    let messages_widget = Paragraph::new(lines)
        .scroll((scroll, 0))
        .block(Block::default().borders(Borders::ALL).title("piper-chat"));
    f.render_widget(messages_widget, top[0]);

    // Peers pane (top right)
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

    // Input pane (full width)
    let input_widget = Paragraph::new(format!("> {}", app.input))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(input_widget, rows[1]);

    // Cursor position
    f.set_cursor_position((
        rows[1].x + 2 + app.cursor_pos as u16,
        rows[1].y + 1,
    ));
}

// ── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let (nickname, ticket) = match cli.command {
        Some(Command::Create { name }) => (name, ChatTicket::new_random()),
        Some(Command::Join { name, ticket }) => {
            let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
            (name, t)
        }
        None => {
            match run_welcome_screen().await? {
                Some(WelcomeResult::Create { nickname }) => {
                    (nickname, ChatTicket::new_random())
                }
                Some(WelcomeResult::Join { nickname, ticket }) => {
                    let t = <ChatTicket as Ticket>::deserialize(&ticket)?;
                    (nickname, t)
                }
                None => return Ok(()), // User quit
            }
        }
    };

    // ── Networking ───────────────────────────────────────────────────────────

    let conn_tracker = ConnTracker::new();
    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .hooks(conn_tracker.hook())
        .bind()
        .await?;

    let gossip = Gossip::builder().spawn(endpoint.clone());

    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();

    // Build shareable ticket (includes our endpoint as bootstrap)
    let mut our_ticket = ticket.clone();
    our_ticket.bootstrap.insert(endpoint.id());
    let ticket_str = <ChatTicket as Ticket>::serialize(&our_ticket);

    // Subscribe to gossip topic
    let bootstrap: Vec<_> = ticket.bootstrap.iter().cloned().collect();
    let topic = gossip.subscribe(ticket.topic_id, bootstrap).await?;
    let (sender, mut receiver) = topic.split();

    // ── Terminal setup ───────────────────────────────────────────────────────

    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(
        std::io::stdout(),
    ))?;

    let our_id = endpoint.id();
    let mut app = App::new();
    app.peers.insert(our_id, PeerInfo {
        name: format!("{nickname} (you)"),
        conn_type: ConnType::Unknown,
    });
    app.ticket(ticket_str);
    app.system("share the ticket above with others to join");
    app.system("waiting for peers...");

    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(50));

    // ── Event loop ───────────────────────────────────────────────────────────

    loop {
        terminal.draw(|f| ui(f, &app))?;

        tokio::select! {
            ev = events.next() => {
                if let Some(Ok(TermEvent::Key(key))) = ev {
                    if key.kind != KeyEventKind::Press { continue; }
                    match key.code {
                        KeyCode::Esc => app.should_quit = true,
                        KeyCode::Enter => {
                            let text: String = app.input.drain(..).collect();
                            app.cursor_pos = 0;
                            if !text.is_empty() {
                                let msg = Message::Chat {
                                    nickname: nickname.clone(),
                                    text: text.clone(),
                                };
                                let encoded = postcard::to_stdvec(&msg)?;
                                sender.broadcast(encoded.into()).await?;
                                app.chat(nickname.clone(), text);
                            }
                        }
                        KeyCode::Backspace => {
                            if app.cursor_pos > 0 {
                                app.cursor_pos -= 1;
                                app.input.remove(app.cursor_pos);
                            }
                        }
                        KeyCode::Left => {
                            app.cursor_pos = app.cursor_pos.saturating_sub(1);
                        }
                        KeyCode::Right => {
                            if app.cursor_pos < app.input.len() {
                                app.cursor_pos += 1;
                            }
                        }
                        KeyCode::Char(c) => {
                            app.input.insert(app.cursor_pos, c);
                            app.cursor_pos += 1;
                        }
                        _ => {}
                    }
                }
            }

            msg = receiver.try_next() => {
                match msg {
                    Ok(Some(GossipEvent::Received(msg))) => {
                        match postcard::from_bytes(&msg.content) {
                            Ok(Message::Join { nickname: name, endpoint_id }) => {
                                app.system(format!("{name} joined"));
                                app.peers.insert(endpoint_id, PeerInfo {
                                    name,
                                    conn_type: ConnType::Unknown,
                                });
                            }
                            Ok(Message::Chat { nickname, text }) => {
                                app.chat(nickname, text);
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(Some(GossipEvent::NeighborUp(id))) => {
                        app.peers.insert(id, PeerInfo {
                            name: id.fmt_short().to_string(),
                            conn_type: ConnType::Unknown,
                        });
                        app.system(format!("peer connected: {}", id.fmt_short()));
                        // Announce ourselves so the new peer learns our name
                        let join = Message::Join {
                            nickname: nickname.clone(),
                            endpoint_id: our_id,
                        };
                        let encoded = postcard::to_stdvec(&join)?;
                        sender.broadcast(encoded.into()).await?;
                    }
                    Ok(Some(GossipEvent::NeighborDown(id))) => {
                        let name = app.peers.remove(&id).map(|p| p.name).unwrap_or_else(|| id.fmt_short().to_string());
                        app.system(format!("{name} left"));
                    }
                    Ok(Some(GossipEvent::Lagged)) => {
                        app.system("warning: gossip stream lagged");
                    }
                    Ok(None) => {
                        app.system("gossip stream closed");
                        app.should_quit = true;
                    }
                    Err(e) => {
                        app.system(format!("gossip error: {e}"));
                    }
                }
            }

            _ = tick.tick() => {
                for (id, peer) in &mut app.peers {
                    if *id != our_id {
                        peer.conn_type = conn_tracker.conn_type(id);
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // ── Restore terminal ─────────────────────────────────────────────────────

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    // ── Shutdown ─────────────────────────────────────────────────────────────

    router.shutdown().await?;
    endpoint.close().await;

    Ok(())
}
