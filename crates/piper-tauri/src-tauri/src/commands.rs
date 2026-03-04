//! Tauri commands wrapping the piper-core Session API.

use std::path::PathBuf;
use std::sync::Arc;

use iroh_blobs::Hash;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{Mutex, mpsc};

use piper_core::protocol::ChatTicket;
use piper_core::session::{SessionCommand, SessionConfig, SessionEvent, start_session};

/// Shared session state managed by Tauri.
#[derive(Default)]
pub struct SessionState {
    pub cmd_tx: Option<mpsc::Sender<SessionCommand>>,
}

/// Start a new session (create a room).
#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    state: State<'_, Arc<Mutex<SessionState>>>,
    nickname: String,
) -> Result<String, String> {
    start_session_inner(app, state, nickname, ChatTicket::new_random()).await
}

/// Join an existing session with a ticket string.
#[tauri::command]
pub async fn join_session(
    app: AppHandle,
    state: State<'_, Arc<Mutex<SessionState>>>,
    nickname: String,
    ticket_str: String,
) -> Result<String, String> {
    let ticket = <ChatTicket as iroh_tickets::Ticket>::deserialize(&ticket_str)
        .map_err(|e| format!("invalid ticket: {e}"))?;
    start_session_inner(app, state, nickname, ticket).await
}

/// Send a chat message.
#[tauri::command]
pub async fn send_chat(
    state: State<'_, Arc<Mutex<SessionState>>>,
    text: String,
) -> Result<(), String> {
    let guard = state.lock().await;
    if let Some(tx) = &guard.cmd_tx {
        tx.send(SessionCommand::SendChat { text })
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("no active session".into())
    }
}

/// Share a file with the room.
#[tauri::command]
pub async fn share_file(
    state: State<'_, Arc<Mutex<SessionState>>>,
    path: String,
    target: Option<String>,
) -> Result<(), String> {
    let guard = state.lock().await;
    if let Some(tx) = &guard.cmd_tx {
        tx.send(SessionCommand::ShareFile {
            path: PathBuf::from(path),
            target,
        })
        .await
        .map_err(|e| e.to_string())
    } else {
        Err("no active session".into())
    }
}

/// Start downloading a file by its hash.
#[tauri::command]
pub async fn start_download(
    state: State<'_, Arc<Mutex<SessionState>>>,
    hash: String,
) -> Result<(), String> {
    let hash: Hash = hash.parse().map_err(|e| format!("invalid hash: {e}"))?;
    let guard = state.lock().await;
    if let Some(tx) = &guard.cmd_tx {
        tx.send(SessionCommand::StartDownload { hash })
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("no active session".into())
    }
}

/// Unshare a file by its hash.
#[tauri::command]
pub async fn unshare_file(
    state: State<'_, Arc<Mutex<SessionState>>>,
    hash: String,
) -> Result<(), String> {
    let hash: Hash = hash.parse().map_err(|e| format!("invalid hash: {e}"))?;
    let guard = state.lock().await;
    if let Some(tx) = &guard.cmd_tx {
        tx.send(SessionCommand::UnshareFile { hash })
            .await
            .map_err(|e| e.to_string())
    } else {
        Err("no active session".into())
    }
}

/// Quit the current session.
#[tauri::command]
pub async fn quit_session(state: State<'_, Arc<Mutex<SessionState>>>) -> Result<(), String> {
    let guard = state.lock().await;
    if let Some(tx) = &guard.cmd_tx {
        let _ = tx.send(SessionCommand::Quit).await;
    }
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn start_session_inner(
    app: AppHandle,
    state: State<'_, Arc<Mutex<SessionState>>>,
    nickname: String,
    ticket: ChatTicket,
) -> Result<String, String> {
    let handle = start_session(SessionConfig { nickname, ticket })
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut guard = state.lock().await;
        guard.cmd_tx = Some(handle.cmd_tx.clone());
    }

    let mut event_rx = handle.event_rx;
    // Forward session events to the webview as properly serialized JSON.
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let event_name = match &event {
                SessionEvent::Ready { .. } => "session:ready",
                SessionEvent::PeerJoined { .. } => "session:peer-joined",
                SessionEvent::PeerLeft { .. } => "session:peer-left",
                SessionEvent::ChatReceived { .. } => "session:chat-received",
                SessionEvent::ChatSent { .. } => "session:chat-sent",
                SessionEvent::FileOffered { .. } => "session:file-offered",
                SessionEvent::FileRetracted { .. } => "session:file-retracted",
                SessionEvent::FileShared { .. } => "session:file-shared",
                SessionEvent::FileShareFailed { .. } => "session:file-share-failed",
                SessionEvent::TransferProgress { .. } => "session:transfer-progress",
                SessionEvent::TransferComplete { .. } => "session:transfer-complete",
                SessionEvent::TransferFailed { .. } => "session:transfer-failed",
                SessionEvent::ConnTypeChanged { .. } => "session:conn-type-changed",
                SessionEvent::HistorySynced { .. } => "session:history-synced",
                SessionEvent::System { .. } => "session:system",
                SessionEvent::Disconnected { .. } => "session:disconnected",
            };
            let _ = app.emit(event_name, &event);
        }
    });

    Ok("session started".into())
}
