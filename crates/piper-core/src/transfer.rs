//! File transfer state machine.
//!
//! Tracks all file offers (sent and received) and their lifecycle states.
//! The rendering of the file pane is handled by each frontend separately.

use iroh::EndpointId;
use iroh_blobs::Hash;
use std::path::PathBuf;

// ── Types ────────────────────────────────────────────────────────────────────

/// A file offer broadcast over gossip.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileOffer {
    pub sender_nickname: String,
    pub sender_id: EndpointId,
    pub filename: String,
    pub size: u64,
    pub hash: Hash,
}

/// The lifecycle state of a single file transfer.
#[derive(Debug, Clone, serde::Serialize)]
pub enum TransferState {
    /// Offer received but download not yet started.
    Pending,
    /// Download in progress.
    Downloading {
        bytes_received: u64,
        total_bytes: u64,
    },
    /// Download completed — the file is available at `path`.
    Complete(PathBuf),
    /// Download failed with an error message.
    Failed(String),
    /// We are the sender — the file is being shared to peers.
    Sharing,
}

/// Events sent from background download tasks back to the main event loop.
#[derive(Debug)]
pub enum TransferEvent {
    Progress {
        hash: Hash,
        bytes_received: u64,
        total_bytes: u64,
    },
    Complete {
        hash: Hash,
        filename: String,
        path: PathBuf,
    },
    Failed {
        hash: Hash,
        filename: String,
        error: String,
    },
}

/// A single entry in the file share pane — an offer paired with its state.
#[derive(Debug)]
pub struct TransferEntry {
    pub offer: FileOffer,
    pub state: TransferState,
}

// ── TransferManager ──────────────────────────────────────────────────────────

/// Manages the list of file transfers (both sent and received) and tracks
/// which entry is currently selected.
#[derive(Debug)]
pub struct TransferManager {
    pub entries: Vec<TransferEntry>,
    pub selected_index: usize,
}

impl Default for TransferManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected_index: 0,
        }
    }

    /// Add an incoming file offer from a remote peer.
    pub fn add_offer(&mut self, offer: FileOffer) {
        self.entries.push(TransferEntry {
            offer,
            state: TransferState::Pending,
        });
    }

    /// Add an entry for a file we are sharing (sender's view).
    pub fn add_sent(&mut self, offer: FileOffer) {
        self.entries.push(TransferEntry {
            offer,
            state: TransferState::Sharing,
        });
    }

    /// Mark a transfer as downloading.
    pub fn start_download(&mut self, hash: &Hash) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Downloading {
                bytes_received: 0,
                total_bytes: entry.offer.size,
            };
        }
    }

    /// Update download progress for a transfer identified by hash.
    pub fn update_progress(&mut self, hash: &Hash, bytes_received: u64, total_bytes: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Downloading {
                bytes_received,
                total_bytes,
            };
        }
    }

    /// Mark a transfer as complete with the path to the downloaded file.
    pub fn complete_download(&mut self, hash: &Hash, path: PathBuf) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Complete(path);
        }
    }

    /// Mark a transfer as failed with an error message.
    pub fn fail_download(&mut self, hash: &Hash, error: String) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.offer.hash == *hash) {
            entry.state = TransferState::Failed(error);
        }
    }

    /// Get a reference to the currently selected entry (if any).
    pub fn selected_entry(&self) -> Option<&TransferEntry> {
        self.entries.get(self.selected_index)
    }

    /// Whether there are any entries to display.
    pub fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Move selection to the next entry (wrapping around).
    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.entries.len();
        }
    }

    /// Remove a transfer entry by its BLAKE3 hash.
    /// Returns the filename if found and removed, or `None` if not found.
    pub fn retract(&mut self, hash: &Hash) -> Option<String> {
        if let Some(idx) = self.entries.iter().position(|e| e.offer.hash == *hash) {
            let filename = self.entries[idx].offer.filename.clone();
            self.entries.remove(idx);
            if self.selected_index >= self.entries.len() && self.selected_index > 0 {
                self.selected_index -= 1;
            }
            Some(filename)
        } else {
            None
        }
    }

    /// Move selection to the previous entry (wrapping around).
    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            self.selected_index = if self.selected_index == 0 {
                self.entries.len() - 1
            } else {
                self.selected_index - 1
            };
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hash() -> Hash {
        Hash::from_bytes([42u8; 32])
    }

    fn test_offer(nickname: &str) -> FileOffer {
        FileOffer {
            sender_nickname: nickname.to_string(),
            sender_id: EndpointId::from_bytes(&[1u8; 32]).unwrap(),
            filename: "test.txt".to_string(),
            size: 1024,
            hash: test_hash(),
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let m = TransferManager::new();
        assert!(!m.has_entries());
        assert_eq!(m.selected_index, 0);
        assert!(m.selected_entry().is_none());
    }

    #[test]
    fn add_offer_and_sent() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        assert!(m.has_entries());
        assert_eq!(m.entries.len(), 1);
        assert!(matches!(m.entries[0].state, TransferState::Pending));

        m.add_sent(test_offer("You"));
        assert_eq!(m.entries.len(), 2);
        assert!(matches!(m.entries[1].state, TransferState::Sharing));
    }

    #[test]
    fn start_download_transitions_state() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.start_download(&hash);
        assert!(matches!(
            m.entries[0].state,
            TransferState::Downloading {
                bytes_received: 0,
                total_bytes: 1024,
            }
        ));
    }

    #[test]
    fn update_progress() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.start_download(&hash);
        m.update_progress(&hash, 512, 1024);
        assert!(matches!(
            m.entries[0].state,
            TransferState::Downloading {
                bytes_received: 512,
                total_bytes: 1024,
            }
        ));
    }

    #[test]
    fn complete_download() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.complete_download(&hash, PathBuf::from("/tmp/test.txt"));
        match &m.entries[0].state {
            TransferState::Complete(p) => assert_eq!(p, &PathBuf::from("/tmp/test.txt")),
            _ => panic!("expected Complete state"),
        }
    }

    #[test]
    fn fail_download() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        m.fail_download(&hash, "network error".into());
        match &m.entries[0].state {
            TransferState::Failed(e) => assert_eq!(e, "network error"),
            _ => panic!("expected Failed state"),
        }
    }

    #[test]
    fn select_next_and_prev() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        let mut offer2 = test_offer("Bob");
        offer2.hash = Hash::from_bytes([99u8; 32]);
        m.add_offer(offer2);

        assert_eq!(m.selected_index, 0);
        m.select_next();
        assert_eq!(m.selected_index, 1);
        m.select_next();
        assert_eq!(m.selected_index, 0); // wraps
        m.select_prev();
        assert_eq!(m.selected_index, 1); // wraps backward
        m.select_prev();
        assert_eq!(m.selected_index, 0);
    }

    #[test]
    fn select_on_empty_is_noop() {
        let mut m = TransferManager::new();
        m.select_next();
        assert_eq!(m.selected_index, 0);
        m.select_prev();
        assert_eq!(m.selected_index, 0);
    }

    #[test]
    fn retract_removes_entry() {
        let mut m = TransferManager::new();
        let hash = test_hash();
        m.add_offer(test_offer("Alice"));
        assert_eq!(m.entries.len(), 1);
        let removed = m.retract(&hash);
        assert_eq!(removed, Some("test.txt".into()));
        assert!(m.entries.is_empty());
    }

    #[test]
    fn retract_returns_none_for_missing() {
        let mut m = TransferManager::new();
        let hash = Hash::from_bytes([99u8; 32]);
        assert_eq!(m.retract(&hash), None);
    }

    #[test]
    fn retract_adjusts_selected_index() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        let mut offer2 = test_offer("Bob");
        offer2.hash = Hash::from_bytes([99u8; 32]);
        m.add_offer(offer2);
        m.selected_index = 1;
        m.retract(&Hash::from_bytes([99u8; 32]));
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.selected_index, 0);
    }

    #[test]
    fn retract_keeps_index_when_earlier_removed() {
        let mut m = TransferManager::new();
        m.add_offer(test_offer("Alice"));
        let mut offer2 = test_offer("Bob");
        offer2.hash = Hash::from_bytes([99u8; 32]);
        m.add_offer(offer2);
        let mut offer3 = test_offer("Charlie");
        offer3.hash = Hash::from_bytes([88u8; 32]);
        m.add_offer(offer3);
        m.selected_index = 2;
        m.retract(&test_hash());
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.selected_index, 1);
    }
}
