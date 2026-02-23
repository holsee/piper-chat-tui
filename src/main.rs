use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{Event as TermEvent, EventStream, KeyCode, KeyEventKind},
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
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, interval};

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "piper-chat")]
enum Cli {
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

// ── App state ────────────────────────────────────────────────────────────────

enum ChatLine {
    System(String),
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

    let (nickname, ticket) = match &cli {
        Cli::Create { name } => (name.clone(), ChatTicket::new_random()),
        Cli::Join { name, ticket } => {
            let t = <ChatTicket as Ticket>::deserialize(ticket)?;
            (name.clone(), t)
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

    // Print ticket before TUI takes over (for copy-paste)
    println!("Ticket (share with others to join):\n\n{ticket_str}\n");
    println!("Press ENTER to start chat...");
    let _ = std::io::stdin().read_line(&mut String::new());

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
    app.system(format!("ticket: {ticket_str}"));
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
