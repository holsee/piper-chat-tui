//! Session API — bidirectional channel-based interface between frontends and core networking.
//!
//! The session spawns a background tokio task that owns all networking state
//! (gossip, blobs, history, peer tracking) and communicates with the frontend
//! via typed channels.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use iroh::EndpointId;
use iroh_blobs::{store::fs::FsStore, BlobsProtocol, Hash, HashAndFormat, ALPN as BLOBS_ALPN};
use serde::Serialize;
use iroh_gossip::{
    api::Event as GossipEvent,
    net::{Gossip, GOSSIP_ALPN},
};
use iroh_tickets::Ticket;
use n0_future::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};

use crate::protocol::{
    ChatTicket, ConnType, HistoryEntry, HistoryEntryKind, Message, MessageId, PeerInfo,
};
use crate::transfer::{FileOffer, TransferEvent, TransferManager};
use crate::util::{mime_from_extension, new_message_id, now_ms};

// ── Session types ────────────────────────────────────────────────────────────

/// Configuration for starting a session.
pub struct SessionConfig {
    pub nickname: String,
    pub ticket: ChatTicket,
}

/// Commands sent from the frontend to the session task.
#[derive(Debug)]
pub enum SessionCommand {
    SendChat { text: String },
    ShareFile { path: PathBuf, target: Option<String> },
    UnshareFile { hash: Hash },
    StartDownload { hash: Hash },
    Quit,
}

/// Events sent from the session task to the frontend.
#[derive(Debug, Clone, Serialize)]
pub enum SessionEvent {
    /// Session is ready — provides our endpoint ID and serialized ticket.
    Ready {
        our_id: EndpointId,
        ticket_str: String,
    },
    /// A peer joined the room.
    PeerJoined {
        endpoint_id: EndpointId,
        nickname: String,
    },
    /// A peer left the room.
    PeerLeft {
        endpoint_id: EndpointId,
        nickname: String,
    },
    /// A chat message was received from a remote peer.
    ChatReceived {
        nickname: String,
        text: String,
        message_id: MessageId,
        timestamp_ms: u64,
    },
    /// A chat message we sent was acknowledged (echo back to UI).
    ChatSent {
        nickname: String,
        text: String,
        message_id: MessageId,
        timestamp_ms: u64,
    },
    /// A file offer was received from a remote peer.
    FileOffered {
        offer: FileOffer,
        message_id: MessageId,
        timestamp_ms: u64,
        target: Option<String>,
    },
    /// A file was retracted by a remote peer.
    FileRetracted {
        nickname: String,
        hash: Hash,
        filename: Option<String>,
    },
    /// We successfully shared a file.
    FileShared {
        offer: FileOffer,
        target: Option<String>,
    },
    /// Sharing a file failed.
    FileShareFailed {
        error: String,
    },
    /// Download progress update.
    TransferProgress {
        hash: Hash,
        bytes_received: u64,
        total_bytes: u64,
    },
    /// Download completed.
    TransferComplete {
        hash: Hash,
        filename: String,
        path: PathBuf,
    },
    /// Download failed.
    TransferFailed {
        hash: Hash,
        filename: String,
        error: String,
    },
    /// Connection type changed for a peer.
    ConnTypeChanged {
        endpoint_id: EndpointId,
        conn_type: ConnType,
    },
    /// History was synced from a peer.
    HistorySynced {
        entries: Vec<HistoryEntry>,
        merged_count: u32,
    },
    /// System message from the session.
    System {
        message: String,
    },
    /// Session disconnected.
    Disconnected {
        reason: String,
    },
}

/// Handle for communicating with a running session.
pub struct SessionHandle {
    pub cmd_tx: mpsc::Sender<SessionCommand>,
    pub event_rx: mpsc::Receiver<SessionEvent>,
    shutdown: Option<tokio::task::JoinHandle<()>>,
}

impl SessionHandle {
    /// Wait for the session task to finish.
    pub async fn join(mut self) {
        if let Some(handle) = self.shutdown.take() {
            let _ = handle.await;
        }
    }
}

/// Start a new session. Spawns a background task and returns a handle.
pub async fn start_session(config: SessionConfig) -> Result<SessionHandle> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCommand>(64);
    let (event_tx, event_rx) = mpsc::channel::<SessionEvent>(256);

    // ── Networking setup ─────────────────────────────────────────────────

    let endpoint = iroh::Endpoint::builder()
        .alpns(vec![GOSSIP_ALPN.to_vec(), BLOBS_ALPN.to_vec()])
        .bind()
        .await?;

    let blob_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piper-chat")
        .join("blobs")
        .join(endpoint.id().fmt_short().to_string());
    let blob_store = FsStore::load(&blob_dir).await?;

    let gossip = Gossip::builder().spawn(endpoint.clone());
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);

    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .accept(BLOBS_ALPN, blobs_protocol)
        .spawn();

    let mut our_ticket = config.ticket.clone();
    our_ticket.bootstrap.insert(endpoint.id());
    let ticket_str = <ChatTicket as Ticket>::serialize(&our_ticket);

    let bootstrap: Vec<_> = config.ticket.bootstrap.iter().cloned().collect();
    let topic = gossip.subscribe(config.ticket.topic_id, bootstrap).await?;
    let (sender, receiver) = topic.split();

    // Download directory
    let download_dir = PathBuf::from("./piper-files");
    tokio::fs::create_dir_all(&download_dir).await?;
    let download_dir = download_dir.canonicalize()?;

    let our_id = endpoint.id();

    // Send Ready event
    let _ = event_tx
        .send(SessionEvent::Ready {
            our_id,
            ticket_str,
        })
        .await;

    // Spawn the session task
    let nickname = config.nickname;
    let handle = tokio::spawn(async move {
        session_task(
            nickname, our_id, endpoint, blob_store, sender, receiver, router, cmd_rx, event_tx,
            download_dir,
        )
        .await;
    });

    Ok(SessionHandle {
        cmd_tx,
        event_rx,
        shutdown: Some(handle),
    })
}

/// The main session task — runs the event loop.
#[allow(clippy::too_many_arguments)]
async fn session_task(
    nickname: String,
    our_id: EndpointId,
    endpoint: iroh::Endpoint,
    blob_store: FsStore,
    sender: iroh_gossip::api::GossipSender,
    mut receiver: iroh_gossip::api::GossipReceiver,
    router: iroh::protocol::Router,
    mut cmd_rx: mpsc::Receiver<SessionCommand>,
    event_tx: mpsc::Sender<SessionEvent>,
    download_dir: PathBuf,
) {
    // Internal state
    let mut transfers = TransferManager::new();
    let mut history: Vec<HistoryEntry> = Vec::new();
    let mut seen_ids: HashSet<MessageId> = HashSet::new();
    let mut history_synced = false;
    let mut peers: BTreeMap<EndpointId, PeerInfo> = BTreeMap::new();

    // Add ourselves
    peers.insert(
        our_id,
        PeerInfo {
            name: format!("{nickname} (you)"),
            conn_type: ConnType::You,
        },
    );

    // Transfer events channel (internal)
    let (transfer_tx, mut transfer_rx) = mpsc::channel::<TransferEvent>(64);

    // History sync channel (internal)
    let (history_tx, mut history_rx) = mpsc::channel::<Result<Vec<u8>, String>>(4);

    let mut tick = interval(Duration::from_millis(50));

    loop {
        tokio::select! {
            // ── Commands from frontend ──────────────────────────────────
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCommand::SendChat { text }) => {
                        let mid = new_message_id();
                        let ts = now_ms();
                        let msg = Message::Chat {
                            nickname: nickname.clone(),
                            text: text.clone(),
                            message_id: mid,
                            timestamp_ms: ts,
                        };
                        if let Ok(encoded) = postcard::to_stdvec(&msg) {
                            let _ = sender.broadcast(encoded.into()).await;
                        }
                        seen_ids.insert(mid);
                        push_history(&mut history, HistoryEntry {
                            message_id: mid,
                            timestamp_ms: ts,
                            kind: HistoryEntryKind::Chat {
                                nickname: nickname.clone(),
                                text: text.clone(),
                            },
                        });
                        let _ = event_tx.send(SessionEvent::ChatSent {
                            nickname: nickname.clone(),
                            text,
                            message_id: mid,
                            timestamp_ms: ts,
                        }).await;
                    }
                    Some(SessionCommand::ShareFile { path, target }) => {
                        match share_file(
                            &blob_store,
                            &sender,
                            &nickname,
                            our_id,
                            &path,
                            target.clone(),
                            &mut seen_ids,
                            &mut history,
                        ).await {
                            Ok((hash, filename, size, _mid, _ts, _mime_type)) => {
                                let offer = FileOffer {
                                    sender_nickname: "You".to_string(),
                                    sender_id: our_id,
                                    filename,
                                    size,
                                    hash,
                                };
                                transfers.add_sent(offer.clone());
                                let _ = event_tx.send(SessionEvent::FileShared {
                                    offer,
                                    target,
                                }).await;
                            }
                            Err(e) => {
                                let _ = event_tx.send(SessionEvent::FileShareFailed {
                                    error: format!("{e}"),
                                }).await;
                            }
                        }
                    }
                    Some(SessionCommand::UnshareFile { hash }) => {
                        // Find the entry to get its filename
                        let hash_bytes = *hash.as_bytes();
                        let mid = new_message_id();
                        let ts = now_ms();
                        let msg = Message::FileRetract {
                            nickname: nickname.clone(),
                            hash: hash_bytes,
                            message_id: mid,
                            timestamp_ms: ts,
                        };
                        if let Ok(encoded) = postcard::to_stdvec(&msg) {
                            let _ = sender.broadcast(encoded.into()).await;
                        }
                        let filename = transfers.retract(&hash);
                        seen_ids.insert(mid);
                        // Remove matching FileOffer entries from history.
                        history.retain(|e| {
                            !matches!(&e.kind, HistoryEntryKind::FileOffer { hash: h, .. } if *h == hash_bytes)
                        });
                        push_history(&mut history, HistoryEntry {
                            message_id: mid,
                            timestamp_ms: ts,
                            kind: HistoryEntryKind::FileRetract { hash: hash_bytes },
                        });
                        let _ = event_tx.send(SessionEvent::FileRetracted {
                            nickname: "You".to_string(),
                            hash,
                            filename,
                        }).await;
                    }
                    Some(SessionCommand::StartDownload { hash }) => {
                        if let Some(entry) = transfers.entries.iter().find(|e| e.offer.hash == hash) {
                            let offer = entry.offer.clone();
                            transfers.start_download(&hash);
                            spawn_download(
                                &blob_store,
                                &endpoint,
                                offer,
                                download_dir.clone(),
                                transfer_tx.clone(),
                            );
                        }
                    }
                    Some(SessionCommand::Quit) | None => {
                        break;
                    }
                }
            }

            // ── Gossip events ───────────────────────────────────────────
            msg = receiver.try_next() => {
                match msg {
                    Ok(Some(GossipEvent::Received(msg))) => {
                        match postcard::from_bytes(&msg.content) {
                            Ok(Message::Join { nickname: name, endpoint_id }) => {
                                peers.insert(endpoint_id, PeerInfo {
                                    name: name.clone(),
                                    conn_type: ConnType::Unknown,
                                });
                                let _ = event_tx.send(SessionEvent::PeerJoined {
                                    endpoint_id,
                                    nickname: name,
                                }).await;
                            }
                            Ok(Message::Chat { nickname: name, text, message_id, timestamp_ms }) => {
                                if !seen_ids.contains(&message_id) {
                                    seen_ids.insert(message_id);
                                    push_history(&mut history, HistoryEntry {
                                        message_id,
                                        timestamp_ms,
                                        kind: HistoryEntryKind::Chat {
                                            nickname: name.clone(),
                                            text: text.clone(),
                                        },
                                    });
                                    let _ = event_tx.send(SessionEvent::ChatReceived {
                                        nickname: name,
                                        text,
                                        message_id,
                                        timestamp_ms,
                                    }).await;
                                }
                            }
                            Ok(Message::FileOffer { nickname: name, endpoint_id, filename, size, hash, message_id, timestamp_ms, mime_type, target }) => {
                                if seen_ids.contains(&message_id) {
                                    continue;
                                }
                                // Skip targeted offers not meant for us.
                                if let Some(ref t) = target
                                    && *t != nickname
                                {
                                    continue;
                                }
                                let blob_hash = Hash::from_bytes(hash);
                                let offer = FileOffer {
                                    sender_nickname: name.clone(),
                                    sender_id: endpoint_id,
                                    filename: filename.clone(),
                                    size,
                                    hash: blob_hash,
                                };
                                transfers.add_offer(offer.clone());
                                seen_ids.insert(message_id);
                                push_history(&mut history, HistoryEntry {
                                    message_id,
                                    timestamp_ms,
                                    kind: HistoryEntryKind::FileOffer {
                                        nickname: name,
                                        endpoint_id,
                                        filename,
                                        size,
                                        hash,
                                        mime_type,
                                        target: target.clone(),
                                    },
                                });
                                let _ = event_tx.send(SessionEvent::FileOffered {
                                    offer,
                                    message_id,
                                    timestamp_ms,
                                    target,
                                }).await;
                            }
                            Ok(Message::FileRetract { nickname: name, hash, message_id, timestamp_ms }) => {
                                if seen_ids.contains(&message_id) {
                                    continue;
                                }
                                seen_ids.insert(message_id);
                                let blob_hash = Hash::from_bytes(hash);
                                let filename = transfers.retract(&blob_hash);
                                // Remove matching FileOffer entries from history.
                                history.retain(|e| {
                                    !matches!(&e.kind, HistoryEntryKind::FileOffer { hash: h, .. } if *h == hash)
                                });
                                push_history(&mut history, HistoryEntry {
                                    message_id,
                                    timestamp_ms,
                                    kind: HistoryEntryKind::FileRetract { hash },
                                });
                                let _ = event_tx.send(SessionEvent::FileRetracted {
                                    nickname: name,
                                    hash: blob_hash,
                                    filename,
                                }).await;
                            }
                            Ok(Message::HistoryOffer { message_count, hash, endpoint_id, .. }) => {
                                if !history_synced {
                                    history_synced = true;
                                    let _ = event_tx.send(SessionEvent::System {
                                        message: format!("syncing {message_count} messages from history..."),
                                    }).await;
                                    let blob_hash = Hash::from_bytes(hash);
                                    let store = blob_store.clone();
                                    let ep = endpoint.clone();
                                    let htx = history_tx.clone();
                                    tokio::spawn(async move {
                                        let conn = match ep.connect(endpoint_id, BLOBS_ALPN).await {
                                            Ok(c) => c,
                                            Err(e) => {
                                                let _ = htx.send(Err(format!("connect: {e}"))).await;
                                                return;
                                            }
                                        };
                                        let content = HashAndFormat::raw(blob_hash);
                                        match store.remote().fetch(conn, content).await {
                                            Ok(_) => {
                                                match store.blobs().get_bytes(blob_hash).await {
                                                    Ok(data) => {
                                                        let _ = htx.send(Ok(data.to_vec())).await;
                                                    }
                                                    Err(e) => {
                                                        let _ = htx.send(Err(format!("read blob: {e}"))).await;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let _ = htx.send(Err(format!("fetch: {e}"))).await;
                                            }
                                        }
                                    });
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(Some(GossipEvent::NeighborUp(id))) => {
                        peers.insert(id, PeerInfo {
                            name: id.fmt_short().to_string(),
                            conn_type: ConnType::Unknown,
                        });
                        let _ = event_tx.send(SessionEvent::System {
                            message: format!("peer connected: {}", id.fmt_short()),
                        }).await;
                        let join = Message::Join {
                            nickname: nickname.clone(),
                            endpoint_id: our_id,
                        };
                        if let Ok(encoded) = postcard::to_stdvec(&join) {
                            let _ = sender.broadcast(encoded.into()).await;
                        }

                        // Offer our history to the new peer if we have any.
                        if !history.is_empty()
                            && let Ok(history_bytes) = postcard::to_stdvec(&history)
                            && let Ok(tag_info) = blob_store.blobs().add_bytes(history_bytes).await
                        {
                            let history_hash = *tag_info.hash.as_bytes();
                            let oldest = history.first().map(|e| e.timestamp_ms).unwrap_or(0);
                            let newest = history.last().map(|e| e.timestamp_ms).unwrap_or(0);
                            let offer = Message::HistoryOffer {
                                message_count: history.len() as u32,
                                oldest_timestamp_ms: oldest,
                                newest_timestamp_ms: newest,
                                hash: history_hash,
                                endpoint_id: our_id,
                            };
                            if let Ok(encoded) = postcard::to_stdvec(&offer) {
                                let _ = sender.broadcast(encoded.into()).await;
                            }
                        }

                        // Re-offer shared files to new peer.
                        for entry in &transfers.entries {
                            if matches!(entry.state, crate::transfer::TransferState::Sharing) {
                                let mid = new_message_id();
                                let ts = now_ms();
                                let offer_msg = Message::FileOffer {
                                    nickname: nickname.clone(),
                                    endpoint_id: our_id,
                                    filename: entry.offer.filename.clone(),
                                    size: entry.offer.size,
                                    hash: *entry.offer.hash.as_bytes(),
                                    message_id: mid,
                                    timestamp_ms: ts,
                                    mime_type: mime_from_extension(&entry.offer.filename),
                                    target: None,
                                };
                                if let Ok(encoded) = postcard::to_stdvec(&offer_msg) {
                                    let _ = sender.broadcast(encoded.into()).await;
                                }
                            }
                        }
                    }
                    Ok(Some(GossipEvent::NeighborDown(id))) => {
                        let name = peers.remove(&id)
                            .map(|p| p.name)
                            .unwrap_or_else(|| id.fmt_short().to_string());
                        let _ = event_tx.send(SessionEvent::PeerLeft {
                            endpoint_id: id,
                            nickname: name,
                        }).await;
                    }
                    Ok(Some(GossipEvent::Lagged)) => {
                        let _ = event_tx.send(SessionEvent::System {
                            message: "warning: gossip stream lagged".to_string(),
                        }).await;
                    }
                    Ok(None) => {
                        let _ = event_tx.send(SessionEvent::Disconnected {
                            reason: "gossip stream closed".to_string(),
                        }).await;
                        break;
                    }
                    Err(e) => {
                        let _ = event_tx.send(SessionEvent::System {
                            message: format!("gossip error: {e}"),
                        }).await;
                    }
                }
            }

            // ── Transfer events from background tasks ───────────────────
            Some(event) = transfer_rx.recv() => {
                match event {
                    TransferEvent::Progress { hash, bytes_received, total_bytes } => {
                        transfers.update_progress(&hash, bytes_received, total_bytes);
                        let _ = event_tx.send(SessionEvent::TransferProgress {
                            hash,
                            bytes_received,
                            total_bytes,
                        }).await;
                    }
                    TransferEvent::Complete { hash, filename, path } => {
                        transfers.complete_download(&hash, path.clone());
                        let _ = event_tx.send(SessionEvent::TransferComplete {
                            hash,
                            filename,
                            path,
                        }).await;
                    }
                    TransferEvent::Failed { hash, filename, error } => {
                        transfers.fail_download(&hash, error.clone());
                        let _ = event_tx.send(SessionEvent::TransferFailed {
                            hash,
                            filename,
                            error,
                        }).await;
                    }
                }
            }

            // ── History sync from background fetch ──────────────────────
            Some(result) = history_rx.recv() => {
                match result {
                    Ok(data) => {
                        match postcard::from_bytes::<Vec<HistoryEntry>>(&data) {
                            Ok(mut entries) => {
                                entries.sort_by_key(|e| e.timestamp_ms);
                                let mut merged = 0u32;
                                let mut synced_entries = Vec::new();
                                for entry in entries {
                                    if seen_ids.contains(&entry.message_id) {
                                        continue;
                                    }
                                    seen_ids.insert(entry.message_id);
                                    merged += 1;
                                    // Process file offers/retracts in the transfer manager
                                    match &entry.kind {
                                        HistoryEntryKind::FileOffer {
                                            nickname: nick,
                                            endpoint_id: eid,
                                            filename,
                                            size,
                                            hash,
                                            target,
                                            ..
                                        } => {
                                            // Skip targeted offers not meant for us.
                                            if let Some(t) = target
                                                && *t != nickname
                                            {
                                                continue;
                                            }
                                            let blob_hash = Hash::from_bytes(*hash);
                                            let offer = FileOffer {
                                                sender_nickname: nick.clone(),
                                                sender_id: *eid,
                                                filename: filename.clone(),
                                                size: *size,
                                                hash: blob_hash,
                                            };
                                            transfers.add_offer(offer);
                                        }
                                        HistoryEntryKind::FileRetract { hash } => {
                                            let blob_hash = Hash::from_bytes(*hash);
                                            transfers.retract(&blob_hash);
                                        }
                                        _ => {}
                                    }
                                    synced_entries.push(entry.clone());
                                    history.push(entry);
                                }
                                // Cap history at 1000.
                                if history.len() > 1000 {
                                    history.drain(0..history.len() - 1000);
                                }
                                let _ = event_tx.send(SessionEvent::HistorySynced {
                                    entries: synced_entries,
                                    merged_count: merged,
                                }).await;
                            }
                            Err(e) => {
                                let _ = event_tx.send(SessionEvent::System {
                                    message: format!("history sync failed: invalid data ({e})"),
                                }).await;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(SessionEvent::System {
                            message: format!("history sync failed: {e}"),
                        }).await;
                    }
                }
            }

            // ── Connection type polling tick ─────────────────────────────
            _ = tick.tick() => {
                let peer_ids: Vec<_> = peers.keys()
                    .filter(|id| **id != our_id)
                    .copied()
                    .collect();
                for id in peer_ids {
                    let conn_type = match endpoint.remote_info(id).await {
                        Some(info) => {
                            use iroh::endpoint::TransportAddrUsage;
                            let mut has_relay = false;
                            let mut has_direct = false;
                            for a in info.addrs().filter(|a| matches!(a.usage(), TransportAddrUsage::Active)) {
                                if a.addr().is_ip() {
                                    has_direct = true;
                                } else {
                                    has_relay = true;
                                }
                            }
                            if has_direct {
                                ConnType::Direct
                            } else if has_relay {
                                ConnType::Relay
                            } else {
                                ConnType::Unknown
                            }
                        }
                        None => ConnType::Unknown,
                    };
                    if let Some(peer) = peers.get_mut(&id) {
                        // Only emit event if the connection type actually changed
                        let changed = !matches!(
                            (&peer.conn_type, &conn_type),
                            (ConnType::Direct, ConnType::Direct)
                            | (ConnType::Relay, ConnType::Relay)
                            | (ConnType::Unknown, ConnType::Unknown)
                            | (ConnType::You, ConnType::You)
                        );
                        if changed {
                            peer.conn_type = conn_type;
                            let _ = event_tx.send(SessionEvent::ConnTypeChanged {
                                endpoint_id: id,
                                conn_type: match &peer.conn_type {
                                    ConnType::Direct => ConnType::Direct,
                                    ConnType::Relay => ConnType::Relay,
                                    ConnType::Unknown => ConnType::Unknown,
                                    ConnType::You => ConnType::You,
                                },
                            }).await;
                        }
                    }
                }
            }
        }
    }

    // Shutdown
    let _ = router.shutdown().await;
    endpoint.close().await;
}

/// Push a history entry, capping at 1000 entries.
fn push_history(history: &mut Vec<HistoryEntry>, entry: HistoryEntry) {
    history.push(entry);
    if history.len() > 1000 {
        history.remove(0);
    }
}

/// Import a file into the blob store and broadcast a `FileOffer` over gossip.
#[allow(clippy::too_many_arguments)]
async fn share_file(
    store: &FsStore,
    sender: &iroh_gossip::api::GossipSender,
    nickname: &str,
    endpoint_id: iroh::EndpointId,
    path: &std::path::Path,
    target: Option<String>,
    seen_ids: &mut HashSet<MessageId>,
    history: &mut Vec<HistoryEntry>,
) -> Result<(Hash, String, u64, MessageId, u64, Option<String>)> {
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());

    let size = tokio::fs::metadata(path).await?.len();
    let tag_info = store.blobs().add_path(path).await?;
    let hash = tag_info.hash;

    let mid = new_message_id();
    let ts = now_ms();
    let mime_type = mime_from_extension(&filename);

    let msg = Message::FileOffer {
        nickname: nickname.to_string(),
        endpoint_id,
        filename: filename.clone(),
        size,
        hash: *hash.as_bytes(),
        message_id: mid,
        timestamp_ms: ts,
        mime_type: mime_type.clone(),
        target: target.clone(),
    };
    let encoded = postcard::to_stdvec(&msg)?;
    sender.broadcast(encoded.into()).await?;

    seen_ids.insert(mid);
    push_history(history, HistoryEntry {
        message_id: mid,
        timestamp_ms: ts,
        kind: HistoryEntryKind::FileOffer {
            nickname: nickname.to_string(),
            endpoint_id,
            filename: filename.clone(),
            size,
            hash: *hash.as_bytes(),
            mime_type: mime_type.clone(),
            target,
        },
    });

    Ok((hash, filename, size, mid, ts, mime_type))
}

/// Spawn a background download task.
fn spawn_download(
    store: &FsStore,
    endpoint: &iroh::Endpoint,
    offer: FileOffer,
    download_dir: PathBuf,
    tx: mpsc::Sender<TransferEvent>,
) {
    let store = store.clone();
    let endpoint = endpoint.clone();

    tokio::spawn(async move {
        let hash = offer.hash;
        let filename = offer.filename.clone();
        let target = download_dir.join(&filename);

        let conn = match endpoint.connect(offer.sender_id, BLOBS_ALPN).await {
            Ok(conn) => conn,
            Err(e) => {
                let _ = tx
                    .send(TransferEvent::Failed {
                        hash,
                        filename,
                        error: format!("connect: {e}"),
                    })
                    .await;
                return;
            }
        };

        let content = HashAndFormat::raw(hash);
        let mut progress_stream = store.remote().fetch(conn, content).stream();

        while let Some(item) = progress_stream.next().await {
            match item {
                iroh_blobs::api::remote::GetProgressItem::Progress(bytes) => {
                    let _ = tx
                        .send(TransferEvent::Progress {
                            hash,
                            bytes_received: bytes,
                            total_bytes: offer.size,
                        })
                        .await;
                }
                iroh_blobs::api::remote::GetProgressItem::Done(_stats) => {
                    match store.blobs().get_bytes(hash).await {
                        Ok(data) => {
                            match tokio::fs::write(&target, &data).await {
                                Ok(_) => {
                                    let _ = tx
                                        .send(TransferEvent::Complete {
                                            hash,
                                            filename: filename.clone(),
                                            path: target.clone(),
                                        })
                                        .await;
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(TransferEvent::Failed {
                                            hash,
                                            filename: filename.clone(),
                                            error: format!("write file: {e}"),
                                        })
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(TransferEvent::Failed {
                                    hash,
                                    filename: filename.clone(),
                                    error: format!("read blob: {e}"),
                                })
                                .await;
                        }
                    }
                    return;
                }
                iroh_blobs::api::remote::GetProgressItem::Error(e) => {
                    let _ = tx
                        .send(TransferEvent::Failed {
                            hash,
                            filename: filename.clone(),
                            error: format!("download: {e}"),
                        })
                        .await;
                    return;
                }
            }
        }
    });
}
